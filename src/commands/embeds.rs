//! 임베드/버튼 빌더 — C# 의 Now Playing 카드, 큐 요약, 재생 컨트롤 버튼, 진행도 바.

use crate::models::*;
use serenity::builder::{
    CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, CreateSelectMenu,
    CreateSelectMenuKind, CreateSelectMenuOption,
};
use std::time::Duration;

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn fmt_duration(d: Option<CsTimeSpan>) -> String {
    d.map(|v| v.display())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// 원본 URL 이 http(s) 면 곡 제목을 마크다운 하이퍼링크로, 아니면 평문으로.
/// (임베드 description/field 안에서만 [text](url) 마스킹 링크가 렌더됨.)
fn linked_title(track: &TrackRef, max: usize) -> String {
    let title = truncate(track.display_title(), max);
    let url = track.source_url.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        // 링크 텍스트 안의 대괄호는 마크다운을 깨뜨리므로 치환.
        let safe = title.replace('[', "(").replace(']', ")");
        format!("[{safe}]({url})")
    } else {
        title
    }
}

/// 임베드 title 클릭용 url — http(s) 일 때만 Some.
fn http_url(track: &TrackRef) -> Option<&str> {
    let url = track.source_url.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url)
    } else {
        None
    }
}

fn fmt_position(d: Duration) -> String {
    CsTimeSpan(d).display()
}

