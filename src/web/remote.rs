//! 마참뮤직 사용자 포털 HTTP/API/WebSocket 진입점.
//! Discord OAuth 세션과 길드 권한(`AccessTier`)을 검증한 뒤 기존 PlayerManager/Coordinator만 호출한다.
//!
//! v2 계약: `docs/REMOTE-API-V2.md`. 상태는 hot/cold로 갈라지고, 변경은 타입드 WS 이벤트
//! (`{"t":토픽,"d":데이터}`)로 밀어 준다. 프런트(`assets/portal.js`, `assets/console.js`)는
//! 이 계약대로 이미 작성돼 있으므로 서버가 프런트에 맞춘다.

use super::{WebState, remote_page};
use crate::app::App;
use crate::models::{
    CsTimeSpan, PlaylistEntry, PlaylistScope, ProviderKind, QueueItem, RepeatMode, TrackRef,
};
use crate::remote::ranking;
use crate::remote::store::is_valid_pref;
use crate::remote::{
    AuditKind, AutoplaySeed, ChatTrackTag, GlobalOverrides, LyricsCacheHit, LyricsDocument,
    LyricsLine, MAX_AUTOPLAY_SEEDS,
    MAX_VOTER_IDS, PERMISSION_KEYS, PermissionRule, QueueScore, QueueSortMode,
    QueueVoteKind, RemoteGuildSettings, SeedAddOutcome, StoredSession, SuggestionStatus,
    SuperLikeStatus, Suspension, SuspensionScope, UserTrackKind, VotePoints, VoteSkipBasis,
    as_limit, as_limit_u32,
};
use crate::remote::{
    AutoplayMode, AutoplayPolicy, BoomttaAction, ChartCategory, ChartDef, ChartSnapshot,
    INTERNAL_CHART_PREFIX, VOTE_POINT_MAX, VOTE_POINT_MIN,
};
use std::collections::BTreeMap;
use axum::Json;
use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serenity::all::{GuildId, Permissions, UserId};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower_cookies::{Cookie, Cookies};

const REMOTE_COOKIE: &str = "macham_session";
/// 로그인 유지(결정 #17) — 봇 재시작·업데이트 후에도 살아 있어야 하므로 30일이다.
/// Discord 액세스 토큰 만료는 세션 만료와 별개로 refresh token으로 갱신한다.
const REMOTE_SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// 액세스 토큰이 이만큼 남았으면 미리 갱신한다.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(15 * 60);
const OAUTH_STATE_TTL: Duration = Duration::from_secs(10 * 60);
const MEMBER_CACHE_TTL: Duration = Duration::from_secs(60);
/// Discord가 일시적으로 실패해도 이 시간 안의 캐시가 있으면 등급을 유지한다(429로 강등 금지).
const MEMBER_CACHE_GRACE: Duration = Duration::from_secs(6 * 60 * 60);
const ADMINISTRATOR_PERMISSION: u64 = 1 << 3;
const MANAGE_GUILD_PERMISSION: u64 = 1 << 5;
const REMOTE_AUTH_FILE: &str = "remote-oauth.json";
/// 접속(presence) 이벤트 코얼레싱 주기 — 사양서 §5.2 E "최대 초당 1회".
const PRESENCE_COALESCE: Duration = Duration::from_secs(1);
/// 길드당 하나만 도는 재생 감시 주기. 탭 수와 무관하므로 탭이 10개여도 비용이 늘지 않는다.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// 프로세스 기동 시각 — `/admin/diagnostics`의 uptime.
static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn mark_started() {
    let _ = STARTED.set(Instant::now());
}

fn uptime_seconds() -> u64 {
    STARTED.get().map(|start| start.elapsed().as_secs()).unwrap_or(0)
}

// ───────────────────────── OAuth 설정 ─────────────────────────

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredRemoteAuthConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
    public_base_url: Option<String>,
    /// 봇 주인 Discord 유저 ID. JS 정밀도 문제를 피하려고 문자열로 저장한다.
    owner_user_ids: Vec<String>,
    /// YouTube Data API v3 키. 브라우저 검색(V3 §6)에 쓰이므로 결국 클라이언트로 나간다.
    youtube_api_key: Option<String>,
}

#[derive(Clone)]
pub struct RemoteAuthConfig {
    pub client_id: Option<String>,
    client_secret: Option<String>,
    pub public_base_url: String,
    pub dev_login: bool,
    /// 봇 주인 Discord 유저 ID 목록. 여기 있으면 유저 UI에서 `Owner` 등급이 된다.
    pub owner_user_ids: Vec<u64>,
    /// YouTube Data API v3 키. 있으면 `/state/cold`가 브라우저 검색 모드를 내려보낸다.
    /// **브라우저에 그대로 노출되는 값**이라 Google Cloud에서 HTTP 리퍼러 제한이 전제다.
    youtube_api_key: Option<String>,
}

impl std::fmt::Debug for RemoteAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret_configured", &self.client_secret.is_some())
            .field("public_base_url", &self.public_base_url)
            .field("dev_login", &self.dev_login)
            .field("owner_user_ids", &self.owner_user_ids)
            .field("youtube_api_key_configured", &self.youtube_api_key.is_some())
            .finish()
    }
}

impl RemoteAuthConfig {
    /// 운영자 UI 저장값이 있으면 환경변수보다 우선한다. 환경변수는 기존 배포 호환용이다.
    pub fn load(data_root: &FsPath) -> Self {
        let stored: Option<StoredRemoteAuthConfig> =
            std::fs::read_to_string(Self::storage_path(data_root))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok());
        let clean = |value: Option<String>| {
            value
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let env = |name: &str| clean(std::env::var(name).ok());
        let (client_id, client_secret, public_base_url, owner_user_ids, youtube_api_key) =
            match stored {
                Some(stored) => (
                    clean(stored.client_id),
                    clean(stored.client_secret),
                    clean(stored.public_base_url).unwrap_or_else(|| "http://localhost:8693".into()),
                    parse_owner_ids(&stored.owner_user_ids.join(",")),
                    clean(stored.youtube_api_key),
                ),
                None => (
                    env("MUSICBOT_DISCORD_CLIENT_ID"),
                    env("MUSICBOT_DISCORD_CLIENT_SECRET"),
                    env("MUSICBOT_PUBLIC_BASE_URL")
                        .unwrap_or_else(|| "http://localhost:8693".into()),
                    parse_owner_ids(env("MUSICBOT_OWNER_USER_IDS").unwrap_or_default().as_str()),
                    env("MUSICBOT_YOUTUBE_API_KEY"),
                ),
            };
        Self {
            client_id,
            client_secret,
            public_base_url: public_base_url.trim_end_matches('/').to_string(),
            dev_login: std::env::var("MUSICBOT_DEV_LOGIN").ok().as_deref() == Some("1"),
            owner_user_ids,
            youtube_api_key,
        }
    }

    pub fn storage_path(data_root: &FsPath) -> PathBuf {
        data_root.join(REMOTE_AUTH_FILE)
    }

    /// 새 Secret이 비어 있으면 기존 Secret을 유지하고, clear_secret일 때만 제거한다.
    pub fn updated(
        &self,
        client_id: String,
        client_secret_update: Option<String>,
        clear_secret: bool,
        public_base_url: String,
    ) -> Self {
        let client_secret = if clear_secret {
            None
        } else {
            client_secret_update.or_else(|| self.client_secret.clone())
        };
        Self {
            client_id: Some(client_id),
            client_secret,
            public_base_url: public_base_url.trim_end_matches('/').to_string(),
            dev_login: self.dev_login,
            owner_user_ids: self.owner_user_ids.clone(),
            youtube_api_key: self.youtube_api_key.clone(),
        }
    }

    /// 봇 주인 ID만 교체한 사본. 운영 패널 `/botsettings`가 쓴다.
    pub fn with_owner_user_ids(&self, owner_user_ids: Vec<u64>) -> Self {
        let mut next = self.clone();
        next.owner_user_ids = owner_user_ids;
        next
    }

    /// YouTube API 키만 교체한 사본. Client Secret과 같은 규칙이에요 —
    /// 빈 값이면 기존 키를 그대로 두고, `clear`일 때만 지워요.
    pub fn with_youtube_api_key(&self, update: Option<String>, clear: bool) -> Self {
        let mut next = self.clone();
        next.youtube_api_key = if clear {
            None
        } else {
            update
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .or_else(|| self.youtube_api_key.clone())
        };
        next
    }

    /// 지금 저장된 YouTube API 키. 브라우저로 내려보낼 값이라 `/state/cold`만 읽어요.
    pub fn youtube_api_key(&self) -> Option<&str> {
        self.youtube_api_key.as_deref()
    }

    /// 운영 패널에 보여줄 마스킹 값 — 앞 4자와 뒤 4자만 남겨요.
    /// 키 자체는 어차피 브라우저로 나가지만, 어깨너머로 통째로 읽히게 두진 않아요.
    pub fn masked_youtube_api_key(&self) -> Option<String> {
        let key = self.youtube_api_key.as_deref()?;
        let chars: Vec<char> = key.chars().collect();
        if chars.len() <= 8 {
            return Some("•".repeat(chars.len().max(4)));
        }
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        Some(format!("{head}{}{tail}", "•".repeat(chars.len() - 8)))
    }

    pub fn save(&self, data_root: &FsPath) -> Result<(), String> {
        std::fs::create_dir_all(data_root)
            .map_err(|error| format!("OAuth 설정 폴더 생성 실패: {error}"))?;
        let stored = StoredRemoteAuthConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            public_base_url: Some(self.public_base_url.clone()),
            owner_user_ids: self
                .owner_user_ids
                .iter()
                .map(|id| id.to_string())
                .collect(),
            youtube_api_key: self.youtube_api_key.clone(),
        };
        let payload = serde_json::to_vec_pretty(&stored)
            .map_err(|error| format!("OAuth 설정 직렬화 실패: {error}"))?;
        let path = Self::storage_path(data_root);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("OAuth 설정 파일 열기 실패: {error}"))?;
        use std::io::Write;
        file.write_all(&payload)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("OAuth 설정 저장 실패: {error}"))
    }

    pub fn configured(&self) -> bool {
        self.client_id.is_some() && self.client_secret.is_some()
    }

    pub fn has_client_secret(&self) -> bool {
        self.client_secret.is_some()
    }

    pub fn redirect_uri(&self) -> String {
        format!("{}/music/oauth/callback", self.public_base_url)
    }
}

/// "123, 456" 같은 입력을 정규화한다. 0과 중복은 버린다.
pub fn parse_owner_ids(raw: &str) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for token in raw.split([',', '\n', ' ', ';']) {
        let Ok(id) = token.trim().parse::<u64>() else {
            continue;
        };
        if id != 0 && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

// ───────────────────────── 세션과 등급 ─────────────────────────

#[derive(Debug, Clone)]
pub struct RemoteSession {
    pub user_id: u64,
    /// Discord 계정명. 지금은 표시에 쓰지 않지만 감사/디버깅용으로 들고 있는다.
    #[allow(dead_code)]
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub guilds: Vec<OAuthGuild>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub csrf_token: String,
    pub created: Instant,
    /// Discord 액세스 토큰 만료. 세션 자체의 수명(`REMOTE_SESSION_TTL`)과는 별개다.
    pub token_expires: Instant,
    /// `MUSICBOT_DEV_LOGIN=1` 로컬 검증 세션인지. DB에 저장하지 않는다.
    pub is_developer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthGuild {
    pub id: u64,
    pub name: String,
    pub icon: Option<String>,
    pub owner: bool,
    pub permissions: u64,
}

impl OAuthGuild {
    pub fn icon_url(&self) -> Option<String> {
        self.icon.as_ref().map(|icon| {
            format!(
                "https://cdn.discordapp.com/icons/{}/{}.png?size=128",
                self.id, icon
            )
        })
    }

    fn is_admin(&self) -> bool {
        self.owner
            || self.permissions & ADMINISTRATOR_PERMISSION != 0
            || self.permissions & MANAGE_GUILD_PERMISSION != 0
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "name": self.name,
            "iconUrl": self.icon_url(),
        })
    }
}

/// 접근 등급 — 사양서 §1.1. 순서가 곧 권한 크기라서 `Ord`로 비교한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessTier {
    /// 읽기전용. 모든 쓰기 라우트에서 서버가 거부한다.
    Viewer,
    Member,
    Manager,
    Owner,
}

impl AccessTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Manager => "manager",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    /// 서버 관리 콘솔·정렬 모드·유저 정지 등 "관리자 이상" 판정.
    pub fn is_manager(self) -> bool {
        self >= Self::Manager
    }

    pub fn is_owner(self) -> bool {
        self == Self::Owner
    }

    /// 쓰기 동작을 아예 못 하는 등급인지.
    pub fn is_viewer(self) -> bool {
        self == Self::Viewer
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemberContext {
    /// 관리자 우회 대상인지 — `AccessTier >= Manager`와 같은 값을 넣는다.
    pub is_admin: bool,
    pub same_voice_channel: bool,
    /// 봇이 지금 이 서버의 음성 채널에 들어가 있는지.
    ///
    /// `same_voice_channel` 만으로는 "봇이 다른 채널에 있다"와 "봇이 아예 없다"를 못 가른다.
    /// 둘은 전혀 다른 상황이다 — 앞은 남의 재생을 흔드는 것이고, 뒤는 흔들 재생 자체가 없다.
    pub bot_in_voice: bool,
    pub role_ids: Vec<u64>,
}

/// 한 요청의 인증·인가 결과 묶음.
pub struct AuthContext {
    pub session: RemoteSession,
    pub guild: OAuthGuild,
    pub settings: RemoteGuildSettings,
    pub member: MemberContext,
    pub tier: AccessTier,
    pub suspensions: Vec<Suspension>,
    /// `Viewer`로 강등된 이유(있으면 화면 상단 배너에 뜬다).
    pub viewer_reason: Option<String>,
    /// 이 요청에서 역할 목록을 **실제로 확인했는지**. Discord 조회가 실패하고
    /// 되살릴 캐시도 없으면 false. 이때 `member.role_ids` 가 빈 것은
    /// "역할이 없다" 가 아니라 "모른다" 는 뜻이라, 권한 거절 문구를 달리 해야 한다.
    pub roles_known: bool,
}

impl AuthContext {
    fn guild_id(&self) -> u64 {
        self.guild.id
    }

    fn user_id(&self) -> u64 {
        self.session.user_id
    }

    /// 규칙 하나에 대한 최종 허용 여부. `Viewer`는 언제나 false다.
    /// `key`는 권한 키(`search`·`volume`…)로, 지정 역할을 **그 키의 역할 목록**에서 찾는다.
    fn allows(&self, key: &str, rule: PermissionRule) -> bool {
        !self.tier.is_viewer() && permission_allowed(key, rule, &self.settings, &self.member)
    }

    fn require(&self, key: &str, rule: PermissionRule, message: &str) -> Result<(), Response> {
        if self.allows(key, rule) {
            return Ok(());
        }
        if self.tier.is_viewer() {
            return Err(json_error(
                StatusCode::FORBIDDEN,
                self.viewer_reason
                    .clone()
                    .unwrap_or_else(|| "읽기 전용이라 아무것도 조작할 수 없어요.".into()),
            ));
        }
        // **역할을 모르는 상태를 권한 없음으로 말하면 거짓말이 된다.** 실제로 겪은 일이다 —
        // 재시작 직후 Discord 429 가 겹치면 지정 역할 권한자가 "권한이 없어요" 를 봤다.
        // 이건 거절이 아니라 판정 실패이므로 503 으로, 다시 해 보라고 말한다.
        if !self.roles_known && rule.needs_roles() {
            return Err(json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Discord에서 내 역할을 확인하지 못했어요. 권한이 없는 게 아니라 조회가 잠시 밀린 거예요. 몇 초 뒤에 다시 해 주세요.",
            ));
        }
        Err(json_error(
            StatusCode::FORBIDDEN,
            format!("{message} {}", self.who_has(key, rule)),
        ))
    }

    /// 이 규칙이면 **누가** 할 수 있고 어떻게 하면 풀리는지 한 문장으로.
    /// 막힌 사실만 말하고 끝내면 다음에 뭘 해야 할지 알 수가 없다 (§23.3).
    fn who_has(&self, key: &str, rule: PermissionRule) -> String {
        match rule {
            // 멤버면 누구나인데 막혔다면 정지·읽기전용 같은 다른 사정이다.
            PermissionRule::GuildMember => {
                "원래는 서버 멤버면 누구나 할 수 있어요. 정지 중이거나 읽기 전용인지 확인해 주세요.".into()
            }
            PermissionRule::SameVoiceChannel => {
                "봇과 같은 음성 채널에 있어야 해요. 봇이 있는 채널로 들어오면 바로 열려요.".into()
            }
            PermissionRule::ConfiguredRole => {
                let count = self.settings.roles_for(key).len();
                if count == 0 {
                    "지정 역할만 쓸 수 있는데 아직 역할이 지정되지 않았어요. 지금은 서버 관리자만 할 수 있어요.".into()
                } else {
                    format!("지정된 역할({count}개) 중 하나를 가진 사람과 서버 관리자만 할 수 있어요. 관리자에게 역할을 요청해 보세요.")
                }
            }
            PermissionRule::Administrator => {
                "서버 관리자만 할 수 있어요. 관리자에게 부탁하거나 관리자 지정 역할을 받아야 해요.".into()
            }
            PermissionRule::Disabled => {
                "이 기능은 서버 설정에서 꺼져 있어요. 관리자도 못 쓰고, 설정에서 켜야 열려요.".into()
            }
        }
    }

    fn require_manager(&self) -> Result<(), Response> {
        if self.tier.is_manager() {
            Ok(())
        } else {
            Err(json_error(
                StatusCode::FORBIDDEN,
                "서버 관리자만 할 수 있어요.",
            ))
        }
    }

    /// 기능별 정지 검사. `All` 정지는 이미 `Viewer` 강등으로 처리돼 있다.
    fn require_not_suspended(&self, scope: SuspensionScope) -> Result<(), Response> {
        let hit = self
            .suspensions
            .iter()
            .find(|item| item.scope == scope || item.scope == SuspensionScope::All);
        match hit {
            Some(item) => Err(json_error(
                StatusCode::FORBIDDEN,
                format!(
                    "{} 정지 중이에요.{}",
                    item.scope.label(),
                    item.reason
                        .as_deref()
                        .map(|reason| format!(" 사유: {reason}"))
                        .unwrap_or_default()
                ),
            )),
            None => Ok(()),
        }
    }

    fn suspension_json(&self) -> Value {
        match self.suspensions.first() {
            Some(item) => json!({
                "scope": item.scope.as_str(),
                "reason": item.reason,
                "expiresUtc": item.expires_utc,
                "byUserId": item.by_user_id.to_string(),
                "byDisplayName": null,
            }),
            None => Value::Null,
        }
    }
}

// ───────────────────────── 이벤트 ─────────────────────────

/// WS로 나가는 타입드 이벤트. 와이어 포맷은 `{"t":토픽,"d":데이터}`이고
/// `guild_id`는 서버 측 필터링에만 쓰인다(팬아웃 방지).
#[derive(Debug, Clone)]
pub struct RemoteEvent {
    pub guild_id: u64,
    pub topic: String,
    pub data: Value,
    /// **수신자 필터**. `Some(user_id)` 면 그 사람의 소켓에만 나간다.
    ///
    /// 개인화된 값(`mine` 같은 것)을 길드 전체로 뿌리면 남의 화면이 내 표를
    /// 자기 표로 착각한다(V3 §10.5). 그런 payload 는 반드시 이 필터를 쓴다.
    pub only_user: Option<u64>,
}

impl RemoteEvent {
    fn wire(&self) -> String {
        serde_json::to_string(&json!({ "t": self.topic, "d": self.data }))
            .unwrap_or_else(|_| "{\"t\":\"notice\",\"d\":{}}".into())
    }

    /// 이 이벤트를 `user_id` 소켓에 보내도 되는가.
    fn targets(&self, guild_id: u64, user_id: u64) -> bool {
        self.guild_id == guild_id && self.only_user.is_none_or(|only| only == user_id)
    }
}

/// 타입드 이벤트 하나를 그 길드 구독자에게만 보낸다.
fn emit(state: &WebState, guild_id: u64, topic: &str, data: Value) {
    let _ = state.remote_events.send(RemoteEvent {
        guild_id,
        topic: topic.into(),
        data,
        only_user: None,
    });
}

/// 재시작이 시작됐다고 모든 창에 알린다 (§24).
///
/// **길드마다 따로 쏜다.** 이벤트는 `guild_id` 로 걸러져 나가므로 전체 방송용 통로가
/// 따로 없다. 여기서 길드를 도는 편이 필터에 특수한 예외를 뚫는 것보다 안전하다.
pub fn broadcast_restarting(state: &Arc<WebState>) {
    for guild_id in state.app.db.list_known_guild_ids() {
        emit(
            state,
            guild_id,
            "server.restarting",
            json!({
                // 화면이 "곧 돌아와요" 를 띄우고 재연결을 기다리게 하는 신호다.
                "message": "업데이트 중이에요. 몇 초 뒤에 자동으로 다시 연결돼요.",
                "resumes": true,
            }),
        );
    }
}

/// 한 사람에게만 보낸다. 개인화된 payload(내 표·내 보관함)는 전체로 뿌리면 안 된다.
fn emit_to(state: &WebState, guild_id: u64, user_id: u64, topic: &str, data: Value) {
    let _ = state.remote_events.send(RemoteEvent {
        guild_id,
        topic: topic.into(),
        data,
        only_user: Some(user_id),
    });
}

// ───────────────────────── 통계 이벤트 (V3 §22.2) ─────────────────────────
//
// 통계 모듈은 `mpsc` 뒤에 있어서 던지고 잊으면 된다 — 재생 경로를 절대 막지 않는다.
// 통계가 꺼져 있으면 `state.app.stats` 가 `None` 이고 아래 함수들이 조용히 아무 일도 안 한다.

/// 통계 이벤트 하나를 던진다. 기다리지 않는다.
fn record_stat(state: &WebState, event: crate::stats::StatEvent) {
    if let Some(stats) = state.app.stats.as_ref() {
        stats.record(event);
    }
}

/// payload 없이 "재조회해라"만 알리는 토픽 (`settings`/`library`/`audit` 등).
fn emit_bare(state: &WebState, guild_id: u64, topic: &str) {
    emit(state, guild_id, topic, json!({}));
}

/// 개인 데이터만 바뀌었을 때의 "재조회해라" — 그 사람에게만 간다 (V3 §23.2).
fn emit_bare_to(state: &WebState, guild_id: u64, user_id: u64, topic: &str) {
    emit_to(state, guild_id, user_id, topic, json!({}));
}

// ───────────────────────── 라우터 ─────────────────────────

pub fn router() -> Router<Arc<WebState>> {
    Router::new()
        // 페이지 셸
        .route("/music", get(portal_home))
        .route("/music/login", get(login_page))
        .route("/music/oauth/start", get(oauth_start))
        .route("/music/oauth/callback", get(oauth_callback))
        .route("/music/dev-login", post(dev_login))
        .route("/music/logout", post(remote_logout))
        .route("/music/guilds/{guild_id}", get(guild_page))
        .route("/music/guilds/{guild_id}/admin", get(admin_page))
        // 정적 에셋 (리모컨 도메인에서 서빙된다 — host_scope_guard가 /music/* 를 통과시킨다)
        .route("/music/assets/{name}", get(super::assets::serve_asset))
        .route("/music/sw.js", get(super::assets::serve_service_worker))
        .route(
            "/music/manifest.webmanifest",
            get(super::assets::serve_manifest),
        )
        // 개인 설정 — 길드와 무관하다 (V3 §2)
        .route("/music/api/prefs", get(api_prefs_get).put(api_prefs_put))
        // 유저 API
        .route("/music/api/guilds/{guild_id}/state", get(api_state))
        .route("/music/api/guilds/{guild_id}/state/hot", get(api_state_hot))
        .route(
            "/music/api/guilds/{guild_id}/state/cold",
            get(api_state_cold),
        )
        .route("/music/api/guilds/{guild_id}/search", get(api_search))
        .route("/music/api/guilds/{guild_id}/lyrics", get(api_lyrics))
        .route("/music/api/guilds/{guild_id}/audit", get(api_audit))
        .route(
            "/music/api/guilds/{guild_id}/mention-candidates",
            get(api_mention_candidates),
        )
        // 대기열 뒤쪽 (V3 §18.2). `/state/hot` 은 앞 200곡만 싣는다.
        .route(
            "/music/api/guilds/{guild_id}/queue",
            get(api_queue_page).post(api_enqueue),
        )
        // 통계 (V3 §22.6) · 사람 카드 (V3 §24.2)
        .route("/music/api/guilds/{guild_id}/stats/me", get(api_stats_me))
        .route(
            "/music/api/guilds/{guild_id}/stats/user/{user_id}",
            get(api_stats_user),
        )
        .route(
            "/music/api/guilds/{guild_id}/stats/server",
            get(api_stats_server),
        )
        // 차트 (V3 §15.5)
        .route("/music/api/guilds/{guild_id}/charts", get(api_charts))
        .route(
            "/music/api/guilds/{guild_id}/charts/{chart_id}",
            get(api_chart_detail),
        )
        .route(
            "/music/api/guilds/{guild_id}/charts/{chart_id}/enqueue",
            post(api_chart_enqueue),
        )
        .route(
            "/music/api/guilds/{guild_id}/charts/{chart_id}/refresh",
            post(api_chart_refresh),
        )
        .route(
            "/music/api/guilds/{guild_id}/queue/action",
            post(api_queue_action),
        )
        .route("/music/api/guilds/{guild_id}/control", post(api_control))
        // 브라우저가 **실제로 소리를 내기 시작/중단한 순간** 알려 준다 (웹 재생기 모드).
        // 이게 없으면 서버는 "듣고 있는 사람" 을 알 방법이 없다 — 자세한 이유는
        // `WebState::web_listeners` 주석에 적어 뒀다.
        .route(
            "/music/api/guilds/{guild_id}/web-listening",
            post(api_web_listening),
        )
        .route("/music/api/guilds/{guild_id}/vote", post(api_vote))
        .route("/music/api/guilds/{guild_id}/library", post(api_library))
        .route(
            "/music/api/guilds/{guild_id}/playlists/action",
            post(api_playlist_action),
        )
        .route(
            "/music/api/guilds/{guild_id}/chat",
            get(api_chat_list).post(api_chat),
        )
        .route(
            "/music/api/guilds/{guild_id}/chat/reaction",
            post(api_chat_reaction),
        )
        .route(
            "/music/api/guilds/{guild_id}/chat/delete",
            post(api_chat_delete),
        )
        .route("/music/api/guilds/{guild_id}/chat/read", post(api_chat_read))
        .route(
            "/music/api/guilds/{guild_id}/chat/report",
            post(api_chat_report),
        )
        .route(
            "/music/api/guilds/{guild_id}/suggestions",
            get(api_suggestions).post(api_suggestion_create),
        )
        .route(
            "/music/api/guilds/{guild_id}/suggestions/vote",
            post(api_suggestion_vote),
        )
        .route(
            "/music/api/guilds/{guild_id}/suggestions/status",
            post(api_suggestion_status),
        )
        .route(
            "/music/api/guilds/{guild_id}/suspensions",
            post(api_suspend),
        )
        .route("/music/api/guilds/{guild_id}/settings", post(api_settings))
        .route("/music/api/guilds/{guild_id}/events", get(api_events))
        // 자동 재생 (V3 §8.6) — 방식·정책은 일반 유저도 바꿀 수 있다.
        .route(
            "/music/api/guilds/{guild_id}/autoplay",
            get(api_autoplay_get).put(api_autoplay_put),
        )
        // 자동 재생 기준 곡 (V3 §8)
        .route(
            "/music/api/guilds/{guild_id}/autoplay/seeds",
            get(api_autoplay_seeds).post(api_autoplay_seed_add),
        )
        .route(
            "/music/api/guilds/{guild_id}/autoplay/seeds/remove",
            post(api_autoplay_seed_remove),
        )
        .route(
            "/music/api/guilds/{guild_id}/autoplay/seeds/reorder",
            post(api_autoplay_seeds_reorder),
        )
        // 바구니에서 **한 줄만** 빼기 (V3 §8.7). `reset` 은 칸 통째로 비우는 것뿐이라
        // "이 곡 하나만 참고에서 빼고 싶다"를 할 방법이 없었다.
        .route(
            "/music/api/guilds/{guild_id}/autoplay/recent/remove",
            post(api_autoplay_recent_remove),
        )
        .route(
            "/music/api/guilds/{guild_id}/autoplay/blocked/remove",
            post(api_autoplay_blocked_remove),
        )
        // `📻 이 곡 말고` (V3 §8.5-3 · §14.3) — 잡혀 있는 다음 추천곡을 7일간 막고 다시 뽑는다.
        .route(
            "/music/api/guilds/{guild_id}/autoplay/reroll",
            post(api_autoplay_reroll),
        )
        // 추천 바구니 비우기 (V3 §8.7). 기준 곡 권한과 같은 규칙 — 기본값은 모든 멤버.
        .route(
            "/music/api/guilds/{guild_id}/autoplay/reset",
            post(api_autoplay_reset),
        )
        // 로그인 없이 보는 지금 곡 (§29). **읽기 전용이고 사람 정보는 안 나간다.**
        .route(
            "/music/api/guilds/{guild_id}/public",
            get(api_public_now_playing),
        )
        .route("/music/guilds/{guild_id}/now", get(public_now_page))
        // 패치노트 (§30). 로그인 여부와 무관하다 — 무엇이 바뀌었는지는 비밀이 아니다.
        .route("/music/api/changelog", get(api_changelog))
        // API 가이드 문서. 링크를 아는 사람만 들어온다 — 어디에서도 이 주소로 링크하지 않는다.
        // 주소가 `/music` 아래인 것은 취향이 아니다. 리모컨 도메인에서는 `/music/*` 밖이
        // 전부 404 라(`mod.rs` 의 `host_scope_guard`) 다른 자리에 두면 열리지 않는다.
        .route("/music/apidoc", get(apidoc_page))
        // 서버 승인 (§26) — **봇 주인 전용**. 길드 인가를 안 태운다:
        // 아직 승인 안 된 서버가 대상이라 길드 게이트를 통과할 수 없기 때문이다.
        .route("/music/api/owner/guilds", get(api_owner_guilds))
        .route("/music/api/owner/guilds/decide", post(api_owner_decide))
        // 전역 강제값 — **봇 주인 전용**. 승인 라우트와 같은 이유로 길드 인가를 안 태운다:
        // 대상이 특정 서버가 아니라 모든 서버다.
        .route(
            "/music/api/owner/overrides",
            get(api_owner_overrides_get).put(api_owner_overrides_put),
        )
        // 서버 관리 콘솔 API — 전부 Manager 이상
        .route(
            "/music/api/guilds/{guild_id}/admin/settings",
            get(admin_settings_get),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/settings/{section}",
            put(admin_settings_put),
        )
        .route("/music/api/guilds/{guild_id}/admin/roles", get(admin_roles))
        // 인원수 미리보기 — "이 규칙이면 지금 몇 명이 통과하나".
        .route(
            "/music/api/guilds/{guild_id}/admin/preview",
            get(admin_permission_preview),
        )
        // 특정 역할로 보기 (§37) — 관리자 이상. Discord 의 "역할로 보기"와 같은 목적이다.
        // **위 `preview` 와 다른 화면이다.** 저쪽은 인원수, 이쪽은 "그 사람에게 뭐가 열리나".
        .route(
            "/music/api/guilds/{guild_id}/admin/roleview",
            get(admin_role_view),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/queue-preview",
            get(admin_queue_preview),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/permission-preview",
            get(admin_permission_preview),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/participants",
            get(admin_participants),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/suspensions",
            get(admin_suspensions_get).post(admin_suspensions_post),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/suspensions/lift",
            post(admin_suspensions_lift),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/reports",
            get(admin_reports),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/reports/{report_id}/resolve",
            post(admin_report_resolve),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/suggestions",
            get(admin_suggestions),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/suggestions/{suggestion_id}/status",
            post(admin_suggestion_status),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/audit",
            get(admin_audit),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/diagnostics",
            get(admin_diagnostics),
        )
        // 서버 차단 목록 (V3 §19.2) — 자기 길드 항목만 만질 수 있다.
        .route(
            "/music/api/guilds/{guild_id}/admin/blacklist",
            get(admin_blacklist_get).post(admin_blacklist_add),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/blacklist/remove",
            post(admin_blacklist_remove),
        )
        .route(
            "/music/api/guilds/{guild_id}/admin/blacklist/test",
            post(admin_blacklist_test),
        )
}

// ───────────────────────── 공용 헬퍼 ─────────────────────────

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn json_ok(value: Value) -> Response {
    Json(value).into_response()
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn auth_config(state: &WebState) -> RemoteAuthConfig {
    state.remote_auth.read().unwrap().clone()
}

/// S5: POST마다 `reqwest::Client::new()`를 만들면 커넥션 풀과 TLS 세션이 매번 버려진다.
/// 프로세스 하나에 클라이언트 하나를 두고 재사용한다(`Client`는 내부가 Arc라 clone이 싸다).
fn http_client(state: &WebState) -> reqwest::Client {
    state
        .http_client
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(format!(
                    "mc-musicbot/{} (https://musicbot.example.com)",
                    env!("CARGO_PKG_VERSION")
                ))
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default()
        })
        .clone()
}

/// S6: 길이를 먼저 비교하고 나머지는 XOR 누산으로 비교한다. 조기 반환이 없어야 한다.
fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn verify_csrf(session: &RemoteSession, headers: &HeaderMap) -> bool {
    let supplied = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    constant_time_eq(supplied, &session.csrf_token)
}

/// S7: `public_base_url` 문자열만 보면 리버스 프록시 뒤(HTTPS 종단)에서 쿠키가 평문으로 나간다.
/// `X-Forwarded-Proto`도 보고, 확실히 로컬 평문일 때만 Secure를 끈다(기본값은 안전한 쪽).
fn cookie_should_be_secure(auth: &RemoteAuthConfig, headers: Option<&HeaderMap>) -> bool {
    if auth.public_base_url.starts_with("https://") {
        return true;
    }
    let forwarded = headers
        .and_then(|headers| headers.get("x-forwarded-proto"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if forwarded == "https" {
        return true;
    }
    // 남은 경우: 명시적으로 http://localhost 계열이면 개발 환경이므로 Secure를 뺀다.
    let local = auth.public_base_url.starts_with("http://localhost")
        || auth.public_base_url.starts_with("http://127.0.0.1")
        || auth.public_base_url.starts_with("http://[::1]");
    !local
}

/// S4: WS가 실제 데이터를 나르므로 Origin 허용목록이 필수다.
/// 브라우저가 아닌 클라이언트(Origin 없음)는 통과시키지 않는다.
fn origin_allowed(state: &WebState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let origin_host = host_of(origin);
    if origin_host.is_empty() {
        return false;
    }
    // 1) 요청 Host와 같은 출처
    let request_host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !request_host.is_empty() && origin_host == request_host {
        return true;
    }
    // 2) 운영자가 설정한 공개 주소
    let auth = auth_config(state);
    if origin_host == host_of(&auth.public_base_url) {
        return true;
    }
    // 3) 리모컨 전용 도메인
    origin_host == "music.example.com"
}

fn host_of(url: &str) -> String {
    url.rsplit("://")
        .next()
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 대기열 갱신 카운트다운(V3 §5)의 기준 시각 두 개 — `(sortedAt, nextSortAt)`.
///
/// `nextSortAt`은 정렬 루프가 **마지막으로 돈 시각 + 주기**다. 클라이언트 타이머만 쓰면
/// 탭이 백그라운드에 갔다 오는 순간 어긋나므로 기준 시각은 서버가 준다.
/// 루프가 아직 한 번도 안 돌았으면(기동 직후) "지금부터 한 주기"로 근사한다.
///
/// **주기는 대기열 길이를 따라간다** — 500곡을 넘으면 5초가 아니라 15초다 (§18.2 (3)).
/// 화면이 5초를 세는데 서버가 15초마다 돌면 카운트다운이 세 번 헛돈다.
fn sort_clock(state: &WebState, guild_id: u64, queue_len: usize) -> (String, String, i64) {
    let now = chrono::Utc::now();
    // **길드별** 다음 재정렬 시각을 쓴다. 전역 `last_queue_sort` 는 길드 길이와 무관하게
    // 5초 tick 마다 갱신되므로, 주기만 15초인 긴 대기열에서는 화면이 15→11 을 반복하며
    // **0 을 영원히 못 지난다**. 카운트다운은 인과를 보여주려는 기능인데 정반대로 돈다.
    let next = state.app.next_queue_sort_at(guild_id);
    let seconds = crate::app::queue_sort_interval_for_len(queue_len).as_secs() as i64;
    (now.to_rfc3339(), next.to_rfc3339(), seconds.max(1))
}

fn parse_utc(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

fn rate_limited(
    state: &WebState,
    guild_id: u64,
    user_id: u64,
    action: &'static str,
    interval: Duration,
) -> bool {
    let mut rates = state.remote_action_rate.lock().unwrap();
    let key = (guild_id, user_id, action);
    if rates
        .get(&key)
        .is_some_and(|seen| seen.elapsed() < interval)
    {
        return true;
    }
    rates.insert(key, Instant::now());
    false
}

fn audit_failure(
    state: &WebState,
    guild_id: u64,
    session: &RemoteSession,
    action: &str,
    target: Option<&str>,
    reason: &str,
) {
    let _ = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        action,
        target,
        None,
        None,
        false,
        Some(reason),
    );
}

fn audit_ok(
    state: &WebState,
    guild_id: u64,
    session: &RemoteSession,
    action: &str,
    target: Option<&str>,
    after: Option<&str>,
) {
    let id = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        action,
        target,
        None,
        after,
        true,
        None,
    );
    // **한 줄만 실어 보낸다** (V3 §13.5). 예전처럼 빈 신호만 주면 로그 탭이 열려 있는
    // 모든 탭이 통째로 재조회해서, 사람이 많을수록 조용해야 할 화면이 제일 시끄러워진다.
    let payload = id
        .ok()
        .and_then(|id| {
            state
                .app
                .remote
                .list_audit(guild_id, 1, None)
                .into_iter()
                .find(|entry| entry.id == id)
        })
        .filter(|entry| entry.is_human_visible())
        .and_then(|entry| serde_json::to_value(entry.feed_item()).ok())
        .unwrap_or(Value::Null);
    emit(state, guild_id, "audit", json!({ "entry": payload }));
}

// ───────────────────────── 세션 ─────────────────────────

fn session_cookie_token(cookies: &Cookies) -> Option<String> {
    cookies.get(REMOTE_COOKIE).map(|c| c.value().to_string())
}

/// 메모리에 없으면 DB(`remote_web_sessions`)에서 복구한다 — 봇을 재시작해도 로그인이 유지된다.
/// username 컬럼이 없어 display_name으로 대신하고, dev 세션은 복구하지 않는다.
/// **CSRF 토큰은 반드시 저장된 값을 그대로 쓴다** (v17) — 새로 만들면 브라우저의 옛 토큰과
/// 어긋나 로그인은 유지된 채 누르는 것마다 CSRF 실패가 난다.
fn current_session(state: &WebState, cookies: &Cookies) -> Option<RemoteSession> {
    let token = session_cookie_token(cookies)?;
    {
        let mut sessions = state.remote_sessions.lock().unwrap();
        match sessions.get(&token) {
            Some(session) if session.created.elapsed() < REMOTE_SESSION_TTL => {
                return Some(session.clone());
            }
            Some(_) => {
                sessions.remove(&token);
                drop(sessions);
                let _ = state.app.remote.delete_session(&token);
                return None;
            }
            None => {}
        }
    }
    let restored = restore_session(state, &token)?;
    state
        .remote_sessions
        .lock()
        .unwrap()
        .insert(token, restored.clone());
    Some(restored)
}

fn restore_session(state: &WebState, token: &str) -> Option<RemoteSession> {
    let stored = state.app.remote.load_session(token)?;
    let guilds: Vec<OAuthGuild> = serde_json::from_str(&stored.guilds_json).ok()?;
    let now = chrono::Utc::now();
    let created_ago = parse_utc(&stored.created_utc)
        .map(|created| (now - created).num_seconds().max(0) as u64)
        .unwrap_or(0);
    let created = Instant::now()
        .checked_sub(Duration::from_secs(created_ago))
        .unwrap_or_else(Instant::now);
    if created.elapsed() >= REMOTE_SESSION_TTL {
        let _ = state.app.remote.delete_session(token);
        return None;
    }
    // 액세스 토큰의 남은 시간은 알 수 없으므로(만료는 세션 만료로만 기록된다)
    // 즉시 만료로 두고, 다음 요청에서 refresh token으로 갱신하게 한다.
    Some(RemoteSession {
        user_id: stored.user_id,
        // 스키마에 username이 없다 — 표시 이름으로 대체한다.
        username: stored.display_name.clone(),
        display_name: stored.display_name,
        avatar_url: stored.avatar_url,
        guilds,
        access_token: stored.access_token.unwrap_or_default(),
        refresh_token: stored.refresh_token,
        // v17 이전에 저장된 세션은 컬럼이 비어 있다. 그때만 새로 만든다 —
        // 그 세션은 어차피 옛 토큰을 되살릴 방법이 없어서 한 번은 새로고침이 필요하다.
        csrf_token: stored
            .csrf_token
            .filter(|value| !value.is_empty())
            .unwrap_or_else(crate::models::uuid_like),
        created,
        token_expires: Instant::now(),
        // dev 세션은 애초에 저장하지 않으므로 복구본은 항상 일반 세션이다.
        is_developer: false,
    })
}

fn persist_session(state: &WebState, token: &str, session: &RemoteSession) {
    if session.is_developer {
        // 로컬 검증 세션은 코디네이터 우회 플래그를 들고 있어 재기동 후 살아나면 안 된다.
        return;
    }
    let expires = chrono::Utc::now()
        + chrono::Duration::from_std(REMOTE_SESSION_TTL).unwrap_or(chrono::Duration::days(30));
    let stored = StoredSession {
        user_id: session.user_id,
        display_name: session.display_name.clone(),
        avatar_url: session.avatar_url.clone(),
        guilds_json: serde_json::to_string(&session.guilds).unwrap_or_else(|_| "[]".into()),
        access_token: Some(session.access_token.clone()).filter(|value| !value.is_empty()),
        refresh_token: session.refresh_token.clone(),
        expires_utc: expires.to_rfc3339(),
        refreshed_utc: Some(now_utc()),
        created_utc: now_utc(),
        // 재시작 뒤에도 같은 토큰이어야 브라우저의 페이지 셸과 맞는다.
        csrf_token: Some(session.csrf_token.clone()),
    };
    if let Err(error) = state.app.remote.save_session(token, &stored) {
        state
            .app
            .log
            .warn("RemoteAuth", &format!("세션 저장 실패: {error}"));
    }
}

fn begin_remote_session(
    state: &WebState,
    cookies: &Cookies,
    headers: Option<&HeaderMap>,
    session: RemoteSession,
) {
    let auth = auth_config(state);
    let token = crate::models::uuid_like();
    persist_session(state, &token, &session);
    state
        .remote_sessions
        .lock()
        .unwrap()
        .insert(token.clone(), session);
    let mut cookie = Cookie::new(REMOTE_COOKIE, token);
    cookie.set_path("/music");
    cookie.set_http_only(true);
    cookie.set_same_site(tower_cookies::cookie::SameSite::Lax);
    cookie.set_secure(cookie_should_be_secure(&auth, headers));
    cookies.add(cookie);
}

fn end_remote_session(state: &WebState, cookies: &Cookies) {
    if let Some(token) = session_cookie_token(cookies) {
        state.remote_sessions.lock().unwrap().remove(&token);
        let _ = state.app.remote.delete_session(&token);
    }
    let mut expired = Cookie::new(REMOTE_COOKIE, "");
    expired.set_path("/music");
    cookies.remove(expired);
}

/// 액세스 토큰 만료가 가까우면 refresh token으로 갱신한다. 실패해도 세션은 유지한다
/// (역할 재조회만 못 하게 되고, 그건 "일시적 실패"로 처리돼 등급이 유지된다).
async fn refresh_access_token(state: &Arc<WebState>, cookies: &Cookies, session: &mut RemoteSession) {
    if session.is_developer {
        return;
    }
    if session.token_expires > Instant::now() + TOKEN_REFRESH_MARGIN {
        return;
    }
    let Some(refresh_token) = session.refresh_token.clone() else {
        return;
    };
    let auth = auth_config(state);
    let (Some(client_id), Some(client_secret)) = (auth.client_id.clone(), auth.client_secret.clone())
    else {
        return;
    };
    let response = http_client(state)
        .post("https://discord.com/api/v10/oauth2/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await;
    let Ok(response) = response else { return };
    if !response.status().is_success() {
        return;
    }
    let Ok(token) = response.json::<OAuthTokenResponse>().await else {
        return;
    };
    session.access_token = token.access_token.clone();
    if token.refresh_token.is_some() {
        session.refresh_token = token.refresh_token.clone();
    }
    session.token_expires =
        Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(60).max(60));
    if let Some(cookie_token) = session_cookie_token(cookies) {
        state
            .remote_sessions
            .lock()
            .unwrap()
            .insert(cookie_token.clone(), session.clone());
        persist_session(state, &cookie_token, session);
    }
}

fn guild_from_session(session: &RemoteSession, guild_id: u64) -> Option<OAuthGuild> {
    session
        .guilds
        .iter()
        .find(|guild| guild.id == guild_id)
        .cloned()
}

/// 세션의 서버 목록을 Discord 에서 다시 받아 온다 (§35).
///
/// **이게 없으면 "멤버인데 멤버가 아니라고" 나온다.** 목록은 로그인할 때 한 번만 받는데
/// 세션은 30일을 산다. 그 사이에 서버에 새로 들어가거나 봇이 새 서버에 초대되면,
/// 그 사람은 다시 로그인하기 전까지 영영 비멤버 취급이다. 실제로 그렇게 막혔다.
///
/// 목록에 없을 때만 부른다. 매 요청마다 부르면 Discord 가 429 를 주기 시작한다.
async fn refresh_session_guilds(
    state: &Arc<WebState>,
    cookies: &Cookies,
    session: &mut RemoteSession,
) -> bool {
    if session.is_developer || session.access_token.is_empty() {
        return false;
    }
    // 같은 사람이 없는 서버를 계속 두드려도 조회는 이 간격으로만 나간다.
    {
        let mut seen = state.guild_refresh_at.lock().unwrap();
        // 넣을 때 지난 것도 같이 걷어낸다. 안 그러면 사람 수만큼 프로세스 수명 내내 쌓인다
        // (`oauth_states` 가 쓰는 방식과 같다).
        seen.retain(|_, at| at.elapsed() < GUILD_REFRESH_INTERVAL);
        if seen.contains_key(&session.user_id) {
            return false;
        }
        seen.insert(session.user_id, Instant::now());
    }
    let client = http_client(state);
    let Ok(rows) =
        discord_get::<Vec<DiscordGuildResponse>>(&client, &session.access_token, "/users/@me/guilds")
            .await
    else {
        return false;
    };
    let guilds = to_oauth_guilds(rows);
    // 빈 목록은 조회가 잘못됐다는 뜻으로 본다. 그대로 덮으면 멀쩡한 사람이
    // 모든 서버에서 비멤버가 된다 — 조회 실패가 권한 박탈로 번지면 안 된다.
    if guilds.is_empty() {
        return false;
    }
    session.guilds = guilds;
    if let Some(cookie_token) = session_cookie_token(cookies) {
        state
            .remote_sessions
            .lock()
            .unwrap()
            .insert(cookie_token.clone(), session.clone());
        persist_session(state, &cookie_token, session);
    }
    true
}

/// 서버 목록을 다시 받아 오는 최소 간격. Discord 는 이 조회에도 한도를 건다.
const GUILD_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Discord 응답 → 세션에 담는 모양. 로그인 때와 갱신 때가 **같은 변환**을 써야
/// 새로 받은 목록만 권한 비트가 달라지는 일이 안 생긴다.
fn to_oauth_guilds(rows: Vec<DiscordGuildResponse>) -> Vec<OAuthGuild> {
    rows.into_iter()
        .filter_map(|guild| {
            Some(OAuthGuild {
                id: guild.id.parse().ok()?,
                name: guild.name,
                icon: guild.icon,
                owner: guild.owner,
                permissions: guild.permissions.parse().unwrap_or(0),
            })
        })
        .collect()
}

fn bot_in_guild(state: &WebState, guild_id: u64) -> bool {
    state
        .app
        .discord_cache
        .get()
        .and_then(|cache| cache.guild(GuildId::new(guild_id)))
        .is_some()
}

fn is_owner_user(state: &WebState, user_id: u64) -> bool {
    state
        .app
        .owner_user_ids
        .read()
        .map(|ids| ids.contains(&user_id))
        .unwrap_or(false)
}

// ───────────────────────── 권한 판정 ─────────────────────────

/// 멤버 재조회 실패의 성격. 강등해야 하는 실패와 그냥 넘겨야 하는 실패를 구분한다.
enum MemberLookupError {
    /// 추방·탈퇴 등 "이 길드에 없음". `Viewer`로 강등한다.
    NotInGuild,
    /// 429·5xx·네트워크 등 일시적 실패. 캐시된 등급을 유지한다.
    Transient(String),
}

#[derive(Debug, Deserialize)]
struct DiscordMemberResponse {
    #[serde(default)]
    roles: Vec<String>,
}

/// Discord에서 이 사람의 길드 역할을 읽는다. `fresh`면 캐시를 무시한다.
async fn fetch_member_roles(
    state: &Arc<WebState>,
    session: &RemoteSession,
    guild_id: u64,
    fresh: bool,
) -> Result<Vec<u64>, MemberLookupError> {
    let key = (guild_id, session.user_id);
    if !fresh {
        let cached = state
            .remote_member_roles
            .lock()
            .unwrap()
            .get(&key)
            .filter(|(seen, _)| seen.elapsed() < MEMBER_CACHE_TTL)
            .map(|(_, roles)| roles.clone());
        if let Some(roles) = cached {
            return Ok(roles);
        }
    }
    let path = format!("/users/@me/guilds/{guild_id}/member");
    let response = http_client(state)
        .get(format!("https://discord.com/api/v10{path}"))
        .bearer_auth(&session.access_token)
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return Err(stale_or_transient(
                state,
                key,
                format!("Discord 연결 실패: {error}"),
            ));
        }
    };
    let status = response.status();
    if !status.is_success() {
        // 404/403 = 이 길드에 없음. 429/5xx = 일시적.
        if status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN {
            return Err(MemberLookupError::NotInGuild);
        }
        return Err(stale_or_transient(
            state,
            key,
            format!("Discord API 응답 {status}"),
        ));
    }
    match response.json::<DiscordMemberResponse>().await {
        Ok(member) => {
            let roles: Vec<u64> = member
                .roles
                .into_iter()
                .filter_map(|role| role.parse().ok())
                .collect();
            state
                .remote_member_roles
                .lock()
                .unwrap()
                .insert(key, (Instant::now(), roles.clone()));
            // **디스크에도 적는다.** 메모리 캐시는 재시작에 날아가고, 그 직후 Discord 가
            // 429 를 주면 역할이 빈 목록이 되어 지정 역할 권한자가 통째로 강등된다.
            state
                .app
                .remote
                .save_member_roles(guild_id, session.user_id, &roles);
            Ok(roles)
        }
        Err(error) => Err(stale_or_transient(
            state,
            key,
            format!("Discord 응답 해석 실패: {error}"),
        )),
    }
}

/// 일시적 실패(429·5xx·네트워크). 호출부가 `stale_roles`로 캐시를 되살려 등급을 유지한다.
fn stale_or_transient(
    _state: &Arc<WebState>,
    _key: (u64, u64),
    reason: String,
) -> MemberLookupError {
    MemberLookupError::Transient(reason)
}

/// 일시 실패 때 되살릴 역할. 메모리를 먼저 보고, 없으면 디스크를 본다.
///
/// **`None` 과 `Some(vec![])` 는 전혀 다른 뜻이다.** 앞은 "아직 모른다", 뒤는
/// "역할이 진짜 하나도 없다". 예전엔 `unwrap_or_default()` 로 둘을 같게 만들어서,
/// 재시작 직후 429 가 나면 지정 역할 권한자가 "권한이 없어요" 를 봤다.
fn stale_roles(state: &Arc<WebState>, guild_id: u64, user_id: u64) -> Option<Vec<u64>> {
    let in_memory = state
        .remote_member_roles
        .lock()
        .unwrap()
        .get(&(guild_id, user_id))
        .filter(|(seen, _)| seen.elapsed() < MEMBER_CACHE_GRACE)
        .map(|(_, roles)| roles.clone());
    if in_memory.is_some() {
        return in_memory;
    }
    let grace_hours = (MEMBER_CACHE_GRACE.as_secs() / 3600).max(1) as i64;
    let from_disk = state
        .app
        .remote
        .load_member_roles(guild_id, user_id, grace_hours);
    if let Some(roles) = from_disk.clone() {
        // 디스크에서 살린 것도 메모리에 올려 둔다. 매 요청마다 SQLite 를 칠 이유가 없다.
        state
            .remote_member_roles
            .lock()
            .unwrap()
            .insert((guild_id, user_id), (Instant::now(), roles));
    }
    from_disk
}

/// 이 사람이 봇과 같은 음성 채널에 있는지.
///
/// **V3 §16 B1**: 봇이 *지금* 어디 있는지는 저장값(`player.voice_channel_id`)이 아니라
/// Discord 캐시가 결정한다. 저장값을 쓰면 봇이 비정상 경로로 빠져나간 뒤에도
/// "같은 채널에 있다"가 되어 권한이 열린 채로 남는다.
fn same_voice_channel(state: &WebState, guild_id: u64, user_id: u64) -> bool {
    let Some(bot_channel) = bot_voice_status(state, guild_id).channel_id else {
        return false;
    };
    state
        .app
        .discord_cache
        .get()
        .and_then(|cache| cache.guild(GuildId::new(guild_id)))
        .and_then(|guild| {
            guild
                .voice_states
                .get(&UserId::new(user_id))
                .and_then(|voice| voice.channel_id)
        })
        .map(|channel| channel.get() == bot_channel)
        .unwrap_or(false)
}

/// **S3 수정**: `Disabled`는 누구도(관리자·봇 주인 포함) 통과하지 못한다.
/// 관리자 우회는 `Disabled`가 아닌 규칙에만 적용된다.
/// 서버 관리 콘솔의 permission-preview도 이 함수를 그대로 쓰므로 화면과 실제 판정이 항상 같다.
///
/// **V3 §1**: 지정 역할은 이제 권한 키마다 따로다. 검색용으로 `@DJ`를 넣었다고
/// 볼륨·대기열편집까지 열리면 안 된다. `key`는 `PERMISSION_KEYS`의 값이고,
/// 목록에 없는 키(`library` 등)는 레거시 지정 역할로 폴백한다(`roles_for` 참고).
pub fn permission_allowed(
    key: &str,
    rule: PermissionRule,
    settings: &RemoteGuildSettings,
    member: &MemberContext,
) -> bool {
    if rule == PermissionRule::Disabled {
        return false;
    }
    if member.is_admin {
        return true;
    }
    match rule {
        PermissionRule::GuildMember => true,
        PermissionRule::SameVoiceChannel => same_voice_satisfied(settings, member),
        PermissionRule::ConfiguredRole => has_configured_role(key, settings, member),
        PermissionRule::Administrator | PermissionRule::Disabled => false,
    }
}

/// `SameVoiceChannel` 규칙이 지금 통과하는지.
///
/// 규칙의 목적은 **같이 듣고 있는 사람들의 재생을 남이 흔들지 못하게** 하는 것이다.
/// 그런데 봇이 음성에 아예 없으면 흔들 재생도, 방해받을 사람도 없다. 그 상태에서까지
/// 막으면 서버가 `봇이 음성 채널에 있어야만 조작` 을 꺼도 아무것도 안 풀린다 —
/// 리모컨을 웹 재생기로 쓰겠다는 선택이 설정만 있고 효과가 없는 셈이 된다.
///
/// 그래서 **봇이 음성에 없고 서버가 그 요구를 껐을 때만** 통과시킨다. 요구가 켜져 있으면
/// 예전 그대로다(봇이 없으면 조작도 없다).
fn same_voice_satisfied(settings: &RemoteGuildSettings, member: &MemberContext) -> bool {
    if member.same_voice_channel {
        return true;
    }
    // 봇이 음성에 **있는데** 내가 다른 채널이면 그대로 막는다. 그때는 방해받을 사람이 실제로 있다.
    if member.bot_in_voice {
        return false;
    }
    // 봇이 아예 없을 때만 두 가지 사정으로 열린다.
    //   1. 서버가 "봇이 있어야 조작" 을 껐다 — 막을 청취자가 없다는 판단을 서버가 내린 것.
    //   2. 웹 재생기 모드 — 봇 없이 리모컨으로 같이 듣는 중이라 조작할 대상이 실제로 있다.
    !settings.require_voice_for_playback || settings.web_player_mode
}

/// 이 조작이 "봇이 음성 채널에 있어야 한다"는 제한(`require_voice_for_playback`)을 받는지.
///
/// **자동 재생만 예외다.** 나머지는 지금 나오는 소리를 건드리는 명령이라 봇이 음성에 없으면
/// 아무 일도 안 일어나는 유령 조작이 된다(V3 §16 B1). 반면 자동 재생 On/Off 는 DB 에 저장되는
/// 설정이고, 봇이 음성에 없을 때야말로 "다음에 들어오면 알아서 틀어" 를 켜 두려는 순간이다.
fn action_requires_voice(action: &str) -> bool {
    action != "autoplay"
}

/// 이 사람이 그 권한 키의 지정 역할을 하나라도 갖고 있는지.
fn has_configured_role(
    key: &str,
    settings: &RemoteGuildSettings,
    member: &MemberContext,
) -> bool {
    let allowed = settings.roles_for(key);
    member.role_ids.iter().any(|role| allowed.contains(role))
}

/// 관리자 우회 없이 규칙 자체로 통과하는지 — "← 관리자라 통과" 표시 판정에 쓴다.
fn rule_base_allowed(
    key: &str,
    rule: PermissionRule,
    settings: &RemoteGuildSettings,
    member: &MemberContext,
) -> bool {
    match rule {
        PermissionRule::GuildMember => true,
        PermissionRule::SameVoiceChannel => same_voice_satisfied(settings, member),
        PermissionRule::ConfiguredRole => has_configured_role(key, settings, member),
        PermissionRule::Administrator => member.is_admin,
        PermissionRule::Disabled => false,
    }
}

fn rule_key(rule: PermissionRule) -> &'static str {
    match rule {
        PermissionRule::GuildMember => "guildMember",
        PermissionRule::SameVoiceChannel => "sameVoiceChannel",
        PermissionRule::ConfiguredRole => "configuredRole",
        PermissionRule::Administrator => "administrator",
        PermissionRule::Disabled => "disabled",
    }
}

fn rule_label(rule: PermissionRule) -> &'static str {
    match rule {
        PermissionRule::GuildMember => "모든 멤버",
        PermissionRule::SameVoiceChannel => "같은 음성채널",
        PermissionRule::ConfiguredRole => "지정 역할",
        PermissionRule::Administrator => "관리자",
        PermissionRule::Disabled => "사용 안 함",
    }
}

/// 이름이 바뀐 권한 키를 지금 이름으로 바꿔 준다 (V3 §1).
///
/// 옛 관리 콘솔이 아직 `autoplaySeed` 를 보낼 수 있다. 400으로 튕기면
/// 저장 버튼이 이유 없이 안 먹는 화면이 되므로 조용히 받아 준다.
fn canonical_permission_key(key: &str) -> &str {
    match key {
        "autoplaySeed" => "autoplay",
        "playlistEnqueue" => "bulkEnqueue",
        other => other,
    }
}

fn parse_rule(value: &str) -> Option<PermissionRule> {
    match value {
        "guildMember" => Some(PermissionRule::GuildMember),
        "sameVoiceChannel" => Some(PermissionRule::SameVoiceChannel),
        "configuredRole" => Some(PermissionRule::ConfiguredRole),
        "administrator" => Some(PermissionRule::Administrator),
        "disabled" => Some(PermissionRule::Disabled),
        _ => None,
    }
}

/// 사양서 §1.1 판정 순서를 그대로 구현한다.
/// `headers`가 있으면 CSRF도 함께 검사한다(변경 요청).
async fn authorize(
    state: &Arc<WebState>,
    cookies: &Cookies,
    guild_id: u64,
    headers: Option<&HeaderMap>,
) -> Result<AuthContext, Response> {
    // 1. 세션
    let mut session = current_session(state, cookies)
        .ok_or_else(|| json_error(StatusCode::UNAUTHORIZED, "Discord 로그인이 필요해요."))?;
    if let Some(headers) = headers {
        if !verify_csrf(&session, headers) {
            return Err(json_error(
                StatusCode::FORBIDDEN,
                "CSRF 검증에 실패했어요.",
            ));
        }
    }
    refresh_access_token(state, cookies, &mut session).await;

    // 2. 세션의 길드 목록에 없음 → **바로 거절하지 않고 목록을 다시 받아 본다** (§35).
    //    목록은 로그인 때 한 번만 받는데 세션은 30일을 산다. 그 사이에 서버에 새로
    //    들어간 사람은 다시 로그인하기 전까지 계속 "멤버가 아니에요" 를 봤다.
    let guild = match guild_from_session(&session, guild_id) {
        Some(guild) => guild,
        None => {
            if refresh_session_guilds(state, cookies, &mut session).await {
                guild_from_session(&session, guild_id)
            } else {
                None
            }
            .ok_or_else(|| {
                json_error(
                    StatusCode::FORBIDDEN,
                    "이 서버의 멤버가 아니에요. 방금 들어오셨다면 잠시 뒤에 새로고침해 주세요.",
                )
            })?
        }
    };

    // 3. 봇이 그 길드에 없음 → 403
    if !session.is_developer && !bot_in_guild(state, guild_id) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "봇이 이 Discord 서버에 없어요.",
        ));
    }

    // 3b. 아직 승인 안 된 서버 (§26). **봇 주인은 통과** — 승인 화면을 보려면 들어와야 한다.
    if !session.is_developer && !is_owner_user(state, session.user_id) {
        let approval = state
            .app
            .remote
            .guild_approval(guild_id)
            .map(|row| row.status)
            .unwrap_or_default();
        if !approval.is_usable() {
            return Err(json_error(StatusCode::FORBIDDEN, approval.reason()));
        }
    }

    let settings = state.app.remote.load_guild_settings(guild_id);

    // 4. 정지 상태 조회 → 전체 정지면 Viewer
    let suspensions = state
        .app
        .remote
        .active_suspensions(guild_id, session.user_id);
    let all_suspended = suspensions
        .iter()
        .any(|item| item.scope == SuspensionScope::All);

    // 5~8. 등급 판정
    let (mut tier, mut member, mut viewer_reason, roles_known) =
        resolve_tier(state, &session, &guild, &settings, headers.is_some()).await;

    if all_suspended {
        tier = AccessTier::Viewer;
        member.is_admin = false;
        viewer_reason = Some("전체 정지 중이라 읽기 전용이에요.".into());
    }

    Ok(AuthContext {
        session,
        guild,
        settings,
        member,
        tier,
        suspensions,
        viewer_reason,
        roles_known,
    })
}

/// 등급과 멤버 컨텍스트를 함께 만든다.
async fn resolve_tier(
    state: &Arc<WebState>,
    session: &RemoteSession,
    guild: &OAuthGuild,
    settings: &RemoteGuildSettings,
    fresh: bool,
) -> (AccessTier, MemberContext, Option<String>, bool) {
    let guild_id = guild.id;
    let owner = is_owner_user(state, session.user_id);

    if session.is_developer {
        return (
            if owner { AccessTier::Owner } else { AccessTier::Manager },
            MemberContext {
                is_admin: true,
                same_voice_channel: true,
                bot_in_voice: true,
                role_ids: Vec::new(),
            },
            None,
            true,
        );
    }

    let same_voice = same_voice_channel(state, guild_id, session.user_id);
    // 봇이 음성에 아예 없는 경우를 따로 안다 — `same_voice` 만으로는 구분되지 않는다.
    let bot_in_voice = bot_voice_status(state, guild_id).in_voice();
    let lookup = fetch_member_roles(state, session, guild_id, fresh).await;

    // `roles_known` 이 false 면 "역할이 없다" 가 아니라 **"아직 모른다"** 이다.
    // 권한을 열어 주지는 않지만, 거절할 때 이유를 그렇게 말해야 한다.
    let (role_ids, demote, roles_known) = match lookup {
        Ok(roles) => (roles, false, true),
        Err(MemberLookupError::NotInGuild) => (Vec::new(), true, true),
        Err(MemberLookupError::Transient(reason)) => match stale_roles(state, guild_id, session.user_id) {
            Some(roles) => {
                state.app.log.warn(
                    "RemoteAuth",
                    &format!("길드 {guild_id} 멤버 재조회 일시 실패 — 저장된 역할로 등급 유지: {reason}"),
                );
                (roles, false, true)
            }
            None => {
                // 되살릴 것이 아무것도 없다. 재시작 직후 + Discord 429 가 겹치면 여기 온다.
                state.app.log.warn(
                    "RemoteAuth",
                    &format!(
                        "길드 {guild_id} 멤버 재조회 실패 + 저장된 역할 없음 — 역할 기반 권한을 판정할 수 없어요: {reason}"
                    ),
                );
                (Vec::new(), false, false)
            }
        },
    };

    // 7. 추방·탈퇴 → 403이 아니라 Viewer로 강등한다.
    if demote && !owner {
        return (
            AccessTier::Viewer,
            MemberContext {
                is_admin: false,
                same_voice_channel: same_voice,
                bot_in_voice,
                role_ids,
            },
            Some("이 서버에서 나갔거나 추방돼서 읽기 전용이에요.".into()),
            roles_known,
        );
    }

    // 5. 봇 주인
    let manager_roles = settings.manager_roles();
    let tier = if owner {
        AccessTier::Owner
    } else if guild.is_admin() || role_ids.iter().any(|role| manager_roles.contains(role)) {
        // 6. ADMINISTRATOR / MANAGE_GUILD / 길드 소유자 / **관리자 지정 역할**
        //    (V3 §1: 권한용 지정 역할과 갈라졌다 — 검색 역할을 준 사람이 관리자가 되면 안 된다)
        AccessTier::Manager
    } else {
        AccessTier::Member
    };

    (
        tier,
        MemberContext {
            is_admin: tier.is_manager(),
            same_voice_channel: same_voice,
            bot_in_voice,
            role_ids,
        },
        None,
        roles_known,
    )
}

// ───────────────────────── 접속 레지스트리 (B4) ─────────────────────────
//
// DB를 쓰지 않는다. WS 연결 수를 메모리에 세고, Discord 캐시와 합쳐서 만든다.
// 변경이 잦아도 broadcast는 최대 초당 1회로 코얼레싱한다(사양서 §5.2 E).

fn presence_add(state: &Arc<WebState>, guild_id: u64, user_id: u64) {
    *state
        .presence
        .lock()
        .unwrap()
        .entry((guild_id, user_id))
        .or_insert(0) += 1;
    schedule_presence(state, guild_id);
}

fn presence_remove(state: &Arc<WebState>, guild_id: u64, user_id: u64) {
    let mut gone = false;
    {
        let mut registry = state.presence.lock().unwrap();
        if let Some(count) = registry.get_mut(&(guild_id, user_id)) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                registry.remove(&(guild_id, user_id));
                gone = true;
            }
        }
    }
    // **마지막 소켓이 닫힐 때만** 웹 리스너에서도 뺀다. 탭을 여러 개 열어 두는 게 흔하고,
    // 하나 닫았다고 듣기가 끝난 것은 아니다. 알림 없이 사라지는 경우(탭 강제 종료·크래시)가
    // 흔하므로 소켓 종료를 진실로 삼는다 — `web-listening` 보고만 믿으면 유령 리스너가 남는다.
    if gone {
        let removed = state
            .web_listeners
            .lock()
            .unwrap()
            .remove(&(guild_id, user_id));
        if removed {
            let state2 = state.clone();
            tokio::spawn(async move {
                on_web_listeners_changed(&state2, guild_id).await;
            });
        }
    }
    schedule_presence(state, guild_id);
}

/// 이 길드의 리모컨 화면을 보고 있는 사람들.
fn viewers_of(state: &WebState, guild_id: u64) -> Vec<u64> {
    let registry = state.presence.lock().unwrap();
    let mut ids: Vec<u64> = registry
        .iter()
        .filter(|((gid, _), count)| *gid == guild_id && **count > 0)
        .map(|((_, uid), _)| *uid)
        .collect();
    ids.sort_unstable();
    ids
}

fn viewer_count(state: &WebState, guild_id: u64) -> usize {
    viewers_of(state, guild_id).len()
}

/// 초당 1회 코얼레싱. 창이 열려 있으면 즉시, 아니면 남은 시간만큼 뒤에 한 번만 보낸다.
fn schedule_presence(state: &Arc<WebState>, guild_id: u64) {
    let wait = {
        let mut gates = state.presence_gate.lock().unwrap();
        let gate = gates.entry(guild_id).or_insert((
            Instant::now()
                .checked_sub(PRESENCE_COALESCE * 2)
                .unwrap_or_else(Instant::now),
            false,
        ));
        let elapsed = gate.0.elapsed();
        if elapsed >= PRESENCE_COALESCE {
            gate.0 = Instant::now();
            None
        } else if gate.1 {
            return; // 이미 예약돼 있다
        } else {
            gate.1 = true;
            Some(PRESENCE_COALESCE - elapsed)
        }
    };
    let state = state.clone();
    tokio::spawn(async move {
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
            let mut gates = state.presence_gate.lock().unwrap();
            if let Some(gate) = gates.get_mut(&guild_id) {
                gate.0 = Instant::now();
                gate.1 = false;
            }
        }
        let payload = build_presence(&state, guild_id).await;
        emit(&state, guild_id, "presence", payload);
    });
}

/// 봇이 지금 어디에 있는지. Discord 캐시만 본다 — **DB도 저장값도 안 본다**(V3 §4·§16 B1).
///
/// **여기가 B1의 진원지였다.** 예전 코드는 캐시가 "음성에 없음"(`None`)을 정확히 줘도
/// `.or(player_channel)` 로 저장값을 덮어써서, 봇이 재시작·연결 끊김·Discord 강제 퇴장으로
/// 빠져나간 뒤에도 화면에는 계속 들어가 있다고 나왔다.
///
/// 저장된 `player.voice_channel_id` 는 **"다음에 어디로 들어갈까"에만** 쓴다.
/// "지금 어디 있나"의 근거로 쓰는 곳은 하나도 없어야 한다.
///
/// 기동 직후 캐시에 길드가 아직 없는 몇 초는 `in_guild: false` 가 받아 준다 —
/// 잠깐 "확인 중"인 게, 없는 걸 있다고 하는 것보다 낫다.
#[derive(Debug, Clone, Default)]
pub(crate) struct BotVoiceStatus {
    in_guild: bool,
    pub(crate) channel_id: Option<u64>,
    pub(crate) channel_name: Option<String>,
}

/// 웹 쪽 호출부를 위한 얇은 래퍼. 판정은 `bot_voice_status_of` 하나뿐이다.
pub(crate) fn bot_voice_status(state: &WebState, guild_id: u64) -> BotVoiceStatus {
    bot_voice_status_of(&state.app, guild_id)
}

impl BotVoiceStatus {
    pub(crate) fn in_voice(&self) -> bool {
        self.channel_id.is_some()
    }
}

/// B1 회귀를 코드로 못 박아 두는 자리.
///
/// 캐시가 진실이고 저장값은 **어떤 경우에도** 결과에 섞이지 않는다.
/// 인자로 받기만 하고 쓰지 않는 게 의도다 — 다시 `.or(stored)` 를 넣으려는 손을 막는다.
fn authoritative_voice_channel(cache_says: Option<u64>, stored: Option<u64>) -> Option<u64> {
    let _ = stored;
    cache_says
}

/// 봇이 지금 이 길드의 어느 음성 채널에 있는지 — **Discord 캐시가 진실이다** (§16 B1).
///
/// `App` 만 받는다. 본문이 `discord_cache` 밖에 안 쓰는데 `WebState` 를 요구하면
/// 코디네이터처럼 웹 바깥에 있는 쪽이 같은 판정을 못 쓴다. 저장된 `voice_channel_id` 로
/// 대신하면 안 된다 — 그건 "다음에 어디로 들어갈까" 이고 강제 퇴장 뒤에도 남는다.
pub(crate) fn bot_voice_status_of(app: &Arc<App>, guild_id: u64) -> BotVoiceStatus {
    let Some(cache) = app.discord_cache.get() else {
        return BotVoiceStatus::default();
    };
    let Some(guild) = cache.guild(GuildId::new(guild_id)) else {
        return BotVoiceStatus::default();
    };
    let bot_id = cache.current_user().id;
    let channel_id = authoritative_voice_channel(
        guild
            .voice_states
            .get(&bot_id)
            .and_then(|voice| voice.channel_id)
            .map(|channel| channel.get()),
        None,
    );
    let channel_name = channel_id.and_then(|channel| {
        guild
            .channels
            .get(&serenity::all::ChannelId::new(channel))
            .map(|channel| channel.name.clone())
    });
    BotVoiceStatus {
        in_guild: true,
        channel_id,
        channel_name,
    }
}

/// 음성에 있는 사람들을 `듣는 중`과 `다른 채널에 있어요`로 가른다 (V3 §4).
///
/// **봇이 음성에 없으면 `듣는 중`은 언제나 빈 배열**이다. 그때 음성에 있는 사람은
/// 전부 "다른 채널"이다 — 봇 없는 방에서 나는 소리를 듣는 중이라고 부를 수는 없다.
fn split_voice_members(
    bot_channel: Option<u64>,
    members: &[(u64, Option<u64>)],
) -> (Vec<String>, Vec<String>) {
    let mut listening: Vec<String> = Vec::new();
    let mut in_other_voice: Vec<String> = Vec::new();
    for (user_id, channel) in members {
        let Some(channel) = channel else { continue };
        match bot_channel {
            Some(bot_channel) if bot_channel == *channel => listening.push(user_id.to_string()),
            _ => in_other_voice.push(user_id.to_string()),
        }
    }
    listening.sort();
    in_other_voice.sort();
    (listening, in_other_voice)
}

/// 🎧 듣는 중 / 🎤 다른 채널 / 🖥 보는 중 / 🟢 온라인 + 봇 상태.
/// 인텐트가 꺼져 있으면 해당 부분을 빼고 보낸다.
///
/// `listening`은 **봇이 들어가 있는 그 채널에 같이 있는 사람만**이다.
/// 봇이 음성에 없으면 언제나 빈 배열이고, 다른 채널에 있는 사람은 `inOtherVoice`로 간다.
async fn build_presence(state: &Arc<WebState>, guild_id: u64) -> Value {
    let viewing: Vec<String> = viewers_of(state, guild_id)
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    let bot = bot_voice_status(state, guild_id);
    let presences_intent = state
        .app
        .intent_status
        .read()
        .map(|status| status.presences)
        .unwrap_or(true);

    let mut voice_members: Vec<(u64, Option<u64>)> = Vec::new();
    let mut online = serde_json::Map::new();
    if let Some(cache) = state.app.discord_cache.get() {
        if let Some(guild) = cache.guild(GuildId::new(guild_id)) {
            let bot_id = cache.current_user().id;
            for (user_id, voice) in guild.voice_states.iter() {
                if *user_id == bot_id {
                    continue;
                }
                // 다른 봇은 인원수에 넣지 않는다 — "3명이 듣는 중"에 봇이 끼면 거짓말이 된다.
                if guild
                    .members
                    .get(user_id)
                    .map(|member| member.user.bot)
                    .unwrap_or(false)
                {
                    continue;
                }
                voice_members.push((
                    user_id.get(),
                    voice.channel_id.map(|channel| channel.get()),
                ));
            }
            if presences_intent {
                for (user_id, presence) in guild.presences.iter() {
                    online.insert(
                        user_id.get().to_string(),
                        Value::String(presence.status.name().to_string()),
                    );
                }
            }
        }
    }
    let (listening, in_other_voice) = split_voice_members(bot.channel_id, &voice_members);
    json!({
        "listening": listening,
        "inOtherVoice": in_other_voice,
        "viewing": viewing,
        "online": Value::Object(online),
        "listeningCount": listening.len(),
        "inOtherVoiceCount": in_other_voice.len(),
        "viewingCount": viewing.len(),
        "bot": {
            "inGuild": bot.in_guild,
            "inVoice": bot.in_voice(),
            "voiceChannelId": bot.channel_id.map(|id| id.to_string()),
            "voiceChannelName": bot.channel_name,
            "listenerCount": listening.len(),
        },
    })
}

/// 길드 멤버 목록(👥). Server Members Intent가 꺼져 있으면 빈 배열이 되고
/// 프런트는 그 부분을 숨긴다 — 봇이 죽으면 안 된다.
fn build_members(state: &WebState, ctx_guild_id: u64, settings: &RemoteGuildSettings) -> Vec<Value> {
    let members_intent = state
        .app
        .intent_status
        .read()
        .map(|status| status.members)
        .unwrap_or(true);
    if !members_intent {
        return Vec::new();
    }
    let Some(cache) = state.app.discord_cache.get() else {
        return Vec::new();
    };
    let Some(guild) = cache.guild(GuildId::new(ctx_guild_id)) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = Vec::with_capacity(guild.members.len());
    for (user_id, member) in guild.members.iter() {
        if member.user.bot {
            continue;
        }
        let admin = guild.owner_id == *user_id
            || member.roles.iter().any(|role| {
                guild
                    .roles
                    .get(role)
                    .map(|role| {
                        role.permissions.contains(Permissions::ADMINISTRATOR)
                            || role.permissions.contains(Permissions::MANAGE_GUILD)
                    })
                    .unwrap_or(false)
            });
        let manager_role = member
            .roles
            .iter()
            .any(|role| settings.manager_roles().contains(&role.get()));
        let tier = if is_owner_user(state, user_id.get()) {
            AccessTier::Owner
        } else if admin || manager_role {
            AccessTier::Manager
        } else {
            AccessTier::Member
        };
        out.push(json!({
            "userId": user_id.get().to_string(),
            "displayName": member.display_name(),
            "avatarUrl": member.face(),
            "tier": tier.as_str(),
        }));
    }
    out.sort_by(|left, right| {
        left["displayName"]
            .as_str()
            .unwrap_or("")
            .cmp(right["displayName"].as_str().unwrap_or(""))
    });
    out
}

// ───────────────────────── 재생 감시 (길드당 1개) ─────────────────────────
//
// 예전 프런트는 탭마다 2초 폴링으로 곡 전환을 알아챘다. 그걸 없앤 대신,
// **길드당 하나**의 태스크가 재생 상태를 훑어 변화가 있을 때만 이벤트를 민다.
// 탭이 10개든 1개든 비용이 같고, 아무도 안 보고 있으면 태스크 자체가 돌지 않는다.

fn ensure_guild_watcher(state: &Arc<WebState>, guild_id: u64) {
    {
        let mut watchers = state.guild_watchers.lock().unwrap();
        if !watchers.insert(guild_id) {
            return;
        }
    }
    let state = state.clone();
    tokio::spawn(async move {
        let mut last_signature = String::new();
        let mut last_presence = String::new();
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            if viewer_count(&state, guild_id) == 0 {
                break;
            }
            // 봇이 음성 채널을 옮기거나 사람이 들락날락하는 건 WS 연결 수와 무관해서
            // 접속 레지스트리만 봐서는 알 수 없다. 여기서 캐시를 훑어 달라졌을 때만 민다.
            // 전부 메모리·Discord 캐시라 DB 쿼리는 0회다 (§5.2 E).
            let presence = build_presence(&state, guild_id).await;
            let presence_wire = presence.to_string();
            if presence_wire != last_presence {
                last_presence = presence_wire;
                emit(&state, guild_id, "presence", presence);
            }
            let player = state.app.player.get_state(guild_id).await;
            let position = state
                .app
                .coordinator
                .current_position(guild_id)
                .await
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            let sampled_at = now_utc();
            // **음성 연결 여부도 변화로 센다** (§36). 이게 없으면 곡이 그대로인 채로
            // 봇만 음성에서 빠졌을 때 아무 프레임도 안 나가고, 화면은 계속 재생 중으로 남는다.
            // 다음 추천곡도 마찬가지다 — 대기열 ID 는 그대로라 `next` 만 바뀐 경우를 놓친다.
            let voice_connected = bot_voice_status(&state, guild_id).in_voice();
            // **시각표 자체를 서명에 넣는다.** 한 곡 반복에서는 `current_item.id` 도 그대로고
            // 세션 유무도 그대로인데 `started_utc` 만 새로 발급된다. 그걸 안 보면 프레임이
            // 안 나가서 웹 재생이 같은 곡을 다시 시작하지 못한다 — 교차검증이 잡아 준 것이다.
            // 가상↔물리 전환도 이 값이 바뀌므로 같이 잡힌다.
            let started_sig = state
                .app
                .coordinator
                .schedule(guild_id)
                .await
                .map(|s| s.started_utc.timestamp_millis().to_string())
                .unwrap_or_default();
            let signature = format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                started_sig,
                player
                    .current_item
                    .as_ref()
                    .map(|item| item.id.as_str())
                    .unwrap_or(""),
                player.is_paused,
                player.effective_volume,
                player.repeat_mode.as_str(),
                voice_connected,
                player
                    .autoplay_preview
                    .as_ref()
                    .map(|item| item.id.as_str())
                    .unwrap_or(""),
                player
                    .upcoming
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if signature == last_signature {
                continue;
            }
            let queue_changed = last_signature
                .rsplit_once('|')
                .map(|(_, ids)| ids)
                .unwrap_or("")
                != signature.rsplit_once('|').map(|(_, ids)| ids).unwrap_or("");
            last_signature = signature;

            emit(
                &state,
                guild_id,
                "playback",
                playback_payload(&state, &player, position, &sampled_at, None, state.app.coordinator.schedule(guild_id).await),
            );
            if queue_changed {
                broadcast_queue(&state, guild_id).await;
            }
            emit_bare(&state, guild_id, "lyrics");
        }
        state.guild_watchers.lock().unwrap().remove(&guild_id);
    });
}

// ───────────────────────── 직렬화 도우미 ─────────────────────────

/// 모든 트랙 객체에 `durationSeconds` 숫자를 넣는다 —
/// `duration`은 C# TimeSpan 문자열이라 프런트가 신뢰할 수 없다(계약 §0).
// ───────────────────────── 무제한(0) · 투표 · 스킵 ─────────────────────────

/// 한 프레임에 싣는 대기열 최대 곡수 (V3 §18.2).
/// 뒤쪽은 `GET .../queue?offset=&limit=` 로 가져간다. 보통 사람은 앞 20곡만 보므로
/// 평소에는 추가 요청이 아예 안 일어난다.
const QUEUE_PAGE_MAX: usize = 200;
/// 통계·차트 응답 캐시 수명 (V3 §22.6). 통계는 60초 늦어도 아무도 손해 보지 않는다.
const STATS_CACHE_TTL: Duration = Duration::from_secs(60);
/// 투표 스킵이 아무도 안 누르면 저절로 닫히는 시간 (V3 §10.5).
const SKIP_VOTE_TTL: Duration = Duration::from_secs(90);
/// 우리 차트 한 장에 싣는 곡수 (V3 §15.2b).
const OURS_CHART_LIMIT: usize = 50;

/// **§23.1 규약: `0` 은 무제한이다.**
///
/// 예전 코드는 `max_queue_per_guild.max(1)` 처럼 클램프를 걸어서 `0` 이 조용히 `1` 이 됐다.
/// 그러면 화면에는 "무제한"이라고 뜨는데 서버는 한 곡만 받는 최악의 조합이 된다.
fn limit_blocks(limit: i32, would_be: usize) -> bool {
    as_limit(limit).is_some_and(|limit| would_be > limit as usize)
}

/// 숫자 설정 검증. **`0` 은 언제나 통과한다** — 무제한이라는 뜻이기 때문이다 (§23.1).
fn unlimited_or(value: i32, min: i32, max: i32) -> bool {
    value == 0 || (min..=max).contains(&value)
}

/// 길이 상한. `0` 이면 아무 곡이나 담을 수 있다 (§23.1).
fn track_too_long(max_seconds: i32, track: &TrackRef) -> bool {
    as_limit(max_seconds).is_some_and(|max| {
        track
            .duration
            .is_some_and(|duration| duration.as_secs_f64() > max as f64)
    })
}

/// 봇과 같은 음성 채널에 있는 사람들 (봇 제외). 투표 스킵의 `listeners` 모수다.
/// 봇이 음성에 없으면 언제나 빈 집합이다 (V3 §4).
fn listener_ids(state: &WebState, guild_id: u64) -> HashSet<u64> {
    let Some(channel) = bot_voice_status(state, guild_id).channel_id else {
        return HashSet::new();
    };
    let Some(cache) = state.app.discord_cache.get() else {
        return HashSet::new();
    };
    let Some(guild) = cache.guild(GuildId::new(guild_id)) else {
        return HashSet::new();
    };
    let bot_id = cache.current_user().id;
    guild
        .voice_states
        .iter()
        .filter(|(user_id, voice)| {
            **user_id != bot_id
                && voice.channel_id.map(|id| id.get()) == Some(channel)
                && !guild
                    .members
                    .get(user_id)
                    .map(|member| member.user.bot)
                    .unwrap_or(false)
        })
        .map(|(user_id, _)| user_id.get())
        .collect()
}

/// 투표 스킵 정족수 판정 (V3 §10.5). **순수 함수** — 테스트가 여기를 못 박는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SkipQuorum {
    have: usize,
    need: usize,
    /// 화면이 `듣는 사람 5명 중 3명이 동의하면 넘어가요` 를 쓸 때 필요한 **모수 크기**.
    /// 이걸 안 보내면 클라가 `need` 를 모수로 오해해 `3명 중 3명` 같은 거짓말을 한다 (V3 §10.5).
    pool: usize,
    passed: bool,
}

/// `have`/`need` 는 화면에 그대로 찍히는 숫자다.
/// `either` 는 둘 중 **더 가까운 쪽**을, `both` 는 **더 먼 쪽**을 보여 준다 —
/// 사람이 보는 "몇 표 남았나"가 실제 통과 조건과 어긋나면 안 되기 때문이다.
/// 모수가 0인 쪽은 `both` 에서 만족한 것으로 친다(아무도 없는 조건 때문에 영영 안 넘어가면 곤란하다).
fn skip_quorum(
    listeners: &HashSet<u64>,
    viewers: &HashSet<u64>,
    voters: &HashSet<u64>,
    basis: VoteSkipBasis,
    ratio: u32,
    min_votes: u32,
) -> SkipQuorum {
    let have_listeners = voters.intersection(listeners).count();
    let have_viewers = voters.intersection(viewers).count();
    let need_listeners =
        VoteSkipBasis::votes_needed(listeners.len() as u32, ratio, min_votes) as usize;
    let need_viewers = VoteSkipBasis::votes_needed(viewers.len() as u32, ratio, min_votes) as usize;
    let listeners_ok = listeners.is_empty() || have_listeners >= need_listeners;
    let viewers_ok = viewers.is_empty() || have_viewers >= need_viewers;

    match basis {
        VoteSkipBasis::Listeners => SkipQuorum {
            have: have_listeners,
            need: need_listeners,
            pool: listeners.len(),
            passed: need_listeners > 0 && have_listeners >= need_listeners,
        },
        VoteSkipBasis::Viewers => SkipQuorum {
            have: have_viewers,
            need: need_viewers,
            pool: viewers.len(),
            passed: need_viewers > 0 && have_viewers >= need_viewers,
        },
        VoteSkipBasis::Either => {
            let left = need_listeners.saturating_sub(have_listeners);
            let right = need_viewers.saturating_sub(have_viewers);
            let take_listeners = need_listeners > 0 && (need_viewers == 0 || left <= right);
            SkipQuorum {
                have: if take_listeners { have_listeners } else { have_viewers },
                need: if take_listeners { need_listeners } else { need_viewers },
                pool: if take_listeners { listeners.len() } else { viewers.len() },
                passed: (need_listeners > 0 && have_listeners >= need_listeners)
                    || (need_viewers > 0 && have_viewers >= need_viewers),
            }
        }
        VoteSkipBasis::Both => {
            let take_listeners = need_listeners.saturating_sub(have_listeners)
                >= need_viewers.saturating_sub(have_viewers);
            SkipQuorum {
                have: if take_listeners { have_listeners } else { have_viewers },
                need: if take_listeners { need_listeners } else { need_viewers },
                pool: if take_listeners { listeners.len() } else { viewers.len() },
                passed: listeners_ok && viewers_ok && (need_listeners > 0 || need_viewers > 0),
            }
        }
    }
}

/// 진행 중인 투표 스킵 하나. **메모리에만** 산다 (V3 §10.5) — 곡 하나 수명짜리 데이터다.
#[derive(Debug, Clone)]
pub struct SkipVoteState {
    /// 어느 곡에 대한 투표인지. 곡이 바뀌면 표가 넘어가면 안 되므로 이걸로 리셋한다.
    item_id: String,
    voters: HashSet<u64>,
    opened: Instant,
}

impl SkipVoteState {
    fn new(item_id: String) -> Self {
        Self {
            item_id,
            voters: HashSet::new(),
            opened: Instant::now(),
        }
    }

    /// 90초가 지났으면 실패한 투표다. 화면에 계속 붙어 있으면 지저분하다.
    pub fn is_expired(&self) -> bool {
        self.opened.elapsed() >= SKIP_VOTE_TTL
    }

    fn expires_utc(&self) -> String {
        let left = SKIP_VOTE_TTL.saturating_sub(self.opened.elapsed());
        (chrono::Utc::now() + chrono::Duration::milliseconds(left.as_millis() as i64)).to_rfc3339()
    }
}

/// `/state/hot` 의 `skipVote`. 진행 중인 투표가 없으면 `null` 이다.
///
/// **곡이 바뀌면 리셋된다** — 저장된 `item_id` 가 지금 곡과 다르면 없는 것으로 본다.
/// 90초가 지난 투표도 마찬가지다.
fn skip_vote_json(
    state: &WebState,
    ctx: &AuthContext,
    player: &crate::models::GuildPlayerState,
) -> Value {
    let guild_id = ctx.guild_id();
    if !ctx.settings.vote_skip_enabled {
        return Value::Null;
    }
    let Some(current_id) = player.current_item.as_ref().map(|item| item.id.clone()) else {
        return Value::Null;
    };
    let votes = state.skip_votes.lock().unwrap();
    let Some(vote) = votes.get(&guild_id) else {
        return Value::Null;
    };
    if vote.item_id != current_id || vote.is_expired() {
        return Value::Null;
    }
    let listeners = listener_ids(state, guild_id);
    let viewers: HashSet<u64> = viewers_of(state, guild_id).into_iter().collect();
    let quorum = skip_quorum(
        &listeners,
        &viewers,
        &vote.voters,
        ctx.settings.vote_skip_basis,
        ctx.settings.vote_skip_ratio,
        ctx.settings.vote_skip_min,
    );
    json!({
        "have": quorum.have,
        "need": quorum.need,
        // 모수 크기를 같이 보낸다 — 없으면 클라가 `need` 를 모수로 써서 툴팁이 거짓말을 한다 (§10.5).
        "pool": quorum.pool,
        "mine": vote.voters.contains(&ctx.user_id()),
        "basis": ctx.settings.vote_skip_basis.as_str(),
        "basisLabel": ctx.settings.vote_skip_basis.description(),
        "expiresUtc": vote.expires_utc(),
    })
}

/// 스킵 투표 상황을 **사람마다 맞는 `mine` 으로** 내보낸다 (V3 §10.5).
///
/// 예전에는 누른 사람 기준의 `mine` 을 길드 전체로 뿌렸다. 그러면 A가 누른 순간
/// B·C 화면도 "내 표가 들어가 있어요"가 되고, B가 취소를 누르면 A의 표가 빠져서
/// 정족수에 영영 도달하지 못했다. `mine` 은 개인화 값이므로 수신자 필터를 쓴다.
///
/// 뿌리는 순서가 중요하다 — 클라는 `skipVote` 를 통째로 갈아끼우므로,
/// 먼저 `mine:false` 전체 프레임을 보내고 그 뒤에 투표자별 `mine:true` 를 덮어씌운다.
fn emit_skip_vote(state: &WebState, guild_id: u64, base: &Value, voters: &HashSet<u64>) {
    let mut shared = base.clone();
    if let Some(map) = shared.as_object_mut() {
        map.insert("mine".into(), Value::Bool(false));
    }
    emit(state, guild_id, "skipvote", shared);
    for voter in voters {
        let mut personal = base.clone();
        if let Some(map) = personal.as_object_mut() {
            map.insert("mine".into(), Value::Bool(true));
        }
        emit_to(state, guild_id, *voter, "skipvote", personal);
    }
}

// ───────── 슈퍼 좋아요 제한 (V3 §10.6) ─────────
//
// 판정·소비·환불·현황은 전부 저장소(`remote_super_like_usage`)가 한다.
// **하루 횟수는 재시작해도 살아남아야** 해서 메모리가 아니라 DB다.
// 쿨타임만 저장소 안의 메모리다 — 짧고, 풀려도 손해가 없다.

/// `/state/cold` 와 투표 응답에 싣는 현황. 회색으로만 두면 고장인 줄 아니까
/// 남은 횟수와 쿨타임 끝나는 시각을 숫자로 그대로 준다.
fn super_like_status(
    state: &WebState,
    guild_id: u64,
    user_id: u64,
    settings: &RemoteGuildSettings,
) -> SuperLikeStatus {
    state.app.remote.super_like_status(
        guild_id,
        user_id,
        settings.super_like_cooldown_sec,
        settings.super_like_daily_limit,
    )
}

/// `/vote` 의 `kind` 문자열 → 투표 종류. `null`·`""`·`"none"` 은 취소다.
fn parse_vote_kind(value: &str) -> Option<QueueVoteKind> {
    match value {
        "like" => Some(QueueVoteKind::Like),
        "superLike" | "superlike" => Some(QueueVoteKind::SuperLike),
        "dislike" => Some(QueueVoteKind::Dislike),
        _ => None,
    }
}

/// 투표자 ID 목록을 문자열 배열로. **최대 12명**까지만 (V3 §10.4) —
/// 대기열 50곡 × 투표자 전원을 실으면 payload 가 터진다. 나머지는 개수로 `+5명` 처럼 보여 준다.
fn voter_ids_json(ids: &[u64]) -> Value {
    json!(
        ids.iter()
            .take(MAX_VOTER_IDS)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
    )
}

fn track_json(track: &TrackRef) -> Value {
    json!({
        "title": track.title.clone().unwrap_or_else(|| track.content_id.clone()),
        "artist": track.artist,
        "provider": track.provider,
        "contentId": track.content_id,
        "sourceUrl": track.source_url,
        "cacheKey": track.cache_key(),
        "durationSeconds": track.duration.map(|duration| duration.as_secs_f64()),
        "durationLabel": track.duration.map(|duration| duration.display()),
        "artUrl": Value::Null,
    })
}

fn repeat_key(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::Off => "off",
        RepeatMode::Track => "track",
        RepeatMode::Queue => "queue",
    }
}

fn parse_repeat(value: &str) -> Option<RepeatMode> {
    match value {
        "off" => Some(RepeatMode::Off),
        "track" => Some(RepeatMode::Track),
        "queue" => Some(RepeatMode::Queue),
        _ => None,
    }
}

fn queue_item_json(
    item: &QueueItem,
    score: &QueueScore,
    viewer_user_id: u64,
    my_vote: Option<QueueVoteKind>,
    points: &VotePoints,
) -> Value {
    json!({
        "id": item.id,
        "track": track_json(&item.track),
        "requestedByDisplay": item.requested_by_display,
        "requestedByUserId": item.requested_by_user_id.map(|id| id.to_string()),
        "isMine": item.requested_by_user_id == Some(viewer_user_id),
        "myVote": my_vote.map(QueueVoteKind::api_key),
        "round": score.round,
        "score": {
            "waitScore": score.wait_score,
            "likeCount": score.like_count,
            "superLikeCount": score.super_like_count,
            "dislikeCount": score.dislike_count,
            "manualPriority": score.manual_priority,
            // 설정된 점수로 계산한다 (V3 §10.1). 화면의 계산식이 이 숫자와 같아야 한다.
            "totalScore": score.total_score(points),
            // 계산식도 서버가 만들어 준다 — 클라이언트가 배수를 다시 곱하면
            // 점수 설정을 바꿨을 때 화면이 거짓말을 한다 (V3 §10.4).
            "formula": score.formula(points),
            // 누가 눌렀는지 (V3 §10.4). 이름이 아니라 ID다 — 클라이언트가 `members` 로 붙인다.
            "likeBy": voter_ids_json(&score.like_by),
            "superBy": voter_ids_json(&score.super_by),
            "dislikeBy": voter_ids_json(&score.dislike_by),
        }
    })
}

/// 지금 나오는 곡 (V3 §10.4).
///
/// **점수·투표자를 같이 싣는다.** 사람들이 제일 궁금해하는 곡이 바로 이 곡인데,
/// 예전에는 `score` 가 없어서 재생 카드의 투표자 줄이 영영 `hidden` 이었다.
/// 점수 행은 곡이 끝나 다음으로 넘어갈 때(`clear_item_runtime`) 지워지므로,
/// 재생 중에는 대기열에 있던 그대로가 남아 있다.
/// 지금 나오는 곡. `viewer` 가 있으면 그 사람 기준의 개인화 필드를 채운다.
///
/// **브로드캐스트에서는 반드시 `None` 이다.** 이 프레임은 모두가 같이 받으므로,
/// 개인화된 값을 실으면 남의 화면이 내 투표를 자기 것으로 착각한다(§10.5에서 큐로 이미 겪음).
/// 클라이언트는 `null` 을 "서버가 안 보냄"으로 읽고 자기 값을 지킨다.
///
/// 투표자 목록으로 클라이언트가 직접 계산하게 두지 않는다 — 그 목록은 12명에서 잘려서
/// 13번째 사람부터는 자기 표가 안 눌린 것처럼 보인다.
fn current_json(
    state: &WebState,
    guild_id: u64,
    item: &QueueItem,
    points: &VotePoints,
    viewer: Option<u64>,
) -> Value {
    let score = state
        .app
        .remote
        .queue_scores(guild_id)
        .get(&item.id)
        .cloned();
    let my_vote = viewer.and_then(|user_id| state.app.remote.user_vote(&item.id, user_id));
    json!({
        "id": item.id,
        "track": track_json(&item.track),
        "durationSeconds": item.track.duration.map(|duration| duration.as_secs_f64()),
        "requestedByDisplay": item.requested_by_display,
        "requestedByUserId": item.requested_by_user_id.map(|id| id.to_string()),
        // 재생 중인 곡에도 투표할 수 있다 (§10.7). 판정과 표시는 대기열 곡과 똑같다.
        "isMine": viewer.map(|user_id| item.requested_by_user_id == Some(user_id)),
        "myVote": my_vote.map(QueueVoteKind::api_key),
        "score": score.map(|score| json!({
            "waitScore": score.wait_score,
            "likeCount": score.like_count,
            "superLikeCount": score.super_like_count,
            "dislikeCount": score.dislike_count,
            "manualPriority": score.manual_priority,
            "totalScore": score.total_score(points),
            "formula": score.formula(points),
            "likeBy": voter_ids_json(&score.like_by),
            "superBy": voter_ids_json(&score.super_by),
            "dislikeBy": voter_ids_json(&score.dislike_by),
        })),
    })
}

/// `viewer` 는 이 payload 를 **한 사람에게만** 보낼 때만 채운다.
/// 브로드캐스트에는 `None` 이어야 한다 — 안 그러면 남의 투표 상태가 내 화면에 붙는다.
fn playback_payload(
    state: &WebState,
    player: &crate::models::GuildPlayerState,
    position: f64,
    sampled_at: &str,
    viewer: Option<u64>,
    schedule: Option<crate::player::coordinator::TrackSchedule>,
) -> Value {
    let state_ref = state;
    let tune = state.app.remote.load_guild_settings(player.guild_id);
    // 다음 곡이 시작될 시각. 웹이 **미리 준비해 두었다가 그 순간에 바꿔 틀** 수 있어야
    // 곡 사이가 안 끊긴다. 길이를 모르면 예고할 수 없으므로 `null` 이다.
    let next_start_utc = schedule.and_then(|s| {
        player
            .current_item
            .as_ref()
            .and_then(|item| item.track.duration)
            .map(|duration| {
                (s.started_utc
                    + chrono::Duration::milliseconds((duration.as_secs_f64() * 1000.0) as i64))
                .to_rfc3339()
            })
    });
    let current_points = state
        .app
        .remote
        .load_guild_settings(player.guild_id)
        .vote_points();
    json!({
        "isPaused": player.is_paused,
        "positionSeconds": position,
        "sampledAtUtc": sampled_at,
        // **절대 시각 일정** (§31). 클라이언트는 `now - startedUtc` 로 위치를 잡는다.
        // 전송 지연을 각자 추정하던 방식은 기기마다 결과가 달라 곡마다 미세하게 어긋났다.
        // `startedUtc` 가 미래면 아직 시작 전이라는 뜻이다(스킵 직후 등).
        "startedUtc": schedule.map(|s| s.started_utc.to_rfc3339()),
        "nextStartUtc": next_start_utc,
        "skipLeadMs": tune.skip_lead_ms,
        "seekLockoutMs": tune.seek_lockout_ms,
        "webSyncOffsetMs": tune.web_sync_offset_ms,
        "currentId": player.current_item.as_ref().map(|item| item.id.clone()),
        "current": player
            .current_item
            .as_ref()
            .map(|item| current_json(state_ref, player.guild_id, item, &current_points, viewer)),
        "durationSeconds": player
            .current_item
            .as_ref()
            .and_then(|item| item.track.duration)
            .map(|duration| duration.as_secs_f64()),
        "effectiveVolume": player.effective_volume,
        "repeatMode": repeat_key(player.repeat_mode),
        "shuffleEnabled": player.shuffle_enabled,
        "autoplayEnabled": player.autoplay_enabled,
        // V3 §16 B1 — 저장값이 아니라 캐시가 진실이다.
        "voiceChannelId": bot_voice_status(state, player.guild_id)
            .channel_id
            .map(|id| id.to_string()),
        "voiceConnected": bot_voice_status(state, player.guild_id).in_voice(),
        // **"재생이 흐르는가" 는 서버가 판정한다** (§36 의 기준을 고쳐 잡은 것).
        //
        // 예전에는 화면이 `voiceConnected` 로 파생했다. 그 판단의 의도는 옳았지만
        // (봇이 빠졌는데 진행바만 혼자 가던 문제) 기준이 틀렸다 — 물어야 할 것은
        // *봇이 음성에 있는가* 가 아니라 *시각표가 도는가* 다. 웹 재생기 모드에서는
        // 봇이 없어도 시각표가 돌고, 그때 화면이 멈춰 버리면 안 된다.
        //
        // 지금은 값이 예전과 **정확히 같다** — 물리 세션이 없으면 시각표도 없기 때문이다.
        "stopped": schedule.is_none() || player.current_item.is_none(),
        "botOnline": bot_in_guild(state, player.guild_id),
        // 곡이 바뀌면 다음 곡도 같이 바뀐다 — 같은 프레임에 실어야 화면이 한 번만 움직인다 (V3 §14).
        "next": next_up_json(player),
    })
}

/// 다음에 나올 곡 (V3 §14). **새 계산이 전혀 없다** — 둘 다 이미 메모리에 있다.
///
/// 대기열에 곡이 있으면 그게 다음이고, 비었으면 자동 재생이 미리 뽑아 둔 후보다.
/// 자동 추천은 "대기열이 비면 나올 곡"이라 확정이 아니다. 화면이 단정하지 않도록
/// `source` 를 같이 내려 준다.
fn next_up_json(player: &crate::models::GuildPlayerState) -> Value {
    if let Some(item) = player.upcoming.first() {
        return json!({ "source": "queue", "item": next_item_json(item) });
    }
    match player.autoplay_preview.as_ref() {
        Some(item) if player.autoplay_enabled => {
            json!({ "source": "autoplay", "item": next_item_json(item) })
        }
        _ => json!({ "source": Value::Null, "item": Value::Null }),
    }
}

fn next_item_json(item: &QueueItem) -> Value {
    json!({
        "id": item.id,
        "track": track_json(&item.track),
        "requestedByDisplay": item.requested_by_display,
        "requestedByUserId": item.requested_by_user_id.map(|id| id.to_string()),
    })
}

/// 5초 재정렬 루프(`app::queue_sort_loop`)가 순서를 바꿨을 때 부르는 훅.
/// 루프는 동기 콜백만 받을 수 있어서 여기서 태스크로 넘긴다.
/// 보는 사람이 없으면 프레임을 만들지 않는다 — 유휴 시 쿼리 0회 계약(§5.2 H).
pub fn spawn_queue_broadcast(state: &Arc<WebState>, guild_id: u64) {
    if viewer_count(state, guild_id) == 0 {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        broadcast_queue(&state, guild_id).await;
    });
}

/// 대기열 전체를 한 프레임으로 밀어 준다. 클라이언트는 FLIP으로 자리를 옮긴다.
async fn broadcast_queue(state: &Arc<WebState>, guild_id: u64) {
    let player = state.app.player.get_state(guild_id).await;
    let settings = state.app.remote.load_guild_settings(guild_id);
    let mut scores = state.app.remote.queue_scores(guild_id);
    ranking::apply_rounds(&player.upcoming, &mut scores);
    let points = settings.vote_points();
    // 이 프레임은 모든 구독자가 같이 받는다 — 개인화 필드(isMine/myVote)는 넣지 않는다.
    //
    // **앞 200곡만 싣는다** (V3 §18.2). 5000곡짜리 대기열을 5초마다 접속자 전원에게
    // 통째로 밀면 프레임 하나가 수 MB 다. 뒤쪽은 `GET .../queue?offset=` 로 가져간다.
    let items: Vec<Value> = player
        .upcoming
        .iter()
        .take(QUEUE_PAGE_MAX)
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            let mut value = queue_item_json(item, &score, 0, None, &points);
            value["isMine"] = Value::Null;
            value["myVote"] = Value::Null;
            value
        })
        .collect();
    let (sorted_at, next_sort_at, sort_period) = sort_clock(state, guild_id, player.upcoming.len());
    emit(
        state,
        guild_id,
        "queue.set",
        json!({
            "items": items,
            "queueTotal": player.upcoming.len(),
            "queueTruncated": player.upcoming.len() > QUEUE_PAGE_MAX,
            "mode": settings.sort_mode.as_str(),
            "sortedAt": sorted_at,
            // 카운트다운 기준(V3 §5). 클라 타이머만 쓰면 백그라운드 탭에서 어긋난다.
            "nextSortAt": next_sort_at,
            "sortPeriodSeconds": sort_period,
            // 재정렬이 돌면 **다음 곡도 같이 바뀐다** (V3 §14.4). 같은 프레임에 실어야
            // 카운트다운이 0이 되는 순간과 다음 곡 줄이 한 번에 움직여 인과가 보인다.
            // 이게 없으면 길드 감시 태스크가 최대 2초 뒤에 `playback` 으로 따라잡는다.
            "next": next_up_json(&player),
        }),
    );
}

// ───────────────────────── 페이지 셸 ─────────────────────────

/// 페이지 셸은 절대 캐시하지 않는다.
///
/// 셸에는 CSRF 토큰과 로그인한 사람의 정보, 그리고 현재 에셋 버전이 박혀 있다.
/// 브라우저가 휴리스틱으로 캐시하면 (1) 남의 화면이 뜰 수 있고
/// (2) 배포해도 옛 `?v=` 를 계속 요청해 새 CSS/JS 를 영원히 못 받는다.
fn html_page(body: String) -> Response {
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    response
}

/// 로그인 후 돌아갈 내부 경로만 통과시킨다.
///
/// `//evil.com` 이나 `https://…` 같은 값이 그대로 리다이렉트에 쓰이면 오픈 리다이렉트가 된다.
/// 그래서 `/music/` 로 시작하고 스킴·호스트가 없는 경로만 받는다.
fn safe_next(value: Option<&str>) -> Option<String> {
    let raw = value?.trim();
    if !raw.starts_with("/music/") || raw.starts_with("//") || raw.contains("://") {
        return None;
    }
    if raw.contains(['\\', '\n', '\r']) {
        return None;
    }
    Some(raw.to_string())
}

async fn login_page(
    State(state): State<Arc<WebState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let auth = auth_config(&state);
    html_page(remote_page::login(
        auth.configured(),
        auth.dev_login,
        query.get("error").map(String::as_str),
        safe_next(query.get("next").map(String::as_str)).as_deref(),
    ))
}

async fn portal_home(State(state): State<Arc<WebState>>, cookies: Cookies) -> Response {
    let Some(session) = current_session(&state, &cookies) else {
        return Redirect::to("/music/login").into_response();
    };
    let guilds: Vec<OAuthGuild> = session
        .guilds
        .iter()
        .filter(|guild| session.is_developer || bot_in_guild(&state, guild.id))
        .cloned()
        .collect();
    // 고를 게 하나뿐이면 고르라고 묻지 않는다. 바로 그 서버의 리모컨으로 보낸다.
    if let [only] = guilds.as_slice() {
        return Redirect::to(&format!("/music/guilds/{}", only.id)).into_response();
    }
    html_page(remote_page::guild_selector(&session, &guilds))
}

async fn guild_page(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => html_page(remote_page::guild(
            &ctx.session,
            &ctx.guild,
            &state.app.build_id,
            ctx.tier,
        )),
        // 로그인만 안 된 경우에는 이 서버로 되돌아오도록 next 를 달아 준다.
        Err(response) => page_error_returning_to(response, &format!("/music/guilds/{guild_id}")),
    }
}

async fn admin_page(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return page_error(response),
    };
    if !ctx.tier.is_manager() {
        let mut response = html_page(remote_page::denied(
            "서버 관리 콘솔은 관리자만 들어올 수 있어요.",
            guild_id,
        ));
        *response.status_mut() = StatusCode::FORBIDDEN;
        return response;
    }
    let intent = state
        .app
        .intent_status
        .read()
        .map(|status| json!({ "members": status.members, "presences": status.presences, "voiceStates": true }))
        .unwrap_or_else(|_| json!({ "members": true, "presences": true, "voiceStates": true }));
    html_page(remote_page::admin(
        &ctx.session,
        &ctx.guild,
        &state.app.build_id,
        ctx.tier,
        &intent,
    ))
}

/// API 오류 Response를 페이지 문맥에 맞게 바꾼다 (401이면 로그인으로).
fn page_error(response: Response) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Redirect::to("/music/login").into_response();
    }
    response
}

/// 로그인이 필요해서 막힌 경우에만 `next` 를 달아 로그인 화면으로 보낸다.
/// 권한 부족(403)은 로그인해도 안 풀리므로 그대로 돌려준다.
fn page_error_returning_to(response: Response, next: &str) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Redirect::to(&format!("/music/login?next={}", percent_encode(next))).into_response();
    }
    response
}

// ───────────────────────── OAuth ─────────────────────────

async fn oauth_start(
    State(state): State<Arc<WebState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let auth = auth_config(&state);
    let Some(client_id) = auth.client_id.as_deref() else {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요해요").into_response();
    };
    if !auth.has_client_secret() {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요해요").into_response();
    }
    let oauth_state = crate::models::uuid_like();
    {
        let mut states = state.oauth_states.lock().unwrap();
        // S8: 발급할 때마다 만료분을 함께 걷어낸다(주기 스위퍼와 이중 방어).
        states.retain(|_, (issued, _)| issued.elapsed() < OAUTH_STATE_TTL);
        states.insert(
            oauth_state.clone(),
            (
                Instant::now(),
                safe_next(query.get("next").map(String::as_str)),
            ),
        );
    }
    let url = format!(
        "https://discord.com/oauth2/authorize?response_type=code&client_id={}&scope={}&state={}&redirect_uri={}&prompt=consent",
        percent_encode(client_id),
        percent_encode("identify guilds guilds.members.read"),
        percent_encode(&oauth_state),
        percent_encode(&auth.redirect_uri()),
    );
    Redirect::temporary(&url).into_response()
}

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscordUserResponse {
    id: String,
    username: String,
    global_name: Option<String>,
    avatar: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscordGuildResponse {
    id: String,
    name: String,
    icon: Option<String>,
    #[serde(default)]
    owner: bool,
    #[serde(default)]
    permissions: String,
}

async fn oauth_callback(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    headers: HeaderMap,
    Query(query): Query<OAuthCallbackQuery>,
) -> Response {
    let auth = auth_config(&state);
    if let Some(error) = query.error {
        return html_page(remote_page::login(
            auth.configured(),
            auth.dev_login,
            Some(&format!("Discord 로그인이 취소됐어요: {error}")),
            None,
        ));
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return Redirect::to("/music/login?error=OAuth%20응답이%20올바르지%20않습니다")
            .into_response();
    };
    let issued = state.oauth_states.lock().unwrap().remove(&returned_state);
    let Some((issued_at, next_path)) = issued else {
        return Redirect::to("/music/login?error=OAuth%20state가%20만료되었습니다").into_response();
    };
    if issued_at.elapsed() >= OAUTH_STATE_TTL {
        return Redirect::to("/music/login?error=OAuth%20state가%20만료되었습니다").into_response();
    }
    let (Some(client_id), Some(client_secret)) = (auth.client_id.clone(), auth.client_secret.clone())
    else {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요해요").into_response();
    };
    let client = http_client(&state);
    let token = match client
        .post("https://discord.com/api/v10/oauth2/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", auth.redirect_uri()),
        ])
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            match response.json::<OAuthTokenResponse>().await {
                Ok(token) => token,
                Err(error) => {
                    state
                        .app
                        .log
                        .error("RemoteAuth", &format!("OAuth token parse failed: {error}"));
                    return Redirect::to("/music/login?error=OAuth%20토큰%20해석에%20실패했습니다")
                        .into_response();
                }
            }
        }
        Ok(response) => {
            state.app.log.warn(
                "RemoteAuth",
                &format!("OAuth token exchange rejected: {}", response.status()),
            );
            return Redirect::to("/music/login?error=Discord%20OAuth%20인증에%20실패했습니다")
                .into_response();
        }
        Err(error) => {
            state.app.log.error(
                "RemoteAuth",
                &format!("OAuth token request failed: {error}"),
            );
            return Redirect::to("/music/login?error=Discord%20연결에%20실패했습니다")
                .into_response();
        }
    };
    let user = match discord_get::<DiscordUserResponse>(&client, &token.access_token, "/users/@me")
        .await
    {
        Ok(user) => user,
        Err(error) => {
            return Redirect::temporary(&format!("/music/login?error={}", percent_encode(&error)))
                .into_response();
        }
    };
    let guild_rows = match discord_get::<Vec<DiscordGuildResponse>>(
        &client,
        &token.access_token,
        "/users/@me/guilds",
    )
    .await
    {
        Ok(guilds) => guilds,
        Err(error) => {
            return Redirect::temporary(&format!("/music/login?error={}", percent_encode(&error)))
                .into_response();
        }
    };
    let Ok(user_id) = user.id.parse::<u64>() else {
        return Redirect::to("/music/login?error=Discord%20사용자%20ID가%20올바르지%20않습니다")
            .into_response();
    };
    let avatar_url = user.avatar.as_ref().map(|avatar| {
        format!("https://cdn.discordapp.com/avatars/{user_id}/{avatar}.png?size=128")
    });
    let guilds = to_oauth_guilds(guild_rows);
    begin_remote_session(
        &state,
        &cookies,
        Some(&headers),
        RemoteSession {
            user_id,
            username: user.username.clone(),
            display_name: user.global_name.unwrap_or(user.username),
            avatar_url,
            guilds,
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            csrf_token: crate::models::uuid_like(),
            created: Instant::now(),
            token_expires: Instant::now()
                + Duration::from_secs(token.expires_in.saturating_sub(60).max(60)),
            is_developer: false,
        },
    );
    // 어느 서버의 리모컨을 열려다 로그인한 거면 그 서버로 바로 돌려보낸다.
    // 그런 맥락이 없으면 /music 이 알아서 서버를 고르거나(여러 개) 바로 넘긴다(하나).
    Redirect::to(next_path.as_deref().unwrap_or("/music")).into_response()
}

async fn discord_get<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    access_token: &str,
    path: &str,
) -> Result<T, String> {
    let response = client
        .get(format!("https://discord.com/api/v10{path}"))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|error| format!("Discord API 연결 실패: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Discord API가 요청을 거부했어요 ({})",
            response.status()
        ));
    }
    response
        .json::<T>()
        .await
        .map_err(|error| format!("Discord API 응답 해석 실패: {error}"))
}

#[derive(Debug, Deserialize, Default)]
struct DevLoginForm {
    #[serde(default)]
    next: Option<String>,
}

async fn dev_login(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Form(form): Form<DevLoginForm>,
) -> Response {
    if !auth_config(&state).dev_login || !address.ip().is_loopback() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let next_path = safe_next(form.next.as_deref());
    let metadata = state.app.db.list_guild_metadata();
    let mut guilds: Vec<OAuthGuild> = metadata
        .into_iter()
        .map(|guild| OAuthGuild {
            id: guild.guild_id,
            name: guild.name,
            icon: None,
            owner: true,
            permissions: ADMINISTRATOR_PERMISSION,
        })
        .collect();
    if guilds.is_empty() {
        guilds.push(OAuthGuild {
            id: state.app.config.register_guild_id.unwrap_or(1),
            name: "마참뮤직 UI 검증 서버".into(),
            icon: None,
            owner: true,
            permissions: ADMINISTRATOR_PERMISSION,
        });
    }
    let user_id = std::env::var("MUSICBOT_DEV_USER_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|id| *id != 0)
        .unwrap_or_else(|| state.app.config.bot_owner_user_id.max(1));
    if std::env::var("MUSICBOT_DEV_SEED").ok().as_deref() == Some("1") {
        if let Some(guild) = guilds.first() {
            seed_dev_guild(&state, guild.id, user_id).await;
        }
    }
    begin_remote_session(
        &state,
        &cookies,
        None,
        RemoteSession {
            user_id,
            username: "local-tester".into(),
            display_name: "로컬 검증자".into(),
            avatar_url: None,
            guilds,
            access_token: String::new(),
            refresh_token: None,
            csrf_token: crate::models::uuid_like(),
            created: Instant::now(),
            token_expires: Instant::now() + REMOTE_SESSION_TTL,
            is_developer: true,
        },
    );
    Redirect::to(next_path.as_deref().unwrap_or("/music")).into_response()
}

#[derive(Debug, Deserialize)]
struct LogoutForm {
    csrf: String,
}

async fn remote_logout(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Form(form): Form<LogoutForm>,
) -> Response {
    let Some(session) = current_session(&state, &cookies) else {
        return Redirect::to("/music/login").into_response();
    };
    if !constant_time_eq(&form.csrf, &session.csrf_token) {
        return json_error(StatusCode::FORBIDDEN, "CSRF 검증에 실패했어요.");
    }
    end_remote_session(&state, &cookies);
    Redirect::to("/music/login").into_response()
}

// ───────────────────────── 개인 설정 (V3 §2) ─────────────────────────
//
// 화면 배치·테마는 **길드가 아니라 사람**에 붙는다. 서버마다 다른 배치를 쓰고 싶은
// 사람은 없으니 길드 인가를 태우지 않고 세션만 본다. 대신 변경은 CSRF를 검사한다.

/// 길드 없이 세션만 확인한다. `headers`가 있으면 CSRF도 같이 본다(변경 요청).
fn session_only(
    state: &Arc<WebState>,
    cookies: &Cookies,
    headers: Option<&HeaderMap>,
) -> Result<RemoteSession, Response> {
    let session = current_session(state, cookies)
        .ok_or_else(|| json_error(StatusCode::UNAUTHORIZED, "Discord 로그인이 필요해요."))?;
    if let Some(headers) = headers {
        if !verify_csrf(&session, headers) {
            return Err(json_error(StatusCode::FORBIDDEN, "CSRF 검증에 실패했어요."));
        }
    }
    Ok(session)
}

/// 저장된 개인 설정. **기본값을 채우지 않는다** — `layout`이 없다는 사실 자체가
/// "아직 한 번도 안 골랐다"는 신호라서, 첫 진입 배치 선택 시트가 그걸 보고 뜬다(V3 §3).
fn prefs_json(state: &WebState, user_id: u64) -> Value {
    Value::Object(
        state
            .app
            .remote
            .load_prefs(user_id)
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

/// 부분 갱신 요청을 `(저장할 것, 지울 것)`으로 가른다.
///
/// 숫자·불리언으로 와도 문자열로 바꿔 받는다(`webVolume: 60`, `lyricsOpen: true`).
/// 모르는 키나 범위를 벗어난 값은 **조용히 버리지 않고 400**으로 돌려준다 —
/// 화면에서 통과한 값이 서버에서 사라지면 아무도 원인을 못 찾는다.
fn parse_pref_patch(
    object: &serde_json::Map<String, Value>,
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut updates: BTreeMap<String, String> = BTreeMap::new();
    let mut removals: Vec<String> = Vec::new();
    for (key, value) in object {
        let text = match value {
            // null = "기본으로 되돌리기". 지운 키는 다시 미선택 상태가 된다.
            Value::Null => {
                removals.push(key.clone());
                continue;
            }
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => (if *flag { "1" } else { "0" }).to_string(),
            _ => return Err(format!("{key}: 값이 문자열이나 숫자여야 해요.")),
        };
        if !is_valid_pref(key, &text) {
            return Err(format!("{key}: 저장할 수 없는 값이에요 ({text})."));
        }
        updates.insert(key.clone(), text);
    }
    Ok((updates, removals))
}

async fn api_prefs_get(State(state): State<Arc<WebState>>, cookies: Cookies) -> Response {
    let session = match session_only(&state, &cookies, None) {
        Ok(session) => session,
        Err(response) => return response,
    };
    json_ok(json!({ "prefs": prefs_json(&state, session.user_id) }))
}

/// 부분 갱신. `null`을 보내면 그 키를 지운다("기본으로 되돌리기").
/// 값 검증은 저장소와 **같은** `is_valid_pref`를 쓴다 — 화면에서 통과한 값이
/// 서버에서 조용히 사라지면 원인을 못 찾는다.
async fn api_prefs_put(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let session = match session_only(&state, &cookies, Some(&headers)) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(object) = body.as_object() else {
        return json_error(StatusCode::BAD_REQUEST, "개인 설정은 객체로 보내 주세요.");
    };
    if object.len() > 20 {
        return json_error(
            StatusCode::BAD_REQUEST,
            "한 번에 바꿀 수 있는 항목은 20개까지예요.",
        );
    }
    if rate_limited(
        &state,
        0,
        session.user_id,
        "prefs",
        Duration::from_millis(200),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "설정 저장이 너무 잦아요. 드래그가 끝난 뒤 한 번만 보내 주세요.",
        );
    }

    let (updates, removals) = match parse_pref_patch(object) {
        Ok(parsed) => parsed,
        Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
    };
    if let Err(error) = state.app.remote.save_prefs(session.user_id, &updates) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if !removals.is_empty() {
        let keys: Vec<&str> = removals.iter().map(String::as_str).collect();
        if let Err(error) = state.app.remote.delete_prefs(session.user_id, &keys) {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
        }
    }
    json_ok(json!({ "ok": true, "prefs": prefs_json(&state, session.user_id) }))
}

// ───────────────────────── 상태 조회 (hot / cold) ─────────────────────────

/// `GET /state/hot` — 진입 시 1회 + WS 재연결 시에만. 재생·대기열·접속 요약.
async fn api_state_hot(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let player = state.app.player.get_state(guild_id).await;
    // sampledAtUtc는 positionSeconds를 읽은 **직후**에 찍는다 (계약 §2).
    let position = state
        .app
        .coordinator
        .current_position(guild_id)
        .await
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_else(|| {
            player
                .current_item
                .as_ref()
                .map(|item| item.start_offset.as_secs_f64())
                .unwrap_or(0.0)
        });
    let sampled_at = now_utc();

    let mut scores = state.app.remote.queue_scores(guild_id);
    ranking::apply_rounds(&player.upcoming, &mut scores);
    let points = ctx.settings.vote_points();
    // 앞 200곡만 싣는다 (V3 §18.2). 나머지는 `queueTotal` 로 개수만 알리고
    // 필요할 때 `GET .../queue?offset=` 으로 가져간다.
    let queue: Vec<Value> = player
        .upcoming
        .iter()
        .take(QUEUE_PAGE_MAX)
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            let my_vote = state.app.remote.user_vote(&item.id, ctx.user_id());
            queue_item_json(item, &score, ctx.user_id(), my_vote, &points)
        })
        .collect();
    let (sorted_at, next_sort_at, sort_period) = sort_clock(&state, guild_id, player.upcoming.len());
    let bot = bot_voice_status(&state, guild_id);
    // 아래 `player.stopped` 와 `startedUtc` 가 **같은 조회 결과**를 써야 한다.
    // 따로 부르면 그 사이에 세션이 생기거나 사라져 둘이 어긋난 프레임이 나갈 수 있다.
    let schedule_now = state.app.coordinator.schedule(guild_id).await;
    let state_ref: &WebState = &state;
    let current_points = points.clone();
    // 한 사람에게만 나가는 응답이라 개인화해도 안전하다(브로드캐스트는 None 이어야 한다).
    let viewer = Some(ctx.user_id());

    json_ok(json!({
        "player": {
            "isPaused": player.is_paused,
            "effectiveVolume": player.effective_volume,
            "repeatMode": repeat_key(player.repeat_mode),
            "shuffleEnabled": player.shuffle_enabled,
            "autoplayEnabled": player.autoplay_enabled,
            // V3 §16 B1 — 캐시가 진실이다. 저장값은 "다음에 어디로 들어갈까"에만 쓴다.
            "voiceChannelId": bot.channel_id.map(|id| id.to_string()),
            "voiceConnected": bot.in_voice(),
            "botOnline": ctx.session.is_developer || bot_in_guild(&state, guild_id),
            "minVolume": ctx.settings.min_volume,
            "maxVolume": ctx.settings.max_volume,
            // 진입 로드에도 같이 싣는다. WS 프레임만 고치면 **새로고침 직후**에는
            // 화면이 여전히 옛 방식으로 파생해서 가상 재생이 멈춘다 (`loadHot` 경로).
            "stopped": schedule_now.is_none() || player.current_item.is_none(),
        },
        "current": player
            .current_item
            .as_ref()
            .map(|item| current_json(state_ref, player.guild_id, item, &current_points, viewer)),
        "positionSeconds": position,
        "sampledAtUtc": sampled_at,
        // 절대 시각 일정 (§31). 진입·재연결 응답에도 실어야 그 순간부터 정확히 맞는다.
        "startedUtc": schedule_now.map(|s| s.started_utc.to_rfc3339()),
        "skipLeadMs": ctx.settings.skip_lead_ms,
        "seekLockoutMs": ctx.settings.seek_lockout_ms,
        "webSyncOffsetMs": ctx.settings.web_sync_offset_ms,
        "queueMode": ctx.settings.sort_mode.as_str(),
        "sortedAt": sorted_at,
        "nextSortAt": next_sort_at,
        // 500곡을 넘으면 15초로 늘어난다 (§18.2). 화면이 이 값을 세야 카운트다운이 안 헛돈다.
        "sortPeriodSeconds": sort_period,
        "queue": queue,
        "queueTotal": player.upcoming.len(),
        "queueTruncated": player.upcoming.len() > QUEUE_PAGE_MAX,
        "votePoints": points,
        // 다음 곡 (V3 §14) — 이미 메모리에 있는 값을 그대로 싣는다.
        "next": next_up_json(&player),
        // 투표 스킵 현황 (V3 §10.5). 진행 중이 아니면 null 이다.
        "skipVote": skip_vote_json(&state, &ctx, &player),
        "presence": build_presence(&state, guild_id).await,
    }))
}

/// `GET /state/cold` — 진입 시 1회 + `settings`/`library`/`suspension` 이벤트 시.
async fn api_state_cold(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let session = &ctx.session;
    let settings = &ctx.settings;

    let guilds: Vec<Value> = session
        .guilds
        .iter()
        .filter(|guild| session.is_developer || bot_in_guild(&state, guild.id))
        .map(OAuthGuild::to_json)
        .collect();

    // 서버 재생목록 + **내 개인 재생목록** (V3 §12). 개인 것은 길드에 안 묶여서
    // 어느 서버에서 열어도 같이 보인다.
    let playlists: Vec<Value> = state
        .app
        .db
        .list_playlists(PlaylistScope::Guild, Some(guild_id))
        .into_iter()
        .map(|playlist| playlist_json(&playlist, session.user_id))
        .chain(
            state
                .app
                .db
                .list_user_playlists(session.user_id)
                .into_iter()
                .map(|playlist| playlist_json(&playlist, session.user_id)),
        )
        .collect();

    let liked: Vec<Value> = state
        .app
        .remote
        .list_user_tracks(guild_id, session.user_id, UserTrackKind::Liked)
        .iter()
        .map(|row| json!({ "track": track_json(&row.track) }))
        .collect();
    let saved: Vec<Value> = state
        .app
        .remote
        .list_user_tracks(guild_id, session.user_id, UserTrackKind::Saved)
        .iter()
        .map(|row| json!({ "track": track_json(&row.track) }))
        .collect();
    let recent: Vec<Value> = state
        .app
        .remote
        .list_recent(guild_id, 50)
        .iter()
        .map(|row| {
            json!({
                "track": track_json(&row.track),
                "playedUtc": row.played_utc,
                "requestedByDisplay": row.requested_by_display,
                "endReason": row.end_reason,
            })
        })
        .collect();

    let intent = state
        .app
        .intent_status
        .read()
        .map(|status| json!({ "members": status.members, "presences": status.presences, "voiceStates": true }))
        .unwrap_or_else(|_| json!({ "members": true, "presences": true, "voiceStates": true }));

    json_ok(json!({
        "buildId": state.app.build_id,
        "guild": ctx.guild.to_json(),
        "guilds": guilds,
        "user": {
            "id": session.user_id.to_string(),
            "displayName": session.display_name,
            "avatarUrl": session.avatar_url,
        },
        "tier": ctx.tier.as_str(),
        "viewerReason": ctx.viewer_reason,
        "intentStatus": intent,
        "settings": {
            "chatEnabled": settings.chat_enabled,
            "suggestionEnabled": settings.suggestion_enabled,
            "visualizerEnabled": settings.visualizer_enabled,
            // **화면도 이 값을 알아야 한다.** 예전에는 관리 콘솔 응답에만 실려서, 서버가
            // 이 설정을 꺼도 리모컨은 여전히 "봇이 음성에 없음"만 보고 버튼을 잠갔다.
            // 설정은 있는데 아무 효과가 없는 상태였다.
            "requireVoiceForPlayback": settings.require_voice_for_playback,
            // 화면이 이 값을 알아야 잠금 판정과 개인 오프셋 처리가 맞는다.
            "webPlayerMode": settings.web_player_mode,
            "minVolume": settings.min_volume,
            "maxVolume": settings.max_volume,
            "sortMode": settings.sort_mode.as_str(),
            // 화면이 계산식을 그리려면 점수표를 알아야 한다 (V3 §10.1).
            "votePoints": settings.vote_points(),
            // **같은 값을 평평하게도 준다.** 화면은 `settings.likePoints` 를 읽는데
            // 중첩 객체만 주면 늘 기본 배점(1/-1/2/1)으로 폴백해서, 좋아요를 3점으로
            // 저장해도 칩이 `👍3 + 대기2 = 11` 처럼 계산식과 합계가 어긋난다 (§10.1).
            "likePoints": settings.like_points,
            "dislikePoints": settings.dislike_points,
            "superLikePoints": settings.super_like_points,
            "waitPoints": settings.wait_points,
            // `0` 은 무제한이다 — 화면이 `∞` 로 그린다 (V3 §23.1).
            "maxQueuePerUser": settings.max_queue_per_user,
            "maxQueuePerGuild": settings.max_queue_per_guild,
            "maxTrackSeconds": settings.max_track_seconds,
            "bulkEnqueueLimit": settings.bulk_enqueue_limit,
            "chartSuperWeight": settings.chart_super_weight,
            "chartLimit": settings.chart_limit,
            // 곡 알림 방식 (§25).
            "nowPlayingMode": settings.now_playing_mode.as_str(),
            // 빈 채널 규칙 (§27). **잠금 여부까지 같이 보낸다** — 값만 보내면
            // 화면이 바꿀 수 있는 것처럼 그려 놓고 저장에서 거절당한다.
            "emptyVoice": empty_voice_json(&state, guild_id),
            // 붐따 (V3 §10.3) — 꺼져 있으면 싫어요는 점수에만 영향을 준다.
            "boomttaEnabled": settings.boomtta_enabled,
            "boomttaThreshold": settings.boomtta_threshold,
            "boomttaAction": settings.boomtta_action.as_str(),
            // 투표 스킵 (V3 §10.5).
            "voteSkipEnabled": settings.vote_skip_enabled,
            "voteSkipBasis": settings.vote_skip_basis.as_str(),
            "voteSkipBasisLabel": settings.vote_skip_basis.description(),
            "voteSkipRatio": settings.vote_skip_ratio,
            "voteSkipMin": settings.vote_skip_min,
        },
        // 슈퍼 좋아요 남은 횟수·쿨타임 (V3 §10.6). 회색으로만 두면 고장인 줄 안다.
        "superLike": super_like_status(&state, guild_id, session.user_id, settings),
        "permissions": permissions_json(&state, &ctx),
        "search": search_json(&state),
        // 진입 시 왕복을 하나 줄이려고 개인 설정도 같이 싣는다 (V3 §2).
        "prefs": prefs_json(&state, session.user_id),
        "suspension": ctx.suspension_json(),
        "playlists": playlists,
        "liked": liked,
        "saved": saved,
        "recent": recent,
        "members": build_members(&state, guild_id, settings),
        // 마참 점수 (V3 §22.4) — 프로필 드롭다운에 조용히 뜬다. 통계가 꺼져 있으면 `null` 이라
        // 화면이 그 줄을 아예 안 그린다(0으로 꾸미지 않는다).
        "machamScore": state
            .app
            .stats
            .as_ref()
            .map(|stats| json!(stats.user_stats(guild_id, session.user_id).karma()))
            .unwrap_or(Value::Null),
    }))
}

/// 검색을 어디서 돌릴지(V3 §6).
///
/// 운영 패널에 YouTube API 키가 있으면 브라우저가 YouTube Data API를 직접 부른다
/// (봇 호스트의 `yt-dlp`가 느리거나 막혀도 검색이 살아 있다). 키가 없으면 지금처럼 서버가 찾는다.
/// 키는 브라우저로 그대로 나가는 값이라 리퍼러 제한이 전제다 — 운영 패널에 그렇게 적어 뒀다.
/// 재생목록 한 장. 개인(`User`)과 서버(`Guild`)를 **아이콘과 색으로 확실히 구분**할 수 있게
/// `scope` 를 같이 준다 — 실수로 서버 것을 지우면 곤란하다 (V3 §12.2).
fn playlist_json(playlist: &crate::models::Playlist, viewer: u64) -> Value {
    let total_seconds: f64 = playlist
        .entries
        .iter()
        .filter_map(|entry| entry.track.as_ref())
        .filter_map(|track| track.duration)
        .map(|duration| duration.as_secs_f64())
        .sum();
    json!({
        "id": playlist.id,
        "name": playlist.name,
        "scope": playlist.scope.as_str(),
        "ownerUserId": playlist.owner_user_id.to_string(),
        "isMine": playlist.owner_user_id == viewer,
        "entryCount": playlist.entries.len(),
        // `12곡 · 48분` 처럼 총 길이까지 보여주면 고를 때 편하다 (§12.2).
        "totalSeconds": total_seconds,
        // `id` 는 **정렬 순서 안의 자리 번호**다. 곡을 뺄 때 클라가 이걸 그대로 돌려준다.
        // 이게 없으면 `✕` 가 `entryId: undefined` 를 보내 서버가 대상을 못 찾는다 (§12.2).
        // 자리 번호는 목록이 바뀌면 밀리므로 `cacheKey` 도 같이 줘서 서버가 대조할 수 있게 한다.
        "entries": playlist
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.track.is_some())
            .map(|(index, entry)| {
                let track = entry.track.as_ref().expect("filtered above");
                json!({
                    "id": index,
                    "index": index,
                    "cacheKey": track.cache_key(),
                    "track": track_json(track),
                })
            })
            .collect::<Vec<_>>(),
    })
}

fn search_json(state: &WebState) -> Value {
    match auth_config(state).youtube_api_key() {
        Some(key) => json!({ "mode": "browser", "youtubeApiKey": key }),
        None => json!({ "mode": "server" }),
    }
}

/// `can` 맵과 "내 권한" 화면의 근거(`entries`)를 한 번에 만든다.
/// 두 값이 같은 판정 함수를 쓰기 때문에 화면이 실제 서버 판정과 어긋나지 않는다.
fn permissions_json(state: &WebState, ctx: &AuthContext) -> Value {
    let settings = &ctx.settings;
    let member = &ctx.member;
    let viewer = ctx.tier.is_viewer();

    // (화면 키, 라벨, 규칙, 추가 게이트, 지정 역할을 찾을 권한 키)
    // 마지막 항목이 §1의 핵심이다. `playlistEdit`처럼 다른 권한의 규칙을 빌려 쓰는 줄은
    // 역할도 그 권한(`queueEdit`)의 것을 봐야 화면과 실제 판정이 어긋나지 않는다.
    //
    // **권한 11종** (V3 §0): 위 10개 + 관리자(`manager_role_ids`). 아래 줄 중 앞 10개가 그것이고,
    // 나머지는 그 10개의 규칙을 빌려 쓰거나 관리자 고정인 화면용 파생 키다.
    let rows: Vec<(&str, &str, PermissionRule, bool, &str)> = vec![
        ("search", "곡 검색·신청", settings.search_rule, true, "search"),
        ("vote", "좋아요·슈퍼 좋아요·싫어요", settings.vote_rule, true, "vote"),
        ("playback", "재생 / 일시정지", settings.playback_rule, true, "playback"),
        ("skip", "곡 넘기기", settings.skip_rule, true, "skip"),
        ("seek", "재생 위치 이동", settings.seek_rule, true, "seek"),
        ("volume", "서버 볼륨 조절", settings.volume_rule, true, "volume"),
        ("queueEdit", "대기열 편집", settings.queue_edit_rule, true, "queueEdit"),
        ("chat", "채팅 쓰기·반응·답장", settings.chat_rule, settings.chat_enabled, "chat"),
        ("autoplay", "자동 재생 켜고 끄기·기준 곡", settings.autoplay_rule, true, "autoplay"),
        ("bulkEnqueue", "재생목록·차트 전부 담기", settings.bulk_enqueue_rule, true, "bulkEnqueue"),
        // ── 아래는 위 규칙을 빌려 쓰는 화면용 파생 키 ──
        ("autoplaySeed", "자동 재생 기준 곡 등록", settings.autoplay_rule, true, "autoplay"),
        ("playlistEnqueue", "재생목록 전부 담기", settings.bulk_enqueue_rule, true, "bulkEnqueue"),
        // **서버 재생목록 편집은 관리자다** (V3 §12.3). `queue_edit_rule` 을 빌려 쓰면
        // 화면은 관리자로 잠그는데 서버는 `queueEdit` 로 열리는 어긋남이 생긴다.
        ("playlistEdit", "서버 재생목록 편집", PermissionRule::Administrator, true, "playlistEdit"),
        ("library", "보관함·재생목록", PermissionRule::GuildMember, true, "library"),
        ("suggest", "제안 작성·공감", PermissionRule::GuildMember, settings.suggestion_enabled, "suggest"),
        ("stats", "기록 보기", PermissionRule::GuildMember, true, "stats"),
        ("chatDelete", "남의 채팅 삭제", PermissionRule::Administrator, true, "chatDelete"),
        ("suggestStatus", "제안 상태 변경", PermissionRule::Administrator, true, "suggestStatus"),
        ("suspend", "유저 정지·해제", PermissionRule::Administrator, true, "suspend"),
        ("sortMode", "정렬 모드 변경", PermissionRule::Administrator, true, "sortMode"),
        ("blacklist", "차단 목록 관리", PermissionRule::Administrator, true, "blacklist"),
        ("console", "서버 관리 콘솔", PermissionRule::Administrator, true, "console"),
        // `ops` 는 여기 넣지 않는다. 아래에 봇 주인 전용 항목이 따로 있고,
        // 둘 다 넣으면 같은 권한이 응답에 두 번 실린다(규칙 설명도 서로 다르게).
    ];

    // "누가 되는지"는 서버가 센다 (V3 §23.3). 클라이언트가 역할과 인원을 다시 세면
    // 틀리기 쉽고 느리다. 캐시 한 번만 읽고 모든 줄이 그걸 나눠 쓴다.
    let audience = PermissionAudience::of(state, ctx.guild_id(), settings);

    let mut can = serde_json::Map::new();
    let mut entries: Vec<Value> = Vec::with_capacity(rows.len() + 1);
    for (key, label, rule, gate, role_key) in rows {
        let base = rule_base_allowed(role_key, rule, settings, member);
        let allowed = !viewer && gate && permission_allowed(role_key, rule, settings, member);
        let via_admin = allowed && !base;
        let role_names_for_rule = if rule == PermissionRule::ConfiguredRole {
            role_names(state, ctx.guild_id(), settings.roles_for(role_key))
        } else {
            Vec::new()
        };
        // 왜 안 되는지 — 조건을 그대로 말한다. `권한 없음`은 답이 아니다 (§23.3).
        let reason = if viewer {
            Some(
                ctx.viewer_reason
                    .clone()
                    .unwrap_or_else(|| "읽기 전용이라 아무것도 조작할 수 없어요.".into()),
            )
        } else if !gate {
            Some("이 기능은 서버에서 꺼 뒀어요.".into())
        } else if allowed {
            None
        } else {
            Some(match rule {
                PermissionRule::Disabled => "이 기능은 서버에서 꺼 뒀어요.".to_string(),
                PermissionRule::SameVoiceChannel => {
                    "봇과 같은 음성 채널에 있어야 눌러요.".to_string()
                }
                PermissionRule::Administrator => "서버 관리자만 할 수 있어요.".to_string(),
                PermissionRule::ConfiguredRole if role_names_for_rule.is_empty() => {
                    "지정된 역할이 없어서 서버 관리자만 할 수 있어요.".to_string()
                }
                PermissionRule::ConfiguredRole => format!(
                    "{} 역할이 있어야 눌러요.",
                    role_names_for_rule
                        .iter()
                        .map(|name| format!("@{name}"))
                        .collect::<Vec<_>>()
                        .join(" · ")
                ),
                PermissionRule::GuildMember => "이 서버의 멤버만 할 수 있어요.".to_string(),
            })
        };
        let allowed_count = audience.for_rule(rule, role_key, settings);
        // 통과하는 대상의 역할 이름. 지정 역할이면 그 역할들, 관리자 규칙이면 관리자 지정 역할.
        let allowed_role_names = match rule {
            PermissionRule::ConfiguredRole => role_names_for_rule.clone(),
            PermissionRule::Administrator | PermissionRule::SameVoiceChannel => {
                role_names(state, ctx.guild_id(), settings.manager_role_ids.as_slice())
            }
            _ => Vec::new(),
        };
        can.insert(key.to_string(), Value::Bool(allowed));
        entries.push(json!({
            "key": key,
            "label": label,
            "description": RemoteGuildSettings::permission_description(role_key),
            "allowed": allowed,
            "rule": rule_key(rule),
            "ruleLabel": rule_label(rule),
            "viaAdmin": via_admin,
            "reason": reason,
            // 왜 되는지/안 되는지 설명하려면 역할 이름이 있어야 말이 된다 (V3 §1).
            "roleNames": role_names_for_rule,
            // 누가 되는지 (V3 §23.3). **사람 이름은 안 나열한다** — 역할 이름 + 인원수면 충분하고,
            // 이름을 다 까면 그 사람들이 부탁 받는 창구가 된다.
            "allowedCount": allowed_count,
            "allowedRoleNames": allowed_role_names,
        }));
    }

    // 운영 패널은 봇 주인 전용 — 길드 설정과 무관하다.
    let ops = ctx.tier.is_owner();
    can.insert("ops".into(), Value::Bool(ops));
    entries.push(json!({
        "key": "ops",
        "label": "운영 패널",
        "description": "봇 전체를 관리하는 화면이에요.",
        "allowed": ops,
        "rule": "owner",
        "ruleLabel": "봇 주인 전용",
        "viaAdmin": false,
        "reason": if ops { Value::Null } else { Value::String("여기는 봇 주인만 들어갈 수 있어요.".into()) },
        "roleNames": json!([]),
        "allowedCount": Value::Null,
        "allowedRoleNames": json!([]),
    }));

    json!({ "can": Value::Object(can), "entries": entries })
}

/// "지금 이 권한을 누가 쓸 수 있나"를 한 번만 세어 두고 모든 줄이 나눠 쓴다 (V3 §23.3).
///
/// 길드 멤버 캐시를 권한마다 다시 훑으면 20줄 × 수천 명이 된다. 그래서 한 번만 훑으며
/// (관리자 수, 역할별 인원, 음성에 같이 있는 사람 수, 전체 멤버 수)를 모아 둔다.
/// 멤버 인텐트가 꺼져 있거나 캐시가 비어 있으면 `None` 을 돌려준다 —
/// **0명이라고 단정하지 않는다.** 모르는 걸 0으로 쓰면 화면이 거짓말을 한다.
struct PermissionAudience {
    known: bool,
    members: usize,
    admins: usize,
    same_voice: usize,
    by_role: HashMap<u64, usize>,
}

impl PermissionAudience {
    fn of(state: &WebState, guild_id: u64, settings: &RemoteGuildSettings) -> Self {
        let mut audience = Self {
            known: false,
            members: 0,
            admins: 0,
            same_voice: 0,
            by_role: HashMap::new(),
        };
        let bot_channel = bot_voice_status(state, guild_id).channel_id;
        let Some(cache) = state.app.discord_cache.get() else {
            return audience;
        };
        let Some(guild) = cache.guild(GuildId::new(guild_id)) else {
            return audience;
        };
        audience.known = true;
        for (user_id, member) in guild.members.iter() {
            if member.user.bot {
                continue;
            }
            audience.members += 1;
            let admin = guild.owner_id == *user_id
                || is_owner_user(state, user_id.get())
                || member
                    .roles
                    .iter()
                    .any(|role| settings.manager_roles().contains(&role.get()))
                || member.roles.iter().any(|role| {
                    guild
                        .roles
                        .get(role)
                        .map(|role| {
                            role.permissions.contains(Permissions::ADMINISTRATOR)
                                || role.permissions.contains(Permissions::MANAGE_GUILD)
                        })
                        .unwrap_or(false)
                });
            if admin {
                audience.admins += 1;
            }
            if bot_channel.is_some()
                && guild
                    .voice_states
                    .get(user_id)
                    .and_then(|voice| voice.channel_id)
                    .map(|id| id.get())
                    == bot_channel
            {
                audience.same_voice += 1;
            }
            for role in member.roles.iter() {
                *audience.by_role.entry(role.get()).or_insert(0) += 1;
            }
        }
        audience
    }

    /// 이 규칙을 통과하는 인원. 캐시가 없어 모르면 `null` 이다 — **0으로 단정하지 않는다.**
    fn for_rule(
        &self,
        rule: PermissionRule,
        role_key: &str,
        settings: &RemoteGuildSettings,
    ) -> Value {
        if !self.known {
            return Value::Null;
        }
        let count = match rule {
            // 사용 안 함은 관리자와 봇 주인까지 전부 막는다.
            PermissionRule::Disabled => 0,
            PermissionRule::GuildMember => self.members,
            PermissionRule::Administrator => self.admins,
            PermissionRule::SameVoiceChannel => self.same_voice.max(self.admins),
            PermissionRule::ConfiguredRole => {
                // 역할이 여럿이면 겹치는 사람이 이중으로 세어질 수 있다. 상한을 멤버 수로 눌러
                // "12명 중 15명이 쓸 수 있어요" 같은 말이 안 되게 한다.
                let by_roles: usize = settings
                    .roles_for(role_key)
                    .iter()
                    .map(|role| self.by_role.get(role).copied().unwrap_or(0))
                    .sum();
                (by_roles + self.admins).min(self.members)
            }
        };
        json!(count)
    }
}

/// 역할 ID를 사람이 읽는 이름으로. 캐시에 없는 역할(지워졌거나 아직 못 받은)은
/// ID를 그대로 보여 준다 — 조용히 빼면 "역할 3개 지정했는데 2개만 보이는" 상황이 된다.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewQuery {
    /// 쉼표로 이은 역할 ID. 비면 "역할 하나도 없는 사람".
    roles: Option<String>,
    /// 봇과 같은 음성 채널에 있다고 칠지.
    same_voice: Option<bool>,
}

/// `GET .../admin/preview?roles=1,2&sameVoice=true` — **이 역할이면 뭘 할 수 있나** (§37).
///
/// Discord 의 "역할로 보기"와 같은 목적이다. 관리자는 자기 화면만 보이므로,
/// 권한을 바꿔 놓고도 **일반 멤버에게 실제로 어떻게 보이는지** 확인할 방법이 없었다.
///
/// **판정은 실제 경로를 그대로 쓴다**(`permission_allowed`). 여기서 따로 계산하면
/// 미리보기와 실제가 갈라져서, 미리보기를 믿고 설정한 게 틀리는 최악이 된다.
async fn admin_role_view(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<PreviewQuery>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let role_ids: Vec<u64> = query
        .roles
        .unwrap_or_default()
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    // **관리자 우회를 끈 채로 본다.** 그게 이 화면의 존재 이유다 —
    // is_admin 을 켜 두면 전부 통과로 나와서 아무것도 확인이 안 된다.
    let member = MemberContext {
        is_admin: false,
        same_voice_channel: query.same_voice.unwrap_or(false),
        // 미리보기라도 봇의 실제 음성 상태를 쓴다. 여기서 지어내면 화면이 보여 주는 결과와
        // 진짜 판정이 갈리는데, 그게 이 화면이 제일 하면 안 되는 일이다.
        bot_in_voice: bot_voice_status(&state, guild_id).in_voice(),
        role_ids: role_ids.clone(),
    };
    let settings = &ctx.settings;
    let rows: Vec<Value> = PERMISSION_KEYS
        .iter()
        .filter_map(|key| {
            let rule = settings.rule_for(key)?;
            Some(json!({
                "key": key,
                "rule": rule_key(rule),
                "ruleLabel": rule_label(rule),
                "allowed": permission_allowed(key, rule, settings, &member),
            }))
        })
        .collect();
    json_ok(json!({
        "roleIds": role_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "roleNames": role_names(&state, guild_id, &role_ids),
        "sameVoice": member.same_voice_channel,
        "permissions": rows,
    }))
}

fn role_names(state: &WebState, guild_id: u64, role_ids: &[u64]) -> Vec<String> {
    let guild = state
        .app
        .discord_cache
        .get()
        .and_then(|cache| cache.guild(GuildId::new(guild_id)));
    role_ids
        .iter()
        .map(|role_id| {
            guild
                .as_ref()
                .and_then(|guild| guild.roles.get(&serenity::all::RoleId::new(*role_id)))
                .map(|role| role.name.clone())
                .unwrap_or_else(|| format!("역할 {role_id}"))
        })
        .collect()
}

/// 기존 단일 스냅샷. 새 프런트는 쓰지 않지만 외부 스크립트 호환을 위해 남긴다.
async fn api_state(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let player = state.app.player.get_state(guild_id).await;
    let position = state
        .app
        .coordinator
        .current_position(guild_id)
        .await
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0);
    let sampled_at = now_utc();
    let mut scores = state.app.remote.queue_scores(guild_id);
    ranking::apply_rounds(&player.upcoming, &mut scores);
    let points = ctx.settings.vote_points();
    let queue: Vec<Value> = player
        .upcoming
        .iter()
        .take(QUEUE_PAGE_MAX)
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            queue_item_json(
                item,
                &score,
                ctx.user_id(),
                state.app.remote.user_vote(&item.id, ctx.user_id()),
                &points,
            )
        })
        .collect();
    let bot = bot_voice_status(&state, guild_id);
    let state_ref: &WebState = &state;
    let current_points = points.clone();
    // 한 사람에게만 나가는 응답이라 개인화해도 안전하다(브로드캐스트는 None 이어야 한다).
    let viewer = Some(ctx.user_id());
    json_ok(json!({
        "guild": ctx.guild.to_json(),
        "user": {
            "id": ctx.session.user_id.to_string(),
            "displayName": ctx.session.display_name,
            "avatarUrl": ctx.session.avatar_url,
        },
        "tier": ctx.tier.as_str(),
        "player": {
            // V3 §16 B1 — 저장값이 아니라 Discord 캐시가 진실이다.
            "voiceChannelId": bot.channel_id.map(|id| id.to_string()),
            "isPaused": player.is_paused,
            "effectiveVolume": player.effective_volume,
            "repeatMode": repeat_key(player.repeat_mode),
            "shuffleEnabled": player.shuffle_enabled,
            "autoplayEnabled": player.autoplay_enabled,
        },
        "connection": {
            "botOnline": ctx.session.is_developer || bot_in_guild(&state, guild_id),
            "voiceConnected": bot.in_voice(),
        },
        "current": player
            .current_item
            .as_ref()
            .map(|item| current_json(state_ref, player.guild_id, item, &current_points, viewer)),
        "positionSeconds": position,
        "sampledAtUtc": sampled_at,
        "startedUtc": state.app.coordinator.schedule(guild_id).await.map(|s| s.started_utc.to_rfc3339()),
        "skipLeadMs": ctx.settings.skip_lead_ms,
        "seekLockoutMs": ctx.settings.seek_lockout_ms,
        "webSyncOffsetMs": ctx.settings.web_sync_offset_ms,
        "queue": queue,
        "queueTotal": player.upcoming.len(),
        "next": next_up_json(&player),
        "settings": ctx.settings,
        "permissions": permissions_json(&state, &ctx),
        "serverTimeUtc": sampled_at,
    }))
}

// ───────────────────────── 채팅 조회 ─────────────────────────

#[derive(Debug, Deserialize)]
struct BeforeQuery {
    before: Option<i64>,
    limit: Option<usize>,
}

fn chat_message_json(message: &crate::remote::ChatMessage, viewer: u64) -> Value {
    json!({
        "id": message.id,
        "userId": message.user_id.to_string(),
        "displayName": message.display_name,
        "avatarUrl": message.avatar_url,
        "content": message.content,
        "createdUtc": message.created_utc,
        "editedUtc": message.edited_utc,
        "deletedUtc": message.deleted_utc,
        "isMine": message.user_id == viewer,
        "replyTo": message.reply_to.as_ref().map(|reply| json!({
            "id": reply.id,
            "displayName": reply.display_name,
            "preview": reply.excerpt,
            "deleted": reply.deleted,
        })),
        "mentions": message.mentions.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "mentionNames": Vec::<String>::new(),
        "tags": message.tags.iter().map(|tag| json!({
            "cacheKey": tag.cache_key,
            "track": track_json(&tag.track),
        })).collect::<Vec<_>>(),
        "reactions": message.reactions.iter().map(|reaction| json!({
            "emoji": reaction.emoji,
            "count": reaction.count,
            "reactedByMe": reaction.reacted_by_me,
            "users": Vec::<Value>::new(),
        })).collect::<Vec<_>>(),
    })
}

async fn api_chat_list(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<BeforeQuery>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    // 채팅 읽기는 멤버 이상만 (사양서 §1.2).
    if ctx.tier.is_viewer() {
        return json_ok(json!({ "messages": [], "nextBefore": Value::Null }));
    }
    let limit = query.limit.unwrap_or(crate::remote::store::CHAT_PAGE_LIMIT);
    let messages = state
        .app
        .remote
        .list_chat_messages(guild_id, ctx.user_id(), limit, query.before);
    let next_before = messages.first().map(|message| message.id);
    let rows: Vec<Value> = messages
        .iter()
        .map(|message| chat_message_json(message, ctx.user_id()))
        .collect();
    json_ok(json!({ "messages": rows, "nextBefore": next_before }))
}

/// `GET .../audit?before=&kinds=song,playlist` — **사람이 읽는 피드** (V3 §13.5).
///
/// `text` 는 서버가 완성해서 내려준다. 클라이언트가 액션명을 문장으로 바꾸는 로직을
/// 갖게 두면 서버가 액션을 하나 추가할 때마다 화면이 조용히 깨진다.
/// 전후값 JSON 과 실패 사유는 **아예 싣지 않는다** — 그건 관리 콘솔(`/admin/audit`)의 몫이다.
async fn api_audit(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<AuditFeedQuery>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if ctx.tier.is_viewer() {
        return json_ok(json!({ "entries": [], "kinds": audit_kind_options() }));
    }
    let limit = query.limit.unwrap_or(100).clamp(1, 300);
    let kinds = parse_audit_kinds(query.kinds.as_deref());
    let rows = state
        .app
        .remote
        .list_audit_kinds(guild_id, limit, query.before, &kinds);
    // 자동 재생이 넣은 곡과 실패한 시도는 사람 피드에 안 남긴다 (§13.3).
    let entries: Vec<Value> = rows
        .iter()
        .filter(|entry| entry.is_human_visible())
        .map(|entry| serde_json::to_value(entry.feed_item()).unwrap_or(Value::Null))
        .collect();
    json_ok(json!({
        "entries": entries,
        // 필터 칩(§13.4)과 기본값을 서버가 알려 준다 — 화면이 목록을 따로 들고 있으면 어긋난다.
        "kinds": audit_kind_options(),
        "defaultKinds": AuditKind::default_filter()
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
        // 필터로 몇 줄이 숨겨졌는지 화면이 `+ 12개 더` 로 알려 줄 수 있게.
        //
        // **`rows` 는 이미 `kinds` 로 걸러진 결과**다. 여기서 `rows.len() - entries.len()` 을 쓰면
        // "안 켠 분류"가 아니라 실패·시스템 행 개수가 되어, 투표 로그가 수백 줄 쌓여도
        // 버튼이 안 뜨고(§13.4 미충족) 반대로 눌러도 안 나오는 줄만 세어졌다.
        "hiddenCount": audit_hidden_count(&state, guild_id, limit, query.before, &kinds),
    }))
}

#[derive(Debug, Deserialize)]
struct AuditFeedQuery {
    before: Option<i64>,
    limit: Option<usize>,
    /// `song,playlist` 처럼 쉼표로 구분. 없거나 비면 전부 본다.
    kinds: Option<String>,
}

/// `+ N개 더 (안 켠 분류에 있어요)` 의 N (V3 §13.4).
///
/// **안 켠 분류에 실제로 남아 있는, 사람이 볼 수 있는 줄 수**다. 칩을 전부 켰거나
/// 필터가 없으면 셀 게 없으므로 쿼리도 안 돈다(§23.2 — 유휴 시 쿼리 0회).
fn audit_hidden_count(
    state: &WebState,
    guild_id: u64,
    limit: usize,
    before: Option<i64>,
    kinds: &[AuditKind],
) -> usize {
    if kinds.is_empty() || kinds.len() >= AuditKind::ALL.len() {
        return 0;
    }
    // `kinds` 를 비우면 전 분류다. 같은 창(limit·before)에서 세어야 숫자가 말이 된다.
    state
        .app
        .remote
        .list_audit_kinds(guild_id, limit, before, &[])
        .iter()
        .filter(|entry| entry.is_human_visible())
        .filter(|entry| !kinds.contains(&entry.kind))
        .count()
}

fn parse_audit_kinds(raw: Option<&str>) -> Vec<AuditKind> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .filter_map(AuditKind::parse)
            .collect::<Vec<_>>()
    })
    .unwrap_or_default()
}

fn audit_kind_options() -> Value {
    json!(
        AuditKind::ALL
            .iter()
            .map(|kind| json!({ "key": kind.as_str(), "label": kind.label() }))
            .collect::<Vec<_>>()
    )
}

fn audit_json(entry: &crate::remote::AuditEntry) -> Value {
    json!({
        "id": entry.id,
        "userId": entry.user_id.to_string(),
        "displayName": entry.display_name,
        "action": entry.action,
        "target": entry.target,
        "beforeValue": entry.before_value,
        "afterValue": entry.after_value,
        "failureReason": entry.failure_reason,
        "success": entry.success,
        "createdUtc": entry.created_utc,
    })
}

fn suggestion_json(item: &crate::remote::Suggestion, viewer: u64) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "body": item.body,
        "userId": item.user_id.to_string(),
        "displayName": item.display_name,
        "avatarUrl": item.avatar_url,
        "status": item.status.as_str(),
        "statusLabel": item.status.label(),
        "statusNote": item.status_note,
        "votes": item.vote_count,
        "voteCount": item.vote_count,
        "votedByMe": item.voted_by_me,
        "createdUtc": item.created_utc,
        "isMine": item.user_id == viewer,
    })
}

async fn api_suggestions(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if ctx.tier.is_viewer() {
        return json_ok(json!({ "items": [] }));
    }
    let items: Vec<Value> = state
        .app
        .remote
        .list_suggestions(guild_id, ctx.user_id())
        .iter()
        .map(|item| suggestion_json(item, ctx.user_id()))
        .collect();
    json_ok(json!({ "items": items }))
}

/// `@멘션` 자동완성 후보 — 이 서버에서 리모컨을 써 본 사람(결정 #11) + Discord 캐시 보정.
async fn api_mention_candidates(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if ctx.tier.is_viewer() {
        return json_ok(json!({ "items": [] }));
    }
    let items: Vec<Value> = mention_candidates(&state, guild_id)
        .into_iter()
        .map(|(user_id, display_name, avatar_url)| {
            json!({
                "userId": user_id.to_string(),
                "displayName": display_name,
                "avatarUrl": avatar_url,
            })
        })
        .collect();
    json_ok(json!({ "items": items }))
}

/// 참여자 목록에 Discord 캐시의 표시 이름/아바타를 덧입힌다.
/// 채팅 기록이 없는(곡만 신청한) 사람은 저장소가 빈 문자열을 주므로 여기서 채운다.
fn mention_candidates(state: &WebState, guild_id: u64) -> Vec<(u64, String, Option<String>)> {
    let participants = state.app.remote.list_remote_participants(guild_id);
    let cache = state.app.discord_cache.get();
    let guild = cache.and_then(|cache| cache.guild(GuildId::new(guild_id)));
    participants
        .into_iter()
        .map(|person| {
            let cached = guild
                .as_ref()
                .and_then(|guild| guild.members.get(&UserId::new(person.user_id)))
                .map(|member| (member.display_name().to_string(), Some(member.face())));
            match cached {
                Some((name, avatar)) => (person.user_id, name, avatar),
                None => (
                    person.user_id,
                    if person.display_name.is_empty() {
                        person.user_id.to_string()
                    } else {
                        person.display_name
                    },
                    person.avatar_url,
                ),
            }
        })
        .filter(|(_, name, _)| !name.is_empty())
        .collect()
}

// ───────────────────────── 검색 / 가사 ─────────────────────────

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    provider: Option<String>,
}

async fn api_search(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require("search", ctx.settings.search_rule, "검색 권한이 없어요.") {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    if rate_limited(
        &state,
        guild_id,
        ctx.user_id(),
        "search",
        Duration::from_millis(600),
    ) {
        return json_error(StatusCode::TOO_MANY_REQUESTS, "검색 요청이 너무 빨라요. 잠깐만 쉬었다 해요.");
    }
    let input = query.q.trim();
    if input.is_empty() || input.chars().count() > 200 {
        return json_error(StatusCode::BAD_REQUEST, "검색어는 1~200자로 입력해요.");
    }
    let provider = match query.provider.as_deref() {
        Some("SoundCloud") => ProviderKind::SoundCloud,
        Some("YouTubeMusic") => ProviderKind::YouTubeMusic,
        _ => ProviderKind::YouTube,
    };
    let results = if crate::media::resolver::can_resolve(input) {
        match crate::media::resolver::resolve(input) {
            Ok(crate::media::resolver::Resolved::Collection(collection)) => state
                .app
                .ytdlp()
                .expand_collection(&collection.source_url, collection.provider)
                .await
                .into_iter()
                .take(50)
                .collect(),
            Ok(crate::media::resolver::Resolved::Track(track)) => state
                .app
                .ytdlp()
                .inspect_track(&track.source_url, track.provider)
                .await
                .into_iter()
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        state.app.ytdlp().search_provider(input, 8, provider).await
    };
    let values: Vec<Value> = results
        .into_iter()
        .filter(|track| !state.app.blacklist.is_blocked(guild_id, track))
        .map(|track| track_json(&track))
        .collect();
    json_ok(json!({ "results": values }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibRow {
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

async fn api_lyrics(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    if let Err(response) = authorize(&state, &cookies, guild_id, None).await {
        return response;
    }
    let player = state.app.player.get_state(guild_id).await;
    let Some(item) = player.current_item else {
        return json_ok(json!({ "plainText": Value::Null, "syncedLines": [] }));
    };
    let cache_key = item.track.cache_key();
    // "아직 안 찾아봄"과 "찾아봤는데 없음"을 구분한다 — 후자는 재조회하지 않는다.
    match state.app.remote.lookup_lyrics(&cache_key) {
        Some(LyricsCacheHit::Found(document)) => return Json(*document).into_response(),
        Some(LyricsCacheHit::Missing) => {
            return json_ok(json!({ "plainText": Value::Null, "syncedLines": [] }));
        }
        None => {}
    }
    let title = item
        .track
        .title
        .clone()
        .unwrap_or_else(|| item.track.content_id.clone());
    let (artist, track_name) = match item.track.artist.clone() {
        Some(artist) if !artist.trim().is_empty() => (artist, title.clone()),
        _ => title
            .split_once(" - ")
            .map(|(artist, title)| (artist.to_string(), title.to_string()))
            .unwrap_or_else(|| (String::new(), title.clone())),
    };
    let mut request = http_client(&state).get("https://lrclib.net/api/search");
    if artist.is_empty() {
        request = request.query(&[("q", track_name.as_str())]);
    } else {
        request = request.query(&[
            ("track_name", track_name.as_str()),
            ("artist_name", artist.as_str()),
        ]);
    }
    let row = match request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<Vec<LrcLibRow>>()
            .await
            .ok()
            .and_then(|rows| rows.into_iter().next()),
        _ => None,
    };
    match row {
        Some(row) => {
            let lyrics = LyricsDocument {
                cache_key,
                plain_text: row.plain_lyrics,
                synced_lines: row
                    .synced_lyrics
                    .as_deref()
                    .map(parse_lrc)
                    .unwrap_or_default(),
                source: "LRCLIB".into(),
                fetched_utc: now_utc(),
            };
            let _ = state.app.remote.save_lyrics(&lyrics);
            Json(lyrics).into_response()
        }
        None => {
            let _ = state.app.remote.save_lyrics_missing(&cache_key);
            json_ok(json!({ "plainText": Value::Null, "syncedLines": [] }))
        }
    }
}

fn parse_lrc(value: &str) -> Vec<LyricsLine> {
    let pattern = regex::Regex::new(r"^\[(\d+):(\d+(?:\.\d+)?)\]\s?(.*)$").unwrap();
    value
        .lines()
        .filter_map(|line| {
            let captures = pattern.captures(line.trim())?;
            let minutes: u64 = captures.get(1)?.as_str().parse().ok()?;
            let seconds: f64 = captures.get(2)?.as_str().parse().ok()?;
            Some(LyricsLine {
                start_ms: minutes * 60_000 + (seconds * 1_000.0).round() as u64,
                text: captures.get(3)?.as_str().to_string(),
            })
        })
        .collect()
}

// ───────────────────────── 대기열 / 재생 제어 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueRequest {
    track: TrackRef,
}

async fn api_enqueue(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(mut request): Json<EnqueueRequest>,
) -> Response {
    // 브라우저 검색으로 온 곡은 `sourceUrl` 이 비어 있을 수 있다. 받는 자리에서 한 번
    // 채우면 그 뒤 재생·캐시 경로는 늘 채워진 값만 본다.
    request.track.ensure_source_url();
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require("search", ctx.settings.search_rule, "대기열에 곡을 담을 권한이 없어요.") {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    let session = &ctx.session;
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "enqueue",
        Duration::from_millis(350),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "곡 등록 요청이 너무 빨라요. 잠깐만 쉬었다 해요.",
        );
    }
    if !crate::media::resolver::can_resolve(&request.track.source_url) {
        return json_error(StatusCode::BAD_REQUEST, "지원하지 않는 곡 URL이에요.");
    }
    if let Some(rule) = state
        .app
        .blacklist
        .try_get_blocker(guild_id, &request.track)
    {
        audit_failure(
            &state,
            guild_id,
            session,
            "queue.add",
            Some(request.track.display_title()),
            "blacklisted",
        );
        return json_error(
            StatusCode::BAD_REQUEST,
            format!(
                "차단 규칙 때문에 담을 수 없어요: {}",
                crate::blacklist::Blacklist::describe_rule(&rule)
            ),
        );
    }
    let player = state.app.player.get_state(guild_id).await;
    // `maxTrackSeconds == 0` 은 무제한이다 (§23.1).
    if track_too_long(ctx.settings.max_track_seconds, &request.track) {
        audit_failure(
            &state,
            guild_id,
            session,
            "queue.add",
            Some(request.track.display_title()),
            "track_too_long",
        );
        return json_error(
            StatusCode::BAD_REQUEST,
            format!(
                "허용 곡 길이({}초)를 넘었어요.",
                ctx.settings.max_track_seconds
            ),
        );
    }
    let cache_key = request.track.cache_key();
    if player
        .current_item
        .iter()
        .chain(player.upcoming.iter())
        .any(|item| item.track.cache_key() == cache_key)
    {
        audit_failure(
            &state,
            guild_id,
            session,
            "queue.add",
            Some(request.track.display_title()),
            "duplicate",
        );
        return json_error(
            StatusCode::CONFLICT,
            "이미 지금 재생 중이거나 대기열에 있는 곡이에요.",
        );
    }
    let user_count = player
        .upcoming
        .iter()
        .filter(|item| item.requested_by_user_id == Some(session.user_id))
        .count();
    // **`0` 은 무제한이다** (§23.1). 예전의 `.max(1)` 클램프는 `0` 을 조용히 `1` 로 바꿔서
    // 화면에는 "무제한"이라고 뜨는데 서버는 한 곡만 받는 최악의 조합을 만들었다.
    let guild_full = limit_blocks(ctx.settings.max_queue_per_guild, player.upcoming.len() + 1);
    let user_full = limit_blocks(ctx.settings.max_queue_per_user, user_count + 1);
    if guild_full || user_full {
        audit_failure(
            &state,
            guild_id,
            session,
            "queue.add",
            Some(request.track.display_title()),
            "queue_limit",
        );
        return json_error(
            StatusCode::CONFLICT,
            if user_full {
                format!(
                    "한 사람이 담을 수 있는 {}곡을 다 채웠어요.",
                    ctx.settings.max_queue_per_user
                )
            } else {
                format!(
                    "서버 대기열 {}곡이 꽉 찼어요.",
                    ctx.settings.max_queue_per_guild
                )
            },
        );
    }
    let item = QueueItem::new_user(
        request.track.clone(),
        session.display_name.clone(),
        Some(session.user_id),
    );
    let item_id = item.id.clone();
    let title = item.track.display_title().to_string();
    let queued = state.app.player.enqueue(guild_id, item, false).await;
    if !session.is_developer {
        state.app.coordinator.sync_guild(&state.app, guild_id).await;
    }
    audit_ok(
        &state,
        guild_id,
        session,
        "queue.add",
        Some(&title),
        Some("queued"),
    );
    // §22.3 `queued_single` 은 `PlayerManager::enqueue` 가 남긴다 — 큐에 넣는 길이 거기 하나뿐이라
    // 디스코드 명령으로 들어온 곡도 같이 잡힌다. 여기서 또 던지면 웹 신청만 두 번 세진다.
    broadcast_queue(&state, guild_id).await;
    let queue_position = queued
        .upcoming
        .iter()
        .position(|item| item.id == item_id)
        .map(|index| index + 1);
    json_ok(json!({
        "ok": true,
        "itemId": item_id,
        "queuePosition": queue_position,
        "playingNow": queued.current_item.as_ref().is_some_and(|item| item.id == item_id),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRequest {
    action: String,
    value: Option<f64>,
    expected_item_id: Option<String>,
    /// `{action:"repeat", mode:"off|track|queue"}`
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebListeningRequest {
    /// 지금 이 브라우저에서 소리가 나고 있는가.
    on: bool,
}

/// 웹 재생 시작·중단 보고.
///
/// **소켓이 끊기면 자동으로 빠진다** — `presence_remove` 가 같이 지운다. 탭을 그냥 닫거나
/// 크래시해도 리스너로 남지 않는다. 알림 없이 사라지는 쪽이 흔하기 때문에 이쪽이 진실이다.
async fn api_web_listening(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<WebListeningRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let key = (guild_id, ctx.session.user_id);
    let changed = {
        let mut listeners = state.web_listeners.lock().unwrap();
        if request.on {
            listeners.insert(key)
        } else {
            listeners.remove(&key)
        }
    };
    // 비었다↔찼다 경계에서만 구동기를 건드린다. 매 보고마다 흔들면 안 된다.
    if changed {
        on_web_listeners_changed(&state, guild_id).await;
    }
    json_ok(json!({ "ok": true }))
}

/// 이 길드에서 실제로 웹으로 듣고 있는 사람 수.
pub(crate) fn web_listener_count(state: &WebState, guild_id: u64) -> usize {
    state
        .web_listeners
        .lock()
        .unwrap()
        .iter()
        .filter(|(gid, _)| *gid == guild_id)
        .count()
}

/// 리스너가 생기거나 사라졌을 때. 지금은 재동기화만 하고, 가상 세션 생성은 `sync_guild` 가 판단한다.
async fn on_web_listeners_changed(state: &Arc<WebState>, guild_id: u64) {
    let app = state.app.clone();
    let coordinator = app.coordinator.clone();
    coordinator.sync_guild(&app, guild_id).await;
}

async fn api_control(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ControlRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let session = &ctx.session;
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "control",
        Duration::from_millis(350),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "재생 제어 요청이 너무 빨라요. 잠깐만 쉬었다 해요.",
        );
    }
    // 어떤 권한 키로 볼지도 같이 정한다 — 지정 역할이 키마다 다르기 때문이다 (V3 §1).
    let (rule_key_for_action, rule, denied) = match request.action.as_str() {
        "seek" => ("seek", ctx.settings.seek_rule, "재생 위치를 옮길 권한이 없어요."),
        "volume" => ("volume", ctx.settings.volume_rule, "볼륨을 바꿀 권한이 없어요."),
        "shuffle" => (
            "queueEdit",
            ctx.settings.queue_edit_rule,
            "대기열을 섞을 권한이 없어요.",
        ),
        // 스킵은 재생/일시정지와 성격이 달라 권한이 따로다 (V3 §10.5). 기본은 모든 멤버.
        "skip" | "skipVoteCancel" => (
            "skip",
            ctx.settings.skip_rule,
            "곡을 넘길 권한이 없어요.",
        ),
        // 자동 재생 On/Off 도 일반 유저 권한이다 (V3 §24.3).
        "autoplay" => (
            "autoplay",
            ctx.settings.autoplay_rule,
            "자동 재생을 바꿀 권한이 없어요.",
        ),
        _ => (
            "playback",
            ctx.settings.playback_rule,
            "재생을 조작할 권한이 없어요.",
        ),
    };
    if let Err(response) = ctx.require(rule_key_for_action, rule, denied) {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    let player = state.app.player.get_state(guild_id).await;
    // 내 표만 거두는 요청은 아무것도 조작하지 않는다. 봇이 이미 나갔어도
    // 내 표는 거둘 수 있어야 한다 — 안 그러면 화면에 유령 투표가 남는다.
    if request.action == "skipVoteCancel" {
        return skip_vote_cancel(&state, &ctx, &player);
    }
    // V3 §16 B1 — 저장값이 아니라 캐시가 진실이다. 저장값을 보면 봇이 이미 나갔는데도
    // 조작이 통과해서 아무 일도 안 일어나는 유령 상태가 된다.
    //
    // 서버마다 끌 수 있다 (§36). 끄면 조작을 받아 두고 봇이 들어오는 순간부터 이어 간다.
    // **자동 재생은 재생 명령이 아니라 저장되는 설정이다.** 봇이 음성에 없을 때야말로
    // "다음에 들어오면 알아서 틀어" 를 켜 두려는 순간이라, 여기에 음성 연결을 요구하면
    // 정작 켜야 할 때 못 켠다. 나머지 조작만 캐시 기준으로 막는다.
    // **게이트가 둘이다.** 위 권한 검사만 열면 여기서 409 로 막힌다 — 실제로 그렇게 설계했다가
    // 교차검증에서 잡혔다. 웹 재생기 모드에서는 봇이 없어도 조작할 시각표가 있으므로 통과시킨다.
    let virtual_playing = ctx.settings.web_player_mode
        && !bot_voice_status_of(&state.app, guild_id).in_voice();
    if action_requires_voice(&request.action)
        && ctx.settings.require_voice_for_playback
        && !virtual_playing
        && !bot_voice_status(&state, guild_id).in_voice()
    {
        return json_error(
            StatusCode::CONFLICT,
            // 안내하는 명령 이름은 **실제로 있는 것**이어야 한다. 예전엔 없는 `/입장` 을
            // 안내해서, 시킨 대로 쳐도 아무 일이 안 일어났다.
            "봇이 음성 채널에 안 들어가 있어요. `/참여` 로 부르거나, 서버 설정에서 이 제한을 끌 수 있어요.",
        );
    }
    if matches!(request.action.as_str(), "skip" | "seek") {
        let current_id = player.current_item.as_ref().map(|item| item.id.clone());
        if request.expected_item_id.as_deref() != current_id.as_deref() {
            return json_error(
                StatusCode::CONFLICT,
                "재생 상태가 그새 바뀌었어요. 화면을 새로 받아 볼게요.",
            );
        }
    }
    // 투표 스킵이 켜져 있으면 스킵은 "투표를 연다"는 뜻이 된다 (V3 §10.5).
    // 곡이 맞는지 확인한 **다음에** 표를 센다 — 이미 지나간 곡에 표를 얹으면 안 된다.
    // 투표로 넘어간 스킵은 로그 문장이 다르다 (§10.5) — 몇 명이 동의했는지를 들고 다닌다.
    let mut skip_votes_passed: Option<usize> = None;
    if request.action == "skip" {
        match resolve_skip(&state, &ctx, &player) {
            SkipDecision::Immediate { by_votes } => skip_votes_passed = by_votes,
            SkipDecision::Pending(response) => return response,
        }
    }
    let mut queue_changed = false;
    let result: Result<String, String> = match request.action.as_str() {
        "pause" => {
            state.app.player.pause(guild_id).await;
            if !session.is_developer {
                state.app.coordinator.apply_pause(guild_id, true).await;
            }
            Ok("paused".into())
        }
        "resume" => {
            state.app.player.resume(guild_id).await;
            if !session.is_developer {
                state.app.coordinator.apply_pause(guild_id, false).await;
                state.app.coordinator.sync_guild(&state.app, guild_id).await;
            }
            Ok("resumed".into())
        }
        "skip" => {
            if !session.is_developer {
                state.app.coordinator.cancel_current(guild_id).await;
            }
            state.app.player.skip(guild_id).await;
            if !session.is_developer {
                state.app.coordinator.sync_guild(&state.app, guild_id).await;
                // 스킵도 모두가 같은 순간에 0초부터 시작하게 조금 미래로 잡는다 (§31).
                state
                    .app
                    .coordinator
                    .schedule_start_in(
                        guild_id,
                        Duration::from_millis(ctx.settings.skip_lead_ms as u64),
                        Duration::ZERO,
                    )
                    .await;
            }
            queue_changed = true;
            Ok("skipped".into())
        }
        "seek" => {
            let seconds = request.value.unwrap_or(-1.0);
            let duration = player
                .current_item
                .as_ref()
                .and_then(|item| item.track.duration)
                .map(|duration| duration.as_secs_f64())
                .unwrap_or(0.0);
            // 곡이 끝나기 직전에는 막는다 (§31). 화면에서도 막지만 **서버가 최종 판정**이다 —
            // 화면만 막으면 다른 창이나 옛 탭에서 그대로 들어온다.
            let now_position = state
                .app
                .coordinator
                .current_position(guild_id)
                .await
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let lockout = ctx.settings.seek_lockout_ms as f64 / 1000.0;
            if seconds < 0.0 || duration <= 0.0 || seconds > duration {
                Err("옮기려는 위치가 곡 길이를 벗어났어요.".into())
            } else if lockout > 0.0 && duration - now_position <= lockout {
                Err(format!(
                    "곡이 끝나기 {}초 전부터는 위치를 못 옮겨요. 다음 곡으로 넘어가는 중이라서요.",
                    lockout.round() as i64
                ))
            } else {
                state
                    .app
                    .player
                    .set_current_start_offset(guild_id, CsTimeSpan::from_secs_f64(seconds))
                    .await;
                if !session.is_developer {
                    state.app.coordinator.cancel_current(guild_id).await;
                    state.app.coordinator.sync_guild(&state.app, guild_id).await;
                    // 모두가 같은 순간에 같은 지점을 시작하게 조금 미래로 잡는다 (§31).
                    state
                        .app
                        .coordinator
                        .schedule_start_in(
                            guild_id,
                            Duration::from_millis(ctx.settings.skip_lead_ms as u64),
                            Duration::from_secs_f64(seconds),
                        )
                        .await;
                }
                Ok(format!("seek:{seconds:.1}"))
            }
        }
        "volume" => {
            let volume = request.value.unwrap_or(-1.0).round() as i32;
            if volume < ctx.settings.min_volume || volume > ctx.settings.max_volume {
                Err(format!(
                    "볼륨은 {}~{} 사이여야 해요.",
                    ctx.settings.min_volume, ctx.settings.max_volume
                ))
            } else {
                state.app.player.set_volume(guild_id, volume).await;
                if !session.is_developer {
                    state.app.coordinator.apply_volume(guild_id, volume).await;
                }
                // **접두사를 붙이지 않는다.** `audit_text` 가 `after` 를 문장에 그대로 박아서
                // `volume:150` 을 넘기면 사람 피드에 `볼륨을 volume:150으로 바꿨어요` 가 나간다.
                Ok(format!("{volume}%"))
            }
        }
        // v2 신규 — 프런트의 🔁 / 🎲 버튼.
        "repeat" => match request.mode.as_deref().and_then(parse_repeat) {
            Some(mode) => {
                state.app.player.set_repeat(guild_id, mode).await;
                Ok(format!("repeat:{}", repeat_key(mode)))
            }
            None => Err("반복 모드는 off / track / queue 중 하나여야 해요.".into()),
        },
        "shuffle" => {
            let enabled = request.value.map(|value| value > 0.5).unwrap_or(false);
            state.app.player.set_shuffle(guild_id, enabled).await;
            queue_changed = true;
            Ok(format!("shuffle:{enabled}"))
        }
        // 🎲 자동 재생 On/Off — 일반 유저도 할 수 있다 (V3 §24.3).
        "autoplay" => {
            let enabled = request.value.map(|value| value > 0.5).unwrap_or(false);
            state.app.player.set_autoplay(guild_id, enabled).await;
            let mut engine = state.app.db.load_guild_settings(guild_id);
            engine.autoplay_default_override = Some(enabled);
            state.app.db.save_guild_settings(&engine);
            // `audit_text` 의 `autoplay.toggle` 이 `on`/`off` 를 읽는다 (§24.3).
            Ok(if enabled { "on".into() } else { "off".into() })
        }
        _ => Err("지원하지 않는 재생 제어예요.".into()),
    };
    match result {
        Ok(after) => {
            // 액션명을 `playback.<action>` 으로 뭉뚱그리면 `audit_text` 의 catch-all 로 떨어져
            // 사람 피드에 `민수님이 playback.autoplay 을 했어요` 같은 기계 문자열이 나간다 (§13.1).
            // 문장이 준비된 액션명은 그 이름으로 남긴다.
            match skip_votes_passed {
                // `N명이 동의해서 곡을 넘겼어요` 의 N 은 로그의 `count` 칸이라, 그 숫자를
                // 실을 수 있는 `add_audit_bulk` 로 남긴다 (§10.5 · §13.3).
                Some(agreed) if request.action == "skip" => {
                    let _ = state.app.remote.add_audit_bulk(
                        guild_id,
                        session.user_id,
                        &session.display_name,
                        "playback.skip.vote",
                        None,
                        agreed.max(1) as u32,
                        &[],
                    );
                    emit_bare(&state, guild_id, "audit");
                }
                _ => {
                    // 액션명을 `playback.<action>` 으로 뭉뚱그리면 `audit_text` 의 catch-all 로 떨어져
                    // 사람 피드에 `민수님이 playback.autoplay 을 했어요` 가 그대로 나간다 (§13.1).
                    let action = if request.action == "autoplay" {
                        "autoplay.toggle".to_string()
                    } else {
                        format!("playback.{}", request.action)
                    };
                    audit_ok(&state, guild_id, session, &action, None, Some(&after));
                }
            }
            let player = state.app.player.get_state(guild_id).await;
            let position = state
                .app
                .coordinator
                .current_position(guild_id)
                .await
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            let sampled_at = now_utc();
            emit(
                &state,
                guild_id,
                "playback",
                playback_payload(&state, &player, position, &sampled_at, None, state.app.coordinator.schedule(guild_id).await),
            );
            if queue_changed {
                broadcast_queue(&state, guild_id).await;
            }
            // 스킵은 "넘어갔는지"를 같이 알려 준다 (V3 §10.5). 여기까지 왔으면 즉시 스킵이다.
            if request.action == "skip" {
                state.skip_votes.lock().unwrap().remove(&guild_id);
                emit(&state, guild_id, "skipvote", Value::Null);
                return json_ok(json!({ "ok": true, "skipped": true }));
            }
            json_ok(json!({ "ok": true }))
        }
        Err(error) => {
            audit_failure(
                &state,
                guild_id,
                session,
                &format!("playback.{}", request.action),
                None,
                &error,
            );
            json_error(StatusCode::BAD_REQUEST, error)
        }
    }
}

/// 스킵 요청 하나의 결말 (V3 §10.5).
enum SkipDecision {
    /// 지금 넘어간다. `by_votes` 가 `Some(n)` 이면 **투표가 통과해서** 넘어간 것이라
    /// 활동 로그 문장이 `N명이 동의해서 곡을 넘겼어요` 가 된다 (§10.5 · §13.3).
    Immediate { by_votes: Option<usize> },
    /// 표만 더해졌다. 응답에 현재 표 상황이 들어 있다.
    Pending(Response),
}

/// 지금 이 스킵이 바로 넘어가는지, 투표를 여는지 정한다.
///
/// **즉시 스킵**: 서버 관리자·봇 주인 / 그 곡을 신청한 본인 / 모수가 0명일 때.
/// 마지막 조건이 없으면 아무도 안 듣고 아무도 안 보는 방에서 곡을 영영 못 넘긴다.
fn resolve_skip(
    state: &Arc<WebState>,
    ctx: &AuthContext,
    player: &crate::models::GuildPlayerState,
) -> SkipDecision {
    let guild_id = ctx.guild_id();
    if !ctx.settings.vote_skip_enabled {
        return SkipDecision::Immediate { by_votes: None };
    }
    let Some(current) = player.current_item.as_ref() else {
        return SkipDecision::Immediate { by_votes: None };
    };
    // 관리자·봇 주인, 그리고 내가 넣은 곡은 투표 없이 넘어간다.
    // 내가 넣은 곡을 내가 빼는 건 남에게 피해가 없다.
    if ctx.tier.is_manager() || current.requested_by_user_id == Some(ctx.user_id()) {
        return SkipDecision::Immediate { by_votes: None };
    }
    let listeners = listener_ids(state, guild_id);
    let viewers: HashSet<u64> = viewers_of(state, guild_id).into_iter().collect();
    if listeners.is_empty() && viewers.is_empty() {
        return SkipDecision::Immediate { by_votes: None };
    }

    let (quorum, mine, voters) = {
        let mut votes = state.skip_votes.lock().unwrap();
        // 곡이 바뀌었거나 90초가 지난 투표는 없던 것으로 한다 — 표가 다음 곡으로 넘어가면 안 된다.
        let stale = votes
            .get(&guild_id)
            .is_some_and(|vote| vote.item_id != current.id || vote.is_expired());
        if stale {
            votes.remove(&guild_id);
        }
        let vote = votes
            .entry(guild_id)
            .or_insert_with(|| SkipVoteState::new(current.id.clone()));
        // 다시 누르면 취소다.
        let mine = if vote.voters.remove(&ctx.user_id()) {
            false
        } else {
            vote.voters.insert(ctx.user_id());
            true
        };
        (
            skip_quorum(
                &listeners,
                &viewers,
                &vote.voters,
                ctx.settings.vote_skip_basis,
                ctx.settings.vote_skip_ratio,
                ctx.settings.vote_skip_min,
            ),
            mine,
            vote.voters.clone(),
        )
    };

    if quorum.passed {
        state.skip_votes.lock().unwrap().remove(&guild_id);
        // 활동 로그는 실제로 넘긴 뒤 `api_control` 이 한 번만 남긴다 —
        // 여기서도 남기면 한 번 스킵에 두 줄이 찍힌다. 대신 몇 명이 동의했는지를 남겨 둔다.
        emit(
            state,
            guild_id,
            "notice",
            json!({
                "kind": "info",
                "message": format!("{}명이 동의해서 곡을 넘겼어요.", quorum.have),
            }),
        );
        return SkipDecision::Immediate { by_votes: Some(quorum.have) };
    }

    let base = json!({
        "have": quorum.have,
        "need": quorum.need,
        "pool": quorum.pool,
        "basis": ctx.settings.vote_skip_basis.as_str(),
        "basisLabel": ctx.settings.vote_skip_basis.description(),
    });
    // 개인화 값(`mine`)은 사람마다 다르므로 브로드캐스트에 실으면 안 된다 (§10.5).
    emit_skip_vote(state, guild_id, &base, &voters);
    let mut payload = base;
    payload["mine"] = Value::Bool(mine);
    SkipDecision::Pending(json_ok(json!({
        "ok": true,
        "skipped": false,
        "vote": payload,
    })))
}

/// `{action:"skipVoteCancel"}` — 내 표만 거둔다 (V3 §10.5).
fn skip_vote_cancel(
    state: &Arc<WebState>,
    ctx: &AuthContext,
    player: &crate::models::GuildPlayerState,
) -> Response {
    let guild_id = ctx.guild_id();
    let mut votes = state.skip_votes.lock().unwrap();
    if let Some(vote) = votes.get_mut(&guild_id) {
        vote.voters.remove(&ctx.user_id());
        if vote.voters.is_empty() {
            votes.remove(&guild_id);
        }
    }
    let remaining = votes
        .get(&guild_id)
        .map(|vote| vote.voters.clone())
        .unwrap_or_default();
    drop(votes);
    let payload = skip_vote_json(state, ctx, player);
    if payload.is_null() {
        // 투표가 통째로 사라졌다 — 모두에게 똑같이 "없음"이다.
        emit(state, guild_id, "skipvote", Value::Null);
    } else {
        emit_skip_vote(state, guild_id, &payload, &remaining);
    }
    json_ok(json!({ "ok": true, "skipped": false, "vote": payload }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoteRequest {
    item_id: String,
    /// `"like"` · `"superLike"` · `"dislike"` · `null`(취소).
    /// 문자열로 받아 직접 판별한다 — 모르는 값에 422 대신 **이유가 담긴 400**을 주기 위해서다.
    kind: Option<String>,
}

async fn api_vote(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<VoteRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require("vote", ctx.settings.vote_rule, "투표할 권한이 없어요.") {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    if rate_limited(
        &state,
        guild_id,
        ctx.user_id(),
        "vote",
        Duration::from_millis(250),
    ) {
        return json_error(StatusCode::TOO_MANY_REQUESTS, "투표 요청이 너무 빨라요. 잠깐만 쉬었다 해요.");
    }
    let player = state.app.player.get_state(guild_id).await;
    // **지금 나오는 곡에도 투표할 수 있다** (V3 §10.7). 대기열만 보면 재생이 시작되는
    // 순간 좋아요 버튼이 사라져서, 정작 곡을 듣고 판단이 선 시점에 누를 수가 없다.
    // 점수는 우리 차트와 개인 통계로 가고, 이미 나가고 있는 곡의 순서는 바꾸지 않는다.
    let Some(item) = player
        .upcoming
        .iter()
        .chain(player.current_item.iter())
        .find(|item| item.id == request.item_id)
    else {
        return json_error(StatusCode::NOT_FOUND, "그 곡을 찾지 못했어요.");
    };
    if item.requested_by_user_id == Some(ctx.user_id()) {
        return json_error(
            StatusCode::FORBIDDEN,
            "자기가 신청한 곡에는 투표할 수 없어요.",
        );
    }
    let item = item.clone();

    // 종류 판별. 모르는 값에는 **이유를 말하고** 거절한다 (§23.3).
    let kind = match request.kind.as_deref() {
        None | Some("") | Some("none") => None,
        // `parse_vote_kind` 가 `"dislike"` 를 이미 받으므로 "아직 미지원" 분기는 도달할 수 없다.
        // 남겨 두면 다음 사람이 싫어요가 미구현이라고 오해한다 (V3 §10.2).
        Some(raw) => match parse_vote_kind(raw) {
            Some(kind) => Some(kind),
            None => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "투표 종류는 like · superLike · dislike 중 하나예요.",
                );
            }
        },
    };

    let points = ctx.settings.vote_points();
    let previous = state.app.remote.user_vote(&item.id, ctx.user_id());
    let is_super = kind == Some(QueueVoteKind::SuperLike);
    let was_super = previous == Some(QueueVoteKind::SuperLike);

    // 슈퍼 좋아요 쿨타임·하루 제한 (V3 §10.6). 관리자·봇 주인도 **똑같이** 적용된다 —
    // 여기서 예외를 두면 그게 특혜다. 거절할 때는 이유를 정확히 말한다 (§23.3).
    //
    // **먼저 검사만 하고 저장이 성공한 뒤에 소비한다.** 저장이 실패했는데 하루 횟수만
    // 깎이면 사용자가 이유 없이 한 번을 잃는다.
    if is_super && !was_super {
        let verdict = state.app.remote.check_super_like(
            guild_id,
            ctx.user_id(),
            ctx.settings.super_like_cooldown_sec,
            ctx.settings.super_like_daily_limit,
        );
        if let Some(message) = verdict.message() {
            return json_error(StatusCode::TOO_MANY_REQUESTS, message);
        }
    }

    if let Err(error) =
        state
            .app
            .remote
            .set_vote(guild_id, &item.id, ctx.user_id(), kind, &item.track)
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    // 슈퍼를 취소하면 사용 횟수를 돌려준다. 실수로 누른 걸 하루 종일 못 쓰게 하면 가혹하다.
    if is_super && !was_super {
        let _ = state.app.remote.consume_super_like(
            guild_id,
            ctx.user_id(),
            ctx.settings.super_like_cooldown_sec,
            ctx.settings.super_like_daily_limit,
        );
    } else if was_super && !is_super {
        state.app.remote.refund_super_like(guild_id, ctx.user_id());
    }

    // §22.3 투표 통계 — 누른 것(`*_give`)과 받은 것(`*_recv`)을 같이 센다.
    // 이게 없으면 `받은 반응: 👍0 ⭐0 👎0` 이 영원히 0이고, §15.2b `많이 사랑받은 곡`
    // 차트 2장이 **구조적으로** 빈 목록이 된다.
    {
        let (cache_key, track_json) = crate::stats::track_parts(&item.track);
        let flavor_of = |kind: QueueVoteKind| match kind {
            QueueVoteKind::Like => crate::stats::VoteFlavor::Like,
            QueueVoteKind::SuperLike => crate::stats::VoteFlavor::Super,
            QueueVoteKind::Dislike => crate::stats::VoteFlavor::Dislike,
        };
        // 종류를 바꾼 경우(좋아요 → 슈퍼)는 **뗀 것과 누른 것 둘 다** 던진다.
        // 안 그러면 좋아요 수가 줄지 않아 차트가 부풀어 오른다.
        if previous != kind {
            if let Some(old) = previous {
                record_stat(
                    &state,
                    crate::stats::StatEvent::Vote {
                        guild_id,
                        voter_id: ctx.user_id(),
                        owner_id: item.requested_by_user_id,
                        cache_key: cache_key.clone(),
                        track_json: track_json.clone(),
                        flavor: flavor_of(old),
                        added: false,
                    },
                );
            }
            if let Some(new) = kind {
                record_stat(
                    &state,
                    crate::stats::StatEvent::Vote {
                        guild_id,
                        voter_id: ctx.user_id(),
                        owner_id: item.requested_by_user_id,
                        cache_key,
                        track_json,
                        flavor: flavor_of(new),
                        added: true,
                    },
                );
            }
        }
    }

    state.app.player.refresh_scored_order(guild_id).await;
    // 사람이 읽는 피드용 액션명 (V3 §13.3). **취소는 기록하지 않는다** —
    // 눌렀다 뗐다 반복하면 피드가 도배된다.
    if let Some(kind) = kind {
        audit_ok(
            &state,
            guild_id,
            &ctx.session,
            kind.audit_action(),
            Some(item.track.display_title()),
            Some(kind.api_key()),
        );
    }

    // 투표는 그 항목 한 줄만 바꾼다 — 전체 재조회를 유발하지 않는다.
    let score = state
        .app
        .remote
        .queue_scores(guild_id)
        .get(&item.id)
        .cloned()
        .unwrap_or_default();
    emit(
        &state,
        guild_id,
        "vote",
        json!({
            "itemId": item.id,
            "like": score.like_count,
            "super": score.super_like_count,
            "dislike": score.dislike_count,
            "total": score.total_score(&points),
            "formula": score.formula(&points),
            "likeBy": voter_ids_json(&score.like_by),
            "superBy": voter_ids_json(&score.super_by),
            "dislikeBy": voter_ids_json(&score.dislike_by),
        }),
    );

    // 붐따 — 싫어요가 모이면 대기열에서 내린다 (V3 §10.3).
    // **재생 중인 곡에는 적용하지 않는다.** 여기서 찾은 항목은 `upcoming` 뿐이라 그 조건은 이미 참이다.
    //
    // **한 번만 발화한다.** 임계를 넘은 뒤에도 👎가 하나 더 붙을 때마다 조건이 계속 참이라,
    // 예전에는 그때마다 다시 돌아 활동 로그와 토스트를 도배했다. 이번 요청이 실제로
    // 👎를 **더한** 경우에만, 그리고 아직 내려가지 않은 곡에만 건다.
    let mut boomtta_fired = false;
    let just_disliked = kind == Some(QueueVoteKind::Dislike) && previous != kind;
    let already_boomtta = score.manual_priority.is_some_and(|priority| priority < 0);
    if just_disliked && !already_boomtta && score.boomtta_triggered(&ctx.settings) {
        boomtta_fired = apply_boomtta(&state, &ctx, &item, score.dislike_count).await;
    }

    broadcast_queue(&state, guild_id).await;
    json_ok(json!({
        "ok": true,
        "boomtta": boomtta_fired,
        "superLike": super_like_status(&state, guild_id, ctx.user_id(), &ctx.settings),
    }))
}

/// 한 번에 담기 결과 (V3 §12.3 · §15.4 · §18.2).
struct BulkOutcome {
    source: String,
    added: usize,
    /// 차단·길이 초과·중복 등으로 건너뛴 곡.
    skipped: usize,
    /// 대기열 상한이나 한 번에 담기 상한(기본 200곡)에 걸려 못 담은 곡.
    limited: usize,
}

impl BulkOutcome {
    /// **조용히 자르지 않는다.** 몇 곡이 왜 안 들어갔는지 한 문장으로 말한다.
    fn message(&self) -> String {
        let mut text = format!("{} 에서 {}곡을 담았어요.", self.source, self.added);
        if self.limited > 0 {
            text.push_str(&format!(" 대기열 한도 때문에 {}곡은 못 담았어요.", self.limited));
        }
        if self.skipped > 0 {
            text.push_str(&format!(
                " 차단·길이 제한·중복으로 {}곡은 건너뛰었어요.",
                self.skipped
            ));
        }
        if self.added == 0 {
            text = format!("{} 에서 담을 수 있는 곡이 없었어요.", self.source);
            if self.limited > 0 {
                text.push_str(" 대기열이 꽉 찼어요.");
            }
        }
        text
    }

    fn to_json(&self) -> Value {
        json!({
            "ok": self.added > 0,
            "added": self.added,
            "skipped": self.skipped,
            "limited": self.limited,
            "message": self.message(),
        })
    }
}

/// 여러 곡을 상한을 지키며 담는다.
///
/// `0` 은 무제한이고(§23.1), 그와 별개로 **한 번에 200곡**이라는 상한이 있다(§18.2).
/// 클릭 한 번이 대기열을 5000곡으로 만들면 되돌리기가 너무 어렵기 때문이다.
async fn bulk_enqueue(
    state: &Arc<WebState>,
    ctx: &AuthContext,
    tracks: &[TrackRef],
    existing: &HashSet<String>,
    player: &crate::models::GuildPlayerState,
    source: &str,
) -> BulkOutcome {
    let guild_id = ctx.guild_id();
    let session = &ctx.session;
    let own = player
        .upcoming
        .iter()
        .filter(|item| item.requested_by_user_id == Some(session.user_id))
        .count();
    let room = |limit: i32, used: usize| -> usize {
        if limit <= 0 {
            usize::MAX
        } else {
            (limit as usize).saturating_sub(used)
        }
    };
    // 한 번에 담는 양 자체의 상한 (§18.2 (4)). 길드 설정을 그대로 따르고 `0` 이면 무제한이다(§23.1).
    // 클릭 한 번이 대기열을 5000곡으로 만들면 되돌리기가 너무 어려워서 기본값이 200곡이다.
    let per_call = as_limit_u32(ctx.settings.bulk_enqueue_limit)
        .map(|limit| limit as usize)
        .unwrap_or(usize::MAX);
    let cap = room(ctx.settings.max_queue_per_guild, player.upcoming.len())
        .min(room(ctx.settings.max_queue_per_user, own))
        .min(per_call);

    let mut outcome = BulkOutcome {
        source: source.to_string(),
        added: 0,
        skipped: 0,
        limited: 0,
    };
    let mut seen: HashSet<String> = existing.clone();
    for track in tracks {
        let key = track.cache_key();
        if seen.contains(&key)
            || track_too_long(ctx.settings.max_track_seconds, track)
            || state.app.blacklist.is_blocked(guild_id, track)
            || !crate::media::resolver::can_resolve(&track.source_url)
        {
            outcome.skipped += 1;
            continue;
        }
        if outcome.added >= cap {
            outcome.limited += 1;
            continue;
        }
        seen.insert(key);
        state
            .app
            .player
            // §22.3 `queued_bulk` — 곡 하나하나가 "담은 곡"이되 한 번에 담은 것으로 갈라 남는다.
            // 통계는 `enqueue_bulk` 안에서 남는다.
            .enqueue_bulk(
                guild_id,
                QueueItem::new_user(
                    track.clone(),
                    session.display_name.clone(),
                    Some(session.user_id),
                ),
                false,
            )
            .await;
        outcome.added += 1;
    }
    // §22.3 `bulk_times` — 곡 수와 별개로 "한 번에 담기를 쓴 횟수"를 센다.
    // 실제로 한 곡이라도 들어갔을 때만 센다(전부 막힌 시도는 쓴 게 아니다).
    if outcome.added > 0 {
        record_stat(
            state,
            crate::stats::StatEvent::BulkUsed {
                guild_id,
                user_id: session.user_id,
            },
        );
    }
    if outcome.added > 0 && !session.is_developer {
        state.app.coordinator.sync_guild(&state.app, guild_id).await;
    }
    outcome
}

/// 붐따 실행 (V3 §10.3). 맨 뒤로 보내거나 아예 뺀다.
///
/// 조용히 사라지면 그게 사고다 — 활동 로그에 남기고 접속자에게 토스트를 띄운다.
async fn apply_boomtta(
    state: &Arc<WebState>,
    ctx: &AuthContext,
    item: &QueueItem,
    dislikes: i32,
) -> bool {
    let guild_id = ctx.guild_id();
    let remove = ctx.settings.boomtta_action == crate::remote::BoomttaAction::Remove;
    let title = item.track.display_title().to_string();
    let done = if remove {
        matches!(
            state.app.player.cancel_by_id(guild_id, &item.id).await,
            crate::player::manager::CancelOutcome::RemovedUpcoming(_)
        )
    } else {
        // 맨 뒤로 — 음수 우선순위를 주면 정렬이 알아서 꼬리로 보낸다.
        state
            .app
            .player
            .set_manual_priority(guild_id, &item.id, Some(-1_000_000))
            .await
            .is_ok()
    };
    if !done {
        return false;
    }
    // 통계에도 남긴다. 이게 빠지면 §22 의 "내 곡이 붐따당한 수"가 영원히 0 이고
    // 마참 점수의 감점 항목도 죽는다. 실제로 한 번 빠졌던 자리다.
    state.app.player.record_boomtta(guild_id, item);
    let _ = state.app.remote.add_audit(
        guild_id,
        ctx.user_id(),
        &ctx.session.display_name,
        "queue.boomtta",
        Some(&title),
        None,
        Some(if remove { "removed" } else { "bottom" }),
        true,
        None,
    );
    emit(
        state,
        guild_id,
        "notice",
        json!({
            "kind": "warn",
            "message": format!(
                "{title} 이 싫어요 {dislikes}개로 대기열{}",
                if remove { "에서 빠졌어요" } else { " 맨 뒤로 갔어요" }
            ),
        }),
    );
    emit_bare(state, guild_id, "audit");
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueActionRequest {
    action: String,
    /// `remove`/`togglePin` 은 필수, `clear` 는 대상이 없다.
    /// **필수로 두면** `clear` 요청이 본문 역직렬화 단계에서 422 로 떨어져
    /// "왜 안 되는지"조차 못 알려 준다 (V3 §18.2(5) · §23.3).
    #[serde(default)]
    item_id: Option<String>,
}

async fn api_queue_action(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<QueueActionRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if ctx.tier.is_viewer() {
        return json_error(
            StatusCode::FORBIDDEN,
            ctx.viewer_reason
                .clone()
                .unwrap_or_else(|| "지금은 읽기 전용이에요.".into()),
        );
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    let player = state.app.player.get_state(guild_id).await;
    // **대기열 비우기** (V3 §18.2(5)). 상한을 10000곡까지 열어 준 대가로 되돌릴 수단이
    // 하나는 있어야 한다. 대상 항목이 없는 유일한 작업이라 조회보다 먼저 처리한다.
    if request.action == "clear" {
        if let Err(response) = ctx.require_manager() {
            return response;
        }
        let removed = player.upcoming.len();
        if removed == 0 {
            return json_error(StatusCode::CONFLICT, "지금은 대기열이 비어 있어요.");
        }
        state.app.player.clear_queue(guild_id).await;
        // `대기열 N곡을 비웠어요` 의 N 은 로그의 `count` 칸이다 — 숫자를 실을 수 있는
        // `add_audit_bulk` 로 남긴다. `audit_ok` 로 남기면 언제나 `1곡` 이 된다.
        let _ = state.app.remote.add_audit_bulk(
            guild_id,
            ctx.user_id(),
            &ctx.session.display_name,
            "queue.clear",
            None,
            removed as u32,
            &[],
        );
        emit_bare(&state, guild_id, "audit");
        emit(
            &state,
            guild_id,
            "notice",
            json!({
                "kind": "warn",
                "message": format!("대기열 {removed}곡을 비웠어요."),
            }),
        );
        broadcast_queue(&state, guild_id).await;
        return json_ok(json!({ "ok": true, "removed": removed }));
    }
    let Some(item_id) = request.item_id.as_deref().filter(|id| !id.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "어떤 곡인지 알려 주지 않았어요.");
    };
    let Some(index) = player.upcoming.iter().position(|item| item.id == item_id) else {
        return json_error(StatusCode::NOT_FOUND, "그 대기열 항목을 찾지 못했어요.");
    };
    let item = &player.upcoming[index];
    match request.action.as_str() {
        "remove" => {
            let own = item.requested_by_user_id == Some(ctx.user_id());
            if !own && !ctx.allows("queueEdit", ctx.settings.queue_edit_rule) {
                return json_error(StatusCode::FORBIDDEN, "이 곡을 뺄 권한이 없어요.");
            }
            let title = item.track.display_title().to_string();
            if !matches!(
                state
                    .app
                    .player
                    .cancel_by_id(guild_id, item_id)
                    .await,
                crate::player::manager::CancelOutcome::RemovedUpcoming(_)
            ) {
                return json_error(
                    StatusCode::CONFLICT,
                    "대기열이 그새 바뀌었어요. 화면을 새로 받아 볼게요.",
                );
            }
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "queue.remove",
                Some(&title),
                Some("removed"),
            );
        }
        "togglePin" => {
            if let Err(response) = ctx.require_manager() {
                return response;
            }
            let scores = state.app.remote.queue_scores(guild_id);
            // **핀만 토글한다.** 붐따가 준 음수 우선순위(§10.3)까지 "핀이 걸려 있다"로 읽으면
            // 미움받은 곡에 📌를 눌렀을 때 맨 앞으로 가는 대신 조용히 붐따가 풀린다.
            let pinned = scores
                .get(item_id)
                .and_then(|score| score.manual_priority)
                .is_some_and(|priority| priority > 0);
            let new_priority = if pinned { None } else { Some(1_000_000) };
            if let Err(error) = state
                .app
                .player
                .set_manual_priority(guild_id, item_id, new_priority)
                .await
            {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, error);
            }
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                // 액션명이 `queue.force_move` 면 `audit_text` 의 catch-all 로 떨어져
                // 사람 피드에 `민수님이 queue.force_move 을 했어요` 가 그대로 나간다 (§13.3).
                "queue.pin",
                Some(item.track.display_title()),
                Some(if new_priority.is_some() {
                    "pinned"
                } else {
                    "unpinned"
                }),
            );
        }
        _ => return json_error(StatusCode::BAD_REQUEST, "지원하지 않는 대기열 작업이에요."),
    }
    broadcast_queue(&state, guild_id).await;
    json_ok(json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryRequest {
    track: TrackRef,
    kind: UserTrackKind,
    present: bool,
}

/// **S1 수정**: 예전에는 권한 규칙이 아예 없어 Viewer도 보관함을 고칠 수 있었다.
async fn api_library(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(mut request): Json<LibraryRequest>,
) -> Response {
    // 브라우저 검색으로 온 곡은 sourceUrl 이 빌 수 있다 (§23.4).
    request.track.ensure_source_url();
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    // 보관함은 "멤버라면 누구나"가 기준이지만, Viewer와 전체 정지자는 막힌다.
    if let Err(response) = ctx.require(
        "library",
        PermissionRule::GuildMember,
        "보관함을 쓸 권한이 없어요.",
    ) {
        return response;
    }
    if rate_limited(
        &state,
        guild_id,
        ctx.user_id(),
        "library",
        Duration::from_millis(250),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "보관함 요청이 너무 빨라요. 잠깐만 쉬었다 해요.",
        );
    }
    if let Err(error) = state.app.remote.set_user_track(
        guild_id,
        ctx.user_id(),
        request.kind,
        &request.track,
        request.present,
    ) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "library.change",
        Some(request.track.display_title()),
        Some(if request.present { "saved" } else { "removed" }),
    );
    // **개인 보관함은 본인에게만 알린다** (V3 §23.2). 전체로 뿌리면 🔖 한 번에
    // 접속자 전원이 `/state/cold`(멤버 전수 순회 + 쿼리 여러 개)를 다시 돌린다.
    emit_bare_to(&state, guild_id, ctx.user_id(), "library");
    json_ok(json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistActionRequest {
    action: String,
    playlist_id: Option<i64>,
    name: Option<String>,
    track: Option<TrackRef>,
    entry_index: Option<usize>,
    /// `entry_index` 의 다른 이름. 화면은 `entries[].id` 를 그대로 돌려준다 (§12.2).
    #[serde(default)]
    entry_id: Option<usize>,
    /// 자리 번호가 밀렸을 때의 대조용. 번호로 못 찾으면 이걸로 찾는다.
    #[serde(default)]
    cache_key: Option<String>,
    /// `"user"` 면 **개인 재생목록**(V3 §12), 그 밖은 서버 재생목록.
    /// `create` 에서만 의미가 있고, 나머지는 대상 재생목록의 실제 범위를 따른다.
    scope: Option<String>,
}

async fn api_playlist_action(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<PlaylistActionRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    // **개인 재생목록을 만들고 고치는 것은 권한 대상이 아니다** (V3 §12.3) — 내 것이니까.
    // 서버 재생목록만 `queueEdit` 을 본다. 어느 쪽인지는 아래에서 대상을 찾은 뒤 판단한다.
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    if ctx.tier.is_viewer() {
        return json_error(
            StatusCode::FORBIDDEN,
            ctx.viewer_reason
                .clone()
                .unwrap_or_else(|| "읽기 전용이라 아무것도 조작할 수 없어요.".into()),
        );
    }
    let session = &ctx.session;
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "playlist",
        Duration::from_millis(300),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "재생목록 요청이 너무 빨라요. 잠깐만 쉬었다 해요.",
        );
    }
    let name = request.name.as_deref().map(str::trim).unwrap_or("");
    let personal_new = request
        .scope
        .as_deref()
        .is_some_and(|scope| scope.eq_ignore_ascii_case("user"));
    let target = request
        .playlist_id
        .and_then(|id| state.app.db.find_playlist(id));
    // 대상이 개인 것인지 서버 것인지. `create` 는 요청의 `scope` 를 따른다.
    let personal = target
        .as_ref()
        .map(|playlist| playlist.scope == PlaylistScope::User)
        .unwrap_or(personal_new);
    if let Some(playlist) = target.as_ref() {
        match playlist.scope {
            // 개인 재생목록은 **내 것만** 보인다. 남의 것은 있는지조차 알려 주지 않는다.
            PlaylistScope::User => {
                if playlist.owner_user_id != session.user_id {
                    return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
                }
            }
            PlaylistScope::Guild => {
                if playlist.guild_id != Some(guild_id) {
                    return json_error(StatusCode::NOT_FOUND, "이 서버의 재생목록이 아니에요.");
                }
                // **서버 재생목록을 고치는 건 관리자 권한이다** (V3 §12.3).
                // 만든 사람이라는 이유로 열어 두면 화면(`can('console')` 로 잠금)과
                // 서버 판정이 어긋나서, 어느 쪽이 진짜인지 아무도 모르게 된다.
                if request.action != "enqueue" && !ctx.tier.is_manager() {
                    return json_error(
                        StatusCode::FORBIDDEN,
                        "서버 재생목록은 서버 관리자만 고칠 수 있어요.",
                    );
                }
            }
            PlaylistScope::Global => {
                return json_error(
                    StatusCode::FORBIDDEN,
                    "봇 전체 재생목록은 여기서 고칠 수 없어요.",
                );
            }
        }
    }
    // 서버 재생목록을 **만드는** 것도 관리자다 (V3 §12.3). 위 블록은 이미 있는 재생목록만
    // 검사하므로, 대상이 없는 `create` 는 여기서 막는다.
    if !personal && request.action != "enqueue" {
        if let Err(response) = ctx.require_manager() {
            return response;
        }
    }
    let audit_target = match request.action.as_str() {
        "create" => {
            if name.is_empty() || name.chars().count() > 80 {
                return json_error(StatusCode::BAD_REQUEST, "이름은 1~80자로 입력해요.");
            }
            let id = if personal_new {
                // 길드에 안 묶인다 — 내 재생목록은 어느 서버에서든 보인다 (§12.1).
                state.app.db.create_user_playlist(session.user_id, name)
            } else {
                state.app.db.create_playlist(
                    PlaylistScope::Guild,
                    Some(guild_id),
                    session.user_id,
                    name,
                )
            };
            // **`＋ 새로 만들어서 담기` 는 곡까지 담아야 한다** (§12.2).
            // 예전에는 `track` 을 무시하고도 `ok:true` 를 줘서, 화면은
            // `새 재생목록에 담았어요.` 를 띄우는데 실제로는 0곡이었다 —
            // 조용한 실패보다 나쁜 **거짓 성공**이다.
            if let Some(track) = request.track.as_ref() {
                if track_too_long(ctx.settings.max_track_seconds, track)
                    || state.app.blacklist.is_blocked(guild_id, track)
                {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "재생목록은 만들었는데, 곡 길이나 차단 규칙 때문에 그 곡은 담지 못했어요.",
                    );
                }
                state.app.db.add_playlist_entry(
                    id,
                    &PlaylistEntry {
                        track: Some(track.clone()),
                        collection: None,
                        start_offset: None,
                        extra: serde_json::Map::new(),
                    },
                );
            }
            format!("{id}:{name}")
        }
        "rename" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
            };
            if name.is_empty() || name.chars().count() > 80 {
                return json_error(StatusCode::BAD_REQUEST, "이름은 1~80자로 입력해요.");
            }
            if !state.app.db.rename_playlist(playlist.id, name) {
                return json_error(StatusCode::CONFLICT, "이름을 바꾸지 못했어요.");
            }
            format!("{}:{name}", playlist.id)
        }
        "delete" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
            };
            if !state.app.db.delete_playlist(playlist.id) {
                return json_error(StatusCode::CONFLICT, "재생목록을 지우지 못했어요.");
            }
            format!("{}:{}", playlist.id, playlist.name)
        }
        "addTrack" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
            };
            let Some(track) = request.track else {
                return json_error(StatusCode::BAD_REQUEST, "추가할 곡이 없어요.");
            };
            if track_too_long(ctx.settings.max_track_seconds, &track)
                || state.app.blacklist.is_blocked(guild_id, &track)
            {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "곡 길이나 차단 규칙 때문에 추가할 수 없어요.",
                );
            }
            state.app.db.add_playlist_entry(
                playlist.id,
                &PlaylistEntry {
                    track: Some(track.clone()),
                    collection: None,
                    start_offset: None,
                    extra: serde_json::Map::new(),
                },
            );
            format!("{}:{}", playlist.id, track.display_title())
        }
        // 화면이 쓰는 이름은 `removeTrack` 이다. 서버가 `removeEntry` 만 알면
        // 매칭되는 분기가 없어 `✕` 가 언제나 400 으로 떨어진다 (§12.2).
        "removeEntry" | "removeTrack" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
            };
            // 자리 번호(`entryIndex`/`entryId`) 우선, 못 찾으면 `cacheKey` 로 대조한다.
            // 카드가 5곡만 그리는 동안 목록이 바뀌면 번호가 밀리는데, 그때 엉뚱한 곡이
            // 지워지는 것보다 제목으로 찾는 게 낫다.
            let by_key = || {
                request.cache_key.as_deref().and_then(|key| {
                    playlist.entries.iter().position(|entry| {
                        entry
                            .track
                            .as_ref()
                            .is_some_and(|track| track.cache_key() == key)
                    })
                })
            };
            let index = match request.entry_index.or(request.entry_id) {
                Some(index) if index < playlist.entries.len() => {
                    // 번호가 가리키는 곡이 요청한 곡과 다르면 제목 쪽을 믿는다.
                    match request.cache_key.as_deref() {
                        Some(key)
                            if playlist.entries[index]
                                .track
                                .as_ref()
                                .is_none_or(|track| track.cache_key() != key) =>
                        {
                            by_key().unwrap_or(index)
                        }
                        _ => index,
                    }
                }
                _ => match by_key() {
                    Some(index) => index,
                    None => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "어떤 곡을 뺄지 알려 주지 않았어요.",
                        );
                    }
                },
            };
            if !state.app.db.remove_playlist_entry(playlist.id, index) {
                return json_error(StatusCode::NOT_FOUND, "재생목록에서 그 곡을 찾지 못했어요.");
            }
            format!("{}:{index}", playlist.id)
        }
        "enqueue" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
            };
            // 한 번에 수십 곡이 들어가 대기열을 점거할 수 있어 권한이 따로다 (V3 §15.4).
            if let Err(response) = ctx.require(
                "bulkEnqueue",
                ctx.settings.bulk_enqueue_rule,
                "재생목록을 통째로 담을 권한이 없어요.",
            ) {
                return response;
            }
            let tracks: Vec<TrackRef> = playlist
                .entries
                .iter()
                .filter_map(|e| e.track.clone())
                .collect();
            if tracks.is_empty() {
                return json_error(
                    StatusCode::CONFLICT,
                    "재생목록에 담을 수 있는 곡이 없어요.",
                );
            }
            let player = state.app.player.get_state(guild_id).await;
            let existing: HashSet<String> = player
                .current_item
                .iter()
                .chain(player.upcoming.iter())
                .map(|item| item.track.cache_key())
                .collect();
            // **조용히 자르지 않는다** (V3 §12.3). 담을 수 있는 만큼만 담고 몇 곡이 왜 빠졌는지 알려 준다.
            let outcome = bulk_enqueue(
                &state,
                &ctx,
                &tracks,
                &existing,
                &player,
                &format!("재생목록 {}", playlist.name),
            )
            .await;
            if outcome.added == 0 {
                return json_error(StatusCode::CONFLICT, outcome.message());
            }
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                "playlist.enqueue",
                Some(&playlist.name),
                None,
                Some(&outcome.added.to_string()),
                true,
                None,
            );
            emit(
                &state,
                guild_id,
                "notice",
                json!({ "kind": "info", "message": outcome.message() }),
            );
            format!("{}:{}", playlist.id, playlist.name)
        }
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "지원하지 않는 재생목록 작업이에요.",
            );
        }
    };
    // 액션 별칭(`removeTrack`)이 로그에 두 이름으로 남지 않게 정규화한다.
    let audit_action = match request.action.as_str() {
        "removeTrack" => "removeEntry",
        other => other,
    };
    audit_ok(
        &state,
        guild_id,
        session,
        &format!("playlist.{audit_action}"),
        Some(&audit_target),
        Some("ok"),
    );
    // 개인 재생목록은 본인만 볼 수 있으니 본인에게만 알린다 (§12.1 · §23.2).
    // 서버 재생목록은 모두에게 보이므로 그대로 브로드캐스트한다.
    if personal {
        emit_bare_to(&state, guild_id, session.user_id, "library");
    } else {
        emit_bare(&state, guild_id, "library");
    }
    broadcast_queue(&state, guild_id).await;
    json_ok(json!({ "ok": true }))
}

// ───────────────────────── 채팅 쓰기 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatRequest {
    content: String,
    #[serde(default)]
    reply_to_message_id: Option<i64>,
    #[serde(default)]
    tags: Vec<ChatTagRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatTagRequest {
    cache_key: String,
    track: TrackRef,
}

/// 채팅 라우트 공통 게이트: 채팅 기능 on/off + `chat_rule` + `Chat` 정지.
fn chat_gate(ctx: &AuthContext) -> Result<(), Response> {
    if !ctx.settings.chat_enabled {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "관리자가 채팅을 꺼 뒀어요.",
        ));
    }
    ctx.require("chat", ctx.settings.chat_rule, "채팅할 권한이 없어요.")?;
    ctx.require_not_suspended(SuspensionScope::Chat)
}

async fn api_chat(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = chat_gate(&ctx) {
        return response;
    }
    let content = request.content.trim();
    if content.is_empty() || content.chars().count() > 2000 {
        return json_error(StatusCode::BAD_REQUEST, "메시지는 1~2000자로 입력해요.");
    }
    {
        let mut rate = state.remote_chat_rate.lock().unwrap();
        if rate
            .get(&(guild_id, ctx.user_id()))
            .map(|last| last.elapsed() < Duration::from_millis(800))
            .unwrap_or(false)
        {
            return json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "메시지를 너무 빠르게 보내고 있어요. 숨 좀 돌려요.",
            );
        }
        rate.insert((guild_id, ctx.user_id()), Instant::now());
    }

    let message_id = match state.app.remote.add_chat_message(
        guild_id,
        ctx.user_id(),
        &ctx.session.display_name,
        ctx.session.avatar_url.as_deref(),
        content,
        request.reply_to_message_id,
    ) {
        Ok(id) => id,
        Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };

    // @멘션 — 후보는 이 서버에서 리모컨을 써 본 사람 + Discord 캐시.
    let candidates = mention_candidates(&state, guild_id);
    let names: Vec<String> = candidates
        .iter()
        .map(|(_, name, _)| name.clone())
        .collect();
    let mentioned: Vec<u64> = match_prefixed(content, '@', &names)
        .into_iter()
        .map(|index| candidates[index].0)
        .filter(|user_id| *user_id != ctx.user_id())
        .collect();
    if !mentioned.is_empty() {
        let _ = state
            .app
            .remote
            .set_chat_mentions(guild_id, message_id, &mentioned);
    }

    // #노래태그 — 프런트가 이미 후보를 확정해서 보내므로 그대로 저장한다.
    if !request.tags.is_empty() {
        let tags: Vec<ChatTrackTag> = request
            .tags
            .into_iter()
            .take(10)
            .map(|tag| ChatTrackTag {
                cache_key: if tag.cache_key.is_empty() {
                    tag.track.cache_key()
                } else {
                    tag.cache_key
                },
                track: tag.track,
            })
            .collect();
        let _ = state.app.remote.set_chat_tags(message_id, &tags);
    }

    // 방금 쓴 한 줄만 다시 읽어 그대로 실어 보낸다 — 전체 재조회가 일어나지 않는다.
    if let Some(message) = state
        .app
        .remote
        .get_chat_message(guild_id, message_id, ctx.user_id())
    {
        emit(
            &state,
            guild_id,
            "chat.add",
            chat_message_json(&message, 0),
        );
    }
    // §22.3 `chats` — 마참 점수의 채팅 항이 여기서만 채워진다.
    record_stat(
        &state,
        crate::stats::StatEvent::Chat {
            guild_id,
            user_id: ctx.user_id(),
        },
    );
    json_ok(json!({ "ok": true, "id": message_id }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatReactionRequest {
    message_id: i64,
    emoji: String,
}

async fn api_chat_reaction(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ChatReactionRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = chat_gate(&ctx) {
        return response;
    }
    if request.emoji.is_empty() || request.emoji.chars().count() > 8 {
        return json_error(StatusCode::BAD_REQUEST, "이모지가 올바르지 않아요.");
    }
    match state.app.remote.toggle_chat_reaction(
        guild_id,
        request.message_id,
        ctx.user_id(),
        &request.emoji,
    ) {
        Ok(active) => {
            // 해당 메시지 노드만 갱신된다.
            emit(
                &state,
                guild_id,
                "chat.react",
                json!({
                    "messageId": request.message_id,
                    "emoji": request.emoji,
                    "userId": ctx.user_id().to_string(),
                    "displayName": ctx.session.display_name,
                    "added": active,
                }),
            );
            json_ok(json!({ "ok": true, "active": active }))
        }
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatDeleteRequest {
    message_id: i64,
}

/// **S2 수정**: `chat_enabled` / `chat_rule` 검사가 빠져 있었다.
/// 단, 남의 메시지를 지우는 Manager는 채팅 규칙과 무관하게 통과해야 한다.
async fn api_chat_delete(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ChatDeleteRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let owner = state
        .app
        .remote
        .chat_message_owner(guild_id, request.message_id);
    let mine = owner == Some(ctx.user_id());
    if !ctx.tier.is_manager() {
        // 내 메시지를 지우는 것도 채팅 권한이 있어야 한다.
        if !mine {
            return json_error(StatusCode::FORBIDDEN, "이 메시지를 지울 권한이 없어요.");
        }
        if let Err(response) = chat_gate(&ctx) {
            return response;
        }
    }
    match state
        .app
        .remote
        .delete_chat_message(guild_id, request.message_id)
    {
        Ok(true) => {
            let deleted_utc = now_utc();
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "chat.delete",
                Some(&request.message_id.to_string()),
                Some("deleted"),
            );
            emit(
                &state,
                guild_id,
                "chat.delete",
                json!({ "messageId": request.message_id, "deletedUtc": deleted_utc }),
            );
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 메시지를 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn api_chat_read(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if ctx.tier.is_viewer() {
        return json_ok(json!({ "ok": true, "unread": 0 }));
    }
    let _ = state
        .app
        .remote
        .mark_mentions_read(guild_id, ctx.user_id());
    json_ok(json!({ "ok": true, "unread": 0 }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatReportRequest {
    message_id: i64,
    reason: String,
    resolve: Option<bool>,
    report_id: Option<i64>,
}

async fn api_chat_report(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ChatReportRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if request.resolve.unwrap_or(false) {
        if let Err(response) = ctx.require_manager() {
            return response;
        }
        let Some(report_id) = request.report_id else {
            return json_error(StatusCode::BAD_REQUEST, "신고 ID가 없어요.");
        };
        return match state.app.remote.resolve_chat_report(guild_id, report_id) {
            Ok(true) => {
                audit_ok(
                    &state,
                    guild_id,
                    &ctx.session,
                    "chat.report.resolve",
                    Some(&report_id.to_string()),
                    Some("resolved"),
                );
                json_ok(json!({ "ok": true }))
            }
            Ok(false) => json_error(StatusCode::NOT_FOUND, "그 신고를 찾지 못했어요."),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    }
    if let Err(response) = chat_gate(&ctx) {
        return response;
    }
    let reason = request.reason.trim();
    if reason.is_empty() || reason.chars().count() > 300 {
        return json_error(StatusCode::BAD_REQUEST, "신고 사유는 1~300자로 입력해요.");
    }
    if state
        .app
        .remote
        .chat_message_owner(guild_id, request.message_id)
        == Some(ctx.user_id())
    {
        return json_error(StatusCode::FORBIDDEN, "자기 메시지는 신고할 수 없어요.");
    }
    match state.app.remote.report_chat_message(
        guild_id,
        request.message_id,
        ctx.user_id(),
        &ctx.session.display_name,
        reason,
    ) {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "chat.report",
                Some(&request.message_id.to_string()),
                Some(reason),
            );
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 메시지를 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

// ───────────────────────── 제안 게시판 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuggestionCreateRequest {
    title: String,
    body: String,
}

async fn api_suggestion_create(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SuggestionCreateRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if !ctx.settings.suggestion_enabled {
        return json_error(StatusCode::FORBIDDEN, "관리자가 제안 게시판을 꺼 뒀어요.");
    }
    if let Err(response) = ctx.require("suggest", PermissionRule::GuildMember, "제안을 올릴 권한이 없어요.") {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Chat) {
        return response;
    }
    let title = request.title.trim();
    let body = request.body.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return json_error(StatusCode::BAD_REQUEST, "제목은 1~120자로 입력해요.");
    }
    if body.is_empty() || body.chars().count() > 4000 {
        return json_error(StatusCode::BAD_REQUEST, "내용은 1~4000자로 입력해요.");
    }
    if rate_limited(
        &state,
        guild_id,
        ctx.user_id(),
        "suggest",
        Duration::from_secs(10),
    ) {
        return json_error(StatusCode::TOO_MANY_REQUESTS, "제안은 10초에 하나만 올릴 수 있어요.");
    }
    match state.app.remote.create_suggestion(
        guild_id,
        ctx.user_id(),
        &ctx.session.display_name,
        ctx.session.avatar_url.as_deref(),
        title,
        body,
    ) {
        Ok(id) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "suggestion.create",
                Some(title),
                Some("open"),
            );
            emit(&state, guild_id, "suggestion.add", json!({ "id": id }));
            json_ok(json!({ "ok": true, "id": id }))
        }
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuggestionVoteRequest {
    suggestion_id: i64,
}

async fn api_suggestion_vote(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SuggestionVoteRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require("suggest", PermissionRule::GuildMember, "공감할 권한이 없어요.") {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Chat) {
        return response;
    }
    match state
        .app
        .remote
        .toggle_suggestion_vote(guild_id, request.suggestion_id, ctx.user_id())
    {
        Ok(Some(active)) => {
            emit(
                &state,
                guild_id,
                "suggestion.vote",
                json!({ "id": request.suggestion_id, "active": active }),
            );
            json_ok(json!({ "ok": true, "active": active }))
        }
        Ok(None) => json_error(StatusCode::NOT_FOUND, "그 제안을 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuggestionStatusRequest {
    suggestion_id: i64,
    status: String,
    note: Option<String>,
}

async fn api_suggestion_status(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SuggestionStatusRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require_manager() {
        return response;
    }
    let Some(status) = SuggestionStatus::parse(&request.status) else {
        return json_error(StatusCode::BAD_REQUEST, "알 수 없는 제안 상태예요.");
    };
    let note = request.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    match state.app.remote.set_suggestion_status(
        guild_id,
        request.suggestion_id,
        status,
        note,
        ctx.user_id(),
    ) {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "suggestion.status",
                Some(&request.suggestion_id.to_string()),
                Some(status.as_str()),
            );
            emit(
                &state,
                guild_id,
                "suggestion.status",
                json!({ "id": request.suggestion_id, "status": status.as_str() }),
            );
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 제안을 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

// ───────────────────────── 자동 재생 기준 곡 (V3 §8) ─────────────────────────
//
// 시드가 하나라도 있으면 추천 엔진이 그 곡들의 라디오를 라운드로빈으로 돈다.
// 하나도 없으면 지금처럼 최근 재생 기반으로 움직인다 — 그 설명은 화면이 한다.

fn autoplay_seed_json(state: &WebState, guild_id: u64, seed: &AutoplaySeed) -> Value {
    let display = state
        .app
        .discord_cache
        .get()
        .and_then(|cache| cache.guild(GuildId::new(guild_id)))
        .and_then(|guild| {
            guild
                .members
                .get(&UserId::new(seed.added_by_user_id))
                .map(|member| member.display_name().to_string())
        });
    json!({
        "cacheKey": seed.cache_key,
        "track": track_json(&seed.track),
        "addedByUserId": seed.added_by_user_id.to_string(),
        "addedByDisplayName": display.unwrap_or_else(|| seed.added_by_user_id.to_string()),
        "addedUtc": seed.added_utc,
    })
}

/// 목록 + 상한 + 내가 고칠 수 있는지. 권한이 없어도 **보이기는 한다**(V3 §8.5).
fn autoplay_payload(state: &WebState, guild_id: u64, can_edit: bool) -> Value {
    let settings = state.app.remote.load_guild_settings(guild_id);
    let seeds: Vec<Value> = state
        .app
        .remote
        .list_autoplay_seeds(guild_id)
        .iter()
        .map(|seed| autoplay_seed_json(state, guild_id, seed))
        .collect();
    json!({
        "seeds": seeds,
        // 상한은 길드 설정을 따르고 `0` 이면 무제한이다 (§23.1).
        "max": settings.seed_limit().unwrap_or(MAX_AUTOPLAY_SEEDS as u32),
        "unlimited": settings.seed_limit().is_none(),
        "canEdit": can_edit,
        // 추천 방식·정책 (V3 §8). 시드가 0개면 화면이 "최근에 튼 곡을 참고해요"를 띄운다.
        "mode": settings.autoplay_mode.as_str(),
        "modeLabel": settings.autoplay_mode.label(),
        "modeDescription": settings.autoplay_mode.description(),
        "recentCount": settings.autoplay_recent_count,
        "genres": settings.autoplay_genres,
        "policy": settings.autoplay_policy.as_str(),
        "policyLabel": settings.autoplay_policy.label(),
        "policyDescription": settings.autoplay_policy.description(),
        "artistCooldown": settings.autoplay_artist_cooldown,
        "recentDecayHours": settings.autoplay_recent_decay_hours,
        // **고를 수 있는 장르 목록** (V3 §8.6). 이게 없으면 유저 UI 는 `🎸 장르` 를 골라도
        // 선택 줄 자체를 안 그려서 `autoplay_genres` 가 영원히 비고, 폴백 사슬이 곧장 내려간다.
        // 관리 콘솔은 `/charts` 폴백이 있어 살아 있었지만 유저 UI 에는 그 폴백이 없다.
        "genreOptions": genre_options(state, guild_id),
        // **무엇이 추천 근거로 쌓이고 있는지** (V3 §8.7). 화면이 이걸 그대로 보여준다.
        // 이게 없으면 자동재생은 "어디서 나온지 모를 곡을 트는 기계"로 보인다.
        "basket": autoplay_basket(state, guild_id, &settings),
    })
}

/// 추천 바구니의 지금 상태. 담긴 것 · 자동으로 쌓인 것 · 빼 둔 것 세 칸이다.
fn autoplay_basket(
    state: &WebState,
    guild_id: u64,
    settings: &RemoteGuildSettings,
) -> Value {
    // 추천이 실제로 참고하는 만큼만 보여준다. 설정값보다 많이 보여주면
    // "이 곡도 참고하나 보다" 하고 오해한다.
    let window = settings.autoplay_recent_count.max(1) as usize;
    let recent: Vec<Value> = state
        .app
        .remote
        .list_recent(guild_id, window)
        .iter()
        .map(|item| {
            json!({
                // 한 줄만 지우려면 화면이 그 줄을 지목할 수 있어야 한다. 같은 곡을 여러 번
                // 틀면 `cacheKey` 가 겹치므로 행 id 가 유일한 식별자다.
                "id": item.id,
                "title": item.track.title.clone().unwrap_or_else(|| "제목 없음".into()),
                "artist": item.track.artist,
                "playedUtc": item.played_utc,
                "cacheKey": item.track.cache_key(),
            })
        })
        .collect();

    let blocked: Vec<Value> = state
        .app
        .remote
        .list_blocked_autoplay(guild_id)
        .into_iter()
        .map(|(cache_key, reason, until, track)| {
            json!({
                "cacheKey": cache_key,
                "reason": reason.unwrap_or_else(|| "이 곡 말고를 눌렀어요".into()),
                "untilUtc": until,
                // v20 부터 트랙을 같이 저장한다. 그 전에 빼 둔 곡은 백필로 채우는데,
                // 어디에도 흔적이 없으면 끝내 `null` 이다 — 화면이 그 사정을 말해야 한다.
                "track": track.as_ref().map(track_json),
                "title": track.as_ref().and_then(|t| t.title.clone()),
            })
        })
        .collect();

    json!({
        "recent": recent,
        "recentWindow": window,
        "blocked": blocked,
        // 지금 방식이 무엇을 실제로 참고하는지. 방식에 따라 안 쓰는 칸이 생긴다.
        "usesSeeds": matches!(settings.autoplay_mode, AutoplayMode::Seed),
        "usesRecent": matches!(settings.autoplay_mode, AutoplayMode::Recent),
        "usesGenres": matches!(settings.autoplay_mode, AutoplayMode::Genre),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplayResetRequest {
    /// `seeds` · `recent` · `blocked` · `all`
    scope: String,
}

/// `POST .../autoplay/reset` — 추천 바구니를 비운다 (V3 §8.7).
///
/// 기준 곡 편집과 같은 권한을 쓴다. 바구니를 비우는 건 기준 곡을 하나씩 빼는 것과
/// 결과가 같아서, 권한을 따로 두면 "하나씩은 되는데 전부는 안 되는" 이상한 상태가 된다.
async fn api_autoplay_reset(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AutoplayResetRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }

    let scope = request.scope.trim();
    let all = scope == "all";
    if !all && !matches!(scope, "seeds" | "recent" | "blocked") {
        return json_error(StatusCode::BAD_REQUEST, "비울 대상을 알 수 없어요.".to_string());
    }

    let mut cleared: Vec<String> = Vec::new();
    if all || scope == "seeds" {
        let n = state.app.remote.clear_autoplay_seeds(guild_id);
        if n > 0 {
            cleared.push(format!("기준 곡 {n}개"));
        }
    }
    if all || scope == "recent" {
        let n = state.app.remote.clear_recent(guild_id);
        if n > 0 {
            cleared.push(format!("최근 재생 {n}개"));
        }
    }
    if all || scope == "blocked" {
        let n = state.app.remote.clear_autoplay_blocked(guild_id);
        if n > 0 {
            cleared.push(format!("빼 둔 곡 {n}개"));
        }
    }

    let summary = if cleared.is_empty() {
        "이미 비어 있었어요.".to_string()
    } else {
        format!("{} 를 비웠어요.", cleared.join(", "))
    };
    // 바구니를 비우면 추천 성향이 통째로 바뀐다. 사람 피드에 반드시 남긴다 —
    // 남이 비웠는데 아무 말도 없으면 "추천이 갑자기 이상해졌다"로만 보인다.
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "autoplay.reset",
        Some(scope),
        Some(&summary),
    );

    let can_edit = ctx.allows("autoplay", ctx.settings.autoplay_rule);
    let mut payload = autoplay_payload(&state, guild_id, can_edit);
    payload["message"] = json!(summary);
    json_ok(payload)
}

/// 자동 재생 `genre` 모드가 고를 수 있는 차트 (§8.2). 키는 차트 ID 문자열이다 —
/// `seeds_for_mode` 가 `autoplay_genres` 를 차트 ID 로 파싱해 캐시를 읽기 때문이다.
///
/// 노래방 차트도 장르처럼 고를 수 있게 같이 싣는다. 실패한 차트는 뺀다 —
/// 고를 수는 있는데 아무 곡도 안 나오는 항목이 제일 나쁘다 (§15.2).
fn genre_options(state: &WebState, guild_id: u64) -> Vec<Value> {
    state
        .app
        .remote
        .list_charts(guild_id)
        .iter()
        .filter(|chart| matches!(chart.category, ChartCategory::Genre | ChartCategory::Karaoke))
        .filter(|chart| chart.enabled && chart.ok())
        .map(|chart| {
            json!({
                "key": chart.id.to_string(),
                "label": chart.name,
                "category": chart.category.as_str(),
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplayConfigRequest {
    mode: Option<String>,
    policy: Option<String>,
    recent_count: Option<u32>,
    genres: Option<Vec<String>>,
}

/// `PUT .../autoplay` — 추천 방식 부분 갱신 (V3 §8.6).
/// **일반 사용자도 바꿀 수 있다** — 권한은 `autoplay`(기본 모든 멤버)다.
async fn api_autoplay_put(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AutoplayConfigRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    let mut settings = ctx.settings.clone();
    if let Some(mode) = request.mode.as_deref() {
        let Some(mode) = AutoplayMode::parse(mode) else {
            return json_error(StatusCode::BAD_REQUEST, "알 수 없는 자동 재생 방식이에요.");
        };
        settings.autoplay_mode = mode;
    }
    if let Some(policy) = request.policy.as_deref() {
        let Some(policy) = AutoplayPolicy::parse(policy) else {
            return json_error(StatusCode::BAD_REQUEST, "알 수 없는 추천 정책이에요.");
        };
        settings.autoplay_policy = policy;
    }
    if let Some(count) = request.recent_count {
        // **`0` 은 무제한이다** (§23.1). `models.rs` 의 `recent_seed_limit()` 이 이미 그렇게 풀고
        // 화면 툴팁도 `0을 넣으면 최근에 튼 곡 전부를 참고해요` 라고 말하는데, 여기만
        // `1..=20` 이라 그 툴팁대로 하면 빨간 토스트가 떴다.
        if count > 20 {
            return json_error(
                StatusCode::BAD_REQUEST,
                "최근 N곡은 20까지예요. 0을 넣으면 전부 참고해요.",
            );
        }
        settings.autoplay_recent_count = count;
    }
    if let Some(genres) = request.genres {
        if genres.len() > 20 {
            return json_error(StatusCode::BAD_REQUEST, "장르는 20개까지 고를 수 있어요.");
        }
        settings.autoplay_genres = genres
            .into_iter()
            .map(|genre| genre.trim().to_string())
            .filter(|genre| !genre.is_empty())
            .collect();
    }
    settings.sanitize();
    // 추천에 실제로 영향을 주는 값이 바뀌었나. 이름만 바뀐 저장에까지 다시 뽑기를 돌리면
    // 아무 이유 없이 추천곡이 흔들린다.
    let recompute = settings.autoplay_mode != ctx.settings.autoplay_mode
        || settings.autoplay_policy != ctx.settings.autoplay_policy
        || settings.autoplay_recent_count != ctx.settings.autoplay_recent_count
        || settings.autoplay_genres != ctx.settings.autoplay_genres;
    if let Err(error) = state.app.remote.save_guild_settings(&settings) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "autoplay.config",
        None,
        Some(settings.autoplay_mode.as_str()),
    );
    // **정책을 바꾸면 지금 잡혀 있는 다음 추천곡을 다시 뽑는다** (V3 §8.5).
    // 안 그러면 다음 곡이 시작될 때까지 바뀐 게 먹었는지 확인할 방법이 없다.
    // 이미 잡혀 있던 후보를 차단하지는 않는다 — 사용자가 싫다고 한 게 아니라 규칙이 바뀐 것뿐이다.
    if recompute {
        // 여기도 기다리면 안 된다 — 추천은 yt-dlp 를 타서 10~20초씩 걸리고, 그동안 이 응답이
        // 안 나가 화면은 저장이 멈춘 것처럼 보인다. 다시 뽑히면 WS 로 따로 알려 준다.
        let app = state.app.clone();
        tokio::spawn(async move {
            crate::player::side_effects::refresh_preview(app, guild_id).await;
        });
    }
    broadcast_autoplay(&state, guild_id);
    emit_bare(&state, guild_id, "settings");
    json_ok(json!({ "ok": true, "autoplay": autoplay_payload(&state, guild_id, true) }))
}

/// `GET .../autoplay` — 시드 + 추천 방식 (V3 §8.6).
async fn api_autoplay_get(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let can_edit = ctx.allows("autoplay", ctx.settings.autoplay_rule);
    json_ok(autoplay_payload(&state, guild_id, can_edit))
}

/// 시드가 바뀌면 보고 있는 사람 모두에게 알린다. 개인화 필드가 없어서 그대로 보내도 된다.
fn broadcast_autoplay(state: &Arc<WebState>, guild_id: u64) {
    let payload = autoplay_payload(state, guild_id, true);
    emit(state, guild_id, "autoplay", payload);
}

async fn api_autoplay_seeds(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let can_edit = ctx.allows("autoplay", ctx.settings.autoplay_rule);
    json_ok(autoplay_payload(&state, guild_id, can_edit))
}

/// 기준 곡 편집 공통 게이트 — 권한 + 신청 정지.
fn autoplay_gate(ctx: &AuthContext) -> Result<(), Response> {
    ctx.require(
        "autoplay",
        ctx.settings.autoplay_rule,
        "자동 재생 기준 곡을 바꿀 권한이 없어요.",
    )?;
    ctx.require_not_suspended(SuspensionScope::Queue)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplaySeedAddRequest {
    track: TrackRef,
}

async fn api_autoplay_seed_add(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(mut request): Json<AutoplaySeedAddRequest>,
) -> Response {
    // 브라우저 검색으로 온 곡은 sourceUrl 이 빌 수 있다 (§23.4).
    request.track.ensure_source_url();
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    if !crate::media::resolver::can_resolve(&request.track.source_url) {
        return json_error(StatusCode::BAD_REQUEST, "지원하지 않는 곡 URL이에요.");
    }
    if state.app.blacklist.is_blocked(guild_id, &request.track) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "차단된 곡은 기준 곡으로 삼을 수 없어요.",
        );
    }
    let title = request.track.display_title().to_string();
    match state
        .app
        .remote
        .add_autoplay_seed(guild_id, &request.track, ctx.user_id())
    {
        Ok(SeedAddOutcome::Added) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "autoplay.seed.add",
                Some(&title),
                Some("added"),
            );
            broadcast_autoplay(&state, guild_id);
            json_ok(json!({ "ok": true, "message": SeedAddOutcome::Added.message() }))
        }
        // 거절 사유 문구는 저장소가 들고 있는 걸 그대로 쓴다 — 화면과 서버가 다른 말을 하면 안 된다.
        Ok(outcome) => json_error(StatusCode::BAD_REQUEST, outcome.message()),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplaySeedRemoveRequest {
    cache_key: String,
}

async fn api_autoplay_seed_remove(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AutoplaySeedRemoveRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    match state
        .app
        .remote
        .remove_autoplay_seed(guild_id, request.cache_key.trim())
    {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "autoplay.seed.remove",
                Some(request.cache_key.trim()),
                Some("removed"),
            );
            broadcast_autoplay(&state, guild_id);
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 기준 곡을 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// 최근 재생은 **행 id** 로 지운다. 같은 곡을 여러 번 틀면 같은 `cacheKey` 가 여러 줄
/// 쌓이는데, 키로 지우면 "이 한 번"을 빼려던 게 그 곡 이력을 통째로 날린다.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplayRecentRemoveRequest {
    id: i64,
}

async fn api_autoplay_recent_remove(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AutoplayRecentRemoveRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    match state.app.remote.remove_recent(guild_id, request.id) {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "autoplay.recent.remove",
                Some(&request.id.to_string()),
                Some("removed"),
            );
            broadcast_autoplay(&state, guild_id);
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 기록을 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// 빼 둔 곡은 `cacheKey` 로 푼다 — 곡 하나당 한 줄이라 키가 곧 그 줄이다.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplayBlockedRemoveRequest {
    cache_key: String,
}

async fn api_autoplay_blocked_remove(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AutoplayBlockedRemoveRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    let key = request.cache_key.trim();
    match state.app.remote.unblock_autoplay_candidate(guild_id, key) {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "autoplay.blocked.remove",
                Some(key),
                Some("unblocked"),
            );
            broadcast_autoplay(&state, guild_id);
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 곡은 빼 둔 목록에 없어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoplaySeedReorderRequest {
    cache_keys: Vec<String>,
}

async fn api_autoplay_seeds_reorder(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AutoplaySeedReorderRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    // 상한은 **길드 설정**을 따른다 (§23.1). 여기만 10 으로 하드코딩해 두면
    // 관리자가 상한을 20으로 올려 15곡을 넣었을 때 담기는 되고 정렬만 400 으로 막힌다.
    if let Some(limit) = ctx.settings.seed_limit() {
        if request.cache_keys.len() > limit as usize {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("기준 곡은 {limit}곡까지예요."),
            );
        }
    }
    match state
        .app
        .remote
        .reorder_autoplay_seeds(guild_id, &request.cache_keys)
    {
        Ok(()) => {
            broadcast_autoplay(&state, guild_id);
            json_ok(json!({ "ok": true }))
        }
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// `POST .../autoplay/reroll` — `📻 이 곡 말고` (V3 §8.5-3 · §14.3).
///
/// 지금 잡혀 있는 다음 자동추천곡을 **7일간 다시 안 뽑히게** 하고 하나를 새로 뽑는다.
/// 저장소(`remote_autoplay_blocked`)와 엔진은 이미 있었는데 **라우트가 없어서**
/// 화면의 버튼이 404 로 떨어지고, 그 결과 차단 목록이 영원히 비어 있었다.
async fn api_autoplay_reroll(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = autoplay_gate(&ctx) {
        return response;
    }
    let rerolled = crate::player::side_effects::reject_preview(
        state.app.clone(),
        guild_id,
        "리모컨에서 이 곡 말고를 눌렀어요",
    )
    .await;
    if !rerolled {
        return json_error(
            StatusCode::CONFLICT,
            "지금은 다시 뽑을 추천곡이 없어요.",
        );
    }
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "autoplay.reroll",
        None,
        Some("rerolled"),
    );
    // `next` 는 `playback` 프레임에만 실린다 — 다시 뽑은 결과가 바로 보여야 한다 (§14.4).
    let player = state.app.player.get_state(guild_id).await;
    let position = state
        .app
        .coordinator
        .current_position(guild_id)
        .await
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0);
    emit(
        &state,
        guild_id,
        "playback",
        playback_payload(&state, &player, position, &now_utc(), None, state.app.coordinator.schedule(guild_id).await),
    );
    json_ok(json!({ "ok": true }))
}

// ───────────────────────── 유저 정지 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuspendRequest {
    user_id: String,
    scope: String,
    /// `null` 또는 0 = 무기한.
    minutes: Option<i64>,
    reason: Option<String>,
}

async fn api_suspend(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SuspendRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    suspend_impl(&state, &ctx, request).await
}

async fn suspend_impl(
    state: &Arc<WebState>,
    ctx: &AuthContext,
    request: SuspendRequest,
) -> Response {
    if let Err(response) = ctx.require_manager() {
        return response;
    }
    let guild_id = ctx.guild_id();
    let Ok(target) = request.user_id.trim().parse::<u64>() else {
        return json_error(StatusCode::BAD_REQUEST, "대상 사용자 ID가 올바르지 않아요.");
    };
    let Some(scope) = SuspensionScope::parse(&request.scope) else {
        return json_error(StatusCode::BAD_REQUEST, "정지 범위는 all / chat / queue 중 하나여야 해요.");
    };
    if target == ctx.user_id() {
        return json_error(StatusCode::BAD_REQUEST, "자기 자신은 정지할 수 없어요.");
    }
    // 관리자는 다른 관리자를 정지할 수 없다. 봇 주인만 가능하다 (사양서 §1.2 마지막 줄).
    let target_tier = tier_of_member(state, guild_id, target, &ctx.settings);
    if target_tier.is_manager() && !ctx.tier.is_owner() {
        return json_error(
            StatusCode::FORBIDDEN,
            "관리자와 봇 주인은 봇 주인만 정지할 수 있어요.",
        );
    }
    if target_tier.is_owner() {
        return json_error(StatusCode::FORBIDDEN, "봇 주인은 정지할 수 없어요.");
    }
    let expires = match request.minutes {
        Some(minutes) if minutes > 0 => Some(
            (chrono::Utc::now() + chrono::Duration::minutes(minutes.min(60 * 24 * 365)))
                .to_rfc3339(),
        ),
        _ => None,
    };
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Err(error) = state.app.remote.suspend_user(
        guild_id,
        target,
        scope,
        reason,
        ctx.user_id(),
        expires.as_deref(),
    ) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    audit_ok(
        state,
        guild_id,
        &ctx.session,
        "user.suspend",
        Some(&target.to_string()),
        Some(scope.as_str()),
    );
    emit(
        state,
        guild_id,
        "suspension",
        json!({
            "scope": scope.as_str(),
            "userId": target.to_string(),
            "reason": reason,
            "expiresUtc": expires,
        }),
    );
    json_ok(json!({ "ok": true }))
}

/// Discord 캐시만 보고 이 사람의 등급을 추정한다(정지 대상 보호용).
fn tier_of_member(
    state: &WebState,
    guild_id: u64,
    user_id: u64,
    settings: &RemoteGuildSettings,
) -> AccessTier {
    if is_owner_user(state, user_id) {
        return AccessTier::Owner;
    }
    let Some(cache) = state.app.discord_cache.get() else {
        return AccessTier::Member;
    };
    let Some(guild) = cache.guild(GuildId::new(guild_id)) else {
        return AccessTier::Member;
    };
    if guild.owner_id.get() == user_id {
        return AccessTier::Manager;
    }
    let Some(member) = guild.members.get(&UserId::new(user_id)) else {
        return AccessTier::Member;
    };
    let admin = member.roles.iter().any(|role| {
        guild
            .roles
            .get(role)
            .map(|role| {
                role.permissions.contains(Permissions::ADMINISTRATOR)
                    || role.permissions.contains(Permissions::MANAGE_GUILD)
            })
            .unwrap_or(false)
    });
    let manager_role = member
        .roles
        .iter()
        .any(|role| settings.manager_roles().contains(&role.get()));
    if admin || manager_role {
        AccessTier::Manager
    } else {
        AccessTier::Member
    }
}

// ───────────────────────── 레거시 설정 저장 ─────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsRequest {
    min_volume: i32,
    max_volume: i32,
    default_volume: i32,
    chat_enabled: bool,
    search_rule: PermissionRule,
    vote_rule: PermissionRule,
    chat_rule: PermissionRule,
    playback_rule: PermissionRule,
    seek_rule: PermissionRule,
    volume_rule: PermissionRule,
    queue_edit_rule: PermissionRule,
    configured_role_ids: Vec<u64>,
    max_queue_per_user: i32,
    max_queue_per_guild: i32,
    max_track_seconds: i32,
    audit_retention_days: i32,
}

async fn api_settings(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SettingsRequest>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require_manager() {
        return response;
    }
    let mut settings = ctx.settings.clone();
    if request.min_volume < 0
        || request.max_volume > 200
        || request.min_volume > request.max_volume
        || request.default_volume < request.min_volume
        || request.default_volume > request.max_volume
        // §18.1 새 상한 + §23.1 무제한(0). 여기가 막고 있으면 화면에서 아무리 밀어도 안 저장된다.
        || !unlimited_or(request.max_queue_per_user, 1, 1_000)
        || !unlimited_or(request.max_queue_per_guild, 1, 10_000)
        || !unlimited_or(request.max_track_seconds, 60, 86_400)
        || !unlimited_or(request.audit_retention_days, 1, 3650)
        || request.configured_role_ids.len() > 50
    {
        return json_error(StatusCode::BAD_REQUEST, "설정 값이 허용 범위를 벗어났어요.");
    }
    // 잠긴 항목을 바꾸려 하면 거절한다. 이 레거시 라우트는 본문에 전 항목을 항상 실어 보내므로
    // 섹션 저장과 같은 판정("값이 실제로 다를 때만")을 쓰려고 키-값 표로 옮겨 넣는다.
    let overrides = state.app.remote.load_global_overrides();
    let sent: serde_json::Map<String, Value> = [
        ("maxVolume", json!(request.max_volume)),
        ("chatEnabled", json!(request.chat_enabled)),
        ("maxQueuePerUser", json!(request.max_queue_per_user)),
        ("maxQueuePerGuild", json!(request.max_queue_per_guild)),
        ("maxTrackSeconds", json!(request.max_track_seconds)),
        ("auditRetentionDays", json!(request.audit_retention_days)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    if let Some(response) = override_lock_response(&overrides, &sent) {
        return response;
    }

    let before = serde_json::to_string(&settings).unwrap_or_default();
    settings.min_volume = request.min_volume;
    settings.max_volume = request.max_volume;
    settings.default_volume = request.default_volume;
    settings.chat_enabled = request.chat_enabled;
    settings.search_rule = request.search_rule;
    settings.vote_rule = request.vote_rule;
    settings.chat_rule = request.chat_rule;
    settings.playback_rule = request.playback_rule;
    settings.seek_rule = request.seek_rule;
    settings.volume_rule = request.volume_rule;
    settings.queue_edit_rule = request.queue_edit_rule;
    // 레거시 라우트는 "지정 역할 하나로 전부"라는 옛 의미 그대로 저장한다.
    // 분리된 값(`rule_role_ids`/`manager_role_ids`)을 비워 두면 읽을 때 이 값으로 폴백한다.
    settings.configured_role_ids = request.configured_role_ids;
    settings.rule_role_ids.clear();
    settings.manager_role_ids.clear();
    settings.max_queue_per_user = request.max_queue_per_user;
    settings.max_queue_per_guild = request.max_queue_per_guild;
    settings.max_track_seconds = request.max_track_seconds;
    settings.audit_retention_days = request.audit_retention_days;
    settings.sanitize();
    if let Err(error) = state.app.remote.save_guild_settings(&settings) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    // **설정 캐시를 통째로 버린다.** `PlayerManager` 는 길드 설정을 캐시하는데 TTL 이 없어서
    // 한 번 채워지면 무효화 전까지 영구다. 예전에는 여기서 정렬 모드만 갈아 끼웠는데,
    // 그 방식은 **나머지 필드의 낡은 값을 오히려 되살려 놓는다** — `set_sort_mode` 가
    // 캐시본을 clone 해서 다시 넣기 때문이다. 실제로 투표 점수(§10.1)가 그렇게 굳어서,
    // 콘솔은 새 계산식을 보여 주는데 대기열은 옛 점수로 정렬되고 있었다.
    state.app.player.invalidate_settings(guild_id);
    apply_engine_volume(&state, &ctx, &settings).await;
    let after = serde_json::to_string(&settings).unwrap_or_default();
    let _ = state.app.remote.add_audit(
        guild_id,
        ctx.user_id(),
        &ctx.session.display_name,
        "settings.update",
        None,
        Some(&before),
        Some(&after),
        true,
        None,
    );
    emit_bare(&state, guild_id, "settings");
    // **`0` 은 무제한이다** (§23.1). 0을 그대로 넘기면 `julianday('now','-0 days')` 가 되어
    // 방금 남긴 기록까지 통째로 지운다 — 무제한을 골랐더니 전부 사라지는 최악의 결과다.
    if settings.audit_retention_days > 0 {
        let _ = state
            .app
            .remote
            .prune_audit(guild_id, settings.audit_retention_days);
    }
    json_ok(json!({ "ok": true }))
}

async fn apply_engine_volume(
    state: &Arc<WebState>,
    ctx: &AuthContext,
    settings: &RemoteGuildSettings,
) {
    let guild_id = ctx.guild_id();
    let mut engine_settings = state.app.db.load_guild_settings(guild_id);
    engine_settings.volume_override = Some(settings.default_volume);
    state.app.db.save_guild_settings(&engine_settings);
    let applied = state.app.player.apply_configured_settings(guild_id).await;
    if !ctx.session.is_developer {
        state
            .app
            .coordinator
            .apply_volume(guild_id, applied.effective_volume)
            .await;
    }
}

// ───────────────────────── 서버 관리 콘솔 API ─────────────────────────
//
// 전부 `/music/api/guilds/{id}/admin` 하위이고 Manager 이상만 통과한다.

/// 관리 콘솔 공통 진입 — 등급 검사까지 끝낸 컨텍스트를 준다.
async fn authorize_admin(
    state: &Arc<WebState>,
    cookies: &Cookies,
    guild_id: u64,
    headers: Option<&HeaderMap>,
) -> Result<AuthContext, Response> {
    let ctx = authorize(state, cookies, guild_id, headers).await?;
    ctx.require_manager()?;
    Ok(ctx)
}

async fn admin_settings_snapshot(state: &Arc<WebState>, guild_id: u64) -> Value {
    let settings = state.app.remote.load_guild_settings(guild_id);
    let player = state.app.player.get_state(guild_id).await;
    let mut snapshot = json!({
        "sortMode": settings.sort_mode.as_str(),
        "autoBgmEnabled": player.autoplay_enabled,
        "repeatMode": repeat_key(player.repeat_mode),
        "defaultVolume": settings.default_volume,
        "minVolume": settings.min_volume,
        "maxVolume": settings.max_volume,
        "searchRule": rule_key(settings.search_rule),
        "voteRule": rule_key(settings.vote_rule),
        "chatRule": rule_key(settings.chat_rule),
        "playbackRule": rule_key(settings.playback_rule),
        "seekRule": rule_key(settings.seek_rule),
        "volumeRule": rule_key(settings.volume_rule),
        "queueEditRule": rule_key(settings.queue_edit_rule),
        "skipRule": rule_key(settings.skip_rule),
        "autoplayRule": rule_key(settings.autoplay_rule),
        "bulkEnqueueRule": rule_key(settings.bulk_enqueue_rule),
        // V3 §1: 통짜 `configuredRoleIds`는 더 이상 내보내지 않는다.
        // 권한 키마다 지정 역할이 따로이고, 관리자 지정 역할은 아예 별개다.
        // 전부 문자열 배열이다 — JS 숫자 정밀도 손실 방지 (계약 §3).
        "ruleRoleIds": Value::Object(
            PERMISSION_KEYS
                .iter()
                .map(|key| {
                    (
                        (*key).to_string(),
                        json!(
                            settings
                                .roles_for(key)
                                .iter()
                                .map(|id| id.to_string())
                                .collect::<Vec<_>>()
                        ),
                    )
                })
                .collect(),
        ),
        "managerRoleIds": settings
            .manager_roles()
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>(),
        "maxQueuePerUser": settings.max_queue_per_user,
        "maxQueuePerGuild": settings.max_queue_per_guild,
        "maxTrackSeconds": settings.max_track_seconds,
        "auditRetentionDays": settings.audit_retention_days,
        "chatRetentionDays": settings.chat_retention_days,
        "chatEnabled": settings.chat_enabled,
        "suggestionEnabled": settings.suggestion_enabled,
        "visualizerEnabled": settings.visualizer_enabled,
    });
    // v3 에서 늘어난 설정은 두 번째 묶음으로 붙인다. `json!` 하나에 다 넣으면
    // 매크로 재귀 한도에 걸려 컴파일이 안 된다.
    let extra = json!({
        // ── V3 §10.1 투표 점수 ──
        "likePoints": settings.like_points,
        "dislikePoints": settings.dislike_points,
        "superLikePoints": settings.super_like_points,
        "waitPoints": settings.wait_points,
        // ── V3 §10.3 붐따 ──
        "boomttaEnabled": settings.boomtta_enabled,
        "boomttaThreshold": settings.boomtta_threshold,
        "boomttaAction": settings.boomtta_action.as_str(),
        // ── V3 §10.5 투표 스킵 ──
        "voteSkipEnabled": settings.vote_skip_enabled,
        "voteSkipBasis": settings.vote_skip_basis.as_str(),
        "voteSkipRatio": settings.vote_skip_ratio,
        "voteSkipMin": settings.vote_skip_min,
        // ── V3 §10.6 슈퍼 좋아요 제한 ──
        "superLikeCooldownSec": settings.super_like_cooldown_sec,
        "superLikeDailyLimit": settings.super_like_daily_limit,
        // ── V3 §8 자동 재생 ──
        "autoplayMode": settings.autoplay_mode.as_str(),
        "autoplayRecentCount": settings.autoplay_recent_count,
        "autoplayGenres": settings.autoplay_genres,
        "autoplayPolicy": settings.autoplay_policy.as_str(),
        "autoplayArtistCooldown": settings.autoplay_artist_cooldown,
        "autoplayRecentDecayHours": settings.autoplay_recent_decay_hours,
        "autoplaySeedMax": settings.autoplay_seed_max,
        // ── V3 §15.2b · §18.2 ──
        "chartSuperWeight": settings.chart_super_weight,
        "bulkEnqueueLimit": settings.bulk_enqueue_limit,
        // ── 재생 동작 (§31 · §36) ──
        // **읽어올 수 있어야 켜고 끌 수 있다.** 저장 핸들러만 만들어 두고 스냅샷에 안 실어서
        // 화면에서는 존재하지도 않는 설정이 됐던 자리다.
        "requireVoiceForPlayback": settings.require_voice_for_playback,
        "webPlayerMode": settings.web_player_mode,
        "publicNowPlaying": settings.public_now_playing,
        "skipLeadMs": settings.skip_lead_ms,
        "seekLockoutMs": settings.seek_lockout_ms,
        "webSyncOffsetMs": settings.web_sync_offset_ms,
        // ── 디스코드 명령 그룹 on/off ──
        // 지금 꺼 둔 그룹의 키. **빈 배열이면 전부 켜져 있다** — 설정을 안 만진 서버는
        // 항상 빈 배열이고, 그때 봇은 이 기능이 생기기 전과 똑같이 동작한다.
        "disabledCommandGroups": settings.disabled_command_groups,
        // 그릴 스위치의 목록. 그룹 이름·설명·속한 명령을 화면이 따로 들고 있으면
        // 명령이 늘어날 때 서버와 화면이 어긋난다 — 표는 서버 한 곳(`catalog::GROUPS`)에만 있다.
        "commandGroups": command_groups_json(),
        // 화면이 `∞` 칸을 그리려면 어떤 항목이 무제한을 받는지 알아야 한다 (§23.1).
        "unlimitedKeys": UNLIMITED_KEYS,
        // **봇 주인이 잠근 항목** — UI 는 이걸 보고 자물쇠를 그리고 입력을 잠근다.
        // 위 값들은 이미 강제값이 덮인 **유효값**이다(`load_guild_settings` 가 덮는다).
        // 그래서 화면에 보이는 값과 실제로 도는 값이 갈라질 수 없다.
        "ownerOverrides": owner_overrides_json(state),
    });
    if let (Some(base), Some(extra)) = (snapshot.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    snapshot
}

/// 디스코드 명령 그룹 표를 그대로 실어 보낸다 (`commands::catalog::GROUPS`).
///
/// 화면이 그룹 목록을 하드코딩하면 명령이 하나 늘 때마다 두 곳을 고쳐야 하고,
/// 한쪽만 고치면 **화면에 없는 스위치가 서버에서만 살아 있는** 상태가 된다.
/// 명령 이름은 canonical 과 한국어를 같이 준다 — 사람에게는 `/재생` 으로 보여야 한다.
fn command_groups_json() -> Value {
    Value::Array(
        crate::commands::catalog::GROUPS
            .iter()
            .map(|group| {
                json!({
                    "key": group.key,
                    "label": group.label,
                    "description": group.description,
                    "commands": group
                        .commands
                        .iter()
                        .map(|name| json!({
                            "name": name,
                            "korean": crate::commands::catalog::korean_alias(name),
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// 관리 콘솔에 실어 보낼 잠금 정보. 봇 주인 화면과 **같은 모양**이다.
fn owner_overrides_json(state: &Arc<WebState>) -> Value {
    overrides_json(&state.app.remote.load_global_overrides())
}

/// `0` 을 무제한으로 받는 숫자 설정 (V3 §23.1). 관리 콘솔이 슬라이더 끝에 `∞` 칸을 붙일 때 쓴다.
/// **예외 둘**은 여기 없다 — 볼륨(0~200 범위가 있어야 의미가 있다)과
/// 투표 스킵 비율(백분율이라 무제한이 말이 안 된다).
const UNLIMITED_KEYS: [&str; 13] = [
    "maxQueuePerUser",
    "maxQueuePerGuild",
    "maxTrackSeconds",
    "auditRetentionDays",
    "chatRetentionDays",
    "superLikeCooldownSec",
    "superLikeDailyLimit",
    "voteSkipMin",
    "autoplayArtistCooldown",
    "autoplayRecentDecayHours",
    "boomttaThreshold",
    "bulkEnqueueLimit",
    "autoplaySeedMax",
];

async fn admin_settings_get(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, None).await {
        return response;
    }
    json_ok(json!({ "settings": admin_settings_snapshot(&state, guild_id).await }))
}

fn json_i32(body: &Value, key: &str) -> Option<i32> {
    body.get(key).and_then(Value::as_i64).map(|value| value as i32)
}

fn json_bool(body: &Value, key: &str) -> Option<bool> {
    body.get(key).and_then(Value::as_bool)
}

fn json_rule(body: &Value, key: &str) -> Result<Option<PermissionRule>, String> {
    match body.get(key).and_then(Value::as_str) {
        Some(value) => parse_rule(value)
            .map(Some)
            .ok_or_else(|| format!("{key}: 알 수 없는 권한 규칙이에요.")),
        None => Ok(None),
    }
}

/// 역할 ID 배열. 문자열로 오는 게 정상이지만(정밀도) 숫자도 받아 준다.
fn parse_role_ids(value: &Value) -> Result<Vec<u64>, String> {
    let Some(items) = value.as_array() else {
        return Err("역할 목록은 배열이어야 해요.".into());
    };
    if items.len() > 50 {
        return Err("지정 역할은 최대 50개까지예요.".into());
    }
    let mut ids: Vec<u64> = Vec::with_capacity(items.len());
    for item in items {
        let parsed = match item {
            Value::String(text) => text.trim().parse::<u64>().ok(),
            Value::Number(number) => number.as_u64(),
            _ => None,
        };
        match parsed {
            Some(id) if id != 0 && !ids.contains(&id) => ids.push(id),
            Some(_) => {}
            None => return Err("역할 ID가 숫자가 아니에요.".into()),
        }
    }
    Ok(ids)
}

/// `PUT /admin/settings/{section}` — 그 섹션의 키만 담은 부분 객체를 받는다.
async fn admin_settings_put(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, section)): Path<(u64, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    // **봇 주인이 잠근 항목을 바꾸려 하면 여기서 거절한다.**
    let overrides = state.app.remote.load_global_overrides();
    if let Some(fields) = body.as_object()
        && let Some(response) = override_lock_response(&overrides, fields)
    {
        return response;
    }

    let mut settings = ctx.settings.clone();
    let before = serde_json::to_string(&settings).unwrap_or_default();
    let mut sort_mode_changed = false;

    match section.as_str() {
        "order" => {
            if let Some(mode) = body.get("sortMode").and_then(Value::as_str) {
                let Some(mode) = QueueSortMode::parse(mode) else {
                    return json_error(StatusCode::BAD_REQUEST, "알 수 없는 정렬 모드예요.");
                };
                sort_mode_changed = settings.sort_mode != mode;
                settings.sort_mode = mode;
            }
            if let Some(volume) = json_i32(&body, "defaultVolume") {
                if volume < settings.min_volume || volume > settings.max_volume {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "기본 볼륨이 최소/최대 범위를 벗어났어요.",
                    );
                }
                settings.default_volume = volume;
            }
            if let Some(enabled) = json_bool(&body, "autoBgmEnabled") {
                state.app.player.set_autoplay(guild_id, enabled).await;
                let mut engine = state.app.db.load_guild_settings(guild_id);
                engine.autoplay_default_override = Some(enabled);
                state.app.db.save_guild_settings(&engine);
            }
            if let Some(mode) = body.get("repeatMode").and_then(Value::as_str) {
                let Some(mode) = parse_repeat(mode) else {
                    return json_error(StatusCode::BAD_REQUEST, "알 수 없는 반복 모드예요.");
                };
                state.app.player.set_repeat(guild_id, mode).await;
            }
            // ── 투표 점수 (V3 §10.1) ──
            for (key, slot) in [
                ("likePoints", &mut settings.like_points),
                ("dislikePoints", &mut settings.dislike_points),
                ("superLikePoints", &mut settings.super_like_points),
                ("waitPoints", &mut settings.wait_points),
            ] {
                if let Some(value) = json_i32(&body, key) {
                    if !(VOTE_POINT_MIN..=VOTE_POINT_MAX).contains(&value) {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("{key}: 점수는 {VOTE_POINT_MIN}~{VOTE_POINT_MAX} 사이여야 해요."),
                        );
                    }
                    *slot = value;
                }
            }
            // ── 붐따 (V3 §10.3) ──
            if let Some(value) = json_bool(&body, "boomttaEnabled") {
                settings.boomtta_enabled = value;
            }
            if let Some(value) = json_i32(&body, "boomttaThreshold") {
                // `0` 은 무제한이라 아무리 눌러도 안 터진다 (§23.1).
                if !(0..=1_000).contains(&value) {
                    return json_error(StatusCode::BAD_REQUEST, "붐따 기준 수는 0~1000이에요.");
                }
                settings.boomtta_threshold = value as u32;
            }
            if let Some(value) = body.get("boomttaAction").and_then(Value::as_str) {
                let Some(action) = BoomttaAction::parse(value) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "붐따 동작은 bottom(맨 뒤로) 또는 remove(빼기)예요.",
                    );
                };
                settings.boomtta_action = action;
            }
            // ── 투표 스킵 (V3 §10.5) ──
            if let Some(value) = json_bool(&body, "voteSkipEnabled") {
                settings.vote_skip_enabled = value;
            }
            if let Some(value) = body.get("voteSkipBasis").and_then(Value::as_str) {
                let Some(basis) = VoteSkipBasis::parse(value) else {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "투표 스킵 기준은 listeners · viewers · either · both 중 하나예요.",
                    );
                };
                settings.vote_skip_basis = basis;
            }
            if let Some(value) = json_i32(&body, "voteSkipRatio") {
                // 백분율이라 무제한이 말이 안 된다 (§23.1 예외).
                if !(10..=100).contains(&value) {
                    return json_error(StatusCode::BAD_REQUEST, "투표 스킵 비율은 10~100%예요.");
                }
                settings.vote_skip_ratio = value as u32;
            }
            if let Some(value) = json_i32(&body, "voteSkipMin") {
                if !unlimited_or(value, 1, 20) {
                    return json_error(StatusCode::BAD_REQUEST, "최소 표 수는 0(없음)~20이에요.");
                }
                settings.vote_skip_min = value as u32;
            }
            // ── 슈퍼 좋아요 제한 (V3 §10.6) ──
            if let Some(value) = json_i32(&body, "superLikeCooldownSec") {
                if !unlimited_or(value, 1, 3_600) {
                    return json_error(StatusCode::BAD_REQUEST, "쿨타임은 0(없음)~3600초예요.");
                }
                settings.super_like_cooldown_sec = value as u32;
            }
            if let Some(value) = json_i32(&body, "superLikeDailyLimit") {
                if !unlimited_or(value, 1, 100) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "하루 횟수는 0(무제한)~100이에요.",
                    );
                }
                settings.super_like_daily_limit = value as u32;
            }
            // ── 자동 재생 (V3 §8) ──
            if let Some(value) = body.get("autoplayMode").and_then(Value::as_str) {
                let Some(mode) = AutoplayMode::parse(value) else {
                    return json_error(StatusCode::BAD_REQUEST, "알 수 없는 자동 재생 방식이에요.");
                };
                settings.autoplay_mode = mode;
            }
            if let Some(value) = body.get("autoplayPolicy").and_then(Value::as_str) {
                let Some(policy) = AutoplayPolicy::parse(value) else {
                    return json_error(StatusCode::BAD_REQUEST, "알 수 없는 추천 정책이에요.");
                };
                settings.autoplay_policy = policy;
            }
            if let Some(value) = json_i32(&body, "autoplayRecentCount") {
                // `0` = 무제한 (§23.1). 관리 콘솔도 유저 UI 와 같은 규약을 써야 한다.
                if !(0..=20).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "최근 N곡은 20까지예요. 0을 넣으면 전부 참고해요.",
                    );
                }
                settings.autoplay_recent_count = value as u32;
            }
            if let Some(value) = json_i32(&body, "autoplayArtistCooldown") {
                if !unlimited_or(value, 1, 20) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "아티스트 쿨다운은 0(없음)~20곡이에요.",
                    );
                }
                settings.autoplay_artist_cooldown = value as u32;
            }
            if let Some(value) = json_i32(&body, "autoplayRecentDecayHours") {
                if !unlimited_or(value, 1, 168) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "재생 이력 감쇠는 0(끔)~168시간이에요.",
                    );
                }
                settings.autoplay_recent_decay_hours = value as u32;
            }
            if let Some(value) = json_i32(&body, "autoplaySeedMax") {
                if !unlimited_or(value, 1, 100) {
                    return json_error(StatusCode::BAD_REQUEST, "기준 곡 상한은 0(무제한)~100이에요.");
                }
                settings.autoplay_seed_max = value as u32;
            }
            if let Some(values) = body.get("autoplayGenres").and_then(Value::as_array) {
                if values.len() > 20 {
                    return json_error(StatusCode::BAD_REQUEST, "장르는 20개까지 고를 수 있어요.");
                }
                settings.autoplay_genres = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|genre| !genre.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            // ── 차트에서 가져올 곡 수 (V3 §15) ──
            if let Some(value) = json_i32(&body, "chartLimit") {
                if !(10..=100).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "차트 곡 수는 10~100곡이에요.",
                    );
                }
                settings.chart_limit = value as u32;
            }
            // ── 차트 가중치 (V3 §15.2b) ──
            if let Some(value) = json_i32(&body, "chartSuperWeight") {
                if !(0..=5).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "슈퍼 좋아요 가중치는 0~5예요. 0이면 아예 안 세요.",
                    );
                }
                settings.chart_super_weight = value as u32;
            }
            // ── 곡 알림 방식 (§25) ──
            if let Some(value) = body.get("nowPlayingMode").and_then(|v| v.as_str())
                && let Some(mode) = crate::remote::NowPlayingMode::parse(value)
            {
                settings.now_playing_mode = mode;
            }
            // ── 빈 채널 규칙 (§27) ──
            //
            // **강제 중이면 여기서 거절한다.** 조용히 무시하면 화면은 저장된 줄 알고
            // 바뀐 값을 보여 주다가 새로고침하면 되돌아간다 — 제일 헷갈리는 실패다.
            let rule = crate::app::empty_voice_rule(&state.app, guild_id);
            let touches_empty_voice = body.get("emptyVoicePolicy").is_some()
                || body.get("emptyVoiceDelaySeconds").is_some();
            if touches_empty_voice && !rule.editable() {
                return json_error(
                    StatusCode::FORBIDDEN,
                    rule.lock_reason().unwrap_or("지금은 바꿀 수 없어요."),
                );
            }
            if let Some(value) = body.get("emptyVoicePolicy").and_then(|v| v.as_str()) {
                let Some(policy) = crate::models::EmptyVoiceChannelPolicy::parse(value) else {
                    return json_error(StatusCode::BAD_REQUEST, "빈 채널 동작을 알 수 없어요.");
                };
                settings.empty_voice_policy = policy;
            }
            if let Some(value) = json_i32(&body, "emptyVoiceDelaySeconds") {
                if !(5..=3600).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "빈 채널 대기 시간은 5초에서 1시간 사이여야 해요.",
                    );
                }
                settings.empty_voice_delay_seconds = value as u32;
            }
            // ── 재생 싱크 (§31) ──
            if let Some(value) = json_i32(&body, "skipLeadMs") {
                if !(0..=5000).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "스킵 여유 시간은 0~5000ms 사이여야 해요.",
                    );
                }
                settings.skip_lead_ms = value as u32;
            }
            if let Some(value) = json_i32(&body, "seekLockoutMs") {
                if !(0..=10000).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "진행바 잠금 구간은 0~10000ms 사이여야 해요.",
                    );
                }
                settings.seek_lockout_ms = value as u32;
            }
            if let Some(value) = json_i32(&body, "webSyncOffsetMs") {
                if !(-5000..=5000).contains(&value) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "전역 싱크 보정은 -5000~5000ms 사이여야 해요.",
                    );
                }
                settings.web_sync_offset_ms = value;
            }
            if let Some(value) = body.get("requireVoiceForPlayback").and_then(|v| v.as_bool()) {
                settings.require_voice_for_playback = value;
            }
            if let Some(value) = body.get("webPlayerMode").and_then(|v| v.as_bool()) {
                settings.web_player_mode = value;
            }
            if let Some(value) = body.get("publicNowPlaying").and_then(|v| v.as_bool()) {
                settings.public_now_playing = value;
            }
        }
        "perms" => {
            // 저장하는 순간 레거시 통짜 값을 8개 키로 펼친다. 그래야 이후로는
            // 읽기 폴백에 기대지 않고 키마다 따로 관리된다 (V3 §1 마이그레이션).
            settings.expand_legacy_roles();
            // 권한 10종 (V3 §1 + §10.5 + §8.3 + §15.4). 관리자 지정 역할은 별개 축이라 11종이다.
            let rules: [(&str, &mut PermissionRule); 10] = [
                ("searchRule", &mut settings.search_rule),
                ("voteRule", &mut settings.vote_rule),
                ("chatRule", &mut settings.chat_rule),
                ("playbackRule", &mut settings.playback_rule),
                ("skipRule", &mut settings.skip_rule),
                ("seekRule", &mut settings.seek_rule),
                ("volumeRule", &mut settings.volume_rule),
                ("queueEditRule", &mut settings.queue_edit_rule),
                ("autoplayRule", &mut settings.autoplay_rule),
                ("bulkEnqueueRule", &mut settings.bulk_enqueue_rule),
            ];
            for (key, slot) in rules {
                match json_rule(&body, key) {
                    Ok(Some(rule)) => *slot = rule,
                    Ok(None) => {}
                    Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                }
            }
            // 옛 이름으로 보내는 콘솔도 받아 준다 — 저장이 조용히 안 먹는 게 제일 나쁘다.
            for (legacy, canonical) in [
                ("autoplaySeedRule", &mut settings.autoplay_rule),
                ("playlistEnqueueRule", &mut settings.bulk_enqueue_rule),
            ] {
                if body.get(legacy).is_some() {
                    match json_rule(&body, legacy) {
                        Ok(Some(rule)) => *canonical = rule,
                        Ok(None) => {}
                        Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                    }
                }
            }
            // `ruleRoleIds`는 보낸 키만 갱신한다. 안 보낸 키는 건드리지 않는다 —
            // 관리 콘솔이 섹션 일부만 저장해도 다른 권한의 역할이 날아가면 안 된다.
            if let Some(map) = body.get("ruleRoleIds").and_then(Value::as_object) {
                for (key, value) in map {
                    let key = canonical_permission_key(key);
                    if !PERMISSION_KEYS.contains(&key) {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("{key}: 알 수 없는 권한 키예요."),
                        );
                    }
                    let ids = match parse_role_ids(value) {
                        Ok(ids) => ids,
                        Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                    };
                    settings.rule_role_ids.insert(key.to_string(), ids);
                }
            }
            if let Some(value) = body.get("managerRoleIds") {
                match parse_role_ids(value) {
                    Ok(ids) => settings.manager_role_ids = ids,
                    Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                }
            }
            // ── 디스코드 명령 그룹 on/off ──
            // **꺼 둘 그룹의 전체 목록**을 받는다(보낸 대로 통째로 갈아 끼운다).
            // 스위치 화면이 늘 전체 상태를 알고 있어서 부분 갱신보다 이쪽이 어긋날 자리가 없다.
            // 모르는 키는 거절한다 — 오타가 조용히 저장되면 화면에는 꺼진 것으로 보이는데
            // 실제로는 아무것도 안 막히는, 제일 찾기 어려운 상태가 된다.
            if let Some(values) = body.get("disabledCommandGroups") {
                let Some(values) = values.as_array() else {
                    return json_error(StatusCode::BAD_REQUEST, "꺼 둘 명령 그룹은 배열이어야 해요.");
                };
                let mut keys: Vec<String> = Vec::with_capacity(values.len());
                for value in values {
                    let key = value.as_str().unwrap_or_default();
                    let Some(group) = crate::commands::catalog::group_for_key(key) else {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("'{key}': 알 수 없는 명령 그룹이에요."),
                        );
                    };
                    if !keys.iter().any(|existing| existing == group.key) {
                        keys.push(group.key.to_string());
                    }
                }
                settings.disabled_command_groups = keys;
            }
        }
        "limits" => {
            if let Some(value) = json_i32(&body, "minVolume") {
                settings.min_volume = value;
            }
            if let Some(value) = json_i32(&body, "maxVolume") {
                settings.max_volume = value;
            }
            if let Some(value) = json_i32(&body, "maxQueuePerUser") {
                settings.max_queue_per_user = value;
            }
            if let Some(value) = json_i32(&body, "maxQueuePerGuild") {
                settings.max_queue_per_guild = value;
            }
            if let Some(value) = json_i32(&body, "maxTrackSeconds") {
                settings.max_track_seconds = value;
            }
            if let Some(value) = json_i32(&body, "auditRetentionDays") {
                settings.audit_retention_days = value;
            }
            if let Some(value) = json_i32(&body, "bulkEnqueueLimit") {
                // 클릭 한 번이 대기열을 5000곡으로 만들면 되돌리기가 너무 어렵다 (§18.2 (4)).
                if !unlimited_or(value, 1, 10_000) {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        "한 번에 담기 상한은 0(무제한)~10000이에요.",
                    );
                }
                settings.bulk_enqueue_limit = value as u32;
            }
            if let Some(value) = json_i32(&body, "chatRetentionDays") {
                // `.max(1)` 클램프 제거 — `0` 은 무제한이라 그대로 저장한다 (§23.1).
                settings.chat_retention_days = value.max(0) as u32;
            }
            if settings.min_volume < 0
                || settings.max_volume > 200
                || settings.min_volume > settings.max_volume
                // §18.1 새 상한(1인 1000 / 서버 10000) + §23.1 무제한(0).
                || !unlimited_or(settings.max_queue_per_user, 1, 1_000)
                || !unlimited_or(settings.max_queue_per_guild, 1, 10_000)
                || !unlimited_or(settings.max_track_seconds, 60, 86_400)
                || !unlimited_or(settings.audit_retention_days, 1, 3650)
                || !unlimited_or(settings.chat_retention_days as i32, 1, 365)
            {
                return json_error(StatusCode::BAD_REQUEST, "허용 범위를 벗어난 값이 있어요.");
            }
            settings.default_volume = settings
                .default_volume
                .clamp(settings.min_volume, settings.max_volume);
        }
        "chat" => {
            if let Some(value) = json_bool(&body, "chatEnabled") {
                settings.chat_enabled = value;
            }
            if let Some(value) = json_bool(&body, "suggestionEnabled") {
                settings.suggestion_enabled = value;
            }
            if let Some(value) = json_bool(&body, "visualizerEnabled") {
                settings.visualizer_enabled = value;
            }
        }
        _ => return json_error(StatusCode::NOT_FOUND, "알 수 없는 설정 섹션이에요."),
    }

    // `0 = 무제한` 규약과 각 범위를 저장 직전에 한 번 더 못 박는다 (§23.1).
    // 어떤 라우트를 거쳐 들어와도 서버가 실제로 그 규약대로 동작해야 한다.
    settings.sanitize();
    if let Err(error) = state.app.remote.save_guild_settings(&settings) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    // **정책을 바꾸면 지금 잡혀 있는 다음 추천곡을 다시 뽑는다** (V3 §8.5).
    // 관리 콘솔이 "바꾸면 바로 다시 뽑아요"라고 적어 놓고 실제로는 안 뽑고 있었다.
    if settings.autoplay_mode != ctx.settings.autoplay_mode
        || settings.autoplay_policy != ctx.settings.autoplay_policy
        || settings.autoplay_recent_count != ctx.settings.autoplay_recent_count
        || settings.autoplay_artist_cooldown != ctx.settings.autoplay_artist_cooldown
        || settings.autoplay_recent_decay_hours != ctx.settings.autoplay_recent_decay_hours
        || settings.autoplay_genres != ctx.settings.autoplay_genres
    {
        // **기다리지 않는다.** `refresh_preview` 는 `resolve_preview` 를 통해 yt-dlp 추천을
        // 통째로 돌린다. 그걸 요청 안에서 await 하면 저장 응답이 추천이 끝날 때까지 안 나가고,
        // 추천은 직렬화돼 있어 10~20초씩 걸린다. 화면에서는 저장 버튼이 영영 도는 것으로 보인다
        // (실제로는 바로 위에서 이미 저장이 끝났는데도). 곡 시작 훅과 같은 방식으로 떼어 던진다.
        let app = state.app.clone();
        tokio::spawn(async move {
            crate::player::side_effects::refresh_preview(app, guild_id).await;
        });
    }
    // 아래 `refresh_scored_order` 가 캐시에서 정렬 모드와 투표 점수를 읽으므로 **그 전에** 버린다.
    // 갈아 끼우기가 아니라 무효화다 — 위 라우트의 주석 참고(정렬 외 필드가 낡은 채로 남는다).
    state.app.player.invalidate_settings(guild_id);
    if section == "order" || section == "limits" {
        apply_engine_volume(&state, &ctx, &settings).await;
    }
    let after = serde_json::to_string(&settings).unwrap_or_default();
    let _ = state.app.remote.add_audit(
        guild_id,
        ctx.user_id(),
        &ctx.session.display_name,
        &format!("settings.{section}"),
        None,
        Some(&before),
        Some(&after),
        true,
        None,
    );
    if sort_mode_changed {
        state.app.player.refresh_scored_order(guild_id).await;
        emit(
            &state,
            guild_id,
            "notice",
            json!({
                "message": format!("정렬 모드가 바뀌었어요 — {}", settings.sort_mode.description()),
                "kind": "info",
            }),
        );
        broadcast_queue(&state, guild_id).await;
    }
    emit_bare(&state, guild_id, "settings");
    json_ok(json!({ "ok": true, "settings": admin_settings_snapshot(&state, guild_id).await }))
}

/// 역할 목록. Discord 캐시가 없으면 빈 배열이어야 나머지 화면이 살아남는다.
async fn admin_roles(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, None).await {
        return response;
    }
    let mut roles: Vec<Value> = Vec::new();
    if let Some(cache) = state.app.discord_cache.get() {
        if let Some(guild) = cache.guild(GuildId::new(guild_id)) {
            let mut rows: Vec<(u16, Value)> = guild
                .roles
                .values()
                .filter(|role| role.name != "@everyone")
                .map(|role| {
                    let member_count = guild
                        .members
                        .values()
                        .filter(|member| member.roles.contains(&role.id))
                        .count();
                    (
                        role.position,
                        json!({
                            "id": role.id.get().to_string(),
                            "name": role.name,
                            "color": format!("#{:06x}", role.colour.0),
                            "memberCount": member_count,
                        }),
                    )
                })
                .collect();
            rows.sort_by(|left, right| right.0.cmp(&left.0));
            roles = rows.into_iter().map(|(_, value)| value).collect();
        }
    }
    json_ok(json!({ "roles": roles }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModeQuery {
    mode: Option<String>,
    // **아직 저장하지 않은 점수로도 미리 볼 수 있어야 한다** (V3 §10.1).
    // 예전에는 `mode` 하나만 파싱해서, 콘솔이 슬라이더로 좋아요 1→10 을 끌어도
    // 미리보기 순서·계산식이 저장값 그대로였다. serde 가 모르는 쿼리 키를 조용히
    // 버리기 때문에 400 도 안 나서 "반영된 줄 아는" 상태가 됐다.
    #[serde(default)]
    like_points: Option<i32>,
    #[serde(default)]
    dislike_points: Option<i32>,
    #[serde(default)]
    super_like_points: Option<i32>,
    #[serde(default)]
    wait_points: Option<i32>,
}

impl ModeQuery {
    /// 쿼리로 온 점수를 저장값 위에 덮는다. 안 보낸 항목은 저장값 그대로다.
    /// 범위는 저장 경로와 같은 규칙으로 자른다 — 미리보기가 저장 못 할 값을 보여주면 안 된다.
    fn points_over(&self, base: VotePoints) -> VotePoints {
        VotePoints {
            like: self.like_points.unwrap_or(base.like),
            dislike: self.dislike_points.unwrap_or(base.dislike),
            super_like: self.super_like_points.unwrap_or(base.super_like),
            wait: self.wait_points.unwrap_or(base.wait),
        }
        .clamped()
    }
}

/// 정렬 모드를 바꾸면 지금 대기열이 어떻게 바뀌는지 미리 보여준다 (사양서 §4.2 "구림" 해소 #4).
async fn admin_queue_preview(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<ModeQuery>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let mode = query
        .mode
        .as_deref()
        .and_then(QueueSortMode::parse)
        .unwrap_or(ctx.settings.sort_mode);
    let player = state.app.player.get_state(guild_id).await;
    let mut scores = state.app.remote.queue_scores(guild_id);
    ranking::apply_rounds(&player.upcoming, &mut scores);

    let current_positions: HashMap<&str, usize> = player
        .upcoming
        .iter()
        .enumerate()
        .map(|(index, item)| (item.id.as_str(), index + 1))
        .collect();
    // 콘솔이 보낸 점수가 있으면 그 값으로, 없으면 저장값으로 계산한다 (V3 §10.1) —
    // 화면이 보여주는 순서·계산식이 실제 판정과 어긋나면 미리보기가 쓸모없어진다.
    let points = query.points_over(ctx.settings.vote_points());
    let mut preview = player.upcoming.clone();
    ranking::sort_queue(&mut preview, &scores, mode, &points);

    let items: Vec<Value> = preview
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            let current = current_positions
                .get(item.id.as_str())
                .copied()
                .unwrap_or(index + 1);
            json!({
                "itemId": item.id,
                "title": item.track.display_title(),
                "requestedBy": item.requested_by_display,
                "roundLabel": format!("{}번째 곡", score.round + 1),
                "score": score.total_score(&points),
                "formula": score.formula(&points),
                "currentPosition": current,
                "previewPosition": index + 1,
                "delta": current as i64 - (index as i64 + 1),
            })
        })
        .collect();
    json_ok(json!({
        "mode": mode.as_str(),
        "description": mode.description(),
        "totalCount": items.len(),
        "items": items,
        // 어떤 점수로 계산했는지 되돌려 준다 — 콘솔이 보낸 값이 실제로 먹었는지 확인할 수 있게.
        "votePoints": {
            "like": points.like,
            "dislike": points.dislike,
            "superLike": points.super_like,
            "wait": points.wait,
        },
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionPreviewQuery {
    rule: String,
    /// 어떤 권한의 미리보기인지. 지정 역할이 키마다 다르므로 이게 있어야 인원이 맞는다 (V3 §1).
    key: Option<String>,
    role_ids: Option<String>,
}

/// 고른 규칙으로 **지금** 이 서버에서 몇 명이 통과하는지 (사양서 §4.2 #3).
/// `rule=disabled`면 관리자 포함 0명이어야 한다 — S3 수정과 같은 판정 함수를 쓴다.
async fn admin_permission_preview(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<PermissionPreviewQuery>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(rule) = parse_rule(&query.rule) else {
        return json_error(StatusCode::BAD_REQUEST, "알 수 없는 권한 규칙이에요.");
    };
    // 키가 없으면 예전처럼 통짜 지정 역할로 본다(옛 콘솔 호환).
    let key = query
        .key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .unwrap_or("search");
    let key = canonical_permission_key(key);
    if !PERMISSION_KEYS.contains(&key) {
        return json_error(StatusCode::BAD_REQUEST, "알 수 없는 권한 키예요.");
    }
    let mut settings = ctx.settings.clone();
    if let Some(role_ids) = query.role_ids.as_deref() {
        // 아직 저장하지 않은 화면 값으로 미리 세어 본다 — 그 키의 역할만 갈아끼운다.
        let ids: Vec<u64> = role_ids
            .split(',')
            .filter_map(|value| value.trim().parse::<u64>().ok())
            .filter(|id| *id != 0)
            .collect();
        settings.rule_role_ids.insert(key.to_string(), ids);
    }

    // V3 §16 B1 — 저장값이 아니라 캐시가 봇의 현재 위치다.
    let bot_channel = bot_voice_status(&state, guild_id).channel_id;
    let mut member_count = 0usize;
    let mut pass_count = 0usize;
    let mut bypass_count = 0usize;
    let mut sample: Vec<Value> = Vec::new();
    let mut note = String::new();

    if let Some(cache) = state.app.discord_cache.get() {
        if let Some(guild) = cache.guild(GuildId::new(guild_id)) {
            for (user_id, member) in guild.members.iter() {
                if member.user.bot {
                    continue;
                }
                member_count += 1;
                let admin = guild.owner_id == *user_id
                    || is_owner_user(&state, user_id.get())
                    // 관리자 지정 역할도 관리자다 — 실제 판정(`resolve_tier`)과 같아야 한다.
                    || member
                        .roles
                        .iter()
                        .any(|role| settings.manager_roles().contains(&role.get()))
                    || member.roles.iter().any(|role| {
                        guild
                            .roles
                            .get(role)
                            .map(|role| {
                                role.permissions.contains(Permissions::ADMINISTRATOR)
                                    || role.permissions.contains(Permissions::MANAGE_GUILD)
                            })
                            .unwrap_or(false)
                    });
                let same_voice = bot_channel.is_some_and(|channel| {
                    guild
                        .voice_states
                        .get(user_id)
                        .and_then(|voice| voice.channel_id)
                        .map(|id| id.get() == channel)
                        .unwrap_or(false)
                });
                let context = MemberContext {
                    is_admin: admin,
                    same_voice_channel: same_voice,
                    bot_in_voice: bot_channel.is_some(),
                    role_ids: member.roles.iter().map(|role| role.get()).collect(),
                };
                if !permission_allowed(key, rule, &settings, &context) {
                    continue;
                }
                pass_count += 1;
                let bypass = !rule_base_allowed(key, rule, &settings, &context);
                if bypass {
                    bypass_count += 1;
                }
                if sample.len() < 12 {
                    sample.push(json!({
                        "userId": user_id.get().to_string(),
                        "displayName": member.display_name(),
                        "avatarUrl": member.face(),
                        "bypass": bypass,
                    }));
                }
            }
        }
    }
    let members_intent = state
        .app
        .intent_status
        .read()
        .map(|status| status.members)
        .unwrap_or(true);
    if !members_intent {
        note = "Server Members Intent가 꺼져 있어서 캐시에 있는 사람만 셌어요.".into();
    } else if member_count == 0 {
        note = "아직 멤버 캐시가 비어 있어요. 봇이 접속을 마치면 정확해져요.".into();
    } else if rule == PermissionRule::Disabled {
        note = "사용 안 함은 관리자와 봇 주인까지 전부 막아요.".into();
    } else if rule == PermissionRule::SameVoiceChannel && bot_channel.is_none() {
        note = "봇이 음성 채널에 없어서 지금은 관리자만 통과해요.".into();
    } else if rule == PermissionRule::ConfiguredRole && settings.roles_for(key).is_empty() {
        note = "이 권한에 지정된 역할이 없어서 지금은 관리자만 통과해요.".into();
    }

    // 관리 콘솔은 §23.3 문구("멤버에게는 … 로 보여요")를 미리 보여 주려고 이 두 값을 읽는다.
    // 유저 화면(`permissions_json`)과 **같은 계산**을 써야 미리보기와 실제가 어긋나지 않는다.
    let allowed_role_names = match rule {
        PermissionRule::ConfiguredRole => role_names(&state, guild_id, settings.roles_for(key)),
        PermissionRule::Administrator | PermissionRule::SameVoiceChannel => {
            role_names(&state, guild_id, settings.manager_role_ids.as_slice())
        }
        _ => Vec::new(),
    };
    json_ok(json!({
        "rule": rule_key(rule),
        "key": key,
        "passCount": pass_count,
        "memberCount": member_count,
        "managerBypassCount": bypass_count,
        "note": note,
        "sample": sample,
        // 미리보기도 "몇 명이 되는지"를 같이 준다 (V3 §23.3 · F6).
        "allowedCount": pass_count,
        "allowedRoleNames": allowed_role_names,
    }))
}

/// 리모컨을 써 본 사람 목록 + 접속 상태 + 정지 현황.
async fn admin_participants(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let participants = state.app.remote.list_remote_participants(guild_id);
    let presence = build_presence(&state, guild_id).await;
    let listening: HashSet<String> = presence["listening"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let viewing: HashSet<String> = presence["viewing"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let online = presence["online"].clone();

    let player = state.app.player.get_state(guild_id).await;
    // 최근 200건에서만 세는 근사값 — 관리 화면 한 번에 쿼리 하나면 충분하다.
    let recent_chat = state
        .app
        .remote
        .list_chat_messages(guild_id, ctx.user_id(), 200, None);

    let cache = state.app.discord_cache.get();
    let guild = cache.and_then(|cache| cache.guild(GuildId::new(guild_id)));

    let members: Vec<Value> = participants
        .iter()
        .map(|person| {
            let key = person.user_id.to_string();
            let status = if listening.contains(&key) {
                "listening"
            } else if viewing.contains(&key) {
                "viewing"
            } else {
                online
                    .get(&key)
                    .and_then(Value::as_str)
                    .unwrap_or("offline")
            };
            let cached = guild
                .as_ref()
                .and_then(|guild| guild.members.get(&UserId::new(person.user_id)));
            let display_name = cached
                .map(|member| member.display_name().to_string())
                .unwrap_or_else(|| {
                    if person.display_name.is_empty() {
                        person.user_id.to_string()
                    } else {
                        person.display_name.clone()
                    }
                });
            json!({
                "userId": key,
                "displayName": display_name,
                "avatarUrl": cached.map(|member| member.face()).or_else(|| person.avatar_url.clone()),
                "tier": tier_of_member(&state, guild_id, person.user_id, &ctx.settings).as_str(),
                "presence": status,
                "lastSeenUtc": person.last_active_utc,
                "queueCount": player
                    .upcoming
                    .iter()
                    .filter(|item| item.requested_by_user_id == Some(person.user_id))
                    .count(),
                "chatCount": recent_chat
                    .iter()
                    .filter(|message| message.user_id == person.user_id)
                    .count(),
                "suspensions": state
                    .app
                    .remote
                    .active_suspensions(guild_id, person.user_id)
                    .iter()
                    .map(|item| json!({ "scope": item.scope.as_str(), "expiresUtc": item.expires_utc }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json_ok(json!({ "members": members }))
}

fn suspension_row(state: &WebState, guild_id: u64, item: &Suspension) -> Value {
    let display = state
        .app
        .discord_cache
        .get()
        .and_then(|cache| cache.guild(GuildId::new(guild_id)))
        .and_then(|guild| {
            guild
                .members
                .get(&UserId::new(item.user_id))
                .map(|member| (member.display_name().to_string(), member.face()))
        });
    json!({
        "userId": item.user_id.to_string(),
        "displayName": display.as_ref().map(|value| value.0.clone()).unwrap_or_else(|| item.user_id.to_string()),
        "avatarUrl": display.as_ref().map(|value| value.1.clone()),
        "scope": item.scope.as_str(),
        "reason": item.reason,
        "byUserId": item.by_user_id.to_string(),
        "byDisplayName": Value::Null,
        "createdUtc": item.created_utc,
        "expiresUtc": item.expires_utc,
    })
}

async fn admin_suspensions_get(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, None).await {
        return response;
    }
    let items: Vec<Value> = state
        .app
        .remote
        .list_suspensions(guild_id)
        .iter()
        .map(|item| suspension_row(&state, guild_id, item))
        .collect();
    json_ok(json!({ "items": items }))
}

async fn admin_suspensions_post(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SuspendRequest>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    suspend_impl(&state, &ctx, request).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuspensionLiftRequest {
    user_id: String,
    scope: Option<String>,
}

async fn admin_suspensions_lift(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<SuspensionLiftRequest>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Ok(target) = request.user_id.trim().parse::<u64>() else {
        return json_error(StatusCode::BAD_REQUEST, "대상 사용자 ID가 올바르지 않아요.");
    };
    let scope = request.scope.as_deref().and_then(SuspensionScope::parse);
    match state.app.remote.unsuspend_user(guild_id, target, scope) {
        Ok(_) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "user.unsuspend",
                Some(&target.to_string()),
                Some(scope.map(SuspensionScope::as_str).unwrap_or("all-scopes")),
            );
            emit(
                &state,
                guild_id,
                "suspension",
                json!({ "userId": target.to_string(), "lifted": true }),
            );
            json_ok(json!({ "ok": true }))
        }
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    status: Option<String>,
    limit: Option<usize>,
}

async fn admin_reports(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<StatusQuery>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, None).await {
        return response;
    }
    let only_open = query.status.as_deref().unwrap_or("open") == "open";
    let items: Vec<Value> = state
        .app
        .remote
        .list_chat_reports(guild_id, query.limit.unwrap_or(100).clamp(1, 300))
        .iter()
        .filter(|report| !only_open || report.resolved_utc.is_none())
        .map(|report| {
            json!({
                "id": report.id,
                "messageId": report.message_id,
                "messageAuthor": report.message_author,
                "messageContent": report.message_content,
                "reporterDisplayName": report.reporter_display_name,
                "reason": report.reason,
                "createdUtc": report.created_utc,
                "resolvedUtc": report.resolved_utc,
            })
        })
        .collect();
    json_ok(json!({ "items": items }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportResolveRequest {
    action: String,
}

async fn admin_report_resolve(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, report_id)): Path<(u64, i64)>,
    headers: HeaderMap,
    Json(request): Json<ReportResolveRequest>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    // 신고 대상 메시지도 같이 지우는 경우.
    if request.action == "delete" {
        let target = state
            .app
            .remote
            .list_chat_reports(guild_id, 300)
            .into_iter()
            .find(|report| report.id == report_id)
            .map(|report| report.message_id);
        if let Some(message_id) = target {
            if let Ok(true) = state.app.remote.delete_chat_message(guild_id, message_id) {
                emit(
                    &state,
                    guild_id,
                    "chat.delete",
                    json!({ "messageId": message_id, "deletedUtc": now_utc() }),
                );
            }
        }
    } else if request.action != "dismiss" {
        return json_error(StatusCode::BAD_REQUEST, "처리 방식은 delete 또는 dismiss예요.");
    }
    match state.app.remote.resolve_chat_report(guild_id, report_id) {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "chat.report.resolve",
                Some(&report_id.to_string()),
                Some(&request.action),
            );
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 신고를 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

async fn admin_suggestions(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<StatusQuery>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let items: Vec<Value> = state
        .app
        .remote
        .list_suggestions(guild_id, ctx.user_id())
        .iter()
        .take(limit)
        .map(|item| suggestion_json(item, ctx.user_id()))
        .collect();
    json_ok(json!({ "items": items }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminSuggestionStatusRequest {
    status: String,
    note: Option<String>,
}

async fn admin_suggestion_status(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, suggestion_id)): Path<(u64, i64)>,
    headers: HeaderMap,
    Json(request): Json<AdminSuggestionStatusRequest>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(status) = SuggestionStatus::parse(&request.status) else {
        return json_error(StatusCode::BAD_REQUEST, "알 수 없는 제안 상태예요.");
    };
    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match state
        .app
        .remote
        .set_suggestion_status(guild_id, suggestion_id, status, note, ctx.user_id())
    {
        Ok(true) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "suggestion.status",
                Some(&suggestion_id.to_string()),
                Some(status.as_str()),
            );
            emit(
                &state,
                guild_id,
                "suggestion.status",
                json!({ "id": suggestion_id, "status": status.as_str() }),
            );
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "그 제안을 찾지 못했어요."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct AdminAuditQuery {
    limit: Option<usize>,
    before: Option<i64>,
    q: Option<String>,
}

async fn admin_audit(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<AdminAuditQuery>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, None).await {
        return response;
    }
    let limit = query.limit.unwrap_or(80).clamp(1, 300);
    let needle = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let raw = state.app.remote.list_audit(guild_id, limit, query.before);
    let next_cursor = raw.last().map(|entry| entry.id);
    let items: Vec<Value> = raw
        .iter()
        .filter(|entry| match &needle {
            Some(needle) => [
                entry.display_name.as_str(),
                entry.action.as_str(),
                entry.target.as_deref().unwrap_or(""),
                entry.after_value.as_deref().unwrap_or(""),
                entry.failure_reason.as_deref().unwrap_or(""),
            ]
            .join(" ")
            .to_lowercase()
            .contains(needle),
            None => true,
        })
        .map(audit_json)
        .collect();
    json_ok(json!({
        "items": items,
        "nextCursor": if raw.len() < limit { Value::Null } else { json!(next_cursor) },
    }))
}

async fn admin_diagnostics(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, None).await {
        return response;
    }
    // V3 §16 B1 — 진단 화면이야말로 저장값이 아니라 실제 상태를 보여 줘야 한다.
    let bot = bot_voice_status(&state, guild_id);
    json_ok(json!({
        "bot": {
            "online": bot_in_guild(&state, guild_id),
            "inGuild": bot.in_guild,
            "voiceConnected": bot.in_voice(),
            "voiceChannelId": bot.channel_id.map(|id| id.to_string()),
            "voiceChannelName": bot.channel_name,
            // 게이트웨이 지연은 ShardManager 핸들이 있어야 읽을 수 있다(App에 없음).
            "gatewayLatencyMs": Value::Null,
        },
        "buildId": state.app.build_id,
        // store::SCHEMA_VERSION 이 비공개라 지금은 노출하지 못한다 (보고서 참고).
        "schemaVersion": Value::Null,
        "uptimeSeconds": uptime_seconds(),
    }))
}

// ───────────────────────── 대기열 페이지네이션 (V3 §18.2) ─────────────────────────

#[derive(Debug, Deserialize)]
struct QueuePageQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

/// `/state/hot` 과 `queue.set` 은 앞 200곡만 싣는다. 그 뒤를 보고 싶을 때만 여기로 온다.
/// 보통 사람은 앞 20곡만 보므로 **평소에는 이 요청이 아예 일어나지 않는다**.
async fn api_queue_page(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<QueuePageQuery>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(QUEUE_PAGE_MAX).clamp(1, QUEUE_PAGE_MAX);
    let player = state.app.player.get_state(guild_id).await;
    let mut scores = state.app.remote.queue_scores(guild_id);
    ranking::apply_rounds(&player.upcoming, &mut scores);
    let points = ctx.settings.vote_points();
    let items: Vec<Value> = player
        .upcoming
        .iter()
        .skip(offset)
        .take(limit)
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            let my_vote = state.app.remote.user_vote(&item.id, ctx.user_id());
            queue_item_json(item, &score, ctx.user_id(), my_vote, &points)
        })
        .collect();
    json_ok(json!({
        "items": items,
        "offset": offset,
        "limit": limit,
        "queueTotal": player.upcoming.len(),
        "hasMore": offset + items.len() < player.upcoming.len(),
    }))
}

// ───────────────────────── 통계 (V3 §22.6) · 사람 카드 (§24.2) ─────────────────────────

/// 봇 전체 합계를 가리키는 길드 ID (통계 DB 규약).
const STATS_ALL_GUILDS: u64 = crate::stats::all_guilds();

/// **60초 캐시** (V3 §22.6 · §23.2). 통계는 실시간일 이유가 없고,
/// 캐시가 없으면 사람 카드를 열 때마다 집계 쿼리가 돈다.
fn stats_cached(state: &WebState, key: String, build: impl FnOnce() -> Value) -> Value {
    {
        let cache = state.stats_cache.lock().unwrap();
        if let Some((stored, value)) = cache.get(&key) {
            if stored.elapsed() < STATS_CACHE_TTL {
                return value.clone();
            }
        }
    }
    let value = build();
    state
        .stats_cache
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), value.clone()));
    value
}

/// 통계 기능이 꺼져 있을 때의 답. **빈 숫자를 0으로 꾸미지 않는다** —
/// 0회 재생과 "기록을 못 받고 있음"은 완전히 다른 이야기다.
fn stats_unavailable() -> Response {
    json_ok(json!({
        "available": false,
        "message": "통계 기록이 꺼져 있어서 보여 줄 게 없어요.",
    }))
}

/// 이 사람이 최근에 담아서 실제로 나간 곡 (§24.2 사람 카드의 `최근 담은 곡`).
///
/// 통계 DB 에는 "최근 순" 질의가 없어서 리모컨 저장소의 최근 재생 목록을 쓴다.
/// 신청자 ID 로 거르므로 자동재생이 채운 곡은 들어오지 않는다.
fn recent_tracks_of(state: &WebState, guild_id: u64, user_id: u64, limit: usize) -> Vec<Value> {
    state
        .app
        .remote
        .list_recent(guild_id, 200)
        .into_iter()
        .filter(|entry| entry.requested_by_user_id == Some(user_id))
        .take(limit)
        .map(|entry| {
            json!({
                "cacheKey": entry.track.cache_key(),
                "track": track_json(&entry.track),
                "playedUtc": entry.played_utc,
                "lastUtc": entry.played_utc,
            })
        })
        .collect()
}

fn user_stats_json(
    state: &WebState,
    stats: &crate::stats::Stats,
    guild_id: u64,
    user_id: u64,
    include_given: bool,
) -> Value {
    let recent = recent_tracks_of(state, guild_id, user_id, 5);
    let totals = stats.user_stats(guild_id, user_id);
    let top = |order: &str| -> Value {
        json!(
            stats
                .top_user_tracks(guild_id, user_id, order, 5)
                .into_iter()
                .map(|row| {
                    // 화면은 `row.count` 로 읽고 백엔드는 `requested`/`liked` 로 보낸다.
                    // 어느 목록이냐에 맞는 값을 `count` 로도 실어 준다 — 안 그러면 전부 `0회` 다.
                    let count = match order {
                        "liked" => row.liked,
                        "likes_recv" => row.likes_recv,
                        _ => row.requested,
                    };
                    json!({
                        "cacheKey": row.cache_key,
                        "track": row.track,
                        "requested": row.requested,
                        "liked": row.liked,
                        "played": row.played,
                        "likesRecv": row.likes_recv,
                        // 프런트가 읽는 평평한 이름들 (§22.5 · §24.2).
                        "count": count,
                        "likes": row.likes_recv,
                        "lastUtc": row.last_utc,
                    })
                })
                .collect::<Vec<_>>()
        )
    };
    let daily: Vec<Value> = stats
        .user_daily(guild_id, user_id, 30)
        .into_iter()
        .map(|(day, queued, played, likes)| {
            json!({ "day": day, "queued": queued, "played": played, "likesRecv": likes })
        })
        .collect();

    let mut summary = json!({
        "queuedSingle": totals.queued_single,
        "queuedBulk": totals.queued_bulk,
        "bulkTimes": totals.bulk_times,
        "queuedTotal": totals.queued_total(),
        "played": totals.played,
        "skipped": totals.skipped,
        "boomtta": totals.boomtta,
        "likesRecv": totals.likes_recv,
        "supersRecv": totals.supers_recv,
        "dislikesRecv": totals.dislikes_recv,
        "chats": totals.chats,
        "firstUtc": totals.first_utc,
        "lastUtc": totals.last_utc,
        // 마참 점수 (§22.4) — 대기열 순서·권한에 **일절 영향을 주지 않는다.** 보는 재미다.
        "karma": totals.karma(),
    });
    // 남의 기록은 **받은 것만** 보여 준다 (§22.5). 누가 누구에게 눌렀는지가 드러나면
    // 채팅방 분위기가 이상해진다.
    if include_given {
        summary["likesGive"] = json!(totals.likes_give);
        summary["supersGive"] = json!(totals.supers_give);
        summary["dislikesGive"] = json!(totals.dislikes_give);
    }

    let mut payload = json!({
        "available": true,
        "userId": user_id.to_string(),
        "summary": summary.clone(),
        "topRequested": top("requested"),
        "topLoved": top("likes_recv"),
        // 내가 누구 곡에 좋아요를 눌렀는지는 나만 본다.
        "topLiked": if include_given { top("liked") } else { json!([]) },
        // 3일치도 없으면 화면이 그래프 대신 안내를 띄운다 (§22.5).
        "daily": daily,
        // §24.2 사람 카드의 `최근 담은 곡`. 이 키가 없으면 그 섹션이 통째로 안 그려진다.
        "recent": recent,
    });

    // **요약을 최상위에도 편다.** 화면은 `stats.queued`·`stats.played`·`stats.machamScore`
    // 처럼 평평한 이름을 읽는데 서버는 `summary` 안에만 넣어서, 데이터가 있어도
    // 타일 4장이 전부 0이고 비율 막대가 아예 사라졌다 (§22.5 · §24.2).
    // `summary` 도 그대로 남긴다 — 어느 쪽을 읽어도 같은 값이 나오게 한다.
    if let (Some(root), Some(summary_map)) = (payload.as_object_mut(), summary.as_object()) {
        for (key, value) in summary_map {
            root.insert(key.clone(), value.clone());
        }
        // 이름이 다른 것들만 따로 맞춘다.
        root.insert("queued".into(), json!(totals.queued_total()));
        // 마참 점수 (§22.4) — 백엔드는 `karma`, 화면은 `machamScore` 로 읽는다.
        root.insert("machamScore".into(), json!(totals.karma()));
    }
    payload
}

async fn api_stats_me(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(stats) = state.app.stats.clone() else {
        return stats_unavailable();
    };
    let user_id = ctx.user_id();
    json_ok(stats_cached(&state, format!("me:{guild_id}:{user_id}"), || {
        user_stats_json(&state, &stats, guild_id, user_id, true)
    }))
}

async fn api_stats_user(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, user_id)): Path<(u64, u64)>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(stats) = state.app.stats.clone() else {
        return stats_unavailable();
    };
    // 자기 자신이면 준 것도 같이 본다 — 내 기록이니까.
    let mine = user_id == ctx.user_id();
    json_ok(stats_cached(
        &state,
        format!("user:{guild_id}:{user_id}:{mine}"),
        || user_stats_json(&state, &stats, guild_id, user_id, mine),
    ))
}

/// 서버 전체 요약 (관리 콘솔용). 통계 보기는 길드 멤버면 누구나 —
/// 권한을 하나 더 만들지 않는다(§22.6).
async fn api_stats_server(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(stats) = state.app.stats.clone() else {
        return stats_unavailable();
    };
    let weight = ctx.settings.chart_super_weight;
    json_ok(stats_cached(&state, format!("server:{guild_id}"), || {
        let plays = stats.chart(
            guild_id,
            crate::stats::ChartKind::Plays,
            crate::stats::ChartWindow::Month,
            weight,
            10,
        );
        let love = stats.chart(
            guild_id,
            crate::stats::ChartKind::Love,
            crate::stats::ChartWindow::Month,
            weight,
            10,
        );
        json!({
            "available": true,
            "superWeight": weight,
            "topPlayed": chart_rows_json(&plays, weight),
            "topLoved": chart_rows_json(&love, weight),
        })
    }))
}

// ───────────────────────── 차트 (V3 §15.5) ─────────────────────────

fn chart_rows_json(rows: &[crate::stats::ChartRow], weight: u32) -> Value {
    json!(
        rows.iter()
            .enumerate()
            .map(|(index, row)| json!({
                "rank": index + 1,
                "cacheKey": row.cache_key,
                "track": row.track,
                // 화면은 `plays` 라는 이름으로 읽는다. 백엔드 이름(`playsUser`)만 주면
                // `42회 재생` 이 `undefined회` 가 된다 — 둘 다 준다 (§15.2b).
                "plays": row.plays_user,
                "playsUser": row.plays_user,
                "superWeight": weight,
                // 자동재생 횟수는 순위에 안 쓰지만 궁금하니 툴팁용으로 같이 준다 (§15.2b).
                "playsAutoplay": row.plays_autoplay,
                "likes": row.likes,
                "supers": row.supers,
                "requesters": row.requesters,
                "loveScore": row.love_score,
                // 순위가 왜 그런지 보여야 한다 — 계산을 그대로 문장으로 준다.
                "loveFormula": format!("👍{} + ⭐{}×{} = {}", row.likes, row.supers, weight, row.love_score),
            }))
            .collect::<Vec<_>>()
    )
}

fn chart_def_json(chart: &ChartDef) -> Value {
    json!({
        "id": chart.id,
        "name": chart.name,
        "provider": chart.provider,
        "category": chart.category.as_str(),
        "internal": chart.is_internal(),
        "builtin": chart.builtin,
        "enabled": chart.enabled,
        "trackCount": chart.track_count,
        "lastFetchedUtc": chart.last_fetched_utc,
        "lastFailureUtc": chart.last_failure_utc,
        "lastFailureReason": chart.last_failure_reason,
        "ok": chart.ok(),
    })
}

/// `GET .../charts` — 1단계 분류 카드 + 각 분류의 차트 목록 (V3 §15.3).
///
/// **작동하지 않는 차트는 유저 UI 목록에서 뺀다**(§15.2). 빈 차트를 눌렀는데 아무 일도
/// 안 일어나는 게 제일 나쁘다. 관리자에게는 실패까지 그대로 보여 준다.
async fn api_charts(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let manager = ctx.tier.is_manager();
    let stats_on = state.app.stats.is_some();
    let charts = state.app.remote.list_charts(guild_id);
    let categories: Vec<Value> = ChartCategory::ALL
        .iter()
        .map(|category| {
            let items: Vec<Value> = charts
                .iter()
                .filter(|chart| chart.category == *category)
                .filter(|chart| manager || (chart.enabled && chart.ok()))
                // 통계가 꺼져 있으면 우리 차트는 만들 수가 없다 — 조용히 뺀다 (§15.2b).
                .filter(|chart| stats_on || !chart.is_internal())
                .map(chart_def_json)
                .collect();
            json!({
                "key": category.as_str(),
                "label": category.label(),
                "icon": category.icon(),
                "blurb": category.blurb(),
                "charts": items,
            })
        })
        .filter(|category| {
            manager
                || !category["charts"]
                    .as_array()
                    .map(|items| items.is_empty())
                    .unwrap_or(true)
        })
        .collect();
    json_ok(json!({
        "categories": categories,
        "superWeight": ctx.settings.chart_super_weight,
        // 전부 담기 버튼을 숨기지 말고 비활성 + 이유로 그리기 위한 값 (§15.3 · §23.3).
        "canBulkEnqueue": ctx.allows("bulkEnqueue", ctx.settings.bulk_enqueue_rule),
    }))
}

#[derive(Debug, Deserialize)]
struct ChartWindowQuery {
    window: Option<String>,
    /// 화면이 실제로 보내는 이름 (`?period=week`). 이 별칭이 없으면 서버가
    /// `window` 만 읽어서 `이번 주`/`전체` 를 눌러도 늘 `month` 순위가 나온다 (§15.2b).
    #[serde(default)]
    period: Option<String>,
}

/// 캐시 키에 쓰는 기간 이름. 기간이 다르면 다른 순위이므로 키가 반드시 갈려야 한다.
fn chart_window_key(window: crate::stats::ChartWindow) -> &'static str {
    match window {
        crate::stats::ChartWindow::Week => "week",
        crate::stats::ChartWindow::Month => "month",
        crate::stats::ChartWindow::All => "all",
    }
}

impl ChartWindowQuery {
    fn resolve(&self) -> crate::stats::ChartWindow {
        // 빈 값은 "안 보낸 것"으로 본다 — `?window=&period=week` 는 주 단위여야 한다.
        let pick = |value: &Option<String>| -> Option<String> {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let raw = pick(&self.window)
            .or_else(|| pick(&self.period))
            .unwrap_or_else(|| "month".to_string());
        let raw = raw.as_str();
        crate::stats::ChartWindow::parse(raw)
    }
}

/// 우리 차트(`internal:…`)를 통계 DB 에서 만든다 (V3 §15.2b).
///
/// **자동재생으로 나간 곡은 순위에 안 센다.** 같이 세면 차트가 "자동재생이 많이 튼 곡"이 된다.
/// 전체 차트는 곡 제목과 횟수만 준다 — 어느 서버에서 나왔는지, 누가 신청했는지는 안 보낸다.
fn internal_chart_json(
    state: &WebState,
    chart: &ChartDef,
    guild_id: u64,
    window: crate::stats::ChartWindow,
    weight: u32,
) -> Option<Value> {
    let stats = state.app.stats.as_ref()?;
    let suffix = chart.url.trim_start_matches(INTERNAL_CHART_PREFIX);
    let (scope, kind) = match suffix {
        "guild-plays" => (guild_id, crate::stats::ChartKind::Plays),
        "guild-love" => (guild_id, crate::stats::ChartKind::Love),
        "global-plays" => (STATS_ALL_GUILDS, crate::stats::ChartKind::Plays),
        "global-love" => (STATS_ALL_GUILDS, crate::stats::ChartKind::Love),
        _ => return None,
    };
    let rows = stats.chart(scope, kind, window, weight, OURS_CHART_LIMIT);
    // 숫자를 **`tracks` 에도 붙인다.** 화면은 `tracks` 를 그리므로, 통계를 `rows` 에만 두면
    // `42회 재생 · 7명이 신청` · `👍284 + ⭐52×2 = 388` 이 하나도 안 나오고
    // 그냥 일반 곡 목록이 된다 (§15.2b — 순위가 왜 그런지 보여야 한다).
    let tracks: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut track = row.track.clone();
            if let Some(map) = track.as_object_mut() {
                map.insert("rank".into(), json!(index + 1));
                map.insert("cacheKey".into(), json!(row.cache_key));
                // 프런트가 읽는 이름(`plays`)과 백엔드 이름(`playsUser`)을 둘 다 준다.
                map.insert("plays".into(), json!(row.plays_user));
                map.insert("playsUser".into(), json!(row.plays_user));
                map.insert("playsAutoplay".into(), json!(row.plays_autoplay));
                map.insert("likes".into(), json!(row.likes));
                map.insert("supers".into(), json!(row.supers));
                map.insert("requesters".into(), json!(row.requesters));
                map.insert("loveScore".into(), json!(row.love_score));
                map.insert("superWeight".into(), json!(weight));
                map.insert(
                    "loveFormula".into(),
                    json!(format!(
                        "👍{} + ⭐{}×{} = {}",
                        row.likes, row.supers, weight, row.love_score
                    )),
                );
            }
            track
        })
        .collect();
    Some(json!({
        "chart": chart_def_json(chart),
        "rows": chart_rows_json(&rows, weight),
        "tracks": tracks,
        // 계산식에 쓰는 가중치는 응답 최상위에도 남긴다 (줄마다 반복하지 않아도 되게).
        "superWeight": weight,
        "fetchedUtc": now_utc(),
        "stale": false,
        "internal": true,
        // 전체 차트는 서버 이름도 신청자도 안 보여 준다 (§15.2b 사생활).
        "anonymous": scope == STATS_ALL_GUILDS,
    }))
}

/// `GET .../charts/{id}` — 그 차트의 곡 목록.
async fn api_chart_detail(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, chart_id)): Path<(u64, i64)>,
    Query(query): Query<ChartWindowQuery>,
    headers: HeaderMap,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(chart) = state.app.remote.get_chart(guild_id, chart_id) else {
        return json_error(StatusCode::NOT_FOUND, "그 차트를 찾지 못했어요.");
    };
    let weight = ctx.settings.chart_super_weight;
    if chart.is_internal() {
        let window = query.resolve();
        // 우리 차트는 요청마다 `stat_track_daily` 를 GROUP BY 로 훑는다. `/stats/*` 와 같은
        // 60초 캐시를 태워야 여러 명이 새로고침해도 통계 DB 뮤텍스를 계속 물지 않는다 (§23.2).
        let key = format!(
            "chart:{guild_id}:{}:{}:{weight}",
            chart.id,
            chart_window_key(window)
        );
        let payload = stats_cached(&state, key, || {
            internal_chart_json(&state, &chart, guild_id, window, weight)
                .unwrap_or(Value::Null)
        });
        return if payload.is_null() {
            stats_unavailable()
        } else {
            json_ok(payload)
        };
    }
    match fetch_chart_tracks(&state, guild_id, &chart, false).await {
        Ok(snapshot) => {
            let payload = json!({
                "chart": chart_def_json(&chart),
                "tracks": snapshot.tracks,
                "fetchedUtc": snapshot.fetched_utc,
                "stale": snapshot.stale,
                "internal": false,
            });
            // 순위는 자주 안 바뀐다. 해시를 붙여 두면 같은 목록을 두 번 내려보내지 않는다 (§15.6).
            json_ok_etag(payload, headers.get(header::IF_NONE_MATCH))
        }
        Err(error) => json_error(StatusCode::BAD_GATEWAY, error),
    }
}

/// 내용 해시를 `ETag` 로 붙여 응답한다. 브라우저가 같은 해시를 들고 오면 `304` 를 준다 (§15.6).
///
/// 한 번 받은 랭킹을 다시 받지 않게 하는 것이 목적이다. 차트 한 장이 100곡이라
/// 새로고침마다 통째로 내려보내면 모바일에서 체감이 크다.
/// **본문이 바뀌면 해시도 바뀌므로** 오래된 목록이 굳을 걱정은 없다.
fn json_ok_etag(value: Value, if_none_match: Option<&HeaderValue>) -> Response {
    use sha2::Digest;
    let body = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    // 앞 8바이트면 충돌 걱정 없이 짧다. 에셋 쪽 `etag_of` 와 같은 규칙이다.
    let digest = sha2::Sha256::digest(body.as_bytes());
    let etag = format!(
        "\"{}\"",
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    if let Some(sent) = if_none_match.and_then(|value| value.to_str().ok())
        && sent.split(',').any(|part| part.trim() == etag)
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        StatusCode::OK,
        [
            (header::ETAG, etag),
            // 캐시는 우리 해시로 판정한다. 시간 기반 캐시를 같이 걸면 순위가 바뀌었는데도
            // 브라우저가 옛 목록을 그냥 쓰는 구간이 생긴다.
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (header::CONTENT_TYPE, "application/json".to_string()),
        ],
        body,
    )
        .into_response()
}

/// 차트 하나를 펼친다. 캐시가 살아 있으면 그걸 그대로 준다 (V3 §15.1).
///
/// **같은 차트를 동시에 요청하면 하나만 실행한다.** 안 그러면 yt-dlp 가 줄줄이 선다.
/// 뒤에 선 요청은 잠깐 기다렸다 캐시를 다시 본다.
/// 차트 페치 잠금을 **어떤 경로로 빠져나가도** 푸는 가드.
/// `?` 로 일찍 빠지거나 요청이 취소돼 future 가 drop 돼도 `Drop` 은 반드시 돈다.
struct ChartFetchGuard {
    state: Arc<WebState>,
    chart_id: i64,
}

impl Drop for ChartFetchGuard {
    fn drop(&mut self) {
        self.state.app.remote.end_chart_fetch(self.chart_id);
    }
}

/// 한가할 때 식은 차트 **한 장**을 미리 받아 둔다 (§15.3).
///
/// 한 장만 하는 게 핵심이다. 40장을 한 번에 돌리면 프리페치가 yt-dlp 를 몇 분씩
/// 붙들어서 정작 사람이 검색할 때 밀린다. 10분마다 한 장이면 6시간 캐시 수명 안에
/// 자주 보는 차트는 늘 따뜻하게 유지된다.
pub fn spawn_chart_prefetch(state: &Arc<WebState>) {
    let state = state.clone();
    tokio::spawn(async move {
        // 재생 중이면 건너뛴다. 사람이 듣고 있을 때 백그라운드가 yt-dlp 를 다투면 안 된다.
        if !state.app.coordinator.active_guild_ids().await.is_empty() {
            return;
        }
        // **화면을 열어 둔 사람이 있다는 이유로는 미루지 않는다.**
        // 예전엔 여기서도 건너뛰었는데, 리모컨을 켜 두는 게 정상 사용이라 프리페치가 사실상
        // 영영 안 돌았다("차트가 늘 비어 있다"). 한 tick 에 한 장이고 재생 중에는 어차피
        // 건너뛰므로, 사람이 검색할 때 밀릴 위험은 차트가 늘 식어 있는 것보다 작다.

        let guild_ids = state.app.db.list_known_guild_ids();
        for guild_id in guild_ids {
            // 승인 안 된 서버 것까지 미리 받아 둘 이유가 없다.
            if !state
                .app
                .remote
                .guild_approval(guild_id)
                .map(|row| row.status.is_usable())
                .unwrap_or(false)
            {
                continue;
            }
            let charts = state.app.remote.list_charts(guild_id);
            let stale = charts.into_iter().find(|chart| {
                chart.enabled
                    && !chart.is_internal()
                    && state
                        .app
                        .remote
                        .chart_cache(chart.id)
                        .is_none_or(|snapshot| snapshot.stale)
            });
            let Some(chart) = stale else { continue };
            match fetch_chart_tracks(&state, guild_id, &chart, false).await {
                Ok(snapshot) => state.app.log.info(
                    "Chart",
                    &format!(
                        "미리 받아 뒀어요: '{}' {}곡 (아무도 안 쓸 때)",
                        chart.name,
                        snapshot.tracks.len()
                    ),
                ),
                Err(reason) => state
                    .app
                    .log
                    .info("Chart", &format!("'{}' 미리 받기 실패: {reason}", chart.name)),
            }
            return; // 한 tick 에 한 장만.
        }
    });
}

/// 빈 채널 규칙을 화면이 쓸 모양으로 (§27).
/// `locked` 면 서버 주인이 못 바꾼다 — 이유까지 같이 실어 준다.
fn empty_voice_json(state: &WebState, guild_id: u64) -> Value {
    let rule = crate::app::empty_voice_rule(&state.app, guild_id);
    json!({
        "policy": rule.policy.as_str(),
        "policyLabel": rule.policy.label(),
        "policyDescription": rule.policy.description(),
        "delaySeconds": rule.delay_seconds,
        "locked": !rule.editable(),
        "lockReason": rule.lock_reason(),
        "options": [
            option_json(crate::models::EmptyVoiceChannelPolicy::DoNothing),
            option_json(crate::models::EmptyVoiceChannelPolicy::StopPlayback),
            option_json(crate::models::EmptyVoiceChannelPolicy::AutoLeave),
        ],
    })
}

fn option_json(policy: crate::models::EmptyVoiceChannelPolicy) -> Value {
    json!({
        "value": policy.as_str(),
        "label": policy.label(),
        "description": policy.description(),
    })
}

/// `GET /music/api/changelog` — 패치노트 (§30).
///
/// exe 에 박아 넣은 `docs/CHANGELOG.md` 를 그대로 준다. **원본이 하나뿐**이라
/// 문서와 화면이 갈라질 수 없다. 마크다운 해석은 클라이언트가 한다 — 서버가 HTML 을
/// 만들어 주면 거기서 이스케이프 실수가 나면 곧장 XSS 다.
async fn api_changelog(headers: HeaderMap) -> Response {
    let text = crate::web::assets::CHANGELOG_MD;
    // 첫 `## ` 제목이 최신 버전 이름이다. 새 버전 안내 문구에 쓴다.
    let latest = text
        .lines()
        .find(|line| line.starts_with("## "))
        .map(|line| line.trim_start_matches("## ").trim().to_string());
    json_ok_etag(
        json!({ "markdown": text, "latest": latest }),
        headers.get(header::IF_NONE_MATCH),
    )
}

/// `GET /music/apidoc` — API 가이드 문서. 링크로만 들어온다(진입 버튼을 만들지 않았다).
///
/// **로그인을 요구한다.** 리모컨에서 세션 없이 열리는 페이지는 `/music/login` 계열과,
/// 서버가 직접 켜야 보이는 `/music/guilds/{id}/now` 뿐이다. 나머지 페이지(`/music`,
/// `/music/guilds/{id}`)는 전부 세션부터 본다. 이 문서는 관리자·봇 주인 전용 경로까지
/// 한자리에 늘어놓아서, "무엇이 바뀌었나" 만 알려 주는 패치노트와는 성격이 다르다.
/// 비밀은 아니지만 공격 표면 목록을 로그인 없이 뿌릴 이유도 없다.
///
/// 반대로 **길드 인가는 태우지 않는다.** 문서에 길드 데이터가 한 줄도 없어서 특정 서버의
/// 멤버여야 할 이유가 없다. 개인 설정(`/music/api/prefs`)과 같은 수준 — 세션만 본다.
///
/// 401 을 JSON 으로 주지 않고 로그인 화면으로 보낸다. 사람이 브라우저로 여는 주소라
/// `{"error":…}` 를 그대로 띄우면 막다른 길이 된다. `next` 를 달아 로그인 뒤 돌아오게 한다.
async fn apidoc_page(State(state): State<Arc<WebState>>, cookies: Cookies) -> Response {
    if current_session(&state, &cookies).is_none() {
        let next = percent_encode("/music/apidoc");
        return Redirect::to(&format!("/music/login?next={next}")).into_response();
    }
    html_page(remote_page::apidoc())
}

/// `GET /music/guilds/{id}/now` — 로그인 없이 보는 화면 (§29).
/// 로그인한 사람은 리모컨으로 보낸다 — 이 화면보다 나은 걸 볼 수 있으니까.
async fn public_now_page(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    if current_session(&state, &cookies).is_some() {
        return Redirect::to(&format!("/music/guilds/{guild_id}")).into_response();
    }
    html_page(remote_page::public_now(guild_id, &state.app.build_id))
}

/// `GET /music/api/guilds/{id}/public` — 로그인 없이 보는 지금 곡 (§29).
///
/// **여기서 나가는 것은 곡뿐이다.** 신청한 사람 이름, 채팅, 멤버, 대기열 신청자,
/// 투표 정보는 하나도 안 실린다 — 그건 그 서버 안 사람들 정보고, 로그인하지 않은
/// 사람에게 줄 이유가 없다. 대기열도 **제목만** 앞 5곡까지다.
///
/// 서버가 끄면 404. 승인 안 된 서버도 404 — 아직 쓸 수 없는 서버의 활동을
/// 밖에 보여 줄 이유가 없다. "꺼짐"과 "없음"을 구분해 주지 않는 것도 의도다.
async fn api_public_now_playing(
    State(state): State<Arc<WebState>>,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let approved = state
        .app
        .remote
        .guild_approval(guild_id)
        .map(|row| row.status.is_usable())
        .unwrap_or(false);
    let settings = state.app.remote.load_guild_settings(guild_id);
    if !approved || !settings.public_now_playing || !bot_in_guild(&state, guild_id) {
        return json_error(StatusCode::NOT_FOUND, "여기서는 볼 수 없어요.");
    }

    let player = state.app.player.get_state(guild_id).await;
    let position = state
        .app
        .coordinator
        .current_position(guild_id)
        .await
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0);

    let current = player.current_item.as_ref().map(|item| {
        json!({
            "title": item.track.title.clone().unwrap_or_else(|| "제목 없음".into()),
            "artist": item.track.artist,
            "durationSeconds": item.track.duration.map(|d| d.as_secs_f64()),
        })
    });
    // 제목만. 누가 넣었는지는 안 준다.
    let upcoming: Vec<Value> = player
        .upcoming
        .iter()
        .take(5)
        .map(|item| {
            json!({ "title": item.track.title.clone().unwrap_or_else(|| "제목 없음".into()) })
        })
        .collect();

    let payload = json!({
        // 서버 이름은 승인 기록에 이미 들고 있다. 그것만 쓴다.
        "guildName": state
            .app
            .remote
            .guild_approval(guild_id)
            .and_then(|row| row.guild_name),
        "current": current,
        "isPaused": player.is_paused,
        "positionSeconds": position,
        "sampledAtUtc": now_utc(),
        "upcoming": upcoming,
        "queueTotal": player.upcoming.len(),
        "readOnly": true,
    });
    json_ok_etag(payload, headers.get(header::IF_NONE_MATCH))
}

// ───────────────────────── 서버 승인 (§26) ─────────────────────────

/// 봇 주인인지 확인하고 세션을 돌려준다. 길드 인가를 안 태우는 라우트 전용이다.
fn require_owner(
    state: &Arc<WebState>,
    cookies: &Cookies,
    headers: Option<&HeaderMap>,
) -> Result<RemoteSession, Response> {
    let session = current_session(state, cookies)
        .ok_or_else(|| json_error(StatusCode::UNAUTHORIZED, "로그인이 필요해요."))?;
    if let Some(headers) = headers
        && !verify_csrf(&session, headers)
    {
        return Err(json_error(StatusCode::FORBIDDEN, "CSRF 검증에 실패했어요."));
    }
    if !session.is_developer && !is_owner_user(state, session.user_id) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "봇 주인만 볼 수 있어요.",
        ));
    }
    Ok(session)
}

/// `GET /music/api/owner/guilds` — 승인 대기·사용 중·차단된 서버 목록.
async fn api_owner_guilds(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
) -> Response {
    if let Err(response) = require_owner(&state, &cookies, None) {
        return response;
    }
    let rows: Vec<Value> = state
        .app
        .remote
        .list_guild_approvals()
        .into_iter()
        .map(|row| {
            json!({
                "guildId": row.guild_id.to_string(),
                "status": row.status.as_str(),
                "statusLabel": row.status.label(),
                // 봇이 지금도 그 서버에 있는지. 나간 서버를 승인해 봐야 소용없다.
                "botInGuild": bot_in_guild(&state, row.guild_id),
                "name": row.guild_name,
                "requestedUtc": row.requested_utc,
                "decidedUtc": row.decided_utc,
                "note": row.note,
            })
        })
        .collect();
    json_ok(json!({ "guilds": rows }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnerDecideRequest {
    guild_id: String,
    /// `approved` 또는 `blocked`. 되돌리려면 `pending`.
    status: String,
    note: Option<String>,
}

/// `POST /music/api/owner/guilds/decide` — 승인·차단.
async fn api_owner_decide(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    headers: HeaderMap,
    Json(request): Json<OwnerDecideRequest>,
) -> Response {
    let session = match require_owner(&state, &cookies, Some(&headers)) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Ok(guild_id) = request.guild_id.trim().parse::<u64>() else {
        return json_error(StatusCode::BAD_REQUEST, "서버 ID가 올바르지 않아요.");
    };
    let Some(status) = crate::remote::GuildApprovalStatus::parse(request.status.trim()) else {
        return json_error(StatusCode::BAD_REQUEST, "알 수 없는 상태예요.");
    };
    let note = request.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if !state
        .app
        .remote
        .decide_guild(guild_id, status, session.user_id, note)
    {
        return json_error(StatusCode::NOT_FOUND, "그 서버 기록을 찾지 못했어요.");
    }
    state.app.log.info(
        "Bot",
        &format!(
            "{}님이 서버 {guild_id} 를 {} 로 바꿨어요.",
            session.display_name,
            status.label()
        ),
    );
    json_ok(json!({ "ok": true, "status": status.as_str() }))
}

// ───────────────────────── 전역 강제값 (봇 주인 오버라이드) ─────────────────────────

/// 강제값 묶음을 화면이 그대로 쓸 수 있는 모양으로. 봇 주인 화면과 서버 관리 콘솔이
/// **같은 함수**를 쓴다 — 두 화면이 다른 모양을 받으면 자물쇠가 한쪽에만 그려진다.
fn overrides_json(overrides: &GlobalOverrides) -> Value {
    let locked = overrides.locked_keys();
    let values: serde_json::Map<String, Value> = locked
        .iter()
        .filter_map(|key| {
            overrides
                .locked_value(key)
                .map(|value| ((*key).to_string(), value))
        })
        .collect();
    let labels: serde_json::Map<String, Value> = GlobalOverrides::LOCKABLE_KEYS
        .iter()
        .map(|key| ((*key).to_string(), json!(GlobalOverrides::label_for(key))))
        .collect();
    json!({
        // 지금 잠긴 항목. 화면은 이 배열만 보고 자물쇠를 그리면 된다.
        "lockedKeys": locked,
        // 잠긴 항목 → 강제된 값. 자물쇠 옆에 "무엇으로 잠겼는지" 를 적을 때 쓴다.
        "values": Value::Object(values),
        // 강제할 수 있는 항목 전부. 봇 주인 화면이 줄을 이걸로 그린다.
        "lockableKeys": GlobalOverrides::LOCKABLE_KEYS,
        // 항목 → 한국어 이름. 화면이 라벨 표를 따로 들고 있다가 어긋나지 않게 서버가 준다.
        "labels": Value::Object(labels),
        // 왜 못 바꾸는지. 잠긴 사실만 보이면 고장으로 읽힌다.
        "reason": crate::remote::OVERRIDE_LOCK_REASON,
    })
}

/// 보낸 본문이 잠긴 항목을 **실제로 바꾸려 하는지**. 바꾸려는 키의 camelCase 이름을 돌려준다.
///
/// **값이 실제로 다를 때만** 시도로 친다. 관리 콘솔은 섹션의 키를 통째로 실어 보내므로
/// "키가 들어 있으면 시도" 로 보면 잠긴 항목 하나 때문에 그 섹션 전체가 저장 불능이 된다.
/// 화면이 보여 준 강제값을 그대로 되보내는 것은 바꾸려는 시도가 아니다.
fn attempted_locked_keys(
    overrides: &GlobalOverrides,
    body: &serde_json::Map<String, Value>,
) -> Vec<&'static str> {
    overrides
        .locked_keys()
        .into_iter()
        .filter(|key| {
            body.get(*key)
                .is_some_and(|sent| overrides.locked_value(key).as_ref() != Some(sent))
        })
        .collect()
}

/// 잠긴 항목을 건드렸으면 403. 아니면 `None`.
///
/// **조용히 무시하지 않는다.** 저장된 척하면 화면은 바뀐 값을 보여 주다가 새로고침하면
/// 되돌아간다 — 제일 헷갈리는 실패다 (빈 채널 규칙 §27 이 같은 이유로 이미 거절한다).
fn override_lock_response(
    overrides: &GlobalOverrides,
    body: &serde_json::Map<String, Value>,
) -> Option<Response> {
    let blocked = attempted_locked_keys(overrides, body);
    if blocked.is_empty() {
        return None;
    }
    let names: Vec<&str> = blocked
        .iter()
        .map(|key| GlobalOverrides::label_for(key))
        .collect();
    Some(json_error(
        StatusCode::FORBIDDEN,
        format!(
            "{} — {}",
            names.join(", "),
            crate::remote::OVERRIDE_LOCK_REASON
        ),
    ))
}

/// `GET /music/api/owner/overrides` — 지금 걸린 전역 강제값.
async fn api_owner_overrides_get(State(state): State<Arc<WebState>>, cookies: Cookies) -> Response {
    if let Err(response) = require_owner(&state, &cookies, None) {
        return response;
    }
    let overrides = state.app.remote.load_global_overrides();
    json_ok(json!({
        "overrides": overrides_json(&overrides),
        // 화면이 `∞` 칸을 그리려면 어떤 항목이 무제한을 받는지 알아야 한다 (§23.1).
        "unlimitedKeys": UNLIMITED_KEYS,
    }))
}

/// `PUT /music/api/owner/overrides` — 부분 갱신.
///
/// 규약은 관리 콘솔의 섹션 저장과 같다: **보낸 키만** 바뀐다.
/// - 값을 보내면 그 값으로 강제한다.
/// - `null` 을 보내면 강제를 푼다 (서버가 정한 값이 되살아난다).
/// - 아예 안 보낸 키는 건드리지 않는다.
///
/// "강제 안 함" 과 "강제로 false" 가 다른 상태라서 `null` 이 꼭 필요하다.
/// 안 보낸 키를 해제로 치면, 한 항목을 켜려고 보낸 요청이 나머지를 전부 풀어 버린다.
async fn api_owner_overrides_put(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let session = match require_owner(&state, &cookies, Some(&headers)) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return json_error(StatusCode::BAD_REQUEST, "본문은 객체여야 해요.");
    };
    // 모르는 키를 조용히 버리면 봇 주인은 저장된 줄 알고 화면을 닫는다.
    for key in body.keys() {
        if !GlobalOverrides::LOCKABLE_KEYS.contains(&key.as_str()) {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("{key}: 강제할 수 없는 항목이에요."),
            );
        }
    }

    // 지금 값 위에 보낸 키만 얹는다. `null` 은 해제라서 `serde_json::from_value` 로
    // 한 번에 못 받는다 — `Option` 필드에 `null` 을 넣으면 "없음" 과 구분이 안 된다.
    let current = state.app.remote.load_global_overrides();
    let mut merged = serde_json::to_value(&current).unwrap_or_else(|_| json!({}));
    let Some(map) = merged.as_object_mut() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "강제값을 읽지 못했어요.");
    };
    for (key, value) in body {
        if value.is_null() {
            map.remove(key);
        } else {
            map.insert(key.clone(), value.clone());
        }
    }
    let Ok(mut next) = serde_json::from_value::<GlobalOverrides>(merged) else {
        return json_error(StatusCode::BAD_REQUEST, "값의 형식이 올바르지 않아요.");
    };
    // 봇 주인이라고 해서 서버가 못 받는 값을 넣을 수 있으면 안 된다 (§23.1).
    next.sanitize();

    if let Err(error) = state.app.remote.save_global_overrides(&next) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    // **캐시를 안 버리면 강제값이 재시작 전까지 안 먹는다.**
    // `PlayerManager` 는 길드 설정을 길드마다 캐시하는데, 그 캐시가 채워진 시점의
    // 유효값을 들고 있다. 길드 설정 저장은 그 길드만 무효화하면 되지만 전역 강제값은
    // **모든 길드**의 유효값을 한꺼번에 바꾼다. 아는 길드를 전부 턴다.
    let mut touched: HashSet<u64> = state.app.remote.remote_guild_ids().into_iter().collect();
    if let Some(cache) = state.app.discord_cache.get() {
        touched.extend(cache.guilds().into_iter().map(|id| id.get()));
    }
    for guild_id in &touched {
        state.app.player.invalidate_settings(*guild_id);
        // 화면도 다시 읽게 한다 — 열어 둔 관리 콘솔이 옛 값을 그대로 보여 주면 안 된다.
        emit_bare(&state, *guild_id, "settings");
    }

    let locked = next.locked_keys();
    state.app.log.info(
        "Bot",
        &format!(
            "{}님이 전역 강제값을 바꿨어요 — 잠긴 항목 {}개: {}",
            session.display_name,
            locked.len(),
            if locked.is_empty() {
                "없음".to_string()
            } else {
                locked.join(", ")
            }
        ),
    );
    json_ok(json!({ "ok": true, "overrides": overrides_json(&next) }))
}

/// 차트에 넣어 줄 한 곡의 최대 길이(초).
///
/// 15분을 넘는 것은 사실상 전부 모음·메들리·라이브 전곡 영상이다. 실제로 겪은 것:
/// 검색형 인기 차트의 7시간짜리 플레이리스트, TJ 노래방 1위였던 86분짜리 임영웅메들리.
/// 진짜 긴 곡(프로그레시브 록 등)이 잘릴 수 있지만, 대기열 한 칸이 몇 시간 잠기는 쪽이 더 나쁘다.
const CHART_MAX_TRACK_SECS: f64 = 15.0 * 60.0;

async fn fetch_chart_tracks(
    state: &Arc<WebState>,
    guild_id: u64,
    chart: &ChartDef,
    force: bool,
) -> Result<ChartSnapshot, String> {
    if !force {
        if let Some(snapshot) = state.app.remote.chart_cache(chart.id) {
            if !snapshot.stale {
                return Ok(snapshot);
            }
        }
    }
    if !state.app.remote.try_begin_chart_fetch(chart.id) {
        // 다른 요청이 이미 돌고 있다. 잠깐 기다렸다 캐시를 본다.
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !state.app.remote.is_chart_fetching(chart.id) {
                break;
            }
        }
        return state
            .app
            .remote
            .chart_cache(chart.id)
            .ok_or_else(|| "차트를 가져오는 중이에요. 잠시 뒤에 다시 열어 주세요.".to_string());
    }
    // **여기부터는 반드시 잠금을 푼다.** yt-dlp 가 몇 초 도는 동안 브라우저가 탭을 닫으면
    // axum 이 이 future 를 drop 하는데, 그때 `end_chart_fetch` 를 그냥 호출문으로 두면
    // 영영 실행되지 않아 그 차트가 프로세스가 죽을 때까지 "가져오는 중"으로 굳는다.
    // 관리자 `↻ 새로고침` 도 같은 경로라 풀 방법이 없었다. RAII 가드가 drop 경로까지 덮는다.
    let _guard = ChartFetchGuard {
        state: state.clone(),
        chart_id: chart.id,
    };
    let provider = match chart.provider.as_str() {
        "YouTubeMusic" => ProviderKind::YouTubeMusic,
        "SoundCloud" => ProviderKind::SoundCloud,
        _ => ProviderKind::YouTube,
    };
    // 검색형 차트는 URL 에 개수가 박혀 있다. 설정값(10~100)으로 갈아 끼운다.
    let limit = state
        .app
        .remote
        .load_guild_settings(guild_id)
        .chart_limit();

    // TJ 노래방은 yt-dlp 로 펼칠 주소가 아니다. 공식 API 로 순위를 받아 곡마다 반주를 찾는다.
    let mut tracks = if let Some(tj_chart) = crate::remote::tj::TjChart::parse(&chart.url) {
        match crate::remote::tj::fetch(
            tj_chart,
            limit as usize,
            &state.app.remote,
            &state.app.ytdlp(),
            &http_client(state),
            crate::remote::tj::default_resolve_budget(),
        )
        .await
        {
            Ok(tracks) => tracks,
            Err(reason) => {
                let _ = state.app.remote.mark_chart_failure(chart.id, &reason);
                return state
                    .app
                    .remote
                    .chart_cache(chart.id)
                    .ok_or(reason);
            }
        }
    } else {
        let url = crate::remote::models::chart_url_with_limit(&chart.url, limit);
        state.app.ytdlp().expand_collection(&url, provider).await
    };
    // **차트에는 곡이 아닌 것이 섞여 들어온다.** 실제로 겪은 두 가지다.
    //   - 검색형 인기 차트가 7시간짜리 "노래모음 플레이리스트" 영상을 물어 왔다.
    //   - TJ 노래방 차트 1위가 86분짜리 `임영웅메들리` 였다.
    // 둘 다 한 곡처럼 들어와서 대기열 한 칸을 몇 시간 동안 차지한다.
    // 길이를 모르는 항목은 남긴다 — 모른다고 버리면 멀쩡한 곡까지 사라진다.
    let before = tracks.len();
    tracks.retain(|track| {
        track
            .duration
            .is_none_or(|duration| duration.as_secs_f64() <= CHART_MAX_TRACK_SECS)
    });
    if tracks.len() < before {
        state.app.log.info(
            "Chart",
            &format!(
                "차트 '{}' 에서 {}분 넘는 항목 {}개를 뺐어요(모음·메들리).",
                chart.name,
                (CHART_MAX_TRACK_SECS / 60.0) as i64,
                before - tracks.len()
            ),
        );
    }
    // 재생목록형은 URL 로 개수를 못 줄이므로 여기서 자른다.
    tracks.truncate(limit as usize);
    if tracks.is_empty() {
        // **숨기지 말고 그대로 알린다** (§15.2). 관리 콘솔이 실패 시각을 보여 준다.
        let _ = state
            .app
            .remote
            .mark_chart_failure(chart.id, "곡 목록이 비어 있어요.");
        // 캐시에 예전 값이 있으면 빈 화면보다는 그게 낫다.
        return state
            .app
            .remote
            .chart_cache(chart.id)
            .ok_or_else(|| "지금 이 차트를 가져오지 못했어요.".to_string());
    }
    let _ = state.app.remote.save_chart_cache(chart.id, &tracks);
    Ok(ChartSnapshot {
        tracks,
        fetched_utc: now_utc(),
        stale: false,
    })
}

/// `POST .../charts/{id}/enqueue` — 전부 담기. `bulkEnqueue` 권한 (V3 §15.4).
async fn api_chart_enqueue(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, chart_id)): Path<(u64, i64)>,
    Query(query): Query<ChartWindowQuery>,
    headers: HeaderMap,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(response) = ctx.require(
        "bulkEnqueue",
        ctx.settings.bulk_enqueue_rule,
        "차트를 통째로 담을 권한이 없어요.",
    ) {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    let Some(chart) = state.app.remote.get_chart(guild_id, chart_id) else {
        return json_error(StatusCode::NOT_FOUND, "그 차트를 찾지 못했어요.");
    };
    let tracks: Vec<TrackRef> = if chart.is_internal() {
        let Some(stats) = state.app.stats.as_ref() else {
            return stats_unavailable();
        };
        let suffix = chart.url.trim_start_matches(INTERNAL_CHART_PREFIX);
        let (scope, kind) = match suffix {
            "guild-love" => (guild_id, crate::stats::ChartKind::Love),
            "global-plays" => (STATS_ALL_GUILDS, crate::stats::ChartKind::Plays),
            "global-love" => (STATS_ALL_GUILDS, crate::stats::ChartKind::Love),
            _ => (guild_id, crate::stats::ChartKind::Plays),
        };
        stats
            .chart(
                scope,
                kind,
                // **화면에 보이는 기간 그대로 담는다** (§15.4). 여기를 `Month` 로 박아 두면
                // `전체` 를 보고 `전부 담기` 를 눌러도 이번 달 목록이 들어가서,
                // 사용자가 본 것과 담긴 것이 달라진다.
                query.resolve(),
                ctx.settings.chart_super_weight,
                OURS_CHART_LIMIT,
            )
            .into_iter()
            .filter_map(|row| serde_json::from_value::<TrackRef>(row.track).ok())
            .collect()
    } else {
        match fetch_chart_tracks(&state, guild_id, &chart, false).await {
            Ok(snapshot) => snapshot.tracks,
            Err(error) => return json_error(StatusCode::BAD_GATEWAY, error),
        }
    };
    if tracks.is_empty() {
        return json_error(StatusCode::CONFLICT, "이 차트에는 담을 곡이 없어요.");
    }

    let player = state.app.player.get_state(guild_id).await;
    let existing: HashSet<String> = player
        .current_item
        .iter()
        .chain(player.upcoming.iter())
        .map(|item| item.track.cache_key())
        .collect();
    let outcome = bulk_enqueue(
        &state,
        &ctx,
        &tracks,
        &existing,
        &player,
        &format!("차트 {}", chart.name),
    )
    .await;
    if outcome.added > 0 {
        // 사람 피드에는 **한 줄**만 남긴다 (§13.3). 100곡을 한 줄씩 남기면 피드가 도배된다.
        let titles: Vec<String> = tracks
            .iter()
            .take(200)
            .map(|track| track.display_title().to_string())
            .collect();
        let _ = state.app.remote.add_audit_bulk(
            guild_id,
            ctx.user_id(),
            &ctx.session.display_name,
            "chart.enqueue",
            Some(&chart.name),
            outcome.added as u32,
            &titles,
        );
        emit_bare(&state, guild_id, "audit");
        broadcast_queue(&state, guild_id).await;
    }
    let mut payload = outcome.to_json();
    payload["chart"] = json!(chart.name);
    json_ok(payload)
}

/// `POST .../charts/{id}/refresh` — 관리자만. 캐시를 무시하고 다시 가져온다.
async fn api_chart_refresh(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path((guild_id, chart_id)): Path<(u64, i64)>,
    headers: HeaderMap,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(chart) = state.app.remote.get_chart(guild_id, chart_id) else {
        return json_error(StatusCode::NOT_FOUND, "그 차트를 찾지 못했어요.");
    };
    if chart.is_internal() {
        // 우리 차트는 통계 DB 에서 즉석에서 만든다 — 새로 받을 것 자체가 없다.
        return json_ok(json!({
            "ok": true,
            "message": "우리 차트는 늘 최신이라 새로고침할 게 없어요.",
        }));
    }
    match fetch_chart_tracks(&state, guild_id, &chart, true).await {
        Ok(snapshot) => {
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "chart.refresh",
                Some(&chart.name),
                Some(&snapshot.tracks.len().to_string()),
            );
            json_ok(json!({
                "ok": true,
                "trackCount": snapshot.tracks.len(),
                "fetchedUtc": snapshot.fetched_utc,
                "message": format!("{}곡을 새로 받았어요.", snapshot.tracks.len()),
            }))
        }
        Err(error) => json_error(StatusCode::BAD_GATEWAY, error),
    }
}

// ───────────────────────── 서버 차단 목록 (V3 §19.2) ─────────────────────────

fn blacklist_json(entry: &crate::models::BlacklistEntry, guild_id: u64) -> Value {
    let global = entry.guild_id == 0;
    json!({
        "id": entry.id,
        "kind": entry.kind.as_str(),
        "kindLabel": entry.kind.label(),
        "pattern": entry.pattern,
        "note": entry.note,
        "createdUtc": entry.created_utc,
        "createdByUserId": entry.created_by_user_id.to_string(),
        // 전역 항목은 보여는 주되 못 지운다 — 왜 막혔는지 모르는 게 제일 답답하다 (§19.1).
        "scope": if global { "global" } else { "guild" },
        "removable": !global && entry.guild_id == guild_id,
    })
}

async fn admin_blacklist_get(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    // 전체 차단 규칙은 **이 화면에 아예 안 나온다** (§19.2). 봇 주인이어도 마찬가지다.
    //
    // 예전에는 주인에게만 섞어서 보여 줬는데, 그러면 같은 화면이 보는 사람에 따라 다른 목록을
    // 내놓는다. 주인이 "이 서버가 막아 둔 것"을 확인하려고 열었는데 남의 서버 방침까지 섞여
    // 나오고, 지우려 해도 여기서는 자기 길드 항목만 만질 수 있어서 손도 못 댄다.
    // 전체 규칙은 운영 패널에서 다룬다 — 화면 하나가 한 가지 범위만 책임지게 둔다.
    let owner = ctx.session.is_developer || is_owner_user(&state, ctx.session.user_id);
    let all = state.app.db.list_blacklist(guild_id);
    let hidden = all.iter().filter(|entry| entry.guild_id == 0).count();
    let items: Vec<Value> = all
        .iter()
        .filter(|entry| entry.guild_id != 0)
        .map(|entry| blacklist_json(entry, guild_id))
        .collect();
    json_ok(json!({
        "items": items,
        // 내용은 숨기되 **있다는 사실은 숨기지 않는다.** 안 그러면 규칙에 걸렸을 때
        // 서버 관리자가 자기 목록만 보고 "여긴 아무것도 없는데 왜 막히지" 로 헤맨다.
        "hasGlobal": hidden > 0,
        "globalNote": (hidden > 0).then_some(if owner {
            "봇 전체에 적용되는 차단 규칙이 따로 있어요. 운영 패널에서 보고 고칠 수 있어요."
        } else {
            "봇 전체에 적용되는 차단 규칙이 따로 있어요. 내용은 봇 주인만 볼 수 있어요."
        }),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlacklistAddRequest {
    kind: String,
    pattern: String,
    note: Option<String>,
}

async fn admin_blacklist_add(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<BlacklistAddRequest>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let Some(kind) = crate::models::BlacklistKind::parse(&request.kind) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "차단 종류는 TitleContains · TitleExact · UrlExact 중 하나예요.",
        );
    };
    let pattern = request.pattern.trim();
    if pattern.is_empty() || pattern.chars().count() > 300 {
        return json_error(StatusCode::BAD_REQUEST, "패턴은 1~300자로 입력해요.");
    }
    let id = state.app.db.add_blacklist(
        guild_id,
        kind,
        pattern,
        ctx.user_id(),
        request.note.as_deref().map(str::trim).filter(|note| !note.is_empty()),
    );
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "blacklist.add",
        Some(pattern),
        Some(kind.as_str()),
    );
    emit_bare(&state, guild_id, "audit");
    json_ok(json!({ "ok": true, "id": id }))
}

#[derive(Debug, Deserialize)]
struct BlacklistRemoveRequest {
    id: i64,
}

/// **전역 항목이면 403** (V3 §19.1). UI 에서 숨기는 것에 의존하지 않고 서버가 막는다.
async fn admin_blacklist_remove(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<BlacklistRemoveRequest>,
) -> Response {
    let ctx = match authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let target = state
        .app
        .db
        .list_blacklist(guild_id)
        .into_iter()
        .find(|entry| entry.id == request.id);
    let Some(target) = target else {
        return json_error(StatusCode::NOT_FOUND, "그 차단 항목을 찾지 못했어요.");
    };
    if !state.app.db.remove_guild_blacklist(request.id, guild_id) {
        return json_error(
            StatusCode::FORBIDDEN,
            if target.guild_id == 0 {
                "이건 봇 전체 규칙이라 서버 관리자가 지울 수 없어요."
            } else {
                "이 서버가 만든 항목만 지울 수 있어요."
            },
        );
    }
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "blacklist.remove",
        Some(&target.pattern),
        Some(target.kind.as_str()),
    );
    emit_bare(&state, guild_id, "audit");
    json_ok(json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
struct BlacklistTestRequest {
    query: String,
}

/// 지금 규칙으로 막히는지 시험한다 (V3 §19.3).
/// 규칙을 넣고 나서 왜 안 막히는지 모르는 상황을 막는 게 목적이라, **어떤 규칙에 걸렸는지**도 준다.
async fn admin_blacklist_test(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<BlacklistTestRequest>,
) -> Response {
    if let Err(response) = authorize_admin(&state, &cookies, guild_id, Some(&headers)).await {
        return response;
    }
    let query = request.query.trim();
    if query.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "시험할 제목이나 주소를 넣어 주세요.");
    }
    // 제목으로도 주소로도 걸릴 수 있으니 둘 다 채운 가짜 곡으로 시험한다.
    let probe = TrackRef {
        provider: ProviderKind::YouTube,
        content_id: query.to_string(),
        source_url: query.to_string(),
        title: Some(query.to_string()),
        artist: None,
        duration: None,
        variant_key: None,
    };
    match state.app.blacklist.try_get_blocker(guild_id, &probe) {
        Some(rule) => json_ok(json!({
            "blocked": true,
            "rule": blacklist_json(&rule, guild_id),
            "message": format!(
                "막혀요 — {}",
                crate::blacklist::Blacklist::describe_rule(&rule)
            ),
        })),
        None => json_ok(json!({
            "blocked": false,
            "rule": Value::Null,
            "message": "지금 규칙으로는 안 막혀요.",
        })),
    }
}

// ───────────────────────── WebSocket ─────────────────────────
//
// S4: 이제 실제 데이터를 나르므로 세션·길드 확인만으로는 부족하다.
// 전체 `authorize` 경로 + Origin 허용목록을 태운다.

async fn api_events(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&state, &headers) {
        return json_error(StatusCode::FORBIDDEN, "허용되지 않은 Origin이에요.");
    }
    if current_session(&state, &cookies).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // CSRF 헤더는 브라우저 WebSocket에서 붙일 수 없으므로 Origin 검사로 대신한다.
    // 읽기전용(Viewer)도 지금 나오는 곡은 볼 수 있어야 하므로 끊지 않는다.
    // 세션·길드·봇 존재 검사에서 떨어지면 4403으로 닫는다.
    let denial = match authorize(&state, &cookies, guild_id, None).await {
        Ok(_) => None,
        Err(response) => Some(response.status().to_string()),
    };
    let user_id = current_session(&state, &cookies)
        .map(|session| session.user_id)
        .unwrap_or(0);
    let receiver = state.remote_events.subscribe();
    ws.on_upgrade(move |socket| async move {
        if let Some(reason) = denial {
            deny_socket(socket, &reason).await;
            return;
        }
        presence_add(&state, guild_id, user_id);
        ensure_guild_watcher(&state, guild_id);
        websocket_loop(socket, receiver, guild_id, user_id).await;
        presence_remove(&state, guild_id, user_id);
    })
}

/// 접근이 거부됐음을 4403으로 알린다. 클라이언트는 재시도하지 않는다.
async fn deny_socket(mut socket: WebSocket, reason: &str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 4403,
            reason: format!("접근이 거부됐어요 ({reason})").into(),
        })))
        .await;
}

async fn websocket_loop(
    mut socket: WebSocket,
    mut receiver: tokio::sync::broadcast::Receiver<RemoteEvent>,
    guild_id: u64,
    user_id: u64,
) {
    // 유휴 시 쿼리 0회 — 하트비트는 Ping 프레임이라 DB를 건드리지 않는다.
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
            event = receiver.recv() => match event {
                Ok(event) if event.targets(guild_id, user_id) => {
                    if socket.send(Message::Text(event.wire().into())).await.is_err() { break; }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

// ───────────────────────── 멘션 / 노래태그 파서 ─────────────────────────

/// 대문자 하나가 여러 글자로 소문자화되는 경우가 있어(터키어 등) 첫 글자만 취해
/// 원문과 1:1 길이를 유지한다. 그래야 매칭 길이를 그대로 커서 이동에 쓸 수 있다.
fn lower_chars(text: &str) -> Vec<char> {
    text.chars()
        .map(|value| value.to_lowercase().next().unwrap_or(value))
        .collect()
}

/// `marker`(`@` 또는 `#`) 뒤에 오는 후보 이름을 **공백 포함 최장일치**로 찾는다.
/// 반환값은 매칭된 후보의 인덱스(등장 순서, 중복 제거).
///
/// 프런트(`portal.js`)의 `collectTags`도 "긴 제목 우선"으로 같은 규칙을 쓴다.
pub fn match_prefixed(content: &str, marker: char, candidates: &[String]) -> Vec<usize> {
    let haystack = lower_chars(content);
    let needles: Vec<Vec<char>> = candidates.iter().map(|name| lower_chars(name)).collect();
    // 최장일치를 보장하려면 긴 후보부터 본다.
    let mut order: Vec<usize> = (0..needles.len()).collect();
    order.sort_by(|left, right| {
        needles[*right]
            .len()
            .cmp(&needles[*left].len())
            .then_with(|| left.cmp(right))
    });

    let mut hits: Vec<usize> = Vec::new();
    let mut cursor = 0usize;
    while cursor < haystack.len() {
        if haystack[cursor] != marker {
            cursor += 1;
            continue;
        }
        let rest = &haystack[cursor + 1..];
        let mut matched: Option<(usize, usize)> = None;
        for index in &order {
            let needle = &needles[*index];
            if needle.is_empty() || needle.len() > rest.len() {
                continue;
            }
            if rest[..needle.len()] == needle[..] {
                matched = Some((*index, needle.len()));
                break;
            }
        }
        match matched {
            Some((index, length)) => {
                if !hits.contains(&index) {
                    hits.push(index);
                }
                cursor += 1 + length;
            }
            None => cursor += 1,
        }
    }
    hits
}

// ───────────────────────── 개발용 시드 ─────────────────────────

async fn seed_dev_guild(state: &WebState, guild_id: u64, user_id: u64) {
    if state
        .app
        .player
        .get_state(guild_id)
        .await
        .current_item
        .is_some()
    {
        return;
    }
    let track = |id: &str, title: &str, artist: &str, seconds: f64| TrackRef {
        provider: ProviderKind::YouTubeMusic,
        content_id: id.into(),
        source_url: format!("https://music.youtube.com/watch?v={id}"),
        title: Some(title.into()),
        artist: Some(artist.into()),
        duration: Some(CsTimeSpan::from_secs_f64(seconds)),
        variant_key: None,
    };
    let current = track("jfKfPfyJRdk", "Midnight Study", "Macham Radio", 214.0);
    let first = track("5qap5aO4i9A", "City Lights", "Lofi Collective", 188.0);
    let second = track("DWcJFNfaw9c", "Soft Focus", "Dream Tapes", 242.0);
    let third = track("7NOSDKb0HlU", "Afterglow", "Night Drive", 205.0);
    state.app.player.connect_voice(guild_id, 1).await;
    state
        .app
        .player
        .enqueue(
            guild_id,
            QueueItem::new_user(current.clone(), "로컬 검증자".into(), Some(user_id)),
            false,
        )
        .await;
    let first_item = QueueItem::new_user(first.clone(), "민서".into(), Some(2001));
    let first_id = first_item.id.clone();
    state.app.player.enqueue(guild_id, first_item, false).await;
    let second_item = QueueItem::new_user(second.clone(), "준호".into(), Some(2002));
    let second_id = second_item.id.clone();
    state.app.player.enqueue(guild_id, second_item, false).await;
    state
        .app
        .player
        .enqueue(
            guild_id,
            QueueItem::new_user(third.clone(), "민서".into(), Some(2001)),
            false,
        )
        .await;
    let _ = state
        .app
        .remote
        .set_vote(guild_id, &first_id, 3001, Some(QueueVoteKind::Like), &first);
    let _ = state.app.remote.set_vote(
        guild_id,
        &second_id,
        3002,
        Some(QueueVoteKind::SuperLike),
        &second,
    );
    state.app.player.refresh_scored_order(guild_id).await;
    let recent_item = QueueItem::new_user(
        track("lTRiuFIWV54", "Rainy Window", "Coffee Shop", 176.0),
        "서연".into(),
        Some(2003),
    );
    let _ = state
        .app
        .remote
        .record_recent(guild_id, &recent_item, "completed");
    let _ = state
        .app
        .remote
        .set_user_track(guild_id, user_id, UserTrackKind::Saved, &third, true);
    let _ = state.app.remote.add_chat_message(
        guild_id,
        2001,
        "민서",
        None,
        "다음 곡 분위기 좋네요 🎧",
        None,
    );
    let _ = state.app.remote.add_chat_message(
        guild_id,
        user_id,
        "로컬 검증자",
        None,
        "마참뮤직 리모컨 동작 확인 중이에요.",
        None,
    );
    let playlist_id =
        state
            .app
            .db
            .create_playlist(PlaylistScope::Guild, Some(guild_id), user_id, "집중할 때");
    for track in [&first, &second] {
        state.app.db.add_playlist_entry(
            playlist_id,
            &PlaylistEntry {
                track: Some(track.clone()),
                collection: None,
                start_offset: None,
                extra: serde_json::Map::new(),
            },
        );
    }
    let _ = state.app.remote.save_lyrics(&LyricsDocument {
        cache_key: current.cache_key(),
        plain_text: Some("깊어지는 밤\n잔잔한 리듬을 따라\n우리의 시간이 흐른다".into()),
        synced_lines: vec![
            LyricsLine {
                start_ms: 0,
                text: "깊어지는 밤".into(),
            },
            LyricsLine {
                start_ms: 12_000,
                text: "잔잔한 리듬을 따라".into(),
            },
            LyricsLine {
                start_ms: 24_000,
                text: "우리의 시간이 흐른다".into(),
            },
        ],
        source: "dev-fixture".into(),
        fetched_utc: now_utc(),
    });
    let _ = state.app.remote.add_audit(
        guild_id,
        user_id,
        "로컬 검증자",
        "dev.seed",
        Some("browser-fixture"),
        None,
        Some("created"),
        true,
        None,
    );
}

// ───────────────────────── 테스트 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_next_only_accepts_internal_remote_paths() {
        // 통과해야 하는 것 — 리모컨 안의 경로
        assert_eq!(
            safe_next(Some("/music/guilds/123")).as_deref(),
            Some("/music/guilds/123")
        );
        assert_eq!(
            safe_next(Some("/music/guilds/123/admin")).as_deref(),
            Some("/music/guilds/123/admin")
        );

        // 막아야 하는 것 — 전부 오픈 리다이렉트로 이어진다
        assert_eq!(safe_next(Some("//evil.example")), None);
        assert_eq!(safe_next(Some("https://evil.example")), None);
        assert_eq!(safe_next(Some("http://evil.example/music/x")), None);
        assert_eq!(safe_next(Some("javascript:alert(1)")), None);
        // 백슬래시를 `/` 로 정규화하는 브라우저가 있어 프로토콜 상대 URL이 될 수 있다
        assert_eq!(safe_next(Some("/music/\\evil.example")), None);
        assert_eq!(safe_next(Some("/music/a\r\nSet-Cookie: x=1")), None);

        // 리모컨 밖은 받지 않는다 — 운영 패널로 튕겨 보내는 데 쓰이면 안 된다
        assert_eq!(safe_next(Some("/botsettings")), None);
        assert_eq!(safe_next(Some("/")), None);
        assert_eq!(safe_next(Some("/music")), None);

        assert_eq!(safe_next(None), None);
        assert_eq!(safe_next(Some("   ")), None);
    }

    fn settings_with(rule: PermissionRule) -> RemoteGuildSettings {
        let mut settings = RemoteGuildSettings::default();
        settings.search_rule = rule;
        settings.configured_role_ids = vec![777];
        settings
    }

    fn member_with_roles(role_ids: Vec<u64>) -> MemberContext {
        MemberContext {
            is_admin: false,
            same_voice_channel: false,
            bot_in_voice: true,
            role_ids,
        }
    }

    // ── V3 §1: 권한 키별 지정 역할 ──

    /// 검색용으로 준 역할이 볼륨·대기열편집까지 열면 안 된다. 그게 §1이 고치는 버그다.
    #[test]
    fn configured_roles_are_scoped_to_their_permission_key() {
        let mut settings = RemoteGuildSettings::default();
        settings.search_rule = PermissionRule::ConfiguredRole;
        settings.volume_rule = PermissionRule::ConfiguredRole;
        settings.rule_role_ids.insert("search".into(), vec![100]);
        settings.rule_role_ids.insert("volume".into(), vec![200]);

        let dj = member_with_roles(vec![100]);
        assert!(permission_allowed("search", settings.search_rule, &settings, &dj));
        assert!(!permission_allowed("volume", settings.volume_rule, &settings, &dj));

        let mixer = member_with_roles(vec![200]);
        assert!(!permission_allowed("search", settings.search_rule, &settings, &mixer));
        assert!(permission_allowed("volume", settings.volume_rule, &settings, &mixer));
    }

    /// 옛 설정을 쓰던 서버의 동작이 조용히 바뀌면 안 된다 — 키가 없으면 레거시 값으로 폴백한다.
    #[test]
    fn legacy_configured_roles_still_open_every_key() {
        let mut settings = RemoteGuildSettings::default();
        settings.configured_role_ids = vec![777];
        settings.search_rule = PermissionRule::ConfiguredRole;
        settings.queue_edit_rule = PermissionRule::ConfiguredRole;
        let member = member_with_roles(vec![777]);
        for key in ["search", "queueEdit"] {
            let rule = settings.rule_for(key).unwrap();
            assert!(
                permission_allowed(key, rule, &settings, &member),
                "{key} 폴백 실패"
            );
        }
    }

    /// 빈 배열은 "일부러 비웠다"는 뜻이라 레거시로 되살아나면 안 된다.
    #[test]
    fn empty_role_list_does_not_fall_back_to_legacy() {
        let mut settings = RemoteGuildSettings::default();
        settings.configured_role_ids = vec![777];
        settings.search_rule = PermissionRule::ConfiguredRole;
        settings.rule_role_ids.insert("search".into(), Vec::new());
        let member = member_with_roles(vec![777]);
        assert!(!permission_allowed("search", settings.search_rule, &settings, &member));
    }

    /// 자동 재생 권한도 같은 판정 경로를 탄다.
    /// **기본은 모든 사람**이다 (V3 §8.3 — "일반사용자도 할수있고").
    #[test]
    fn autoplay_rule_defaults_to_every_member() {
        let mut settings = RemoteGuildSettings::default();
        assert_eq!(settings.autoplay_rule, PermissionRule::GuildMember);
        let member = MemberContext::default();
        assert!(permission_allowed(
            "autoplay",
            settings.autoplay_rule,
            &settings,
            &member
        ));

        // 필요하면 관리자가 조일 수 있다.
        settings.autoplay_rule = PermissionRule::Administrator;
        assert!(!permission_allowed(
            "autoplay",
            settings.autoplay_rule,
            &settings,
            &member
        ));
        let admin = MemberContext {
            is_admin: true,
            ..Default::default()
        };
        assert!(permission_allowed(
            "autoplay",
            settings.autoplay_rule,
            &settings,
            &admin
        ));
    }

    // ── V3 §2: 개인 설정 화이트리스트 ──

    #[test]
    fn pref_patch_accepts_known_keys_and_coerces_scalars() {
        let body = json!({
            "layout": "panel",
            "theme": "light",
            "webVolume": 60,
            "lyricsOpen": true,
            "webPlayback": false,
        });
        let (updates, removals) = parse_pref_patch(body.as_object().unwrap()).unwrap();
        assert!(removals.is_empty());
        assert_eq!(updates.get("layout").map(String::as_str), Some("panel"));
        assert_eq!(updates.get("theme").map(String::as_str), Some("light"));
        // 숫자·불리언도 받아 문자열로 저장한다.
        assert_eq!(updates.get("webVolume").map(String::as_str), Some("60"));
        assert_eq!(updates.get("lyricsOpen").map(String::as_str), Some("1"));
        assert_eq!(updates.get("webPlayback").map(String::as_str), Some("0"));
    }

    #[test]
    fn pref_patch_rejects_unknown_keys_and_out_of_range_values() {
        for body in [
            json!({ "adminPassword": "hunter2" }),
            json!({ "layout": "four" }),
            json!({ "theme": "solarized" }),
            json!({ "webVolume": 101 }),
            json!({ "layoutSizes": "그냥 문자열" }),
            json!({ "layout": ["three"] }),
        ] {
            assert!(
                parse_pref_patch(body.as_object().unwrap()).is_err(),
                "{body} 를 통과시켰다"
            );
        }
    }

    /// null은 "기본으로 되돌리기"다. 저장이 아니라 삭제로 간다.
    #[test]
    fn pref_patch_treats_null_as_removal() {
        let body = json!({ "layout": Value::Null, "theme": "dark" });
        let (updates, removals) = parse_pref_patch(body.as_object().unwrap()).unwrap();
        assert_eq!(removals, vec!["layout".to_string()]);
        assert_eq!(updates.get("theme").map(String::as_str), Some("dark"));
    }

    // ── V3 §4: 접속 표시 정확도 ──

    /// 봇이 있는 그 채널의 사람만 "듣는 중"이다. 나머지는 "다른 채널".
    #[test]
    fn listening_counts_only_the_bot_channel() {
        let members = [(11, Some(500)), (12, Some(500)), (13, Some(900)), (14, None)];
        let (listening, other) = split_voice_members(Some(500), &members);
        assert_eq!(listening, vec!["11".to_string(), "12".to_string()]);
        assert_eq!(other, vec!["13".to_string()]);
    }

    /// 봇이 음성에 없으면 듣는 사람도 없다 — 음성에 있는 사람은 전부 "다른 채널"이다.
    #[test]
    fn listening_is_empty_when_the_bot_is_not_in_voice() {
        let members = [(11, Some(500)), (12, Some(900))];
        let (listening, other) = split_voice_members(None, &members);
        assert!(listening.is_empty());
        assert_eq!(other, vec!["11".to_string(), "12".to_string()]);
    }

    // ── V3 §5: 대기열 갱신 카운트다운 ──

    /// 카운트다운 기준은 `app.rs`의 재정렬 주기와 같은 값이어야 한다.
    /// 상수가 갈라지면 화면의 숫자가 실제 재정렬과 어긋난다.
    ///
    /// **길이에 따라 달라진다** (§18.2 (3)) — 500곡을 넘으면 5초가 아니라 15초다.
    #[test]
    fn countdown_period_follows_the_sort_loop() {
        assert_eq!(crate::app::queue_sort_interval_for_len(0).as_secs(), 5);
        assert_eq!(crate::app::queue_sort_interval_for_len(500).as_secs(), 5);
        assert_eq!(crate::app::queue_sort_interval_for_len(501).as_secs(), 15);
    }

    // ── V3 §6: 브라우저 검색 ──

    #[test]
    fn youtube_api_key_is_kept_when_the_form_field_is_blank() {
        let base = RemoteAuthConfig {
            client_id: None,
            client_secret: None,
            public_base_url: "https://music.example.com".into(),
            dev_login: false,
            owner_user_ids: Vec::new(),
            youtube_api_key: Some("AIzaKEEPME0123456789".into()),
        };
        // 빈 값으로 저장 → 기존 키 유지 (Client Secret과 같은 규칙)
        assert_eq!(
            base.with_youtube_api_key(None, false).youtube_api_key(),
            Some("AIzaKEEPME0123456789")
        );
        assert_eq!(
            base.with_youtube_api_key(Some("   ".into()), false).youtube_api_key(),
            Some("AIzaKEEPME0123456789")
        );
        // 새 값은 교체, 제거 체크박스는 삭제
        assert_eq!(
            base.with_youtube_api_key(Some("AIzaNEW9876543210".into()), false)
                .youtube_api_key(),
            Some("AIzaNEW9876543210")
        );
        assert_eq!(base.with_youtube_api_key(None, true).youtube_api_key(), None);
        // 마스킹은 앞뒤 4자만 남긴다 — 전체 키가 운영 화면에 그대로 찍히지 않는다.
        let masked = base.masked_youtube_api_key().unwrap();
        assert!(masked.starts_with("AIza") && masked.ends_with("6789"));
        assert!(!masked.contains("KEEPME"));
    }

    #[test]
    fn lrc_parser_supports_fractional_seconds() {
        let lines = parse_lrc("[00:17.12] first\n[03:20.310] second");
        assert_eq!(lines[0].start_ms, 17_120);
        assert_eq!(lines[1].start_ms, 200_310);
        assert_eq!(lines[1].text, "second");
    }

    /// S3: `Disabled`는 누구도 통과하지 못한다. 관리자도, 봇 주인도.
    #[test]
    fn disabled_rule_blocks_everyone_including_admins() {
        let settings = settings_with(PermissionRule::Disabled);
        for is_admin in [false, true] {
            let member = MemberContext {
                is_admin,
                same_voice_channel: true,
                bot_in_voice: true,
                role_ids: vec![777],
            };
            assert!(
                !permission_allowed("search", PermissionRule::Disabled, &settings, &member),
                "is_admin={is_admin} 인데 Disabled를 통과했다"
            );
        }
    }

    /// permission-preview가 같은 함수를 쓰므로 `rule=disabled`의 통과 인원은 반드시 0이다.
    #[test]
    fn disabled_preview_pass_count_is_zero() {
        let settings = settings_with(PermissionRule::Disabled);
        let members = [
            MemberContext { is_admin: true, same_voice_channel: true, bot_in_voice: true, role_ids: vec![777] },
            MemberContext { is_admin: false, same_voice_channel: true, bot_in_voice: true, role_ids: vec![777] },
            MemberContext::default(),
        ];
        let passed = members
            .iter()
            .filter(|member| permission_allowed("search", PermissionRule::Disabled, &settings, member))
            .count();
        assert_eq!(passed, 0);
    }

    #[test]
    fn admin_bypasses_every_rule_except_disabled() {
        let settings = RemoteGuildSettings::default();
        let admin = MemberContext {
            is_admin: true,
            same_voice_channel: false,
            bot_in_voice: true,
            role_ids: Vec::new(),
        };
        for rule in [
            PermissionRule::GuildMember,
            PermissionRule::SameVoiceChannel,
            PermissionRule::ConfiguredRole,
            PermissionRule::Administrator,
        ] {
            assert!(permission_allowed("search", rule, &settings, &admin), "{rule:?}");
        }
        assert!(!permission_allowed(
            "search",
            PermissionRule::Disabled,
            &settings,
            &admin
        ));
    }

    /// 웹 재생기 모드는 **봇이 아예 없을 때만** 권한을 연다.
    ///
    /// 봇이 다른 채널에서 틀고 있으면 그대로 막아야 한다 — 그때는 방해받을 사람이 실제로 있다.
    /// 그리고 모드가 꺼져 있으면 이 기능 도입 전과 **완전히 같아야** 한다(R1).
    #[test]
    fn web_player_mode_only_opens_when_the_bot_is_absent() {
        let outsider = MemberContext {
            same_voice_channel: false,
            bot_in_voice: false,
            ..Default::default()
        };
        let bot_elsewhere = MemberContext {
            same_voice_channel: false,
            bot_in_voice: true,
            ..Default::default()
        };

        // 모드 Off — 도입 전과 같다.
        let off = RemoteGuildSettings::default();
        assert!(off.require_voice_for_playback);
        assert!(!off.web_player_mode);
        assert!(!same_voice_satisfied(&off, &outsider), "모드 Off 면 예전 그대로 막힌다");
        assert!(!same_voice_satisfied(&off, &bot_elsewhere));

        // 모드 On — 봇이 없을 때만 열린다.
        let mut on = RemoteGuildSettings::default();
        on.web_player_mode = true;
        assert!(
            same_voice_satisfied(&on, &outsider),
            "웹 재생기 모드면 봇이 없어도 조작할 시각표가 있다"
        );
        assert!(
            !same_voice_satisfied(&on, &bot_elsewhere),
            "봇이 남의 채널에서 틀고 있으면 밖에서 흔들면 안 된다"
        );

        // 같은 채널이면 어느 설정에서도 열린다 — 기존 계약.
        let together = MemberContext {
            same_voice_channel: true,
            bot_in_voice: true,
            ..Default::default()
        };
        assert!(same_voice_satisfied(&off, &together));
        assert!(same_voice_satisfied(&on, &together));
    }

    /// `봇이 음성 채널에 있어야만 조작` 을 끄면 **실제로 조작이 풀려야 한다.**
    ///
    /// 규칙의 목적은 같이 듣는 사람의 재생을 남이 흔들지 못하게 하는 것인데, 봇이 음성에
    /// 아예 없으면 흔들 재생도 방해받을 사람도 없다. 그때까지 막으면 설정만 있고 효과가 없다.
    /// 반대로 요구가 켜져 있으면 예전 그대로 막아야 한다.
    #[test]
    fn turning_off_the_voice_requirement_actually_opens_the_controls() {
        let mut settings = RemoteGuildSettings::default();
        let outsider = MemberContext {
            same_voice_channel: false,
            bot_in_voice: false,
            ..Default::default()
        };

        // 요구가 켜져 있으면(기본) 봇이 없을 때 조작도 없다.
        assert!(settings.require_voice_for_playback);
        assert!(!permission_allowed("playback", settings.playback_rule, &settings, &outsider));
        assert!(!permission_allowed("skip", settings.skip_rule, &settings, &outsider));

        // 껐으면 봇이 음성에 없을 때 열린다.
        settings.require_voice_for_playback = false;
        for (key, rule) in [
            ("playback", settings.playback_rule),
            ("seek", settings.seek_rule),
            ("skip", settings.skip_rule),
            ("volume", settings.volume_rule),
        ] {
            assert!(
                permission_allowed(key, rule, &settings, &outsider),
                "{key} 는 봇이 음성에 없고 요구를 껐으면 열려야 한다"
            );
        }

        // **봇이 다른 채널에 들어가 있으면 얘기가 다르다.** 그때는 듣는 사람이 실제로 있다.
        let bot_elsewhere = MemberContext {
            same_voice_channel: false,
            bot_in_voice: true,
            ..Default::default()
        };
        assert!(
            !permission_allowed("skip", settings.skip_rule, &settings, &bot_elsewhere),
            "봇이 남의 채널에서 틀고 있으면 밖에서 넘기면 안 된다"
        );
    }

    /// 제안 #3 — 재생 중인 곡이 없을 때 자동 재생을 못 켜던 문제.
    /// 자동 재생은 저장되는 설정이라 봇이 음성에 없어도 켜져야 한다.
    #[test]
    fn only_autoplay_escapes_the_voice_requirement() {
        assert!(!action_requires_voice("autoplay"));
        for action in ["pause", "resume", "seek", "skip", "volume", "shuffle", "repeat"] {
            assert!(
                action_requires_voice(action),
                "{action} 은 봇이 음성에 있어야 하는 조작이다"
            );
        }
    }

    #[test]
    fn permission_defaults_match_remote_contract() {
        let settings = RemoteGuildSettings::default();
        let member = MemberContext::default();
        // 음성에 없는 사람: 소리를 흔드는 것은 전부 막힌다.
        for (key, rule) in [
            ("playback", settings.playback_rule),
            ("seek", settings.seek_rule),
            ("volume", settings.volume_rule),
            ("skip", settings.skip_rule),
            ("bulkEnqueue", settings.bulk_enqueue_rule),
        ] {
            assert!(
                !permission_allowed(key, rule, &settings, &member),
                "{key} 는 음성 밖에서 막혀야 한다"
            );
        }
        // 반대로 신청·좋아요·채팅·자동 재생은 음성에 없어도 된다.
        for (key, rule) in [
            ("search", settings.search_rule),
            ("vote", settings.vote_rule),
            ("chat", settings.chat_rule),
            ("autoplay", settings.autoplay_rule),
        ] {
            assert!(
                permission_allowed(key, rule, &settings, &member),
                "{key} 는 음성 밖에서도 돼야 한다"
            );
        }
        let same_voice = MemberContext {
            same_voice_channel: true,
            bot_in_voice: true,
            ..Default::default()
        };
        // 음성에 들어오면 막혔던 것들이 전부 열린다.
        for (key, rule) in [
            ("playback", settings.playback_rule),
            ("seek", settings.seek_rule),
            ("skip", settings.skip_rule),
            ("bulkEnqueue", settings.bulk_enqueue_rule),
        ] {
            assert!(
                permission_allowed(key, rule, &settings, &same_voice),
                "{key} 는 같은 음성 채널이면 열려야 한다"
            );
        }
    }

    /// 관리자 우회로 통과한 항목은 "← 관리자라 통과"로 표시돼야 한다.
    #[test]
    fn via_admin_is_detected_by_base_rule() {
        let settings = RemoteGuildSettings::default();
        let admin_outside_voice = MemberContext {
            is_admin: true,
            same_voice_channel: false,
            bot_in_voice: true,
            role_ids: Vec::new(),
        };
        assert!(permission_allowed(
            "playback",
            PermissionRule::SameVoiceChannel,
            &settings,
            &admin_outside_voice
        ));
        assert!(!rule_base_allowed(
            "playback",
            PermissionRule::SameVoiceChannel,
            &settings,
            &admin_outside_voice
        ));
    }

    #[test]
    fn access_tier_ordering_matches_privilege() {
        assert!(AccessTier::Owner > AccessTier::Manager);
        assert!(AccessTier::Manager > AccessTier::Member);
        assert!(AccessTier::Member > AccessTier::Viewer);
        assert!(AccessTier::Manager.is_manager());
        assert!(!AccessTier::Member.is_manager());
        assert!(AccessTier::Viewer.is_viewer());
        assert_eq!(AccessTier::Owner.as_str(), "owner");
    }

    #[test]
    fn viewer_cannot_write_even_when_rule_is_permissive() {
        // AuthContext::allows 의 규칙: Viewer면 규칙과 무관하게 false.
        let settings = RemoteGuildSettings::default();
        let member = MemberContext {
            is_admin: false,
            same_voice_channel: true,
            bot_in_voice: true,
            role_ids: Vec::new(),
        };
        // 규칙 자체는 통과한다.
        assert!(permission_allowed(
            "chat",
            PermissionRule::GuildMember,
            &settings,
            &member
        ));
        // 등급이 Viewer면 최종 판정은 거부다.
        let viewer_allows = !AccessTier::Viewer.is_viewer()
            && permission_allowed("chat", PermissionRule::GuildMember, &settings, &member);
        assert!(!viewer_allows);
    }

    #[test]
    fn mention_parser_matches_longest_name_with_spaces() {
        let candidates = vec![
            "민수".to_string(),
            "민수 형".to_string(),
            "지훈".to_string(),
        ];
        let hits = match_prefixed("@민수 형 이거 들어봐 @지훈", '@', &candidates);
        // "민수 형"이 "민수"보다 길므로 최장일치가 이긴다.
        assert_eq!(hits, vec![1, 2]);
    }

    #[test]
    fn mention_parser_is_case_insensitive_and_dedupes() {
        let candidates = vec!["MinSu".to_string()];
        let hits = match_prefixed("@minsu @MINSU @minsu", '@', &candidates);
        assert_eq!(hits, vec![0]);
    }

    #[test]
    fn song_tag_parser_handles_titles_with_spaces() {
        let titles = vec![
            "City".to_string(),
            "City Lights".to_string(),
            "Soft Focus".to_string(),
        ];
        let hits = match_prefixed("#City Lights 좋다 #Soft Focus 도", '#', &titles);
        assert_eq!(hits, vec![1, 2]);
    }

    #[test]
    fn parser_ignores_unknown_tokens() {
        let candidates = vec!["민수".to_string()];
        assert!(match_prefixed("@없는사람 안녕", '@', &candidates).is_empty());
        assert!(match_prefixed("이메일 a@b.com", '@', &candidates).is_empty());
    }

    #[test]
    fn constant_time_compare_still_compares_correctly() {
        assert!(constant_time_eq("abcdef", "abcdef"));
        assert!(!constant_time_eq("abcdef", "abcdeg"));
        assert!(!constant_time_eq("abc", "abcdef"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn owner_ids_are_parsed_and_deduped() {
        assert_eq!(parse_owner_ids("1, 2 ,2,0,x,3"), vec![1, 2, 3]);
        assert!(parse_owner_ids("").is_empty());
    }

    #[test]
    fn repeat_mode_is_camel_case_lowercase() {
        assert_eq!(repeat_key(RepeatMode::Off), "off");
        assert_eq!(repeat_key(RepeatMode::Track), "track");
        assert_eq!(repeat_key(RepeatMode::Queue), "queue");
        assert_eq!(parse_repeat("queue"), Some(RepeatMode::Queue));
        assert_eq!(parse_repeat("Queue"), None);
    }

    #[test]
    fn every_track_carries_duration_seconds() {
        let track = TrackRef {
            provider: ProviderKind::YouTube,
            content_id: "abc".into(),
            source_url: "https://example.test/abc".into(),
            title: Some("제목".into()),
            artist: None,
            duration: Some(CsTimeSpan::from_secs_f64(245.0)),
            variant_key: None,
        };
        let value = track_json(&track);
        assert_eq!(value["durationSeconds"], json!(245.0));
        assert_eq!(value["cacheKey"], json!(track.cache_key()));
    }

    #[test]
    fn events_serialize_as_typed_frames() {
        let event = RemoteEvent {
            guild_id: 1,
            topic: "chat.add".into(),
            data: json!({ "id": 7 }),
            only_user: None,
        };
        let wire: Value = serde_json::from_str(&event.wire()).unwrap();
        assert_eq!(wire["t"], json!("chat.add"));
        assert_eq!(wire["d"]["id"], json!(7));
        // 길드 id는 와이어에 실리지 않는다 — 서버가 필터링한다.
        assert!(wire.get("guildId").is_none());
    }

    #[test]
    fn origin_host_extraction_ignores_scheme_and_path() {
        assert_eq!(host_of("https://music.example.com/music"), "music.example.com");
        assert_eq!(host_of("http://localhost:8693"), "localhost:8693");
        assert_eq!(host_of(""), "");
    }

    #[test]
    fn cookie_secure_prefers_the_safe_side() {
        let mut auth = RemoteAuthConfig {
            client_id: None,
            client_secret: None,
            public_base_url: "https://music.example.com".into(),
            dev_login: false,
            owner_user_ids: Vec::new(),
            youtube_api_key: None,
        };
        assert!(cookie_should_be_secure(&auth, None));
        auth.public_base_url = "http://localhost:8693".into();
        assert!(!cookie_should_be_secure(&auth, None));
        // 프록시가 HTTPS를 종단해도 Secure가 붙어야 한다.
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(cookie_should_be_secure(&auth, Some(&headers)));
        // 정체를 모르는 도메인은 안전한 쪽(Secure)을 기본으로 한다.
        auth.public_base_url = "http://music.example.test".into();
        assert!(cookie_should_be_secure(&auth, None));
    }

    #[test]
    fn oauth_config_persists_and_redacts_the_secret() {
        let root = std::env::temp_dir().join(format!(
            "mc-musicbot-oauth-config-{}",
            crate::models::uuid_like()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = RemoteAuthConfig {
            client_id: Some("100000000000000001".into()),
            client_secret: Some("unit-test-secret-never-log".into()),
            public_base_url: "https://musicbot.example.test".into(),
            dev_login: false,
            owner_user_ids: vec![42, 43],
            youtube_api_key: Some("AIzaTESTKEY0123456789".into()),
        };
        config.save(&root).unwrap();
        let loaded = RemoteAuthConfig::load(&root);
        assert_eq!(loaded.client_id, config.client_id);
        assert_eq!(loaded.client_secret, config.client_secret);
        assert_eq!(loaded.public_base_url, config.public_base_url);
        assert_eq!(loaded.owner_user_ids, vec![42, 43]);
        assert!(!format!("{loaded:?}").contains("unit-test-secret-never-log"));

        let retained = loaded.updated(
            "100000000000000001".into(),
            None,
            false,
            "https://musicbot.example.test/".into(),
        );
        assert!(retained.has_client_secret());
        // 봇 주인 목록은 OAuth 값을 저장해도 살아남는다.
        assert_eq!(retained.owner_user_ids, vec![42, 43]);
        let cleared = retained.updated(
            "100000000000000001".into(),
            None,
            true,
            "https://musicbot.example.test".into(),
        );
        assert!(!cleared.has_client_secret());
        std::fs::remove_dir_all(&root).unwrap();
    }

    // ══════════════ V3 §16 B1 — 봇이 음성채널에 없는데 있다고 나오던 버그 ══════════════

    /// **회귀 테스트**: Discord 캐시에 봇 voice_state 가 없고 저장값(`player.voice_channel_id`)만
    /// 남아 있는 상태 → 봇은 음성에 **없다**.
    ///
    /// 예전 코드의 `.or(player_channel)` 이 정확히 이 상황에서 stale 값을 살려 냈다.
    /// 봇이 재시작·연결 끊김·강제 퇴장으로 빠져나가도 화면은 계속 들어가 있다고 말했다.
    #[test]
    fn stored_voice_channel_never_revives_a_bot_that_left() {
        // 캐시: 없음 / 저장값: 있음 → 결과는 없음이어야 한다.
        assert_eq!(authoritative_voice_channel(None, Some(123)), None);
        // 캐시가 말하면 그게 진실이다. 저장값이 달라도 캐시가 이긴다.
        assert_eq!(authoritative_voice_channel(Some(777), Some(123)), Some(777));
        assert_eq!(authoritative_voice_channel(Some(777), None), Some(777));

        // 그 결과가 `inVoice` 로 그대로 흘러간다.
        let stale = BotVoiceStatus {
            in_guild: true,
            channel_id: authoritative_voice_channel(None, Some(123)),
            channel_name: None,
        };
        assert!(!stale.in_voice(), "저장값만으로 inVoice 가 켜지면 B1 재발이다");
    }

    /// 봇이 음성에 없으면 `듣는 중`은 **언제나 빈 배열**이다 (§4).
    /// 봇 없는 방에서 나는 소리를 듣는 중이라고 부를 수는 없다.
    #[test]
    fn nobody_is_listening_when_the_bot_is_not_in_voice() {
        let members = vec![(10u64, Some(500u64)), (11, Some(500)), (12, None)];
        let (listening, other) = split_voice_members(None, &members);
        assert!(listening.is_empty());
        assert_eq!(other, vec!["10".to_string(), "11".to_string()]);

        // 봇이 500번 방에 있으면 그 방 사람만 듣는 중이다.
        let members = vec![(10u64, Some(500u64)), (11, Some(600))];
        let (listening, other) = split_voice_members(Some(500), &members);
        assert_eq!(listening, vec!["10".to_string()]);
        assert_eq!(other, vec!["11".to_string()]);
    }

    // ══════════════ V3 §10.5 — 투표 스킵 정족수 ══════════════

    fn ids(values: &[u64]) -> HashSet<u64> {
        values.iter().copied().collect()
    }

    /// 모수가 1명이면 그 사람 혼자 눌러도 넘어간다 —
    /// 혼자 듣는데 투표를 시키면 그냥 괴롭힘이다.
    #[test]
    fn a_single_listener_can_always_skip() {
        assert_eq!(VoteSkipBasis::votes_needed(1, 50, 2), 1);
        let quorum = skip_quorum(
            &ids(&[10]),
            &HashSet::new(),
            &ids(&[10]),
            VoteSkipBasis::Listeners,
            50,
            2,
        );
        assert!(quorum.passed);
        assert_eq!((quorum.have, quorum.need), (1, 1));
    }

    /// 3명 중 50% → 2표. 1표로는 안 넘어가고 2표에서 넘어간다.
    #[test]
    fn listener_quorum_needs_a_majority() {
        assert_eq!(VoteSkipBasis::votes_needed(3, 50, 2), 2);
        let listeners = ids(&[10, 11, 12]);
        let one = skip_quorum(
            &listeners,
            &HashSet::new(),
            &ids(&[10]),
            VoteSkipBasis::Listeners,
            50,
            2,
        );
        assert!(!one.passed);
        assert_eq!((one.have, one.need), (1, 2));

        let two = skip_quorum(
            &listeners,
            &HashSet::new(),
            &ids(&[10, 11]),
            VoteSkipBasis::Listeners,
            50,
            2,
        );
        assert!(two.passed);
    }

    /// 듣지 않는 사람의 표는 `listeners` 기준에서 안 센다.
    #[test]
    fn viewers_do_not_count_towards_the_listener_basis() {
        let quorum = skip_quorum(
            &ids(&[10, 11]),
            &ids(&[90, 91]),
            &ids(&[90, 91]),
            VoteSkipBasis::Listeners,
            50,
            1,
        );
        assert_eq!(quorum.have, 0);
        assert!(!quorum.passed);
    }

    /// `either` 는 한쪽만 넘어도 통과하고, `both` 는 둘 다 넘어야 통과한다.
    #[test]
    fn either_and_both_bases_behave_differently() {
        let listeners = ids(&[10, 11]);
        let viewers = ids(&[90, 91, 92, 93]);
        let voters = ids(&[10, 11]); // 듣는 사람은 전원, 보는 사람은 0표

        let either = skip_quorum(&listeners, &viewers, &voters, VoteSkipBasis::Either, 50, 1);
        assert!(either.passed, "한쪽만 넘어도 통과해야 한다");

        let both = skip_quorum(&listeners, &viewers, &voters, VoteSkipBasis::Both, 50, 1);
        assert!(!both.passed, "보는 사람 쪽이 모자라면 통과하면 안 된다");
    }

    /// 모수가 0명이면 필요 표도 0이고 정족수로는 절대 통과하지 않는다.
    /// (그 상황은 호출부에서 **즉시 스킵**으로 따로 처리한다 — 투표가 무의미하니까.)
    #[test]
    fn an_empty_room_never_passes_by_vote() {
        assert_eq!(VoteSkipBasis::votes_needed(0, 50, 2), 0);
        let quorum = skip_quorum(
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            VoteSkipBasis::Listeners,
            50,
            2,
        );
        assert!(!quorum.passed);
    }

    /// 최소 표 수가 모수보다 크면 모수가 곧 필요 표 수다 — 안 그러면 영원히 안 넘어간다.
    #[test]
    fn minimum_votes_never_exceed_the_population() {
        assert_eq!(VoteSkipBasis::votes_needed(2, 50, 20), 2);
    }

    // ══════════════ V3 §10.3 — 붐따 임계값 ══════════════

    fn score_with_dislikes(dislikes: i32) -> QueueScore {
        QueueScore {
            dislike_count: dislikes,
            ..Default::default()
        }
    }

    /// 꺼져 있으면(기본) 싫어요가 아무리 모여도 곡이 사라지지 않는다.
    #[test]
    fn boomtta_stays_off_by_default() {
        let settings = RemoteGuildSettings::default();
        assert!(!settings.boomtta_enabled);
        assert!(!score_with_dislikes(99).boomtta_triggered(&settings));
    }

    /// 켜면 기준 수에 **닿는 순간** 터진다. 하나 모자라면 안 터진다.
    #[test]
    fn boomtta_fires_exactly_at_the_threshold() {
        let settings = RemoteGuildSettings {
            boomtta_enabled: true,
            boomtta_threshold: 3,
            ..Default::default()
        };
        assert!(!score_with_dislikes(2).boomtta_triggered(&settings));
        assert!(score_with_dislikes(3).boomtta_triggered(&settings));
        assert!(score_with_dislikes(4).boomtta_triggered(&settings));
    }

    /// 기준 수 `0` 은 무제한이라 절대 안 터진다 (§23.1).
    #[test]
    fn boomtta_threshold_zero_means_never() {
        let settings = RemoteGuildSettings {
            boomtta_enabled: true,
            boomtta_threshold: 0,
            ..Default::default()
        };
        assert!(!score_with_dislikes(1000).boomtta_triggered(&settings));
    }

    // ══════════════ V3 §23.1 — 무제한(0) ══════════════

    /// **`0` 은 무제한이다.** 예전 `.max(1)` 클램프는 `0` 을 `1` 로 바꿔서
    /// 화면에는 "무제한"이 뜨는데 서버는 한 곡만 받는 최악의 조합을 만들었다.
    #[test]
    fn zero_limits_block_nothing() {
        assert!(!limit_blocks(0, 1));
        assert!(!limit_blocks(0, 10_000));
        assert!(!limit_blocks(-1, 10_000));
    }

    /// 양수 상한은 정확히 그 수까지만 받는다.
    #[test]
    fn positive_limits_stop_at_the_configured_number() {
        assert!(!limit_blocks(5, 5), "5번째 곡은 들어가야 한다");
        assert!(limit_blocks(5, 6), "6번째 곡은 막혀야 한다");
    }

    /// 길이 제한 `0` 이면 아무리 긴 곡도 담을 수 있다.
    #[test]
    fn zero_track_length_limit_allows_anything() {
        let long = TrackRef {
            provider: ProviderKind::YouTube,
            content_id: "x".into(),
            source_url: "https://example.test/x".into(),
            title: Some("10시간 백색소음".into()),
            artist: None,
            duration: Some(CsTimeSpan::from_secs_f64(36_000.0)),
            variant_key: None,
        };
        assert!(!track_too_long(0, &long));
        assert!(track_too_long(3600, &long));
        assert!(!track_too_long(40_000, &long));
    }

    /// 설정 검증도 `0` 을 통과시켜야 한다. 여기가 막혀 있으면
    /// 화면에서 아무리 `∞` 로 밀어도 저장이 안 된다.
    /// §18.1 새 상한(1인 1000곡 / 서버 10000곡)도 같이 못 박는다.
    #[test]
    fn settings_validation_accepts_unlimited_and_the_new_maxima() {
        assert!(unlimited_or(0, 1, 1_000));
        assert!(unlimited_or(1_000, 1, 1_000));
        assert!(!unlimited_or(1_001, 1, 1_000));
        assert!(unlimited_or(10_000, 1, 10_000));
        assert!(!unlimited_or(10_001, 1, 10_000));
    }

    // ══════════════ V3 §10.1 — 점수는 설정값으로 계산한다 ══════════════

    /// 좋아요를 2점으로 바꾸면 총점도 화면 계산식도 같이 바뀐다.
    /// 서버와 화면이 다른 숫자로 더하면 화면이 거짓말을 하게 된다.
    #[test]
    fn total_score_follows_the_configured_points() {
        let score = QueueScore {
            wait_score: 2,
            like_count: 3,
            super_like_count: 1,
            dislike_count: 0,
            ..Default::default()
        };
        let default_points = VotePoints::default();
        assert_eq!(score.total_score(&default_points), 2 + 3 + 2);

        let doubled = VotePoints {
            like: 2,
            ..VotePoints::default()
        };
        assert_eq!(score.total_score(&doubled), 2 + 6 + 2);
    }

    // ══════════════ V3 §13.5 — 활동 로그 분류 필터 ══════════════

    #[test]
    fn audit_kind_filter_parses_only_known_kinds() {
        assert_eq!(
            parse_audit_kinds(Some("song,playlist")),
            vec![AuditKind::Song, AuditKind::Playlist]
        );
        // 모르는 값은 조용히 버린다 — 필터 하나 오타로 전체가 500이 되면 안 된다.
        assert_eq!(parse_audit_kinds(Some("song,nope")), vec![AuditKind::Song]);
        assert!(parse_audit_kinds(None).is_empty());
        assert!(parse_audit_kinds(Some("")).is_empty());
    }

    /// 기본 필터는 조용해야 쓸모가 있다 — 곡과 재생목록만 켠다 (§13.4).
    #[test]
    fn audit_default_filter_is_quiet() {
        assert_eq!(
            AuditKind::default_filter(),
            [AuditKind::Song, AuditKind::Playlist]
        );
    }

    // ══════════════ V3 §10.2 — 투표 종류 ══════════════

    #[test]
    fn vote_kinds_round_trip_through_the_api_key() {
        for kind in [
            QueueVoteKind::Like,
            QueueVoteKind::SuperLike,
            QueueVoteKind::Dislike,
        ] {
            assert_eq!(parse_vote_kind(kind.api_key()), Some(kind));
        }
        assert_eq!(parse_vote_kind("nope"), None);
    }

    // ══════════════ V3 §10.5 — 스킵 투표가 남의 표를 내 표로 만들지 않는다 ══════════════

    fn voter_ids(values: &[u64]) -> HashSet<u64> {
        values.iter().copied().collect()
    }

    /// **회귀 방지**: `mine` 은 사람마다 다른 값이다.
    ///
    /// 예전에는 누른 사람 기준의 `mine` 을 길드 전체에 뿌려서, A가 ⏭를 누르면
    /// B·C 화면도 "내 표가 들어가 있어요"가 됐다. 그 상태에서 B가 누르면 취소가 나가
    /// A의 표가 빠지고, 정족수에 영영 도달하지 못하는 교착이 됐다.
    #[test]
    fn skip_vote_frames_are_personalised_per_recipient() {
        let (sender, mut receiver) = tokio::sync::broadcast::channel(64);
        let base = json!({ "have": 1, "need": 2, "pool": 3 });
        let voters = voter_ids(&[11]);
        // `emit_skip_vote` 와 같은 규칙 (WebState 없이 채널만 확인).
        let mut shared = base.clone();
        shared["mine"] = Value::Bool(false);
        let _ = sender.send(RemoteEvent {
            guild_id: 7,
            topic: "skipvote".into(),
            data: shared,
            only_user: None,
        });
        for voter in &voters {
            let mut personal = base.clone();
            personal["mine"] = Value::Bool(true);
            let _ = sender.send(RemoteEvent {
                guild_id: 7,
                topic: "skipvote".into(),
                data: personal,
                only_user: Some(*voter),
            });
        }

        let broadcast = receiver.try_recv().unwrap();
        // 브로드캐스트 프레임은 **누구에게도** `mine:true` 를 말하지 않는다.
        assert_eq!(broadcast.data["mine"], json!(false));
        assert!(broadcast.targets(7, 11));
        assert!(broadcast.targets(7, 22));

        let personal = receiver.try_recv().unwrap();
        assert_eq!(personal.data["mine"], json!(true));
        // 투표한 사람에게만 간다.
        assert!(personal.targets(7, 11));
        assert!(!personal.targets(7, 22));
        // 길드가 다르면 아무에게도 안 간다.
        assert!(!personal.targets(8, 11));
    }

    /// 개인화 이벤트는 다른 사람 소켓을 통과하지 못한다 (`library` 도 같은 규칙).
    #[test]
    fn targeted_events_never_leak_to_other_sockets() {
        let event = RemoteEvent {
            guild_id: 1,
            topic: "library".into(),
            data: json!({}),
            only_user: Some(42),
        };
        assert!(event.targets(1, 42));
        assert!(!event.targets(1, 43));
        let broadcast = RemoteEvent {
            guild_id: 1,
            topic: "settings".into(),
            data: json!({}),
            only_user: None,
        };
        assert!(broadcast.targets(1, 42));
        assert!(broadcast.targets(1, 43));
    }

    /// **회귀 방지**: 툴팁이 쓰는 모수(`pool`)는 `need` 가 아니라 실제 인원이다.
    /// 이게 없으면 `듣는 사람 5명 중 3명 필요` 가 `3명 중 3명` 으로 표시된다.
    #[test]
    fn skip_quorum_reports_the_real_population() {
        let listeners = voter_ids(&[1, 2, 3, 4, 5]);
        let viewers = voter_ids(&[1, 2]);
        let voters = voter_ids(&[1]);
        let quorum = skip_quorum(&listeners, &viewers, &voters, VoteSkipBasis::Listeners, 50, 1);
        assert_eq!(quorum.pool, 5);
        assert_eq!(quorum.need, 3);
        assert_eq!(quorum.have, 1);
        assert!(!quorum.passed);

        let viewers_only =
            skip_quorum(&listeners, &viewers, &voters, VoteSkipBasis::Viewers, 50, 1);
        assert_eq!(viewers_only.pool, 2);
    }

    // ══════════════ V3 §18.2(5) — 대기열 비우기 ══════════════

    /// **회귀 방지**: `{action:"clear"}` 는 대상 항목이 없다.
    /// `item_id` 가 필수면 본문 역직렬화 단계에서 422 로 떨어져 이유조차 못 알려 준다.
    #[test]
    fn queue_clear_request_needs_no_item_id() {
        let request: QueueActionRequest = serde_json::from_value(json!({ "action": "clear" }))
            .expect("clear 요청은 itemId 없이도 파싱돼야 한다");
        assert_eq!(request.action, "clear");
        assert!(request.item_id.is_none());
        let remove: QueueActionRequest =
            serde_json::from_value(json!({ "action": "remove", "itemId": "abc" })).unwrap();
        assert_eq!(remove.item_id.as_deref(), Some("abc"));
    }

    // ══════════════ V3 §12.2 — 재생목록에서 곡 빼기 ══════════════

    /// **회귀 방지**: 화면이 보내는 이름은 `removeTrack` + `entryId` + `cacheKey` 다.
    /// 서버가 `removeEntry` + `entryIndex` 만 알면 `✕` 가 언제나 400 이 된다.
    #[test]
    fn playlist_remove_accepts_the_names_the_screen_sends() {
        let request: PlaylistActionRequest = serde_json::from_value(json!({
            "action": "removeTrack",
            "playlistId": 3,
            "entryId": 2,
            "cacheKey": "youtube:abc",
        }))
        .expect("removeTrack 요청이 파싱돼야 한다");
        assert_eq!(request.action, "removeTrack");
        assert_eq!(request.entry_id, Some(2));
        assert_eq!(request.cache_key.as_deref(), Some("youtube:abc"));
        let legacy: PlaylistActionRequest = serde_json::from_value(json!({
            "action": "removeEntry",
            "playlistId": 3,
            "entryIndex": 1,
        }))
        .unwrap();
        assert_eq!(legacy.entry_index, Some(1));
    }

    /// **회귀 방지**: `＋ 새로 만들어서 담기` 는 `track` 을 같이 보낸다.
    /// 서버가 그걸 안 보면 0곡짜리 재생목록을 만들고도 성공 토스트가 뜬다.
    #[test]
    fn playlist_create_carries_the_track_to_add() {
        let request: PlaylistActionRequest = serde_json::from_value(json!({
            "action": "create",
            "name": "밤샘용",
            "scope": "user",
            "track": {
                "provider": "YouTube",
                "contentId": "abc",
                "sourceUrl": "https://youtu.be/abc",
                "title": "테스트",
            },
        }))
        .expect("create 요청이 track 과 함께 파싱돼야 한다");
        assert!(
            request.track.is_some(),
            "track 을 버리면 0곡짜리가 만들어진다"
        );
    }

    // ══════════════ V3 §15.2b — 차트 기간 ══════════════

    /// **회귀 방지**: 화면은 `?period=` 를 보내고 예전 서버는 `window` 만 읽었다.
    /// 둘 다 받아야 `이번 주`/`전체` 버튼이 실제로 순위를 바꾼다.
    #[test]
    fn chart_window_accepts_the_period_alias() {
        let by_period: ChartWindowQuery =
            serde_json::from_value(json!({ "period": "week" })).unwrap();
        assert_eq!(by_period.resolve(), crate::stats::ChartWindow::Week);

        let by_window: ChartWindowQuery =
            serde_json::from_value(json!({ "window": "all" })).unwrap();
        assert_eq!(by_window.resolve(), crate::stats::ChartWindow::All);

        let empty: ChartWindowQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty.resolve(), crate::stats::ChartWindow::Month);

        // `window` 가 우선이지만, 빈 문자열이면 `period` 로 내려간다.
        let both: ChartWindowQuery =
            serde_json::from_value(json!({ "window": "", "period": "week" })).unwrap();
        assert_eq!(both.resolve(), crate::stats::ChartWindow::Week);

        // 캐시 키가 기간마다 갈려야 `이번 주` 를 눌렀는데 이번 달이 나오지 않는다.
        assert_ne!(
            chart_window_key(crate::stats::ChartWindow::Week),
            chart_window_key(crate::stats::ChartWindow::Month)
        );
    }

    // ══════════════ V3 §10.1 — 대기열 미리보기가 보낸 점수를 쓴다 ══════════════

    /// **회귀 방지**: 콘솔이 슬라이더로 보낸 점수를 서버가 그대로 써야 한다.
    /// 예전에는 `mode` 만 파싱해 미리보기가 늘 저장값으로 계산됐다(serde 가 조용히 버림).
    #[test]
    fn queue_preview_uses_the_points_the_console_sent() {
        let query: ModeQuery = serde_json::from_value(json!({
            "mode": "score",
            "likePoints": 10,
            "waitPoints": 0,
        }))
        .unwrap();
        let base = VotePoints {
            like: 1,
            dislike: -1,
            super_like: 2,
            wait: 1,
        };
        let merged = query.points_over(base);
        assert_eq!(merged.like, 10, "보낸 값이 반영돼야 한다");
        assert_eq!(merged.wait, 0);
        // 안 보낸 항목은 저장값 그대로다.
        assert_eq!(merged.dislike, -1);
        assert_eq!(merged.super_like, 2);

        // 범위를 벗어난 값은 저장 경로와 같은 규칙으로 잘린다.
        let wild: ModeQuery = serde_json::from_value(json!({ "likePoints": 999 })).unwrap();
        assert_eq!(wild.points_over(base).like, VOTE_POINT_MAX);
    }

    // ══════════════ V3 §13.3 — 활동 로그 문장 ══════════════

    /// **회귀 방지**: 핸들러가 쓰는 액션명이 `audit_text` 의 이름과 달라지면
    /// 사람 피드에 `민수님이 queue.force_move 을 했어요` 같은 기계 문자열이 나간다.
    #[test]
    fn audit_actions_used_by_handlers_have_human_sentences() {
        for action in [
            "queue.pin",
            "queue.clear",
            "autoplay.toggle",
            "playback.skip.vote",
        ] {
            let text = crate::remote::audit_text(
                action,
                "민수",
                Some("아이브 - I AM"),
                None,
                Some("on"),
                3,
            );
            assert!(
                !text.contains(action),
                "{action} 의 문장이 없어서 기계 액션명이 그대로 나가요: {text}"
            );
        }
    }

    /// **회귀 방지**: 볼륨 로그의 `after` 에 `volume:` 접두사가 섞이면
    /// `서버 볼륨을 volume:150으로 바꿨어요` 가 그대로 사람 피드에 나간다.
    #[test]
    fn volume_audit_value_has_no_machine_prefix() {
        let text =
            crate::remote::audit_text("playback.volume", "지훈", None, None, Some("150%"), 1);
        assert!(text.contains("150%"), "{text}");
        assert!(!text.contains("volume:"), "{text}");
    }

    // ───────── 디스코드 명령 그룹 ─────────

    /// 관리 콘솔과의 **계약**을 못 박는다. 화면은 이 모양만 보고 스위치를 그린다.
    /// 키 이름 하나가 바뀌면 화면은 그룹이 하나도 없는 것처럼 보이고, 그건 조용히 벌어진다.
    #[test]
    fn command_groups_payload_keeps_its_shape() {
        let payload = command_groups_json();
        let groups = payload.as_array().expect("배열이어야 해요");
        assert_eq!(groups.len(), crate::commands::catalog::GROUPS.len());
        for group in groups {
            for key in ["key", "label", "description", "commands"] {
                assert!(group.get(key).is_some(), "'{key}' 가 빠졌어요: {group}");
            }
            let commands = group.get("commands").and_then(Value::as_array).unwrap();
            assert!(!commands.is_empty(), "빈 그룹: {group}");
            for command in commands {
                // 화면은 `/play` 가 아니라 `/재생` 을 보여 줘야 한다.
                assert!(command.get("name").and_then(Value::as_str).is_some());
                assert!(command.get("korean").and_then(Value::as_str).is_some());
            }
        }
        // 곡 담기 그룹이 실제로 그 이름으로 나가는지 (설정 값이 이 키로 저장된다).
        assert!(groups.iter().any(|group| group.get("key") == Some(&json!("enqueue"))));
    }

    // ───────── 봇 주인 전역 강제값 ─────────

    fn body(pairs: &[(&str, Value)]) -> serde_json::Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    /// 강제값이 없으면 무엇을 보내도 안 막힌다 — 도입 전과 완전히 같아야 한다.
    #[test]
    fn nothing_is_blocked_when_the_owner_forced_nothing() {
        let overrides = GlobalOverrides::default();
        let sent = body(&[("maxQueuePerUser", json!(9)), ("chatEnabled", json!(true))]);
        assert!(attempted_locked_keys(&overrides, &sent).is_empty());
        assert!(override_lock_response(&overrides, &sent).is_none());
    }

    /// 잠긴 항목을 **다른 값으로** 바꾸려 하면 막고, 이름과 이유를 말한다.
    /// 조용히 저장된 척하면 화면은 새로고침 전까지 거짓말을 한다.
    #[test]
    fn changing_a_locked_setting_is_refused_but_resending_the_same_value_is_not() {
        let overrides = GlobalOverrides {
            max_queue_per_user: Some(3),
            chat_enabled: Some(false),
            ..Default::default()
        };

        // 바꾸려는 시도 → 막힌다.
        let changing = body(&[("maxQueuePerUser", json!(9))]);
        assert_eq!(
            attempted_locked_keys(&overrides, &changing),
            vec!["maxQueuePerUser"]
        );
        assert!(override_lock_response(&overrides, &changing).is_some());

        // 화면이 보여 준 강제값을 그대로 되보내는 것은 시도가 아니다.
        // 이걸 막으면 잠긴 항목 하나 때문에 그 섹션 전체가 저장 불능이 된다.
        let unchanged = body(&[
            ("maxQueuePerUser", json!(3)),
            ("chatEnabled", json!(false)),
            // 안 잠긴 항목은 자유롭게 같이 실려 온다.
            ("maxQueuePerGuild", json!(500)),
        ]);
        assert!(attempted_locked_keys(&overrides, &unchanged).is_empty());
        assert!(override_lock_response(&overrides, &unchanged).is_none());

        // 잠긴 항목을 아예 안 보내면 당연히 안 막힌다.
        let elsewhere = body(&[("maxTrackSeconds", json!(600))]);
        assert!(attempted_locked_keys(&overrides, &elsewhere).is_empty());
    }

    /// **"강제 안 함"과 "강제로 false" 는 다른 상태다.** `Option` 을 쓰는 이유 그 자체다.
    #[test]
    fn not_forced_and_forced_false_are_different_states() {
        let free = GlobalOverrides::default();
        let forced_off = GlobalOverrides {
            chat_enabled: Some(false),
            ..Default::default()
        };
        assert!(free.locked_value("chatEnabled").is_none());
        assert_eq!(
            forced_off.locked_value("chatEnabled"),
            Some(json!(false)),
            "강제로 끈 것과 안 건드린 것이 구분되지 않는다"
        );
        // 켜려는 시도는 강제로 껐을 때만 막힌다.
        let turning_on = body(&[("chatEnabled", json!(true))]);
        assert!(attempted_locked_keys(&free, &turning_on).is_empty());
        assert_eq!(
            attempted_locked_keys(&forced_off, &turning_on),
            vec!["chatEnabled"]
        );
    }

    /// UI 와의 계약. 키 이름이 바뀌면 자물쇠가 안 그려지므로 여기서 못 박는다.
    #[test]
    fn the_lock_payload_keeps_its_shape() {
        let payload = overrides_json(&GlobalOverrides {
            max_queue_per_user: Some(3),
            ..Default::default()
        });
        assert_eq!(payload["lockedKeys"], json!(["maxQueuePerUser"]));
        assert_eq!(payload["values"]["maxQueuePerUser"], json!(3));
        assert_eq!(payload["labels"]["maxQueuePerUser"], json!("1인 대기열 수"));
        assert!(payload["reason"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(
            payload["lockableKeys"]
                .as_array()
                .is_some_and(|keys| keys.len() == GlobalOverrides::LOCKABLE_KEYS.len())
        );
        // 강제값이 없으면 빈 목록이다 — 화면은 자물쇠를 하나도 안 그린다.
        let empty = overrides_json(&GlobalOverrides::default());
        assert_eq!(empty["lockedKeys"], json!([]));
        assert_eq!(empty["values"], json!({}));
    }

    /// 강제할 수 있는 항목은 전부 관리 콘솔이 실제로 쓰는 키여야 한다.
    /// 오타 하나면 그 항목은 영영 안 잠기고 아무도 눈치채지 못한다.
    #[test]
    fn every_lockable_key_has_a_korean_label() {
        for key in GlobalOverrides::LOCKABLE_KEYS {
            assert_ne!(
                GlobalOverrides::label_for(key),
                *key,
                "{key} 에 한국어 이름이 없어요 — 거절 메시지에 기계 키가 그대로 나가요"
            );
        }
    }
}
