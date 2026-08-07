//! C# DiscordMyMusicBot 와 JSON/SQLite 바이트 호환되는 데이터 모델.
//! 직렬화 규칙: camelCase 키, enum 은 문자열(JsonStringEnumConverter), TimeSpan 은 "hh:mm:ss" 계열.
//! C# 쪽 .musicbot-data/musicbot.sqlite 를 그대로 읽고 쓸 수 있어야 한다 (드롭인 마이그레이션).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ───────────────────────── TimeSpan 호환 ─────────────────────────

/// C# System.Text.Json 의 TimeSpan 표현("[-][d.]hh:mm:ss[.fffffff]")과 호환되는 Duration 래퍼.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CsTimeSpan(pub Duration);

impl CsTimeSpan {
    pub fn zero() -> Self {
        CsTimeSpan(Duration::ZERO)
    }
    pub fn from_secs_f64(secs: f64) -> Self {
        CsTimeSpan(Duration::from_secs_f64(secs.max(0.0)))
    }
    pub fn as_secs_f64(&self) -> f64 {
        self.0.as_secs_f64()
    }
    /// "3:25" 또는 "1:02:03" 형태의 사용자 표시용 문자열.
    pub fn display(&self) -> String {
        let total = self.0.as_secs();
        let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
        if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        }
    }
    fn to_cs_string(self) -> String {
        let total = self.0.as_secs();
        let days = total / 86400;
        let (h, m, s) = ((total % 86400) / 3600, (total % 3600) / 60, total % 60);
        let frac = self.0.subsec_nanos() / 100; // 100ns 틱 단위 7자리
        if days > 0 && frac > 0 {
            format!("{days}.{h:02}:{m:02}:{s:02}.{frac:07}")
        } else if days > 0 {
            format!("{days}.{h:02}:{m:02}:{s:02}")
        } else if frac > 0 {
            format!("{h:02}:{m:02}:{s:02}.{frac:07}")
        } else {
            format!("{h:02}:{m:02}:{s:02}")
        }
    }
    fn parse_cs(value: &str) -> Option<Self> {
        // [-][d.]hh:mm:ss[.fffffff]
        let v = value.trim().trim_start_matches('-');
        let (days, rest) = match v.split_once('.') {
            // 점이 시간 앞(일수 구분)인지 소수점인지 구분: 점 앞 조각에 ':' 가 없으면 일수.
            Some((head, tail)) if !head.contains(':') => {
                (head.parse::<u64>().ok()?, tail.to_string())
            }
            _ => (0u64, v.to_string()),
        };
        let (hms, frac) = match rest.split_once('.') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (rest, String::new()),
        };
        let parts: Vec<&str> = hms.split(':').collect();
        if parts.len() != 3 {
            return None;
        }
        let h: u64 = parts[0].parse().ok()?;
        let m: u64 = parts[1].parse().ok()?;
        let s: u64 = parts[2].parse().ok()?;
        let mut nanos: u32 = 0;
        if !frac.is_empty() {
            let padded = format!("{:0<9}", frac); // 7자리 틱 → 9자리 나노로 패딩
            nanos = padded[..9].parse().unwrap_or(0);
        }
        Some(CsTimeSpan(Duration::new(
            days * 86400 + h * 3600 + m * 60 + s,
            nanos,
        )))
    }
}

impl Serialize for CsTimeSpan {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_cs_string())
    }
}

impl<'de> Deserialize<'de> for CsTimeSpan {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        CsTimeSpan::parse_cs(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid TimeSpan: {raw}")))
    }
}

// ───────────────────────── enums ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    YouTube,
    YouTubeMusic,
    SoundCloud,
}

impl ProviderKind {
    pub fn label(&self) -> &'static str {
        match self {
            ProviderKind::YouTube => "YT",
            ProviderKind::YouTubeMusic => "YTM",
            ProviderKind::SoundCloud => "SC",
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::YouTube => "YouTube",
            ProviderKind::YouTubeMusic => "YouTubeMusic",
            ProviderKind::SoundCloud => "SoundCloud",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackRequestKind {
    User,
    Autoplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    Track,
    Queue,
}

impl RepeatMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepeatMode::Off => "Off",
            RepeatMode::Track => "Track",
            RepeatMode::Queue => "Queue",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmptyVoiceChannelPolicy {
    AutoLeave,
    StopPlayback,
    DoNothing,
}

// ───────────────────────── 트랙/큐 ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRef {
    pub provider: ProviderKind,
    pub content_id: String,
    /// **없이 와도 받는다.** 예전에는 필수라, 클라이언트가 빠뜨리면 본문 해석 단계에서
    /// 통째로 실패해 422 만 나갔다. 화면에는 "입력값을 확인해 주세요" 라는, 사람이 고칠 수
    /// 없는 문구만 떴다 — 실제로 브라우저 검색으로 곡을 담을 때 그랬다.
    /// 비어 있으면 [`TrackRef::ensure_source_url`] 이 provider·content_id 로 만들어 준다.
    #[serde(default)]
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<CsTimeSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_key: Option<String>,
}

impl TrackRef {
    /// `source_url` 이 비어 있으면 provider 와 `content_id` 로 만들어 채운다.
    ///
    /// 받는 쪽에서 한 번만 부르면 그 뒤로는 늘 채워진 값이라, 재생기와 캐시가
    /// "주소가 비었을 수도 있다" 를 신경 쓰지 않아도 된다.
    pub fn ensure_source_url(&mut self) {
        if !self.source_url.trim().is_empty() {
            return;
        }
        let id = &self.content_id;
        self.source_url = match self.provider {
            ProviderKind::YouTubeMusic => format!("https://music.youtube.com/watch?v={id}"),
            ProviderKind::YouTube => format!("https://www.youtube.com/watch?v={id}"),
            // 사운드클라우드는 ID 로 주소를 만들 수 없다(경로가 사람이 정한 문자열이다).
            // 여기서 지어내면 재생 단계에서 더 헷갈리는 실패가 나므로 비운 채로 둔다.
            ProviderKind::SoundCloud => String::new(),
        };
    }