/// 16칸 이모지 진행 바. 일시정지면 헤드가 ⏸️.
pub fn progress_bar(elapsed: Duration, total: Duration, paused: bool) -> String {
    const SLOTS: usize = 16;
    let ratio = if total > Duration::ZERO {
        (elapsed.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let head = (ratio * (SLOTS - 1) as f64).round() as usize;
    let mut bar = String::new();
    for i in 0..SLOTS {
        if i == head {
            bar.push_str(if paused { "⏸️" } else { "🔘" });
        } else if i < head {
            bar.push('▬');
        } else {
            bar.push('▭');
        }
    }
    bar
}

fn describe_requester(item: &QueueItem) -> String {
    if item.request_kind == PlaybackRequestKind::Autoplay {
        "자동추천".to_string()
    } else {
        item.requested_by_display.clone()
    }
}

fn youtube_thumb(track: &TrackRef) -> Option<String> {
    match track.provider {
        ProviderKind::YouTube | ProviderKind::YouTubeMusic => Some(format!(
            "https://i.ytimg.com/vi/{}/hqdefault.jpg",
            track.content_id
        )),
        _ => None,
    }
}

/// Now Playing 임베드 (+ 선택적 진행 위치).
pub fn now_playing_embed(
    state: &GuildPlayerState,
    item: &QueueItem,
    position: Option<Duration>,
) -> CreateEmbed {
    let track = &item.track;
    let mut description = format!("> {}", linked_title(track, 256));
    if let Some(artist) = &track.artist {
        description.push_str(&format!("\n> {artist}"));
    }
    if let (Some(pos), Some(total)) = (position, track.duration) {
        let total_d = Duration::from_secs_f64(total.as_secs_f64());
        let pos = pos.min(total_d);
        description.push_str(&format!(
            "\n\n{}\n`{} / {}` · 조회 시점 기준",
            progress_bar(pos, total_d, state.is_paused),
            fmt_position(pos),
            fmt_duration(Some(total)),
        ));
    }
    let mut footer = format!("Volume {}%", state.effective_volume);
    if state.is_paused {
        footer.push_str(" | Paused");
    }
    let mut embed = CreateEmbed::new()
        .colour(0xF1C40F)
        .title("Now Playing")
        .description(description)
        .field("Provider", track.provider.as_str(), true)
        .field("Duration", fmt_duration(track.duration), true)
        .field("Requested By", describe_requester(item), true)
        .field(
            "Queue",
            format!("{} tracks waiting", state.upcoming.len()),
            true,
        )
        .field("Repeat", state.repeat_mode.as_str(), true)
        .field(
            "Autoplay",
            if state.autoplay_enabled { "On" } else { "Off" },
            true,
        )
        .footer(CreateEmbedFooter::new(footer));
    if let Some(next) = state.upcoming.first() {
        embed = embed.field("Next Up", truncate(next.track.display_title(), 80), false);
    } else if let Some(preview) = &state.autoplay_preview {
        embed = embed.field(
            "Next Up",
            format!(
                "{} (자동추천 예정)",
                truncate(preview.track.display_title(), 70)
            ),
            false,
        );
    }
    if let Some(thumb) = youtube_thumb(track) {
        embed = embed.thumbnail(thumb);
    }
    // "Now Playing" 제목 자체도 원본 영상으로 클릭되도록.
    if let Some(url) = http_url(track) {
        embed = embed.url(url);
    }
    embed
}

/// 큐 요약 임베드 (페이지네이션).
pub fn queue_page_embed(state: &GuildPlayerState, page: usize) -> (CreateEmbed, usize) {
    const PER_PAGE: usize = 10;
    let total_pages = (state.upcoming.len().max(1) + PER_PAGE - 1) / PER_PAGE;
    let page = page.min(total_pages.saturating_sub(1));

    let total_secs: f64 = state
        .upcoming
        .iter()
        .filter_map(|i| i.track.duration.map(|d| d.as_secs_f64()))
        .sum();

    // C# 과 동일한 상태별 색상: 재생=초록, 일시정지=주황, 빈 상태=회색.
    let colour = if state.current_item.is_some() {
        if state.is_paused { 0xE67E22 } else { 0x2ECC71 }
    } else {
        0x95A5A6
    };
    let mut embed = CreateEmbed::new().colour(colour).title(format!(
        "📋 대기열 · {}곡 · 총 {}",
        state.upcoming.len(),
        CsTimeSpan(Duration::from_secs_f64(total_secs)).display()
    ));

    if let Some(cur) = &state.current_item {
        let mut line = format!("**{}**", linked_title(&cur.track, 80));
        if let Some(a) = &cur.track.artist {
            line.push_str(&format!("  ·  {}", truncate(a, 40)));
        }
        line.push_str(&format!(
            "\n⏱ {}  ·  👤 {}",
            fmt_duration(cur.track.duration),
            describe_requester(cur)
        ));
        embed = embed.description(line);
    } else {
        embed = embed.description("지금은 재생 중인 곡이 없어요.");
    }

    // C# 과 동일한 "다음 곡" 필드 — 대기열 첫 곡 또는 자동추천 예정 곡.
    if let Some(next) = state.upcoming.first() {
        embed = embed.field("▶ 다음 곡", linked_title(&next.track, 80), false);
    } else if let Some(preview) = &state.autoplay_preview {
        embed = embed.field(
            "▶ 다음 곡",
            format!(
                "{} (자동추천 예정)",
                truncate(preview.track.display_title(), 70)
            ),
            false,
        );
    }

    if state.upcoming.is_empty() {
        embed = embed.field("대기열", "아직 비어 있어요.", false);
    } else {
        let start = page * PER_PAGE;
        let lines: Vec<String> = state
            .upcoming
            .iter()
            .enumerate()
            .skip(start)
            .take(PER_PAGE)
            .map(|(i, q)| {
                format!(
                    "`{:>2}.` {} · {}",
                    i + 1,
                    linked_title(&q.track, 50),
                    fmt_duration(q.track.duration)
                )
            })
            .collect();
        embed = embed.field(
            format!("페이지 {}/{}", page + 1, total_pages),
            lines.join("\n"),
            false,
        );
    }

    let mut footer = format!(
        "Repeat {} · Autoplay {} · Volume {}%",
        state.repeat_mode.as_str(),
        if state.autoplay_enabled { "On" } else { "Off" },
        state.effective_volume
    );
    if state.is_paused {
        footer.push_str(" · Paused");
    }
    embed = embed.footer(CreateEmbedFooter::new(footer));
    (embed, total_pages)
}

/// 재생 컨트롤 버튼 (mb: prefix — 큐 카드에 붙는다). 누르면 큐 카드로 갱신.
pub fn playback_buttons(state: &GuildPlayerState) -> Vec<CreateActionRow> {
    buttons_with_prefix(state, "mb:")
}

/// Now Playing 카드용 컨트롤 버튼 (mbnp: prefix). 누르면 NP 카드로 갱신돼
/// 진행바/썸네일이 큐 목록으로 바뀌지 않는다. ("📋 대기열" 버튼만 큐 카드로 전환)
pub fn playback_buttons_np(state: &GuildPlayerState) -> Vec<CreateActionRow> {
    buttons_with_prefix(state, "mbnp:")
}

/// 카드에 붙는 컨트롤. 버튼은 최소한만 남기고 나머지는 웹 리모컨으로 보낸다.
///
/// 예전에는 두 줄 10개(셔플·반복·볼륨±·자동추천·다시재생·대기열까지)였다.
/// 채팅창을 잡아먹는 데다, 셔플·반복·볼륨·대기열은 리모컨에서 훨씬 잘 보이고 잘 눌린다.
/// 여기에는 "지금 당장 눌러야 하는 것"만 남긴다.
fn buttons_with_prefix(state: &GuildPlayerState, p: &str) -> Vec<CreateActionRow> {
    let mut row = vec![
        CreateButton::new(format!("{p}playpause"))
            .style(serenity::all::ButtonStyle::Secondary)
            .label(if state.is_paused {
                "▶ 재생"
            } else {
                "⏸ 일시정지"
            }),
        CreateButton::new(format!("{p}skip"))
            .style(serenity::all::ButtonStyle::Primary)
            .label("⏭ 스킵"),
        CreateButton::new(format!("{p}stop"))
            .style(serenity::all::ButtonStyle::Danger)
            .label("⏹ 정지"),
    ];
    // 링크 버튼은 인터랙션을 만들지 않으므로 커스텀 ID 가 없다.
    // 공개 주소가 아직 설정되지 않았으면 조용히 빼고 나머지만 보여준다.
    if let Some(url) = crate::app::remote_url_for(state.guild_id) {
        row.push(CreateButton::new_link(url).label("🎛 리모컨"));
    }
    vec![CreateActionRow::Buttons(row)]
}

/// `/상태` — 봇 버전 + 사용자에게 보여줄 만한 재생/전역 설정 요약(시크릿 제외).
///
/// `voice_connected` 는 **songbird 라이브 연결**로 판정한 값을 호출부가 넣어 준다 (v3 §16 B1).
/// 저장된 `state.voice_channel_id` 는 봇이 강제 퇴장·재시작·네트워크 끊김으로 빠져나가도
/// 그대로 남아 있어서 "지금 어디 있나"의 근거가 될 수 없다. 그 값은 "다음에 어디로 들어갈까"
/// 에만 쓴다. 여기서 인자를 받는 이유가 그것이므로 `state` 를 다시 보지 않는다.
pub fn status_embed(
    state: &GuildPlayerState,
    g: &GlobalSettings,
    build_id: &str,
    version: &str,
    voice_connected: bool,
) -> CreateEmbed {
    let on = |b: bool| if b { "켜짐" } else { "꺼짐" };
    let now = match &state.current_item {
        Some(i) => truncate(i.track.display_title(), 60),
        None => "없어요".to_string(),
    };
    let voice = if voice_connected { "연결됨" } else { "미연결" };
    let empty_policy = match g.empty_voice_policy {
        EmptyVoiceChannelPolicy::AutoLeave => "비면 나가기",
        EmptyVoiceChannelPolicy::StopPlayback => "비면 정지",
        EmptyVoiceChannelPolicy::DoNothing => "유지",
    };
    let playback = format!(
        "▶ 재생 중: **{now}**\n📋 대기열: **{}곡**\n🔊 음성: **{voice}**\n🔈 볼륨: **{}%**\n🔁 반복: **{}**\n🔀 셔플: **{}**\n✨ 자동추천: **{}**",
        state.upcoming.len(),
        state.effective_volume,
        state.repeat_mode.as_str(),
        on(state.shuffle_enabled),
        on(state.autoplay_enabled),
    );
    let global = format!(
        "✨ 자동추천 기본값: **{}**\n⏱ 자동추천 최대 길이: **10분** (더 긴 곡은 추천에서 빠져요)\n✂️ 인트로/아웃트로 제거: **{}** (SponsorBlock)\n🎚 볼륨 평준화: **{}**\n🔔 곡 시작 알림: **{}**\n📡 음성 비트레이트: **{}kbps**\n🚪 빈 채널일 때: **{}** ({}초 뒤에)\n💾 캐시 한도: **{}GB**\n🗂 로그 보관: **{}일**",
        on(g.autoplay_default),
        on(g.sponsorblock_remove),
        on(g.normalize_enabled),
        on(g.announce_now_playing),
        g.voice_bitrate_kbps,
        empty_policy,
        g.auto_leave_delay_seconds,
        g.cache_limit_gb,
        g.log_retention_days,
    );
    let tweaks = format!(
        "fastStart: {} · directOut: {} · smallBuf: {} · lowLoss: {} · sendThread: {}",
        on(g.tweak_ffmpeg_fast_start),
        on(g.tweak_ffmpeg_direct_output),
        on(g.tweak_small_buffer),
        on(g.tweak_low_packet_loss),
        on(g.tweak_dedicated_send_thread),
    );
    CreateEmbed::new()
        .colour(0x2ECC71)
        .title("🎛 봇 상태")
        .field(
            "버전",
            format!("v{version} · build `{build_id}`\n엔진: Rust · serenity + songbird"),
            false,
        )
        .field("현재 서버 재생", playback, false)
        .field("전역 설정", global, false)
        .field("끊김 최적화 토글", tweaks, false)
        .footer(CreateEmbedFooter::new(
            "설정은 웹 대시보드(포트 8693)에서 바꿔요.",
        ))
}

// ───────── 검색 / 취소 ─────────

/// `/검색` 후보 목록 임베드.
pub fn search_results_embed(
    query: &str,
    provider: ProviderKind,
    candidates: &[TrackRef],
) -> CreateEmbed {
    let lines: Vec<String> = candidates
        .iter()
        .enumerate()
        .take(25)
        .map(|(i, t)| {
            let mut line = format!("`{:>2}.` {}", i + 1, linked_title(t, 60));
            if let Some(a) = &t.artist {
                line.push_str(&format!(" · {}", truncate(a, 25)));
            }
            line.push_str(&format!(" · {}", fmt_duration(t.duration)));
            line
        })
        .collect();
    CreateEmbed::new()
        .colour(0x3498DB)
        .title(format!(
            "🔎 '{}' 검색 결과 · {}",
            truncate(query, 80),
            provider.as_str()
        ))
        .description(if lines.is_empty() {
            "찾은 곡이 없어요.".to_string()
        } else {
            lines.join("\n")
        })
        .footer(CreateEmbedFooter::new(
            "아래 메뉴에서 곡을 고르면 대기열에 담겨요. ✖ 취소로 닫을 수 있어요.",
        ))
}

/// 검색 후보 셀렉트 메뉴 + 검색 취소 버튼. token 으로 서버의 후보 세션을 되찾는다.
pub fn search_results_components(token: &str, candidates: &[TrackRef]) -> Vec<CreateActionRow> {
    let options: Vec<CreateSelectMenuOption> = candidates
        .iter()
        .enumerate()
        .take(25)
        .map(|(i, t)| {
            let label = format!("{}. {}", i + 1, truncate(t.display_title(), 92));
            let mut desc = format!("{} · {}", t.provider.label(), fmt_duration(t.duration));
            if let Some(a) = &t.artist {
                desc = format!("{} · {}", desc, truncate(a, 40));
            }
            CreateSelectMenuOption::new(truncate(&label, 100), i.to_string())
                .description(truncate(&desc, 100))
        })
        .collect();

    let menu = CreateSelectMenu::new(
        format!("mbsel:{token}"),
        CreateSelectMenuKind::String { options },
    )
    .placeholder("재생할 곡을 골라 주세요");

    vec![
        CreateActionRow::SelectMenu(menu),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("mbsx:{token}"))
                .style(serenity::all::ButtonStyle::Danger)
                .label("✖ 취소"),
        ]),
    ]
}

