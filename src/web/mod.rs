//! 웹 관리 UI — axum 기반. 기본 포트 8693.
//! 인증: 최초 접속 시 localhost 에서 비밀번호 설정 → SHA-256 해시를 data 디렉터리에 저장.
//! `MUSICBOT_WEB_PASSWORD` 환경변수가 있으면 그 값으로 오버라이드(설정 파일보다 우선).

pub mod assets;
pub mod pages;
pub mod remote;
pub mod remote_page;

use crate::app::App;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::Router;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use sha2::Digest;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tower_cookies::{Cookie, CookieManagerLayer, Cookies};

/// 세션 유효 기간 — 12시간 쿠키.
const SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(12 * 60 * 60);
/// 관리자 패널과 리모컨의 공개 호스트명. **배포마다 다르므로 저장소에 박지 않는다.**
/// `MUSICBOT_ADMIN_HOST` / `MUSICBOT_REMOTE_HOST` 로 기동 시 한 번 읽는다.
///
/// 둘 다 비어 있으면 호스트 분리를 끄고 단일 호스트로 동작한다(로컬 개발 기본값).
/// **운영에서 이 값을 비워 두면 리모컨 도메인에서도 `/botsettings` `/logs` 가 열린다.**
/// 그래서 `spawn_web` 이 기동 로그에 경고를 남긴다.
static ADMIN_HOST: OnceLock<Option<String>> = OnceLock::new();
static REMOTE_HOST: OnceLock<Option<String>> = OnceLock::new();

fn host_from_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| normalize_host(&value))
        .filter(|value| !value.is_empty())
}

/// 포트와 대소문자를 떼어 낸 비교용 호스트명.
fn normalize_host(raw: &str) -> String {
    raw.split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn admin_host() -> Option<&'static str> {
    ADMIN_HOST
        .get_or_init(|| host_from_env("MUSICBOT_ADMIN_HOST"))
        .as_deref()
}

fn remote_host() -> Option<&'static str> {
    REMOTE_HOST
        .get_or_init(|| host_from_env("MUSICBOT_REMOTE_HOST"))
        .as_deref()
}

/// 운영 패널로 가는 주소. **리모컨 화면이 이걸 모르면 링크를 만들 수가 없다.**
///
/// 운영 패널은 리모컨과 **다른 도메인**이라 `/` 같은 상대경로로는 절대 닿지 않는다.
/// 예전에는 링크가 `/` 로 박혀 있었고, 리모컨 도메인의 `/` 는 `host_scope_guard` 가
/// `/music` 으로 되돌리므로 **누르면 방금 보던 화면으로 돌아왔다.**
///
/// 호스트 분리를 안 켠 로컬 개발에서는 같은 서버의 `/` 가 진짜 운영 패널이라 그대로 둔다.
pub(crate) fn ops_panel_url() -> String {
    build_ops_url(admin_host())
}

