//! 재생 코디네이터 — 저장 상태와 실제 songbird 음성 세션 동기화.
//! C# DiscordPlaybackCoordinator 의 역할이지만, 20ms 송신 루프/페이싱/DAVE 는
//! songbird 드라이버(전용 스레드)가 담당하므로 훨씬 얇다. 끊김 방지의 핵심이
//! 라이브러리 레벨에서 해결되는 구조 (JDA-NAS/Lavalink 와 같은 사상).
//!
//! 남는 책임: ffmpeg 파이프 구성, 곡 전환/이어재생, 스톨 워치독, 진행도 추적.

use crate::app::App;
use crate::models::*;
use songbird::events::{Event, EventContext, EventHandler as VoiceEventHandler, TrackEvent};
use songbird::input::core::io::ReadOnlySource;
use songbird::input::{AudioStream, Input, LiveInput, RawAdapter};
use songbird::tracks::TrackHandle;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

pub struct Session {
    pub handle: TrackHandle,
    pub item_id: String,
    /// 이 세션의 고유 세대 번호. play_track 마다 새로 발급된다. seek/replay 는 item_id 가
    /// 같으므로, 멈춘 옛 핸들에서 늦게 도착하는 TrackEnd 가 item_id 만으로는 구분되지 않는다.
    /// 세대 번호로 "현재 살아있는 세션이 발급한 이벤트인지"를 판별한다.
    pub generation: u64,
    /// ffmpeg -ss 로 건너뛴 시작 오프셋 (진행도 = offset + track position).
    pub start_offset: Duration,
    pub retry_count: u32,
    /// **이 곡의 0초 지점에 해당하는 UTC 시각** (§31).
    ///
    /// 모든 클라이언트가 같은 지점을 계산하게 만드는 기준이다. 전에는 서버가 "지금 몇 초"
    /// 를 보내고 클라이언트가 전송 지연을 추정해 더했는데, 그 추정이 기기마다 달라서
    /// 사람마다 소리가 어긋났다. 절대 시각을 주면 각자 `now - started_utc` 로 계산하므로
    /// **곡마다 생기던 미세한 오차가 사라진다.**
    ///
    /// 스킵처럼 서버가 앞으로 잡아 두는 경우 이 값이 미래일 수 있다 — 그때는 아직 시작 전이다.
    pub started_utc: chrono::DateTime<chrono::Utc>,
}

/// 웹이 따라갈 재생 일정 (§31).
#[derive(Debug, Clone, Copy)]
pub struct TrackSchedule {
    /// 0초 지점의 UTC. 미래면 아직 시작 전이다.
    pub started_utc: chrono::DateTime<chrono::Utc>,
}

/// 봇이 음성에 없을 때 도는 **시각표만 있는 세션** (웹 재생기 모드).
///
/// 오디오는 한 바이트도 안 나른다. 시각을 흘려 보내는 것이 전부고, 브라우저들이 그 하나의
/// 시각표를 따라가므로 모두가 같은 곡·같은 위치가 된다.
///
/// **일시정지는 위치를 얼려서 담는다.** `started_utc` 만으로 계산하면 멈춰 있는 동안에도
/// 위치가 계속 흐른다 — 물리 세션은 songbird 핸들이 멈춰서 저절로 해결되지만 여기는 아니다.
#[derive(Debug, Clone)]
pub struct VirtualSession {
    pub item_id: String,
    /// 0초 지점의 UTC. **미래일 수 있다** (스킵 직후처럼 앞으로 잡아 둔 경우).
    pub started_utc: chrono::DateTime<chrono::Utc>,
    /// 멈춘 위치. `Some` 이면 정지 중이고 그 값이 곧 현재 위치다.
    pub paused_at: Option<Duration>,
    /// 이 세션의 세대. 뒤늦게 깨어난 타이머가 자기 세대인지 확인한다.
    pub generation: u64,
}

impl VirtualSession {
    /// 지금 위치. 멈춰 있으면 얼려 둔 값, 아니면 흐른 시간.
    ///
    /// `started_utc` 가 미래면 아직 시작 전이라 0 이다 — `Duration` 은 음수를 못 담는다.
    pub fn position(&self) -> Duration {
        if let Some(frozen) = self.paused_at {
            return frozen;
        }
        let elapsed = chrono::Utc::now() - self.started_utc;
        elapsed.to_std().unwrap_or(Duration::ZERO)
    }
}

pub struct Coordinator {
    sessions: Mutex<HashMap<u64, Session>>,
    /// 시각표만 도는 가상 세션. 물리 세션과 **같은 길드에 동시에 있지 않는다.**
    virtual_sessions: Mutex<HashMap<u64, VirtualSession>>,
    /// 길드별 마지막으로 재생 횟수를 +1 한 item_id. 스톨 워치독/게이트웨이 재접속이
    /// 같은 곡으로 play_track 을 다시 호출해도 중복 카운트하지 않도록 막는 게이트.
    played_counted: Mutex<HashMap<u64, String>>,
    /// 단조 증가 세대 카운터 (play_track 마다 +1).
    gen_counter: AtomicU64,
    /// 길드별 연속 재생 실패 횟수. 다운로드/ffmpeg 실패가 반복될 때 무한 스킵을 막는다.
    play_fail: Mutex<HashMap<u64, u32>>,
    /// 지금 가상 재생 중인 길드. `PlayerManager` 와 같은 손잡이를 나눠 갖는다 —
    /// 통계가 재생을 `plays_virtual` 로 가를 때 이 값을 읽는다.
    virtual_guilds: Arc<std::sync::Mutex<std::collections::HashSet<u64>>>,
}

const MAX_CONSECUTIVE_PLAY_FAILS: u32 = 5;

/// `reconcile_virtual` 이 이번 동기화를 어떻게 처리했는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VirtualOutcome {
    /// 웹 재생을 돌봤다 — 여기서 끝낸다.
    Handled,
    /// 내 일이 아니다 — 아래 기존(물리) 경로가 그대로 돈다.
    NotMine,
    /// 대기열을 바꿨다 — **처음부터 다시 맞춰야 한다.**
    ///
    /// 이게 없으면 곡을 넘겨 놓고 아무도 그다음을 안 틀어서 조용히 멈춘다
    /// (`skip` 도 `ensure_autoplay` 도 스스로 동기화를 부르지 않는다).
    Again,
}

/// `play_track` 이 실제로 소리를 내보냈는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayOutcome {
    /// 세션을 걸었다.
    Started,
    /// 준비를 끝냈더니 이미 다른 곡이 현재 곡이었다 — 내보내지 않고 손을 뗐다.
    Stale,
}

/// 음성 상태가 밖에서 바뀌었을 때 저장된 채널 바인딩을 어떻게 할지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingChange {
    /// 그대로 둔다.
    Keep,
    /// 실제로 있는 채널로 옮겨 적는다.
    Rebind(u64),
    /// 지운다 — 다시 부를 때까지 안 들어간다.
    Clear,
}

