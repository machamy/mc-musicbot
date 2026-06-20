//! 길드 재생 상태기계 — C# GuildPlayerManager 1:1 포팅.
//! 반복/셔플/CycleHistory/자동추천 시드 규칙/최근기록(25개 상한) 의미론을 그대로 유지한다.

use crate::db::Db;
use crate::logging::LogService;
use crate::models::*;
use rand::seq::SliceRandom;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

pub struct PlayerManager {
    db: Arc<Db>,
    log: Arc<LogService>,
    gate: Mutex<()>,
    previews: StdMutex<HashMap<u64, QueueItem>>,
    preview_inflight: StdMutex<HashSet<u64>>,
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
    pub fn new(db: Arc<Db>, log: Arc<LogService>) -> PlayerManager {
        PlayerManager {
            db,
            log,
            gate: Mutex::new(()),
            previews: StdMutex::new(HashMap::new()),
            preview_inflight: StdMutex::new(HashSet::new()),
        }
    }

    pub fn effective_settings(&self, guild_id: u64) -> EffectiveGuildSettings {
        let global = self.db.load_global_settings();
        let guild = self.db.load_guild_settings(guild_id);
        EffectiveGuildSettings {
            effective_volume: guild.volume_override.unwrap_or(global.master_volume),
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
        f(&mut state);
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

    pub async fn shuffle(&self, guild_id: u64) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Queue",
            &format!("Shuffled queue for guild {guild_id}."),
            |s| {
                s.shuffle_enabled = true;
                shuffle_upcoming(&mut s.upcoming);
            },
        )
        .await
    }

    /// 셔플 모드 토글용. 켤 때만 대기열을 즉시 섞고, 끌 때는 플래그만 해제한다
    /// (원래 순서는 복원하지 않음). 버튼이 셔플을 on/off 토글로 표시하므로 필요.
    pub async fn set_shuffle(&self, guild_id: u64, enabled: bool) -> GuildPlayerState {
        self.mutate(
            guild_id,
            "Queue",
            &format!("Shuffle {enabled} for guild {guild_id}."),
            |s| {
                s.shuffle_enabled = enabled;
                if enabled {
                    shuffle_upcoming(&mut s.upcoming);
                }
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
        let v = volume.clamp(0, 200);
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
