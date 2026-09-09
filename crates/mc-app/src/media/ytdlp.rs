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
    /// 다운로드 재시도를 몇 바퀴 돌까 (`download` 참고). 기본은 3 — 예전과 같다.
    /// 연속 실패 중이거나 미리 받는 중이면 부르는 쪽이 1로 줄인다.
    pub retry_rounds: usize,
}

/// 연속 실패 횟수에 따라 사다리를 몇 바퀴 돌지.
///
/// **첫 실패는 반드시 3이다.** 여기가 1로 바뀌면 v4.14 가 잡은 "들쭉날쭉한 403 하나에
/// 멀쩡한 곡이 줄줄이 스킵되는" 버그가 되살아난다.
pub fn retry_rounds(consecutive_fails: u32) -> usize {
    if consecutive_fails == 0 {
        3
    } else {
        1
    }
}

impl YtDlp {
    /// 이 한 번의 내려받기에만 다른 사다리 길이를 쓴다. 값 타입이라 복제해서 넘긴다.
    pub fn with_retry_rounds(mut self, rounds: usize) -> Self {
        self.retry_rounds = rounds;
        self
    }
}

#[cfg(test)]
mod retry_budget_tests {
    use super::retry_rounds;

    /// 처음 튕긴 곡은 원래대로 다 해 본다 — 이게 v4.14 의 존재 이유다.
    #[test]
    fn a_first_failure_still_gets_the_full_ladder() {
        assert_eq!(retry_rounds(0), 3);
    }

    /// 앞 곡이 이미 사다리를 다 돌고 실패했다. 같은 실험을 또 하지 않는다.
    #[test]
    fn a_streak_collapses_the_ladder() {
        for n in 1..=4 {
            assert_eq!(retry_rounds(n), 1, "연속 {n}회째인데 사다리를 또 돌아요");
        }
    }

