//! 프런트엔드 자산의 바이트와 그 버전 해시.
//!
//! 배포 단위가 `mc-musicbot.exe` 하나이고, 포터블 매니페스트(1241개 파일)와
//! "exe 하나 SHA 하나" 계약이 있으므로 `ServeDir` 로 느슨한 파일을 깔지 않는다.
//! 모든 프런트엔드 파일은 `include_str!`/`include_bytes!` 로 컴파일 시점에 박아 넣는다.
//!
//! **이 크레이트는 HTTP 를 모른다.** 응답을 만드는 일(ETag·캐시 정책·304)은
//! `mc-app` 쪽 `web::assets` 가 하고, 여기서는 바이트와 MIME 만 내준다.
//! 그렇게 갈라 둔 덕분에 `mc-app` 이 이 크레이트를 의존하지 않을 수 있고,
//! JS 한 글자를 고쳐도 40,000줄이 다시 컴파일되지 않는다.
//!
//! 이름 경계가 곧 보안 경계다:
//! - [`get`] 은 **화이트리스트 12개**만 해석한다. 경로 조작이 통하지 않는다.
//! - `sw.js`·`apidoc.html`·CHANGELOG 는 **화이트리스트 밖**이고 전용 접근자로만 나간다.
//!   화이트리스트에 넣으면 지금 없는 `/music/assets/sw.js` 나
//!   `/music/assets/apidoc.html` 이 새로 열려 버린다 — 후자는 세션 게이트 뒤에 있어야 한다.

use std::sync::OnceLock;

// ── CSS ──
const TOKENS_CSS: &str = include_str!("assets/tokens.css");
const PORTAL_CSS: &str = include_str!("assets/portal.css");
const CONSOLE_CSS: &str = include_str!("assets/console.css");
/// API 가이드 문서(`/music/apidoc`) 전용. 문서 화면은 토큰 다음에 이 파일 하나만 링크한다.
const APIDOC_CSS: &str = include_str!("assets/apidoc.css");

// ── JS ──
const CORE_JS: &str = include_str!("assets/core.js");
const PORTAL_JS: &str = include_str!("assets/portal.js");
const CONSOLE_JS: &str = include_str!("assets/console.js");
const SW_JS: &str = include_str!("assets/sw.js");

// ── 기타 ──
/// 사용자용 패치노트 (§30). **원본은 `docs/CHANGELOG.md` 하나뿐이다.**
/// 화면용으로 따로 옮겨 적으면 둘이 갈라져서 결국 화면 쪽이 낡는다.
const CHANGELOG_MD: &str = include_str!("../../../docs/CHANGELOG.md");
const MANIFEST: &str = include_str!("assets/manifest.webmanifest");
const FAVICON_SVG: &str = include_str!("assets/favicon.svg");
const ICON_192: &[u8] = include_bytes!("assets/icon-192.png");
const ICON_512: &[u8] = include_bytes!("assets/icon-512.png");
const ICON_180: &[u8] = include_bytes!("assets/icon-180.png");

/// API 가이드 본문 마크업.
///
/// **화이트리스트에 넣으면 안 된다.** `/music/assets/apidoc.html` 이 인증 없이 열리고,
/// 문서 페이지에 걸어 둔 세션 검사가 우회된다. 셸 안에서만 내보낸다.
const APIDOC_HTML: &str = include_str!("assets/apidoc.html");

const MIME_CSS: &str = "text/css; charset=utf-8";
const MIME_JS: &str = "text/javascript; charset=utf-8";
const MIME_MANIFEST: &str = "application/manifest+json; charset=utf-8";
const MIME_SVG: &str = "image/svg+xml; charset=utf-8";
const MIME_PNG: &str = "image/png";

