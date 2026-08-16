use super::{
    AUDIT_MERGE_WINDOW_SECS, AuditEntry, AuditKind, AutoplaySeed, CHART_CACHE_TTL_HOURS,
    ChartCategory, ChartDef, ChartSnapshot, ChatMessage, ChatReactionSummary, ChatReplyPreview,
    ChatReport, ChatTrackTag, GlobalOverrides, GuildApproval, GuildApprovalStatus, LyricsCacheHit,
    LyricsDocument, KARAOKE_CACHE_TTL_HOURS, MAX_VOTER_IDS, Participant,
    PruneReport, QueueScore, QueueVoteKind, RecentTrack, RemoteGuildSettings, ResumePoint,
    RetentionConfig,
    SeedAddOutcome, StoredSession, Suggestion, SuggestionStatus, SuperLikeStatus, SuperLikeVerdict,
    Suspension, SuspensionScope, UserTrack, UserTrackKind, as_limit_u32, audit_kind_for,
    audit_text, is_mergeable_action, truncate_title,
};
use crate::models::{QueueItem, TrackRef};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

/// 마참뮤직 전용 스키마 버전. `PRAGMA user_version`에 기록된다.
/// 레거시(C# 공용) 테이블은 이 러너가 절대 건드리지 않는다.
const SCHEMA_VERSION: i64 = 22;

/// 채팅 페이지 기본 크기.
pub const CHAT_PAGE_LIMIT: usize = 50;
const CHAT_PAGE_MAX: usize = 200;

