//! yt-dlp 연동: 검색/메타조회/컬렉션 펼치기/라디오 후보/다운로드.
//! C# ExternalMediaTools + AudioPreparationService 다운로드부 포팅.
//! 다운로드 인자는 실청취 검증된 조합(bestaudio → libopus 재인코딩, output_gain=0 보장).

use crate::models::{CsTimeSpan, ProviderKind, TrackRef};
use serde_json::Value;
use std::process::Stdio;
use tokio::process::Command;

#[derive(Clone)]
pub struct YtDlp {
    pub exe: String,
    pub browser_profile: String,
    pub cookie_file: Option<String>,
}

/* ── JS 런타임 (§EJS) ──────────────────────────────────────────────
 *
 * 유튜브는 재생 주소에 서명을 걸어 두고, 그 서명은 유튜브가 내려보내는 자바스크립트를
 * **실행해야** 풀린다. yt-dlp 는 그 실행기를 밖에서 찾는데(기본은 deno), 못 찾으면
 * 서명이 필요 없는 옛 경로로 우회하다가 결국 `HTTP Error 403: Forbidden` 을 맞는다.
 *
 * 우리 포터블에는 `deno.exe` 가 yt-dlp 바로 옆에 들어 있다. 그런데 그 폴더가 PATH 에
 * 없어서 yt-dlp 는 없는 것으로 알고 있었다 — **PATH 에 넣어 줘도 안 찾는다**(실측).
 * 그래서 위치를 인자로 못 박아 준다.
 *
 * `--js-runtimes` 는 최근에 생긴 인자라, 옛 yt-dlp 에 붙이면 **모르는 인자라고 전부
 * 실패한다.** 그래서 기동 때 한 번 물어보고, 받아 주는 판일 때만 붙인다.
 */
static JS_RUNTIME_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// yt-dlp 옆에 있는 JS 런타임. 없으면 `None` — 그때는 지금까지와 똑같이 동작한다.
fn js_runtime_beside(exe: &str) -> Option<(String, String)> {
    let dir = std::path::Path::new(exe).parent()?;
    // deno 가 yt-dlp 의 기본값이라 먼저 본다. 나머지는 있으면 쓰는 정도다.
    for name in ["deno", "node", "bun"] {
        for file in [format!("{name}.exe"), name.to_string()] {
            let path = dir.join(&file);
            if path.is_file() {
                return Some((name.to_string(), path.to_string_lossy().into_owned()));
            }
        }
    }
    None
}

/// 이 yt-dlp 가 `--js-runtimes` 를 아는지 한 번만 물어본다.
async fn probe_js_runtime_args(exe: &str) -> Vec<String> {
    let Some((name, path)) = js_runtime_beside(exe) else {
        return Vec::new();
    };
    let ok = Command::new(exe)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("--js-runtimes"))
        .unwrap_or(false);
    if !ok {
        return Vec::new();
    }
    vec!["--js-runtimes".into(), format!("{name}:{path}")]
}

/* ── 못 읽는 쿠키 창구는 이번 실행 동안 건너뛴다 ──────────────────
 *
 * 인증 체인은 브라우저 쿠키 → 쿠키 파일 → 공개 순서로 시도한다. 그런데 쿠키를 **아예
 * 못 읽는** 창구(브라우저가 켜져 있어 DB 가 잠겼거나, 윈도우 크롬처럼 요즘 복호화가
 * 막힌 경우)는 몇 번을 해도 같은 자리에서 같은 이유로 실패한다.
 *
 * 그걸 곡마다 두 번씩 다시 해 보고 있었다. 곡 하나 받을 때마다 헛도는 yt-dlp 가 둘씩
 * 붙는 셈이고, 재시도까지 겹치면 그 수가 배로 는다. 한 번 못 읽은 창구는 이번 실행
 * 동안 접어 둔다 — 브라우저를 닫고 다시 켜는 것 같은 변화는 봇을 다시 켤 때 반영된다.
 *
 * **곡이 안 받아지는 것과는 다른 실패다.** 403 같은 건 여기 해당하지 않는다 —
 * 그건 창구는 멀쩡한데 그 곡을 못 준 것이라, 접었다가는 멀쩡한 창구를 잃는다.
 */
