//! 캐시 매니저: 다운로드-후-재생 전략의 디스크 캐시 + SQLite 메타.
//! C# CacheManager + CacheMigrationService 포팅 (LRU 정리, MP3→Opus 마이그, 전체 비우기).

use crate::db::Db;
use crate::logging::LogService;
use crate::media::ytdlp::YtDlp;
use crate::models::{CacheEntry, TrackRef};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

pub struct CacheManager {
    pub dir: PathBuf,
    db: Arc<Db>,
    log: Arc<LogService>,
    /// 지금 받고 있는 곡 → 그 곡 전용 잠금 (`prepare` 참고).
    inflight: std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn sanitize_file_name(value: &str) -> String {
    value
        .replace(':', "_")
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect()
}

impl CacheManager {
    pub fn new(dir: PathBuf, db: Arc<Db>, log: Arc<LogService>) -> CacheManager {
        let _ = std::fs::create_dir_all(&dir);
        CacheManager {
            dir,
            db,
            log,
            inflight: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 캐시 적중 검사 — 메타와 실제 파일 둘 다 있어야 한다. 적중 시 LRU 갱신.
    pub fn get(&self, cache_key: &str) -> Option<CacheEntry> {
        let mut entry = self.db.get_cache_entry(cache_key)?;
        if !Path::new(&entry.file_path).is_file() {
            return None;
        }
        entry.last_access_utc = chrono::Utc::now().to_rfc3339();
        self.db.upsert_cache_entry(&entry);
        Some(entry)
    }

    pub fn register(&self, track: &TrackRef, file_path: &str, size_bytes: i64) {
        let entry = CacheEntry {
            cache_key: track.cache_key(),
            provider: track.provider,
            content_id: track.content_id.clone(),
            source_url: track.source_url.clone(),
            title: track.title.clone(),
            duration: track.duration,
            file_path: file_path.to_string(),
            size_bytes,
            loudness_profile: None,
            last_access_utc: chrono::Utc::now().to_rfc3339(),
            play_count: 0,
            last_played_utc: None,
            per_guild: std::collections::HashMap::new(),
        };
        self.db.upsert_cache_entry(&entry);
    }

    /// 곡이 실제로 재생되기 시작할 때 호출 — 전역/서버별 재생 횟수와 마지막 재생 시각 갱신.
    /// 캐시 미스로 아직 메타가 없으면 조용히 무시(prepare 가 먼저 register 하므로 보통 존재).
    pub fn record_play(&self, cache_key: &str, guild_id: u64) {
        if let Some(mut entry) = self.db.get_cache_entry(cache_key) {
            let now = chrono::Utc::now().to_rfc3339();
            entry.play_count += 1;
            entry.last_played_utc = Some(now.clone());
            let g = entry.per_guild.entry(guild_id).or_default();
            g.count += 1;
            g.last_played_utc = Some(now);
            self.db.upsert_cache_entry(&entry);
        }
    }

    /// 트랙을 재생 가능한 로컬 파일로 준비한다 (캐시 미스 시 yt-dlp 다운로드).
    ///
    /// **같은 곡은 한 번만 받는다.** 예전에는 이걸 막는 게 아무것도 없어서, 미리 받아 두는
    /// 쪽과 실제 재생 쪽이 같은 곡에 겹치면 **서로를 망가뜨렸다.** 뒤에 온 쪽이 아직 등록
    /// 전이라 캐시 미스로 판정하고, 아래 잔재 정리에서 **앞선 다운로드가 쓰고 있던 중간
    /// 파일을 지운 뒤** 같은 이름으로 yt-dlp 를 하나 더 띄웠다. 미리 받아 둔 보람이 사라질
    /// 뿐 아니라 처음부터 다시 받게 된다 — 곡을 빨리 넘겼을 때 유독 오래 걸리던 게 이것이다.
    ///
    /// 이제 곡별 잠금을 잡고, 잠금을 얻은 뒤 **캐시를 한 번 더 본다.** 기다리는 동안 앞선
    /// 쪽이 끝냈으면 그 결과를 그대로 쓴다.
    pub async fn prepare(
        &self,
        track: &TrackRef,
        ytdlp: &YtDlp,
        cache_limit_gb: i32,
        remove_segments: bool,
    ) -> Result<(String, bool), String> {
        let cache_key = track.cache_key();
        if let Some(hit) = self.get(&cache_key) {
            return Ok((hit.file_path, true));
        }

        let gate = {
            let mut map = self.inflight.lock().unwrap();
            map.entry(cache_key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = gate.lock().await;
        // 기다리는 동안 앞선 쪽이 끝냈을 수 있다. 그러면 받지 않는다.
        if let Some(hit) = self.get(&cache_key) {
            self.release_inflight(&cache_key, &gate);
            return Ok((hit.file_path, true));
        }
        let outcome = self
            .download_uncached(track, ytdlp, cache_limit_gb, remove_segments)
            .await;
        self.release_inflight(&cache_key, &gate);
        outcome
    }

    /// 잠금을 놓은 뒤 지도에서도 치운다. 기다리는 사람이 남아 있으면 그대로 둔다 —
    /// 지워 버리면 그 사람들이 서로 다른 잠금을 잡게 되어 중복 방지가 무너진다.
    fn release_inflight(&self, cache_key: &str, gate: &Arc<tokio::sync::Mutex<()>>) {
        let mut map = self.inflight.lock().unwrap();
        // 나(1) + 지도(1) 뿐이면 아무도 안 기다리는 것이다.
        if Arc::strong_count(gate) <= 2 {
            map.remove(cache_key);
        }
    }

    async fn download_uncached(
        &self,
        track: &TrackRef,
        ytdlp: &YtDlp,
        cache_limit_gb: i32,
        remove_segments: bool,
    ) -> Result<(String, bool), String> {
        let base = sanitize_file_name(&track.cache_key());
        // 같은 base 의 이전 잔재 제거 (부분 다운로드 등).
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                /* **접두사만 맞으면 지우던 것을 고쳤다.**
                 *
                 * `soundcloud_artist_song` 을 받으려는데 이미 캐시돼 있던
                 * `soundcloud_artist_song-remix.opus` 가 같이 지워졌다. 유튜브 ID 는 길이가
                 * 고정이라 안 겪지만 사운드클라우드는 슬러그라 길이가 제각각이다.
                 * 그래서 분명히 예전에 튼 곡인데 매번 다시 받는 일이 생긴다.
                 * 내 잔재는 `base` 바로 뒤가 반드시 `.` 다 (`{base}.%(ext)s`). */
                let mine = name
                    .strip_prefix(&base)
                    .is_some_and(|rest| rest.starts_with('.'));
                if mine {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let template = self
            .dir
            .join(format!("{base}.%(ext)s"))
            .to_string_lossy()
            .to_string();
        let (path, mode) = ytdlp
            .download(&track.source_url, &template, remove_segments)
            .await?;
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        self.register(track, &path, size);
        self.log.info(
            "Download",
            &format!("Prepared {} using auth mode '{mode}'.", track.cache_key()),
        );
        self.prune_to_limit((cache_limit_gb as i64) * 1024 * 1024 * 1024);
        Ok((path, false))
    }

    /// LRU 정리: 상한 초과분을 오래된 접근순으로 삭제. 잠긴 파일(재생 중)은 건너뜀.
    pub fn prune_to_limit(&self, limit_bytes: i64) {
        let mut entries = self.db.all_cache_entries();
        let mut total: i64 = entries
            .iter()
            .map(|e| {
                std::fs::metadata(&e.file_path)
                    .map(|m| m.len() as i64)
                    .unwrap_or(e.size_bytes)
            })
            .sum();
        if total <= limit_bytes {
            return;
        }
        entries.sort_by(|a, b| a.last_access_utc.cmp(&b.last_access_utc));
        let mut removed_keys = Vec::new();
        for e in entries {
            if total <= limit_bytes {
                break;
            }
            let size = std::fs::metadata(&e.file_path)
                .map(|m| m.len() as i64)
                .unwrap_or(e.size_bytes);
            match std::fs::remove_file(&e.file_path) {
                Ok(_) => {
                    total -= size;
                    removed_keys.push(e.cache_key);
                }
                Err(_) => continue, // 재생 중 잠김 등 — 다음 기회에.
            }
        }
        if !removed_keys.is_empty() {
            self.db.delete_cache_entries(&removed_keys);
            self.log.info(
                "Cache",
                &format!("Pruned {} cached tracks (limit).", removed_keys.len()),
            );
        }
    }

    pub fn stats(&self) -> (usize, i64) {
        let entries = self.db.all_cache_entries();
        let total = entries
            .iter()
            .map(|e| {
                std::fs::metadata(&e.file_path)
                    .map(|m| m.len() as i64)
                    .unwrap_or(e.size_bytes)
            })
            .sum();
        (entries.len(), total)
    }

    /// 전체 비우기 — 파일+메타. 잠긴 파일은 skip 카운트로 보고.
    pub fn wipe_all(&self) -> (usize, usize) {
        let entries = self.db.all_cache_entries();
        let mut deleted = Vec::new();
        let mut skipped = 0usize;
        for e in entries {
            if Path::new(&e.file_path).is_file() {
                if std::fs::remove_file(&e.file_path).is_err() {
                    skipped += 1;
                    continue;
                }
            }
            deleted.push(e.cache_key);
        }
        let count = deleted.len();
        self.db.delete_cache_entries(&deleted);
        (count, skipped)
    }

    /// 포맷 분포 분석 (마이그 계획).
    pub fn inspect_formats(&self) -> (usize, usize, usize, usize, i64) {
        let entries = self.db.all_cache_entries();
        let (mut mp3, mut opus, mut other) = (0usize, 0usize, 0usize);
        let mut mp3_bytes: i64 = 0;
        for e in &entries {
            let ext = Path::new(&e.file_path)
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            match ext.as_str() {
                "mp3" => {
                    mp3 += 1;
                    mp3_bytes += std::fs::metadata(&e.file_path)
                        .map(|m| m.len() as i64)
                        .unwrap_or(e.size_bytes);
                }
                "opus" | "ogg" => opus += 1,
                _ => other += 1,
            }
        }
        let saved_mb = (mp3_bytes as f64 * 0.45 / 1024.0 / 1024.0) as i64;
        (entries.len(), mp3, opus, other, saved_mb)
    }

    /// MP3 → Opus 일괄 재인코딩 (ffmpeg). 결과: (성공, 실패).
    pub async fn migrate_mp3_to_opus(&self, ffmpeg: &str) -> (usize, usize) {
        let entries: Vec<CacheEntry> = self
            .db
            .all_cache_entries()
            .into_iter()
            .filter(|e| e.file_path.to_lowercase().ends_with(".mp3"))
            .collect();
        let (mut ok, mut failed) = (0usize, 0usize);
        for mut entry in entries {
            let src = entry.file_path.clone();
            if !Path::new(&src).is_file() {
                failed += 1;
                continue;
            }
            let dst = Path::new(&src)
                .with_extension("opus")
                .to_string_lossy()
                .to_string();
            // libopus 는 -vbr 에 on/off 가 아닌 0/1/2 만 받는다 (기본 1=VBR). 명시 불필요.
            let status = Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-i",
                    &src,
                    "-vn",
                    "-c:a",
                    "libopus",
                    "-b:a",
                    "128k",
                    "-application",
                    "audio",
                    "-compression_level",
                    "10",
                    &dst,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            match status {
                Ok(s) if s.success() && Path::new(&dst).is_file() => {
                    let _ = std::fs::remove_file(&src);
                    entry.file_path = dst.clone();
                    entry.size_bytes = std::fs::metadata(&dst)
                        .map(|m| m.len() as i64)
                        .unwrap_or(entry.size_bytes);
                    entry.last_access_utc = chrono::Utc::now().to_rfc3339();
                    self.db.upsert_cache_entry(&entry);
                    ok += 1;
                }
                _ => {
                    let _ = std::fs::remove_file(&dst);
                    failed += 1;
                }
            }
        }
        (ok, failed)
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_file_name;

    /// 잔재를 지우는 규칙을 여기 한 번 더 적어 둔다. `prepare` 는 `{base}.%(ext)s` 로
    /// 받으므로 **내 파일은 `base` 바로 뒤가 반드시 `.`** 다.
    fn is_my_residue(name: &str, base: &str) -> bool {
        name.strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('.'))
    }

    /// **접두사만 맞으면 지우던 것을 막는다.**
    ///
    /// 사운드클라우드 키는 길이가 제각각인 슬러그라, `.../song` 을 받으면서
    /// 이미 캐시된 `.../song-remix` 를 같이 지웠다. 그러면 분명히 예전에 튼 곡인데
    /// 매번 다시 받게 된다 — 이 봇은 통째로 받아야 소리가 나므로 그대로 지연이다.
    #[test]
    fn residue_cleanup_does_not_eat_a_longer_neighbour() {
        let base = sanitize_file_name("soundcloud:artist/song");

        // 내 것 — 지워야 한다
        assert!(is_my_residue(&format!("{base}.opus"), &base));
        assert!(is_my_residue(&format!("{base}.part"), &base));
        assert!(is_my_residue(&format!("{base}.webm.part"), &base));

        // 남의 것 — 건드리면 안 된다
        assert!(!is_my_residue(&format!("{base}-remix.opus"), &base));
        assert!(!is_my_residue(&format!("{base}2.opus"), &base));
        assert!(!is_my_residue(&format!("{base}_live.opus"), &base));
    }

    /// 키에 든 경로 구분자가 파일 이름으로 새 나가면 안 된다.
    #[test]
    fn cache_keys_never_become_paths() {
        let name = sanitize_file_name("soundcloud:artist/song");
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains(':'), "{name}");
        assert!(!name.contains('\\'), "{name}");
    }
}