/// v0 → v1. 러너 도입 이전 DB에도 그대로 존재하므로 전부 `IF NOT EXISTS`.
const MIGRATION_V1: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_queue_scores (
        item_id TEXT PRIMARY KEY,
        guild_id INTEGER NOT NULL,
        requester_user_id INTEGER NULL,
        wait_score INTEGER NOT NULL DEFAULT 0,
        manual_priority INTEGER NULL,
        original_order INTEGER NOT NULL,
        updated_utc TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_remote_queue_scores_guild
        ON remote_queue_scores(guild_id, original_order);

    CREATE TABLE IF NOT EXISTS remote_queue_votes (
        item_id TEXT NOT NULL,
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        kind TEXT NOT NULL,
        created_utc TEXT NOT NULL,
        PRIMARY KEY(item_id, user_id)
    );
    CREATE INDEX IF NOT EXISTS idx_remote_queue_votes_guild
        ON remote_queue_votes(guild_id, item_id);

    CREATE TABLE IF NOT EXISTS remote_user_tracks (
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        kind TEXT NOT NULL,
        cache_key TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        created_utc TEXT NOT NULL,
        PRIMARY KEY(guild_id, user_id, kind, cache_key)
    );

    CREATE TABLE IF NOT EXISTS remote_recent_tracks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        guild_id INTEGER NOT NULL,
        track_json TEXT NOT NULL,
        requested_by_user_id INTEGER NULL,
        requested_by_display TEXT NOT NULL,
        played_utc TEXT NOT NULL,
        end_reason TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_remote_recent_guild
        ON remote_recent_tracks(guild_id, id DESC);

    CREATE TABLE IF NOT EXISTS remote_chat_messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        avatar_url TEXT NULL,
        content TEXT NOT NULL,
        created_utc TEXT NOT NULL,
        deleted_utc TEXT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_remote_chat_guild
        ON remote_chat_messages(guild_id, id DESC);

    CREATE TABLE IF NOT EXISTS remote_chat_reactions (
        message_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        emoji TEXT NOT NULL,
        created_utc TEXT NOT NULL,
        PRIMARY KEY(message_id, user_id, emoji),
        FOREIGN KEY(message_id) REFERENCES remote_chat_messages(id) ON DELETE CASCADE
    );

    CREATE TABLE IF NOT EXISTS remote_chat_reports (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        guild_id INTEGER NOT NULL,
        message_id INTEGER NOT NULL,
        reporter_user_id INTEGER NOT NULL,
        reporter_display_name TEXT NOT NULL,
        reason TEXT NOT NULL,
        created_utc TEXT NOT NULL,
        resolved_utc TEXT NULL,
        UNIQUE(message_id, reporter_user_id),
        FOREIGN KEY(message_id) REFERENCES remote_chat_messages(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_remote_chat_reports_guild
        ON remote_chat_reports(guild_id, resolved_utc, id DESC);

    CREATE TABLE IF NOT EXISTS remote_audit_logs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        action TEXT NOT NULL,
        target TEXT NULL,
        before_value TEXT NULL,
        after_value TEXT NULL,
        success INTEGER NOT NULL,
        failure_reason TEXT NULL,
        created_utc TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_remote_audit_guild
        ON remote_audit_logs(guild_id, id DESC);

    CREATE TABLE IF NOT EXISTS remote_lyrics (
        cache_key TEXT PRIMARY KEY,
        payload_json TEXT NOT NULL,
        fetched_utc TEXT NOT NULL
    );
"#;

/// v2 → v3. 멘션·노래태그.
const MIGRATION_V3: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_chat_mentions (
        message_id INTEGER NOT NULL,
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        read_utc TEXT NULL,
        PRIMARY KEY(message_id, user_id)
    );
    CREATE INDEX IF NOT EXISTS idx_chat_mentions_unread
        ON remote_chat_mentions(guild_id, user_id, read_utc);

    CREATE TABLE IF NOT EXISTS remote_chat_tags (
        message_id INTEGER NOT NULL,
        cache_key TEXT NOT NULL,
        track_json TEXT NOT NULL,
        PRIMARY KEY(message_id, cache_key)
    );
"#;

/// v3 → v4. 제안 게시판.
const MIGRATION_V4: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_suggestions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        avatar_url TEXT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'open',
        status_note TEXT NULL,
        status_by_user_id INTEGER NULL,
        status_utc TEXT NULL,
        created_utc TEXT NOT NULL,
        deleted_utc TEXT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_suggestions_guild
        ON remote_suggestions(guild_id, id DESC);

    CREATE TABLE IF NOT EXISTS remote_suggestion_votes (
        suggestion_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        created_utc TEXT NOT NULL,
        PRIMARY KEY(suggestion_id, user_id)
    );
"#;

/// v4 → v5. 기능별·기간제 유저 정지.
const MIGRATION_V5: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_user_suspensions (
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        scope TEXT NOT NULL,
        reason TEXT NULL,
        by_user_id INTEGER NOT NULL,
        created_utc TEXT NOT NULL,
        expires_utc TEXT NULL,
        PRIMARY KEY(guild_id, user_id, scope)
    );
"#;

/// v5 → v6. 세션 영속화. 토큰 원문 대신 SHA-256 해시가 PK다.
const MIGRATION_V6: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_web_sessions (
        token_hash TEXT PRIMARY KEY,
        user_id INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        avatar_url TEXT NULL,
        guilds_json TEXT NOT NULL,
        access_token TEXT NULL,
        refresh_token TEXT NULL,
        expires_utc TEXT NOT NULL,
        refreshed_utc TEXT NULL,
        created_utc TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_web_sessions_expiry
        ON remote_web_sessions(expires_utc);
"#;

/// v7 → v8. 보존 정리·페이지네이션이 실제로 인덱스를 타게 한다.
const MIGRATION_V8: &str = r#"
    CREATE INDEX IF NOT EXISTS idx_remote_chat_created
        ON remote_chat_messages(guild_id, created_utc);
    CREATE INDEX IF NOT EXISTS idx_remote_audit_created
        ON remote_audit_logs(guild_id, created_utc);
    CREATE INDEX IF NOT EXISTS idx_remote_queue_scores_requester
        ON remote_queue_scores(guild_id, requester_user_id);
    CREATE INDEX IF NOT EXISTS idx_remote_suggestion_votes
        ON remote_suggestion_votes(suggestion_id);
    CREATE INDEX IF NOT EXISTS idx_remote_lyrics_fetched
        ON remote_lyrics(found, fetched_utc);
"#;

/// v8 → v9. 개인 설정(화면 배치·테마 등). **길드가 아니라 유저 단위**다.
const MIGRATION_V9: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_user_prefs (
        user_id INTEGER NOT NULL,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        updated_utc TEXT NOT NULL,
        PRIMARY KEY(user_id, key)
    );
"#;

/// v9 → v10. 자동 재생이 참고할 기준 곡.
const MIGRATION_V10: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_autoplay_seeds (
        guild_id INTEGER NOT NULL,
        cache_key TEXT NOT NULL,
        track_json TEXT NOT NULL,
        sort_order INTEGER NOT NULL,
        added_by_user_id INTEGER NOT NULL,
        added_utc TEXT NOT NULL,
        PRIMARY KEY(guild_id, cache_key)
    );
    CREATE INDEX IF NOT EXISTS idx_autoplay_seeds_order
        ON remote_autoplay_seeds(guild_id, sort_order);
"#;

/// v10 → v11. 차트 (§15.1). 차트는 **코드가 아니라 데이터**라 유튜브가 주소를 바꿔도
/// 관리 콘솔에서 갈아 끼우면 된다.
const MIGRATION_V11: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_charts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        guild_id INTEGER NULL,
        category TEXT NOT NULL,
        name TEXT NOT NULL,
        provider TEXT NOT NULL,
        url TEXT NOT NULL,
        sort_order INTEGER NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1,
        builtin INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_remote_charts_scope
        ON remote_charts(guild_id, category, sort_order);
    -- 기본 제공분은 이름으로 한 번만 심는다(마이그레이션을 다시 돌려도 중복되지 않게).
    CREATE UNIQUE INDEX IF NOT EXISTS idx_remote_charts_builtin_name
        ON remote_charts(name) WHERE builtin = 1;

    CREATE TABLE IF NOT EXISTS remote_chart_cache (
        chart_id INTEGER PRIMARY KEY,
        tracks_json TEXT NOT NULL,
        fetched_utc TEXT NOT NULL,
        failed_utc TEXT NULL,
        failure_reason TEXT NULL
    );
"#;

/// v11 → v12. 슈퍼 좋아요 하루 사용량 (§10.6).
/// 재시작해도 살아남아야 하니 메모리가 아니라 DB다. 쿨타임만 메모리로 둔다.
const MIGRATION_V12: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_super_like_usage (
        guild_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL,
        day TEXT NOT NULL,
        used INTEGER NOT NULL DEFAULT 0,
        last_utc TEXT NOT NULL,
        PRIMARY KEY(guild_id, user_id, day)
    );
"#;

/// v12 → v13. 막힌 자동재생 후보(§8.5-3)와 사람이 읽는 활동 로그(§13).
const MIGRATION_V13: &str = r#"
    CREATE TABLE IF NOT EXISTS remote_autoplay_blocked (
        guild_id INTEGER NOT NULL,
        cache_key TEXT NOT NULL,
        until_utc TEXT NOT NULL,
        reason TEXT NULL,
        created_utc TEXT NOT NULL,
        PRIMARY KEY(guild_id, cache_key)
    );
    CREATE INDEX IF NOT EXISTS idx_autoplay_blocked_until
        ON remote_autoplay_blocked(guild_id, until_utc);
    CREATE INDEX IF NOT EXISTS idx_remote_audit_kind
        ON remote_audit_logs(guild_id, kind, id DESC);
    -- 합치기(§13.3)가 "같은 사람 · 같은 종류의 최신 한 줄"을 곧장 찾아가게 한다.
    -- 이 인덱스가 없으면 최근에 아무 일도 안 한 사람이 곡을 담을 때마다
    -- 그 길드의 로그를 통째로 거슬러 훑는다.
    CREATE INDEX IF NOT EXISTS idx_remote_audit_merge
        ON remote_audit_logs(guild_id, user_id, action, id DESC);
"#;

/// v14 — 죽은 차트 주소를 고치고 노래방 장르를 늘린다.
///
/// YouTube Music 인기곡 재생목록 두 개가 죽어서 "한국 인기곡"·"전세계 인기곡"이
/// **빈 차트로 나가고 있었다**(yt-dlp 로 0곡 확인, 2026-08-07).
/// `builtin = 1` 이고 **관리자가 손대지 않은 것만** 고친다 — 직접 바꾼 주소를 덮어쓰면 안 된다.
const MIGRATION_V14: &str = r#"
    UPDATE remote_charts
       SET url = 'ytsearch50:한국 인기곡 최신', provider = 'YouTube'
     WHERE builtin = 1 AND name = '한국 인기곡'
       AND url = 'https://music.youtube.com/playlist?list=PL4fGSI1pDJn5Kj4TvUZBcNlkzuxCe4vVh';
    UPDATE remote_charts
       SET url = 'ytsearch50:global top songs this week', provider = 'YouTube'
     WHERE builtin = 1 AND name = '전세계 인기곡'
       AND url = 'https://music.youtube.com/playlist?list=PL4fGSI1pDJn6puJdseH2Rt9sMvt9E2M4i';
    -- 실패 기록을 지워 다음 조회에서 다시 시도하게 한다.
    DELETE FROM remote_chart_cache
     WHERE chart_id IN (SELECT id FROM remote_charts WHERE builtin = 1 AND name IN ('한국 인기곡','전세계 인기곡'));
"#;

/// v16 — 역할 캐시와 재시작 이어듣기를 디스크에 남긴다.
///
/// **역할 캐시가 메모리에만 있어서 생긴 실제 버그.** 재시작하면 캐시가 비는데, 그때
/// Discord 가 429 를 주면 일시 실패 경로가 `unwrap_or_default()` 로 **빈 역할 목록**을
/// 만든다. 그러면 지정 역할로 권한을 받은 사람이 통째로 일반 멤버가 되어
/// "권한이 없어요" 를 본다. 로그에는 429 만 찍히고 화면에는 권한 없음만 뜬다.
/// 디스크에 두면 재시작을 건너뛰어도 유예 시간(6시간) 동안 등급이 유지된다.
const MIGRATION_V16: &str = r#"
    -- 서버 승인 (§26). 봇이 나 혼자 쓰는 것이 아니게 되면서, 아무나 초대해서
    -- 바로 쓰는 것을 막아야 한다. 새 서버는 `pending` 으로 들어오고 봇 주인이
    -- 승인해야 명령어와 리모컨이 열린다.
    CREATE TABLE IF NOT EXISTS remote_guild_approval (
        guild_id      INTEGER PRIMARY KEY,
        status        TEXT NOT NULL,          -- pending | approved | blocked
        guild_name    TEXT NULL,
        invited_by    INTEGER NULL,
        invited_by_name TEXT NULL,
        requested_utc TEXT NOT NULL,
        decided_by    INTEGER NULL,
        decided_utc   TEXT NULL,
        note          TEXT NULL
    );

    -- 노래방 차트는 **순위만** TJ 에서 빌려 오고 트는 것은 원곡이다.
    -- 앞선 버전은 반주(MR)를 찾아 넣었으므로 그때 저장된 해석은 전부 버린다.
    DELETE FROM remote_tj_tracks;

    CREATE TABLE IF NOT EXISTS remote_member_roles (
        guild_id    INTEGER NOT NULL,
        user_id     INTEGER NOT NULL,
        roles_json  TEXT NOT NULL,
        fetched_utc TEXT NOT NULL,
        PRIMARY KEY (guild_id, user_id)
    );

    -- 재시작 이어듣기 (§24). 껐을 때의 재생 위치만 남긴다 —
    -- 음성 채널과 현재 곡은 guild_states 가 이미 들고 있다.
    CREATE TABLE IF NOT EXISTS remote_resume (
        guild_id         INTEGER PRIMARY KEY,
        item_id          TEXT NULL,
        position_seconds REAL NOT NULL,
        was_paused       INTEGER NOT NULL DEFAULT 0,
        saved_utc        TEXT NOT NULL
    );
"#;

/// v17 — **이미 쓰고 있던 서버는 승인된 것으로 넘긴다** (§26).
///
/// v16 이 승인 게이트를 넣으면서 모든 서버가 `pending` 으로 시작했다. 그런데
/// 게이트는 *앞으로 초대될* 서버를 막으려는 것이지, 어제까지 잘 쓰던 서버를
/// 잠그려던 게 아니다. 실제로 배포하자마자 쓰던 서버 3개가 통째로 막혔다 —
/// 봇은 멀쩡히 붙어 있는데 명령어도 리모컨도 전부 거절당했다.
///
/// 봇이 이미 알던 서버(재생 상태·메타데이터가 남아 있는 서버)를 승인으로 채운다.
/// **`INSERT OR IGNORE` 라서 이미 판정이 있는 서버는 안 건드린다** — 차단해 둔 서버가
/// 이 마이그레이션으로 되살아나면 안 된다.
///
/// `guild_states` · `guild_metadata` 는 **레거시(C# 공용) 테이블**이라 이 러너가 만들지
/// 않는다. 없을 수도 있으므로(새 설치에서 `Db::open` 보다 먼저 열리면) 있는 것만 훑는다.
/// 처음엔 raw SQL 로 두 테이블을 UNION 했다가, 테이블이 없으면 마이그레이션이 통째로
/// 실패해 **저장소가 아예 안 열리는** 상태를 만들 뻔했다.
fn migrate_v17_grandfather_guilds(conn: &Connection) -> rusqlite::Result<()> {
    let mut ids: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for table in ["guild_states", "guild_metadata"] {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            continue;
        }
        let mut statement =
            conn.prepare(&format!("SELECT guild_id FROM {table} WHERE guild_id IS NOT NULL"))?;
        let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
        ids.extend(rows.flatten());
    }
    let now = Utc::now().to_rfc3339();
    for guild_id in ids {
        conn.execute(
            r#"INSERT OR IGNORE INTO remote_guild_approval(guild_id, status, requested_utc, note)
               VALUES(?1, 'approved', ?2, '기존 서버 자동 승인')"#,
            params![guild_id, now],
        )?;
        // **INSERT OR IGNORE 만으로는 부족했다.** 게이트를 켠 빌드가 이미 한 번 떠서
        // 이 서버들을 `pending` 으로 등록해 둔 상태였고, 그래서 위 INSERT 가 조용히
        // 무시돼 쓰던 서버 세 개가 계속 잠겨 있었다.
        //
        // 이 마이그레이션이 도는 시점에 DB 에 있는 서버는 전부 게이트보다 먼저 있던 서버다.
        // 그러니 대기 중인 것도 승인으로 올린다. **`blocked` 는 절대 안 건드린다.**
        conn.execute(
            r#"UPDATE remote_guild_approval
                  SET status = 'approved', note = '기존 서버 자동 승인'
                WHERE guild_id = ?1 AND status = 'pending'"#,
            params![guild_id],
        )?;
    }
    Ok(())
}

/// 인기곡 두 장은 마이그레이션에서도 같은 값을 써야 해서 상수로 뽑았다.
/// 여기와 [`MIGRATION_V15`] 가 어긋나면 새 DB 와 기존 DB 의 차트가 서로 달라진다.
const PL_GLOBAL_SONGS: &str =
    "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6puJdseH2Rt9sMvt9E2M4i";
const PL_KOREA_SONGS: &str =
    "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6jXS_Tv_N9B8Z0HTRVJE0m";

/// v15 — 검색으로 만들던 차트를 진짜 차트로 바꾼다.
///
/// v14 에서 죽은 재생목록을 `ytsearch50:` 로 갈아 끼워 **빈 차트는 면했지만**,
/// "인기곡"류 검색어는 개별 곡이 아니라 **7시간짜리 모음 영상**을 물어 온다(2026-08-07 확인).
/// 그대로 담으면 대기열에 몇 시간짜리 영상이 줄줄이 들어간다. 그래서
///   - 인기곡은 YouTube Music 이 매일 갱신하는 자동 생성 재생목록으로,
///   - 노래방은 TJ 공식 차트 API 로
/// 바꾼다. 옛 검색 차트는 지우고 [`BUILTIN_CHARTS`] 시더가 새로 심게 둔다.
///
/// **관리자가 손댄 주소는 건드리지 않는다** — 옛 기본값과 정확히 같을 때만 지운다.
const MIGRATION_V15: &str = r#"
    UPDATE remote_charts
       SET url = 'https://music.youtube.com/playlist?list=PL4fGSI1pDJn6jXS_Tv_N9B8Z0HTRVJE0m'
     WHERE builtin = 1 AND name = '한국 인기곡' AND url = 'ytsearch50:한국 인기곡 최신';
    UPDATE remote_charts
       SET url = 'https://music.youtube.com/playlist?list=PL4fGSI1pDJn6puJdseH2Rt9sMvt9E2M4i'
     WHERE builtin = 1 AND name = '전세계 인기곡' AND url = 'ytsearch50:global top songs this week';

    -- 옛 검색 기반 기본 차트. 이름이 겹치는 것도 있어서 지운 뒤 시더가 새로 심게 한다.
    DELETE FROM remote_chart_cache WHERE chart_id IN (
        SELECT id FROM remote_charts WHERE builtin = 1 AND url IN (
            'ytsearch50:인기 급상승 음악',
            'ytsearch50:US top songs this week',
            'ytsearch50:日本 人気曲 ランキング',
            'ytsearch50:UK top songs this week',
            'ytsearch50:K-Pop 인기곡',
            'ytsearch50:J-Pop 人気曲',
            'ytsearch50:힙합 인기곡',
            'ytsearch50:R&B 인기곡',
            'ytsearch50:록 밴드 인기곡',
            'ytsearch50:EDM 인기곡',
            'ytsearch50:TJ노래방 인기차트',
            'ytsearch50:TJ노래방 발라드',
            'ytsearch50:TJ노래방 댄스',
            'ytsearch50:TJ노래방 힙합 랩',
            'ytsearch50:TJ노래방 일본노래',
            'ytsearch50:TJ노래방 팝송',
            'ytsearch50:TJ노래방 락 밴드'
        )
    );
    DELETE FROM remote_charts WHERE builtin = 1 AND url IN (
        'ytsearch50:인기 급상승 음악',
        'ytsearch50:US top songs this week',
        'ytsearch50:日本 人気曲 ランキング',
        'ytsearch50:UK top songs this week',
        'ytsearch50:K-Pop 인기곡',
        'ytsearch50:J-Pop 人気曲',
        'ytsearch50:힙합 인기곡',
        'ytsearch50:R&B 인기곡',
        'ytsearch50:록 밴드 인기곡',
        'ytsearch50:EDM 인기곡',
        'ytsearch50:TJ노래방 인기차트',
        'ytsearch50:TJ노래방 발라드',
        'ytsearch50:TJ노래방 댄스',
        'ytsearch50:TJ노래방 힙합 랩',
        'ytsearch50:TJ노래방 일본노래',
        'ytsearch50:TJ노래방 팝송',
        'ytsearch50:TJ노래방 락 밴드'
    );

    -- 주소가 바뀐 차트의 옛 캐시는 버린다. 안 버리면 6시간 동안 옛 목록이 그대로 나간다.
    DELETE FROM remote_chart_cache WHERE chart_id IN (
        SELECT id FROM remote_charts WHERE builtin = 1 AND name IN ('한국 인기곡','전세계 인기곡')
    );

    -- CSRF 토큰을 세션과 함께 남긴다.
    -- **이게 없어서 재시작하면 일시정지조차 "CSRF 검증에 실패했어요" 로 막혔다.**
    -- 세션은 DB 에서 되살아나는데 토큰만 새로 만들어져서, 브라우저가 들고 있는
    -- 페이지 셸의 옛 토큰과 영원히 어긋났다. 로그인은 멀쩡해 보여서 더 헷갈렸다.
    ALTER TABLE remote_web_sessions ADD COLUMN csrf_token TEXT;

    -- TJ 곡번호 → 재생 가능한 영상. TJ 는 순위와 곡 정보만 주고 재생 주소는 안 준다.
    -- 곡번호는 TJ 가 영구히 쓰는 값이라 한 번 찾으면 계속 재사용한다.
    -- **이 표가 없으면 노래방 차트를 열 때마다 100번 검색한다.**
    CREATE TABLE IF NOT EXISTS remote_tj_tracks (
        tj_number   INTEGER PRIMARY KEY,
        title       TEXT NOT NULL,
        artist      TEXT NOT NULL,
        provider    TEXT NULL,
        content_id  TEXT NULL,
        source_url  TEXT NULL,
        duration_ms INTEGER NULL,
        -- 못 찾은 곡도 기록한다. 안 그러면 없는 곡을 매번 다시 찾는다.
        resolved_utc TEXT NOT NULL,
        miss_count  INTEGER NOT NULL DEFAULT 0
    );
"#;

/// 기본 제공 차트 (§15.2). `guild_id IS NULL` 이라 모든 서버가 같이 본다.
///
/// **주소는 네 가지 방식이 있다.**
///   - `https://...playlist?list=...` — 실제 재생목록. **지금 인기·나라별·장르가 전부 이것이다.**
///     아래 인기/나라별/장르 ID 는 전부 `YouTube Music Global Charts`
///     (채널 `UCrKZcyOJVWnJ60zM1XWllNw`) 가 매일 갱신하는 자동 생성 차트다.
///     ID 가 바뀔 수 있으니 **넣거나 고칠 때는 반드시 yt-dlp 로 곡 수를 확인한다.**
///   - `tj:top:N` / `tj:hot` — TJ 노래방 **공식 차트 API** 를 직접 긁는다([`crate::remote::tj`]).
///     검색으로 흉내내던 것을 진짜 순위로 바꾼 자리다. 순위는 TJ 가 주고, 재생용 영상은
///     곡명+가수로 한 번 찾아 TJ 번호에 붙여 캐시한다.
///   - `ytsearchN:검색어` — yt-dlp 검색. 안 죽지만 **"인기곡"류 검색어는 개별 곡이 아니라
///     몇 시간짜리 모음 영상이 올라온다**(2026-08-07 확인). 그래서 인기 차트에서는 뺐다.
///     앞의 숫자는 설정값(`chart_limit`)으로 갈아 끼운다.
///   - `internal:...` — 우리가 튼 기록으로 만드는 차트(§15.2b). 외부 호출이 없다.
///
/// 관리 콘솔에서 주소를 바꿀 수 있다. 여기 값은 **처음 한 번만** 심어진다.
const BUILTIN_CHARTS: [(ChartCategory, &str, &str, &str); 49] = [
    // 우리가 실제로 튼 것으로 만드는 차트 — 자동재생으로 나간 곡은 세지 않는다.
    (ChartCategory::Ours, "우리 서버 인기곡", "Internal", "internal:guild-plays"),
    (ChartCategory::Ours, "우리 서버 사랑받은 곡", "Internal", "internal:guild-love"),
    (ChartCategory::Ours, "마참뮤직 전체 인기곡", "Internal", "internal:global-plays"),
    (ChartCategory::Ours, "마참뮤직 전체 사랑받은 곡", "Internal", "internal:global-love"),
    // 인기 — 전부 YouTube Music Global Charts 의 자동 생성 재생목록. 2026-08-07 곡 수 확인함.
    (ChartCategory::Popular, "전세계 인기곡", "YouTube", PL_GLOBAL_SONGS),
    (ChartCategory::Popular, "한국 인기곡", "YouTube", PL_KOREA_SONGS),
    (ChartCategory::Popular, "전세계 뮤직비디오", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn5kI81J1fYWK5eZRl1zJ5kM"),
    (ChartCategory::Popular, "한국 뮤직비디오", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn5S09aId3dUGp40ygUqmPGc"),
    (ChartCategory::Popular, "한국 쇼츠 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4mJcF9T0qw-h-gUobHcNVU"),
    (ChartCategory::Popular, "전세계 쇼츠 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6H1adzIM6ez64e8bHXKNTj"),
    // 나라별 — 같은 채널의 "Top 100 Songs <나라>".
    (ChartCategory::Region, "미국 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6O1LS0XSdF3RyO0Rq_LDeI"),
    (ChartCategory::Region, "일본 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4-UIb6RKHdxam-oAUULIGB"),
    (ChartCategory::Region, "영국 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6_f5P3MnzXg9l3GDfnSlXa"),
    (ChartCategory::Region, "캐나다 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn57Q7WbODbmXjyjgXi0BTyD"),
    (ChartCategory::Region, "호주 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn7xvYy-bP6UFeG5tITQgScd"),
    (ChartCategory::Region, "독일 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6KpOXlp0MH8qA9tngXaUJ-"),
    (ChartCategory::Region, "프랑스 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn7bK3y1Hx-qpHBqfr6cesNs"),
    (ChartCategory::Region, "브라질 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn7rGBE8kEC0CqTa1nMh9AKB"),
    (ChartCategory::Region, "멕시코 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn6fko1AmNa_pdGPZr5ROFvd"),
    (ChartCategory::Region, "대만 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn68Qgd3kW_hKrqMhxAHz62W"),
    (ChartCategory::Region, "인도 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4pTWyM3t61lOyZ6_4jcNOw"),
    (ChartCategory::Region, "인도네시아 인기곡", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn5ObxTlEPlkkornHXUiKX1z"),
    // 장르 — 같은 채널의 "Top 50 <장르> Music Videos". 미국 기준이라 이름에 나라를 안 붙였다.
    (ChartCategory::Genre, "팝", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn77aK7sAW2AT0oOzo5inWY8"),
    (ChartCategory::Genre, "힙합", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4fmCoF1vKHLtivI0f9yHiF"),
    (ChartCategory::Genre, "록", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn5LOptOQixqnzXNGjNXAgYY"),
    (ChartCategory::Genre, "하드록·메탈", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4w4wTTgOmP_S80PoCtbGrL"),
    (ChartCategory::Genre, "댄스·일렉트로닉", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4rBU0RHnR6-b1_uE20CzRH"),
    (ChartCategory::Genre, "라틴", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn5O8siDeZuI_4hbk6JWtTX1"),
    (ChartCategory::Genre, "재즈", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn7Wkr6Ll6ds1AhA42rT8uaU"),
    (ChartCategory::Genre, "컨트리", "YouTube", "https://music.youtube.com/playlist?list=PL4fGSI1pDJn4EBsWVeFpcSAVOFMfhyipg"),
    // J-POP 계열은 위 "Top 50 …United States" 묶음에 아예 없다. 그래서 나라별에만 있고
    // 장르에서는 빠져 있었다. 유튜브 뮤직이 직접 큐레이션한 재생목록(`RDCLAK5uy_…`)으로 채운다.
    // 2026-08-08 곡 수·길이 확인함.
    (ChartCategory::Genre, "J-POP", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_nbK9qSkqYZvtMXH1fLCMmC1yn8HEm0W90"),
    (ChartCategory::Genre, "J-POP 최신", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_lwbizuU3lWX-XkvD8tvEd8phxcIneMvwc"),
    (ChartCategory::Genre, "J-POP 봄노래", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_lRj2PxRYIUGDG0p0KjsQ62d2lLYLfgXAw"),
    (ChartCategory::Genre, "애니송", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_mRcc2Y3l-RoZsDt27qu8CBGpKt-5w7v8g"),
    (ChartCategory::Genre, "시티팝", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_nEjjAWEM3M3fk2tT4Lhb5JOr_HoD0tjnk"),
    (ChartCategory::Genre, "시티팝 최신", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_muPPezCrTrwoL7Ep_9a69YkIaBjsyKTg0"),
    // 노래방 — TJ 는 공식 API 를 직접 긁는다. 검색으로 흉내내지 않는다.
    (ChartCategory::Karaoke, "TJ 인기 100", "TJ", "tj:hot"),
    (ChartCategory::Karaoke, "TJ 가요 100", "TJ", "tj:top:1"),
    (ChartCategory::Karaoke, "TJ 발라드", "TJ", "tj:top:4"),
    (ChartCategory::Karaoke, "TJ 댄스", "TJ", "tj:top:5"),
    (ChartCategory::Karaoke, "TJ 트로트", "TJ", "tj:top:6"),
    (ChartCategory::Karaoke, "TJ 인디·어쿠스틱", "TJ", "tj:top:7"),
    (ChartCategory::Karaoke, "TJ 팝송", "TJ", "tj:top:2"),
    (ChartCategory::Karaoke, "TJ J-POP", "TJ", "tj:top:3"),
    (ChartCategory::Karaoke, "TJ OST", "TJ", "tj:top:8"),
    (ChartCategory::Karaoke, "일본 노래방 히트", "YouTube", "https://music.youtube.com/playlist?list=RDCLAK5uy_kW4l3hmtC_Aq2XCvin1b3h6tziPMH0tsk"),
    // 금영은 공개 API 를 못 찾아서 검색으로 남긴다. 노래방 채널은 개별 곡을 올리므로
    // 인기곡 검색과 달리 모음 영상이 안 잡힌다(2026-08-07 실측: 6/6 개별 반주).
    (ChartCategory::Karaoke, "금영 인기차트", "YouTube", "ytsearch50:금영노래방 인기차트"),
    // SoundCloud
    (
        ChartCategory::Soundcloud,
        "SoundCloud 인기 (한국)",
        "SoundCloud",
        "https://soundcloud.com/discover/sets/charts-top:all-music:kr",
    ),
    (
        ChartCategory::Soundcloud,
        "SoundCloud 인기 (전세계)",
        "SoundCloud",
        "https://soundcloud.com/discover/sets/charts-top:all-music",
    ),
];

/// 막힌 자동재생 후보를 며칠 기억할지 (§8.5-3).
pub const AUTOPLAY_BLOCK_DAYS: i64 = 7;

/// 차트 펼치기 표시를 버려진 것으로 보는 시간(초) (§15.1).
/// yt-dlp 로 100곡짜리 재생목록을 펼치는 데 걸리는 시간보다 넉넉해야 하고,
/// 사람이 다시 눌러 보는 주기보다는 짧아야 한다.
const CHART_FETCH_STALE_SECS: i64 = 180;

/// 서버가 받아 주는 개인 설정 키. 여기 없는 키는 조용히 버린다 —
/// 아무 값이나 저장되면 개인 설정 테이블이 남의 키-밸류 저장소가 돼 버린다.
pub const PREF_KEYS: [&str; 11] = [
    "layout",
    "theme",
    "layoutSizes",
    "panelLayout",
    "panelSlots",
    "lyricsOpen",
    "webPlayback",
    "webVolume",
    "webOffset",
    "auditFilter",
    "notify",
];

/// 웹에서 듣기 싱크 보정의 한계(초). 사람마다 회선과 버퍼가 달라서 봇과 어긋난다.
/// ±10초면 실제로 겪는 어긋남은 다 덮는다. 그보다 크면 곡을 잘못 맞춘 것이다.
pub const WEB_OFFSET_LIMIT: f64 = 10.0;

/// 화면 배치 6종 (§7.2).
pub const LAYOUT_VALUES: [&str; 6] = ["three", "two", "focus", "dj", "talk", "panel"];
/// 테마 7종 + 시스템 따라가기 (§17.1 · §17.3).
pub const THEME_VALUES: [&str; 8] = [
    "auto", "dark", "light", "midnight", "slate", "sepia", "retro", "nord",
];

/// `layoutSizes` 값 길이 상한(바이트). 열 너비 몇 개면 충분한 크기다.
const PREF_LAYOUT_SIZES_MAX: usize = 2 * 1024;
/// `panelLayout` 값 길이 상한(바이트). 도킹 트리라 조금 더 준다.
const PREF_PANEL_LAYOUT_MAX: usize = 8 * 1024;

/// 개인 설정 한 쌍이 저장 가능한 값인지. 웹도 같은 판정을 쓰라고 공개해 둔다.
/// (화면에서 통과한 값이 서버에서 조용히 버려지면 원인을 못 찾는다.)
pub fn is_valid_pref(key: &str, value: &str) -> bool {
    match key {
        "layout" => LAYOUT_VALUES.contains(&value),
        "theme" => THEME_VALUES.contains(&value),
        // `nowVoters` — 지금 곡에 누가 눌렀는지 명단을 펼쳐 둘까 (§10.4).
        //
        // **여기 없으면 저장이 통째로 실패한다.** 이 검사는 키 하나만 모르면
        // `api_prefs_put` 이 배치 전체를 거절하는데, 화면은 300ms 동안 여러 설정을 모아
        // 한 번에 보낸다. 그래서 명단을 접었다 폈다 하는 동안 같은 배치에 실린
        // `webVolume`·`webOffset` 저장까지 같이 날아갔다 — 실제로 그랬다.
        "lyricsOpen" | "webPlayback" | "nowVoters" => matches!(value, "0" | "1"),
        // 영상 크기 1~4 (§40). 목록에 없으면 저장이 통째로 실패한다 — `nowVoters` 가 그랬다.
        "videoSize" => matches!(value, "1" | "2" | "3" | "4"),
        /* 개발자 콘솔 창의 자리와 크기. `"x,y"` · `"w,h"` 꼴의 정수 두 개다.
         *
         * **`devPos` 는 화면이 보내는데 여기 없었다.** 그래서 콘솔을 옮겨 놔도 다음에
         * 열면 늘 오른쪽 아래로 돌아갔고, 게다가 같은 배치에 실린 다른 설정까지
         * 같이 거절당했다(`nowVoters` 와 같은 사고). */
        "devPos" | "devSize" => {
            let mut parts = value.split(',');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(a), Some(b), None) => {
                    a.parse::<i32>().is_ok_and(|v| v.abs() <= 20_000)
                        && b.parse::<i32>().is_ok_and(|v| v.abs() <= 20_000)
                }
                _ => false,
            }
        }
        "webVolume" => value
            .parse::<u32>()
            .map(|volume| volume <= 100 && !value.starts_with('+'))
            .unwrap_or(false),
        // 웹에서 듣기 싱크 보정 (§20.4). 사람마다 다르므로 개인 설정이다.
        // 음수를 받아야 해서 `webVolume` 처럼 부호를 막으면 안 된다.
        "webOffset" => value
            .parse::<f64>()
            .map(|seconds| seconds.is_finite() && seconds.abs() <= WEB_OFFSET_LIMIT)
            .unwrap_or(false),
        "layoutSizes" => is_valid_json_pref(value, PREF_LAYOUT_SIZES_MAX),
        "panelLayout" => is_valid_json_pref(value, PREF_PANEL_LAYOUT_MAX),
        // 배치 슬롯 (최대 5개 × 트리 하나). 트리 상한의 6배까지 받는다.
        // 내용 검증은 클라이언트의 sanitizeTree 가 한다 — 여기서는 크기와 JSON 여부만 본다.
        "panelSlots" => is_valid_json_pref(value, PREF_PANEL_LAYOUT_MAX * 6),
        // 로그 필터 칩 선택 (§13.4). 분류 이름을 콤마로 이은 값이다.
        // 전부 끈 상태는 빈 문자열이 아니라 `none` 으로 저장한다 —
        // 빈 값은 "저장한 적 없음"과 구분이 안 돼서 기본 필터가 되살아난다.
        "auditFilter" => {
            value == "none"
                || (value.len() <= 128
                    && value
                        .split(',')
                        .all(|kind| AuditKind::parse(kind.trim()).is_some()))
        }
        // 알림 종류별 on/off (§16 B3). 예: {"song":1,"mention":1,"reply":0}
        "notify" => is_valid_json_pref(value, 512),
        _ => false,
    }
}

/// JSON 문자열 설정: 길이 상한을 넘거나 JSON이 아니면 거부한다.
fn is_valid_json_pref(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && serde_json::from_str::<serde_json::Value>(value).is_ok()
}

/// 마참뮤직 전용 테이블 저장소. 기존 음악봇 테이블과 같은 SQLite 파일을 WAL로 공유한다.
pub struct RemoteStore {
    conn: Mutex<Connection>,
    /// 지금 yt-dlp 로 펼치는 중인 차트 (§15.1). 같은 차트를 여러 사람이 동시에 눌러도
    /// **하나만 돌리고 나머지는 그 결과를 기다린다** — 안 그러면 yt-dlp 가 줄줄이 선다.
    ///
    /// 값은 시작 시각이다. 핸들러 future 가 도중에 drop 되면(탭을 닫거나 페이지를 옮기면
    /// axum 이 그렇게 한다) `end_chart_fetch` 가 안 불려 그 차트가 프로세스가 죽을 때까지
    /// "가져오는 중"에 갇힌다 — `CHART_FETCH_STALE_SECS` 가 지난 표시는 버려진 것으로 보고
    /// 다음 사람이 이어받는다.
    chart_inflight: Mutex<HashMap<i64, DateTime<Utc>>>,
    /// 슈퍼 좋아요 쿨타임 (§10.6). 짧고 재시작 때 풀려도 손해가 없어 메모리로 충분하다.
    /// 하루 사용량만 DB에 둔다.
    super_like_cooldowns: Mutex<HashMap<(u64, u64), DateTime<Utc>>>,
}

impl RemoteStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let mut conn = Connection::open(path)?;
        // journal_mode 는 트랜잭션 안에서 못 바꾸므로 마이그레이션보다 먼저 건다.
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;
            "#,
        )?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            chart_inflight: Mutex::new(HashMap::new()),
            super_like_cooldowns: Mutex::new(HashMap::new()),
        })
    }

    fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// UTC 자정 기준의 오늘 (§10.6). 서버마다 시간대를 따로 두면
    /// "언제 초기화되지?"가 헷갈리고 코드도 지저분해진다.
    fn utc_day() -> String {
        Utc::now().format("%Y-%m-%d").to_string()
    }

    /// 세션 토큰의 SHA-256 16진 해시. 저장소에는 이 값만 남는다.
    pub fn session_token_hash(token: &str) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(token.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// **이 서버에 실제로 적용되는 설정.** 저장된 길드 값 위에 봇 주인의 전역 강제값을 덮는다.
    ///
    /// 강제값 해석을 여기 넣은 이유: 길드 설정을 읽는 자리는 이미 스무 곳이 넘고
    /// (권한 판정 · 플레이어 · 대기열 상한 · 화면 응답) 앞으로도 늘어난다. 부르는 쪽이
    /// "강제값도 챙겨서 덮어라" 를 기억해야 하는 구조면 언젠가 한 곳이 빠지고,
    /// 그 순간 **잠갔는데 안 먹는** 상태가 된다. 그래서 기본 경로가 곧 유효값이고,
    /// 날것이 필요한 자리만 `load_guild_settings_raw` 를 명시적으로 부른다.
    pub fn load_guild_settings(&self, guild_id: u64) -> RemoteGuildSettings {
        let mut settings = self.load_guild_settings_raw(guild_id);
        // 뮤텍스를 겹쳐 잡지 않는다 — 둘 다 `self.conn` 을 잠그므로 순서대로 부른다.
        let overrides = self.load_global_overrides();
        overrides.apply(&mut settings);
        // 강제값이 범위를 벗어나 들어와도 `0 = 무제한` 규약은 지켜져야 한다 (§23.1).
        settings.sanitize();
        settings
    }

    /// **저장된 길드 값 그대로.** 강제값을 안 덮는다.
    ///
    /// 서버 관리자가 원래 무엇을 골라 뒀는지 알아야 하는 자리 전용이다 —
    /// 강제가 풀렸을 때 되살릴 값이고, 판정에 쓰면 안 된다.
    pub fn load_guild_settings_raw(&self, guild_id: u64) -> RemoteGuildSettings {
        let key = format!("remote_guild_settings:{guild_id}");
        let conn = self.conn.lock().unwrap();
        let json = conn
            .query_row(
                "SELECT json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten();
        let mut settings: RemoteGuildSettings = json
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        settings.guild_id = guild_id;
        // 읽을 때도 한 번 정리한다 — 옛 버전이 저장한 범위 밖 값이 그대로 동작에 쓰이면 안 된다.
        settings.sanitize();
        settings
    }

    /// **저장 직전에 `sanitize`를 강제한다** (§23.1). 어느 라우트를 거쳐 들어와도
    /// `0 = 무제한` 규약과 허용 범위가 실제로 지켜진다.
    ///
    /// 강제된 항목은 여기서 **저장돼 있던 길드 값으로 되돌린다.** 부르는 쪽 대부분이
    /// `ctx.settings`(= 이미 강제값이 덮인 유효값)를 고쳐서 그대로 넘기기 때문에,
    /// 그냥 쓰면 강제값이 길드 JSON 에 구워져 강제를 풀어도 옛 설정이 안 돌아온다.
    /// API 층의 거절(§요구4)과 별개로 여기서도 한 번 더 막는다 — 새 라우트가 생겨도 안전하다.
    pub fn save_guild_settings(&self, settings: &RemoteGuildSettings) -> rusqlite::Result<()> {
        let mut settings = settings.clone();
        settings.sanitize();
        let overrides = self.load_global_overrides();
        if !overrides.is_empty() {
            let stored = self.load_guild_settings_raw(settings.guild_id);
            overrides.restore(&mut settings, &stored);
            // 되돌리면서 볼륨 세 값이 다시 어긋날 수 있다.
            settings.sanitize();
        }
        let settings = &settings;
        let key = format!("remote_guild_settings:{}", settings.guild_id);
        let json = serde_json::to_string(settings).unwrap_or_else(|_| "{}".into());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings(key, json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            params![key, json],
        )?;
        Ok(())
    }

    // ───────── 봇 주인 전역 강제값 ─────────

    /// 전역 강제값이 사는 `settings` 테이블 키. **길드 키와 완전히 분리돼 있다** —
    /// 길드 JSON 에 섞으면 길드 저장 한 번에 강제값이 통째로 날아간다.
    const GLOBAL_OVERRIDES_KEY: &'static str = "remote_global_overrides";

    /// 봇 주인이 걸어 둔 전역 강제값. 아직 아무것도 안 걸었으면 전부 `None` 이라
    /// `apply` 가 아무 일도 하지 않는다 — 도입 전과 완전히 같은 동작이다.
    pub fn load_global_overrides(&self) -> GlobalOverrides {
        let conn = self.conn.lock().unwrap();
        let json = conn
            .query_row(
                "SELECT json FROM settings WHERE key = ?1",
                params![Self::GLOBAL_OVERRIDES_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten();
        let mut overrides: GlobalOverrides = json
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        // 읽을 때도 조인다. 옛 버전이나 손으로 고친 JSON 이 범위 밖 값을 들고 있어도
        // 그게 그대로 모든 서버의 동작이 되면 안 된다.
        overrides.sanitize();
        overrides
    }

    pub fn save_global_overrides(&self, overrides: &GlobalOverrides) -> rusqlite::Result<()> {
        let mut overrides = overrides.clone();
        overrides.sanitize();
        let json = serde_json::to_string(&overrides).unwrap_or_else(|_| "{}".into());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings(key, json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            params![Self::GLOBAL_OVERRIDES_KEY, json],
        )?;
        Ok(())
    }

    // ───────── 대기열 점수 ─────────

    /// 현재 상태의 모든 사용자 요청에 점수 행이 존재하도록 보장한다.
    /// 새 행에는 그 사람이 마지막으로 곡을 튼 시각을 물려줘 공평제가 바로 동작하게 한다.
    pub fn ensure_queue_items(&self, guild_id: u64, items: &[QueueItem]) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut next_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(original_order), -1) + 1 FROM remote_queue_scores WHERE guild_id = ?1",
            params![guild_id as i64],
            |row| row.get(0),
        )?;
        let now = Self::now_iso();
        for item in items {
            let last_played: Option<String> = match item.requested_by_user_id {
                Some(user_id) => tx
                    .query_row(
                        "SELECT MAX(last_played_utc) FROM remote_queue_scores WHERE guild_id = ?1 AND requester_user_id = ?2",
                        params![guild_id as i64, user_id as i64],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten(),
                None => None,
            };
            let changed = tx.execute(
                r#"INSERT OR IGNORE INTO remote_queue_scores
                   (item_id, guild_id, requester_user_id, wait_score, manual_priority, original_order, updated_utc, round, last_played_utc)
                   VALUES(?1, ?2, ?3, 0, NULL, ?4, ?5, 0, ?6)"#,
                params![
                    item.id,
                    guild_id as i64,
                    item.requested_by_user_id.map(|id| id as i64),
                    next_order,
                    now,
                    last_played,
                ],
            )?;
            if changed > 0 {
                next_order += 1;
            }
        }
        tx.commit()
    }

    /// 대기열 점수 + **누가 눌렀는지**(§10.4)를 한 방 쿼리로 가져온다.
    /// 항목마다 투표자를 다시 물으면 대기열이 길어질수록 쿼리가 N배로 늘어난다.
    /// 투표자 ID 는 항목·종류당 `MAX_VOTER_IDS`(12)명까지만 싣는다 — 그 이상은 개수로 보여 준다.
    pub fn queue_scores(&self, guild_id: u64) -> HashMap<String, QueueScore> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT s.item_id, s.requester_user_id, s.wait_score, s.manual_priority, s.original_order,
                      s.round, s.last_played_utc, v.user_id, v.kind
               FROM remote_queue_scores s
               LEFT JOIN remote_queue_votes v ON v.item_id = s.item_id
               WHERE s.guild_id = ?1
               ORDER BY s.item_id, v.created_utc, v.user_id"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return HashMap::new(),
        };
        let rows = match statement.query_map(params![guild_id as i64], |row| {
            Ok((
                QueueScore {
                    item_id: row.get(0)?,
                    guild_id,
                    requester_user_id: row.get::<_, Option<i64>>(1)?.map(|id| id as u64),
                    wait_score: row.get(2)?,
                    manual_priority: row.get(3)?,
                    original_order: row.get(4)?,
                    round: row.get(5)?,
                    last_played_utc: row.get(6)?,
                    ..Default::default()
                },
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return HashMap::new(),
        };

        let mut scores: HashMap<String, QueueScore> = HashMap::new();
        for (score, voter, kind) in rows.flatten() {
            let entry = scores.entry(score.item_id.clone()).or_insert(score);
            let (Some(voter), Some(kind)) = (voter, kind.as_deref().and_then(QueueVoteKind::parse))
            else {
                continue; // 투표가 하나도 없는 항목의 LEFT JOIN 행.
            };
            let voter = voter as u64;
            let (count, ids) = match kind {
                QueueVoteKind::Like => (&mut entry.like_count, &mut entry.like_by),
                QueueVoteKind::SuperLike => {
                    (&mut entry.super_like_count, &mut entry.super_by)
                }
                QueueVoteKind::Dislike => (&mut entry.dislike_count, &mut entry.dislike_by),
            };
            *count += 1;
            if ids.len() < MAX_VOTER_IDS {
                ids.push(voter);
            }
        }
        scores
    }

    pub fn increment_wait_scores(&self, item_ids: &[String]) -> rusqlite::Result<()> {
        if item_ids.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = Self::now_iso();
        for item_id in item_ids {
            tx.execute(
                "UPDATE remote_queue_scores SET wait_score = wait_score + 1, updated_utc = ?2 WHERE item_id = ?1",
                params![item_id, now],
            )?;
        }
        tx.commit()
    }

    /// 공평제 핵심: 내 곡이 하나 재생되면 내 대기 점수를 0으로 되돌리고 마지막 재생 시각을 갱신한다.
    /// 남아 있는 내 곡 전부에 적용해야 다음 라운드 비교가 맞는다.
    pub fn mark_requester_played(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now_iso();
        conn.execute(
            r#"UPDATE remote_queue_scores
               SET wait_score = 0, last_played_utc = ?3, updated_utc = ?3
               WHERE guild_id = ?1 AND requester_user_id = ?2"#,
            params![guild_id as i64, user_id as i64, now],
        )
    }

    /// 계산된 라운드를 저장한다. 표시용이라 정렬 때마다 부를 필요는 없다.
    pub fn save_queue_rounds(
        &self,
        guild_id: u64,
        rounds: &HashMap<String, i32>,
    ) -> rusqlite::Result<()> {
        if rounds.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for (item_id, round) in rounds {
            tx.execute(
                "UPDATE remote_queue_scores SET round = ?3 WHERE guild_id = ?1 AND item_id = ?2",
                params![guild_id as i64, item_id, round],
            )?;
        }
        tx.commit()
    }

    pub fn set_manual_priority(
        &self,
        guild_id: u64,
        item_id: &str,
        priority: Option<i32>,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE remote_queue_scores SET manual_priority = ?3, updated_utc = ?4 WHERE guild_id = ?1 AND item_id = ?2",
            params![guild_id as i64, item_id, priority, Self::now_iso()],
        )?;
        Ok(changed > 0)
    }

    pub fn clear_item_runtime(&self, item_id: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM remote_queue_votes WHERE item_id = ?1",
            params![item_id],
        )?;
        tx.execute(
            "DELETE FROM remote_queue_scores WHERE item_id = ?1",
            params![item_id],
        )?;
        tx.commit()
    }

    pub fn user_vote(&self, item_id: &str, user_id: u64) -> Option<QueueVoteKind> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT kind FROM remote_queue_votes WHERE item_id = ?1 AND user_id = ?2",
            params![item_id, user_id as i64],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|kind| QueueVoteKind::parse(&kind))
    }

    pub fn set_vote(
        &self,
        guild_id: u64,
        item_id: &str,
        user_id: u64,
        kind: Option<QueueVoteKind>,
        track: &TrackRef,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = Self::now_iso();
        tx.execute(
            "DELETE FROM remote_queue_votes WHERE item_id = ?1 AND user_id = ?2",
            params![item_id, user_id as i64],
        )?;
        let cache_key = track.cache_key();
        // 싫어요는 개인 "좋아요 목록"에 들어가면 안 된다 — 좋아요를 싫어요로 바꾸면 목록에서 빠진다.
        let liked = matches!(kind, Some(QueueVoteKind::Like | QueueVoteKind::SuperLike));
        if let Some(kind) = kind {
            tx.execute(
                "INSERT INTO remote_queue_votes(item_id, guild_id, user_id, kind, created_utc) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![item_id, guild_id as i64, user_id as i64, kind.as_str(), now],
            )?;
        }
        if liked {
            let payload = serde_json::to_string(track).unwrap_or_else(|_| "{}".into());
            tx.execute(
                r#"INSERT INTO remote_user_tracks(guild_id, user_id, kind, cache_key, payload_json, created_utc)
                   VALUES(?1, ?2, 'Liked', ?3, ?4, ?5)
                   ON CONFLICT(guild_id, user_id, kind, cache_key) DO UPDATE SET
                     payload_json = excluded.payload_json, created_utc = excluded.created_utc"#,
                params![guild_id as i64, user_id as i64, cache_key, payload, now],
            )?;
        } else {
            tx.execute(
                "DELETE FROM remote_user_tracks WHERE guild_id = ?1 AND user_id = ?2 AND kind = 'Liked' AND cache_key = ?3",
                params![guild_id as i64, user_id as i64, cache_key],
            )?;
        }
        tx.commit()
    }

    pub fn set_user_track(
        &self,
        guild_id: u64,
        user_id: u64,
        kind: UserTrackKind,
        track: &TrackRef,
        present: bool,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let cache_key = track.cache_key();
        if present {
            let payload = serde_json::to_string(track).unwrap_or_else(|_| "{}".into());
            conn.execute(
                r#"INSERT INTO remote_user_tracks(guild_id, user_id, kind, cache_key, payload_json, created_utc)
                   VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(guild_id, user_id, kind, cache_key) DO UPDATE SET
                     payload_json = excluded.payload_json, created_utc = excluded.created_utc"#,
                params![guild_id as i64, user_id as i64, kind.as_str(), cache_key, payload, Self::now_iso()],
            )?;
        } else {
            conn.execute(
                "DELETE FROM remote_user_tracks WHERE guild_id = ?1 AND user_id = ?2 AND kind = ?3 AND cache_key = ?4",
                params![guild_id as i64, user_id as i64, kind.as_str(), cache_key],
            )?;
        }
        Ok(())
    }

    pub fn list_user_tracks(
        &self,
        guild_id: u64,
        user_id: u64,
        kind: UserTrackKind,
    ) -> Vec<UserTrack> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            "SELECT payload_json, created_utc FROM remote_user_tracks WHERE guild_id = ?1 AND user_id = ?2 AND kind = ?3 ORDER BY created_utc DESC",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map(
            params![guild_id as i64, user_id as i64, kind.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.flatten()
            .filter_map(|(json, created_utc)| {
                serde_json::from_str::<TrackRef>(&json)
                    .ok()
                    .map(|track| UserTrack {
                        guild_id,
                        user_id,
                        kind,
                        track,
                        created_utc,
                    })
            })
            .collect()
    }

    pub fn record_recent(
        &self,
        guild_id: u64,
        item: &QueueItem,
        reason: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO remote_recent_tracks(guild_id, track_json, requested_by_user_id, requested_by_display, played_utc, end_reason) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                guild_id as i64,
                serde_json::to_string(&item.track).unwrap_or_else(|_| "{}".into()),
                item.requested_by_user_id.map(|id| id as i64),
                item.requested_by_display,
                Self::now_iso(),
                reason,
            ],
        )?;
        Ok(())
    }

    pub fn list_recent(&self, guild_id: u64, limit: usize) -> Vec<RecentTrack> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            "SELECT id, track_json, requested_by_user_id, requested_by_display, played_utc, end_reason FROM remote_recent_tracks WHERE guild_id = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map(params![guild_id as i64, limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.flatten()
            .filter_map(|(id, json, requester, display, played, reason)| {
                serde_json::from_str(&json).ok().map(|track| RecentTrack {
                    id,
                    guild_id,
                    track,
                    requested_by_user_id: requester.map(|id| id as u64),
                    requested_by_display: display,
                    played_utc: played,
                    end_reason: reason,
                })
            })
            .collect()
    }

    // ───────── 채팅 ─────────

    pub fn add_chat_message(
        &self,
        guild_id: u64,
        user_id: u64,
        display_name: &str,
        avatar_url: Option<&str>,
        content: &str,
        reply_to_message_id: Option<i64>,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        // 답장 대상이 같은 길드에 없으면 인용을 버린다 — 남의 서버 메시지를 끌어오지 못하게.
        let reply_to = reply_to_message_id.filter(|id| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM remote_chat_messages WHERE guild_id = ?1 AND id = ?2)",
                params![guild_id as i64, id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false)
        });
        conn.execute(
            r#"INSERT INTO remote_chat_messages
               (guild_id, user_id, display_name, avatar_url, content, created_utc, reply_to_message_id)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![guild_id as i64, user_id as i64, display_name, avatar_url, content, Self::now_iso(), reply_to],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 본인 메시지 수정. 내용과 함께 `edited_utc`를 남긴다.
    pub fn edit_chat_message(
        &self,
        guild_id: u64,
        message_id: i64,
        user_id: u64,
        content: &str,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            r#"UPDATE remote_chat_messages SET content = ?4, edited_utc = ?5
               WHERE guild_id = ?1 AND id = ?2 AND user_id = ?3 AND deleted_utc IS NULL"#,
            params![
                guild_id as i64,
                message_id,
                user_id as i64,
                content,
                Self::now_iso()
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_chat_message(&self, guild_id: u64, message_id: i64) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE remote_chat_messages SET content = '', deleted_utc = ?3 WHERE guild_id = ?1 AND id = ?2 AND deleted_utc IS NULL",
            params![guild_id as i64, message_id, Self::now_iso()],
        )?;
        Ok(changed > 0)
    }

    pub fn chat_message_owner(&self, guild_id: u64, message_id: i64) -> Option<u64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id FROM remote_chat_messages WHERE guild_id = ?1 AND id = ?2",
            params![guild_id as i64, message_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|id| id as u64)
    }

    pub fn toggle_chat_reaction(
        &self,
        guild_id: u64,
        message_id: i64,
        user_id: u64,
        emoji: &str,
    ) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let belongs: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_chat_messages WHERE guild_id = ?1 AND id = ?2 AND deleted_utc IS NULL)",
            params![guild_id as i64, message_id], |row| row.get(0),
        )?;
        if !belongs {
            return Ok(false);
        }
        let removed = tx.execute(
            "DELETE FROM remote_chat_reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
            params![message_id, user_id as i64, emoji],
        )?;
        let active = if removed > 0 {
            false
        } else {
            tx.execute(
                "INSERT INTO remote_chat_reactions(message_id, user_id, emoji, created_utc) VALUES(?1, ?2, ?3, ?4)",
                params![message_id, user_id as i64, emoji, Self::now_iso()],
            )?;
            true
        };
        tx.commit()?;
        Ok(active)
    }

    /// 최신 `limit`건을 오래된 순으로 돌려준다. `before_id`가 있으면 그보다 과거만 본다.
    /// 반응·태그·멘션·인용은 메시지 수와 무관하게 각각 한 번씩만 조회한다 (N+1 제거).
    pub fn list_chat_messages(
        &self,
        guild_id: u64,
        user_id: u64,
        limit: usize,
        before_id: Option<i64>,
    ) -> Vec<ChatMessage> {
        let limit = limit.clamp(1, CHAT_PAGE_MAX);
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT id, user_id, display_name, avatar_url, content, created_utc, deleted_utc,
                      edited_utc, reply_to_message_id
               FROM remote_chat_messages
               WHERE guild_id = ?1 AND (?2 IS NULL OR id < ?2)
               ORDER BY id DESC LIMIT ?3"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let raw: Vec<ChatRow> = match statement.query_map(
            params![guild_id as i64, before_id, limit as i64],
            |row| {
                Ok(ChatRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    display_name: row.get(2)?,
                    avatar_url: row.get(3)?,
                    content: row.get(4)?,
                    created_utc: row.get(5)?,
                    deleted_utc: row.get(6)?,
                    edited_utc: row.get(7)?,
                    reply_to: row.get(8)?,
                })
            },
        ) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => return Vec::new(),
        };
        drop(statement);
        if raw.is_empty() {
            return Vec::new();
        }

        let ids: Vec<i64> = raw.iter().map(|row| row.id).collect();
        let mut reactions = load_reactions(&conn, &ids, user_id);
        let mut tags = load_tags(&conn, &ids);
        let mut mentions = load_mentions(&conn, &ids);
        // 인용 원문은 이번 페이지 밖일 수 있으므로 대상 id로 따로 한 번 더 조회한다.
        let reply_ids: Vec<i64> = raw.iter().filter_map(|row| row.reply_to).collect();
        let replies = load_reply_previews(&conn, guild_id, &reply_ids);

        raw.into_iter()
            .rev()
            .map(|row| ChatMessage {
                id: row.id,
                guild_id,
                user_id: row.user_id as u64,
                display_name: row.display_name,
                avatar_url: row.avatar_url,
                content: row.content,
                created_utc: row.created_utc,
                deleted_utc: row.deleted_utc,
                edited_utc: row.edited_utc,
                reactions: reactions.remove(&row.id).unwrap_or_default(),
                reply_to: row.reply_to.and_then(|id| replies.get(&id).cloned()),
                mentions: mentions.remove(&row.id).unwrap_or_default(),
                tags: tags.remove(&row.id).unwrap_or_default(),
            })
            .collect()
    }

    /// 방금 쓴 메시지 하나만 다시 읽는다. WS `chat.add` payload 용.
    pub fn get_chat_message(
        &self,
        guild_id: u64,
        message_id: i64,
        viewer_user_id: u64,
    ) -> Option<ChatMessage> {
        self.list_chat_messages(guild_id, viewer_user_id, 1, Some(message_id + 1))
            .into_iter()
            .find(|message| message.id == message_id)
    }

    /// 이 메시지가 부른 사람들을 기록한다. 같은 메시지를 다시 저장하면 통째로 교체한다.
    pub fn set_chat_mentions(
        &self,
        guild_id: u64,
        message_id: i64,
        user_ids: &[u64],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM remote_chat_mentions WHERE message_id = ?1",
            params![message_id],
        )?;
        for user_id in user_ids {
            tx.execute(
                "INSERT OR IGNORE INTO remote_chat_mentions(message_id, guild_id, user_id, read_utc) VALUES(?1, ?2, ?3, NULL)",
                params![message_id, guild_id as i64, *user_id as i64],
            )?;
        }
        tx.commit()
    }

    /// 메시지에 붙은 노래 태그를 기록한다.
    pub fn set_chat_tags(&self, message_id: i64, tags: &[ChatTrackTag]) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM remote_chat_tags WHERE message_id = ?1",
            params![message_id],
        )?;
        for tag in tags {
            let payload = serde_json::to_string(&tag.track).unwrap_or_else(|_| "{}".into());
            tx.execute(
                "INSERT OR REPLACE INTO remote_chat_tags(message_id, cache_key, track_json) VALUES(?1, ?2, ?3)",
                params![message_id, tag.cache_key, payload],
            )?;
        }
        tx.commit()
    }

    pub fn unread_mention_count(&self, guild_id: u64, user_id: u64) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM remote_chat_mentions WHERE guild_id = ?1 AND user_id = ?2 AND read_utc IS NULL",
            params![guild_id as i64, user_id as i64],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    pub fn mark_mentions_read(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE remote_chat_mentions SET read_utc = ?3 WHERE guild_id = ?1 AND user_id = ?2 AND read_utc IS NULL",
            params![guild_id as i64, user_id as i64, Self::now_iso()],
        )
    }

    /// 이 서버에서 리모컨을 써 본 사람 목록(채팅했거나 곡을 신청한 사람). 최근 활동순.
    /// 표시 이름·아바타는 채팅 테이블의 최신값이고, 채팅 기록이 없으면 빈 문자열이다.
    pub fn list_remote_participants(&self, guild_id: u64) -> Vec<Participant> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"WITH activity AS (
                   SELECT user_id AS uid, MAX(created_utc) AS last_utc
                   FROM remote_chat_messages WHERE guild_id = ?1 GROUP BY user_id
                   UNION ALL
                   SELECT requester_user_id AS uid, MAX(updated_utc) AS last_utc
                   FROM remote_queue_scores
                   WHERE guild_id = ?1 AND requester_user_id IS NOT NULL
                   GROUP BY requester_user_id
               )
               SELECT a.uid, MAX(a.last_utc) AS last_utc,
                      COALESCE((SELECT m.display_name FROM remote_chat_messages m
                                WHERE m.guild_id = ?1 AND m.user_id = a.uid
                                ORDER BY m.id DESC LIMIT 1), '') AS display_name,
                      (SELECT m.avatar_url FROM remote_chat_messages m
                       WHERE m.guild_id = ?1 AND m.user_id = a.uid
                       ORDER BY m.id DESC LIMIT 1) AS avatar_url
               FROM activity a
               GROUP BY a.uid
               ORDER BY last_utc DESC"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64], |row| {
                Ok(Participant {
                    user_id: row.get::<_, i64>(0)? as u64,
                    last_active_utc: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    display_name: row.get(2)?,
                    avatar_url: row.get(3)?,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn report_chat_message(
        &self,
        guild_id: u64,
        message_id: i64,
        reporter_user_id: u64,
        reporter_display_name: &str,
        reason: &str,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_chat_messages WHERE guild_id = ?1 AND id = ?2 AND deleted_utc IS NULL)",
            params![guild_id as i64, message_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        conn.execute(
            r#"INSERT INTO remote_chat_reports
               (guild_id, message_id, reporter_user_id, reporter_display_name, reason, created_utc)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6)
               ON CONFLICT(message_id, reporter_user_id) DO UPDATE SET
                 reason = excluded.reason, created_utc = excluded.created_utc, resolved_utc = NULL"#,
            params![
                guild_id as i64,
                message_id,
                reporter_user_id as i64,
                reporter_display_name,
                reason,
                Self::now_iso(),
            ],
        )?;
        Ok(true)
    }

    pub fn resolve_chat_report(&self, guild_id: u64, report_id: i64) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE remote_chat_reports SET resolved_utc = ?3 WHERE guild_id = ?1 AND id = ?2 AND resolved_utc IS NULL",
            params![guild_id as i64, report_id, Self::now_iso()],
        )?;
        Ok(changed > 0)
    }

    pub fn list_chat_reports(&self, guild_id: u64, limit: usize) -> Vec<ChatReport> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT r.id, r.message_id, r.reporter_user_id, r.reporter_display_name, r.reason,
                      m.content, m.display_name, r.created_utc, r.resolved_utc
               FROM remote_chat_reports r
               JOIN remote_chat_messages m ON m.id = r.message_id
               WHERE r.guild_id = ?1
               ORDER BY CASE WHEN r.resolved_utc IS NULL THEN 0 ELSE 1 END, r.id DESC
               LIMIT ?2"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64, limit as i64], |row| {
                Ok(ChatReport {
                    id: row.get(0)?,
                    guild_id,
                    message_id: row.get(1)?,
                    reporter_user_id: row.get::<_, i64>(2)? as u64,
                    reporter_display_name: row.get(3)?,
                    reason: row.get(4)?,
                    message_content: row.get(5)?,
                    message_author: row.get(6)?,
                    created_utc: row.get(7)?,
                    resolved_utc: row.get(8)?,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    // ───────── 제안 게시판 ─────────

    pub fn create_suggestion(
        &self,
        guild_id: u64,
        user_id: u64,
        display_name: &str,
        avatar_url: Option<&str>,
        title: &str,
        body: &str,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_suggestions
               (guild_id, user_id, display_name, avatar_url, title, body, status, created_utc)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7)"#,
            params![
                guild_id as i64,
                user_id as i64,
                display_name,
                avatar_url,
                title,
                body,
                Self::now_iso()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 공감수와 "내가 공감했는지"를 같이 돌려준다. 공감 많은 순 → 최신순.
    pub fn list_suggestions(&self, guild_id: u64, viewer_user_id: u64) -> Vec<Suggestion> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(SUGGESTION_SELECT) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64, viewer_user_id as i64], |row| {
                map_suggestion(row, guild_id)
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn get_suggestion(
        &self,
        guild_id: u64,
        suggestion_id: i64,
        viewer_user_id: u64,
    ) -> Option<Suggestion> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            SUGGESTION_SELECT_ONE,
            params![guild_id as i64, viewer_user_id as i64, suggestion_id],
            |row| map_suggestion(row, guild_id),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 공감 토글. 제안이 없으면 `None`, 있으면 토글 후 상태를 돌려준다.
    pub fn toggle_suggestion_vote(
        &self,
        guild_id: u64,
        suggestion_id: i64,
        user_id: u64,
    ) -> rusqlite::Result<Option<bool>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_suggestions WHERE guild_id = ?1 AND id = ?2 AND deleted_utc IS NULL)",
            params![guild_id as i64, suggestion_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let removed = tx.execute(
            "DELETE FROM remote_suggestion_votes WHERE suggestion_id = ?1 AND user_id = ?2",
            params![suggestion_id, user_id as i64],
        )?;
        let active = if removed > 0 {
            false
        } else {
            tx.execute(
                "INSERT INTO remote_suggestion_votes(suggestion_id, user_id, created_utc) VALUES(?1, ?2, ?3)",
                params![suggestion_id, user_id as i64, Self::now_iso()],
            )?;
            true
        };
        tx.commit()?;
        Ok(Some(active))
    }

    pub fn set_suggestion_status(
        &self,
        guild_id: u64,
        suggestion_id: i64,
        status: SuggestionStatus,
        note: Option<&str>,
        by_user_id: u64,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            r#"UPDATE remote_suggestions
               SET status = ?3, status_note = ?4, status_by_user_id = ?5, status_utc = ?6
               WHERE guild_id = ?1 AND id = ?2 AND deleted_utc IS NULL"#,
            params![
                guild_id as i64,
                suggestion_id,
                status.as_str(),
                note,
                by_user_id as i64,
                Self::now_iso()
            ],
        )?;
        Ok(changed > 0)
    }

    /// 소프트 삭제. 공감 기록은 같이 지운다.
    pub fn delete_suggestion(&self, guild_id: u64, suggestion_id: i64) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE remote_suggestions SET deleted_utc = ?3 WHERE guild_id = ?1 AND id = ?2 AND deleted_utc IS NULL",
            params![guild_id as i64, suggestion_id, Self::now_iso()],
        )?;
        tx.execute(
            "DELETE FROM remote_suggestion_votes WHERE suggestion_id = ?1",
            params![suggestion_id],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }

    // ───────── 유저 정지 ─────────

    #[allow(clippy::too_many_arguments)]
    pub fn suspend_user(
        &self,
        guild_id: u64,
        user_id: u64,
        scope: SuspensionScope,
        reason: Option<&str>,
        by_user_id: u64,
        expires_utc: Option<&str>,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_user_suspensions
               (guild_id, user_id, scope, reason, by_user_id, created_utc, expires_utc)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(guild_id, user_id, scope) DO UPDATE SET
                 reason = excluded.reason, by_user_id = excluded.by_user_id,
                 created_utc = excluded.created_utc, expires_utc = excluded.expires_utc"#,
            params![
                guild_id as i64,
                user_id as i64,
                scope.as_str(),
                reason,
                by_user_id as i64,
                Self::now_iso(),
                expires_utc,
            ],
        )?;
        Ok(())
    }

    /// `scope`가 없으면 그 사람의 정지를 전부 푼다.
    pub fn unsuspend_user(
        &self,
        guild_id: u64,
        user_id: u64,
        scope: Option<SuspensionScope>,
    ) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        match scope {
            Some(scope) => conn.execute(
                "DELETE FROM remote_user_suspensions WHERE guild_id = ?1 AND user_id = ?2 AND scope = ?3",
                params![guild_id as i64, user_id as i64, scope.as_str()],
            ),
            None => conn.execute(
                "DELETE FROM remote_user_suspensions WHERE guild_id = ?1 AND user_id = ?2",
                params![guild_id as i64, user_id as i64],
            ),
        }
    }

    /// 아직 살아 있는 정지만. 만료된 행은 이 자리에서 지운다(지연 삭제).
    pub fn active_suspensions(&self, guild_id: u64, user_id: u64) -> Vec<Suspension> {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"DELETE FROM remote_user_suspensions
               WHERE guild_id = ?1 AND user_id = ?2
                 AND expires_utc IS NOT NULL AND julianday(expires_utc) <= julianday('now')"#,
            params![guild_id as i64, user_id as i64],
        );
        let mut statement = match conn.prepare(
            r#"SELECT user_id, scope, reason, by_user_id, created_utc, expires_utc
               FROM remote_user_suspensions
               WHERE guild_id = ?1 AND user_id = ?2"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64, user_id as i64], |row| {
                map_suspension(row, guild_id)
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// 관리 콘솔용 전체 목록. 만료된 것도 정리한 뒤 돌려준다.
    pub fn list_suspensions(&self, guild_id: u64) -> Vec<Suspension> {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"DELETE FROM remote_user_suspensions
               WHERE guild_id = ?1 AND expires_utc IS NOT NULL
                 AND julianday(expires_utc) <= julianday('now')"#,
            params![guild_id as i64],
        );
        let mut statement = match conn.prepare(
            r#"SELECT user_id, scope, reason, by_user_id, created_utc, expires_utc
               FROM remote_user_suspensions WHERE guild_id = ?1
               ORDER BY created_utc DESC"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64], |row| map_suspension(row, guild_id))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    // ───────── 세션 영속화 ─────────

    /// 토큰 원문은 저장하지 않는다. 해시만 PK로 남긴다.
    pub fn save_session(&self, token: &str, session: &StoredSession) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_web_sessions
               (token_hash, user_id, display_name, avatar_url, guilds_json,
                access_token, refresh_token, expires_utc, refreshed_utc, created_utc, csrf_token)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
               ON CONFLICT(token_hash) DO UPDATE SET
                 user_id = excluded.user_id, display_name = excluded.display_name,
                 avatar_url = excluded.avatar_url, guilds_json = excluded.guilds_json,
                 access_token = excluded.access_token, refresh_token = excluded.refresh_token,
                 expires_utc = excluded.expires_utc, refreshed_utc = excluded.refreshed_utc,
                 csrf_token = excluded.csrf_token"#,
            params![
                Self::session_token_hash(token),
                session.user_id as i64,
                session.display_name,
                session.avatar_url,
                session.guilds_json,
                session.access_token,
                session.refresh_token,
                session.expires_utc,
                session.refreshed_utc,
                session.created_utc,
                session.csrf_token,
            ],
        )?;
        Ok(())
    }

    /// 만료된 세션은 없는 것으로 친다.
    pub fn load_session(&self, token: &str) -> Option<StoredSession> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"SELECT user_id, display_name, avatar_url, guilds_json,
                      access_token, refresh_token, expires_utc, refreshed_utc, created_utc,
                      csrf_token
               FROM remote_web_sessions
               WHERE token_hash = ?1 AND julianday(expires_utc) > julianday('now')"#,
            params![Self::session_token_hash(token)],
            |row| {
                Ok(StoredSession {
                    user_id: row.get::<_, i64>(0)? as u64,
                    display_name: row.get(1)?,
                    avatar_url: row.get(2)?,
                    guilds_json: row.get(3)?,
                    access_token: row.get(4)?,
                    refresh_token: row.get(5)?,
                    expires_utc: row.get(6)?,
                    refreshed_utc: row.get(7)?,
                    created_utc: row.get(8)?,
                    csrf_token: row.get(9)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn delete_session(&self, token: &str) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM remote_web_sessions WHERE token_hash = ?1",
            params![Self::session_token_hash(token)],
        )?;
        Ok(changed > 0)
    }

    pub fn prune_sessions(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM remote_web_sessions WHERE julianday(expires_utc) <= julianday('now')",
            [],
        )
    }

    // ───────── 개인 설정 ─────────

    /// 이 사람이 저장해 둔 개인 설정 전부. 기본값은 채우지 않는다 —
    /// "한 번도 고른 적 없음"(예: `layout` 없음)을 화면이 구분해야 첫 진입 시트를 띄울 수 있다.
    pub fn load_prefs(&self, user_id: u64) -> BTreeMap<String, String> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn
            .prepare("SELECT key, value FROM remote_user_prefs WHERE user_id = ?1")
        {
            Ok(statement) => statement,
            Err(_) => return BTreeMap::new(),
        };
        let rows = match statement.query_map(params![user_id as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            Ok(rows) => rows,
            Err(_) => return BTreeMap::new(),
        };
        // 화이트리스트가 바뀌어 옛 키가 남아 있을 수 있으니 읽을 때도 한 번 거른다.
        rows.flatten()
            .filter(|(key, value)| is_valid_pref(key, value))
            .collect()
    }

    /// 부분 갱신. 모르는 키와 상한을 넘는 값은 저장하지 않는다.
    /// 400을 돌려주고 싶으면 호출부가 먼저 `is_valid_pref`로 걸러라 — 여기서는 조용히 버린다.
    pub fn save_prefs(
        &self,
        user_id: u64,
        updates: &BTreeMap<String, String>,
    ) -> rusqlite::Result<()> {
        let accepted: Vec<(&String, &String)> = updates
            .iter()
            .filter(|(key, value)| is_valid_pref(key, value))
            .collect();
        if accepted.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = Self::now_iso();
        for (key, value) in accepted {
            tx.execute(
                r#"INSERT INTO remote_user_prefs(user_id, key, value, updated_utc)
                   VALUES(?1, ?2, ?3, ?4)
                   ON CONFLICT(user_id, key) DO UPDATE SET
                     value = excluded.value, updated_utc = excluded.updated_utc"#,
                params![user_id as i64, key, value, now],
            )?;
        }
        tx.commit()
    }

    /// "기본으로 되돌리기"용. 지운 키는 다시 미선택 상태가 된다.
    pub fn delete_prefs(&self, user_id: u64, keys: &[&str]) -> rusqlite::Result<usize> {
        if keys.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut removed = 0;
        for key in keys {
            removed += tx.execute(
                "DELETE FROM remote_user_prefs WHERE user_id = ?1 AND key = ?2",
                params![user_id as i64, key],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    // ───────── 자동 재생 기준 곡 ─────────

    /// 등록된 기준 곡을 정렬 순서대로. 추천 엔진이 이 순서를 라운드로빈으로 돈다.
    pub fn list_autoplay_seeds(&self, guild_id: u64) -> Vec<AutoplaySeed> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT cache_key, track_json, sort_order, added_by_user_id, added_utc
               FROM remote_autoplay_seeds WHERE guild_id = ?1
               ORDER BY sort_order, cache_key"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map(params![guild_id as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.flatten()
            .filter_map(|(cache_key, json, sort_order, added_by, added_utc)| {
                serde_json::from_str::<TrackRef>(&json)
                    .ok()
                    .map(|track| AutoplaySeed {
                        guild_id,
                        cache_key,
                        track,
                        sort_order,
                        added_by_user_id: added_by as u64,
                        added_utc,
                    })
            })
            .collect()
    }

    /// 기준 곡 추가. 상한과 중복은 여기서 막는다 — 라우트마다 다시 세면 어긋난다.
    /// 상한은 길드 설정 `autoplay_seed_max`이고 `0`이면 무제한이다(§23.1).
    pub fn add_autoplay_seed(
        &self,
        guild_id: u64,
        track: &TrackRef,
        added_by_user_id: u64,
    ) -> rusqlite::Result<SeedAddOutcome> {
        // 설정을 먼저 읽는다 — 같은 뮤텍스라 커넥션을 잡은 뒤에 읽으면 교착한다.
        let seed_limit = self.load_guild_settings(guild_id).seed_limit();
        let cache_key = track.cache_key();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_autoplay_seeds WHERE guild_id = ?1 AND cache_key = ?2)",
            params![guild_id as i64, cache_key],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(SeedAddOutcome::Duplicate);
        }
        if let Some(limit) = seed_limit {
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM remote_autoplay_seeds WHERE guild_id = ?1",
                params![guild_id as i64],
                |row| row.get(0),
            )?;
            if count as u32 >= limit {
                return Ok(SeedAddOutcome::LimitReached(limit));
            }
        }
        let next_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM remote_autoplay_seeds WHERE guild_id = ?1",
            params![guild_id as i64],
            |row| row.get(0),
        )?;
        tx.execute(
            r#"INSERT INTO remote_autoplay_seeds
               (guild_id, cache_key, track_json, sort_order, added_by_user_id, added_utc)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                guild_id as i64,
                cache_key,
                serde_json::to_string(track).unwrap_or_else(|_| "{}".into()),
                next_order,
                added_by_user_id as i64,
                Self::now_iso(),
            ],
        )?;
        tx.commit()?;
        Ok(SeedAddOutcome::Added)
    }

    /// 없는 곡을 지우려 하면 `false` — 라우트가 404를 줄 수 있게.
    pub fn remove_autoplay_seed(&self, guild_id: u64, cache_key: &str) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM remote_autoplay_seeds WHERE guild_id = ?1 AND cache_key = ?2",
            params![guild_id as i64, cache_key],
        )?;
        Ok(changed > 0)
    }

    /// 드래그 정렬 결과 반영. 목록에 빠진 곡은 지우지 않고 뒤로 밀어 둔다
    /// (다른 탭에서 그사이 추가된 곡이 사라지면 안 된다).
    pub fn reorder_autoplay_seeds(
        &self,
        guild_id: u64,
        cache_keys: &[String],
    ) -> rusqlite::Result<()> {
        if cache_keys.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let current: Vec<String> = {
            let mut statement = tx.prepare(
                "SELECT cache_key FROM remote_autoplay_seeds WHERE guild_id = ?1 ORDER BY sort_order, cache_key",
            )?;
            let rows = statement.query_map(params![guild_id as i64], |row| row.get::<_, String>(0))?;
            rows.flatten().collect()
        };
        // 요청 순서 중 실제로 있는 곡만 앞에, 목록에 없던 곡은 원래 순서 그대로 뒤에.
        let mut ordered: Vec<&String> = Vec::with_capacity(current.len());
        for cache_key in cache_keys {
            if current.contains(cache_key) && !ordered.iter().any(|key| *key == cache_key) {
                ordered.push(cache_key);
            }
        }
        for cache_key in &current {
            if !ordered.iter().any(|key| *key == cache_key) {
                ordered.push(cache_key);
            }
        }
        for (order, cache_key) in ordered.into_iter().enumerate() {
            tx.execute(
                "UPDATE remote_autoplay_seeds SET sort_order = ?3 WHERE guild_id = ?1 AND cache_key = ?2",
                params![guild_id as i64, cache_key, order as i64],
            )?;
        }
        tx.commit()
    }

    // ───────── 활동 로그 ─────────

    /// 활동 로그 한 줄. **분류와 사람 문장은 서버가 정한다** (§13.3·§13.5) —
    /// 클라이언트가 액션명을 문장으로 바꾸는 로직을 갖지 않게 하려는 것이다.
    /// 같은 사람이 같은 종류를 60초 안에 다시 하면 새 줄을 만들지 않고 기존 줄을 갱신한다(§13.3).
    #[allow(clippy::too_many_arguments)]
    pub fn add_audit(
        &self,
        guild_id: u64,
        user_id: u64,
        display_name: &str,
        action: &str,
        target: Option<&str>,
        before_value: Option<&str>,
        after_value: Option<&str>,
        success: bool,
        failure_reason: Option<&str>,
    ) -> rusqlite::Result<i64> {
        self.write_audit(
            guild_id,
            user_id,
            display_name,
            action,
            target,
            before_value,
            after_value,
            success,
            failure_reason,
            1,
        )
    }

    /// 재생목록·차트처럼 **한 번에 여러 곡**이 들어간 일 (§13.3).
    /// 사람 피드에는 `…에서 50곡을 담았어요` 한 줄만 남는다.
    pub fn add_audit_bulk(
        &self,
        guild_id: u64,
        user_id: u64,
        display_name: &str,
        action: &str,
        label: Option<&str>,
        count: u32,
        items: &[String],
    ) -> rusqlite::Result<i64> {
        let id = self.write_audit(
            guild_id,
            user_id,
            display_name,
            action,
            label,
            None,
            None,
            true,
            None,
            count.max(1),
        )?;
        if !items.is_empty() {
            // 펼치면 무엇이 들어갔는지 보여야 한다. 숫자만 남기면 "뭘 넣은 거지?"가 남는다.
            let trimmed: Vec<String> = items.iter().take(200).map(|it| truncate_title(it)).collect();
            let json = serde_json::to_string(&trimmed).unwrap_or_else(|_| "[]".into());
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_audit_logs SET merged_items_json = ?2 WHERE id = ?1",
                params![id, json],
            )?;
        }
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_audit(
        &self,
        guild_id: u64,
        user_id: u64,
        display_name: &str,
        action: &str,
        target: Option<&str>,
        before_value: Option<&str>,
        after_value: Option<&str>,
        success: bool,
        failure_reason: Option<&str>,
        count: u32,
    ) -> rusqlite::Result<i64> {
        let kind = audit_kind_for(action);
        let conn = self.conn.lock().unwrap();
        let now = Self::now_iso();

        // ── 합치기 (§13.3). 실패한 시도는 합치지 않는다 — 관리자가 각각을 봐야 한다.
        if success && is_mergeable_action(action) {
            let cutoff = (Utc::now() - ChronoDuration::seconds(AUDIT_MERGE_WINDOW_SECS)).to_rfc3339();
            let previous: Option<(i64, i64, Option<String>, Option<String>)> = conn
                .query_row(
                    r#"SELECT id, merged_count, merged_items_json, target
                       FROM remote_audit_logs
                       WHERE guild_id = ?1 AND user_id = ?2 AND action = ?3 AND success = 1
                         AND julianday(created_utc) >= julianday(?4)
                       ORDER BY id DESC LIMIT 1"#,
                    params![guild_id as i64, user_id as i64, action, cutoff],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            if let Some((id, merged_count, items_json, first_target)) = previous {
                let mut items: Vec<String> = items_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or_else(|| first_target.iter().map(|t| truncate_title(t)).collect());
                if let Some(title) = target {
                    items.push(truncate_title(title));
                }
                items.truncate(200);
                let merged_count = (merged_count.max(1) as u32).saturating_add(count);
                let text = audit_text(
                    action,
                    display_name,
                    target.or(first_target.as_deref()),
                    before_value,
                    after_value,
                    merged_count,
                );
                conn.execute(
                    r#"UPDATE remote_audit_logs
                       SET merged_count = ?2, merged_items_json = ?3, human_text = ?4, created_utc = ?5
                       WHERE id = ?1"#,
                    params![
                        id,
                        merged_count as i64,
                        serde_json::to_string(&items).unwrap_or_else(|_| "[]".into()),
                        text,
                        now
                    ],
                )?;
                return Ok(id);
            }
        }

        let text = audit_text(
            action,
            display_name,
            target,
            before_value,
            after_value,
            count,
        );
        conn.execute(
            r#"INSERT INTO remote_audit_logs
               (guild_id, user_id, display_name, action, kind, human_text, target,
                before_value, after_value, success, failure_reason, created_utc, merged_count)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
            params![
                guild_id as i64,
                user_id as i64,
                display_name,
                action,
                kind.as_str(),
                text,
                target,
                before_value,
                after_value,
                success as i64,
                failure_reason,
                now,
                count.max(1) as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// `before_id`가 있으면 그보다 과거만. 활동 로그 탭의 커서 페이지네이션이다.
    /// 분류 필터가 필요하면 `list_audit_kinds`를 쓴다.
    pub fn list_audit(
        &self,
        guild_id: u64,
        limit: usize,
        before_id: Option<i64>,
    ) -> Vec<AuditEntry> {
        self.list_audit_kinds(guild_id, limit, before_id, &[])
    }

    /// 분류 필터(§13.5). `kinds`가 비어 있으면 전부 본다.
    pub fn list_audit_kinds(
        &self,
        guild_id: u64,
        limit: usize,
        before_id: Option<i64>,
        kinds: &[AuditKind],
    ) -> Vec<AuditEntry> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            r#"SELECT id, user_id, display_name, action, kind, human_text, target,
                      before_value, after_value, success, failure_reason, created_utc,
                      merged_count, merged_items_json
               FROM remote_audit_logs
               WHERE guild_id = ?1 AND (?2 IS NULL OR id < ?2)"#,
        );
        let mut binds: Vec<SqlValue> = vec![
            SqlValue::Integer(guild_id as i64),
            match before_id {
                Some(id) => SqlValue::Integer(id),
                None => SqlValue::Null,
            },
        ];
        if !kinds.is_empty() {
            sql.push_str(&format!(" AND kind IN ({})", placeholders(kinds.len())));
            binds.extend(kinds.iter().map(|kind| SqlValue::Text(kind.as_str().into())));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        binds.push(SqlValue::Integer(limit as i64));

        let mut statement = match conn.prepare(&sql) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params_from_iter(binds), |row| {
                let action: String = row.get(3)?;
                let kind: Option<String> = row.get(4)?;
                let stored_text: Option<String> = row.get(5)?;
                let target: Option<String> = row.get(6)?;
                let before_value: Option<String> = row.get(7)?;
                let after_value: Option<String> = row.get(8)?;
                let display_name: String = row.get(2)?;
                let merged_count: i64 = row.get::<_, Option<i64>>(12)?.unwrap_or(1).max(1);
                let items: Vec<String> = row
                    .get::<_, Option<String>>(13)?
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or_default();
                // v13 이전에 쌓인 줄에는 문장이 없다 — 읽는 자리에서 만들어 준다.
                let text = stored_text.filter(|t| !t.is_empty()).unwrap_or_else(|| {
                    audit_text(
                        &action,
                        &display_name,
                        target.as_deref(),
                        before_value.as_deref(),
                        after_value.as_deref(),
                        merged_count as u32,
                    )
                });
                Ok(AuditEntry {
                    id: row.get(0)?,
                    guild_id,
                    user_id: row.get::<_, i64>(1)? as u64,
                    kind: kind
                        .as_deref()
                        .and_then(AuditKind::parse)
                        .unwrap_or_else(|| audit_kind_for(&action)),
                    display_name,
                    action,
                    text,
                    target,
                    before_value,
                    after_value,
                    success: row.get::<_, i64>(9)? != 0,
                    failure_reason: row.get(10)?,
                    created_utc: row.get(11)?,
                    merged_count: merged_count as u32,
                    merged_items: items,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// created_utc 는 RFC3339(`...T...+00:00`)라 문자열로 `datetime('now')`(`... ...`)와 비교하면
    /// 'T' > ' ' 때문에 아무것도 안 지워진다. julianday 로 실제 시각을 비교한다.
    ///
    /// **분류별 보존**(§13.6): 투표·재생은 3일, 나머지는 설정값 그대로.
    /// `retention_days == 0`이면 무제한이라 **한 줄도 지우지 않는다**(§23.1).
    pub fn prune_audit(&self, guild_id: u64, retention_days: i32) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        prune_audit_with(&conn, guild_id, retention_days)
    }

    // ───────── 보존 정리 ─────────

    /// 기동 시 + 하루 1회 부른다. 길드 설정이 있으면 길드 설정이 이긴다.
    ///
    /// `remote_user_prefs`와 `remote_autoplay_seeds`는 **건드리지 않는다.**
    /// 화면 배치와 기준 곡은 사용자가 직접 지우기 전까지 오래 남아 있어야 한다
    /// (한 달 뒤에 들어왔더니 배치가 초기화돼 있으면 그건 그냥 고장이다).
    pub fn prune_all(&self, retention: RetentionConfig) -> rusqlite::Result<PruneReport> {
        // load_guild_settings 가 같은 뮤텍스를 잡으므로 설정은 먼저 다 읽어 둔다.
        let guild_ids = self.remote_guild_ids();
        let plans: Vec<(u64, u32, i32)> = guild_ids
            .into_iter()
            .map(|guild_id| {
                let settings = self.load_guild_settings(guild_id);
                // "길드 설정이 있으면 길드 설정이 이긴다." 한 번도 저장한 적 없는 길드만
                // 앱 기본값을 쓴다 — 저장된 0은 "일부러 무제한"이라 기본값으로 덮으면 안 된다.
                if self.has_guild_settings(guild_id) {
                    (
                        guild_id,
                        settings.chat_retention_days,
                        settings.audit_retention_days,
                    )
                } else {
                    (guild_id, retention.chat_days, retention.audit_days)
                }
            })
            .collect();

        let mut report = PruneReport::default();
        let conn = self.conn.lock().unwrap();
        for (guild_id, chat_days, audit_days) in plans {
            // 0 = 무제한(§23.1). 예전 코드의 `.clamp(1, ..)` 는 0을 1일로 바꿔 버려서
            // "무제한"을 고른 서버의 채팅이 하루 만에 사라졌다.
            // (설정을 한 번도 안 만진 길드는 필드 기본값 30이 오므로 0은 "일부러 무제한"뿐이다.)
            if let Some(chat_days) = as_limit_u32(chat_days) {
                report.chat += conn.execute(
                    r#"DELETE FROM remote_chat_messages
                       WHERE guild_id = ?1 AND julianday(created_utc) < julianday('now', ?2)"#,
                    params![guild_id as i64, format!("-{} days", chat_days.min(3650))],
                )?;
            }
            report.audit += prune_audit_with(&conn, guild_id, audit_days)?;
            report.recent += conn.execute(
                r#"DELETE FROM remote_recent_tracks
                   WHERE guild_id = ?1 AND id NOT IN (
                       SELECT id FROM remote_recent_tracks WHERE guild_id = ?1
                       ORDER BY id DESC LIMIT ?2)"#,
                params![guild_id as i64, retention.recent_keep as i64],
            )?;
        }
        // 채팅 삭제로 남은 고아 행 (멘션·태그에는 FK가 없다).
        conn.execute(
            "DELETE FROM remote_chat_mentions WHERE message_id NOT IN (SELECT id FROM remote_chat_messages)",
            [],
        )?;
        conn.execute(
            "DELETE FROM remote_chat_tags WHERE message_id NOT IN (SELECT id FROM remote_chat_messages)",
            [],
        )?;
        report.lyrics = conn.execute(
            r#"DELETE FROM remote_lyrics
               WHERE found = 0 AND julianday(fetched_utc) < julianday('now', ?1)"#,
            params![format!(
                "-{} days",
                retention.lyrics_failure_days.clamp(1, 3650)
            )],
        )?;
        report.sessions = conn.execute(
            "DELETE FROM remote_web_sessions WHERE julianday(expires_utc) <= julianday('now')",
            [],
        )?;
        // 기한이 지난 자동재생 차단(§8.5-3)과 지난 날짜의 슈퍼 좋아요 사용량(§10.6).
        // 둘 다 오래된 행이 남아 있어도 동작에는 영향이 없지만 계속 불어나서 같이 턴다.
        conn.execute(
            "DELETE FROM remote_autoplay_blocked WHERE julianday(until_utc) <= julianday('now')",
            [],
        )?;
        conn.execute(
            "DELETE FROM remote_super_like_usage WHERE day < ?1",
            params![(Utc::now() - ChronoDuration::days(7))
                .format("%Y-%m-%d")
                .to_string()],
        )?;
        Ok(report)
    }

    // ───────── 슈퍼 좋아요 제한 (§10.6) ─────────

    /// 지금 슈퍼 좋아요를 쓸 수 있는지 본다. **소비하지 않는다.**
    /// 관리자·봇 주인도 똑같이 적용된다 — 여기서 예외를 두면 그게 특혜다.
    pub fn check_super_like(
        &self,
        guild_id: u64,
        user_id: u64,
        cooldown_sec: u32,
        daily_limit: u32,
    ) -> SuperLikeVerdict {
        if let Some(remaining) = self.super_like_cooldown_remaining(guild_id, user_id, cooldown_sec)
        {
            return SuperLikeVerdict::Cooldown {
                remaining_sec: remaining,
            };
        }
        let used = self.super_like_used_today(guild_id, user_id);
        match as_limit_u32(daily_limit) {
            Some(limit) if used >= limit => SuperLikeVerdict::DailyLimitReached { limit },
            Some(limit) => SuperLikeVerdict::Allowed {
                used_today: used,
                remaining: Some(limit - used),
            },
            None => SuperLikeVerdict::Allowed {
                used_today: used,
                remaining: None,
            },
        }
    }

    /// 통과하면 하루 사용량을 올리고 쿨타임을 건다. 막히면 아무것도 바꾸지 않는다.
    pub fn consume_super_like(
        &self,
        guild_id: u64,
        user_id: u64,
        cooldown_sec: u32,
        daily_limit: u32,
    ) -> SuperLikeVerdict {
        let verdict = self.check_super_like(guild_id, user_id, cooldown_sec, daily_limit);
        if !verdict.is_allowed() {
            return verdict;
        }
        let day = Self::utc_day();
        {
            let conn = self.conn.lock().unwrap();
            let _ = conn.execute(
                r#"INSERT INTO remote_super_like_usage(guild_id, user_id, day, used, last_utc)
                   VALUES(?1, ?2, ?3, 1, ?4)
                   ON CONFLICT(guild_id, user_id, day) DO UPDATE SET
                     used = used + 1, last_utc = excluded.last_utc"#,
                params![guild_id as i64, user_id as i64, day, Self::now_iso()],
            );
        }
        if cooldown_sec > 0 {
            let mut map = self
                .super_like_cooldowns
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            map.insert(
                (guild_id, user_id),
                Utc::now() + ChronoDuration::seconds(cooldown_sec as i64),
            );
        }
        let used = self.super_like_used_today(guild_id, user_id);
        SuperLikeVerdict::Allowed {
            used_today: used,
            remaining: as_limit_u32(daily_limit).map(|limit| limit.saturating_sub(used)),
        }
    }

    /// **취소하면 횟수를 돌려준다** (§10.6). 실수로 누른 걸 하루 종일 못 쓰게 하면 가혹하다.
    /// 쿨타임은 안 돌려준다 — 연타 방지가 목적이라 취소로 풀리면 의미가 없다.
    pub fn refund_super_like(&self, guild_id: u64, user_id: u64) -> u32 {
        {
            let conn = self.conn.lock().unwrap();
            let _ = conn.execute(
                r#"UPDATE remote_super_like_usage SET used = used - 1
                   WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3 AND used > 0"#,
                params![guild_id as i64, user_id as i64, Self::utc_day()],
            );
        }
        self.super_like_used_today(guild_id, user_id)
    }

    /// `/state/cold` 의 `superLike` (§10.6).
    pub fn super_like_status(
        &self,
        guild_id: u64,
        user_id: u64,
        cooldown_sec: u32,
        daily_limit: u32,
    ) -> SuperLikeStatus {
        let used = self.super_like_used_today(guild_id, user_id);
        let available_at = self
            .super_like_cooldown_remaining(guild_id, user_id, cooldown_sec)
            .map(|remaining| (Utc::now() + ChronoDuration::seconds(remaining as i64)).to_rfc3339());
        SuperLikeStatus {
            cooldown_sec,
            daily_limit,
            used_today: used,
            remaining: as_limit_u32(daily_limit).map(|limit| limit.saturating_sub(used)),
            available_at_utc: available_at,
        }
    }

    pub fn super_like_used_today(&self, guild_id: u64, user_id: u64) -> u32 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT used FROM remote_super_like_usage WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3",
            params![guild_id as i64, user_id as i64, Self::utc_day()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or(0)
        .max(0) as u32
    }

    /// 남은 쿨타임(초). 쿨타임이 없거나 이미 풀렸으면 `None`.
    fn super_like_cooldown_remaining(
        &self,
        guild_id: u64,
        user_id: u64,
        cooldown_sec: u32,
    ) -> Option<u32> {
        if cooldown_sec == 0 {
            return None;
        }
        let map = self
            .super_like_cooldowns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let until = map.get(&(guild_id, user_id))?;
        let remaining = (*until - Utc::now()).num_seconds();
        (remaining > 0).then_some(remaining as u32)
    }

    // ───────── 자동재생 차단 후보 (§8.5-3) ─────────

    /// `📻 이 곡 말고`로 뺀 곡과 재생에 실패한 곡을 7일간 기억한다.
    /// 후보 하나를 한동안 안 뽑게 막는다.
    ///
    /// **트랙을 통째로 같이 남긴다.** 예전에는 `cache_key` 만 저장해서, 화면이 빼 둔 곡을
    /// `youtube:dQw4w9WgXcQ` 같은 코드로 보여 줄 수밖에 없었다. 호출부가 어차피 트랙을
    /// 들고 있으니 여기서 받아 두면 나중에 어디서도 다시 찾을 필요가 없다.
    pub fn block_autoplay_candidate(
        &self,
        guild_id: u64,
        track: &TrackRef,
        reason: Option<&str>,
    ) -> rusqlite::Result<()> {
        let cache_key = track.cache_key();
        let track_json = serde_json::to_string(track).unwrap_or_default();
        let conn = self.conn.lock().unwrap();
        let until = (Utc::now() + ChronoDuration::days(AUTOPLAY_BLOCK_DAYS)).to_rfc3339();
        conn.execute(
            r#"INSERT INTO remote_autoplay_blocked(guild_id, cache_key, until_utc, reason, created_utc, track_json)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6)
               ON CONFLICT(guild_id, cache_key) DO UPDATE SET
                 until_utc = excluded.until_utc, reason = excluded.reason,
                 track_json = excluded.track_json"#,
            params![guild_id as i64, cache_key, until, reason, Self::now_iso(), track_json],
        )?;
        Ok(())
    }

    /// 아직 살아 있는 차단만. 만료된 행은 이 자리에서 지운다(지연 삭제).
    pub fn blocked_autoplay_keys(&self, guild_id: u64) -> HashSet<String> {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"DELETE FROM remote_autoplay_blocked
               WHERE guild_id = ?1 AND julianday(until_utc) <= julianday('now')"#,
            params![guild_id as i64],
        );
        let mut statement = match conn
            .prepare("SELECT cache_key FROM remote_autoplay_blocked WHERE guild_id = ?1")
        {
            Ok(statement) => statement,
            Err(_) => return HashSet::new(),
        };
        statement
            .query_map(params![guild_id as i64], |row| row.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn unblock_autoplay_candidate(
        &self,
        guild_id: u64,
        cache_key: &str,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM remote_autoplay_blocked WHERE guild_id = ?1 AND cache_key = ?2",
            params![guild_id as i64, cache_key],
        )?;
        Ok(changed > 0)
    }

    // ───────── 추천 바구니 비우기 (§8.7) ─────────
    //
    // 자동재생은 **세 가지**를 근거로 곡을 고른다. 하나만 비워서는 추천 성향이 안 바뀐다.
    //   1. 기준 곡(seeds) — 내가 직접 담은 것
    //   2. 최근 재생(recent) — 봇이 자동으로 쌓은 것
    //   3. 막힌 후보(blocked) — 건너뛰거나 싫어요를 받아 한동안 빼 둔 것
    // 그래서 각각 따로도, 한 번에도 비울 수 있게 한다.

    /// 담아 둔 기준 곡을 전부 뺀다. 몇 개를 뺐는지 돌려준다.
    pub fn clear_autoplay_seeds(&self, guild_id: u64) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM remote_autoplay_seeds WHERE guild_id = ?1",
            params![guild_id as i64],
        )
        .unwrap_or(0)
    }

    /// 막아 둔 후보를 전부 푼다. "왜 이 곡이 다시 안 나오지" 를 되돌리는 자리다.
    pub fn clear_autoplay_blocked(&self, guild_id: u64) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM remote_autoplay_blocked WHERE guild_id = ?1",
            params![guild_id as i64],
        )
        .unwrap_or(0)
    }

    /// 최근 재생 이력을 지운다.
    ///
    /// **통계와 우리 차트는 건드리지 않는다.** 그쪽은 별도 DB(`musicbot-stats.sqlite`)라
    /// 여기서 지워도 개인 통계와 재생 횟수 차트는 그대로 남는다. 추천이 참고하는
    /// 이력만 리셋된다 — 사람들이 원하는 건 "추천을 새로 시작" 이지 "기록을 지움" 이 아니다.
    /// 최근 재생 목록에서 **한 줄만** 지운다 (§8.7).
    ///
    /// `cache_key` 가 아니라 행 `id` 로 지운다. 같은 곡을 여러 번 틀면 같은 `cache_key` 가
    /// 여러 줄 쌓이는데, 키로 지우면 "이 한 번"을 지우려다 그 곡 이력이 통째로 날아간다.
    ///
    /// 길드를 조건에 같이 넣는다 — id 만 믿으면 남의 서버 이력을 지울 수 있다.
    pub fn remove_recent(&self, guild_id: u64, id: i64) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM remote_recent_tracks WHERE guild_id = ?1 AND id = ?2",
            params![guild_id as i64, id],
        )?;
        Ok(changed > 0)
    }

    pub fn clear_recent(&self, guild_id: u64) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM remote_recent_tracks WHERE guild_id = ?1",
            params![guild_id as i64],
        )
        .unwrap_or(0)
    }

    /// 지금 막혀 있는 후보들. 화면이 "무엇이 왜 빠져 있는지" 를 보여줄 때 쓴다.
    /// 만료된 것은 [`Self::blocked_autoplay_keys`] 처럼 지나는 길에 치운다.
    /// 아직 살아 있는 차단 목록. `(cache_key, reason, until_utc, track)`.
    ///
    /// `track` 은 v20 이전에 쌓인 줄이나 백필로도 못 찾은 줄에서 `None` 이다.
    /// 그때 화면이 코드를 그대로 보여 주면 안 된다 — 호출부가 그 사정을 말해 줘야 한다.
    pub fn list_blocked_autoplay(
        &self,
        guild_id: u64,
    ) -> Vec<(String, Option<String>, String, Option<TrackRef>)> {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"DELETE FROM remote_autoplay_blocked
               WHERE guild_id = ?1 AND julianday(until_utc) <= julianday('now')"#,
            params![guild_id as i64],
        );
        let mut statement = match conn.prepare(
            r#"SELECT cache_key, reason, until_utc, track_json FROM remote_autoplay_blocked
               WHERE guild_id = ?1 ORDER BY created_utc DESC LIMIT 100"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64], |row| {
                let raw: Option<String> = row.get(3)?;
                let track = raw
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<TrackRef>(json).ok());
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, track))
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// 최근 재생 이력을 자동추천이 쓰기 좋은 모양으로. (§8.5-1·2)
    /// `cache_key → 재생 후 지난 시간(시간 단위)` 과 **최신순 아티스트 목록**을 같이 돌려준다.
    /// 한 번의 조회로 둘 다 만든다 — 추천 한 번에 쿼리를 두 번 돌 이유가 없다.
    pub fn recent_play_history(
        &self,
        guild_id: u64,
        limit: usize,
    ) -> (HashMap<String, f64>, Vec<String>) {
        let now = Utc::now();
        let mut ages: HashMap<String, f64> = HashMap::new();
        let mut artists: Vec<String> = Vec::new();
        for recent in self.list_recent(guild_id, limit) {
            let key = recent.track.cache_key();
            let hours = DateTime::parse_from_rfc3339(&recent.played_utc)
                .map(|played| {
                    (now - played.with_timezone(&Utc)).num_minutes() as f64 / 60.0
                })
                .unwrap_or(f64::MAX);
            // 같은 곡이 여러 번 나오면 **가장 최근**이 기준이다.
            ages.entry(key).and_modify(|old| *old = old.min(hours)).or_insert(hours);
            if let Some(artist) = recent.track.artist.as_deref() {
                let artist = artist.trim();
                if !artist.is_empty() {
                    artists.push(artist.to_lowercase());
                }
            }
        }
        (ages, artists)
    }

    // ───────── 차트 (§15) ─────────

    /// 이 길드가 볼 수 있는 차트 전부(공용 + 자기 것). 꺼진 것도 포함하니
    /// 유저 UI 는 `enabled`와 `ok()`로 한 번 더 거른다.
    pub fn list_charts(&self, guild_id: u64) -> Vec<ChartDef> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(CHART_SELECT) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64], map_chart)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn get_chart(&self, guild_id: u64, chart_id: i64) -> Option<ChartDef> {
        self.list_charts(guild_id)
            .into_iter()
            .find(|chart| chart.id == chart_id)
    }

    // ───────── 서버 승인 (§26) ─────────

    /// 이 서버의 승인 상태. 기록이 없으면 `None` — 아직 본 적 없는 서버다.
    pub fn guild_approval(&self, guild_id: u64) -> Option<GuildApproval> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"SELECT status, guild_name, invited_by, invited_by_name,
                      requested_utc, decided_by, decided_utc, note
                 FROM remote_guild_approval WHERE guild_id = ?1"#,
            params![guild_id as i64],
            |row| {
                Ok(GuildApproval {
                    guild_id,
                    status: GuildApprovalStatus::parse(&row.get::<_, String>(0)?)
                        .unwrap_or_default(),
                    guild_name: row.get(1)?,
                    invited_by: row.get::<_, Option<i64>>(2)?.map(|id| id as u64),
                    invited_by_name: row.get(3)?,
                    requested_utc: row.get(4)?,
                    decided_by: row.get::<_, Option<i64>>(5)?.map(|id| id as u64),
                    decided_utc: row.get(6)?,
                    note: row.get(7)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    /// 새 서버를 대기 목록에 올린다. **이미 있는 서버는 건드리지 않는다** —
    /// 안 그러면 봇을 내보냈다 다시 부르는 것만으로 차단이 풀린다.
    pub fn register_guild(&self, guild_id: u64, name: Option<&str>) -> GuildApprovalStatus {
        if let Some(existing) = self.guild_approval(guild_id) {
            // 이름만 최신으로 맞춰 둔다. 운영 패널에서 어느 서버인지 알아보려면 필요하다.
            if let Some(name) = name {
                let conn = self.conn.lock().unwrap();
                let _ = conn.execute(
                    "UPDATE remote_guild_approval SET guild_name = ?2 WHERE guild_id = ?1",
                    params![guild_id as i64, name],
                );
            }
            return existing.status;
        }
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"INSERT INTO remote_guild_approval(guild_id, status, guild_name, requested_utc)
               VALUES(?1, 'pending', ?2, ?3)"#,
            params![guild_id as i64, name, Self::now_iso()],
        );
        GuildApprovalStatus::Pending
    }

    /// 승인·거절. 결정한 사람과 시각을 함께 남긴다.
    pub fn decide_guild(
        &self,
        guild_id: u64,
        status: GuildApprovalStatus,
        decided_by: u64,
        note: Option<&str>,
    ) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"UPDATE remote_guild_approval
                  SET status = ?2, decided_by = ?3, decided_utc = ?4, note = ?5
                WHERE guild_id = ?1"#,
            params![
                guild_id as i64,
                status.as_str(),
                decided_by as i64,
                Self::now_iso(),
                note
            ],
        )
        .map(|changed| changed > 0)
        .unwrap_or(false)
    }

    /// 운영 패널 표. 대기 중인 것이 위로 온다 — 그게 사람이 처리해야 할 일이다.
    pub fn list_guild_approvals(&self) -> Vec<GuildApproval> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT guild_id, status, guild_name, invited_by, invited_by_name,
                      requested_utc, decided_by, decided_utc, note
                 FROM remote_guild_approval
                ORDER BY CASE status WHEN 'pending' THEN 0 WHEN 'approved' THEN 1 ELSE 2 END,
                         requested_utc DESC"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map([], |row| {
                Ok(GuildApproval {
                    guild_id: row.get::<_, i64>(0)? as u64,
                    status: GuildApprovalStatus::parse(&row.get::<_, String>(1)?)
                        .unwrap_or_default(),
                    guild_name: row.get(2)?,
                    invited_by: row.get::<_, Option<i64>>(3)?.map(|id| id as u64),
                    invited_by_name: row.get(4)?,
                    requested_utc: row.get(5)?,
                    decided_by: row.get::<_, Option<i64>>(6)?.map(|id| id as u64),
                    decided_utc: row.get(7)?,
                    note: row.get(8)?,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    // ───────── 역할 캐시 (S-429) ─────────

    /// Discord 에서 읽은 역할을 디스크에 남긴다. 메모리 캐시와 **함께** 쓴다 —
    /// 메모리는 빠르고, 이쪽은 재시작을 견딘다.
    pub fn save_member_roles(&self, guild_id: u64, user_id: u64, roles: &[u64]) {
        let Ok(json) = serde_json::to_string(roles) else {
            return;
        };
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"INSERT INTO remote_member_roles(guild_id, user_id, roles_json, fetched_utc)
               VALUES(?1, ?2, ?3, ?4)
               ON CONFLICT(guild_id, user_id) DO UPDATE SET
                 roles_json = excluded.roles_json, fetched_utc = excluded.fetched_utc"#,
            params![guild_id as i64, user_id as i64, json, Self::now_iso()],
        );
    }

    /// `grace_hours` 안에 읽어 둔 역할. 없으면 `None` —
    /// **빈 목록과 구분되어야 한다.** 빈 벡터를 돌려주면 "역할이 하나도 없는 사람" 과
    /// "아직 모르는 사람" 이 같아져서, 모를 때 권한을 막아 버린다.
    pub fn load_member_roles(
        &self,
        guild_id: u64,
        user_id: u64,
        grace_hours: i64,
    ) -> Option<Vec<u64>> {
        let conn = self.conn.lock().unwrap();
        let (json, fetched): (String, String) = conn
            .query_row(
                "SELECT roles_json, fetched_utc FROM remote_member_roles
                  WHERE guild_id = ?1 AND user_id = ?2",
                params![guild_id as i64, user_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()?;
        let fresh_enough = DateTime::parse_from_rfc3339(&fetched)
            .map(|at| Utc::now() - at.with_timezone(&Utc) < ChronoDuration::hours(grace_hours))
            .unwrap_or(false);
        if !fresh_enough {
            return None;
        }
        serde_json::from_str(&json).ok()
    }

    // ───────── 재시작 이어듣기 (§24) ─────────

    /// 끄기 직전의 재생 위치를 남긴다.
    pub fn save_resume(
        &self,
        guild_id: u64,
        item_id: Option<&str>,
        position_seconds: f64,
        was_paused: bool,
    ) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"INSERT INTO remote_resume(guild_id, item_id, position_seconds, was_paused, saved_utc)
               VALUES(?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(guild_id) DO UPDATE SET
                 item_id = excluded.item_id, position_seconds = excluded.position_seconds,
                 was_paused = excluded.was_paused, saved_utc = excluded.saved_utc"#,
            params![
                guild_id as i64,
                item_id,
                position_seconds,
                was_paused as i64,
                Self::now_iso()
            ],
        );
    }

    /// 기록을 읽고 **곧바로 지운다.** 한 번만 이어 붙이려는 것이다 —
    /// 안 지우면 나중에 그냥 재시작했을 때도 몇 시간 전 곡이 되살아난다.
    pub fn take_resume(&self, guild_id: u64) -> Option<ResumePoint> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(Option<String>, f64, i64, String)> = conn
            .query_row(
                "SELECT item_id, position_seconds, was_paused, saved_utc
                   FROM remote_resume WHERE guild_id = ?1",
                params![guild_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (item_id, position_seconds, was_paused, saved_utc) = row?;
        let _ = conn.execute(
            "DELETE FROM remote_resume WHERE guild_id = ?1",
            params![guild_id as i64],
        );
        let age_hours = DateTime::parse_from_rfc3339(&saved_utc)
            .map(|at| (Utc::now() - at.with_timezone(&Utc)).num_seconds() as f64 / 3600.0)
            .unwrap_or(f64::MAX);
        Some(ResumePoint {
            item_id,
            position_seconds,
            was_paused: was_paused != 0,
            age_hours,
        })
    }

    pub fn clear_resume(&self, guild_id: u64) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "DELETE FROM remote_resume WHERE guild_id = ?1",
            params![guild_id as i64],
        );
    }

    // ───────── TJ 곡번호 → 재생 주소 (§15.2c) ─────────

    /// 이 TJ 곡번호로 저장해 둔 재생 가능한 트랙. 못 찾았던 곡이면 `None`.
    pub fn tj_track(&self, tj_number: i64) -> Option<TrackRef> {
        let conn = self.conn.lock().unwrap();
        let (provider, content_id, source_url, title, artist, duration_ms): (
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT provider, content_id, source_url, title, artist, duration_ms
                   FROM remote_tj_tracks WHERE tj_number = ?1",
                params![tj_number],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()?;
        // content_id 가 비어 있으면 "찾아봤지만 없더라" 는 기록이다. 트랙이 아니다.
        let content_id = content_id.filter(|value| !value.is_empty())?;
        Some(TrackRef {
            provider: crate::remote::tj::provider_from_str(provider.as_deref().unwrap_or("")),
            content_id,
            source_url: source_url.unwrap_or_default(),
            title: Some(title),
            artist: (!artist.is_empty()).then_some(artist),
            duration: duration_ms.map(|ms| crate::models::CsTimeSpan::from_secs_f64(ms as f64 / 1000.0)),
            variant_key: None,
            is_live: false,
        })
    }

    /// 이 곡을 몇 번이나 못 찾았는지. 반주 영상이 아예 없는 곡에 검색을 계속 쓰지 않으려고 센다.
    pub fn tj_miss_count(&self, tj_number: i64) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT miss_count FROM remote_tj_tracks WHERE tj_number = ?1",
            params![tj_number],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or(0)
    }

    /// 찾은 결과를 저장한다. `track` 이 `None` 이면 "못 찾음" 으로 기록하고 횟수를 올린다.
    /// **못 찾은 것도 반드시 남긴다** — 안 남기면 없는 곡을 차트 열 때마다 다시 찾는다.
    pub fn save_tj_track(
        &self,
        tj_number: i64,
        title: &str,
        artist: &str,
        track: Option<&TrackRef>,
    ) {
        let conn = self.conn.lock().unwrap();
        let now = Self::now_iso();
        let result = match track {
            Some(track) => conn.execute(
                "INSERT INTO remote_tj_tracks
                     (tj_number, title, artist, provider, content_id, source_url, duration_ms, resolved_utc, miss_count)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)
                 ON CONFLICT(tj_number) DO UPDATE SET
                     title = excluded.title, artist = excluded.artist,
                     provider = excluded.provider, content_id = excluded.content_id,
                     source_url = excluded.source_url, duration_ms = excluded.duration_ms,
                     resolved_utc = excluded.resolved_utc, miss_count = 0",
                params![
                    tj_number,
                    title,
                    artist,
                    track.provider.as_str(),
                    track.content_id,
                    track.source_url,
                    track.duration.map(|d| (d.as_secs_f64() * 1000.0) as i64),
                    now
                ],
            ),
            None => conn.execute(
                "INSERT INTO remote_tj_tracks
                     (tj_number, title, artist, provider, content_id, source_url, duration_ms, resolved_utc, miss_count)
                 VALUES(?1, ?2, ?3, NULL, NULL, NULL, NULL, ?4, 1)
                 ON CONFLICT(tj_number) DO UPDATE SET
                     resolved_utc = excluded.resolved_utc,
                     miss_count = remote_tj_tracks.miss_count + 1",
                params![tj_number, title, artist, now],
            ),
        };
        let _ = result;
    }

    /// 캐시된 곡 목록. `stale`이면 TTL(6시간)이 지난 것이라 다시 받아야 한다 —
    /// 그래도 일단 보여 주는 편이 빈 화면보다 낫다.
    /// 이 차트의 분류. 캐시 수명이 분류마다 달라서 필요하다.
    ///
    /// **연결을 인자로 받는다.** `self.conn.lock()` 을 안에서 다시 잡으면
    /// 이미 락을 쥔 `chart_cache` 에서 부를 때 자기 자신을 기다리며 멈춘다.
    /// 실제로 그렇게 만들어서 테스트가 60초 넘게 걸렸다 — `Mutex` 는 재진입이 안 된다.
    fn chart_category_on(conn: &Connection, chart_id: i64) -> Option<ChartCategory> {
        let raw: String = conn
            .query_row(
                "SELECT category FROM remote_charts WHERE id = ?1",
                params![chart_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        ChartCategory::parse(&raw)
    }

    pub fn chart_cache(&self, chart_id: i64) -> Option<ChartSnapshot> {
        let conn = self.conn.lock().unwrap();
        let (json, fetched): (String, String) = conn
            .query_row(
                "SELECT tracks_json, fetched_utc FROM remote_chart_cache WHERE chart_id = ?1",
                params![chart_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()?;
        let tracks: Vec<TrackRef> = serde_json::from_str(&json).ok()?;
        if tracks.is_empty() {
            return None;
        }
        // 노래방은 훨씬 오래 들고 있는다 (§15.2c). TJ 순위는 하루 단위로 움직이고,
        // 곡마다 원곡을 찾느라 다시 받는 비용이 다른 차트보다 크다.
        let ttl = Self::chart_category_on(&conn, chart_id)
            .filter(|category| *category == ChartCategory::Karaoke)
            .map(|_| KARAOKE_CACHE_TTL_HOURS)
            .unwrap_or(CHART_CACHE_TTL_HOURS);
        let stale = DateTime::parse_from_rfc3339(&fetched)
            .map(|at| Utc::now() - at.with_timezone(&Utc) > ChronoDuration::hours(ttl))
            .unwrap_or(true);
        Some(ChartSnapshot {
            tracks,
            fetched_utc: fetched,
            stale,
        })
    }

    pub fn save_chart_cache(&self, chart_id: i64, tracks: &[TrackRef]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_chart_cache(chart_id, tracks_json, fetched_utc, failed_utc, failure_reason)
               VALUES(?1, ?2, ?3, NULL, NULL)
               ON CONFLICT(chart_id) DO UPDATE SET
                 tracks_json = excluded.tracks_json, fetched_utc = excluded.fetched_utc,
                 failed_utc = NULL, failure_reason = NULL"#,
            params![
                chart_id,
                serde_json::to_string(tracks).unwrap_or_else(|_| "[]".into()),
                Self::now_iso()
            ],
        )?;
        Ok(())
    }

    /// 갱신 실패를 그대로 남긴다 (§15.2). 숨기면 빈 차트를 눌렀는데 아무 일도 안 일어난다.
    pub fn mark_chart_failure(&self, chart_id: i64, reason: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_chart_cache(chart_id, tracks_json, fetched_utc, failed_utc, failure_reason)
               VALUES(?1, '[]', ?2, ?2, ?3)
               ON CONFLICT(chart_id) DO UPDATE SET
                 failed_utc = excluded.failed_utc, failure_reason = excluded.failure_reason"#,
            params![chart_id, Self::now_iso(), reason],
        )?;
        Ok(())
    }

    /// **동시 요청 합치기** (§15.1). `true`가 돌아온 쪽만 yt-dlp 를 돌리고,
    /// `false`를 받은 쪽은 잠깐 기다렸다 캐시를 다시 본다.
    /// `resolve_preview`의 `try_begin_preview_resolve`와 같은 방식이다.
    ///
    /// **버려진 표시는 스스로 풀린다.** 핸들러가 `end_chart_fetch` 까지 못 가고 취소되면
    /// (탭 닫기·페이지 이동으로 axum 이 future 를 drop) 그 차트가 영원히 잠겨서
    /// `차트를 가져오는 중이에요` 만 반복하고 관리자의 `↻ 새로고침`으로도 못 푼다.
    pub fn try_begin_chart_fetch(&self, chart_id: i64) -> bool {
        self.try_begin_chart_fetch_at(chart_id, Utc::now())
    }

    fn try_begin_chart_fetch_at(&self, chart_id: i64, now: DateTime<Utc>) -> bool {
        let mut inflight = self
            .chart_inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match inflight.get(&chart_id) {
            Some(started) if !Self::chart_fetch_is_stale(*started, now) => false,
            _ => {
                inflight.insert(chart_id, now);
                true
            }
        }
    }

    pub fn end_chart_fetch(&self, chart_id: i64) {
        let mut inflight = self
            .chart_inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inflight.remove(&chart_id);
    }

    pub fn is_chart_fetching(&self, chart_id: i64) -> bool {
        self.is_chart_fetching_at(chart_id, Utc::now())
    }

    fn is_chart_fetching_at(&self, chart_id: i64, now: DateTime<Utc>) -> bool {
        let inflight = self
            .chart_inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inflight
            .get(&chart_id)
            .is_some_and(|started| !Self::chart_fetch_is_stale(*started, now))
    }

    fn chart_fetch_is_stale(started: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        now - started > ChronoDuration::seconds(CHART_FETCH_STALE_SECS)
    }

    /// 관리 콘솔의 차트 추가. 길드 것으로만 만들 수 있다.
    pub fn add_chart(
        &self,
        guild_id: u64,
        category: ChartCategory,
        name: &str,
        provider: &str,
        url: &str,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let next_order: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM remote_charts WHERE guild_id = ?1",
            params![guild_id as i64],
            |row| row.get(0),
        )?;
        conn.execute(
            r#"INSERT INTO remote_charts(guild_id, category, name, provider, url, sort_order, enabled, builtin)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, 1, 0)"#,
            params![guild_id as i64, category.as_str(), name, provider, url, next_order],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 주소·이름 수정과 켜기/끄기. 기본 제공분도 여기까지는 만질 수 있다.
    pub fn update_chart(
        &self,
        chart_id: i64,
        name: Option<&str>,
        url: Option<&str>,
        enabled: Option<bool>,
        sort_order: Option<i64>,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            r#"UPDATE remote_charts SET
                 name = COALESCE(?2, name),
                 url = COALESCE(?3, url),
                 enabled = COALESCE(?4, enabled),
                 sort_order = COALESCE(?5, sort_order)
               WHERE id = ?1"#,
            params![chart_id, name, url, enabled.map(|on| on as i64), sort_order],
        )?;
        if url.is_some() {
            // 주소가 바뀌면 예전 곡 목록은 거짓말이다.
            conn.execute(
                "DELETE FROM remote_chart_cache WHERE chart_id = ?1",
                params![chart_id],
            )?;
        }
        Ok(changed > 0)
    }

    /// **기본 제공분은 지울 수 없고 끄기만 된다** (§15.5). 되돌릴 수 없는 삭제는 위험하다.
    pub fn remove_chart(&self, guild_id: u64, chart_id: i64) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "DELETE FROM remote_charts WHERE id = ?1 AND guild_id = ?2 AND builtin = 0",
            params![chart_id, guild_id as i64],
        )?;
        tx.execute(
            "DELETE FROM remote_chart_cache WHERE chart_id = ?1",
            params![chart_id],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// 이 길드가 리모컨 설정을 한 번이라도 저장한 적이 있는지.
    /// 저장한 적 없는 길드에 앱 기본 보존값을 쓰기 위한 판정이다.
    fn has_guild_settings(&self, guild_id: u64) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
            params![format!("remote_guild_settings:{guild_id}")],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    /// 마참뮤직 테이블에 흔적이 있는 길드 id 전부.
    pub fn remote_guild_ids(&self) -> Vec<u64> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT guild_id FROM remote_chat_messages
               UNION SELECT guild_id FROM remote_audit_logs
               UNION SELECT guild_id FROM remote_recent_tracks
               UNION SELECT guild_id FROM remote_queue_scores"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map([], |row| row.get::<_, i64>(0))
            .map(|rows| rows.flatten().map(|id| id as u64).collect())
            .unwrap_or_default()
    }

    // ───────── 가사 ─────────

    /// "아직 안 찾아봄"과 "찾아봤는데 없음"을 구분한다. 후자는 negative cache다.
    pub fn lookup_lyrics(&self, cache_key: &str) -> Option<LyricsCacheHit> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(i64, String)> = conn
            .query_row(
                "SELECT found, payload_json FROM remote_lyrics WHERE cache_key = ?1",
                params![cache_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (found, payload) = row?;
        if found == 0 {
            return Some(LyricsCacheHit::Missing);
        }
        serde_json::from_str::<LyricsDocument>(&payload)
            .ok()
            .map(|doc| LyricsCacheHit::Found(Box::new(doc)))
    }

    pub fn load_lyrics(&self, cache_key: &str) -> Option<LyricsDocument> {
        match self.lookup_lyrics(cache_key) {
            Some(LyricsCacheHit::Found(doc)) => Some(*doc),
            _ => None,
        }
    }

    pub fn save_lyrics(&self, lyrics: &LyricsDocument) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_lyrics(cache_key, payload_json, fetched_utc, found)
               VALUES(?1, ?2, ?3, 1)
               ON CONFLICT(cache_key) DO UPDATE SET
                 payload_json = excluded.payload_json, fetched_utc = excluded.fetched_utc, found = 1"#,
            params![
                lyrics.cache_key,
                serde_json::to_string(lyrics).unwrap_or_else(|_| "{}".into()),
                lyrics.fetched_utc
            ],
        )?;
        Ok(())
    }

    /// 가사를 못 찾았다는 사실을 캐시한다. TTL이 지나면 `prune_all`이 지운다.
    pub fn save_lyrics_missing(&self, cache_key: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_lyrics(cache_key, payload_json, fetched_utc, found)
               VALUES(?1, '{}', ?2, 0)
               ON CONFLICT(cache_key) DO UPDATE SET
                 payload_json = '{}', fetched_utc = excluded.fetched_utc, found = 0"#,
            params![cache_key, Self::now_iso()],
        )?;
        Ok(())
    }
}