    /// C# 과 동일: YouTubeMusic 은 캐시 키 차원에서 youtube 로 통일 (같은 영상 ID 네임스페이스).
    pub fn cache_key(&self) -> String {
        let provider_key = match self.provider {
            ProviderKind::YouTubeMusic => ProviderKind::YouTube,
            other => other,
        };
        match &self.variant_key {
            Some(v) if !v.trim().is_empty() => {
                format!("{}:{}:{}", provider_key.as_str(), self.content_id, v).to_lowercase()
            }
            _ => format!("{}:{}", provider_key.as_str(), self.content_id).to_lowercase(),
        }
    }
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.content_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: String,
    pub track: TrackRef,
    pub requested_by_display: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by_user_id: Option<u64>,
    pub request_kind: PlaybackRequestKind,
    pub requested_at: String, // DateTimeOffset ISO 문자열 — 가공하지 않으므로 문자열 보존
    pub start_offset: CsTimeSpan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_profile: Option<serde_json::Value>, // 사용 안 하지만 round-trip 보존
}

impl QueueItem {
    pub fn new_user(track: TrackRef, requester: String, user_id: Option<u64>) -> Self {
        QueueItem {
            id: uuid_like(),
            track,
            requested_by_display: requester,
            requested_by_user_id: user_id,
            request_kind: PlaybackRequestKind::User,
            requested_at: chrono::Utc::now().to_rfc3339(),
            start_offset: CsTimeSpan::zero(),
            loudness_profile: None,
        }
    }
    pub fn new_autoplay(track: TrackRef) -> Self {
        QueueItem {
            id: uuid_like(),
            track,
            requested_by_display: "(자동추천)".to_string(),
            requested_by_user_id: None,
            request_kind: PlaybackRequestKind::Autoplay,
            requested_at: chrono::Utc::now().to_rfc3339(),
            start_offset: CsTimeSpan::zero(),
            loudness_profile: None,
        }
    }
}

/// C# 의 Guid.NewGuid().ToString("N") 호환 32-hex 식별자.
pub fn uuid_like() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..32)
        .map(|_| {
            let v = rng.random_range(0u8..16u8);
            char::from_digit(v as u32, 16).unwrap()
        })
        .collect()
}

// ───────────────────────── 길드 상태 ─────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GuildPlayerState {
    pub guild_id: u64,
    pub voice_channel_id: Option<u64>,
    pub is_paused: bool,
    pub shuffle_enabled: bool,
    pub autoplay_enabled: bool,
    pub repeat_mode: RepeatMode,
    pub effective_volume: i32,
    pub current_item: Option<QueueItem>,
    pub upcoming: Vec<QueueItem>,
    pub cycle_history: Vec<QueueItem>,
    pub recent_tracks: Vec<TrackRef>,
    /// 휘발성 autoplay 미리보기 (SQLite 비저장).
    pub autoplay_preview: Option<QueueItem>,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Off
    }
}

