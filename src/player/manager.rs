//! 길드 재생 상태기계 — C# GuildPlayerManager 1:1 포팅.
//! 반복/셔플/CycleHistory/자동추천 시드 규칙/최근기록(25개 상한) 의미론을 그대로 유지한다.

use crate::app::QUEUE_SORT_INTERVAL;
use crate::db::Db;
use crate::logging::LogService;
use crate::models::*;
use crate::remote::ranking::{self, sort_queue, wait_score_targets};
use crate::remote::{QueueSortMode, RemoteGuildSettings, RemoteStore, VotePoints};
use crate::stats::{PlayOutcome, StatEvent, Stats};
use rand::seq::SliceRandom;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::Mutex;

/// 대기열이 이보다 길면 재정렬을 느리게 돌린다 (v3 §18.2).
/// 그 정도 길이면 순서가 급하지 않고, 5초마다 전체를 정렬하면 그게 그대로 부하가 된다.
pub const LONG_QUEUE_THRESHOLD: usize = 500;
/// 긴 대기열의 재정렬 주기. 화면 카운트다운(§5)도 이 값을 따라와야 한다.
pub const LONG_QUEUE_SORT_INTERVAL: Duration = Duration::from_secs(15);

pub struct PlayerManager {
    db: Arc<Db>,
    remote: Arc<RemoteStore>,
    log: Arc<LogService>,
    gate: Mutex<()>,
    previews: StdMutex<HashMap<u64, QueueItem>>,
    preview_inflight: StdMutex<HashSet<u64>>,
    /// 길드별 리모컨 설정 캐시. 정렬은 5초마다·모든 상태 변경마다 돌기 때문에
    /// 매번 설정 JSON을 읽으면 유휴 상태에서도 쿼리가 계속 나간다(사양서 §5.2 H).
    ///
    /// 정렬 모드만이 아니라 **투표 점수(v3 §10.1)도 여기서 나온다** — 점수를 읽으려고
    /// DB를 또 치면 방금 없앤 문제가 그대로 되살아난다. 웹이 설정을 저장하면
    /// `set_sort_mode`/`refresh_settings`/`invalidate_settings` 로 캐시를 맞춘다.
    settings: StdMutex<HashMap<u64, Arc<RemoteGuildSettings>>>,
    /// 길드별 셔플 시드. 셔플은 별도 정렬 모드가 아니라 `Fifo` + 무작위 `original_order`이며
    /// (사양서 §3.3), 그 무작위 순서를 시드 하나로 재현한다.
    shuffle_seeds: StdMutex<HashMap<u64, u64>>,
    /// 길드별 마지막으로 확인된 대기열 길이. 재정렬 주기 결정(v3 §18.2)에 쓰는데,
    /// 그것 때문에 5초마다 길드마다 DB를 읽으면 §18.2 를 고치려다 §23.2 를 깨뜨린다.
    queue_lens: StdMutex<HashMap<u64, usize>>,
    /// 통계 기록기 (v3 §22). 통계 DB가 안 열렸거나 아직 안 붙었으면 `None` 이고
    /// 그때는 **조용히 건너뛴다** — 통계 한 줄 때문에 음악이 멈추면 본말전도다.
    stats: StdMutex<Option<Arc<Stats>>>,
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
            settings: StdMutex::new(HashMap::new()),
            shuffle_seeds: StdMutex::new(HashMap::new()),
            queue_lens: StdMutex::new(HashMap::new()),
            stats: StdMutex::new(None),
        }
    }

    // ───────── 통계 (v3 §22) ─────────

    /// 통계 기록기를 붙인다. 통계 DB가 안 열리면 아예 안 부르면 되고, 그러면 통계만 꺼진다.
    /// `App::new` 가 `Stats::open` 성공 시 한 번 부른다.
    pub fn attach_stats(&self, stats: Arc<Stats>) {
        *self.stats.lock().unwrap() = Some(stats);
    }

    fn stats(&self) -> Option<Arc<Stats>> {
        self.stats.lock().unwrap().clone()
    }

    /// 곡 하나가 끝났다는 사실을 통계에 남긴다. 채널에 던지기만 하고 즉시 돌아온다 —
    /// 통계 쓰기가 재생 경로를 막으면 안 된다(§22.2).
    fn record_play(&self, guild_id: u64, finished: &QueueItem, outcome: PlayOutcome) {
        if let Some(stats) = self.stats() {
            stats.record(StatEvent::played_from_item(guild_id, finished, outcome));
        }
    }

    /// 붐따(§10.3)로 대기열에서 내려간 곡을 통계에 남긴다.
    /// 붐따 실행 자체는 웹이 하므로, 웹은 곡을 내린 **뒤에** 이걸 한 번 부른다.
    pub fn record_boomtta(&self, guild_id: u64, item: &QueueItem) {
        if let Some(stats) = self.stats() {
            stats.record(StatEvent::boomtta_from_item(guild_id, item));
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
                "대기열에 없는 순번이에요. 1~{}번 중에서 골라 주세요.",
                state.upcoming.len()
            ));
        }
        let item = state.upcoming.remove(from);
        state.upcoming.insert(to, item);
        self.db.save_guild_state(&state);
        self.note_queue_len(&state);
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
                "대기열에 없는 순번이에요. 1~{}번 중에서 골라 주세요.",
                state.upcoming.len()
            ));
        }
        let removed = state.upcoming.remove(index);
        let _ = self.remote.clear_item_runtime(&removed.id);
        self.db.save_guild_state(&state);
        self.note_queue_len(&state);
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
            self.note_queue_len(&state);
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
                // 취소는 스킵과 같은 전이다 — 통계에도 스킵으로 남긴다.
                self.record_play(guild_id, &cur, PlayOutcome::Skipped);
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
            self.note_queue_len(&state);
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
                    "대기열에 없는 순번이에요. 1~{}번 중에서 골라 주세요.",
                    state.upcoming.len()
                ));
            }
            // 방금 재생하던 곡을 먼저 사이클에 보존(재생 순서 유지), 그 다음 건너뛴 곡들을 보존.
            if let Some(cur) = state.current_item.take() {
                // 재생하던 곡만 스킵으로 센다. 건너뛴 대기열 곡들은 재생된 적이 없다.
                self.record_play(guild_id, &cur, PlayOutcome::Skipped);
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
                        self.record_play(guild_id, current, PlayOutcome::Completed);
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
                    self.record_play(guild_id, current, PlayOutcome::Skipped);
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
        self.note_queue_len(&state);
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

    // ───────── 길드 설정 캐시 ─────────

    /// 이 길드의 리모컨 설정. 캐시가 비었을 때만 설정 JSON을 읽는다.
    ///
    /// 정렬 모드와 투표 점수(§10.1)가 같은 캐시에서 나오므로, 점수 하나 읽자고
    /// 정렬마다 DB를 다시 치는 일이 없다.
    pub fn cached_settings(&self, guild_id: u64) -> Arc<RemoteGuildSettings> {
        if let Some(cached) = self.settings.lock().unwrap().get(&guild_id) {
            return cached.clone();
        }
        // 설정 조회는 remote 커넥션 뮤텍스를 잡으므로 캐시 락을 놓은 뒤에 읽는다.
        let loaded = Arc::new(self.remote.load_guild_settings(guild_id));
        self.settings
            .lock()
            .unwrap()
            .insert(guild_id, loaded.clone());
        loaded
    }

    /// 길드의 정렬 모드.
    pub fn sort_mode(&self, guild_id: u64) -> QueueSortMode {
        self.cached_settings(guild_id).sort_mode
    }

    /// 웹이 서버 관리 콘솔에서 모드를 저장한 직후 호출한다. 저장은 웹이 하고 여기선 캐시만 맞춘다.
    pub fn set_sort_mode(&self, guild_id: u64, mode: QueueSortMode) {
        let mut updated = (*self.cached_settings(guild_id)).clone();
        updated.sort_mode = mode;
        self.settings
            .lock()
            .unwrap()
            .insert(guild_id, Arc::new(updated));
    }

    /// 웹이 설정을 통째로 저장했을 때. 방금 쓴 값을 그대로 넘겨 주면 DB를 다시 읽지 않는다.
    pub fn refresh_settings(&self, settings: &RemoteGuildSettings) {
        self.settings
            .lock()
            .unwrap()
            .insert(settings.guild_id, Arc::new(settings.clone()));
    }

    /// 무엇이 바뀌었는지 모를 때 쓰는 무효화. 다음 조회에서 한 번만 DB를 읽는다.
    pub fn invalidate_settings(&self, guild_id: u64) {
        self.settings.lock().unwrap().remove(&guild_id);
    }

    /// 예전 이름. 이제 설정 전체를 버린다.
    pub fn invalidate_sort_mode(&self, guild_id: u64) {
        self.invalidate_settings(guild_id);
    }

    // ───────── 대기열 길이와 재정렬 주기 (v3 §18.2) ─────────

    /// 마지막으로 확인된 대기열 길이. **쿼리를 내지 않는다.**
    /// 한 번도 상태를 읽은 적 없는 길드는 0이다(그런 길드는 정렬할 것도 없다).
    pub fn queue_len(&self, guild_id: u64) -> usize {
        self.queue_lens
            .lock()
            .unwrap()
            .get(&guild_id)
            .copied()
            .unwrap_or(0)
    }

    /// 이 길드를 얼마 주기로 재정렬해야 하는가. 500곡을 넘으면 15초로 늘어난다.
    /// 웹의 `nextSortAt`(§5 카운트다운)도 이 값을 써야 화면과 실제가 어긋나지 않는다.
    pub fn sort_interval(&self, guild_id: u64) -> Duration {
        if self.queue_len(guild_id) > LONG_QUEUE_THRESHOLD {
            LONG_QUEUE_SORT_INTERVAL
        } else {
            QUEUE_SORT_INTERVAL
        }
    }

    /// 5초 tick 하나를 세는 재정렬 루프용. 긴 대기열은 3틱에 한 번만 돌린다.
    /// `tick` 은 루프가 켜진 뒤 몇 번째 tick 인지(0부터).
    pub fn due_for_resort(&self, guild_id: u64, tick: u64) -> bool {
        let base = QUEUE_SORT_INTERVAL.as_secs().max(1);
        let every = (self.sort_interval(guild_id).as_secs() / base).max(1);
        tick % every == 0
    }

    /// 상태를 읽거나 저장할 때마다 길이를 적어 둔다. 여기가 유일한 갱신 지점이다.
    fn note_queue_len(&self, state: &GuildPlayerState) {
        self.queue_lens
            .lock()
            .unwrap()
            .insert(state.guild_id, state.upcoming.len());
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
        self.note_queue_len(state);
    }

    fn sort_scored_queue(&self, state: &mut GuildPlayerState) {
        // 정렬 모드와 투표 점수를 **한 번에** 캐시에서 꺼낸다. 점수(§10.1)를 읽자고
        // 정렬마다 설정 JSON을 다시 파싱하면 유휴 상태의 쿼리 0회 기준(§23.2)이 깨진다.
        let settings = self.cached_settings(state.guild_id);
        // 셔플은 별도 모드가 아니라 `Fifo` + 무작위 `original_order`다(사양서 §3.3).
        // 예전처럼 여기서 조기 반환하면 셔플을 켜는 순간 랭킹·수동 우선순위가 통째로 죽었다.
        let mode = if state.shuffle_enabled {
            QueueSortMode::Fifo
        } else {
            settings.sort_mode
        };
        let points = VotePoints::from_settings(&settings);
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

        sort_queue(&mut state.upcoming, &scores, mode, &points);
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

    /// 자동재생으로 나간 곡은 우리 차트(v3 §15.2b)에 오르지 않는다.
    ///
    /// 판정이 시작되는 곳이 여기(`advance`)라서, 이벤트를 쏘는 지점부터 차트 숫자까지
    /// 한 번은 끝까지 이어 봐야 한다. 여기가 틀리면 차트가 조용히
    /// "자동재생이 많이 튼 곡" 목록이 되는데, 화면만 봐서는 절대 눈치챌 수 없다.
    #[tokio::test]
    async fn autoplay_playback_never_reaches_our_chart() {
        use crate::stats::{ChartKind, ChartWindow};

        let (player, remote, root) = temp_player("stats");
        let log = Arc::new(LogService::new(root.join("logs")));
        let stats = Stats::open(&root.join("musicbot-stats.sqlite"), log).expect("통계 DB 열기");
        player.attach_stats(stats.clone());
        let guild_id = 1;

        let human = user_item("사람곡", 7);
        let human_key = human.track.cache_key();
        let mut auto = QueueItem::new_autoplay(human.track.clone());
        // 자동재생도 사람이 신청한 곡과 **같은 곡**으로 둔다. 그래야 신청자 유무가 아니라
        // request_kind 로 갈리는지가 드러난다.
        auto.id = "자동곡".into();

        player.enqueue(guild_id, human, false).await;
        player.enqueue(guild_id, auto, false).await;
        player.advance(guild_id).await; // 사람곡 종료
        player.advance(guild_id).await; // 자동곡 종료

        // 통계 쓰기는 배치라서 바로 보이지 않는다 — 재생 경로를 안 막는 대가다.
        let row = wait_for_plays(&stats, guild_id, 2).await;
        assert_eq!(row.cache_key, human_key);
        assert_eq!(row.plays_user, 1, "사람이 신청한 재생만 순위에 든다");
        assert_eq!(row.plays_autoplay, 1, "자동재생도 세긴 세되 순위에는 안 쓴다");

        let chart = stats.chart(guild_id, ChartKind::Plays, ChartWindow::All, 2, 10);
        assert_eq!(chart.len(), 1);
        cleanup(player, remote, root);
    }

    /// 배치 쓰기가 `total` 건 반영될 때까지 기다린다. 고정 sleep 은 느리거나 불안정해서 폴링한다.
    /// **합계**로 기다려야 갈림(사람/자동재생)이 틀렸을 때도 멈추지 않고 그 자리에서 단언이 깨진다.
    async fn wait_for_plays(
        stats: &Arc<Stats>,
        guild_id: u64,
        total: i64,
    ) -> crate::stats::ChartRow {
        use crate::stats::{ChartKind, ChartWindow};
        for _ in 0..50 {
            // 사랑받은 곡 기준으로 뽑으면 plays_user 가 0인 행도 보여서, 갈림이 틀린 경우도 잡힌다.
            let chart = stats.chart(guild_id, ChartKind::Plays, ChartWindow::All, 2, 10);
            if let Some(row) = chart.into_iter().next() {
                if row.plays_user + row.plays_autoplay >= total {
                    return row;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("통계 {total}건이 5초 안에 반영되지 않았다");
    }

    /// 대기열이 500곡을 넘으면 재정렬 주기가 15초로 늘어난다(v3 §18.2).
    /// 주기 결정은 `app.rs` 가 하지만, 길이를 아는 건 여기뿐이라 판단 근거도 여기서 준다.
    #[tokio::test]
    async fn long_queues_get_a_slower_resort_interval() {
        let (player, remote, root) = temp_player("interval");
        // 아직 아무것도 안 본 길드는 정렬할 것도 없으니 기본 주기다.
        assert_eq!(player.queue_len(1), 0);
        assert_eq!(player.sort_interval(1), QUEUE_SORT_INTERVAL);
        assert!(player.due_for_resort(1, 0) && player.due_for_resort(1, 1));

        player.enqueue(1, user_item("한곡", 1), false).await;
        assert_eq!(player.queue_len(1), 0, "현재 곡으로 올라갔으니 대기열은 비었다");
        player.enqueue(1, user_item("두곡", 1), false).await;
        assert_eq!(player.queue_len(1), 1);
        assert_eq!(player.sort_interval(1), QUEUE_SORT_INTERVAL);

        // 500곡을 실제로 넣으면 테스트가 느려지기만 하니 길이만 밀어 넣는다.
        player
            .queue_lens
            .lock()
            .unwrap()
            .insert(1, LONG_QUEUE_THRESHOLD + 1);
        assert_eq!(player.sort_interval(1), LONG_QUEUE_SORT_INTERVAL);
        // 5초 tick 기준으로 3틱에 한 번만 돈다.
        assert!(player.due_for_resort(1, 0));
        assert!(!player.due_for_resort(1, 1));
        assert!(!player.due_for_resort(1, 2));
        assert!(player.due_for_resort(1, 3));
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