// ───────── 마이그레이션 러너 ─────────

/// `PRAGMA user_version` 기반 단계 실행. 각 단계는 트랜잭션이고 전부 멱등하다.
/// 레거시(C# 공용) 테이블은 어떤 단계도 손대지 않는다.
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    while version < SCHEMA_VERSION {
        let tx = conn.transaction()?;
        match version {
            0 => tx.execute_batch(MIGRATION_V1)?,
            1 => {
                add_column(&tx, "remote_chat_messages", "reply_to_message_id", "INTEGER")?;
                add_column(&tx, "remote_chat_messages", "edited_utc", "TEXT")?;
            }
            2 => tx.execute_batch(MIGRATION_V3)?,
            3 => tx.execute_batch(MIGRATION_V4)?,
            4 => tx.execute_batch(MIGRATION_V5)?,
            5 => tx.execute_batch(MIGRATION_V6)?,
            6 => {
                add_column(
                    &tx,
                    "remote_queue_scores",
                    "round",
                    "INTEGER NOT NULL DEFAULT 0",
                )?;
                add_column(&tx, "remote_queue_scores", "last_played_utc", "TEXT")?;
            }
            7 => {
                add_column(&tx, "remote_lyrics", "found", "INTEGER NOT NULL DEFAULT 1")?;
                tx.execute_batch(MIGRATION_V8)?;
            }
            8 => tx.execute_batch(MIGRATION_V9)?,
            9 => tx.execute_batch(MIGRATION_V10)?,
            10 => {
                tx.execute_batch(MIGRATION_V11)?;
                seed_builtin_charts(&tx)?;
            }
            11 => tx.execute_batch(MIGRATION_V12)?,
            12 => {
                // 컬럼을 먼저 붙이고 나서 그 컬럼을 쓰는 인덱스를 만든다.
                add_column(
                    &tx,
                    "remote_audit_logs",
                    "kind",
                    "TEXT NOT NULL DEFAULT 'admin'",
                )?;
                // `text` 는 타입 이름과 겹쳐 헷갈리므로 컬럼명은 human_text 로 둔다.
                add_column(&tx, "remote_audit_logs", "human_text", "TEXT")?;
                add_column(
                    &tx,
                    "remote_audit_logs",
                    "merged_count",
                    "INTEGER NOT NULL DEFAULT 1",
                )?;
                add_column(&tx, "remote_audit_logs", "merged_items_json", "TEXT")?;
                tx.execute_batch(MIGRATION_V13)?;
                // 이미 쌓인 줄에도 분류를 채워 준다 — 필터가 옛 줄만 통째로 놓치면 안 된다.
                backfill_audit_kinds(&tx)?;
            }
            13 => {
                tx.execute_batch(MIGRATION_V14)?;
                // 새로 늘린 노래방·장르 차트를 기존 DB 에도 심는다.
                seed_builtin_charts(&tx)?;
            }
            14 => {
                // 지우기가 먼저다. 시더를 먼저 돌리면 이름이 겹치는 새 차트가 INSERT OR IGNORE 에
                // 걸려 조용히 안 들어가고, 그 뒤 DELETE 가 옛 줄을 지워 차트가 통째로 사라진다.
                tx.execute_batch(MIGRATION_V15)?;
                seed_builtin_charts(&tx)?;
            }
            15 => tx.execute_batch(MIGRATION_V16)?,
            16 => migrate_v17_grandfather_guilds(&tx)?,
            // v17 은 `INSERT OR IGNORE` 만 해서, 게이트를 켠 빌드가 이미 만들어 둔
            // `pending` 행을 못 고쳤다. 같은 함수가 이제 대기 상태도 올린다 — 다시 돌린다.
            17 => migrate_v17_grandfather_guilds(&tx)?,
            // 장르에 J-POP 계열이 통째로 없었다. 새 차트를 기존 DB 에도 심는다.
            18 => seed_builtin_charts(&tx)?,
            // 빼 둔 곡이 화면에 `youtube:dQw4w9WgXcQ` 같은 코드로만 나왔다. 이 표만
            // `cache_key` 하나로 살고 있어서 제목을 줄 방법이 아예 없었다
            // (이웃인 `remote_autoplay_seeds`·`remote_recent_tracks` 는 `track_json` 을 갖고 있다).
            19 => {
                add_column(&tx, "remote_autoplay_blocked", "track_json", "TEXT")?;
                // **이미 쌓인 줄이 문제의 전부다.** 컬럼만 붙이면 지금 빼 둔 곡들은
                // 그대로 코드로 남는다. 같은 길드의 이웃 표에서 찾아 채운다.
                backfill_blocked_tracks(&tx)?;
            }
            /* TJ 가요 100 과 OST 100 이 통째로 빠져 있었다.
             *
             * TJ 는 분류 번호의 뜻을 공개하지 않아서 곡을 보고 하나씩 채운 표인데,
             * `1`(가요)과 `8`(OST)은 **번호가 있는지조차 몰라서** 목록에 없었다.
             * 2026-08-16 에 실제 응답을 훑어 둘 다 100곡짜리 정식 차트임을 확인했다.
             * 새 차트는 기존 DB 에도 심어야 보인다 — v18 때와 같은 이유다. */
            20 => seed_builtin_charts(&tx)?,
            /* "가사 없음" 으로 적어 둔 것을 비운다.
             *
             * 찾는 방법을 고쳤는데(§41 — 제목을 씻고 여러 번 물어본다) 예전에 못 찾아서
             * `found = 0` 으로 박아 둔 곡은 **다시 안 찾아본다.** 고친 보람이 그 곡들에는
             * 영영 닿지 않는다. 찾은 가사는 그대로 두고 못 찾은 기록만 지운다. */
            21 => {
                tx.execute("DELETE FROM remote_lyrics WHERE found = 0", [])?;
            }
            // 여기 오면 SCHEMA_VERSION 만 올리고 단계를 안 쓴 것이다.
            _ => {}
        }
        version += 1;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// 기본 제공 차트를 한 번 심는다 (§15.2). `guild_id IS NULL` 이라 모든 서버가 같이 본다.
/// 이름에 unique 인덱스가 걸려 있어 다시 돌아도 중복되지 않는다.
fn seed_builtin_charts(conn: &Connection) -> rusqlite::Result<()> {
    let now_order = 0i64;
    for (index, (category, name, provider, url)) in BUILTIN_CHARTS.iter().enumerate() {
        conn.execute(
            r#"INSERT OR IGNORE INTO remote_charts
               (guild_id, category, name, provider, url, sort_order, enabled, builtin)
               VALUES(NULL, ?1, ?2, ?3, ?4, ?5, 1, 1)"#,
            params![
                category.as_str(),
                name,
                provider,
                url,
                now_order + index as i64
            ],
        )?;
    }
    Ok(())
}

/// v19 이전에 빼 둔 곡에 제목을 채운다.
///
/// `remote_autoplay_blocked` 는 `cache_key` 만 들고 있었다. 같은 `cache_key` 로 트랙 정보를
/// 갖고 있는 표가 둘 있다 — 기준 곡과 최근 재생. **길드까지 같은 줄만** 본다(캐시 키는 곡
/// 식별자라 서버가 달라도 겹치는데, 남의 서버 데이터를 끌어다 쓸 이유가 없다).
///
/// 못 찾는 줄이 남는 것은 정상이다. 오래 전에 빼 두고 그 뒤로 한 번도 안 튼 곡은 어디에도
/// 흔적이 없다. 그때는 화면이 코드 대신 "제목을 못 찾았어요" 라고 말한다.
fn backfill_blocked_tracks(conn: &Connection) -> rusqlite::Result<()> {
    // 1) 기준 곡은 `cache_key` 를 컬럼으로 갖고 있어 SQL 만으로 맞출 수 있다.
    conn.execute(
        r#"UPDATE remote_autoplay_blocked
           SET track_json = (
               SELECT s.track_json FROM remote_autoplay_seeds s
               WHERE s.guild_id = remote_autoplay_blocked.guild_id
                 AND s.cache_key = remote_autoplay_blocked.cache_key
               LIMIT 1)
           WHERE track_json IS NULL
             AND EXISTS (
               SELECT 1 FROM remote_autoplay_seeds s
               WHERE s.guild_id = remote_autoplay_blocked.guild_id
                 AND s.cache_key = remote_autoplay_blocked.cache_key)"#,
        [],
    )?;

    // 2) 최근 재생에는 `cache_key` 컬럼이 없다(트랙 JSON 만 있다). 키는 파생값이라
    //    SQL 로는 못 만든다 — 여기서 계산해서 맞춘다.
    let still_missing: Vec<(i64, String)> = {
        let mut statement = conn.prepare(
            "SELECT guild_id, cache_key FROM remote_autoplay_blocked WHERE track_json IS NULL",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.flatten().collect()
    };
    if still_missing.is_empty() {
        return Ok(());
    }
    for (guild_id, cache_key) in still_missing {
        let candidates: Vec<String> = {
            let mut statement = conn.prepare(
                "SELECT track_json FROM remote_recent_tracks WHERE guild_id = ?1 ORDER BY id DESC",
            )?;
            let rows = statement.query_map(params![guild_id], |row| row.get::<_, String>(0))?;
            rows.flatten().collect()
        };
        let found = candidates.into_iter().find(|json| {
            serde_json::from_str::<TrackRef>(json)
                .map(|track| track.cache_key() == cache_key)
                .unwrap_or(false)
        });
        if let Some(track_json) = found {
            conn.execute(
                "UPDATE remote_autoplay_blocked SET track_json = ?3
                 WHERE guild_id = ?1 AND cache_key = ?2",
                params![guild_id, cache_key, track_json],
            )?;
        }
    }
    Ok(())
}

/// v13 이전에 쌓인 활동 로그에 분류를 채운다. 액션명만 보면 정할 수 있어 손실이 없다.
fn backfill_audit_kinds(conn: &Connection) -> rusqlite::Result<()> {
    let actions: Vec<String> = {
        let mut statement = conn.prepare("SELECT DISTINCT action FROM remote_audit_logs")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.flatten().collect()
    };
    for action in actions {
        conn.execute(
            "UPDATE remote_audit_logs SET kind = ?2 WHERE action = ?1",
            params![action, audit_kind_for(&action).as_str()],
        )?;
    }
    Ok(())
}

/// 분류별 보존 정리 (§13.6). `retention_days == 0`이면 무제한이라 한 줄도 안 지운다(§23.1).
fn prune_audit_with(
    conn: &Connection,
    guild_id: u64,
    retention_days: i32,
) -> rusqlite::Result<usize> {
    if retention_days <= 0 {
        return Ok(0);
    }
    let mut removed = 0;
    for kind in AuditKind::ALL {
        let days = kind.retention_days(retention_days).clamp(1, 3650);
        removed += conn.execute(
            r#"DELETE FROM remote_audit_logs
               WHERE guild_id = ?1 AND kind = ?2
                 AND julianday(created_utc) < julianday('now', ?3)"#,
            params![guild_id as i64, kind.as_str(), format!("-{days} days")],
        )?;
    }
    // 분류가 비어 있는(아주 오래된) 줄도 설정값 기준으로 같이 턴다.
    removed += conn.execute(
        r#"DELETE FROM remote_audit_logs
           WHERE guild_id = ?1 AND (kind IS NULL OR kind = '')
             AND julianday(created_utc) < julianday('now', ?2)"#,
        params![
            guild_id as i64,
            format!("-{} days", retention_days.clamp(1, 3650))
        ],
    )?;
    Ok(removed)
}

/// 공용(기본 제공) 차트 + 이 길드 차트. ?1 = guild_id.
const CHART_SELECT: &str = concat!(
    "SELECT c.id, c.guild_id, c.category, c.name, c.provider, c.url, c.sort_order, ",
    "c.enabled, c.builtin, k.fetched_utc, k.failed_utc, k.failure_reason, k.tracks_json ",
    "FROM remote_charts c LEFT JOIN remote_chart_cache k ON k.chart_id = c.id ",
    "WHERE c.guild_id IS NULL OR c.guild_id = ?1 ",
    "ORDER BY c.category, c.sort_order, c.id"
);

fn map_chart(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChartDef> {
    let category: String = row.get(2)?;
    let tracks_json: Option<String> = row.get(12)?;
    let track_count = tracks_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<TrackRef>>(json).ok())
        .map(|tracks| tracks.len())
        .unwrap_or(0);
    Ok(ChartDef {
        id: row.get(0)?,
        guild_id: row.get::<_, Option<i64>>(1)?.map(|id| id as u64),
        category: ChartCategory::parse(&category).unwrap_or(ChartCategory::Popular),
        name: row.get(3)?,
        provider: row.get(4)?,
        url: row.get(5)?,
        sort_order: row.get(6)?,
        enabled: row.get::<_, i64>(7)? != 0,
        builtin: row.get::<_, i64>(8)? != 0,
        last_fetched_utc: row.get(9)?,
        last_failure_utc: row.get(10)?,
        last_failure_reason: row.get(11)?,
        track_count,
    })
}

/// `ALTER TABLE ... ADD COLUMN`은 이미 있으면 에러가 나므로 먼저 확인한다.
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> rusqlite::Result<()> {
    if column_exists(conn, table, column)? {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name.eq_ignore_ascii_case(column) {
            return Ok(true);
        }
    }
    Ok(false)
}

// ───────── 내부 헬퍼 ─────────

struct ChatRow {
    id: i64,
    user_id: i64,
    display_name: String,
    avatar_url: Option<String>,
    content: String,
    created_utc: String,
    deleted_utc: Option<String>,
    edited_utc: Option<String>,
    reply_to: Option<i64>,
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

/// 메시지 수와 무관하게 반응을 한 번에 읽어 `message_id`로 묶는다 (N+1 제거).
fn load_reactions(
    conn: &Connection,
    ids: &[i64],
    viewer_user_id: u64,
) -> HashMap<i64, Vec<ChatReactionSummary>> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let sql = format!(
        r#"SELECT message_id, emoji, COUNT(*),
                  MAX(CASE WHEN user_id = ? THEN 1 ELSE 0 END)
           FROM remote_chat_reactions
           WHERE message_id IN ({})
           GROUP BY message_id, emoji
           ORDER BY message_id, emoji"#,
        placeholders(ids.len())
    );
    let mut binds: Vec<SqlValue> = Vec::with_capacity(ids.len() + 1);
    binds.push(SqlValue::Integer(viewer_user_id as i64));
    binds.extend(ids.iter().map(|id| SqlValue::Integer(*id)));

    let mut statement = match conn.prepare(&sql) {
        Ok(statement) => statement,
        Err(_) => return HashMap::new(),
    };
    let rows = statement.query_map(params_from_iter(binds), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            ChatReactionSummary {
                emoji: row.get(1)?,
                count: row.get::<_, i64>(2)? as i32,
                reacted_by_me: row.get::<_, i64>(3)? != 0,
            },
        ))
    });
    let mut grouped: HashMap<i64, Vec<ChatReactionSummary>> = HashMap::new();
    if let Ok(rows) = rows {
        for (message_id, summary) in rows.flatten() {
            grouped.entry(message_id).or_default().push(summary);
        }
    }
    grouped
}

