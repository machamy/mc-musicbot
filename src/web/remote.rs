//! 마참뮤직 사용자 포털 HTTP/API/WebSocket 진입점.
//! Discord OAuth 세션과 길드 권한(`AccessTier`)을 검증한 뒤 기존 PlayerManager/Coordinator만 호출한다.
//!
//! v2 계약: `docs/REMOTE-API-V2.md`. 상태는 hot/cold로 갈라지고, 변경은 타입드 WS 이벤트
//! (`{"t":토픽,"d":데이터}`)로 밀어 준다. 프런트(`assets/portal.js`, `assets/console.js`)는
//! 이 계약대로 이미 작성돼 있으므로 서버가 프런트에 맞춘다.

use super::{WebState, remote_page};
use crate::models::{
    CsTimeSpan, PlaylistEntry, PlaylistScope, ProviderKind, QueueItem, RepeatMode, TrackRef,
};
use crate::remote::ranking;
use crate::remote::store::is_valid_pref;
use crate::remote::{
    AutoplaySeed, ChatTrackTag, LyricsCacheHit, LyricsDocument, LyricsLine, MAX_AUTOPLAY_SEEDS,
    PERMISSION_KEYS, PermissionRule, QueueScore, QueueSortMode, QueueVoteKind, RemoteGuildSettings,
    SeedAddOutcome, StoredSession, SuggestionStatus, Suspension, SuspensionScope, UserTrackKind,
};
use std::collections::BTreeMap;
use axum::Json;
use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
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
            Ok(())
        } else if self.tier.is_viewer() {
            Err(json_error(
                StatusCode::FORBIDDEN,
                self.viewer_reason
                    .clone()
                    .unwrap_or_else(|| "읽기 전용이라 아무것도 조작할 수 없어요.".into()),
            ))
        } else {
            Err(json_error(StatusCode::FORBIDDEN, message.to_string()))
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
}

impl RemoteEvent {
    fn wire(&self) -> String {
        serde_json::to_string(&json!({ "t": self.topic, "d": self.data }))
            .unwrap_or_else(|_| "{\"t\":\"notice\",\"d\":{}}".into())
    }
}

/// 타입드 이벤트 하나를 그 길드 구독자에게만 보낸다.
fn emit(state: &WebState, guild_id: u64, topic: &str, data: Value) {
    let _ = state.remote_events.send(RemoteEvent {
        guild_id,
        topic: topic.into(),
        data,
    });
}

/// payload 없이 "재조회해라"만 알리는 토픽 (`settings`/`library`/`audit` 등).
fn emit_bare(state: &WebState, guild_id: u64, topic: &str) {
    emit(state, guild_id, topic, json!({}));
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
        .route("/music/api/guilds/{guild_id}/queue", post(api_enqueue))
        .route(
            "/music/api/guilds/{guild_id}/queue/action",
            post(api_queue_action),
        )
        .route("/music/api/guilds/{guild_id}/control", post(api_control))
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

/// `app::queue_sort_loop`의 재정렬 주기(초). 값 자체는 `app.rs`가 갖고 있다.
const QUEUE_SORT_PERIOD_SECONDS: i64 = crate::app::QUEUE_SORT_INTERVAL.as_secs() as i64;

/// 대기열 갱신 카운트다운(V3 §5)의 기준 시각 두 개 — `(sortedAt, nextSortAt)`.
///
/// `nextSortAt`은 5초 루프가 **마지막으로 돈 시각 + 주기**다. 클라이언트 타이머만 쓰면
/// 탭이 백그라운드에 갔다 오는 순간 어긋나므로 기준 시각은 서버가 준다.
/// 루프가 아직 한 번도 안 돌았으면(기동 직후) "지금부터 한 주기"로 근사한다.
fn sort_clock(state: &WebState) -> (String, String) {
    let now = chrono::Utc::now();
    let last = state
        .app
        .last_queue_sort
        .read()
        .ok()
        .and_then(|slot| *slot)
        .unwrap_or(now);
    let period = chrono::Duration::seconds(QUEUE_SORT_PERIOD_SECONDS);
    let mut next = last + period;
    // 루프가 밀렸으면(길드가 많거나 tick을 건너뛰었으면) 이미 지난 시각을 주지 않는다.
    while next <= now {
        next += period;
    }
    (now.to_rfc3339(), next.to_rfc3339())
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
    let _ = state.app.remote.add_audit(
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
    emit_bare(state, guild_id, "audit");
}

// ───────────────────────── 세션 ─────────────────────────

fn session_cookie_token(cookies: &Cookies) -> Option<String> {
    cookies.get(REMOTE_COOKIE).map(|c| c.value().to_string())
}

/// 메모리에 없으면 DB(`remote_web_sessions`)에서 복구한다 — 봇을 재시작해도 로그인이 유지된다.
/// 스키마에 `username`/`csrf_token`/`is_developer` 컬럼이 없으므로 복구 시
/// CSRF 토큰은 새로 만들고, username은 display_name으로 대신하며, dev 세션은 복구하지 않는다.
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
        csrf_token: crate::models::uuid_like(),
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

fn stale_roles(state: &Arc<WebState>, guild_id: u64, user_id: u64) -> Option<Vec<u64>> {
    state
        .remote_member_roles
        .lock()
        .unwrap()
        .get(&(guild_id, user_id))
        .filter(|(seen, _)| seen.elapsed() < MEMBER_CACHE_GRACE)
        .map(|(_, roles)| roles.clone())
}

/// 이 사람이 봇과 같은 음성 채널에 있는지.
async fn same_voice_channel(state: &WebState, guild_id: u64, user_id: u64) -> bool {
    let player = state.app.player.get_state(guild_id).await;
    let Some(bot_channel) = player.voice_channel_id else {
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
        PermissionRule::SameVoiceChannel => member.same_voice_channel,
        PermissionRule::ConfiguredRole => has_configured_role(key, settings, member),
        PermissionRule::Administrator | PermissionRule::Disabled => false,
    }
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
        PermissionRule::SameVoiceChannel => member.same_voice_channel,
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

    // 2. 세션의 길드 목록에 없음 → 403
    let guild = guild_from_session(&session, guild_id)
        .ok_or_else(|| json_error(StatusCode::FORBIDDEN, "이 서버의 멤버가 아니에요."))?;

    // 3. 봇이 그 길드에 없음 → 403
    if !session.is_developer && !bot_in_guild(state, guild_id) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "봇이 이 Discord 서버에 없어요.",
        ));
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
    let (mut tier, mut member, mut viewer_reason) =
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
    })
}

