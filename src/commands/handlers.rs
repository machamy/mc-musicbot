//! 슬래시 명령/버튼 핸들러 — C# Program.cs 의 HandleCommandAsync + HandlePlaybackButtonAsync 포팅.
//! 음성 채널 바인딩 규칙: 재생 시작 명령은 항상 명령자 방으로, 제어 명령은 같은 방(또는 관리자)만.

use crate::app::{App, SearchSession};
use crate::commands::{catalog, embeds};
use crate::media::resolver::{self, Resolved};
use crate::models::*;
use crate::player::manager::CancelOutcome;
use crate::player::side_effects;
use crate::remote::{PermissionRule, RemoteGuildSettings};
use crate::web::remote::{MemberContext, permission_allowed};
use serenity::all::{
    CommandInteraction, CommandOptionType, ComponentInteraction, ComponentInteractionDataKind,
    Context, CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage,
    GuildId, Permissions,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ───────── 명령 등록 ─────────

pub fn build_commands() -> Vec<CreateCommand> {
    catalog::ALL
        .iter()
        // 초성 전용 별칭(ㅈㅅ, ㅂㄹㅈㅅ …)은 등록하지 않는다 — 사용자 요청으로 비활성화.
        .filter(|def| !catalog::is_chosung_alias(def.name))
        .map(|def| {
            let mut cmd = CreateCommand::new(def.name).description(def.description);
            match def.canonical {
                "play" | "playnow" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "input",
                            "곡/플레이리스트 URL 또는 검색어",
                        )
                        .required(true),
                    );
                }
                "search" | "scsearch" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(CommandOptionType::String, "query", "검색어")
                            .required(true),
                    );
                }
                "repeat" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(CommandOptionType::String, "mode", "반복 모드")
                            .required(true)
                            .add_string_choice("Off", "off")
                            .add_string_choice("Track", "track")
                            .add_string_choice("Queue", "queue"),
                    );
                }
                "autoplay" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Boolean,
                            "enabled",
                            "자동추천 켜기",
                        )
                        .required(true),
                    );
                }
                "normalize" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Boolean,
                            "enabled",
                            "볼륨 평준화 켜기",
                        )
                        .required(true),
                    );
                }
                "move" => {
                    cmd = cmd
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::Integer,
                                "from",
                                "원래 순번 (1부터)",
                            )
                            .min_int_value(1)
                            .required(true),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::Integer,
                                "to",
                                "이동할 순번 (1부터)",
                            )
                            .min_int_value(1)
                            .required(true),
                        );
                }
                "remove" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "index",
                            "제거할 순번 (1부터)",
                        )
                        .min_int_value(1)
                        .required(true),
                    );
                }
                "skipto" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(
                            CommandOptionType::Integer,
                            "position",
                            "건너뛸 순번 (1부터)",
                        )
                        .min_int_value(1)
                        .required(true),
                    );
                }
                "volume" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(CommandOptionType::Integer, "level", "볼륨 0~200")
                            .min_int_value(0)
                            .max_int_value(200)
                            .required(true),
                    );
                }
                "seek" => {
                    cmd = cmd.add_option(
                        CreateCommandOption::new(
                            CommandOptionType::String,
                            "time",
                            "이동할 시간 (예: 1:23 또는 83)",
                        )
                        .required(true),
                    );
                }
                "playlist" => {
                    cmd = cmd
                        .add_option(CreateCommandOption::new(
                            CommandOptionType::SubCommand,
                            "list",
                            "플레이리스트 목록",
                        ))
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "create",
                                "새 플레이리스트 생성",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(CommandOptionType::String, "name", "이름")
                                    .required(true),
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "scope",
                                    "범위",
                                )
                                .add_string_choice("길드", "guild")
                                .add_string_choice("전역", "global"),
                            ),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "delete",
                                "플레이리스트 삭제",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(CommandOptionType::String, "name", "이름")
                                    .required(true),
                            ),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "rename",
                                "이름 변경",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "name",
                                    "기존 이름",
                                )
                                .required(true),
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "newname",
                                    "새 이름",
                                )
                                .required(true),
                            ),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "add",
                                "곡 추가",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "name",
                                    "플레이리스트 이름",
                                )
                                .required(true),
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "input",
                                    "곡 URL 또는 검색어",
                                )
                                .required(true),
                            ),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "remove",
                                "곡 제거",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "name",
                                    "플레이리스트 이름",
                                )
                                .required(true),
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::Integer,
                                    "index",
                                    "제거할 순번 (1부터)",
                                )
                                .required(true),
                            ),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "show",
                                "내용 보기",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "name",
                                    "플레이리스트 이름",
                                )
                                .required(true),
                            ),
                        )
                        .add_option(
                            CreateCommandOption::new(
                                CommandOptionType::SubCommand,
                                "load",
                                "대기열에 적재",
                            )
                            .add_sub_option(
                                CreateCommandOption::new(
                                    CommandOptionType::String,
                                    "name",
                                    "플레이리스트 이름",
                                )
                                .required(true),
                            ),
                        );
                }
                _ => {}
            }
            cmd
        })
        .collect()
}

pub async fn register_commands(app: &Arc<App>, ctx: &Context) {
    let commands = build_commands();
    let result = match app.config.register_guild_id {
        Some(gid) => GuildId::new(gid)
            .set_commands(&ctx.http, commands)
            .await
            .map(|v| v.len()),
        None => serenity::all::Command::set_global_commands(&ctx.http, commands)
            .await
            .map(|v| v.len()),
    };
    match result {
        Ok(n) => app
            .log
            .info("Bot", &format!("Registered {n} slash commands.")),
        Err(e) => app
            .log
            .error("Bot", &format!("Slash command registration failed: {e}")),
    }
}

// ───────── 권한/바인딩 ─────────

fn requester_voice_channel(ctx: &Context, guild_id: u64, user_id: u64) -> Option<u64> {
    ctx.cache
        .guild(GuildId::new(guild_id))
        .and_then(|g| {
            g.voice_states
                .get(&serenity::all::UserId::new(user_id))
                .and_then(|vs| vs.channel_id)
        })
        .map(|c| c.get())
}

/// 봇이 **실제로** 연결된 음성 채널 — songbird 의 라이브 연결 상태가 권위 소스다.
/// 게이트웨이 `voice_states` 캐시는 stale 할 수 있어("봇이 방에 있나" 가 틀릴 수 있음)
/// 채널 이동 여부 판단에는 반드시 이 함수를 쓴다. 미연결이면 `None`.
async fn bot_live_voice_channel(app: &Arc<App>, guild_id: u64) -> Option<u64> {
    let manager = app.songbird.get()?;
    let gid = songbird::id::GuildId(std::num::NonZeroU64::new(guild_id)?);
    let call = manager.get(gid)?;
    let chan = call.lock().await.current_channel();
    chan.map(|c| c.0.get())
}

/// 이 사람이 **지금 봇과 같은 방에** 있는가 — 조작 권한의 공통 판정 (v3 §16 B1).
///
/// 근거는 `bot_live_voice_channel` 이 준 **라이브 연결 하나뿐**이다. 저장된
/// `player.voice_channel_id` 로 폴백하면 안 된다. 봇이 강제 퇴장·재시작·네트워크 끊김으로
/// 빠져나가도 그 값은 남아서, (1) 봇이 없는 방에 남아 있던 사람만 계속 버튼을 쥐고,
/// (2) 봇이 아무 방에도 없는데 "봇과 같은 음성 채널에 있어야 해요" 라고 거절하게 된다.
/// 둘 다 화면이 거짓말하는 쪽이다.
fn in_bot_voice_channel(bot_live: Option<u64>, requester_vc: Option<u64>) -> bool {
    bot_live.is_some() && bot_live == requester_vc
}

fn is_admin(cmd: &CommandInteraction) -> bool {
    cmd.member
        .as_ref()
        .and_then(|m| m.permissions)
        .map(|p| p.contains(Permissions::ADMINISTRATOR) || p.contains(Permissions::MANAGE_GUILD))
        .unwrap_or(false)
}

