//! 길드 재생 상태기계 — C# GuildPlayerManager 1:1 포팅.
//! 반복/셔플/CycleHistory/자동추천 시드 규칙/최근기록(25개 상한) 의미론을 그대로 유지한다.

use crate::db::Db;
use crate::logging::LogService;
use crate::models::*;
use crate::remote::ranking::{self, sort_queue, wait_score_targets};
use crate::remote::{QueueSortMode, RemoteStore};
use rand::seq::SliceRandom;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

pub struct PlayerManager {
    db: Arc<Db>,
    remote: Arc<RemoteStore>,
    log: Arc<LogService>,
    gate: Mutex<()>,
    previews: StdMutex<HashMap<u64, QueueItem>>,
    preview_inflight: StdMutex<HashSet<u64>>,
    /// 길드별 대기열 정렬 모드 캐시. 정렬은 5초마다·모든 상태 변경마다 돌기 때문에
    /// 매번 설정 JSON을 읽으면 유휴 상태에서도 쿼리가 계속 나간다(사양서 §5.2 H).
    /// 웹이 모드를 바꾸면 `set_sort_mode`/`invalidate_sort_mode`로 갱신한다.
    sort_modes: StdMutex<HashMap<u64, QueueSortMode>>,
    /// 길드별 셔플 시드. 셔플은 별도 정렬 모드가 아니라 `Fifo` + 무작위 `original_order`이며
    /// (사양서 §3.3), 그 무작위 순서를 시드 하나로 재현한다.
    shuffle_seeds: StdMutex<HashMap<u64, u64>>,
}

/// `/재생` 응답의 ✖ 취소 버튼 결과 — 호출 측이 코디네이터를 어떻게 정리할지 결정한다.
pub enum CancelOutcome {
    /// 대기 중이던 곡을 큐에서 제거 (현재 재생엔 영향 없음).
    RemovedUpcoming(String),
    /// 이미 재생 중이던 곡이라 스킵과 동일하게 다음 곡으로 전이.
    SkippedCurrent(String),
    /// 이미 재생이 끝났거나 큐에 없어 취소할 대상이 없음.
    NotFound,
}

impl PlayerManager {
    pub fn new(db: Arc<Db>, remote: Arc<RemoteStore>, log: Arc<LogService>) -> PlayerManager {
        PlayerManager {
            db,
            remote,
            log,
            gate: Mutex::new(()),
            previews: StdMutex::new(HashMap::new()),
            preview_inflight: StdMutex::new(HashSet::new()),
            sort_modes: StdMutex::new(HashMap::new()),
            shuffle_seeds: StdMutex::new(HashMap::new()),
        }
    }

    pub fn effective_settings(&self, guild_id: u64) -> EffectiveGuildSettings {
        let global = self.db.load_global_settings();
        let guild = self.db.load_guild_settings(guild_id);
        let remote = self.remote.load_guild_settings(guild_id);
        EffectiveGuildSettings {
            effective_volume: guild
                .volume_override
                .unwrap_or(global.master_volume)
                .clamp(remote.min_volume, remote.max_volume),
            normalize_enabled: guild
                .normalize_enabled_override
                .unwrap_or(global.normalize_enabled),
            autoplay_default: guild
                .autoplay_default_override
                .unwrap_or(global.autoplay_default),
        }
    }

    pub async fn get_state(&self, guild_id: u64) -> GuildPlayerState {
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
        self.prepare_scored_queue(&mut state);
        self.attach_preview(&mut state);
        state
    }

    /// 상태 변경의 공통 패턴: 잠금 → 로드 → 변형 → 저장 → 로그.
    async fn mutate<F>(
        &self,
        guild_id: u64,
        category: &str,
        log_msg: &str,
        f: F,
    ) -> GuildPlayerState
    where
        F: FnOnce(&mut GuildPlayerState),
    {
        let _g = self.gate.lock().await;
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
        self.prepare_scored_queue(&mut state);
        f(&mut state);
        self.prepare_scored_queue(&mut state);
        self.db.save_guild_state(&state);
        self.log.info(category, log_msg);
        self.attach_preview(&mut state);
        state
    }

    // ───────── 음성 채널 바인딩 ─────────

