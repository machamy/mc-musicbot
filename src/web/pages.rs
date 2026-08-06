//! 웹 UI 페이지 핸들러 13종 + 액션 — C# Razor Pages 의 기능 등가 포팅.

use crate::models::*;
use crate::web::{
    Ctx, admin_csrf_token, begin_session, end_session, hash_password, html_escape, layout,
    require_auth, store_hash, verify_admin_csrf,
};
use axum::Form;
use axum::extract::{ConnectInfo, Query};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use tower_cookies::Cookies;

/// 두 해시의 고정시간 비교 (타이밍 누설 방지).
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ───────── 공용 헬퍼 ─────────

/// C# TempData 상태 배너 흐름의 등가물 — `?msg=...(&err=1)` 로 리다이렉트하면
/// layout 의 FLASH_JS 가 페이지 상단에 초록/빨강 배너를 그린다.
fn redirect_flash(path: &str, msg: &str, is_err: bool) -> Response {
    let sep = if path.contains('?') { '&' } else { '?' };
    let err = if is_err { "&err=1" } else { "" };
    Redirect::to(&format!("{path}{sep}msg={}{err}", url_encode(msg))).into_response()
}

/// ISO UTC 타임스탬프 → "MM-dd HH:mm:ss" (C# 로그 뷰어 표기와 동일 모양, UTC 기준).
fn fmt_ts(ts: &str) -> String {
    if ts.len() >= 19
        && ts.is_char_boundary(5)
        && ts.is_char_boundary(10)
        && ts.is_char_boundary(11)
        && ts.is_char_boundary(19)
    {
        format!("{} {}", &ts[5..10], &ts[11..19])
    } else {
        ts.to_string()
    }
}

/// C# ToolsModel.FormatBytes 등가.
fn format_bytes(bytes: i64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes.max(0) as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < units.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.2} {}", units[unit])
}

/// 길드 아이콘 — URL 있으면 <img>, 없으면 C# 처럼 이름 첫 글자 폴백.
fn guild_icon_html(meta: Option<&GuildMetadata>) -> String {
    match meta
        .and_then(|m| m.icon_url.as_deref())
        .filter(|u| !u.is_empty())
    {
        Some(u) => format!(
            r#"<img class="guild-icon" src="{}" alt="" loading="lazy"/>"#,
            html_escape(u)
        ),
        None => {
            let initial = meta
                .map(|m| m.name.as_str())
                .and_then(|n| n.chars().next())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            format!(
                r#"<span class="guild-icon guild-icon-fallback">{}</span>"#,
                html_escape(&initial)
            )
        }
    }
}

// ───────── 로그인 ─────────