#[cfg(test)]
mod voice_binding_tests {
    use super::*;

    /// **강제 퇴장을 당하면 바인딩을 지운다.**
    ///
    /// 이게 없으면 바로 뒤의 `sync_guild` 가 저장값으로 `play_track` 을 불러서
    /// 봇이 쫓겨난 그 순간 다시 들어왔다. 부르지도 않았는데 돌아오는 게 이것이었다.
    #[test]
    fn being_removed_from_voice_clears_the_binding() {
        assert_eq!(
            Coordinator::handoff_binding(None, Some(42)),
            BindingChange::Clear
        );
    }

    /// 다른 방으로 끌려가면 그 방으로 따라간다. 저장값이 옛 방이면 도로 돌아가 버린다.
    #[test]
    fn being_dragged_elsewhere_moves_the_binding() {
        assert_eq!(
            Coordinator::handoff_binding(Some(7), Some(42)),
            BindingChange::Rebind(7)
        );
    }

    /// 평소에는 아무것도 안 한다 — 우리가 넣어 둔 자리에 그대로 있는 상태.
    #[test]
    fn staying_put_changes_nothing() {
        assert_eq!(
            Coordinator::handoff_binding(Some(42), Some(42)),
            BindingChange::Keep
        );
        // 이미 나가 있고 저장값도 없으면 지울 것도 없다 (`/나가기` 뒤).
        assert_eq!(Coordinator::handoff_binding(None, None), BindingChange::Keep);
    }

    /// 우리가 부르기 전에 들어와 있는 걸 발견하면 적어 둔다 — 기동 직후 같은 때.
    #[test]
    fn finding_ourselves_already_in_voice_records_it() {
        assert_eq!(
            Coordinator::handoff_binding(Some(7), None),
            BindingChange::Rebind(7)
        );
    }
}

#[cfg(test)]
mod virtual_session_tests {
    use super::*;

    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() - chrono::Duration::seconds(secs)
    }

    fn session(started: chrono::DateTime<chrono::Utc>, paused: Option<Duration>) -> VirtualSession {
        VirtualSession {
            item_id: "곡".into(),
            started_utc: started,
            paused_at: paused,
            generation: 1,
        }
    }

    /// 흐르는 중에는 시작 시각으로부터 지난 만큼이 위치다.
    #[test]
    fn a_running_session_reports_elapsed_time() {
        let v = session(at(30), None);
        let pos = v.position().as_secs();
        assert!((29..=31).contains(&pos), "약 30초여야 하는데 {pos}초");
    }

    /// **미래 `started_utc` 에서 패닉하거나 뒤집히면 안 된다.**
    ///
    /// `schedule_start_in` 은 스킵 직후처럼 시작을 앞으로 잡아 두고, 그때 `started_utc` 가
    /// 미래가 된다. `Duration` 은 음수를 못 담으므로 그대로 빼면 터진다. 아직 시작 전이니 0 이다.
    #[test]
    fn a_future_start_reads_as_zero_not_a_panic() {
        let v = session(chrono::Utc::now() + chrono::Duration::seconds(5), None);
        assert_eq!(v.position(), Duration::ZERO);
    }

    /// **멈춰 있는 동안에는 위치가 안 흐른다.**
    ///
    /// 물리 세션은 songbird 핸들이 멈춰서 저절로 해결되지만 가상은 시각 계산이라
    /// 얼려 두지 않으면 정지 중에도 계속 간다.
    #[test]
    fn a_paused_session_freezes_its_position() {
        let v = session(at(100), Some(Duration::from_secs(42)));
        assert_eq!(v.position(), Duration::from_secs(42));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(v.position(), Duration::from_secs(42), "정지 중에는 안 흘러야 한다");
    }

    /// 재개하면 얼려 둔 지점이 0초 기준이 된다 — `apply_pause` 가 하는 계산과 같은 식.
    #[test]
    fn resuming_rebases_the_start_to_the_frozen_point() {
        let frozen = Duration::from_secs(42);
        let mut v = session(at(100), Some(frozen));
        v.started_utc = chrono::Utc::now() - chrono::Duration::from_std(frozen).unwrap();
        v.paused_at = None;
        let pos = v.position().as_secs();
        assert!((41..=43).contains(&pos), "재개 직후는 얼린 지점이어야 하는데 {pos}초");
    }
}

impl Coordinator {
    pub fn new(
        virtual_guilds: Arc<std::sync::Mutex<std::collections::HashSet<u64>>>,
    ) -> Coordinator {
        Coordinator {
            virtual_guilds,
            sessions: Mutex::new(HashMap::new()),
            virtual_sessions: Mutex::new(HashMap::new()),
            played_counted: Mutex::new(HashMap::new()),
            gen_counter: AtomicU64::new(1),
            play_fail: Mutex::new(HashMap::new()),
        }
    }

    /// 이 곡의 0초 지점 UTC (§31). 웹이 절대 시각으로 따라가게 하는 값이다.
    pub async fn schedule(&self, guild_id: u64) -> Option<TrackSchedule> {
        {
            let sessions = self.sessions.lock().await;
            if let Some(s) = sessions.get(&guild_id) {
                return Some(TrackSchedule {
                    started_utc: s.started_utc,
                });
            }
        }
        // 물리 세션이 없으면 가상 세션을 본다. **화면은 시각표가 어디서 왔는지 묻지 않는다** —
        // 이 한 자리 덕분에 진행바·웹 재생·동기화가 전부 그대로 따라온다.
        let virtuals = self.virtual_sessions.lock().await;
        virtuals.get(&guild_id).map(|v| TrackSchedule {
            started_utc: v.started_utc,
        })
    }

    /// 스킵·되감기처럼 **모두가 같은 순간에 같은 지점을 시작해야 할 때** 쓴다 (§31).
    ///
    /// 지금 당장이 아니라 `lead` 만큼 미래로 잡는다. 서버가 "지금부터" 라고 하면
    /// 그 말이 클라이언트마다 다른 시각에 도착해서 각자 다른 지점에서 시작한다.
    /// 조금 미래로 잡아 두면 모두가 그 시각을 기다렸다 함께 출발한다.
    pub async fn schedule_start_in(&self, guild_id: u64, lead: Duration, position: Duration) {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(&guild_id) {
            session.started_utc = chrono::Utc::now()
                + chrono::Duration::from_std(lead).unwrap_or_default()
                - chrono::Duration::from_std(position).unwrap_or_default();
        }
    }

