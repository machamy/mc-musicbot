//! 마참뮤직 리모컨 페이지 셸.
//!
//! **여기에는 마크업 로직이 들어가지 않는다.** 렌더링은 전부 클라이언트가 한다(사양서 §5.2 F).
//! 서버가 주는 것은 빈 컨테이너 하나(`#app`)와 부트스트랩 JSON(`window.MACHAM`)뿐이다.
//!
//! 지켜야 하는 순서 (`docs/REMOTE-API-V2.md` §1):
//!   1. `<head>` 안에 FOUC 방지 인라인 스크립트 — 키 이름은 `macham.theme` 고정.
//!   2. `tokens.css` → `portal.css`(또는 `console.css`) 순서로 링크.
//!   3. `window.MACHAM` 을 심은 뒤 `<script type="module">` 로 진입점을 로드.
//!
//! 캐시버스팅은 **쿼리스트링으로만** 한다. `portal.js`/`console.js`가 `./core.js`를
//! 상대경로로 import하므로 셋이 같은 URL 디렉터리(`/music/assets/`)에 있어야 한다.

use super::html_escape;
use super::remote::{AccessTier, OAuthGuild, RemoteSession};
use serde_json::{Value, json};

/// 테마 깜빡임 방지 — 스타일시트보다 먼저 실행돼야 한다.
const THEME_BOOT: &str =
    r#"<script>try{document.documentElement.dataset.theme=localStorage.getItem('macham.theme')||'dark'}catch(e){}</script>"#;

/// JSON 리터럴을 `<script>` 안에 넣을 때 `</script>` 조기 종료를 막는다.
fn script_json(value: &Value) -> String {
    value
        .to_string()
        .replace("</", r"<\/")
        .replace("\u{2028}", r" ")
        .replace("\u{2029}", r" ")
}

/// 공통 셸. `stylesheet`는 `tokens.css` 다음에 오는 화면별 CSS 이름이다.
fn shell(
    title: &str,
    // 셸은 더 이상 BUILD_ID 를 쓰지 않는다. 부트스트랩 JSON 의 buildId 는 호출부가 넣는다.
    _build_id: &str,
    stylesheet: &str,
    entry: &str,
    bootstrap: &Value,
    body: &str,
) -> String {
    // 캐시 무효화는 BUILD_ID 가 아니라 에셋 내용 해시로 한다.
    // BUILD_ID 는 포터블 배포본에만 있어 개발 중에는 비는데, 빈 ?v= 와 immutable 이
    // 겹치면 브라우저가 옛 에셋을 영원히 붙든다. bootstrap 의 buildId 는 그대로 둔다.
    let build = super::assets::version();
    format!(
        r##"<!doctype html><html lang="ko"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="color-scheme" content="dark light">
<meta name="theme-color" content="#07090f">
<title>{title} · 마참뮤직</title>
{THEME_BOOT}
<link rel="icon" href="/music/assets/favicon.svg?v={build}">
<link rel="apple-touch-icon" href="/music/assets/icon-180.png?v={build}">
<link rel="manifest" href="/music/manifest.webmanifest?v={build}">
<link rel="stylesheet" href="/music/assets/tokens.css?v={build}">
<link rel="stylesheet" href="/music/assets/{stylesheet}?v={build}">
</head><body>
{body}
<script>window.MACHAM={bootstrap};</script>
<script type="module" src="/music/assets/{entry}?v={build}"></script>
<script>if('serviceWorker' in navigator)window.addEventListener('load',function(){{navigator.serviceWorker.register('/music/sw.js?v={build}').catch(function(){{}})}});</script>
</body></html>"##,
        title = html_escape(title),
        bootstrap = script_json(bootstrap),
    )
}

/// 스크립트 없이 그리는 최소 페이지 (로그인·서버 선택·거부 화면).
fn plain(title: &str, _build_id: &str, body: &str) -> String {
    let build = super::assets::version();
    format!(
        r#"<!doctype html><html lang="ko"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="color-scheme" content="dark light">
<title>{title} · 마참뮤직</title>
{THEME_BOOT}
<link rel="icon" href="/music/assets/favicon.svg?v={build}">
<link rel="stylesheet" href="/music/assets/tokens.css?v={build}">
<link rel="stylesheet" href="/music/assets/portal.css?v={build}">
</head><body class="gate">{body}</body></html>"#,
        title = html_escape(title),
    )
}

fn user_json(session: &RemoteSession) -> Value {
    json!({
        // 모든 u64 ID는 JSON에서 문자열이다 (계약 §0).
        "id": session.user_id.to_string(),
        "displayName": session.display_name,
        "avatarUrl": session.avatar_url,
    })
}

fn guild_json(guild: &OAuthGuild) -> Value {
    json!({
        "id": guild.id.to_string(),
        "name": guild.name,
        "iconUrl": guild.icon_url(),
    })
}

