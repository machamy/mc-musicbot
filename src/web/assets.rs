//! 정적 에셋 임베드와 서빙.
//!
//! 배포 단위가 `mc-musicbot.exe` 하나이고, 포터블 매니페스트(1241개 파일)와
//! "exe 하나 SHA 하나" 계약이 있으므로 `ServeDir`로 느슨한 파일을 깔지 않는다.
//! 모든 프런트엔드 파일은 `include_str!`/`include_bytes!`로 컴파일 시점에 박아 넣는다.
//!
//! 경로는 `/music/assets/{name}`이고 `?v={build_id}`로 캐시를 무효화한다.
//! 서비스워커만 예외로 `/music/sw.js`에서 서빙한다 — 스코프가 경로에서 파생되기 때문에
//! 하위 디렉터리에 두면 `/music/*`를 제어하지 못한다.

use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::OnceLock;

// ── CSS ──
const TOKENS_CSS: &str = include_str!("assets/tokens.css");
const PORTAL_CSS: &str = include_str!("assets/portal.css");
const CONSOLE_CSS: &str = include_str!("assets/console.css");
/// API 가이드 문서(`/music/apidoc`) 전용. 문서 화면은 토큰 다음에 이 파일 하나만 링크한다.
///
/// 본문 마크업(`assets/apidoc.html`)은 **여기 없다.** `remote_page.rs` 가 직접 읽어 셸에 넣는다.
/// 에셋으로 등록하면 `/music/assets/apidoc.html` 이 로그인 게이트 밖에서 열려서,
/// 문서 페이지에 걸어 둔 세션 검사가 무의미해진다.
const APIDOC_CSS: &str = include_str!("assets/apidoc.css");

// ── JS ──
const CORE_JS: &str = include_str!("assets/core.js");
const PORTAL_JS: &str = include_str!("assets/portal.js");
const CONSOLE_JS: &str = include_str!("assets/console.js");
const SW_JS: &str = include_str!("assets/sw.js");

// ── 기타 ──
/// 사용자용 패치노트 (§30). **원본은 `docs/CHANGELOG.md` 하나뿐이다.**
/// 화면용으로 따로 옮겨 적으면 둘이 갈라져서 결국 화면 쪽이 낡는다.
pub const CHANGELOG_MD: &str = include_str!("../../docs/CHANGELOG.md");
const MANIFEST: &str = include_str!("assets/manifest.webmanifest");
const FAVICON_SVG: &str = include_str!("assets/favicon.svg");
const ICON_192: &[u8] = include_bytes!("assets/icon-192.png");
const ICON_512: &[u8] = include_bytes!("assets/icon-512.png");
const ICON_180: &[u8] = include_bytes!("assets/icon-180.png");

/// 에셋 전체 내용에서 뽑은 짧은 버전 문자열. 페이지 셸이 `?v=`에 쓴다.
///
/// `BUILD_ID.txt`는 포터블 배포본에만 있고 개발 중에는 비어 있다. 빈 `?v=`와
/// `Cache-Control: immutable`이 겹치면 브라우저가 옛 에셋을 영원히 붙들어
/// "배포했는데 화면이 그대로"가 된다. 내용에서 버전을 뽑으면 파일이 실제로
/// 바뀔 때만 URL이 바뀌고, 안 바뀌면 캐시가 그대로 살아 있다.
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
        hex_16(&hasher.finalize())
    })
}

/// 에셋 본문과 MIME.
enum Asset {
    Text(&'static str, &'static str),
    Bytes(&'static [u8], &'static str),
}

fn lookup(name: &str) -> Option<Asset> {
    Some(match name {
        "tokens.css" => Asset::Text(TOKENS_CSS, "text/css; charset=utf-8"),
        "portal.css" => Asset::Text(PORTAL_CSS, "text/css; charset=utf-8"),
        "console.css" => Asset::Text(CONSOLE_CSS, "text/css; charset=utf-8"),
        "apidoc.css" => Asset::Text(APIDOC_CSS, "text/css; charset=utf-8"),

        "core.js" => Asset::Text(CORE_JS, "text/javascript; charset=utf-8"),
        "portal.js" => Asset::Text(PORTAL_JS, "text/javascript; charset=utf-8"),
        "console.js" => Asset::Text(CONSOLE_JS, "text/javascript; charset=utf-8"),

        "manifest.webmanifest" => Asset::Text(MANIFEST, "application/manifest+json; charset=utf-8"),
        "favicon.svg" => Asset::Text(FAVICON_SVG, "image/svg+xml; charset=utf-8"),

        "icon-192.png" => Asset::Bytes(ICON_192, "image/png"),
        "icon-512.png" => Asset::Bytes(ICON_512, "image/png"),
        "icon-180.png" => Asset::Bytes(ICON_180, "image/png"),

        _ => return None,
    })
}

