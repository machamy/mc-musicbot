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
/// 웹이 `last_queue_sort + QUEUE_SORT_INTERVAL` 로 `nextSortAt` 을 계산해 카운트다운을 그린다(v3 §5).
pub const QUEUE_SORT_INTERVAL: Duration = Duration::from_secs(5);
/// 대기열이 아주 긴 길드의 재정렬 주기 (v3 §18.2).
/// 5000곡을 5초마다 정렬하면 CPU 도 WS payload 도 감당이 안 되고, 그 정도 길이면 순서가 급하지도 않다.
pub const QUEUE_SORT_INTERVAL_LONG: Duration = Duration::from_secs(15);
/// 이 곡 수를 **넘으면** 긴 주기로 갈아탄다 (v3 §18.2 — "500곡을 넘으면 5초 → 15초").
pub const QUEUE_SORT_LONG_THRESHOLD: usize = 500;
/// 보존 정리 주기 (사양서 B16) — 기동 직후 1회 + 24시간마다.
const RETENTION_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 대기열 길이에 맞는 재정렬 주기. 화면의 카운트다운(v3 §5)도 이 함수를 거친 값을 따라간다.
pub const fn queue_sort_interval_for_len(queue_len: usize) -> Duration {
    if queue_len > QUEUE_SORT_LONG_THRESHOLD {
        QUEUE_SORT_INTERVAL_LONG
    } else {
        QUEUE_SORT_INTERVAL
    }
}

/// `/검색` 후보 묶음 — 셀렉트 메뉴 선택 시 인덱스로 되찾는다.
/// custom_id 에 트랙 전체를 담을 수 없어(100자 제한) 토큰으로 서버에 보관한다.
pub struct SearchSession {
    pub candidates: Vec<TrackRef>,
    pub created: Instant,
}

/// 길드별 재정렬 스케줄. 대기열이 길면 주기를 늘리기 때문에(v3 §18.2)
/// "언제 마지막으로 돌았나"와 "그 길드 대기열이 얼마나 긴가"를 같이 들고 있어야 한다.
#[derive(Debug, Clone, Copy, Default)]
struct QueueSortSlot {
    /// 이 길드를 마지막으로 재정렬 검사한 시각. 아직 한 번도 안 돌았으면 `None`.
    last: Option<chrono::DateTime<chrono::Utc>>,
    /// 마지막으로 관측된 대기열 길이. 아무도 알려 주지 않았으면 0 = 기본 주기.
    queue_len: usize,
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
    /// 개인 통계·우리 차트 저장소 (v3 §22). **본 DB와 파일이 분리돼 있다.**
    ///
    /// **`None` 이어도 봇은 정상 동작해야 한다.** 통계 DB를 못 열면 통계 기능만 꺼진 채로 계속 간다 —
    /// 통계 때문에 음악이 멈추면 본말전도다. 그래서 `Option` 이고, 호출부는
    /// `if let Some(stats) = &app.stats { stats.record(...) }` 로 조용히 건너뛴다.
    pub stats: Option<Arc<crate::stats::Stats>>,
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
    /// 마지막 재정렬 tick 시각(길드 무관). 웹이 nextSortAt 을 계산해 카운트다운을 그린다.
    /// 순서가 바뀌었는지와 무관하게 매 tick 갱신된다 — 카운트다운은 "다음 검사 시각"이지
    /// "다음에 순서가 바뀌는 시각"이 아니다. 아직 한 번도 안 돌았으면 `None`.
    ///
    /// **길드마다 주기가 다를 수 있으므로**(v3 §18.2) 정확한 카운트다운은
    /// [`App::next_queue_sort_at`] 를 쓴다. 이 필드는 그 길드를 모를 때의 근사값이다.
    pub last_queue_sort: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    /// 길드별 재정렬 스케줄 (마지막 시각 + 마지막으로 관측된 대기열 길이).
    /// 웹이 `note_queue_len` 으로 길이를 알려 주면 다음 tick 부터 주기가 자동으로 갈린다.
    queue_sort_slots: Mutex<HashMap<u64, QueueSortSlot>>,
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

