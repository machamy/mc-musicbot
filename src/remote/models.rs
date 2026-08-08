use crate::models::{EmptyVoiceChannelPolicy, TrackRef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 권한 규칙 키 10종 (v3 §1 + §8.3 + §10.5 + §15.4). 관리 콘솔의 "권한" 섹션 순서이자
/// `rule_role_ids`의 키다. 관리자(`manager_role_ids`)는 여기 없는 별개 축이라 총 11종이다.
/// 여기 없는 키로 `roles_for`를 부르면 레거시 지정 역할로 폴백한다.
pub const PERMISSION_KEYS: [&str; 10] = [
    "search",
    "vote",
    "chat",
    "playback",
    "skip",
    "seek",
    "volume",
    "queueEdit",
    "autoplay",
    "bulkEnqueue",
];

/// 이름이 바뀐 권한 키의 옛 이름. 저장된 `rule_role_ids`에 새 키가 아직 없으면
/// 옛 키를 먼저 본다 — 관리자가 지정해 둔 역할이 개명 때문에 조용히 사라지면 안 된다.
const PERMISSION_KEY_ALIASES: [(&str, &str); 1] = [("autoplay", "autoplaySeed")];

/// 길드당 자동 재생 시드곡 기본 상한. 길드 설정(`autoplay_seed_max`)이 이기고,
/// 그 값이 `0`이면 무제한이다(§23.1).
pub const MAX_AUTOPLAY_SEEDS: usize = 10;

/// 투표 점수 설정의 허용 범위 (§10.1).
pub const VOTE_POINT_MIN: i32 = -10;
pub const VOTE_POINT_MAX: i32 = 10;

/// `0 = 무제한` 규약(§23.1)을 한 곳에서 푼다. 서버 코드가 `.max(1)` 같은 클램프를 쓰면
/// `0`이 `1`로 둔갑해 "무제한"이 "가장 빡빡함"이 돼 버린다 — 그래서 전부 이 함수를 지난다.
pub fn as_limit(value: i32) -> Option<i32> {
    if value <= 0 { None } else { Some(value) }
}

/// `as_limit`의 부호 없는 버전.
pub fn as_limit_u32(value: u32) -> Option<u32> {
    if value == 0 { None } else { Some(value) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueVoteKind {
    Like,
    SuperLike,
    /// 싫어요 (§10.2). 좋아요·슈퍼 좋아요와 상호 배타다.
    Dislike,
}

impl QueueVoteKind {
    /// 이 투표 한 표가 곡 점수에 더하는 값. 하드코딩 배수는 없고 서버 설정을 그대로 쓴다(§10.1).
    pub fn points(self, points: &VotePoints) -> i32 {
        match self {
            Self::Like => points.like,
            Self::SuperLike => points.super_like,
            Self::Dislike => points.dislike,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Like => "Like",
            Self::SuperLike => "SuperLike",
            Self::Dislike => "Dislike",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Like" => Some(Self::Like),
            "SuperLike" => Some(Self::SuperLike),
            "Dislike" => Some(Self::Dislike),
            _ => None,
        }
    }

    /// `/vote` 요청·응답에 쓰는 소문자 키.
    pub fn api_key(self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::SuperLike => "superLike",
            Self::Dislike => "dislike",
        }
    }

    /// 활동 로그 액션명 (§13.3).
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::Like => "vote.like",
            Self::SuperLike => "vote.superlike",
            Self::Dislike => "vote.dislike",
        }
    }
}

/// 서버가 정한 투표 점수표 (§10.1). `total_score`가 이 값으로만 계산한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VotePoints {
    pub like: i32,
    pub dislike: i32,
    pub super_like: i32,
    /// 곡 하나가 지날 때마다 붙는 대기 가점.
    pub wait: i32,
}

impl Default for VotePoints {
    fn default() -> Self {
        Self {
            like: default_like_points(),
            dislike: default_dislike_points(),
            super_like: default_super_like_points(),
            wait: default_wait_points(),
        }
    }
}

impl VotePoints {
    /// 길드 설정에서 뽑아 온 점수표. 범위를 벗어난 값은 여기서 잘린다.
    pub fn from_settings(settings: &RemoteGuildSettings) -> Self {
        Self {
            like: settings.like_points,
            dislike: settings.dislike_points,
            super_like: settings.super_like_points,
            wait: settings.wait_points,
        }
        .clamped()
    }

    pub fn clamped(self) -> Self {
        let clamp = |value: i32| value.clamp(VOTE_POINT_MIN, VOTE_POINT_MAX);
        Self {
            like: clamp(self.like),
            dislike: clamp(self.dislike),
            super_like: clamp(self.super_like),
            wait: clamp(self.wait),
        }
    }
}

fn default_like_points() -> i32 {
    1
}
fn default_dislike_points() -> i32 {
    -1
}
fn default_super_like_points() -> i32 {
    2
}
fn default_wait_points() -> i32 {
    1
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
            Self::Score => "좋아요와 기다린 시간을 점수로 합산해 높은 곡부터 재생해요.",
            Self::Fifo => "신청한 순서 그대로 재생해요. 좋아요는 표시만 되고 순서를 바꾸지 않아요.",
            Self::Fair => "사람별로 돌아가며 한 곡씩 재생해요. 미리 여러 곡을 넣어도 새치기가 안 돼요.",
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
    /// 싫어요 수 (§10.2). 붐따 판정(§10.3)도 이 값을 본다.
    #[serde(default)]
    pub dislike_count: i32,
    pub manual_priority: Option<i32>,
    pub original_order: i64,
    /// 공평제에서 "그 사람의 몇 번째 곡"인지 (0-based). 정렬 시 계산해 채운다.
    #[serde(default)]
    pub round: i32,
    /// 이 곡을 신청한 사람이 마지막으로 곡을 재생한 시각. 없으면 아직 한 곡도 못 튼 사람.
    #[serde(default)]
    pub last_played_utc: Option<String>,
    /// 좋아요를 누른 사람 (§10.4). **이름이 아니라 ID**이고 항목당 최대 `MAX_VOTER_IDS`명이다.
    #[serde(default)]
    pub like_by: Vec<u64>,
    #[serde(default)]
    pub super_by: Vec<u64>,
    #[serde(default)]
    pub dislike_by: Vec<u64>,
}

/// 한 항목이 내보내는 투표자 ID 상한 (§10.4). 대기열 50곡 × 투표자 전원을 실으면 payload가 터진다.
pub const MAX_VOTER_IDS: usize = 12;

impl QueueScore {
    /// 서버가 정한 점수표로 계산한 총점. **하드코딩 배수는 없다** (§10.1).
    pub fn total_score(&self, points: &VotePoints) -> i32 {
        self.wait_score * points.wait
            + self.like_count * points.like
            + self.super_like_count * points.super_like
            + self.dislike_count * points.dislike
    }

    /// 화면의 계산식(`👍3 + ⭐1×2 + 대기2 = 7`)을 서버가 만들어 준다.
    /// 클라이언트가 배수를 다시 곱하면 설정을 바꿨을 때 화면이 거짓말을 한다(§10.4).
    pub fn formula(&self, points: &VotePoints) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut push = |emoji: &str, count: i32, unit: i32| {
            if count == 0 || unit == 0 {
                return;
            }
            if unit == 1 {
                parts.push(format!("{emoji}{count}"));
            } else {
                parts.push(format!("{emoji}{count}×{unit}"));
            }
        };
        push("👍", self.like_count, points.like);
        push("⭐", self.super_like_count, points.super_like);
        push("👎", self.dislike_count, points.dislike);
        push("대기", self.wait_score, points.wait);
        if parts.is_empty() {
            return format!("아직 점수가 없어요 = {}", self.total_score(points));
        }
        format!("{} = {}", parts.join(" + "), self.total_score(points))
    }

    /// 붐따 기준을 넘겼는지 (§10.3). 꺼져 있거나 기준이 `0`(무제한)이면 절대 안 걸린다.
    pub fn boomtta_triggered(&self, settings: &RemoteGuildSettings) -> bool {
        settings.boomtta_enabled
            && as_limit_u32(settings.boomtta_threshold)
                .is_some_and(|threshold| self.dislike_count >= threshold as i32)
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

/// 자동 재생이 참고할 기준 곡. 길드당 최대 `MAX_AUTOPLAY_SEEDS`곡이고,
/// 추천 엔진이 `sort_order` 순서를 라운드로빈으로 돈다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoplaySeed {
    pub guild_id: u64,
    pub cache_key: String,
    pub track: TrackRef,
    pub sort_order: i64,
    pub added_by_user_id: u64,
    pub added_utc: String,
}

/// 시드곡 추가 결과. 실패 사유마다 화면에 그대로 쓸 문장이 붙는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedAddOutcome {
    Added,
    /// 같은 곡이 이미 기준 곡에 있다.
    Duplicate,
    /// 상한을 넘겼다. 값은 그 서버의 상한(곡 수)이다.
    LimitReached(u32),
}

impl SeedAddOutcome {
    pub fn is_added(self) -> bool {
        matches!(self, Self::Added)
    }

    /// 사용자에게 그대로 보여줄 안내 문구.
    pub fn message(self) -> String {
        match self {
            Self::Added => "기준 곡에 넣었어요.".into(),
            Self::Duplicate => "이미 기준 곡에 있는 곡이에요.".into(),
            Self::LimitReached(max) => format!("시드곡은 {max}곡까지 넣을 수 있어요."),
        }
    }
}

/// 붐따가 걸렸을 때 그 곡을 어떻게 할지 (§10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoomttaAction {
    /// 맨 뒤로 보낸다 (기본). 곡이 사라지지 않아 되돌리기 쉽다.
    #[default]
    Bottom,
    /// 대기열에서 아예 뺀다.
    Remove,
}

impl BoomttaAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Remove => "remove",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bottom" => Some(Self::Bottom),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bottom => "맨 뒤로 보내요",
            Self::Remove => "대기열에서 빼요",
        }
    }
}

/// 투표 스킵의 모수를 무엇으로 볼지 (§10.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteSkipBasis {
    /// 봇과 같은 음성 채널에 있는 사람 (기본).
    #[default]
    Listeners,
    /// 리모컨을 보고 있는 사람.
    Viewers,
    /// 둘 중 하나라도 넘으면 통과.
    Either,
    /// 둘 다 넘어야 통과.
    Both,
}

impl VoteSkipBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Listeners => "listeners",
            Self::Viewers => "viewers",
            Self::Either => "either",
            Self::Both => "both",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "listeners" => Some(Self::Listeners),
            "viewers" => Some(Self::Viewers),
            "either" => Some(Self::Either),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Listeners => "봇과 같은 음성 채널에 있는 사람만 세요.",
            Self::Viewers => "리모컨을 보고 있는 사람을 세요.",
            Self::Either => "듣는 사람이나 보는 사람 중 한쪽만 넘어도 넘어가요.",
            Self::Both => "듣는 사람과 보는 사람이 둘 다 넘어야 넘어가요.",
        }
    }

    /// 필요 표 수 = `ceil(모수 × ratio / 100)`, 단 최소 `min_votes`명.
    /// 모수가 그보다 적으면 모수가 곧 필요 표 수다 — 혼자 듣는데 2명을 요구하면 영원히 안 넘어간다.
    pub fn votes_needed(population: u32, ratio: u32, min_votes: u32) -> u32 {
        if population == 0 {
            return 0;
        }
        let ratio = ratio.clamp(10, 100);
        let by_ratio = population.saturating_mul(ratio).div_ceil(100).max(1);
        by_ratio.max(min_votes).min(population)
    }
}

/// 곡이 바뀔 때 Discord 채널에 **새 카드를 보낼지, 있던 카드를 고칠지** (§25).
///
/// 봇 하나를 여러 서버가 쓰기 시작하면 이게 곧바로 문제가 된다. 곡마다 새 글을 쌓으면
/// 세 시간짜리 파티 뒤에 채널이 재생 카드 60장으로 도배된다.
/// 그래서 **기본은 갱신**이다 — 카드 한 장이 계속 지금 곡을 보여 준다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NowPlayingMode {
    /// 있던 카드를 고쳐 쓴다. 채널이 조용하다.
    #[default]
    Edit,
    /// 곡마다 새 카드를 보낸다. 기록이 남지만 채널이 길어진다.
    New,
    /// 아예 안 보낸다. 리모컨만 쓰는 서버용.
    Off,
}

