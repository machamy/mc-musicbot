//! 재생 부수효과: 다음 곡 프리페치, autoplay preview 해결, "현재 재생 중" 알림.
//! 실패해도 재생 경로에 영향을 주지 않는 fire-and-forget 작업들.

use crate::app::App;
use crate::models::*;
use crate::player::autoplay::{AutoplayContext, AutoplayTuning};

/// 다음 곡 후보를 몇 개 보여 줄지 (§8.6). 하나만 정해 주는 대신 고르게 한다.
/// 모바일 세로에서 세 줄이 적당하고, 후보를 더 뽑는 비용은 0이다(라디오는 이미 긁었다).
const AUTOPLAY_OPTION_COUNT: usize = 3;
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
    /// `🎲 후보 다시 뽑기` 횟수 (`PlayerManager::reroll_salt`).
    salt: u64,
}

impl AutoplayPlan {
    fn context(&self) -> AutoplayContext<'_> {
        AutoplayContext {
            excluded: &self.excluded,
            blocked: &self.blocked,
            recent_ages: &self.recent_ages,
            recent_artists: &self.recent_artists,
            tuning: self.tuning,
            salt: self.salt,
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
        salt: app.player.reroll_salt(guild_id),
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
        AutoplayMode::Recent => recent_seeds(
            state.current_item.as_ref().map(|item| &item.track),
            &state.recent_tracks,
            settings.recent_count_limit(),
        ),
        AutoplayMode::Genre => {
            // §15 의 차트 인프라를 그대로 쓴다. **캐시만 본다** — 재생 경로에서 yt-dlp 를 돌리면
            // 곡 경계가 몇 초씩 밀린다. 캐시가 비어 있으면 폴백 사슬이 알아서 내려간다.
            let per_genre: Vec<Vec<TrackRef>> = settings
                .autoplay_genres
                .iter()
                .filter_map(|key| key.parse::<i64>().ok())
                .filter_map(|chart_id| app.remote.chart_cache(chart_id))
                .map(|snapshot| snapshot.tracks.into_iter().take(GENRE_SEEDS_PER_CHART).collect())
                .collect();
            genre_seeds(per_genre)
        }
    }
}

/// 장르 하나에서 가져올 시드 곡 수. 여러 장르를 골랐을 때 한 장르가 목록을 다 잡아먹지 않게 한다.
const GENRE_SEEDS_PER_CHART: usize = 5;

