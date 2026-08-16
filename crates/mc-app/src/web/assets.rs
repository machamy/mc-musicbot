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
    //! ## 왜 전부 핸들러를 통과시키나
    //!
    //! 예전 테스트 셋은 `(assets().get)(name)` 만 불렀다. 그건 이 파일의 코드가 아니라
    //! `mc-assets::get` 이고, 그쪽 크레이트에 **같은 이름의 같은 테스트가 이미 있다.**
    //! 그래서 `serve_asset`·`serve_service_worker`·`serve_manifest`·`respond`·`etag_of`·
    //! `short_hex` 를 통째로 지워도 이 파일의 테스트는 전부 초록이었다 — 304 재검증도,
    //! 404 갈래도, `nosniff` 도 아무도 안 보고 있었다.
    //!
    //! 그래서 여기서는 **핸들러를 실제로 부른다.** 핸들러가 `async fn` 이라 `#[tokio::test]`
    //! 가 필요하고, 추출자(`Path`·`Query`·`HeaderMap`)는 손으로 만든다. axum 라우터를
    //! 세우지 않는 이유는 그러면 검사하는 것이 라우팅 표가 되어 버려서, 정작 이 파일의
    //! 응답 조립 코드가 다시 사각지대로 들어가기 때문이다.

    use super::*;
    use crate::assets_di::install_test_assets;
    use axum::http::Request;

    /// 자산 이름 → 이 서버가 약속한 MIME. **화면이 이 값으로 동작이 갈린다** —
    /// `text/javascript` 가 아니면 브라우저가 ES 모듈을 아예 실행하지 않는다.
    const EXPECTED: &[(&str, &str)] = &[
        ("tokens.css", "text/css; charset=utf-8"),
        ("portal.css", "text/css; charset=utf-8"),
        ("console.css", "text/css; charset=utf-8"),
        ("apidoc.css", "text/css; charset=utf-8"),
        ("core.js", "text/javascript; charset=utf-8"),
        ("portal.js", "text/javascript; charset=utf-8"),
        ("console.js", "text/javascript; charset=utf-8"),
        (
            "manifest.webmanifest",
            "application/manifest+json; charset=utf-8",
        ),
        ("favicon.svg", "image/svg+xml; charset=utf-8"),
        ("icon-192.png", "image/png"),
        ("icon-512.png", "image/png"),
        ("icon-180.png", "image/png"),
    ];

    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    /// `?v=abc` 같은 질의 문자열을 실제 요청과 **같은 경로로** 판다.
    /// 손으로 `HashMap` 을 채우면 추출자가 안 쓰이므로, 라우팅이 주는 것과
    /// 다른 모양을 넣어도 아무도 못 잡는다.
    fn query_of(raw: &str) -> Query<HashMap<String, String>> {
        let uri = format!("http://x/music/assets/x{raw}");
        let request = Request::builder().uri(uri).body(()).unwrap();
        let (parts, ()) = request.into_parts();
        Query::try_from_uri(&parts.uri).expect("질의 문자열을 읽을 수 있어야 한다")
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("본문을 읽을 수 있어야 한다")
            .to_vec()
    }

    fn header_of(response: &Response, name: header::HeaderName) -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// 등록된 자산은 **200 + 본문 + 제 MIME + `nosniff`** 로 나간다.
    ///
    /// `nosniff` 가 빠지면 브라우저가 내용을 보고 타입을 추측한다. 사용자가 올린 것이
    /// 섞이지 않는 자산이라도, 추측이 한 번 어긋나면 CSS 가 HTML 로 읽혀 화면이
    /// 통째로 안 그려진다. 헤더 한 줄이라 조용히 사라지기 딱 좋아서 여기서 못 박는다.
    #[tokio::test]
    async fn every_known_asset_is_served_with_its_type_and_nosniff() {
        install_test_assets();
        for (name, mime) in EXPECTED {
            let response =
                serve_asset(Path((*name).to_string()), query_of(""), HeaderMap::new()).await;
            assert_eq!(response.status(), StatusCode::OK, "{name}");
            assert_eq!(
                header_of(&response, header::CONTENT_TYPE).as_deref(),
                Some(*mime),
                "{name} 의 MIME 이 달라졌다"
            );
            assert_eq!(
                header_of(&response, header::X_CONTENT_TYPE_OPTIONS).as_deref(),
                Some("nosniff"),
                "{name} 에 nosniff 가 없다"
            );
            // `?v=` 없이 온 요청은 매번 재검증이다 (`portal.js` 가 정적 import 하는 `core.js`).
            assert_eq!(
                header_of(&response, header::CACHE_CONTROL).as_deref(),
                Some("no-cache"),
                "{name}"
            );
            let etag = header_of(&response, header::ETAG).expect("ETag 가 있어야 한다");
            // `"` 로 감싼 16자리 hex — 따옴표를 빼먹으면 브라우저가 ETag 로 안 읽는다.
            assert_eq!(etag.len(), 18, "{name} 의 ETag 모양이 다르다: {etag}");
            assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
            assert!(
                etag[1..17].chars().all(|c| c.is_ascii_hexdigit()),
                "{etag}"
            );

            let body = body_bytes(response).await;
            assert!(!body.is_empty(), "{name} 이 빈 본문으로 나갔다");
            if name.ends_with(".png") {
                assert!(body.starts_with(b"\x89PNG"), "{name} 이 PNG 가 아니다");
            } else {
                // `is_empty` 가 아니라 `trim`. 공백만 든 자산도 잡아야 한다.
                let text = std::str::from_utf8(&body).expect("텍스트 자산");
                assert!(!text.trim().is_empty(), "{name} 이 비었다");
            }
        }
    }

    /// 자산은 **파일 읽기가 아니라 화이트리스트 조회**다. 없는 이름은 404 로 끝난다.
    /// 여기가 파일 시스템을 건드리게 되는 순간 `../../` 이 통하는 서버가 된다.
    #[tokio::test]
    async fn an_unknown_name_is_a_404_and_never_a_file_read() {
        install_test_assets();
        for name in ["../../secret.txt", "portal.js.map", "", "core.js "] {
            let response =
                serve_asset(Path(name.to_string()), query_of(""), HeaderMap::new()).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{name:?}");
            // 404 는 자산 응답 조립을 아예 안 탄다 — ETag 를 붙이면 없는 것을 캐시하게 된다.
            assert!(header_of(&response, header::ETAG).is_none(), "{name:?}");
        }
    }

    /// **재검증은 본문을 안 보낸다.** 자산 열두 개가 매 새로고침마다 통째로 다시 나가면
    /// 리모컨은 열 때마다 수백 KB 를 다시 받는다. 그 절약이 실제로 도는지 확인한다.
    #[tokio::test]
    async fn a_matching_if_none_match_earns_a_304_with_no_body() {
        install_test_assets();
        let first = serve_asset(
            Path("core.js".to_string()),
            query_of(""),
            HeaderMap::new(),
        )
        .await;
        let etag = header_of(&first, header::ETAG).expect("ETag");
        let full = body_bytes(first).await;
        assert!(!full.is_empty());

        // 같은 ETag 로 다시 물으면 304 + 빈 본문.
        let again = serve_asset(
            Path("core.js".to_string()),
            query_of(""),
            headers_with(&[("if-none-match", &etag)]),
        )
        .await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_of(&again, header::ETAG).as_deref(), Some(&*etag));
        assert_eq!(
            header_of(&again, header::CACHE_CONTROL).as_deref(),
            Some("no-cache")
        );
        assert!(body_bytes(again).await.is_empty(), "304 에 본문이 실렸다");

        // 브라우저는 캐시에 여러 판이 있으면 **쉼표로 이어 붙여** 보낸다.
        // 목록을 통째로 한 값으로 비교하면 이 흔한 경우가 영원히 200 이 된다.
        let listed = serve_asset(
            Path("core.js".to_string()),
            query_of(""),
            headers_with(&[("if-none-match", &format!("\"0000000000000000\", {etag}"))]),
        )
        .await;
        assert_eq!(listed.status(), StatusCode::NOT_MODIFIED);

        // 남의 ETag 면 당연히 본문을 보낸다.
        let stale = serve_asset(
            Path("core.js".to_string()),
            query_of(""),
            headers_with(&[("if-none-match", "\"0000000000000000\"")]),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::OK);
        assert_eq!(body_bytes(stale).await, full);

        // 자산이 다르면 ETag 도 달라야 한다 — 같으면 한쪽이 영원히 갱신되지 않는다.
        let other = serve_asset(
            Path("portal.js".to_string()),
            query_of(""),
            HeaderMap::new(),
        )
        .await;
        assert_ne!(header_of(&other, header::ETAG).as_deref(), Some(&*etag));
    }

    /// 서비스워커와 매니페스트는 **전용 경로**로 나간다. 스코프가 경로에서 파생되므로
    /// `sw.js` 가 하위 디렉터리로 내려가면 `/music/*` 를 제어하지 못한다.
    /// 둘 다 절대 immutable 이 아니다 — 갱신을 못 받으면 앱이 옛 판에 갇힌다.
    #[tokio::test]
    async fn the_service_worker_and_manifest_have_their_own_routes() {
        install_test_assets();
        for (label, response) in [
            ("sw.js", serve_service_worker(HeaderMap::new()).await),
            ("manifest", serve_manifest(HeaderMap::new()).await),
        ] {
            assert_eq!(response.status(), StatusCode::OK, "{label}");
            assert_eq!(
                header_of(&response, header::CACHE_CONTROL).as_deref(),
                Some("no-cache"),
                "{label} 에 영구 캐시가 걸리면 앱이 옛 판에 갇힌다"
            );
            assert_eq!(
                header_of(&response, header::X_CONTENT_TYPE_OPTIONS).as_deref(),
                Some("nosniff"),
                "{label}"
            );
            assert!(!body_bytes(response).await.is_empty(), "{label} 이 비었다");
        }

        let sw = serve_service_worker(HeaderMap::new()).await;
        assert_eq!(
            header_of(&sw, header::CONTENT_TYPE).as_deref(),
            Some("text/javascript; charset=utf-8")
        );
        let etag = header_of(&sw, header::ETAG).expect("ETag");
        let revalidated = serve_service_worker(headers_with(&[("if-none-match", &etag)])).await;
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);

        let manifest = serve_manifest(HeaderMap::new()).await;
        assert_eq!(
            header_of(&manifest, header::CONTENT_TYPE).as_deref(),
            Some("application/manifest+json; charset=utf-8")
        );
        let etag = header_of(&manifest, header::ETAG).expect("ETag");
        let revalidated = serve_manifest(headers_with(&[("if-none-match", &etag)])).await;
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    }

    /// `?v=` 가 현재 버전과 **정확히** 같을 때만 영구 캐시를 준다.
    ///
    /// 예전 단언은 `cache_policy(Some(version()))` 이 immutable 이라는 것뿐이었는데,
    /// 그건 `version()` 이 `""` 를 돌려줘도 통과한다 — 그러면 `?v=` 가 비어 오는
    /// 정적 import 요청까지 1년 immutable 을 받아서 `core.js` 가 브라우저에 영원히 박힌다.
    /// 그래서 **버전 문자열 자체의 모양**을 먼저 못 박고, 빈 값·앞자리만 같은 값·
    /// 남의 값이 전부 재검증으로 떨어지는지를 본다.
    ///
    /// 해시 리터럴을 그대로 박지 않는 이유: 이 값은 자산 열세 개의 내용 해시라
    /// CSS 한 줄만 고쳐도 바뀐다. 그 리터럴을 박으면 이 저장소에서 제일 잦은 작업이
    /// 매번 테스트를 깨뜨리고, 결국 아무 생각 없이 갱신하는 줄이 된다.
    #[test]
    fn only_an_exact_version_query_earns_immutable() {
        install_test_assets();
        let current = version();
        // SHA-256 앞 16자리 hex. 빈 문자열·짧은 값이면 여기서 걸린다.
        assert_eq!(current.len(), 16, "자산 버전 모양이 달라졌다: {current:?}");
        assert!(
            current.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "자산 버전은 소문자 hex 여야 한다: {current}"
        );

        const IMMUTABLE: &str = "public, max-age=31536000, immutable";
        assert_eq!(cache_policy(Some(current)), IMMUTABLE);
        assert_eq!(cache_policy(None), "no-cache");
        assert_eq!(cache_policy(Some("deadbeef")), "no-cache");
        // 빈 `?v=` — `portal.js` 가 `./core.js` 를 정적 import 할 때 실제로 이렇게 온다.
        assert_eq!(cache_policy(Some("")), "no-cache");
        // 앞자리만 같은 값도 안 된다. 접두사 비교로 느슨해지면 옛 판이 영구 캐시된다.
        assert_eq!(cache_policy(Some(&current[..8])), "no-cache");
        assert_eq!(cache_policy(Some(&format!("{current}x"))), "no-cache");
    }

    /// 정책이 **응답 헤더까지** 그대로 흘러가는지. `cache_policy` 만 맞고 배선이 끊기면
    /// 단위 테스트는 초록인데 브라우저는 아무것도 캐시하지 않는다.
    #[tokio::test]
    async fn the_cache_policy_reaches_the_response() {
        install_test_assets();
        let hit = serve_asset(
            Path("core.js".to_string()),
            query_of(&format!("?v={}", version())),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            header_of(&hit, header::CACHE_CONTROL).as_deref(),
            Some("public, max-age=31536000, immutable")
        );

        let miss = serve_asset(
            Path("core.js".to_string()),
            query_of("?v=deadbeef"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            header_of(&miss, header::CACHE_CONTROL).as_deref(),
            Some("no-cache")
        );

        // 304 로 떨어져도 캐시 정책은 같이 나가야 한다. 안 그러면 브라우저가
        // 재검증 뒤에 정책을 잃고 다음 번에 또 통째로 받아 간다.
        let etag = header_of(&hit, header::ETAG).expect("ETag");
        let revalidated = serve_asset(
            Path("core.js".to_string()),
            query_of(&format!("?v={}", version())),
            headers_with(&[("if-none-match", &etag)]),
        )
        .await;
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            header_of(&revalidated, header::CACHE_CONTROL).as_deref(),
            Some("public, max-age=31536000, immutable")
        );
    }
}
