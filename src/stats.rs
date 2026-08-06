//! 개인 통계와 우리 차트 (V3 §22, §15.2b).
//!
//! **본 DB(`musicbot.sqlite`)와 파일을 분리한다.** 이유는 두 가지다.
//!   1. 본 DB는 C# 엔진과 공유하고, 재생 경로가 같은 뮤텍스를 물고 있다.
//!      통계는 곡 하나 담을 때마다·좋아요 하나 누를 때마다 쌓이는 제일 빨리 부푸는 데이터라
//!      거기 넣으면 통계 쓰기가 재생 쿼리와 락을 다툰다.
//!   2. 파일이 커지면 백업·이동이 무거워진다.
//!
//! **쓰기는 재생 경로를 절대 막지 않는다.** 호출부는 `Stats::record()` 로 이벤트를 던지고 즉시 돌아간다.
//! 전용 태스크가 배치로 반영한다. 채널이 꽉 차면 **그냥 버린다** — 통계 한 줄 때문에 음악이 밀리면 본말전도다.
//!
//! **통계 DB가 깨져도 봇은 정상 동작해야 한다.** 열기에 실패하면 로그만 남기고
//! 통계 기능만 꺼진 채 계속 간다.

use crate::logging::LogService;
use crate::models::{PlaybackRequestKind, QueueItem, TrackRef};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;

/// 채널 용량. 넘치면 오래된 것부터 버린다.
const EVENT_CHANNEL_CAP: usize = 4096;
/// 이만큼 모이거나 아래 시간이 지나면 한 트랜잭션으로 반영한다.
const FLUSH_BATCH: usize = 200;
const FLUSH_INTERVAL_MS: u64 = 1000;
/// 일별 테이블 보존 기간.
const DAILY_KEEP_DAYS: i64 = 90;
/// 봇 전체 합계를 담는 가짜 guild_id. 실제 길드 ID는 0이 될 수 없다.
const ALL_GUILDS: u64 = 0;

const SCHEMA_VERSION: i64 = 1;

/// 어떤 투표인지. `remote::QueueVoteKind` 와 별개로 두어 통계 모듈이 remote 에 의존하지 않게 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteFlavor {
    Like,
    Super,
    Dislike,
}

/// 재생이 어떻게 끝났는지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayOutcome {
    Completed,
    Skipped,
}

/// 통계 이벤트. 호출부는 이걸 던지기만 하고 기다리지 않는다.
#[derive(Debug, Clone)]
pub enum StatEvent {
    /// 곡을 담았다. `bulk` 면 한 번에 담기로 들어온 것.
    Queued {
        guild_id: u64,
        user_id: u64,
        cache_key: String,
        track_json: String,
        bulk: bool,
    },
    /// 한 번에 담기를 한 번 썼다 (곡 수와 별개로 "횟수"를 센다).
    BulkUsed { guild_id: u64, user_id: u64 },
    /// 곡이 재생됐다.
    ///
    /// **자동재생은 차트에서 제외한다** — 사람이 고른 게 아니라 알고리즘이 채운 거라
    /// 같이 세면 차트가 "자동재생이 많이 튼 곡"이 되어 버린다(§15.2b).
    ///
    /// 판정은 **`autoplay` 하나로만** 한다. `requester.is_none()` 으로 대신하면
    /// `/이전곡` 처럼 사람이 시켰는데 신청자 ID 가 없는 항목까지 자동재생으로 세어져
    /// 차트가 조용히 오염된다. 만드는 건 [`StatEvent::played_from_item`] 에 맡긴다.
    Played {
        guild_id: u64,
        /// 신청한 사람. 모르면 `None` 이고 그때는 사람 통계만 건너뛴다.
        requester: Option<u64>,
        /// 자동재생이 채운 곡인가. 차트의 `plays_user`/`plays_autoplay` 를 가른다.
        autoplay: bool,
        cache_key: String,
        track_json: String,
        outcome: PlayOutcome,
    },
    /// 투표가 켜지거나(`added`) 꺼졌다.
    Vote {
        guild_id: u64,
        voter_id: u64,
        owner_id: Option<u64>,
        cache_key: String,
        track_json: String,
        flavor: VoteFlavor,
        added: bool,
    },
    /// 붐따로 대기열에서 내려갔다.
    Boomtta {
        guild_id: u64,
        owner_id: Option<u64>,
        cache_key: String,
    },
    /// 채팅 한 줄.
    Chat { guild_id: u64, user_id: u64 },
}

impl StatEvent {
    /// 큐 항목 하나가 끝났을 때의 재생 이벤트. **호출부는 반드시 이걸 쓴다.**
    ///
    /// 자동재생 판정은 `request_kind` 하나로 한다 — 신청자 ID 가 비었다는 이유로
    /// 자동재생 취급하면 안 된다. `/이전곡`으로 되돌아간 곡은 사람이 시킨 곡인데도
    /// 신청자 ID 가 없어서, 그걸 자동재생으로 세면 §15.2b 차트가 틀어진다.
    pub fn played_from_item(guild_id: u64, item: &QueueItem, outcome: PlayOutcome) -> StatEvent {
        let autoplay = item.request_kind == PlaybackRequestKind::Autoplay;
        let (cache_key, track_json) = track_parts(&item.track);
        StatEvent::Played {
            guild_id,
            // 자동재생 항목은 신청자가 있을 수 없다. 혹시 남아 있어도 사람 통계에 넣지 않는다.
            requester: if autoplay {
                None
            } else {
                item.requested_by_user_id
            },
            autoplay,
            cache_key,
            track_json,
            outcome,
        }
    }