    /// 현재 곡의 재생 위치 (시작 오프셋 포함). /현재곡 진행바용.
    pub async fn current_position(&self, guild_id: u64) -> Option<Duration> {
        {
            let sessions = self.sessions.lock().await;
            if let Some(s) = sessions.get(&guild_id) {
                // **물리 계산은 그대로 둔다.** `started_utc` 를 보지 않고 핸들 위치를 쓴다 —
                // 둘을 통일하려 들면 기존 진행바가 틀어진다.
                let info = s.handle.get_info().await.ok()?;
                return Some(s.start_offset + info.position);
            }
        }
        let virtuals = self.virtual_sessions.lock().await;
        virtuals.get(&guild_id).map(|v| v.position())
    }

    pub async fn apply_volume(&self, guild_id: u64, volume_percent: i32) {
        let sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get(&guild_id) {
            let _ = s
                .handle
                .set_volume((volume_percent.clamp(0, 200) as f32) / 100.0);
        }
    }

    /// 현재 음성 세션이 있는(재생 중인) 길드 id 목록 — 웹 전역 설정 변경을 라이브 반영할 때 사용.
    pub async fn active_guild_ids(&self) -> Vec<u64> {
        self.sessions.lock().await.keys().copied().collect()
    }

    pub async fn apply_pause(&self, guild_id: u64, paused: bool) {
        {
            let sessions = self.sessions.lock().await;
            if let Some(s) = sessions.get(&guild_id) {
                if paused {
                    let _ = s.handle.pause();
                } else {
                    let _ = s.handle.play();
                }
                return;
            }
        }
        // 가상 세션은 멈출 핸들이 없다. **위치를 얼려서 담고**, 풀 때 그 지점을 0초로 삼아
        // `started_utc` 를 다시 잡는다. 안 그러면 멈춰 있는 동안에도 위치가 계속 흐른다.
        let mut virtuals = self.virtual_sessions.lock().await;
        if let Some(v) = virtuals.get_mut(&guild_id) {
            match (paused, v.paused_at) {
                (true, None) => v.paused_at = Some(v.position()),
                (false, Some(frozen)) => {
                    v.started_utc = chrono::Utc::now()
                        - chrono::Duration::from_std(frozen).unwrap_or_default();
                    v.paused_at = None;
                }
                _ => {}
            }
        }
    }

    pub async fn cancel_current(&self, guild_id: u64) {
        {
            let mut sessions = self.sessions.lock().await;
            if let Some(s) = sessions.remove(&guild_id) {
                let _ = s.handle.stop();
            }
        }
        // 가상 세션도 같이 버린다. 남겨 두면 물리와 시각표가 두 벌이 된다.
        self.virtual_sessions.lock().await.remove(&guild_id);
        self.virtual_guilds.lock().unwrap().remove(&guild_id);
    }

    /// 봇이 음성 채널을 들락날락한 것을 활동 기록에 남긴다 (§13.3).
    ///
    /// **행위자가 사람이 아닐 수 있다.** 강제 퇴장이나 빈 채널 정리는 아무도 시키지 않은
    /// 일이라 `봇` 으로 적는다. 사람이 `/참여`·`/나가기` 로 시킨 것은 그 명령 쪽에서 이미
    /// 남기므로, 여기서는 **상태가 실제로 바뀌었을 때만** 한 줄 남긴다 — 안 그러면 같은
    /// 일이 두 줄이 된다.
    async fn record_voice_change(&self, app: &Arc<App>, guild_id: u64) {
        let now = crate::web::remote::bot_voice_status_of(app, guild_id);
        let was_in_voice = self.sessions.lock().await.contains_key(&guild_id);
        let (action, channel) = match (was_in_voice, now.in_voice()) {
            // 세션이 있었는데 지금 음성에 없다 — 나갔거나 쫓겨났다.
            (true, false) => ("voice.leave", None),
            // 세션이 없었는데 지금 있다 — 들어왔다.
            (false, true) => ("voice.join", now.channel_name.clone()),
            // 둘 다 있으면 방을 옮긴 것이다. 둘 다 없으면 기록할 게 없다.
            (true, true) => ("voice.move", now.channel_name.clone()),
            (false, false) => return,
        };
        let _ = app.remote.add_audit(
            guild_id,
            0,
            "봇",
            action,
            channel.as_deref(),
            None,
            None,
            true,
            None,
        );
    }

    /* 재생 실패로 곡이 넘어간 것을 **리모컨 활동 기록에도** 남긴다 (§10.8).
     *
     * 예전에는 디스코드 채널에만 알렸다. 그래서 리모컨만 보는 사람에게는 곡이 아무 이유
     * 없이 줄줄이 사라지는 것으로 보였고, 활동 기록에는 아무것도 없으니 원인을 찾을
     * 실마리가 없었다. 사람이 넘긴 것과 문구를 다르게 해서 서로 의심하지 않게 한다.
     */
    fn record_playback_failure(&self, app: &Arc<App>, guild_id: u64, action: &str, title: &str) {
        let _ = app.remote.add_audit(
            guild_id,
            0,
            "봇",
            action,
            Some(title),
            None,
            None,
            true,
            None,
        );
    }

    /// 봇의 음성 상태가 바뀌었다 — 재생 위치를 잃지 않고 넘긴다.
    ///
    /// **순서가 전부다.** songbird 시작 위치는 `QueueItem.start_offset` 에서 오고
    /// `started_utc` 도 그 값으로 다시 만들어진다(`play_track`). 그래서 세션을 먼저 버리면
    /// 흘러간 위치를 잃고 곡이 **처음부터 다시 시작한다.**
    ///
    /// ```text
    /// 1. 지금 위치를 읽는다        2. start_offset 에 쓴다        3. 그다음에 세션을 버린다
    /// ```
    ///
    /// 강제 퇴장도 여기로 온다. 예전에는 `voice_state_update` 가 세션을 안 건드려서,
    /// Discord 캐시는 미연결인데 물리 세션이 남아 시각표가 계속 나갔다 — 그 자체가 버그였다.
    /// 음성 상태가 밖에서 바뀌었을 때 저장된 바인딩을 어떻게 할까.
    /// 판단만 하는 함수라 Discord 없이도 검사할 수 있다.
    fn handoff_binding(live: Option<u64>, stored: Option<u64>) -> BindingChange {
        match live {
            Some(actual) if stored != Some(actual) => BindingChange::Rebind(actual),
            None if stored.is_some() => BindingChange::Clear,
            _ => BindingChange::Keep,
        }
    }