/// `/재생` 응답에 붙는 ✖ 취소 버튼 (해당 곡만 큐에서 취소).
pub fn cancel_button(item_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("mbcx:{item_id}"))
            .style(serenity::all::ButtonStyle::Danger)
            .label("✖ 취소"),
    ])]
}

/// 큐 페이지 이동 버튼.
pub fn queue_page_buttons(page: usize, total_pages: usize) -> Vec<CreateActionRow> {
    if total_pages <= 1 {
        return Vec::new();
    }
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("mbq:prev:{page}"))
            .style(serenity::all::ButtonStyle::Secondary)
            .label("◀ 이전")
            .disabled(page == 0),
        CreateButton::new(format!("mbq:next:{page}"))
            .style(serenity::all::ButtonStyle::Secondary)
            .label("다음 ▶")
            .disabled(page + 1 >= total_pages),
    ])]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 임베드 본문 전체를 한 덩어리 문자열로 — 필드 이름이 바뀌어도 검사가 안 깨지게.
    fn embed_text(embed: &CreateEmbed) -> String {
        serde_json::to_string(embed).expect("임베드 직렬화")
    }

    /// `/상태` 의 `🔊 음성` 은 **저장값이 아니라 라이브 연결**을 말한다 (v3 §16 B1).
    ///
    /// 봇이 강제 퇴장·재시작으로 음성에서 빠져도 `guild_states.voice_channel_id` 는 남는다.
    /// 예전에는 그 값만 보고 "연결됨" 을 찍어서, 봇이 없는데 있다고 말하는 화면이
    /// 리모컨 말고 Discord 임베드에도 하나 더 있었다. 저장값이 있어도 라이브가 끊겼으면
    /// 반드시 "미연결" 이어야 한다.
    #[test]
    fn status_embed_reports_voice_from_the_live_connection_not_the_stored_channel() {
        let mut state = GuildPlayerState::default();
        // 저장값은 남아 있다 — "다음에 어디로 들어갈까" 용으로는 이게 맞다.
        state.voice_channel_id = Some(1234);
        let g = GlobalSettings::default();

        let left = status_embed(&state, &g, "build", "0.0.0", false);
        assert!(
            embed_text(&left).contains("🔊 음성: **미연결**"),
            "라이브 연결이 없으면 저장값이 남아 있어도 미연결이다"
        );

        let joined = status_embed(&state, &g, "build", "0.0.0", true);
        assert!(embed_text(&joined).contains("🔊 음성: **연결됨**"));

        // 저장값이 비어 있어도 라이브가 붙어 있으면 연결됨이다(재시작 직후 등).
        state.voice_channel_id = None;
        let live_only = status_embed(&state, &g, "build", "0.0.0", true);
        assert!(embed_text(&live_only).contains("🔊 음성: **연결됨**"));
    }
}