/// 재생 제어 권한 검사 + (재생 시작 명령은) 명령자 방으로 바인딩.
async fn ensure_playback_allowed(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    allow_initial_binding: bool,
) -> Result<(), String> {
    let guild_id = cmd
        .guild_id
        .ok_or("이 명령은 서버 안에서만 쓸 수 있어요.")?
        .get();
    let requester_vc = requester_voice_channel(ctx, guild_id, cmd.user.id.get());
    let admin = is_admin(cmd);
    // 봇이 실제로 연결된 채널(songbird 라이브 상태 = 권위 소스). 캐시 기반 판단 금지.
    let bot_live = bot_live_voice_channel(app, guild_id).await;

    if allow_initial_binding {
        // 봇이 이미 어떤 음성 채널에 있으면 **어떤 경우에도 스스로 옮기지 않는다** —
        // 곡은 현재 방의 큐로만 추가된다. (명령자가 다른 방/이전 방에 있어도 이동 금지.)
        if bot_live.is_some() {
            return Ok(());
        }
        // 봇이 아무 방에도 없을 때만 명령자 방으로 최초 합류.
        let Some(rvc) = requester_vc else {
            return Err("먼저 음성 채널에 들어간 뒤에 재생 명령을 써 주세요.".into());
        };
        app.log.info(
            "Voice",
            &format!("Joining requester channel {rvc} for guild {guild_id} (bot not connected)."),
        );
        app.player.connect_voice(guild_id, rvc).await;
        return Ok(());
    }

    // "봇과 같은 방인가" 는 **라이브 연결로만** 판단한다 (v3 §16 B1). 저장값으로 폴백하면
    // 봇이 강제 퇴장·재접속으로 빠진 뒤에도 그 방에 남아 있던 사람만 계속 조작할 수 있고,
    // 반대로 봇이 아무 방에도 없는데 "봇과 같은 음성 채널에 있어야 해요" 라고 거절하게 된다.
    let same_channel = in_bot_voice_channel(bot_live, requester_vc);
    if same_channel || admin {
        Ok(())
    } else {
        Err("재생을 조작하려면 봇과 같은 음성 채널에 있어야 해요. 서버 관리자는 어디서든 할 수 있어요.".into())
    }
}

// ───────── 리모컨과 같은 권한 판정 (v3 §8.3 · §23.3 · §24.3) ─────────

/// 자동 재생 권한 키. 리모컨과 **같은 키·같은 판정 함수**를 쓴다 —
/// "웹에서는 되는데 명령으로는 안 돼요" 만큼 설명하기 어려운 게 없다.
const AUTOPLAY_KEY: &str = "autoplay";

/// 리모컨 판정 함수(`web::remote::permission_allowed`)에 넣을 멤버 정보를 만든다.
///
/// `same_voice_channel` 은 **songbird 의 라이브 연결**로만 판단한다.
/// 저장된 `voice_channel_id` 를 쓰면 봇이 이미 나갔는데도 "같은 채널"이 되는
/// v3 §16 B1 함정이 명령 쪽에 그대로 재현된다.
async fn member_context(
    app: &Arc<App>,
    ctx: &Context,
    guild_id: u64,
    user_id: u64,
    admin: bool,
    role_ids: Vec<u64>,
) -> MemberContext {
    let bot_live = bot_live_voice_channel(app, guild_id).await;
    let requester_vc = requester_voice_channel(ctx, guild_id, user_id);
    MemberContext {
        is_admin: admin,
        same_voice_channel: in_bot_voice_channel(bot_live, requester_vc),
        role_ids,
    }
}

fn role_ids_of(member: Option<&serenity::all::Member>) -> Vec<u64> {
    member
        .map(|m| m.roles.iter().map(|role| role.get()).collect())
        .unwrap_or_default()
}

/// 이 서버의 자동 재생 규칙. `autoplay_rule` 이 아직 없는 설정 파일이면
/// v3 §8.3 의 기본값(모든 멤버)으로 본다 — 기본이 조여지면 안 된다.
fn autoplay_rule(settings: &RemoteGuildSettings) -> PermissionRule {
    settings
        .rule_for(AUTOPLAY_KEY)
        .unwrap_or(PermissionRule::GuildMember)
}

/// 막혔을 때 **왜** 안 되는지 + **누가** 되는지 (v3 §23.3).
/// `권한 없음` 은 답이 아니라서, 조건과 통과 대상을 같이 말한다.
///
/// `console` 은 **관리자에게만** 붙이는 관리 콘솔 링크다. 일반 멤버에게 못 들어가는
/// 링크를 보여 주는 건 놀리는 것이라 그때는 `None` 을 넣는다(§23.3-4).
fn denied_reason(
    ctx: &Context,
    guild_id: u64,
    what: &str,
    key: &str,
    rule: PermissionRule,
    settings: &RemoteGuildSettings,
    members_known: bool,
    console: Option<String>,
) -> String {
    match rule {
        // 관리자도 못 지나가는 유일한 규칙이라, 여기서만 "어디서 바꾸는지"가 쓸모 있다.
        PermissionRule::Disabled => {
            let where_to = console
                .map(|url| format!(" 관리 콘솔에서 다시 켤 수 있어요 → {url}"))
                .unwrap_or_default();
            format!("{what}은(는) 이 서버에서 꺼 뒀어요.{where_to}")
        }
        PermissionRule::SameVoiceChannel => {
            format!("{what}은(는) 봇과 같은 음성 채널에 있어야 바꿀 수 있어요.")
        }
        PermissionRule::Administrator => {
            let who = manager_count(ctx, guild_id, members_known)
                .map(|n| format!(" 지금은 서버 관리자 {n}명이 쓸 수 있어요."))
                .unwrap_or_default();
            format!("{what}은(는) 서버 관리자만 바꿀 수 있어요.{who}")
        }
        PermissionRule::ConfiguredRole => {
            let (names, holders) = configured_role_info(ctx, guild_id, settings.roles_for(key), members_known);
            if names.is_empty() {
                return format!(
                    "{what}에 쓸 역할이 아직 지정되지 않았어요. 서버 관리자가 관리 콘솔에서 정해야 해요."
                );
            }
            // 사람 이름은 나열하지 않는다 — 그러면 그 사람들이 부탁 받는 창구가 된다(§23.3).
            let who = holders
                .map(|n| format!(" ({n}명)"))
                .unwrap_or_default();
            format!(
                "{what}은(는) {} 역할이 있어야 바꿀 수 있어요.{who}",
                names.join(" · ")
            )
        }
        PermissionRule::GuildMember => format!("{what}을(를) 지금은 바꿀 수 없어요."),
    }
}

/// 지정 역할 이름과, 그 역할을 가진 사람 수. 멤버 목록을 못 받는 서버면 인원수는 `None`.
fn configured_role_info(
    ctx: &Context,
    guild_id: u64,
    role_ids: &[u64],
    members_known: bool,
) -> (Vec<String>, Option<usize>) {
    let Some(guild) = ctx.cache.guild(GuildId::new(guild_id)) else {
        return (Vec::new(), None);
    };
    let wanted: HashSet<u64> = role_ids.iter().copied().collect();
    let names: Vec<String> = guild
        .roles
        .iter()
        .filter(|(id, _)| wanted.contains(&id.get()))
        .map(|(_, role)| format!("@{}", role.name))
        .collect();
    if !members_known || guild.members.is_empty() {
        return (names, None);
    }
    let holders = guild
        .members
        .values()
        .filter(|m| !m.user.bot && m.roles.iter().any(|role| wanted.contains(&role.get())))
        .count();
    (names, Some(holders))
}

/// 이 서버에서 관리자로 통과하는 사람 수. 멤버 목록이 없으면 `None` — 모르면 모른다고 한다.
fn manager_count(ctx: &Context, guild_id: u64, members_known: bool) -> Option<usize> {
    if !members_known {
        return None;
    }
    let guild = ctx.cache.guild(GuildId::new(guild_id))?;
    if guild.members.is_empty() {
        return None;
    }
    let admin_roles: HashSet<u64> = guild
        .roles
        .iter()
        .filter(|(_, role)| {
            role.permissions.contains(Permissions::ADMINISTRATOR)
                || role.permissions.contains(Permissions::MANAGE_GUILD)
        })
        .map(|(id, _)| id.get())
        .collect();
    let owner = guild.owner_id;
    Some(
        guild
            .members
            .values()
            .filter(|m| {
                !m.user.bot
                    && (m.user.id == owner
                        || m.roles.iter().any(|role| admin_roles.contains(&role.get())))
            })
            .count(),
    )
}

/// 자동 재생을 바꿀 수 있는지. 기본이 `GuildMember` 라 **누구나** 켜고 끌 수 있고
/// (v3 §24.3), 관리자가 조여 뒀으면 그때만 막힌다.
async fn ensure_autoplay_allowed(
    app: &Arc<App>,
    ctx: &Context,
    guild_id: u64,
    user_id: u64,
    admin: bool,
    role_ids: Vec<u64>,
) -> Result<(), String> {
    let settings = app.player.cached_settings(guild_id);
    let rule = autoplay_rule(&settings);
    let member = member_context(app, ctx, guild_id, user_id, admin, role_ids).await;
    if permission_allowed(AUTOPLAY_KEY, rule, &settings, &member) {
        return Ok(());
    }
    let members_known = app
        .intent_status
        .read()
        .map(|status| status.members)
        .unwrap_or(false);
    let console = if admin {
        app.public_base_url
            .get()
            .map(|base| format!("{}/music/guilds/{guild_id}/admin", base.trim_end_matches('/')))
    } else {
        None
    };
    Err(denied_reason(
        ctx,
        guild_id,
        "자동 재생",
        AUTOPLAY_KEY,
        rule,
        &settings,
        members_known,
        console,
    ))
}

