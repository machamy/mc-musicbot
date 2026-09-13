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
    /// 받아 놓은 파일에서 길이를 읽을 때 쓴다 (`probe_duration`).
    ffmpeg: String,
    /// 지금 받고 있는 곡 → 그 곡 전용 잠금 (`prepare` 참고).
    inflight: std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pins: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    background_attempts: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

pub struct CachePin {
    cache: Arc<CacheManager>,
    key: String,
}

impl Drop for CachePin {
    fn drop(&mut self) {
        let mut pins = self.cache.pins.lock().unwrap();
        if let Some(count) = pins.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 { pins.remove(&self.key); }
        }
    }
}

fn sanitize_file_name(value: &str) -> String {
    value
        .replace(':', "_")
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect()
}

impl CacheManager {
    pub fn new(dir: PathBuf, db: Arc<Db>, log: Arc<LogService>, ffmpeg: String) -> CacheManager {
        let _ = std::fs::create_dir_all(&dir);
        CacheManager {
            dir,
            db,
            log,
            ffmpeg,
            inflight: std::sync::Mutex::new(std::collections::HashMap::new()),
            pins: std::sync::Mutex::new(std::collections::HashMap::new()),
            background_attempts: std::sync::Mutex::new(std::collections::HashMap::new()),
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

    /// 파일을 열기 전부터 보호해야 LRU 검사와 open 사이에 지워지지 않는다.
    pub fn pin(self: &Arc<Self>, key: &str) -> Option<(CacheEntry, CachePin)> {
        let mut pins = self.pins.lock().unwrap();
        let entry = self.get(key)?;
        *pins.entry(key.to_string()).or_default() += 1;
        Some((entry, CachePin { cache: self.clone(), key: key.to_string() }))
    }

    pub fn delete(&self, key: &str) -> bool {
        let pins = self.pins.lock().unwrap();
        if pins.contains_key(key) { return false; }
        if let Some(entry) = self.db.get_cache_entry(key) {
            if Path::new(&entry.file_path).exists() && std::fs::remove_file(&entry.file_path).is_err() {
                return false;
            }
        }
        self.db.delete_cache_entries(&[key.to_string()]);
        true
    }

    pub fn begin_background_prepare(&self, key: &str) -> bool {
        let mut attempts = self.background_attempts.lock().unwrap();
        attempts.retain(|_, at| at.elapsed() < std::time::Duration::from_secs(60));
        if attempts.contains_key(key) { return false; }
        attempts.insert(key.to_string(), std::time::Instant::now());
        true
    }

    pub fn register(&self, track: &TrackRef, file_path: &str, size_bytes: i64) {
        self.register_with_duration(track, file_path, size_bytes, track.duration);
    }

    /// 길이를 따로 알아냈을 때 쓰는 등록 (`probe_duration` 참고).
    pub fn register_with_duration(
        &self,
        track: &TrackRef,
        file_path: &str,
        size_bytes: i64,
        duration: Option<crate::models::CsTimeSpan>,
    ) {
        let entry = CacheEntry {
            cache_key: track.cache_key(),
            provider: track.provider,
            content_id: track.content_id.clone(),
            source_url: track.source_url.clone(),
            title: track.title.clone(),
            duration,
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
        /* **여기서 곡 길이를 확정한다.**
         *
         * 검색(`--flat-playlist`)으로 담은 곡은 길이가 안 온다. 그러면 화면의 총 시간이
         * `0:00` 으로 나오고(실측: 최근 50곡 중 4곡), 웹 재생기는 아예 **길이를 모른다는
         * 이유로 그 곡을 건너뛴다**(`coordinator` 의 가상 재생 갈래).
         *
         * 그런데 여기까지 왔으면 파일이 손에 있다. 받아 놓고도 안 물어본 셈이었다.
         * 실패해도 예전과 같을 뿐이라 잃을 게 없다. */
        let duration = match track.duration {
            Some(d) => Some(d),
            None => probe_duration(&self.ffmpeg, &path).await,
        };
        self.register_with_duration(track, &path, size, duration);
        self.log.info(
            "Download",
            &format!("Prepared {} using auth mode '{mode}'.", track.cache_key()),
        );
        /* 기동 때는 "우리가 못 박지 못했다" 까지만 말할 수 있다. 무엇이 실제로 쓰이는지는
         * 곡을 한 곡 받아 봐야 yt-dlp 가 stderr 로 알려 준다. 그 답을 여기서 한 번만 적는다.
         * 이 줄이 뜨면 운영자는 JS 런타임 쪽을 지우고 다른 데를 볼 수 있다. */
        if let Some(runtime) = crate::media::ytdlp::take_jsc_notice() {
            self.log.info(
                "Tools",
                &format!("yt-dlp 가 유튜브 서명을 {runtime} 로 풀고 있어요 (yt-dlp 자체 탐색)."),
            );
        }
        self.prune_to_limit((cache_limit_gb as i64) * 1024 * 1024 * 1024);
        Ok((path, false))
    }

    /// LRU 정리: 상한 초과분을 오래된 접근순으로 삭제. 잠긴 파일(재생 중)은 건너뜀.
    pub fn prune_to_limit(&self, limit_bytes: i64) {
        let pins = self.pins.lock().unwrap();
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
            if pins.contains_key(&e.cache_key) { continue; }
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
        let pins = self.pins.lock().unwrap();
        let entries = self.db.all_cache_entries();
        let mut deleted = Vec::new();
        let mut skipped = 0usize;
        for e in entries {
            if pins.contains_key(&e.cache_key) { skipped += 1; continue; }
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
            if self.pins.lock().unwrap().contains_key(&entry.cache_key) { failed += 1; continue; }
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

/* ── 받아 놓은 파일에서 길이를 읽는다 ────────────────────────────
 *
 * 검색(`--flat-playlist`)으로 담은 곡은 메타에 길이가 없다. 그러면 화면 총 시간이
 * `0:00` 이 되고, 웹 재생기는 그 곡을 **길이를 모른다는 이유로 건너뛴다.**
 *
 * `ffprobe` 는 배포본에 없다(실서버 `tools\` 에 `ffmpeg.exe` 하나뿐). 그런데 `ffmpeg -i`
 * 는 입력만 읽고 `Duration: 00:03:24.15` 를 stderr 에 적어 준다 — 출력이 없다고 실패
 * 코드로 끝나지만 그 줄은 이미 나온 뒤다. 그래서 종료 코드를 안 보고 stderr 만 읽는다.
 */
async fn probe_duration(ffmpeg: &str, path: &str) -> Option<crate::models::CsTimeSpan> {
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-i", path]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::process::Command::from(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_ffmpeg_duration(&String::from_utf8_lossy(&out.stderr))
}

/// `ffmpeg -i` 의 stderr 에서 `Duration: HH:MM:SS.ss` 를 뽑는다.
/// **순수 함수라 ffmpeg 없이도 검사할 수 있다.**
fn parse_ffmpeg_duration(stderr: &str) -> Option<crate::models::CsTimeSpan> {
    let rest = stderr.split("Duration:").nth(1)?;
    let head = rest.split(',').next()?.trim();
    // `N/A` 는 길이를 모른다는 뜻이다 — 0으로 적어 두면 아는 척이 된다.
    if head.starts_with("N/A") {
        return None;
    }
    let mut secs = 0f64;
    for part in head.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        secs = secs * 60.0 + v;
    }
    (secs > 0.0).then(|| crate::models::CsTimeSpan::from_secs_f64(secs))
}

#[cfg(test)]
mod duration_probe_tests {
    use super::parse_ffmpeg_duration;

    /// ffmpeg 이 실제로 뱉는 줄. **여기가 깨지면 길이 없는 곡이 다시 `0:00` 이 된다.**
    #[test]
    fn a_real_ffmpeg_line_is_read() {
        let out = "  Duration: 00:03:24.15, start: 0.000000, bitrate: 128 kb/s\n";
        let d = parse_ffmpeg_duration(out).expect("길이를 못 읽었어요");
        assert!((d.as_secs_f64() - 204.15).abs() < 0.01, "{}", d.as_secs_f64());
    }

    /// 한 시간이 넘는 것도 자릿수 그대로 읽는다.
    #[test]
    fn an_hour_long_file_is_read() {
        let d = parse_ffmpeg_duration("Duration: 01:02:03.00, bitrate: 1 kb/s").unwrap();
        assert!((d.as_secs_f64() - 3723.0).abs() < 0.01);
    }

    /// **모르면 모른다고 한다.** 0으로 적어 두면 아는 척이 되어 화면이 `0:00` 을 확신한다.
    #[test]
    fn unknown_stays_unknown() {
        assert!(parse_ffmpeg_duration("Duration: N/A, bitrate: N/A").is_none());
        assert!(parse_ffmpeg_duration("아무 상관 없는 출력").is_none());
        assert!(parse_ffmpeg_duration("Duration: 00:00:00.00, x").is_none());
    }
}