    pub async fn handoff_voice_change(self: &Arc<Self>, app: &Arc<App>, guild_id: u64) {
        self.record_voice_change(app, guild_id).await;

        /* **바인딩을 현실에 맞춘다.** 이게 없으면 봇이 자기 발로 돌아온다.
         *
         * 저장된 `voice_channel_id` 는 "다음에 어디로 들어갈까" 라서 강제 퇴장 뒤에도
         * 남아 있었다. 그런데 바로 아래 `sync_guild` 가 그 값으로 `play_track` 을 부르니,
         * 누가 봇을 뺀 **그 순간** 다시 들어왔다. 부르지도 않았는데 돌아오는 게 이것이다.
         *
         * 그래서 누가 밖에서 우리를 옮기거나 뺐으면 저장값도 따라간다.
         *   · 다른 방으로 끌려갔다  → 그 방으로 바꾼다. 끌고 간 자리에서 계속 튼다.
         *   · 아무 데도 없다        → 지운다. 다시 부를 때까지 안 들어간다.
         *
         * 웹 재생기 모드는 이 값과 무관하다. `sync_guild` 가 Discord 캐시를 먼저 보고
         * 가상 재생을 챙긴 뒤에야 여기로 오므로, 봇 없이 듣던 사람은 그대로 이어 듣는다.
         */
        let live = crate::web::remote::bot_voice_status_of(app, guild_id);
        let stored = app.player.get_state(guild_id).await.voice_channel_id;
        match Self::handoff_binding(live.channel_id, stored) {
            BindingChange::Rebind(actual) => {
                app.player.connect_voice(guild_id, actual).await;
            }
            BindingChange::Clear => {
                app.log.info(
                    "Voice",
                    &format!(
                        "길드 {guild_id}: 음성에서 빠져서 채널 연결을 풀었어요. 다시 부르기 전까지는 안 들어가요."
                    ),
                );
                app.player.disconnect_voice(guild_id).await;
            }
            BindingChange::Keep => {}
        }

        let position = self.current_position(guild_id).await;
        if let Some(pos) = position {
            // 0초면 굳이 쓰지 않는다 — 아직 시작 전이거나 방금 바뀐 곡이다.
            if pos > Duration::from_millis(500) {
                app.player
                    .set_current_start_offset(guild_id, CsTimeSpan(pos))
                    .await;
            }
        }
        // 여기서 물리·가상 양쪽을 다 버린다. 위치는 이미 큐 항목에 적어 뒀다.
        self.cancel_current(guild_id).await;
        self.sync_guild(app, guild_id).await;
    }

    /// 봇이 음성에 없을 때 가상 재생을 맞춘다. **돌봐 줬으면 `true`** 를 준다.
    ///
    /// `false` 면 아래의 기존 경로가 그대로 돈다 — 모드가 꺼져 있거나 들을 사람이 없는
    /// 평소 상태에서는 **도입 전과 완전히 같게** 동작한다는 뜻이다.
    async fn reconcile_virtual(
        self: &Arc<Self>,
        app: &Arc<App>,
        guild_id: u64,
        state: &GuildPlayerState,
    ) -> VirtualOutcome {
        let settings = app.remote.load_guild_settings(guild_id);
        let listeners = app
            .web_listener_count
            .get()
            .map(|counter| counter(guild_id))
            .unwrap_or(0);

        // 켤 이유가 없으면 도는 것을 정리하고 손을 뗀다.
        if !settings.web_player_mode || listeners == 0 || state.current_item.is_none() {
            let had = self.virtual_sessions.lock().await.remove(&guild_id).is_some();
            self.virtual_guilds.lock().unwrap().remove(&guild_id);
            if had {
                app.log.info(
                    "Playback",
                    &format!("웹 재생기 시각표를 멈췄어요 (guild {guild_id})."),
                );
            }
            return VirtualOutcome::NotMine;
        }
        let current = state.current_item.clone().expect("바로 위에서 확인함");

        // **길이를 모르면 시작하지 않는다.** 0 으로 두면 곡이 즉시 끝난 것으로 처리돼
        // 대기열이 순식간에 비워진다. 대기열은 손대지 않고 그 자리에 멈춘 채 둔다.
        /* **길이를 모르면 물어보고, 그래도 모르면 넘긴다.**
         *
         * 웹 재생은 곡이 언제 끝나는지 알아야 다음 곡으로 넘어갈 수 있어서 길이가 꼭 필요하다.
         * 예전에는 여기서 그냥 손을 떼면서 `true`(내가 처리했다)를 돌려줬는데, 그러면
         * `sync_guild` 가 **아래 실제 재생 경로까지 건너뛰고 그대로 끝난다.** 곡은 영영
         * 시작하지 않고 대기열도 안 넘어가서, 같은 경고만 몇 분이고 반복됐다 — 실제로 그랬다.
         *
         * 길이는 검색 결과(flat playlist)로 담긴 곡에 자주 비어 있다. 그럴 때 한 번
         * 물어보면 대개 알 수 있다. 그래도 모르면 **멈춰 있는 것보다 넘기는 게 낫다.** */
        let duration = match current.track.duration {
            Some(value) => Duration::from_secs_f64(value.as_secs_f64()),
            /* **라이브는 길이가 없는 게 정상이다** (§40).
             *
             * 아래 "길이를 모르면 물어보고, 그래도 모르면 넘긴다" 는 규칙에 그대로 걸려서
             * 라이브를 담으면 조용히 지나갔다. 라이브는 끝나는 시각이 없으므로 끝을 기다리는
             * 타이머도 걸지 않는다 — 방송이 끝나면 그때 다음 곡으로 넘어간다. */
            None if current.track.is_live => {
                app.log.info(
                    "Playback",
                    &format!(
                        "'{}' 은 라이브라 끝나는 시각 없이 틀어요 (guild {guild_id}).",
                        current.track.display_title()
                    ),
                );
                Duration::ZERO
            }
            None => {
                /* 싼 것부터 본다.
                 *
                 * 1) **받아 둔 파일.** 한 번이라도 튼 곡이면 캐시에 길이가 이미 적혀 있다.
                 *    네트워크도 안 타고 유튜브를 또 두드리지도 않는다.
                 * 2) 그래도 없으면 물어본다. */
                let from_cache = app
                    .cache
                    .get(&current.track.cache_key())
                    .and_then(|entry| entry.duration)
                    .map(|value| Duration::from_secs_f64(value.as_secs_f64()))
                    .filter(|value| !value.is_zero());

                let looked_up = match from_cache {
                    Some(found) => Some(found),
                    None => app
                        .ytdlp()
                        .inspect_track(&current.track.source_url, current.track.provider)
                        .await
                        .and_then(|track| track.duration)
                        .map(|value| Duration::from_secs_f64(value.as_secs_f64()))
                        .filter(|value| !value.is_zero()),
                };

                match looked_up {
                    Some(found) => {
                        app.log.info(
                            "Playback",
                            &format!(
                                "'{}' 의 길이를 알아내서 웹 재생을 시작해요 ({}초, guild {guild_id}).",
                                current.track.display_title(),
                                found.as_secs()
                            ),
                        );
                        // 알아낸 길이를 큐 항목에 적어 둔다. 안 그러면 다음 호출마다 또 물어본다.
                        app.player
                            .set_current_duration(guild_id, CsTimeSpan(found))
                            .await;
                        found
                    }
                    None => {
                        app.log.warn(
                            "Playback",
                            &format!(
                                "'{}' 은 길이를 알 수 없어 넘어가요 (guild {guild_id}).",
                                current.track.display_title()
                            ),
                        );
                        self.record_playback_failure(
                            app,
                            guild_id,
                            "playback.failed",
                            current.track.display_title(),
                        );
                        self.virtual_sessions.lock().await.remove(&guild_id);
                        // **여기서 멈추면 안 된다.** 대기열을 한 칸 밀고 다시 맞춘다.
                        app.player.skip(guild_id).await;
                        crate::player::side_effects::ensure_autoplay(
                            app.clone(),
                            self.clone(),
                            guild_id,
                            true,
                        )
                        .await;
                        return VirtualOutcome::Again;
                    }
                }
            }
        };

        // 같은 곡이 이미 돌고 있으면 그대로 둔다. 매 호출마다 새로 잡으면 위치가 0 으로 돌아간다.
        {
            let virtuals = self.virtual_sessions.lock().await;
            if let Some(v) = virtuals.get(&guild_id) {
                if v.item_id == current.id {
                    return VirtualOutcome::Handled;
                }
            }
        }

        let generation = self.gen_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let start_offset = Duration::from_secs_f64(current.start_offset.as_secs_f64());
        let started_utc = chrono::Utc::now()
            - chrono::Duration::from_std(start_offset).unwrap_or_default();
        {
            let mut virtuals = self.virtual_sessions.lock().await;
            self.virtual_guilds.lock().unwrap().insert(guild_id);
            virtuals.insert(
                guild_id,
                VirtualSession {
                    item_id: current.id.clone(),
                    started_utc,
                    paused_at: if state.is_paused {
                        Some(start_offset)
                    } else {
                        None
                    },
                    generation,
                },
            );
        }
        app.log.info(
            "Playback",
            &format!(
                "웹 재생기 시각표 시작: '{}' (guild {guild_id}, 듣는 사람 {listeners}명).",
                current.track.display_title()
            ),
        );

        // 물리 재생과 **같은 시작 훅**을 태운다. 안 부르면 다음 곡 프리페치·자동추천
        // preview·재생 카드가 하나도 안 돈다.
        //
        // 다만 **인계는 새 시작이 아니다.** 봇이 나가서 가상이 이어받는 경우 같은 곡인데도
        // 훅을 다시 태우면 프리페치·preview 가 두 번 돌고 디스코드에 카드가 또 올라간다.
        // `played_counted` 가 이미 "이 곡은 세었다" 를 길드별로 들고 있으므로 그걸 본다.
        let already_started = {
            let counted = self.played_counted.lock().await;
            counted.get(&guild_id).map(|id| id == &current.id).unwrap_or(false)
        };
        if !already_started {
            crate::player::side_effects::on_track_started(
                app.clone(),
                self.clone(),
                guild_id,
                current.clone(),
            );
        }

        self.clone()
            .spawn_virtual_timer(app.clone(), guild_id, generation, duration, start_offset);
        VirtualOutcome::Handled
    }

