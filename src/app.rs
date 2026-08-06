//! 전역 앱 컨텍스트 — C# MusicBotRuntime + WebAdminContext 에 해당.
//! Discord 클라이언트/웹 서버/플레이어가 공유하는 모든 서비스의 루트.

use crate::blacklist::Blacklist;
use crate::config::Config;
use crate::db::Db;
use crate::logging::LogService;
use crate::media::cache::CacheManager;
use crate::media::ytdlp::YtDlp;
use crate::models::TrackRef;
use crate::player::autoplay::AutoplayEngine;
use crate::player::coordinator::Coordinator;
use crate::player::manager::PlayerManager;
use crate::remote::{RemoteStore, RetentionConfig};
use serde::Serialize;
use serenity::cache::Cache;
use serenity::http::Http;
use songbird::Songbird;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// 대기열 재정렬 주기 (사양서 §3.3 — 10초에서 단축).
const QUEUE_SORT_INTERVAL: Duration = Duration::from_secs(5);
/// 보존 정리 주기 (사양서 B16) — 기동 직후 1회 + 24시간마다.
const RETENTION_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// `/검색` 후보 묶음 — 셀렉트 메뉴 선택 시 인덱스로 되찾는다.
/// custom_id 에 트랙 전체를 담을 수 없어(100자 제한) 토큰으로 서버에 보관한다.
pub struct SearchSession {
    pub candidates: Vec<TrackRef>,
    pub created: Instant,
}

/// 특권 게이트웨이 인텐트 가용 여부 — 웹(멤버 목록·온라인 상태)이 표시를 축소할 근거.
/// 개발자 포털에서 꺼져 있으면 serenity 연결이 거부되므로 main.rs 가 인텐트를 빼고
/// 재접속하면서 여기에 사실을 기록한다.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentStatus {
    /// Server Members Intent (GUILD_MEMBERS) — 길드 멤버 전체 목록.
    pub members: bool,
    /// Presence Intent (GUILD_PRESENCES) — 온라인/자리비움 상태.
    pub presences: bool,
    /// 축소된 이유 (정상이면 None).
    pub degraded_reason: Option<String>,
}

impl Default for IntentStatus {
    /// 기동 시엔 특권 인텐트를 요청한 상태로 본다 — 거부되면 재접속 루프가 즉시 false 로 낮춘다.
    fn default() -> Self {
        IntentStatus {
            members: true,
            presences: true,
            degraded_reason: None,
        }
    }
}

pub struct App {
    pub config: Config,
    pub db: Arc<Db>,
    pub log: Arc<LogService>,
    pub cache: Arc<CacheManager>,
    pub blacklist: Arc<Blacklist>,
    pub player: Arc<PlayerManager>,
    pub autoplay: Arc<AutoplayEngine>,
    pub coordinator: Arc<Coordinator>,
    /// 마참뮤직 점수 큐·개인 목록·채팅·감사 로그 저장소.
    pub remote: Arc<RemoteStore>,
    /// serenity 클라이언트 기동 후 채워지는 핸들들.
    pub songbird: OnceLock<Arc<Songbird>>,
    pub http: OnceLock<Arc<Http>>,
    /// OAuth 사용자의 현재 길드 역할·음성 채널을 서버 측에서 재검증할 Discord 캐시.
    pub discord_cache: OnceLock<Arc<Cache>>,
    /// 웹 리모컨 공개 주소 (`RemoteAuthConfig.public_base_url`, 끝 슬래시 없음).
    /// `web::serve()` 가 채우고 `/리모컨` 명령이 링크를 만들 때 읽는다. 미설정이면 안내만 한다.
    pub public_base_url: OnceLock<String>,
    /// 특권 인텐트 가용 여부 — 기동/재접속 시 main.rs 가 갱신한다.
    pub intent_status: RwLock<IntentStatus>,
    /// 봇 주인 Discord 유저 ID 목록 (remote-oauth.json 의 ownerUserIds). 웹이 기동 시 채운다.
    pub owner_user_ids: RwLock<Vec<u64>>,
    /// 5초 재정렬 태스크가 대기열 순서를 실제로 바꿨을 때 부르는 훅 (인자: guild_id).
    /// 웹이 `web::serve()`에서 걸어 두면 `queue.set` 이벤트를 그때만 broadcast 할 수 있다.
    /// 비어 있으면 정렬만 하고 아무도 깨우지 않는다.
    pub on_queue_sorted: OnceLock<Box<dyn Fn(u64) + Send + Sync>>,
    /// 길드별 마지막 명령 채널 (현재 재생 중 알림 대상).
    pub announce_channels: Mutex<HashMap<u64, u64>>,
    /// 길드별 직전 Now-Playing 메시지 (채널, 메시지) — 새 카드 전송 시 이전 카드 버튼 제거용.
    pub last_np_message: Mutex<HashMap<u64, (u64, u64)>>,
    /// 빈 채널 자동퇴장 디바운스 타이머 세대 카운터.
    pub pending_leaves: Mutex<HashMap<u64, u64>>,
    /// `/검색` 후보 세션 (토큰 → 후보 목록). 선택/취소 시 소비, 오래된 항목은 lazy 정리.
    pub search_sessions: Mutex<HashMap<String, SearchSession>>,
    pub build_id: String,
}

