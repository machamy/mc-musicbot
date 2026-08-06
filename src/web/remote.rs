//! 마참뮤직 사용자 포털 HTTP/API/WebSocket 진입점.
//! Discord OAuth 세션과 길드 권한을 검증한 뒤 기존 PlayerManager/Coordinator만 호출한다.

use super::{WebState, remote_page};
use crate::models::{CsTimeSpan, PlaylistEntry, PlaylistScope, ProviderKind, QueueItem, TrackRef};
use crate::remote::{
    LyricsDocument, LyricsLine, PermissionRule, QueueVoteKind, RemoteGuildSettings, UserTrackKind,
};
use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serenity::all::{GuildId, UserId};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower_cookies::{Cookie, Cookies};

const REMOTE_COOKIE: &str = "macham_session";
const REMOTE_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const OAUTH_STATE_TTL: Duration = Duration::from_secs(10 * 60);
const ADMINISTRATOR_PERMISSION: u64 = 1 << 3;
const MANAGE_GUILD_PERMISSION: u64 = 1 << 5;
const REMOTE_AUTH_FILE: &str = "remote-oauth.json";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredRemoteAuthConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
    public_base_url: Option<String>,
}

#[derive(Clone)]
pub struct RemoteAuthConfig {
    pub client_id: Option<String>,
    client_secret: Option<String>,
    pub public_base_url: String,
    pub dev_login: bool,
}

