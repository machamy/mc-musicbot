use super::{
    AuditEntry, ChatMessage, ChatReactionSummary, ChatReplyPreview, ChatReport, ChatTrackTag,
    LyricsCacheHit, LyricsDocument, Participant, PruneReport, QueueScore, QueueVoteKind,
    RecentTrack, RemoteGuildSettings, RetentionConfig, StoredSession, Suggestion, SuggestionStatus,
    Suspension, SuspensionScope, UserTrack, UserTrackKind,
};
use crate::models::{QueueItem, TrackRef};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// 마참뮤직 전용 스키마 버전. `PRAGMA user_version`에 기록된다.
/// 레거시(C# 공용) 테이블은 이 러너가 절대 건드리지 않는다.
const SCHEMA_VERSION: i64 = 8;

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

/// 마참뮤직 전용 테이블 저장소. 기존 음악봇 테이블과 같은 SQLite 파일을 WAL로 공유한다.
pub struct RemoteStore {
    conn: Mutex<Connection>,
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
        })
    }

    fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// 세션 토큰의 SHA-256 16진 해시. 저장소에는 이 값만 남는다.
    pub fn session_token_hash(token: &str) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(token.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn load_guild_settings(&self, guild_id: u64) -> RemoteGuildSettings {
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
        settings
    }

    pub fn save_guild_settings(&self, settings: &RemoteGuildSettings) -> rusqlite::Result<()> {
        let key = format!("remote_guild_settings:{}", settings.guild_id);
        let json = serde_json::to_string(settings).unwrap_or_else(|_| "{}".into());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings(key, json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            params![key, json],
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

    pub fn queue_scores(&self, guild_id: u64) -> HashMap<String, QueueScore> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT s.item_id, s.requester_user_id, s.wait_score, s.manual_priority, s.original_order,
                      s.round, s.last_played_utc,
                      SUM(CASE WHEN v.kind = 'Like' THEN 1 ELSE 0 END) AS likes,
                      SUM(CASE WHEN v.kind = 'SuperLike' THEN 1 ELSE 0 END) AS super_likes
               FROM remote_queue_scores s
               LEFT JOIN remote_queue_votes v ON v.item_id = s.item_id
               WHERE s.guild_id = ?1
               GROUP BY s.item_id, s.requester_user_id, s.wait_score, s.manual_priority,
                        s.original_order, s.round, s.last_played_utc"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return HashMap::new(),
        };
        let rows = match statement.query_map(params![guild_id as i64], |row| {
            let item_id: String = row.get(0)?;
            Ok((
                item_id.clone(),
                QueueScore {
                    item_id,
                    guild_id,
                    requester_user_id: row.get::<_, Option<i64>>(1)?.map(|id| id as u64),
                    wait_score: row.get(2)?,
                    manual_priority: row.get(3)?,
                    original_order: row.get(4)?,
                    round: row.get(5)?,
                    last_played_utc: row.get(6)?,
                    like_count: row.get::<_, i64>(7)? as i32,
                    super_like_count: row.get::<_, i64>(8)? as i32,
                },
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return HashMap::new(),
        };
        rows.flatten().collect()
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
        if let Some(kind) = kind {
            tx.execute(
                "INSERT INTO remote_queue_votes(item_id, guild_id, user_id, kind, created_utc) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![item_id, guild_id as i64, user_id as i64, kind.as_str(), now],
            )?;
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
                access_token, refresh_token, expires_utc, refreshed_utc, created_utc)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
               ON CONFLICT(token_hash) DO UPDATE SET
                 user_id = excluded.user_id, display_name = excluded.display_name,
                 avatar_url = excluded.avatar_url, guilds_json = excluded.guilds_json,
                 access_token = excluded.access_token, refresh_token = excluded.refresh_token,
                 expires_utc = excluded.expires_utc, refreshed_utc = excluded.refreshed_utc"#,
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
            ],
        )?;
        Ok(())
    }

    /// 만료된 세션은 없는 것으로 친다.
    pub fn load_session(&self, token: &str) -> Option<StoredSession> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"SELECT user_id, display_name, avatar_url, guilds_json,
                      access_token, refresh_token, expires_utc, refreshed_utc, created_utc
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

    // ───────── 활동 로그 ─────────

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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO remote_audit_logs
               (guild_id, user_id, display_name, action, target, before_value, after_value, success, failure_reason, created_utc)
               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
            params![guild_id as i64, user_id as i64, display_name, action, target, before_value,
                after_value, success as i64, failure_reason, Self::now_iso()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// `before_id`가 있으면 그보다 과거만. 활동 로그 탭의 커서 페이지네이션이다.
    pub fn list_audit(&self, guild_id: u64, limit: usize, before_id: Option<i64>) -> Vec<AuditEntry> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            r#"SELECT id, user_id, display_name, action, target, before_value, after_value,
                      success, failure_reason, created_utc
               FROM remote_audit_logs
               WHERE guild_id = ?1 AND (?2 IS NULL OR id < ?2)
               ORDER BY id DESC LIMIT ?3"#,
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64, before_id, limit as i64], |row| {
                Ok(AuditEntry {
                    id: row.get(0)?,
                    guild_id,
                    user_id: row.get::<_, i64>(1)? as u64,
                    display_name: row.get(2)?,
                    action: row.get(3)?,
                    target: row.get(4)?,
                    before_value: row.get(5)?,
                    after_value: row.get(6)?,
                    success: row.get::<_, i64>(7)? != 0,
                    failure_reason: row.get(8)?,
                    created_utc: row.get(9)?,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// created_utc 는 RFC3339(`...T...+00:00`)라 문자열로 `datetime('now')`(`... ...`)와 비교하면
    /// 'T' > ' ' 때문에 아무것도 안 지워진다. julianday 로 실제 시각을 비교한다.
    pub fn prune_audit(&self, guild_id: u64, retention_days: i32) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"DELETE FROM remote_audit_logs
               WHERE guild_id = ?1 AND julianday(created_utc) < julianday('now', ?2)"#,
            params![
                guild_id as i64,
                format!("-{} days", retention_days.clamp(1, 3650))
            ],
        )
    }

    // ───────── 보존 정리 ─────────

    /// 기동 시 + 하루 1회 부른다. 길드 설정이 있으면 길드 설정이 이긴다.
    pub fn prune_all(&self, retention: RetentionConfig) -> rusqlite::Result<PruneReport> {
        // load_guild_settings 가 같은 뮤텍스를 잡으므로 설정은 먼저 다 읽어 둔다.
        let guild_ids = self.remote_guild_ids();
        let plans: Vec<(u64, u32, i32)> = guild_ids
            .into_iter()
            .map(|guild_id| {
                let settings = self.load_guild_settings(guild_id);
                let chat_days = if settings.chat_retention_days == 0 {
                    retention.chat_days
                } else {
                    settings.chat_retention_days
                };
                (guild_id, chat_days, settings.audit_retention_days)
            })
            .collect();

        let mut report = PruneReport::default();
        let conn = self.conn.lock().unwrap();
        for (guild_id, chat_days, audit_days) in plans {
            report.chat += conn.execute(
                r#"DELETE FROM remote_chat_messages
                   WHERE guild_id = ?1 AND julianday(created_utc) < julianday('now', ?2)"#,
                params![
                    guild_id as i64,
                    format!("-{} days", chat_days.clamp(1, 3650))
                ],
            )?;
            report.audit += conn.execute(
                r#"DELETE FROM remote_audit_logs
                   WHERE guild_id = ?1 AND julianday(created_utc) < julianday('now', ?2)"#,
                params![
                    guild_id as i64,
                    format!("-{} days", audit_days.clamp(1, 3650))
                ],
            )?;
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
        Ok(report)
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
            // 여기 오면 SCHEMA_VERSION 만 올리고 단계를 안 쓴 것이다.
            _ => {}
        }
        version += 1;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
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

    fn temp_store(tag: &str) -> (RemoteStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "macham-{tag}-{}.sqlite",
            crate::models::uuid_like()
        ));
        let store = RemoteStore::open(&path).unwrap();
        (store, path)
    }

    fn cleanup(store: RemoteStore, path: std::path::PathBuf) {
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
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
        assert_eq!(store.queue_scores(1)["item"].total_score(), 2);
        assert_eq!(store.list_user_tracks(1, 20, UserTrackKind::Liked).len(), 1);
        store.set_vote(1, &item.id, 20, None, &item.track).unwrap();
        assert_eq!(store.queue_scores(1)["item"].total_score(), 0);
        assert!(
            store
                .list_user_tracks(1, 20, UserTrackKind::Liked)
                .is_empty()
        );
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
        };
        store.save_session(token, &session).unwrap();
        let loaded = store.load_session(token).expect("세션 유실");
        assert_eq!(loaded.user_id, 10);
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
        assert!(store.load_session("wrong-token").is_none());

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
}