    pub async fn connect_voice(&self, guild_id: u64, channel_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Voice",
            &format!("Bound voice channel {channel_id} for guild {guild_id}."),
            |s| {
                s.voice_channel_id = Some(channel_id);
            },
        )
        .await
    }

    pub async fn disconnect_voice(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Voice",
            &format!("Unbound voice channel for guild {guild_id}."),
            |s| {
                s.voice_channel_id = None;
            },
        )
        .await
    }

    // ───────── 큐 조작 ─────────

    pub async fn enqueue(
        &self,
        guild_id: u64,
        item: QueueItem,
        priority: bool,
    ) -> GuildPlayerState {
        if item.request_kind == PlaybackRequestKind::User {
            self.clear_preview(guild_id);
        }
        let title = item.track.display_title().to_string();
        self.mutate(
            guild_id,
            "Queue",
            &format!("Queued {title} for guild {guild_id}."),
            move |s| {
                if priority {
                    s.upcoming.insert(0, item);
                } else {
                    s.upcoming.push(item);
                }
                promote_if_idle(s);
            },
        )
        .await
    }

    /// 바로재생: 반복 Off 면 큐 폐기, 반복 On 이면 사이클 끝으로 보존 후 즉시 재생.
    pub async fn play_now(&self, guild_id: u64, item: QueueItem) -> GuildPlayerState {
        if item.request_kind == PlaybackRequestKind::User {
            self.clear_preview(guild_id);
        }
        let title = item.track.display_title().to_string();
        self.mutate(
            guild_id,
            "Queue",
            &format!("PlayNow {title} for guild {guild_id}."),
            move |s| {
                if s.repeat_mode == RepeatMode::Queue {
                    // 큐 반복일 때만 사이클에 보존 (Track/Off 는 cycle_history 를 읽지 않음).
                    if let Some(cur) = s.current_item.take() {
                        s.cycle_history.push(clone_item(&cur));
                    }
                    for pending in s.upcoming.drain(..) {
                        s.cycle_history.push(clone_item(&pending));
                    }
                } else {
                    s.upcoming.clear();
                }
                s.current_item = None;
                s.is_paused = false;
                s.upcoming.insert(0, item);
                promote_if_idle(s);
            },
        )
        .await
    }

    /// 다시 섞기. 시드를 새로 뽑으면 정렬이 그 시드대로 순서를 다시 만들어 준다.
    pub async fn shuffle(&self, guild_id: u64) -> GuildPlayerState {
        self.reseed_shuffle(guild_id);
        self.mutate(
            guild_id,
            "Queue",
            &format!("Shuffled queue for guild {guild_id}."),
            |s| {
                s.shuffle_enabled = true;
            },
        )
        .await
    }

    /// 셔플 모드 토글용. 켤 때 새 시드를 뽑고, 끌 때는 시드를 버려 등록 순서로 돌아간다.
    /// 버튼이 셔플을 on/off 토글로 표시하므로 필요.
    pub async fn set_shuffle(&self, guild_id: u64, enabled: bool) -> GuildPlayerState {
        if enabled {
            self.reseed_shuffle(guild_id);
        } else {
            self.shuffle_seeds.lock().unwrap().remove(&guild_id);
        }
        self.mutate(
            guild_id,
            "Queue",
            &format!("Shuffle {enabled} for guild {guild_id}."),
            |s| {
                s.shuffle_enabled = enabled;
            },
        )
        .await
    }

    pub async fn move_item(
        &self,
        guild_id: u64,
        from: usize,
        to: usize,
    ) -> Result<GuildPlayerState, String> {
        let _g = self.gate.lock().await;
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
        if from >= state.upcoming.len() || to >= state.upcoming.len() {
            return Err(format!(
                "순번이 대기열 범위를 벗어났습니다 (1~{}).",
                state.upcoming.len()
            ));
        }
        let item = state.upcoming.remove(from);
        state.upcoming.insert(to, item);
        self.db.save_guild_state(&state);
        self.log.info(
            "Queue",
            &format!("Moved {from}->{to} for guild {guild_id}."),
        );
        Ok(state)
    }

    pub async fn remove_upcoming(
        &self,
        guild_id: u64,
        index: usize,
    ) -> Result<GuildPlayerState, String> {
        let _g = self.gate.lock().await;
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
        if index >= state.upcoming.len() {
            return Err(format!(
                "순번이 대기열 범위를 벗어났습니다 (1~{}).",
                state.upcoming.len()
            ));
        }
        let removed = state.upcoming.remove(index);
        let _ = self.remote.clear_item_runtime(&removed.id);
        self.db.save_guild_state(&state);
        self.log.info(
            "Queue",
            &format!(
                "Removed {} for guild {guild_id}.",
                removed.track.display_title()
            ),
        );
        Ok(state)
    }

    /// 특정 QueueItem 을 id 로 취소한다. 대기열에 있으면 제거, 현재 재생 중이면 스킵.
    /// (✖ 취소 버튼 전용 — 순번 대신 id 로 찾으므로 그 사이 큐가 바뀌어도 정확히 그 곡만 취소.)
    pub async fn cancel_by_id(&self, guild_id: u64, item_id: &str) -> CancelOutcome {
        let _g = self.gate.lock().await;
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);

        if let Some(pos) = state.upcoming.iter().position(|i| i.id == item_id) {
            let removed = state.upcoming.remove(pos);
            let _ = self.remote.clear_item_runtime(&removed.id);
            self.db.save_guild_state(&state);
            let title = removed.track.display_title().to_string();
            self.log.info(
                "Queue",
                &format!("Cancelled queued {title} for guild {guild_id}."),
            );
            return CancelOutcome::RemovedUpcoming(title);
        }

        if state.current_item.as_ref().map(|c| c.id.as_str()) == Some(item_id) {
            // skip() 과 동일한 전이 — 큐 반복 사이클 보존 포함.
            let title = state
                .current_item
                .as_ref()
                .map(|c| c.track.display_title().to_string())
                .unwrap_or_default();
            if let Some(cur) = state.current_item.take() {
                let _ = self.remote.clear_item_runtime(&cur.id);
                if state.repeat_mode == RepeatMode::Queue {
                    state.cycle_history.push(clone_item(&cur));
                }
                push_recent(&mut state, cur.track.clone());
            }
            if state.upcoming.is_empty()
                && state.repeat_mode == RepeatMode::Queue
                && !state.cycle_history.is_empty()
            {
                let mut next_cycle: Vec<QueueItem> =
                    state.cycle_history.iter().map(clone_item).collect();
                state.cycle_history.clear();
                if state.shuffle_enabled {
                    shuffle_upcoming(&mut next_cycle);
                }
                state.upcoming.append(&mut next_cycle);
            }
            promote_if_idle(&mut state);
            self.db.save_guild_state(&state);
            self.log.info(
                "Queue",
                &format!("Cancelled current {title} (skipped) for guild {guild_id}."),
            );
            return CancelOutcome::SkippedCurrent(title);
        }

        CancelOutcome::NotFound
    }

    pub async fn clear_queue(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Queue",
            &format!("Cleared queue for guild {guild_id}."),
            |s| {
                s.upcoming.clear();
                // 큐 반복 사이클 원본도 비운다 — 안 비우면 다음 곡 종료 시 cycle_history 가
                // 다시 채워져 "비웠는데 곡이 되살아나는" 현상이 생긴다(2026-06-17 감사).
                s.cycle_history.clear();
            },
        )
        .await
    }

    pub async fn previous(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Returned to the previous track for guild {guild_id}."),
            |s| {
                if s.recent_tracks.is_empty() {
                    return;
                }
                let prev = s.recent_tracks.remove(0);
                if let Some(cur) = s.current_item.take() {
                    s.upcoming.insert(0, cur);
                }
                let mut item = QueueItem::new_user(prev, "(이전 곡)".into(), None);
                item.request_kind = PlaybackRequestKind::User;
                s.current_item = Some(item);
            },
        )
        .await
    }

    pub async fn skip_to(
        &self,
        guild_id: u64,
        position: usize,
    ) -> Result<GuildPlayerState, String> {
        {
            let _g = self.gate.lock().await;
            let eff = self.effective_settings(guild_id);
            let mut state =
                self.db
                    .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
            if position < 1 || position > state.upcoming.len() {
                return Err(format!(
                    "순번이 대기열 범위를 벗어났습니다 (1~{}).",
                    state.upcoming.len()
                ));
            }
            // 방금 재생하던 곡을 먼저 사이클에 보존(재생 순서 유지), 그 다음 건너뛴 곡들을 보존.
            if let Some(cur) = state.current_item.take() {
                if state.repeat_mode == RepeatMode::Queue {
                    state.cycle_history.push(clone_item(&cur));
                }
                push_recent(&mut state, cur.track.clone());
            }
            if position > 1 {
                if state.repeat_mode == RepeatMode::Queue {
                    // 큐 반복: 건너뛴 곡도 사이클에 남겨 다음 바퀴에 다시 나오게 한다
                    // (그냥 drain 하면 루프에서 영영 사라진다 — 2026-06-17 감사).
                    for it in state.upcoming.drain(0..position - 1) {
                        state.cycle_history.push(it);
                    }
                } else {
                    // Off/Track: 건너뛴 곡은 사용자의 의도대로 폐기.
                    state.upcoming.drain(0..position - 1);
                }
            }
            promote_if_idle(&mut state);
            self.db.save_guild_state(&state);
            self.log.info(
                "Queue",
                &format!("Skipped to position {position} for guild {guild_id}."),
            );
        }
        Ok(self.get_state(guild_id).await)
    }

    // ───────── 재생 제어 ─────────

    pub async fn pause(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Paused guild {guild_id}."),
            |s| s.is_paused = true,
        )
        .await
    }

    pub async fn resume(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Resumed guild {guild_id}."),
            |s| {
                s.is_paused = false;
                promote_if_idle(s);
            },
        )
        .await
    }

    pub async fn stop(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Stopped playback for guild {guild_id}."),
            |s| {
                s.is_paused = false;
                s.current_item = None;
                s.upcoming.clear();
                s.cycle_history.clear();
            },
        )
        .await
    }

    /// 곡 종료 후 다음 상태로 진행 (반복 의미론 포함).
    pub async fn advance(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Advanced playback for guild {guild_id}."),
            |s| {
                if s.repeat_mode != RepeatMode::Track {
                    if let Some(current) = &s.current_item {
                        self.mark_played(guild_id, current);
                        let _ = self.remote.record_recent(guild_id, current, "completed");
                        let _ = self.remote.clear_item_runtime(&current.id);
                    }
                    self.age_wait_scores(guild_id, s);
                    self.sort_scored_queue(s);
                }
                advance_unsafe(s);
            },
        )
        .await
    }

    pub async fn skip(&self, guild_id: u64) -> GuildPlayerState {
        // C#: Skip = Advance 와 동일 전이 (Track 반복이어도 강제 전진).
        self.mutate(
            guild_id,
            "Playback",
            &format!("Advanced playback for guild {guild_id}."),
            |s| {
                if let Some(current) = &s.current_item {
                    self.mark_played(guild_id, current);
                    let _ = self.remote.record_recent(guild_id, current, "skipped");
                    let _ = self.remote.clear_item_runtime(&current.id);
                }
                self.age_wait_scores(guild_id, s);
                self.sort_scored_queue(s);
                if let Some(cur) = s.current_item.take() {
                    if s.repeat_mode == RepeatMode::Queue {
                        s.cycle_history.push(clone_item(&cur));
                    }
                    push_recent(s, cur.track.clone());
                }
                if s.upcoming.is_empty()
                    && s.repeat_mode == RepeatMode::Queue
                    && !s.cycle_history.is_empty()
                {
                    let mut next_cycle: Vec<QueueItem> =
                        s.cycle_history.iter().map(clone_item).collect();
                    s.cycle_history.clear();
                    if s.shuffle_enabled {
                        shuffle_upcoming(&mut next_cycle);
                    }
                    s.upcoming.append(&mut next_cycle);
                }
                promote_if_idle(s);
            },
        )
        .await
    }

    pub async fn set_repeat(&self, guild_id: u64, mode: RepeatMode) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Repeat mode {:?} for guild {guild_id}.", mode),
            |s| {
                s.repeat_mode = mode;
                // 큐 반복을 벗어나면 사이클 원본을 비운다 — 안 그러면 Off/Track 동안 쌓인
                // (그리고 다시 Queue 로 돌아오면 되살아나는) stale 사이클이 남는다.
                if mode != RepeatMode::Queue {
                    s.cycle_history.clear();
                }
            },
        )
        .await
    }

    pub async fn set_autoplay(&self, guild_id: u64, enabled: bool) -> GuildPlayerState {
        if !enabled {
            self.clear_preview(guild_id);
        }
        self.mutate(
            guild_id,
            "Playback",
            &format!("Autoplay {enabled} for guild {guild_id}."),
            |s| {
                s.autoplay_enabled = enabled;
                // 자동추천을 끄면 아직 재생 안 한 추천 곡을 큐에서 제거한다(현재 곡은 그대로).
                // 안 그러면 "껐는데 봇 추천곡이 한 곡 더 재생"되는 현상이 생긴다(2026-06-17 감사).
                if !enabled {
                    s.upcoming
                        .retain(|i| i.request_kind != PlaybackRequestKind::Autoplay);
                }
            },
        )
        .await
    }

    pub async fn set_volume(&self, guild_id: u64, volume: i32) -> GuildPlayerState {
        let remote = self.remote.load_guild_settings(guild_id);
        let v = volume.clamp(remote.min_volume, remote.max_volume);
        self.mutate(
            guild_id,
            "Playback",
            &format!("Set effective volume for guild {guild_id} to {v}."),
            |s| {
                s.effective_volume = v;
            },
        )
        .await
    }

    pub async fn set_current_start_offset(
        &self,
        guild_id: u64,
        offset: CsTimeSpan,
    ) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Playback",
            &format!(
                "Set resume offset {} for guild {guild_id}.",
                offset.display()
            ),
            |s| {
                if let Some(cur) = s.current_item.as_mut() {
                    cur.start_offset = offset;
                }
            },
        )
        .await
    }

    pub async fn apply_configured_settings(&self, guild_id: u64) -> GuildPlayerState {
        let eff = self.effective_settings(guild_id);
        self.mutate(
            guild_id,
            "Settings",
            &format!("Applied configured settings for guild {guild_id}."),
            move |s| {
                s.effective_volume = eff.effective_volume;
            },
        )
        .await
    }

    pub async fn prune_recent_tracks<F>(&self, guild_id: u64, should_remove: F) -> GuildPlayerState
    where
        F: Fn(&TrackRef) -> bool,
    {
        self.mutate(
            guild_id,
            "Playback",
            &format!("Pruned recent tracks for guild {guild_id}."),
            |s| {
                s.recent_tracks.retain(|t| !should_remove(t));
            },
        )
        .await
    }

    // ───────── autoplay preview (휘발성) ─────────

    pub fn get_preview(&self, guild_id: u64) -> Option<QueueItem> {
        self.previews.lock().unwrap().get(&guild_id).cloned()
    }

    pub fn set_preview(&self, guild_id: u64, item: QueueItem) {
        self.previews.lock().unwrap().insert(guild_id, item);
    }

    pub fn clear_preview(&self, guild_id: u64) {
        self.previews.lock().unwrap().remove(&guild_id);
    }

    pub fn take_preview(&self, guild_id: u64) -> Option<QueueItem> {
        self.previews.lock().unwrap().remove(&guild_id)
    }

    pub fn try_begin_preview_resolve(&self, guild_id: u64) -> bool {
        self.preview_inflight.lock().unwrap().insert(guild_id)
    }

    pub fn end_preview_resolve(&self, guild_id: u64) {
        self.preview_inflight.lock().unwrap().remove(&guild_id);
    }

    /// preview 추천이 진행 중인가 (스킵 직후 ensure_autoplay 가 중복 추천 대신 이걸 기다리도록).
    pub fn is_preview_resolving(&self, guild_id: u64) -> bool {
        self.preview_inflight.lock().unwrap().contains(&guild_id)
    }

    pub fn has_preview(&self, guild_id: u64) -> bool {
        self.previews.lock().unwrap().contains_key(&guild_id)
    }

    fn attach_preview(&self, state: &mut GuildPlayerState) {
        let previews = self.previews.lock().unwrap();
        if let Some(p) = previews.get(&state.guild_id) {
            if state.upcoming.is_empty() {
                state.autoplay_preview = Some(p.clone());
            }
        }
    }

    /// 자동추천 시드 가능 여부 (C# ShouldSeedAutoplay).
    pub fn should_seed_autoplay(state: &GuildPlayerState, allow_continuation: bool) -> bool {
        if !state.autoplay_enabled || state.repeat_mode != RepeatMode::Off {
            return false;
        }
        if state
            .upcoming
            .iter()
            .any(|i| i.request_kind == PlaybackRequestKind::Autoplay)
        {
            return false;
        }
        if state.current_item.is_some() && state.upcoming.len() == 1 {
            return true;
        }
        allow_continuation && state.current_item.is_none() && state.upcoming.is_empty()
    }

    /// preview 채움 가능 여부 (C# ShouldFillAutoplayPreview).
    pub fn should_fill_preview(&self, state: &GuildPlayerState) -> bool {
        if !state.autoplay_enabled || state.repeat_mode != RepeatMode::Off {
            return false;
        }
        if !state.upcoming.is_empty() {
            return false;
        }
        !self.has_preview(state.guild_id)
    }

    /// 추천 제외 키 집합 (현재+대기열+최근).
    pub fn excluded_keys(state: &GuildPlayerState) -> HashSet<String> {
        let mut set = HashSet::new();
        if let Some(c) = &state.current_item {
            set.insert(c.track.cache_key());
        }
        for i in &state.upcoming {
            set.insert(i.track.cache_key());
        }
        for t in &state.recent_tracks {
            set.insert(t.cache_key());
        }
        set
    }

    /// autoplay 후보를 큐에 채운다 (preview 우선, 조건 재검사 포함).
    pub async fn seed_autoplay_item(
        &self,
        guild_id: u64,
        item: QueueItem,
        allow_continuation: bool,
    ) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Queue",
            &format!("Seeded autoplay candidate for guild {guild_id}."),
            move |s| {
                if Self::should_seed_autoplay(s, allow_continuation) {
                    s.upcoming.push(item);
                    promote_if_idle(s);
                }
            },
        )
        .await
    }

    /// 외부 웹이 관리자 강제 이동을 적용한다. 큰 우선순위일수록 먼저 재생된다.
    pub async fn set_manual_priority(
        &self,
        guild_id: u64,
        item_id: &str,
        priority: Option<i32>,
    ) -> Result<GuildPlayerState, String> {
        self.remote
            .set_manual_priority(guild_id, item_id, priority)
            .map_err(|error| error.to_string())?;
        Ok(self.refresh_scored_order(guild_id).await)
    }

    /// 투표/관리자 우선순위 변경 직후 정렬 결과를 영속 상태에도 반영한다.
    pub async fn refresh_scored_order(&self, guild_id: u64) -> GuildPlayerState {
        let _g = self.gate.lock().await;
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
        self.prepare_scored_queue(&mut state);
        self.db.save_guild_state(&state);
        self.attach_preview(&mut state);
        state
    }

    /// 5초 주기 재정렬 태스크용(사양서 §3.3). 순서가 실제로 바뀐 길드만 저장하고 그 사실을 알린다.
    /// 유휴 길드에서 쓰기 쿼리와 브로드캐스트가 발생하지 않아야 하므로(§5.2 H) 항상 저장하지 않는다.
    pub async fn resort_if_changed(&self, guild_id: u64) -> bool {
        let _g = self.gate.lock().await;
        let eff = self.effective_settings(guild_id);
        let mut state =
            self.db
                .load_guild_state(guild_id, eff.effective_volume, eff.autoplay_default);
        if state.upcoming.len() < 2 {
            return false; // 바꿀 순서가 없다 — 로드만 하고 끝낸다.
        }
        let before: Vec<String> = state.upcoming.iter().map(|i| i.id.clone()).collect();
        self.prepare_scored_queue(&mut state);
        let changed = state
            .upcoming
            .iter()
            .map(|i| i.id.as_str())
            .ne(before.iter().map(String::as_str));
        if changed {
            self.db.save_guild_state(&state);
        }
        changed
    }

    // ───────── 정렬 모드 ─────────

    /// 길드의 정렬 모드. 캐시가 비었을 때만 설정 JSON을 읽는다.
    pub fn sort_mode(&self, guild_id: u64) -> QueueSortMode {
        if let Some(mode) = self.sort_modes.lock().unwrap().get(&guild_id) {
            return *mode;
        }
        // 설정 조회는 remote 커넥션 뮤텍스를 잡으므로 캐시 락을 놓은 뒤에 읽는다.
        let mode = self.remote.load_guild_settings(guild_id).sort_mode;
        self.sort_modes.lock().unwrap().insert(guild_id, mode);
        mode
    }

    /// 웹이 서버 관리 콘솔에서 모드를 저장한 직후 호출한다. 저장은 웹이 하고 여기선 캐시만 맞춘다.
    pub fn set_sort_mode(&self, guild_id: u64, mode: QueueSortMode) {
        self.sort_modes.lock().unwrap().insert(guild_id, mode);
    }

    /// 설정을 통째로 덮어써서 모드를 모를 때 쓰는 무효화. 다음 정렬에서 한 번만 DB를 읽는다.
    pub fn invalidate_sort_mode(&self, guild_id: u64) {
        self.sort_modes.lock().unwrap().remove(&guild_id);
    }

    /// 셔플 시드를 새로 뽑는다(= 다시 섞기).
    fn reseed_shuffle(&self, guild_id: u64) {
        self.shuffle_seeds
            .lock()
            .unwrap()
            .insert(guild_id, rand::random::<u64>());
    }

    /// 셔플이 켜져 있는 동안 쓸 시드. 봇 재시작 등으로 비어 있으면 즉석에서 하나 뽑는다.
    fn shuffle_seed(&self, guild_id: u64) -> u64 {
        *self
            .shuffle_seeds
            .lock()
            .unwrap()
            .entry(guild_id)
            .or_insert_with(rand::random::<u64>)
    }

    /// 점수제에서만 대기 점수를 올린다. 공평제는 대기 점수를 순서에 쓰지 않고(사양서 §3.1),
    /// 시간제는 아예 점수를 보지 않으므로 곡 경계마다 쓰기 쿼리를 낼 이유가 없다.
    fn age_wait_scores(&self, guild_id: u64, state: &GuildPlayerState) {
        if self.sort_mode(guild_id) != QueueSortMode::Score {
            return;
        }
        let targets = wait_score_targets(&state.upcoming);
        let _ = self.remote.increment_wait_scores(&targets);
    }

    /// 곡 하나가 끝났음을 신청자에게 기록한다 — 대기 점수 0 초기화 + 마지막 재생 시각 갱신.
    /// 공평제의 라운드는 이 시각으로 돌기 때문에, 빠지면 같은 사람 곡만 계속 나간다.
    fn mark_played(&self, guild_id: u64, finished: &QueueItem) {
        let Some(user_id) = finished.requested_by_user_id else {
            return;
        };
        let _ = self.remote.mark_requester_played(guild_id, user_id);
    }

    fn prepare_scored_queue(&self, state: &mut GuildPlayerState) {
        let mut items =
            Vec::with_capacity(state.upcoming.len() + usize::from(state.current_item.is_some()));
        if let Some(current) = &state.current_item {
            items.push(current.clone());
        }
        items.extend(state.upcoming.iter().cloned());
        let _ = self.remote.ensure_queue_items(state.guild_id, &items);
        self.sort_scored_queue(state);
    }

    fn sort_scored_queue(&self, state: &mut GuildPlayerState) {
        // 셔플은 별도 모드가 아니라 `Fifo` + 무작위 `original_order`다(사양서 §3.3).
        // 예전처럼 여기서 조기 반환하면 셔플을 켜는 순간 랭킹·수동 우선순위가 통째로 죽었다.
        let mode = if state.shuffle_enabled {
            QueueSortMode::Fifo
        } else {
            self.sort_mode(state.guild_id)
        };
        let mut scores = self.remote.queue_scores(state.guild_id);
        if state.shuffle_enabled {
            let seed = self.shuffle_seed(state.guild_id);
            for (item_id, score) in scores.iter_mut() {
                score.original_order = shuffled_order(seed, item_id);
            }
        }

        // 라운드는 정렬 입력이자 화면 표시값이다. 메모리에서 계산해 두고(쿼리 0회),
        // 실제로 달라진 항목만 저장해 유휴 상태에서 쓰기 쿼리가 나가지 않게 한다.
        let persisted: HashMap<String, i32> = scores
            .iter()
            .map(|(item_id, score)| (item_id.clone(), score.round))
            .collect();
        ranking::apply_rounds(&state.upcoming, &mut scores);
        let stale: HashMap<String, i32> = scores
            .iter()
            .filter(|(item_id, score)| persisted.get(*item_id) != Some(&score.round))
            .map(|(item_id, score)| (item_id.clone(), score.round))
            .collect();
        if !stale.is_empty() {
            let _ = self.remote.save_queue_rounds(state.guild_id, &stale);
        }

        sort_queue(&mut state.upcoming, &scores, mode);
    }
}

