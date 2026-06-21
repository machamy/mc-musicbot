//! 도구 자동 확보 — yt-dlp 가 없으면 GitHub 최신 릴리스에서 받아 toolsRoot 에 두고,
//! 우리가 관리하는(toolsRoot 안의) yt-dlp 는 주기적으로 self-update(`yt-dlp -U`) 한다.
//! ffmpeg 는 OS/빌드별 차이가 커서 자동 다운로드하지 않고, 없으면 안내만 한다.

use crate::app::App;
use crate::config::Config;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// 이 OS 의 yt-dlp 릴리스 자산 이름.
const YT_DLP_ASSET: &str = if cfg!(windows) {
    "yt-dlp.exe"
} else if cfg!(target_os = "macos") {
    "yt-dlp_macos"
} else {
    "yt-dlp"
};

/// Windows 에서 콘솔 창이 깜빡이지 않도록 CREATE_NO_WINDOW 를 단 tokio 커맨드를 만든다.
fn command(program: &str) -> tokio::process::Command {
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    tokio::process::Command::from(c)
}

/// 명령이 실제로 실행 가능한지(`<cmd> <probe_arg>` 가 성공 종료) 확인.
async fn runnable(cmd: &str, probe_arg: &str) -> bool {
    command(cmd)
        .arg(probe_arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// URL 을 파일로 받는다 (리다이렉트 자동 추적). 임시파일에 쓰고 원자적으로 교체.
async fn download_to(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let bytes = reqwest::Client::builder()
        .user_agent("mc-musicbot")
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let tmp = dest.with_extension("download.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// 시작 시 도구 확보. yt-dlp 가 없으면 받아서 `config.yt_dlp_path` 를 갱신한다.
/// (로그 서비스가 아직 없는 단계라 콘솔로 출력한다.)
pub async fn ensure_tools(config: &mut Config) {
    // ffmpeg — 자동 다운로드는 하지 않고, 없으면 설치 방법을 안내만 한다.
    if !runnable(&config.ffmpeg_path, "-version").await {
        eprintln!(
            "[tools] ffmpeg 를 찾지 못했습니다. PATH 에 두거나 '{}' 에 ffmpeg 실행파일을 넣으세요. \
             (설치: winget install Gyan.FFmpeg  /  apt install ffmpeg  /  brew install ffmpeg)",
            config.tools_root.display()
        );
    }

    // yt-dlp — 있으면 그대로 쓰고, 없으면 GitHub 최신 릴리스에서 toolsRoot 로 받는다.
    if runnable(&config.yt_dlp_path, "--version").await {
        return;
    }
    let target = config.tools_root.join(YT_DLP_ASSET);
    let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{YT_DLP_ASSET}");
    println!(
        "[tools] yt-dlp 를 찾지 못해 다운로드합니다: {url} -> {}",
        target.display()
    );
    match download_to(&url, &target).await {
        Ok(()) => {
            config.yt_dlp_path = target.to_string_lossy().to_string();
            println!("[tools] yt-dlp 다운로드 완료: {}", target.display());
        }
        Err(e) => eprintln!(
            "[tools] yt-dlp 다운로드 실패: {e}. 수동 설치가 필요합니다 (예: pip install -U yt-dlp)."
        ),
    }
}

/// 우리가 관리하는(toolsRoot 안의) yt-dlp 를 시작 직후 1회 + 24시간마다 self-update.
/// 시스템/PATH 의 yt-dlp 는 사용자가 패키지매니저로 관리하므로 건드리지 않는다.
/// (YouTube 가 바뀌어 다운로드가 깨질 때 yt-dlp 최신화가 가장 흔한 해법이라 자동화한다.)
pub fn spawn_auto_update(app: Arc<App>) {
    if !app.db.load_global_settings().auto_update_tools {
        return;
    }
    let ytdlp = app.config.yt_dlp_path.clone();
    if !Path::new(&ytdlp).starts_with(&app.config.tools_root) {
        app.log.info(
            "Tools",
            "yt-dlp 가 toolsRoot 밖(PATH/시스템)이라 자동 업데이트는 건너뜁니다 (직접 관리).",
        );
        return;
    }
    let log = app.log.clone();
    tokio::spawn(async move {
        loop {
            match command(&ytdlp)
                .arg("-U")
                .stdin(std::process::Stdio::null())
                .output()
                .await
            {
                Ok(out) => {
                    let txt = String::from_utf8_lossy(&out.stdout);
                    let line = txt
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("(출력 없음)");
                    log.info("Tools", &format!("yt-dlp 자동 업데이트: {}", line.trim()));
                }
                Err(e) => log.warn("Tools", &format!("yt-dlp -U 실패: {e}")),
            }
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
}
