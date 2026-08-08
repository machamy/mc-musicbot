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

/// `next` 경로를 쿼리스트링에 넣기 위한 최소 인코딩.
/// 호출부에서 이미 `/music/` 로 시작하는 내부 경로만 통과시켰으므로
/// 여기서는 쿼리 구분자로 오해될 문자만 막으면 된다.
fn percent_encode_path(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// 테마 깜빡임 방지 — 스타일시트보다 먼저 실행돼야 한다.
///
/// 저장값을 그대로 `data-theme` 에 넣는다. 테마가 7종으로 늘어도(V3 §17) 여기는 그대로다.
/// **`auto` 만 예외**다 — 그건 값이 아니라 규칙이라 여기서 풀어 준다.
/// 안 풀면 `data-theme="auto"` 가 박혀서 어느 토큰 블록에도 안 걸리고 화면이 기본색으로 뜬다.
///
/// `<meta name="theme-color">` 도 같이 맞춘다. 모바일 주소창만 반대 색이면 어색하다.
const THEME_BOOT: &str = r#"<script>try{
var t=localStorage.getItem('macham.theme')||'dark';
if(t==='auto')t=matchMedia('(prefers-color-scheme: light)').matches?'light':'dark';
document.documentElement.dataset.theme=t;
var light=t==='light'||t==='sepia';
document.documentElement.style.colorScheme=light?'light':'dark';
}catch(e){}</script>"#;

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
<meta name="theme-color" id="theme-color" content="#07090f">
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
///
/// `next` 는 로그인 후 돌아갈 내부 경로다. 특정 서버의 리모컨을 열려다 막힌 경우
/// 그 주소가 넘어오고, 로그인이 끝나면 서버 선택을 건너뛰고 바로 그 화면으로 간다.
/// 호출부에서 이미 `/music/` 로 시작하는지 검증한 값만 들어온다.
pub fn login(
    configured: bool,
    dev_login: bool,
    message: Option<&str>,
    next: Option<&str>,
) -> String {
    let next_query = next
        .map(|path| format!("?next={}", percent_encode_path(path)))
        .unwrap_or_default();
    let action = if configured {
        format!(
            r#"<a class="btn btn--primary btn--wide" data-testid="discord-login" href="/music/oauth/start{next_query}">Discord로 계속하기</a>"#
        )
    } else {
        r#"<div class="gate__note"><strong>OAuth 설정이 아직 없어요.</strong><br>운영 패널 → 봇 설정에서 Discord Client ID / Secret / 공개 URL을 넣어 주세요.</div>"#.to_string()
    };
    let dev = if dev_login {
        // dev 로그인은 폼 POST 라 next 를 hidden 으로 실어 보낸다.
        let hidden = next
            .map(|path| format!(r#"<input type="hidden" name="next" value="{}">"#, html_escape(path)))
            .unwrap_or_default();
        &format!(
            r#"<form method="post" action="/music/dev-login" class="gate__dev">{hidden}<button class="btn btn--ghost btn--wide" data-testid="dev-login" type="submit">로컬 검증 계정으로 입장</button></form>"#
        )
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
<p class="gate__lead">서버 음악을 같이 고르고, 투표하고, 한 화면에서 조작해요.</p>
{notice}{action}{dev}
<p class="gate__foot">로그인하면 Discord 서버 멤버십과 권한을 확인해요.</p>
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
        r#"<p class="gate__foot">봇이 들어가 있는 서버가 없어요. 먼저 봇을 서버에 초대해 주세요.</p>"#
    } else {
        ""
    };
    plain(
        "서버 선택",
        "",
        &format!(
            r#"<main class="gate__wrap"><section class="gate__card gate__card--wide">
<div class="gate__logo" aria-hidden="true">♫</div>
<h1>어서 오세요, {user}</h1>
<p class="gate__lead">리모컨을 열 서버를 골라 주세요. 좋아요와 보관함은 서버마다 따로 관리돼요.</p>
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
        // 테마 7종 + 시스템 따라가기 (V3 §17). 화면이 목록을 따로 들고 있으면 서버와 어긋난다.
        "themes": ["auto", "dark", "light", "midnight", "slate", "sepia", "retro", "nord"],
    });
    shell(
        &guild.name,
        build_id,
        "portal.css",
        "portal.js",
        &bootstrap,
        r#"<div id="app" data-testid="music-portal"></div><noscript><p style="padding:24px">마참뮤직 리모컨은 자바스크립트가 있어야 움직여요. 브라우저에서 자바스크립트를 켜 주세요.</p></noscript>"#,
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
        r#"<div id="app"></div><noscript><p style="padding:24px">서버 관리 콘솔은 자바스크립트가 있어야 움직여요. 브라우저에서 자바스크립트를 켜 주세요.</p></noscript>"#,
    )
}

/// 로그인 없이 보는 "지금 이 곡" 화면 (§29).
///
/// **일부러 아주 작다.** 리모컨 셸을 재사용하면 로그인 안 한 사람에게 채팅·멤버·대기열
/// 코드까지 내려가고, 언젠가 그중 하나가 데이터를 요구하게 된다. 이 화면은 곡 하나만
/// 그리는 독립 페이지라 실수로 더 내보낼 여지가 없다.
///
/// 조작 요소가 하나도 없다 — 버튼도, 폼도 없다. 읽기 전용이라는 말이 코드에서도 참이다.
pub fn public_now(guild_id: u64, build_id: &str) -> String {
    plain(
        "지금 이 곡",
        "",
        &format!(
            r#"<main class="gate__wrap"><section class="gate__card" id="pub" data-guild="{guild_id}">
<div class="gate__logo" aria-hidden="true">🎵</div>
<h1 id="pub-title">불러오는 중…</h1>
<p class="gate__lead" id="pub-artist"></p>
<p class="gate__lead" id="pub-queue"></p>
<p class="gate__lead" style="opacity:.7;font-size:.85em">보기만 할 수 있어요. 조작하려면 로그인해야 해요.</p>
<a class="btn btn--primary btn--wide" href="/music/guilds/{guild_id}">Discord로 로그인하고 조작하기</a>
</section></main>
<script>
// 5초마다 갱신한다. WebSocket 을 안 쓰는 이유: 로그인 안 한 사람에게 소켓을 열어 주면
// 그 자체가 붙잡아 둘 자원이 된다. 이 화면은 가볍게 폴링만 한다.
(function () {{
  var box = document.getElementById('pub');
  var url = '/music/api/guilds/' + box.dataset.guild + '/public?v={build_id}';
  var tag = null;
  function paint(d) {{
    var t = document.getElementById('pub-title');
    var a = document.getElementById('pub-artist');
    var q = document.getElementById('pub-queue');
    if (!d || !d.current) {{ t.textContent = '지금은 아무 곡도 안 나와요'; a.textContent = ''; q.textContent = ''; return; }}
    t.textContent = d.current.title;
    a.textContent = (d.current.artist || '') + (d.isPaused ? ' · 일시정지' : '');
    q.textContent = d.queueTotal ? ('대기열 ' + d.queueTotal + '곡') : '';
  }}
  function tick() {{
    var opt = {{ headers: {{}} }};
    if (tag) opt.headers['If-None-Match'] = tag;
    fetch(url, opt).then(function (r) {{
      if (r.status === 304) return null;      // 안 바뀌었으면 아무것도 안 한다
      if (!r.ok) throw new Error(String(r.status));
      tag = r.headers.get('ETag');
      return r.json();
    }}).then(function (d) {{ if (d) paint(d); }}).catch(function () {{
      document.getElementById('pub-title').textContent = '지금은 볼 수 없어요';
    }});
  }}
  tick();
  setInterval(tick, 5000);
}})();
</script>"#,
        ),
    )
}

/// API 가이드 본문. 마크업이 길어서 별도 파일로 뺐다.
///
/// **에셋 테이블(`assets.rs` 의 `lookup`)에는 일부러 넣지 않았다.** 거기 넣으면
/// `/music/assets/apidoc.html` 이 인증 없이 열리고, 아래 페이지에 걸어 둔 세션 검사가
/// 우회된다. 여기서 `include_str!` 로 읽어 셸 안에서만 내보낸다.
const APIDOC_BODY: &str = include_str!("assets/apidoc.html");

/// `GET /music/apidoc` — API 가이드 문서.
///
/// `plain()` 을 쓰지 않는다. 그쪽은 `portal.css` 를 박아 두는데, 문서 화면은 자기 CSS 를
/// 써야 하고 셸 규칙상 토큰 다음에 오는 화면별 CSS 는 **하나뿐**이기 때문이다.
///
/// 스크립트가 없다. 읽기만 하는 화면이라 붙일 것이 없고, 없으면 새어 나갈 것도 없다.
/// (테마 깜빡임 방지 스크립트만 예외 — 스타일시트보다 먼저 돌아야 한다.)
pub fn apidoc() -> String {
    let build = super::assets::version();
    format!(
        r#"<!doctype html><html lang="ko"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="color-scheme" content="dark light">
<title>API 가이드 · 마참뮤직</title>
{THEME_BOOT}
<link rel="icon" href="/music/assets/favicon.svg?v={build}">
<link rel="stylesheet" href="/music/assets/tokens.css?v={build}">
<link rel="stylesheet" href="/music/assets/apidoc.css?v={build}">
</head><body>{APIDOC_BODY}</body></html>"#
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
<h1>여기는 못 들어가요</h1>
<p class="gate__lead">{message}</p>
<a class="btn btn--primary btn--wide" href="/music/guilds/{guild_id}">← 리모컨으로 돌아가기</a>
</section></main>"#,
            message = html_escape(message),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 셸 규칙: 토큰 다음에 오는 화면별 CSS 는 하나여야 한다.
    /// `portal.css` 가 같이 실리면 문서 화면이 리모컨 레이아웃을 뒤집어쓴다.
    #[test]
    fn apidoc_links_tokens_then_only_its_own_stylesheet() {
        let html = apidoc();
        let tokens = html.find("tokens.css").expect("tokens.css 링크가 없다");
        let own = html.find("apidoc.css").expect("apidoc.css 링크가 없다");
        assert!(tokens < own, "tokens.css 가 화면 CSS 보다 먼저 와야 한다");
        assert!(!html.contains("portal.css"));
        assert!(!html.contains("console.css"));
    }

    /// 문서가 **코드에 없는 주소를 적지 못하게** 막는다.
    ///
    /// 이 문서의 값어치는 "지금 서버가 실제로 여는 경로" 라는 데 있다. 라우트를 지웠는데
    /// 문서만 남으면 그때부터는 도움이 아니라 함정이다. 그래서 본문의 `<code>` 안에 적힌
    /// 경로를 전부 긁어서 `remote.rs` 원문에 있는지 본다.
    ///
    /// 표에서는 길드 접두사를 빼고 뒷부분만 적으므로(`/state/hot`), `/music` 으로 시작하지
    /// 않는 값에는 접두사를 붙여서 찾는다.
    #[test]
    fn every_path_in_the_apidoc_exists_in_the_router() {
        // 테스트에서만 원문을 읽는다. 배포 바이너리에 소스가 딸려 들어가면 안 된다.
        let router_source = include_str!("remote.rs");

        // 이 라우터에 없는 것이 정상인 주소. 늘리기 전에 정말 그런지 확인할 것.
        const SKIP: &[&str] = &["/healthz"]; // mod.rs 의 라우터에 있다

        let mut checked = 0;
        for chunk in APIDOC_BODY.split("<code>").skip(1) {
            let Some(text) = chunk.split("</code>").next() else {
                continue;
            };
            // 하나의 주소가 아니라 "이 아래 전부" 를 가리키는 조각은 건너뛴다
            // (`/music/*` · `/music/api/guilds/{guild_id}/…` · 접두사를 뜻하는 `/music/`).
            let is_prefix_form = text.ends_with('*') || text.ends_with('…') || text.ends_with('/');
            // `//evil.example` 같은 프로토콜 상대 주소는 우리 라우트가 아니라 반례다.
            let is_external = text.starts_with("//");
            if !text.starts_with('/') || is_prefix_form || is_external || SKIP.contains(&text) {
                continue;
            }
            let needle = if text.starts_with("/music") {
                text.to_string()
            } else {
                format!("/music/api/guilds/{{guild_id}}{text}")
            };
            assert!(
                router_source.contains(&needle),
                "문서에 적힌 {needle} 이 remote.rs 에 없다"
            );
            checked += 1;
        }
        // 스킵 목록이 잘못 커져서 사실상 아무것도 안 보는 상태가 되는 걸 막는다.
        assert!(checked > 50, "검사한 경로가 {checked}개뿐이다");
    }
}