impl std::fmt::Debug for RemoteAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret_configured", &self.client_secret.is_some())
            .field("public_base_url", &self.public_base_url)
            .field("dev_login", &self.dev_login)
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
        let (client_id, client_secret, public_base_url) = match stored {
            Some(stored) => (
                clean(stored.client_id),
                clean(stored.client_secret),
                clean(stored.public_base_url).unwrap_or_else(|| "http://localhost:8693".into()),
            ),
            None => (
                env("MUSICBOT_DISCORD_CLIENT_ID"),
                env("MUSICBOT_DISCORD_CLIENT_SECRET"),
                env("MUSICBOT_PUBLIC_BASE_URL").unwrap_or_else(|| "http://localhost:8693".into()),
            ),
        };
        Self {
            client_id,
            client_secret,
            public_base_url: public_base_url.trim_end_matches('/').to_string(),
            dev_login: std::env::var("MUSICBOT_DEV_LOGIN").ok().as_deref() == Some("1"),
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
        }
    }

    pub fn save(&self, data_root: &FsPath) -> Result<(), String> {
        std::fs::create_dir_all(data_root)
            .map_err(|error| format!("OAuth 설정 폴더 생성 실패: {error}"))?;
        let stored = StoredRemoteAuthConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            public_base_url: Some(self.public_base_url.clone()),
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

#[derive(Debug, Clone)]
pub struct RemoteSession {
    pub user_id: u64,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub guilds: Vec<OAuthGuild>,
    pub access_token: String,
    pub csrf_token: String,
    pub created: Instant,
    pub token_expires: Instant,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEvent {
    pub guild_id: u64,
    pub topic: String,
    pub emitted_utc: String,
}

pub fn router() -> Router<Arc<WebState>> {
    Router::new()
        .route("/music", get(portal_home))
        .route("/music/login", get(login_page))
        .route("/music/oauth/start", get(oauth_start))
        .route("/music/oauth/callback", get(oauth_callback))
        .route("/music/dev-login", post(dev_login))
        .route("/music/logout", post(remote_logout))
        .route("/music/guilds/{guild_id}", get(guild_page))
        .route("/music/api/guilds/{guild_id}/state", get(api_state))
        .route("/music/api/guilds/{guild_id}/search", get(api_search))
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
        .route("/music/api/guilds/{guild_id}/chat", post(api_chat))
        .route(
            "/music/api/guilds/{guild_id}/chat/reaction",
            post(api_chat_reaction),
        )
        .route(
            "/music/api/guilds/{guild_id}/chat/delete",
            post(api_chat_delete),
        )
        .route(
            "/music/api/guilds/{guild_id}/chat/report",
            post(api_chat_report),
        )
        .route("/music/api/guilds/{guild_id}/lyrics", get(api_lyrics))
        .route("/music/api/guilds/{guild_id}/settings", post(api_settings))
        .route("/music/api/guilds/{guild_id}/events", get(api_events))
}

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

fn current_session(state: &WebState, cookies: &Cookies) -> Option<RemoteSession> {
    let token = cookies.get(REMOTE_COOKIE)?.value().to_string();
    let mut sessions = state.remote_sessions.lock().unwrap();
    let expired = sessions
        .get(&token)
        .map(|session| {
            session.created.elapsed() >= REMOTE_SESSION_TTL
                || Instant::now() >= session.token_expires
        })
        .unwrap_or(true);
    if expired {
        sessions.remove(&token);
        None
    } else {
        sessions.get(&token).cloned()
    }
}

fn begin_remote_session(state: &WebState, cookies: &Cookies, session: RemoteSession) {
    let auth = auth_config(state);
    let token = crate::models::uuid_like();
    state
        .remote_sessions
        .lock()
        .unwrap()
        .insert(token.clone(), session);
    let mut cookie = Cookie::new(REMOTE_COOKIE, token);
    cookie.set_path("/music");
    cookie.set_http_only(true);
    cookie.set_same_site(tower_cookies::cookie::SameSite::Lax);
    cookie.set_secure(auth.public_base_url.starts_with("https://"));
    cookies.add(cookie);
}

fn end_remote_session(state: &WebState, cookies: &Cookies) {
    if let Some(cookie) = cookies.get(REMOTE_COOKIE) {
        state.remote_sessions.lock().unwrap().remove(cookie.value());
    }
    let mut expired = Cookie::new(REMOTE_COOKIE, "");
    expired.set_path("/music");
    cookies.remove(expired);
}

fn guild_from_session(session: &RemoteSession, guild_id: u64) -> Option<OAuthGuild> {
    session
        .guilds
        .iter()
        .find(|guild| guild.id == guild_id)
        .cloned()
}

fn verify_csrf(session: &RemoteSession, headers: &HeaderMap) -> bool {
    headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        == Some(session.csrf_token.as_str())
}

async fn login_page(
    State(state): State<Arc<WebState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Html<String> {
    let auth = auth_config(&state);
    Html(remote_page::login(
        auth.configured(),
        auth.dev_login,
        query.get("error").map(String::as_str),
    ))
}

async fn portal_home(State(state): State<Arc<WebState>>, cookies: Cookies) -> Response {
    let Some(session) = current_session(&state, &cookies) else {
        return Redirect::to("/music/login").into_response();
    };
    let guilds: Vec<_> = session
        .guilds
        .iter()
        .filter(|guild| {
            session.is_developer
                || state
                    .app
                    .discord_cache
                    .get()
                    .map(|cache| cache.guild(GuildId::new(guild.id)).is_some())
                    .unwrap_or(false)
        })
        .cloned()
        .collect();
    Html(remote_page::guild_selector(&session, &guilds)).into_response()
}

async fn guild_page(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let Some(session) = current_session(&state, &cookies) else {
        return Redirect::to("/music/login").into_response();
    };
    let Some(guild) = guild_from_session(&session, guild_id) else {
        return (
            StatusCode::FORBIDDEN,
            "이 Discord 서버에 접근할 수 없습니다.",
        )
            .into_response();
    };
    if !session.is_developer
        && state
            .app
            .discord_cache
            .get()
            .and_then(|cache| cache.guild(GuildId::new(guild_id)))
            .is_none()
    {
        return (StatusCode::FORBIDDEN, "봇이 이 Discord 서버에 없습니다.").into_response();
    }
    Html(remote_page::guild(&session, &guild)).into_response()
}

async fn oauth_start(State(state): State<Arc<WebState>>) -> Response {
    let auth = auth_config(&state);
    let Some(client_id) = auth.client_id.as_deref() else {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
    };
    if !auth.has_client_secret() {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
    }
    let oauth_state = crate::models::uuid_like();
    state
        .oauth_states
        .lock()
        .unwrap()
        .insert(oauth_state.clone(), Instant::now());
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
    Query(query): Query<OAuthCallbackQuery>,
) -> Response {
    let auth = auth_config(&state);
    if let Some(error) = query.error {
        return Html(remote_page::login(
            auth.configured(),
            auth.dev_login,
            Some(&format!("Discord 로그인이 취소되었습니다: {error}")),
        ))
        .into_response();
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return Redirect::to("/music/login?error=OAuth%20응답이%20올바르지%20않습니다")
            .into_response();
    };
    let issued = state.oauth_states.lock().unwrap().remove(&returned_state);
    if !matches!(issued, Some(instant) if instant.elapsed() < OAUTH_STATE_TTL) {
        return Redirect::to("/music/login?error=OAuth%20state가%20만료되었습니다").into_response();
    }
    let Some(client_id) = auth.client_id.clone() else {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
    };
    let Some(client_secret) = auth.client_secret.clone() else {
        return Redirect::to("/music/login?error=OAuth%20설정이%20필요합니다").into_response();
    };
    let client = reqwest::Client::new();
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
    let user_id = match user.id.parse::<u64>() {
        Ok(id) => id,
        Err(_) => {
            return Redirect::to(
                "/music/login?error=Discord%20사용자%20ID가%20올바르지%20않습니다",
            )
            .into_response();
        }
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
        RemoteSession {
            user_id,
            username: user.username.clone(),
            display_name: user.global_name.unwrap_or(user.username),
            avatar_url,
            guilds,
            access_token: token.access_token,
            csrf_token: crate::models::uuid_like(),
            created: Instant::now(),
            token_expires: Instant::now()
                + Duration::from_secs(token.expires_in.saturating_sub(60)),
            is_developer: false,
        },
    );
    Redirect::to("/music").into_response()
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
            "Discord API가 요청을 거부했습니다 ({})",
            response.status()
        ));
    }
    response
        .json::<T>()
        .await
        .map_err(|error| format!("Discord API 응답 해석 실패: {error}"))
}

async fn dev_login(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Response {
    if !auth_config(&state).dev_login || !address.ip().is_loopback() {
        return StatusCode::NOT_FOUND.into_response();
    }
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
        if let Some(id) = state.app.config.register_guild_id {
            guilds.push(OAuthGuild {
                id,
                name: "로컬 검증 서버".into(),
                icon: None,
                owner: true,
                permissions: ADMINISTRATOR_PERMISSION,
            });
        } else {
            guilds.push(OAuthGuild {
                id: 1,
                name: "마참뮤직 UI 검증 서버".into(),
                icon: None,
                owner: true,
                permissions: ADMINISTRATOR_PERMISSION,
            });
        }
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
        RemoteSession {
            user_id,
            username: "local-tester".into(),
            display_name: "로컬 검증자".into(),
            avatar_url: None,
            guilds,
            access_token: String::new(),
            csrf_token: crate::models::uuid_like(),
            created: Instant::now(),
            token_expires: Instant::now() + REMOTE_SESSION_TTL,
            is_developer: true,
        },
    );
    Redirect::to("/music").into_response()
}

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
    let _ =
        state
            .app
            .remote
            .add_chat_message(guild_id, 2001, "민서", None, "다음 곡 분위기 좋네요 🎧");
    let _ = state.app.remote.add_chat_message(
        guild_id,
        user_id,
        "로컬 검증자",
        None,
        "마참뮤직 리모컨 동작 확인 중입니다.",
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
        fetched_utc: chrono::Utc::now().to_rfc3339(),
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
    if form.csrf != session.csrf_token {
        return json_error(StatusCode::FORBIDDEN, "CSRF 검증에 실패했습니다.");
    }
    end_remote_session(&state, &cookies);
    Redirect::to("/music/login").into_response()
}