        // 통계 DB (v3 §22.1) — 본 DB 옆에 별도 파일로 둔다. 포터블 업데이트가 지우지 않도록
        // `data/` 안, 기존 DB와 같은 자리다. 쓰기 태스크가 tokio::spawn 을 쓰므로
        // 런타임 밖에서 App 을 만들면(테스트 등) 아예 열지 않는다 — 통계는 없어도 봇이 돈다.
        let stats = if tokio::runtime::Handle::try_current().is_ok() {
            let path = config.data_root.join("musicbot-stats.sqlite");
            let opened = crate::stats::Stats::open(&path, log.clone());
            if opened.is_none() {
                log.warn(
                    "Stats",
                    "통계 없이 계속 갑니다. 재생·대기열·채팅은 그대로 동작합니다.",
                );
            }
            opened
        } else {
            None
        };

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
            stats,
            songbird: OnceLock::new(),
            http: OnceLock::new(),
            discord_cache: OnceLock::new(),
            public_base_url: OnceLock::new(),
            intent_status: RwLock::new(IntentStatus::default()),
            owner_user_ids: RwLock::new(Vec::new()),
            on_queue_sorted: OnceLock::new(),
            last_queue_sort: RwLock::new(None),
            queue_sort_slots: Mutex::new(HashMap::new()),
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

    // ───────── 대기열 재정렬 스케줄 (v3 §5 · §18.2) ─────────

    /// 이 길드의 대기열 길이를 알려 준다. **웹이 대기열을 만들 때마다 부르면 된다** —
    /// 이미 계산해 둔 숫자를 넘기는 것뿐이라 추가 쿼리가 0이다.
    ///
    /// 재정렬 루프는 이 값만 보고 주기를 5초/15초로 가른다. 아무도 안 알려 주면 계속 5초다
    /// (지금까지의 동작 그대로라 조용히 달라지는 게 없다).
    pub fn note_queue_len(&self, guild_id: u64, queue_len: usize) {
        if let Ok(mut slots) = self.queue_sort_slots.lock() {
            slots.entry(guild_id).or_default().queue_len = queue_len;
        }
    }

    /// 이 길드의 지금 재정렬 주기. 화면의 `4820곡 · 정렬은 15초마다` 표시(v3 §18.3)가 이 값을 쓴다.
    pub fn queue_sort_interval(&self, guild_id: u64) -> Duration {
        let len = self
            .queue_sort_slots
            .lock()
            .ok()
            .and_then(|slots| slots.get(&guild_id).map(|slot| slot.queue_len))
            .unwrap_or(0);
        queue_sort_interval_for_len(len)
    }

    /// 이 길드의 다음 재정렬 예정 시각 — 대기열 카운트다운(v3 §5)의 기준.
    ///
    /// 클라이언트 타이머만 쓰면 탭이 백그라운드에 갔다 오는 순간 어긋나므로 기준 시각은 서버가 준다.
    /// 루프가 밀렸으면 이미 지난 시각을 주지 않고 다음 주기로 넘긴다.
    pub fn next_queue_sort_at(&self, guild_id: u64) -> chrono::DateTime<chrono::Utc> {
        let now = chrono::Utc::now();
        let (last, len) = self
            .queue_sort_slots
            .lock()
            .ok()
            .and_then(|slots| slots.get(&guild_id).copied())
            .map(|slot| (slot.last, slot.queue_len))
            .unwrap_or((None, 0));
        let seconds = queue_sort_interval_for_len(len).as_secs() as i64;
        let period = chrono::Duration::seconds(seconds.max(1));
        let mut next = last.unwrap_or(now) + period;
        while next <= now {
            next += period;
        }
        next
    }