/// 에셋 하나의 SHA-256 앞 16자리. 클라이언트 캐시 버스팅 확인용으로만 쓴다.
fn etag_of(body: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(body);
    format!("\"{}\"", hex_16(&digest))
}

fn hex_16(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 응답 캐시 정책.
///
/// `immutable`은 **URL이 진짜로 내용 주소일 때만** 안전하다.
/// `portal.js`가 `./core.js`를 정적 import 하기 때문에 그 요청에는 `?v=`가 붙지 않는다.
/// 거기에 1년 immutable을 걸면 core.js가 영원히 갱신되지 않는다.
/// 그래서 `?v=`가 현재 에셋 버전과 정확히 일치할 때만 immutable을 준다.
fn cache_policy(query_version: Option<&str>) -> &'static str {
    match query_version {
        Some(value) if value == version() => "public, max-age=31536000, immutable",
        // 그 외에는 매번 재검증. 본문 없는 304라 비용이 거의 없다.
        _ => "no-cache",
    }
}

fn respond(
    body: Vec<u8>,
    mime: &'static str,
    cache: &'static str,
    if_none_match: Option<&str>,
) -> Response {
    let etag = etag_of(&body);

    // 재검증 요청이면 본문을 보내지 않는다.
    if if_none_match.is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag)) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        if let Ok(value) = HeaderValue::from_str(&etag) {
            headers.insert(header::ETAG, value);
        }
        return response;
    }

    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn header_str<'a>(headers: &'a HeaderMap, name: header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// `GET /music/assets/{name}`
pub async fn serve_asset(
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let cache = cache_policy(query.get("v").map(String::as_str));
    let inm = header_str(&headers, header::IF_NONE_MATCH);
    // 경로 조작 방지 — 이름은 화이트리스트 조회로만 해석한다.
    match lookup(&name) {
        Some(Asset::Text(body, mime)) => respond(body.as_bytes().to_vec(), mime, cache, inm),
        Some(Asset::Bytes(body, mime)) => respond(body.to_vec(), mime, cache, inm),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `GET /music/sw.js` — 스코프 때문에 반드시 `/music` 바로 아래에서 서빙한다.
pub async fn serve_service_worker(headers: HeaderMap) -> Response {
    respond(
        SW_JS.as_bytes().to_vec(),
        "text/javascript; charset=utf-8",
        "no-cache",
        header_str(&headers, header::IF_NONE_MATCH),
    )
}

/// `GET /music/manifest.webmanifest`
pub async fn serve_manifest(headers: HeaderMap) -> Response {
    respond(
        MANIFEST.as_bytes().to_vec(),
        "application/manifest+json; charset=utf-8",
        "no-cache",
        header_str(&headers, header::IF_NONE_MATCH),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_asset_resolves() {
        for name in [
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
        ] {
            assert!(lookup(name).is_some(), "{name} 에셋이 등록되지 않았다");
        }
    }

    #[test]
    fn unknown_asset_is_rejected() {
        assert!(lookup("../../secret.txt").is_none());
        assert!(lookup("portal.js.map").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn assets_are_not_empty() {
        assert!(!CORE_JS.trim().is_empty());
        assert!(!PORTAL_JS.trim().is_empty());
        assert!(!PORTAL_CSS.trim().is_empty());
        assert!(!CONSOLE_JS.trim().is_empty());
        assert!(!APIDOC_CSS.trim().is_empty());
        assert!(ICON_192.starts_with(b"\x89PNG"));
        assert!(ICON_512.starts_with(b"\x89PNG"));
    }
}