impl NowPlayingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::New => "new",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "edit" => Some(Self::Edit),
            "new" => Some(Self::New),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Edit => "카드 하나를 갱신",
            Self::New => "곡마다 새 글",
            Self::Off => "안 보냄",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Edit => "재생 카드 한 장이 계속 지금 곡을 보여줘요. 채널이 안 밀려요.",
            Self::New => "곡이 바뀔 때마다 새 글이 올라와요. 뭘 들었는지 기록이 남아요.",
            Self::Off => "Discord에는 안 알려요. 리모컨에서만 봐요.",
        }
    }
}

/// 자동 재생이 **시드를 어디서 고르는지** (§8). 기본은 지금 동작인 `Recent`다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoplayMode {
    /// 직접 등록한 기준 곡 1~10곡을 라운드로빈.
    Seed,
    /// 최근에 튼 N곡 중 무작위 (기본, 지금 동작).
    #[default]
    Recent,
    /// 고른 장르 차트에서 무작위.
    Genre,
}

impl AutoplayMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Recent => "recent",
            Self::Genre => "genre",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "seed" => Some(Self::Seed),
            "recent" => Some(Self::Recent),
            "genre" => Some(Self::Genre),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Seed => "기준 곡",
            Self::Recent => "최근에 튼 곡",
            Self::Genre => "장르",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Seed => "직접 고른 기준 곡을 돌아가며 참고해요.",
            Self::Recent => "최근에 튼 곡 중 하나를 골라 참고해요.",
            Self::Genre => "고른 장르 차트에서 한 곡을 골라 참고해요.",
        }
    }

    /// 폴백 사슬 (§8.2). 후보를 못 구하면 조용히 멈추지 말고 다음 모드로 내려간다.
    pub fn fallback(self) -> Option<Self> {
        match self {
            Self::Seed => Some(Self::Recent),
            Self::Recent => Some(Self::Genre),
            Self::Genre => None,
        }
    }
}

/// 라디오 후보 **목록에서 어떤 곡을 집는지** (§8.5). 시드 선택(`AutoplayMode`)과는 다른 축이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoplayPolicy {
    /// 후보 상위 3곡 중 무작위.
    Similar,
    /// 후보 상위 10곡 중 가중 무작위 (기본).
    #[default]
    Balanced,
    /// 후보 전체에서 균등 무작위.
    Explore,
    /// 길이가 무난한 후보 위주로 무작위.
    Popular,
}

impl AutoplayPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Similar => "similar",
            Self::Balanced => "balanced",
            Self::Explore => "explore",
            Self::Popular => "popular",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "similar" => Some(Self::Similar),
            "balanced" => Some(Self::Balanced),
            "explore" => Some(Self::Explore),
            "popular" => Some(Self::Popular),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Similar => "비슷하게",
            Self::Balanced => "적당히",
            Self::Explore => "새롭게",
            Self::Popular => "무난하게",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Similar => "기준 곡과 가장 비슷한 곡 중에서 골라요. 분위기가 유지돼요.",
            Self::Balanced => "비슷한 곡 위주로 고르되 매번 다른 곡이 나와요.",
            Self::Explore => "후보 전체에서 골라요. 예상 못 한 곡이 나와요.",
            Self::Popular => "길이가 무난한 곡 위주로 골라요.",
        }
    }

    /// 후보가 부족해 시드를 갈아탈 때마다 한 단계 느슨해진다 (§8.5-4).
    pub fn loosened(self) -> Self {
        match self {
            Self::Similar => Self::Balanced,
            Self::Balanced | Self::Explore | Self::Popular => Self::Explore,
        }
    }

    /// 이 정책이 들여다볼 후보 상위 개수. `None`이면 전체를 본다.
    pub fn window(self) -> Option<usize> {
        match self {
            Self::Similar => Some(3),
            Self::Balanced => Some(10),
            Self::Explore => None,
            Self::Popular => None,
        }
    }
}

// ───────── 차트 (§15) ─────────

/// 차트 캐시 수명(시간). 차트 하나 펼치는 데 yt-dlp 가 몇 초씩 걸려서
/// 여러 사람이 같은 차트를 눌러도 이 시간 안에는 한 번만 돈다.
pub const CHART_CACHE_TTL_HOURS: i64 = 6;

/// 노래방 차트 캐시 수명(시간). **하루가 넘는다** (§15.2c).
///
/// TJ 순위는 하루 단위로 움직인다. 6시간마다 다시 긁어 봐야 같은 목록이고,
/// 노래방은 곡마다 원곡을 찾느라 다른 차트보다 훨씬 오래 걸린다.
/// 자주 받아서 얻는 게 없고 잃는 것만 있다.
pub const KARAOKE_CACHE_TTL_HOURS: i64 = 30;

/// 차트 분류 (§15.2 · §15.3). 유저 UI 1단계의 카드 6장이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartCategory {
    /// 우리가 실제로 튼 것으로 만드는 차트 (§15.2b). 통계 DB 에서 나온다.
    Ours,
    Popular,
    Region,
    Genre,
    Karaoke,
    Soundcloud,
}

impl ChartCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ours => "ours",
            Self::Popular => "popular",
            Self::Region => "region",
            Self::Genre => "genre",
            Self::Karaoke => "karaoke",
            Self::Soundcloud => "soundcloud",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ours" => Some(Self::Ours),
            "popular" => Some(Self::Popular),
            "region" => Some(Self::Region),
            "genre" => Some(Self::Genre),
            "karaoke" => Some(Self::Karaoke),
            "soundcloud" => Some(Self::Soundcloud),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ours => "우리 차트",
            Self::Popular => "인기",
            Self::Region => "나라별",
            Self::Genre => "장르",
            Self::Karaoke => "노래방",
            Self::Soundcloud => "SoundCloud",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Ours => "⭐",
            Self::Popular => "🔥",
            Self::Region => "🌏",
            Self::Genre => "🎸",
            Self::Karaoke => "🎤",
            Self::Soundcloud => "☁",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Ours => "우리가 많이 튼 곡",
            Self::Popular => "지금 많이 듣는 곡",
            Self::Region => "미국·일본·영국",
            Self::Genre => "K-Pop·힙합·록·R&B",
            Self::Karaoke => "TJ·금영 장르별",
            Self::Soundcloud => "SoundCloud 인기곡",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::Ours,
        Self::Popular,
        Self::Region,
        Self::Genre,
        Self::Karaoke,
        Self::Soundcloud,
    ];
}

/// 차트 한 장의 정의. **코드가 아니라 데이터**라 유튜브가 재생목록 ID 를 바꿔도
/// 관리 콘솔에서 주소만 갈아 끼우면 된다 (§15.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartDef {
    pub id: i64,
    /// `None`이면 모든 서버 공용(기본 제공분).
    pub guild_id: Option<u64>,
    pub category: ChartCategory,
    pub name: String,
    pub provider: String,
    pub url: String,
    pub sort_order: i64,
    pub enabled: bool,
    /// 기본 제공분은 지울 수 없고 끄기만 된다 (§15.5). 되돌릴 수 없는 삭제는 위험하다.
    pub builtin: bool,
    pub last_fetched_utc: Option<String>,
    pub last_failure_utc: Option<String>,
    pub last_failure_reason: Option<String>,
    /// 캐시에 들어 있는 곡 수. 0이면 아직 한 번도 안 펼쳤거나 실패했다.
    pub track_count: usize,
}

impl ChartDef {
    /// 바깥에서 가져오지 않고 통계 DB 에서 만드는 차트인지 (§15.2b).
    pub fn is_internal(&self) -> bool {
        self.url.starts_with(INTERNAL_CHART_PREFIX)
    }

    /// 마지막 갱신이 성공했는지. 실패한 차트는 유저 UI 목록에서 빼고
    /// 관리 콘솔에는 실패로 표시한다 — 빈 차트를 눌렀는데 아무 일도 안 일어나는 게 제일 나쁘다.
    pub fn ok(&self) -> bool {
        self.is_internal() || self.track_count > 0 || self.last_failure_utc.is_none()
    }
}

/// 통계 DB 로 만드는 차트의 주소 접두사. `internal:guild-plays` 같은 값이 들어간다.
pub const INTERNAL_CHART_PREFIX: &str = "internal:";

/// 캐시에서 꺼낸 차트 곡 목록.
#[derive(Debug, Clone)]
pub struct ChartSnapshot {
    pub tracks: Vec<TrackRef>,
    pub fetched_utc: String,
    /// TTL(6시간)이 지났는지. 지났어도 일단 보여 주고 뒤에서 다시 받는 편이 화면이 덜 비어 보인다.
    pub stale: bool,
}

/// 이 서버가 봇을 쓸 수 있는지 (§26).
///
/// 봇 초대는 Discord 쪽에서 아무나 할 수 있다. 그래서 **초대는 막지 못해도 사용은 막는다** —
/// 새 서버는 대기 상태로 들어오고, 봇 주인이 승인해야 명령어와 리모컨이 열린다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuildApprovalStatus {
    /// 초대는 됐지만 아직 못 쓴다.
    #[default]
    Pending,
    Approved,
    /// 거절됐다. 다시 초대해도 대기로 돌아가지 않는다.
    Blocked,
}

impl GuildApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "승인 대기",
            Self::Approved => "사용 중",
            Self::Blocked => "차단됨",
        }
    }

    /// 이 상태에서 봇을 쓸 수 있는가.
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Approved)
    }

    /// 못 쓸 때 사람에게 보여 줄 이유. 막힌 사실만 말하면 다음에 뭘 할지 모른다.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Pending => {
                "이 서버는 아직 봇 주인의 승인을 기다리고 있어요. 승인되면 바로 쓸 수 있어요."
            }
            Self::Approved => "",
            Self::Blocked => "이 서버에서는 봇을 쓸 수 없어요. 봇 주인이 사용을 막아 뒀어요.",
        }
    }
}

/// 승인 대기·사용 중인 서버 한 줄. 운영 패널이 이걸 표로 보여준다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildApproval {
    pub guild_id: u64,
    pub status: GuildApprovalStatus,
    pub guild_name: Option<String>,
    pub invited_by: Option<u64>,
    pub invited_by_name: Option<String>,
    pub requested_utc: String,
    pub decided_by: Option<u64>,
    pub decided_utc: Option<String>,
    pub note: Option<String>,
}

/// 재시작 직전에 남겨 둔 재생 지점 (§24).
#[derive(Debug, Clone)]
pub struct ResumePoint {
    /// 그때 틀던 대기열 항목. 기동 뒤 현재 곡과 다르면 이어 붙이지 않는다.
    pub item_id: Option<String>,
    pub position_seconds: f64,
    pub was_paused: bool,
    /// 저장한 지 몇 시간 지났는지. 오래된 기록으로 옛날 곡을 되살리면 안 된다.
    pub age_hours: f64,
}

// ───────── 슈퍼 좋아요 제한 (§10.6) ─────────

/// 슈퍼 좋아요를 지금 쓸 수 있는지. 거부할 때 **이유를 정확히** 말한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperLikeVerdict {
    Allowed {
        used_today: u32,
        /// 오늘 남은 횟수. `None`이면 무제한.
        remaining: Option<u32>,
    },
    Cooldown {
        remaining_sec: u32,
    },
    DailyLimitReached {
        limit: u32,
    },
}

impl SuperLikeVerdict {
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed { .. })
    }

    /// 거부 사유 문장. 통과했으면 `None`.
    pub fn message(self) -> Option<String> {
        match self {
            Self::Allowed { .. } => None,
            Self::Cooldown { remaining_sec } => {
                let minutes = remaining_sec / 60;
                let seconds = remaining_sec % 60;
                let when = if minutes > 0 {
                    format!("{minutes}분 {seconds}초")
                } else {
                    format!("{seconds}초")
                };
                Some(format!("슈퍼 좋아요는 {when} 뒤에 다시 쓸 수 있어요."))
            }
            Self::DailyLimitReached { limit } => Some(format!(
                "오늘 슈퍼 좋아요를 {limit}번 다 썼어요 (UTC 자정에 초기화돼요)."
            )),
        }
    }
}

