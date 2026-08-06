//! 재생 부수효과: 다음 곡 프리페치, autoplay preview 해결, "현재 재생 중" 알림.
//! 실패해도 재생 경로에 영향을 주지 않는 fire-and-forget 작업들.

use crate::app::App;
use crate::models::*;
use crate::player::autoplay::{AutoplayContext, AutoplayTuning};
use crate::player::coordinator::Coordinator;
use crate::player::manager::PlayerManager;
use crate::remote::AutoplayMode;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// preview 추천 진행중 플래그를 패닉/조기반환에도 반드시 해제하는 가드.
/// 수동 end_preview_resolve 만 두면 recommend()/lock 패닉 시 플래그가 영구히 남아
/// 그 길드의 autoplay 가 다시는 채워지지 않고 매 곡 경계마다 15초씩 멈춘다(2026-06-17 감사).
struct InflightGuard {
    player: Arc<PlayerManager>,
    guild_id: u64,
}
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.player.end_preview_resolve(self.guild_id);
    }
}

/// announce 채널에 짧은 안내 한 줄을 보낸다 (재생 실패/복구 등). 채널 미기록 시 무시.
/// announce_now_playing 설정과 무관하게 항상 보낸다(오류/복구 통지이므로).
pub async fn announce_text(app: &Arc<App>, guild_id: u64, content: &str) {
    let Some(http) = app.http.get() else { return };
    let channel_id = {
        let map = app.announce_channels.lock().unwrap();
        match map.get(&guild_id) {
            Some(c) => *c,
            None => return,
        }
    };
    let builder = serenity::builder::CreateMessage::new().content(content);
    let _ = serenity::model::id::ChannelId::new(channel_id)
        .send_message(http, builder)
        .await;
}

/// 곡 시작 시 부수효과 일괄 시동.
pub fn on_track_started(
    app: Arc<App>,
    coordinator: Arc<Coordinator>,
    guild_id: u64,
    item: QueueItem,
) {
    // 1) 일정 시간 정상 재생되면 재시도 카운터 리셋.
    {
        let coordinator = coordinator.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            coordinator.reset_retry(guild_id).await;
        });
    }
    // 2) autoplay preview 해결 + 다음 곡 프리페치.
    {
        let app2 = app.clone();
        let coordinator2 = coordinator.clone();
        tokio::spawn(async move {
            resolve_preview(app2.clone(), guild_id).await;
            prefetch_next(app2, coordinator2, guild_id).await;
        });
    }
    // 3) "현재 재생 중" 알림.
    {
        tokio::spawn(async move {
            announce_now_playing(app, guild_id, item).await;
        });
    }
}

/// 자동추천 한 번에 필요한 입력 한 벌 (§8.2 · §8.5).
/// DB 조회는 여기서 **한 번씩만** 한다 — 추천 경로에서 여러 번 왕복하면 곡 경계가 밀린다.
struct AutoplayPlan {
    /// 라운드로빈으로 돌 시드 목록. 비어 있으면 `fallback` 한 곡만 쓴다.
    seeds: Vec<TrackRef>,
    fallback: Option<TrackRef>,
    /// 지금 재생 중·대기열에 있는 곡. 무조건 제외.
    excluded: HashSet<String>,
    /// `📻 이 곡 말고`로 뺐거나 재생에 실패한 곡 (§8.5-3).
    blocked: HashSet<String>,
    /// `cache_key → 마지막 재생 후 지난 시간`. **최근 목록에 있다고 영원히 빼지 않는다** (§8.5-2).
    recent_ages: HashMap<String, f64>,
    recent_artists: Vec<String>,
    tuning: AutoplayTuning,
}

impl AutoplayPlan {
    fn context(&self) -> AutoplayContext<'_> {
        AutoplayContext {
            excluded: &self.excluded,
            blocked: &self.blocked,
            recent_ages: &self.recent_ages,
            recent_artists: &self.recent_artists,
            tuning: self.tuning,
        }
    }

    fn is_empty(&self) -> bool {
        self.seeds.is_empty() && self.fallback.is_none()
    }
}