// ───────── 옵션 헬퍼 ─────────

fn opt_str(cmd: &CommandInteraction, name: &str) -> Option<String> {
    cmd.data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| o.value.as_str().map(|s| s.to_string()))
}

fn opt_int(cmd: &CommandInteraction, name: &str) -> Option<i64> {
    cmd.data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| o.value.as_i64())
}

fn opt_bool(cmd: &CommandInteraction, name: &str) -> Option<bool> {
    cmd.data
        .options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| o.value.as_bool())
}

async fn respond_text(ctx: &Context, cmd: &CommandInteraction, text: &str, ephemeral: bool) {
    let _ = cmd
        .create_followup(
            &ctx.http,
            CreateInteractionResponseFollowup::new()
                .content(text.to_string())
                .ephemeral(ephemeral),
        )
        .await;
}

/// 텍스트 + 그 곡만 취소하는 ✖ 버튼. /재생·/바로재생·검색 선택의 응답에 사용.
async fn respond_with_cancel(ctx: &Context, cmd: &CommandInteraction, text: &str, item_id: &str) {
    let _ = cmd
        .create_followup(
            &ctx.http,
            CreateInteractionResponseFollowup::new()
                .content(text.to_string())
                .components(embeds::cancel_button(item_id)),
        )
        .await;
}

async fn respond_queue_summary(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    guild_id: u64,
) {
    let state = app.player.get_state(guild_id).await;
    let (embed, total_pages) = embeds::queue_page_embed(&state, 0);
    let mut followup = CreateInteractionResponseFollowup::new().embed(embed);
    let mut components = embeds::playback_buttons(&state);
    components.extend(embeds::queue_page_buttons(0, total_pages));
    followup = followup.components(components);
    let _ = cmd.create_followup(&ctx.http, followup).await;
}

async fn settle_manual_skip(app: &Arc<App>, guild_id: u64) {
    // 스킵된 곡의 오디오를 즉시 중단.
    app.coordinator.cancel_current(guild_id).await;
    let state = app.player.get_state(guild_id).await;
    // 다음 곡이 이미 큐에 있어 바로 넘어간 경우엔 이전 곡 기준의 preview 를 버린다
    // (새 현재 곡이 자기 preview 를 따로 채우도록). 큐가 비었으면 preview 는 살려서 재사용.
    if state.current_item.is_some() {
        app.player.clear_preview(guild_id);
    }
    // 자연 종료 경로(TrackEndHandler)와 동일하게 ensure_autoplay → sync_guild.
    // ensure_autoplay 가 preview 소비/진행 중 추천 재사용/신규 추천을 모두 처리하므로
    // 예전의 선행 sync_guild·선행 consume(중복·경합 원인)는 제거했다.
    let app2 = app.clone();
    tokio::spawn(async move {
        side_effects::ensure_autoplay(app2.clone(), app2.coordinator.clone(), guild_id, true).await;
        app2.coordinator.sync_guild(&app2, guild_id).await;
        side_effects::prefetch_next(app2.clone(), app2.coordinator.clone(), guild_id).await;
    });
}