    /// 붐따(§10.3)로 대기열에서 내려간 곡. 신청자에게만 기록이 남는다.
    pub fn boomtta_from_item(guild_id: u64, item: &QueueItem) -> StatEvent {
        StatEvent::Boomtta {
            guild_id,
            owner_id: item.requested_by_user_id,
            cache_key: item.track.cache_key(),
        }
    }

    fn guild_id(&self) -> u64 {
        match self {
            StatEvent::Queued { guild_id, .. }
            | StatEvent::BulkUsed { guild_id, .. }
            | StatEvent::Played { guild_id, .. }
            | StatEvent::Vote { guild_id, .. }
            | StatEvent::Boomtta { guild_id, .. }
            | StatEvent::Chat { guild_id, .. } => *guild_id,
        }
    }
}

/// 사람 한 명의 누적 기록.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStats {
    pub queued_single: i64,
    pub queued_bulk: i64,
    pub bulk_times: i64,
    pub played: i64,
    pub skipped: i64,
    pub boomtta: i64,
    pub likes_recv: i64,
    pub supers_recv: i64,
    pub dislikes_recv: i64,
    pub likes_give: i64,
    pub supers_give: i64,
    pub dislikes_give: i64,
    pub chats: i64,
    pub first_utc: Option<String>,
    pub last_utc: Option<String>,
}

impl UserStats {
    /// 마참 점수 (§22.4). **대기열 순서와 권한에 일절 영향을 주지 않는다.**
    /// 섞으면 잘 쌓은 사람이 새치기하게 되어 "사람마다 공평하게"가 무너진다.
    pub fn karma(&self) -> i64 {
        let raw = self.likes_recv + self.supers_recv * 3 + self.played - self.boomtta * 2;
        raw.max(0)
    }

    pub fn queued_total(&self) -> i64 {
        self.queued_single + self.queued_bulk
    }
}

/// 사람×곡 기록. "가장 많이 신청한 곡" 같은 목록에 쓴다.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserTrackStat {
    pub cache_key: String,
    pub track: serde_json::Value,
    pub requested: i64,
    pub liked: i64,
    pub played: i64,
    pub likes_recv: i64,
    pub last_utc: String,
}

/// 차트 한 줄.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartRow {
    pub cache_key: String,
    pub track: serde_json::Value,
    /// 사람이 신청해서 재생된 횟수. 순위 기준.
    pub plays_user: i64,
    /// 자동재생으로 나간 횟수. 순위에는 안 쓰고 툴팁에만 쓴다.
    pub plays_autoplay: i64,
    pub likes: i64,
    pub supers: i64,
    /// 서로 다른 신청자 수. 동점일 때 이게 많은 쪽이 위다 —
    /// 한 사람이 20번 튼 것보다 다섯 명이 네 번씩 튼 게 더 인기곡이다.
    pub requesters: i64,
    /// 사랑받은 곡 차트의 점수 (`likes + supers * weight`).
    pub love_score: i64,
}

/// 차트 기준.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    /// 많이 튼 곡
    Plays,
    /// 많이 사랑받은 곡
    Love,
}

/// 차트 기간.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartWindow {
    Week,
    Month,
    All,
}

impl ChartWindow {
    fn days(self) -> Option<i64> {
        match self {
            ChartWindow::Week => Some(7),
            ChartWindow::Month => Some(30),
            ChartWindow::All => None,
        }
    }

    pub fn parse(value: &str) -> ChartWindow {
        match value {
            "week" => ChartWindow::Week,
            "all" => ChartWindow::All,
            _ => ChartWindow::Month,
        }
    }
}

pub struct Stats {
    conn: Mutex<Connection>,
    tx: mpsc::Sender<StatEvent>,
}

impl Stats {
    /// 통계 DB를 연다. 실패하면 `None` — 호출부는 통계만 끄고 계속 간다.
    pub fn open(path: &Path, log: Arc<LogService>) -> Option<Arc<Stats>> {
        let conn = match Connection::open(path) {
            Ok(conn) => conn,
            Err(error) => {
                log.warn(
                    "Stats",
                    &format!("통계 DB를 열지 못해 통계 기능을 끕니다: {error}"),
                );
                return None;
            }
        };
        if let Err(error) = migrate(&conn) {
            log.warn(
                "Stats",
                &format!("통계 DB 마이그레이션에 실패해 통계 기능을 끕니다: {error}"),
            );
            return None;
        }
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let stats = Arc::new(Stats {
            conn: Mutex::new(conn),
            tx,
        });
        spawn_writer(stats.clone(), rx, log);
        Some(stats)
    }

    /// 이벤트를 던진다. **절대 블로킹하지 않는다.**
    /// 채널이 꽉 차면 조용히 버린다 — 통계 한 줄보다 재생이 중요하다.
    pub fn record(&self, event: StatEvent) {
        let _ = self.tx.try_send(event);
    }

    // ───────────────────────── 조회 ─────────────────────────