/// 길드 설정의 추천 방식(§8.2)대로 시드를 고르고, 정책 입력을 모아 온다.
///
/// **폴백 사슬**: `seed`(시드 없음) → `recent` → `genre` → 포기.
/// 어떤 모드든 시드를 못 구하면 조용히 멈추지 말고 다음으로 내려간다 —
/// 자동재생이 이유 없이 멈춘 것처럼 보이는 게 제일 나쁘다.
fn build_autoplay_plan(app: &Arc<App>, guild_id: u64, state: &GuildPlayerState) -> AutoplayPlan {
    let settings = app.remote.load_guild_settings(guild_id);

    // 하드 제외는 지금 나오는 곡과 대기열뿐이다. 최근 재생은 감쇠로 다룬다.
    let mut excluded: HashSet<String> = HashSet::new();
    if let Some(current) = &state.current_item {
        excluded.insert(current.track.cache_key());
    }
    for item in &state.upcoming {
        excluded.insert(item.track.cache_key());
    }
    if let Some(preview) = &state.autoplay_preview {
        excluded.insert(preview.track.cache_key());
    }

    let (recent_ages, recent_artists) = app.remote.recent_play_history(guild_id, 200);
    let blocked = app.remote.blocked_autoplay_keys(guild_id);

    let mut mode = settings.autoplay_mode;
    let mut seeds;
    loop {
        seeds = seeds_for_mode(app, guild_id, &settings, state, mode);
        if !seeds.is_empty() {
            break;
        }
        match mode.fallback() {
            Some(next) => {
                app.log.info(
                    "Autoplay",
                    &format!(
                        "{} 기준으로는 참고할 곡이 없어서 {} 기준으로 내려가요.",
                        mode.label(),
                        next.label()
                    ),
                );
                mode = next;
            }
            None => break,
        }
    }

    // 사슬 끝까지 갔는데도 비었으면 지금 나오는 곡이라도 쓴다 (지금까지의 동작).
    let fallback = state
        .current_item
        .as_ref()
        .map(|item| item.track.clone())
        .or_else(|| state.recent_tracks.first().cloned());

    AutoplayPlan {
        seeds,
        fallback,
        excluded,
        blocked,
        recent_ages,
        recent_artists,
        tuning: AutoplayTuning {
            policy: settings.autoplay_policy,
            artist_cooldown: settings.autoplay_artist_cooldown,
            recent_decay_hours: settings.autoplay_recent_decay_hours,
        },
    }
}

/// 모드별 시드 목록 (§8.2). 엔진이 이 목록을 길드별 커서로 라운드로빈한다.
fn seeds_for_mode(
    app: &Arc<App>,
    guild_id: u64,
    settings: &crate::remote::RemoteGuildSettings,
    state: &GuildPlayerState,
    mode: AutoplayMode,
) -> Vec<TrackRef> {
    match mode {
        AutoplayMode::Seed => app
            .remote
            .list_autoplay_seeds(guild_id)
            .into_iter()
            .map(|seed| seed.track)
            .collect(),
        AutoplayMode::Recent => {
            // 지금 나오는 곡을 맨 앞에 둔다 — 첫 회전은 지금까지와 똑같이 동작한다.
            let mut seeds: Vec<TrackRef> = state
                .current_item
                .as_ref()
                .map(|item| item.track.clone())
                .into_iter()
                .collect();
            for track in &state.recent_tracks {
                if seeds.len() >= settings.autoplay_recent_count.max(1) as usize {
                    break;
                }
                if !seeds
                    .iter()
                    .any(|seed| seed.cache_key().eq_ignore_ascii_case(&track.cache_key()))
                {
                    seeds.push(track.clone());
                }
            }
            seeds
        }
        AutoplayMode::Genre => {
            // §15 의 차트 인프라를 그대로 쓴다. **캐시만 본다** — 재생 경로에서 yt-dlp 를 돌리면
            // 곡 경계가 몇 초씩 밀린다. 캐시가 비어 있으면 폴백 사슬이 알아서 내려간다.
            let mut seeds = Vec::new();
            for key in &settings.autoplay_genres {
                let Ok(chart_id) = key.parse::<i64>() else {
                    continue;
                };
                let Some(snapshot) = app.remote.chart_cache(chart_id) else {
                    continue;
                };
                // 장르를 여러 개 고르면 장르도 라운드로빈이 되도록 앞에서 몇 곡씩만 섞어 넣는다.
                seeds.extend(snapshot.tracks.into_iter().take(5));
            }
            seeds
        }
    }
}