/// "1:23" / "01:02:03" / "83" → Duration.
fn parse_time(input: &str) -> Option<Duration> {
    let parts: Vec<&str> = input.trim().split(':').collect();
    let secs: u64 = match parts.len() {
        1 => parts[0].parse().ok()?,
        2 => parts[0].parse::<u64>().ok()? * 60 + parts[1].parse::<u64>().ok()?,
        3 => {
            parts[0].parse::<u64>().ok()? * 3600
                + parts[1].parse::<u64>().ok()? * 60
                + parts[2].parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

// ───────── 트랙 해석 (URL/검색) ─────────

enum ResolveOutcome {
    Single(TrackRef),
    Collection(Vec<TrackRef>),
}

async fn resolve_input(app: &Arc<App>, input: &str) -> Result<ResolveOutcome, String> {
    let ytdlp = app.ytdlp();
    if !resolver::can_resolve(input) {
        let results = ytdlp.search(input, 1).await;
        return results
            .into_iter()
            .next()
            .map(ResolveOutcome::Single)
            .ok_or_else(|| {
                "찾은 곡이 없어요. 다른 검색어를 쓰거나 주소를 직접 넣어 주세요."
                    .to_string()
            });
    }
    match resolver::resolve(input)? {
        Resolved::Track(t) => {
            let track = ytdlp
                .inspect_track(&t.source_url, t.provider)
                .await
                .unwrap_or(TrackRef {
                    provider: t.provider,
                    content_id: t.content_id,
                    source_url: t.source_url,
                    title: None,
                    artist: None,
                    duration: None,
                    variant_key: None,
                });
            Ok(ResolveOutcome::Single(track))
        }
        Resolved::Collection(c) => {
            let tracks = ytdlp.expand_collection(&c.source_url, c.provider).await;
            if tracks.is_empty() {
                Err(
                    "재생할 수 있는 곡이 없어요. 링크 접근 권한과 인증 설정을 확인해 주세요."
                        .into(),
                )
            } else {
                Ok(ResolveOutcome::Collection(tracks))
            }
        }
    }
}

// ───────── 슬래시 디스패치 ─────────

pub async fn handle_command(app: Arc<App>, ctx: Context, cmd: CommandInteraction) {
    let canonical = catalog::canonical_of(&cmd.data.name);
    let guild_id = match cmd.guild_id {
        Some(g) => g.get(),
        None => {
            let _ = cmd
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("서버 안에서만 쓸 수 있어요.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
    };

    // 승인 안 된 서버는 여기서 멈춘다 (§26). **명령을 처리하기 전에** 봐야 한다 —
    // 뒤에서 막으면 대기열이 바뀌거나 음성에 들어간 뒤에 거절하게 된다.
    let approval = app
        .remote
        .guild_approval(guild_id)
        .map(|row| row.status)
        .unwrap_or_else(|| app.remote.register_guild(guild_id, None));
    if !approval.is_usable() {
        let _ = cmd
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(approval.reason())
                        .ephemeral(true),
                ),
            )
            .await;
        return;
    }

    app.log.info(
        "Command",
        &format!(
            "Received slash command '{}' => '{canonical}' from user {} in guild {guild_id}.",
            cmd.data.name, cmd.user.id
        ),
    );

    // 알림 대상 채널 기억.
    {
        let mut map = app.announce_channels.lock().unwrap();
        map.insert(guild_id, cmd.channel_id.get());
    }

    // 3초 내 defer (다운로드 등 긴 작업 대비).
    // `/리모컨` 은 응답 전체가 "나만 보임"이어야 하므로 대기 표시부터 ephemeral 로 연다.
    let deferred = if canonical == "remote" {
        cmd.defer_ephemeral(&ctx.http).await
    } else {
        cmd.defer(&ctx.http).await
    };
    if deferred.is_err() {
        return;
    }

    let result = dispatch(&app, &ctx, &cmd, canonical, guild_id).await;
    // 디스코드에서 한 일도 사람 피드에 남긴다 (§32). 전에는 리모컨에서 한 것만 보여서
    // 채널에서 `/스킵` 을 눌러 곡이 넘어가도 웹에서는 **아무 이유 없이 바뀐 것처럼** 보였다.
    // 어디서 했는지도 같이 적는다 — 같은 동작이라도 출처를 알아야 상황이 읽힌다.
    log_discord_command(&app, guild_id, &cmd, canonical, result.is_ok());
    if let Err(msg) = result {
        respond_text(&ctx, &cmd, &format!("⚠️ {msg}"), true).await;
    }
    app.log.info(
        "Command",
        &format!("Completed '{canonical}' for interaction {}.", cmd.id),
    );
}

/// 디스코드 명령어를 사람 피드에 한 줄로 남긴다 (§32).
///
/// **조회만 하는 명령은 뺀다.** `/도움말` 같은 걸 남기면 피드가 남의 조회 기록으로 덮인다.
/// 남기는 것은 다른 사람 눈에 결과가 보이는 것들뿐이다.
fn log_discord_command(
    app: &Arc<App>,
    guild_id: u64,
    cmd: &CommandInteraction,
    canonical: &str,
    ok: bool,
) {
    let Some(verb) = discord_log_verb(canonical) else {
        return;
    };
    let who = cmd
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .unwrap_or_else(|| cmd.user.name.clone());
    // 분류와 문장은 `audit_kind_for` · `audit_text` 가 만든다 — 리모컨 기록과 같은 길을 탄다.
    let _ = app.remote.add_audit(
        guild_id,
        cmd.user.id.get(),
        &who,
        &format!("discord.{canonical}"),
        Some(verb),
        None,
        None,
        ok,
        (!ok).then_some("명령이 실패했어요"),
    );
}

/// 명령어 → 사람이 읽는 동사구. `None` 이면 피드에 안 남긴다.
///
/// **조회형은 뺀다.** `/도움말`·`/현재곡` 까지 남기면 피드가 남의 조회 기록으로 덮여
/// 정작 무슨 일이 있었는지가 묻힌다. 남기는 것은 남 눈에 결과가 보이는 것들뿐이다.
fn discord_log_verb(canonical: &str) -> Option<&'static str> {
    Some(match canonical {
        "play" => "곡을 신청했어요",
        "playnow" => "곡을 바로 틀었어요",
        "skip" => "곡을 넘겼어요",
        "stop" => "재생을 멈췄어요",
        "pause" => "일시정지했어요",
        "resume" => "다시 재생했어요",
        "replay" => "처음부터 다시 틀었어요",
        "previous" => "이전 곡으로 돌아갔어요",
        "volume" => "볼륨을 바꿨어요",
        "shuffle" => "대기열을 섞었어요",
        "repeat" => "반복 설정을 바꿨어요",
        "autoplay" => "자동 재생을 바꿨어요",
        "clear" => "대기열을 비웠어요",
        "remove" => "대기열에서 곡을 뺐어요",
        "join" => "봇을 음성 채널로 불렀어요",
        "leave" => "봇을 음성 채널에서 내보냈어요",
        "playlist" => "재생목록을 건드렸어요",
        _ => return None,
    })
}

async fn dispatch(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    canonical: &str,
    guild_id: u64,
) -> Result<(), String> {
    match canonical {
        "play" => {
            ensure_playback_allowed(app, ctx, cmd, true).await?;
            let input = opt_str(cmd, "input").ok_or("무엇을 재생할지 같이 알려 주세요.")?;
            handle_play(app, ctx, cmd, guild_id, &input, false).await
        }
        "playnow" => {
            ensure_playback_allowed(app, ctx, cmd, true).await?;
            let input = opt_str(cmd, "input").ok_or("무엇을 재생할지 같이 알려 주세요.")?;
            handle_play(app, ctx, cmd, guild_id, &input, true).await
        }
        "search" => {
            let query = opt_str(cmd, "query").ok_or("검색어를 같이 알려 주세요.")?;
            handle_search(app, ctx, cmd, &query, ProviderKind::YouTube).await
        }
        "scsearch" => {
            let query = opt_str(cmd, "query").ok_or("검색어를 같이 알려 주세요.")?;
            handle_search(app, ctx, cmd, &query, ProviderKind::SoundCloud).await
        }
        "queue" => {
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "status" => {
            let state = app.player.get_state(guild_id).await;
            let g = app.db.load_global_settings();
            // "🔊 음성" 은 저장값이 아니라 songbird 라이브 연결로 판정한다 (v3 §16 B1).
            let voice_connected = bot_live_voice_channel(app, guild_id).await.is_some();
            let embed = embeds::status_embed(
                &state,
                &g,
                &app.build_id,
                env!("CARGO_PKG_VERSION"),
                voice_connected,
            );
            let _ = cmd
                .create_followup(
                    &ctx.http,
                    CreateInteractionResponseFollowup::new().embed(embed),
                )
                .await;
            Ok(())
        }
        "nowplaying" => {
            let state = app.player.get_state(guild_id).await;
            match &state.current_item {
                None => respond_text(ctx, cmd, "> 지금 재생 중인 곡이 없어요.", false).await,
                Some(item) => {
                    let position = app.coordinator.current_position(guild_id).await;
                    let embed = embeds::now_playing_embed(&state, item, position);
                    let followup = CreateInteractionResponseFollowup::new()
                        .embed(embed)
                        .components(embeds::playback_buttons_np(&state));
                    let _ = cmd.create_followup(&ctx.http, followup).await;
                }
            }
            Ok(())
        }
        "shuffle" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            app.player.shuffle(guild_id).await;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "repeat" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let mode = match opt_str(cmd, "mode").unwrap_or_default().as_str() {
                "track" => RepeatMode::Track,
                "queue" => RepeatMode::Queue,
                _ => RepeatMode::Off,
            };
            app.player.set_repeat(guild_id, mode).await;
            let label = match mode {
                RepeatMode::Off => "반복 없음",
                RepeatMode::Track => "한곡 반복",
                RepeatMode::Queue => "전체 반복",
            };
            respond_text(ctx, cmd, &format!("🔁 반복: **{label}**"), false).await;
            Ok(())
        }
        "autoplay" => {
            // 자동 재생은 재생 제어가 아니라 **자동 재생 권한**을 본다(v3 §24.3).
            // 기본이 모든 멤버라, 리모컨만 보는 사람도 켜고 끌 수 있다.
            ensure_autoplay_allowed(
                app,
                ctx,
                guild_id,
                cmd.user.id.get(),
                is_admin(cmd),
                role_ids_of(cmd.member.as_deref()),
            )
            .await?;
            let enabled = opt_bool(cmd, "enabled").unwrap_or(true);
            app.player.set_autoplay(guild_id, enabled).await;
            if enabled {
                side_effects::ensure_autoplay(
                    app.clone(),
                    app.coordinator.clone(),
                    guild_id,
                    false,
                )
                .await;
                app.coordinator.sync_guild(app, guild_id).await;
            }
            respond_text(
                ctx,
                cmd,
                &format!("✨ 자동 재생: **{}**", if enabled { "켜짐" } else { "꺼짐" }),
                false,
            )
            .await;
            Ok(())
        }
        "pause" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            app.player.pause(guild_id).await;
            app.coordinator.apply_pause(guild_id, true).await;
            respond_text(ctx, cmd, "⏸ 잠깐 멈췄어요.", false).await;
            Ok(())
        }
        "resume" => {
            ensure_playback_allowed(app, ctx, cmd, true).await?;
            app.player.resume(guild_id).await;
            app.coordinator.apply_pause(guild_id, false).await;
            app.coordinator.sync_guild(app, guild_id).await;
            respond_text(ctx, cmd, "▶ 다시 재생해요.", false).await;
            Ok(())
        }
        "skip" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            app.player.skip(guild_id).await;
            settle_manual_skip(app, guild_id).await;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "stop" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            app.player.stop(guild_id).await;
            app.coordinator.cancel_current(guild_id).await;
            respond_text(
                ctx,
                cmd,
                "⏹ 정지하고 대기열을 비웠어요. (음성 채널엔 남아 있어요 — 내보내려면 `/나가기`)",
                false,
            )
            .await;
            Ok(())
        }
        "leave" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            app.player.stop(guild_id).await;
            app.player.disconnect_voice(guild_id).await;
            app.coordinator.leave_voice(app, guild_id).await;
            respond_text(ctx, cmd, "👋 음성 채널에서 나갔어요.", false).await;
            Ok(())
        }
        "clear" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            app.player.clear_queue(guild_id).await;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "previous" => {
            ensure_playback_allowed(app, ctx, cmd, true).await?;
            let bl = app.blacklist.clone();
            let gid = guild_id;
            app.player
                .prune_recent_tracks(guild_id, move |t| bl.is_blocked(gid, t))
                .await;
            app.player.previous(guild_id).await;
            app.coordinator.sync_guild(app, guild_id).await;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "replay" => {
            ensure_playback_allowed(app, ctx, cmd, true).await?;
            app.player
                .set_current_start_offset(guild_id, CsTimeSpan::zero())
                .await;
            app.coordinator.cancel_current(guild_id).await;
            app.coordinator.sync_guild(app, guild_id).await;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "seek" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let time = opt_str(cmd, "time").ok_or("이동할 시간을 같이 알려 주세요.")?;
            let target =
                parse_time(&time).ok_or("시간 형식을 못 읽었어요 (예: 1:23 또는 83).")?;
            let state = app.player.get_state(guild_id).await;
            let current = state
                .current_item
                .as_ref()
                .ok_or("지금 재생 중인 곡이 없어요.")?;
            let total = current
                .track
                .duration
                .ok_or("이 곡은 길이를 알 수 없어서(예: 라이브) 특정 시간으로 못 옮겨요.")?;
            if target.as_secs_f64() >= total.as_secs_f64() {
                return Err(format!("곡 길이({})보다 뒤예요.", total.display()));
            }
            app.player
                .set_current_start_offset(guild_id, CsTimeSpan(target))
                .await;
            app.coordinator.cancel_current(guild_id).await;
            app.coordinator.sync_guild(app, guild_id).await;
            respond_text(
                ctx,
                cmd,
                &format!("⏩ {} 로 옮겼어요.", CsTimeSpan(target).display()),
                false,
            )
            .await;
            Ok(())
        }
        "skipto" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let position = opt_int(cmd, "position").unwrap_or(1).max(1) as usize;
            app.player.skip_to(guild_id, position).await?;
            settle_manual_skip(app, guild_id).await;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "move" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let from = opt_int(cmd, "from").unwrap_or(1).max(1) as usize - 1;
            let to = opt_int(cmd, "to").unwrap_or(1).max(1) as usize - 1;
            app.player.move_item(guild_id, from, to).await?;
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "remove" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let index = opt_int(cmd, "index").unwrap_or(1).max(1) as usize - 1;
            let state = app.player.remove_upcoming(guild_id, index).await?;
            // 마지막 대기열 곡을 지워 큐가 비면, 다음 자동추천 후보를 미리 풀어둬
            // (현재 곡 종료 시 침묵이 생기지 않도록) + 다음 곡 프리페치.
            let app2 = app.clone();
            tokio::spawn(async move {
                if state.upcoming.is_empty() {
                    side_effects::resolve_preview(app2.clone(), guild_id).await;
                }
                side_effects::prefetch_next(app2.clone(), app2.coordinator.clone(), guild_id).await;
            });
            respond_queue_summary(app, ctx, cmd, guild_id).await;
            Ok(())
        }
        "volume" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let level = opt_int(cmd, "level").unwrap_or(100).clamp(0, 200) as i32;
            app.player.set_volume(guild_id, level).await;
            app.coordinator.apply_volume(guild_id, level).await;
            respond_text(ctx, cmd, &format!("🔊 서버 볼륨: **{level}%** (모두에게 적용돼요)"), false).await;
            Ok(())
        }
        "normalize" => {
            ensure_playback_allowed(app, ctx, cmd, false).await?;
            let enabled = opt_bool(cmd, "enabled").unwrap_or(true);
            let mut settings = app.db.load_global_settings();
            settings.normalize_enabled = enabled;
            app.db.save_global_settings(&settings);
            respond_text(
                ctx,
                cmd,
                &format!(
                    "🎚 볼륨 평준화: **{}** (다음 곡부터 적용돼요)",
                    if enabled { "켜짐" } else { "꺼짐" }
                ),
                false,
            )
            .await;
            Ok(())
        }
        "playlist" => handle_playlist(app, ctx, cmd, guild_id).await,
        "remote" => handle_remote(app, ctx, cmd, guild_id).await,
        other => Err(format!("이건 없는 명령이에요: {other}")),
    }
}