#[derive(Default)]
struct MemberContext {
    is_admin: bool,
    same_voice_channel: bool,
    role_ids: Vec<u64>,
}

#[derive(Debug, Deserialize)]
struct DiscordMemberResponse {
    #[serde(default)]
    roles: Vec<String>,
}

async fn member_context(
    state: &WebState,
    session: &RemoteSession,
    guild: &OAuthGuild,
    fresh: bool,
) -> Result<MemberContext, String> {
    if session.is_developer {
        return Ok(MemberContext {
            is_admin: true,
            same_voice_channel: true,
            role_ids: Vec::new(),
        });
    }
    let cache_key = (guild.id, session.user_id);
    let cached = state
        .remote_member_roles
        .lock()
        .unwrap()
        .get(&cache_key)
        .filter(|(seen, _)| !fresh && seen.elapsed() < Duration::from_secs(60))
        .map(|(_, roles)| roles.clone());
    let role_ids = if let Some(roles) = cached {
        roles
    } else {
        let path = format!("/users/@me/guilds/{}/member", guild.id);
        let member = discord_get::<DiscordMemberResponse>(
            &reqwest::Client::new(),
            &session.access_token,
            &path,
        )
        .await?;
        let roles: Vec<u64> = member
            .roles
            .into_iter()
            .filter_map(|role| role.parse().ok())
            .collect();
        state
            .remote_member_roles
            .lock()
            .unwrap()
            .insert(cache_key, (Instant::now(), roles.clone()));
        roles
    };
    let player_state = state.app.player.get_state(guild.id).await;
    let same_voice_channel = state
        .app
        .discord_cache
        .get()
        .and_then(|cache| cache.guild(GuildId::new(guild.id)))
        .and_then(|cached_guild| {
            cached_guild
                .voice_states
                .get(&UserId::new(session.user_id))
                .and_then(|voice| voice.channel_id)
        })
        .map(|channel| Some(channel.get()) == player_state.voice_channel_id)
        .unwrap_or(false);
    Ok(MemberContext {
        is_admin: guild.is_admin(),
        same_voice_channel,
        role_ids,
    })
}

fn permission_allowed(
    rule: PermissionRule,
    settings: &RemoteGuildSettings,
    member: &MemberContext,
) -> bool {
    if member.is_admin {
        return true;
    }
    match rule {
        PermissionRule::GuildMember => true,
        PermissionRule::SameVoiceChannel => member.same_voice_channel,
        PermissionRule::ConfiguredRole => member
            .role_ids
            .iter()
            .any(|role| settings.configured_role_ids.contains(role)),
        PermissionRule::Administrator | PermissionRule::Disabled => false,
    }
}

fn is_manager(settings: &RemoteGuildSettings, member: &MemberContext) -> bool {
    member.is_admin
        || member
            .role_ids
            .iter()
            .any(|role| settings.configured_role_ids.contains(role))
}

async fn authorize(
    state: &WebState,
    cookies: &Cookies,
    guild_id: u64,
    headers: Option<&HeaderMap>,
    rule: Option<PermissionRule>,
) -> Result<
    (
        RemoteSession,
        OAuthGuild,
        RemoteGuildSettings,
        MemberContext,
    ),
    Response,
> {
    let session = current_session(state, cookies)
        .ok_or_else(|| json_error(StatusCode::UNAUTHORIZED, "Discord 로그인이 필요합니다."))?;
    if let Some(headers) = headers {
        if !verify_csrf(&session, headers) {
            return Err(json_error(
                StatusCode::FORBIDDEN,
                "CSRF 검증에 실패했습니다.",
            ));
        }
    }
    let guild = guild_from_session(&session, guild_id)
        .ok_or_else(|| json_error(StatusCode::FORBIDDEN, "이 서버의 멤버가 아닙니다."))?;
    if !session.is_developer
        && state
            .app
            .discord_cache
            .get()
            .and_then(|cache| cache.guild(GuildId::new(guild_id)))
            .is_none()
    {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "봇이 이 Discord 서버에 없습니다.",
        ));
    }
    let settings = state.app.remote.load_guild_settings(guild_id);
    let member = member_context(state, &session, &guild, headers.is_some())
        .await
        .map_err(|error| json_error(StatusCode::FORBIDDEN, error))?;
    if let Some(rule) = rule {
        if !permission_allowed(rule, &settings, &member) {
            return Err(json_error(
                StatusCode::FORBIDDEN,
                "현재 역할 또는 음성 채널에서는 이 기능을 사용할 수 없습니다.",
            ));
        }
    }
    Ok((session, guild, settings, member))
}