/// 로그인 페이지 전용 스타일 — C# site.css 의 로그인 관련 부분 발췌.
const LOGIN_PAGE_CSS: &str = r#"<style>
:root{--bg:#F8FAFC;--ink:#0F172A;--muted:#64748B;--card:#FFFFFF;--line:#E2E8F0;--accent:#2563EB}
*{box-sizing:border-box}
body{margin:0;font-family:"Malgun Gothic","Segoe UI",system-ui,sans-serif;background:var(--bg);color:var(--ink)}
.login-wrap{display:flex;align-items:center;justify-content:center;min-height:100vh;width:100%}
.login-card{width:360px;background:var(--card);border:1px solid var(--line);border-radius:16px;padding:28px}
.kv{color:var(--muted);font-size:13px}
label.field{display:block;margin:12px 0 4px;font-weight:600;font-size:14px}
input[type=password]{width:100%;padding:10px 12px;border:1px solid #CBD5E1;border-radius:8px;background:#F8FAFC;color:var(--ink);font-size:14px;font-family:inherit}
input:focus{outline:none;border-color:var(--accent)}
.btn{display:inline-block;cursor:pointer;font-size:14px;font-weight:600;padding:10px 16px;border-radius:10px;border:1px solid transparent}
.btn-primary{background:var(--accent);color:#fff;border-color:var(--accent)}
.actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:14px}
.status{padding:12px 14px;border-radius:12px;margin-bottom:16px;font-weight:600}
.status.err{background:#FEF2F2;color:#DC2626;border:1px solid #DC2626}
.brand-badge{display:inline-block;background:#7C3AED;color:#fff;font-size:11px;font-weight:600;padding:2px 8px;border-radius:10px;vertical-align:middle;margin-left:8px}
.build{margin-top:18px;text-align:right;font-size:11px;color:#888;font-family:monospace;letter-spacing:.04em}
</style>"#;

#[derive(Deserialize, Default)]
pub struct LoginQuery {
    error: Option<String>,
}

pub async fn login_page(State(state): Ctx, Query(q): Query<LoginQuery>) -> Html<String> {
    let build = &state.app.build_id;
    let error_html = if q.error.is_some() {
        r#"<div class="status err">비밀번호가 올바르지 않습니다.</div>"#
    } else {
        ""
    };
    Html(format!(
        r#"<!DOCTYPE html><html lang="ko"><head><meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1.0"/>
<title>로그인 · 뮤직봇 관리</title>
{css}</head>
<body><div class="login-wrap"><div class="login-card">
<h2 style="margin-top:0">mc-musicbot 관리자</h2>
<p class="kv">계속하려면 비밀번호를 입력하세요.</p>
{error_html}
<form method="post" action="/login">
<label class="field" for="password">비밀번호</label>
<input type="password" id="password" name="password" autofocus/>
<div class="actions"><button type="submit" class="btn btn-primary">로그인</button></div>
</form>
<div class="build">build {build}</div>
</div></div></body></html>"#,
        css = LOGIN_PAGE_CSS,
    ))
}

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
}

pub async fn login_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(form): Form<LoginForm>,
) -> Response {
    let stored = *state.password_hash.lock().unwrap();
    let Some(stored) = stored else {
        // 비밀번호 미설정 — 최초 설정으로.
        return Redirect::to("/setup").into_response();
    };
    let ok = ct_eq(&hash_password(&form.password), &stored);
    if ok {
        begin_session(&state, &cookies);
        Redirect::to("/").into_response()
    } else {
        // 브루트포스 완화 — 실패 시 1초 지연, 페이지 내 빨간 배너로 안내.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        Redirect::to("/login?error=1").into_response()
    }
}

pub async fn logout(State(state): Ctx, cookies: Cookies) -> Redirect {
    end_session(&state, &cookies);
    Redirect::to("/login")
}

// ───────── 최초 비밀번호 설정 (localhost) ─────────

fn setup_html(error: Option<&str>, csrf_token: &str) -> String {
    let err = error
        .map(|e| format!(r#"<div class="status err">{}</div>"#, html_escape(e)))
        .unwrap_or_default();
    format!(
        r#"<!DOCTYPE html><html lang="ko"><head><meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1.0"/>
<title>최초 설정 · 뮤직봇 관리</title>
{css}</head>
<body><div class="login-wrap"><div class="login-card">
<h2 style="margin-top:0">최초 비밀번호 설정</h2>
<p class="kv">처음 실행입니다. 웹 관리자 비밀번호를 설정하세요. (보안상 봇이 실행 중인 PC, 즉 localhost 에서만 설정할 수 있습니다.)</p>
{err}
<form method="post" action="/setup">
<input type="hidden" name="csrf_token" value="{csrf_token}"/>
<label class="field" for="password">새 비밀번호</label>
<input type="password" id="password" name="password" autofocus/>
<label class="field" for="confirm">비밀번호 확인</label>
<input type="password" id="confirm" name="confirm"/>
<div class="actions"><button type="submit" class="btn btn-primary">설정하고 시작</button></div>
</form>
</div></div></body></html>"#,
        css = LOGIN_PAGE_CSS,
        csrf_token = html_escape(csrf_token),
    )
}

pub async fn setup_page(State(state): Ctx) -> Response {
    if state.password_hash.lock().unwrap().is_some() {
        return Redirect::to("/login").into_response();
    }
    Html(setup_html(None, &state.setup_csrf)).into_response()
}

#[derive(Deserialize)]
pub struct SetupForm {
    csrf_token: String,
    password: String,
    confirm: String,
}

pub async fn setup_post(
    State(state): Ctx,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    cookies: Cookies,
    Form(f): Form<SetupForm>,
) -> Response {
    if state.password_hash.lock().unwrap().is_some() {
        return Redirect::to("/login").into_response();
    }
    if f.csrf_token != state.setup_csrf {
        return (axum::http::StatusCode::FORBIDDEN, "CSRF 검증에 실패했습니다.")
            .into_response();
    }
    // 최초 설정은 호스트(localhost)에서만 — 원격에서 비밀번호를 선점하는 것을 방지.
    if !peer.ip().is_loopback() {
        return Html(setup_html(
            Some("최초 비밀번호 설정은 봇이 실행 중인 PC(localhost)에서만 가능합니다."),
            &state.setup_csrf,
        ))
        .into_response();
    }
    if f.password.chars().count() < 4 {
        return Html(setup_html(
            Some("비밀번호는 4자 이상이어야 합니다."),
            &state.setup_csrf,
        ))
        .into_response();
    }
    if f.password != f.confirm {
        return Html(setup_html(
            Some("비밀번호 확인이 일치하지 않습니다."),
            &state.setup_csrf,
        ))
        .into_response();
    }
    let hash = hash_password(&f.password);
    if store_hash(&state.app, &hash).is_err() {
        return Html(setup_html(
            Some("비밀번호 저장에 실패했습니다 (data 디렉터리 쓰기 권한을 확인하세요)."),
            &state.setup_csrf,
        ))
        .into_response();
    }
    *state.password_hash.lock().unwrap() = Some(hash);
    state.app.log.info("Web", "웹 비밀번호 최초 설정 완료.");
    begin_session(&state, &cookies);
    Redirect::to("/").into_response()
}

// ───────── 비밀번호 변경 ─────────

pub async fn password_page(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let body = r#"<div class="card"><h2>비밀번호 변경</h2>
<form method="post" action="/password">
<label class="field">현재 비밀번호</label><input type="password" name="current"/>
<label class="field">새 비밀번호 (4자 이상)</label><input type="password" name="new_password"/>
<label class="field">새 비밀번호 확인</label><input type="password" name="confirm"/>
<div class="actions"><button class="btn btn-primary" type="submit">변경</button></div>
</form></div>"#;
    layout(&state, "비밀번호 변경", "/password", body).into_response()
}

#[derive(Deserialize)]
pub struct PasswordForm {
    current: String,
    new_password: String,
    confirm: String,
}

pub async fn password_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<PasswordForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let stored = *state.password_hash.lock().unwrap();
    let Some(stored) = stored else {
        return Redirect::to("/setup").into_response();
    };
    if !ct_eq(&hash_password(&f.current), &stored) {
        return redirect_flash("/password", "현재 비밀번호가 올바르지 않습니다.", true);
    }
    if f.new_password.chars().count() < 4 {
        return redirect_flash("/password", "새 비밀번호는 4자 이상이어야 합니다.", true);
    }
    if f.new_password != f.confirm {
        return redirect_flash("/password", "새 비밀번호 확인이 일치하지 않습니다.", true);
    }
    let hash = hash_password(&f.new_password);
    if store_hash(&state.app, &hash).is_err() {
        return redirect_flash("/password", "비밀번호 저장에 실패했습니다.", true);
    }
    *state.password_hash.lock().unwrap() = Some(hash);
    state.app.log.info("Web", "웹 비밀번호 변경됨.");
    redirect_flash("/password", "비밀번호가 변경되었습니다.", false)
}

use axum::extract::State;

// ───────── 메인 대시보드 ─────────

pub async fn index(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let (cache_count, cache_bytes) = app.cache.stats();
    let yt_ok =
        std::path::Path::new(&app.config.yt_dlp_path).is_file() || which(&app.config.yt_dlp_path);
    let ff_ok =
        std::path::Path::new(&app.config.ffmpeg_path).is_file() || which(&app.config.ffmpeg_path);
    let pid = std::process::id();
    // C# Index 와 동일하게 최근 40줄을 pre.log 한 덩어리로.
    let logs = app.log.recent(40);
    let log_text = if logs.is_empty() {
        "(로그 없음)".to_string()
    } else {
        logs.iter()
            .map(|l| {
                format!(
                    "[{}] {} {}: {}",
                    fmt_ts(&l.timestamp),
                    l.level,
                    l.category,
                    l.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let pill = |ok: bool| {
        if ok {
            r#"<span class="pill run">사용 가능</span>"#
        } else {
            r#"<span class="pill stop">없음</span>"#
        }
    };
    let body = format!(
        r#"<h1 class="page-title">메인 대시보드</h1>
<p class="page-sub">봇 프로세스와 도구 상태를 확인합니다.</p>
<div class="card">
<h2>봇 프로세스</h2>
<p class="sub">상태: <span class="pill run">실행 중</span> <span class="kv">· PID {pid}</span></p>
<p class="kv">봇과 웹 관리자가 단일 프로세스(Rust)로 통합되어 있습니다.</p>
</div>
<div class="grid2">
<div class="card">
<h2>도구 / 경로</h2>
<p class="sub">봇이 사용하는 외부 도구와 데이터 위치입니다.</p>
<p>yt-dlp: {yt_pill}</p>
<p>ffmpeg: {ff_pill}</p>
<p class="kv">데이터 루트: {data_root}</p>
<p class="kv">도구 루트: {tools_root}</p>
</div>
<div class="card">
<h2>캐시</h2>
<p class="sub">받아둔 오디오 캐시 현황입니다.</p>
<p class="kv">항목 {cache_count}개 · 총 {cache_size}</p>
<div class="actions"><a class="btn btn-secondary" href="/cache">캐시 라이브러리 열기</a></div>
</div>
</div>
<div class="card">
<h2>최근 운영 로그</h2>
<p class="sub">전체 로그는 로그 뷰어에서 확인하세요.</p>
<pre class="log">{log_text}</pre>
</div>"#,
        yt_pill = pill(yt_ok),
        ff_pill = pill(ff_ok),
        data_root = html_escape(&app.config.data_root.to_string_lossy()),
        tools_root = html_escape(&app.config.tools_root.to_string_lossy()),
        cache_size = format_bytes(cache_bytes),
        log_text = html_escape(&log_text),
    );
    layout(&state, "메인 대시보드", "/", &body).into_response()
}

fn which(name: &str) -> bool {
    // PATH 상 존재 추정 — 이름만 있으면 true 로 둔다 (실행 시 검증됨).
    !name.contains('\\') && !name.contains('/')
}

// ───────── 진단 ─────────

pub async fn diagnostics(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let meta: HashMap<u64, GuildMetadata> = app
        .db
        .list_guild_metadata()
        .into_iter()
        .map(|m| (m.guild_id, m))
        .collect();
    let gset = app.db.load_global_settings();
    let mut cards = String::new();
    for gid in app.db.list_known_guild_ids() {
        let s = app.player.get_state(gid).await;
        let name = meta
            .get(&gid)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("서버 {gid}"));
        let icon = guild_icon_html(meta.get(&gid));
        let current = s
            .current_item
            .as_ref()
            .map(|c| html_escape(c.track.display_title()))
            .unwrap_or_else(|| "(없음)".into());
        let upcoming = if s.upcoming.is_empty() {
            "0곡".to_string()
        } else {
            format!(
                "{}곡 · 다음: {}",
                s.upcoming.len(),
                html_escape(s.upcoming[0].track.display_title())
            )
        };
        let autoplay_pill = if s.autoplay_enabled {
            r#"<span class="pill run">켜짐</span>"#
        } else {
            r#"<span class="pill stop">꺼짐</span>"#
        };
        // C# 진단의 '최근 재생' 행 — 직전 3곡까지 표시.
        let recent = if s.recent_tracks.is_empty() {
            "0곡 기록".to_string()
        } else {
            let titles: Vec<String> = s
                .recent_tracks
                .iter()
                .take(3)
                .map(|t| html_escape(t.display_title()))
                .collect();
            format!(
                "{}곡 기록 · 최근: {}",
                s.recent_tracks.len(),
                titles.join(" · ")
            )
        };
        cards.push_str(&format!(
            r#"<div class="card">
<h2 style="display:flex;align-items:center;gap:10px">{icon}<span>{name}</span><span class="guild-id">{gid}</span></h2>
<div class="diag-grid">
<div class="k">현재 곡</div><div>{current}</div>
<div class="k">대기열</div><div>{upcoming}</div>
<div class="k">자동추천</div><div>{autoplay_pill} <span class="kv">(전역 기본: {autoplay_default})</span></div>
<div class="k">반복 / 셔플</div><div>{repeat} / {shuffle}</div>
<div class="k">일시정지</div><div>{paused}</div>
<div class="k">볼륨</div><div>{volume}%</div>
<div class="k">음성 채널</div><div>{voice}</div>
<div class="k">최근 재생</div><div>{recent}</div>
</div></div>"#,
            name = html_escape(&name),
            autoplay_default = if gset.autoplay_default { "켜짐" } else { "꺼짐" },
            repeat = s.repeat_mode.as_str(),
            shuffle = if s.shuffle_enabled { "셔플 On" } else { "셔플 Off" },
            paused = if s.is_paused { "예" } else { "아니오" },
            volume = s.effective_volume,
            voice = s.voice_channel_id.map(|v| v.to_string()).unwrap_or_else(|| "미연결".into()),
        ));
    }
    if cards.is_empty() {
        cards = r#"<div class="card"><p class="kv">알려진 길드가 없습니다. 봇이 한 번이라도 재생/설정을 했어야 표시됩니다.</p></div>"#.into();
    }
    let body = format!(
        r#"<h1 class="page-title">진단 / 현재 상태</h1>
<p class="page-sub">길드별 재생·자동추천·큐 상태를 한눈에 봅니다.</p>
<div class="card"><h2>봇 프로세스</h2><p>상태: <span class="pill run">실행 중</span></p></div>
{cards}"#
    );
    layout(&state, "진단", "/diagnostics", &body).into_response()
}

// ───────── 재생 설정 ─────────

pub async fn settings_page(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let s = state.app.db.load_global_settings();
    let chk = |b: bool| if b { "checked" } else { "" };
    let pol = |p: EmptyVoiceChannelPolicy, v: EmptyVoiceChannelPolicy| {
        if p == v { "selected" } else { "" }
    };
    let body = format!(
        r#"<h1 class="page-title">재생 설정</h1><p class="page-sub">모든 서버가 기본으로 따르는 전역 재생 옵션입니다.</p>
<form method="post" action="/settings">
<div class="card"><h2>기본 재생</h2>
<label class="field">마스터 볼륨 (0–200)</label><input type="number" name="master_volume" min="0" max="200" value="{mv}"/>
<label class="checkbox"><input type="checkbox" name="normalize_enabled" {ne}/> 볼륨 평준화 켜기</label>
<label class="checkbox"><input type="checkbox" name="autoplay_default" {ad}/> 자동추천 기본값 켜기</label>
</div>
<div class="card"><h2>음성 채널 / 알림</h2>
<label class="checkbox"><input type="checkbox" name="auto_leave_when_empty" {al}/> 빈 음성 채널 감지 켜기</label>
<label class="field">빈 채널 대기 시간(초, 5–3600)</label><input type="number" name="auto_leave_delay_seconds" min="5" max="3600" value="{ald}"/>
<label class="field">빈 음성 채널 정책</label>
<select name="empty_voice_policy" title="자동 퇴장: 대기 시간 후 재생 중단 + 음성 채널 퇴장. 재생 중단: 대기 시간 후 재생만 멈추고 채널에는 그대로 머문다. 그대로 둠: 비어 있어도 계속 재생한다.">
<option value="AutoLeave" {p1} title="대기 시간이 지나면 재생을 중단하고 음성 채널에서 나간다.">자동 퇴장</option>
<option value="StopPlayback" {p2} title="대기 시간이 지나면 재생만 멈추고 음성 채널에는 그대로 남는다.">재생 중단</option>
<option value="DoNothing" {p3} title="채널이 비어도 아무 동작도 하지 않고 계속 재생한다.">그대로 둠</option>
</select>
<label class="checkbox"><input type="checkbox" name="announce_now_playing" {an}/> 곡 시작 시 '현재 재생 중' 알림 보내기</label>
</div>
<div class="card"><h2>캐시 / 로그 / 소싱</h2>
<label class="field">캐시 한도(GB, 1–4096)</label><input type="number" name="cache_limit_gb" min="1" max="4096" value="{cl}"/>
<label class="field">로그 보관 일수 (1–3650)</label><input type="number" name="log_retention_days" min="1" max="3650" value="{lr}"/>
<label class="field">선호 브라우저 프로필 (쿠키 추출)</label><input type="text" name="preferred_browser_profile" value="{bp}"/>
<label class="field">쿠키 파일 경로 (선택)</label><input type="text" name="cookie_file_path" value="{cf}"/>
<label class="checkbox" title="yt-dlp --sponsorblock-remove music_offtopic,intro,outro — SponsorBlock 데이터가 있는 영상의 인트로/아웃트로/비음악 구간을 다운로드 시 잘라냅니다. 이미 캐시된 곡엔 적용 안 되고 새로 받는 곡부터 적용됩니다."><input type="checkbox" name="sponsorblock_remove" {sb}/> 인트로/아웃트로 제거 (SponsorBlock · 새로 받는 곡부터)</label>
<label class="checkbox" title="봇이 받은(tools 폴더 안의) yt-dlp 를 하루 1회 자동으로 yt-dlp -U 합니다. YouTube 변경으로 다운로드가 깨지는 것을 예방합니다. 시스템/PATH 의 yt-dlp 는 건드리지 않습니다."><input type="checkbox" name="auto_update_tools" {au}/> yt-dlp 자동 업데이트 (하루 1회)</label>
</div>
<div class="card"><h2>끊김 최적화 (실험)</h2>
<p class="sub">
기본은 모두 꺼짐(검증된 보수 경로). 하나씩 켜고 디스코드에서 실제로 들어보며 검증하세요.
저장하면 <b>다음 곡부터</b> 반영됩니다.
</p>
<label class="checkbox" title="ffmpeg -probesize 32k -analyzeduration 0 -fflags +nobuffer — 곡 시작 지연 단축"><input type="checkbox" name="tweak_ffmpeg_fast_start" {t1}/> ① ffmpeg 빠른 시작 (probe/analyze 생략)</label>
<label class="checkbox" title="ffmpeg -avioflags direct -flush_packets 1 — 파이프 즉시 flush"><input type="checkbox" name="tweak_ffmpeg_direct_output" {t2}/> ② ffmpeg 즉시 출력 (pipe flush)</label>
<p class="kv">③ 작은 송신 버퍼 · ④ 낮은 패킷로스 힌트 · ⑤ 전용 송출 스레드 — songbird 엔진은 전용 스레드 페이싱이 기본이라 항상 적용된 것과 같아 토글이 없습니다.</p>
<label class="field">송출 비트레이트 (kbps, 32–128 · 기본 96)</label><input type="number" name="voice_bitrate_kbps" min="32" max="128" value="{br}"/>
</div>
<div class="actions"><button class="btn btn-primary" type="submit">재생 설정 저장</button></div>
</form>"#,
        mv = s.master_volume,
        ne = chk(s.normalize_enabled),
        ad = chk(s.autoplay_default),
        an = chk(s.announce_now_playing),
        al = chk(s.auto_leave_when_empty),
        ald = s.auto_leave_delay_seconds,
        p1 = pol(s.empty_voice_policy, EmptyVoiceChannelPolicy::AutoLeave),
        p2 = pol(s.empty_voice_policy, EmptyVoiceChannelPolicy::StopPlayback),
        p3 = pol(s.empty_voice_policy, EmptyVoiceChannelPolicy::DoNothing),
        cl = s.cache_limit_gb,
        lr = s.log_retention_days,
        bp = html_escape(&s.preferred_browser_profile),
        cf = html_escape(s.cookie_file_path.as_deref().unwrap_or("")),
        sb = chk(s.sponsorblock_remove),
        au = chk(s.auto_update_tools),
        t1 = chk(s.tweak_ffmpeg_fast_start),
        t2 = chk(s.tweak_ffmpeg_direct_output),
        br = s.voice_bitrate_kbps,
    );
    layout(&state, "재생 설정", "/settings", &body).into_response()
}

#[derive(Deserialize, Default)]
pub struct SettingsForm {
    master_volume: Option<i32>,
    normalize_enabled: Option<String>,
    autoplay_default: Option<String>,
    announce_now_playing: Option<String>,
    auto_leave_when_empty: Option<String>,
    auto_leave_delay_seconds: Option<i32>,
    empty_voice_policy: Option<String>,
    cache_limit_gb: Option<i32>,
    log_retention_days: Option<i32>,
    preferred_browser_profile: Option<String>,
    cookie_file_path: Option<String>,
    sponsorblock_remove: Option<String>,
    auto_update_tools: Option<String>,
    tweak_ffmpeg_fast_start: Option<String>,
    tweak_ffmpeg_direct_output: Option<String>,
    voice_bitrate_kbps: Option<i32>,
}

pub async fn settings_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<SettingsForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let mut s = state.app.db.load_global_settings();
    s.master_volume = f.master_volume.unwrap_or(s.master_volume).clamp(0, 200);
    s.normalize_enabled = f.normalize_enabled.is_some();
    s.autoplay_default = f.autoplay_default.is_some();
    s.announce_now_playing = f.announce_now_playing.is_some();
    s.auto_leave_when_empty = f.auto_leave_when_empty.is_some();
    s.auto_leave_delay_seconds = f
        .auto_leave_delay_seconds
        .unwrap_or(s.auto_leave_delay_seconds)
        .clamp(5, 3600);
    s.empty_voice_policy = match f.empty_voice_policy.as_deref() {
        Some("StopPlayback") => EmptyVoiceChannelPolicy::StopPlayback,
        Some("DoNothing") => EmptyVoiceChannelPolicy::DoNothing,
        _ => EmptyVoiceChannelPolicy::AutoLeave,
    };
    s.cache_limit_gb = f.cache_limit_gb.unwrap_or(s.cache_limit_gb).clamp(1, 4096);
    s.log_retention_days = f
        .log_retention_days
        .unwrap_or(s.log_retention_days)
        .clamp(1, 3650);
    s.preferred_browser_profile = f
        .preferred_browser_profile
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "Default".into());
    s.cookie_file_path = f.cookie_file_path.filter(|v| !v.trim().is_empty());
    s.sponsorblock_remove = f.sponsorblock_remove.is_some();
    s.auto_update_tools = f.auto_update_tools.is_some();
    s.tweak_ffmpeg_fast_start = f.tweak_ffmpeg_fast_start.is_some();
    s.tweak_ffmpeg_direct_output = f.tweak_ffmpeg_direct_output.is_some();
    s.voice_bitrate_kbps = f
        .voice_bitrate_kbps
        .unwrap_or(s.voice_bitrate_kbps)
        .clamp(32, 128);
    state.app.db.save_global_settings(&s);
    // 마스터 볼륨/평준화 변경을 재생 중인 길드에 즉시 반영 (per-guild override 는 그대로 존중).
    for gid in state.app.coordinator.active_guild_ids().await {
        let st = state.app.player.apply_configured_settings(gid).await;
        state.app.coordinator.apply_volume(gid, st.effective_volume).await;
    }
    state
        .app
        .log
        .info("Web", "재생 설정 저장됨 (볼륨은 즉시, 그 외는 다음 곡부터 반영).");
    redirect_flash(
        "/settings",
        "재생 설정이 저장되었습니다. (볼륨은 바로, 그 외 설정은 다음 곡부터 반영됩니다.)",
        false,
    )
}

// ───────── 봇 설정 / 공용 설정 (읽기 전용 정보) ─────────

pub async fn botsettings_page(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let auth = state.remote_auth.read().unwrap().clone();
    let csrf = admin_csrf_token(&state, &cookies).unwrap_or_default();
    let oauth_status = if auth.configured() {
        r#"<span class="pill run">설정 완료</span>"#
    } else {
        r#"<span class="pill stop">설정 필요</span>"#
    };
    let secret_status = if auth.has_client_secret() {
        "설정됨 — 값은 다시 표시하지 않습니다."
    } else {
        "미설정"
    };
    let meta = app.db.list_guild_metadata();
    let known: String = meta
        .iter()
        .map(|m| format!("{}({})", html_escape(&m.name), m.guild_id))
        .collect::<Vec<_>>()
        .join(", ");
    let known_html = if known.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="kv">알려진 서버: {known}</p>"#)
    };
    let body = format!(
        r#"<h1 class="page-title">봇 설정</h1>
<p class="page-sub">봇 본체가 읽는 botsettings.json입니다. 읽기 전용 — 파일을 수정한 뒤 재시작하면 반영됩니다.</p>
<div class="card"><h2>토큰 / 명령 등록</h2>
<p class="kv">파일: {cfg}</p>
<div class="diag-grid">
<div class="k">Discord 봇 토큰</div><div>●●●●●● (설정됨 — 파일에서만 변경 가능)</div>
<div class="k">명령 등록 길드 ID</div><div>{reg}</div>
</div>
{known_html}
</div>
<div class="card"><h2>마참뮤직 Discord OAuth {oauth_status}</h2>
<p class="sub">사용자 리모컨 로그인 설정입니다. 저장 즉시 반영되며 봇 재시작이 필요 없습니다. Secret은 저장 후 다시 표시하지 않습니다.</p>
<form method="post" action="/botsettings/oauth" autocomplete="off">
<input type="hidden" name="csrf_token" value="{csrf}"/>
<label class="field" for="oauth-client-id">Discord Client ID</label>
<input id="oauth-client-id" type="text" name="client_id" inputmode="numeric" pattern="[0-9]+" required value="{client_id}"/>
<label class="field" for="oauth-client-secret">Discord Client Secret</label>
<input id="oauth-client-secret" type="password" name="client_secret" autocomplete="new-password" placeholder="변경할 때만 새 Secret 입력"/>
<p class="kv">현재 상태: {secret_status}</p>
<label class="field" for="oauth-public-url">공개 기본 URL</label>
<input id="oauth-public-url" type="text" name="public_base_url" required value="{public_base_url}"/>
<p class="kv">Discord Redirect URI: <code>{redirect_uri}</code></p>
<p class="kv">요청 스코프: <code>identify guilds guilds.members.read</code></p>
<p class="kv">저장 파일: <code>{oauth_path}</code> (Git/NAS 패키지 제외)</p>
<label class="field" for="oauth-owner-ids">봇 주인 Discord 유저 ID (쉼표 구분)</label>
<input id="oauth-owner-ids" type="text" name="owner_user_ids" inputmode="numeric" placeholder="예: 1234567890, 9876543210" value="{owner_user_ids}"/>
<p class="kv">여기 등록된 사람은 리모컨에서 <strong>봇 주인</strong> 등급이 되어 배지·전용 컨트롤·운영 패널 링크를 받습니다. 저장 즉시 반영됩니다.</p>
<label class="checkbox"><input type="checkbox" name="clear_secret"/> 저장된 Client Secret 제거</label>
<div class="actions"><button class="btn btn-primary" type="submit">OAuth 설정 저장</button>
<a class="btn btn-secondary" href="/music/login" target="_blank" rel="noopener">로그인 화면 열기</a></div>
</form></div>
<div class="card"><h2>전용 override (선택)</h2>
<p class="sub">비워 두면 공용 설정을 그대로 따릅니다. 현재 적용 값:</p>
<div class="diag-grid">
<div class="k">Bot owner ID</div><div>{owner}</div>
<div class="k">데이터 루트</div><div>{data_root}</div>
<div class="k">도구 루트</div><div>{tools_root}</div>
<div class="k">yt-dlp 경로</div><div>{ytdlp}</div>
<div class="k">ffmpeg 경로</div><div>{ffmpeg}</div>
</div>
</div>"#,
        cfg = html_escape(
            &app.config
                .config_dir
                .join("botsettings.json")
                .to_string_lossy()
        ),
        reg = app
            .config
            .register_guild_id
            .map(|g| g.to_string())
            .unwrap_or_else(|| "(비어 있음 — 전역 등록)".into()),
        owner = app.config.bot_owner_user_id,
        data_root = html_escape(&app.config.data_root.to_string_lossy()),
        tools_root = html_escape(&app.config.tools_root.to_string_lossy()),
        ytdlp = html_escape(&app.config.yt_dlp_path),
        ffmpeg = html_escape(&app.config.ffmpeg_path),
        oauth_status = oauth_status,
        csrf = html_escape(&csrf),
        client_id = html_escape(auth.client_id.as_deref().unwrap_or("")),
        secret_status = secret_status,
        public_base_url = html_escape(&auth.public_base_url),
        owner_user_ids = html_escape(
            &auth
                .owner_user_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        redirect_uri = html_escape(&auth.redirect_uri()),
        oauth_path = html_escape(
            &crate::web::remote::RemoteAuthConfig::storage_path(&app.config.data_root)
                .to_string_lossy()
        ),
    );
    layout(&state, "봇 설정", "/botsettings", &body).into_response()
}

#[derive(Deserialize, Default)]
pub struct OAuthSettingsForm {
    csrf_token: String,
    client_id: String,
    client_secret: Option<String>,
    public_base_url: String,
    /// 봇 주인 Discord 유저 ID — 쉼표 구분. 리모컨의 `AccessTier::Owner` 판정 근거다.
    owner_user_ids: Option<String>,
    clear_secret: Option<String>,
}

pub async fn botsettings_oauth_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(form): Form<OAuthSettingsForm>,
) -> Response {
    if let Some(response) = require_auth(&state, &cookies) {
        return response;
    }
    if !verify_admin_csrf(&state, &cookies, &form.csrf_token) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "CSRF 검증에 실패했습니다.",
        )
            .into_response();
    }

    let client_id = form.client_id.trim().to_string();
    if client_id
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .is_none()
    {
        return redirect_flash(
            "/botsettings",
            "Discord Client ID는 0이 아닌 숫자여야 합니다.",
            true,
        );
    }

    let public_base_url = form
        .public_base_url
        .trim()
        .trim_end_matches('/')
        .to_string();
    let parsed = match reqwest::Url::parse(&public_base_url) {
        Ok(url) => url,
        Err(_) => {
            return redirect_flash(
                "/botsettings",
                "공개 기본 URL 형식이 올바르지 않습니다.",
                true,
            );
        }
    };
    let loopback_http = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if (parsed.scheme() != "https" && !loopback_http)
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return redirect_flash(
            "/botsettings",
            "공개 기본 URL은 경로·쿼리 없는 HTTPS 주소여야 합니다. localhost만 HTTP를 허용합니다.",
            true,
        );
    }

    let secret_update = form
        .client_secret
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let owner_user_ids =
        crate::web::remote::parse_owner_ids(form.owner_user_ids.as_deref().unwrap_or(""));
    if owner_user_ids.len() > 20 {
        return redirect_flash(
            "/botsettings",
            "봇 주인 ID는 최대 20개까지 등록할 수 있습니다.",
            true,
        );
    }
    let current = state.remote_auth.read().unwrap().clone();
    let next = current
        .updated(
            client_id,
            secret_update,
            form.clear_secret.is_some(),
            public_base_url,
        )
        .with_owner_user_ids(owner_user_ids.clone());
    if let Err(error) = next.save(&state.app.config.data_root) {
        state
            .app
            .log
            .error("Web", &format!("OAuth 설정 저장 실패: {error}"));
        return redirect_flash("/botsettings", &error, true);
    }
    let configured = next.configured();
    let public_base_url = next.public_base_url.clone();
    *state.remote_auth.write().unwrap() = next;
    // 봇 주인 판정은 App이 들고 있다 — 프로세스 재시작 없이 즉시 갱신한다.
    if let Ok(mut owners) = state.app.owner_user_ids.write() {
        *owners = owner_user_ids;
    }
    let _ = state.app.public_base_url.set(public_base_url);
    state.oauth_states.lock().unwrap().clear();
    state.remote_sessions.lock().unwrap().clear();
    state.app.log.info(
        "Web",
        if configured {
            "운영자 UI에서 Discord OAuth 설정 저장됨."
        } else {
            "운영자 UI에서 Discord OAuth Secret 제거됨."
        },
    );
    redirect_flash(
        "/botsettings",
        if configured {
            "Discord OAuth 설정이 저장되어 즉시 반영되었습니다."
        } else {
            "OAuth 설정은 저장됐지만 Client Secret이 없어 사용자 로그인이 비활성화됩니다."
        },
        !configured,
    )
}

pub async fn sharedconfig_page(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let body = format!(
        r#"<h1 class="page-title">공용 설정</h1>
<p class="page-sub">Bot · CLI · 웹/관리자가 공통으로 읽는 musicbot.runtime.json입니다. 읽기 전용 — 파일을 수정한 뒤 재시작하면 반영됩니다.</p>
<div class="card"><h2>경로 / 소유자</h2>
<p class="kv">파일: {file}</p>
<div class="diag-grid">
<div class="k">전역 플레이리스트 소유자 ID</div><div>{owner}</div>
<div class="k">데이터 루트</div><div>{data_root}</div>
<div class="k">캐시 디렉터리</div><div>{cache_dir}</div>
<div class="k">로그 디렉터리</div><div>{logs_dir}</div>
<div class="k">도구 루트</div><div>{tools_root}</div>
<div class="k">yt-dlp 경로</div><div>{ytdlp}</div>
<div class="k">ffmpeg 경로</div><div>{ffmpeg}</div>
</div>
</div>"#,
        file = html_escape(
            &app.config
                .config_dir
                .join("musicbot.runtime.json")
                .to_string_lossy()
        ),
        owner = app.config.bot_owner_user_id,
        data_root = html_escape(&app.config.data_root.to_string_lossy()),
        cache_dir = html_escape(&app.config.cache_dir().to_string_lossy()),
        logs_dir = html_escape(&app.config.logs_dir().to_string_lossy()),
        tools_root = html_escape(&app.config.tools_root.to_string_lossy()),
        ytdlp = html_escape(&app.config.yt_dlp_path),
        ffmpeg = html_escape(&app.config.ffmpeg_path),
    );
    layout(&state, "공용 설정", "/sharedconfig", &body).into_response()
}

// ───────── 서버(길드) 설정 ─────────

#[derive(Deserialize, Default)]
pub struct GuildQuery {
    /// 빈 문자열 제출(직접 입력 폼)도 400 없이 받기 위해 String 으로 받고 직접 파싱.
    guild_id: Option<String>,
}

impl GuildQuery {
    fn parsed(&self) -> Option<u64> {
        self.guild_id
            .as_deref()
            .and_then(|s| s.trim().parse::<u64>().ok())
    }
}

pub async fn guilds_page(
    State(state): Ctx,
    cookies: Cookies,
    Query(q): Query<GuildQuery>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let meta: HashMap<u64, GuildMetadata> = app
        .db
        .list_guild_metadata()
        .into_iter()
        .map(|m| (m.guild_id, m))
        .collect();
    let known = app.db.list_known_guild_ids();
    let list: String = known
        .iter()
        .map(|gid| {
            let name = meta.get(gid).map(|m| m.name.clone()).unwrap_or_else(|| "(이름 미확보)".into());
            let icon = guild_icon_html(meta.get(gid));
            format!(
                r#"<li><a class="guild-row" href="/guilds?guild_id={gid}">{icon}<span class="guild-name">{}</span><span class="guild-id">{gid}</span></a></li>"#,
                html_escape(&name)
            )
        })
        .collect();
    let list = if list.is_empty() {
        r#"<p class="kv">(아직 없음)</p>"#.to_string()
    } else {
        format!(r#"<ul class="guild-list">{list}</ul>"#)
    };

    let editor = if let Some(gid) = q.parsed() {
        let gs = app.db.load_guild_settings(gid);
        let name = meta.get(&gid).map(|m| m.name.clone()).unwrap_or_default();
        let title = if name.is_empty() {
            format!("서버 {gid} 설정")
        } else {
            format!("{} 설정", html_escape(&name))
        };
        let tri = |v: Option<bool>| match v {
            None => ("selected", "", ""),
            Some(true) => ("", "selected", ""),
            Some(false) => ("", "", "selected"),
        };
        let (n0, n1, n2) = tri(gs.normalize_enabled_override);
        let (a0, a1, a2) = tri(gs.autoplay_default_override);
        format!(
            r#"<h2>{title}</h2>
<form method="post" action="/guilds"><input type="hidden" name="guild_id" value="{gid}"/>
<label class="field">볼륨 override (0–200, 비우면 전역)</label>
<input type="text" name="volume_override" value="{vol}"/>
<label class="field">볼륨 평준화 override</label>
<select name="normalize_override"><option value="" {n0}>전역 따름</option><option value="true" {n1}>켬</option><option value="false" {n2}>끔</option></select>
<label class="field">자동추천 override</label>
<select name="autoplay_override"><option value="" {a0}>전역 따름</option><option value="true" {a1}>켬</option><option value="false" {a2}>끔</option></select>
<div class="actions"><button class="btn btn-primary" type="submit">서버 설정 저장</button></div>
</form>"#,
            vol = gs
                .volume_override
                .map(|v| v.to_string())
                .unwrap_or_default(),
        )
    } else {
        r#"<h2>서버를 선택하세요</h2><p class="kv">왼쪽 목록에서 서버를 고르거나 길드 ID를 직접 입력하세요.</p>"#.to_string()
    };

    let body = format!(
        r#"<h1 class="page-title">서버 설정</h1>
<p class="page-sub">길드(서버)별 재생 설정 override입니다. 비워 두면 전역 설정을 따릅니다.</p>
<div class="grid2">
<div class="card">
<h2>알려진 서버</h2>
<p class="sub">봇이 본 적 있는 길드. 이름/아이콘은 봇이 Ready 시점에 갱신.</p>
{list}
<form method="get" action="/guilds" class="actions">
<input type="text" name="guild_id" placeholder="길드 ID 직접 입력" style="max-width:240px"/>
<button type="submit" class="btn btn-secondary">불러오기</button>
</form>
</div>
<div class="card">{editor}</div>
</div>"#
    );
    layout(&state, "서버 설정", "/guilds", &body).into_response()
}

#[derive(Deserialize)]
pub struct GuildForm {
    guild_id: u64,
    volume_override: Option<String>,
    normalize_override: Option<String>,
    autoplay_override: Option<String>,
}

pub async fn guilds_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<GuildForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let tri = |v: Option<&String>| match v.map(|s| s.as_str()) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let gs = GuildSettings {
        guild_id: f.guild_id,
        volume_override: f
            .volume_override
            .as_deref()
            .and_then(|v| v.trim().parse::<i32>().ok())
            .map(|v| v.clamp(0, 200)),
        normalize_enabled_override: tri(f.normalize_override.as_ref()),
        autoplay_default_override: tri(f.autoplay_override.as_ref()),
    };
    state.app.db.save_guild_settings(&gs);
    let st = state.app.player.apply_configured_settings(f.guild_id).await;
    // 재생 중이면 새 볼륨을 라이브 세션에도 즉시 반영 (디스코드 /볼륨 과 동일하게).
    state
        .app
        .coordinator
        .apply_volume(f.guild_id, st.effective_volume)
        .await;
    redirect_flash(
        &format!("/guilds?guild_id={}", f.guild_id),
        "서버 설정이 저장되었습니다.",
        false,
    )
}

// ───────── 도구 ─────────

pub async fn tools_page(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let yt_ver = tool_version(&app.config.yt_dlp_path, "--version").await;
    let ff_ver = tool_version(&app.config.ffmpeg_path, "-version").await;
    let yt_ok = !yt_ver.starts_with("실행 실패");
    let ff_ok = !ff_ver.starts_with("실행 실패");
    let pill = |ok: bool| {
        if ok {
            r#"<span class="pill run">사용 가능</span>"#
        } else {
            r#"<span class="pill stop">없음</span>"#
        }
    };
    let (cache_count, cache_bytes) = app.cache.stats();
    // C# Tools 와 동일 — 최근 접근 50개를 pre.log 로.
    let mut entries = app.db.all_cache_entries();
    entries.sort_by(|a, b| b.last_access_utc.cmp(&a.last_access_utc));
    let listing = if entries.is_empty() {
        r#"<p class="kv">(비어 있음)</p>"#.to_string()
    } else {
        let lines: Vec<String> = entries
            .iter()
            .take(50)
            .map(|e| {
                format!(
                    "{}  ·  {}  ·  ▶{}회  ·  {}",
                    e.cache_key,
                    format_bytes(e.size_bytes),
                    e.play_count,
                    e.title.as_deref().unwrap_or(&e.content_id)
                )
            })
            .collect();
        format!(
            r#"<pre class="log">{}</pre>"#,
            html_escape(&lines.join("\n"))
        )
    };
    let body = format!(
        r#"<h1 class="page-title">도구 / 캐시</h1>
<p class="page-sub">외부 도구 상태와 로컬 오디오 캐시를 점검합니다.</p>
<div class="grid2">
<div class="card">
<h2>도구 상태</h2>
<p>yt-dlp: {yt_pill} <span class="kv">{yt_ver}</span></p>
<p class="kv">경로: {yt_path}</p>
<p>ffmpeg: {ff_pill} <span class="kv">{ff_ver}</span></p>
<p class="kv">경로: {ff_path}</p>
<h2 style="margin-top:18px">링크 검사</h2>
<form method="post" action="/tools">
<input type="text" name="link_url" placeholder="https://www.youtube.com/watch?v=..."/>
<div class="actions"><button type="submit" class="btn btn-secondary">검사</button></div>
</form>
</div>
<div class="card">
<h2>캐시</h2>
<p class="kv">항목 {cache_count}개 · 총 {cache_size}</p>
<form method="post" action="/tools/prune">
<div class="actions"><button type="submit" class="btn btn-secondary">캐시 한도까지 정리</button></div>
</form>
<div class="actions"><a class="btn btn-secondary" href="/cache">캐시 라이브러리 열기</a></div>
</div>
</div>
<div class="card">
<h2>캐시 항목 (최근 접근 50개)</h2>
{listing}
</div>"#,
        yt_pill = pill(yt_ok),
        ff_pill = pill(ff_ok),
        yt_ver = html_escape(&yt_ver),
        ff_ver = html_escape(&ff_ver),
        yt_path = html_escape(&app.config.yt_dlp_path),
        ff_path = html_escape(&app.config.ffmpeg_path),
        cache_size = format_bytes(cache_bytes),
    );
    layout(&state, "도구 / 캐시", "/tools", &body).into_response()
}

async fn tool_version(exe: &str, arg: &str) -> String {
    match tokio::process::Command::new(exe).arg(arg).output().await {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("(출력 없음)")
            .to_string(),
        Err(e) => format!("실행 실패: {e}"),
    }
}

#[derive(Deserialize, Default)]
pub struct ToolsLinkForm {
    link_url: Option<String>,
}

/// C# Tools 의 링크 검사 — URL 이 어떤 공급자/곡/컬렉션으로 해석되는지 확인.
pub async fn tools_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<ToolsLinkForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let url = f.link_url.unwrap_or_default().trim().to_string();
    let (msg, is_err) = if url.is_empty() {
        ("URL을 입력하세요.".to_string(), true)
    } else if !crate::media::resolver::can_resolve(&url) {
        (
            format!("'{url}' 은(는) 지원 URL이 아닙니다. /play 에서는 키워드 검색으로 처리됩니다."),
            true,
        )
    } else {
        match crate::media::resolver::resolve(&url) {
            Ok(crate::media::resolver::Resolved::Track(t)) => (
                format!(
                    "단일 곡으로 해석됨: {} / {}",
                    t.provider.label(),
                    t.content_id
                ),
                false,
            ),
            Ok(crate::media::resolver::Resolved::Collection(c)) => (
                format!(
                    "컬렉션으로 해석됨: {} / {}",
                    c.provider.label(),
                    c.collection_id
                ),
                false,
            ),
            Err(e) => (format!("해석 실패: {e}"), true),
        }
    };
    redirect_flash("/tools", &msg, is_err)
}