// ───────── play / playnow ─────────

async fn handle_play(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    guild_id: u64,
    input: &str,
    play_now: bool,
) -> Result<(), String> {
    let requester = cmd
        .member
        .as_ref()
        .map(|m| m.display_name().to_string())
        .unwrap_or_else(|| cmd.user.name.clone());
    let user_id = Some(cmd.user.id.get());

    match resolve_input(app, input).await? {
        ResolveOutcome::Single(track) => {
            if let Some(rule) = app.blacklist.try_get_blocker(guild_id, &track) {
                respond_text(
                    ctx,
                    cmd,
                    &format!(
                        "차단된 곡이에요: {}",
                        crate::blacklist::Blacklist::describe_rule(&rule)
                    ),
                    true,
                )
                .await;
                return Ok(());
            }
            let title = track.display_title().to_string();
            let item = QueueItem::new_user(track, requester, user_id);
            let item_id = item.id.clone();
            if play_now {
                let state = app.player.play_now(guild_id, item).await;
                app.coordinator.cancel_current(guild_id).await;
                app.coordinator.sync_guild(app, guild_id).await;
                let mode = if state.repeat_mode == RepeatMode::Off {
                    "대기열을 비웠어요"
                } else {
                    "반복 대기열 뒤로 보냈어요"
                };
                respond_with_cancel(
                    ctx,
                    cmd,
                    &format!("⏯ 바로 재생해요: '{title}' ({mode})"),
                    &item_id,
                )
                .await;
            } else {
                app.player.enqueue(guild_id, item, false).await;
                app.coordinator.sync_guild(app, guild_id).await;
                respond_with_cancel(
                    ctx,
                    cmd,
                    &format!("'{title}' 을(를) 대기열에 담았어요."),
                    &item_id,
                )
                .await;
            }
            Ok(())
        }
        ResolveOutcome::Collection(tracks) => {
            let mut allowed = Vec::new();
            let mut blocked = 0usize;
            for t in tracks {
                if app.blacklist.is_blocked(guild_id, &t) {
                    blocked += 1;
                } else {
                    allowed.push(t);
                }
            }
            if allowed.is_empty() {
                respond_text(
                    ctx,
                    cmd,
                    &format!("전부 차단된 곡({blocked}개)이라 하나도 못 담았어요."),
                    true,
                )
                .await;
                return Ok(());
            }
            if play_now {
                // 컬렉션 바로재생: 첫 곡만.
                let first = allowed.remove(0);
                let title = first.display_title().to_string();
                let item = QueueItem::new_user(first, requester, user_id);
                let state = app.player.play_now(guild_id, item).await;
                app.coordinator.cancel_current(guild_id).await;
                app.coordinator.sync_guild(app, guild_id).await;
                let mode = if state.repeat_mode == RepeatMode::Off {
                    "대기열을 비웠어요"
                } else {
                    "반복 대기열 뒤로 보냈어요"
                };
                respond_text(ctx, cmd, &format!("⏯ 바로 재생해요: '{title}' ({mode})"), false).await;
                return Ok(());
            }
            let count = allowed.len();
            let first_title = allowed[0].display_title().to_string();
            for t in allowed {
                let item = QueueItem::new_user(t, requester.clone(), user_id);
                app.player.enqueue(guild_id, item, false).await;
            }
            app.coordinator.sync_guild(app, guild_id).await;
            let suffix = if blocked > 0 {
                format!(" (차단된 {blocked}곡은 빼고요)")
            } else {
                String::new()
            };
            respond_text(
                ctx,
                cmd,
                &format!("'{first_title}' 을(를) 포함해 {count}곡을 담았어요.{suffix}"),
                false,
            )
            .await;
            Ok(())
        }
    }
}

// ───────── search ─────────

const SEARCH_RESULT_LIMIT: usize = 10;
const SEARCH_SESSION_TTL: Duration = Duration::from_secs(15 * 60);

async fn handle_search(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    query: &str,
    provider: ProviderKind,
) -> Result<(), String> {
    let guild_id = cmd.guild_id.map(|g| g.get()).unwrap_or(0);
    let ytdlp = app.ytdlp();
    let results = ytdlp
        .search_provider(query, SEARCH_RESULT_LIMIT, provider)
        .await;

    // 차단 곡은 후보에서 미리 거른다 (선택 후 거절보다 친절).
    let candidates: Vec<TrackRef> = results
        .into_iter()
        .filter(|t| !app.blacklist.is_blocked(guild_id, t))
        .collect();

    if candidates.is_empty() {
        return Err(
            "찾은 곡이 없어요. 다른 검색어를 쓰거나 주소를 직접 넣어 주세요.".to_string(),
        );
    }

    let token = uuid_like();
    {
        let mut sessions = app.search_sessions.lock().unwrap();
        sessions.retain(|_, s| s.created.elapsed() < SEARCH_SESSION_TTL);
        sessions.insert(
            token.clone(),
            SearchSession {
                candidates: candidates.clone(),
                created: Instant::now(),
            },
        );
    }

    let embed = embeds::search_results_embed(query, provider, &candidates);
    let components = embeds::search_results_components(&token, &candidates);
    let _ = cmd
        .create_followup(
            &ctx.http,
            CreateInteractionResponseFollowup::new()
                .embed(embed)
                .components(components),
        )
        .await;
    Ok(())
}

// ───────── playlist ─────────

