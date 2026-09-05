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

/* ── yt-dlp 가 얼마나 묵었는지 ────────────────────────────────────
 *
 * 유튜브가 추출 방식을 바꾸면 낡은 yt-dlp 는 곡을 못 받는다. 그런데 **우리가 관리하지
 * 않는 yt-dlp(PATH·winget 설치본)는 낡아도 아무 말이 없었다** — 아래 함수가 "직접 관리"
 * 한 줄만 남기고 곧장 나가 버렸기 때문이다. 실제로 6개월(약 172일) 묵은 판이 그대로
 * 돌면서 곡을 403 으로 떨어뜨리고 있었고, 그 사실이 어디에도 안 적혔다.
 *
 * 업데이트는 여전히 안 한다(사용자가 직접 관리하는 것을 우리가 건드리면 안 된다).
 * 다만 **보기는 한다.** 보는 것은 아무것도 바꾸지 않는다.
 */
const YT_DLP_STALE_DAYS: i64 = 45;

/// `yt-dlp --version` 이 뱉는 날짜를 읽는다.
///
/// 세 가지 모양이 온다 — 안정판 `2026.03.17`, 나이틀리 `2026.03.17.232134`,
/// 배포 패치 `2026.3.17-1`. 하나라도 어긋나면 `None` 을 돌려 **조용히 넘어간다**:
/// 잘못 읽고 엉뚱한 경고를 내는 것보다 아무 말 안 하는 편이 낫다.
fn parse_ytdlp_date(version: &str) -> Option<chrono::NaiveDate> {
    let head = version.trim().lines().next()?.trim();
    let mut parts = head.split('.');
    let mut next_num = || -> Option<i32> {
        let raw = parts.next()?;
        let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    };
    let (y, m, d) = (next_num()?, next_num()?, next_num()?);
    if !(2000..=2999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    chrono::NaiveDate::from_ymd_opt(y, m as u32, d as u32)
}

/// 버전 문자열 옆에 붙일 한 조각. 날짜를 못 읽으면 빈 문자열이라 화면이 그대로다.
/// **HTML 을 직접 만든다** — 부르는 쪽(`/tools`)이 이 값을 이스케이프하지 않고 쓴다.
/// 그래서 여기서는 바깥에서 온 문자열을 절대 섞지 않는다(숫자와 고정 문구뿐).
pub fn version_age_note(version: &str) -> String {
    let Some(date) = parse_ytdlp_date(version) else {
        return String::new();
    };
    let days = (chrono::Local::now().date_naive() - date).num_days();
    if days >= YT_DLP_STALE_DAYS {
        format!(r#"<span class="pill stop">오래됨 · {days}일</span>"#)
    } else {
        format!(r#"<span class="kv">{days}일 전</span>"#)
    }
}

/// `yt-dlp --version` 첫 줄. 실행 못 하면 `None`.
pub async fn ytdlp_version(exe: &str) -> Option<String> {
    let out = command(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// 우리가 관리하는(toolsRoot 안의) yt-dlp 를 시작 직후 1회 + 24시간마다 self-update.
/// 시스템/PATH 의 yt-dlp 는 사용자가 패키지매니저로 관리하므로 **갱신은** 건드리지 않는다.
/// (YouTube 가 바뀌어 다운로드가 깨질 때 yt-dlp 최신화가 가장 흔한 해법이라 자동화한다.)
///
/// **버전 확인은 두 갈래 모두에 한다.** 관리형이어도 `-U` 가 권한 부족으로 조용히 실패하면
/// 낡은 채로 남는데, 예전에는 그것도 Info 한 줄에 묻혔다.
pub fn spawn_auto_update(app: Arc<App>) {
    let ytdlp = app.config.yt_dlp_path.clone();
    // 예전에는 여기서 `return` 했다. 이제는 "갱신은 안 한다" 는 표시만 남기고 계속 간다.
    let managed = Path::new(&ytdlp).starts_with(&app.config.tools_root);
    if !managed {
        app.log.info(
            "Tools",
            "yt-dlp 가 toolsRoot 밖(PATH/시스템)이라 자동 업데이트는 건너뜁니다 (직접 관리).",
        );
    }
    let log = app.log.clone();
    let db = app.db.clone();
    tokio::spawn(async move {
        // 24시간마다 도는 고리에서 같은 잔소리를 매번 하지 않는다.
        let mut warned = false;
        loop {
            // 갱신 설정은 **고리 안에서** 본다 — 운영 중에 꺼도 다음 바퀴에 반영된다.
            if managed && db.load_global_settings().auto_update_tools {
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
            }

            if !warned {
                let version = ytdlp_version(&ytdlp).await;
                let age = version
                    .as_deref()
                    .and_then(parse_ytdlp_date)
                    .map(|d| (chrono::Local::now().date_naive() - d).num_days());
                /* yt-dlp 가 **스스로** 낡았다고 말했으면 날짜 산술보다 그쪽이 정확하다.
                 * 곡을 한 곡이라도 받아 봤어야 이 신호가 잡히므로, 날짜 쪽도 같이 본다. */
                let says_so = crate::media::ytdlp::observed_outdated();
                let too_old = age.is_some_and(|d| d >= YT_DLP_STALE_DAYS);
                if says_so || too_old {
                    let shown = version.as_deref().unwrap_or("버전 미상");
                    let how_old = match age {
                        Some(d) => format!("{d}일"),
                        None => "얼마나 됐는지 모를 만큼".into(),
                    };
                    let fix = if managed {
                        "자동 업데이트가 도는데도 그대로예요 — tools 폴더 쓰기 권한을 확인하세요."
                    } else {
                        "(winget upgrade yt-dlp.yt-dlp  /  pip install -U yt-dlp)"
                    };
                    log.warn(
                        "Tools",
                        &format!(
                            "yt-dlp 가 {shown} 로 {how_old} 됐어요. 유튜브 추출기가 그동안 여러 번 \
                             바뀌었을 수 있어요 — 곡이 안 받아지면 이걸 가장 먼저 의심하세요. {fix}"
                        ),
                    );
                    warned = true;
                }
            }
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
}

#[cfg(test)]
mod stale_tests {
    use super::{parse_ytdlp_date, YT_DLP_STALE_DAYS};

    /// `--version` 이 내는 세 가지 모양을 다 읽는다.
    /// **여기가 `None` 으로 떨어지면 낡은 도구가 또 조용히 지나간다.**
    #[test]
    fn every_shape_of_version_string_is_read() {
        let want = chrono::NaiveDate::from_ymd_opt(2026, 3, 17).unwrap();
        for raw in ["2026.03.17", "2026.03.17.232134", "2026.3.17-1", "2026.03.17\n다른 줄"] {
            assert_eq!(parse_ytdlp_date(raw), Some(want), "못 읽었어요: {raw}");
        }
    }

    /// 못 읽는 것은 조용히 넘어간다 — 엉뚱한 경고보다 침묵이 낫다.
    #[test]
    fn nonsense_is_ignored_rather_than_guessed() {
        for raw in ["", "나이틀리", "abc.def.ghi", "2026.13.17", "1999.03.17", "2026.03"] {
            assert_eq!(parse_ytdlp_date(raw), None, "이건 못 읽어야 해요: {raw}");
        }
    }

    /// 8/31 에 실제로 돌던 판. **이게 안 걸리면 이번 사고가 그대로 되풀이된다.**
    #[test]
    fn the_version_that_actually_broke_playback_is_caught() {
        let d = parse_ytdlp_date("2026.03.17").unwrap();
        let age = (chrono::NaiveDate::from_ymd_opt(2026, 9, 5).unwrap() - d).num_days();
        assert!(age >= YT_DLP_STALE_DAYS, "{age}일 된 판을 안 잡아요");
    }

    /// 안정판을 한 판 건너뛴 정도는 봐준다 — 잔소리가 배경 소음이 되면 진짜 경고를 못 본다.
    #[test]
    fn keeping_reasonably_current_is_not_nagged() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 8, 19).unwrap();
        let age = (chrono::NaiveDate::from_ymd_opt(2026, 9, 5).unwrap() - d).num_days();
        assert!(age < YT_DLP_STALE_DAYS, "{age}일밖에 안 된 판에 잔소리해요");
    }
}
