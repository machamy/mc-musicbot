//! mc-musicbot — Rust(serenity + songbird) 디스코드 음악봇.
//! 오디오 엔진: songbird (전용 스레드 페이싱 + DAVE E2EE 내장) — 끊김 방지의 구조적 해법.
//! 데이터: .musicbot-data/musicbot.sqlite (SQLite).

mod app;
mod blacklist;
mod commands;
mod config;
mod db;
mod events;
mod logging;
mod media;
mod models;
mod player;
mod remote;
/// 끊김을 줄이는 종료·복구 (V3 §24).
mod shutdown;
/// 개인 통계와 우리 차트. **musicbot.sqlite 와 파일이 분리된 별도 DB**를 쓴다 (V3 §22.1).
mod stats;
mod web;

use serenity::all::{GatewayError, GatewayIntents};
use songbird::SerenityInit;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// 개발자 포털에서 특권 인텐트가 꺼져 있어 게이트웨이가 IDENTIFY 를 거부한 경우인가.
/// (undocumented 인텐트를 보낸 경우도 같은 처방 — 특권 인텐트를 빼고 다시 붙는다.)
fn is_intent_rejection(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Gateway(
            GatewayError::DisallowedGatewayIntents | GatewayError::InvalidGatewayIntents
        )
    )
}

/// songbird/serenity 내부 tracing 이벤트를 웹 로그 뷰어로 포워딩.
/// 무음/DAVE 협상 문제는 우리 코드가 아니라 드라이버가 안다 — 이게 없으면
/// "초록불인데 무음" 류 장애의 원인이 허공으로 버려진다 (2026-06-11 실측).
struct DriverLogLayer {
    log: Arc<logging::LogService>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for DriverLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        let level = *meta.level();
        let target = meta.target();
        // WARN 이상은 전부, INFO 는 음성 드라이버(songbird/davey)만.
        let keep = level <= tracing::Level::WARN
            || (level == tracing::Level::INFO
                && (target.starts_with("songbird") || target.starts_with("davey")));
        if !keep {
            return;
        }
        struct V(String);
        impl tracing::field::Visit for V {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                if field.name() == "message" {
                    let _ = write!(self.0, "{value:?}");
                } else {
                    let _ = write!(self.0, " {}={:?}", field.name(), value);
                }
            }
        }
        let mut v = V(String::new());
        event.record(&mut v);
        let msg = format!("[{target}] {}", v.0);
        match level {
            tracing::Level::ERROR => self.log.error("Driver", &msg),
            tracing::Level::WARN => self.log.warn("Driver", &msg),
            _ => self.log.info("Driver", &msg),
        }
    }
}

#[cfg(windows)]
mod win {
    #[link(name = "winmm")]
    unsafe extern "system" {
        pub fn timeBeginPeriod(u_period: u32) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateMutexW(
            attrs: *const core::ffi::c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut core::ffi::c_void;
        pub fn GetLastError() -> u32;
    }
    /// 단일 인스턴스 뮤텍스 — 이중 실행 방지 (C# 의 Global\ 뮤텍스와 동일 사상).
    pub fn acquire_single_instance() -> bool {
        let name: Vec<u16> = "Global\\McMusicbot"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let handle = CreateMutexW(core::ptr::null(), 1, name.as_ptr());
            !(handle.is_null() || GetLastError() == 183) // ERROR_ALREADY_EXISTS
        }
    }
}

