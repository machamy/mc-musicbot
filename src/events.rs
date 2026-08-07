//! Discord 게이트웨이 이벤트 핸들러: Ready(명령등록/길드메타/아바타), 상호작용 디스패치,
//! 음성 상태 변화(빈 채널 정책), 길드 입퇴장 메타 동기화.

use crate::app::App;
use crate::commands::handlers;
use crate::models::{EmptyVoiceChannelPolicy, GuildMetadata};
use serenity::all::{
    Context, EventHandler, Guild, GuildId, Interaction, Ready, UnavailableGuild, VoiceState,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Handler {
    pub app: Arc<App>,
    pub ready_once: AtomicBool,
}

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        let app = self.app.clone();
        let _ = app.http.set(ctx.http.clone());
        let _ = app.discord_cache.set(ctx.cache.clone());
        if self.ready_once.swap(true, Ordering::SeqCst) {
            app.log.info("Bot", "Gateway resumed.");
            return;
        }
        app.log.info(
            "Bot",
            &format!(
                "Ready handler started (build {}). Registering commands...",
                app.build_id
            ),
        );
        handlers::register_commands(&app, &ctx).await;
        app.log.info(
            "Bot",
            &format!(
                "Discord bot connected as {} (engine=serenity+songbird).",
                ready.user.name
            ),
        );

        // 길드 메타 동기화.
        for gid in ctx.cache.guilds() {
            if let Some(g) = ctx.cache.guild(gid) {
                sync_guild_metadata(&app, &g);
            }
        }

        // 아바타 자동 적용 (assets/avatar.png 해시 변경 시 1회).
        apply_pending_avatar(&app, &ctx).await;

        // 로그 보관 정리.
        let retention = app.db.load_global_settings().log_retention_days as i64;
        app.log.prune(retention);

        // 업데이트로 껐다면 끊긴 지점부터 잇는다 (§24).
        // **여기여야 한다** — 길드 캐시와 음성 상태가 준비된 뒤라야 다시 들어갈 수 있다.
        crate::shutdown::resume_after_restart(&app).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(cmd) => {
                let app = self.app.clone();
                tokio::spawn(async move {
                    handlers::handle_command(app, ctx, cmd).await;
                });
            }
            Interaction::Component(comp) => {
                let app = self.app.clone();
                tokio::spawn(async move {
                    handlers::handle_button(app, ctx, comp).await;
                });
            }
            _ => {}
        }
    }

    async fn guild_create(&self, _ctx: Context, guild: Guild, _is_new: Option<bool>) {
        sync_guild_metadata(&self.app, &guild);
    }

    async fn guild_delete(
        &self,
        _ctx: Context,
        incomplete: UnavailableGuild,
        _full: Option<Guild>,
    ) {
        let gid = incomplete.id.get();
        self.app.db.delete_guild_metadata(gid);
        self.app.announce_channels.lock().unwrap().remove(&gid);
        self.app.last_np_message.lock().unwrap().remove(&gid);
    }

    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        let Some(guild_id) = new.guild_id else { return };
        evaluate_auto_leave(self.app.clone(), ctx, guild_id).await;
    }
}

fn sync_guild_metadata(app: &Arc<App>, guild: &Guild) {
    let meta = GuildMetadata {
        guild_id: guild.id.get(),
        name: guild.name.clone(),
        icon_url: guild.icon_url(),
        member_count: if guild.member_count > 0 {
            Some(guild.member_count as i32)
        } else {
            None
        },
        last_seen_utc: chrono::Utc::now().to_rfc3339(),
    };
    app.db.upsert_guild_metadata(&meta);
}

/// assets/avatar.png 해시가 마지막 적용분과 다르면 봇 아바타를 갱신한다.
async fn apply_pending_avatar(app: &Arc<App>, ctx: &Context) {
    use sha2::Digest;
    let avatar_path = app.config.portable_root.join("assets").join("avatar.png");
    let Ok(bytes) = std::fs::read(&avatar_path) else {
        return;
    };
    let hash: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let marker = app.config.data_root.join("avatar-applied.txt");
    let applied = std::fs::read_to_string(&marker).unwrap_or_default();
    if applied.trim().eq_ignore_ascii_case(&hash) {
        return;
    }
    let attachment = serenity::builder::CreateAttachment::bytes(bytes, "avatar.png");
    let edit = serenity::builder::EditProfile::new().avatar(&attachment);
    match ctx.http.get_current_user().await {
        Ok(mut user) => {
            if user.edit(&ctx.http, edit).await.is_ok() {
                let _ = std::fs::write(&marker, &hash);
                app.log
                    .info("Bot", "봇 아바타를 assets/avatar.png 로 갱신했습니다.");
            } else {
                app.log
                    .warn("Bot", "봇 아바타 갱신 실패 (다음 시작 시 재시도).");
            }
        }
        Err(_) => {}
    }
}