fn build_ops_url(admin: Option<&str>) -> String {
    match admin {
        Some(host) => format!("https://{host}/"),
        None => "/".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSurface {
    Admin,
    Remote,
    Internal,
}

/// 호스트 분류의 순수 함수 부분. 환경변수를 읽지 않으므로 테스트가 값을 직접 준다.
fn classify_host(host: Option<&str>, admin: Option<&str>, remote: Option<&str>) -> WebSurface {
    let host = normalize_host(host.unwrap_or_default());
    if host.is_empty() {
        return WebSurface::Internal;
    }
    if admin == Some(host.as_str()) {
        WebSurface::Admin
    } else if remote == Some(host.as_str()) {
        WebSurface::Remote
    } else {
        WebSurface::Internal
    }
}

fn web_surface(host: Option<&str>) -> WebSurface {
    classify_host(host, admin_host(), remote_host())
}

fn is_remote_path(path: &str) -> bool {
    path == "/music" || path.starts_with("/music/")
}

async fn host_scope_guard(request: Request<Body>, next: Next) -> Response {
    let surface = web_surface(
        request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
    );
    let path = request.uri().path();
    match surface {
        WebSurface::Remote if path == "/" => Redirect::temporary("/music").into_response(),
        WebSurface::Remote if !is_remote_path(path) && path != "/healthz" => {
            (StatusCode::NOT_FOUND, "리모컨 도메인에서는 이 경로를 제공하지 않습니다.")
                .into_response()
        }
        // 리모컨 호스트가 설정되어 있을 때만 넘긴다. 없으면 보낼 곳이 없으므로 그대로 처리한다.
        WebSurface::Admin if is_remote_path(path) && remote_host().is_some() => {
            let remote = remote_host().unwrap_or_default();
            let suffix = request
                .uri()
                .path_and_query()
                .map(|value| value.as_str())
                .unwrap_or(path);
            Redirect::temporary(&format!("https://{remote}{suffix}")).into_response()
        }
        _ => next.run(request).await,
    }
}

pub struct WebState {
    pub app: Arc<App>,
    /// 웹 비밀번호 SHA-256 해시. None 이면 미설정(최초 설정 필요) 상태.
    pub password_hash: Mutex<Option<[u8; 32]>>,
    /// 최초 비밀번호 설정 폼의 일회성 프로세스 CSRF 토큰.
    pub setup_csrf: String,
    pub sessions: Mutex<HashMap<String, Instant>>,
    /// 운영자 세션별 CSRF 토큰. OAuth 비밀 설정처럼 민감한 POST에 사용한다.
    pub admin_csrf: Mutex<HashMap<String, String>>,
    /// 사용자용 마참뮤직 Discord OAuth 세션.
    pub remote_sessions: Mutex<HashMap<String, remote::RemoteSession>>,
    /// OAuth state 일회용 토큰과 발급 시각.
    /// OAuth state 토큰 → (발급 시각, 로그인 후 돌아갈 내부 경로).
    /// `next` 는 `/music/...` 로 시작하는 값만 저장한다(오픈 리다이렉트 방지).
    pub oauth_states: Mutex<HashMap<String, (Instant, Option<String>)>>,
    /// 길드별 상태 변경을 WebSocket 접속자에게 알리는 브로드캐스트 채널.
    pub remote_events: broadcast::Sender<remote::RemoteEvent>,
    /// 운영자 UI에서 저장하면 프로세스 재시작 없이 교체되는 OAuth 구성.
    pub remote_auth: RwLock<remote::RemoteAuthConfig>,
    /// 길드·사용자별 마지막 채팅 전송 시각(간단한 도배 방지).
    pub remote_chat_rate: Mutex<HashMap<(u64, u64), Instant>>,
    /// Discord 멤버 역할 조회 캐시. 읽기 화면의 2초 동기화가 Discord API를 과호출하지 않게 한다.
    pub remote_member_roles: Mutex<HashMap<(u64, u64), (Instant, Vec<u64>)>>,
    /// 사람별 마지막 **서버 목록 재조회** 시각 (§35). 없는 서버를 계속 두드려도
    /// Discord 조회는 이 간격으로만 나간다.
    pub guild_refresh_at: Mutex<HashMap<u64, Instant>>,
    /// 주요 사용자 동작의 마지막 요청 시각. 연타와 자동화 오용을 완화한다.
    pub remote_action_rate: Mutex<HashMap<(u64, u64, &'static str), Instant>>,
    /// 접속 레지스트리 — `(guild_id, user_id)` → 열려 있는 WebSocket 수. **DB를 쓰지 않는다.**
    pub presence: Mutex<HashMap<(u64, u64), usize>>,
    /// **지금 실제로 웹에서 듣고 있는 사람** — `(guild_id, user_id)`.
    ///
    /// `presence` 로는 알 수 없다. 그건 리모컨 화면을 열어 둔 사람이고, 그중 상당수는
    /// `웹에서 듣기` 를 꺼 둔 채 보기만 한다. 그 사람들까지 세면 아무도 안 듣는데
    /// 대기열이 계속 도는 상태가 된다.
    ///
    /// 개인 설정(`webPlayback`)으로도 알 수 없다. 그건 "켜 두겠다는 뜻" 이 영속화된 값이고,
    /// 새로고침하면 브라우저의 실제 재생 상태(`webOn`)는 항상 false 로 시작해 사용자가
    /// 다시 눌러야 한다. 그래서 **브라우저가 실제로 소리를 내기 시작한 순간** 알려 준다.
    pub web_listeners: Mutex<HashSet<(u64, u64)>>,
    /// 길드별 presence 브로드캐스트 게이트 — `(마지막 송신, 예약됨)`. 최대 초당 1회로 코얼레싱한다.
    pub presence_gate: Mutex<HashMap<u64, (Instant, bool)>>,
    /// 재생 변화를 감시 중인 길드. 보는 사람이 있을 때만 길드당 하나가 돈다.
    pub guild_watchers: Mutex<HashSet<u64>>,
    /// S5: Discord/LRCLIB 호출에 재사용하는 공유 HTTP 클라이언트.
    /// 요청마다 `Client::new()`를 하면 커넥션 풀과 TLS 세션이 매번 버려진다.
    pub http_client: OnceLock<reqwest::Client>,
    /// 투표 스킵 (V3 §10.5). 길드 하나에 진행 중인 투표 하나.
    /// **DB를 쓰지 않는다** — 곡 하나 수명짜리 데이터라 재시작하면 사라지는 게 맞다.
    pub skip_votes: Mutex<HashMap<u64, remote::SkipVoteState>>,
    /// 통계·차트 응답 60초 캐시 (V3 §22.6 · §23.2).
    /// 무거운 집계를 매 요청 돌리지 않으려는 것이지 실시간성을 포기한 게 아니다 —
    /// 통계는 60초 늦어도 아무도 손해 보지 않는다.
    pub stats_cache: Mutex<HashMap<String, (Instant, serde_json::Value)>>,
}

/// 비밀번호 해시 저장 파일 (data 디렉터리, gitignore 대상).
pub fn auth_hash_path(app: &Arc<App>) -> PathBuf {
    app.config.data_root.join("web-auth.hash")
}

fn load_stored_hash(app: &Arc<App>) -> Option<[u8; 32]> {
    let hex = std::fs::read_to_string(auth_hash_path(app)).ok()?;
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// 비밀번호 해시를 저장한다 (최초 설정/변경 시).
pub fn store_hash(app: &Arc<App>, hash: &[u8; 32]) -> std::io::Result<()> {
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all(&app.config.data_root);
    std::fs::write(auth_hash_path(app), hex)
}

pub fn hash_password(pw: &str) -> [u8; 32] {
    sha2::Sha256::digest(pw.as_bytes()).into()
}

pub type Ctx = State<Arc<WebState>>;

const SESSION_COOKIE: &str = "mk2_session";

pub async fn serve(app: Arc<App>) {
    // 비밀번호 해시 결정: 환경변수 오버라이드 > 저장 파일 > 미설정(최초 설정 모드).
    let initial_hash = std::env::var("MUSICBOT_WEB_PASSWORD")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(|p| hash_password(&p))
        .or_else(|| load_stored_hash(&app));
    if initial_hash.is_none() {
        println!(
            "[web] 웹 비밀번호 미설정 — 호스트에서 http://localhost:8693 에 접속해 최초 비밀번호를 설정하세요."
        );
    }
    let (remote_events, _) = broadcast::channel(256);
    let remote_auth = remote::RemoteAuthConfig::load(&app.config.data_root);

    // `/리모컨` 슬래시 명령이 링크를 만들 때 읽는다. 미설정이면 명령이 안내만 한다.
    let _ = app.public_base_url.set(remote_auth.public_base_url.clone());
    // 재생 카드의 "🎛 리모컨" 링크 버튼처럼 `&App` 이 없는 곳에서도 쓸 수 있게 전역에도 둔다.
    crate::app::set_public_base_url(&remote_auth.public_base_url);
    // 봇 주인 판정(AccessTier::Owner)의 근거. 운영 패널에서 저장하면 즉시 갱신된다.
    if let Ok(mut owners) = app.owner_user_ids.write() {
        *owners = remote_auth.owner_user_ids.clone();
    }
    remote::mark_started();

    let state = Arc::new(WebState {
        app: app.clone(),
        password_hash: Mutex::new(initial_hash),
        setup_csrf: crate::models::uuid_like(),
        sessions: Mutex::new(HashMap::new()),
        admin_csrf: Mutex::new(HashMap::new()),
        remote_sessions: Mutex::new(HashMap::new()),
        oauth_states: Mutex::new(HashMap::new()),
        remote_events,
        remote_auth: RwLock::new(remote_auth),
        remote_chat_rate: Mutex::new(HashMap::new()),
        remote_member_roles: Mutex::new(HashMap::new()),
        guild_refresh_at: Mutex::new(HashMap::new()),
        remote_action_rate: Mutex::new(HashMap::new()),
        presence: Mutex::new(HashMap::new()),
        web_listeners: Mutex::new(HashSet::new()),
        presence_gate: Mutex::new(HashMap::new()),
        guild_watchers: Mutex::new(HashSet::new()),
        http_client: OnceLock::new(),
        skip_votes: Mutex::new(HashMap::new()),
        stats_cache: Mutex::new(HashMap::new()),
    });

    spawn_sweeper(state.clone());

    // 5초 재정렬 루프가 순서를 바꾸면 그 결과를 WS로 밀어 준다.
    // 이 훅을 안 걸면 대기열이 조용히 재정렬되기만 하고 화면은 안 움직인다.
    {
        let hook_state = state.clone();
        let _ = app.on_queue_sorted.set(Box::new(move |guild_id| {
            remote::spawn_queue_broadcast(&hook_state, guild_id);
        }));
    }

    // 재시작이 시작되면 접속 중인 모든 창에 알린다 (§24).
    {
        let hook_state = state.clone();
        let _ = app.on_restarting.set(Box::new(move || {
            remote::broadcast_restarting(&hook_state);
        }));
    }

    // 코디네이터가 "지금 듣는 사람이 있나" 를 물어볼 수 있게 해 준다 (웹 재생기 모드).
    {
        let hook_state = state.clone();
        let _ = app.web_listener_count.set(Box::new(move |guild_id| {
            remote::web_listener_count(&hook_state, guild_id)
        }));
    }

    // 한가할 때 차트를 미리 받아 둔다 (§15.3).
    {
        let hook_state = state.clone();
        let _ = app.on_chart_prefetch.set(Box::new(move || {
            remote::spawn_chart_prefetch(&hook_state);
        }));
    }

    let router = Router::new()
        .route("/", get(pages::index))
        .route("/login", get(pages::login_page).post(pages::login_post))
        .route("/Logout", post(pages::logout))
        .route("/diagnostics", get(pages::diagnostics))
        .route(
            "/settings",
            get(pages::settings_page).post(pages::settings_post),
        )
        .route("/botsettings", get(pages::botsettings_page))
        .route("/botsettings/oauth", post(pages::botsettings_oauth_post))
        // 쿠키 파일 업로드 (§10.9). 봇 주인 화면이라 파일을 그대로 받는다.
        .route("/botsettings/cookies", post(pages::botsettings_cookies_post))
        .route("/sharedconfig", get(pages::sharedconfig_page))
        .route("/guilds", get(pages::guilds_page).post(pages::guilds_post))
        .route("/tools", get(pages::tools_page).post(pages::tools_post))
        .route("/tools/prune", post(pages::tools_prune))
        .route("/cache", get(pages::cache_page))
        .route("/cache/wipe", post(pages::cache_wipe))
        .route("/cache/migrate", post(pages::cache_migrate))
        .route("/cache/delete", post(pages::cache_delete))
        .route("/cache/bulkdelete", post(pages::cache_bulk_delete))
        .route("/cache/addtoplaylist", post(pages::cache_add_to_playlist))
        .route(
            "/playlists",
            get(pages::playlists_page).post(pages::playlists_post),
        )
        .route(
            "/blacklist",
            get(pages::blacklist_page).post(pages::blacklist_post),
        )
        .route("/logs", get(pages::logs_page))
        .route("/setup", get(pages::setup_page).post(pages::setup_post))
        .route(
            "/password",
            get(pages::password_page).post(pages::password_post),
        )
        .route("/healthz", get(|| async { "ok" }))
        .merge(remote::router())
        .layer(CookieManagerLayer::new())
        .layer(middleware::from_fn(host_scope_guard))
        .with_state(state);

    let urls = std::env::var("MUSICBOT_WEB_URLS").unwrap_or_else(|_| "http://0.0.0.0:8693".into());
    let addr = urls.trim_start_matches("http://").to_string();
    app.log
        .info("Web", &format!("Web admin listening on {addr}."));
    match (admin_host(), remote_host()) {
        (Some(admin), Some(remote)) => app.log.info(
            "Web",
            &format!("호스트 분리: 관리자 {admin} · 리모컨 {remote}."),
        ),
        _ => app.log.warn(
            "Web",
            "MUSICBOT_ADMIN_HOST / MUSICBOT_REMOTE_HOST 가 없어 호스트 분리를 끕니다. \
             공개 배포라면 리모컨 도메인에서도 /botsettings · /logs 가 열립니다.",
        ),
    }
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            // ConnectInfo<SocketAddr> 로 피어 IP 를 받아 /setup 의 localhost 게이트에 사용.
            if let Err(e) = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            {
                app.log.error("Web", &format!("web server stopped: {e}"));
            }
        }
        Err(e) => app
            .log
            .error("Web", &format!("web bind failed ({addr}): {e}")),
    }
}

/// S8: `oauth_states`에 스위퍼가 없어 취소된 로그인 시도가 계속 쌓였다.
/// 같은 태스크에서 만료 세션·역할 캐시·레이트리밋 흔적도 함께 걷어낸다.
fn spawn_sweeper(state: Arc<WebState>) {
    const SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);
    const OAUTH_STATE_TTL: Duration = Duration::from_secs(10 * 60);
    const ROLE_CACHE_KEEP: Duration = Duration::from_secs(6 * 60 * 60);
    const RATE_KEEP: Duration = Duration::from_secs(10 * 60);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_INTERVAL).await;
            state
                .oauth_states
                .lock()
                .unwrap()
                .retain(|_, (issued, _)| issued.elapsed() < OAUTH_STATE_TTL);
            state
                .remote_member_roles
                .lock()
                .unwrap()
                .retain(|_, (seen, _)| seen.elapsed() < ROLE_CACHE_KEEP);
            state
                .remote_action_rate
                .lock()
                .unwrap()
                .retain(|_, seen| seen.elapsed() < RATE_KEEP);
            state
                .remote_chat_rate
                .lock()
                .unwrap()
                .retain(|_, seen| seen.elapsed() < RATE_KEEP);
            state.presence.lock().unwrap().retain(|_, count| *count > 0);
            // 끝난 투표와 지나간 캐시는 남겨 둘 이유가 없다 (V3 §10.5 · §22.6).
            state
                .skip_votes
                .lock()
                .unwrap()
                .retain(|_, vote| !vote.is_expired());
            state
                .stats_cache
                .lock()
                .unwrap()
                .retain(|_, (stored, _)| stored.elapsed() < Duration::from_secs(300));
            if let Err(error) = state.app.remote.prune_sessions() {
                state
                    .app
                    .log
                    .warn("Web", &format!("만료 세션 정리 실패: {error}"));
            }
        }
    });
}