/// `/state/cold` 의 `superLike` (§10.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperLikeStatus {
    pub cooldown_sec: u32,
    pub daily_limit: u32,
    pub used_today: u32,
    /// 오늘 남은 횟수. `None`이면 무제한.
    pub remaining: Option<u32>,
    /// 쿨타임이 끝나는 시각. 지금 쓸 수 있으면 `None`.
    pub available_at_utc: Option<String>,
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

/// 활동 로그 분류 6종 (§13.3~13.4). 유저 UI 의 필터 칩이자 보존 기간의 단위다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    /// 곡을 담고 빼고 올린 일.
    #[default]
    Song,
    /// 좋아요·슈퍼 좋아요·싫어요.
    Vote,
    /// 재생·일시정지·스킵·이동·볼륨·자동 재생 토글.
    Playback,
    /// 재생목록과 자동 재생 기준 곡.
    Playlist,
    /// 메시지 삭제·정지·차단 목록.
    Moderation,
    /// 서버 설정 변경.
    Admin,
}

impl AuditKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Vote => "vote",
            Self::Playback => "playback",
            Self::Playlist => "playlist",
            Self::Moderation => "moderation",
            Self::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "song" => Some(Self::Song),
            "vote" => Some(Self::Vote),
            "playback" => Some(Self::Playback),
            "playlist" => Some(Self::Playlist),
            "moderation" => Some(Self::Moderation),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    /// 필터 칩 문구 (§13.4).
    pub fn label(self) -> &'static str {
        match self {
            Self::Song => "🎵 곡",
            Self::Vote => "👍 투표",
            Self::Playback => "▶ 재생",
            Self::Playlist => "📃 재생목록",
            Self::Moderation => "🛡 관리",
            Self::Admin => "🛡 관리",
        }
    }

    /// 유저 UI 로그 탭의 기본 필터 (§13.4) — 곡과 재생목록만 켠다.
    /// 투표는 사람이 많으면 초당 여러 줄이 쌓여 다른 게 안 보인다.
    pub fn default_filter() -> [Self; 2] {
        [Self::Song, Self::Playlist]
    }

    pub const ALL: [Self; 6] = [
        Self::Song,
        Self::Vote,
        Self::Playback,
        Self::Playlist,
        Self::Moderation,
        Self::Admin,
    ];

    /// 분류별 보존 기간 (§13.6). 투표·재생은 양이 확 늘어나므로 3일만 남긴다.
    /// `0`(무제한)이면 그대로 무제한이다 — 짧은 쪽으로 덮어쓰지 않는다.
    pub fn retention_days(self, configured_days: i32) -> i32 {
        match self {
            Self::Vote | Self::Playback if configured_days != 0 => configured_days.min(3),
            _ => configured_days,
        }
    }
}

/// 액션명 → 분류 (§13.3). 모르는 액션은 관리자 화면에만 나오도록 `Admin`으로 떨어뜨린다 —
/// 사람 피드의 기본 필터에 안 잡히므로 조용히 새어 나가지 않는다.
pub fn audit_kind_for(action: &str) -> AuditKind {
    match action {
        _ if action.starts_with("queue.") => AuditKind::Song,
        "playlist.enqueue" | "chart.enqueue" => AuditKind::Song,
        _ if action.starts_with("vote.") => AuditKind::Vote,
        _ if action.starts_with("playback.") || action.starts_with("autoplay.toggle") => {
            AuditKind::Playback
        }
        _ if action.starts_with("playlist.") || action.starts_with("autoplay.") => {
            AuditKind::Playlist
        }
        _ if action.starts_with("chat.")
            || action.starts_with("user.")
            || action.starts_with("blacklist.")
            || action.starts_with("suggestion.") =>
        {
            AuditKind::Moderation
        }
        _ => AuditKind::Admin,
    }
}

/// 곡 제목은 40자에서 자른다 (§13.3). 전체는 툴팁이 보여준다.
pub const AUDIT_TITLE_CHARS: usize = 40;

pub fn truncate_title(title: &str) -> String {
    let trimmed: String = title.chars().take(AUDIT_TITLE_CHARS).collect();
    if trimmed.chars().count() < title.chars().count() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

/// 합쳐진 로그 한 줄이 될 수 있는 액션 (§13.3). 같은 사람·같은 종류가 60초 안에 반복되면
/// 새 줄을 만들지 않고 기존 줄의 숫자만 올린다.
pub fn is_mergeable_action(action: &str) -> bool {
    matches!(
        action,
        "queue.add" | "queue.remove" | "vote.like" | "vote.superlike" | "vote.dislike"
    )
}

/// 로그 합치기 창(초). 이 안에 같은 사람이 같은 일을 또 하면 기존 줄을 갱신한다.
pub const AUDIT_MERGE_WINDOW_SECS: i64 = 60;

/// **사람이 읽는 문장을 서버가 완성한다** (§13.5). 클라이언트가 액션명을 문장으로 바꾸는
/// 로직을 갖지 않게 하려는 것이라, 여기 없는 액션도 반드시 말이 되는 문장을 돌려준다.
///
/// - `actor` 는 누가 했는지, `target` 은 곡·항목 이름, `count` 는 합쳐진 개수(1이면 단수 문장).
/// `POST /control` 은 결과를 `"volume:150"` · `"autoplay:true"` 처럼 `키:값` 으로 남긴다.
/// 사람 피드에 그대로 박으면 `서버 볼륨을 volume:150으로 바꿨어요` 가 나간다 —
/// 문장을 만들 때 접두사를 벗겨서 값만 쓴다. 접두사가 없으면 값을 그대로 돌려준다.
/// 재생목록 감사 대상은 `12:밤샘용` 처럼 `id:이름` 으로 남는다.
/// 사람 피드에 id 가 새면 안 되므로 이름만 꺼낸다. 이름에 `:` 가 있어도 첫 번째만 자른다.
fn playlist_name(item: Option<&str>) -> Option<&str> {
    let raw = item?.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.split_once(':') {
        // 앞이 전부 숫자일 때만 id 로 본다. `팝:최애` 같은 이름을 잘라내면 안 된다.
        Some((head, rest)) if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) => {
            Some(rest.trim())
        }
        _ => Some(raw),
    }
}

/// 설정 키를 사람이 읽는 이름으로. 모르는 키는 그대로 쓴다.
fn settings_label(key: &str) -> &str {
    match key {
        "minVolume" => "최소 볼륨",
        "maxVolume" => "최대 볼륨",
        "defaultVolume" => "기본 볼륨",
        "maxQueuePerUser" => "1인 대기열 수",
        "maxQueuePerGuild" => "서버 대기열 수",
        "maxTrackSeconds" => "곡 최대 길이",
        "auditRetentionDays" => "로그 보관일",
        "chatRetentionDays" => "채팅 보관일",
        "sortMode" => "대기열 정렬 방식",
        "chatEnabled" => "웹 채팅",
        "suggestionEnabled" => "제안 게시판",
        "visualizerEnabled" => "비주얼라이저",
        "autoBgmEnabled" => "자동 재생",
        "repeatMode" => "반복",
        "searchRule" => "곡 검색·신청 권한",
        "voteRule" => "좋아요 권한",
        "chatRule" => "채팅 쓰기 권한",
        "playbackRule" => "재생·일시정지 권한",
        "skipRule" => "스킵 권한",
        "seekRule" => "재생 위치 이동 권한",
        "volumeRule" => "볼륨 권한",
        "queueEditRule" => "대기열 편집 권한",
        "autoplayRule" => "자동 재생 설정 권한",
        "bulkEnqueueRule" => "한 번에 담기 권한",
        "managerRoleIds" => "관리자 지정 역할",
        "ruleRoleIds" => "권한별 지정 역할",
        "likePoints" => "좋아요 점수",
        "dislikePoints" => "싫어요 점수",
        "superLikePoints" => "슈퍼 좋아요 점수",
        "waitPoints" => "대기 가점",
        "boomttaEnabled" => "붐따",
        "boomttaThreshold" => "붐따 기준 수",
        "boomttaAction" => "붐따 동작",
        "voteSkipEnabled" => "투표 스킵",
        "voteSkipBasis" => "투표 스킵 기준",
        "voteSkipRatio" => "투표 스킵 비율",
        "voteSkipMin" => "투표 스킵 최소 인원",
        "superLikeCooldownSec" => "슈퍼 좋아요 쿨타임",
        "superLikeDailyLimit" => "슈퍼 좋아요 하루 횟수",
        "autoplayMode" => "자동 재생 방식",
        "autoplayPolicy" => "자동 재생 정책",
        "autoplayRecentCount" => "자동 재생 참고 곡 수",
        "autoplayGenres" => "자동 재생 장르",
        "autoplayArtistCooldown" => "같은 아티스트 쿨다운",
        "autoplayRecentDecayHours" => "최근 재생 회피 시간",
        "autoplaySeedMax" => "기준 곡 최대 수",
        "chartSuperWeight" => "차트 슈퍼 좋아요 가중치",
        "bulkEnqueueLimit" => "한 번에 담기 상한",
        "chartLimit" => "차트에서 가져올 곡 수",
        other => other,
    }
}

/// 설정 값 하나를 사람이 읽는 문자열로. 0은 대부분 "무제한"이다 (§23.1).
fn settings_value(key: &str, value: &serde_json::Value) -> String {
    // 규칙 값은 한국어 라벨이 따로 있다.
    if key.ends_with("Rule")
        && let Some(text) = value.as_str()
    {
        let label = match text {
            "guildMember" => Some("모든 멤버"),
            "sameVoiceChannel" => Some("같은 음성 채널"),
            "configuredRole" => Some("지정 역할"),
            "administrator" => Some("관리자"),
            "disabled" => Some("사용 안 함"),
            _ => None,
        };
        if let Some(label) = label {
            return label.to_string();
        }
    }
    match value {
        serde_json::Value::Bool(true) => "켬".into(),
        serde_json::Value::Bool(false) => "끔".into(),
        serde_json::Value::Null => "없음".into(),
        serde_json::Value::Array(items) if items.is_empty() => "없음".into(),
        serde_json::Value::Array(items) => format!("{}개", items.len()),
        serde_json::Value::Object(map) => format!("{}개", map.len()),
        serde_json::Value::Number(number) => {
            let raw = number.to_string();
            // 0 = 무제한 규약. 볼륨·비율처럼 0이 진짜 숫자인 키는 예외다.
            let unlimited_ok = !matches!(
                key,
                "minVolume"
                    | "maxVolume"
                    | "defaultVolume"
                    | "voteSkipRatio"
                    | "likePoints"
                    | "dislikePoints"
                    | "superLikePoints"
                    | "waitPoints"
                    | "chartSuperWeight"
            );
            if raw == "0" && unlimited_ok {
                "무제한".into()
            } else if key == "maxTrackSeconds"
                && let Some(seconds) = number.as_i64()
                && seconds > 0
            {
                let hours = seconds / 3600;
                let minutes = (seconds % 3600) / 60;
                if hours > 0 {
                    format!("{hours}시간")
                } else {
                    format!("{minutes}분")
                }
            } else {
                raw
            }
        }
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                "없음".into()
            } else {
                truncate_title(trimmed)
            }
        }
    }
}

