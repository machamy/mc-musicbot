use crate::models::TrackRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueVoteKind {
    Like,
    SuperLike,
}

impl QueueVoteKind {
    pub fn points(self) -> i32 {
        match self {
            Self::Like => 1,
            Self::SuperLike => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Like => "Like",
            Self::SuperLike => "SuperLike",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Like" => Some(Self::Like),
            "SuperLike" => Some(Self::SuperLike),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UserTrackKind {
    Liked,
    Saved,
}

impl UserTrackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Liked => "Liked",
            Self::Saved => "Saved",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Liked" => Some(Self::Liked),
            "Saved" => Some(Self::Saved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueScore {
    pub item_id: String,
    pub guild_id: u64,
    pub requester_user_id: Option<u64>,
    pub wait_score: i32,
    pub like_count: i32,
    pub super_like_count: i32,
    pub manual_priority: Option<i32>,
    pub original_order: i64,
}

impl QueueScore {
    pub fn total_score(&self) -> i32 {
        self.wait_score + self.like_count + self.super_like_count * 2
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTrack {
    pub guild_id: u64,
    pub user_id: u64,
    pub kind: UserTrackKind,
    pub track: TrackRef,
    pub created_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentTrack {
    pub id: i64,
    pub guild_id: u64,
    pub track: TrackRef,
    pub requested_by_user_id: Option<u64>,
    pub requested_by_display: String,
    pub played_utc: String,
    pub end_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub content: String,
    pub created_utc: String,
    pub deleted_utc: Option<String>,
    pub reactions: Vec<ChatReactionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReactionSummary {
    pub emoji: String,
    pub count: i32,
    pub reacted_by_me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReport {
    pub id: i64,
    pub guild_id: u64,
    pub message_id: i64,
    pub reporter_user_id: u64,
    pub reporter_display_name: String,
    pub reason: String,
    pub message_content: String,
    pub message_author: String,
    pub created_utc: String,
    pub resolved_utc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    pub display_name: String,
    pub action: String,
    pub target: Option<String>,
    pub before_value: Option<String>,
    pub after_value: Option<String>,
    pub success: bool,
    pub failure_reason: Option<String>,
    pub created_utc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionRule {
    GuildMember,
    SameVoiceChannel,
    ConfiguredRole,
    Administrator,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteGuildSettings {
    pub guild_id: u64,
    pub min_volume: i32,
    pub max_volume: i32,
    pub default_volume: i32,
    pub search_rule: PermissionRule,
    pub vote_rule: PermissionRule,
    pub chat_rule: PermissionRule,
    pub playback_rule: PermissionRule,
    pub seek_rule: PermissionRule,
    pub volume_rule: PermissionRule,
    pub queue_edit_rule: PermissionRule,
    pub configured_role_ids: Vec<u64>,
    pub max_queue_per_user: i32,
    pub max_queue_per_guild: i32,
    pub max_track_seconds: i32,
    pub chat_enabled: bool,
    pub audit_retention_days: i32,
}

impl Default for RemoteGuildSettings {
    fn default() -> Self {
        Self {
            guild_id: 0,
            min_volume: 0,
            max_volume: 200,
            default_volume: 100,
            search_rule: PermissionRule::GuildMember,
            vote_rule: PermissionRule::GuildMember,
            chat_rule: PermissionRule::GuildMember,
            playback_rule: PermissionRule::SameVoiceChannel,
            seek_rule: PermissionRule::SameVoiceChannel,
            volume_rule: PermissionRule::SameVoiceChannel,
            queue_edit_rule: PermissionRule::SameVoiceChannel,
            configured_role_ids: Vec::new(),
            max_queue_per_user: 5,
            max_queue_per_guild: 100,
            max_track_seconds: 14_400,
            chat_enabled: true,
            audit_retention_days: 14,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsDocument {
    pub cache_key: String,
    pub plain_text: Option<String>,
    pub synced_lines: Vec<LyricsLine>,
    pub source: String,
    pub fetched_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    pub start_ms: u64,
    pub text: String,
}