#[cfg(test)]
mod host_scope_tests {
    use super::*;

    const ADMIN: Option<&str> = Some("musicbot.example.com");
    const REMOTE: Option<&str> = Some("music.example.com");

    /// 운영 패널 링크는 **다른 도메인**을 가리켜야 한다.
    ///
    /// 예전에는 화면에 `/` 가 박혀 있었는데, 리모컨 도메인의 `/` 는 위 `host_scope_guard`
    /// 가 `/music` 으로 되돌린다. 그래서 운영 패널을 누르면 **방금 보던 화면으로 돌아왔다.**
    /// 상대경로로는 절대 닿을 수 없는 자리라 못 박아 둔다.
    #[test]
    fn ops_link_points_at_the_admin_domain_not_at_ourselves() {
        assert_eq!(build_ops_url(ADMIN), "https://musicbot.example.com/");
        // 호스트 분리를 안 켠 로컬 개발에서는 같은 서버의 `/` 가 진짜 운영 패널이다.
        assert_eq!(build_ops_url(None), "/");
    }

    #[test]
    fn separates_admin_remote_and_internal_hosts() {
        assert_eq!(
            classify_host(Some("musicbot.example.com"), ADMIN, REMOTE),
            WebSurface::Admin
        );
        assert_eq!(
            classify_host(Some("MUSIC.EXAMPLE.COM:443"), ADMIN, REMOTE),
            WebSurface::Remote
        );
        assert_eq!(
            classify_host(Some("localhost:8693"), ADMIN, REMOTE),
            WebSurface::Internal
        );
        assert!(is_remote_path("/music"));
        assert!(is_remote_path("/music/oauth/callback"));
        assert!(!is_remote_path("/botsettings"));
    }