impl App {
    pub fn new(config: Config) -> Arc<App> {
        let log = Arc::new(LogService::new(config.logs_dir()));
        let db = Arc::new(Db::open(&config.db_path()).expect("musicbot.sqlite 열기 실패"));
        let blacklist = Arc::new(Blacklist::new(db.clone()));
        let cache = Arc::new(CacheManager::new(
            config.cache_dir(),
            db.clone(),
            log.clone(),
        ));
        let remote = Arc::new(
            RemoteStore::open(&config.db_path()).expect("마참뮤직 SQLite 테이블 준비 실패"),
        );
        let player = Arc::new(PlayerManager::new(db.clone(), remote.clone(), log.clone()));

        let global = db.load_global_settings();
        let ytdlp = YtDlp {
            exe: config.yt_dlp_path.clone(),
            browser_profile: global.preferred_browser_profile.clone(),
            cookie_file: global.cookie_file_path.clone(),
        };
        let autoplay = Arc::new(AutoplayEngine {
            ytdlp: ytdlp.clone(),
            blacklist: blacklist.clone(),
            log: log.clone(),
        });
        let coordinator = Arc::new(Coordinator::new());

        let build_id = std::fs::read_to_string(config.portable_root.join("BUILD_ID.txt"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "dev".to_string());

        let app = Arc::new(App {
            config,
            db,
            log,
            cache,
            blacklist,
            player,
            autoplay,
            coordinator,
            remote,
            songbird: OnceLock::new(),
            http: OnceLock::new(),
            discord_cache: OnceLock::new(),
            public_base_url: OnceLock::new(),
            intent_status: RwLock::new(IntentStatus::default()),
            owner_user_ids: RwLock::new(Vec::new()),
            on_queue_sorted: OnceLock::new(),
            announce_channels: Mutex::new(HashMap::new()),
            last_np_message: Mutex::new(HashMap::new()),
            pending_leaves: Mutex::new(HashMap::new()),
            search_sessions: Mutex::new(HashMap::new()),
            build_id,
        });
        app.spawn_background_tasks();
        app
    }

    /// 재생·웹과 무관하게 계속 도는 잡무들. 런타임 밖에서 App 을 만들면(테스트 등)
    /// tokio::spawn 이 패닉하므로 런타임이 있을 때만 띄운다.
    fn spawn_background_tasks(self: &Arc<App>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        tokio::spawn(queue_sort_loop(self.clone()));
        tokio::spawn(retention_prune_loop(self.clone()));
    }

    /// 전역 설정 기준 최신 yt-dlp 래퍼 (브라우저 프로필/쿠키 변경 즉시 반영).
    pub fn ytdlp(&self) -> YtDlp {
        let global = self.db.load_global_settings();
        YtDlp {
            exe: self.config.yt_dlp_path.clone(),
            browser_profile: global.preferred_browser_profile,
            cookie_file: global.cookie_file_path,
        }
    }
}

/// 대기열 재정렬 루프. 순서가 실제로 바뀐 길드만 저장되고 훅이 불린다.
/// 길드 하나를 처리할 때마다 양보해 재생 경로가 게이트를 오래 기다리지 않게 한다.
async fn queue_sort_loop(app: Arc<App>) {
    let mut ticker = tokio::time::interval(QUEUE_SORT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        for guild_id in app.db.list_known_guild_ids() {
            if app.player.resort_if_changed(guild_id).await
                && let Some(hook) = app.on_queue_sorted.get()
            {
                hook(guild_id);
            }
            tokio::task::yield_now().await;
        }
    }
}

/// 보존 정리 루프 (사양서 B16). 첫 tick 은 즉시 발화하므로 기동 시 1회가 자동으로 포함된다.
/// 길드별 `chat_retention_days`·`audit_retention_days` 는 `prune_all` 이 직접 읽어 반영한다.
/// 아무것도 안 지웠으면 로그를 남기지 않는다 — 하루 한 줄이라도 의미 없는 줄은 소음이다.
async fn retention_prune_loop(app: Arc<App>) {
    let mut ticker = tokio::time::interval(RETENTION_PRUNE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match app.remote.prune_all(RetentionConfig::default()) {
            Ok(report) if report.is_empty() => {}
            Ok(report) => app.log.info(
                "Remote",
                &format!(
                    "보존 정리 완료: 채팅 {}건, 최근재생 {}건, 활동로그 {}건, 가사실패 {}건, 만료세션 {}건 삭제.",
                    report.chat, report.recent, report.audit, report.lyrics, report.sessions
                ),
            ),
            Err(error) => app
                .log
                .warn("Remote", &format!("보존 정리 실패: {error}")),
        }
    }
}
