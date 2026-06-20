//! 차단 규칙 평가 — C# BlacklistService 와 동일 의미론 (TitleContains/TitleExact/UrlExact + URL 정규화).

use crate::db::Db;
use crate::models::{BlacklistEntry, BlacklistKind, TrackRef};
use std::sync::Arc;

pub struct Blacklist {
    db: Arc<Db>,
}

impl Blacklist {
    pub fn new(db: Arc<Db>) -> Blacklist {
        Blacklist { db }
    }

    /// URL 정규화: scheme/host 소문자화, path 끝 슬래시 제거, 쿼리는 v 파라미터만 유지.
    pub fn canonicalize_url(url: &str) -> String {
        let trimmed = url.trim();
        let Some(u) = crate::media::resolver::url_lite::Url::parse(trimmed) else {
            return trimmed.to_string();
        };
        let path = u.path.trim_end_matches('/');
        let v = u.query_pairs().find(|(k, _)| k == "v").map(|(_, val)| val);
        match v {
            Some(v) => format!("{}://{}{}?v={}", u.scheme, u.host, path, v),
            None => format!("{}://{}{}", u.scheme, u.host, path),
        }
    }

    pub fn try_get_blocker(&self, guild_id: u64, track: &TrackRef) -> Option<BlacklistEntry> {
        let entries = self.db.list_blacklist(guild_id);
        let title = track.title.as_deref().unwrap_or("").trim().to_string();
        let title_lower = title.to_lowercase();
        let canonical = Self::canonicalize_url(&track.source_url);
        entries.into_iter().find(|e| match e.kind {
            BlacklistKind::TitleContains => {
                let needle = e.pattern.to_lowercase();
                !needle.is_empty() && title_lower.contains(&needle)
            }
            BlacklistKind::TitleExact => {
                let needle = e.pattern.trim();
                !needle.is_empty() && title.eq_ignore_ascii_case(needle)
            }
            BlacklistKind::UrlExact => !e.pattern.is_empty() && canonical == e.pattern,
        })
    }

    pub fn is_blocked(&self, guild_id: u64, track: &TrackRef) -> bool {
        self.try_get_blocker(guild_id, track).is_some()
    }

    pub fn describe_rule(rule: &BlacklistEntry) -> String {
        format!("{} '{}'", rule.kind.label(), rule.pattern)
    }
}