// ───────── 공용 전이 함수 ─────────

fn clone_item(item: &QueueItem) -> QueueItem {
    let mut c = item.clone();
    c.id = uuid_like();
    c.requested_at = chrono::Utc::now().to_rfc3339();
    c
}

fn promote_if_idle(state: &mut GuildPlayerState) {
    if state.current_item.is_some() || state.upcoming.is_empty() {
        return;
    }
    state.current_item = Some(state.upcoming.remove(0));
}

fn push_recent(state: &mut GuildPlayerState, track: TrackRef) {
    state.recent_tracks.insert(0, track);
    state.recent_tracks.truncate(25);
}

fn shuffle_upcoming(items: &mut Vec<QueueItem>) {
    let mut rng = rand::rng();
    items.shuffle(&mut rng);
}

/// 셔플 순서를 시드 하나로 재현한다. 같은 시드면 항상 같은 순서라 5초마다 재정렬해도
/// 큐가 요동치지 않고, 뒤늦게 신청된 곡도 무작위 위치에 자연스럽게 끼어든다.
fn shuffled_order(seed: u64, item_id: &str) -> i64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    item_id.hash(&mut hasher);
    // 최상위 비트를 버려 항상 양수로 만든다(등록순 값과 같은 부호 영역에 둔다).
    (hasher.finish() >> 1) as i64
}