async fn handle_playlist(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    guild_id: u64,
) -> Result<(), String> {
    let sub = cmd.data.options.first().ok_or("무엇을 할지 하위 명령을 골라 주세요.")?;
    let sub_name = sub.name.clone();
    let sub_opts = match &sub.value {
        serenity::all::CommandDataOptionValue::SubCommand(opts) => opts.clone(),
        _ => Vec::new(),
    };
    let get_str = |name: &str| -> Option<String> {
        sub_opts
            .iter()
            .find(|o| o.name == name)
            .and_then(|o| o.value.as_str().map(|s| s.to_string()))
    };
    let get_int = |name: &str| -> Option<i64> {
        sub_opts
            .iter()
            .find(|o| o.name == name)
            .and_then(|o| o.value.as_i64())
    };

    let find_by_name = |name: &str| -> Option<Playlist> {
        let mut all = app.db.list_playlists(PlaylistScope::Guild, Some(guild_id));
        all.extend(app.db.list_playlists(PlaylistScope::Global, None));
        all.into_iter().find(|p| p.name.eq_ignore_ascii_case(name))
    };

    match sub_name.as_str() {
        "list" => {
            let guild_lists = app.db.list_playlists(PlaylistScope::Guild, Some(guild_id));
            let global_lists = app.db.list_playlists(PlaylistScope::Global, None);
            let mut lines = Vec::new();
            for p in &guild_lists {
                lines.push(format!("• [서버] **{}** — {}곡", p.name, p.entries.len()));
            }
            for p in &global_lists {
                lines.push(format!("• [전역] **{}** — {}곡", p.name, p.entries.len()));
            }
            let text = if lines.is_empty() {
                "저장해 둔 재생목록이 없어요.".to_string()
            } else {
                lines.join("\n")
            };
            respond_text(ctx, cmd, &text, false).await;
            Ok(())
        }
        "create" => {
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            if find_by_name(&name).is_some() {
                return Err(format!("'{name}' 이름의 재생목록이 이미 있어요."));
            }
            let scope = if get_str("scope").as_deref() == Some("global") {
                PlaylistScope::Global
            } else {
                PlaylistScope::Guild
            };
            let gid = if scope == PlaylistScope::Guild {
                Some(guild_id)
            } else {
                None
            };
            app.db.create_playlist(scope, gid, cmd.user.id.get(), &name);
            respond_text(
                ctx,
                cmd,
                &format!("재생목록 '{name}' 을(를) 만들었어요."),
                false,
            )
            .await;
            Ok(())
        }
        "delete" => {
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            let pl =
                find_by_name(&name).ok_or(format!("'{name}' 재생목록을 못 찾았어요."))?;
            app.db.delete_playlist(pl.id);
            respond_text(
                ctx,
                cmd,
                &format!("재생목록 '{name}' 을(를) 지웠어요."),
                false,
            )
            .await;
            Ok(())
        }
        "rename" => {
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            let new_name = get_str("newname").ok_or("새 이름을 같이 알려 주세요.")?;
            let pl =
                find_by_name(&name).ok_or(format!("'{name}' 재생목록을 못 찾았어요."))?;
            app.db.rename_playlist(pl.id, &new_name);
            respond_text(
                ctx,
                cmd,
                &format!("'{name}' 을(를) '{new_name}' 으로 바꿨어요."),
                false,
            )
            .await;
            Ok(())
        }
        "add" => {
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            let input = get_str("input").ok_or("어떤 곡인지 같이 알려 주세요.")?;
            let pl =
                find_by_name(&name).ok_or(format!("'{name}' 재생목록을 못 찾았어요."))?;
            match resolve_input(app, &input).await? {
                ResolveOutcome::Single(track) => {
                    let title = track.display_title().to_string();
                    app.db.add_playlist_entry(
                        pl.id,
                        &PlaylistEntry {
                            track: Some(track),
                            collection: None,
                            start_offset: Some(CsTimeSpan::zero()),
                            extra: Default::default(),
                        },
                    );
                    respond_text(
                        ctx,
                        cmd,
                        &format!("'{title}' 을(를) '{name}' 에 담았어요."),
                        false,
                    )
                    .await;
                }
                ResolveOutcome::Collection(tracks) => {
                    let count = tracks.len();
                    for track in tracks {
                        app.db.add_playlist_entry(
                            pl.id,
                            &PlaylistEntry {
                                track: Some(track),
                                collection: None,
                                start_offset: Some(CsTimeSpan::zero()),
                                extra: Default::default(),
                            },
                        );
                    }
                    respond_text(
                        ctx,
                        cmd,
                        &format!("{count}곡을 '{name}' 에 담았어요."),
                        false,
                    )
                    .await;
                }
            }
            Ok(())
        }
        "remove" => {
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            let index = get_int("index").unwrap_or(1).max(1) as usize - 1;
            let pl =
                find_by_name(&name).ok_or(format!("'{name}' 재생목록을 못 찾았어요."))?;
            if app.db.remove_playlist_entry(pl.id, index) {
                respond_text(
                    ctx,
                    cmd,
                    &format!("'{name}' 의 {}번째 곡을 뺐어요.", index + 1),
                    false,
                )
                .await;
                Ok(())
            } else {
                Err("그 순번에는 곡이 없어요.".into())
            }
        }
        "show" => {
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            let pl =
                find_by_name(&name).ok_or(format!("'{name}' 재생목록을 못 찾았어요."))?;
            let lines: Vec<String> = pl
                .entries
                .iter()
                .enumerate()
                .take(25)
                .map(|(i, e)| {
                    let title = e
                        .track
                        .as_ref()
                        .map(|t| t.display_title().to_string())
                        .unwrap_or_else(|| "(컬렉션)".into());
                    format!("`{:>2}.` {title}", i + 1)
                })
                .collect();
            let more = pl.entries.len().saturating_sub(25);
            let mut text = format!(
                "**{}** — {}곡\n{}",
                pl.name,
                pl.entries.len(),
                lines.join("\n")
            );
            if more > 0 {
                text.push_str(&format!("\n… 그 외 {more}곡"));
            }
            respond_text(ctx, cmd, &text, false).await;
            Ok(())
        }
        "load" => {
            ensure_playback_allowed(app, ctx, cmd, true).await?;
            let name = get_str("name").ok_or("재생목록 이름을 같이 알려 주세요.")?;
            let pl =
                find_by_name(&name).ok_or(format!("'{name}' 재생목록을 못 찾았어요."))?;
            let requester = cmd
                .member
                .as_ref()
                .map(|m| m.display_name().to_string())
                .unwrap_or_else(|| cmd.user.name.clone());
            let mut added = 0usize;
            for entry in &pl.entries {
                if let Some(track) = &entry.track {
                    if app.blacklist.is_blocked(guild_id, track) {
                        continue;
                    }
                    let item = QueueItem::new_user(
                        track.clone(),
                        requester.clone(),
                        Some(cmd.user.id.get()),
                    );
                    app.player.enqueue(guild_id, item, false).await;
                    added += 1;
                }
            }
            app.coordinator.sync_guild(app, guild_id).await;
            respond_text(
                ctx,
                cmd,
                &format!("'{}' 에서 {added}곡을 대기열에 담았어요.", pl.name),
                false,
            )
            .await;
            Ok(())
        }
        other => Err(format!(
            "재생목록 하위 명령 '{other}' 은(는) 없어요."
        )),
    }
}

// ───────── remote (웹 리모컨 안내) ─────────

/// 서버 관리자 판정 — 인터랙션이 실어 준 권한 비트(관리자/서버 관리) 또는 길드 소유자.
/// 길드 설정의 "지정 역할"까지는 보지 않는다. 관리 콘솔 링크를 하나 더 붙일지만 정하는 용도다.
fn is_guild_manager(ctx: &Context, cmd: &CommandInteraction, guild_id: u64) -> bool {
    is_admin(cmd)
        || ctx
            .cache
            .guild(GuildId::new(guild_id))
            .map(|g| g.owner_id == cmd.user.id)
            .unwrap_or(false)
}

/// `/리모컨` — 웹 리모컨 주소를 나만 보이는 응답으로 안내한다.
/// 링크에는 어떤 접근 토큰도 넣지 않는다 — 접속자는 항상 Discord 로그인을 거친다.
async fn handle_remote(
    app: &Arc<App>,
    ctx: &Context,
    cmd: &CommandInteraction,
    guild_id: u64,
) -> Result<(), String> {
    // 공개 주소는 web::serve() 가 기동 시 주입한다. 없으면 링크를 만들 수 없다.
    let Some(base) = app.public_base_url.get() else {
        respond_text(
            ctx,
            cmd,
            "웹 리모컨 주소가 아직 설정되지 않았어요. 서버 관리자에게 말해 주세요.",
            true,
        )
        .await;
        return Ok(());
    };
    let base = base.trim_end_matches('/');

    let guild_name = ctx
        .cache
        .guild(GuildId::new(guild_id))
        .map(|g| g.name.to_string())
        .unwrap_or_else(|| "이 서버".to_string());

    let state = app.player.get_state(guild_id).await;
    let now = match &state.current_item {
        Some(item) => {
            let title: String = item.track.display_title().chars().take(80).collect();
            format!("{title}\n신청: {}", describe_requester_short(item))
        }
        None => "지금은 없어요".to_string(),
    };

    // v3 §4: 봇이 음성 채널에 없으면 "듣는 중 N명"이 아니라 그 사실을 그대로 말한다.
    let listening = match listening_summary(app, ctx, guild_id).await {
        Some((channel, count)) => format!("🎧 {count}명 · #{channel}"),
        None => "봇이 음성 채널에 없어요".to_string(),
    };

    let portal = format!("{base}/music/guilds/{guild_id}");
    let mut embed = CreateEmbed::new()
        .colour(0x5865F2)
        .title("🎛 마참뮤직 리모컨")
        .description(format!("**{guild_name}**"))
        .field("지금 재생 중", now, false)
        .field("듣는 중", listening, true)
        .field("대기열", format!("{}곡", state.upcoming.len()), true)
        .field("리모컨", format!("[열기 →]({portal})"), true);
    if is_guild_manager(ctx, cmd, guild_id) {
        embed = embed.field("서버 관리 콘솔", format!("[열기 →]({portal}/admin)"), false);
    }
    embed = embed.footer(CreateEmbedFooter::new(
        "링크에 접근 토큰은 없어요. 열면 Discord 로그인을 거쳐요.",
    ));

    let _ = cmd
        .create_followup(
            &ctx.http,
            CreateInteractionResponseFollowup::new()
                .embed(embed)
                .ephemeral(true),
        )
        .await;
    Ok(())
}