    /// 지금 이 길드를 재정렬할 차례인가. 한 번도 안 돌았으면 바로 차례다.
    fn queue_sort_due(&self, guild_id: u64, now: chrono::DateTime<chrono::Utc>) -> bool {
        let Ok(slots) = self.queue_sort_slots.lock() else {
            return true;
        };
        let Some(slot) = slots.get(&guild_id) else {
            return true;
        };
        let Some(last) = slot.last else {
            return true;
        };
        let seconds = queue_sort_interval_for_len(slot.queue_len).as_secs() as i64;
        // 밀린 tick 을 정확히 맞추려 들면 15초 길드가 5초마다 조금씩 앞당겨진다.
        // 여유를 반 tick 두어 "거의 다 됐으면 이번에 같이 돈다"로 처리한다.
        now - last >= chrono::Duration::milliseconds(seconds * 1000 - 250)
    }

    fn mark_queue_sorted(&self, guild_id: u64, now: chrono::DateTime<chrono::Utc>) {
        if let Ok(mut slots) = self.queue_sort_slots.lock() {
            slots.entry(guild_id).or_default().last = Some(now);
        }
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
///
/// tick 자체는 늘 5초지만, **대기열이 500곡을 넘는 길드는 15초에 한 번만** 실제로 정렬한다(v3 §18.2).
/// 5000곡을 5초마다 정렬하면 정렬 비용도, 그때마다 접속자 전원에게 나가는 `queue.set` 도 감당이 안 된다.
async fn queue_sort_loop(app: Arc<App>) {
    let mut ticker = tokio::time::interval(QUEUE_SORT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        // 길드를 다 돌기 전에 찍는다 — 웹의 카운트다운 기준은 "이번 tick 이 시작된 시각"이어야
        // 다음 tick 까지 정확히 QUEUE_SORT_INTERVAL 이 남는다.
        let now = chrono::Utc::now();
        if let Ok(mut slot) = app.last_queue_sort.write() {
            *slot = Some(now);
        }
        for guild_id in app.db.list_known_guild_ids() {
            if !app.queue_sort_due(guild_id, now) {
                continue; // 대기열이 긴 길드 — 아직 15초가 안 됐다. DB 도 안 건드린다.
            }
            app.mark_queue_sorted(guild_id, now);
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
        // 통계 DB 도 같은 태스크에서 하루 한 번 정리한다 (v3 §22.7).
        // 일별 표만 90일로 자르고 누적 롤업은 그대로 둔다 — 사람×곡이라 행이 안 터진다.
        if let Some(stats) = &app.stats {
            match stats.prune() {
                Ok(0) => {}
                Ok(removed) => app
                    .log
                    .info("Stats", &format!("통계 보존 정리 완료: {removed}건 삭제.")),
                Err(error) => app
                    .log
                    .warn("Stats", &format!("통계 보존 정리 실패: {error}")),
            }
        }
    }
}

/// 웹 리모컨 공개 주소의 프로세스 전역 사본.
///
/// `App.public_base_url` 과 같은 값이지만, Discord 임베드의 버튼 빌더처럼
/// `&App` 핸들이 없는 곳에서 링크를 만들어야 해서 전역으로도 둔다.
/// `web::serve()` 가 둘을 같이 채운다.
static PUBLIC_BASE_URL: OnceLock<String> = OnceLock::new();

/// `web::serve()` 전용. 끝 슬래시를 떼고 저장한다.
pub fn set_public_base_url(url: &str) {
    let trimmed = url.trim().trim_end_matches('/');
    if !trimmed.is_empty() {
        let _ = PUBLIC_BASE_URL.set(trimmed.to_string());
    }
}

/// 이 길드의 웹 리모컨 주소. 공개 주소가 설정되지 않았으면 `None`.
pub fn remote_url_for(guild_id: u64) -> Option<String> {
    PUBLIC_BASE_URL
        .get()
        .map(|base| format!("{base}/music/guilds/{guild_id}"))
}
