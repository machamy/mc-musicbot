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
use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

// ── CSS ──
const TOKENS_CSS: &str = include_str!("assets/tokens.css");
const PORTAL_CSS: &str = include_str!("assets/portal.css");
const CONSOLE_CSS: &str = include_str!("assets/console.css");

// ── JS ──
const CORE_JS: &str = include_str!("assets/core.js");
const PORTAL_JS: &str = include_str!("assets/portal.js");
const CONSOLE_JS: &str = include_str!("assets/console.js");
const SW_JS: &str = include_str!("assets/sw.js");

// ── 기타 ──
const MANIFEST: &str = include_str!("assets/manifest.webmanifest");
const FAVICON_SVG: &str = include_str!("assets/favicon.svg");
const ICON_192: &[u8] = include_bytes!("assets/icon-192.png");
const ICON_512: &[u8] = include_bytes!("assets/icon-512.png");
const ICON_180: &[u8] = include_bytes!("assets/icon-180.png");

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

fn respond(body: Vec<u8>, mime: &'static str, immutable: bool) -> Response {
    let etag = etag_of(&body);
    let cache = if immutable {
        // build_id가 쿼리에 붙으므로 내용이 바뀌면 URL도 바뀐다.
        "public, max-age=31536000, immutable"
    } else {
        // 서비스워커와 매니페스트는 갱신이 보여야 한다.
        "no-cache"
    };
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

/// `GET /music/assets/{name}`
pub async fn serve_asset(Path(name): Path<String>) -> Response {
    // 경로 조작 방지 — 이름은 화이트리스트 조회로만 해석한다.
    match lookup(&name) {
        Some(Asset::Text(body, mime)) => respond(body.as_bytes().to_vec(), mime, true),
        Some(Asset::Bytes(body, mime)) => respond(body.to_vec(), mime, true),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `GET /music/sw.js` — 스코프 때문에 반드시 `/music` 바로 아래에서 서빙한다.
pub async fn serve_service_worker() -> Response {
    respond(
        SW_JS.as_bytes().to_vec(),
        "text/javascript; charset=utf-8",
        false,
    )
}

/// `GET /music/manifest.webmanifest`
pub async fn serve_manifest() -> Response {
    respond(
        MANIFEST.as_bytes().to_vec(),
        "application/manifest+json; charset=utf-8",
        false,
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
        assert!(ICON_192.starts_with(b"\x89PNG"));
        assert!(ICON_512.starts_with(b"\x89PNG"));
    }
}