fn load_tags(conn: &Connection, ids: &[i64]) -> HashMap<i64, Vec<ChatTrackTag>> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let sql = format!(
        "SELECT message_id, cache_key, track_json FROM remote_chat_tags WHERE message_id IN ({}) ORDER BY message_id, cache_key",
        placeholders(ids.len())
    );
    let binds: Vec<SqlValue> = ids.iter().map(|id| SqlValue::Integer(*id)).collect();
    let mut statement = match conn.prepare(&sql) {
        Ok(statement) => statement,
        Err(_) => return HashMap::new(),
    };
    let rows = statement.query_map(params_from_iter(binds), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    });
    let mut grouped: HashMap<i64, Vec<ChatTrackTag>> = HashMap::new();
    if let Ok(rows) = rows {
        for (message_id, cache_key, json) in rows.flatten() {
            if let Ok(track) = serde_json::from_str::<TrackRef>(&json) {
                grouped
                    .entry(message_id)
                    .or_default()
                    .push(ChatTrackTag { cache_key, track });
            }
        }
    }
    grouped
}

fn load_mentions(conn: &Connection, ids: &[i64]) -> HashMap<i64, Vec<u64>> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let sql = format!(
        "SELECT message_id, user_id FROM remote_chat_mentions WHERE message_id IN ({})",
        placeholders(ids.len())
    );
    let binds: Vec<SqlValue> = ids.iter().map(|id| SqlValue::Integer(*id)).collect();
    let mut statement = match conn.prepare(&sql) {
        Ok(statement) => statement,
        Err(_) => return HashMap::new(),
    };
    let rows =
        statement.query_map(params_from_iter(binds), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        });
    let mut grouped: HashMap<i64, Vec<u64>> = HashMap::new();
    if let Ok(rows) = rows {
        for (message_id, user_id) in rows.flatten() {
            grouped.entry(message_id).or_default().push(user_id as u64);
        }
    }
    grouped
}

