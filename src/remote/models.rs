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

/// 대기열 정렬 방식. 서버 관리자만 바꿀 수 있고 길드 설정 JSON에 그대로 저장된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueSortMode {
    /// 점수제 — 대기 점수 + 좋아요로 순서가 바뀐다 (기존 동작).
    #[default]
    Score,
    /// 시간제 — 신청 순서 그대로. 좋아요는 표시만 된다.
    Fifo,
    /// 공평제 — 사람별로 돌아가며 한 곡씩.
    Fair,
}

impl QueueSortMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Score => "score",
            Self::Fifo => "fifo",
            Self::Fair => "fair",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "score" => Some(Self::Score),
            "fifo" => Some(Self::Fifo),
            "fair" => Some(Self::Fair),
            _ => None,
        }
    }

    /// 서버 관리 콘솔에서 모드마다 보여줄 한 줄 설명.
    pub fn description(self) -> &'static str {
        match self {
            Self::Score => "좋아요와 기다린 시간을 점수로 합산해 높은 곡부터 재생한다.",
            Self::Fifo => "신청한 순서 그대로 재생한다. 좋아요는 표시만 되고 순서를 바꾸지 않는다.",
            Self::Fair => "사람별로 돌아가며 한 곡씩 재생한다. 미리 여러 곡을 넣어도 새치기가 안 된다.",
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
    /// 공평제에서 "그 사람의 몇 번째 곡"인지 (0-based). 정렬 시 계산해 채운다.
    #[serde(default)]
    pub round: i32,
    /// 이 곡을 신청한 사람이 마지막으로 곡을 재생한 시각. 없으면 아직 한 곡도 못 튼 사람.
    #[serde(default)]
    pub last_played_utc: Option<String>,
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
    pub edited_utc: Option<String>,
    pub reactions: Vec<ChatReactionSummary>,
    /// 인용 답장 프리뷰. 원문이 조회 범위 밖이어도 채워진다.
    pub reply_to: Option<ChatReplyPreview>,
    /// 이 메시지가 부른 사람들.
    pub mentions: Vec<u64>,
    /// #노래태그로 붙은 곡들. 클라이언트가 칩으로 렌더한다.
    pub tags: Vec<ChatTrackTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReactionSummary {
    pub emoji: String,
    pub count: i32,
    pub reacted_by_me: bool,
}

/// 답장 대상의 최소 정보. 원문 전체를 다시 싣지 않는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReplyPreview {
    pub id: i64,
    pub display_name: String,
    /// 본문 앞 80자.
    pub excerpt: String,
    pub deleted: bool,
}

impl ChatReplyPreview {
    pub const EXCERPT_CHARS: usize = 80;

    pub fn excerpt_of(content: &str) -> String {
        content.chars().take(Self::EXCERPT_CHARS).collect()
    }
}

/// 채팅에 붙은 노래 태그. 클릭하면 그대로 대기열에 담을 수 있도록 곡 전체를 들고 있다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTrackTag {
    pub cache_key: String,
    pub track: TrackRef,
}

/// 이 서버에서 리모컨을 써 본 사람. @멘션 자동완성 후보다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    pub user_id: u64,
    /// 채팅 기록이 없는(곡만 신청한) 사람은 빈 문자열 — 호출부가 Discord 캐시로 채운다.
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub last_active_utc: String,
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

/// 앱 개선 제안의 처리 상태. 관리자만 바꾼다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    #[default]
    Open,
    Reviewing,
    Planned,
    Done,
    Declined,
}

impl SuggestionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Reviewing => "reviewing",
            Self::Planned => "planned",
            Self::Done => "done",
            Self::Declined => "declined",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "reviewing" => Some(Self::Reviewing),
            "planned" => Some(Self::Planned),
            "done" => Some(Self::Done),
            "declined" => Some(Self::Declined),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "접수됨",
            Self::Reviewing => "검토중",
            Self::Planned => "반영 예정",
            Self::Done => "반영됨",
            Self::Declined => "보류",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub title: String,
    pub body: String,
    pub status: SuggestionStatus,
    pub status_note: Option<String>,
    pub status_by_user_id: Option<u64>,
    pub status_utc: Option<String>,
    pub created_utc: String,
    pub vote_count: i32,
    pub voted_by_me: bool,
}

/// 정지 범위. `All`은 읽기전용 강등까지 포함한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspensionScope {
    All,
    Chat,
    Queue,
}

impl SuspensionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Chat => "chat",
            Self::Queue => "queue",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "chat" => Some(Self::Chat),
            "queue" => Some(Self::Queue),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "전체",
            Self::Chat => "채팅만",
            Self::Queue => "신청만",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suspension {
    pub guild_id: u64,
    pub user_id: u64,
    pub scope: SuspensionScope,
    pub reason: Option<String>,
    pub by_user_id: u64,
    pub created_utc: String,
    /// NULL = 무기한.
    pub expires_utc: Option<String>,
}

/// DB에 영속화되는 웹 세션. 토큰 원문은 저장하지 않으므로 여기에도 없다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
    pub user_id: u64,
    pub display_name: String,
    pub avatar_url: Option<String>,
    /// `Vec<OAuthGuild>`를 직렬화한 그대로. 도메인 계층은 내용을 해석하지 않는다.
    pub guilds_json: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_utc: String,
    pub refreshed_utc: Option<String>,
    pub created_utc: String,
}

/// 보존 정리 기준값. 길드 설정이 있으면 길드 설정이 이긴다.
#[derive(Debug, Clone, Copy)]
pub struct RetentionConfig {
    /// 길드 설정이 없을 때 쓸 채팅 보존 일수.
    pub chat_days: u32,
    /// 길드별로 남길 최근 재생 건수.
    pub recent_keep: usize,
    /// 길드 설정이 없을 때 쓸 활동 로그 보존 일수.
    pub audit_days: i32,
    /// 가사 실패(negative cache) 보존 일수.
    pub lyrics_failure_days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            chat_days: 30,
            recent_keep: 500,
            audit_days: 14,
            lyrics_failure_days: 7,
        }
    }
}

/// 정리 결과. 기동 로그에 그대로 찍을 수 있게 건수만 담는다.
#[derive(Debug, Clone, Copy, Default)]
pub struct PruneReport {
    pub chat: usize,
    pub recent: usize,
    pub audit: usize,
    pub lyrics: usize,
    pub sessions: usize,
}

impl PruneReport {
    pub fn is_empty(&self) -> bool {
        self.chat == 0 && self.recent == 0 && self.audit == 0 && self.lyrics == 0
            && self.sessions == 0
    }
}

/// 가사 캐시 조회 결과. "아직 안 찾아봄"과 "찾아봤는데 없음"을 구분한다.
#[derive(Debug, Clone)]
pub enum LyricsCacheHit {
    Found(Box<LyricsDocument>),
    Missing,
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
    /// 대기열 정렬 방식. 관리자만 변경 가능.
    #[serde(default)]
    pub sort_mode: QueueSortMode,
    /// 채팅 보존 일수.
    #[serde(default = "default_chat_retention_days")]
    pub chat_retention_days: u32,
    /// 제안 게시판 사용 여부.
    #[serde(default = "default_true")]
    pub suggestion_enabled: bool,
    /// 장식용 비주얼라이저 표시 여부.
    #[serde(default = "default_true")]
    pub visualizer_enabled: bool,
}

fn default_chat_retention_days() -> u32 {
    30
}

fn default_true() -> bool {
    true
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
            sort_mode: QueueSortMode::Score,
            chat_retention_days: default_chat_retention_days(),
            suggestion_enabled: true,
            visualizer_enabled: true,
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
