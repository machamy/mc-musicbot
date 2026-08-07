//! 끊김을 최소로 줄이는 종료·복구 (V3 §24).
//!
//! **먼저 솔직하게.** 프로세스 사이에 음성 연결을 넘기는 진짜 무중단은 불가능하다.
//! Discord 음성은 UDP 소켓과 암호화 세션이 프로세스 안에 있고, 봇 하나가 같은 길드에
//! 음성 연결을 두 개 들 수도 없다. 그래서 목표는 *무중단*이 아니라 **최단 중단**이다.
//!
//! 지금까지는 이랬다. `Stop-Process -Force` → 음성이 끊긴 채로 방치 → exe 복사 →
//! 재시작 → **처음부터 다시 틀거나 아예 안 들어감**. 그 사이 cloudflared 는
//! `dial tcp ...: i/o timeout` 을 몇 분씩 뱉는다(2026-08-07 실측).
//!
//! 이제는 이렇게 한다.
//!
//! 1. 종료 신호를 받는다 (Ctrl+C · Ctrl+Break · 콘솔 닫기 · SIGTERM).
//! 2. 길드마다 **지금 재생 위치**와 음성 채널을 `remote_resume` 에 적는다.
//! 3. 접속한 브라우저에 `server.restarting` 을 쏜다 — 오류 화면 대신 안내가 뜨게.
//! 4. 음성에서 깨끗이 빠진다. 안 빠지면 Discord 쪽에 유령 연결이 몇 초 남는다.
//! 5. 끝낸다.
//!
//! 다음 기동에서 [`resume_after_restart`] 가 그 기록을 읽어 **같은 채널에 다시 들어가
//! 끊긴 지점부터** 잇는다.
//!
//! 배포 스크립트는 exe 를 먼저 옆에 복사해 두고 나서 신호를 보낸다. 그래야 멈춘 시간이
//! "복사 + 기동" 이 아니라 "기동" 만 남는다.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::App;

/// 종료 절차가 시작됐는지. 웹이 이걸 보고 "곧 돌아와요" 를 내보낸다.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// 상태를 적고 음성에서 빠지는 데 줄 시간. 이 안에 못 끝내면 그냥 끝낸다 —
/// **종료가 안 되는 것이 제일 나쁘다.** 배포가 통째로 멈추고 사람이 강제 종료하게 된다.
const DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_secs(6);

pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

/// 종료 신호를 기다렸다가 정리하고 프로세스를 끝낸다. 기동 직후 한 번만 띄운다.
pub fn watch(app: Arc<App>) {
    tokio::spawn(async move {
        wait_for_signal().await;
        if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
            return; // 두 번째 신호는 무시한다. 정리 중에 또 부르면 상태가 반쪽으로 남는다.
        }
        app.log
            .info("Shutdown", "종료 신호를 받았어요. 상태를 저장하고 빠질게요.");

        // 정리가 어디서 막혀도 프로세스는 반드시 끝난다.
        match tokio::time::timeout(DRAIN_LIMIT, drain(&app)).await {
            Ok(()) => app.log.info("Shutdown", "정리를 마쳤어요."),
            Err(_) => app.log.warn(
                "Shutdown",
                "정리가 제한 시간을 넘겨서 그대로 끝냅니다. 다음 기동 때 마지막 저장 지점부터 이어져요.",
            ),
        }
        std::process::exit(0);
    });
}

/// 상태 저장 → 브라우저 안내 → 음성 이탈. 순서가 중요하다.
/// **저장이 먼저다** — 음성에서 먼저 빠지면 재생 위치를 읽을 수 없다.
async fn drain(app: &Arc<App>) {
    let guild_ids = app.db.list_known_guild_ids();
    for guild_id in guild_ids {
        let player = app.player.get_state(guild_id).await;
        let Some(item) = player.current_item.clone() else {
            // 틀고 있지 않았으면 이어 붙일 것도 없다. 옛 기록은 지운다 —
            // 안 지우면 다음 기동에 엉뚱한 옛날 곡이 되살아난다.
            app.remote.clear_resume(guild_id);
            continue;
        };
        let position = app
            .coordinator
            .current_position(guild_id)
            .await
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let channel = app.player.voice_channel_id(guild_id).await;

        app.remote.save_resume(
            guild_id,
            channel,
            &item,
            position,
            player.is_paused,
        );
        app.log.info(
            "Shutdown",
            &format!("길드 {guild_id}: {:.0}초 지점을 저장했어요.", position),
        );
    }

    // 브라우저가 오류 화면 대신 안내를 띄우게. 음성에서 빠지기 전에 보내야
    // 사람이 "왜 봇이 나갔지" 를 먼저 겪지 않는다.
    app.notify_restarting();

    for guild_id in app.db.list_known_guild_ids() {
        app.coordinator.leave_voice(app, guild_id).await;
    }
}

/// 이 플랫폼에서 "이제 끝내라" 로 볼 수 있는 신호 전부.
///
/// 윈도우는 Ctrl+C 하나만 봐서는 부족하다. 예약 작업이나 배포 스크립트가 창을 닫는
/// 방식으로 끝내면 `ctrl_close` 로 온다. 그 경우 **OS 가 주는 유예가 짧아서**
/// 정리를 오래 붙들면 그냥 죽는다 — 그래서 [`DRAIN_LIMIT`] 이 짧다.
#[cfg(windows)]
async fn wait_for_signal() {
    use tokio::signal::windows;
    let mut c = windows::ctrl_c().expect("ctrl_c 핸들러");
    let mut brk = windows::ctrl_break().expect("ctrl_break 핸들러");
    let mut close = windows::ctrl_close().expect("ctrl_close 핸들러");
    let mut shutdown = windows::ctrl_shutdown().expect("ctrl_shutdown 핸들러");
    tokio::select! {
        _ = c.recv() => {}
        _ = brk.recv() => {}
        _ = close.recv() => {}
        _ = shutdown.recv() => {}
    }
}

#[cfg(not(windows))]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM 핸들러");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// 저장해 둔 지점부터 잇는다. 게이트웨이가 준비된 뒤에 부른다.
///
/// **기록은 한 번만 쓴다.** 이어 붙이고 나면 지운다 — 안 지우면 나중에 봇이 그냥
/// 재시작했을 때도 몇 시간 전 곡이 되살아난다.
pub async fn resume_after_restart(app: &Arc<App>) {
    for guild_id in app.db.list_known_guild_ids() {
        let Some(saved) = app.remote.take_resume(guild_id) else {
            continue;
        };
        // 너무 오래된 기록은 버린다. 어제 껐던 곡이 오늘 아침에 갑자기 나오면 안 된다.
        if saved.age_hours() > 6.0 {
            app.log.info(
                "Shutdown",
                &format!("길드 {guild_id}: 저장 지점이 오래돼서 이어 붙이지 않아요."),
            );
            continue;
        }
        let Some(channel_id) = saved.voice_channel_id else {
            continue;
        };
        app.log.info(
            "Shutdown",
            &format!(
                "길드 {guild_id}: {:.0}초 지점부터 이어서 틀게요.",
                saved.position_seconds
            ),
        );
        app.resume_playback(guild_id, channel_id, saved).await;
    }
}