static DEAD_COOKIE_SOURCES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn dead_sources() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    DEAD_COOKIE_SOURCES.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 쿠키를 못 읽어서 난 실패인가. **곡을 못 받은 것과 구분해야 한다.**
pub(crate) fn is_cookie_source_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    const NEEDLES: [&str; 6] = [
        "could not copy",
        "cookie database",
        "unable to decrypt",
        "could not find",
        "unsupported browser",
        "permission denied",
    ];
    NEEDLES.iter().any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod cookie_source_tests {
    use super::{is_cookie_source_failure, is_transient};

    /// 브라우저가 켜져 있으면 크롬 쿠키 DB 를 못 베낀다 — 이번 실행 내내 같다.
    #[test]
    fn a_locked_cookie_database_is_a_source_failure() {
        assert!(is_cookie_source_failure(
            "ERROR: Could not copy Chrome cookie database. See https://github.com/yt-dlp/yt-dlp/issues/7271"
        ));
    }

    /// **403 은 창구 문제가 아니다.** 이걸 창구 실패로 보면 멀쩡한 쿠키 창구를 접어 버린다.
    #[test]
    fn a_403_is_not_a_source_failure() {
        let err = "ERROR: unable to download video data: HTTP Error 403: Forbidden";
        assert!(!is_cookie_source_failure(err));
        assert!(is_transient(err), "403 은 다시 해 볼 것으로 남아야 한다");
    }
}

/// 다시 해 보면 될 것 같은 실패인가 (§10.8).
///
/// **판단을 틀리는 쪽의 대가가 다르다.** 잠깐의 문제를 영구 실패로 보면 멀쩡한 곡이
/// 넘어가고(사람이 바로 알아챈다), 반대로 보면 몇 초 더 기다렸다 똑같이 넘어간다.
/// 그래서 애매하면 다시 해 보는 쪽으로 기운다 — 다만 "없는 영상" 처럼 결과가 뻔한
/// 것들만 골라서 즉시 포기한다.
pub(crate) fn is_transient(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    // 다시 해도 소용없는 것들. 이것들이 먼저다.
    const HOPELESS: [&str; 7] = [
        "video unavailable",
        "private video",
        "members-only",
        "removed by the uploader",
        "account associated with this video has been terminated",
        "is not a valid url",
        "unsupported url",
    ];
    if HOPELESS.iter().any(|needle| lower.contains(needle)) {
        return false;
    }
    const TRANSIENT: [&str; 10] = [
        "403",
        "forbidden",
        "429",
        "too many requests",
        "timed out",
        "timeout",
        "temporary failure",
        "connection",
        // 시간 초과는 우리가 우리말로 적어 보낸다. 영어 낱말만 보면 이걸 놓친다.
        "초과해 중단",
        "실행 실패",
    ];
    TRANSIENT.iter().any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod transient_tests {
    use super::is_transient;

    /// 유튜브가 들쭉날쭉 뱉는 403 은 **다시 해 볼 값어치가 있다.**
    /// 이걸 영구 실패로 보는 바람에 멀쩡한 곡이 줄줄이 스킵됐다.
    #[test]
    fn a_403_is_worth_another_try() {
        assert!(is_transient("ERROR: unable to download video data: HTTP Error 403: Forbidden"));
        assert!(is_transient("HTTP Error 429: Too Many Requests"));
        assert!(is_transient("yt-dlp 다운로드가 10분을 초과해 중단했습니다."));
    }

    /// 없는 영상은 몇 번을 해도 없다. 기다리게 할 이유가 없다.
    #[test]
    fn a_missing_video_is_not_worth_waiting_for() {
        assert!(!is_transient("ERROR: [youtube] xxxx: Video unavailable"));
        assert!(!is_transient("ERROR: [youtube] xxxx: Private video. Sign in if you've been granted access"));
    }

    /// **없는 영상 판정이 403 판정보다 먼저다.** 두 낱말이 한 줄에 같이 나오는 응답이
    /// 실제로 있어서, 순서가 뒤집히면 죽은 영상을 붙잡고 계속 기다린다.
    #[test]
    fn hopeless_wins_over_transient_when_both_appear() {
        assert!(!is_transient("HTTP Error 403: Forbidden — Video unavailable"));
    }
}

/// 기동 때 한 번 불러 둔다. 안 불러도 동작은 같고, 첫 재생이 조금 느려질 뿐이다.
pub async fn init_js_runtime(exe: &str) -> Option<String> {
    let args = probe_js_runtime_args(exe).await;
    let described = args.get(1).cloned();
    let _ = JS_RUNTIME_ARGS.set(args);
    described
}

pub enum AuthMode {
    BrowserProfile,
    CookieFile,
    Public,
}

impl AuthMode {
    pub fn describe(&self) -> &'static str {
        match self {
            AuthMode::BrowserProfile => "browser-profile",
            AuthMode::CookieFile => "cookie-file",
            AuthMode::Public => "public",
        }
    }
}