fn broadcast(state: &WebState, guild_id: u64, topic: &str) {
    let _ = state.remote_events.send(RemoteEvent {
        guild_id,
        topic: topic.into(),
        emitted_utc: chrono::Utc::now().to_rfc3339(),
    });
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

async fn api_state(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
) -> Response {
    let (session, guild, settings, member) =
        match authorize(&state, &cookies, guild_id, None, None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    let player = state.app.player.get_state(guild_id).await;
    let bot_online = session.is_developer
        || state
            .app
            .discord_cache
            .get()
            .and_then(|cache| cache.guild(GuildId::new(guild_id)))
            .is_some();
    let voice_connected = player.voice_channel_id.is_some();
    let scores = state.app.remote.queue_scores(guild_id);
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
    let queue: Vec<Value> = player
        .upcoming
        .iter()
        .map(|item| {
            let score = scores.get(&item.id).cloned().unwrap_or_default();
            json!({
                "id": item.id,
                "track": item.track,
                "requestedByDisplay": item.requested_by_display,
                "requestedByUserId": item.requested_by_user_id,
                "isMine": item.requested_by_user_id == Some(session.user_id),
                "myVote": state.app.remote.user_vote(&item.id, session.user_id),
                "score": {
                    "waitScore": score.wait_score,
                    "likeCount": score.like_count,
                    "superLikeCount": score.super_like_count,
                    "manualPriority": score.manual_priority,
                    "totalScore": score.total_score(),
                }
            })
        })
        .collect();
    let current = player.current_item.as_ref().map(|item| {
        json!({
            "id": item.id,
            "track": item.track,
            "requestedByDisplay": item.requested_by_display,
            "requestedByUserId": item.requested_by_user_id,
            "durationSeconds": item.track.duration.map(|duration| duration.as_secs_f64()),
        })
    });
    let chat: Vec<Value> = state
        .app
        .remote
        .list_chat_messages(guild_id, session.user_id, 100)
        .into_iter()
        .map(|message| {
            json!({
                "id": message.id,
                "userId": message.user_id,
                "displayName": message.display_name,
                "avatarUrl": message.avatar_url,
                "content": message.content,
                "createdUtc": message.created_utc,
                "deletedUtc": message.deleted_utc,
                "reactions": message.reactions,
                "isMine": message.user_id == session.user_id,
            })
        })
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
                "ownerUserId": playlist.owner_user_id,
                "isMine": playlist.owner_user_id == session.user_id,
                "entryCount": playlist.entries.len(),
                "entries": playlist.entries,
            })
        })
        .collect();
    let manager = is_manager(&settings, &member);
    let permissions = json!({
        "admin": manager,
        "search": permission_allowed(settings.search_rule, &settings, &member),
        "vote": permission_allowed(settings.vote_rule, &settings, &member),
        "chat": settings.chat_enabled && permission_allowed(settings.chat_rule, &settings, &member),
        "playback": permission_allowed(settings.playback_rule, &settings, &member),
        "seek": permission_allowed(settings.seek_rule, &settings, &member),
        "volume": permission_allowed(settings.volume_rule, &settings, &member),
        "queueEdit": permission_allowed(settings.queue_edit_rule, &settings, &member),
        "playlistEdit": manager || permission_allowed(settings.queue_edit_rule, &settings, &member),
    });
    json_ok(json!({
        "guild": guild,
        "user": { "id": session.user_id, "displayName": session.display_name, "avatarUrl": session.avatar_url },
        "player": {
            "voiceChannelId": player.voice_channel_id,
            "isPaused": player.is_paused,
            "effectiveVolume": player.effective_volume,
            "repeatMode": player.repeat_mode,
            "autoplayEnabled": player.autoplay_enabled,
        },
        "connection": {
            "botOnline": bot_online,
            "voiceConnected": voice_connected,
        },
        "current": current,
        "positionSeconds": position,
        "queue": queue,
        "recent": state.app.remote.list_recent(guild_id, 50),
        "liked": state.app.remote.list_user_tracks(guild_id, session.user_id, UserTrackKind::Liked),
        "saved": state.app.remote.list_user_tracks(guild_id, session.user_id, UserTrackKind::Saved),
        "playlists": playlists,
        "chat": chat,
        "chatReports": if manager { state.app.remote.list_chat_reports(guild_id, 100) } else { Vec::new() },
        "audit": state.app.remote.list_audit(guild_id, 100),
        "settings": settings,
        "permissions": permissions,
        "serverTimeUtc": chrono::Utc::now().to_rfc3339(),
    }))
}

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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, None, None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !permission_allowed(settings.search_rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "검색 권한이 없습니다.");
    }
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "search",
        Duration::from_millis(600),
    ) {
        return json_error(StatusCode::TOO_MANY_REQUESTS, "검색 요청이 너무 빠릅니다.");
    }
    let input = query.q.trim();
    if input.is_empty() || input.chars().count() > 200 {
        return json_error(StatusCode::BAD_REQUEST, "검색어는 1~200자로 입력하세요.");
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
        .map(|track| {
            json!({
                "durationLabel": track.duration.map(|duration| duration.display()),
                "provider": track.provider,
                "contentId": track.content_id,
                "sourceUrl": track.source_url,
                "title": track.title,
                "artist": track.artist,
                "duration": track.duration,
                "variantKey": track.variant_key,
            })
        })
        .collect();
    json_ok(json!({ "results": values }))
}

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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !permission_allowed(settings.search_rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "대기열 등록 권한이 없습니다.");
    }
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "enqueue",
        Duration::from_millis(350),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "곡 등록 요청이 너무 빠릅니다.",
        );
    }
    if !crate::media::resolver::can_resolve(&request.track.source_url) {
        return json_error(StatusCode::BAD_REQUEST, "지원하지 않는 곡 URL입니다.");
    }
    if let Some(rule) = state
        .app
        .blacklist
        .try_get_blocker(guild_id, &request.track)
    {
        audit_failure(
            &state,
            guild_id,
            &session,
            "queue.add",
            Some(request.track.display_title()),
            "blacklisted",
        );
        return json_error(
            StatusCode::BAD_REQUEST,
            format!(
                "차단 규칙에 의해 등록할 수 없습니다: {}",
                crate::blacklist::Blacklist::describe_rule(&rule)
            ),
        );
    }
    let player = state.app.player.get_state(guild_id).await;
    if request
        .track
        .duration
        .is_some_and(|duration| duration.as_secs_f64() > settings.max_track_seconds as f64)
    {
        audit_failure(
            &state,
            guild_id,
            &session,
            "queue.add",
            Some(request.track.display_title()),
            "track_too_long",
        );
        return json_error(
            StatusCode::BAD_REQUEST,
            format!(
                "허용 곡 길이({}초)를 초과했습니다.",
                settings.max_track_seconds
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
            &session,
            "queue.add",
            Some(request.track.display_title()),
            "duplicate",
        );
        return json_error(
            StatusCode::CONFLICT,
            "이미 현재 곡이나 대기열에 있는 곡입니다.",
        );
    }
    let user_count = player
        .upcoming
        .iter()
        .filter(|item| item.requested_by_user_id == Some(session.user_id))
        .count();
    if player.upcoming.len() >= settings.max_queue_per_guild.max(1) as usize
        || user_count >= settings.max_queue_per_user.max(1) as usize
    {
        audit_failure(
            &state,
            guild_id,
            &session,
            "queue.add",
            Some(request.track.display_title()),
            "queue_limit",
        );
        return json_error(
            StatusCode::CONFLICT,
            "서버 또는 사용자 대기열 제한에 도달했습니다.",
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
    let _ = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        "queue.add",
        Some(&title),
        None,
        Some("queued"),
        true,
        None,
    );
    broadcast(&state, guild_id, "queue");
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
        "initialScore": 0,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRequest {
    action: String,
    value: Option<f64>,
    expected_item_id: Option<String>,
}

async fn api_control(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ControlRequest>,
) -> Response {
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "control",
        Duration::from_millis(350),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "재생 제어 요청이 너무 빠릅니다.",
        );
    }
    let rule = match request.action.as_str() {
        "seek" => settings.seek_rule,
        "volume" => settings.volume_rule,
        _ => settings.playback_rule,
    };
    if !permission_allowed(rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "재생 제어 권한이 없습니다.");
    }
    if state
        .app
        .player
        .get_state(guild_id)
        .await
        .voice_channel_id
        .is_none()
    {
        return json_error(
            StatusCode::CONFLICT,
            "봇이 음성 채널에 연결되어 있지 않습니다.",
        );
    }
    if matches!(request.action.as_str(), "skip" | "seek") {
        let current_id = state
            .app
            .player
            .get_state(guild_id)
            .await
            .current_item
            .map(|item| item.id);
        if request.expected_item_id.as_deref() != current_id.as_deref() {
            return json_error(
                StatusCode::CONFLICT,
                "재생 상태가 이미 바뀌었습니다. 최신 상태를 다시 확인해 주세요.",
            );
        }
    }
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
            Ok("skipped".into())
        }
        "seek" => {
            let seconds = request.value.unwrap_or(-1.0);
            let player = state.app.player.get_state(guild_id).await;
            let duration = player
                .current_item
                .as_ref()
                .and_then(|item| item.track.duration)
                .map(|duration| duration.as_secs_f64())
                .unwrap_or(0.0);
            if seconds < 0.0 || duration <= 0.0 || seconds > duration {
                Err("탐색 위치가 곡 길이를 벗어났습니다.".into())
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
            if volume < settings.min_volume || volume > settings.max_volume {
                Err(format!(
                    "볼륨은 {}~{} 사이여야 합니다.",
                    settings.min_volume, settings.max_volume
                ))
            } else {
                state.app.player.set_volume(guild_id, volume).await;
                if !session.is_developer {
                    state.app.coordinator.apply_volume(guild_id, volume).await;
                }
                Ok(format!("volume:{volume}"))
            }
        }
        _ => Err("지원하지 않는 재생 제어입니다.".into()),
    };
    match result {
        Ok(after) => {
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                &format!("playback.{}", request.action),
                None,
                None,
                Some(&after),
                true,
                None,
            );
            broadcast(&state, guild_id, "playback");
            json_ok(json!({ "ok": true }))
        }
        Err(error) => {
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                &format!("playback.{}", request.action),
                None,
                None,
                None,
                false,
                Some(&error),
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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !permission_allowed(settings.vote_rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "투표 권한이 없습니다.");
    }
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "vote",
        Duration::from_millis(250),
    ) {
        return json_error(StatusCode::TOO_MANY_REQUESTS, "투표 요청이 너무 빠릅니다.");
    }
    let player = state.app.player.get_state(guild_id).await;
    let Some(item) = player
        .upcoming
        .iter()
        .find(|item| item.id == request.item_id)
    else {
        return json_error(StatusCode::NOT_FOUND, "대기열 항목을 찾을 수 없습니다.");
    };
    if item.requested_by_user_id == Some(session.user_id) {
        return json_error(
            StatusCode::FORBIDDEN,
            "자신이 신청한 곡에는 투표할 수 없습니다.",
        );
    }
    if let Err(error) = state.app.remote.set_vote(
        guild_id,
        &item.id,
        session.user_id,
        request.kind,
        &item.track,
    ) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    state.app.player.refresh_scored_order(guild_id).await;
    let _ = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        "queue.vote",
        Some(item.track.display_title()),
        None,
        request.kind.map(QueueVoteKind::as_str),
        true,
        None,
    );
    broadcast(&state, guild_id, "vote");
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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    let player = state.app.player.get_state(guild_id).await;
    let Some(index) = player
        .upcoming
        .iter()
        .position(|item| item.id == request.item_id)
    else {
        return json_error(StatusCode::NOT_FOUND, "대기열 항목을 찾을 수 없습니다.");
    };
    let item = &player.upcoming[index];
    match request.action.as_str() {
        "remove" => {
            let own = item.requested_by_user_id == Some(session.user_id);
            if !own
                && !is_manager(&settings, &member)
                && !permission_allowed(settings.queue_edit_rule, &settings, &member)
            {
                return json_error(StatusCode::FORBIDDEN, "이 곡을 제거할 권한이 없습니다.");
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
                    "대기열이 이미 바뀌었습니다. 최신 상태를 다시 확인해 주세요.",
                );
            }
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                "queue.remove",
                Some(&title),
                None,
                Some("removed"),
                true,
                None,
            );
        }
        "togglePin" => {
            if !is_manager(&settings, &member) {
                return json_error(StatusCode::FORBIDDEN, "관리자만 강제 이동할 수 있습니다.");
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
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                "queue.force_move",
                Some(item.track.display_title()),
                None,
                Some(if new_priority.is_some() {
                    "pinned"
                } else {
                    "unpinned"
                }),
                true,
                None,
            );
        }
        _ => return json_error(StatusCode::BAD_REQUEST, "지원하지 않는 큐 작업입니다."),
    }
    broadcast(&state, guild_id, "queue");
    json_ok(json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryRequest {
    track: TrackRef,
    kind: UserTrackKind,
    present: bool,
}

async fn api_library(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<LibraryRequest>,
) -> Response {
    let (session, _, _, _) = match authorize(&state, &cookies, guild_id, Some(&headers), None).await
    {
        Ok(context) => context,
        Err(response) => return response,
    };
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "library",
        Duration::from_millis(250),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "보관함 요청이 너무 빠릅니다.",
        );
    }
    if let Err(error) = state.app.remote.set_user_track(
        guild_id,
        session.user_id,
        request.kind,
        &request.track,
        request.present,
    ) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let _ = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        "library.change",
        Some(request.track.display_title()),
        None,
        Some(if request.present { "saved" } else { "removed" }),
        true,
        None,
    );
    broadcast(&state, guild_id, "library");
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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if rate_limited(
        &state,
        guild_id,
        session.user_id,
        "playlist",
        Duration::from_millis(300),
    ) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "재생목록 요청이 너무 빠릅니다.",
        );
    }
    if !is_manager(&settings, &member)
        && !permission_allowed(settings.queue_edit_rule, &settings, &member)
    {
        return json_error(StatusCode::FORBIDDEN, "재생목록을 편집할 권한이 없습니다.");
    }
    let name = request.name.as_deref().map(str::trim).unwrap_or("");
    let target = request
        .playlist_id
        .and_then(|id| state.app.db.find_playlist(id));
    if let Some(playlist) = target.as_ref() {
        if playlist.scope != PlaylistScope::Guild || playlist.guild_id != Some(guild_id) {
            return json_error(StatusCode::NOT_FOUND, "이 서버의 재생목록이 아닙니다.");
        }
        if playlist.owner_user_id != session.user_id && !is_manager(&settings, &member) {
            return json_error(
                StatusCode::FORBIDDEN,
                "소유자나 관리자만 수정할 수 있습니다.",
            );
        }
    }
    let audit_target = match request.action.as_str() {
        "create" => {
            if name.is_empty() || name.chars().count() > 80 {
                return json_error(StatusCode::BAD_REQUEST, "이름은 1~80자로 입력하세요.");
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
                return json_error(StatusCode::NOT_FOUND, "재생목록을 찾을 수 없습니다.");
            };
            if name.is_empty() || name.chars().count() > 80 {
                return json_error(StatusCode::BAD_REQUEST, "이름은 1~80자로 입력하세요.");
            }
            if !state.app.db.rename_playlist(playlist.id, name) {
                return json_error(StatusCode::CONFLICT, "이름을 변경하지 못했습니다.");
            }
            format!("{}:{name}", playlist.id)
        }
        "delete" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "재생목록을 찾을 수 없습니다.");
            };
            if !state.app.db.delete_playlist(playlist.id) {
                return json_error(StatusCode::CONFLICT, "재생목록을 삭제하지 못했습니다.");
            }
            format!("{}:{}", playlist.id, playlist.name)
        }
        "addTrack" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "재생목록을 찾을 수 없습니다.");
            };
            let Some(track) = request.track else {
                return json_error(StatusCode::BAD_REQUEST, "추가할 곡이 없습니다.");
            };
            if track
                .duration
                .is_some_and(|duration| duration.as_secs_f64() > settings.max_track_seconds as f64)
                || state.app.blacklist.is_blocked(guild_id, &track)
            {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "곡 길이 또는 차단 규칙 때문에 추가할 수 없습니다.",
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
                return json_error(StatusCode::NOT_FOUND, "재생목록을 찾을 수 없습니다.");
            };
            let Some(index) = request.entry_index else {
                return json_error(StatusCode::BAD_REQUEST, "삭제할 곡 순서가 없습니다.");
            };
            if !state.app.db.remove_playlist_entry(playlist.id, index) {
                return json_error(StatusCode::NOT_FOUND, "재생목록 곡을 찾을 수 없습니다.");
            }
            format!("{}:{index}", playlist.id)
        }
        "enqueue" => {
            let Some(playlist) = target else {
                return json_error(StatusCode::NOT_FOUND, "재생목록을 찾을 수 없습니다.");
            };
            let tracks: Vec<TrackRef> = playlist
                .entries
                .iter()
                .filter_map(|e| e.track.clone())
                .collect();
            if tracks.is_empty() {
                return json_error(
                    StatusCode::CONFLICT,
                    "재생목록에 등록 가능한 곡이 없습니다.",
                );
            }
            if tracks.iter().any(|track| {
                track.duration.is_some_and(|duration| {
                    duration.as_secs_f64() > settings.max_track_seconds as f64
                }) || state.app.blacklist.is_blocked(guild_id, track)
            }) {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "재생목록에 길이 제한을 넘거나 차단된 곡이 있습니다.",
                );
            }
            let player = state.app.player.get_state(guild_id).await;
            let existing: std::collections::HashSet<String> = player
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
                    "재생목록 곡 중 하나가 이미 현재 곡이나 대기열에 있습니다.",
                );
            }
            let own = player
                .upcoming
                .iter()
                .filter(|item| item.requested_by_user_id == Some(session.user_id))
                .count();
            if player.upcoming.len() + tracks.len() > settings.max_queue_per_guild.max(1) as usize
                || own + tracks.len() > settings.max_queue_per_user.max(1) as usize
            {
                return json_error(StatusCode::CONFLICT, "대기열 제한을 초과합니다.");
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
                "지원하지 않는 재생목록 작업입니다.",
            );
        }
    };
    let _ = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        &format!("playlist.{}", request.action),
        Some(&audit_target),
        None,
        Some("ok"),
        true,
        None,
    );
    broadcast(&state, guild_id, "playlists");
    json_ok(json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    content: String,
}