    pub fn user_stats(&self, guild_id: u64, user_id: u64) -> UserStats {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT queued_single, queued_bulk, bulk_times, played, skipped, boomtta,
                    likes_recv, supers_recv, dislikes_recv,
                    likes_give, supers_give, dislikes_give, chats, first_utc, last_utc
             FROM stat_user WHERE guild_id = ?1 AND user_id = ?2",
            params![guild_id as i64, user_id as i64],
            |row| {
                Ok(UserStats {
                    queued_single: row.get(0)?,
                    queued_bulk: row.get(1)?,
                    bulk_times: row.get(2)?,
                    played: row.get(3)?,
                    skipped: row.get(4)?,
                    boomtta: row.get(5)?,
                    likes_recv: row.get(6)?,
                    supers_recv: row.get(7)?,
                    dislikes_recv: row.get(8)?,
                    likes_give: row.get(9)?,
                    supers_give: row.get(10)?,
                    dislikes_give: row.get(11)?,
                    chats: row.get(12)?,
                    first_utc: row.get(13)?,
                    last_utc: row.get(14)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    /// 상위 곡 목록. `order` 는 `requested` / `liked` / `likes_recv` 중 하나만 받는다
    /// (문자열을 그대로 SQL 에 넣으므로 화이트리스트 밖은 거부한다).
    pub fn top_user_tracks(
        &self,
        guild_id: u64,
        user_id: u64,
        order: &str,
        limit: usize,
    ) -> Vec<UserTrackStat> {
        let column = match order {
            "requested" => "requested",
            "liked" => "liked",
            "likes_recv" => "likes_recv",
            _ => return Vec::new(),
        };
        let sql = format!(
            "SELECT cache_key, track_json, requested, liked, played, likes_recv, last_utc
             FROM stat_user_track
             WHERE guild_id = ?1 AND user_id = ?2 AND {column} > 0
             ORDER BY {column} DESC, last_utc DESC LIMIT ?3"
        );
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare(&sql) else {
            return Vec::new();
        };
        let rows = stmt.query_map(
            params![guild_id as i64, user_id as i64, limit as i64],
            |row| {
                let track_json: String = row.get(1)?;
                Ok(UserTrackStat {
                    cache_key: row.get(0)?,
                    track: serde_json::from_str(&track_json).unwrap_or(serde_json::Value::Null),
                    requested: row.get(2)?,
                    liked: row.get(3)?,
                    played: row.get(4)?,
                    likes_recv: row.get(5)?,
                    last_utc: row.get(6)?,
                })
            },
        );
        rows.map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// 최근 N일 일별 추이. `(day, queued, played, likes_recv)`.
    pub fn user_daily(
        &self,
        guild_id: u64,
        user_id: u64,
        days: i64,
    ) -> Vec<(String, i64, i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare(
            "SELECT day, queued, played, likes_recv FROM stat_daily
             WHERE guild_id = ?1 AND user_id = ?2 AND day >= date('now', ?3)
             ORDER BY day",
        ) else {
            return Vec::new();
        };
        let offset = format!("-{days} days");
        stmt.query_map(
            params![guild_id as i64, user_id as i64, offset],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// 우리 차트 (§15.2b). `guild_id` 에 [`ALL_GUILDS`] 를 주면 봇 전체 합계다.
    ///
    /// **자동재생 재생은 순위에 쓰지 않는다.** `plays_autoplay` 는 같이 돌려주되 정렬에는 안 쓴다.
    pub fn chart(
        &self,
        guild_id: u64,
        kind: ChartKind,
        window: ChartWindow,
        super_weight: u32,
        limit: usize,
    ) -> Vec<ChartRow> {
        let weight = super_weight as i64;
        let conn = self.conn.lock().unwrap();

        // 기간이 있으면 일별 테이블을 합산하고, 전체면 누적 테이블을 그대로 쓴다.
        let sql = match window.days() {
            Some(_) => format!(
                "SELECT d.cache_key, t.track_json,
                        SUM(d.plays_user), COALESCE(t.plays_autoplay, 0),
                        SUM(d.likes), SUM(d.supers), COALESCE(t.requesters, 0)
                 FROM stat_track_daily d
                 LEFT JOIN stat_track_plays t
                        ON t.guild_id = d.guild_id AND t.cache_key = d.cache_key
                 WHERE d.guild_id = ?1 AND d.day >= date('now', ?2)
                 GROUP BY d.cache_key
                 HAVING {have} > 0
                 ORDER BY {order} DESC, COALESCE(t.requesters, 0) DESC LIMIT ?3",
                have = match kind {
                    ChartKind::Plays => "SUM(d.plays_user)".to_string(),
                    ChartKind::Love => format!("(SUM(d.likes) + SUM(d.supers) * {weight})"),
                },
                order = match kind {
                    ChartKind::Plays => "SUM(d.plays_user)".to_string(),
                    ChartKind::Love => format!("(SUM(d.likes) + SUM(d.supers) * {weight})"),
                },
            ),
            None => format!(
                "SELECT cache_key, track_json, plays_user, plays_autoplay, likes, supers, requesters
                 FROM stat_track_plays
                 WHERE guild_id = ?1 AND {have} > 0
                 ORDER BY {order} DESC, requesters DESC LIMIT ?2",
                have = match kind {
                    ChartKind::Plays => "plays_user".to_string(),
                    ChartKind::Love => format!("(likes + supers * {weight})"),
                },
                order = match kind {
                    ChartKind::Plays => "plays_user".to_string(),
                    ChartKind::Love => format!("(likes + supers * {weight})"),
                },
            ),
        };

        let Ok(mut stmt) = conn.prepare(&sql) else {
            return Vec::new();
        };
        let offset = window
            .days()
            .map(|days| format!("-{days} days"))
            .unwrap_or_default();
        let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<ChartRow> {
            let track_json: Option<String> = row.get(1)?;
            let likes: i64 = row.get(4)?;
            let supers: i64 = row.get(5)?;
            Ok(ChartRow {
                cache_key: row.get(0)?,
                track: track_json
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or(serde_json::Value::Null),
                plays_user: row.get(2)?,
                plays_autoplay: row.get(3)?,
                likes,
                supers,
                requesters: row.get(6)?,
                love_score: likes + supers * weight,
            })
        };
        let rows = if window.days().is_some() {
            stmt.query_map(params![guild_id as i64, offset, limit as i64], map)
        } else {
            stmt.query_map(params![guild_id as i64, limit as i64], map)
        };
        rows.map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// 보존 정리. 일별 테이블만 자른다 — 누적 롤업은 사람×곡이라 안 터진다.
    pub fn prune(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = format!("-{DAILY_KEEP_DAYS} days");
        let a = conn.execute(
            "DELETE FROM stat_daily WHERE day < date('now', ?1)",
            params![cutoff],
        )?;
        let b = conn.execute(
            "DELETE FROM stat_track_daily WHERE day < date('now', ?1)",
            params![cutoff],
        )?;
        // 아무 활동도 안 남은 사람×곡 행은 지운다.
        let c = conn.execute(
            "DELETE FROM stat_user_track WHERE requested = 0 AND liked = 0 AND likes_recv = 0",
            [],
        )?;
        Ok(a + b + c)
    }
}

// ───────────────────────── 마이그레이션 ─────────────────────────

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;",
    )?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    if version < 1 {
        conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE IF NOT EXISTS stat_user (
                guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
                queued_single INTEGER NOT NULL DEFAULT 0,
                queued_bulk   INTEGER NOT NULL DEFAULT 0,
                bulk_times    INTEGER NOT NULL DEFAULT 0,
                played        INTEGER NOT NULL DEFAULT 0,
                skipped       INTEGER NOT NULL DEFAULT 0,
                boomtta       INTEGER NOT NULL DEFAULT 0,
                likes_recv INTEGER NOT NULL DEFAULT 0,
                supers_recv INTEGER NOT NULL DEFAULT 0,
                dislikes_recv INTEGER NOT NULL DEFAULT 0,
                likes_give INTEGER NOT NULL DEFAULT 0,
                supers_give INTEGER NOT NULL DEFAULT 0,
                dislikes_give INTEGER NOT NULL DEFAULT 0,
                chats INTEGER NOT NULL DEFAULT 0,
                first_utc TEXT, last_utc TEXT,
                PRIMARY KEY (guild_id, user_id));

            CREATE TABLE IF NOT EXISTS stat_user_track (
                guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, cache_key TEXT NOT NULL,
                track_json TEXT NOT NULL,
                requested INTEGER NOT NULL DEFAULT 0,
                liked     INTEGER NOT NULL DEFAULT 0,
                played    INTEGER NOT NULL DEFAULT 0,
                likes_recv INTEGER NOT NULL DEFAULT 0,
                last_utc TEXT NOT NULL,
                PRIMARY KEY (guild_id, user_id, cache_key));
            CREATE INDEX IF NOT EXISTS idx_stat_track_req
                ON stat_user_track(guild_id, user_id, requested DESC);
            CREATE INDEX IF NOT EXISTS idx_stat_track_liked
                ON stat_user_track(guild_id, user_id, liked DESC);

            CREATE TABLE IF NOT EXISTS stat_daily (
                guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, day TEXT NOT NULL,
                queued INTEGER NOT NULL DEFAULT 0,
                played INTEGER NOT NULL DEFAULT 0,
                likes_recv INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (guild_id, user_id, day));

            CREATE TABLE IF NOT EXISTS stat_track_plays (
                guild_id INTEGER NOT NULL, cache_key TEXT NOT NULL,
                track_json TEXT NOT NULL,
                plays_user     INTEGER NOT NULL DEFAULT 0,
                plays_autoplay INTEGER NOT NULL DEFAULT 0,
                likes  INTEGER NOT NULL DEFAULT 0,
                supers INTEGER NOT NULL DEFAULT 0,
                requesters INTEGER NOT NULL DEFAULT 0,
                last_utc TEXT NOT NULL,
                PRIMARY KEY (guild_id, cache_key));
            CREATE INDEX IF NOT EXISTS idx_track_plays_rank
                ON stat_track_plays(guild_id, plays_user DESC);

            CREATE TABLE IF NOT EXISTS stat_track_daily (
                guild_id INTEGER NOT NULL, cache_key TEXT NOT NULL, day TEXT NOT NULL,
                plays_user INTEGER NOT NULL DEFAULT 0,
                likes INTEGER NOT NULL DEFAULT 0,
                supers INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (guild_id, cache_key, day));

            -- 서로 다른 신청자 수를 세기 위한 보조 테이블.
            -- COUNT(DISTINCT) 를 매번 돌리는 것보다 여기서 존재 여부만 보는 게 싸다.
            CREATE TABLE IF NOT EXISTS stat_track_requester (
                guild_id INTEGER NOT NULL, cache_key TEXT NOT NULL, user_id INTEGER NOT NULL,
                PRIMARY KEY (guild_id, cache_key, user_id));
            COMMIT;
            "#,
        )?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

// ───────────────────────── 쓰기 태스크 ─────────────────────────

fn spawn_writer(stats: Arc<Stats>, mut rx: mpsc::Receiver<StatEvent>, log: Arc<LogService>) {
    tokio::spawn(async move {
        let mut batch: Vec<StatEvent> = Vec::with_capacity(FLUSH_BATCH);
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                received = rx.recv() => {
                    match received {
                        Some(event) => {
                            batch.push(event);
                            if batch.len() >= FLUSH_BATCH {
                                flush(&stats, &mut batch, &log);
                            }
                        }
                        // 채널이 닫혔다 — 남은 것을 비우고 나간다.
                        None => { flush(&stats, &mut batch, &log); break; }
                    }
                }
                _ = ticker.tick() => {
                    if !batch.is_empty() { flush(&stats, &mut batch, &log); }
                }
            }
        }
    });
}

fn flush(stats: &Stats, batch: &mut Vec<StatEvent>, log: &LogService) {
    if batch.is_empty() {
        return;
    }
    let events = std::mem::take(batch);
    let mut conn = stats.conn.lock().unwrap();
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(error) => {
            log.warn("Stats", &format!("통계 트랜잭션 시작 실패: {error}"));
            return;
        }
    };
    for event in &events {
        if let Err(error) = apply(&tx, event) {
            // 한 건이 실패해도 나머지는 반영한다. 통계는 완벽할 필요가 없다.
            log.warn("Stats", &format!("통계 반영 실패({:?}): {error}", event.guild_id()));
        }
    }
    if let Err(error) = tx.commit() {
        log.warn("Stats", &format!("통계 커밋 실패: {error}"));
    }
}

fn apply(tx: &rusqlite::Transaction<'_>, event: &StatEvent) -> rusqlite::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();

    match event {
        StatEvent::Queued {
            guild_id,
            user_id,
            cache_key,
            track_json,
            bulk,
        } => {
            let (single, bulk_n) = if *bulk { (0, 1) } else { (1, 0) };
            touch_user(tx, *guild_id, *user_id, &now)?;
            tx.execute(
                "UPDATE stat_user SET queued_single = queued_single + ?3,
                                      queued_bulk = queued_bulk + ?4, last_utc = ?5
                 WHERE guild_id = ?1 AND user_id = ?2",
                params![*guild_id as i64, *user_id as i64, single, bulk_n, now],
            )?;
            touch_user_track(tx, *guild_id, *user_id, cache_key, track_json, &now)?;
            tx.execute(
                "UPDATE stat_user_track SET requested = requested + 1, last_utc = ?4
                 WHERE guild_id = ?1 AND user_id = ?2 AND cache_key = ?3",
                params![*guild_id as i64, *user_id as i64, cache_key, now],
            )?;
            bump_daily(tx, *guild_id, *user_id, &day, 1, 0, 0)?;
            // 신청자 집합 — 새로 들어온 사람이면 requesters 를 올린다.
            for scope in [*guild_id, ALL_GUILDS] {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO stat_track_requester(guild_id, cache_key, user_id)
                     VALUES(?1, ?2, ?3)",
                    params![scope as i64, cache_key, *user_id as i64],
                )?;
                if inserted > 0 {
                    touch_track(tx, scope, cache_key, track_json, &now)?;
                    tx.execute(
                        "UPDATE stat_track_plays SET requesters = requesters + 1
                         WHERE guild_id = ?1 AND cache_key = ?2",
                        params![scope as i64, cache_key],
                    )?;
                }
            }
        }

        StatEvent::BulkUsed { guild_id, user_id } => {
            touch_user(tx, *guild_id, *user_id, &now)?;
            tx.execute(
                "UPDATE stat_user SET bulk_times = bulk_times + 1, last_utc = ?3
                 WHERE guild_id = ?1 AND user_id = ?2",
                params![*guild_id as i64, *user_id as i64, now],
            )?;
        }

        StatEvent::Played {
            guild_id,
            requester,
            autoplay,
            cache_key,
            track_json,
            outcome,
        } => {
            // 사람 통계는 신청자가 있을 때만.
            if let Some(user_id) = requester {
                touch_user(tx, *guild_id, *user_id, &now)?;
                let (played, skipped) = match outcome {
                    PlayOutcome::Completed => (1, 0),
                    PlayOutcome::Skipped => (0, 1),
                };
                tx.execute(
                    "UPDATE stat_user SET played = played + ?3, skipped = skipped + ?4, last_utc = ?5
                     WHERE guild_id = ?1 AND user_id = ?2",
                    params![*guild_id as i64, *user_id as i64, played, skipped, now],
                )?;
                touch_user_track(tx, *guild_id, *user_id, cache_key, track_json, &now)?;
                tx.execute(
                    "UPDATE stat_user_track SET played = played + ?4, last_utc = ?5
                     WHERE guild_id = ?1 AND user_id = ?2 AND cache_key = ?3",
                    params![*guild_id as i64, *user_id as i64, cache_key, played, now],
                )?;
                bump_daily(tx, *guild_id, *user_id, &day, 0, played, 0)?;
            }

            // 차트는 자동재생을 따로 센다 (§15.2b). 순위에는 plays_user 만 쓴다.
            // 기준은 `autoplay` 플래그다 — 신청자 유무로 가르면 안 된다(위 주석 참고).
            let column = if *autoplay {
                "plays_autoplay"
            } else {
                "plays_user"
            };
            for scope in [*guild_id, ALL_GUILDS] {
                touch_track(tx, scope, cache_key, track_json, &now)?;
                tx.execute(
                    &format!(
                        "UPDATE stat_track_plays SET {column} = {column} + 1, last_utc = ?3
                         WHERE guild_id = ?1 AND cache_key = ?2"
                    ),
                    params![scope as i64, cache_key, now],
                )?;
                if !*autoplay {
                    tx.execute(
                        "INSERT INTO stat_track_daily(guild_id, cache_key, day, plays_user)
                         VALUES(?1, ?2, ?3, 1)
                         ON CONFLICT(guild_id, cache_key, day)
                         DO UPDATE SET plays_user = plays_user + 1",
                        params![scope as i64, cache_key, day],
                    )?;
                }
            }
        }

        StatEvent::Vote {
            guild_id,
            voter_id,
            owner_id,
            cache_key,
            track_json,
            flavor,
            added,
        } => {
            let delta: i64 = if *added { 1 } else { -1 };
            let (give, recv) = match flavor {
                VoteFlavor::Like => ("likes_give", "likes_recv"),
                VoteFlavor::Super => ("supers_give", "supers_recv"),
                VoteFlavor::Dislike => ("dislikes_give", "dislikes_recv"),
            };

            // 준 사람
            touch_user(tx, *guild_id, *voter_id, &now)?;
            tx.execute(
                &format!(
                    "UPDATE stat_user SET {give} = MAX(0, {give} + ?3), last_utc = ?4
                     WHERE guild_id = ?1 AND user_id = ?2"
                ),
                params![*guild_id as i64, *voter_id as i64, delta, now],
            )?;
            if matches!(flavor, VoteFlavor::Like | VoteFlavor::Super) {
                touch_user_track(tx, *guild_id, *voter_id, cache_key, track_json, &now)?;
                tx.execute(
                    "UPDATE stat_user_track SET liked = MAX(0, liked + ?4), last_utc = ?5
                     WHERE guild_id = ?1 AND user_id = ?2 AND cache_key = ?3",
                    params![*guild_id as i64, *voter_id as i64, cache_key, delta, now],
                )?;
            }

            // 받은 사람 (자기 곡에는 투표할 수 없으니 보통 다른 사람이다)
            if let Some(owner) = owner_id {
                touch_user(tx, *guild_id, *owner, &now)?;
                tx.execute(
                    &format!(
                        "UPDATE stat_user SET {recv} = MAX(0, {recv} + ?3), last_utc = ?4
                         WHERE guild_id = ?1 AND user_id = ?2"
                    ),
                    params![*guild_id as i64, *owner as i64, delta, now],
                )?;
                touch_user_track(tx, *guild_id, *owner, cache_key, track_json, &now)?;
                tx.execute(
                    "UPDATE stat_user_track SET likes_recv = MAX(0, likes_recv + ?4), last_utc = ?5
                     WHERE guild_id = ?1 AND user_id = ?2 AND cache_key = ?3",
                    params![*guild_id as i64, *owner as i64, cache_key, delta, now],
                )?;
                if matches!(flavor, VoteFlavor::Like) {
                    bump_daily(tx, *guild_id, *owner, &day, 0, 0, delta)?;
                }
            }

            // 차트용 곡 누적
            if matches!(flavor, VoteFlavor::Like | VoteFlavor::Super) {
                let column = if matches!(flavor, VoteFlavor::Like) {
                    "likes"
                } else {
                    "supers"
                };
                for scope in [*guild_id, ALL_GUILDS] {
                    touch_track(tx, scope, cache_key, track_json, &now)?;
                    tx.execute(
                        &format!(
                            "UPDATE stat_track_plays SET {column} = MAX(0, {column} + ?3), last_utc = ?4
                             WHERE guild_id = ?1 AND cache_key = ?2"
                        ),
                        params![scope as i64, cache_key, delta, now],
                    )?;
                    tx.execute(
                        &format!(
                            "INSERT INTO stat_track_daily(guild_id, cache_key, day, {column})
                             VALUES(?1, ?2, ?3, MAX(0, ?4))
                             ON CONFLICT(guild_id, cache_key, day)
                             DO UPDATE SET {column} = MAX(0, {column} + ?4)"
                        ),
                        params![scope as i64, cache_key, day, delta],
                    )?;
                }
            }
        }

        StatEvent::Boomtta {
            guild_id,
            owner_id,
            cache_key: _,
        } => {
            if let Some(owner) = owner_id {
                touch_user(tx, *guild_id, *owner, &now)?;
                tx.execute(
                    "UPDATE stat_user SET boomtta = boomtta + 1, last_utc = ?3
                     WHERE guild_id = ?1 AND user_id = ?2",
                    params![*guild_id as i64, *owner as i64, now],
                )?;
            }
        }

        StatEvent::Chat { guild_id, user_id } => {
            touch_user(tx, *guild_id, *user_id, &now)?;
            tx.execute(
                "UPDATE stat_user SET chats = chats + 1, last_utc = ?3
                 WHERE guild_id = ?1 AND user_id = ?2",
                params![*guild_id as i64, *user_id as i64, now],
            )?;
        }
    }
    Ok(())
}

fn touch_user(
    tx: &rusqlite::Transaction<'_>,
    guild_id: u64,
    user_id: u64,
    now: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO stat_user(guild_id, user_id, first_utc, last_utc)
         VALUES(?1, ?2, ?3, ?3)",
        params![guild_id as i64, user_id as i64, now],
    )?;
    Ok(())
}

fn touch_user_track(
    tx: &rusqlite::Transaction<'_>,
    guild_id: u64,
    user_id: u64,
    cache_key: &str,
    track_json: &str,
    now: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO stat_user_track(guild_id, user_id, cache_key, track_json, last_utc)
         VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(guild_id, user_id, cache_key) DO UPDATE SET track_json = ?4",
        params![guild_id as i64, user_id as i64, cache_key, track_json, now],
    )?;
    Ok(())
}

fn touch_track(
    tx: &rusqlite::Transaction<'_>,
    guild_id: u64,
    cache_key: &str,
    track_json: &str,
    now: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO stat_track_plays(guild_id, cache_key, track_json, last_utc)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(guild_id, cache_key) DO UPDATE SET track_json = ?3",
        params![guild_id as i64, cache_key, track_json, now],
    )?;
    Ok(())
}