impl YtDlp {
    /// 모든 호출 앞에 붙는 공통 인자. 지금은 JS 런타임 위치 하나뿐이다.
    /// 아직 안 물어봤으면 빈 목록 — 그때는 기능이 생기기 전과 완전히 같다.
    fn base_args(&self) -> Vec<String> {
        JS_RUNTIME_ARGS.get().cloned().unwrap_or_default()
    }

    /// C# YtDlpAuthArguments.Build 과 동일: 프로필에 ':' 가 있으면 그대로,
    /// 없으면 edge → chrome 순서로 둘 다 시도. 그 다음 쿠키 파일, 마지막은 공개 접근.
    fn auth_chain(&self) -> Vec<(AuthMode, Vec<String>)> {
        let mut chain = Vec::new();
        let profile = self.browser_profile.trim();
        if !profile.is_empty() {
            if profile.contains(':') {
                chain.push((
                    AuthMode::BrowserProfile,
                    vec!["--cookies-from-browser".into(), profile.to_string()],
                ));
            } else {
                chain.push((
                    AuthMode::BrowserProfile,
                    vec!["--cookies-from-browser".into(), format!("edge:{profile}")],
                ));
                chain.push((
                    AuthMode::BrowserProfile,
                    vec!["--cookies-from-browser".into(), format!("chrome:{profile}")],
                ));
            }
        }
        if let Some(file) = &self.cookie_file {
            if !file.trim().is_empty() {
                chain.push((AuthMode::CookieFile, vec!["--cookies".into(), file.clone()]));
            }
        }
        chain.push((AuthMode::Public, Vec::new()));
        // 이번 실행에서 쿠키를 못 읽는 것으로 판명된 창구는 뺀다. 공개 접근은 쿠키가
        // 없으니 걸러질 일이 없다 — 마지막 폴백이 사라지는 일은 생기지 않는다.
        let dead = dead_sources().lock().unwrap();
        chain.retain(|(_, args)| args.is_empty() || !dead.contains(&args.join(" ")));
        chain
    }

    /// 메타 조회 1회 실행. 30초 타임아웃 — 초과 시 future drop 으로 프로세스가 kill 된다.
    async fn run_json_once(&self, args: &[String]) -> Option<Value> {
        let mut full = self.base_args();
        full.extend_from_slice(args);
        let fut = Command::new(&self.exe)
            .args(&full)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(std::time::Duration::from_secs(30), fut)
            .await
            .ok()?
            .ok()?;
        if !out.status.success() {
            return None;
        }
        serde_json::from_slice(&out.stdout).ok()
    }

    /// 메타 조회 — C# 과 동일하게 검색/조회도 인증 체인을 차례로 시도한다
    /// (유튜브 봇 차단 시 로그인 쿠키로 우회, 쿠키 만료 시 공개 접근 폴백).
    async fn run_json(&self, args: &[String]) -> Option<Value> {
        for (_mode, auth_args) in self.auth_chain() {
            let mut full: Vec<String> = auth_args;
            full.extend(args.iter().cloned());
            if let Some(v) = self.run_json_once(&full).await {
                return Some(v);
            }
        }
        None
    }