    /// 곡이 끝날 시각에 깨어나 다음 곡으로 넘긴다.
    ///
    /// **남은 시간만 기다린다** — 이어재생·seek 로 중간부터 시작했으면 그만큼 빼야 한다.
    /// 깨어나서는 자기 세대가 아직 살아 있는지 먼저 본다. 그 사이 스킵·정지·봇 합류가
    /// 있었으면 세대가 바뀌어 있고, 그때는 아무것도 하지 않는다.
    fn spawn_virtual_timer(
        self: Arc<Self>,
        app: Arc<App>,
        guild_id: u64,
        generation: u64,
        duration: Duration,
        start_offset: Duration,
    ) {
        /* **라이브는 끝나는 시각이 없다** (§40).
         *
         * 길이가 0 이면 "이미 끝났다" 로 읽혀서 곡이 시작하자마자 다음으로 넘어간다.
         * 라이브는 방송이 실제로 끊길 때 넘어가면 되므로 타이머를 아예 안 건다. */
        if duration.is_zero() {
            return;
        }
        let remaining = duration.saturating_sub(start_offset);
        tokio::spawn(async move {
            tokio::time::sleep(remaining).await;
            {
                let virtuals = self.virtual_sessions.lock().await;
                match virtuals.get(&guild_id) {
                    Some(v) if v.generation == generation => {}
                    // 세대가 다르거나 사라졌으면 내 차례가 아니다.
                    _ => return,
                }
            }
            // **물리 자연 종료와 같은 순서를 그대로 탄다.** 이걸 재사용하지 않으면
            // 자동재생 보충과 다음 세션 생성이 끊긴다.
            app.player.advance(guild_id).await;
            crate::player::side_effects::ensure_autoplay(
                app.clone(),
                self.clone(),
                guild_id,
                true,
            )
            .await;
            self.sync_guild(&app, guild_id).await;
        });
    }

    pub async fn leave_voice(&self, app: &Arc<App>, guild_id: u64) {
        self.cancel_current(guild_id).await;
        if let Some(manager) = app.songbird.get() {
            let _ = manager
                .remove(songbird::id::GuildId(
                    std::num::NonZeroU64::new(guild_id).unwrap(),
                ))
                .await;
        }
    }