    /// 호스트명을 설정하지 않으면 분리가 꺼진다 — 전부 Internal 로 떨어져 단일 호스트로 돈다.
    #[test]
    fn unset_hosts_disable_the_split() {
        assert_eq!(
            classify_host(Some("musicbot.example.com"), None, None),
            WebSurface::Internal
        );
        assert_eq!(classify_host(Some("localhost:8693"), None, None), WebSurface::Internal);
    }

    /// 빈 Host 헤더가 `None` 설정과 우연히 같아져서 Admin 으로 분류되면 안 된다.
    #[test]
    fn empty_host_never_matches_an_unset_setting() {
        assert_eq!(classify_host(Some(""), None, None), WebSurface::Internal);
        assert_eq!(classify_host(None, None, None), WebSurface::Internal);
        assert_eq!(classify_host(Some(":8693"), None, None), WebSurface::Internal);
    }
}

// ───────── 인증 ─────────

pub fn is_authed(state: &WebState, cookies: &Cookies) -> bool {
    let Some(c) = cookies.get(SESSION_COOKIE) else {
        return false;
    };
    let mut sessions = state.sessions.lock().unwrap();
    match sessions.get(c.value()) {
        Some(created) if created.elapsed() < SESSION_TTL => true,
        Some(_) => {
            sessions.remove(c.value());
            false
        }
        None => false,
    }
}