/// 등급과 멤버 컨텍스트를 함께 만든다.
async fn resolve_tier(
    state: &Arc<WebState>,
    session: &RemoteSession,
    guild: &OAuthGuild,
    settings: &RemoteGuildSettings,
    fresh: bool,
) -> (AccessTier, MemberContext, Option<String>) {
    let guild_id = guild.id;
    let owner = is_owner_user(state, session.user_id);

    if session.is_developer {
        return (
            if owner { AccessTier::Owner } else { AccessTier::Manager },
            MemberContext {
                is_admin: true,
                same_voice_channel: true,
                role_ids: Vec::new(),
            },
            None,
        );
    }

    let same_voice = same_voice_channel(state, guild_id, session.user_id).await;
    let lookup = fetch_member_roles(state, session, guild_id, fresh).await;

    let (role_ids, demote) = match lookup {
        Ok(roles) => (roles, false),
        Err(MemberLookupError::NotInGuild) => (Vec::new(), true),
        Err(MemberLookupError::Transient(reason)) => {
            state.app.log.warn(
                "RemoteAuth",
                &format!("길드 {guild_id} 멤버 재조회 일시 실패 — 등급 유지: {reason}"),
            );
            (
                stale_roles(state, guild_id, session.user_id).unwrap_or_default(),
                false,
            )
        }
    };

    // 7. 추방·탈퇴 → 403이 아니라 Viewer로 강등한다.
    if demote && !owner {
        return (
            AccessTier::Viewer,
            MemberContext {
                is_admin: false,
                same_voice_channel: same_voice,
                role_ids,
            },
            Some("이 서버에서 나갔거나 추방돼서 읽기 전용이에요.".into()),
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
            role_ids,
        },
        None,
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
    {
        let mut registry = state.presence.lock().unwrap();
        if let Some(count) = registry.get_mut(&(guild_id, user_id)) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                registry.remove(&(guild_id, user_id));
            }
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

/// 봇이 지금 어디에 있는지. Discord 캐시 + 메모리만 본다 — **DB를 쓰지 않는다**(V3 §4).
///
/// `voice_channel_id`(플레이어가 기억하는 값)와 캐시의 실제 음성 상태가 어긋날 수 있어서,
/// 캐시에 봇의 voice_state가 있으면 그쪽을 진짜로 친다. 화면에 "듣는 중"이 뜨는데
/// 실제로는 아무도 같은 방에 없는 상황이 §4가 지적한 바로 그 버그다.
#[derive(Debug, Clone, Default)]
struct BotVoiceStatus {
    in_guild: bool,
    channel_id: Option<u64>,
    channel_name: Option<String>,
}

fn bot_voice_status(state: &WebState, guild_id: u64, player_channel: Option<u64>) -> BotVoiceStatus {
    let Some(cache) = state.app.discord_cache.get() else {
        return BotVoiceStatus::default();
    };
    let Some(guild) = cache.guild(GuildId::new(guild_id)) else {
        return BotVoiceStatus::default();
    };
    let bot_id = cache.current_user().id;
    let channel_id = guild
        .voice_states
        .get(&bot_id)
        .and_then(|voice| voice.channel_id)
        .map(|channel| channel.get())
        .or(player_channel);
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
    let player = state.app.player.get_state(guild_id).await;
    let bot = bot_voice_status(state, guild_id, player.voice_channel_id);
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
            "inVoice": bot.channel_id.is_some(),
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
            let signature = format!(
                "{}|{}|{}|{}|{}",
                player
                    .current_item
                    .as_ref()
                    .map(|item| item.id.as_str())
                    .unwrap_or(""),
                player.is_paused,
                player.effective_volume,
                player.repeat_mode.as_str(),
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
                playback_payload(&state, &player, position, &sampled_at),
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
) -> Value {
    json!({
        "id": item.id,
        "track": track_json(&item.track),
        "requestedByDisplay": item.requested_by_display,
        "requestedByUserId": item.requested_by_user_id.map(|id| id.to_string()),
        "isMine": item.requested_by_user_id == Some(viewer_user_id),
        "myVote": my_vote.map(|kind| match kind {
            QueueVoteKind::Like => "like",
            QueueVoteKind::SuperLike => "superLike",
        }),
        "round": score.round,
        "score": {
            "waitScore": score.wait_score,
            "likeCount": score.like_count,
            "superLikeCount": score.super_like_count,
            "manualPriority": score.manual_priority,
            "totalScore": score.total_score(),
        }
    })
}

fn current_json(item: &QueueItem) -> Value {
    json!({
        "id": item.id,
        "track": track_json(&item.track),
        "durationSeconds": item.track.duration.map(|duration| duration.as_secs_f64()),
        "requestedByDisplay": item.requested_by_display,
        "requestedByUserId": item.requested_by_user_id.map(|id| id.to_string()),
    })
}

fn playback_payload(
    state: &WebState,
    player: &crate::models::GuildPlayerState,
    position: f64,
    sampled_at: &str,
) -> Value {
    json!({
        "isPaused": player.is_paused,
        "positionSeconds": position,
        "sampledAtUtc": sampled_at,
        "currentId": player.current_item.as_ref().map(|item| item.id.clone()),
        "current": player.current_item.as_ref().map(current_json),
        "durationSeconds": player
            .current_item
            .as_ref()
            .and_then(|item| item.track.duration)
            .map(|duration| duration.as_secs_f64()),
        "effectiveVolume": player.effective_volume,
        "repeatMode": repeat_key(player.repeat_mode),
        "shuffleEnabled": player.shuffle_enabled,
        "voiceChannelId": player.voice_channel_id.map(|id| id.to_string()),
        "botOnline": bot_in_guild(state, player.guild_id),
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
    // 이 프레임은 모든 구독자가 같이 받는다 — 개인화 필드(isMine/myVote)는 넣지 않는다.
    let items: Vec<Value> = player
        .upcoming
        .iter()
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            let mut value = queue_item_json(item, &score, 0, None);
            value["isMine"] = Value::Null;
            value["myVote"] = Value::Null;
            value
        })
        .collect();
    let (sorted_at, next_sort_at) = sort_clock(state);
    emit(
        state,
        guild_id,
        "queue.set",
        json!({
            "items": items,
            "mode": settings.sort_mode.as_str(),
            "sortedAt": sorted_at,
            // 카운트다운 기준(V3 §5). 클라 타이머만 쓰면 백그라운드 탭에서 어긋난다.
            "nextSortAt": next_sort_at,
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
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
    };
    if !auth.has_client_secret() {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
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
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
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
    let guilds = guild_rows
        .into_iter()
        .filter_map(|guild| {
            Some(OAuthGuild {
                id: guild.id.parse().ok()?,
                name: guild.name,
                icon: guild.icon,
                owner: guild.owner,
                permissions: guild.permissions.parse().unwrap_or(0),
            })
        })
        .collect();
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
    let queue: Vec<Value> = player
        .upcoming
        .iter()
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            let my_vote = state.app.remote.user_vote(&item.id, ctx.user_id());
            queue_item_json(item, &score, ctx.user_id(), my_vote)
        })
        .collect();
    let (sorted_at, next_sort_at) = sort_clock(&state);

    json_ok(json!({
        "player": {
            "isPaused": player.is_paused,
            "effectiveVolume": player.effective_volume,
            "repeatMode": repeat_key(player.repeat_mode),
            "shuffleEnabled": player.shuffle_enabled,
            "voiceChannelId": player.voice_channel_id.map(|id| id.to_string()),
            "botOnline": ctx.session.is_developer || bot_in_guild(&state, guild_id),
            "minVolume": ctx.settings.min_volume,
            "maxVolume": ctx.settings.max_volume,
        },
        "current": player.current_item.as_ref().map(current_json),
        "positionSeconds": position,
        "sampledAtUtc": sampled_at,
        "queueMode": ctx.settings.sort_mode.as_str(),
        "sortedAt": sorted_at,
        "nextSortAt": next_sort_at,
        "queue": queue,
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

    let playlists: Vec<Value> = state
        .app
        .db
        .list_playlists(PlaylistScope::Guild, Some(guild_id))
        .into_iter()
        .map(|playlist| {
            json!({
                "id": playlist.id,
                "name": playlist.name,
                "ownerUserId": playlist.owner_user_id.to_string(),
                "isMine": playlist.owner_user_id == session.user_id,
                "entryCount": playlist.entries.len(),
                "entries": playlist
                    .entries
                    .iter()
                    .filter_map(|entry| entry.track.as_ref())
                    .map(|track| json!({ "track": track_json(track) }))
                    .collect::<Vec<_>>(),
            })
        })
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
            "minVolume": settings.min_volume,
            "maxVolume": settings.max_volume,
            "sortMode": settings.sort_mode.as_str(),
        },
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
    }))
}

/// 검색을 어디서 돌릴지(V3 §6).
///
/// 운영 패널에 YouTube API 키가 있으면 브라우저가 YouTube Data API를 직접 부른다
/// (봇 호스트의 `yt-dlp`가 느리거나 막혀도 검색이 살아 있다). 키가 없으면 지금처럼 서버가 찾는다.
/// 키는 브라우저로 그대로 나가는 값이라 리퍼러 제한이 전제다 — 운영 패널에 그렇게 적어 뒀다.
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
    let rows: Vec<(&str, &str, PermissionRule, bool, &str)> = vec![
        ("search", "곡 검색·신청", settings.search_rule, true, "search"),
        ("vote", "좋아요·슈퍼 좋아요", settings.vote_rule, true, "vote"),
        ("playback", "재생 / 일시정지 / 스킵", settings.playback_rule, true, "playback"),
        ("seek", "재생 위치 이동", settings.seek_rule, true, "seek"),
        ("volume", "볼륨 조절", settings.volume_rule, true, "volume"),
        ("queueEdit", "대기열 편집", settings.queue_edit_rule, true, "queueEdit"),
        ("chat", "채팅 쓰기·반응·답장", settings.chat_rule, settings.chat_enabled, "chat"),
        ("autoplaySeed", "자동 재생 기준 곡 등록", settings.autoplay_seed_rule, true, "autoplaySeed"),
        ("playlistEdit", "재생목록 편집", settings.queue_edit_rule, true, "queueEdit"),
        ("library", "보관함·재생목록", PermissionRule::GuildMember, true, "library"),
        ("suggest", "제안 작성·공감", PermissionRule::GuildMember, settings.suggestion_enabled, "suggest"),
        ("chatDelete", "남의 채팅 삭제", PermissionRule::Administrator, true, "chatDelete"),
        ("suggestStatus", "제안 상태 변경", PermissionRule::Administrator, true, "suggestStatus"),
        ("suspend", "유저 정지·해제", PermissionRule::Administrator, true, "suspend"),
        ("sortMode", "정렬 모드 변경", PermissionRule::Administrator, true, "sortMode"),
        ("console", "서버 관리 콘솔", PermissionRule::Administrator, true, "console"),
    ];

    let mut can = serde_json::Map::new();
    let mut entries: Vec<Value> = Vec::with_capacity(rows.len() + 1);
    for (key, label, rule, gate, role_key) in rows {
        let base = rule_base_allowed(role_key, rule, settings, member);
        let allowed = !viewer && gate && permission_allowed(role_key, rule, settings, member);
        let via_admin = allowed && !base;
        let reason = if viewer {
            Some(
                ctx.viewer_reason
                    .clone()
                    .unwrap_or_else(|| "읽기 전용이라 아무것도 조작할 수 없어요.".into()),
            )
        } else if !gate {
            Some("관리자가 이 기능을 꺼 뒀어요.".into())
        } else if !allowed && rule == PermissionRule::Disabled {
            Some("사용 안 함으로 설정돼 있어서 아무도 쓸 수 없어요.".into())
        } else {
            None
        };
        can.insert(key.to_string(), Value::Bool(allowed));
        entries.push(json!({
            "key": key,
            "label": label,
            "allowed": allowed,
            "rule": rule_key(rule),
            "ruleLabel": rule_label(rule),
            "viaAdmin": via_admin,
            "reason": reason,
            // 왜 되는지/안 되는지 설명하려면 역할 이름이 있어야 말이 된다 (V3 §1).
            // 지정 역할 규칙이 아닌 줄은 빈 배열이다.
            "roleNames": if rule == PermissionRule::ConfiguredRole {
                json!(role_names(state, ctx.guild_id(), settings.roles_for(role_key)))
            } else {
                json!([])
            },
        }));
    }

    // 운영 패널은 봇 주인 전용 — 길드 설정과 무관하다.
    let ops = ctx.tier.is_owner();
    can.insert("ops".into(), Value::Bool(ops));
    entries.push(json!({
        "key": "ops",
        "label": "운영 패널",
        "allowed": ops,
        "rule": "owner",
        "ruleLabel": "봇 주인 전용",
        "viaAdmin": false,
        "reason": if ops { Value::Null } else { Value::String("여기는 봇 주인만 들어갈 수 있어요.".into()) },
        "roleNames": json!([]),
    }));

    json!({ "can": Value::Object(can), "entries": entries })
}

/// 역할 ID를 사람이 읽는 이름으로. 캐시에 없는 역할(지워졌거나 아직 못 받은)은
/// ID를 그대로 보여 준다 — 조용히 빼면 "역할 3개 지정했는데 2개만 보이는" 상황이 된다.
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
    let queue: Vec<Value> = player
        .upcoming
        .iter()
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            queue_item_json(
                item,
                &score,
                ctx.user_id(),
                state.app.remote.user_vote(&item.id, ctx.user_id()),
            )
        })
        .collect();
    json_ok(json!({
        "guild": ctx.guild.to_json(),
        "user": {
            "id": ctx.session.user_id.to_string(),
            "displayName": ctx.session.display_name,
            "avatarUrl": ctx.session.avatar_url,
        },
        "tier": ctx.tier.as_str(),
        "player": {
            "voiceChannelId": player.voice_channel_id.map(|id| id.to_string()),
            "isPaused": player.is_paused,
            "effectiveVolume": player.effective_volume,
            "repeatMode": repeat_key(player.repeat_mode),
            "shuffleEnabled": player.shuffle_enabled,
            "autoplayEnabled": player.autoplay_enabled,
        },
        "connection": {
            "botOnline": ctx.session.is_developer || bot_in_guild(&state, guild_id),
            "voiceConnected": player.voice_channel_id.is_some(),
        },
        "current": player.current_item.as_ref().map(current_json),
        "positionSeconds": position,
        "sampledAtUtc": sampled_at,
        "queue": queue,
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

async fn api_audit(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    Query(query): Query<BeforeQuery>,
) -> Response {
    let ctx = match authorize(&state, &cookies, guild_id, None).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if ctx.tier.is_viewer() {
        return json_ok(json!({ "entries": [] }));
    }
    let limit = query.limit.unwrap_or(100).clamp(1, 300);
    let entries: Vec<Value> = state
        .app
        .remote
        .list_audit(guild_id, limit, query.before)
        .iter()
        .map(audit_json)
        .collect();
    json_ok(json!({ "entries": entries }))
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
    Json(request): Json<EnqueueRequest>,
) -> Response {
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
    if request
        .track
        .duration
        .is_some_and(|duration| duration.as_secs_f64() > ctx.settings.max_track_seconds as f64)
    {
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
    if player.upcoming.len() >= ctx.settings.max_queue_per_guild.max(1) as usize
        || user_count >= ctx.settings.max_queue_per_user.max(1) as usize
    {
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
            "서버나 개인 대기열 제한에 닿았어요.",
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
    let (rule_key_for_action, rule) = match request.action.as_str() {
        "seek" => ("seek", ctx.settings.seek_rule),
        "volume" => ("volume", ctx.settings.volume_rule),
        "shuffle" => ("queueEdit", ctx.settings.queue_edit_rule),
        _ => ("playback", ctx.settings.playback_rule),
    };
    if let Err(response) = ctx.require(rule_key_for_action, rule, "재생을 조작할 권한이 없어요.") {
        return response;
    }
    if let Err(response) = ctx.require_not_suspended(SuspensionScope::Queue) {
        return response;
    }
    let player = state.app.player.get_state(guild_id).await;
    if player.voice_channel_id.is_none() {
        return json_error(
            StatusCode::CONFLICT,
            "봇이 음성 채널에 안 들어가 있어요.",
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
            if seconds < 0.0 || duration <= 0.0 || seconds > duration {
                Err("옮기려는 위치가 곡 길이를 벗어났어요.".into())
            } else {
                state
                    .app
                    .player
                    .set_current_start_offset(guild_id, CsTimeSpan::from_secs_f64(seconds))
                    .await;
                if !session.is_developer {
                    state.app.coordinator.cancel_current(guild_id).await;
                    state.app.coordinator.sync_guild(&state.app, guild_id).await;
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
                Ok(format!("volume:{volume}"))
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
        _ => Err("지원하지 않는 재생 제어예요.".into()),
    };
    match result {
        Ok(after) => {
            audit_ok(
                &state,
                guild_id,
                session,
                &format!("playback.{}", request.action),
                None,
                Some(&after),
            );
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
                playback_payload(&state, &player, position, &sampled_at),
            );
            if queue_changed {
                broadcast_queue(&state, guild_id).await;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoteRequest {
    item_id: String,
    kind: Option<QueueVoteKind>,
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
    let Some(item) = player
        .upcoming
        .iter()
        .find(|item| item.id == request.item_id)
    else {
        return json_error(StatusCode::NOT_FOUND, "그 대기열 항목을 찾지 못했어요.");
    };
    if item.requested_by_user_id == Some(ctx.user_id()) {
        return json_error(
            StatusCode::FORBIDDEN,
            "자기가 신청한 곡에는 투표할 수 없어요.",
        );
    }
    if let Err(error) = state.app.remote.set_vote(
        guild_id,
        &item.id,
        ctx.user_id(),
        request.kind,
        &item.track,
    ) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    state.app.player.refresh_scored_order(guild_id).await;
    audit_ok(
        &state,
        guild_id,
        &ctx.session,
        "queue.vote",
        Some(item.track.display_title()),
        request.kind.map(QueueVoteKind::as_str),
    );
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
            "total": score.total_score(),
        }),
    );
    broadcast_queue(&state, guild_id).await;
    json_ok(json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueActionRequest {
    action: String,
    item_id: String,
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
    let Some(index) = player
        .upcoming
        .iter()
        .position(|item| item.id == request.item_id)
    else {
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
                    .cancel_by_id(guild_id, &request.item_id)
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
            let new_priority = if scores
                .get(&request.item_id)
                .and_then(|score| score.manual_priority)
                .is_some()
            {
                None
            } else {
                Some(1_000_000)
            };
            if let Err(error) = state
                .app
                .player
                .set_manual_priority(guild_id, &request.item_id, new_priority)
                .await
            {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, error);
            }
            audit_ok(
                &state,
                guild_id,
                &ctx.session,
                "queue.force_move",
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
    Json(request): Json<LibraryRequest>,
) -> Response {
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
    emit_bare(&state, guild_id, "library");
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
    if let Err(response) = ctx.require(
        "queueEdit",
        ctx.settings.queue_edit_rule,
        "재생목록을 편집할 권한이 없어요.",
    ) {
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
        "playlist",
        Duration::from_millis(300),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "재생목록 요청이 너무 빨라요. 잠깐만 쉬었다 해요.",
        );
    }
    let name = request.name.as_deref().map(str::trim).unwrap_or("");
    let target = request
        .playlist_id
        .and_then(|id| state.app.db.find_playlist(id));
    if let Some(playlist) = target.as_ref() {
        if playlist.scope != PlaylistScope::Guild || playlist.guild_id != Some(guild_id) {
            return json_error(StatusCode::NOT_FOUND, "이 서버의 재생목록이 아니에요.");
        }
        if playlist.owner_user_id != session.user_id && !ctx.tier.is_manager() {
            return json_error(
                StatusCode::FORBIDDEN,
                "만든 사람이나 관리자만 고칠 수 있어요.",
            );
        }
    }
    let audit_target = match request.action.as_str() {
        "create" => {
            if name.is_empty() || name.chars().count() > 80 {
                return json_error(StatusCode::BAD_REQUEST, "이름은 1~80자로 입력해요.");
            }
            let id = state.app.db.create_playlist(
                PlaylistScope::Guild,
                Some(guild_id),
                session.user_id,
                name,
            );
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
            if track.duration.is_some_and(|duration| {
                duration.as_secs_f64() > ctx.settings.max_track_seconds as f64
            }) || state.app.blacklist.is_blocked(guild_id, &track)
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
        "removeEntry" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "그 재생목록을 찾지 못했어요.");
            };
            let Some(index) = request.entry_index else {
                return json_error(StatusCode::BAD_REQUEST, "지울 곡의 순서 번호가 없어요.");
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
            if tracks.iter().any(|track| {
                track.duration.is_some_and(|duration| {
                    duration.as_secs_f64() > ctx.settings.max_track_seconds as f64
                }) || state.app.blacklist.is_blocked(guild_id, track)
            }) {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "재생목록에 길이 제한을 넘거나 차단된 곡이 섞여 있어요.",
                );
            }
            let player = state.app.player.get_state(guild_id).await;
            let existing: HashSet<String> = player
                .current_item
                .iter()
                .chain(player.upcoming.iter())
                .map(|item| item.track.cache_key())
                .collect();
            if tracks
                .iter()
                .any(|track| existing.contains(&track.cache_key()))
            {
                return json_error(
                    StatusCode::CONFLICT,
                    "재생목록의 곡 하나가 이미 재생 중이거나 대기열에 있어요.",
                );
            }
            let own = player
                .upcoming
                .iter()
                .filter(|item| item.requested_by_user_id == Some(session.user_id))
                .count();
            if player.upcoming.len() + tracks.len()
                > ctx.settings.max_queue_per_guild.max(1) as usize
                || own + tracks.len() > ctx.settings.max_queue_per_user.max(1) as usize
            {
                return json_error(StatusCode::CONFLICT, "대기열 제한을 넘어요.");
            }
            for track in tracks {
                if crate::media::resolver::can_resolve(&track.source_url)
                    && !state.app.blacklist.is_blocked(guild_id, &track)
                {
                    state
                        .app
                        .player
                        .enqueue(
                            guild_id,
                            QueueItem::new_user(
                                track,
                                session.display_name.clone(),
                                Some(session.user_id),
                            ),
                            false,
                        )
                        .await;
                }
            }
            if !session.is_developer {
                state.app.coordinator.sync_guild(&state.app, guild_id).await;
            }
            format!("{}:{}", playlist.id, playlist.name)
        }
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "지원하지 않는 재생목록 작업이에요.",
            );
        }
    };
    audit_ok(
        &state,
        guild_id,
        session,
        &format!("playlist.{}", request.action),
        Some(&audit_target),
        Some("ok"),
    );
    emit_bare(&state, guild_id, "library");
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
    let seeds: Vec<Value> = state
        .app
        .remote
        .list_autoplay_seeds(guild_id)
        .iter()
        .map(|seed| autoplay_seed_json(state, guild_id, seed))
        .collect();
    json!({ "seeds": seeds, "max": MAX_AUTOPLAY_SEEDS, "canEdit": can_edit })
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
    let can_edit = ctx.allows("autoplaySeed", ctx.settings.autoplay_seed_rule);
    json_ok(autoplay_payload(&state, guild_id, can_edit))
}

/// 기준 곡 편집 공통 게이트 — 권한 + 신청 정지.
fn autoplay_gate(ctx: &AuthContext) -> Result<(), Response> {
    ctx.require(
        "autoplaySeed",
        ctx.settings.autoplay_seed_rule,
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
    Json(request): Json<AutoplaySeedAddRequest>,
) -> Response {
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
    if request.cache_keys.len() > MAX_AUTOPLAY_SEEDS {
        return json_error(
            StatusCode::BAD_REQUEST,
            format!("기준 곡은 {MAX_AUTOPLAY_SEEDS}곡까지예요."),
        );
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
        || !(1..=100).contains(&request.max_queue_per_user)
        || !(1..=1000).contains(&request.max_queue_per_guild)
        || !(60..=86_400).contains(&request.max_track_seconds)
        || !(1..=3650).contains(&request.audit_retention_days)
        || request.configured_role_ids.len() > 50
    {
        return json_error(StatusCode::BAD_REQUEST, "설정 값이 허용 범위를 벗어났어요.");
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
    if let Err(error) = state.app.remote.save_guild_settings(&settings) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    // PlayerManager 는 정렬 모드를 캐시한다. 저장만 하고 캐시를 안 맞추면
    // 봇을 재시작할 때까지 옛 모드로 정렬된다.
    state.app.player.set_sort_mode(guild_id, settings.sort_mode);
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
    let _ = state
        .app
        .remote
        .prune_audit(guild_id, settings.audit_retention_days);
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
    json!({
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
        "autoplaySeedRule": rule_key(settings.autoplay_seed_rule),
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
    })
}

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
        }
        "perms" => {
            // 저장하는 순간 레거시 통짜 값을 8개 키로 펼친다. 그래야 이후로는
            // 읽기 폴백에 기대지 않고 키마다 따로 관리된다 (V3 §1 마이그레이션).
            settings.expand_legacy_roles();
            let rules: [(&str, &mut PermissionRule); 8] = [
                ("searchRule", &mut settings.search_rule),
                ("voteRule", &mut settings.vote_rule),
                ("chatRule", &mut settings.chat_rule),
                ("playbackRule", &mut settings.playback_rule),
                ("seekRule", &mut settings.seek_rule),
                ("volumeRule", &mut settings.volume_rule),
                ("queueEditRule", &mut settings.queue_edit_rule),
                ("autoplaySeedRule", &mut settings.autoplay_seed_rule),
            ];
            for (key, slot) in rules {
                match json_rule(&body, key) {
                    Ok(Some(rule)) => *slot = rule,
                    Ok(None) => {}
                    Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                }
            }
            // `ruleRoleIds`는 보낸 키만 갱신한다. 안 보낸 키는 건드리지 않는다 —
            // 관리 콘솔이 섹션 일부만 저장해도 다른 권한의 역할이 날아가면 안 된다.
            if let Some(map) = body.get("ruleRoleIds").and_then(Value::as_object) {
                for (key, value) in map {
                    if !PERMISSION_KEYS.contains(&key.as_str()) {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("{key}: 알 수 없는 권한 키예요."),
                        );
                    }
                    let ids = match parse_role_ids(value) {
                        Ok(ids) => ids,
                        Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                    };
                    settings.rule_role_ids.insert(key.clone(), ids);
                }
            }
            if let Some(value) = body.get("managerRoleIds") {
                match parse_role_ids(value) {
                    Ok(ids) => settings.manager_role_ids = ids,
                    Err(error) => return json_error(StatusCode::BAD_REQUEST, error),
                }
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
            if let Some(value) = json_i32(&body, "chatRetentionDays") {
                settings.chat_retention_days = value.max(1) as u32;
            }
            if settings.min_volume < 0
                || settings.max_volume > 200
                || settings.min_volume > settings.max_volume
                || !(1..=100).contains(&settings.max_queue_per_user)
                || !(1..=1000).contains(&settings.max_queue_per_guild)
                || !(60..=86_400).contains(&settings.max_track_seconds)
                || !(1..=3650).contains(&settings.audit_retention_days)
                || !(1..=365).contains(&settings.chat_retention_days)
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

    if let Err(error) = state.app.remote.save_guild_settings(&settings) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    // 아래 refresh_scored_order 가 캐시된 모드를 읽으므로 반드시 그 전에 맞춘다.
    state.app.player.set_sort_mode(guild_id, settings.sort_mode);
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
struct ModeQuery {
    mode: Option<String>,
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
    let mut preview = player.upcoming.clone();
    ranking::sort_queue(&mut preview, &scores, mode);

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
                "score": score.total_score(),
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

    let player = state.app.player.get_state(guild_id).await;
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
                let same_voice = player.voice_channel_id.is_some_and(|channel| {
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
    } else if rule == PermissionRule::SameVoiceChannel && player.voice_channel_id.is_none() {
        note = "봇이 음성 채널에 없어서 지금은 관리자만 통과해요.".into();
    } else if rule == PermissionRule::ConfiguredRole && settings.roles_for(key).is_empty() {
        note = "이 권한에 지정된 역할이 없어서 지금은 관리자만 통과해요.".into();
    }

    json_ok(json!({
        "rule": rule_key(rule),
        "key": key,
        "passCount": pass_count,
        "memberCount": member_count,
        "managerBypassCount": bypass_count,
        "note": note,
        "sample": sample,
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
    let player = state.app.player.get_state(guild_id).await;
    let voice_channel_name = player.voice_channel_id.and_then(|channel| {
        state
            .app
            .discord_cache
            .get()
            .and_then(|cache| cache.guild(GuildId::new(guild_id)))
            .and_then(|guild| {
                guild
                    .channels
                    .get(&serenity::all::ChannelId::new(channel))
                    .map(|channel| channel.name.clone())
            })
    });
    json_ok(json!({
        "bot": {
            "online": bot_in_guild(&state, guild_id),
            "voiceConnected": player.voice_channel_id.is_some(),
            "voiceChannelName": voice_channel_name,
            // 게이트웨이 지연은 ShardManager 핸들이 있어야 읽을 수 있다(App에 없음).
            "gatewayLatencyMs": Value::Null,
        },
        "buildId": state.app.build_id,
        // store::SCHEMA_VERSION 이 비공개라 지금은 노출하지 못한다 (보고서 참고).
        "schemaVersion": Value::Null,
        "uptimeSeconds": uptime_seconds(),
    }))
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
        websocket_loop(socket, receiver, guild_id).await;
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
                Ok(event) if event.guild_id == guild_id => {
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

    /// 자동 재생 기준 곡 권한도 같은 판정 경로를 탄다(기본은 관리자만).
    #[test]
    fn autoplay_seed_rule_defaults_to_administrator_only() {
        let settings = RemoteGuildSettings::default();
        assert_eq!(settings.autoplay_seed_rule, PermissionRule::Administrator);
        let member = MemberContext::default();
        assert!(!permission_allowed(
            "autoplaySeed",
            settings.autoplay_seed_rule,
            &settings,
            &member
        ));
        let admin = MemberContext {
            is_admin: true,
            ..Default::default()
        };
        assert!(permission_allowed(
            "autoplaySeed",
            settings.autoplay_seed_rule,
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
    #[test]
    fn countdown_period_follows_the_sort_loop() {
        assert_eq!(
            QUEUE_SORT_PERIOD_SECONDS,
            crate::app::QUEUE_SORT_INTERVAL.as_secs() as i64
        );
        assert_eq!(QUEUE_SORT_PERIOD_SECONDS, 5);
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
            MemberContext { is_admin: true, same_voice_channel: true, role_ids: vec![777] },
            MemberContext { is_admin: false, same_voice_channel: true, role_ids: vec![777] },
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

    #[test]
    fn permission_defaults_match_remote_contract() {
        let settings = RemoteGuildSettings::default();
        let member = MemberContext::default();
        assert!(!permission_allowed("playback", settings.playback_rule, &settings, &member));
        assert!(permission_allowed("seek", settings.seek_rule, &settings, &member));
        assert!(!permission_allowed("volume", settings.volume_rule, &settings, &member));
        let same_voice = MemberContext {
            same_voice_channel: true,
            ..Default::default()
        };
        assert!(permission_allowed(
            "playback",
            settings.playback_rule,
            &settings,
            &same_voice
        ));
    }

    /// 관리자 우회로 통과한 항목은 "← 관리자라 통과"로 표시돼야 한다.
    #[test]
    fn via_admin_is_detected_by_base_rule() {
        let settings = RemoteGuildSettings::default();
        let admin_outside_voice = MemberContext {
            is_admin: true,
            same_voice_channel: false,
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
}