/// 봇이 **실제로** 들어가 있는 음성 채널과, 거기서 같이 듣고 있는 사람 수 (봇 제외).
///
/// v3 §4: 봇이 음성 채널에 없으면 "듣는 중"이라는 말 자체가 성립하지 않으므로 `None` 이다.
/// 채널 판단은 §16 B1 대로 songbird 라이브 연결만 믿는다 — 저장값은 봇이 튕겨 나가도 남는다.
async fn listening_summary(
    app: &Arc<App>,
    ctx: &Context,
    guild_id: u64,
) -> Option<(String, usize)> {
    let channel_id = bot_live_voice_channel(app, guild_id).await?;
    let guild = ctx.cache.guild(GuildId::new(guild_id))?;
    let listeners = guild
        .voice_states
        .values()
        .filter(|voice| voice.channel_id.map(|c| c.get()) == Some(channel_id))
        .filter(|voice| {
            // 봇 자신은 청취자가 아니다. 멤버 정보가 없으면 사람으로 본다(과소 집계보다 낫다).
            voice
                .member
                .as_ref()
                .map(|member| !member.user.bot)
                .unwrap_or(true)
        })
        .count();
    let name = guild
        .channels
        .get(&serenity::all::ChannelId::new(channel_id))
        .map(|channel| channel.name.clone())
        .unwrap_or_else(|| "음성 채널".to_string());
    Some((name, listeners))
}

/// 신청자 표기 — 자동추천 곡은 사람 이름 대신 "자동추천".
fn describe_requester_short(item: &QueueItem) -> String {
    if item.request_kind == PlaybackRequestKind::Autoplay {
        "자동추천".to_string()
    } else {
        item.requested_by_display.clone()
    }
}

// ───────── 버튼 핸들러 ─────────

/// 컴포넌트 거부 응답 (ephemeral 안내) — 상호작용당 1회만 호출할 것.
async fn comp_reject(ctx: &Context, comp: &ComponentInteraction, text: &str) {
    let _ = comp
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(text.to_string())
                    .ephemeral(true),
            ),
        )
        .await;
}

/// 컴포넌트 조작 권한: 봇과 같은 음성 채널에 있거나 관리자.
async fn component_control_allowed(
    app: &Arc<App>,
    ctx: &Context,
    comp: &ComponentInteraction,
    guild_id: u64,
) -> bool {
    let requester_vc = requester_voice_channel(ctx, guild_id, comp.user.id.get());
    let bot_live = bot_live_voice_channel(app, guild_id).await;
    let admin = comp
        .member
        .as_ref()
        .and_then(|m| m.permissions)
        .map(|p| p.contains(Permissions::ADMINISTRATOR) || p.contains(Permissions::MANAGE_GUILD))
        .unwrap_or(false);
    // 저장값으로 폴백하지 않는다 — 봇이 이미 나간 방 사람이 버튼을 계속 쥐게 된다 (v3 §16 B1).
    admin || in_bot_voice_channel(bot_live, requester_vc)
}

/// 검색 후보 선택(mbsel) — 고른 곡을 대기열에 추가하고 취소 버튼을 단다.
async fn handle_search_select(
    app: &Arc<App>,
    ctx: &Context,
    comp: &ComponentInteraction,
    guild_id: u64,
    token: &str,
) {
    let index = match &comp.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => {
            values.first().and_then(|v| v.parse::<usize>().ok())
        }
        _ => None,
    };
    let chosen = {
        let sessions = app.search_sessions.lock().unwrap();
        index.and_then(|i| {
            sessions
                .get(token)
                .and_then(|s| s.candidates.get(i).cloned())
        })
    };
    let Some(track) = chosen else {
        comp_reject(ctx, comp, "검색 결과가 만료됐어요. 다시 찾아 주세요.").await;
        return;
    };

    let Some(rvc) = requester_voice_channel(ctx, guild_id, comp.user.id.get()) else {
        comp_reject(ctx, comp, "먼저 음성 채널에 들어간 뒤에 곡을 골라 주세요.").await;
        return;
    };

    if let Some(rule) = app.blacklist.try_get_blocker(guild_id, &track) {
        comp_reject(
            ctx,
            comp,
            &format!(
                "차단된 곡이에요: {}",
                crate::blacklist::Blacklist::describe_rule(&rule)
            ),
        )
        .await;
        return;
    }

    // 검사를 통과했으니 세션 소비.
    app.search_sessions.lock().unwrap().remove(token);

    // 음성 합류 (재생 시작 명령과 동일 규칙): 봇이 이미 방에 있으면 옮기지 않고,
    // 아무 방에도 없을 때만 명령자 방으로 합류. (songbird 라이브 상태 = 권위 소스.)
    if bot_live_voice_channel(app, guild_id).await.is_none() {
        app.player.connect_voice(guild_id, rvc).await;
    }

    let requester = comp
        .member
        .as_ref()
        .map(|m| m.display_name().to_string())
        .unwrap_or_else(|| comp.user.name.clone());
    let title = track.display_title().to_string();
    let item = QueueItem::new_user(track, requester, Some(comp.user.id.get()));
    let item_id = item.id.clone();
    app.player.enqueue(guild_id, item, false).await;

    let _ = comp
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .content(format!("✅ '{title}' 을(를) 대기열에 담았어요."))
                    .embeds(Vec::new())
                    .components(embeds::cancel_button(&item_id)),
            ),
        )
        .await;

    // 실제 재생/합류는 3초 제한을 피해 백그라운드에서 (다운로드가 길 수 있음).
    let app2 = app.clone();
    tokio::spawn(async move {
        app2.coordinator.sync_guild(&app2, guild_id).await;
    });
}

