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
use crate::remote::RemoteStore;
use serenity::cache::Cache;
use serenity::http::Http;
use songbird::Songbird;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// `/검색` 후보 묶음 — 셀렉트 메뉴 선택 시 인덱스로 되찾는다.
/// custom_id 에 트랙 전체를 담을 수 없어(100자 제한) 토큰으로 서버에 보관한다.
pub struct SearchSession {
    pub candidates: Vec<TrackRef>,
    pub created: Instant,
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

        Arc::new(App {
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
            announce_channels: Mutex::new(HashMap::new()),
            last_np_message: Mutex::new(HashMap::new()),
            pending_leaves: Mutex::new(HashMap::new()),
            search_sessions: Mutex::new(HashMap::new()),
            build_id,
        })
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