fn advance_unsafe(state: &mut GuildPlayerState) {
    if state.current_item.is_some() && state.repeat_mode == RepeatMode::Track {
        return; // Track 반복: 같은 곡 재시작.
    }
    if let Some(cur) = state.current_item.take() {
        // 큐 반복일 때만 사이클에 보존 (Off/Track 은 cycle_history 를 읽지 않아 무한 증가만 됨).
        if state.repeat_mode == RepeatMode::Queue {
            state.cycle_history.push(clone_item(&cur));
        }
        push_recent(state, cur.track.clone());
    }
    if state.upcoming.is_empty()
        && state.repeat_mode == RepeatMode::Queue
        && !state.cycle_history.is_empty()
    {
        let mut next_cycle: Vec<QueueItem> = state.cycle_history.iter().map(clone_item).collect();
        state.cycle_history.clear();
        if state.shuffle_enabled {
            shuffle_upcoming(&mut next_cycle);
        }
        state.upcoming.append(&mut next_cycle);
    }
    promote_if_idle(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 SQLite 두 개(레거시 Db + RemoteStore)를 같은 파일에 여는 운영 구성 그대로 만든다.
    fn temp_player(tag: &str) -> (PlayerManager, Arc<RemoteStore>, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("macham-player-{tag}-{}", uuid_like()));
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("musicbot.sqlite");
        let db = Arc::new(Db::open(&db_path).unwrap());
        let remote = Arc::new(RemoteStore::open(&db_path).unwrap());
        let log = Arc::new(LogService::new(root.join("logs")));
        (PlayerManager::new(db, remote.clone(), log), remote, root)
    }

    fn cleanup(player: PlayerManager, remote: Arc<RemoteStore>, root: std::path::PathBuf) {
        drop(player);
        drop(remote);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 항목 id 를 곡 이름으로 고정해 단언문이 읽히게 한다.
    fn user_item(content_id: &str, user_id: u64) -> QueueItem {
        let mut item = QueueItem::new_user(
            TrackRef {
                provider: ProviderKind::YouTube,
                content_id: content_id.into(),
                source_url: format!("https://example.test/{content_id}"),
                title: Some(content_id.into()),
                artist: None,
                duration: None,
                variant_key: None,
            },
            format!("user-{user_id}"),
            Some(user_id),
        );
        item.id = content_id.into();
        item
    }

    fn queue_ids(state: &GuildPlayerState) -> Vec<&str> {
        state.upcoming.iter().map(|item| item.id.as_str()).collect()
    }

    fn current_id(state: &GuildPlayerState) -> &str {
        state
            .current_item
            .as_ref()
            .map(|item| item.id.as_str())
            .unwrap_or("")
    }

    /// 민수(1) 3곡 뒤에 지훈(2) 1곡. 공평제·점수제 테스트가 공유하는 시나리오.
    async fn seed_two_requesters(player: &PlayerManager, guild_id: u64) {
        for content_id in ["민수1", "민수2", "민수3"] {
            player.enqueue(guild_id, user_item(content_id, 1), false).await;
        }
        player.enqueue(guild_id, user_item("지훈1", 2), false).await;
    }

    /// 공평제 라운드가 실제로 돈다 — 민수 곡이 하나 끝나면 아직 한 곡도 못 튼 지훈이 먼저 나간다.
    /// `mark_requester_played` 가 빠지면 민수 곡만 끝까지 나가므로 이 테스트가 잡아낸다.
    #[tokio::test]
    async fn fair_mode_rotates_requesters_after_each_song() {
        let (player, remote, root) = temp_player("fair");
        let guild_id = 1;
        player.set_sort_mode(guild_id, QueueSortMode::Fair);
        seed_two_requesters(&player, guild_id).await;

        let state = player.get_state(guild_id).await;
        assert_eq!(current_id(&state), "민수1");
        // 이미 1라운드가 적용된다: 민수의 2번째 곡(민수3)은 지훈의 1번째 곡 뒤로 밀린다.
        // 아직 아무도 재생을 끝내지 않아 같은 라운드끼리는 등록순이다.
        assert_eq!(queue_ids(&state), vec!["민수2", "지훈1", "민수3"]);

        let state = player.advance(guild_id).await;
        assert_eq!(
            current_id(&state),
            "지훈1",
            "끝난 곡의 신청자를 기록하지 않으면 라운드가 영원히 안 돈다"
        );
        assert_eq!(queue_ids(&state), vec!["민수2", "민수3"]);

        // 지훈도 한 곡 받았으니 다시 민수 차례.
        let state = player.advance(guild_id).await;
        assert_eq!(current_id(&state), "민수2");

        // 공평제는 대기 점수를 순서에 쓰지 않으므로 곡 경계마다 점수를 올리지 않는다.
        let scores = remote.queue_scores(guild_id);
        assert!(scores.values().all(|score| score.wait_score == 0));
        cleanup(player, remote, root);
    }

    /// "누구의 몇 번째 곡" 표시값이 정렬과 같은 계산에서 나와 응답 JSON까지 도달한다.
    #[tokio::test]
    async fn rounds_land_in_the_score_rows() {
        let (player, remote, root) = temp_player("rounds");
        let guild_id = 1;
        player.set_sort_mode(guild_id, QueueSortMode::Fair);
        seed_two_requesters(&player, guild_id).await;

        let scores = remote.queue_scores(guild_id);
        assert_eq!(scores["민수2"].round, 0);
        assert_eq!(scores["민수3"].round, 1);
        assert_eq!(scores["지훈1"].round, 0);
        cleanup(player, remote, root);
    }

    /// 점수제는 기존 동작 유지 — 곡 경계마다 요청자별 맨 위 한 곡이 나이를 먹는다.
    #[tokio::test]
    async fn score_mode_still_ages_the_top_item_per_requester() {
        let (player, remote, root) = temp_player("score");
        let guild_id = 1;
        player.set_sort_mode(guild_id, QueueSortMode::Score);
        seed_two_requesters(&player, guild_id).await;

        player.advance(guild_id).await;
        let scores = remote.queue_scores(guild_id);
        assert_eq!(scores["민수2"].wait_score, 1);
        assert_eq!(scores["지훈1"].wait_score, 1);
        assert_eq!(scores["민수3"].wait_score, 0, "요청자당 한 곡만 나이를 먹는다");
        cleanup(player, remote, root);
    }

    /// 셔플을 켜도 랭킹이 죽지 않는다 — 예전에는 `sort_scored_queue` 가 조기 반환해
    /// 수동 우선순위(관리자 강제 이동)까지 무시됐다.
    #[tokio::test]
    async fn shuffle_keeps_manual_priority_on_top_and_is_stable() {
        let (player, remote, root) = temp_player("shuffle");
        let guild_id = 1;
        for content_id in ["a", "b", "c", "d", "e"] {
            player.enqueue(guild_id, user_item(content_id, 1), false).await;
        }
        player.set_shuffle(guild_id, true).await;
        player
            .set_manual_priority(guild_id, "e", Some(1))
            .await
            .unwrap();

        let state = player.get_state(guild_id).await;
        assert_eq!(state.upcoming.first().map(|item| item.id.as_str()), Some("e"));
        // 셔플 순서는 시드로 재현되므로 5초마다 재정렬해도 큐가 요동치지 않는다.
        let again = player.get_state(guild_id).await;
        assert_eq!(queue_ids(&state), queue_ids(&again));
        cleanup(player, remote, root);
    }

    /// 정렬 모드 캐시가 비어 있으면 DB에서 한 번 읽고, 웹이 바꾸면 즉시 반영된다.
    #[tokio::test]
    async fn sort_mode_cache_reads_db_once_then_follows_the_web() {
        let (player, remote, root) = temp_player("mode");
        let guild_id = 1;
        let mut settings = remote.load_guild_settings(guild_id);
        settings.sort_mode = QueueSortMode::Fair;
        remote.save_guild_settings(&settings).unwrap();

        assert_eq!(player.sort_mode(guild_id), QueueSortMode::Fair);
        player.set_sort_mode(guild_id, QueueSortMode::Fifo);
        assert_eq!(player.sort_mode(guild_id), QueueSortMode::Fifo);
        player.invalidate_sort_mode(guild_id);
        assert_eq!(player.sort_mode(guild_id), QueueSortMode::Fair);
        cleanup(player, remote, root);
    }
}