#[tokio::main]
async fn main() {
    // 윈도우 타이머 해상도 1ms — 소프트 타이머 정밀도 (송신 페이싱 보조).
    #[cfg(windows)]
    unsafe {
        let _ = win::timeBeginPeriod(1);
    }
    #[cfg(windows)]
    if !win::acquire_single_instance() {
        eprintln!("mc-musicbot 이 이미 실행 중입니다. 기존 창을 닫고 다시 실행하세요.");
        std::process::exit(2);
    }

    // botsettings.json 보다 먼저 읽는다 — 로그·웹 호스트·OAuth 가 전부 환경변수를 본다.
    let env_file = config::load_env_file();

    let mut config = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("설정 로드 실패: {e}");
            std::process::exit(1);
        }
    };

    // 도구 확보 — yt-dlp 가 없으면 받아오고(없으면 config.yt_dlp_path 갱신), ffmpeg 는 없으면 안내.
    media::tools::ensure_tools(&mut config).await;

    let app = app::App::new(config);
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let _ = tracing_subscriber::registry()
            .with(DriverLogLayer {
                log: app.log.clone(),
            })
            .try_init();
    }
    app.log.info(
        "Bot",
        &format!("Starting mc-musicbot (build {}).", app.build_id),
    );
    if let Some(path) = env_file {
        app.log
            .info("Bot", &format!("환경변수 파일을 읽었습니다: {}", path.display()));
    }

    // 우리가 관리하는 yt-dlp 를 백그라운드로 주기 자동 업데이트 (설정으로 끌 수 있음).
    media::tools::spawn_auto_update(app.clone());

    // songbird 인스턴스를 직접 만들어 보관 — 코디네이터가 게이트웨이 컨텍스트 없이 접근 가능.
    let manager = songbird::Songbird::serenity();
    let _ = app.songbird.set(manager.clone());

    // 웹 관리 UI (axum) — 기본 포트 8693 (MUSICBOT_WEB_URLS 로 변경 가능).
    {
        let app2 = app.clone();
        tokio::spawn(async move {
            web::serve(app2).await;
        });
    }

    // 종료 신호를 지켜본다 (§24). `web::serve` 가 재시작 알림 훅을 채운 뒤에 띄운다 —
    // 먼저 띄우면 훅이 아직 비어 있어서 브라우저가 안내를 못 받는다.
    shutdown::watch(app.clone());

    // 게이트웨이가 죽어도(토큰 오류/네트워크 단절) 웹 UI 는 살아 있어야 운영자가 로그를 본다.
    // 30초 간격으로 클라이언트를 재생성/재접속한다.
    //
    // 특권 인텐트(GUILD_MEMBERS/GUILD_PRESENCES)는 멤버 목록·온라인 상태 표시에 필요하지만,
    // 개발자 포털에서 꺼져 있으면 게이트웨이가 IDENTIFY 를 거부해 봇이 아예 뜨지 않는다.
    // 그래서 거부로 판단되면 특권 인텐트를 빼고 **즉시** 재접속하고, 그 사실을 상태에 남긴다.
    let base_intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;
    let privileged_intents = GatewayIntents::GUILD_MEMBERS | GatewayIntents::GUILD_PRESENCES;
    let mut privileged = true;
    loop {
        let intents = if privileged {
            base_intents | privileged_intents
        } else {
            base_intents
        };
        let handler = events::Handler {
            app: app.clone(),
            ready_once: AtomicBool::new(false),
        };
        let client = serenity::Client::builder(&app.config.token, intents)
            .event_handler(handler)
            .register_songbird_with(manager.clone())
            .await;
        match client {
            Ok(mut client) => {
                if let Err(e) = client.start().await {
                    // 특권 인텐트 거부 — 한 번만 축소하고 대기 없이 다시 붙는다.
                    if privileged && is_intent_rejection(&e) {
                        privileged = false;
                        let reason = "Discord가 특권 인텐트를 거부했습니다. 개발자 포털 → 내 봇 → Bot → Privileged Gateway Intents에서 Server Members / Presence Intent를 켠 뒤 봇을 재시작하세요.";
                        if let Ok(mut status) = app.intent_status.write() {
                            status.members = false;
                            status.presences = false;
                            status.degraded_reason = Some(reason.to_string());
                        }
                        app.log.warn(
                            "Bot",
                            &format!("{reason} (게이트웨이 응답: {e}) — 특권 인텐트 없이 즉시 재접속합니다."),
                        );
                        continue;
                    }
                    app.log
                        .error("Bot", &format!("게이트웨이 종료: {e} — 30초 후 재접속."));
                }
            }
            Err(e) => {
                app.log.error(
                    "Bot",
                    &format!("클라이언트 생성 실패: {e} — 30초 후 재시도."),
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}