/// 빈 음성 채널 정책 — C# EvaluateAutoLeaveAsync 포팅 (디바운스 + 3정책).
async fn evaluate_auto_leave(app: Arc<App>, ctx: Context, guild_id: GuildId) {
    let gid = guild_id.get();

    // 봇이 있는 채널에 사람(봇 제외)이 남아 있는지 검사.
    let (bot_in_channel, humans_present) = {
        let Some(guild) = ctx.cache.guild(guild_id) else {
            return;
        };
        let bot_id = ctx.cache.current_user().id;
        let Some(bot_vc) = guild.voice_states.get(&bot_id).and_then(|vs| vs.channel_id) else {
            // 봇이 어느 채널에도 없음 — 타이머 취소.
            app.pending_leaves.lock().unwrap().remove(&gid);
            return;
        };
        let humans = guild
            .voice_states
            .iter()
            .filter(|(uid, vs)| vs.channel_id == Some(bot_vc) && **uid != bot_id)
            .filter(|(uid, _)| {
                guild.members.get(*uid).map(|m| !m.user.bot).unwrap_or(true) // 캐시에 없으면 사람으로 간주 (보수적).
            })
            .count();
        (true, humans > 0)
    };

    if !bot_in_channel || humans_present {
        app.pending_leaves.lock().unwrap().remove(&gid);
        return;
    }

    let settings = app.db.load_global_settings();
    if !settings.auto_leave_when_empty {
        return;
    }
    if settings.empty_voice_policy == EmptyVoiceChannelPolicy::DoNothing {
        app.log.info(
            "Voice",
            &format!("Empty voice policy = DoNothing; leaving guild {gid} playback untouched."),
        );
        return;
    }

    // 디바운스: 세대 카운터로 최신 타이머만 발화.
    let generation = {
        let mut map = app.pending_leaves.lock().unwrap();
        let next_gen = map.get(&gid).copied().unwrap_or(0) + 1;
        map.insert(gid, next_gen);
        next_gen
    };
    let delay = settings.auto_leave_delay_seconds.clamp(5, 3600) as u64;
    let policy = settings.empty_voice_policy;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        // 아직 같은 세대인지 + 여전히 비어 있는지 재확인.
        {
            let map = app.pending_leaves.lock().unwrap();
            if map.get(&gid).copied() != Some(generation) {
                return;
            }
        }
        let still_empty = {
            let Some(guild) = ctx.cache.guild(guild_id) else {
                return;
            };
            let bot_id = ctx.cache.current_user().id;
            match guild.voice_states.get(&bot_id).and_then(|vs| vs.channel_id) {
                None => return,
                Some(bot_vc) => !guild.voice_states.iter().any(|(uid, vs)| {
                    vs.channel_id == Some(bot_vc)
                        && *uid != bot_id
                        && guild.members.get(uid).map(|m| !m.user.bot).unwrap_or(true)
                }),
            }
        };
        if !still_empty {
            return;
        }
        match policy {
            EmptyVoiceChannelPolicy::StopPlayback => {
                app.player.stop(gid).await;
                app.coordinator.cancel_current(gid).await;
                app.log.info("Voice", &format!("Empty voice policy = StopPlayback; stopped playback but kept voice connection alive in guild {gid}."));
            }
            _ => {
                app.player.stop(gid).await;
                app.player.disconnect_voice(gid).await;
                app.coordinator.leave_voice(&app, gid).await;
                app.log.info("Voice", &format!("Empty voice policy = AutoLeave; auto-left empty voice channel in guild {gid}."));
            }
        }
        app.pending_leaves.lock().unwrap().remove(&gid);
    });
}