/// 자산 전체 내용에서 뽑은 짧은 버전 문자열. 페이지 셸이 `?v=` 에 쓴다.
///
/// `BUILD_ID.txt` 는 포터블 배포본에만 있고 개발 중에는 비어 있다. 빈 `?v=` 와
/// `Cache-Control: immutable` 이 겹치면 브라우저가 옛 자산을 영원히 붙들어
/// "배포했는데 화면이 그대로" 가 된다. 내용에서 버전을 뽑으면 파일이 실제로
/// 바뀔 때만 URL 이 바뀌고, 안 바뀌면 캐시가 그대로 살아 있다.
///
/// **CHANGELOG 와 `apidoc.html` 은 여기에 참여하지 않는다.** 원래부터 그랬다 —
/// 이 목록을 건드리면 배포된 브라우저의 캐시 키가 통째로 바뀐다.
pub fn version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        for text in [
            TOKENS_CSS,
            PORTAL_CSS,
            CONSOLE_CSS,
            APIDOC_CSS,
            CORE_JS,
            PORTAL_JS,
            CONSOLE_JS,
            SW_JS,
            MANIFEST,
            FAVICON_SVG,
        ] {
            hasher.update(text.as_bytes());
        }
        for bytes in [ICON_192, ICON_512, ICON_180] {
            hasher.update(bytes);
        }
        short_hex(&hasher.finalize())
    })
}

/// SHA-256 앞 16자리.
///
/// **`mc-app` 쪽 `etag_of` 에 같은 함수가 하나 더 있다. 합치지 마라.**
/// 합치려면 `mc-app` 이 이 크레이트를 의존해야 하는데, 그러면 자산 한 글자에
/// 본체 40,000줄이 다시 컴파일된다(실측 54.4초). 6줄 중복이 그보다 싸다.
fn short_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 공개 자산 하나 — `(바이트, MIME)`.
///
/// **화이트리스트 조회로만 해석한다.** 경로 조작이 통하지 않는 이유가 이것이다.
pub fn get(name: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match name {
        "tokens.css" => (TOKENS_CSS.as_bytes(), MIME_CSS),
        "portal.css" => (PORTAL_CSS.as_bytes(), MIME_CSS),
        "console.css" => (CONSOLE_CSS.as_bytes(), MIME_CSS),
        "apidoc.css" => (APIDOC_CSS.as_bytes(), MIME_CSS),

        "core.js" => (CORE_JS.as_bytes(), MIME_JS),
        "portal.js" => (PORTAL_JS.as_bytes(), MIME_JS),
        "console.js" => (CONSOLE_JS.as_bytes(), MIME_JS),

        "manifest.webmanifest" => (MANIFEST.as_bytes(), MIME_MANIFEST),
        "favicon.svg" => (FAVICON_SVG.as_bytes(), MIME_SVG),

        "icon-192.png" => (ICON_192, MIME_PNG),
        "icon-512.png" => (ICON_512, MIME_PNG),
        "icon-180.png" => (ICON_180, MIME_PNG),

        _ => return None,
    })
}

/// 서비스워커. **`/music/sw.js` 에서만 나간다** — 스코프가 경로에서 파생되기 때문에
/// 하위 디렉터리에 두면 `/music/*` 를 제어하지 못한다. 그래서 [`get`] 밖에 있다.
pub fn service_worker() -> (&'static [u8], &'static str) {
    (SW_JS.as_bytes(), MIME_JS)
}

/// 웹 앱 매니페스트. `/music/manifest.webmanifest` 전용 경로가 따로 있다.
/// ([`get`] 으로도 나가는데, 그건 도입 전부터 그랬다 — 바꾸면 캐시 키가 흔들린다.)
pub fn manifest() -> (&'static [u8], &'static str) {
    (MANIFEST.as_bytes(), MIME_MANIFEST)
}

/// API 가이드 본문. 세션 게이트 뒤 셸에서만 쓴다.
pub fn apidoc_body() -> &'static str {
    APIDOC_HTML
}

