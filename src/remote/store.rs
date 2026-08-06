use super::{
    AuditEntry, ChatMessage, ChatReactionSummary, ChatReport, LyricsDocument, QueueScore,
    QueueVoteKind, RecentTrack, RemoteGuildSettings, UserTrack, UserTrackKind,
};
use crate::models::{QueueItem, TrackRef};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// 마참뮤직 전용 테이블 저장소. 기존 음악봇 테이블과 같은 SQLite 파일을 WAL로 공유한다.
pub struct RemoteStore {
    conn: Mutex<Connection>,
}

impl RemoteStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;

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
            "#,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339()
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

    /// 현재 상태의 모든 사용자 요청에 점수 행이 존재하도록 보장한다.
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
            let changed = tx.execute(
                r#"INSERT OR IGNORE INTO remote_queue_scores
                   (item_id, guild_id, requester_user_id, wait_score, manual_priority, original_order, updated_utc)
                   VALUES(?1, ?2, ?3, 0, NULL, ?4, ?5)"#,
                params![
                    item.id,
                    guild_id as i64,
                    item.requested_by_user_id.map(|id| id as i64),
                    next_order,
                    now,
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
                      SUM(CASE WHEN v.kind = 'Like' THEN 1 ELSE 0 END) AS likes,
                      SUM(CASE WHEN v.kind = 'SuperLike' THEN 1 ELSE 0 END) AS super_likes
               FROM remote_queue_scores s
               LEFT JOIN remote_queue_votes v ON v.item_id = s.item_id
               WHERE s.guild_id = ?1
               GROUP BY s.item_id, s.requester_user_id, s.wait_score, s.manual_priority, s.original_order"#,
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
                    like_count: row.get::<_, i64>(5)? as i32,
                    super_like_count: row.get::<_, i64>(6)? as i32,
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

    pub fn add_chat_message(
        &self,
        guild_id: u64,
        user_id: u64,
        display_name: &str,
        avatar_url: Option<&str>,
        content: &str,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO remote_chat_messages(guild_id, user_id, display_name, avatar_url, content, created_utc) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![guild_id as i64, user_id as i64, display_name, avatar_url, content, Self::now_iso()],
        )?;
        Ok(conn.last_insert_rowid())
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

    pub fn list_chat_messages(
        &self,
        guild_id: u64,
        user_id: u64,
        limit: usize,
    ) -> Vec<ChatMessage> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            "SELECT id, user_id, display_name, avatar_url, content, created_utc, deleted_utc FROM remote_chat_messages WHERE guild_id = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let raw: Vec<_> = match statement.query_map(params![guild_id as i64, limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        }) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => return Vec::new(),
        };
        drop(statement);
        let mut messages = Vec::new();
        for (id, author_id, display, avatar, content, created, deleted) in raw.into_iter().rev() {
            let mut reaction_statement = match conn.prepare(
                "SELECT emoji, COUNT(*), MAX(CASE WHEN user_id = ?2 THEN 1 ELSE 0 END) FROM remote_chat_reactions WHERE message_id = ?1 GROUP BY emoji ORDER BY emoji",
            ) {
                Ok(statement) => statement,
                Err(_) => continue,
            };
            let reactions = reaction_statement
                .query_map(params![id, user_id as i64], |row| {
                    Ok(ChatReactionSummary {
                        emoji: row.get(0)?,
                        count: row.get::<_, i64>(1)? as i32,
                        reacted_by_me: row.get::<_, i64>(2)? != 0,
                    })
                })
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default();
            messages.push(ChatMessage {
                id,
                guild_id,
                user_id: author_id as u64,
                display_name: display,
                avatar_url: avatar,
                content,
                created_utc: created,
                deleted_utc: deleted,
                reactions,
            });
        }
        messages
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

    pub fn list_audit(&self, guild_id: u64, limit: usize) -> Vec<AuditEntry> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            "SELECT id, user_id, display_name, action, target, before_value, after_value, success, failure_reason, created_utc FROM remote_audit_logs WHERE guild_id = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![guild_id as i64, limit as i64], |row| {
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

    pub fn prune_audit(&self, guild_id: u64, retention_days: i32) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM remote_audit_logs WHERE guild_id = ?1 AND created_utc < datetime('now', ?2)",
            params![guild_id as i64, format!("-{} days", retention_days.clamp(1, 3650))],
        )
    }

    pub fn load_lyrics(&self, cache_key: &str) -> Option<LyricsDocument> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT payload_json FROM remote_lyrics WHERE cache_key = ?1",
            params![cache_key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
    }

    pub fn save_lyrics(&self, lyrics: &LyricsDocument) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO remote_lyrics(cache_key, payload_json, fetched_utc) VALUES(?1, ?2, ?3) ON CONFLICT(cache_key) DO UPDATE SET payload_json = excluded.payload_json, fetched_utc = excluded.fetched_utc",
            params![lyrics.cache_key, serde_json::to_string(lyrics).unwrap_or_else(|_| "{}".into()), lyrics.fetched_utc],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderKind;

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
    fn vote_updates_score_and_personal_likes() {
        let path = std::env::temp_dir().join(format!(
            "macham-remote-{}.sqlite",
            crate::models::uuid_like()
        ));
        let store = RemoteStore::open(&path).unwrap();
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
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn chat_reaction_is_a_toggle() {
        let path =
            std::env::temp_dir().join(format!("macham-chat-{}.sqlite", crate::models::uuid_like()));
        let store = RemoteStore::open(&path).unwrap();
        let message = store
            .add_chat_message(1, 10, "tester", None, "hello")
            .unwrap();
        assert!(store.toggle_chat_reaction(1, message, 10, "👍").unwrap());
        assert_eq!(store.list_chat_messages(1, 10, 10)[0].reactions[0].count, 1);
        assert!(!store.toggle_chat_reaction(1, message, 10, "👍").unwrap());
        assert!(store.list_chat_messages(1, 10, 10)[0].reactions.is_empty());
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn chat_report_can_be_reviewed_and_resolved() {
        let path = std::env::temp_dir().join(format!(
            "macham-report-{}.sqlite",
            crate::models::uuid_like()
        ));
        let store = RemoteStore::open(&path).unwrap();
        let message = store
            .add_chat_message(1, 10, "author", None, "review me")
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
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