pub async fn handle_button(app: Arc<App>, ctx: Context, comp: ComponentInteraction) {
    let id = comp.data.custom_id.clone();
    let Some(guild_id) = comp.guild_id.map(|g| g.get()) else {
        return;
    };

    // 검색 후보 선택.
    if let Some(token) = id.strip_prefix("mbsel:") {
        handle_search_select(&app, &ctx, &comp, guild_id, token).await;
        return;
    }

    // 검색 취소 — 후보 세션 폐기 후 메시지 정리.
    if let Some(token) = id.strip_prefix("mbsx:") {
        app.search_sessions.lock().unwrap().remove(token);
        let _ = comp
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("검색을 닫았어요.")
                        .embeds(Vec::new())
                        .components(Vec::new()),
                ),
            )
            .await;
        return;
    }

    // ✖ 곡 취소 — /재생·검색선택 응답의 그 곡만 큐에서 취소(또는 재생 중이면 스킵).
    if let Some(raw_id) = id.strip_prefix("mbcx:") {
        let item_id = raw_id.to_string();
        if !component_control_allowed(&app, &ctx, &comp, guild_id).await {
            comp_reject(
                &ctx,
                &comp,
                "봇과 같은 음성 채널에 있어야 취소할 수 있어요.",
            )
            .await;
            return;
        }
        let outcome = app.player.cancel_by_id(guild_id, &item_id).await;
        let (text, skipped) = match outcome {
            CancelOutcome::RemovedUpcoming(t) => {
                (format!("✖ '{t}' 을(를) 대기열에서 뺐어요."), false)
            }
            CancelOutcome::SkippedCurrent(t) => {
                (format!("✖ 재생 중이던 '{t}' 을(를) 껐어요."), true)
            }
            CancelOutcome::NotFound => (
                "이미 재생됐거나 취소할 수 없는 곡이에요.".to_string(),
                false,
            ),
        };
        let _ = comp
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content(text)
                        .embeds(Vec::new())
                        .components(Vec::new()),
                ),
            )
            .await;
        let app2 = app.clone();
        tokio::spawn(async move {
            if skipped {
                settle_manual_skip(&app2, guild_id).await;
            } else {
                app2.coordinator.sync_guild(&app2, guild_id).await;
            }
        });
        return;
    }

    // 큐 페이지 버튼 — mbq:prev:N / mb:queue:prev:N / mb:queue:noop 형식 모두 인식.
    // 엔진 전환 직후 C# 이 올려둔 옛 메시지의 버튼이 계속 동작하게 한다.
    let queue_nav = id
        .strip_prefix("mbq:")
        .or_else(|| id.strip_prefix("mb:queue:"));
    if let Some(rest) = queue_nav {
        if rest == "noop" {
            let _ = comp
                .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
                .await;
            return;
        }
        let mut parts = rest.split(':');
        let dir = parts.next().unwrap_or("");
        let page: usize = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let new_page = if dir == "prev" {
            page.saturating_sub(1)
        } else {
            page + 1
        };
        let state = app.player.get_state(guild_id).await;
        let (embed, total_pages) = embeds::queue_page_embed(&state, new_page);
        let mut components = embeds::playback_buttons(&state);
        components.extend(embeds::queue_page_buttons(
            new_page.min(total_pages.saturating_sub(1)),
            total_pages,
        ));
        let _ = comp
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(components),
                ),
            )
            .await;
        return;
    }

    // "mbnp:" = Now Playing 카드 버튼(누르면 NP 카드로 갱신), "mb:" = 큐 카드 버튼.
    let np = id.starts_with("mbnp:");
    if !np && !id.starts_with("mb:") {
        return;
    }
    let action = id
        .trim_start_matches("mbnp:")
        .trim_start_matches("mb:")
        .to_string();
    app.log.info(
        "Command",
        &format!(
            "Playback button '{action}' from user {} in guild {guild_id}.",
            comp.user.id
        ),
    );

    // 같은 채널 검사 (관리자 제외).
    let requester_vc = ctx
        .cache
        .guild(GuildId::new(guild_id))
        .and_then(|g| {
            g.voice_states
                .get(&comp.user.id)
                .and_then(|vs| vs.channel_id)
        })
        .map(|c| c.get());
    let bot_live = bot_live_voice_channel(&app, guild_id).await;
    let admin = comp
        .member
        .as_ref()
        .and_then(|m| m.permissions)
        .map(|p| p.contains(Permissions::ADMINISTRATOR) || p.contains(Permissions::MANAGE_GUILD))
        .unwrap_or(false);
    let state_before = app.player.get_state(guild_id).await;
    // 저장값으로 폴백하지 않는다 (v3 §16 B1) — 봇이 나간 뒤에도 그 방 사람만 조작할 수 있게 된다.
    // 🎲 자동 재생만 규칙이 다르다 — 재생 제어가 아니라 자동 재생 권한을 본다(v3 §24.3).
    let denial = if action == "autoplay" {
        ensure_autoplay_allowed(
            &app,
            &ctx,
            guild_id,
            comp.user.id.get(),
            admin,
            role_ids_of(comp.member.as_ref()),
        )
        .await
        .err()
    } else if admin || in_bot_voice_channel(bot_live, requester_vc) {
        None
    } else {
        Some("봇과 같은 음성 채널에 있어야 조작할 수 있어요.".to_string())
    };
    if let Some(reason) = denial {
        let _ = comp
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(reason)
                        .ephemeral(true),
                ),
            )
            .await;
        return;
    }

    // 무거운 코디네이터 작업(추천/세션 재생성)은 응답 후로 미룬다 — 컴포넌트 상호작용은
    // defer 가 없어서 추천/다운로드를 응답 전에 돌리면 3초 마감을 넘겨 "상호작용 실패"가 뜬다.
    let mut need_sync = false;
    let mut need_autoplay = false;

    match action.as_str() {
        "playpause" => {
            let paused = !state_before.is_paused;
            if paused {
                app.player.pause(guild_id).await;
                app.coordinator.apply_pause(guild_id, true).await;
            } else {
                app.player.resume(guild_id).await;
                app.coordinator.apply_pause(guild_id, false).await;
                // 재시작 등으로 세션이 사라졌으면 sync_guild 가 재생을 다시 띄운다(/계속 과 동일).
                need_sync = true;
            }
        }
        "skip" => {
            app.player.skip(guild_id).await;
            settle_manual_skip(&app, guild_id).await;
        }
        "stop" => {
            app.player.stop(guild_id).await;
            app.coordinator.cancel_current(guild_id).await;
        }
        "shuffle" => {
            app.player
                .set_shuffle(guild_id, !state_before.shuffle_enabled)
                .await;
        }
        "repeat" => {
            let next = match state_before.repeat_mode {
                RepeatMode::Off => RepeatMode::Track,
                RepeatMode::Track => RepeatMode::Queue,
                RepeatMode::Queue => RepeatMode::Off,
            };
            app.player.set_repeat(guild_id, next).await;
        }
        // "vol-"/"vol+" 는 C# 엔진의 버튼 ID — 전환 후 옛 메시지 호환.
        "voldown" | "volup" | "vol-" | "vol+" => {
            let delta = if action == "volup" || action == "vol+" {
                10
            } else {
                -10
            };
            let new_vol = (state_before.effective_volume + delta).clamp(0, 200);
            app.player.set_volume(guild_id, new_vol).await;
            app.coordinator.apply_volume(guild_id, new_vol).await;
        }
        // 옛 버튼 세트의 이전곡 — 현재 버튼엔 없지만 호환 처리.
        "previous" => {
            app.player.previous(guild_id).await;
            app.coordinator.cancel_current(guild_id).await;
            need_sync = true;
        }
        "autoplay" => {
            let enabled = !state_before.autoplay_enabled;
            app.player.set_autoplay(guild_id, enabled).await;
            if enabled {
                need_autoplay = true;
                need_sync = true;
            }
        }
        "replay" => {
            app.player
                .set_current_start_offset(guild_id, CsTimeSpan::zero())
                .await;
            app.coordinator.cancel_current(guild_id).await;
            need_sync = true;
        }
        "queue" => {}
        _ => {}
    }

    // 갱신된 상태로 메시지 업데이트 — NP 카드 버튼(mbnp:)이면 NP 카드로 새로고침,
    // 아니면 큐 카드로. (큐 버튼은 의도적으로 큐 카드로 전환)
    let state = app.player.get_state(guild_id).await;
    let (embed, components) = if np && action != "queue" {
        match &state.current_item {
            Some(item) => {
                let position = app.coordinator.current_position(guild_id).await;
                let e = embeds::now_playing_embed(&state, item, position);
                (e, embeds::playback_buttons_np(&state))
            }
            None => {
                let (e, total_pages) = embeds::queue_page_embed(&state, 0);
                let mut c = embeds::playback_buttons(&state);
                c.extend(embeds::queue_page_buttons(0, total_pages));
                (e, c)
            }
        }
    } else {
        let (e, total_pages) = embeds::queue_page_embed(&state, 0);
        let mut c = embeds::playback_buttons(&state);
        c.extend(embeds::queue_page_buttons(0, total_pages));
        (e, c)
    };
    let _ = comp
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(components),
            ),
        )
        .await;

    // 응답을 보낸 뒤에 무거운 작업(추천 + 세션 재생성)을 spawn.
    if need_sync || need_autoplay {
        let app2 = app.clone();
        tokio::spawn(async move {
            if need_autoplay {
                side_effects::ensure_autoplay(
                    app2.clone(),
                    app2.coordinator.clone(),
                    guild_id,
                    false,
                )
                .await;
            }
            app2.coordinator.sync_guild(&app2, guild_id).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::in_bot_voice_channel;

    /// "봇과 같은 방인가" 는 라이브 연결로만 판단한다 (v3 §16 B1).
    ///
    /// 예전에는 세 군데(`ensure_playback_allowed` · `component_control_allowed` · 컨트롤 버튼)가
    /// `bot_live.or(state.voice_channel_id)` 로 저장값에 폴백했다. 봇이 튕겨 나가면 그 값이
    /// 그대로 남아서, 봇이 없는 방 사람만 조작이 되고 다른 사람은 "봇과 같은 음성 채널에
    /// 있어야 해요" 로 막혔다. 판정을 한 함수로 모았으니 여기가 유일한 관문이다.
    #[test]
    fn same_channel_is_decided_by_the_live_connection_only() {
        assert!(in_bot_voice_channel(Some(10), Some(10)), "같은 방");
        assert!(!in_bot_voice_channel(Some(10), Some(20)), "다른 방");
        assert!(
            !in_bot_voice_channel(None, Some(10)),
            "봇이 아무 방에도 없으면 누구도 '같은 방'이 아니다 — 저장값이 남아 있어도"
        );
        assert!(
            !in_bot_voice_channel(None, None),
            "둘 다 없을 때 None == None 으로 통과시키면 음성 밖 사람이 전부 통과한다"
        );
        assert!(!in_bot_voice_channel(Some(10), None), "듣는 사람이 음성에 없다");
    }
}