/// 인용 원문은 조회 페이지 밖일 수 있으므로 대상 id로 따로 읽는다.
fn load_reply_previews(
    conn: &Connection,
    guild_id: u64,
    ids: &[i64],
) -> HashMap<i64, ChatReplyPreview> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let sql = format!(
        "SELECT id, display_name, content, deleted_utc FROM remote_chat_messages WHERE guild_id = ? AND id IN ({})",
        placeholders(unique.len())
    );
    let mut binds: Vec<SqlValue> = Vec::with_capacity(unique.len() + 1);
    binds.push(SqlValue::Integer(guild_id as i64));
    binds.extend(unique.iter().map(|id| SqlValue::Integer(*id)));

    let mut statement = match conn.prepare(&sql) {
        Ok(statement) => statement,
        Err(_) => return HashMap::new(),
    };
    let rows = statement.query_map(params_from_iter(binds), |row| {
        let id: i64 = row.get(0)?;
        let display_name: String = row.get(1)?;
        let content: String = row.get(2)?;
        let deleted: Option<String> = row.get(3)?;
        Ok((
            id,
            ChatReplyPreview {
                id,
                display_name,
                excerpt: if deleted.is_some() {
                    "삭제된 메시지".into()
                } else {
                    ChatReplyPreview::excerpt_of(&content)
                },
                deleted: deleted.is_some(),
            },
        ))
    });
    match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => HashMap::new(),
    }
}