pub fn begin_session(state: &WebState, cookies: &Cookies) {
    let token = crate::models::uuid_like();
    let csrf = crate::models::uuid_like();
    state
        .sessions
        .lock()
        .unwrap()
        .insert(token.clone(), Instant::now());
    state.admin_csrf.lock().unwrap().insert(token.clone(), csrf);
    let mut cookie = Cookie::new(SESSION_COOKIE, token);
    cookie.set_http_only(true);
    cookie.set_path("/");
    cookie.set_same_site(tower_cookies::cookie::SameSite::Strict);
    cookies.add(cookie);
}

pub fn end_session(state: &WebState, cookies: &Cookies) {
    if let Some(c) = cookies.get(SESSION_COOKIE) {
        state.sessions.lock().unwrap().remove(c.value());
        state.admin_csrf.lock().unwrap().remove(c.value());
    }
    cookies.remove(Cookie::new(SESSION_COOKIE, ""));
}

/// 현재 운영자 세션의 CSRF 토큰. 인증된 폼에만 삽입하며 Secret과 무관한 난수다.
pub fn admin_csrf_token(state: &WebState, cookies: &Cookies) -> Option<String> {
    if !is_authed(state, cookies) {
        return None;
    }
    let session = cookies.get(SESSION_COOKIE)?.value().to_string();
    let mut tokens = state.admin_csrf.lock().unwrap();
    Some(
        tokens
            .entry(session)
            .or_insert_with(crate::models::uuid_like)
            .clone(),
    )
}