/// C# Tools 의 '캐시 한도까지 정리' — 전역 설정 한도 기준 prune.
pub async fn tools_prune(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let s = state.app.db.load_global_settings();
    let limit_bytes = (s.cache_limit_gb as i64) * 1024 * 1024 * 1024;
    state.app.cache.prune_to_limit(limit_bytes);
    state.app.log.info("Cache", "웹에서 캐시 한도 정리 실행.");
    redirect_flash("/tools", "캐시 한도까지 정리했습니다.", false)
}

// ───────── 캐시 라이브러리 ─────────

#[derive(Deserialize, Default)]
pub struct CacheQuery {
    q: Option<String>,
    provider: Option<String>,
    ext: Option<String>,
    sort: Option<String>,
}

/// 캐시 파일 확장자 분류 — mp3 / opus / other.
fn cache_ext_class(file_path: &str) -> &'static str {
    let ext = std::path::Path::new(file_path)
        .extension()
        .map(|x| x.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp3" => "mp3",
        "opus" => "opus",
        _ => "other",
    }
}

/// 쿼리 문자열 값 percent-encoding (한글 검색어 등 — RFC 3986 unreserved 만 통과).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 캐시 페이지 전용 스타일 — 칩 필터 · 카드 그리드 · 선택 모드 (layout 공통 CSS 위에 얹음).
const CACHE_PAGE_CSS: &str = r#"<style>
.cstats{display:flex;gap:28px;flex-wrap:wrap}
.cstat-num{font-size:22px;font-weight:700}.cstat-label{font-size:12px;color:#64748B}
.chips{display:flex;gap:6px;flex-wrap:wrap;align-items:center;margin:10px 0 0}
.chiplabel{font-size:12px;color:#64748B;font-weight:600;min-width:34px}
.chip{display:inline-block;padding:4px 12px;border-radius:999px;border:1px solid #E2E8F0;background:#fff;color:#0F172A;font-size:12px;text-decoration:none}
.chip.on{background:#7C3AED;border-color:#7C3AED;color:#fff;font-weight:600}
.cgrid{display:grid;grid-template-columns:repeat(auto-fill,minmax(175px,1fr));gap:12px}
.ccard{position:relative;background:#fff;border:1px solid #E2E8F0;border-radius:12px;overflow:hidden;display:flex;flex-direction:column}
.cthumb{width:100%;aspect-ratio:16/9;object-fit:cover;display:block;background:#E2E8F0;border:none}
.cph{display:flex;align-items:center;justify-content:center;font-size:30px;color:#fff;background:linear-gradient(135deg,#7C3AED,#4F46E5)}
.cph-sc{background:linear-gradient(135deg,#F97316,#EA580C)}
.cbody{padding:8px 10px 10px;display:flex;flex-direction:column;gap:6px;flex:1}
.ctitle{font-size:13px;font-weight:600;line-height:1.35;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden;min-height:35px;word-break:break-all}
.cmeta{font-size:11px;color:#64748B}
.cmeta2{color:#94A3B8;display:flex;align-items:center;gap:4px}
.cplays{position:absolute;top:6px;right:6px;z-index:2;background:rgba(11,18,32,.82);color:#fff;font-size:11px;font-weight:700;padding:2px 8px;border-radius:999px;backdrop-filter:blur(2px)}
.cbtns{display:flex;gap:4px;flex-wrap:wrap;margin-top:auto;align-items:center}
.cbtn{padding:3px 8px;font-size:11px}
.plform{display:flex;gap:4px;align-items:center}
.plform select{font-size:11px;padding:2px;max-width:90px}
.cselbox{position:absolute;top:6px;left:6px;z-index:2;display:none;background:rgba(255,255,255,.92);border-radius:6px;padding:3px 5px;cursor:pointer}
.cselbox input{transform:scale(1.25);cursor:pointer}
body.selmode .cselbox{display:block}
body.selmode .ccard:has(.csel:checked){outline:3px solid #7C3AED}
.selbar{display:none;position:sticky;bottom:12px;background:#0B1220;color:#fff;border-radius:12px;padding:10px 16px;gap:12px;align-items:center;box-shadow:0 6px 20px rgba(0,0,0,.25);z-index:5}
body.selmode .selbar{display:flex}
</style>"#;

/// 캐시 페이지 전용 스크립트 — 선택 모드 토글 + 일괄 삭제 제출.
const CACHE_PAGE_JS: &str = r#"<script>
function toggleSel(){document.body.classList.toggle('selmode');selChanged();}
function selChanged(){document.getElementById('selcount').textContent=document.querySelectorAll('.csel:checked').length;}
function selAll(v){document.querySelectorAll('.csel').forEach(c=>c.checked=v);selChanged();}
function bulkSubmit(){
  const k=[...document.querySelectorAll('.csel:checked')].map(c=>c.value);
  if(k.length===0){alert('선택된 곡이 없습니다.');return false;}
  if(!confirm(k.length+'곡을 캐시에서 삭제합니다. 진행할까요?'))return false;
  document.getElementById('bulkkeys').value=k.join(',');
  return true;
}
</script>"#;

pub async fn cache_page(
    State(state): Ctx,
    cookies: Cookies,
    Query(cq): Query<CacheQuery>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let (total, mp3, opus, other, saved_mb) = app.cache.inspect_formats();
    let q_raw = cq.q.clone().unwrap_or_default();
    let needle = q_raw.to_lowercase();
    // 필터/정렬 파라미터는 허용값으로 정규화 (escape 불필요해짐).
    let provider_f = match cq.provider.as_deref() {
        Some(p @ ("YouTube" | "YouTubeMusic" | "SoundCloud")) => p.to_string(),
        _ => String::new(),
    };
    let ext_f = match cq.ext.as_deref() {
        Some(e @ ("mp3" | "opus" | "other")) => e.to_string(),
        _ => String::new(),
    };
    let sort = match cq.sort.as_deref() {
        Some(s @ ("title" | "size" | "duration" | "plays" | "played")) => s.to_string(),
        _ => "recent".to_string(),
    };

    let mut entries = app.db.all_cache_entries();
    let total_bytes: i64 = entries.iter().map(|e| e.size_bytes).sum();
    let total_plays: i64 = entries.iter().map(|e| e.play_count).sum();
    // 서버(길드)별 통계 툴팁에 쓸 이름 맵.
    let guild_names: std::collections::HashMap<u64, String> = app
        .db
        .list_guild_metadata()
        .into_iter()
        .map(|m| (m.guild_id, m.name))
        .collect();
    let total_size = if total_bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", total_bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        format!("{:.1} MB", total_bytes as f64 / 1024.0 / 1024.0)
    };

    entries.retain(|e| {
        (needle.is_empty()
            || e.title
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&needle)
            || e.content_id.to_lowercase().contains(&needle))
            && (provider_f.is_empty() || e.provider.as_str() == provider_f)
            && (ext_f.is_empty() || cache_ext_class(&e.file_path) == ext_f)
    });
    match sort.as_str() {
        "title" => {
            entries.sort_by_key(|e| e.title.as_deref().unwrap_or(&e.content_id).to_lowercase())
        }
        "size" => entries.sort_by_key(|e| std::cmp::Reverse(e.size_bytes)),
        "duration" => entries
            .sort_by_key(|e| std::cmp::Reverse(e.duration.map(|d| d.0.as_secs()).unwrap_or(0))),
        "plays" => entries.sort_by_key(|e| std::cmp::Reverse(e.play_count)),
        // 마지막 재생 시각 내림차순 — Option 은 None < Some 이라 미재생 곡이 뒤로 간다.
        "played" => entries.sort_by(|a, b| b.last_played_utc.cmp(&a.last_played_utc)),
        _ => entries.sort_by(|a, b| b.last_access_utc.cmp(&a.last_access_utc)), // recent — ISO 문자열 내림차순
    }
    let filtered = entries.len();
    const CARD_CAP: usize = 500;

    // 필터 칩 URL — 다른 파라미터 유지.
    let mk_url = |prov: &str, ext: &str, sort_v: &str| -> String {
        let mut parts: Vec<String> = Vec::new();
        if !q_raw.is_empty() {
            parts.push(format!("q={}", url_encode(&q_raw)));
        }
        if !prov.is_empty() {
            parts.push(format!("provider={prov}"));
        }
        if !ext.is_empty() {
            parts.push(format!("ext={ext}"));
        }
        if sort_v != "recent" {
            parts.push(format!("sort={sort_v}"));
        }
        if parts.is_empty() {
            "/cache".to_string()
        } else {
            format!("/cache?{}", parts.join("&"))
        }
    };
    let chip = |label: &str, href: String, on: bool| {
        format!(
            r#"<a class="chip{}" href="{href}">{label}</a>"#,
            if on { " on" } else { "" }
        )
    };
    let provider_chips: String = [
        ("", "전체"),
        ("YouTube", "YouTube"),
        ("YouTubeMusic", "YTM"),
        ("SoundCloud", "SoundCloud"),
    ]
    .iter()
    .map(|(v, l)| chip(l, mk_url(v, &ext_f, &sort), provider_f == *v))
    .collect();
    let ext_chips: String = [
        ("", "전체"),
        ("mp3", ".mp3"),
        ("opus", ".opus"),
        ("other", "기타"),
    ]
    .iter()
    .map(|(v, l)| chip(l, mk_url(&provider_f, v, &sort), ext_f == *v))
    .collect();
    let sort_chips: String = [
        ("recent", "최근 사용"),
        ("played", "최근 재생"),
        ("plays", "재생 많은순"),
        ("title", "제목"),
        ("size", "크기"),
        ("duration", "길이"),
    ]
    .iter()
    .map(|(v, l)| chip(l, mk_url(&provider_f, &ext_f, v), sort == *v))
    .collect();
    // 검색 폼이 현재 필터/정렬을 잃지 않도록 hidden 으로 동반.
    let mut hidden_filters = String::new();
    if !provider_f.is_empty() {
        hidden_filters.push_str(&format!(
            r#"<input type="hidden" name="provider" value="{provider_f}"/>"#
        ));
    }
    if !ext_f.is_empty() {
        hidden_filters.push_str(&format!(
            r#"<input type="hidden" name="ext" value="{ext_f}"/>"#
        ));
    }
    if sort != "recent" {
        hidden_filters.push_str(&format!(
            r#"<input type="hidden" name="sort" value="{sort}"/>"#
        ));
    }

    // 플리 추가 드롭다운용 목록 (전역 + 모든 길드).
    let mut playlists = app.db.list_playlists(PlaylistScope::Global, None);
    for gid in app.db.list_known_guild_ids() {
        playlists.extend(app.db.list_playlists(PlaylistScope::Guild, Some(gid)));
    }
    let pl_options: String = playlists
        .iter()
        .map(|p| {
            format!(
                r#"<option value="{}">{}</option>"#,
                p.id,
                html_escape(&p.name)
            )
        })
        .collect();

    let cards: String = entries
        .iter()
        .take(CARD_CAP)
        .map(|e| {
            let title = html_escape(e.title.as_deref().unwrap_or(&e.content_id));
            let key = html_escape(&e.cache_key);
            let url = html_escape(&e.source_url);
            let size_mb = e.size_bytes as f64 / 1024.0 / 1024.0;
            let dur = e.duration.map(|d| d.display()).unwrap_or_else(|| "-".into());
            let ext = cache_ext_class(&e.file_path);
            let plays = e.play_count;
            let last_played = match &e.last_played_utc {
                Some(s) => html_escape(&s[..19.min(s.len())]),
                None => "재생 기록 없음".to_string(),
            };
            // 서버별 재생 횟수 — 재생 배지 툴팁에.
            let mut pg: Vec<String> = e
                .per_guild
                .iter()
                .map(|(gid, st)| {
                    let name = guild_names
                        .get(gid)
                        .cloned()
                        .unwrap_or_else(|| format!("서버 {gid}"));
                    format!("{name}: {}회", st.count)
                })
                .collect();
            pg.sort();
            let plays_tip = if pg.is_empty() {
                format!("총 {plays}회 재생")
            } else {
                html_escape(&pg.join(" / "))
            };
            let plays_badge = if plays > 0 {
                format!(r#"<span class="cplays" title="{plays_tip}">▶ {plays}</span>"#)
            } else {
                String::new()
            };
            let thumb = match e.provider {
                ProviderKind::YouTube | ProviderKind::YouTubeMusic => format!(
                    r#"<img class="cthumb" loading="lazy" alt="" src="https://i.ytimg.com/vi/{cid}/mqdefault.jpg" onerror="this.style.display='none';this.nextElementSibling.style.display='flex'"/><div class="cthumb cph" style="display:none">♪</div>"#,
                    cid = html_escape(&e.content_id),
                ),
                ProviderKind::SoundCloud => r#"<div class="cthumb cph cph-sc">☁</div>"#.to_string(),
            };
            let pl_form = if playlists.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<form method="post" action="/cache/addtoplaylist" class="plform"><input type="hidden" name="cache_key" value="{key}"/><select name="playlist_id">{pl_options}</select><button class="btn btn-secondary cbtn">플리 추가</button></form>"#
                )
            };
            format!(
                r#"<div class="ccard">
<label class="cselbox"><input type="checkbox" class="csel" value="{key}" onchange="selChanged()"/></label>
{plays_badge}
{thumb}
<div class="cbody">
<div class="ctitle" title="{title}">{title}</div>
<div class="cmeta" title="마지막 사용: {last}&#10;마지막 재생: {last_played}">{prov} · {size_mb:.1} MB · {dur} · .{ext}</div>
<div class="cmeta cmeta2">▶ {plays}회 · {last_played}</div>
<div class="cbtns">
<a class="btn btn-secondary cbtn" href="{url}" target="_blank" rel="noopener">원본 링크</a>
{pl_form}
<form method="post" action="/cache/delete" onsubmit="return confirm('이 곡을 캐시에서 삭제할까요?')"><input type="hidden" name="cache_key" value="{key}"/><button class="btn btn-danger cbtn">삭제</button></form>
</div></div></div>"#,
                prov = e.provider.label(),
                last = html_escape(&e.last_access_utc[..19.min(e.last_access_utc.len())]),
            )
        })
        .collect();
    let grid = if cards.is_empty() {
        r#"<p class="kv">조건에 맞는 곡이 없습니다.</p>"#.to_string()
    } else {
        format!(r#"<div class="cgrid">{cards}</div>"#)
    };
    let truncate_note = if filtered > CARD_CAP {
        format!(
            r#"<p class="kv">전체 {filtered}곡 중 {CARD_CAP}곡만 표시 중 — 검색/필터로 좁혀보세요.</p>"#
        )
    } else {
        String::new()
    };

    let body = format!(
        r#"{css}<h1 class="page-title">캐시 라이브러리</h1><p class="page-sub">받아둔 곡을 검색·필터·정렬하고 플레이리스트에 담거나 정리합니다.</p>
<div class="card"><div class="cstats">
<div><div class="cstat-num">{total}</div><div class="cstat-label">전체 곡</div></div>
<div><div class="cstat-num">{total_size}</div><div class="cstat-label">총 용량</div></div>
<div><div class="cstat-num">{total_plays}</div><div class="cstat-label">총 재생</div></div>
<div><div class="cstat-num">{mp3}</div><div class="cstat-label">MP3</div></div>
<div><div class="cstat-num">{opus}</div><div class="cstat-label">Opus</div></div>
<div><div class="cstat-num">{other}</div><div class="cstat-label">기타</div></div>
<div><div class="cstat-num">{filtered}</div><div class="cstat-label">현재 조건</div></div>
</div></div>
<div class="card"><div class="actions" style="margin-top:0">
<form method="get" action="/cache" style="display:flex;gap:8px">{hidden_filters}<input type="text" name="q" placeholder="제목 / ID 검색" value="{q}"/><button class="btn btn-secondary" type="submit">검색</button></form>
<button class="btn btn-secondary" type="button" onclick="toggleSel()">☑ 선택 모드</button>
<form method="post" action="/cache/migrate" onsubmit="return confirm('MP3 {mp3}개를 Opus 로 변환합니다 (~{saved_mb}MB 절약 예상). 진행할까요?')"><button class="btn btn-primary" type="submit" {mig_dis}>📦 MP3 → Opus 변환</button></form>
<form method="post" action="/cache/wipe" onsubmit="return confirm('캐시 전체를 비웁니다. 다음 재생부터 자동 재다운로드됩니다.')"><button class="btn btn-danger" type="submit">🗑 캐시 전체 비우기</button></form>
</div>
<div class="chips"><span class="chiplabel">출처</span>{provider_chips}</div>
<div class="chips"><span class="chiplabel">형식</span>{ext_chips}</div>
<div class="chips"><span class="chiplabel">정렬</span>{sort_chips}</div>
</div>
<div class="card">{truncate_note}{grid}</div>
<div class="selbar" id="selbar">
<span>선택됨 <b id="selcount">0</b>곡</span>
<form method="post" action="/cache/bulkdelete" onsubmit="return bulkSubmit()" style="display:inline"><input type="hidden" id="bulkkeys" name="cache_keys" value=""/><button class="btn btn-danger">선택 삭제</button></form>
<button class="btn btn-secondary" type="button" onclick="selAll(true)">전체 선택</button>
<button class="btn btn-secondary" type="button" onclick="selAll(false)">선택 해제</button>
<button class="btn btn-secondary" type="button" onclick="toggleSel()">닫기</button>
</div>{js}"#,
        css = CACHE_PAGE_CSS,
        js = CACHE_PAGE_JS,
        q = html_escape(&q_raw),
        mig_dis = if mp3 == 0 { "disabled" } else { "" },
    );
    layout(&state, "캐시", "/cache", &body).into_response()
}

#[derive(Deserialize)]
pub struct AddToPlaylistForm {
    cache_key: String,
    playlist_id: i64,
}

pub async fn cache_add_to_playlist(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<AddToPlaylistForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    if let Some(entry) = state.app.db.get_cache_entry(&f.cache_key) {
        let title = entry
            .title
            .clone()
            .unwrap_or_else(|| entry.content_id.clone());
        let track = TrackRef {
            provider: entry.provider,
            content_id: entry.content_id,
            source_url: entry.source_url,
            title: entry.title,
            artist: None,
            duration: entry.duration,
            variant_key: None,
        };
        state.app.db.add_playlist_entry(
            f.playlist_id,
            &PlaylistEntry {
                track: Some(track),
                collection: None,
                start_offset: Some(CsTimeSpan::zero()),
                extra: Default::default(),
            },
        );
        redirect_flash(
            "/cache",
            &format!("플레이리스트에 추가했습니다: {title}"),
            false,
        )
    } else {
        redirect_flash("/cache", "캐시 항목을 찾지 못했습니다.", true)
    }
}

pub async fn cache_wipe(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let (deleted, skipped) = state.app.cache.wipe_all();
    state.app.log.info(
        "Cache",
        &format!("캐시 비움: {deleted}곡 삭제, 잠금 {skipped}건."),
    );
    redirect_flash(
        "/cache",
        &format!("캐시를 비웠습니다: {deleted}곡 삭제, 잠금 {skipped}건."),
        false,
    )
}

pub async fn cache_migrate(State(state): Ctx, cookies: Cookies) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = state.app.clone();
    tokio::spawn(async move {
        let (ok, failed) = app.cache.migrate_mp3_to_opus(&app.config.ffmpeg_path).await;
        app.log.info(
            "Cache",
            &format!("MP3→Opus 변환 완료: 성공 {ok} · 실패 {failed}."),
        );
    });
    redirect_flash(
        "/cache",
        "MP3 → Opus 변환을 백그라운드에서 시작했습니다. 진행 상황은 로그 뷰어에서 확인하세요.",
        false,
    )
}

#[derive(Deserialize)]
pub struct CacheDeleteForm {
    cache_key: String,
}

pub async fn cache_delete(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<CacheDeleteForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    if let Some(entry) = state.app.db.get_cache_entry(&f.cache_key) {
        let _ = std::fs::remove_file(&entry.file_path);
    }
    state.app.db.delete_cache_entries(&[f.cache_key]);
    redirect_flash("/cache", "캐시에서 1곡을 삭제했습니다.", false)
}

#[derive(Deserialize)]
pub struct CacheBulkDeleteForm {
    /// 쉼표로 이어붙인 cache_key 목록 (선택 모드 JS 가 채움).
    cache_keys: String,
}

pub async fn cache_bulk_delete(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<CacheBulkDeleteForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let keys: Vec<String> = f
        .cache_keys
        .split(',')
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    if keys.is_empty() {
        return redirect_flash("/cache", "선택된 곡이 없습니다.", true);
    }
    // cache_delete 와 동일 로직: 파일 제거 후 DB 항목 삭제 (파일 잠금 등 실패는 무시 — DB 만 정리).
    for k in &keys {
        if let Some(entry) = state.app.db.get_cache_entry(k) {
            let _ = std::fs::remove_file(&entry.file_path);
        }
    }
    state.app.db.delete_cache_entries(&keys);
    state.app.log.info(
        "Cache",
        &format!("웹에서 캐시 일괄 삭제: {}곡.", keys.len()),
    );
    redirect_flash(
        "/cache",
        &format!("캐시에서 {}곡을 삭제했습니다.", keys.len()),
        false,
    )
}

// ───────── 플레이리스트 ─────────

pub async fn playlists_page(
    State(state): Ctx,
    cookies: Cookies,
    Query(q): Query<GuildQuery>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let global = app.db.list_playlists(PlaylistScope::Global, None);
    // 플레이리스트 행(pl-row): 이름 + ID/항목수 + 이름변경/삭제 폼 + 항목 미리보기.
    let render = |title: &str, lists: &[Playlist]| -> String {
        let items: String = lists
            .iter()
            .map(|p| {
                let entries: String = p
                    .entries
                    .iter()
                    .take(15)
                    .enumerate()
                    .map(|(i, e)| {
                        let t = e.track.as_ref().map(|t| t.display_title().to_string()).unwrap_or_else(|| "(컬렉션)".into());
                        format!(r#"<div class="kv">{}. {}</div>"#, i + 1, html_escape(&t))
                    })
                    .collect();
                let more = if p.entries.len() > 15 {
                    format!(r#"<div class="kv">… 외 {}곡</div>"#, p.entries.len() - 15)
                } else {
                    String::new()
                };
                format!(
                    r#"<div class="pl-row">
<strong>{name}</strong> <span class="kv">· ID {id} · 항목 {count}개</span>
<div class="actions" style="margin-top:6px">
<form method="post" action="/playlists" style="display:flex;gap:6px"><input type="hidden" name="action" value="rename"/><input type="hidden" name="playlist_id" value="{id}"/><input type="text" name="name" placeholder="새 이름" style="max-width:200px"/><button type="submit" class="btn btn-secondary">이름변경</button></form>
<form method="post" action="/playlists" onsubmit="return confirm('삭제할까요?')"><input type="hidden" name="action" value="delete"/><input type="hidden" name="playlist_id" value="{id}"/><button type="submit" class="btn btn-danger">삭제</button></form>
</div>
{entries}{more}</div>"#,
                    name = html_escape(&p.name),
                    id = p.id,
                    count = p.entries.len(),
                )
            })
            .collect();
        format!(
            r#"<div class="card"><h2>{title}</h2>{}</div>"#,
            if items.is_empty() {
                r#"<p class="kv">(없음)</p>"#.to_string()
            } else {
                items
            }
        )
    };
    let guild_options: String = app
        .db
        .list_known_guild_ids()
        .iter()
        .map(|gid| {
            let name = meta_name(app, *gid);
            format!(
                r#"<option value="{gid}">{} ({gid})</option>"#,
                html_escape(&name)
            )
        })
        .collect();
    let mut body = format!(
        r#"<h1 class="page-title">플레이리스트</h1>
<p class="page-sub">전역/길드 플레이리스트를 생성·이름변경·삭제합니다. 디스코드 /플레이리스트 명령과 동일 데이터.</p>
<div class="card"><h2>새 플레이리스트</h2>
<form method="post" action="/playlists"><input type="hidden" name="action" value="create"/>
<label class="field">범위</label><select name="scope_guild"><option value="">전역</option>{guild_options}</select>
<label class="field">이름</label><input type="text" name="name" required/>
<div class="actions"><button class="btn btn-primary" type="submit">생성</button></div></form></div>
<div class="card"><h2>곡 추가 (URL / 검색어)</h2>
<form method="post" action="/playlists"><input type="hidden" name="action" value="addtrack"/>
<label class="field">대상 플레이리스트 ID</label><input type="number" name="playlist_id" required/>
<label class="field">곡 URL 또는 검색어</label><input type="text" name="input" required/>
<div class="actions"><button class="btn btn-primary" type="submit">추가</button></div></form></div>
{}"#,
        render("전역 플레이리스트", &global)
    );
    let meta: HashMap<u64, GuildMetadata> = app
        .db
        .list_guild_metadata()
        .into_iter()
        .map(|m| (m.guild_id, m))
        .collect();
    for gid in app.db.list_known_guild_ids() {
        let lists = app.db.list_playlists(PlaylistScope::Guild, Some(gid));
        if !lists.is_empty() || q.parsed() == Some(gid) {
            let name = meta
                .get(&gid)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| format!("길드 {gid}"));
            body.push_str(&render(
                &format!("{} 플레이리스트", html_escape(&name)),
                &lists,
            ));
        }
    }
    layout(&state, "플레이리스트", "/playlists", &body).into_response()
}

fn meta_name(app: &crate::app::App, gid: u64) -> String {
    app.db
        .list_guild_metadata()
        .into_iter()
        .find(|m| m.guild_id == gid)
        .map(|m| m.name)
        .unwrap_or_else(|| format!("길드 {gid}"))
}

#[derive(Deserialize)]
pub struct PlaylistForm {
    action: String,
    playlist_id: Option<i64>,
    name: Option<String>,
    scope_guild: Option<String>,
    input: Option<String>,
}

pub async fn playlists_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<PlaylistForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let (msg, is_err) = match f.action.as_str() {
        "delete" => match f.playlist_id {
            Some(id) => {
                state.app.db.delete_playlist(id);
                ("플레이리스트를 삭제했습니다.".to_string(), false)
            }
            None => ("플레이리스트 ID가 없습니다.".to_string(), true),
        },
        "create" => match f.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => {
                let gid = f.scope_guild.as_deref().and_then(|s| s.parse::<u64>().ok());
                let scope = if gid.is_some() {
                    PlaylistScope::Guild
                } else {
                    PlaylistScope::Global
                };
                state.app.db.create_playlist(scope, gid, 0, name);
                (format!("플레이리스트 '{name}' 을(를) 생성했습니다."), false)
            }
            None => ("이름을 입력하세요.".to_string(), true),
        },
        "rename" => match (
            f.playlist_id,
            f.name.as_deref().map(str::trim).filter(|n| !n.is_empty()),
        ) {
            (Some(id), Some(name)) => {
                if state.app.db.rename_playlist(id, name) {
                    (format!("이름을 '{name}' (으)로 변경했습니다."), false)
                } else {
                    ("해당 ID의 플레이리스트가 없습니다.".to_string(), true)
                }
            }
            _ => ("새 이름을 입력하세요.".to_string(), true),
        },
        "addtrack" => {
            match (
                f.playlist_id,
                f.input.as_deref().map(str::trim).filter(|i| !i.is_empty()),
            ) {
                (Some(id), Some(input)) => {
                    let app = state.app.clone();
                    let ytdlp = app.ytdlp();
                    // URL 이면 해석, 아니면 검색 1건.
                    let track = if crate::media::resolver::can_resolve(input) {
                        match crate::media::resolver::resolve(input) {
                            Ok(crate::media::resolver::Resolved::Track(t)) => {
                                ytdlp.inspect_track(&t.source_url, t.provider).await
                            }
                            _ => None,
                        }
                    } else {
                        ytdlp.search(input, 1).await.into_iter().next()
                    };
                    match track {
                        Some(track) => {
                            let title = track.display_title().to_string();
                            app.db.add_playlist_entry(
                                id,
                                &PlaylistEntry {
                                    track: Some(track),
                                    collection: None,
                                    start_offset: Some(CsTimeSpan::zero()),
                                    extra: Default::default(),
                                },
                            );
                            (format!("곡을 추가했습니다: {title}"), false)
                        }
                        None => (
                            "곡을 해석하지 못했습니다 — URL/검색어를 확인하세요.".to_string(),
                            true,
                        ),
                    }
                }
                _ => (
                    "플레이리스트 ID와 곡 URL/검색어를 입력하세요.".to_string(),
                    true,
                ),
            }
        }
        _ => ("알 수 없는 동작입니다.".to_string(), true),
    };
    redirect_flash("/playlists", &msg, is_err)
}

// ───────── 차단 목록 ─────────

#[derive(Deserialize, Default)]
pub struct BlacklistQuery {
    scope: Option<String>,
}

pub async fn blacklist_page(
    State(state): Ctx,
    cookies: Cookies,
    Query(q): Query<BlacklistQuery>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let app = &state.app;
    let meta: HashMap<u64, GuildMetadata> = app
        .db
        .list_guild_metadata()
        .into_iter()
        .map(|m| (m.guild_id, m))
        .collect();
    let scope_gid: u64 = q
        .scope
        .as_deref()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let scope_label = if scope_gid == 0 {
        "전역".to_string()
    } else {
        meta.get(&scope_gid)
            .map(|m| format!("{} ({scope_gid})", m.name))
            .unwrap_or_else(|| format!("길드 {scope_gid}"))
    };
    let entries = if scope_gid == 0 {
        app.db.list_all_blacklist()
    } else {
        app.db.list_blacklist(scope_gid)
    };
    let rows: String = entries
        .iter()
        .map(|e| {
            let entry_scope = if e.guild_id == 0 { "전역".to_string() } else { format!("길드 {}", e.guild_id) };
            let created = fmt_ts(&e.created_utc);
            let note = e
                .note
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .map(|n| format!(r#"<div class="kv" style="margin-top:4px">메모: {}</div>"#, html_escape(n)))
                .unwrap_or_default();
            format!(
                r#"<div class="pl-row">
<strong>{kind}</strong> <span class="kv">· ID {id} · {entry_scope} · {created}</span>
<div style="margin-top:4px"><code style="word-break:break-all">{pattern}</code></div>
{note}
<div class="actions" style="margin-top:6px">
<form method="post" action="/blacklist" onsubmit="return confirm('이 규칙을 삭제할까요?')"><input type="hidden" name="action" value="remove"/><input type="hidden" name="scope" value="{scope_gid}"/><input type="hidden" name="rule_id" value="{id}"/><button type="submit" class="btn btn-danger">삭제</button></form>
</div></div>"#,
                kind = e.kind.label(),
                id = e.id,
                pattern = html_escape(&e.pattern),
            )
        })
        .collect();
    let rows = if rows.is_empty() {
        r#"<p class="kv">(등록된 규칙이 없습니다)</p>"#.to_string()
    } else {
        rows
    };
    let options: String = app
        .db
        .list_known_guild_ids()
        .iter()
        .map(|gid| {
            let name = meta
                .get(gid)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| format!("길드 {gid}"));
            let sel = if scope_gid == *gid { "selected" } else { "" };
            format!(
                r#"<option value="{gid}" {sel}>{} ({gid})</option>"#,
                html_escape(&name)
            )
        })
        .collect();
    let global_sel = if scope_gid == 0 { "selected" } else { "" };
    let body = format!(
        r#"<h1 class="page-title">차단 목록</h1>
<p class="page-sub">트랙 제목 또는 URL 패턴으로 재생을 막습니다. 전역은 모든 서버에 동시에 적용됩니다.</p>
<div class="card"><h2>범위 선택</h2>
<p class="sub">차단 규칙을 적용할 범위를 고르세요. 평가 시 길드 전용 규칙과 전역 규칙이 항상 함께 검사됩니다.</p>
<form method="get" action="/blacklist" class="actions">
<select name="scope" style="max-width:320px"><option value="0" {global_sel}>전역 (모든 서버)</option>{options}</select>
<button type="submit" class="btn btn-secondary">불러오기</button>
</form>
<form method="get" action="/blacklist" class="actions" style="margin-top:10px">
<input type="text" name="scope" placeholder="길드 ID 직접 입력" style="max-width:240px"/>
<button type="submit" class="btn btn-secondary">직접 불러오기</button>
</form>
</div>
<div class="card"><h2>새 차단 규칙 추가 ({scope_label})</h2>
<form method="post" action="/blacklist"><input type="hidden" name="action" value="add"/><input type="hidden" name="scope" value="{scope_gid}"/><input type="hidden" name="guild_id" value="{scope_gid}"/>
<label class="field">일치 방식</label>
<div class="checkbox">
<label style="display:flex;align-items:center;gap:6px;margin-right:14px"><input type="radio" name="kind" value="TitleContains" checked/> 제목 포함 (대소문자 무시, 부분 일치)</label>
<label style="display:flex;align-items:center;gap:6px;margin-right:14px"><input type="radio" name="kind" value="TitleExact"/> 제목 일치 (앞뒤 공백 무시, 대소문자 무시, 완전 일치)</label>
<label style="display:flex;align-items:center;gap:6px"><input type="radio" name="kind" value="UrlExact"/> URL 일치 (정규화 후 비교)</label>
</div>
<label class="field">패턴</label><input type="text" name="pattern" required placeholder="예: (가사) 또는 https://www.youtube.com/watch?v=XXXX"/>
<label class="field">메모 (선택)</label><input type="text" name="note" placeholder="예: 가사 영상 노이즈 차단"/>
<div class="actions"><button class="btn btn-primary" type="submit">규칙 추가</button></div></form></div>
<div class="card"><h2>현재 규칙 목록 ({scope_label} · {count}개)</h2>{rows}</div>"#,
        count = entries.len(),
    );
    layout(&state, "차단 목록", "/blacklist", &body).into_response()
}

#[derive(Deserialize)]
pub struct BlacklistForm {
    action: String,
    /// 처리 후 돌아갈 범위 (페이지 상태 유지용).
    scope: Option<String>,
    rule_id: Option<i64>,
    guild_id: Option<u64>,
    kind: Option<String>,
    pattern: Option<String>,
    note: Option<String>,
}

pub async fn blacklist_post(
    State(state): Ctx,
    cookies: Cookies,
    Form(f): Form<BlacklistForm>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    let back = match f
        .scope
        .as_deref()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
    {
        0 => "/blacklist".to_string(),
        gid => format!("/blacklist?scope={gid}"),
    };
    let (msg, is_err) = match f.action.as_str() {
        "add" => {
            let kind = f
                .kind
                .as_deref()
                .and_then(BlacklistKind::parse)
                .unwrap_or(BlacklistKind::TitleContains);
            let pattern = f.pattern.unwrap_or_default();
            if pattern.trim().is_empty() {
                ("패턴을 입력하세요.".to_string(), true)
            } else {
                let pattern = if kind == BlacklistKind::UrlExact {
                    crate::blacklist::Blacklist::canonicalize_url(&pattern)
                } else {
                    pattern.trim().to_string()
                };
                state.app.db.add_blacklist(
                    f.guild_id.unwrap_or(0),
                    kind,
                    &pattern,
                    0,
                    f.note.as_deref(),
                );
                (
                    format!("차단 규칙을 추가했습니다: {} '{pattern}'", kind.label()),
                    false,
                )
            }
        }
        "remove" => match f.rule_id {
            Some(id) => {
                if state.app.db.remove_blacklist(id) {
                    (format!("규칙 ID {id} 을(를) 삭제했습니다."), false)
                } else {
                    (format!("규칙 ID {id} 을(를) 찾지 못했습니다."), true)
                }
            }
            None => ("규칙 ID가 없습니다.".to_string(), true),
        },
        _ => ("알 수 없는 동작입니다.".to_string(), true),
    };
    redirect_flash(&back, &msg, is_err)
}

// ───────── 로그 뷰어 ─────────

#[derive(Deserialize, Default)]
pub struct LogsQuery {
    level: Option<String>,
    category: Option<String>,
    count: Option<usize>,
}

pub async fn logs_page(
    State(state): Ctx,
    cookies: Cookies,
    Query(q): Query<LogsQuery>,
) -> Response {
    if let Some(r) = require_auth(&state, &cookies) {
        return r;
    }
    // C# 로그 뷰어와 동일하게 기본 600줄 (상한 2000 유지).
    let count = q.count.unwrap_or(600).clamp(20, 2000);
    let level_filter = q.level.unwrap_or_default();
    let level_filter = if level_filter.eq_ignore_ascii_case("All") {
        String::new()
    } else {
        level_filter
    };
    let cat_filter = q.category.unwrap_or_default();
    let cat_filter = if cat_filter.eq_ignore_ascii_case("All") {
        String::new()
    } else {
        cat_filter
    };
    let logs = state.app.log.recent(count);
    // 카테고리 드롭다운 — C# 처럼 실제 로그에서 본 종류만 나열.
    let mut categories: Vec<String> = logs.iter().map(|l| l.category.clone()).collect();
    categories.sort();
    categories.dedup();
    let filtered: Vec<&LogEntry> = logs
        .iter()
        .filter(|l| level_filter.is_empty() || l.level.eq_ignore_ascii_case(&level_filter))
        .filter(|l| cat_filter.is_empty() || l.category.eq_ignore_ascii_case(&cat_filter))
        .collect();
    let rows: String = filtered
        .iter()
        .map(|l| {
            let cls = match l.level.as_str() {
                "Warn" => "logrow log-warn",
                "Error" => "logrow log-err",
                _ => "logrow log-info",
            };
            format!(
                r#"<div class="{cls}"><span class="logtime">{}</span><span class="loglevel">{}</span><span class="logcat">{}</span><span class="logmsg">{}</span></div>"#,
                fmt_ts(&l.timestamp),
                l.level,
                html_escape(&l.category),
                html_escape(&l.message)
            )
        })
        .collect();
    let table = if rows.is_empty() {
        r#"<p class="kv">(해당 조건의 로그 없음)</p>"#.to_string()
    } else {
        format!(r#"<div class="logtable">{rows}</div>"#)
    };
    let lv_sel = |v: &str| {
        if level_filter.eq_ignore_ascii_case(v) {
            "selected"
        } else {
            ""
        }
    };
    let cat_options: String = categories
        .iter()
        .map(|c| {
            let sel = if cat_filter.eq_ignore_ascii_case(c) {
                "selected"
            } else {
                ""
            };
            format!(r#"<option value="{0}" {sel}>{0}</option>"#, html_escape(c))
        })
        .collect();
    let body = format!(
        r#"<h1 class="page-title">로그 뷰어</h1>
<p class="page-sub">레벨·종류(카테고리)로 걸러서 봅니다. 최근 {count}줄 기준 (메모리 링 + 일자별 JSONL).</p>
<div class="card">
<form method="get" action="/logs" class="logfilter">
<div><label class="field">레벨</label>
<select name="level"><option value="All">전체</option><option value="Info" {info_sel}>Info</option><option value="Warn" {warn_sel}>Warn</option><option value="Error" {err_sel}>Error</option></select></div>
<div><label class="field">종류(카테고리)</label>
<select name="category"><option value="All">전체</option>{cat_options}</select></div>
<div><label class="field">줄 수 (20–2000)</label>
<input type="number" name="count" value="{count}" min="20" max="2000" style="max-width:110px"/></div>
<div style="align-self:flex-end">
<button type="submit" class="btn btn-primary">적용</button>
<a class="btn btn-secondary" href="/logs">초기화</a>
</div>
</form>
</div>
<div class="card">{table}</div>"#,
        info_sel = lv_sel("Info"),
        warn_sel = lv_sel("Warn"),
        err_sel = lv_sel("Error"),
    );
    layout(&state, "로그 뷰어", "/logs", &body).into_response()
}