// ───────────────────────── 전역/길드 설정 ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalSettings {
    pub master_volume: i32,
    pub normalize_enabled: bool,
    pub cache_limit_gb: i32,
    pub preferred_browser_profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_file_path: Option<String>,
    pub autoplay_default: bool,
    pub log_retention_days: i32,
    pub auto_leave_when_empty: bool,
    pub auto_leave_delay_seconds: i32,
    pub empty_voice_policy: EmptyVoiceChannelPolicy,
    pub announce_now_playing: bool,
    // 끊김 최적화 실험 토글 (C# 과 동일 키)
    pub tweak_ffmpeg_fast_start: bool,
    pub tweak_ffmpeg_direct_output: bool,
    pub tweak_small_buffer: bool,
    pub tweak_low_packet_loss: bool,
    pub tweak_dedicated_send_thread: bool,
    pub voice_bitrate_kbps: i32,
    /// 켜면 다운로드 시 SponsorBlock 으로 인트로/아웃트로/비음악 구간을 잘라낸다
    /// (music_offtopic,intro,outro). 크라우드 데이터가 있는 영상에만 적용된다.
    pub sponsorblock_remove: bool,
    /// 켜면 우리가 받은(toolsRoot 안의) yt-dlp 를 하루 1회 자동 업데이트(`yt-dlp -U`)한다.
    /// YouTube 변경으로 다운로드가 깨지는 것을 예방. PATH/시스템 yt-dlp 는 건드리지 않는다.
    pub auto_update_tools: bool,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings {
            master_volume: 100,
            normalize_enabled: true,
            cache_limit_gb: 30,
            preferred_browser_profile: "Default".to_string(),
            cookie_file_path: None,
            autoplay_default: true,
            log_retention_days: 14,
            auto_leave_when_empty: true,
            auto_leave_delay_seconds: 60,
            empty_voice_policy: EmptyVoiceChannelPolicy::AutoLeave,
            announce_now_playing: true,
            tweak_ffmpeg_fast_start: false,
            tweak_ffmpeg_direct_output: false,
            tweak_small_buffer: false,
            tweak_low_packet_loss: false,
            tweak_dedicated_send_thread: false,
            voice_bitrate_kbps: 96,
            sponsorblock_remove: false,
            auto_update_tools: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GuildSettings {
    pub guild_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_override: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalize_enabled_override: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoplay_default_override: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub struct EffectiveGuildSettings {
    pub effective_volume: i32,
    pub normalize_enabled: bool,
    pub autoplay_default: bool,
}

// ───────────────────────── 캐시 ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    pub cache_key: String,
    pub provider: ProviderKind,
    pub content_id: String,
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<CsTimeSpan>,
    pub file_path: String,
    pub size_bytes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_profile: Option<serde_json::Value>,
    pub last_access_utc: String,
    /// 누적 재생 횟수(전역). 캐시 조회(last_access)와 달리 실제 곡 시작에만 +1.
    #[serde(default)]
    pub play_count: i64,
    /// 마지막으로 실제 재생된 시각(전역).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played_utc: Option<String>,
    /// 서버(길드)별 재생 통계. 키는 guild_id.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub per_guild: HashMap<u64, GuildPlayStat>,
}

/// 서버별 재생 통계 — [[CacheEntry]] 안에 중첩 저장.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildPlayStat {
    pub count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played_utc: Option<String>,
}

// ───────────────────────── 차단목록 ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlacklistKind {
    TitleContains,
    TitleExact,
    UrlExact,
}

impl BlacklistKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlacklistKind::TitleContains => "TitleContains",
            BlacklistKind::TitleExact => "TitleExact",
            BlacklistKind::UrlExact => "UrlExact",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "TitleContains" => Some(BlacklistKind::TitleContains),
            "TitleExact" => Some(BlacklistKind::TitleExact),
            "UrlExact" => Some(BlacklistKind::UrlExact),
            _ => None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            BlacklistKind::TitleContains => "제목 포함",
            BlacklistKind::TitleExact => "제목 일치",
            BlacklistKind::UrlExact => "URL 일치",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlacklistEntry {
    pub id: i64,
    pub guild_id: u64, // 0 = 전역
    pub kind: BlacklistKind,
    pub pattern: String,
    pub created_utc: String,
    pub created_by_user_id: u64,
    pub note: Option<String>,
}

// ───────────────────────── 플레이리스트 ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistScope {
    Global,
    Guild,
    /// 개인 재생목록 (V3 §12). 길드에 묶이지 않아 어느 서버에서든 보인다.
    /// `owner_user_id` 로 거른다. 레거시 `playlists` 테이블에 컬럼 추가 없이
    /// `scope` 값만 늘렸다. C# 엔진은 이 값을 모르지만 Global/Guild 조회 조건에
    /// 걸리지 않으므로 서로 간섭하지 않는다.
    User,
}

impl PlaylistScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlaylistScope::Global => "Global",
            PlaylistScope::Guild => "Guild",
            PlaylistScope::User => "User",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("guild") {
            PlaylistScope::Guild
        } else if s.eq_ignore_ascii_case("user") {
            PlaylistScope::User
        } else {
            PlaylistScope::Global
        }
    }
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: i64,
    pub scope: PlaylistScope,
    pub guild_id: Option<u64>,
    pub owner_user_id: u64,
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
}

/// playlist_entries.payload_json 의 round-trip 보존용. C# 쪽 페이로드 구조를 그대로 유지한다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    #[serde(default)]
    pub track: Option<TrackRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_offset: Option<CsTimeSpan>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ───────────────────────── 길드 메타 ─────────────────────────

#[derive(Debug, Clone)]
pub struct GuildMetadata {
    pub guild_id: u64,
    pub name: String,
    pub icon_url: Option<String>,
    pub member_count: Option<i32>,
    pub last_seen_utc: String,
}

// ───────────────────────── 로그 ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    // 한국시간(로컬) rfc3339. 구버전이 쓰던 "timestampUtc" 키도 읽도록 alias 유지.
    #[serde(alias = "timestampUtc")]
    pub timestamp: String,
    pub level: String, // Info / Warn / Error
    pub category: String,
    pub message: String,
}