async fn api_chat(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Response {
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !settings.chat_enabled || !permission_allowed(settings.chat_rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "채팅 권한이 없습니다.");
    }
    let content = request.content.trim();
    if content.is_empty() || content.chars().count() > 2000 {
        return json_error(StatusCode::BAD_REQUEST, "메시지는 1~2000자로 입력하세요.");
    }
    {
        let mut rate = state.remote_chat_rate.lock().unwrap();
        if rate
            .get(&(guild_id, session.user_id))
            .map(|last| last.elapsed() < Duration::from_millis(800))
            .unwrap_or(false)
        {
            return json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "메시지를 너무 빠르게 보내고 있습니다.",
            );
        }
        rate.insert((guild_id, session.user_id), Instant::now());
    }
    match state.app.remote.add_chat_message(
        guild_id,
        session.user_id,
        &session.display_name,
        session.avatar_url.as_deref(),
        content,
    ) {
        Ok(id) => {
            broadcast(&state, guild_id, "chat");
            json_ok(json!({ "ok": true, "id": id }))
        }
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !settings.chat_enabled || !permission_allowed(settings.chat_rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "채팅 권한이 없습니다.");
    }
    if request.emoji.is_empty() || request.emoji.chars().count() > 8 {
        return json_error(StatusCode::BAD_REQUEST, "이모지가 올바르지 않습니다.");
    }
    match state.app.remote.toggle_chat_reaction(
        guild_id,
        request.message_id,
        session.user_id,
        &request.emoji,
    ) {
        Ok(active) => {
            broadcast(&state, guild_id, "chat");
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

async fn api_chat_delete(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<ChatDeleteRequest>,
) -> Response {
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    let owner = state
        .app
        .remote
        .chat_message_owner(guild_id, request.message_id);
    if owner != Some(session.user_id) && !is_manager(&settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "이 메시지를 삭제할 권한이 없습니다.");
    }
    match state
        .app
        .remote
        .delete_chat_message(guild_id, request.message_id)
    {
        Ok(true) => {
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                "chat.delete",
                Some(&request.message_id.to_string()),
                None,
                Some("deleted"),
                true,
                None,
            );
            broadcast(&state, guild_id, "chat");
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "메시지를 찾을 수 없습니다."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
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
    let (session, _, settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !settings.chat_enabled || !permission_allowed(settings.chat_rule, &settings, &member) {
        return json_error(StatusCode::FORBIDDEN, "채팅 권한이 없습니다.");
    }
    if request.resolve.unwrap_or(false) {
        if !is_manager(&settings, &member) {
            return json_error(StatusCode::FORBIDDEN, "관리자만 신고를 처리할 수 있습니다.");
        }
        let Some(report_id) = request.report_id else {
            return json_error(StatusCode::BAD_REQUEST, "신고 ID가 없습니다.");
        };
        return match state.app.remote.resolve_chat_report(guild_id, report_id) {
            Ok(true) => {
                let _ = state.app.remote.add_audit(
                    guild_id,
                    session.user_id,
                    &session.display_name,
                    "chat.report.resolve",
                    Some(&report_id.to_string()),
                    None,
                    Some("resolved"),
                    true,
                    None,
                );
                broadcast(&state, guild_id, "chat-report");
                json_ok(json!({ "ok": true }))
            }
            Ok(false) => json_error(StatusCode::NOT_FOUND, "신고를 찾을 수 없습니다."),
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
    }
    let reason = request.reason.trim();
    if reason.is_empty() || reason.chars().count() > 300 {
        return json_error(StatusCode::BAD_REQUEST, "신고 사유는 1~300자로 입력하세요.");
    }
    if state
        .app
        .remote
        .chat_message_owner(guild_id, request.message_id)
        == Some(session.user_id)
    {
        return json_error(StatusCode::FORBIDDEN, "자신의 메시지는 신고할 수 없습니다.");
    }
    match state.app.remote.report_chat_message(
        guild_id,
        request.message_id,
        session.user_id,
        &session.display_name,
        reason,
    ) {
        Ok(true) => {
            let _ = state.app.remote.add_audit(
                guild_id,
                session.user_id,
                &session.display_name,
                "chat.report",
                Some(&request.message_id.to_string()),
                None,
                Some(reason),
                true,
                None,
            );
            broadcast(&state, guild_id, "chat-report");
            json_ok(json!({ "ok": true }))
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, "메시지를 찾을 수 없습니다."),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

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
    let (session, _, mut settings, member) =
        match authorize(&state, &cookies, guild_id, Some(&headers), None).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    if !is_manager(&settings, &member) {
        return json_error(
            StatusCode::FORBIDDEN,
            "서버 관리자만 설정을 변경할 수 있습니다.",
        );
    }
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
        return json_error(StatusCode::BAD_REQUEST, "볼륨 범위가 올바르지 않습니다.");
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
    settings.configured_role_ids = request.configured_role_ids;
    settings.max_queue_per_user = request.max_queue_per_user;
    settings.max_queue_per_guild = request.max_queue_per_guild;
    settings.max_track_seconds = request.max_track_seconds;
    settings.audit_retention_days = request.audit_retention_days;
    if let Err(error) = state.app.remote.save_guild_settings(&settings) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let mut engine_settings = state.app.db.load_guild_settings(guild_id);
    engine_settings.volume_override = Some(settings.default_volume);
    state.app.db.save_guild_settings(&engine_settings);
    let applied = state.app.player.apply_configured_settings(guild_id).await;
    if !session.is_developer {
        state
            .app
            .coordinator
            .apply_volume(guild_id, applied.effective_volume)
            .await;
    }
    let after = serde_json::to_string(&settings).unwrap_or_default();
    let _ = state.app.remote.add_audit(
        guild_id,
        session.user_id,
        &session.display_name,
        "settings.update",
        None,
        Some(&before),
        Some(&after),
        true,
        None,
    );
    broadcast(&state, guild_id, "settings");
    let _ = state
        .app
        .remote
        .prune_audit(guild_id, settings.audit_retention_days);
    json_ok(json!({ "ok": true }))
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
    if let Err(response) = authorize(&state, &cookies, guild_id, None, None).await {
        return response;
    }
    let player = state.app.player.get_state(guild_id).await;
    let Some(item) = player.current_item else {
        return json_error(StatusCode::NOT_FOUND, "현재 재생 중인 곡이 없습니다.");
    };
    let cache_key = item.track.cache_key();
    if let Some(lyrics) = state.app.remote.load_lyrics(&cache_key) {
        return Json(lyrics).into_response();
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
    let client = match reqwest::Client::builder()
        .user_agent(format!(
            "mc-musicbot/{} (https://musicbot.example.com)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
    {
        Ok(client) => client,
        Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let mut request = client.get("https://lrclib.net/api/search");
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
    let lyrics = match row {
        Some(row) => LyricsDocument {
            cache_key,
            plain_text: row.plain_lyrics,
            synced_lines: row
                .synced_lyrics
                .as_deref()
                .map(parse_lrc)
                .unwrap_or_default(),
            source: "LRCLIB".into(),
            fetched_utc: chrono::Utc::now().to_rfc3339(),
        },
        None => LyricsDocument {
            cache_key,
            plain_text: None,
            synced_lines: Vec::new(),
            source: "LRCLIB".into(),
            fetched_utc: chrono::Utc::now().to_rfc3339(),
        },
    };
    let _ = state.app.remote.save_lyrics(&lyrics);
    Json(lyrics).into_response()
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

async fn api_events(
    State(state): State<Arc<WebState>>,
    cookies: Cookies,
    Path(guild_id): Path<u64>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(session) = current_session(&state, &cookies) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if guild_from_session(&session, guild_id).is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let receiver = state.remote_events.subscribe();
    ws.on_upgrade(move |socket| websocket_loop(socket, receiver, guild_id))
}

async fn websocket_loop(
    mut socket: WebSocket,
    mut receiver: tokio::sync::broadcast::Receiver<RemoteEvent>,
    guild_id: u64,
) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let initial = RemoteEvent {
        guild_id,
        topic: "connected".into(),
        emitted_utc: chrono::Utc::now().to_rfc3339(),
    };
    if socket
        .send(Message::Text(
            serde_json::to_string(&initial).unwrap_or_default().into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let event = RemoteEvent {
                    guild_id,
                    topic: "sync".into(),
                    emitted_utc: chrono::Utc::now().to_rfc3339(),
                };
                let payload = serde_json::to_string(&event).unwrap_or_default();
                if socket.send(Message::Text(payload.into())).await.is_err() { break; }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Ping(data))) => {
                    if socket.send(Message::Pong(data)).await.is_err() { break; }
                }
                _ => {}
            },
            event = receiver.recv() => match event {
                Ok(event) if event.guild_id == guild_id => {
                    let payload = serde_json::to_string(&event).unwrap_or_default();
                    if socket.send(Message::Text(payload.into())).await.is_err() { break; }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_parser_supports_fractional_seconds() {
        let lines = parse_lrc("[00:17.12] first\n[03:20.310] second");
        assert_eq!(lines[0].start_ms, 17_120);
        assert_eq!(lines[1].start_ms, 200_310);
        assert_eq!(lines[1].text, "second");
    }

    #[test]
    fn permission_defaults_match_remote_contract() {
        let settings = RemoteGuildSettings::default();
        let member = MemberContext::default();
        assert!(!permission_allowed(
            settings.playback_rule,
            &settings,
            &member
        ));
        assert!(permission_allowed(settings.seek_rule, &settings, &member));
        assert!(!permission_allowed(
            settings.volume_rule,
            &settings,
            &member
        ));
        let same_voice = MemberContext {
            same_voice_channel: true,
            ..Default::default()
        };
        assert!(permission_allowed(
            settings.playback_rule,
            &settings,
            &same_voice
        ));
        assert!(permission_allowed(
            settings.volume_rule,
            &settings,
            &same_voice
        ));
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
        };
        config.save(&root).unwrap();
        let loaded = RemoteAuthConfig::load(&root);
        assert_eq!(loaded.client_id, config.client_id);
        assert_eq!(loaded.client_secret, config.client_secret);
        assert_eq!(loaded.public_base_url, config.public_base_url);
        assert!(!format!("{loaded:?}").contains("unit-test-secret-never-log"));

        let retained = loaded.updated(
            "100000000000000001".into(),
            None,
            false,
            "https://musicbot.example.test/".into(),
        );
        assert!(retained.has_client_secret());
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
