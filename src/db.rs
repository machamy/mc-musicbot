//! C# SqliteStore 와 동일 스키마/직렬화로 musicbot.sqlite 를 공유하는 저장소.
//! 단일 커넥션 + Mutex (개인 봇 규모에선 충분; C# 도 호출마다 새 커넥션이었다).

use crate::models::*;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scope TEXT NOT NULL,
                guild_id INTEGER NULL,
                owner_user_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                created_utc TEXT NOT NULL,
                updated_utc TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS playlist_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                playlist_id INTEGER NOT NULL,
                sort_order INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS guild_states (
                guild_id INTEGER PRIMARY KEY,
                voice_channel_id INTEGER NULL,
                is_paused INTEGER NOT NULL,
                shuffle_enabled INTEGER NOT NULL,
                autoplay_enabled INTEGER NOT NULL,
                repeat_mode TEXT NOT NULL,
                effective_volume INTEGER NOT NULL,
                current_item_json TEXT NULL,
                recent_tracks_json TEXT NOT NULL,
                cycle_history_json TEXT NOT NULL,
                updated_utc TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS guild_queue (
                id TEXT PRIMARY KEY,
                guild_id INTEGER NOT NULL,
                sort_order INTEGER NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS cache_entries (
                cache_key TEXT PRIMARY KEY,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS blacklist (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                pattern TEXT NOT NULL,
                created_utc TEXT NOT NULL,
                created_by_user_id INTEGER NOT NULL,
                note TEXT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_blacklist_guild ON blacklist(guild_id);
            CREATE TABLE IF NOT EXISTS guild_metadata (
                guild_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                icon_url TEXT NULL,
                member_count INTEGER NULL,
                last_seen_utc TEXT NOT NULL
            );
            "#,
        )?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    // ───────── settings (key/value JSON 문서) ─────────

    pub fn get_setting_json(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT json FROM settings WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .ok()
    }

    pub fn set_setting_json(&self, key: &str, json: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO settings(key, json) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            params![key, json],
        );
    }

    pub fn load_global_settings(&self) -> GlobalSettings {
        self.get_setting_json("global_settings")
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    pub fn save_global_settings(&self, s: &GlobalSettings) {
        if let Ok(j) = serde_json::to_string(s) {
            self.set_setting_json("global_settings", &j);
        }
    }

    pub fn load_guild_settings(&self, guild_id: u64) -> GuildSettings {
        let key = format!("guild_settings:{guild_id}");
        let mut gs: GuildSettings = self
            .get_setting_json(&key)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        gs.guild_id = guild_id;
        gs
    }

    pub fn save_guild_settings(&self, s: &GuildSettings) {
        let key = format!("guild_settings:{}", s.guild_id);
        if let Ok(j) = serde_json::to_string(s) {
            self.set_setting_json(&key, &j);
        }
    }

    pub fn list_known_guild_ids(&self) -> Vec<u64> {
        let conn = self.conn.lock().unwrap();
        let mut ids = std::collections::BTreeSet::new();
        if let Ok(mut st) = conn.prepare("SELECT guild_id FROM guild_states") {
            let rows = st.query_map([], |r| r.get::<_, i64>(0));
            if let Ok(rows) = rows {
                for r in rows.flatten() {
                    ids.insert(r as u64);
                }
            }
        }
        if let Ok(mut st) = conn.prepare(
            "SELECT guild_id FROM playlists WHERE scope = 'Guild' AND guild_id IS NOT NULL",
        ) {
            if let Ok(rows) = st.query_map([], |r| r.get::<_, i64>(0)) {
                for r in rows.flatten() {
                    ids.insert(r as u64);
                }
            }
        }
        if let Ok(mut st) =
            conn.prepare("SELECT key FROM settings WHERE key LIKE 'guild_settings:%'")
        {
            if let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) {
                for key in rows.flatten() {
                    if let Some(id) = key
                        .strip_prefix("guild_settings:")
                        .and_then(|v| v.parse::<u64>().ok())
                    {
                        ids.insert(id);
                    }
                }
            }
        }
        ids.into_iter().collect()
    }

    // ───────── 길드 상태 ─────────

    pub fn load_guild_state(
        &self,
        guild_id: u64,
        default_volume: i32,
        autoplay_default: bool,
    ) -> GuildPlayerState {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT voice_channel_id, is_paused, shuffle_enabled, autoplay_enabled, repeat_mode, effective_volume, current_item_json, recent_tracks_json, cycle_history_json FROM guild_states WHERE guild_id = ?1",
            params![guild_id as i64],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            },
        );

        let mut state = GuildPlayerState {
            guild_id,
            effective_volume: default_volume,
            autoplay_enabled: autoplay_default,
            ..Default::default()
        };

        if let Ok((vc, paused, shuffle, autoplay, repeat, vol, current, recents, cycle)) = row {
            state.voice_channel_id = vc.map(|v| v as u64);
            state.is_paused = paused != 0;
            state.shuffle_enabled = shuffle != 0;
            state.autoplay_enabled = autoplay != 0;
            state.repeat_mode = match repeat.as_str() {
                "Track" => RepeatMode::Track,
                "Queue" => RepeatMode::Queue,
                _ => RepeatMode::Off,
            };
            state.effective_volume = vol as i32;
            state.current_item = current.and_then(|j| serde_json::from_str(&j).ok());
            state.recent_tracks = serde_json::from_str(&recents).unwrap_or_default();
            state.cycle_history = serde_json::from_str(&cycle).unwrap_or_default();
        }

        // upcoming 큐.
        if let Ok(mut st) = conn.prepare(
            "SELECT payload_json FROM guild_queue WHERE guild_id = ?1 ORDER BY sort_order ASC",
        ) {
            if let Ok(rows) = st.query_map(params![guild_id as i64], |r| r.get::<_, String>(0)) {
                for payload in rows.flatten() {
                    if let Ok(item) = serde_json::from_str::<QueueItem>(&payload) {
                        state.upcoming.push(item);
                    }
                }
            }
        }
        // 로드 시 promote: current 가 비었는데 대기열이 있으면 첫 곡을 현재로 올린다.
        // (current_item_json 이 NULL 이거나 역직렬화 실패한 C#/구버전 상태에서 대기열이
        // 영영 재생 안 되는 것을 막는다 — 모든 Rust 내부 전이가 보장하는 불변식을 로드에도 적용.)
        if state.current_item.is_none() && !state.upcoming.is_empty() {
            state.current_item = Some(state.upcoming.remove(0));
        }
        state
    }

    pub fn save_guild_state(&self, state: &GuildPlayerState) {
        let mut conn = self.conn.lock().unwrap();
        let tx = match conn.transaction() {
            Ok(t) => t,
            Err(_) => return,
        };
        let current_json = state
            .current_item
            .as_ref()
            .and_then(|c| serde_json::to_string(c).ok());
        let recents = serde_json::to_string(&state.recent_tracks).unwrap_or_else(|_| "[]".into());
        let cycle = serde_json::to_string(&state.cycle_history).unwrap_or_else(|_| "[]".into());
        let _ = tx.execute(
            r#"INSERT INTO guild_states(guild_id, voice_channel_id, is_paused, shuffle_enabled, autoplay_enabled, repeat_mode, effective_volume, current_item_json, recent_tracks_json, cycle_history_json, updated_utc)
               VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
               ON CONFLICT(guild_id) DO UPDATE SET
                 voice_channel_id=excluded.voice_channel_id, is_paused=excluded.is_paused,
                 shuffle_enabled=excluded.shuffle_enabled, autoplay_enabled=excluded.autoplay_enabled,
                 repeat_mode=excluded.repeat_mode, effective_volume=excluded.effective_volume,
                 current_item_json=excluded.current_item_json, recent_tracks_json=excluded.recent_tracks_json,
                 cycle_history_json=excluded.cycle_history_json, updated_utc=excluded.updated_utc"#,
            params![
                state.guild_id as i64,
                state.voice_channel_id.map(|v| v as i64),
                state.is_paused as i64,
                state.shuffle_enabled as i64,
                state.autoplay_enabled as i64,
                state.repeat_mode.as_str(),
                state.effective_volume as i64,
                current_json,
                recents,
                cycle,
                Self::now_iso(),
            ],
        );
        let _ = tx.execute(
            "DELETE FROM guild_queue WHERE guild_id = ?1",
            params![state.guild_id as i64],
        );
        for (i, item) in state.upcoming.iter().enumerate() {
            if let Ok(payload) = serde_json::to_string(item) {
                let _ = tx.execute(
                    "INSERT INTO guild_queue(id, guild_id, sort_order, payload_json) VALUES(?1,?2,?3,?4)",
                    params![item.id, state.guild_id as i64, i as i64, payload],
                );
            }
        }
        let _ = tx.commit();
    }

    // ───────── 캐시 ─────────

    pub fn get_cache_entry(&self, cache_key: &str) -> Option<CacheEntry> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT payload_json FROM cache_entries WHERE cache_key = ?1",
            params![cache_key],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|j| serde_json::from_str(&j).ok())
    }

    pub fn all_cache_entries(&self) -> Vec<CacheEntry> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        if let Ok(mut st) = conn.prepare("SELECT payload_json FROM cache_entries") {
            if let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) {
                for j in rows.flatten() {
                    if let Ok(e) = serde_json::from_str::<CacheEntry>(&j) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }

    pub fn upsert_cache_entry(&self, entry: &CacheEntry) {
        if let Ok(j) = serde_json::to_string(entry) {
            let conn = self.conn.lock().unwrap();
            let _ = conn.execute(
                "INSERT INTO cache_entries(cache_key, payload_json) VALUES(?1,?2) ON CONFLICT(cache_key) DO UPDATE SET payload_json = excluded.payload_json",
                params![entry.cache_key, j],
            );
        }
    }

    pub fn delete_cache_entries(&self, keys: &[String]) {
        let conn = self.conn.lock().unwrap();
        for k in keys {
            let _ = conn.execute("DELETE FROM cache_entries WHERE cache_key = ?1", params![k]);
        }
    }

    // ───────── 차단목록 ─────────

    pub fn list_all_blacklist(&self) -> Vec<BlacklistEntry> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT id, guild_id, kind, pattern, created_utc, created_by_user_id, note FROM blacklist ORDER BY id DESC",
        ) {
            if let Ok(rows) = st.query_map([], |r| {
                Ok(BlacklistEntry {
                    id: r.get(0)?,
                    guild_id: r.get::<_, i64>(1)? as u64,
                    kind: BlacklistKind::parse(&r.get::<_, String>(2)?).unwrap_or(BlacklistKind::TitleContains),
                    pattern: r.get(3)?,
                    created_utc: r.get(4)?,
                    created_by_user_id: r.get::<_, i64>(5)? as u64,
                    note: r.get(6)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }

    pub fn list_blacklist(&self, guild_id: u64) -> Vec<BlacklistEntry> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT id, guild_id, kind, pattern, created_utc, created_by_user_id, note FROM blacklist WHERE guild_id = ?1 OR guild_id = 0 ORDER BY id DESC",
        ) {
            if let Ok(rows) = st.query_map(params![guild_id as i64], |r| {
                Ok(BlacklistEntry {
                    id: r.get(0)?,
                    guild_id: r.get::<_, i64>(1)? as u64,
                    kind: BlacklistKind::parse(&r.get::<_, String>(2)?).unwrap_or(BlacklistKind::TitleContains),
                    pattern: r.get(3)?,
                    created_utc: r.get(4)?,
                    created_by_user_id: r.get::<_, i64>(5)? as u64,
                    note: r.get(6)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }

    pub fn add_blacklist(
        &self,
        guild_id: u64,
        kind: BlacklistKind,
        pattern: &str,
        created_by: u64,
        note: Option<&str>,
    ) -> i64 {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO blacklist(guild_id, kind, pattern, created_utc, created_by_user_id, note) VALUES(?1,?2,?3,?4,?5,?6)",
            params![guild_id as i64, kind.as_str(), pattern, Self::now_iso(), created_by as i64, note],
        );
        conn.last_insert_rowid()
    }

    pub fn remove_blacklist(&self, id: i64) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM blacklist WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// 서버 관리자가 지우는 경로 (V3 §19).
    ///
    /// **이 길드가 직접 만든 항목만** 지운다. `guild_id = 0` 인 전역 규칙과
    /// 다른 길드 항목은 조건에 안 걸려 `false` 가 나온다. 호출부는 이걸 403 으로 바꾼다.
    /// UI 에서 버튼을 숨기는 것에 의존하지 않기 위해 쿼리 자체로 막는다.
    pub fn remove_guild_blacklist(&self, id: i64, guild_id: u64) -> bool {
        if guild_id == 0 {
            return false;
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM blacklist WHERE id = ?1 AND guild_id = ?2",
            params![id, guild_id as i64],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    // ───────── 플레이리스트 ─────────

    /// 범위별 재생목록 조회.
    ///
    /// `PlaylistScope::User` 는 **개인 재생목록**(V3 §12)이라 길드가 아니라 소유자로 거른다.
    /// 그래서 이 경우 두 번째 인자는 guild_id 가 아니라 **user_id** 로 해석한다.
    /// 헷갈리기 쉬워서 `list_user_playlists` 헬퍼를 같이 둔다 — 그쪽을 쓰는 게 안전하다.
    pub fn list_playlists(&self, scope: PlaylistScope, guild_id: Option<u64>) -> Vec<Playlist> {
        let conn = self.conn.lock().unwrap();
        let mut lists = Vec::new();
        let q = match scope {
            PlaylistScope::Global => "SELECT id, scope, guild_id, owner_user_id, name FROM playlists WHERE scope = 'Global' ORDER BY name COLLATE NOCASE".to_string(),
            PlaylistScope::Guild => format!(
                "SELECT id, scope, guild_id, owner_user_id, name FROM playlists WHERE scope = 'Guild' AND guild_id = {} ORDER BY name COLLATE NOCASE",
                guild_id.unwrap_or(0) as i64
            ),
            PlaylistScope::User => format!(
                "SELECT id, scope, guild_id, owner_user_id, name FROM playlists WHERE scope = 'User' AND owner_user_id = {} ORDER BY name COLLATE NOCASE",
                guild_id.unwrap_or(0) as i64
            ),
        };
        if let Ok(mut st) = conn.prepare(&q) {
            if let Ok(rows) = st.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            }) {
                for (id, scope_s, gid, owner, name) in rows.flatten() {
                    lists.push(Playlist {
                        id,
                        scope: PlaylistScope::parse(&scope_s),
                        guild_id: gid.map(|v| v as u64),
                        owner_user_id: owner as u64,
                        name,
                        entries: Vec::new(),
                    });
                }
            }
        }
        drop(conn);
        for pl in &mut lists {
            pl.entries = self.playlist_entries(pl.id);
        }
        lists
    }

    pub fn playlist_entries(&self, playlist_id: i64) -> Vec<PlaylistEntry> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT payload_json FROM playlist_entries WHERE playlist_id = ?1 ORDER BY sort_order ASC",
        ) {
            if let Ok(rows) = st.query_map(params![playlist_id], |r| r.get::<_, String>(0)) {
                for j in rows.flatten() {
                    if let Ok(e) = serde_json::from_str::<PlaylistEntry>(&j) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }

    pub fn find_playlist(&self, id: i64) -> Option<Playlist> {
        let conn = self.conn.lock().unwrap();
        let head = conn
            .query_row(
                "SELECT id, scope, guild_id, owner_user_id, name FROM playlists WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .ok()?;
        drop(conn);
        Some(Playlist {
            id: head.0,
            scope: PlaylistScope::parse(&head.1),
            guild_id: head.2.map(|v| v as u64),
            owner_user_id: head.3 as u64,
            name: head.4,
            entries: self.playlist_entries(head.0),
        })
    }

    pub fn create_playlist(
        &self,
        scope: PlaylistScope,
        guild_id: Option<u64>,
        owner: u64,
        name: &str,
    ) -> i64 {
        let conn = self.conn.lock().unwrap();
        let now = Self::now_iso();
        let _ = conn.execute(
            "INSERT INTO playlists(scope, guild_id, owner_user_id, name, created_utc, updated_utc) VALUES(?1,?2,?3,?4,?5,?6)",
            params![scope.as_str(), guild_id.map(|v| v as i64), owner as i64, name, now, now],
        );
        conn.last_insert_rowid()
    }

    /// 내 개인 재생목록 (V3 §12). 길드와 무관하게 소유자로만 거른다.
    pub fn list_user_playlists(&self, user_id: u64) -> Vec<Playlist> {
        self.list_playlists(PlaylistScope::User, Some(user_id))
    }

    /// 개인 재생목록을 만든다. `guild_id` 는 넣지 않는다 — 어느 서버에서든 보여야 하니까.
    pub fn create_user_playlist(&self, owner: u64, name: &str) -> i64 {
        self.create_playlist(PlaylistScope::User, None, owner, name)
    }

    /// 이 재생목록을 이 사람이 고쳐도 되는지. 개인 것은 주인만, 나머지는 호출부가 관리자 권한으로 판단한다.
    pub fn is_own_user_playlist(&self, id: i64, user_id: u64) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM playlists WHERE id = ?1 AND scope = 'User' AND owner_user_id = ?2",
            params![id, user_id as i64],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn delete_playlist(&self, id: i64) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM playlists WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE playlists SET name = ?2, updated_utc = ?3 WHERE id = ?1",
            params![id, name, Self::now_iso()],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    pub fn add_playlist_entry(&self, playlist_id: i64, entry: &PlaylistEntry) {
        if let Ok(j) = serde_json::to_string(entry) {
            let conn = self.conn.lock().unwrap();
            let next: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM playlist_entries WHERE playlist_id = ?1",
                    params![playlist_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let _ = conn.execute(
                "INSERT INTO playlist_entries(playlist_id, sort_order, payload_json) VALUES(?1,?2,?3)",
                params![playlist_id, next, j],
            );
            let _ = conn.execute(
                "UPDATE playlists SET updated_utc = ?2 WHERE id = ?1",
                params![playlist_id, Self::now_iso()],
            );
        }
    }

    pub fn remove_playlist_entry(&self, playlist_id: i64, index: usize) -> bool {
        let conn = self.conn.lock().unwrap();
        let ids: Vec<i64> = conn
            .prepare(
                "SELECT id FROM playlist_entries WHERE playlist_id = ?1 ORDER BY sort_order ASC",
            )
            .and_then(|mut st| {
                st.query_map(params![playlist_id], |r| r.get::<_, i64>(0))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default();
        if index >= ids.len() {
            return false;
        }
        conn.execute(
            "DELETE FROM playlist_entries WHERE id = ?1",
            params![ids[index]],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    // ───────── 길드 메타 ─────────

    pub fn upsert_guild_metadata(&self, m: &GuildMetadata) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            r#"INSERT INTO guild_metadata(guild_id, name, icon_url, member_count, last_seen_utc)
               VALUES(?1,?2,?3,?4,?5)
               ON CONFLICT(guild_id) DO UPDATE SET name=excluded.name, icon_url=excluded.icon_url,
                 member_count=excluded.member_count, last_seen_utc=excluded.last_seen_utc"#,
            params![
                m.guild_id as i64,
                m.name,
                m.icon_url,
                m.member_count,
                m.last_seen_utc
            ],
        );
    }

    pub fn delete_guild_metadata(&self, guild_id: u64) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "DELETE FROM guild_metadata WHERE guild_id = ?1",
            params![guild_id as i64],
        );
    }

    pub fn list_guild_metadata(&self) -> Vec<GuildMetadata> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT guild_id, name, icon_url, member_count, last_seen_utc FROM guild_metadata",
        ) {
            if let Ok(rows) = st.query_map([], |r| {
                Ok(GuildMetadata {
                    guild_id: r.get::<_, i64>(0)? as u64,
                    name: r.get(1)?,
                    icon_url: r.get(2)?,
                    member_count: r.get(3)?,
                    last_seen_utc: r.get(4)?,
                })
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }
}