    /// 저장 상태와 실제 음성 세션을 맞춘다 — 모든 재생 전이의 단일 진입점.
    /// 재생 실패(다운로드 403/삭제된 영상/ffmpeg 등) 시 다음 곡으로 넘어가며 재시도한다.
    /// 큐 전체가 재생 불가면 연속 실패 상한에서 멈춰 무한 스킵을 막는다.
    pub async fn sync_guild(self: &Arc<Self>, app: &Arc<App>, guild_id: u64) {
        // 준비가 끝날 때마다 곡이 또 바뀌어 있는 상황이 이어질 수 있다. 무한히 쫓아가지
        // 않도록 몇 바퀴만 돈다 — 어차피 다음 `sync_guild` 가 다시 맞춘다.
        let mut stale_rounds = 0u32;
        loop {
            let state = app.player.get_state(guild_id).await;

            // **봇이 실제로 음성에 없으면** 가상 재생을 살펴본다 (웹 재생기 모드).
            //
            // 저장된 `voice_channel_id` 가 아니라 Discord 캐시를 본다 — 저장값은 "다음에
            // 어디로 들어갈까" 라서 강제 퇴장 뒤에도 남는다(§16 B1).
            //
            // 모드가 꺼져 있거나 듣는 사람이 없으면 `reconcile_virtual` 이 아무것도 안 만들고,
            // 그러면 아래 기존 경로가 그대로 돈다 — **도입 전과 완전히 같다.**
            if !crate::web::remote::bot_voice_status_of(app, guild_id).in_voice() {
                match self.reconcile_virtual(app, guild_id, &state).await {
                    VirtualOutcome::Handled => return,
                    // 길이를 모르는 곡을 넘겼다. 넘긴 채로 두면 아무도 그다음을 안 트니
                    // 여기서 처음부터 다시 맞춘다. 계속 넘어가기만 하면 몇 번에서 끊는다.
                    VirtualOutcome::Again => {
                        stale_rounds += 1;
                        if stale_rounds >= 5 {
                            app.log.warn(
                                "Playback",
                                &format!(
                                    "길이를 알 수 없는 곡이 이어져서 이번 동기화는 여기서 멈춰요 (guild {guild_id})."
                                ),
                            );
                            return;
                        }
                        continue;
                    }
                    VirtualOutcome::NotMine => {}
                }
            }

            // 음성 채널 미바인딩 → 송출 중지.
            let Some(channel_id) = state.voice_channel_id else {
                self.cancel_current(guild_id).await;
                return;
            };

            // 현재 곡 없음 → 송출 중지 (연결은 유지: 빈채널 정책이 따로 정리).
            let Some(current) = state.current_item.clone() else {
                self.cancel_current(guild_id).await;
                return;
            };

            // 같은 곡이 이미 재생 중이면 볼륨/일시정지만 동기화.
            {
                let sessions = self.sessions.lock().await;
                if let Some(s) = sessions.get(&guild_id) {
                    if s.item_id == current.id {
                        if let Ok(info) = s.handle.get_info().await {
                            use songbird::tracks::PlayMode;
                            let playing =
                                matches!(info.playing, PlayMode::Play | PlayMode::Pause);
                            if playing {
                                drop(sessions);
                                self.apply_volume(guild_id, state.effective_volume).await;
                                self.apply_pause(guild_id, state.is_paused).await;
                                return;
                            }
                        }
                    }
                }
            }

            self.cancel_current(guild_id).await;
            match self
                .play_track(app, guild_id, channel_id, &state, &current)
                .await
            {
                Ok(PlayOutcome::Started) => {
                    self.play_fail.lock().await.remove(&guild_id);
                    return;
                }
                // 준비하는 사이에 곡이 바뀌었다. 실패가 아니므로 실패 수를 세지 않고,
                // 위로 돌아가 **지금 곡**으로 다시 맞춘다.
                Ok(PlayOutcome::Stale) => {
                    stale_rounds += 1;
                    if stale_rounds >= 5 {
                        app.log.warn(
                            "Playback",
                            &format!(
                                "곡이 계속 바뀌어서 이번 동기화는 여기서 멈춰요 (guild {guild_id})."
                            ),
                        );
                        return;
                    }
                    continue;
                }
                Err(e) => {
                    app.log.error(
                        "Playback",
                        &format!("Playback failed for guild {guild_id}: {e}"),
                    );
                    let fails = {
                        let mut map = self.play_fail.lock().await;
                        let c = map.entry(guild_id).or_insert(0);
                        *c += 1;
                        *c
                    };
                    let title = current.track.display_title().to_string();
                    if fails >= MAX_CONSECUTIVE_PLAY_FAILS {
                        self.play_fail.lock().await.remove(&guild_id);
                        self.cancel_current(guild_id).await;
                        self.record_playback_failure(app, guild_id, "playback.failed.stop", &title);
                        crate::player::side_effects::announce_text(
                            app,
                            guild_id,
                            &format!(
                                "⚠️ 재생이 연속 {fails}번 실패해서 멈췄어요. 잠시 뒤에 `/재생` 으로 다시 시도해 주세요. (마지막 실패: {title})"
                            ),
                        )
                        .await;
                        return;
                    }
                    self.record_playback_failure(app, guild_id, "playback.failed", &title);
                    crate::player::side_effects::announce_text(
                        app,
                        guild_id,
                        &format!("⚠️ 재생에 실패해서 다음 곡으로 넘어가요: {title}"),
                    )
                    .await;
                    // 망가진 곡을 강제로 지나친다(Track 반복이어도 같은 곡 재착석 방지).
                    app.player.skip(guild_id).await;
                    // 큐가 비었으면 자동추천을 시드해 자연종료 경로와 동일하게 이어지게 한다
                    // (안 그러면 마지막 곡이 실패했을 때 autoplay 가 켜져 있어도 침묵으로 끝난다).
                    crate::player::side_effects::ensure_autoplay(
                        app.clone(),
                        self.clone(),
                        guild_id,
                        true,
                    )
                    .await;
                    // 다음 곡으로 루프 재시도.
                }
            }
        }
    }

