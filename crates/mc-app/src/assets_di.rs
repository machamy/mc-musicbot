//! 자산 주입 — `mc-app` 이 `mc-assets` 를 **의존하지 않고** 자산 바이트를 얻는 통로.
//!
//! ## 왜 의존하지 않나
//!
//! 자산 파일 한 글자를 고치면 그것을 의존하는 크레이트가 전부 다시 컴파일된다.
//! 이 저장소에서 그건 40,000줄이고 릴리스 빌드로 **54.4초**다(실측). JS·CSS 수정은
//! 이 프로젝트에서 제일 잦은 작업이라 그 비용이 매일 쌓인다.
//!
//! 그래서 방향을 뒤집었다. `mc-app` 은 함수 포인터 묶음을 받고, 실제 바이트를 아는 것은
//! 얇은 실행 파일(`src/main.rs`)뿐이다. 자산이 바뀌면 `mc-assets` 와 그 얇은 bin 만
//! 다시 컴파일되고 `mc-app` 은 `Fresh` 로 남는다.
//!
//! **`Cargo.toml` 의 `[dependencies]` 에 `mc-assets` 를 넣는 순간 이 이점이 사라진다.**
//! (`[dev-dependencies]` 는 괜찮다 — dev 의존은 릴리스 lib 타깃 지문에 안 들어간다.)
//!
//! ## 왜 트레이트가 아니라 함수 포인터인가
//!
//! 자산은 프로세스당 하나이고 상태가 없다. `dyn Trait` 을 쓰면 수명과 `Send + Sync` 를
//! 서명마다 끌고 다녀야 하는데 얻는 것이 없다.
//!
//! ## 왜 `OnceLock` 인가
//!
//! 이 저장소가 이미 쓰는 관용구다 — `App` 의 훅 넷(`on_queue_sorted`·`on_restarting`·
//! `on_chart_prefetch`·`web_listener_count`)과 같은 방식이라 새 개념이 아니다.

use std::sync::OnceLock;

/// 자산 하나 — `(바이트, MIME)`.
pub type Asset = (&'static [u8], &'static str);

/// 실행 파일이 넘겨 주는 자산 접근자 묶음.
#[derive(Clone, Copy)]
pub struct Assets {
    /// 공개 화이트리스트 조회. 여기 없는 이름은 밖으로 못 나간다.
    pub get: fn(&str) -> Option<Asset>,
    /// 서비스워커 — 스코프 때문에 `/music/sw.js` 에서만 나간다.
    pub service_worker: fn() -> Asset,
    pub manifest: fn() -> Asset,
    /// API 가이드 본문. 세션 게이트 뒤에서만 쓴다.
    pub apidoc_body: fn() -> &'static str,
    pub changelog: fn() -> &'static str,
    /// 자산 내용 해시. 페이지 셸의 `?v=` 와 캐시 정책이 쓴다.
    pub version: fn() -> &'static str,
}

static ASSETS: OnceLock<Assets> = OnceLock::new();

/// 기동 시 한 번. 두 번째 호출은 조용히 무시된다.
pub fn install_assets(assets: Assets) {
    let _ = ASSETS.set(assets);
}

/// 설치된 자산 접근자.
///
/// # Panics
/// 설치 전에 부르면 패닉한다. 이건 배선 실수라 기동 즉시 드러나는 편이 낫다 —
/// 빈 자산을 내보내면 화면이 조용히 깨진 채로 돌아간다.
pub(crate) fn assets() -> &'static Assets {
    ASSETS
        .get()
        .expect("install_assets 가 먼저 불려야 한다 (src/main.rs 배선을 확인하세요)")
}

/// 테스트 전용 설치. 단위 테스트는 `main()` 을 안 거치므로 스스로 꽂아야 한다.
///
/// **`#[cfg(test)]` 가 붙어 있어야 한다.** 안 붙이면 릴리스 빌드에도 `mc_assets` 참조가
/// 생겨서 `[dependencies]` 로 승격해야 하고, 그 순간 이 모듈의 존재 이유가 사라진다.
#[cfg(test)]
pub(crate) fn install_test_assets() {
    install_assets(Assets {
        get: mc_assets::get,
        service_worker: mc_assets::service_worker,
        manifest: mc_assets::manifest,
        apidoc_body: mc_assets::apidoc_body,
        changelog: mc_assets::changelog,
        version: mc_assets::version,
    });
}