    fn entry_to_track(entry: &Value, provider: ProviderKind) -> Option<TrackRef> {
        let id = entry.get("id")?.as_str()?.to_string();
        let title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let artist = entry
            .get("artist")
            .or_else(|| entry.get("uploader"))
            .or_else(|| entry.get("channel"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let duration = entry
            .get("duration")
            .and_then(|v| v.as_f64())
            .filter(|d| *d > 0.0)
            .map(CsTimeSpan::from_secs_f64);
        let source_url = entry
            .get("webpage_url")
            .or_else(|| entry.get("url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| match provider {
                ProviderKind::SoundCloud => format!("https://soundcloud.com/{id}"),
                ProviderKind::YouTubeMusic => format!("https://music.youtube.com/watch?v={id}"),
                _ => format!("https://www.youtube.com/watch?v={id}"),
            });
        Some(TrackRef {
            provider,
            content_id: id,
            source_url,
            title,
            artist,
            duration,
            variant_key: None,
        })
    }

    /// ytsearchN — 유튜브 키워드 검색 (기존 호출부 호환용 기본 진입점).
    pub async fn search(&self, query: &str, count: usize) -> Vec<TrackRef> {
        self.search_provider(query, count, ProviderKind::YouTube)
            .await
    }

    /// 공급자별 키워드 검색 — YouTube=ytsearchN, SoundCloud=scsearchN.
    /// flat-playlist 라서 duration/artist 가 빠질 수 있으나 후보 나열엔 충분하다.
    pub async fn search_provider(
        &self,
        query: &str,
        count: usize,
        provider: ProviderKind,
    ) -> Vec<TrackRef> {
        let prefix = match provider {
            ProviderKind::SoundCloud => "scsearch",
            // YouTubeMusic 검색도 일반 ytsearch 로 — 결과 영상 ID 네임스페이스가 동일.
            _ => "ytsearch",
        };
        let target = format!("{prefix}{count}:{query}");
        let args: Vec<String> = vec![
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
            "--".into(),
            target,
        ];
        let Some(json) = self.run_json(&args).await else {
            return Vec::new();
        };
        json.get("entries")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| Self::entry_to_track(e, provider))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 단일 트랙 메타 조회.
    pub async fn inspect_track(&self, url: &str, provider: ProviderKind) -> Option<TrackRef> {
        let args: Vec<String> = vec![
            "--no-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
            "--".into(),
            url.to_string(),
        ];
        let json = self.run_json(&args).await?;
        Self::entry_to_track(&json, provider)
    }

    /// 플레이리스트/세트 펼치기.
    pub async fn expand_collection(&self, url: &str, provider: ProviderKind) -> Vec<TrackRef> {
        let args: Vec<String> = vec![
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
            "--".into(),
            url.to_string(),
        ];
        let Some(json) = self.run_json(&args).await else {
            return Vec::new();
        };
        json.get("entries")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| Self::entry_to_track(e, provider))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 자동추천 라디오/스테이션 후보 (C# BuildStationUrl 과 동일 URL 규칙).
    pub async fn station_candidates(&self, seed: &TrackRef) -> Vec<TrackRef> {
        let url = match seed.provider {
            ProviderKind::YouTube => {
                format!(
                    "https://www.youtube.com/watch?v={0}&list=RD{0}",
                    seed.content_id
                )
            }
            ProviderKind::YouTubeMusic => {
                format!(
                    "https://music.youtube.com/watch?v={0}&list=RDAMVM{0}",
                    seed.content_id
                )
            }
            ProviderKind::SoundCloud => {
                format!("{}/recommended", seed.source_url.trim_end_matches('/'))
            }
        };
        self.expand_collection(&url, seed.provider).await
    }

    /// 곡 다운로드 — 인증 fallback 체인을 따라 시도, 성공 시 실제 파일 경로 반환.
    /// 곡 하나를 받는다. 잠깐 튕긴 것뿐이면 **몇 번 더 해 본다** (§10.8).
    ///
    /// 유튜브는 같은 요청에도 `403 Forbidden` 을 들쭉날쭉 낸다 — 두 번에 한 번 꼴로
    /// 튕기다가 잠시 뒤엔 멀쩡히 주기도 한다. 예전에는 한 번 튕기면 곧장 실패로 보고
    /// 곡을 넘겨 버려서, **멀쩡한 곡이 줄줄이 스킵됐다.**
    ///
    /// 그래서 잠깐의 문제로 보이는 실패에만 쉬었다가 다시 해 본다. 없는 영상·비공개처럼
    /// 다시 해도 소용없는 실패는 그대로 넘긴다 — 기다리게 할 이유가 없다.
    pub async fn download(
        &self,
        source_url: &str,
        output_template: &str,
        remove_segments: bool,
    ) -> Result<(String, &'static str), String> {
        // 쉬는 시간을 늘려 가며 시도한다. 바로 다시 하면 같은 이유로 또 튕긴다.
        const BACKOFF_SECS: [u64; 2] = [3, 8];
        let mut last = String::new();
        for (round, wait) in BACKOFF_SECS.iter().enumerate() {
            match self
                .download_once(source_url, output_template, remove_segments)
                .await
            {
                Ok(found) => return Ok(found),
                Err(err) => {
                    if !is_transient(&err) {
                        return Err(err);
                    }
                    last = err;
                    let _ = round;
                    tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
                }
            }
        }
        // 마지막 한 번.
        match self
            .download_once(source_url, output_template, remove_segments)
            .await
        {
            Ok(found) => Ok(found),
            Err(err) => Err(if err.trim().is_empty() { last } else { err }),
        }
    }

    async fn download_once(
        &self,
        source_url: &str,
        output_template: &str,
        remove_segments: bool,
    ) -> Result<(String, &'static str), String> {
        let mut last_err = String::new();
        for (mode, auth_args) in self.auth_chain() {
            let mut args: Vec<String> = self.base_args();
            args.extend([
                "--no-playlist".into(),
                "-f".into(),
                "bestaudio".into(),
                "-x".into(),
                "--audio-format".into(),
                "opus".into(),
                "--audio-quality".into(),
                "128K".into(),
                "--newline".into(),
                "--print".into(),
                "after_move:filepath".into(),
            ]);
            // SponsorBlock: 인트로/아웃트로/비음악 구간 컷 (해당 데이터가 있는 영상에만).
            if remove_segments {
                args.push("--sponsorblock-remove".into());
                args.push("music_offtopic,intro,outro".into());
            }
            let source_key = auth_args.join(" ");
            args.extend(auth_args);
            args.push("-o".into());
            args.push(output_template.to_string());
            args.push("--".into());
            args.push(source_url.to_string());

            let fut = Command::new(&self.exe)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output();
            // 다운로드는 10분 한도 — 행 걸린 yt-dlp 가 재생 명령을 영원히 잡아두지 않게.
            let out = match tokio::time::timeout(std::time::Duration::from_secs(600), fut).await {
                Ok(r) => r.map_err(|e| format!("yt-dlp 실행 실패: {e}"))?,
                Err(_) => {
                    last_err = "yt-dlp 다운로드가 10분을 초과해 중단했습니다.".into();
                    continue;
                }
            };

            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(path) = stdout
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .rev()
                    .find(|l| std::path::Path::new(l).is_file())
                {
                    return Ok((path.to_string(), mode.describe()));
                }
                last_err = "yt-dlp 가 출력 파일 경로를 알려주지 않았습니다.".into();
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
                last_err = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
                // 쿠키를 아예 못 읽는 창구면 접어 둔다 (곡을 못 받은 것과는 다르다).
                if !source_key.is_empty() && is_cookie_source_failure(&last_err) {
                    dead_sources().lock().unwrap().insert(source_key.clone());
                }
            }
        }
        Err(last_err)
    }
}