/// 설정 변경을 사람이 읽는 문장으로.
///
/// **여기가 없으면 사람 피드에 설정 JSON 통째가 그대로 나간다.**
/// `마참 님이 limits 을 {"guildId":497...,"minVolume":0,...} → {...} 로 바꿨어요` 가
/// 실제로 화면에 나갔던 문장이다. 무엇이 바뀌었는지만 짚어 준다.
fn settings_change_text(actor: &str, section: &str, before: Option<&str>, after: Option<&str>) -> String {
    let parse = |raw: Option<&str>| -> Option<serde_json::Map<String, serde_json::Value>> {
        serde_json::from_str::<serde_json::Value>(raw?.trim())
            .ok()?
            .as_object()
            .cloned()
    };
    let (Some(old), Some(new)) = (parse(before), parse(after)) else {
        // 값이 통짜 JSON 이 아니라 단일 값인 경로도 있다(`settings.maxVolume` 처럼).
        // 그때는 그대로 전후를 보여 준다 — 이건 짧아서 읽는 데 문제가 없다.
        let what = settings_label(section);
        return match (before.map(str::trim), after.map(str::trim)) {
            (Some(from), Some(to)) if !from.is_empty() && !to.is_empty() => {
                format!(
                    "{actor}님이 **{what}** 을 {} → {} 로 바꿨어요",
                    truncate_title(from),
                    truncate_title(to)
                )
            }
            (_, Some(to)) if !to.is_empty() => {
                format!("{actor}님이 **{what}** 을 {} 로 바꿨어요", truncate_title(to))
            }
            _ => format!("{actor}님이 {what} 설정을 바꿨어요"),
        };
    };

    // 실제로 달라진 키만 모은다. guildId 처럼 설정이 아닌 것은 뺀다.
    let mut changed: Vec<(String, String, String)> = Vec::new();
    for (key, next) in &new {
        if key == "guildId" {
            continue;
        }
        let previous = old.get(key).unwrap_or(&serde_json::Value::Null);
        if previous == next {
            continue;
        }
        changed.push((
            settings_label(key).to_string(),
            settings_value(key, previous),
            settings_value(key, next),
        ));
    }

    match changed.len() {
        // 저장은 했는데 값이 그대로면 굳이 남길 게 없다.
        0 => format!("{actor}님이 설정을 저장했어요"),
        1 => {
            let (label, from, to) = &changed[0];
            format!("{actor}님이 **{label}** 을 {from} → {to} 로 바꿨어요")
        }
        _ => {
            let names: Vec<&str> = changed.iter().take(3).map(|(label, _, _)| label.as_str()).collect();
            let rest = changed.len().saturating_sub(names.len());
            let listed = names.join(", ");
            if rest > 0 {
                format!("{actor}님이 설정 {}개를 바꿨어요 ({listed} 외 {rest}개)", changed.len())
            } else {
                format!("{actor}님이 설정 {}개를 바꿨어요 ({listed})", changed.len())
            }
        }
    }
}

fn audit_value<'a>(after: Option<&'a str>, prefix: &str) -> Option<&'a str> {
    let raw = after?.trim();
    Some(
        raw.strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(':'))
            .unwrap_or(raw),
    )
}

/// 켜짐/꺼짐 계열 값을 한 곳에서 읽는다 (`true`/`on`/`1`).
fn audit_flag(value: Option<&str>) -> bool {
    matches!(value, Some("on" | "true" | "1"))
}

pub fn audit_text(
    action: &str,
    actor: &str,
    target: Option<&str>,
    before: Option<&str>,
    after: Option<&str>,
    count: u32,
) -> String {
    let item = target.map(truncate_title);
    let item = item.as_deref();
    let many = count > 1;
    match action {
        "queue.add" => match (item, many) {
            (_, true) => format!("{actor}님이 곡 {count}개를 담았어요"),
            (Some(title), false) => format!("{actor}님이 **{title}** 을 담았어요"),
            (None, false) => format!("{actor}님이 곡을 담았어요"),
        },
        "queue.remove" => match (item, many) {
            (_, true) => format!("{actor}님이 곡 {count}개를 뺐어요"),
            (Some(title), false) => format!("{actor}님이 **{title}** 을 뺐어요"),
            (None, false) => format!("{actor}님이 곡을 뺐어요"),
        },
        // `queue.force_move` 는 핸들러가 쓰는 옛 액션명이다. 문장을 못 찾으면
        // 사람 피드에 `민수님이 queue.force_move 을 했어요` 라는 기계 문자열이 그대로 나간다.
        "queue.pin" | "queue.force_move" => match (item, after == Some("unpinned")) {
            (Some(title), false) => format!("{actor}님이 **{title}** 을 맨 앞으로 올렸어요"),
            (Some(title), true) => format!("{actor}님이 **{title}** 을 맨 앞에서 내렸어요"),
            (None, false) => format!("{actor}님이 곡을 맨 앞으로 올렸어요"),
            (None, true) => format!("{actor}님이 곡을 맨 앞에서 내렸어요"),
        },
        "queue.boomtta" => match item {
            Some(title) => format!("**{title}** 이 싫어요 {count}개로 대기열에서 내려갔어요"),
            None => format!("어떤 곡이 싫어요 {count}개로 대기열에서 내려갔어요"),
        },
        "queue.clear" => format!("{actor}님이 대기열 {count}곡을 비웠어요"),
        "playlist.enqueue" => match playlist_name(item) {
            Some(name) => format!("{actor}님이 재생목록 **{name}** 에서 {count}곡을 담았어요"),
            None => format!("{actor}님이 재생목록에서 {count}곡을 담았어요"),
        },
        "chart.enqueue" => match item {
            Some(name) => format!("{actor}님이 차트 **{name}** 에서 {count}곡을 담았어요"),
            None => format!("{actor}님이 차트에서 {count}곡을 담았어요"),
        },
        "vote.like" | "vote.superlike" | "vote.dislike" => {
            let what = match action {
                "vote.like" => "좋아요",
                "vote.superlike" => "슈퍼 좋아요",
                _ => "싫어요",
            };
            match (item, many) {
                (_, true) => format!("{actor}님이 곡 {count}개에 {what}를 눌렀어요"),
                (Some(title), false) => format!("{actor}님이 **{title}** 에 {what}를 눌렀어요"),
                (None, false) => format!("{actor}님이 {what}를 눌렀어요"),
            }
        }
        "playback.pause" => format!("{actor}님이 일시정지했어요"),
        "playback.resume" => format!("{actor}님이 다시 재생했어요"),
        "playback.skip" => format!("{actor}님이 곡을 넘겼어요"),
        "playback.skip.vote" => format!("{count}명이 동의해서 곡을 넘겼어요"),
        "playback.seek" => format!("{actor}님이 재생 위치를 옮겼어요"),
        "playback.volume" => match audit_value(after, "volume") {
            Some(value) => format!("{actor}님이 서버 볼륨을 {value}으로 바꿨어요"),
            None => format!("{actor}님이 서버 볼륨을 바꿨어요"),
        },
        // 🔁 / 🎲 도 문장이 있어야 한다 — 없으면 `playback.repeat 을 했어요` 로 떨어진다.
        "playback.repeat" => match audit_value(after, "repeat") {
            Some("track") => format!("{actor}님이 한 곡 반복을 켰어요"),
            Some("queue") => format!("{actor}님이 대기열 반복을 켰어요"),
            _ => format!("{actor}님이 반복을 껐어요"),
        },
        "playback.shuffle" => {
            if audit_flag(audit_value(after, "shuffle")) {
                format!("{actor}님이 셔플을 켰어요")
            } else {
                format!("{actor}님이 셔플을 껐어요")
            }
        }
        // `playback.autoplay` 는 핸들러가 쓰는 옛 액션명이다 (§24.3 은 `autoplay.toggle`).
        "autoplay.toggle" | "playback.autoplay" => {
            if audit_flag(audit_value(after, "autoplay")) {
                format!("{actor}님이 자동 재생을 켰어요")
            } else {
                format!("{actor}님이 자동 재생을 껐어요")
            }
        }
        "playlist.create" => match playlist_name(item) {
            Some(name) => format!("{actor}님이 재생목록 **{name}** 을 만들었어요"),
            None => format!("{actor}님이 재생목록을 만들었어요"),
        },
        "playlist.rename" => match (playlist_name(before), playlist_name(after)) {
            (Some(old), Some(new)) => {
                format!("{actor}님이 재생목록 이름을 **{old}** 에서 **{new}** 로 바꿨어요")
            }
            _ => format!("{actor}님이 재생목록 이름을 바꿨어요"),
        },
        "playlist.delete" => match playlist_name(item) {
            Some(name) => format!("{actor}님이 재생목록 **{name}** 을 지웠어요"),
            None => format!("{actor}님이 재생목록을 지웠어요"),
        },
        // 곡 추가/제거는 대상이 `id:재생목록이름` 이라 이름만 꺼내 쓴다.
        // 이 두 개가 빠져 있어서 화면에 `playlist.addTrack 을 했어요 (1:aespa - Spicy)` 가 그대로 나갔다.
        "playlist.addTrack" => match playlist_name(item) {
            Some(name) => format!("{actor}님이 재생목록 **{name}** 에 곡을 담았어요"),
            None => format!("{actor}님이 재생목록에 곡을 담았어요"),
        },
        "playlist.removeEntry" => match playlist_name(item) {
            Some(name) => format!("{actor}님이 재생목록 **{name}** 에서 곡을 뺐어요"),
            None => format!("{actor}님이 재생목록에서 곡을 뺐어요"),
        },
        "autoplay.seed.add" => match item {
            Some(title) => format!("{actor}님이 **{title}** 을 자동 재생 기준 곡으로 등록했어요"),
            None => format!("{actor}님이 자동 재생 기준 곡을 등록했어요"),
        },
        "autoplay.seed.remove" => match item {
            Some(title) => format!("{actor}님이 자동 재생 기준 곡에서 **{title}** 을 뺐어요"),
            None => format!("{actor}님이 자동 재생 기준 곡을 뺐어요"),
        },
        "autoplay.seed.reorder" => format!("{actor}님이 자동 재생 기준 곡 순서를 바꿨어요"),
        "chat.delete" => format!("{actor}님이 메시지를 지웠어요"),
        "user.suspend" => match item {
            Some(who) => format!("{actor}님이 {who}님을 정지했어요"),
            None => format!("{actor}님이 누군가를 정지했어요"),
        },
        "user.unsuspend" => match item {
            Some(who) => format!("{actor}님이 {who}님의 정지를 풀었어요"),
            None => format!("{actor}님이 정지를 풀었어요"),
        },
        "blacklist.add" => match item {
            Some(pattern) => format!("{actor}님이 **{pattern}** 을 차단 목록에 넣었어요"),
            None => format!("{actor}님이 차단 목록에 규칙을 넣었어요"),
        },
        "blacklist.remove" => match item {
            Some(pattern) => format!("{actor}님이 차단 목록에서 **{pattern}** 을 뺐어요"),
            None => format!("{actor}님이 차단 목록에서 규칙을 뺐어요"),
        },
        _ if action.starts_with("settings.") => settings_change_text(
            actor,
            action.trim_start_matches("settings."),
            before,
            after,
        ),
        other => match item {
            Some(title) => format!("{actor}님이 {other} 을 했어요 (**{title}**)"),
            None => format!("{actor}님이 {other} 을 했어요"),
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    pub display_name: String,
    pub action: String,
    /// 분류. 저장 시 `audit_kind_for`로 정해져 컬럼에 박혀 있다.
    #[serde(default)]
    pub kind: AuditKind,
    /// 서버가 완성한 사람 문장 (§13.5).
    #[serde(default)]
    pub text: String,
    pub target: Option<String>,
    pub before_value: Option<String>,
    pub after_value: Option<String>,
    pub success: bool,
    pub failure_reason: Option<String>,
    pub created_utc: String,
    /// 합쳐진 줄이면 2 이상 (§13.3). 1이면 평범한 한 줄이다.
    #[serde(default)]
    pub merged_count: u32,
    /// 합쳐진 줄을 펼쳤을 때 보여줄 항목들. 숫자만 보여주면 "뭘 넣은 거지?"가 남는다.
    #[serde(default)]
    pub merged_items: Vec<String>,
}

/// 유저 UI 로 나가는 투영 (§13.2·§13.5). **전후값 JSON과 실패 사유는 아예 싣지 않는다** —
/// 사람이 볼 화면에 기계용 덩어리가 나가면 그 탭은 못 읽는 화면이 된다.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFeedItem {
    pub id: i64,
    pub kind: AuditKind,
    pub actor_id: u64,
    pub actor_name: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_title: Option<String>,
    pub created_utc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_count: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merged_items: Vec<String>,
}

impl AuditEntry {
    /// 사람 피드용 투영. 관리 콘솔은 `AuditEntry` 자체를 쓴다.
    pub fn feed_item(&self) -> AuditFeedItem {
        AuditFeedItem {
            id: self.id,
            kind: self.kind,
            actor_id: self.user_id,
            actor_name: self.display_name.clone(),
            text: self.text.clone(),
            track_title: self.target.clone(),
            created_utc: self.created_utc.clone(),
            merged_count: (self.merged_count > 1).then_some(self.merged_count),
            merged_items: self.merged_items.clone(),
        }
    }

    /// 자동 재생이 넣은 곡은 사람 피드에 안 남긴다 (§13.3).
    /// 사람이 한 일이 아니고, 계속 쌓이면 피드가 자동재생 로그가 된다.
    pub fn is_human_visible(&self) -> bool {
        self.user_id != 0 && self.success
    }
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
    /// **재시작을 넘겨야 하는 값.** 복구할 때 새로 만들면 브라우저가 들고 있는
    /// 옛 토큰과 어긋나서, 로그인은 유지되는데 누르는 것마다 CSRF 실패가 난다.
    pub csrf_token: Option<String>,
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
    /// 곡 넘기기 권한 (§10.5). 재생/일시정지와 성격이 달라 `playback_rule`에서 갈라냈다.
    /// **기본이 `GuildMember`** — 리모컨만 보는 사람도 곡을 넘길 수 있어야 한다.
    #[serde(default = "default_open_rule")]
    pub skip_rule: PermissionRule,
    /// 자동 재생 권한 (§8.3). 추천 방식 전환·기준 곡 등록/삭제/정렬·최근 N곡 수·장르 선택·
    /// `📻 이 곡 말고`·자동 재생 On/Off 를 전부 관장한다. **기본이 `GuildMember`** 다.
    /// (v2 의 `autoplaySeedRule` 이 이 이름으로 바뀌었고 alias 로 옛 값을 그대로 읽는다.)
    #[serde(default = "default_open_rule", alias = "autoplaySeedRule")]
    pub autoplay_rule: PermissionRule,
    /// 한 번에 담기 권한 (§15.4). 재생목록 전체 담기 + 차트 전체 담기를 함께 관장한다.
    #[serde(default = "default_open_rule")]
    pub bulk_enqueue_rule: PermissionRule,
    /// 레거시 통짜 지정 역할. **직접 읽지 말고** `roles_for`/`manager_roles`를 쓴다.
    /// 새 값이 없을 때만 폴백으로 쓰이고, 저장 시점에 분리된 값으로 대체된다.
    pub configured_role_ids: Vec<u64>,
    /// 권한 키별 지정 역할. 키는 `PERMISSION_KEYS` 8개.
    /// 키가 아예 없으면 레거시 값으로 폴백하고, 빈 배열이면 "일부러 비웠다"로 읽는다.
    #[serde(default)]
    pub rule_role_ids: BTreeMap<String, Vec<u64>>,
    /// 관리자 지정 역할. 권한용 역할과 완전히 분리돼 있다.
    #[serde(default)]
    pub manager_role_ids: Vec<u64>,
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

    // ───────── 투표 점수 (§10.1) ─────────
    /// 좋아요 한 표의 점수. 허용 범위 −10~10.
    #[serde(default = "default_like_points")]
    pub like_points: i32,
    #[serde(default = "default_dislike_points")]
    pub dislike_points: i32,
    #[serde(default = "default_super_like_points")]
    pub super_like_points: i32,
    /// 곡 하나가 지날 때마다 붙는 대기 가점.
    #[serde(default = "default_wait_points")]
    pub wait_points: i32,

    // ───────── 붐따 (§10.3) ─────────
    /// 꺼져 있으면(기본) 싫어요는 점수에만 영향을 준다. 곡이 사라지지 않는다.
    #[serde(default)]
    pub boomtta_enabled: bool,
    /// 이 수만큼 싫어요가 모이면 실행한다. `0`이면 무제한(=절대 안 걸림).
    #[serde(default = "default_boomtta_threshold")]
    pub boomtta_threshold: u32,
    #[serde(default)]
    pub boomtta_action: BoomttaAction,

    // ───────── 투표 스킵 (§10.5) ─────────
    #[serde(default)]
    pub vote_skip_enabled: bool,
    #[serde(default)]
    pub vote_skip_basis: VoteSkipBasis,
    /// 필요 비율(%). 백분율이라 무제한이 없다(§23.1 예외) — 10~100.
    #[serde(default = "default_vote_skip_ratio")]
    pub vote_skip_ratio: u32,
    /// 최소 필요 인원. `0`이면 비율만 본다.
    #[serde(default = "default_vote_skip_min")]
    pub vote_skip_min: u32,

    // ───────── 슈퍼 좋아요 제한 (§10.6) ─────────
    /// 연타 방지 쿨타임(초). `0`이면 없음(기본).
    #[serde(default)]
    pub super_like_cooldown_sec: u32,
    /// 하루(UTC 자정 기준) 사용 횟수. `0`이면 무제한(기본).
    #[serde(default)]
    pub super_like_daily_limit: u32,

    // ───────── 자동 재생 (§8.4 · §8.5) ─────────
    #[serde(default)]
    pub autoplay_mode: AutoplayMode,
    /// `recent` 모드가 참고할 최근 곡 수 (1~20).
    #[serde(default = "default_autoplay_recent")]
    pub autoplay_recent_count: u32,
    /// `genre` 모드가 쓸 장르 차트 키.
    #[serde(default)]
    pub autoplay_genres: Vec<String>,
    #[serde(default)]
    pub autoplay_policy: AutoplayPolicy,
    /// 최근 이 곡 수 안에 나온 아티스트는 후보에서 뺀다. `0`이면 끔.
    #[serde(default = "default_artist_cooldown")]
    pub autoplay_artist_cooldown: u32,
    /// 최근 재생 이력의 회피가 완전히 풀리는 시간(시간). `0`이면 감쇠 없이 그냥 제외한다.
    #[serde(default = "default_recent_decay_hours")]
    pub autoplay_recent_decay_hours: u32,
    /// 기준 곡 상한. `0`이면 무제한(§23.1).
    #[serde(default = "default_autoplay_seed_max")]
    pub autoplay_seed_max: u32,

    // ───────── 한 번에 담기 · 차트 (§15 · §18.2) ─────────
    /// 한 번의 클릭으로 들어올 수 있는 최대 곡 수. `0`이면 무제한.
    #[serde(default = "default_bulk_enqueue_limit")]
    pub bulk_enqueue_limit: u32,
    /// 사랑받은 곡 차트에서 슈퍼 좋아요를 몇 배로 칠지 (0~5). `0`이면 슈퍼를 무시한다.
    #[serde(default = "default_chart_super_weight")]
    pub chart_super_weight: u32,
    /// 차트 하나에서 가져올 곡 수 (§15). 10~100. 검색형 차트는 `ytsearchN:` 의 N 을 이 값으로 갈아 끼운다.
    /// 재생목록형은 앞에서부터 이 개수만 쓴다.
    #[serde(default = "default_chart_limit")]
    pub chart_limit: u32,
    /// 곡이 바뀔 때 Discord 채널에 새 카드를 보낼지 갱신할지 (§25).
    /// 기본은 갱신 — 여러 서버가 쓰면 새 글 방식은 채널을 금방 도배한다.
    #[serde(default)]
    pub now_playing_mode: NowPlayingMode,
    /// 음성 채널에 봇만 남았을 때 무엇을 할지 (§27). **기본은 아무것도 안 함.**
    /// 남이 듣고 있는데 갑자기 나가면 그게 더 놀랍다 — 켜는 건 서버 주인이 정한다.
    #[serde(default)]
    pub empty_voice_policy: EmptyVoiceChannelPolicy,
    /// 비었다고 판단하고 나서 기다릴 시간(초). 5~3600.
    /// 잠깐 나갔다 오는 사람 때문에 바로 끊기면 안 된다.
    #[serde(default = "default_empty_voice_delay")]
    pub empty_voice_delay_seconds: u32,
    /// 스킵·되감기 때 **몇 ms 뒤를 시작 시각으로 잡을지** (§31). 0~5000.
    ///
    /// 0이면 "지금부터"인데, 그 말이 사람마다 다른 시각에 도착해서 각자 다른 지점에서
    /// 시작한다. 조금 미래로 잡아 두면 다 같이 그 시각을 기다렸다 출발한다.
    /// 회선이 느린 사람이 많으면 늘린다.
    #[serde(default = "default_skip_lead_ms")]
    pub skip_lead_ms: u32,
    /// 곡이 끝나기 **몇 ms 전부터 진행바를 못 움직이게** 할지 (§31). 0~10000.
    ///
    /// 끝나기 직전에 되감으면 그 이동이 반영되기 전에 다음 곡으로 넘어가서,
    /// 웹만 엉뚱한 지점에 남고 봇은 다음 곡을 튼다. 그 구간을 아예 막는다.
    #[serde(default = "default_seek_lockout_ms")]
    pub seek_lockout_ms: u32,
    /// 서버 전체에 적용하는 웹 재생 보정(ms) (§31). -5000~5000.
    ///
    /// 개인 보정(`webOffset`)과 **더해진다.** 디스코드 송출 경로가 브라우저보다 늘 일정하게
    /// 늦거나 빠르면 여기서 한 번에 맞추고, 사람마다 남는 차이만 개인 설정으로 다듬는다.
    #[serde(default)]
    pub web_sync_offset_ms: i32,
    /// 로그인 없이 **지금 무슨 곡인지**만 볼 수 있게 할지 (§29).
    ///
    /// 켜져 있어도 나가는 것은 곡 제목·가수·진행 상태뿐이다. 신청한 사람 이름,
    /// 채팅, 멤버 목록은 **절대 안 나간다** — 그건 서버 안 사람들 정보다.
    /// 서버마다 끌 수 있다. 활동을 밖에 안 보이고 싶은 서버가 있을 수 있다.
    #[serde(default = "default_true")]
    pub public_now_playing: bool,
}

fn default_empty_voice_delay() -> u32 {
    300
}

fn default_skip_lead_ms() -> u32 {
    1000
}

fn default_seek_lockout_ms() -> u32 {
    3000
}

/// 빈 채널 규칙을 **누가 정했는지**까지 담은 결과 (§27).
///
/// 봇 주인이 운영 패널에서 강제로 걸면 서버 주인은 못 바꾼다. 그때 화면이
/// "왜 잠겼는지" 를 말해야 해서, 값만이 아니라 출처도 같이 들고 다닌다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyVoiceRule {
    pub policy: EmptyVoiceChannelPolicy,
    pub delay_seconds: u32,
    /// 봇 주인이 강제로 건 값인가. true 면 서버 주인은 못 바꾼다.
    pub forced: bool,
}

