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
}

pub struct Coordinator {
    sessions: Mutex<HashMap<u64, Session>>,
    /// 길드별 마지막으로 재생 횟수를 +1 한 item_id. 스톨 워치독/게이트웨이 재접속이
    /// 같은 곡으로 play_track 을 다시 호출해도 중복 카운트하지 않도록 막는 게이트.
    played_counted: Mutex<HashMap<u64, String>>,
    /// 단조 증가 세대 카운터 (play_track 마다 +1).
    gen_counter: AtomicU64,
    /// 길드별 연속 재생 실패 횟수. 다운로드/ffmpeg 실패가 반복될 때 무한 스킵을 막는다.
    play_fail: Mutex<HashMap<u64, u32>>,
}

const MAX_CONSECUTIVE_PLAY_FAILS: u32 = 5;

impl Coordinator {
    pub fn new() -> Coordinator {
        Coordinator {
            sessions: Mutex::new(HashMap::new()),
            played_counted: Mutex::new(HashMap::new()),
            gen_counter: AtomicU64::new(1),
            play_fail: Mutex::new(HashMap::new()),
        }
    }

    /// 현재 곡의 재생 위치 (시작 오프셋 포함). /현재곡 진행바용.
    pub async fn current_position(&self, guild_id: u64) -> Option<Duration> {
        let sessions = self.sessions.lock().await;
        let s = sessions.get(&guild_id)?;
        let info = s.handle.get_info().await.ok()?;
        Some(s.start_offset + info.position)
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
        let sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get(&guild_id) {
            if paused {
                let _ = s.handle.pause();
            } else {
                let _ = s.handle.play();
            }
        }
    }

    pub async fn cancel_current(&self, guild_id: u64) {
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.remove(&guild_id) {
            let _ = s.handle.stop();
        }
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
        loop {
            let state = app.player.get_state(guild_id).await;

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
                Ok(()) => {
                    self.play_fail.lock().await.remove(&guild_id);
                    return;
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
    ) -> Result<(), String> {
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
        Ok(())
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