    async fn play_track(
        self: &Arc<Self>,
        app: &Arc<App>,
        guild_id: u64,
        channel_id: u64,
        state: &GuildPlayerState,
        item: &QueueItem,
    ) -> Result<PlayOutcome, String> {
        let manager = app.songbird.get().ok_or("songbird not ready")?.clone();
        let global = app.db.load_global_settings();

        // 1) 파일 준비 (캐시 미스 시 다운로드).
        let ytdlp = app.ytdlp();
        let (file_path, from_cache) = app
            .cache
            .prepare(
                &item.track,
                &ytdlp,
                global.cache_limit_gb,
                global.sponsorblock_remove,
            )
            .await?;
        if !from_cache {
            app.log.info(
                "Download",
                &format!(
                    "Downloaded {} for guild {guild_id}.",
                    item.track.cache_key()
                ),
            );
        }

        // 2) 음성 채널 합류. **봇이 이미 어떤 채널에 연결돼 있으면 절대 다른 채널로 옮기지 않는다.**
        //    songbird 의 라이브 연결(current_channel)이 권위 소스이고, 저장된 voice_channel_id 가
        //    stale 하더라도(명령자의 이전 방 등) 그걸 따라 봇이 자리를 옮겨선 안 된다(사용자 요구,
        //    2026-06-12). 미연결일 때만 저장된 채널로 최초 합류한다.
        let gid = songbird::id::GuildId(std::num::NonZeroU64::new(guild_id).ok_or("bad guild id")?);
        let existing = manager.get(gid);
        let already_connected = match &existing {
            Some(call) => call.lock().await.current_channel().is_some(),
            None => false,
        };
        let call = if already_connected {
            existing.expect("already_connected => Some")
        } else {
            let cid = songbird::id::ChannelId(
                std::num::NonZeroU64::new(channel_id).ok_or("bad channel id")?,
            );
            let c = manager
                .join(gid, cid)
                .await
                .map_err(|e| format!("voice join failed: {e}"))?;
            app.log.info(
                "Voice",
                &format!("Connected to voice channel {channel_id} in guild {guild_id}."),
            );
            c
        };

        // 3) ffmpeg 파이프 (검증된 인자 + 토글).
        let eff = app.player.effective_settings(guild_id);
        let offset = Duration::from_secs_f64(item.start_offset.as_secs_f64());
        let child = spawn_ffmpeg(
            &app.config.ffmpeg_path,
            &file_path,
            offset,
            eff.normalize_enabled,
            global.tweak_ffmpeg_fast_start,
            global.tweak_ffmpeg_direct_output,
        )?;
        let container = songbird::input::ChildContainer::new(vec![child]);
        let adapter = RawAdapter::new(ReadOnlySource::new(container), 48000, 2);
        let stream = AudioStream {
            input: Box::new(adapter) as Box<dyn songbird::input::core::io::MediaSource>,
        };
        let input = Input::Live(LiveInput::Raw(stream), None);

        app.log.info(
            "Playback",
            &format!(
                "Streaming starting for guild {guild_id}: file={} paused={} normalize={} volume={}% | tweaks: fastStart={} directOut={} (engine=songbird, bitrate={}k)",
                std::path::Path::new(&file_path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                state.is_paused,
                eff.normalize_enabled,
                state.effective_volume,
                if global.tweak_ffmpeg_fast_start { "ON" } else { "off" },
                if global.tweak_ffmpeg_direct_output { "ON" } else { "off" },
                global.voice_bitrate_kbps,
            ),
        );

        // 4) 재생 시작 (play_only: 이전 트랙 자동 중지) + 볼륨/일시정지 반영.
        let handle = {
            let mut call_guard = call.lock().await;
            call_guard.set_bitrate(songbird::driver::Bitrate::Bits(
                (global.voice_bitrate_kbps.clamp(32, 128) * 1000) as i32,
            ));
            call_guard.play_only_input(input)
        };
        let generation = self.gen_counter.fetch_add(1, Ordering::Relaxed);
        // 다음 곡 다운로드(수 초)가 진행되는 동안 사용자가 누른 일시정지/볼륨이 stale 스냅샷에
        // 묻히지 않도록, 실제 재생 직전에 최신 상태를 다시 읽어 반영한다.
        let live = app.player.get_state(guild_id).await;

        /* **그 사이에 곡이 바뀌었으면 이 소리를 내보내면 안 된다.**
         *
         * 여기까지 오는 데 다운로드가 수 초 걸린다. 그동안 스킵이 들어오면 대기열은 이미
         * 다음 곡으로 넘어가 있는데, 뒤늦게 끝난 이 다운로드가 그대로 세션을 덮어써서
         * **화면은 새 곡, 귀에는 옛 곡**이 된다. 403 이 연달아 나던 날처럼 곡이 빠르게
         * 넘어갈 때 특히 잘 벌어진다 — 실패한 곡을 지나치는 동안에도 다운로드는 계속 돈다.
         *
         * 위에서 볼륨·일시정지를 다시 읽는 것과 같은 이유인데, 정작 **어떤 곡인지**는
         * 안 보고 있었다. 여기서 손을 떼고 부른 쪽이 지금 곡으로 다시 맞추게 한다. */
        if live.current_item.as_ref().map(|c| c.id.as_str()) != Some(item.id.as_str()) {
            let _ = handle.stop();
            app.log.info(
                "Playback",
                &format!(
                    "'{}' 준비가 끝나기 전에 곡이 바뀌어서 내보내지 않았어요 (guild {guild_id}).",
                    item.track.display_title()
                ),
            );
            return Ok(PlayOutcome::Stale);
        }

        let _ = handle.set_volume((live.effective_volume.clamp(0, 200) as f32) / 100.0);
        if live.is_paused {
            let _ = handle.pause();
        }

        // 재생 횟수 집계 — 같은 곡 이어재생(재접속/워치독)은 item_id 가 같으므로 건너뛴다.
        {
            let mut counted = self.played_counted.lock().await;
            if counted.get(&guild_id).map(String::as_str) != Some(item.id.as_str()) {
                counted.insert(guild_id, item.id.clone());
                app.cache.record_play(&item.track.cache_key(), guild_id);
            }
        }

        // 5) 곡 종료/에러 이벤트 → 다음 곡 전이 / 이어재생.
        let end_handler = TrackEndHandler {
            app: app.clone(),
            coordinator: self.clone(),
            guild_id,
        };
        let _ = handle.add_event(Event::Track(TrackEvent::End), end_handler);
        let err_handler = TrackErrorHandler {
            app: app.clone(),
            coordinator: self.clone(),
            guild_id,
            start_offset: offset,
        };
        let _ = handle.add_event(Event::Track(TrackEvent::Error), err_handler);

        // 6) 세션 등록 + 스톨 워치독.
        {
            let mut sessions = self.sessions.lock().await;
            let retry = sessions
                .remove(&guild_id)
                .map(|s| s.retry_count)
                .unwrap_or(0);
            sessions.insert(
                guild_id,
                Session {
                    handle: handle.clone(),
                    item_id: item.id.clone(),
                    generation,
                    start_offset: offset,
                    retry_count: retry,
                    // 0초 지점의 시각. `-ss` 로 건너뛴 만큼은 이미 흘러간 것으로 친다.
                    started_utc: chrono::Utc::now()
                        - chrono::Duration::from_std(offset).unwrap_or_default(),
                },
            );
        }
        self.spawn_stall_watchdog(app.clone(), guild_id, generation);

        // 7) 부가 동작: 다음 곡 프리페치 + autoplay preview + 알림.
        crate::player::side_effects::on_track_started(
            app.clone(),
            self.clone(),
            guild_id,
            item.clone(),
        );

        app.log.info(
            "Playback",
            &format!("First PCM frame path armed for guild {guild_id} (songbird driver)."),
        );
        Ok(PlayOutcome::Started)
    }

    /// 일시정지가 아닌데 10초간 position 이 안 움직이면 죽은 것으로 보고 그 위치에서 이어재생.
    fn spawn_stall_watchdog(self: &Arc<Self>, app: Arc<App>, guild_id: u64, generation: u64) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            let mut last_pos = Duration::ZERO;
            let mut stalled_for = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let info = {
                    let sessions = coordinator.sessions.lock().await;
                    match sessions.get(&guild_id) {
                        // 세대 번호로 식별 — seek/replay 로 같은 곡을 다시 시작한 경우
                        // 옛 워치독이 새 세션을 감시하는 중복을 막는다.
                        Some(s) if s.generation == generation => s.handle.get_info().await.ok(),
                        _ => return, // 다른 곡/새 세션으로 넘어감 — 워치독 종료.
                    }
                };
                let Some(info) = info else { return }; // 트랙 종료됨.
                use songbird::tracks::PlayMode;
                match info.playing {
                    PlayMode::Pause => {
                        stalled_for = 0;
                        continue;
                    }
                    PlayMode::Play => {}
                    _ => return, // End/Stop — 정상 경로가 처리.
                }
                if info.position == last_pos {
                    stalled_for += 2;
                    if stalled_for >= 10 {
                        let resume_at = {
                            let sessions = coordinator.sessions.lock().await;
                            sessions
                                .get(&guild_id)
                                .map(|s| s.start_offset + info.position)
                                .unwrap_or(info.position)
                        };
                        app.log.warn(
                            "Playback",
                            &format!("Audio pipeline stalled for {stalled_for}s (guild {guild_id}) — forcing reconnect + resume at {resume_at:?}."),
                        );
                        coordinator
                            .schedule_resume(&app, guild_id, resume_at, true)
                            .await;
                        return;
                    }
                } else {
                    last_pos = info.position;
                    stalled_for = 0;
                }
            }
        });
    }

    /// 순단/스톨 후 같은 곡을 마지막 위치부터 다시 시작 (최대 연속 3회).
    pub async fn schedule_resume(
        self: &Arc<Self>,
        app: &Arc<App>,
        guild_id: u64,
        resume_at: Duration,
        force_reconnect: bool,
    ) {
        let retry = {
            let mut sessions = self.sessions.lock().await;
            match sessions.get_mut(&guild_id) {
                Some(s) => {
                    s.retry_count += 1;
                    s.retry_count
                }
                None => 1,
            }
        };
        if retry > 3 {
            app.log.error(
                "Playback",
                &format!("Voice resume retries exhausted for guild {guild_id} (3/3). /재생 또는 /다시재생 으로 재시작하세요."),
            );
            // 서버 로그(KST)는 사용자가 못 보므로 채널에도 안내한다.
            crate::player::side_effects::announce_text(
                app,
                guild_id,
                "⚠️ 음성 재연결을 3번 시도했지만 안 됐어요. `/다시재생` 또는 `/재생` 으로 다시 시작해 주세요.",
            )
            .await;
            return;
        }
        app.log.warn(
            "Playback",
            &format!(
                "Auto-resuming guild {guild_id} at {}s in 2s (retry {retry}/3).",
                resume_at.as_secs()
            ),
        );
        let app = app.clone();
        let coordinator = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if force_reconnect || retry >= 2 {
                coordinator.leave_voice(&app, guild_id).await;
            }
            let _ = app
                .player
                .set_current_start_offset(guild_id, CsTimeSpan(resume_at))
                .await;
            coordinator.sync_guild(&app, guild_id).await;
        });
    }

    /// 곡이 성공적으로 일정 시간 재생되면 재시도 카운터 리셋.
    pub async fn reset_retry(&self, guild_id: u64) {
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get_mut(&guild_id) {
            s.retry_count = 0;
        }
    }
}