impl EmptyVoiceRule {
    /// 서버 주인이 이 값을 바꿀 수 있는지.
    pub fn editable(self) -> bool {
        !self.forced
    }

    /// 못 바꿀 때 보여 줄 이유. 잠긴 사실만 보이면 고장으로 읽힌다.
    pub fn lock_reason(self) -> Option<&'static str> {
        self.forced
            .then_some("봇 주인이 모든 서버에 같은 규칙을 걸어 뒀어요. 서버에서는 바꿀 수 없어요.")
    }
}

fn default_chat_retention_days() -> u32 {
    30
}

/// 새 권한들의 기본값. 사용자가 "일반사용자도 할수있고"라고 명시했다(§8.3·§10.5·§15.4).
fn default_open_rule() -> PermissionRule {
    PermissionRule::GuildMember
}

fn default_boomtta_threshold() -> u32 {
    3
}
fn default_vote_skip_ratio() -> u32 {
    50
}
fn default_vote_skip_min() -> u32 {
    2
}
fn default_autoplay_recent() -> u32 {
    5
}
fn default_artist_cooldown() -> u32 {
    3
}
fn default_recent_decay_hours() -> u32 {
    24
}
fn default_autoplay_seed_max() -> u32 {
    MAX_AUTOPLAY_SEEDS as u32
}
fn default_bulk_enqueue_limit() -> u32 {
    200
}
/// 차트 곡 수 기본값. 50 이면 한 화면에 담기고 yt-dlp 도 빠르다.
fn default_chart_limit() -> u32 {
    50
}

/// `ytsearch50:...` 의 숫자를 설정값으로 갈아 끼운다.
///
/// 검색형 차트는 URL 에 개수가 박혀 있어서, 설정을 바꿔도 주소를 안 고치면 그대로 50 이다.
/// 재생목록·internal 주소는 건드리지 않는다.
pub fn chart_url_with_limit(url: &str, limit: u32) -> String {
    let limit = limit.clamp(1, 100);
    for prefix in ["ytsearch", "scsearch"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            // `ytsearch50:검색어` 에서 숫자와 콜론을 찾는다.
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let tail = &rest[digits.len()..];
            if tail.starts_with(':') {
                return format!("{prefix}{limit}{tail}");
            }
        }
    }
    url.to_string()
}