    /// 라운드는 0이 될 수 없다 — 0이면 곡을 아예 시도조차 안 한다.
    #[test]
    fn rounds_never_reach_zero() {
        for n in 0..100 {
            assert!(retry_rounds(n) >= 1);
        }
    }
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
    const NEEDLES: [&str; 8] = [
        "could not copy",
        "cookie database",
        "unable to decrypt",
        "could not find",
        "unsupported browser",
        "permission denied",
        /* **엣지가 내는 문구를 못 알아보고 있었다.**
         *
         * 실서버가 매번 `Failed to decrypt with DPAPI` 로 실패하는데, 위의
         * `unable to decrypt` 로는 안 걸린다("failed" 와 "unable" 은 다른 낱말이다).
         * 그래서 이 창구는 **한 번도 접힌 적이 없고** 곡마다·조회마다 다시 시도해
         * 왔다. 실측 1.9초씩이다. */
        "failed to decrypt",
        "dpapi",
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

    /* **실서버 엣지가 내는 문구.** 이걸 못 알아보는 바람에 이 창구가 한 번도
     * 접히지 않았고, 검색·라디오·메타조회마다 1.9초씩 헛돌았다. */
    #[test]
    fn a_dpapi_failure_is_a_source_failure() {
        assert!(is_cookie_source_failure(
            "ERROR: Failed to decrypt with DPAPI. See  https://github.com/yt-dlp/yt-dlp/issues/10927"
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
    const HOPELESS: [&str; 9] = [
        "video unavailable",
        "private video",
        "members-only",
        "removed by the uploader",
        "account associated with this video has been terminated",
        "is not a valid url",
        "unsupported url",
        /* **끝난 방송과 아직 처리 중인 영상.**
         *
         * 재생 직전의 라이브 가드(`coordinator`)는 `is_live` 를 보는데, 끝난 방송은
         * `live_status` 가 `was_live` 라 그 값이 false 다 — 가드를 그냥 통과한다.
         * 여기서 안 잡으면 사다리를 30초 넘게 헛돈다.
         *
         * "일시적" 의 기준은 절대 시간이 아니라 **우리 백오프 창(11초) 안에 풀리는가** 다.
         * "나중에 다시 오라" 고 말하는 상태가 11초 만에 풀릴 리 없다.
         *
         * `we're` 의 아포스트로피는 일부러 뺐다 — 유튜브가 `'` 와 `’` 를 둘 다 쓴다.
         * 한쪽만 맞추면 나머지 절반을 **조용히** 놓친다. */
        "live stream recording is not available",
        "processing this video",
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

/* ── yt-dlp 가 하는 말을 흘려듣지 않는다 ──────────────────────────
 *
 * yt-dlp 는 스스로 두 가지를 stderr 에 적어 보낸다. **낡았다** 는 잔소리와, 유튜브 서명을
 * **어떤 런타임으로 풀고 있는지**(`[youtube] [jsc:deno] ...`). 둘 다 우리가 제일 알고 싶던 것이다.
 *
 * 그런데 그 버퍼를 매번 버리고 있었다 — 다운로드가 성공하면 stderr 를 읽지도 않았고,
 * 실패해도 `rev().take(3)` 으로 마지막 3줄만 남겼다. 낡았다는 경고는 두 줄짜리라 그 3칸
 * 중 2칸을 차지하면서도 정작 머리줄("You are using an outdated version…")은 잘려 나갔다.
 * 그래서 로그에는 "업데이트하라" 는 꼬리만 남고 **왜 그 꼬리가 붙었는지는 알 수 없었다.**
 *
 * 프로세스를 새로 띄울 필요가 없다. 이미 손에 있는 것을 지나가면서 한 번 훑기만 하면 된다.
 * 관찰만 하고 판단은 안 한다 — 무엇을 경고할지는 `tools.rs` 가 정한다.
 */
static OBSERVED_OUTDATED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
static OBSERVED_JSC: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// yt-dlp 가 남긴 stderr 를 지나가며 훑는다. 처음 본 것만 기억한다.
pub(crate) fn observe_stderr(stderr: &str) {
    let lower = stderr.to_ascii_lowercase();
    // 판마다 문구가 조금씩 다르다. 여러 개를 보되 하나만 걸려도 낡은 것으로 친다.
    const OUTDATED: [&str; 4] = [
        "you are using an outdated version",
        "yt-dlp is out of date",
        "--update-to",
        r#"run "yt-dlp --update""#,
    ];
    if OUTDATED.iter().any(|n| lower.contains(n)) {
        let _ = OBSERVED_OUTDATED.set(());
    }
    /* `[youtube] [jsc:deno] Solving JS challenges using deno` 에서 런타임 이름만 꺼낸다.
     * **이게 있으면 서명이 실제로 풀리고 있다는 뜻이다** — 우리가 경로를 못 박아 주지
     * 못했어도 yt-dlp 가 스스로 찾아 쓰고 있는 것이다. */
    if OBSERVED_JSC.get().is_none() {
        let name = stderr
            .split("[jsc:")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(str::trim)
            // 런타임 이름은 짧다. 길면 `[jsc:` 를 우연히 품은 다른 줄을 주운 것이다.
            .filter(|name| !name.is_empty() && name.len() <= 16);
        if let Some(name) = name {
            let _ = OBSERVED_JSC.set(name.to_string());
        }
    }
}

/// yt-dlp 가 스스로 "낡았다" 고 말한 적이 있는가.
pub fn observed_outdated() -> bool {
    OBSERVED_OUTDATED.get().is_some()
}

/// yt-dlp 가 실제로 쓰고 있는 JS 런타임 이름. 아직 한 곡도 안 받았으면 `None`.
pub fn observed_jsc() -> Option<String> {
    OBSERVED_JSC.get().cloned()
}

/// **우리가 못 박아 준** JS 런타임 (`--js-runtimes deno:<경로>` 의 뒷부분). 못 박았으면
/// 곡을 안 틀어 봐도 알 수 있다 — `observed_jsc` 는 실제로 받아 봐야 채워지므로,
/// 화면은 이것을 **먼저** 봐야 한다.
pub fn js_runtime_status() -> Option<String> {
    JS_RUNTIME_ARGS.get()?.get(1).cloned()
}

/// 처음 한 번만 `Some` 을 돌려준다 — 기동 로그의 "다시 적힙니다" 를 실제로 지키는 자리다.
/// 곡마다 같은 줄을 쏟아 내면 그건 그것대로 소음이라 한 번으로 끊는다.
pub fn take_jsc_notice() -> Option<String> {
    static TOLD: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let name = OBSERVED_JSC.get()?;
    TOLD.set(()).ok()?;
    Some(name.clone())
}

#[cfg(test)]
mod observe_tests {
    use super::*;

    /// 8/31 로그에 실제로 남아 있던 꼬리. **이걸 못 알아보면 6개월 묵은 도구가 또 조용히 지난다.**
    #[test]
    fn the_update_nag_we_actually_saw_is_recognised() {
        // 정적에 기록하므로 앞선 상태를 전제하지 않는다 — 테스트 순서에 기대면 흔들린다.
        observe_stderr(
            "         Run \"yt-dlp --update\" or \"yt-dlp -U\" to update.\n\
             ERROR: unable to download video data: HTTP Error 403: Forbidden",
        );
        assert!(observed_outdated());
    }

    /// **런타임을 쓰고 있다는 사실은 stderr 에만 있다.** 이걸 놓치면 기동 로그가
    /// "못 찾았다" 고 단정하는 것을 바로잡을 근거가 없어진다.
    #[test]
    fn the_jsc_line_tells_us_which_runtime_is_actually_used() {
        observe_stderr("[youtube] [jsc:deno] Solving JS challenges using deno");
        assert_eq!(observed_jsc().as_deref(), Some("deno"));
    }

    /// 평범한 출력에서 아무것이나 주워 담지 않는다.
    #[test]
    fn ordinary_output_teaches_us_nothing() {
        observe_stderr("[download] 100% of 3.63MiB in 00:00:01");
        // 위 두 테스트가 이미 채웠을 수 있으므로 값 자체가 아니라 **오염**만 본다.
        assert_ne!(observed_jsc().as_deref(), Some("download"));
    }
}

/* ── 실패 사유를 사람 말로 (§10.8) ────────────────────────────────
 *
 * **곡이 왜 사라졌는지 아무 데도 안 적혀 있었다.** 채널에는 제목만 나가고
 * (`⚠️ 재생에 실패해서 다음 곡으로 넘어가요: {제목}`), 활동 기록에는 사유 자리에
 * 아예 `None` 이 들어갔다. 그래서 듣던 사람도, 나중에 들여다보는 운영자도
 * "그냥 곡이 없어졌다" 는 것 말고는 알 방법이 없었다.
 *
 * 두 군데가 필요로 하는 길이가 다르다. 채널은 흐르는 대화라 괄호 한 조각이면 되고,
 * 리모컨은 사후에 들여다보는 원장이라 한 문장이 낫다. 그래서 **한 함수가 둘 다** 낸다 —
 * 갈라 놓으면 한쪽만 고쳐져서 서로 다른 말을 하게 된다.
 *
 * 원문은 어디에도 버리지 않는다. 활동 기록에는 이 요약과 별개로 원문이 그대로 실린다.
 */
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FailCode {
    /// 채널 괄호 안에 들어갈 짧은 말.
    pub short: &'static str,
    /// 리모컨에 보일 한 문장.
    pub long: &'static str,
}

/* ── 원문을 사람에게 보여 주기 전에 경로를 지운다 ────────────────
 *
 * 리모컨은 서버에 있는 사람 누구나 본다. 그런데 도구가 뱉는 오류에는 호스트의 절대 경로가
 * 섞여 나온다(`C:\Users\macham\Desktop\musicbot-portable-...\tools\yt-dlp.exe`). 그게 그대로
 * 나가면 남의 PC 안쪽 구조를 알려 주는 셈이다.
 *
 * **그렇다고 원문을 안 보여 주는 것은 답이 아니었다.** 처음엔 그렇게 했는데, 그러면
 * "자세히 보기" 로 볼 것이 없어진다. 지울 것만 지우고 나머지는 그대로 보여 준다 —
 * 파일 이름은 남긴다(`…\yt-dlp.exe`). 무엇이 문제인지 아는 데는 그게 필요하다.
 */
static PATH_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

/// 절대 경로를 `…\마지막이름` 으로 줄인다. 사람에게 보여 줄 원문에만 쓴다.
pub fn redact_paths(text: &str) -> String {
    let re = PATH_RE.get_or_init(|| {
        // 드라이브 문자 경로(C:\...), UNC(\\서버\...), 유닉스 홈(/home/..., /Users/...).
        regex::Regex::new(
            r#"(?x)
              [A-Za-z]:[\\/][^\s"'|,;)\]}]*      # C:\... 또는 C:/...
            | \\\\[^\s"'|,;)\]}]+                # \\서버\공유\...
            | /(?:home|Users|root)/[^\s"'|,;)\]}]*  # /home/me/...
            "#,
        )
        .expect("경로 정규식")
    });
    re.replace_all(text, |caps: &regex::Captures| {
        let whole = &caps[0];
        // 마지막 조각(파일 이름)만 남긴다. 그것마저 없으면 통째로 가린다.
        match whole.rsplit(['\\', '/']).find(|s| !s.is_empty()) {
            Some(name) => format!("…/{name}"),
            None => "…".to_string(),
        }
    })
    .into_owned()
}

#[cfg(test)]
mod redact_tests {
    use super::redact_paths;

    /// **호스트 경로가 리모컨으로 새면 안 된다.** 이 저장소의 실제 배포 경로로 검사한다.
    #[test]
    fn absolute_paths_are_reduced_to_their_file_name() {
        let raw = r"yt-dlp 실행 실패: C:\Users\someone\Desktop\musicbot-portable\tools\yt-dlp.exe 없음";
        let out = redact_paths(raw);
        assert!(!out.contains("someone"), "사용자 이름이 남았어요: {out}");
        assert!(!out.contains("Desktop"), "폴더 구조가 남았어요: {out}");
        assert!(out.contains("yt-dlp.exe"), "무엇이 문제인지까지 지웠어요: {out}");
    }

    /// 유닉스와 UNC 도 같이 가린다.
    #[test]
    fn unix_and_unc_paths_are_covered() {
        assert!(!redact_paths("/home/someone/bot/data/x.opus 없음").contains("someone"));
        assert!(!redact_paths(r"\\NAS\music\a.opus 없음").contains("NAS"));
    }

    /// **경로가 아닌 것은 건드리지 않는다.** 실서버에서 실제로 난 오류들이 그대로 남아야 한다.
    #[test]
    fn ordinary_errors_pass_through_untouched() {
        for raw in [
            "ERROR: [youtube] xgQHpTdw6Kg: Video unavailable",
            "ERROR: unable to download video data: HTTP Error 403: Forbidden",
            "ERROR: [youtube] s29lt0E27Mc: Private video",
        ] {
            assert_eq!(redact_paths(raw), raw, "멀쩡한 오류를 건드렸어요");
        }
    }

    /// 시각처럼 콜론이 든 평범한 글자를 경로로 오해하지 않는다.
    #[test]
    fn a_timestamp_is_not_a_path() {
        let raw = "01:39:47 에 실패";
        assert_eq!(redact_paths(raw), raw);
    }
}

/// 오류 원문을 사람이 읽는 짧은 말로 접는다.
///
/// **판정 순서가 규칙이다.** 위에서부터 먼저 걸리는 것이 이긴다. 한 응답에 여러 낱말이
/// 같이 나오는 일이 실제로 있어서(`403 ... Video unavailable`), 결과가 뻔한 쪽을 위에 둔다.
pub fn fail_code(error: &str) -> FailCode {
    let lower = error.to_ascii_lowercase();
    // (판정 낱말, 짧은 말, 한 문장). 위가 이긴다.
    const TABLE: [(&str, &str, &str); 17] = [
        ("live stream recording is not available",
         "다시보기 없음", "끝난 생방송이라 다시보기가 남아 있지 않아요."),
        ("processing this video",
         "처리 중", "유튜브가 아직 이 영상을 처리하고 있어요. 나중에는 될 수도 있어요."),
        ("video unavailable",
         "영상 없음", "유튜브에서 볼 수 없는 영상이에요."),
        ("private video",
         "비공개", "비공개 영상이라 받을 수 없어요."),
        ("members-only",
         "멤버 전용", "채널 멤버에게만 열린 영상이에요."),
        /* 아래 셋은 **실서버 로그에서 실제로 나온** 것들이다(2026-08-18~09-05 40건).
         * 처음 표를 만들 때 로컬 dev 로그만 보고 짜서 이 셋이 빠져 있었고,
         * 그러면 정작 사람들이 제일 자주 보는 실패가 `알 수 없음` 으로 나갔다. */
        ("only available to music premium",
         "프리미엄 전용", "유튜브 뮤직 프리미엄에서만 들을 수 있는 곡이에요."),
        ("confirm your age",
         "연령 확인", "성인 인증이 필요한 영상이라 받을 수 없어요."),
        ("removed for violating",
         "약관 위반 삭제", "유튜브 약관 위반으로 지워진 영상이에요."),
        ("removed by the uploader",
         "올린이 삭제", "올린 사람이 지운 영상이에요."),
        ("account associated with this video has been terminated",
         "계정 정지", "영상을 올린 계정이 정지됐어요."),
        ("requested format is not available",
         "형식 없음", "받을 수 있는 소리 형식이 없어요. yt-dlp 가 낡으면 이렇게 나오기도 해요."),
        ("429",
         "요청 과다", "유튜브가 요청이 너무 잦다고 잠시 막았어요."),
        ("too many requests",
         "요청 과다", "유튜브가 요청이 너무 잦다고 잠시 막았어요."),
        // 403 은 429 보다 아래다 — 둘이 같이 나오면 속도 제한 쪽이 더 쓸모 있는 말이다.
        ("403",
         "403", "유튜브가 거절했어요(403). yt-dlp 가 낡았을 때 가장 흔해요."),
        ("forbidden",
         "403", "유튜브가 거절했어요(403). yt-dlp 가 낡았을 때 가장 흔해요."),
        ("초과해 중단",
         "시간 초과", "정해진 시간 안에 다 받지 못했어요."),
        /* 웹 재생기 갈래는 도구가 준 오류가 없다 — 우리가 우리말로 적어 보낸다.
         * 그 문장도 여기를 지나야 리모컨이 같은 말을 한다. */
        ("길이를 알 수 없",
         "길이 모름", "곡 길이를 알 수 없어 웹 재생기가 틀 수 없었어요."),
    ];
    for (needle, short, long) in TABLE {
        if lower.contains(needle) {
            return FailCode { short, long };
        }
    }
    /* **빈 문자열이 실제로 올라온다.** `download_once` 의 `last_err` 은 인증 창구를 다
     * 돌고도 아무 stderr 를 못 받으면 빈 채로 남는다. 그때 `()` 만 덩그러니 붙으면
     * 오히려 더 헷갈리니 말로 적는다. */
    if error.trim().is_empty() {
        return FailCode {
            short: "사유 없음",
            long: "왜 실패했는지 도구가 아무 말도 남기지 않았어요.",
        };
    }
    FailCode {
        short: "알 수 없음",
        long: "분류되지 않은 오류예요. 자세한 내용은 원문을 보세요.",
    }
}

#[cfg(test)]
mod fail_code_tests {
    use super::{fail_code, is_transient};

    /// **끝난 방송의 다시보기는 다시 물어봐도 안 준다.**
    /// 재생 직전의 `is_live` 가드는 "방송 중" 만 막는다 — 끝난 방송은 `is_live=false` 라
    /// 그 가드를 통과하고, 여기서 안 잡으면 사다리를 30초 넘게 헛돈다.
    #[test]
    fn a_finished_live_archive_is_not_worth_waiting_for() {
        assert!(!is_transient(
            "ERROR: [youtube] xxxx: This live stream recording is not available."
        ));
        assert!(!is_transient(
            "ERROR: [youtube] xxxx: We're processing this video. Check back later."
        ));
    }

    /// **아포스트로피를 믿지 않는다.** 유튜브는 `'` 와 `’` 를 둘 다 쓴다.
    /// 한쪽만 맞추면 나머지 절반이 **조용히** 예전처럼 사다리를 돈다 — 제일 나쁜 종류의 버그다.
    #[test]
    fn the_processing_notice_matches_either_apostrophe() {
        assert!(!is_transient("We're processing this video. Check back later."));
        assert!(!is_transient("We\u{2019}re processing this video. Check back later."));
    }

    /// 403 과 같이 나와도 결과가 뻔한 쪽이 이긴다 (기존 불변식의 확장).
    #[test]
    fn a_live_archive_beats_a_403_in_the_same_tail() {
        assert!(!is_transient(
            "HTTP Error 403: Forbidden | This live stream recording is not available."
        ));
    }

    /// 로그에 실제로 남아 있던 문구들이 전부 사람 말이 된다.
    /// **여기가 `알 수 없음` 으로 떨어지면** 사용자는 또 이유를 못 본다.
    ///
    /// 아래 목록은 지어낸 것이 아니라 **실서버 로그 19일치(2026-08-18~09-05, 실패 40건)와
    /// 로컬 dev 로그에서 그대로 꺼낸 원문**이다. 새 문구를 만나면 여기 추가한다.
    #[test]
    fn the_errors_we_actually_saw_all_get_a_name() {
        for (raw, want) in [
            // ── 실서버에서 실제로 난 것 (많은 순) ──
            ("ERROR: [youtube] xgQHpTdw6Kg: Video unavailable", "영상 없음"),
            ("ERROR: [youtube] s29lt0E27Mc: Private video", "비공개"),
            ("ERROR: [youtube] 7Ia6meT4fKU: This video is only available to Music Premium members", "프리미엄 전용"),
            ("ERROR: [youtube] L4rJPHUCu_4: Sign in to confirm your age. Use --cookies-from-browser or --cookies for the authentication.", "연령 확인"),
            ("ERROR: [youtube] 2ctTxnkRbIo: This video has been removed for violating YouTube's Terms of Service", "약관 위반 삭제"),
            // ── 로컬 dev 에서 난 것 ──
            ("ERROR: unable to download video data: HTTP Error 403: Forbidden", "403"),
            ("ERROR: [youtube] jfKfPfyJRdk: This live stream recording is not available.", "다시보기 없음"),
            ("ERROR: [youtube] DWcJFNfaw9c: We're processing this video. Check back later.", "처리 중"),
            ("ERROR: [youtube] 7NOSDKb0HlU: Requested format is not available.", "형식 없음"),
            ("yt-dlp 다운로드가 10분을 초과해 중단했습니다.", "시간 초과"),
        ] {
            assert_eq!(fail_code(raw).short, want, "이 오류를 못 알아봤어요: {raw}");
        }
    }

    /// 실서버 실패는 전부 **다시 해도 소용없는** 것들이다. 사다리를 돌면 안 된다 —
    /// 죽은 영상 하나에 30초씩 매달리면 그 서버는 그동안 아무 소리도 못 낸다.
    #[test]
    fn the_real_world_failures_never_spin_the_ladder() {
        for raw in [
            "ERROR: [youtube] xgQHpTdw6Kg: Video unavailable",
            "ERROR: [youtube] s29lt0E27Mc: Private video",
            "ERROR: [youtube] 7Ia6meT4fKU: This video is only available to Music Premium members",
            "ERROR: [youtube] L4rJPHUCu_4: Sign in to confirm your age.",
            "ERROR: [youtube] 2ctTxnkRbIo: This video has been removed for violating YouTube's Terms of Service",
        ] {
            assert!(!is_transient(raw), "죽은 영상을 붙잡고 재시도해요: {raw}");
        }
    }

    /// **속도 제한이 403 보다 먼저다.** 둘이 같이 나오는 응답이 있는데,
    /// "요청이 잦다" 가 "거절당했다" 보다 사람에게 쓸모 있는 말이다.
    #[test]
    fn rate_limiting_wins_over_a_bare_403() {
        assert_eq!(fail_code("HTTP Error 429: Too Many Requests (403)").short, "요청 과다");
    }

    /// **빈 오류가 실제로 올라온다.** `()` 만 덩그러니 붙으면 더 헷갈린다.
    #[test]
    fn an_empty_error_still_says_something() {
        assert_eq!(fail_code("").short, "사유 없음");
        assert_eq!(fail_code("   ").short, "사유 없음");
    }

    /// 모르는 오류도 반드시 무언가를 돌려준다 — 빈 괄호가 나가면 안 된다.
    #[test]
    fn an_unknown_error_never_yields_an_empty_label() {
        let c = fail_code("무언가 새로운 오류");
        assert!(!c.short.is_empty() && !c.long.is_empty());
    }
}

#[cfg(test)]
mod live_tests {
    use super::YtDlp;
    use crate::models::ProviderKind;

    fn parse(entry: serde_json::Value) -> Option<crate::models::TrackRef> {
        YtDlp::entry_to_track(&entry, ProviderKind::YouTube)
    }

    /// **검색과 단일 조회가 라이브를 다른 이름으로 알려 준다.**
    ///
    /// `--flat-playlist`(검색)는 `live_status` 만 주고 `is_live` 는 비어 있다. 단일 조회는
    /// `is_live` 를 준다. 한쪽만 보면 **담는 경로에 따라 라이브인지 아닌지가 달라진다** —
    /// 검색으로 담으면 라이브로 안 잡혀서 "길이를 모른다" 는 이유로 조용히 넘어간다.
    #[test]
    fn live_is_detected_from_either_field() {
        let from_search = parse(serde_json::json!({
            "id": "abc", "live_status": "is_live",
        }));
        assert!(from_search.expect("트랙").is_live, "검색 결과의 라이브를 놓쳤어요");

        let from_lookup = parse(serde_json::json!({
            "id": "abc", "is_live": true,
        }));
        assert!(from_lookup.expect("트랙").is_live, "단일 조회의 라이브를 놓쳤어요");
    }

    /// 평범한 곡은 라이브가 아니다. 여기가 뒤집히면 모든 곡이 끝나지 않는 것으로 취급된다.
    #[test]
    fn a_normal_track_is_not_live() {
        let track = parse(serde_json::json!({
            "id": "abc", "duration": 214.0, "live_status": "not_live",
        }))
        .expect("트랙");
        assert!(!track.is_live);
        assert!(track.duration.is_some());
    }
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

/* 라디오에서 가져올 후보 상한 (`expand_collection_capped` 참고).
 *
 * 정책이 고르는 것은 열 곡 남짓이고 앞쪽이 유사도 순위다. 다만 최근 재생·차단·
 * 아티스트 쿨다운으로 걸러내는 양이 많은 서버도 있어서, 열 곡을 뽑을 여유는
 * 넉넉히 둔다. 모자라면 2순위 검색과 시드 8회 재시도가 받아 준다. */
const RADIO_CANDIDATE_CAP: usize = 150;

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
    /* 조회 한 번. **stderr 를 버리지 않는다.**
     *
     * 예전에는 `Stdio::null()` 이라 무엇 때문에 실패했는지 알 수 없었다. 그래서
     * 이 경로는 **죽은 쿠키 창구를 영영 못 접었다** — 다운로드 쪽은 접는데 조회 쪽은
     * 못 접으니, 검색·라디오·메타조회마다 못 읽는 창구 둘을 매번 다시 두드렸다
     * (실서버 실측 1.9초 + 2.7초). 자동추천이 곡마다 라디오를 도는 것을 생각하면
     * 이게 그대로 다음 곡 지연이다.
     *
     * `source_key` 는 부르는 쪽이 준다 — 어느 창구로 시도했는지는 거기만 안다. */
    async fn run_json_once(&self, args: &[String], source_key: &str) -> Option<Value> {
        let mut full = self.base_args();
        full.extend_from_slice(args);
        let fut = Command::new(&self.exe)
            .args(&full)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(std::time::Duration::from_secs(30), fut)
            .await
            .ok()?
            .ok()?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        observe_stderr(&stderr);
        if !out.status.success() {
            // 쿠키를 아예 못 읽는 창구면 이번 실행 동안 접는다 (다운로드 쪽과 같은 규칙).
            if !source_key.is_empty() && is_cookie_source_failure(&stderr) {
                dead_sources().lock().unwrap().insert(source_key.to_string());
            }
            return None;
        }
        serde_json::from_slice(&out.stdout).ok()
    }

    /// 메타 조회 — C# 과 동일하게 검색/조회도 인증 체인을 차례로 시도한다
    /// (유튜브 봇 차단 시 로그인 쿠키로 우회, 쿠키 만료 시 공개 접근 폴백).
    async fn run_json(&self, args: &[String]) -> Option<Value> {
        for (_mode, auth_args) in self.auth_chain() {
            let source_key = auth_args.join(" ");
            let mut full: Vec<String> = auth_args;
            full.extend(args.iter().cloned());
            if let Some(v) = self.run_json_once(&full, &source_key).await {
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
        /* 라이브 판정 (§40).
         *
         * 검색(`--flat-playlist`)에서는 `is_live` 가 비어 있고 `live_status` 만 온다.
         * 단일 조회에서는 `is_live` 가 온다. 둘 다 본다 — 한쪽만 보면 담는 경로에 따라
         * 라이브인지 아닌지가 달라진다. */
        let is_live = entry.get("is_live").and_then(|v| v.as_bool()).unwrap_or(false)
            || entry.get("live_status").and_then(|v| v.as_str()) == Some("is_live");
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
            is_live,
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
        // 사람이 담는 재생목록은 **끝까지 다 가져온다.** 여기서 자르면 §48 의
        // `재생목록 링크 전체 담기` 가 조용히 일부만 담게 된다.
        self.expand_collection_capped(url, provider, None).await
    }

    /* 앞에서 `limit` 곡만 가져오는 판. **라디오에만 쓴다.**
     *
     * 라디오 믹스는 400~1000곡을 돌려주는데, 자동추천은 그중 열 곡 남짓만 쓴다.
     * 그런데 그 전부를 받아 오느라 한 번에 16~18초를 썼다(실서버 실측). 그동안
     * 다음 곡 자리가 비어 있고, 곡이 그 사이 넘어가면 받아 온 것을 통째로 버리고
     * 처음부터 다시 돈다.
     *
     * **앞쪽이 곧 유사도 순위**라(§gather_candidates 주석) 앞에서 자르는 것은
     * 무작위로 줄이는 것과 다르다 — 가장 비슷한 곡들만 남는다.
     *
     * 실측 (같은 시드, 실서버):
     *   제한 없음  15.7초 / 406곡      100곡  6.4초      50곡  4.4초
     */
    async fn expand_collection_capped(
        &self,
        url: &str,
        provider: ProviderKind,
        limit: Option<usize>,
    ) -> Vec<TrackRef> {
        let mut args: Vec<String> = vec![
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
        ];
        if let Some(n) = limit {
            args.push("--playlist-end".into());
            args.push(n.to_string());
        }
        args.push("--".into());
        args.push(url.to_string());
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
        self.expand_collection_capped(&url, seed.provider, Some(RADIO_CANDIDATE_CAP))
            .await
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
        /* 쉬는 시간을 늘려 가며 시도한다. 바로 다시 하면 같은 이유로 또 튕긴다.
         *
         * **다만 앞 곡이 이미 이 사다리를 다 돌고 실패했다면 이야기가 다르다.** 그건 사다리가
         * 지금 안 듣는다는 것을 이미 한 번 측정한 것이다. 같은 실험을 곡마다 반복하면
         * 한 곡당 30초씩 쌓이고, 그동안 그 서버는 아무 소리도 못 낸다. 게다가 우리가 띄우는
         * yt-dlp 가 배로 늘어 유튜브 쪽 속도 제한(429)을 부르는데, 그 429 는 다시 "다시 해 볼
         * 실패" 로 분류돼 사다리를 또 돌린다 — 스스로를 먹여 키우는 고리다.
         *
         * 그래서 **첫 실패는 원래대로 다 해 보고**(v4.14 가 잡은 들쭉날쭉 403 복구력),
         * 연속 실패 중이면 한 번만 해 본다. `retry_rounds` 를 부르는 쪽이 그것을 정한다.
         *
         * 인증 창구 순회(`download_once` 안쪽)는 **줄이지 않는다.** 그건 시간이 아니라
         * 다양성이라, 줄이면 쿠키가 살아 있는 창구를 못 만나 본 채로 포기하게 된다. */
        const BACKOFF_SECS: [u64; 2] = [3, 8];
        let rounds = self.retry_rounds.clamp(1, BACKOFF_SECS.len() + 1);
        let mut last = String::new();
        for (round, wait) in BACKOFF_SECS.iter().enumerate().take(rounds - 1) {
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

            // **답을 손에 쥐었다가 버리고 있었다.** 성공하면 stderr 를 읽지도 않고 버렸고,
            // 실패해도 마지막 3줄만 남겼다. 그 앞쪽에 yt-dlp 가 "나 낡았다" 고 적어 보내는
            // 줄과 어떤 JS 런타임을 쓰는지가 있다. 버리기 전에 한 번 훑는다.
            let stderr = String::from_utf8_lossy(&out.stderr);
            observe_stderr(&stderr);

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