/// 큐가 빌 예정일 때 autoplay 미리보기를 풀어 둔다 (C# ResolveAutoplayPreviewAsync).
pub async fn resolve_preview(app: Arc<App>, guild_id: u64) {
    let state = app.player.get_state(guild_id).await;
    if !app.player.should_fill_preview(&state) {
        return;
    }
    let seed_item_id = state.current_item.as_ref().map(|c| c.id.clone());
    if !app.player.try_begin_preview_resolve(guild_id) {
        return; // 이미 진행 중.
    }
    // 패닉/조기반환에도 inflight 플래그가 풀리도록 가드로 보호.
    let _inflight = InflightGuard {
        player: app.player.clone(),
        guild_id,
    };
    let result = async {
        let plan = build_autoplay_plan(&app, guild_id, &state);
        if plan.is_empty() {
            return None;
        }
        app.autoplay
            .recommend_with_context(
                guild_id,
                plan.fallback.as_ref(),
                &plan.seeds,
                &plan.context(),
            )
            .await
    }
    .await;
    if let Some(track) = result {
        // 도중에 사용자가 곡을 추가했을 수 있으니 재검사.
        let fresh = app.player.get_state(guild_id).await;
        let same_seed_item =
            fresh.current_item.as_ref().map(|c| c.id.as_str()) == seed_item_id.as_deref();
        // 시드 곡이 그대로이거나(정상 prefetch) 스킵으로 큐가 비어 현재 곡이 사라진 경우
        // (current=None) 모두 유효한 후보다 — 후자에서 버리면 스킵 후 ensure_autoplay 가
        // 똑같은 추천을 처음부터 다시 돌려 긴 침묵이 생긴다(2026-06-17). 다른 곡으로 바뀐
        // 경우에만(그 곡이 자기 preview 를 따로 채움) 폐기한다.
        let usable =
            app.player.should_fill_preview(&fresh) && (same_seed_item || fresh.current_item.is_none());
        if usable {
            let title = track.display_title().to_string();
            app.player
                .set_preview(guild_id, QueueItem::new_autoplay(track));
            app.log.info(
                "Autoplay",
                &format!("AutoplayPreview 채움 guild={guild_id} title={title}"),
            );
        } else {
            app.log.info(
                "Autoplay",
                &format!("AutoplayPreview 폐기 guild={guild_id}: 다른 곡으로 바뀐 뒤 도착한 추천."),
            );
        }
    }
    // inflight 해제는 _inflight 가드의 Drop 이 담당 (패닉 시에도 보장).
}

/// 다음에 재생될 곡(큐 첫 항목 또는 preview)을 미리 캐시에 받아 둔다.
pub async fn prefetch_next(app: Arc<App>, _coordinator: Arc<Coordinator>, guild_id: u64) {
    let state = app.player.get_state(guild_id).await;
    let next = state
        .upcoming
        .first()
        .map(|i| i.track.clone())
        .or_else(|| state.autoplay_preview.as_ref().map(|p| p.track.clone()));
    let Some(track) = next else { return };
    let global = app.db.load_global_settings();
    let ytdlp = app.ytdlp();
    let _ = app
        .cache
        .prepare(
            &track,
            &ytdlp,
            global.cache_limit_gb,
            global.sponsorblock_remove,
        )
        .await;
}

/// 곡 종료/스킵 후 autoplay 후보를 큐에 채운다 (C# EnsureAutoplayCandidateAsync).
pub async fn ensure_autoplay(
    app: Arc<App>,
    _coordinator: Arc<Coordinator>,
    guild_id: u64,
    allow_continuation: bool,
) {
    let state = app.player.get_state(guild_id).await;
    if !PlayerManager::should_seed_autoplay(&state, allow_continuation) {
        return;
    }
    // 미리 풀어 둔 preview 가 있으면 네트워크 없이 그것을 사용.
    if consume_autoplay_preview(app.clone(), guild_id, allow_continuation).await {
        return;
    }
    // preview 추천이 진행 중이면(스킵을 빨리 눌러 아직 안 끝남) 그 결과를 기다렸다 재사용한다.
    // 여기서 곧장 새 추천을 돌리면 yt-dlp 가 직렬화돼 있어 두 추천이 줄서며 긴 침묵이 생긴다.
    if app.player.is_preview_resolving(guild_id) {
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if !app.player.is_preview_resolving(guild_id) {
                break;
            }
        }
        if consume_autoplay_preview(app.clone(), guild_id, allow_continuation).await {
            return;
        }
    }
    let plan = build_autoplay_plan(&app, guild_id, &state);
    if plan.is_empty() {
        app.log.info(
            "Autoplay",
            "참고할 곡이 없어서 이번 자동 재생은 건너뛰어요(기준 곡·최근 곡·장르 차트가 전부 비었어요).",
        );
        return;
    }
    if let Some(track) = app
        .autoplay
        .recommend_with_context(
            guild_id,
            plan.fallback.as_ref(),
            &plan.seeds,
            &plan.context(),
        )
        .await
    {
        app.player
            .seed_autoplay_item(guild_id, QueueItem::new_autoplay(track), allow_continuation)
            .await;
    }
}