fn default_chart_super_weight() -> u32 {
    2
}

fn default_true() -> bool {
    true
}

impl PermissionRule {
    /// 판정하려면 그 사람의 **역할 목록을 알아야 하는** 규칙인지.
    ///
    /// Discord 조회가 실패해 역할을 모를 때, 이 규칙이면 "권한 없음"이라고 말하면 안 된다.
    /// 실제로 재시작 직후 429 가 겹쳐서 지정 역할 권한자가 권한 없음을 봤다.
    /// 나머지 규칙은 역할과 무관하게 판정되므로 그대로 거절해도 정확하다.
    pub fn needs_roles(self) -> bool {
        matches!(self, Self::ConfiguredRole | Self::Administrator)
    }
}

impl RemoteGuildSettings {
    /// 이 권한 키의 지정 역할. 비어 있으면 레거시 `configured_role_ids`로 폴백한다.
    ///
    /// "비어 있으면"은 **키 자체가 없을 때**를 말한다. 빈 배열이 저장돼 있으면
    /// 관리자가 일부러 비운 것이므로 폴백하지 않는다 — 안 그러면 지운 역할이 되살아난다.
    pub fn roles_for(&self, key: &str) -> &[u64] {
        if let Some(ids) = self.rule_role_ids.get(key) {
            return ids;
        }
        // 키가 개명됐으면 옛 이름에 저장된 값을 먼저 본다 (예: autoplay ← autoplaySeed).
        for (new_key, legacy_key) in PERMISSION_KEY_ALIASES {
            if new_key == key {
                if let Some(ids) = self.rule_role_ids.get(legacy_key) {
                    return ids;
                }
            }
        }
        &self.configured_role_ids
    }

    /// 관리자 지정 역할. 비어 있으면 레거시 폴백.
    /// 관리자 판정은 권한 규칙과 별개라 `rule_role_ids`를 보지 않는다.
    pub fn manager_roles(&self) -> &[u64] {
        if self.manager_role_ids.is_empty() {
            &self.configured_role_ids
        } else {
            &self.manager_role_ids
        }
    }

    /// 권한 키 → 규칙. 모르는 키면 `None`이라 호출부가 조용히 통과시키지 않는다.
    pub fn rule_for(&self, key: &str) -> Option<PermissionRule> {
        Some(match key {
            "search" => self.search_rule,
            "vote" => self.vote_rule,
            "chat" => self.chat_rule,
            "playback" => self.playback_rule,
            "seek" => self.seek_rule,
            "volume" => self.volume_rule,
            "queueEdit" => self.queue_edit_rule,
            "skip" => self.skip_rule,
            // 옛 이름도 계속 받아 준다 — 저장된 설정 JSON 과 관리 콘솔의 과거 요청이 살아 있다.
            "autoplay" | "autoplaySeed" => self.autoplay_rule,
            "bulkEnqueue" => self.bulk_enqueue_rule,
            _ => return None,
        })
    }

    /// 권한 키의 설명 문구. 관리 콘솔과 "왜 안 되는지"(§23.3) 툴팁이 같은 문장을 쓴다.
    pub fn permission_description(key: &str) -> &'static str {
        match key {
            "search" => "곡을 찾아 대기열에 담는 동작이에요.",
            "vote" => "곡에 좋아요·슈퍼 좋아요·싫어요를 누르는 동작이에요.",
            "chat" => "리모컨 채팅에 글을 쓰는 동작이에요.",
            "playback" => "재생·일시정지처럼 지금 나오는 곡을 조작하는 동작이에요.",
            "skip" => "지금 나오는 곡을 다음으로 넘기는 동작이에요.",
            "seek" => "재생 위치를 앞뒤로 옮기는 동작이에요.",
            "volume" => "모두에게 들리는 서버 볼륨을 바꾸는 동작이에요.",
            "queueEdit" => "대기열 순서를 바꾸거나 곡을 빼는 동작이에요.",
            "autoplay" | "autoplaySeed" => {
                "자동 재생이 무엇을 기준으로 곡을 고를지 정하는 동작이에요."
            }
            "bulkEnqueue" => "재생목록이나 차트를 한 번에 전부 담는 동작이에요.",
            _ => "이 동작을 누가 할 수 있는지 정해요.",
        }
    }

    /// 이 서버의 투표 점수표.
    pub fn vote_points(&self) -> VotePoints {
        VotePoints::from_settings(self)
    }

    /// 기준 곡 상한. `0`이면 무제한이라 `None`이다 (§23.1).
    /// 차트에서 가져올 곡 수. 10~100 으로 조인다.
    pub fn chart_limit(&self) -> u32 {
        self.chart_limit.clamp(10, 100)
    }

    pub fn seed_limit(&self) -> Option<u32> {
        as_limit_u32(self.autoplay_seed_max)
    }

    /// `recent` 모드가 참고할 최근 곡 수 (§8.2). `0`이면 무제한 — 최근 목록 전부를 본다(§23.1).
    ///
    /// 유저 UI 가 이미 `0을 넣으면 최근에 튼 곡 전부를 참고해요` 라고 안내하므로,
    /// 저장·엔진 어느 쪽에서도 `0`을 `1`로 둔갑시키지 않는다.
    pub fn recent_count_limit(&self) -> Option<u32> {
        as_limit_u32(self.autoplay_recent_count)
    }

    /// **`0 = 무제한` 규약을 여기서 강제한다** (§23.1). 저장 직전에 한 번 부르면
    /// 어떤 라우트를 거쳐 들어와도 서버가 실제로 그 규약대로 동작한다.
    /// `.max(1)` 같은 클램프가 남아 있으면 `0`이 `1`이 돼 "무제한"이 "가장 빡빡함"이 된다.
    pub fn sanitize(&mut self) {
        // 볼륨은 §23.1 예외 — 0~200 범위가 있어야 의미가 있다.
        self.min_volume = self.min_volume.clamp(0, 200);
        self.max_volume = self.max_volume.clamp(0, 200);
        if self.max_volume < self.min_volume {
            self.max_volume = self.min_volume;
        }
        self.default_volume = self.default_volume.clamp(self.min_volume, self.max_volume);

        // 0 = 무제한. 음수만 0으로 올리고, 위쪽은 v3 §18.1 의 새 상한을 쓴다.
        self.max_queue_per_user = self.max_queue_per_user.clamp(0, 1_000);
        self.max_queue_per_guild = self.max_queue_per_guild.clamp(0, 10_000);
        self.max_track_seconds = self.max_track_seconds.max(0);
        self.audit_retention_days = self.audit_retention_days.clamp(0, 3650);
        self.chat_retention_days = self.chat_retention_days.min(3650);

        let points = self.vote_points();
        self.like_points = points.like;
        self.dislike_points = points.dislike;
        self.super_like_points = points.super_like;
        self.wait_points = points.wait;

        self.boomtta_threshold = self.boomtta_threshold.min(1_000);
        // 비율은 백분율이라 무제한이 말이 안 된다(§23.1 예외).
        self.vote_skip_ratio = self.vote_skip_ratio.clamp(10, 100);
        self.vote_skip_min = self.vote_skip_min.min(20);
        self.super_like_cooldown_sec = self.super_like_cooldown_sec.min(3_600);
        self.super_like_daily_limit = self.super_like_daily_limit.min(100);

        // 0 = 무제한(최근 목록 전부). 여기서 `.max(1)`/`clamp(1, ..)` 을 하면
        // 유저 UI 의 `0을 넣으면 최근에 튼 곡 전부를 참고해요` 가 거짓말이 된다.
        self.autoplay_recent_count = self.autoplay_recent_count.min(20);
        self.autoplay_artist_cooldown = self.autoplay_artist_cooldown.min(20);
        self.autoplay_recent_decay_hours = self.autoplay_recent_decay_hours.min(168);
        self.autoplay_seed_max = self.autoplay_seed_max.min(100);
        self.autoplay_genres.retain(|genre| !genre.trim().is_empty());
        self.autoplay_genres.truncate(20);

        self.bulk_enqueue_limit = self.bulk_enqueue_limit.min(10_000);
        self.chart_super_weight = self.chart_super_weight.min(5);
    }

    /// 레거시 값을 8개 키에 펼쳐 넣는다. 관리 콘솔이 처음 저장할 때 한 번 부르면
    /// 그 뒤로는 키마다 따로 관리된다(읽기 폴백에 계속 기대지 않게).
    pub fn expand_legacy_roles(&mut self) {
        if !self.rule_role_ids.is_empty() {
            return;
        }
        for key in PERMISSION_KEYS {
            self.rule_role_ids
                .insert(key.to_string(), self.configured_role_ids.clone());
        }
        if self.manager_role_ids.is_empty() {
            self.manager_role_ids = self.configured_role_ids.clone();
        }
    }
}