/// 사용자용 패치노트 원문.
pub fn changelog() -> &'static str {
    CHANGELOG_MD
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 공개되는 이름 전부. 여기 없는 이름은 밖에서 못 가져간다.
    const PUBLIC: [&str; 12] = [
        "tokens.css",
        "portal.css",
        "console.css",
        "apidoc.css",
        "core.js",
        "portal.js",
        "console.js",
        "manifest.webmanifest",
        "favicon.svg",
        "icon-192.png",
        "icon-512.png",
        "icon-180.png",
    ];

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn every_known_asset_resolves() {
        for name in PUBLIC {
            assert!(get(name).is_some(), "{name} 자산이 등록되지 않았다");
        }
    }

    #[test]
    fn unknown_asset_is_rejected() {
        assert!(get("../../secret.txt").is_none());
        assert!(get("portal.js.map").is_none());
        assert!(get("").is_none());
    }

    /// 이 셋은 **일부러** 화이트리스트 밖이다.
    ///
    /// `sw.js` 를 넣으면 `/music/assets/sw.js` 라는 없던 경로가 열리고,
    /// `apidoc.html` 을 넣으면 문서 페이지의 세션 검사가 통째로 우회된다.
    #[test]
    fn gated_assets_are_not_in_the_public_whitelist() {
        assert!(get("sw.js").is_none(), "sw.js 는 전용 경로로만 나가야 한다");
        assert!(
            get("apidoc.html").is_none(),
            "apidoc.html 이 인증 없이 열리면 문서 페이지의 세션 검사가 무의미해진다"
        );
        assert!(get("CHANGELOG.md").is_none());
    }

    #[test]
    fn assets_are_not_empty() {
        assert!(!CORE_JS.trim().is_empty());
        assert!(!PORTAL_JS.trim().is_empty());
        assert!(!PORTAL_CSS.trim().is_empty());
        assert!(!CONSOLE_JS.trim().is_empty());
        assert!(!APIDOC_CSS.trim().is_empty());
        assert!(!APIDOC_HTML.trim().is_empty());
        assert!(!CHANGELOG_MD.trim().is_empty());
        assert!(ICON_192.starts_with(b"\x89PNG"));
        assert!(ICON_512.starts_with(b"\x89PNG"));
        assert!(ICON_180.starts_with(b"\x89PNG"));
    }

    /// 배포된 브라우저의 캐시 키다. **바뀌면 전원이 자산을 다시 받는다.**
    ///
    /// 자산을 일부러 고쳤으면 이 값을 같이 갱신한다 — 그 갱신이 곧
    /// "자산이 바뀌었다" 는 명시적 선언이다. 의도 없이 바뀌면 여기서 먼저 걸린다.
    /// (워크스페이스 이사 때는 `d44ff894b8ef9398` 이 그대로 유지되는 것으로 무변경을 증명했다.)

    /// **화면이 없는 권한 키를 쓰고 있지 않은지.**
    ///
    /// `portal.js` 의 `can(key)` 는 모르는 키를 조용히 `false` 로 돌려준다. 그래서 오타나
    /// 존재하지 않는 키를 쓰면 **그 기능이 아무에게도 안 보이는데 화면에도 콘솔에도
    /// 아무 흔적이 안 남는다.**
    ///
    /// 실제로 그렇게 당했다 — 다음 곡 후보 줄을 `can('queue')` 로 잠갔는데 `queue` 라는
    /// 권한 키는 없어서(진짜는 `queueEdit`) 후보가 통째로 안 보였다. 서버 로그에는 후보가
    /// 셋 잘 뽑혔다고 찍혀 있어서 원인을 찾기도 어려웠다.
    ///
    /// 실행으로는 못 잡는다(조용히 거짓이라 예외도 안 난다). 소스를 직접 읽는 이유다.
    ///
    /// **주석 안의 예시도 똑같이 걸린다.** 문자열만 훑는 단순한 검사라 그렇다 —
    /// 그게 싫으면 주석에 `can(` 을 그대로 쓰지 말고 키 이름만 적으면 된다.
    /// 파서를 정교하게 만드는 것보다 이 규칙이 싸다.
    #[test]
    fn the_portal_never_asks_for_a_permission_key_that_does_not_exist() {
        // `PERM_LABELS = { ... }` 블록에서 키를 긁는다.
        let start = PORTAL_JS
            .find("const PERM_LABELS = {")
            .expect("PERM_LABELS 를 못 찾았다 — 이름이 바뀌었으면 이 테스트도 같이 고친다");
        let end = PORTAL_JS[start..]
            .find("
};")
            .expect("PERM_LABELS 블록이 안 닫혔다")
            + start;
        let known: Vec<&str> = PORTAL_JS[start..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let name = line.split(':').next()?.trim();
                (!name.is_empty()
                    && !name.starts_with('/')
                    && !name.starts_with('*')
                    && name.chars().all(|c| c.is_ascii_alphanumeric()))
                .then_some(name)
            })
            .collect();
        assert!(known.len() > 8, "권한 키를 제대로 못 긁었다: {known:?}");

        // `can('...')` 로 쓰이는 키를 전부 모은다.
        let mut unknown = Vec::new();
        let mut rest = PORTAL_JS;
        while let Some(at) = rest.find("can('") {
            rest = &rest[at + 5..];
            if let Some(close) = rest.find("'") {
                let key = &rest[..close];
                if !key.is_empty() && !known.contains(&key) {
                    unknown.push(key.to_string());
                }
            }
        }
        unknown.sort();
        unknown.dedup();
        assert!(
            unknown.is_empty(),
            "화면이 없는 권한 키를 쓰고 있다 — `can()` 이 조용히 false 를 돌려줘서 그 기능이              아무에게도 안 보인다. PERM_LABELS 에 있는 이름만 써라.
  {}",
            unknown.join(", ")
        );
    }

    /// **검색한 곳은 재생목록도 물어봐야 한다.**
    ///
    /// `searchTracks` 는 검색 패널과 대기열 안 검색이 같이 쓰는데, 재생목록이 딸려 왔는지
    /// 물어보는 `offerPlaylist` 는 검색 패널에만 붙어 있었다. 그래서 같은 링크를 붙여넣어도
    /// **어느 칸에 넣었느냐에 따라** 재생목록이 통째로 담기기도 하고 조용히 버려지기도 했다.
    ///
    /// 이 결함은 파일 하나만 봐서는 안 보인다. 두 호출부가 서로 멀리 떨어져 있고 각각은
    /// 그 자체로 멀쩡하기 때문이다. 그래서 "쌍으로 다녀야 하는 것"을 여기서 못 박는다.
    #[test]
    fn every_search_call_site_also_offers_the_playlist() {
        let mut missing = Vec::new();
        // 함수 단위로 자른다 — 호출부가 어느 함수 안에 있는지가 판정 기준이다.
        for chunk in PORTAL_JS.split("
async function ") {
            let name = chunk.split('(').next().unwrap_or("").trim();
            // 정의 자체(`async function searchTracks(`)는 건너뛴다.
            if name == "searchTracks" {
                continue;
            }
            let body = chunk.split("
async function ").next().unwrap_or(chunk);
            /* **재생목록에 담는 검색은 규칙이 다르다.**
             *
             * 이 불변식은 "링크에 재생목록이 딸려 있으면 **대기열에** 통째로 담을지
             * 물어본다" 는 것이다. 그런데 `openPlaylistAdder` 안의 검색은 대기열이
             * 아니라 **재생목록**을 채운다 — 거기서 `offerPlaylist` 를 부르면 사람이
             * 재생목록을 채우려는 중에 곡이 대기열로 쏟아진다.
             *
             * **이름이 아니라 하는 일로 가른다.** 함수 이름으로 거르면 앞으로 생기는
             * 같은 이름의 함수가 이 예외를 조용히 물려받는다.
             *
             * 남은 구멍은 알고 둔다: 재생목록에 담기 창에 재생목록 링크를 넣으면 앞
             * 50곡이 나열될 뿐 `전부 이 목록에 담기` 가 없다. 서버에 목록 대 목록으로
             * 담는 길이 아직 없어서다(`addTrack` 은 한 곡씩이다). 그 길이 생기면 이
             * 예외를 지우고 그쪽을 부르게 하면 된다. */
            if body.contains("'addTrack'") {
                continue;
            }
            if body.contains("searchTracks(") && !body.contains("offerPlaylist(") {
                missing.push(name.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "검색은 하는데 재생목록은 안 물어보는 곳이 있다 — 같은 링크가 어느 칸에                         들어가느냐에 따라 다르게 동작한다:
  {}",
            missing.join(", ")
        );
    }

    /// **개인 설정은 서버가 받는 형식으로 보내야 한다.**
    ///
    /// 로그 필터가 `JSON.stringify(배열)` 로 나가는데 서버는 콤마로 이은 값만 받아서,
    /// 칩을 누를 때마다 저장이 400 으로 튕겼다. 운영 DB 에 이 키의 행이 0개였다 —
    /// **한 번도 저장된 적이 없다.** 게다가 이 API 는 키 하나가 틀리면 묶음 전체를
    /// 거절하는데 화면은 여러 설정을 모아 한 번에 보내므로, 같이 실린 다른 설정까지
    /// 함께 날아갔다.
    ///
    /// 이 검사는 그 재발을 막는다. `prefSet('auditFilter', …)` 에 `JSON.stringify` 를
    /// 넘기면 여기서 걸린다.
    #[test]
    fn the_log_filter_pref_is_not_sent_as_json() {
        let mut offenders = Vec::new();
        let mut rest = PORTAL_JS;
        while let Some(at) = rest.find("prefSet('auditFilter'") {
            rest = &rest[at..];
            let line_end = rest.find('\n').unwrap_or(rest.len());
            let line = &rest[..line_end];
            if line.contains("JSON.stringify") {
                offenders.push(line.trim().to_string());
            }
            rest = &rest[line_end..];
        }
        assert!(
            offenders.is_empty(),
            "로그 필터를 JSON 으로 보내고 있다 — 서버는 콤마로 이은 값만 받아서 저장이                         통째로 튕긴다(같은 묶음의 다른 설정까지):
  {}",
            offenders.join("
  ")
        );
    }

    #[test]
    fn version_hash_is_pinned() {
        assert_eq!(version(), "635e37fa70588451");
    }

    /// 이름과 바이트가 서로 **뒤바뀌어도** 버전 해시는 그대로다(같은 것을 다 더하므로).
    /// 그래서 이름별로 직접 못 박는다.
    #[test]
    fn each_name_returns_its_own_file() {
        let expected: [(&str, &str); 13] = [
            (
                "tokens.css",
                "7883eae6b3bd112a19864801180421a9f3fc41bb66acdd8a0d485025c611f761",
            ),
            (
                "portal.css",
                "5fc1fba111c1ff4e4c7371f5b71336c7f3c4b90d100cb546971a632663a454d6",
            ),
            (
                "console.css",
                "4be121e6b9d412c74f451fc61b2f2bcd453d81c5cd7bb8e3a1b1f6084a7c4354",
            ),
            (
                "apidoc.css",
                "abd2ac070236056d4600447bb8dfe8be63b77b5117234790aff32cf76a584ec3",
            ),
            (
                "core.js",
                "28be9c9d3ef97b4edf309837d0923c7830d17154088ff63458afd0f929c336fe",
            ),
            (
                "portal.js",
                "d2267b089eb284480b95cd6993715ea102b61c000bf3475c9851f7da314dd0cc",
            ),
            (
                "console.js",
                "e31802f8bc21cf51ebe45528fd21f4227ea640e8916d94ec4d520949b0124d16",
            ),
            (
                "manifest.webmanifest",
                "5915414f652f30793140b61b3ebaef02d194a535912e9aa9312348a148ab29c8",
            ),
            (
                "favicon.svg",
                "9a8e1a34a49b547644f4b63515510db1b3a801cf3146893a88b6dcec4456ba0b",
            ),
            (
                "icon-192.png",
                "e99649f8c034801965353eb21623c2d80d277faa355d818a71ad1c01144a9f97",
            ),
            (
                "icon-512.png",
                "dc9dd896a38f0c8da1dbbd323dc3433abbf21ddc5dfef2b5bedf4022659e4f4a",
            ),
            (
                "icon-180.png",
                "431ad912ce3645ecb16234a20fe9d0d8cf45187f498ec29b2def129daf0f1759",
            ),
            // 화이트리스트 밖이지만 바이트는 똑같이 못 박는다.
            (
                "sw.js",
                "90f8f8dd3a1859d953ad0d803e2ad13d4126686425669504910a1f7fb3859686",
            ),
        ];
        for (name, want) in expected {
            let bytes = match name {
                "sw.js" => service_worker().0,
                _ => get(name).unwrap_or_else(|| panic!("{name} 이 없다")).0,
            };
            assert_eq!(sha256_hex(bytes), want, "{name} 의 바이트가 다르다");
        }
        assert_eq!(
            sha256_hex(apidoc_body().as_bytes()),
            "94b5c25d7519cb2cebdfd3ece99ae59aac1001fa762ab18abbd7765664c0763a"
        );
    }

    /// **회귀 가드: 화면이 개인 설정에 빈 문자열을 보내면 배치가 통째로 거절된다.**
    ///
    /// 서버(`api_prefs_put`)는 키 하나만 값이 이상해도 **400 으로 배치 전체를 버린다.**
    /// 화면은 300ms 동안 여러 설정을 모아 한 번에 보내므로, 개발자 콘솔 자리를
    /// 초기화하는 순간 같은 배치에 실린 볼륨·싱크 보정 저장까지 같이 날아갔다 —
    /// 로컬 인스턴스로 실측해 확인했다(`devPos: ""` → 400, `webVolume` 미저장).
    ///
    /// 같은 사고가 `nowVoters`(v4.20) · `devPos`(v4.24) 에 이어 세 번째였다.
    /// 그래서 `prefSet` 이 빈 문자열을 `null`("기본으로 되돌리기")로 접는지,
    /// 그리고 그 `null` 을 **서버로 실제로 보내는지**를 여기서 본다.
    #[test]
    fn the_screen_never_sends_an_empty_pref_value() {
        let js = PORTAL_JS;
        let start = js
            .find("function prefSet(")
            .expect("prefSet 이 있어야 한다");
        // **문자 경계로 자른다.** `start + 700` 을 그대로 쓰면 한글 주석 한가운데를
        // 잘라 슬라이스가 패닉한다 (실제로 그랬다).
        let body: String = js[start..].chars().take(400).collect();
        let body = body.as_str();

        assert!(
            body.contains("value === ''"),
            "prefSet 이 빈 문자열을 되돌리기로 접지 않는다 — 그대로 보내면 배치가 400 으로 죽는다"
        );
        /* 되돌리기가 **서버까지** 가야 한다. 예전에는 `if (next !== null)` 로 감싸져
         * 있어서 지우기가 로컬에만 남았고, 다른 기기에서 열면 지운 값이 되살아났다. */
        assert!(
            !body.contains("if (next !== null) prefsPending.set"),
            "되돌리기가 서버로 안 나간다 — 로컬에서만 지워진다"
        );
        assert!(body.contains("prefsPending.set(key, next)"));
    }

    /// 전용 접근자의 MIME 도 못 박는다 — 여기가 틀리면 서비스워커 등록이 조용히 실패한다.
    #[test]
    fn gated_accessors_keep_their_mime() {
        assert_eq!(service_worker().1, MIME_JS);
        assert_eq!(manifest().1, MIME_MANIFEST);
    }
}
