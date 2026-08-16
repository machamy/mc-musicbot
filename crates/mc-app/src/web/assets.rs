//! 정적 자산 서빙 — HTTP 쪽 절반.
//!
//! 바이트 자체는 여기 없다. `mc-assets` 크레이트에 있고, 얇은 실행 파일이 함수 포인터로
//! 꽂아 준다(`crate::assets_di`). **그 분리가 이 파일의 전제다** — 여기서 `mc_assets` 를
//! 직접 부르면 자산 한 글자에 크레이트 전체가 다시 컴파일된다.
//!
//! `ServeDir` 을 쓰지 않는 이유는 그대로다. 배포 단위가 exe 하나이고
//! "exe 하나 SHA 하나" 계약이 있다.
//!
//! 경로는 `/music/assets/{name}` 이고 `?v={version}` 으로 캐시를 무효화한다.
//! 서비스워커만 예외로 `/music/sw.js` 에서 서빙한다 — 스코프가 경로에서 파생되기 때문에
//! 하위 디렉터리에 두면 `/music/*` 를 제어하지 못한다.

use crate::assets_di::assets;
use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;

/// 자산 내용 해시. 페이지 셸이 `?v=` 에 쓴다.
pub fn version() -> &'static str {
    (assets().version)()
}

/// 사용자용 패치노트 원문 (§30). **원본은 `docs/CHANGELOG.md` 하나뿐이다.**
pub fn changelog() -> &'static str {
    (assets().changelog)()
}

/// API 가이드 본문 마크업. 세션 게이트 뒤 셸에서만 내보낸다.
pub fn apidoc_body() -> &'static str {
    (assets().apidoc_body)()
}

/// 자산 하나의 SHA-256 앞 16자리. 클라이언트 캐시 버스팅 확인용으로만 쓴다.
fn etag_of(body: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(body);
    format!("\"{}\"", short_hex(&digest))
}

/// SHA-256 앞 16자리.
///
/// **`mc-assets` 에 같은 함수가 하나 더 있다. 합치지 마라.**
/// 합치려면 이 크레이트가 `mc-assets` 를 의존해야 하는데, 그러면 자산 한 글자에
/// 40,000줄이 다시 컴파일된다(실측 54.4초). 6줄 중복이 그보다 훨씬 싸다.
fn short_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 응답 캐시 정책.
///
/// `immutable` 은 **URL 이 진짜로 내용 주소일 때만** 안전하다.
/// `portal.js` 가 `./core.js` 를 정적 import 하기 때문에 그 요청에는 `?v=` 가 붙지 않는다.
/// 거기에 1년 immutable 을 걸면 core.js 가 영원히 갱신되지 않는다.
/// 그래서 `?v=` 가 현재 자산 버전과 정확히 일치할 때만 immutable 을 준다.
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
    // 경로 조작 방지 — 이름은 화이트리스트 조회로만 해석한다 (`mc-assets::get`).
    match (assets().get)(&name) {
        Some((body, mime)) => respond(body.to_vec(), mime, cache, inm),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `GET /music/sw.js` — 스코프 때문에 반드시 `/music` 바로 아래에서 서빙한다.
pub async fn serve_service_worker(headers: HeaderMap) -> Response {
    let (body, mime) = (assets().service_worker)();
    respond(
        body.to_vec(),
        mime,
        "no-cache",
        header_str(&headers, header::IF_NONE_MATCH),
    )
}

/// `GET /music/manifest.webmanifest`
pub async fn serve_manifest(headers: HeaderMap) -> Response {
    let (body, mime) = (assets().manifest)();
    respond(
        body.to_vec(),
        mime,
        "no-cache",
        header_str(&headers, header::IF_NONE_MATCH),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets_di::install_test_assets;

    #[test]
    fn every_known_asset_resolves() {
        install_test_assets();
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
            assert!((assets().get)(name).is_some(), "{name} 자산이 등록되지 않았다");
        }
    }

    #[test]
    fn unknown_asset_is_rejected() {
        install_test_assets();
        assert!((assets().get)("../../secret.txt").is_none());
        assert!((assets().get)("portal.js.map").is_none());
        assert!((assets().get)("").is_none());
    }

    #[test]
    fn assets_are_not_empty() {
        install_test_assets();
        for name in ["core.js", "portal.js", "portal.css", "console.js", "apidoc.css"] {
            let (body, _) = (assets().get)(name).expect("등록된 자산");
            // `is_empty` 가 아니라 `trim`. 공백만 든 자산도 잡아야 한다 (이사 전 단언).
            let text = std::str::from_utf8(body).expect("텍스트 자산");
            assert!(!text.trim().is_empty(), "{name} 이 비었다");
        }
        assert!((assets().get)("icon-192.png").unwrap().0.starts_with(b"\x89PNG"));
        assert!((assets().get)("icon-512.png").unwrap().0.starts_with(b"\x89PNG"));
    }

    /// `?v=` 가 현재 버전과 정확히 같을 때만 영구 캐시를 준다.
    /// 이게 틀어지면 `core.js` 가 브라우저에 영원히 박힌다.
    #[test]
    fn only_an_exact_version_query_earns_immutable() {
        install_test_assets();
        let current = version();
        assert_eq!(
            cache_policy(Some(current)),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_policy(None), "no-cache");
        assert_eq!(cache_policy(Some("deadbeef")), "no-cache");
    }
}