/// `recent` 모드의 시드 (§8.2). 지금 나오는 곡을 맨 앞에 두고 최근 곡을 `limit` 만큼 잇는다.
///
/// `limit` 이 `None` 이면 **무제한** — 최근 목록 전부를 본다 (§23.1 `0 = 무제한`).
/// `.max(1)` 로 0을 1로 둔갑시키면 "무제한"이 "가장 빡빡함"이 된다.
fn recent_seeds(
    current: Option<&TrackRef>,
    recent: &[TrackRef],
    limit: Option<u32>,
) -> Vec<TrackRef> {
    let cap = limit.map(|value| value as usize).unwrap_or(usize::MAX);
    let mut seeds: Vec<TrackRef> = current.cloned().into_iter().collect();
    for track in recent {
        if seeds.len() >= cap {
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

/// 장르가 여러 개면 **장르도 라운드로빈**이다 (§8.2).
///
/// 그냥 이어 붙이면 목록이 `[장르A×5, 장르B×5]` 가 되고, 엔진의 시드 커서는 한 칸씩만 움직이므로
/// 앞의 다섯 번은 전부 장르A 로 쏠린다. 장르별 목록을 번갈아 하나씩 꺼내 섞어 둔다.
fn genre_seeds(per_genre: Vec<Vec<TrackRef>>) -> Vec<TrackRef> {
    let depth = per_genre.iter().map(Vec::len).max().unwrap_or(0);
    let mut seen: HashSet<String> = HashSet::new();
    let mut seeds = Vec::new();
    for index in 0..depth {
        for genre in &per_genre {
            let Some(track) = genre.get(index) else {
                continue;
            };
            // 같은 곡이 두 장르에 겹치면 시드가 그 곡으로 쏠린다.
            if seen.insert(track.cache_key().to_lowercase()) {
                seeds.push(track.clone());
            }
        }
    }
    seeds
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
            return Vec::new();
        }
        /* 후보를 **셋** 받아 둔다 (§8.6).
         *
         * 라디오를 긁는 일은 이미 한 번이라 추가 비용이 없다 — 같은 목록에서 두 개를
         * 더 뽑을 뿐이다. 첫 번째가 예전과 똑같이 "안 고르면 나갈 곡" 이고,
         * 나머지는 사람이 고를 수 있게 화면에 같이 보낸다. */
        app.autoplay
            .recommend_many_with_context(
                guild_id,
                plan.fallback.as_ref(),
                &plan.seeds,
                &plan.context(),
                AUTOPLAY_OPTION_COUNT,
            )
            .await
    }
    .await;
    if let Some(track) = result.first().cloned() {
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
            let options: Vec<QueueItem> = result
                .iter()
                .cloned()
                .map(QueueItem::new_autoplay)
                .collect();
            app.player.set_preview_options(guild_id, options);
            app.log.info(
                "Autoplay",
                &format!("AutoplayPreview 채움 guild={guild_id} title={title}"),
            );
            // **정해졌으면 알린다** (§38). 전에는 저장만 하고 끝이라, 웹은 다음에 무슨 곡이
            // 올지 알면서도 새로고침해야 보였다. 이 훅이 `queue.set` 을 다시 쏘고
            // 그 프레임에 `next` 가 실려 나간다.
            if let Some(hook) = app.on_queue_sorted.get() {
                hook(guild_id);
            }
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
/// **수동 스킵 뒤의 자동재생 보충.** 디스코드와 웹이 반드시 같은 것을 써야 한다.
///
/// 이게 따로 놀아서 실제로 깨져 있었다. 디스코드 `/스킵` 과 버튼 스킵은
/// `settle_manual_skip` 을 거쳐 [`ensure_autoplay`] 를 불렀는데, **웹 리모컨의 스킵만
/// 안 불렀다**(`web/remote.rs` 의 `api_control`). 그래서 대기열이 빈 상태에서 리모컨으로
/// 넘기면 화면에 다음 추천곡이 떠 있는데도 아무것도 재생되지 않고 침묵으로 끝났다.
/// `sync_guild` 는 현재 곡이 없으면 그냥 송출을 멈추고 돌아갈 뿐 추천을 채우지 않는다
/// (`coordinator.rs` 의 "현재 곡 없음 → 송출 중지").
///
/// `lead` 는 §31 의 스킵 선행 시간이다. 모두가 같은 순간에 0초부터 출발하게 만든다.
/// 디스코드 쪽은 그 개념이 없으므로 `Duration::ZERO` 를 넘긴다.
///
/// **이미 다음 곡이 앉아 있으면 재생을 건드리지 않는다.** 그 경우 호출부가 이미
/// 동기화를 마쳤고, 여기서 또 `sync_guild` 를 부르면 방금 시작한 곡을 다시 만진다.
pub fn refill_after_skip(
    app: Arc<App>,
    coordinator: Arc<Coordinator>,
    guild_id: u64,
    lead: std::time::Duration,
) {
    tokio::spawn(async move {
        let state = app.player.get_state(guild_id).await;
        let already_playing_next = state.current_item.is_some();
        if already_playing_next {
            // 넘어간 자리에 이미 곡이 앉았다. 옛 곡 기준으로 풀어 둔 추천은 버린다 —
            // 새 현재 곡이 자기 것을 따로 채운다.
            app.player.clear_preview(guild_id);
        }
        ensure_autoplay(app.clone(), coordinator.clone(), guild_id, true).await;
        if !already_playing_next {
            // 자동재생이 채웠을 때만 맞춘다.
            coordinator.sync_guild(&app, guild_id).await;
            coordinator
                .schedule_start_in(guild_id, lead, std::time::Duration::ZERO)
                .await;
        }
        prefetch_next(app, coordinator, guild_id).await;
    });
}

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
    let _ = app
        .remote
        .block_autoplay_candidate(guild_id, &preview.track, Some(reason));
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
    /* **후보 목록까지 같이 버린다.**
     *
     * `take_preview` 만 하면 "안 고르면 나갈 곡" 한 자리만 비고, 옆에 놓인 나머지
     * 후보는 옛것 그대로 남는다. 다시 뽑았다고 했는데 화면에는 방금 본 곡이 그대로
     * 앉아 있고, 심지어 고를 수도 있는 상태다. */
    app.player.clear_preview(guild_id);
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
    // 서버마다 고른다 (§25). 기본은 갱신 — 곡마다 새 글을 쌓으면 채널이 금방 도배된다.
    let mode = app.remote.load_guild_settings(guild_id).now_playing_mode;
    if mode == crate::remote::NowPlayingMode::Off {
        return;
    }

    let state = app.player.get_state(guild_id).await;
    let embed = crate::commands::embeds::now_playing_embed(&state, &item, None);
    let components = crate::commands::embeds::playback_buttons_np(&state);
    let channel = serenity::model::id::ChannelId::new(channel_id);

    // 갱신 모드면 있던 카드를 고쳐 쓴다. **같은 채널일 때만** — 알림 채널이 바뀌었는데
    // 옛 채널의 카드를 고치면 지금 보고 있는 채널에는 아무것도 안 뜬다.
    if mode == crate::remote::NowPlayingMode::Edit {
        let previous = app.last_np_message.lock().unwrap().get(&guild_id).copied();
        if let Some((prev_ch, prev_msg)) = previous
            && prev_ch == channel_id
        {
            let edit = serenity::builder::EditMessage::new()
                .embed(embed.clone())
                .components(components.clone());
            match channel
                .edit_message(http, serenity::model::id::MessageId::new(prev_msg), edit)
                .await
            {
                Ok(_) => return,
                // 카드를 누가 지웠거나 권한이 없어졌다. **조용히 포기하지 않는다** —
                // 그러면 곡이 바뀌어도 채널에 아무 일도 안 일어나 고장으로 보인다.
                Err(error) => {
                    app.log.info(
                        "Bot",
                        &format!("재생 카드 갱신 실패({error}) — 새 카드로 보낼게요."),
                    );
                    app.last_np_message.lock().unwrap().remove(&guild_id);
                }
            }
        }
    }

    let builder = serenity::builder::CreateMessage::new()
        .embed(embed)
        .components(components);
    match channel.send_message(http, builder).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderKind;

    fn track(id: &str) -> TrackRef {
        TrackRef {
            provider: ProviderKind::YouTube,
            content_id: id.into(),
            source_url: format!("https://example.test/{id}"),
            title: Some(id.into()),
            artist: None,
            duration: None,
            variant_key: None,
            is_live: false,
        }
    }

    fn ids(tracks: &[TrackRef]) -> Vec<&str> {
        tracks.iter().map(|t| t.content_id.as_str()).collect()
    }

    /// 최근 N곡: 지금 나오는 곡이 맨 앞이고 중복은 안 들어간다 (§8.2).
    #[test]
    fn recent_seeds_start_from_the_current_song() {
        let current = track("지금곡");
        let recent = [track("지금곡"), track("최근1"), track("최근2"), track("최근3")];
        assert_eq!(
            ids(&recent_seeds(Some(&current), &recent, Some(3))),
            vec!["지금곡", "최근1", "최근2"]
        );
        // 지금 나오는 곡이 없어도(스킵 직후) 최근 곡만으로 시드를 만든다.
        assert_eq!(ids(&recent_seeds(None, &recent, Some(2))), vec!["지금곡", "최근1"]);
    }

    /// `0 = 무제한` (§23.1). 여기에 `.max(1)` 이 남아 있으면 최근 1곡만 보게 된다.
    #[test]
    fn recent_seeds_treat_zero_as_unlimited() {
        let current = track("지금곡");
        let recent: Vec<TrackRef> = (0..25).map(|i| track(&format!("최근{i}"))).collect();

        let unlimited = recent_seeds(Some(&current), &recent, None);
        assert_eq!(unlimited.len(), 26, "무제한인데 최근 목록이 잘렸다");
        assert_eq!(unlimited[0].content_id, "지금곡");

        // 값이 있으면 그 값이 그대로 상한이다.
        assert_eq!(recent_seeds(Some(&current), &recent, Some(1)).len(), 1);
        assert_eq!(recent_seeds(Some(&current), &recent, Some(5)).len(), 5);
        // 최근 목록이 상한보다 짧으면 있는 만큼만.
        assert_eq!(recent_seeds(None, &recent[..2], Some(20)).len(), 2);
    }

    /// 장르가 여러 개면 장르도 라운드로빈이다 (§8.2).
    /// 이어 붙이기만 하면 시드 커서가 앞 장르에서만 돈다.
    #[test]
    fn genre_seeds_interleave_between_charts() {
        let kpop = vec![track("K1"), track("K2"), track("K3")];
        let rock = vec![track("R1"), track("R2")];
        assert_eq!(
            ids(&genre_seeds(vec![kpop, rock])),
            vec!["K1", "R1", "K2", "R2", "K3"]
        );

        // 장르가 하나면 순서 그대로.
        assert_eq!(
            ids(&genre_seeds(vec![vec![track("K1"), track("K2")]])),
            vec!["K1", "K2"]
        );
        // 캐시가 하나도 없으면 빈 목록 → 폴백 사슬이 내려간다.
        assert!(genre_seeds(Vec::new()).is_empty());
        assert!(genre_seeds(vec![Vec::new(), Vec::new()]).is_empty());
    }

    /// 두 장르에 같은 곡이 있으면 시드가 그 곡으로 쏠린다 — 한 번만 넣는다.
    #[test]
    fn genre_seeds_drop_duplicates_across_charts() {
        let a = vec![track("겹침"), track("A2")];
        let b = vec![track("겹침"), track("B2")];
        assert_eq!(ids(&genre_seeds(vec![a, b])), vec!["겹침", "A2", "B2"]);
    }
}

#[cfg(test)]
mod skip_settlement_tests {
    /// **회귀 가드: 스킵 경로가 둘인데 한쪽만 자동재생을 보충하던 문제.**
    ///
    /// 디스코드 스킵(`/스킵`·버튼)은 `settle_manual_skip` → `ensure_autoplay` 를 거쳤는데
    /// 웹 리모컨 스킵만 안 거쳤다. 그래서 대기열이 빈 상태에서 리모컨으로 넘기면
    /// 다음 추천곡이 화면에 떠 있는데도 침묵으로 끝났다.
    ///
    /// 구현이 둘로 갈린 것이 원인이라, **양쪽이 보충을 부르는지**를 소스에서 직접 본다.
    /// 동작 테스트로는 못 잡는다 — 한쪽만 지워도 다른 쪽 테스트는 그대로 통과한다.
    #[test]
    fn both_skip_paths_refill_autoplay() {
        let web = include_str!("../web/remote.rs");
        let discord = include_str!("../commands/handlers.rs");

        // 웹은 api_control 의 "skip" 갈래에서 보충을 부른다.
        assert!(
            web.contains("refill_after_skip"),
            "웹 스킵이 자동재생 보충을 안 부른다 — 큐가 빈 채로 리모컨에서 넘기면 침묵으로 끝난다"
        );
        // 디스코드는 settle_manual_skip 을 거쳐 ensure_autoplay 를 부른다.
        assert!(
            discord.contains("settle_manual_skip"),
            "디스코드 스킵이 뒷정리를 안 거친다"
        );
        assert!(
            discord.contains("ensure_autoplay"),
            "디스코드 스킵 뒷정리가 자동재생 보충을 안 부른다"
        );

        // 웹의 스킵 갈래 안에 실제로 들어 있는지 (파일 어딘가가 아니라).
        let arm = web
            .split("\"skip\" => {")
            .nth(1)
            .expect("api_control 에 skip 갈래가 있어야 한다");
        let arm = &arm[..arm.len().min(1400)];
        /* **주석을 걷어내고 본다.** 바로 위에 `refill_after_skip` 을 언급하는 설명
         * 주석이 있어서, 그냥 `contains` 만 하면 호출을 지우고 주석만 남겨도 통과한다.
         * 실제로 이 테스트의 첫 판이 그랬다. */
        let code: String = arm
            .lines()
            .map(str::trim_start)
            .filter(|line| {
                !line.starts_with("//") && !line.starts_with('*') && !line.starts_with("/*")
            })
            .collect::<Vec<_>>()
            .join("
");
        assert!(
            code.contains("refill_after_skip("),
            "skip 갈래의 **코드**에서 보충을 불러야 한다 (주석만으로는 안 된다)"
        );
    }
}