pub fn verify_admin_csrf(state: &WebState, cookies: &Cookies, supplied: &str) -> bool {
    let Some(session) = cookies.get(SESSION_COOKIE) else {
        return false;
    };
    state
        .admin_csrf
        .lock()
        .unwrap()
        .get(session.value())
        .is_some_and(|expected| expected == supplied)
}

/// 인증 가드 — 비밀번호 미설정이면 최초 설정으로, 미인증이면 로그인으로 리다이렉트.
pub fn require_auth(state: &WebState, cookies: &Cookies) -> Option<Response> {
    if state.password_hash.lock().unwrap().is_none() {
        return Some(Redirect::to("/setup").into_response());
    }
    if is_authed(state, cookies) {
        None
    } else {
        Some(Redirect::to("/login").into_response())
    }
}

// ───────── 공용 레이아웃 ─────────

/// C# wwwroot/css/site.css 를 그대로 가져온 전역 스타일 (인라인화).
/// .brand-badge + table 기본 스타일.
const LAYOUT_CSS: &str = r#"<style>
:root{--bg:#F8FAFC;--ink:#0F172A;--muted:#64748B;--card:#FFFFFF;--line:#E2E8F0;--accent:#2563EB;--sidebar:#111827;--sidebar-line:#1F2937;--ok-bg:#F0FDF4;--ok-ink:#15803D;--err-bg:#FEF2F2;--err-ink:#DC2626}
*{box-sizing:border-box}
body{margin:0;font-family:"Malgun Gothic","Segoe UI",system-ui,sans-serif;background:var(--bg);color:var(--ink)}
.app{display:flex;min-height:100vh}
.sidebar{width:280px;flex:0 0 280px;background:var(--sidebar);color:#fff;padding:20px 16px;display:flex;flex-direction:column}
.brand{display:flex;align-items:center;gap:8px}
.brand-title{font-size:24px;font-weight:700}
.brand-badge{background:#7C3AED;color:#fff;font-size:11px;font-weight:600;padding:2px 8px;border-radius:10px}
.brand-sub{color:#CBD5E1;font-size:12px;margin:8px 0 18px}
.brand-build{color:#64748B;font-size:11px;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;letter-spacing:.04em;text-align:center;margin-top:10px}
nav{display:flex;flex-direction:column;gap:10px;flex:1}
.nav-item{display:flex;flex-direction:column;gap:4px;text-decoration:none;color:#fff;background:#1F2937;border:1px solid #334155;border-radius:10px;padding:12px 14px;transition:background .15s,border-color .15s}
.nav-item:hover{background:#273449}
.nav-item.active{background:#1D4ED8;border-color:#3B82F6}
.nav-remote{background:#4C1D95;border-color:#7C3AED;margin-bottom:14px}
.nav-remote:hover{background:#5B21B6}
.nav-title{font-size:15px;font-weight:600}
.nav-desc{font-size:12px;color:#CBD5E1}
.logout{margin-top:16px}
.content{flex:1;padding:24px 28px;max-width:1200px}
.card{background:var(--card);border:1px solid var(--line);border-radius:16px;padding:18px;margin-bottom:16px}
.card h2{margin:0 0 4px;font-size:20px}
.card .sub{color:var(--muted);font-size:13px;margin:0 0 14px}
.page-title{font-size:28px;font-weight:700;margin:0 0 4px}
.page-sub{color:var(--muted);margin:0 0 20px}
label.field{display:block;margin:12px 0 4px;font-weight:600;font-size:14px}
input[type=text],input[type=password],input[type=number],textarea,select{width:100%;padding:10px 12px;border:1px solid #CBD5E1;border-radius:8px;background:#F8FAFC;color:var(--ink);font-size:14px;font-family:inherit}
input:focus,textarea:focus,select:focus{outline:none;border-color:var(--accent)}
textarea{resize:vertical}
.checkbox{display:flex;align-items:center;gap:8px;margin:10px 0;font-size:14px}
.btn{display:inline-block;cursor:pointer;font-size:14px;font-weight:600;padding:10px 16px;border-radius:10px;border:1px solid transparent;text-decoration:none;transition:filter .15s}
.btn:hover{filter:brightness(.95)}
.btn:active{filter:brightness(.88)}
.btn-primary{background:var(--accent);color:#fff;border-color:var(--accent)}
.btn-secondary{background:#fff;color:var(--ink);border-color:#CBD5E1}
.btn-danger{background:#DC2626;color:#fff;border-color:#DC2626}
.actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:14px}
.status{padding:12px 14px;border-radius:12px;margin-bottom:16px;font-weight:600}
.status.ok{background:var(--ok-bg);color:var(--ok-ink);border:1px solid var(--ok-ink)}
.status.err{background:var(--err-bg);color:var(--err-ink);border:1px solid var(--err-ink)}
.pill{display:inline-block;padding:2px 10px;border-radius:999px;font-size:12px;font-weight:600}
.pill.run{background:var(--ok-bg);color:var(--ok-ink)}
.pill.stop{background:#FFF7ED;color:#C2410C}
.kv{color:var(--muted);font-size:13px}
pre.log{background:#0B1220;color:#E2E8F0;border-radius:10px;padding:14px;font-family:Consolas,monospace;font-size:12px;line-height:1.5;white-space:pre-wrap;word-break:break-all;max-height:520px;overflow:auto;margin:0}
.grid2{display:grid;grid-template-columns:1fr 1fr;gap:16px}
@media (max-width:820px){.grid2{grid-template-columns:1fr}.sidebar{display:none}}
.pl-row{border-bottom:1px solid var(--line);padding:10px 0}
.guild-list{list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:6px}
.guild-list li{margin:0}
.guild-row{display:flex;align-items:center;gap:10px;padding:8px 10px;border-radius:8px;text-decoration:none;color:var(--ink);border:1px solid var(--line)}
.guild-row:hover{background:#F8FAFC}
.guild-icon{width:32px;height:32px;border-radius:8px;flex:0 0 32px;object-fit:cover;background:#E2E8F0}
.guild-icon-fallback{display:inline-flex;align-items:center;justify-content:center;font-weight:700;color:#64748B;background:#E2E8F0}
.guild-name{flex:1;font-weight:600}
.guild-id{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;color:var(--muted)}
.logfilter{display:flex;gap:16px;flex-wrap:wrap;align-items:flex-end}
.logfilter select{min-width:160px}
.logtable{font-family:Consolas,monospace;font-size:12px}
.logrow{display:grid;grid-template-columns:120px 56px 110px 1fr;gap:10px;padding:5px 6px;border-bottom:1px solid #F1F5F9;align-items:start}
.logtime{color:#64748B}
.loglevel{font-weight:700}
.logcat{color:#1D4ED8;font-weight:600}
.logmsg{white-space:pre-wrap;word-break:break-word}
.logrow.log-info .loglevel{color:#0F766E}
.logrow.log-warn{background:#FFFBEB}
.logrow.log-warn .loglevel{color:#C2410C}
.logrow.log-err{background:#FEF2F2}
.logrow.log-err .loglevel{color:#DC2626}
.diag-grid{display:grid;grid-template-columns:130px 1fr;gap:6px 14px;font-size:14px}
.diag-grid .k{color:var(--muted)}
ul{margin:6px 0;padding-left:20px}
.login-wrap{display:flex;align-items:center;justify-content:center;min-height:100vh;width:100%}
.login-card{width:360px;background:var(--card);border:1px solid var(--line);border-radius:16px;padding:28px}
table{width:100%;border-collapse:collapse;font-size:14px}
th,td{text-align:left;padding:8px 10px;border-bottom:1px solid var(--line)}
th{color:var(--muted);font-weight:600;font-size:13px}
</style>"#;

/// C# TempData 상태 배너의 등가물 — 폼 POST 가 `?msg=...&err=1` 로 리다이렉트하면
/// 모든 페이지에서 page-sub 아래에 초록/빨강 배너를 그린다 (주소창은 즉시 정리).
const FLASH_JS: &str = r#"<script>
(function(){
  var p = new URLSearchParams(location.search);
  var m = p.get('msg');
  if (!m) return;
  var d = document.createElement('div');
  d.className = 'status ' + (p.get('err') ? 'err' : 'ok');
  d.textContent = m;
  var c = document.querySelector('.content');
  if (!c) return;
  var s = c.querySelector('.page-sub');
  if (s && s.parentElement === c) { s.insertAdjacentElement('afterend', d); }
  else { c.insertBefore(d, c.firstChild); }
  p.delete('msg'); p.delete('err');
  var q = p.toString();
  if (history.replaceState) { history.replaceState(null, '', location.pathname + (q ? '?' + q : '')); }
})();
</script>"#;

pub fn layout(state: &WebState, title: &str, active: &str, body: &str) -> Html<String> {
    let build = &state.app.build_id;
    let nav_items = [
        ("/", "메인 대시보드", "상태 · 봇 제어"),
        ("/diagnostics", "진단 / 상태", "길드별 재생 · 자동추천 · 큐"),
        (
            "/settings",
            "재생 설정",
            "볼륨 · 자동추천 · 자동퇴장 · 알림",
        ),
        ("/botsettings", "봇 설정", "토큰 · 명령 등록 · override"),
        ("/sharedconfig", "공용 설정", "owner · 데이터/도구 경로"),
        ("/guilds", "서버 설정", "길드별 override"),
        ("/tools", "도구 / 캐시", "yt-dlp · ffmpeg · 캐시"),
        ("/cache", "캐시 라이브러리", "받아둔 곡 둘러보기"),
        ("/playlists", "플레이리스트", "전역 · 길드"),
        ("/blacklist", "차단 목록", "제목 · URL · 길드/전역"),
        ("/logs", "로그 뷰어", "최근 운영 로그"),
        ("/password", "비밀번호 변경", "웹 관리자 비밀번호"),
    ];
    let nav: String = nav_items
        .iter()
        .map(|(href, t, d)| {
            let cls = if *href == active { "nav-item active" } else { "nav-item" };
            format!(r#"<a class="{cls}" href="{href}"><span class="nav-title">{t}</span><span class="nav-desc">{d}</span></a>"#)
        })
        .collect();
    Html(format!(
        r#"<!DOCTYPE html><html lang="ko"><head><meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1.0"/>
<title>{title} · Discord 뮤직봇 관리</title>
{css}</head><body><div class="app">
<aside class="sidebar">
  <div class="brand"><div class="brand-title">mc-musicbot</div></div>
  <div class="brand-sub">Discord 음악봇 운영 패널 · Rust 엔진</div>
  <a class="nav-item nav-remote" href="/music" target="_blank" rel="noopener"><span class="nav-title">리모컨 열기 →</span><span class="nav-desc">사용자 화면(마참뮤직)으로 이동</span></a>
  <nav>{nav}</nav>
  <form class="logout" method="post" action="/Logout"><button type="submit" class="btn btn-secondary" style="width:100%">로그아웃</button></form>
  <div class="brand-build" title="현재 실행 중인 빌드 ID.">build {build}</div>
</aside>
<main class="content">{body}</main>
</div>{flash}</body></html>"#,
        css = LAYOUT_CSS,
        flash = FLASH_JS,
    ))
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