/// `📻 이 곡 말고` (§14.3). 지금 잡혀 있는 다음 자동추천곡을 **7일간 다시 안 뽑히게** 하고
/// 새로 하나 뽑는다. 권한은 호출부(`autoplay_rule`)가 이미 봤다고 본다.
///
/// 자동 재생 정책을 바꿨을 때도 이걸 부른다 — 안 그러면 바꾼 게 언제 먹는지 알 수 없다(§8.5).
pub async fn reject_preview(app: Arc<App>, guild_id: u64, reason: &str) -> bool {
    let Some(preview) = app.player.take_preview(guild_id) else {
        return false;
    };
    let cache_key = preview.track.cache_key();
    let _ = app
        .remote
        .block_autoplay_candidate(guild_id, &cache_key, Some(reason));
    app.log.info(
        "Autoplay",
        &format!(
            "'{}'는 다시 안 뽑아요 ({reason}). 다른 곡을 다시 골라요.",
            preview.track.display_title()
        ),
    );
    resolve_preview(app, guild_id).await;
    true
}

/// 정책·기준 곡이 바뀌었을 때 다음 추천곡만 다시 뽑는다 (§8.5 UI).
/// 이미 잡혀 있던 후보는 **차단하지 않는다** — 사용자가 싫다고 한 게 아니라 규칙이 바뀐 것뿐이다.
pub async fn refresh_preview(app: Arc<App>, guild_id: u64) {
    app.player.take_preview(guild_id);
    resolve_preview(app, guild_id).await;
}

/// 이미 계산된 preview 만 소비한다. 네트워크 추천은 하지 않으므로 스킵 경로에서 즉시 호출 가능하다.
pub async fn consume_autoplay_preview(
    app: Arc<App>,
    guild_id: u64,
    allow_continuation: bool,
) -> bool {
    let state = app.player.get_state(guild_id).await;
    if !PlayerManager::should_seed_autoplay(&state, allow_continuation) {
        return false;
    }
    let Some(preview) = app.player.take_preview(guild_id) else {
        return false;
    };
    app.log.info(
        "Queue",
        &format!("Consumed autoplay preview for guild {guild_id}."),
    );
    app.player
        .seed_autoplay_item(guild_id, preview, allow_continuation)
        .await;
    true
}

/// 곡 시작 알림 (전역 설정 announceNowPlaying). 이전 카드의 버튼은 제거.
async fn announce_now_playing(app: Arc<App>, guild_id: u64, item: QueueItem) {
    let global = app.db.load_global_settings();
    if !global.announce_now_playing {
        return;
    }
    let Some(http) = app.http.get() else { return };
    let channel_id = {
        let map = app.announce_channels.lock().unwrap();
        match map.get(&guild_id) {
            Some(c) => *c,
            None => return,
        }
    };
    let state = app.player.get_state(guild_id).await;
    let embed = crate::commands::embeds::now_playing_embed(&state, &item, None);
    let components = crate::commands::embeds::playback_buttons_np(&state);
    let builder = serenity::builder::CreateMessage::new()
        .embed(embed)
        .components(components);
    match serenity::model::id::ChannelId::new(channel_id)
        .send_message(http, builder)
        .await
    {
        Ok(msg) => {
            // 이전 카드 버튼 제거 (두 개가 동시에 활성으로 보이는 사고 방지).
            let prev = {
                let mut map = app.last_np_message.lock().unwrap();
                map.insert(guild_id, (channel_id, msg.id.get()))
            };
            if let Some((prev_ch, prev_msg)) = prev {
                let edit = serenity::builder::EditMessage::new().components(Vec::new());
                let _ = serenity::model::id::ChannelId::new(prev_ch)
                    .edit_message(http, serenity::model::id::MessageId::new(prev_msg), edit)
                    .await;
            }
        }
        Err(e) => app.log.warn("Bot", &format!("Now-playing 알림 실패: {e}")),
    }
}
