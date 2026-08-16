//! mc-musicbot — 실행 파일.
//!
//! **여기에 로직을 넣지 마라.** 이 파일이 하는 일은 자산(`mc-assets`)과 본체(`mc-app`)를
//! 잇는 것뿐이고, 얇은 상태를 유지하는 데 이유가 있다.
//!
//! `mc-app` 이 `mc-assets` 를 의존하면 JS·CSS 한 글자를 고칠 때마다 본체 40,000줄이
//! 다시 컴파일된다(실측 54.4초). 그래서 자산을 아는 크레이트는 이 파일 하나뿐이고,
//! 본체는 함수 포인터로 주입받는다. 자산이 바뀌면 `mc-assets` 와 이 파일만 다시 컴파일된다.

/// 실행 파일이 꽂는 자산 접근자 묶음.
///
/// **함수로 뽑아 둔 이유는 테스트 때문이다.** 이 묶음의 필드는 타입이 겹친다 —
/// `service_worker` 와 `manifest` 가 둘 다 `fn() -> Asset` 이고, `apidoc_body` 와
/// `changelog` 가 둘 다 `fn() -> &'static str` 이다. 둘을 서로 바꿔 꽂아도
/// **컴파일이 통과한다.** 그러면 `/music/sw.js` 가 매니페스트 MIME 으로 나가서
/// 모든 브라우저의 서비스워커 등록이 깨지는데, 아무 테스트도 안 깨진다.
fn wiring() -> mc_app::Assets {
    mc_app::Assets {
        get: mc_assets::get,
        service_worker: mc_assets::service_worker,
        manifest: mc_assets::manifest,
        apidoc_body: mc_assets::apidoc_body,
        changelog: mc_assets::changelog,
        version: mc_assets::version,
    }
}

fn main() {
    mc_app::install_assets(wiring());
    mc_app::run();
}

#[cfg(test)]
mod tests {
    /// 배선이 서로 뒤바뀌지 않았는지 **실제로 꽂는 그 묶음**으로 확인한다.
    ///
    /// `mc-app` 쪽 `install_test_assets` 는 이 배선을 손으로 베낀 사본이라,
    /// 거기만 맞아도 여기가 틀린 것을 못 잡는다. 그래서 이 테스트는 얇은 bin 에 있다.
    #[test]
    fn the_asset_wiring_is_not_crossed() {
        let a = super::wiring();

        // 같은 타입이라 바꿔 꽂아도 컴파일되는 짝 — 내용으로 가른다.
        assert_eq!(
            (a.service_worker)().1,
            "text/javascript; charset=utf-8",
            "sw.js 자리에 매니페스트가 꽂혔다 — 서비스워커 등록이 깨진다"
        );
        assert_eq!(
            (a.manifest)().1,
            "application/manifest+json; charset=utf-8",
            "매니페스트 자리에 다른 게 꽂혔다"
        );
        assert!(
            (a.apidoc_body)().contains("<div"),
            "apidoc 자리에 HTML 이 아닌 것이 꽂혔다"
        );
        assert!(
            (a.changelog)().starts_with("# "),
            "changelog 자리에 마크다운이 아닌 것이 꽂혔다"
        );

        // 화이트리스트와 버전도 제 것인지.
        assert!((a.get)("portal.js").is_some());
        assert!((a.get)("sw.js").is_none(), "sw.js 는 화이트리스트 밖이어야 한다");
        assert_eq!((a.version)().len(), 16);
    }
}