/// `GET /music/login`
pub fn login(configured: bool, dev_login: bool, message: Option<&str>) -> String {
    let action = if configured {
        r#"<a class="btn btn--primary btn--wide" data-testid="discord-login" href="/music/oauth/start">Discord로 계속하기</a>"#.to_string()
    } else {
        r#"<div class="gate__note"><strong>OAuth 설정이 필요하다.</strong><br>운영 패널 → 봇 설정에서 Discord Client ID / Secret / 공개 URL을 넣어라.</div>"#.to_string()
    };
    let dev = if dev_login {
        r#"<form method="post" action="/music/dev-login" class="gate__dev"><button class="btn btn--ghost btn--wide" data-testid="dev-login" type="submit">로컬 검증 계정으로 입장</button></form>"#
    } else {
        ""
    };
    let notice = message
        .map(|value| format!(r#"<div class="gate__note">{}</div>"#, html_escape(value)))
        .unwrap_or_default();
    plain(
        "로그인",
        "",
        &format!(
            r#"<main class="gate__wrap"><section class="gate__card">
<div class="gate__logo" aria-hidden="true">♫</div>
<h1>마참뮤직</h1>
<p class="gate__lead">서버 음악을 같이 고르고, 투표하고, 한 화면에서 조작한다.</p>
{notice}{action}{dev}
<p class="gate__foot">로그인하면 Discord 서버 멤버십과 권한을 확인한다.</p>
</section></main>"#
        ),
    )
}

/// `GET /music` — 봇이 들어가 있는 서버 목록.
pub fn guild_selector(session: &RemoteSession, guilds: &[OAuthGuild]) -> String {
    let cards: String = guilds
        .iter()
        .map(|guild| {
            let icon = guild
                .icon_url()
                .map(|url| format!(r#"<img class="ava" src="{}" alt="">"#, html_escape(&url)))
                .unwrap_or_else(|| r#"<span class="ava ava--fallback">🎵</span>"#.to_string());
            format!(
                r#"<a class="gate__guild" data-testid="guild-card" href="/music/guilds/{id}">{icon}<span>{name}</span></a>"#,
                id = guild.id,
                name = html_escape(&guild.name),
            )
        })
        .collect();
    let empty = if guilds.is_empty() {
        r#"<p class="gate__foot">봇이 들어가 있는 서버가 없다. 먼저 봇을 서버에 초대해라.</p>"#
    } else {
        ""
    };
    plain(
        "서버 선택",
        "",
        &format!(
            r#"<main class="gate__wrap"><section class="gate__card gate__card--wide">
<div class="gate__logo" aria-hidden="true">♫</div>
<h1>어서 와, {user}</h1>
<p class="gate__lead">리모컨을 열 서버를 골라라. 좋아요와 보관함은 서버마다 따로 관리된다.</p>
<div class="gate__guilds">{cards}</div>{empty}
<form method="post" action="/music/logout" class="gate__dev"><input type="hidden" name="csrf" value="{csrf}"><button class="btn btn--ghost" type="submit">로그아웃</button></form>
</section></main>"#,
            user = html_escape(&session.display_name),
            csrf = html_escape(&session.csrf_token),
        ),
    )
}

/// `GET /music/guilds/{id}` — 유저 UI 셸.
pub fn guild(
    session: &RemoteSession,
    guild: &OAuthGuild,
    build_id: &str,
    tier: AccessTier,
) -> String {
    let bootstrap = json!({
        "guildId": guild.id.to_string(),
        "csrf": session.csrf_token,
        "buildId": build_id,
        "user": user_json(session),
        "tier": tier.as_str(),
        "guild": guild_json(guild),
        "themeDefault": "dark",
    });
    shell(
        &guild.name,
        build_id,
        "portal.css",
        "portal.js",
        &bootstrap,
        r#"<div id="app" data-testid="music-portal"></div><noscript><p style="padding:24px">마참뮤직 리모컨은 자바스크립트가 필요하다.</p></noscript>"#,
    )
}

/// `GET /music/guilds/{id}/admin` — 서버 관리 콘솔 셸 (Manager 이상).
pub fn admin(
    session: &RemoteSession,
    guild: &OAuthGuild,
    build_id: &str,
    tier: AccessTier,
    intent_status: &Value,
) -> String {
    let bootstrap = json!({
        "guildId": guild.id.to_string(),
        "csrf": session.csrf_token,
        "buildId": build_id,
        "user": user_json(session),
        "tier": tier.as_str(),
        "guild": guild_json(guild),
        "intentStatus": intent_status,
    });
    shell(
        &format!("서버 관리 · {}", guild.name),
        build_id,
        "console.css",
        "console.js",
        &bootstrap,
        r#"<div id="app"></div><noscript><p style="padding:24px">서버 관리 콘솔은 자바스크립트가 필요하다.</p></noscript>"#,
    )
}

/// 관리 콘솔 진입 거부 화면. 서버가 403을 주지만 사람이 읽을 수 있어야 한다.
pub fn denied(message: &str, guild_id: u64) -> String {
    plain(
        "권한 없음",
        "",
        &format!(
            r#"<main class="gate__wrap"><section class="gate__card">
<div class="gate__logo" aria-hidden="true">🔒</div>
<h1>들어갈 수 없다</h1>
<p class="gate__lead">{message}</p>
<a class="btn btn--primary btn--wide" href="/music/guilds/{guild_id}">← 리모컨으로 돌아가기</a>
</section></main>"#,
            message = html_escape(message),
        ),
    )
}