fn bump_daily(
    tx: &rusqlite::Transaction<'_>,
    guild_id: u64,
    user_id: u64,
    day: &str,
    queued: i64,
    played: i64,
    likes: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO stat_daily(guild_id, user_id, day, queued, played, likes_recv)
         VALUES(?1, ?2, ?3, MAX(0,?4), MAX(0,?5), MAX(0,?6))
         ON CONFLICT(guild_id, user_id, day) DO UPDATE SET
            queued = MAX(0, queued + ?4),
            played = MAX(0, played + ?5),
            likes_recv = MAX(0, likes_recv + ?6)",
        params![guild_id as i64, user_id as i64, day, queued, played, likes],
    )?;
    Ok(())
}

/// 트랙에서 통계용 키와 JSON을 뽑는다. 호출부 편의 함수.
pub fn track_parts(track: &TrackRef) -> (String, String) {
    (
        track.cache_key(),
        serde_json::to_string(track).unwrap_or_else(|_| "null".into()),
    )
}

/// 봇 전체 합계를 가리키는 guild_id.
pub const fn all_guilds() -> u64 {
    ALL_GUILDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_temp() -> Stats {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let (tx, _rx) = mpsc::channel(16);
        Stats {
            conn: Mutex::new(conn),
            tx,
        }
    }

    /// 이벤트를 태스크 없이 바로 반영해 테스트한다.
    fn apply_now(stats: &Stats, events: &[StatEvent]) {
        let mut conn = stats.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        for event in events {
            apply(&tx, event).unwrap();
        }
        tx.commit().unwrap();
    }

    fn queued(guild: u64, user: u64, key: &str, bulk: bool) -> StatEvent {
        StatEvent::Queued {
            guild_id: guild,
            user_id: user,
            cache_key: key.into(),
            track_json: format!("{{\"title\":\"{key}\"}}"),
            bulk,
        }
    }

    /// 신청자가 없으면 자동재생인 흔한 경우. 둘이 갈리는 경우는 아래 전용 테스트가 본다.
    fn played(guild: u64, requester: Option<u64>, key: &str) -> StatEvent {
        StatEvent::Played {
            guild_id: guild,
            requester,
            autoplay: requester.is_none(),
            cache_key: key.into(),
            track_json: format!("{{\"title\":\"{key}\"}}"),
            outcome: PlayOutcome::Completed,
        }
    }

    fn track(content_id: &str) -> TrackRef {
        TrackRef {
            provider: crate::models::ProviderKind::YouTube,
            content_id: content_id.into(),
            source_url: format!("https://example.test/{content_id}"),
            title: Some(content_id.into()),
            artist: None,
            duration: None,
            variant_key: None,
        }
    }

    #[test]
    fn counts_single_and_bulk_separately() {
        let stats = open_temp();
        apply_now(
            &stats,
            &[
                queued(1, 10, "a", false),
                queued(1, 10, "b", true),
                queued(1, 10, "c", true),
                StatEvent::BulkUsed {
                    guild_id: 1,
                    user_id: 10,
                },
            ],
        );
        let user = stats.user_stats(1, 10);
        assert_eq!(user.queued_single, 1);
        assert_eq!(user.queued_bulk, 2);
        assert_eq!(user.bulk_times, 1);
        assert_eq!(user.queued_total(), 3);
    }

    #[test]
    fn autoplay_plays_never_enter_the_chart() {
        let stats = open_temp();
        apply_now(
            &stats,
            &[
                // 사람이 신청해서 1회
                played(1, Some(10), "song"),
                // 자동재생으로 5회 — 순위에 들어가면 안 된다
                played(1, None, "song"),
                played(1, None, "song"),
                played(1, None, "song"),
                played(1, None, "song"),
                played(1, None, "song"),
                // 사람이 신청한 다른 곡 2회
                played(1, Some(11), "other"),
                played(1, Some(12), "other"),
            ],
        );
        let chart = stats.chart(1, ChartKind::Plays, ChartWindow::All, 2, 10);
        assert_eq!(chart.len(), 2);
        // other(2회) 가 song(1회) 보다 위여야 한다. 자동재생 5회는 무시된다.
        assert_eq!(chart[0].cache_key, "other");
        assert_eq!(chart[0].plays_user, 2);
        assert_eq!(chart[1].cache_key, "song");
        assert_eq!(chart[1].plays_user, 1);
        assert_eq!(chart[1].plays_autoplay, 5);
    }

    #[test]
    fn ties_break_on_distinct_requesters() {
        let stats = open_temp();
        apply_now(
            &stats,
            &[
                // 혼자 두 번 신청하고 두 번 재생
                queued(1, 10, "solo", false),
                queued(1, 10, "solo", false),
                played(1, Some(10), "solo"),
                played(1, Some(10), "solo"),
                // 두 사람이 한 번씩
                queued(1, 20, "shared", false),
                queued(1, 21, "shared", false),
                played(1, Some(20), "shared"),
                played(1, Some(21), "shared"),
            ],
        );
        let chart = stats.chart(1, ChartKind::Plays, ChartWindow::All, 2, 10);
        // 재생 수가 같으면 서로 다른 신청자가 많은 쪽이 위
        assert_eq!(chart[0].cache_key, "shared");
        assert_eq!(chart[0].requesters, 2);
        assert_eq!(chart[1].requesters, 1);
    }

    #[test]
    fn love_chart_weights_super_likes() {
        let stats = open_temp();
        let vote = |voter: u64, owner: u64, key: &str, flavor: VoteFlavor| StatEvent::Vote {
            guild_id: 1,
            voter_id: voter,
            owner_id: Some(owner),
            cache_key: key.into(),
            track_json: format!("{{\"title\":\"{key}\"}}"),
            flavor,
            added: true,
        };
        apply_now(
            &stats,
            &[
                // 좋아요 3개
                vote(10, 99, "many_likes", VoteFlavor::Like),
                vote(11, 99, "many_likes", VoteFlavor::Like),
                vote(12, 99, "many_likes", VoteFlavor::Like),
                // 슈퍼 2개
                vote(10, 98, "supers", VoteFlavor::Super),
                vote(11, 98, "supers", VoteFlavor::Super),
            ],
        );
        // 가중치 2 → supers 는 4점, many_likes 는 3점
        let weighted = stats.chart(1, ChartKind::Love, ChartWindow::All, 2, 10);
        assert_eq!(weighted[0].cache_key, "supers");
        assert_eq!(weighted[0].love_score, 4);

        // 가중치 0 → 슈퍼는 무시되므로 many_likes 가 위
        let ignored = stats.chart(1, ChartKind::Love, ChartWindow::All, 0, 10);
        assert_eq!(ignored[0].cache_key, "many_likes");
    }

    #[test]
    fn karma_never_goes_negative() {
        let mut user = UserStats {
            boomtta: 100,
            ..Default::default()
        };
        assert_eq!(user.karma(), 0);
        user.likes_recv = 10;
        user.supers_recv = 2;
        user.played = 4;
        user.boomtta = 1;
        // 10 + 2*3 + 4 - 1*2 = 18
        assert_eq!(user.karma(), 18);
    }

    #[test]
    fn removing_a_vote_gives_the_count_back() {
        let stats = open_temp();
        let make = |added: bool| StatEvent::Vote {
            guild_id: 1,
            voter_id: 10,
            owner_id: Some(20),
            cache_key: "k".into(),
            track_json: "{}".into(),
            flavor: VoteFlavor::Like,
            added,
        };
        apply_now(&stats, &[make(true)]);
        assert_eq!(stats.user_stats(1, 20).likes_recv, 1);
        apply_now(&stats, &[make(false)]);
        assert_eq!(stats.user_stats(1, 20).likes_recv, 0);
        // 취소를 한 번 더 해도 음수로 내려가지 않는다
        apply_now(&stats, &[make(false)]);
        assert_eq!(stats.user_stats(1, 20).likes_recv, 0);
    }

    #[test]
    fn bot_wide_chart_aggregates_every_guild() {
        let stats = open_temp();
        apply_now(
            &stats,
            &[
                played(1, Some(10), "hit"),
                played(2, Some(20), "hit"),
                played(3, Some(30), "hit"),
                played(1, Some(10), "local"),
            ],
        );
        let guild = stats.chart(1, ChartKind::Plays, ChartWindow::All, 2, 10);
        assert_eq!(guild[0].plays_user, 1); // 길드 1 에서는 hit 도 local 도 1회

        let all = stats.chart(all_guilds(), ChartKind::Plays, ChartWindow::All, 2, 10);
        assert_eq!(all[0].cache_key, "hit");
        assert_eq!(all[0].plays_user, 3);
    }

    /// 자동재생 판정은 `request_kind` 로만 한다. 신청자 ID 가 없다는 이유로 자동재생 취급하면
    /// `/이전곡`으로 되돌아간 곡(사람이 시켰지만 신청자 ID 가 없다)이 차트에서 빠진다.
    #[test]
    fn autoplay_is_decided_by_request_kind_not_by_a_missing_requester() {
        let auto = QueueItem::new_autoplay(track("auto"));
        let StatEvent::Played {
            requester, autoplay, ..
        } = StatEvent::played_from_item(1, &auto, PlayOutcome::Completed)
        else {
            panic!("played_from_item 은 Played 를 만들어야 한다");
        };
        assert!(autoplay);
        assert_eq!(requester, None);

        // `/이전곡` 이 만드는 항목: 사람이 시켰는데 신청자 ID 가 없다.
        let previous = QueueItem::new_user(track("prev"), "(이전 곡)".into(), None);
        let StatEvent::Played {
            requester, autoplay, ..
        } = StatEvent::played_from_item(1, &previous, PlayOutcome::Completed)
        else {
            panic!("played_from_item 은 Played 를 만들어야 한다");
        };
        assert!(!autoplay, "신청자 ID 가 없다고 자동재생으로 보면 안 된다");
        assert_eq!(requester, None);
    }

    /// 위 판정이 실제 차트 숫자까지 그대로 흘러간다.
    #[test]
    fn a_requesterless_human_play_still_counts_in_the_chart() {
        let stats = open_temp();
        let previous = QueueItem::new_user(track("prev"), "(이전 곡)".into(), None);
        let auto = QueueItem::new_autoplay(track("auto"));
        apply_now(
            &stats,
            &[
                StatEvent::played_from_item(1, &previous, PlayOutcome::Completed),
                StatEvent::played_from_item(1, &auto, PlayOutcome::Completed),
            ],
        );
        let chart = stats.chart(1, ChartKind::Plays, ChartWindow::All, 2, 10);
        assert_eq!(chart.len(), 1, "자동재생 곡은 재생 차트에 오르지 않는다");
        assert_eq!(chart[0].cache_key, previous.track.cache_key());
        assert_eq!(chart[0].plays_user, 1);
        assert_eq!(chart[0].plays_autoplay, 0);
    }

    /// 스킵도 재생 횟수로는 센다 — 신청자에게는 `skipped` 로 따로 남는다.
    #[test]
    fn skipping_marks_the_requester_but_still_counts_a_play() {
        let stats = open_temp();
        let item = QueueItem::new_user(track("song"), "민수".into(), Some(10));
        apply_now(
            &stats,
            &[StatEvent::played_from_item(1, &item, PlayOutcome::Skipped)],
        );
        let user = stats.user_stats(1, 10);
        assert_eq!(user.played, 0);
        assert_eq!(user.skipped, 1);
        let chart = stats.chart(1, ChartKind::Plays, ChartWindow::All, 2, 10);
        assert_eq!(chart[0].plays_user, 1);
    }

    #[test]
    fn top_tracks_reject_unknown_order_columns() {
        let stats = open_temp();
        apply_now(&stats, &[queued(1, 10, "a", false)]);
        assert_eq!(stats.top_user_tracks(1, 10, "requested", 5).len(), 1);
        // SQL 에 그대로 들어가는 값이라 화이트리스트 밖은 빈 결과로 막는다
        assert!(
            stats
                .top_user_tracks(1, 10, "1; DROP TABLE stat_user_track", 5)
                .is_empty()
        );
    }
}