impl Default for RemoteGuildSettings {
    fn default() -> Self {
        Self {
            guild_id: 0,
            chart_limit: default_chart_limit(),
            now_playing_mode: NowPlayingMode::default(),
            empty_voice_policy: EmptyVoiceChannelPolicy::default(),
            empty_voice_delay_seconds: default_empty_voice_delay(),
            skip_lead_ms: default_skip_lead_ms(),
            seek_lockout_ms: default_seek_lockout_ms(),
            web_sync_offset_ms: 0,
            public_now_playing: true,
            min_volume: 0,
            max_volume: 200,
            default_volume: 100,
            search_rule: PermissionRule::GuildMember,
            vote_rule: PermissionRule::GuildMember,
            chat_rule: PermissionRule::GuildMember,
            playback_rule: PermissionRule::SameVoiceChannel,
            seek_rule: PermissionRule::GuildMember,
            volume_rule: PermissionRule::SameVoiceChannel,
            queue_edit_rule: PermissionRule::SameVoiceChannel,
            skip_rule: default_open_rule(),
            autoplay_rule: default_open_rule(),
            bulk_enqueue_rule: default_open_rule(),
            configured_role_ids: Vec::new(),
            rule_role_ids: BTreeMap::new(),
            manager_role_ids: Vec::new(),
            max_queue_per_user: 5,
            max_queue_per_guild: 100,
            max_track_seconds: 14_400,
            chat_enabled: true,
            audit_retention_days: 14,
            sort_mode: QueueSortMode::Score,
            chat_retention_days: default_chat_retention_days(),
            suggestion_enabled: true,
            visualizer_enabled: true,
            like_points: default_like_points(),
            dislike_points: default_dislike_points(),
            super_like_points: default_super_like_points(),
            wait_points: default_wait_points(),
            boomtta_enabled: false,
            boomtta_threshold: default_boomtta_threshold(),
            boomtta_action: BoomttaAction::Bottom,
            vote_skip_enabled: false,
            vote_skip_basis: VoteSkipBasis::Listeners,
            vote_skip_ratio: default_vote_skip_ratio(),
            vote_skip_min: default_vote_skip_min(),
            super_like_cooldown_sec: 0,
            super_like_daily_limit: 0,
            autoplay_mode: AutoplayMode::Recent,
            autoplay_recent_count: default_autoplay_recent(),
            autoplay_genres: Vec::new(),
            autoplay_policy: AutoplayPolicy::Balanced,
            autoplay_artist_cooldown: default_artist_cooldown(),
            autoplay_recent_decay_hours: default_recent_decay_hours(),
            autoplay_seed_max: default_autoplay_seed_max(),
            bulk_enqueue_limit: default_bulk_enqueue_limit(),
            chart_super_weight: default_chart_super_weight(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 레거시 설정(통짜 지정 역할만 있는 JSON)이 조용히 동작을 바꾸면 안 된다:
    /// 8개 권한 키 전부에서 기존 역할이 그대로 나와야 한다.
    #[test]
    fn legacy_configured_roles_fall_back_for_every_permission_key() {
        let json = r#"{"configuredRoleIds":[123,456],"searchRule":"guildMember"}"#;
        let settings: RemoteGuildSettings = serde_json::from_str(json).unwrap();
        assert!(settings.rule_role_ids.is_empty());
        for key in PERMISSION_KEYS {
            assert_eq!(settings.roles_for(key), &[123, 456], "키 {key} 폴백 실패");
        }
        // 관리자 지정 역할도 같은 방식으로 폴백한다.
        assert_eq!(settings.manager_roles(), &[123, 456]);
        // v3 의 새 권한 3종은 기본이 "모든 사람"이다.
        assert_eq!(settings.skip_rule, PermissionRule::GuildMember);
        assert_eq!(settings.autoplay_rule, PermissionRule::GuildMember);
        assert_eq!(settings.bulk_enqueue_rule, PermissionRule::GuildMember);
    }

    /// v2 를 쓰던 서버의 `autoplaySeedRule` 과 그 지정 역할이 개명 때문에 사라지면 안 된다.
    #[test]
    fn renamed_autoplay_key_still_reads_the_old_setting() {
        let json = r#"{"autoplaySeedRule":"administrator",
                       "ruleRoleIds":{"autoplaySeed":[42]}}"#;
        let settings: RemoteGuildSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.autoplay_rule, PermissionRule::Administrator);
        assert_eq!(settings.roles_for("autoplay"), &[42]);
        assert_eq!(
            settings.rule_for("autoplaySeed"),
            Some(PermissionRule::Administrator)
        );
        // 새 키로 저장된 값이 있으면 그쪽이 이긴다.
        let mut settings = settings;
        settings.rule_role_ids.insert("autoplay".into(), vec![7]);
        assert_eq!(settings.roles_for("autoplay"), &[7]);
    }

    /// 관리자가 일부러 비운 키는 레거시 값으로 되살아나면 안 된다.
    #[test]
    fn explicitly_empty_rule_roles_stay_empty() {
        let mut settings = RemoteGuildSettings {
            configured_role_ids: vec![123],
            ..Default::default()
        };
        settings
            .rule_role_ids
            .insert("volume".into(), vec![456, 789]);
        settings.rule_role_ids.insert("search".into(), Vec::new());

        assert_eq!(settings.roles_for("volume"), &[456, 789]);
        assert!(settings.roles_for("search").is_empty());
        // 저장된 적 없는 키만 레거시로 폴백한다.
        assert_eq!(settings.roles_for("queueEdit"), &[123]);
    }

    /// 검색 권한 역할을 준 사람이 관리자가 돼버리던 문제 — 이제 완전히 분리된다.
    #[test]
    fn manager_roles_are_independent_from_rule_roles() {
        let mut settings = RemoteGuildSettings {
            manager_role_ids: vec![999],
            configured_role_ids: vec![123],
            ..Default::default()
        };
        settings.rule_role_ids.insert("search".into(), vec![123]);
        assert_eq!(settings.manager_roles(), &[999]);
        assert_eq!(settings.roles_for("search"), &[123]);
    }

    #[test]
    fn rule_for_covers_all_permission_keys_and_rejects_unknown() {
        let settings = RemoteGuildSettings::default();
        for key in PERMISSION_KEYS {
            assert!(settings.rule_for(key).is_some(), "키 {key} 규칙 누락");
        }
        assert!(settings.rule_for("nope").is_none());
    }

    #[test]
    fn expanding_legacy_roles_pins_the_current_behaviour() {
        let mut settings = RemoteGuildSettings {
            configured_role_ids: vec![7],
            ..Default::default()
        };
        settings.expand_legacy_roles();
        assert_eq!(settings.rule_role_ids.len(), PERMISSION_KEYS.len());
        assert_eq!(settings.manager_role_ids, vec![7]);

        // 이미 분리돼 있으면 덮어쓰지 않는다.
        let mut kept = RemoteGuildSettings {
            configured_role_ids: vec![7],
            ..Default::default()
        };
        kept.rule_role_ids.insert("chat".into(), vec![1]);
        kept.expand_legacy_roles();
        assert_eq!(kept.rule_role_ids.len(), 1);
    }

    #[test]
    fn seed_add_outcome_messages_are_specific() {
        assert!(SeedAddOutcome::Added.is_added());
        assert_eq!(
            SeedAddOutcome::LimitReached(10).message(),
            "시드곡은 10곡까지 넣을 수 있어요."
        );
        assert!(!SeedAddOutcome::Duplicate.is_added());
    }

    fn score_of(wait: i32, likes: i32, supers: i32, dislikes: i32) -> QueueScore {
        QueueScore {
            wait_score: wait,
            like_count: likes,
            super_like_count: supers,
            dislike_count: dislikes,
            ..Default::default()
        }
    }

    /// `*2` 하드코딩이 사라지고 설정값만 쓰는지 (§10.1).
    #[test]
    fn total_score_uses_the_configured_points_only() {
        let score = score_of(2, 3, 1, 0);
        // 기본 점수표는 지금 동작 그대로: 대기2 + 👍3 + ⭐1×2 = 7
        assert_eq!(score.total_score(&VotePoints::default()), 7);

        // 좋아요를 2점으로 올리면 화면도 서버도 같이 움직여야 한다.
        let doubled = VotePoints {
            like: 2,
            ..VotePoints::default()
        };
        assert_eq!(score.total_score(&doubled), 2 + 6 + 2);

        // 싫어요는 음수 점수로 들어간다.
        let disliked = score_of(0, 0, 0, 3);
        assert_eq!(disliked.total_score(&VotePoints::default()), -3);

        // 슈퍼를 0점으로 두면 아예 안 세진다 — `*2` 가 남아 있으면 여기서 터진다.
        let ignored = VotePoints {
            super_like: 0,
            ..VotePoints::default()
        };
        assert_eq!(score_of(0, 0, 5, 0).total_score(&ignored), 0);
    }

    #[test]
    fn vote_points_are_clamped_to_the_allowed_range() {
        let wild = VotePoints {
            like: 999,
            dislike: -999,
            super_like: 11,
            wait: -11,
        }
        .clamped();
        assert_eq!(wild.like, VOTE_POINT_MAX);
        assert_eq!(wild.dislike, VOTE_POINT_MIN);
        assert_eq!(wild.super_like, 10);
        assert_eq!(wild.wait, -10);
    }

    /// 화면의 계산식은 설정값을 반영해야 한다. 안 그러면 화면이 거짓말을 한다(§10.4).
    #[test]
    fn formula_reflects_the_settings() {
        let score = score_of(2, 3, 1, 0);
        assert_eq!(score.formula(&VotePoints::default()), "👍3 + ⭐1×2 + 대기2 = 7");
        let none = score_of(0, 0, 0, 0);
        assert!(none.formula(&VotePoints::default()).starts_with("아직 점수가 없어요"));
    }

    /// 붐따는 기본으로 꺼져 있고, 기준이 0(무제한)이면 절대 안 걸린다 (§10.3 · §23.1).
    #[test]
    fn boomtta_stays_off_until_it_is_turned_on() {
        let mut settings = RemoteGuildSettings::default();
        let score = score_of(0, 0, 0, 5);
        assert!(!score.boomtta_triggered(&settings), "기본은 꺼져 있어야 한다");

        settings.boomtta_enabled = true;
        assert!(score.boomtta_triggered(&settings));
        assert!(!score_of(0, 0, 0, 2).boomtta_triggered(&settings));

        settings.boomtta_threshold = 0;
        assert!(!score.boomtta_triggered(&settings), "0은 무제한이라 안 걸린다");
    }

    /// 모수가 1명이면 그 사람 혼자 눌러도 넘어간다 — 혼자 듣는데 투표를 시키면 괴롭힘이다.
    #[test]
    fn vote_skip_threshold_never_exceeds_the_population() {
        assert_eq!(VoteSkipBasis::votes_needed(0, 50, 2), 0);
        assert_eq!(VoteSkipBasis::votes_needed(1, 50, 2), 1);
        assert_eq!(VoteSkipBasis::votes_needed(3, 50, 2), 2);
        assert_eq!(VoteSkipBasis::votes_needed(4, 50, 2), 2);
        assert_eq!(VoteSkipBasis::votes_needed(5, 50, 2), 3);
        assert_eq!(VoteSkipBasis::votes_needed(10, 100, 2), 10);
        // 최소 인원이 모수보다 크면 모수가 이긴다.
        assert_eq!(VoteSkipBasis::votes_needed(2, 10, 20), 2);
    }

    /// `0 = 무제한` 규약이 저장 직전에 실제로 강제되는지 (§23.1).
    #[test]
    fn sanitize_keeps_zero_as_unlimited_and_clamps_the_rest() {
        let mut settings = RemoteGuildSettings {
            max_queue_per_user: 0,
            max_queue_per_guild: 99_999,
            audit_retention_days: 0,
            super_like_daily_limit: 0,
            vote_skip_ratio: 3,
            like_points: 42,
            autoplay_recent_count: 0,
            autoplay_seed_max: 0,
            default_volume: 500,
            ..Default::default()
        };
        settings.sanitize();

        // 0 은 살아남는다 — .max(1) 이 남아 있으면 여기서 터진다.
        assert_eq!(settings.max_queue_per_user, 0);
        assert_eq!(settings.audit_retention_days, 0);
        assert_eq!(settings.super_like_daily_limit, 0);
        assert_eq!(settings.autoplay_seed_max, 0);
        assert!(settings.seed_limit().is_none());
        // 최근 N곡도 예외가 아니다 — 0 이 1 로 둔갑하면 "무제한"이 "가장 빡빡함"이 된다.
        assert_eq!(settings.autoplay_recent_count, 0);
        assert!(settings.recent_count_limit().is_none());
        assert!(as_limit(settings.max_queue_per_user).is_none());
        assert_eq!(as_limit(5), Some(5));

        // 나머지는 범위 안으로.
        assert_eq!(settings.max_queue_per_guild, 10_000);
        assert_eq!(settings.vote_skip_ratio, 10);
        assert_eq!(settings.like_points, VOTE_POINT_MAX);
        assert_eq!(settings.default_volume, settings.max_volume);

        // 위쪽 상한은 그대로 산다.
        let mut too_many = RemoteGuildSettings {
            autoplay_recent_count: 999,
            ..Default::default()
        };
        too_many.sanitize();
        assert_eq!(too_many.autoplay_recent_count, 20);
        assert_eq!(too_many.recent_count_limit(), Some(20));
    }

    #[test]
    fn audit_kinds_match_the_action_table() {
        assert_eq!(audit_kind_for("queue.add"), AuditKind::Song);
        assert_eq!(audit_kind_for("queue.boomtta"), AuditKind::Song);
        assert_eq!(audit_kind_for("playlist.enqueue"), AuditKind::Song);
        assert_eq!(audit_kind_for("chart.enqueue"), AuditKind::Song);
        assert_eq!(audit_kind_for("vote.superlike"), AuditKind::Vote);
        assert_eq!(audit_kind_for("playback.skip"), AuditKind::Playback);
        assert_eq!(audit_kind_for("autoplay.toggle"), AuditKind::Playback);
        assert_eq!(audit_kind_for("autoplay.seed.add"), AuditKind::Playlist);
        assert_eq!(audit_kind_for("playlist.create"), AuditKind::Playlist);
        assert_eq!(audit_kind_for("chat.delete"), AuditKind::Moderation);
        assert_eq!(audit_kind_for("user.suspend"), AuditKind::Moderation);
        assert_eq!(audit_kind_for("blacklist.add"), AuditKind::Moderation);
        assert_eq!(audit_kind_for("settings.update"), AuditKind::Admin);
        // 기본 필터는 조용해야 로그창이 쓸모 있다 (§13.4).
        assert_eq!(AuditKind::default_filter(), [AuditKind::Song, AuditKind::Playlist]);
    }

    /// 투표·재생은 3일, 나머지는 설정값 그대로. 무제한(0)은 짧은 쪽으로 덮이지 않는다 (§13.6).
    #[test]
    fn vote_and_playback_logs_are_kept_for_three_days() {
        assert_eq!(AuditKind::Vote.retention_days(14), 3);
        assert_eq!(AuditKind::Playback.retention_days(14), 3);
        assert_eq!(AuditKind::Vote.retention_days(2), 2);
        assert_eq!(AuditKind::Song.retention_days(14), 14);
        assert_eq!(AuditKind::Vote.retention_days(0), 0);
    }

    /// 문장은 서버가 완성한다 — 클라이언트가 액션명을 문장으로 바꾸지 않는다 (§13.5).
    #[test]
    fn audit_sentences_are_written_by_the_server_in_haeyo() {
        assert_eq!(
            audit_text("queue.add", "민수", Some("I AM"), None, None, 1),
            "민수님이 **I AM** 을 담았어요"
        );
        assert_eq!(
            audit_text("queue.add", "민수", Some("I AM"), None, None, 7),
            "민수님이 곡 7개를 담았어요"
        );
        assert_eq!(
            audit_text("playlist.enqueue", "민수", Some("밤샘용"), None, None, 50),
            "민수님이 재생목록 **밤샘용** 에서 50곡을 담았어요"
        );
        assert_eq!(
            audit_text("chart.enqueue", "민수", Some("한국 인기곡"), None, None, 100),
            "민수님이 차트 **한국 인기곡** 에서 100곡을 담았어요"
        );
        assert_eq!(
            audit_text("playback.volume", "지훈", None, Some("200"), Some("150"), 1),
            "지훈님이 서버 볼륨을 150으로 바꿨어요"
        );
        assert_eq!(
            audit_text("queue.boomtta", "", Some("Spicy"), None, None, 3),
            "**Spicy** 이 싫어요 3개로 대기열에서 내려갔어요"
        );
        // 모르는 액션도 문장이 되어야 한다.
        assert!(audit_text("something.new", "민수", None, None, None, 1).contains("민수님이"));
        // 곡 제목은 40자에서 자른다.
        let long = "가".repeat(60);
        let text = audit_text("queue.add", "민수", Some(&long), None, None, 1);
        assert!(text.contains('…'));
        assert!(text.chars().count() < long.chars().count() + 20);
    }

    /// `POST /control` 이 남기는 `키:값` 결과와 옛 액션명이 사람 피드에 그대로 새면 안 된다.
    /// (`민수님이 playback.autoplay 을 했어요` · `서버 볼륨을 volume:150으로 바꿨어요`)
    /// 실제로 화면에 나갔던 문장:
    /// `마참 님이 limits 을 {"guildId":497...,"minVolume":0,...} → {...} 로 바꿨어요`
    #[test]
    fn settings_changes_never_dump_json_into_the_feed() {
        let before = r#"{"guildId":100000000000000002,"minVolume":0,"maxVolume":100,
            "maxQueuePerUser":100,"maxQueuePerGuild":100,"sortMode":"score","chatEnabled":true}"#;
        let after = r#"{"guildId":100000000000000002,"minVolume":0,"maxVolume":100,
            "maxQueuePerUser":100,"maxQueuePerGuild":991,"sortMode":"score","chatEnabled":true}"#;
        let text = audit_text("settings.limits", "마참", None, Some(before), Some(after), 0);
        assert!(!text.contains('{'), "JSON 이 새어 나갔다: {text}");
        assert!(!text.contains("guildId"), "내부 필드가 새어 나갔다: {text}");
        assert!(text.contains("서버 대기열 수"), "바뀐 항목 이름이 없다: {text}");
        assert!(text.contains("100") && text.contains("991"), "전후 값이 없다: {text}");
    }

    #[test]
    fn settings_changes_summarize_when_many_moved() {
        let before = r#"{"maxVolume":100,"maxQueuePerUser":5,"chatEnabled":true,"sortMode":"score"}"#;
        let after = r#"{"maxVolume":150,"maxQueuePerUser":10,"chatEnabled":false,"sortMode":"fair"}"#;
        let text = audit_text("settings.limits", "마참", None, Some(before), Some(after), 0);
        assert!(!text.contains('{'), "{text}");
        assert!(text.contains("4개"), "바뀐 개수가 없다: {text}");
    }

    #[test]
    fn settings_values_read_like_korean() {
        // 0 = 무제한 (§23.1), 불리언은 켬/끔, 규칙은 한국어 라벨
        let before = r#"{"maxQueuePerGuild":100,"chatEnabled":true,"searchRule":"guildMember"}"#;
        let after = r#"{"maxQueuePerGuild":0,"chatEnabled":true,"searchRule":"guildMember"}"#;
        let text = audit_text("settings.limits", "마참", None, Some(before), Some(after), 0);
        assert!(text.contains("무제한"), "0 이 무제한으로 안 읽힌다: {text}");

        let rule_before = r#"{"searchRule":"guildMember"}"#;
        let rule_after = r#"{"searchRule":"sameVoiceChannel"}"#;
        let rule_text = audit_text("settings.perms", "마참", None, Some(rule_before), Some(rule_after), 0);
        assert!(rule_text.contains("같은 음성 채널"), "규칙이 코드값 그대로다: {rule_text}");
        assert!(!rule_text.contains("sameVoiceChannel"), "{rule_text}");

        let flag_before = r#"{"chatEnabled":true}"#;
        let flag_after = r#"{"chatEnabled":false}"#;
        let flag_text = audit_text("settings.chat", "마참", None, Some(flag_before), Some(flag_after), 0);
        assert!(flag_text.contains("끔"), "{flag_text}");
    }

    #[test]
    fn playlist_sentences_never_leak_the_id_or_the_action_name() {
        // 화면에 `playlist.addTrack 을 했어요 (1:aespa - Spicy)` 가 그대로 나갔던 자리다.
        for action in [
            "playlist.create",
            "playlist.addTrack",
            "playlist.removeEntry",
            "playlist.delete",
            "playlist.enqueue",
        ] {
            let text = audit_text(action, "민수", Some("12:밤샘용"), None, None, 0);
            assert!(
                !text.contains("playlist."),
                "{action} 문장에 액션명이 남았다: {text}"
            );
            assert!(!text.contains("12:"), "{action} 문장에 id 가 샜다: {text}");
            assert!(
                text.contains("밤샘용"),
                "{action} 문장에 이름이 없다: {text}"
            );
        }
    }

    #[test]
    fn playlist_names_that_contain_a_colon_survive() {
        // `팝:최애` 처럼 이름에 콜론이 있으면 자르면 안 된다. 숫자 접두사일 때만 id 로 본다.
        let text = audit_text("playlist.create", "민수", Some("팝:최애"), None, None, 0);
        assert!(text.contains("팝:최애"), "{text}");
    }

    #[test]
    fn machine_strings_never_reach_the_human_feed() {
        // 값 접두사를 벗긴다.
        assert_eq!(
            audit_text("playback.volume", "지훈", None, None, Some("volume:150"), 1),
            "지훈님이 서버 볼륨을 150으로 바꿨어요"
        );
        // 접두사가 없는 옛 기록도 그대로 읽힌다.
        assert_eq!(
            audit_text("playback.volume", "지훈", None, None, Some("150"), 1),
            "지훈님이 서버 볼륨을 150으로 바꿨어요"
        );

        // 🎲 자동 재생 — 핸들러의 옛 액션명과 §24.3 의 새 이름 둘 다 문장이 된다.
        for action in ["playback.autoplay", "autoplay.toggle"] {
            assert_eq!(
                audit_text(action, "민수", None, None, Some("autoplay:true"), 1),
                "민수님이 자동 재생을 켰어요"
            );
            assert_eq!(
                audit_text(action, "민수", None, None, Some("autoplay:false"), 1),
                "민수님이 자동 재생을 껐어요"
            );
        }
        assert_eq!(
            audit_text("autoplay.toggle", "민수", None, None, Some("on"), 1),
            "민수님이 자동 재생을 켰어요"
        );

        // 🔁 반복 · 셔플.
        assert_eq!(
            audit_text("playback.repeat", "수연", None, None, Some("repeat:track"), 1),
            "수연님이 한 곡 반복을 켰어요"
        );
        assert_eq!(
            audit_text("playback.repeat", "수연", None, None, Some("repeat:off"), 1),
            "수연님이 반복을 껐어요"
        );
        assert_eq!(
            audit_text("playback.shuffle", "수연", None, None, Some("shuffle:true"), 1),
            "수연님이 셔플을 켰어요"
        );
        assert_eq!(
            audit_text("playback.shuffle", "수연", None, None, Some("shuffle:false"), 1),
            "수연님이 셔플을 껐어요"
        );

        // 📌 맨 앞으로 — 핸들러가 쓰는 `queue.force_move` 도 §13.3 문장으로 나간다.
        assert_eq!(
            audit_text("queue.force_move", "민수", Some("I AM"), None, Some("pinned"), 1),
            "민수님이 **I AM** 을 맨 앞으로 올렸어요"
        );
        assert_eq!(
            audit_text("queue.force_move", "민수", Some("I AM"), None, Some("unpinned"), 1),
            "민수님이 **I AM** 을 맨 앞에서 내렸어요"
        );
        assert_eq!(
            audit_text("queue.pin", "민수", Some("I AM"), None, None, 1),
            "민수님이 **I AM** 을 맨 앞으로 올렸어요"
        );

        // 사람 피드에 액션명이 남는 문장이 하나도 없어야 한다.
        for action in [
            "queue.force_move",
            "playback.autoplay",
            "playback.repeat",
            "playback.shuffle",
            "playback.volume",
        ] {
            let text = audit_text(action, "민수", Some("I AM"), None, Some("x"), 1);
            assert!(!text.contains(action), "'{action}' 이 문장에 그대로 남았다: {text}");
        }
    }

    /// 정책은 시드를 갈아탈 때마다 한 단계씩 느슨해진다 (§8.5-4).
    #[test]
    fn policies_loosen_and_never_tighten() {
        assert_eq!(AutoplayPolicy::Similar.loosened(), AutoplayPolicy::Balanced);
        assert_eq!(AutoplayPolicy::Balanced.loosened(), AutoplayPolicy::Explore);
        assert_eq!(AutoplayPolicy::Explore.loosened(), AutoplayPolicy::Explore);
        assert_eq!(AutoplayPolicy::default(), AutoplayPolicy::Balanced);
        assert_eq!(AutoplayPolicy::Similar.window(), Some(3));
        assert_eq!(AutoplayPolicy::Balanced.window(), Some(10));
        assert!(AutoplayPolicy::Explore.window().is_none());
    }

    /// 폴백 사슬: seed → recent → genre → 포기 (§8.2).
    #[test]
    fn autoplay_modes_fall_back_in_order() {
        assert_eq!(AutoplayMode::default(), AutoplayMode::Recent);
        assert_eq!(AutoplayMode::Seed.fallback(), Some(AutoplayMode::Recent));
        assert_eq!(AutoplayMode::Recent.fallback(), Some(AutoplayMode::Genre));
        assert!(AutoplayMode::Genre.fallback().is_none());
        for mode in [AutoplayMode::Seed, AutoplayMode::Recent, AutoplayMode::Genre] {
            assert_eq!(AutoplayMode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn super_like_denials_say_exactly_why() {
        assert!(
            SuperLikeVerdict::Allowed {
                used_today: 1,
                remaining: Some(4)
            }
            .is_allowed()
        );
        assert_eq!(
            SuperLikeVerdict::Cooldown { remaining_sec: 180 }
                .message()
                .unwrap(),
            "슈퍼 좋아요는 3분 0초 뒤에 다시 쓸 수 있어요."
        );
        assert_eq!(
            SuperLikeVerdict::DailyLimitReached { limit: 5 }
                .message()
                .unwrap(),
            "오늘 슈퍼 좋아요를 5번 다 썼어요 (UTC 자정에 초기화돼요)."
        );
    }

    /// 싫어요가 붙어도 좋아요/슈퍼와 문자열 표현이 겹치지 않아야 DB 왕복이 깨지지 않는다.
    #[test]
    fn vote_kinds_round_trip_including_dislike() {
        for kind in [
            QueueVoteKind::Like,
            QueueVoteKind::SuperLike,
            QueueVoteKind::Dislike,
        ] {
            assert_eq!(QueueVoteKind::parse(kind.as_str()), Some(kind));
        }
        let points = VotePoints::default();
        assert_eq!(QueueVoteKind::Dislike.points(&points), -1);
        assert_eq!(QueueVoteKind::SuperLike.points(&points), 2);
        assert_eq!(QueueVoteKind::Dislike.audit_action(), "vote.dislike");
        assert_eq!(QueueVoteKind::SuperLike.api_key(), "superLike");
    }

    #[test]
    fn permission_keys_cover_every_rule_and_stay_at_ten() {
        assert_eq!(PERMISSION_KEYS.len(), 10);
        let settings = RemoteGuildSettings::default();
        for key in PERMISSION_KEYS {
            assert!(settings.rule_for(key).is_some(), "키 {key} 규칙 누락");
            assert!(
                !RemoteGuildSettings::permission_description(key).is_empty(),
                "키 {key} 설명 누락"
            );
        }
    }
}