/// 제안 목록. ?1 = guild_id, ?2 = 보는 사람. 공감 많은 순 → 최신순.
const SUGGESTION_SELECT: &str = concat!(
    "SELECT s.id, s.user_id, s.display_name, s.avatar_url, s.title, s.body, ",
    "s.status, s.status_note, s.status_by_user_id, s.status_utc, s.created_utc, ",
    "(SELECT COUNT(*) FROM remote_suggestion_votes v WHERE v.suggestion_id = s.id) AS vote_count, ",
    "EXISTS(SELECT 1 FROM remote_suggestion_votes v WHERE v.suggestion_id = s.id AND v.user_id = ?2) AS voted_by_me ",
    "FROM remote_suggestions s WHERE s.guild_id = ?1 AND s.deleted_utc IS NULL ",
    "ORDER BY vote_count DESC, s.id DESC"
);

/// ?3 = 제안 id.
const SUGGESTION_SELECT_ONE: &str = concat!(
    "SELECT s.id, s.user_id, s.display_name, s.avatar_url, s.title, s.body, ",
    "s.status, s.status_note, s.status_by_user_id, s.status_utc, s.created_utc, ",
    "(SELECT COUNT(*) FROM remote_suggestion_votes v WHERE v.suggestion_id = s.id) AS vote_count, ",
    "EXISTS(SELECT 1 FROM remote_suggestion_votes v WHERE v.suggestion_id = s.id AND v.user_id = ?2) AS voted_by_me ",
    "FROM remote_suggestions s WHERE s.guild_id = ?1 AND s.deleted_utc IS NULL AND s.id = ?3"
);

fn map_suggestion(row: &rusqlite::Row<'_>, guild_id: u64) -> rusqlite::Result<Suggestion> {
    let status: String = row.get(6)?;
    Ok(Suggestion {
        id: row.get(0)?,
        guild_id,
        user_id: row.get::<_, i64>(1)? as u64,
        display_name: row.get(2)?,
        avatar_url: row.get(3)?,
        title: row.get(4)?,
        body: row.get(5)?,
        status: SuggestionStatus::parse(&status).unwrap_or_default(),
        status_note: row.get(7)?,
        status_by_user_id: row.get::<_, Option<i64>>(8)?.map(|id| id as u64),
        status_utc: row.get(9)?,
        created_utc: row.get(10)?,
        vote_count: row.get::<_, i64>(11)? as i32,
        voted_by_me: row.get::<_, i64>(12)? != 0,
    })
}

