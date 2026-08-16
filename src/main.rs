//! mc-musicbot — 실행 파일.
//!
//! **여기에 로직을 넣지 마라.** 이 파일이 하는 일은 자산(`mc-assets`)과 본체(`mc-app`)를
//! 잇는 것뿐이고, 얇은 상태를 유지하는 데 이유가 있다.
//!
//! `mc-app` 이 `mc-assets` 를 의존하면 JS·CSS 한 글자를 고칠 때마다 본체 40,000줄이
//! 다시 컴파일된다(실측 54.4초). 그래서 자산을 아는 크레이트는 이 파일 하나뿐이고,
//! 본체는 함수 포인터로 주입받는다. 자산이 바뀌면 `mc-assets` 와 이 파일만 다시 컴파일된다.

fn main() {
    mc_app::install_assets(mc_app::Assets {
        get: mc_assets::get,
        service_worker: mc_assets::service_worker,
        manifest: mc_assets::manifest,
        apidoc_body: mc_assets::apidoc_body,
        changelog: mc_assets::changelog,
        version: mc_assets::version,
    });
    mc_app::run();
}