// ───────── ffmpeg ─────────

fn spawn_ffmpeg(
    ffmpeg: &str,
    file: &str,
    offset: Duration,
    normalize: bool,
    fast_start: bool,
    direct_output: bool,
) -> Result<std::process::Child, String> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];
    if fast_start {
        args.extend([
            "-probesize".into(),
            "32k".into(),
            "-analyzeduration".into(),
            "0".into(),
            "-fflags".into(),
            "+nobuffer".into(),
        ]);
    }
    if offset > Duration::ZERO {
        args.push("-ss".into());
        args.push(format!("{}", offset.as_secs_f64()));
    }
    args.push("-i".into());
    args.push(file.to_string());
    if normalize {
        args.push("-af".into());
        args.push("dynaudnorm=f=200:g=15".into());
    }
    // songbird RawAdapter/RawReader 는 f32 인터리브 PCM 전용 (s16le 는 C# Discord.Net 시절 포맷).
    args.extend([
        "-ac".into(),
        "2".into(),
        "-f".into(),
        "f32le".into(),
        "-ar".into(),
        "48000".into(),
    ]);
    if direct_output {
        args.extend([
            "-avioflags".into(),
            "direct".into(),
            "-flush_packets".into(),
            "1".into(),
        ]);
    }
    args.push("pipe:1".into());

    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map_err(|e| format!("ffmpeg 을(를) 실행하지 못했어요: {e}"))
}

// ───────── 이벤트 핸들러 ─────────

struct TrackEndHandler {
    app: Arc<App>,
    coordinator: Arc<Coordinator>,
    guild_id: u64,
}

#[serenity::async_trait]
impl VoiceEventHandler for TrackEndHandler {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        use songbird::tracks::PlayMode;
        let EventContext::Track(list) = ctx else {
            return None;
        };
        // 곡이 "실제로 끝났을 때(End)"만 다음 곡으로 넘긴다. 곡 길이를 다 재생한 자연 종료는
        // PlayMode::End 이고, seek/replay/skip/정지/교체로 인한 수동 중단은 PlayMode::Stop,
        // 디코드/네트워크 실패는 PlayMode::Errored(이어재생 핸들러가 담당)다.
        //   - Stop 을 무시 → seek/replay 시 멈춘 옛 핸들의 stale End 가 곡을 한 칸 더 넘기던 버그 방지.
        //   - End 는 세션 식별과 무관하게 항상 처리 → 자연 종료가 안 넘어가 멈추던 회귀 방지(2026-06-21).
        if !list.iter().any(|(s, _)| matches!(s.playing, PlayMode::End)) {
            return None;
        }
        let state = self.app.player.advance(self.guild_id).await;
        self.app.log.info(
            "Playback",
            &format!("Completed track for guild {} (advanced).", self.guild_id),
        );
        // autoplay 이어가기 (큐 소진 시 preview/추천 사용).
        crate::player::side_effects::ensure_autoplay(
            self.app.clone(),
            self.coordinator.clone(),
            self.guild_id,
            true,
        )
        .await;
        if state.current_item.is_some()
            || self
                .app
                .player
                .get_state(self.guild_id)
                .await
                .current_item
                .is_some()
        {
            self.coordinator.sync_guild(&self.app, self.guild_id).await;
        }
        None
    }
}

struct TrackErrorHandler {
    app: Arc<App>,
    coordinator: Arc<Coordinator>,
    guild_id: u64,
    start_offset: Duration,
}

#[serenity::async_trait]
impl VoiceEventHandler for TrackErrorHandler {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        use songbird::tracks::PlayMode;
        let (position, detail) = match ctx {
            EventContext::Track(list) => {
                let pos = list
                    .first()
                    .map(|(s, _)| s.position)
                    .unwrap_or(Duration::ZERO);
                let detail = list
                    .iter()
                    .find_map(|(s, _)| match &s.playing {
                        PlayMode::Errored(e) => Some(format!("{e:?}")),
                        _ => None,
                    })
                    .unwrap_or_else(|| "unknown".into());
                (pos, detail)
            }
            _ => (Duration::ZERO, "unknown".into()),
        };
        self.app.log.warn(
            "Playback",
            &format!(
                "Track errored mid-play for guild {} — scheduling resume. cause={detail}",
                self.guild_id
            ),
        );
        self.coordinator
            .schedule_resume(
                &self.app,
                self.guild_id,
                self.start_offset + position,
                false,
            )
            .await;
        None
    }
}