fn map_suspension(row: &rusqlite::Row<'_>, guild_id: u64) -> rusqlite::Result<Suspension> {
    let scope: String = row.get(1)?;
    Ok(Suspension {
        guild_id,
        user_id: row.get::<_, i64>(0)? as u64,
        scope: SuspensionScope::parse(&scope).unwrap_or(SuspensionScope::All),
        reason: row.get(2)?,
        by_user_id: row.get::<_, i64>(3)? as u64,
        created_utc: row.get(4)?,
        expires_utc: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderKind;
    use crate::remote::{MAX_AUTOPLAY_SEEDS, VotePoints};

    fn temp_store(tag: &str) -> (RemoteStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "macham-{tag}-{}.sqlite",
            crate::models::uuid_like()
        ));
        let store = RemoteStore::open(&path).unwrap();
        (store, path)
    }

    /// 길드 설정은 **레거시(C# 공용) `settings` 테이블**에 얹혀 산다.
    /// 이 러너는 그 테이블을 만들지 않으므로(절대 안 건드린다는 규칙) 테스트에서만 흉내낸다.
    fn with_legacy_settings_table(store: &RemoteStore) {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, json TEXT NOT NULL)")
            .unwrap();
    }

    fn cleanup(store: RemoteStore, path: std::path::PathBuf) {
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    /// 차트 펼치기 잠금은 **스스로 풀려야 한다** (§15.1).
    /// 핸들러 future 가 취소돼 `end_chart_fetch` 가 안 불리면 그 차트가 프로세스가 죽을 때까지
    /// `차트를 가져오는 중이에요` 만 반복하고 관리자의 `↻ 새로고침`으로도 못 푼다.
    #[test]
    fn an_abandoned_chart_fetch_unlocks_itself() {
        let (store, path) = temp_store("chart-lock");
        let start = Utc::now();

        // 처음 누른 사람만 yt-dlp 를 돌린다.
        assert!(store.try_begin_chart_fetch_at(7, start));
        assert!(!store.try_begin_chart_fetch_at(7, start + ChronoDuration::seconds(1)));
        assert!(store.is_chart_fetching_at(7, start + ChronoDuration::seconds(1)));
        // 다른 차트는 서로 안 막는다.
        assert!(store.try_begin_chart_fetch_at(8, start));

        // 끝나면 곧바로 풀린다.
        store.end_chart_fetch(7);
        assert!(!store.is_chart_fetching_at(7, start));
        assert!(store.try_begin_chart_fetch_at(7, start));

        // end 를 못 부르고 죽어도 유통기한이 지나면 다음 사람이 이어받는다.
        let stale = start + ChronoDuration::seconds(CHART_FETCH_STALE_SECS + 1);
        assert!(!store.is_chart_fetching_at(7, stale), "버려진 표시가 안 풀렸다");
        assert!(store.try_begin_chart_fetch_at(7, stale));
        // 이어받은 쪽의 시계는 새로 시작한다.
        assert!(store.is_chart_fetching_at(7, stale + ChronoDuration::seconds(1)));

        cleanup(store, path);
    }

    fn test_track(id: &str) -> TrackRef {
        TrackRef {
            provider: ProviderKind::YouTube,
            content_id: id.into(),
            source_url: format!("https://example.test/{id}"),
            title: Some(id.into()),
            artist: None,
            duration: None,
            variant_key: None,
            is_live: false,
        }
    }

    #[test]
    fn migration_runner_reaches_latest_and_is_idempotent() {
        let (store, path) = temp_store("migrate");
        {
            let conn = store.conn.lock().unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, SCHEMA_VERSION);
        }
        drop(store);
        // 같은 파일을 다시 열어도 ALTER TABLE 이 두 번 돌지 않는다.
        let store = RemoteStore::open(&path).unwrap();
        cleanup(store, path);
    }

    #[test]
    fn legacy_style_database_upgrades_without_losing_rows() {
        // 러너 도입 이전 스키마(user_version = 0, 새 컬럼 없음)를 흉내낸다.
        let path = std::env::temp_dir().join(format!(
            "macham-legacy-{}.sqlite",
            crate::models::uuid_like()
        ));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_V1).unwrap();
            conn.execute(
                "INSERT INTO remote_chat_messages(guild_id, user_id, display_name, content, created_utc) VALUES(1, 10, 'old', 'hi', '2026-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
        }
        let store = RemoteStore::open(&path).unwrap();
        let messages = store.list_chat_messages(1, 10, CHAT_PAGE_LIMIT, None);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hi");
        assert!(messages[0].edited_utc.is_none());
        cleanup(store, path);
    }

    #[test]
    fn vote_updates_score_and_personal_likes() {
        let (store, path) = temp_store("remote");
        let mut item = QueueItem::new_user(test_track("song"), "requester".into(), Some(10));
        item.id = "item".into();
        store.ensure_queue_items(1, &[item.clone()]).unwrap();
        store
            .set_vote(1, &item.id, 20, Some(QueueVoteKind::SuperLike), &item.track)
            .unwrap();
        let points = VotePoints::default();
        let score = store.queue_scores(1)["item"].clone();
        assert_eq!(score.total_score(&points), 2);
        // 누가 눌렀는지도 같은 조회에서 나온다 (§10.4).
        assert_eq!(score.super_by, vec![20]);
        assert!(score.like_by.is_empty());
        assert_eq!(store.list_user_tracks(1, 20, UserTrackKind::Liked).len(), 1);
        store.set_vote(1, &item.id, 20, None, &item.track).unwrap();
        assert_eq!(store.queue_scores(1)["item"].total_score(&points), 0);
        assert!(
            store
                .list_user_tracks(1, 20, UserTrackKind::Liked)
                .is_empty()
        );

        // 싫어요는 점수를 깎고, 개인 "좋아요 목록"에는 들어가지 않는다 (§10.2).
        store
            .set_vote(1, &item.id, 21, Some(QueueVoteKind::Dislike), &item.track)
            .unwrap();
        let score = store.queue_scores(1)["item"].clone();
        assert_eq!(score.dislike_count, 1);
        assert_eq!(score.dislike_by, vec![21]);
        assert_eq!(score.total_score(&points), -1);
        assert!(store.list_user_tracks(1, 21, UserTrackKind::Liked).is_empty());
        cleanup(store, path);
    }

    #[test]
    fn marking_played_resets_wait_score_for_that_person_only() {
        let (store, path) = temp_store("fair");
        let mut mine = QueueItem::new_user(test_track("mine"), "민수".into(), Some(1));
        mine.id = "mine".into();
        let mut theirs = QueueItem::new_user(test_track("theirs"), "지훈".into(), Some(2));
        theirs.id = "theirs".into();
        store
            .ensure_queue_items(1, &[mine.clone(), theirs.clone()])
            .unwrap();
        store
            .increment_wait_scores(&["mine".into(), "theirs".into()])
            .unwrap();

        store.mark_requester_played(1, 1).unwrap();
        let scores = store.queue_scores(1);
        assert_eq!(scores["mine"].wait_score, 0);
        assert!(scores["mine"].last_played_utc.is_some());
        assert_eq!(scores["theirs"].wait_score, 1);
        assert!(scores["theirs"].last_played_utc.is_none());
        cleanup(store, path);
    }

    #[test]
    fn chat_reaction_is_a_toggle() {
        let (store, path) = temp_store("chat");
        let message = store
            .add_chat_message(1, 10, "tester", None, "hello", None)
            .unwrap();
        assert!(store.toggle_chat_reaction(1, message, 10, "👍").unwrap());
        assert_eq!(
            store.list_chat_messages(1, 10, 10, None)[0].reactions[0].count,
            1
        );
        assert!(!store.toggle_chat_reaction(1, message, 10, "👍").unwrap());
        assert!(
            store.list_chat_messages(1, 10, 10, None)[0]
                .reactions
                .is_empty()
        );
        cleanup(store, path);
    }

    #[test]
    fn chat_pagination_and_reply_preview_survive_the_cursor() {
        let (store, path) = temp_store("chatpage");
        let first = store
            .add_chat_message(1, 10, "민수", None, "첫 메시지입니다", None)
            .unwrap();
        for index in 0..5 {
            store
                .add_chat_message(1, 11, "지훈", None, &format!("메시지 {index}"), None)
                .unwrap();
        }
        let reply = store
            .add_chat_message(1, 12, "수연", None, "그거 좋다", Some(first))
            .unwrap();

        // 최신 2건만 보면 원문(first)은 범위 밖인데도 인용이 채워져야 한다.
        let page = store.list_chat_messages(1, 12, 2, None);
        assert_eq!(page.len(), 2);
        let last = page.last().unwrap();
        assert_eq!(last.id, reply);
        let preview = last.reply_to.as_ref().expect("인용 프리뷰 누락");
        assert_eq!(preview.id, first);
        assert_eq!(preview.display_name, "민수");
        assert_eq!(preview.excerpt, "첫 메시지입니다");
        assert!(!preview.deleted);

        // 커서로 과거를 더 읽으면 겹치지 않는다.
        let older = store.list_chat_messages(1, 12, 50, Some(page[0].id));
        assert!(older.iter().all(|message| message.id < page[0].id));
        assert_eq!(older.len(), 5);
        cleanup(store, path);
    }

    #[test]
    fn mentions_are_counted_until_read() {
        let (store, path) = temp_store("mention");
        let message = store
            .add_chat_message(1, 10, "민수", None, "@지훈 이거 들어봐", None)
            .unwrap();
        store.set_chat_mentions(1, message, &[11]).unwrap();
        assert_eq!(store.unread_mention_count(1, 11), 1);
        assert_eq!(store.unread_mention_count(1, 12), 0);
        assert_eq!(store.list_chat_messages(1, 11, 10, None)[0].mentions, vec![11]);
        assert_eq!(store.mark_mentions_read(1, 11).unwrap(), 1);
        assert_eq!(store.unread_mention_count(1, 11), 0);
        cleanup(store, path);
    }

    #[test]
    fn song_tags_round_trip_as_tracks() {
        let (store, path) = temp_store("tag");
        let track = test_track("아이브");
        let message = store
            .add_chat_message(1, 10, "민수", None, "#아이브 좋아", None)
            .unwrap();
        store
            .set_chat_tags(
                message,
                &[ChatTrackTag {
                    cache_key: track.cache_key(),
                    track: track.clone(),
                }],
            )
            .unwrap();
        let tags = &store.list_chat_messages(1, 10, 10, None)[0].tags;
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].track.content_id, "아이브");
        cleanup(store, path);
    }

    #[test]
    fn participants_include_requesters_without_chat_history() {
        let (store, path) = temp_store("participant");
        store
            .add_chat_message(1, 10, "민수", Some("avatar"), "안녕", None)
            .unwrap();
        let mut item = QueueItem::new_user(test_track("song"), "지훈".into(), Some(11));
        item.id = "song-item".into();
        store.ensure_queue_items(1, &[item]).unwrap();

        let participants = store.list_remote_participants(1);
        let ids: Vec<u64> = participants.iter().map(|p| p.user_id).collect();
        assert!(ids.contains(&10));
        assert!(ids.contains(&11));
        let 민수 = participants.iter().find(|p| p.user_id == 10).unwrap();
        assert_eq!(민수.display_name, "민수");
        assert_eq!(민수.avatar_url.as_deref(), Some("avatar"));
        // 채팅 기록이 없는 신청자는 빈 이름 — 호출부가 Discord 캐시로 채운다.
        assert_eq!(
            participants
                .iter()
                .find(|p| p.user_id == 11)
                .unwrap()
                .display_name,
            ""
        );
        cleanup(store, path);
    }

    #[test]
    fn chat_report_can_be_reviewed_and_resolved() {
        let (store, path) = temp_store("report");
        let message = store
            .add_chat_message(1, 10, "author", None, "review me", None)
            .unwrap();
        assert!(
            store
                .report_chat_message(1, message, 20, "reporter", "spam")
                .unwrap()
        );
        let reports = store.list_chat_reports(1, 10);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].message_content, "review me");
        assert!(store.resolve_chat_report(1, reports[0].id).unwrap());
        assert!(store.list_chat_reports(1, 10)[0].resolved_utc.is_some());
        cleanup(store, path);
    }

    #[test]
    fn suggestions_track_votes_and_status() {
        let (store, path) = temp_store("suggest");
        let id = store
            .create_suggestion(1, 10, "민수", None, "다크모드", "눈이 아프다")
            .unwrap();
        assert_eq!(store.toggle_suggestion_vote(1, id, 11).unwrap(), Some(true));
        let listed = store.list_suggestions(1, 11);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].vote_count, 1);
        assert!(listed[0].voted_by_me);
        assert!(!store.list_suggestions(1, 12)[0].voted_by_me);

        assert_eq!(store.toggle_suggestion_vote(1, id, 11).unwrap(), Some(false));
        assert_eq!(store.list_suggestions(1, 11)[0].vote_count, 0);

        assert!(
            store
                .set_suggestion_status(1, id, SuggestionStatus::Done, Some("반영했다"), 9)
                .unwrap()
        );
        let done = store.get_suggestion(1, id, 10).unwrap();
        assert_eq!(done.status, SuggestionStatus::Done);
        assert_eq!(done.status_note.as_deref(), Some("반영했다"));

        assert!(store.delete_suggestion(1, id).unwrap());
        assert!(store.list_suggestions(1, 10).is_empty());
        assert_eq!(store.toggle_suggestion_vote(1, id, 11).unwrap(), None);
        cleanup(store, path);
    }

    #[test]
    fn expired_suspensions_disappear_on_read() {
        let (store, path) = temp_store("suspend");
        let past = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let future = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        store
            .suspend_user(1, 10, SuspensionScope::Chat, Some("도배"), 9, Some(&past))
            .unwrap();
        store
            .suspend_user(1, 10, SuspensionScope::Queue, None, 9, Some(&future))
            .unwrap();
        store
            .suspend_user(1, 11, SuspensionScope::All, Some("무기한"), 9, None)
            .unwrap();

        let active = store.active_suspensions(1, 10);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].scope, SuspensionScope::Queue);
        assert_eq!(store.active_suspensions(1, 11).len(), 1);
        assert!(store.active_suspensions(1, 11)[0].expires_utc.is_none());

        assert_eq!(store.unsuspend_user(1, 10, None).unwrap(), 1);
        assert!(store.active_suspensions(1, 10).is_empty());
        assert_eq!(store.list_suspensions(1).len(), 1);
        cleanup(store, path);
    }

    #[test]
    /// 회귀: 역할 캐시가 메모리에만 있어서 재시작 뒤 429 가 나면 지정 역할 권한자가
    /// "권한이 없어요" 를 봤다. 디스크에 남아야 재시작을 건너뛰어도 등급이 유지된다.
    ///
    /// **`None` 과 `Some(vec![])` 를 구분하는지가 핵심이다.** 둘을 같게 다루면
    /// "아직 모른다" 가 "역할이 없다" 가 되어 다시 같은 버그가 난다.
    #[test]
    fn member_roles_survive_a_restart_and_unknown_differs_from_empty() {
        let (store, path) = temp_store("member-roles");

        // 저장한 적이 없으면 **빈 목록이 아니라 모름**이다.
        assert!(store.load_member_roles(1, 7, 6).is_none());

        store.save_member_roles(1, 7, &[100, 200]);
        assert_eq!(store.load_member_roles(1, 7, 6), Some(vec![100, 200]));

        // 역할이 진짜 하나도 없는 사람은 빈 목록으로 남아야 한다 — 모름이 아니다.
        store.save_member_roles(1, 8, &[]);
        assert_eq!(store.load_member_roles(1, 8, 6), Some(Vec::new()));

        // 유예 시간이 0이면 방금 적은 것도 못 쓴다(오래된 캐시로 권한을 열지 않는다).
        assert!(store.load_member_roles(1, 7, 0).is_none());

        // 다른 길드는 별개다.
        assert!(store.load_member_roles(2, 7, 6).is_none());
        cleanup(store, path);
    }

    /// 빈 채널 규칙은 기본이 **꺼짐**이고, 봇 주인이 강제로 걸면 서버 값이 무시된다 (§27).
    /// 강제가 기본이면 서버 주인이 아무것도 못 정한다 — 그래서 기본은 강제 안 함이다.
    #[test]
    fn empty_voice_defaults_to_off_and_force_overrides_the_guild() {
        use crate::models::EmptyVoiceChannelPolicy as P;
        use crate::remote::EmptyVoiceRule;

        let guild = RemoteGuildSettings::default();
        assert_eq!(guild.empty_voice_policy, P::DoNothing, "기본은 아무것도 안 함");
        assert_eq!(guild.empty_voice_delay_seconds, 300);

        let global = crate::models::GlobalSettings::default();
        assert!(!global.empty_voice_forced, "기본은 강제 안 함");

        // 잠금 문구는 강제일 때만 나온다.
        let free = EmptyVoiceRule {
            policy: P::AutoLeave,
            delay_seconds: 60,
            forced: false,
        };
        assert!(free.editable());
        assert!(free.lock_reason().is_none());

        let forced = EmptyVoiceRule { forced: true, ..free };
        assert!(!forced.editable());
        assert!(forced.lock_reason().is_some(), "왜 잠겼는지 말해야 한다");
    }

    /// 회귀: 봇을 내보냈다 다시 부르는 것만으로 차단이 풀리면 승인 자체가 의미가 없다.
    #[test]
    fn rejoining_does_not_reset_a_decision() {
        let (store, path) = temp_store("guild-approval");
        use crate::remote::GuildApprovalStatus as S;

        // 처음 보는 서버는 대기다.
        assert_eq!(store.register_guild(1, Some("첫 서버")), S::Pending);
        assert!(!S::Pending.is_usable());

        assert!(store.decide_guild(1, S::Approved, 999, Some("내 서버")));
        assert_eq!(store.guild_approval(1).unwrap().status, S::Approved);

        // 다시 초대돼도 승인 상태 그대로.
        assert_eq!(store.register_guild(1, Some("첫 서버")), S::Approved);

        // 차단한 뒤 재초대해도 대기로 안 돌아간다 — 여기가 핵심이다.
        assert!(store.decide_guild(1, S::Blocked, 999, None));
        assert_eq!(store.register_guild(1, Some("첫 서버")), S::Blocked);
        assert!(!S::Blocked.is_usable());

        // 이름은 최신으로 따라온다(운영 패널에서 알아봐야 하므로).
        store.register_guild(1, Some("이름 바뀜"));
        assert_eq!(
            store.guild_approval(1).unwrap().guild_name.as_deref(),
            Some("이름 바뀜")
        );

        // 결정한 사람과 시각이 남는다.
        let row = store.guild_approval(1).unwrap();
        assert_eq!(row.decided_by, Some(999));
        assert!(row.decided_utc.is_some());

        // 대기 중인 것이 목록 위로 온다.
        store.register_guild(2, Some("새 서버"));
        let listed = store.list_guild_approvals();
        assert_eq!(listed.first().map(|row| row.guild_id), Some(2));
        cleanup(store, path);
    }

    /// 회귀: 승인 게이트를 켠 순간 **쓰던 서버가 통째로 잠겼다.** 실제로 배포하고 나서
    /// 서버 3개가 명령어도 리모컨도 못 쓰게 됐다. 게이트는 앞으로 초대될 서버용이지
    /// 어제까지 잘 쓰던 서버를 막으려던 게 아니다.
    #[test]
    fn existing_guilds_are_grandfathered_but_decisions_survive() {
        use crate::remote::GuildApprovalStatus as S;
        let (store, path) = temp_store("grandfather");

        // 레거시 테이블은 이 러너가 만들지 않는다. 테스트에서만 흉내낸다.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS guild_metadata(guild_id INTEGER PRIMARY KEY, name TEXT);
                 INSERT OR REPLACE INTO guild_metadata VALUES(4242, '오래된 서버');
                 INSERT OR REPLACE INTO guild_metadata VALUES(777, '차단된 서버');",
            )
            .unwrap();
        }
        // 차단해 둔 서버는 마이그레이션이 되살리면 안 된다.
        store.register_guild(777, Some("차단된 서버"));
        store.decide_guild(777, S::Blocked, 1, None);
        // **게이트를 켠 빌드가 이미 대기로 등록해 둔 상태**를 재현한다. 실제로 이랬고,
        // INSERT OR IGNORE 만 있던 첫 수정본은 이걸 못 고쳐서 서버가 계속 잠겨 있었다.
        store.register_guild(4242, Some("오래된 서버"));
        assert_eq!(store.guild_approval(4242).map(|r| r.status), Some(S::Pending));

        {
            let conn = store.conn.lock().unwrap();
            migrate_v17_grandfather_guilds(&conn).unwrap();
        }

        // 알던 서버는 승인으로 넘어온다.
        assert_eq!(store.guild_approval(4242).map(|r| r.status), Some(S::Approved));
        // **판정이 있던 서버는 그대로다.**
        assert_eq!(store.guild_approval(777).map(|r| r.status), Some(S::Blocked));
        // 모르는 서버는 여전히 없다.
        assert!(store.guild_approval(9999).is_none());
        cleanup(store, path);
    }

    /// 레거시 테이블이 아직 없어도 마이그레이션이 통째로 실패하면 안 된다.
    /// 실패하면 저장소가 안 열리고 봇이 아예 못 뜬다.
    #[test]
    fn grandfathering_survives_missing_legacy_tables() {
        let (store, path) = temp_store("grandfather-empty");
        let conn = store.conn.lock().unwrap();
        assert!(migrate_v17_grandfather_guilds(&conn).is_ok());
        drop(conn);
        cleanup(store, path);
    }

    /// 재시작 이어듣기는 **한 번만** 쓰인다. 안 그러면 나중에 그냥 재시작했을 때도
    /// 몇 시간 전 곡이 되살아난다.
    #[test]
    fn resume_point_is_consumed_once() {
        let (store, path) = temp_store("resume");
        assert!(store.take_resume(1).is_none());

        store.save_resume(1, Some("item-9"), 123.5, false);
        let point = store.take_resume(1).expect("저장한 지점");
        assert_eq!(point.item_id.as_deref(), Some("item-9"));
        assert!((point.position_seconds - 123.5).abs() < 0.001);
        assert!(!point.was_paused);
        assert!(point.age_hours < 1.0);

        // 두 번째는 없어야 한다.
        assert!(store.take_resume(1).is_none());

        // 틀던 게 없으면 지운다 — 옛 기록이 남아 엉뚱한 곡이 살아나면 안 된다.
        store.save_resume(1, Some("item-9"), 10.0, true);
        store.clear_resume(1);
        assert!(store.take_resume(1).is_none());
        cleanup(store, path);
    }

    #[test]
    fn sessions_persist_by_hash_only() {
        let (store, path) = temp_store("session");
        let token = "super-secret-token";
        let session = StoredSession {
            user_id: 10,
            display_name: "민수".into(),
            avatar_url: None,
            guilds_json: "[]".into(),
            access_token: Some("access".into()),
            refresh_token: Some("refresh".into()),
            expires_utc: (chrono::Utc::now() + chrono::Duration::hours(12)).to_rfc3339(),
            refreshed_utc: None,
            created_utc: chrono::Utc::now().to_rfc3339(),
            csrf_token: Some("csrf-abc".into()),
        };
        store.save_session(token, &session).unwrap();
        let loaded = store.load_session(token).expect("세션 유실");
        assert_eq!(loaded.user_id, 10);
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
        assert!(store.load_session("wrong-token").is_none());

        // 회귀: CSRF 토큰이 그대로 돌아와야 한다. 복구할 때 새로 만들면 브라우저가 든
        // 옛 토큰과 어긋나서 **일시정지조차 CSRF 실패로 막힌다.**
        assert_eq!(loaded.csrf_token.as_deref(), Some("csrf-abc"));

        // 원문 토큰은 어디에도 남지 않는다.
        {
            let conn = store.conn.lock().unwrap();
            let hash: String = conn
                .query_row("SELECT token_hash FROM remote_web_sessions", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_ne!(hash, token);
            assert_eq!(hash, RemoteStore::session_token_hash(token));
        }

        assert!(store.delete_session(token).unwrap());
        assert!(store.load_session(token).is_none());
        cleanup(store, path);
    }

    #[test]
    fn expired_sessions_are_pruned() {
        let (store, path) = temp_store("session-prune");
        let session = StoredSession {
            user_id: 10,
            display_name: "민수".into(),
            avatar_url: None,
            guilds_json: "[]".into(),
            access_token: None,
            refresh_token: None,
            expires_utc: (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
            refreshed_utc: None,
            created_utc: chrono::Utc::now().to_rfc3339(),
            csrf_token: None,
        };
        store.save_session("dead", &session).unwrap();
        assert!(store.load_session("dead").is_none());
        assert_eq!(store.prune_sessions().unwrap(), 1);
        cleanup(store, path);
    }

    /// 회귀 테스트: RFC3339 문자열을 그대로 비교하면 'T' > ' ' 라 한 건도 안 지워졌다.
    #[test]
    fn prune_audit_actually_deletes_old_rows() {
        let (store, path) = temp_store("audit");
        let id = store
            .add_audit(1, 10, "민수", "playback.skip", None, None, None, true, None)
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_audit_logs SET created_utc = ?2 WHERE id = ?1",
                params![id, "2020-01-01T00:00:00+00:00"],
            )
            .unwrap();
        }
        assert_eq!(store.prune_audit(1, 14).unwrap(), 1);
        assert!(store.list_audit(1, 50, None).is_empty());
        cleanup(store, path);
    }

    #[test]
    fn prune_all_trims_chat_recent_and_failed_lyrics() {
        let (store, path) = temp_store("prune");
        let message = store
            .add_chat_message(1, 10, "민수", None, "오래된 메시지", None)
            .unwrap();
        store.save_lyrics_missing("youtube:none").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_chat_messages SET created_utc = '2020-01-01T00:00:00+00:00' WHERE id = ?1",
                params![message],
            )
            .unwrap();
            conn.execute(
                "UPDATE remote_lyrics SET fetched_utc = '2020-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
        }
        let report = store.prune_all(RetentionConfig::default()).unwrap();
        assert_eq!(report.chat, 1);
        assert_eq!(report.lyrics, 1);
        assert!(store.list_chat_messages(1, 10, 50, None).is_empty());
        cleanup(store, path);
    }

    #[test]
    fn pref_whitelist_rejects_unknown_keys_and_oversized_values() {
        let (store, path) = temp_store("prefs");
        let mut updates = BTreeMap::new();
        updates.insert("layout".into(), "panel".into());
        updates.insert("theme".into(), "light".into());
        updates.insert("webVolume".into(), "45".into());
        // 화이트리스트 밖 키 — 조용히 버린다.
        updates.insert("evilKey".into(), "payload".into());
        // 값 자체가 틀린 것들.
        updates.insert("layout".into(), "panel".into());
        updates.insert("lyricsOpen".into(), "yes".into());
        updates.insert("panelLayout".into(), "{ not json".into());
        // layoutSizes 2KB 상한 초과 (JSON 자체는 멀쩡하다).
        updates.insert("layoutSizes".into(), format!("[\"{}\"]", "x".repeat(3000)));
        store.save_prefs(10, &updates).unwrap();

        let saved = store.load_prefs(10);
        assert_eq!(saved.get("layout").map(String::as_str), Some("panel"));
        assert_eq!(saved.get("theme").map(String::as_str), Some("light"));
        assert_eq!(saved.get("webVolume").map(String::as_str), Some("45"));
        assert!(!saved.contains_key("evilKey"));
        assert!(!saved.contains_key("lyricsOpen"));
        assert!(!saved.contains_key("panelLayout"));
        assert!(!saved.contains_key("layoutSizes"), "2KB 상한이 안 걸렸다");

        // 다른 사람 설정은 섞이지 않는다.
        assert!(store.load_prefs(11).is_empty());

        // 부분 갱신 — 보낸 키만 바뀐다.
        let mut patch = BTreeMap::new();
        patch.insert("theme".into(), "dark".into());
        store.save_prefs(10, &patch).unwrap();
        let saved = store.load_prefs(10);
        assert_eq!(saved.get("theme").map(String::as_str), Some("dark"));
        assert_eq!(saved.get("layout").map(String::as_str), Some("panel"));

        // 되돌리기 — 지운 키는 다시 미선택 상태.
        assert_eq!(store.delete_prefs(10, &["layout"]).unwrap(), 1);
        assert!(!store.load_prefs(10).contains_key("layout"));
        cleanup(store, path);
    }

    /// **화면이 보내는 키는 여기 전부 있어야 한다.**
    ///
    /// 하나라도 빠지면 그 키만 못 쓰는 게 아니다. `api_prefs_put` 이 배치 전체를 거절하고,
    /// 화면은 여러 설정을 300ms 동안 모아 한 번에 보낸다 — 같이 실린 멀쩡한 값까지 날아간다.
    /// `nowVoters` 가 실제로 그랬다.
    #[test]
    fn every_pref_the_screen_sends_is_accepted() {
        for (key, value) in [
            ("layout", "three"),
            ("theme", "dark"),
            ("lyricsOpen", "1"),
            ("webPlayback", "1"),
            ("webVolume", "60"),
            ("webOffset", "0"),
            ("nowVoters", "0"),
            ("videoSize", "3"),
            ("devPos", "120,80"),
            ("devSize", "640,420"),
        ] {
            assert!(is_valid_pref(key, value), "화면이 보내는 '{key}' 를 거절해요");
        }
    }

    #[test]
    fn pref_validation_matches_the_spec_table() {
        assert!(is_valid_pref("layout", "three"));
        assert!(is_valid_pref("layout", "two"));
        assert!(is_valid_pref("layout", "panel"));
        assert!(!is_valid_pref("layout", "four"));
        assert!(is_valid_pref("theme", "dark"));
        assert!(!is_valid_pref("theme", "solarized"));
        assert!(is_valid_pref("lyricsOpen", "0"));
        assert!(is_valid_pref("webPlayback", "1"));
        assert!(!is_valid_pref("webPlayback", "true"));
        assert!(is_valid_pref("webVolume", "0"));
        assert!(is_valid_pref("webVolume", "100"));
        assert!(!is_valid_pref("webVolume", "101"));
        assert!(!is_valid_pref("webVolume", "-1"));
        assert!(is_valid_pref("layoutSizes", r#"{"three":{"rail":320}}"#));
        assert!(!is_valid_pref("layoutSizes", "그냥 문자열"));
        assert!(is_valid_pref(
            "panelLayout",
            r#"{"type":"tabs","panels":["now"]}"#
        ));
        assert!(!is_valid_pref("panelLayout", &"\"".repeat(9000)));
        assert!(!is_valid_pref("unknown", "value"));
        // v3 에서 넓어진 값들 (§7.2 배치 6종 · §17.1 테마 7종 + auto).
        for layout in LAYOUT_VALUES {
            assert!(is_valid_pref("layout", layout), "배치 {layout} 이 막혔다");
        }
        for theme in THEME_VALUES {
            assert!(is_valid_pref("theme", theme), "테마 {theme} 이 막혔다");
        }
        assert!(!is_valid_pref("layout", "grid"));
        assert!(!is_valid_pref("theme", "solarized"));
        // 로그 필터 칩(§13.4)과 알림 설정(§16 B3).
        assert!(is_valid_pref("auditFilter", "song,playlist"));
        assert!(is_valid_pref("auditFilter", "none"));
        assert!(!is_valid_pref("auditFilter", "song,없는분류"));
        assert!(is_valid_pref("notify", r#"{"song":1,"mention":0}"#));
        assert!(!is_valid_pref("notify", "song"));

        // 역할을 판정하는 데 실제로 목록이 필요한 규칙만 true 여야 한다.
        // 여기가 틀리면 조회 실패 때 멀쩡한 거절까지 "잠시 뒤 다시" 로 바뀐다.
        // 디스코드 명령어 기록은 **어디서 했는지가 문장에 있어야** 한다 (§32).
        // 그리고 재생 계열은 Playback 으로 분류돼야 로그 필터에서 보인다.
        use crate::remote::{AuditKind as K, audit_kind_for, audit_text};
        assert_eq!(audit_kind_for("discord.skip"), K::Playback);
        assert_eq!(audit_kind_for("discord.play"), K::Song);
        assert_eq!(audit_kind_for("discord.playlist"), K::Playlist);
        let line = audit_text("discord.skip", "마참", Some("곡을 넘겼어요"), None, None, 1);
        assert_eq!(line, "마참님이 디스코드에서 곡을 넘겼어요");
        assert!(line.contains("디스코드"), "출처가 빠지면 리모컨 기록과 구분이 안 된다");

        use crate::remote::PermissionRule as Rule;
        assert!(Rule::ConfiguredRole.needs_roles());
        assert!(Rule::Administrator.needs_roles());
        assert!(!Rule::GuildMember.needs_roles());
        assert!(!Rule::SameVoiceChannel.needs_roles());
        assert!(!Rule::Disabled.needs_roles());

        // 싱크 보정은 **음수가 정상값**이다. 볼륨처럼 부호를 막으면 앞으로 당기질 못한다.
        assert!(is_valid_pref("webOffset", "-2.5"));
        assert!(is_valid_pref("webOffset", "0"));
        assert!(is_valid_pref("webOffset", "10"));
        assert!(!is_valid_pref("webOffset", "10.5"), "한계를 넘으면 곡을 잘못 맞춘 것이다");
        assert!(!is_valid_pref("webOffset", "-11"));
        assert!(!is_valid_pref("webOffset", "NaN"));
        assert!(!is_valid_pref("webOffset", "inf"));
        assert!(!is_valid_pref("webOffset", "2초"));
        // 화이트리스트 상수와 판정이 어긋나지 않는지.
        for key in PREF_KEYS {
            assert!(!is_valid_pref(key, ""), "빈 값은 어떤 키도 통과하면 안 된다");
        }
    }

    #[test]
    fn autoplay_seeds_are_capped_at_ten_and_reject_duplicates() {
        let (store, path) = temp_store("seeds");
        for index in 0..MAX_AUTOPLAY_SEEDS {
            let outcome = store
                .add_autoplay_seed(1, &test_track(&format!("seed{index}")), 10)
                .unwrap();
            assert_eq!(outcome, SeedAddOutcome::Added);
        }
        assert_eq!(store.list_autoplay_seeds(1).len(), MAX_AUTOPLAY_SEEDS);

        // 11번째는 거부. 문구까지 계약이다.
        let overflow = store.add_autoplay_seed(1, &test_track("seed11"), 10).unwrap();
        assert_eq!(overflow, SeedAddOutcome::LimitReached(10));
        assert_eq!(overflow.message(), "시드곡은 10곡까지 넣을 수 있어요.");
        assert_eq!(store.list_autoplay_seeds(1).len(), MAX_AUTOPLAY_SEEDS);

        // 중복도 거부하되 상한과는 다른 사유다.
        assert!(store.remove_autoplay_seed(1, &test_track("seed0").cache_key()).unwrap());
        assert_eq!(store.list_autoplay_seeds(1).len(), MAX_AUTOPLAY_SEEDS - 1);
        assert_eq!(
            store.add_autoplay_seed(1, &test_track("seed1"), 10).unwrap(),
            SeedAddOutcome::Duplicate
        );
        assert!(!store.remove_autoplay_seed(1, "없는키").unwrap());

        // 길드끼리 섞이지 않는다.
        assert!(store.list_autoplay_seeds(2).is_empty());
        cleanup(store, path);
    }

    #[test]
    fn reordering_seeds_keeps_unlisted_songs_at_the_back() {
        let (store, path) = temp_store("seed-order");
        for name in ["가", "나", "다"] {
            store
                .add_autoplay_seed(1, &test_track(name), 10)
                .unwrap();
        }
        let keys: Vec<String> = store
            .list_autoplay_seeds(1)
            .iter()
            .map(|seed| seed.cache_key.clone())
            .collect();
        assert_eq!(keys.len(), 3);

        // 3 → 1번째로 끌어올린다.
        store
            .reorder_autoplay_seeds(1, &[keys[2].clone(), keys[0].clone(), keys[1].clone()])
            .unwrap();
        let after: Vec<String> = store
            .list_autoplay_seeds(1)
            .iter()
            .map(|seed| seed.cache_key.clone())
            .collect();
        assert_eq!(after, vec![keys[2].clone(), keys[0].clone(), keys[1].clone()]);

        // 목록에 빠진 곡은 사라지지 않고 뒤로 간다. 없는 키는 무시된다.
        store
            .reorder_autoplay_seeds(1, &[keys[1].clone(), "유령키".into()])
            .unwrap();
        let after: Vec<String> = store
            .list_autoplay_seeds(1)
            .iter()
            .map(|seed| seed.cache_key.clone())
            .collect();
        assert_eq!(after.len(), 3);
        assert_eq!(after[0], keys[1]);
        cleanup(store, path);
    }

    /// 보존 정리가 개인 설정과 기준 곡을 건드리면 안 된다.
    #[test]
    fn prune_all_leaves_prefs_and_seeds_alone() {
        let (store, path) = temp_store("prune-prefs");
        let mut prefs = BTreeMap::new();
        prefs.insert("layout".into(), "two".into());
        store.save_prefs(10, &prefs).unwrap();
        store.add_autoplay_seed(1, &test_track("seed"), 10).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_user_prefs SET updated_utc = '2020-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE remote_autoplay_seeds SET added_utc = '2020-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
        }
        store.prune_all(RetentionConfig::default()).unwrap();
        assert_eq!(store.load_prefs(10).get("layout").map(String::as_str), Some("two"));
        assert_eq!(store.list_autoplay_seeds(1).len(), 1);
        cleanup(store, path);
    }

    // ───────── V3 §10.6 슈퍼 좋아요 제한 ─────────

    /// 기본은 둘 다 꺼져 있고(기존 서버 동작 유지), 켜면 정확히 그 이유로 막는다.
    /// **취소하면 횟수를 돌려준다** — 실수로 누른 걸 하루 종일 못 쓰게 하면 가혹하다.
    #[test]
    fn super_like_limits_are_off_by_default_and_refund_on_cancel() {
        let (store, path) = temp_store("superlike");
        // 기본(0/0) — 몇 번을 써도 안 막힌다.
        for _ in 0..20 {
            assert!(store.consume_super_like(1, 10, 0, 0).is_allowed());
        }
        assert!(store.check_super_like(1, 10, 0, 0).is_allowed());

        // 하루 3번으로 조이면 이미 20번 쓴 사람은 바로 막힌다.
        let verdict = store.check_super_like(1, 10, 0, 3);
        assert_eq!(verdict, SuperLikeVerdict::DailyLimitReached { limit: 3 });
        assert!(verdict.message().unwrap().contains("UTC 자정에 초기화돼요"));

        // 다른 사람·다른 서버는 따로 센다.
        assert!(store.check_super_like(1, 11, 0, 3).is_allowed());
        assert!(store.check_super_like(2, 10, 0, 3).is_allowed());

        // 취소하면 횟수가 돌아온다.
        let before = store.super_like_used_today(1, 10);
        assert_eq!(store.refund_super_like(1, 10), before - 1);
        // 0 아래로는 안 내려간다.
        for _ in 0..50 {
            store.refund_super_like(1, 10);
        }
        assert_eq!(store.super_like_used_today(1, 10), 0);
        cleanup(store, path);
    }

    /// 쿨타임은 메모리로 충분하고, 남은 시간을 초 단위로 정확히 말해 준다.
    #[test]
    fn super_like_cooldown_says_how_long_is_left() {
        let (store, path) = temp_store("superlike-cool");
        assert!(store.consume_super_like(1, 10, 300, 0).is_allowed());
        match store.check_super_like(1, 10, 300, 0) {
            SuperLikeVerdict::Cooldown { remaining_sec } => {
                assert!(remaining_sec > 290 && remaining_sec <= 300);
            }
            other => panic!("쿨타임이 안 걸렸다: {other:?}"),
        }
        // 쿨타임 설정이 0이면 아무리 눌러도 안 걸린다.
        assert!(store.check_super_like(1, 10, 0, 0).is_allowed());

        let status = store.super_like_status(1, 10, 300, 5);
        assert_eq!(status.used_today, 1);
        assert_eq!(status.remaining, Some(4));
        assert!(status.available_at_utc.is_some());
        // 무제한이면 남은 횟수를 세지 않는다.
        assert!(store.super_like_status(1, 10, 0, 0).remaining.is_none());
        cleanup(store, path);
    }

    // ───────── V3 §13 활동 로그 ─────────

    /// 문장과 분류는 서버가 채운다. 옛 줄(문장 없음)도 읽는 자리에서 문장이 만들어진다.
    #[test]
    fn audit_rows_carry_a_kind_and_a_human_sentence() {
        let (store, path) = temp_store("audit-kind");
        store
            .add_audit(1, 10, "민수", "queue.add", Some("I AM"), None, None, true, None)
            .unwrap();
        store
            .add_audit(1, 11, "지훈", "settings.maxVolume", Some("최대 볼륨"), Some("200"), Some("150"), true, None)
            .unwrap();

        let entries = store.list_audit(1, 50, None);
        assert_eq!(entries.len(), 2);
        let song = entries.iter().find(|e| e.action == "queue.add").unwrap();
        assert_eq!(song.kind, AuditKind::Song);
        assert_eq!(song.text, "민수님이 **I AM** 을 담았어요");
        let admin = entries.iter().find(|e| e.kind == AuditKind::Admin).unwrap();
        assert!(admin.text.contains("200 → 150"));

        // 유저 투영에는 전후값 JSON 이 아예 없다 (§13.2).
        let feed = admin.feed_item();
        let json = serde_json::to_string(&feed).unwrap();
        assert!(!json.contains("beforeValue"));
        assert!(!json.contains("failureReason"));
        assert!(json.contains("actorName"));
        cleanup(store, path);
    }

    /// 같은 사람이 같은 종류를 60초 안에 반복하면 **새 줄을 만들지 않는다** (§13.3).
    /// 펼치면 무엇이 들어갔는지 목록이 나와야 한다.
    #[test]
    fn repeated_actions_merge_into_one_line() {
        let (store, path) = temp_store("audit-merge");
        let mut ids = Vec::new();
        for title in ["곡1", "곡2", "곡3"] {
            ids.push(
                store
                    .add_audit(1, 10, "민수", "queue.add", Some(title), None, None, true, None)
                    .unwrap(),
            );
        }
        // 세 번 담았지만 줄은 하나다.
        assert!(ids.iter().all(|id| *id == ids[0]));
        let entries = store.list_audit(1, 50, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].merged_count, 3);
        assert_eq!(entries[0].text, "민수님이 곡 3개를 담았어요");
        assert_eq!(entries[0].merged_items, vec!["곡1", "곡2", "곡3"]);
        assert_eq!(entries[0].feed_item().merged_count, Some(3));

        // 다른 사람은 따로 쌓인다.
        store
            .add_audit(1, 11, "지훈", "queue.add", Some("곡4"), None, None, true, None)
            .unwrap();
        assert_eq!(store.list_audit(1, 50, None).len(), 2);

        // 합치지 않는 종류는 매번 새 줄이다.
        store
            .add_audit(1, 10, "민수", "playback.skip", None, None, None, true, None)
            .unwrap();
        store
            .add_audit(1, 10, "민수", "playback.skip", None, None, None, true, None)
            .unwrap();
        assert_eq!(store.list_audit(1, 50, None).len(), 4);
        cleanup(store, path);
    }

    /// 한 번에 담기는 사람 피드에 한 줄, 곡 목록은 펼침용으로 남는다 (§13.3).
    #[test]
    fn bulk_enqueue_leaves_exactly_one_line() {
        let (store, path) = temp_store("audit-bulk");
        let songs: Vec<String> = (1..=50).map(|index| format!("곡{index}")).collect();
        store
            .add_audit_bulk(1, 10, "민수", "playlist.enqueue", Some("밤샘용"), 50, &songs)
            .unwrap();
        let entries = store.list_audit(1, 50, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].text,
            "민수님이 재생목록 **밤샘용** 에서 50곡을 담았어요"
        );
        assert_eq!(entries[0].merged_items.len(), 50);
        assert_eq!(entries[0].kind, AuditKind::Song);
        cleanup(store, path);
    }

    #[test]
    fn audit_can_be_filtered_by_kind() {
        let (store, path) = temp_store("audit-filter");
        store
            .add_audit(1, 10, "민수", "queue.add", Some("곡"), None, None, true, None)
            .unwrap();
        store
            .add_audit(1, 10, "민수", "vote.like", Some("곡"), None, None, true, None)
            .unwrap();
        store
            .add_audit(1, 10, "민수", "settings.update", None, None, None, true, None)
            .unwrap();

        assert_eq!(store.list_audit(1, 50, None).len(), 3);
        assert_eq!(
            store.list_audit_kinds(1, 50, None, &[AuditKind::Vote]).len(),
            1
        );
        // 기본 필터(곡 + 재생목록)에는 투표와 설정이 안 잡힌다.
        let default_view = store.list_audit_kinds(1, 50, None, &AuditKind::default_filter());
        assert_eq!(default_view.len(), 1);
        assert_eq!(default_view[0].kind, AuditKind::Song);
        cleanup(store, path);
    }

    /// 투표·재생은 3일, 나머지는 설정값. `0`이면 한 줄도 안 지운다 (§13.6 · §23.1).
    #[test]
    fn audit_retention_is_per_kind_and_zero_means_forever() {
        let (store, path) = temp_store("audit-retention");
        let song = store
            .add_audit(1, 10, "민수", "queue.add", Some("곡"), None, None, true, None)
            .unwrap();
        let vote = store
            .add_audit(1, 10, "민수", "vote.like", Some("곡"), None, None, true, None)
            .unwrap();
        // 둘 다 5일 전으로 밀어 둔다.
        let five_days_ago = (chrono::Utc::now() - chrono::Duration::days(5)).to_rfc3339();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_audit_logs SET created_utc = ?1",
                params![five_days_ago],
            )
            .unwrap();
        }

        // 무제한이면 아무것도 안 지운다 — `.max(1)` 이 남아 있으면 여기서 전부 날아간다.
        assert_eq!(store.prune_audit(1, 0).unwrap(), 0);
        assert_eq!(store.list_audit(1, 50, None).len(), 2);

        // 14일 설정이어도 투표는 3일만 남는다.
        assert_eq!(store.prune_audit(1, 14).unwrap(), 1);
        let left = store.list_audit(1, 50, None);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, song);
        assert_ne!(left[0].id, vote);
        cleanup(store, path);
    }

    // ───────── V3 §8.5 자동재생 정책 입력 ─────────

    /// `📻 이 곡 말고`로 뺀 곡은 7일간 다시 안 뽑히고, 기한이 지나면 스스로 사라진다.
    #[test]
    fn blocked_autoplay_candidates_expire_on_their_own() {
        let (store, path) = temp_store("blocked");
        let track = test_track("싫은곡");
        store
            .block_autoplay_candidate(1, &track, Some("이 곡 말고"))
            .unwrap();
        assert!(store.blocked_autoplay_keys(1).contains(&track.cache_key()));
        // 길드끼리 섞이지 않는다.
        assert!(store.blocked_autoplay_keys(2).is_empty());

        // 기한이 지난 행은 읽는 자리에서 사라진다.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_autoplay_blocked SET until_utc = ?1",
                params![(chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339()],
            )
            .unwrap();
        }
        assert!(store.blocked_autoplay_keys(1).is_empty());

        store
            .block_autoplay_candidate(1, &track, None)
            .unwrap();
        assert!(store.unblock_autoplay_candidate(1, &track.cache_key()).unwrap());
        assert!(!store.unblock_autoplay_candidate(1, "없는키").unwrap());
        cleanup(store, path);
    }

    /// 이력 감쇠·아티스트 쿨다운이 쓸 입력을 한 번의 조회로 만든다 (§8.5-1·2).
    #[test]
    /// 빼 둔 곡은 **제목이 같이 남아야** 한다 (§8.7).
    ///
    /// 예전에는 `cache_key` 만 저장해서 화면이 `youtube:xxx` 를 제목 자리에 그렸다.
    /// 이제 차단할 때 트랙을 통째로 남긴다.
    #[test]
    fn blocking_keeps_the_track_so_the_screen_can_name_it() {
        let (store, path) = temp_store("blocked-title");
        let track = test_track("빼둘곡");
        store.block_autoplay_candidate(1, &track, Some("이 곡 말고")).unwrap();

        let rows = store.list_blocked_autoplay(1);
        assert_eq!(rows.len(), 1);
        let (key, reason, _until, saved) = &rows[0];
        assert_eq!(key, &track.cache_key());
        assert_eq!(reason.as_deref(), Some("이 곡 말고"));
        assert_eq!(
            saved.as_ref().and_then(|t| t.title.clone()),
            track.title.clone(),
            "차단 시점에 제목이 같이 남아야 한다"
        );
        cleanup(store, path);
    }

    /// v20 이전에 쌓인 줄은 트랙이 없다. **이미 빼 둔 곡이 문제의 전부**라
    /// 컬럼만 붙이고 끝내면 화면은 그대로 코드를 보여 준다. 이웃 표에서 찾아 채운다.
    #[test]
    fn old_blocked_rows_get_their_titles_back_from_neighbours() {
        let (store, path) = temp_store("blocked-backfill");
        let from_seed = test_track("기준곡에있던곡");
        let from_recent = test_track("최근에튼곡");
        let orphan = test_track("어디에도없는곡");

        // 이웃 표에 흔적을 만든다.
        store
            .add_autoplay_seed(1, &from_seed, 10)
            .expect("기준 곡 추가");
        let mut item = QueueItem::new_user(from_recent.clone(), "민수".into(), Some(10));
        item.id = "recent".into();
        store.record_recent(1, &item, "completed").unwrap();

        // v20 이전 상태를 흉내낸다 — 트랙 없이 키만 있는 줄.
        {
            let conn = store.conn.lock().unwrap();
            for track in [&from_seed, &from_recent, &orphan] {
                conn.execute(
                    "INSERT INTO remote_autoplay_blocked(guild_id, cache_key, until_utc, reason, created_utc, track_json)
                     VALUES(1, ?1, ?2, NULL, ?3, NULL)",
                    params![
                        track.cache_key(),
                        (Utc::now() + ChronoDuration::days(3)).to_rfc3339(),
                        RemoteStore::now_iso()
                    ],
                )
                .unwrap();
            }
            backfill_blocked_tracks(&conn).unwrap();
        }

        let found: std::collections::HashMap<String, Option<String>> = store
            .list_blocked_autoplay(1)
            .into_iter()
            .map(|(key, _, _, track)| (key, track.and_then(|t| t.title)))
            .collect();
        assert_eq!(
            found.get(&from_seed.cache_key()).cloned().flatten().as_deref(),
            Some("기준곡에있던곡"),
            "기준 곡에서 제목을 찾아야 한다"
        );
        assert_eq!(
            found.get(&from_recent.cache_key()).cloned().flatten().as_deref(),
            Some("최근에튼곡"),
            "최근 재생에서도 찾아야 한다 (cache_key 컬럼이 없어 계산으로 맞춘다)"
        );
        assert!(
            found.get(&orphan.cache_key()).cloned().flatten().is_none(),
            "흔적이 없으면 못 찾는 게 맞다 — 화면이 그 사정을 말한다"
        );
        cleanup(store, path);
    }

    /// 최근 재생은 **한 줄씩** 지워야 한다 (§8.7).
    ///
    /// 같은 곡을 여러 번 틀면 같은 `cache_key` 로 여러 줄이 쌓인다. 키로 지우면
    /// "이 한 번"을 빼려던 게 그 곡 이력을 통째로 날린다. 그래서 행 id 로 지운다.
    /// 남의 길드 이력을 id 만으로 지울 수 없어야 한다는 것도 같이 못 박는다.
    #[test]
    fn recent_rows_are_removed_one_at_a_time_and_stay_inside_the_guild() {
        let (store, path) = temp_store("recent-remove");
        let track = test_track("같은곡");
        let mut first = QueueItem::new_user(track.clone(), "민수".into(), Some(10));
        first.id = "first".into();
        let mut second = QueueItem::new_user(track.clone(), "민수".into(), Some(10));
        second.id = "second".into();
        store.record_recent(1, &first, "completed").unwrap();
        store.record_recent(1, &second, "completed").unwrap();
        // 다른 길드에도 같은 곡이 한 줄 있다.
        let mut other = QueueItem::new_user(track.clone(), "지훈".into(), Some(11));
        other.id = "other".into();
        store.record_recent(2, &other, "completed").unwrap();

        let rows = store.list_recent(1, 10);
        assert_eq!(rows.len(), 2, "같은 곡이라도 튼 횟수만큼 쌓인다");

        // 한 줄만 지운다 — 나머지 한 줄은 남아야 한다.
        assert!(store.remove_recent(1, rows[0].id).unwrap());
        let left = store.list_recent(1, 10);
        assert_eq!(left.len(), 1, "키가 같아도 한 줄만 지워진다");

        // 길드가 다르면 id 가 맞아도 안 지워진다.
        let outsider = store.list_recent(2, 10);
        assert_eq!(outsider.len(), 1);
        assert!(!store.remove_recent(1, outsider[0].id).unwrap());
        assert_eq!(store.list_recent(2, 10).len(), 1, "남의 서버 이력은 못 지운다");

        cleanup(store, path);
    }

    #[test]
    fn recent_history_gives_ages_and_artists_for_the_policy() {
        let (store, path) = temp_store("recent-history");
        let mut old = test_track("옛날곡");
        old.artist = Some("아이브".into());
        let mut fresh = test_track("방금곡");
        fresh.artist = Some("뉴진스".into());

        let mut item_old = QueueItem::new_user(old.clone(), "민수".into(), Some(10));
        item_old.id = "old".into();
        let mut item_fresh = QueueItem::new_user(fresh.clone(), "민수".into(), Some(10));
        item_fresh.id = "fresh".into();
        store.record_recent(1, &item_old, "completed").unwrap();
        store.record_recent(1, &item_fresh, "completed").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_recent_tracks SET played_utc = ?1 WHERE track_json LIKE '%옛날곡%'",
                params![(chrono::Utc::now() - chrono::Duration::hours(30)).to_rfc3339()],
            )
            .unwrap();
        }

        let (ages, artists) = store.recent_play_history(1, 50);
        assert!(ages[&old.cache_key()] > 29.0);
        assert!(ages[&fresh.cache_key()] < 1.0);
        // 최신순 — 방금 튼 곡의 가수가 앞이다.
        assert_eq!(artists.first().map(String::as_str), Some("뉴진스"));
        assert!(artists.contains(&"아이브".to_string()));
        cleanup(store, path);
    }

    // ───────── V3 §15 차트 ─────────

    /// 기본 제공 차트가 심어지고, 마이그레이션을 다시 돌려도 중복되지 않는다.
    #[test]
    fn builtin_charts_are_seeded_once_and_cover_every_category() {
        let (store, path) = temp_store("charts");
        let charts = store.list_charts(1);
        assert_eq!(charts.len(), BUILTIN_CHARTS.len());
        for category in ChartCategory::ALL {
            assert!(
                charts.iter().any(|chart| chart.category == category),
                "{} 분류의 기본 차트가 없다",
                category.as_str()
            );
        }
        // 우리 차트는 바깥에서 가져오지 않는다 (§15.2b).
        assert!(
            charts
                .iter()
                .filter(|chart| chart.category == ChartCategory::Ours)
                .all(|chart| chart.is_internal())
        );
        // 전부 공용이고 지울 수 없다.
        assert!(charts.iter().all(|chart| chart.guild_id.is_none() && chart.builtin));
        let builtin_id = charts[0].id;
        assert!(!store.remove_chart(1, builtin_id).unwrap());

        // 다시 심어도 늘지 않는다.
        {
            let conn = store.conn.lock().unwrap();
            seed_builtin_charts(&conn).unwrap();
        }
        assert_eq!(store.list_charts(1).len(), BUILTIN_CHARTS.len());
        cleanup(store, path);
    }

    /// 캐시는 TTL 이 지나면 낡은 것이 되고, 실패는 숨기지 않고 그대로 남는다 (§15.1 · §15.2).
    /// **시간을 상수에서 끌어온다** — 예전엔 6시간이 테스트에 박혀 있어서 TTL 을 늘리자 깨졌다.
    #[test]
    fn chart_cache_goes_stale_after_the_ttl_and_records_failures() {
        let (store, path) = temp_store("chart-cache");
        let chart_id = store
            .add_chart(1, ChartCategory::Genre, "우리 장르", "YouTube", "ytsearch10:테스트")
            .unwrap();
        assert!(store.chart_cache(chart_id).is_none());

        store
            .save_chart_cache(chart_id, &[test_track("곡1"), test_track("곡2")])
            .unwrap();
        let snapshot = store.chart_cache(chart_id).expect("캐시 유실");
        assert_eq!(snapshot.tracks.len(), 2);
        assert!(!snapshot.stale, "방금 받은 캐시가 낡았다고 나온다");
        assert_eq!(store.get_chart(1, chart_id).unwrap().track_count, 2);

        // TTL 을 넘기면 낡은 것으로 보되 내용은 그대로 준다 — 빈 화면보다 낫다.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_chart_cache SET fetched_utc = ?1",
                params![(chrono::Utc::now()
                    - chrono::Duration::hours(crate::remote::models::CHART_CACHE_TTL_HOURS + 1))
                .to_rfc3339()],
            )
            .unwrap();
        }
        assert!(store.chart_cache(chart_id).unwrap().stale);

        // 실패는 그대로 남는다.
        store.mark_chart_failure(chart_id, "재생목록이 비어 있어요").unwrap();
        let chart = store.get_chart(1, chart_id).unwrap();
        assert!(chart.last_failure_utc.is_some());
        assert_eq!(chart.last_failure_reason.as_deref(), Some("재생목록이 비어 있어요"));

        // 주소를 갈면 옛 곡 목록은 버린다 — 거짓말이 되니까.
        assert!(store.update_chart(chart_id, None, Some("ytsearch10:다른것"), None, None).unwrap());
        assert!(store.chart_cache(chart_id).is_none());

        // 길드 차트는 지울 수 있고, 남의 길드 것은 못 지운다.
        assert!(!store.remove_chart(2, chart_id).unwrap());
        assert!(store.remove_chart(1, chart_id).unwrap());
        cleanup(store, path);
    }

    /// 동시에 같은 차트를 요청하면 **하나만 yt-dlp 를 돌린다** (§15.1).
    #[test]
    fn only_one_fetch_runs_per_chart() {
        let (store, path) = temp_store("chart-inflight");
        assert!(store.try_begin_chart_fetch(7));
        assert!(!store.try_begin_chart_fetch(7), "두 번째 요청이 같이 돌고 있다");
        assert!(store.is_chart_fetching(7));
        // 다른 차트는 막히지 않는다.
        assert!(store.try_begin_chart_fetch(8));
        store.end_chart_fetch(7);
        assert!(!store.is_chart_fetching(7));
        assert!(store.try_begin_chart_fetch(7));
        cleanup(store, path);
    }

    /// 길드 차트는 자기 서버에서만 보인다.
    #[test]
    fn guild_charts_stay_in_their_guild() {
        let (store, path) = temp_store("chart-scope");
        let mine = store
            .add_chart(1, ChartCategory::Karaoke, "우리 노래방", "YouTube", "ytsearch10:노래방")
            .unwrap();
        assert!(store.list_charts(1).iter().any(|chart| chart.id == mine));
        assert!(!store.list_charts(2).iter().any(|chart| chart.id == mine));
        // 공용 차트는 양쪽 다 본다.
        assert!(store.list_charts(2).iter().any(|chart| chart.builtin));
        cleanup(store, path);
    }

    // ───────── V3 §23.1 무제한 ─────────

    /// 설정 저장이 `0 = 무제한`을 실제로 지키는지. 저장했다 다시 읽어도 0이 살아 있어야 하고,
    /// 범위를 벗어난 값은 저장 시점에 잘려야 한다 (§23.1).
    #[test]
    fn saved_settings_keep_zero_as_unlimited() {
        let (store, path) = temp_store("settings-zero");
        with_legacy_settings_table(&store);
        store
            .save_guild_settings(&RemoteGuildSettings {
                guild_id: 1,
                max_queue_per_user: 0,
                max_queue_per_guild: 0,
                audit_retention_days: 0,
                super_like_daily_limit: 0,
                autoplay_seed_max: 0,
                like_points: 99,
                vote_skip_ratio: 1,
                ..Default::default()
            })
            .unwrap();

        let loaded = store.load_guild_settings(1);
        assert_eq!(loaded.max_queue_per_user, 0, "0이 1로 둔갑했다");
        assert_eq!(loaded.max_queue_per_guild, 0);
        assert_eq!(loaded.audit_retention_days, 0);
        assert_eq!(loaded.super_like_daily_limit, 0);
        assert!(loaded.seed_limit().is_none());
        // 무제한이면 11번째 기준 곡도 들어간다.
        for index in 0..12 {
            assert!(
                store
                    .add_autoplay_seed(1, &test_track(&format!("무제한{index}")), 10)
                    .unwrap()
                    .is_added()
            );
        }
        assert_eq!(store.list_autoplay_seeds(1).len(), 12);

        // 범위 밖 값은 잘렸다.
        assert_eq!(loaded.like_points, 10);
        assert_eq!(loaded.vote_skip_ratio, 10);
        cleanup(store, path);
    }

    /// 보존 정리도 `0 = 무제한`을 지킨다 — 무제한을 고른 서버의 채팅이 하루 만에 사라지면 안 된다.
    #[test]
    fn pruning_never_deletes_when_retention_is_unlimited() {
        let (store, path) = temp_store("prune-zero");
        with_legacy_settings_table(&store);
        store
            .save_guild_settings(&RemoteGuildSettings {
                guild_id: 1,
                chat_retention_days: 0,
                audit_retention_days: 0,
                ..Default::default()
            })
            .unwrap();
        let message = store
            .add_chat_message(1, 10, "민수", None, "아주 오래된 메시지", None)
            .unwrap();
        store
            .add_audit(1, 10, "민수", "queue.add", Some("곡"), None, None, true, None)
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE remote_chat_messages SET created_utc = '2020-01-01T00:00:00+00:00' WHERE id = ?1",
                params![message],
            )
            .unwrap();
            conn.execute(
                "UPDATE remote_audit_logs SET created_utc = '2020-01-01T00:00:00+00:00'",
                [],
            )
            .unwrap();
        }
        let report = store.prune_all(RetentionConfig::default()).unwrap();
        assert_eq!(report.audit, 0, "무제한인데 활동 로그가 지워졌다");
        assert_eq!(store.list_audit(1, 50, None).len(), 1);
        assert_eq!(report.chat, 0, "무제한인데 채팅이 지워졌다");
        assert_eq!(store.list_chat_messages(1, 10, 50, None).len(), 1);
        cleanup(store, path);
    }

    #[test]
    fn lyrics_negative_cache_is_distinguishable_from_a_miss() {
        let (store, path) = temp_store("lyrics");
        assert!(store.lookup_lyrics("youtube:unknown").is_none());
        store.save_lyrics_missing("youtube:unknown").unwrap();
        assert!(matches!(
            store.lookup_lyrics("youtube:unknown"),
            Some(LyricsCacheHit::Missing)
        ));
        assert!(store.load_lyrics("youtube:unknown").is_none());

        store
            .save_lyrics(&LyricsDocument {
                cache_key: "youtube:unknown".into(),
                plain_text: Some("가사".into()),
                synced_lines: Vec::new(),
                source: "lrclib".into(),
                fetched_utc: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();
        assert!(matches!(
            store.lookup_lyrics("youtube:unknown"),
            Some(LyricsCacheHit::Found(_))
        ));
        cleanup(store, path);
    }

    // ───────── 봇 주인 전역 강제값 ─────────

    /// **강제값이 하나도 없으면 도입 전과 완전히 같아야 한다.**
    /// 이게 깨지면 모든 서버의 동작이 조용히 바뀐다.
    #[test]
    fn without_overrides_nothing_changes() {
        let (store, path) = temp_store("ovr-none");
        with_legacy_settings_table(&store);
        let saved = RemoteGuildSettings {
            guild_id: 1,
            max_queue_per_user: 7,
            chat_enabled: false,
            max_volume: 150,
            ..Default::default()
        };
        store.save_guild_settings(&saved).unwrap();

        assert!(store.load_global_overrides().is_empty(), "기본은 강제 안 함");
        let effective = store.load_guild_settings(1);
        let raw = store.load_guild_settings_raw(1);
        assert_eq!(effective.max_queue_per_user, 7);
        assert!(!effective.chat_enabled);
        assert_eq!(effective.max_volume, 150);
        // 유효값과 날것이 한 글자도 다르지 않아야 한다.
        assert_eq!(
            serde_json::to_string(&effective).unwrap(),
            serde_json::to_string(&raw).unwrap(),
        );
        cleanup(store, path);
    }

    /// 강제하면 **실제로 쓰이는 값**이 바뀐다. `0 = 무제한`도 그대로 살아 있어야 한다 (§23.1) —
    /// 여기서 `0`이 `1`이 되면 봇 주인이 "무제한"을 걸었는데 모든 서버가 1곡 제한이 된다.
    #[test]
    fn forcing_a_value_wins_over_the_guild_and_keeps_zero_unlimited() {
        let (store, path) = temp_store("ovr-apply");
        with_legacy_settings_table(&store);
        store
            .save_guild_settings(&RemoteGuildSettings {
                guild_id: 1,
                max_queue_per_user: 50,
                autoplay_seed_max: 10,
                chat_enabled: true,
                ..Default::default()
            })
            .unwrap();

        store
            .save_global_overrides(&GlobalOverrides {
                max_queue_per_user: Some(3),
                // "강제 안 함"과 "강제로 false"는 다른 상태다.
                chat_enabled: Some(false),
                autoplay_seed_max: Some(0),
                ..Default::default()
            })
            .unwrap();

        let effective = store.load_guild_settings(1);
        assert_eq!(effective.max_queue_per_user, 3, "강제값이 안 먹었다");
        assert!(!effective.chat_enabled, "강제로 끈 기능이 켜져 있다");
        assert_eq!(effective.autoplay_seed_max, 0);
        assert!(effective.seed_limit().is_none(), "0이 무제한이 아니게 됐다");
        // 안 건드린 항목은 서버 값 그대로다.
        assert_eq!(effective.max_queue_per_guild, 100);

        let overrides = store.load_global_overrides();
        assert_eq!(
            overrides.locked_keys(),
            vec!["maxQueuePerUser", "autoplaySeedMax", "chatEnabled"],
            "잠긴 키 목록이 선언 순서를 안 따른다",
        );
        assert_eq!(
            overrides.locked_value("chatEnabled"),
            Some(serde_json::json!(false)),
        );
        assert!(overrides.locked_value("maxVolume").is_none());
        cleanup(store, path);
    }

    /// **길드 저장이 강제값을 지우면 안 된다.** 별도 키에 사는 이유가 이것이고,
    /// 동시에 강제값이 길드 JSON 에 구워지지도 않아야 한다 — 구워지면 풀어도 안 돌아온다.
    #[test]
    fn saving_guild_settings_neither_erases_nor_bakes_in_the_override() {
        let (store, path) = temp_store("ovr-keep");
        with_legacy_settings_table(&store);
        store
            .save_guild_settings(&RemoteGuildSettings {
                guild_id: 1,
                max_queue_per_user: 50,
                ..Default::default()
            })
            .unwrap();
        store
            .save_global_overrides(&GlobalOverrides {
                max_queue_per_user: Some(3),
                ..Default::default()
            })
            .unwrap();

        // 관리 콘솔이 하는 그대로: 유효값을 읽어 다른 항목만 고쳐서 되저장한다.
        let mut edited = store.load_guild_settings(1);
        assert_eq!(edited.max_queue_per_user, 3);
        edited.max_queue_per_guild = 42;
        store.save_guild_settings(&edited).unwrap();

        assert_eq!(
            store.load_global_overrides().max_queue_per_user,
            Some(3),
            "길드 저장이 강제값을 지웠다",
        );
        assert_eq!(store.load_guild_settings(1).max_queue_per_guild, 42);
        assert_eq!(
            store.load_guild_settings_raw(1).max_queue_per_user,
            50,
            "강제값이 길드 JSON 에 구워졌다",
        );

        // 강제를 풀면 서버가 원래 쓰던 값이 되살아난다.
        store
            .save_global_overrides(&GlobalOverrides::default())
            .unwrap();
        assert_eq!(store.load_guild_settings(1).max_queue_per_user, 50);
        cleanup(store, path);
    }

    /// 강제값도 길드 설정과 같은 범위로 조인다. 봇 주인이라고 해서 서버가 못 받는 값을
    /// 모든 서버에 뿌릴 수 있으면 안 된다 (§23.1).
    #[test]
    fn override_values_are_clamped_like_guild_settings() {
        let (store, path) = temp_store("ovr-clamp");
        with_legacy_settings_table(&store);
        store
            .save_global_overrides(&GlobalOverrides {
                max_queue_per_user: Some(99_999),
                max_volume: Some(500),
                chart_limit: Some(1),
                // 0 은 무제한이라 잘리면 안 된다.
                super_like_daily_limit: Some(0),
                ..Default::default()
            })
            .unwrap();
        let overrides = store.load_global_overrides();
        assert_eq!(overrides.max_queue_per_user, Some(1_000));
        assert_eq!(overrides.max_volume, Some(200));
        assert_eq!(overrides.chart_limit, Some(10));
        assert_eq!(overrides.super_like_daily_limit, Some(0), "0이 잘렸다");
        cleanup(store, path);
    }

    /// 볼륨 상한을 강제로 내리면 최소·기본도 따라 내려가야 한다.
    /// `sanitize()` 는 `max < min` 이면 **max 를 올려서** 맞추므로, 그냥 덮으면 강제가 풀린다.
    #[test]
    fn forcing_the_volume_ceiling_pulls_the_floor_down_with_it() {
        let (store, path) = temp_store("ovr-volume");
        with_legacy_settings_table(&store);
        store
            .save_guild_settings(&RemoteGuildSettings {
                guild_id: 1,
                min_volume: 120,
                max_volume: 200,
                default_volume: 180,
                ..Default::default()
            })
            .unwrap();
        store
            .save_global_overrides(&GlobalOverrides {
                max_volume: Some(80),
                ..Default::default()
            })
            .unwrap();

        let effective = store.load_guild_settings(1);
        assert_eq!(effective.max_volume, 80, "강제 상한이 도로 올라갔다");
        assert!(effective.min_volume <= 80);
        assert!(effective.default_volume <= 80);
        cleanup(store, path);
    }
}
