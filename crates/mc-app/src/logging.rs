//! 운영 로그: 일자별 JSONL 파일 + 메모리 링버퍼(웹 로그뷰어용).
//! C# LogService 와 같은 3레벨(Info/Warn/Error) + 카테고리 구조.

use crate::models::LogEntry;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct LogService {
    dir: PathBuf,
    ring: Mutex<VecDeque<LogEntry>>,
    /// 파일에 붙이는 동안 잡는 자물쇠. **왜 필요한지는 `write` 의 주석에 적어 뒀다.**
    file: Mutex<()>,
}

const RING_CAP: usize = 2000;

impl LogService {
    pub fn new(dir: PathBuf) -> LogService {
        let _ = std::fs::create_dir_all(&dir);
        let svc = LogService {
            dir,
            ring: Mutex::new(VecDeque::with_capacity(RING_CAP)),
            file: Mutex::new(()),
        };
        svc.load_recent_from_disk();
        svc
    }

    fn load_recent_from_disk(&self) {
        // 시작 시 오늘 파일 꼬리를 링으로 복원해 웹뷰어가 재시작 전 로그도 보게 한다.
        let path = self.today_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut ring = self.ring.lock().unwrap();
            for line in text
                .lines()
                .rev()
                .take(RING_CAP)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                if let Ok(e) = serde_json::from_str::<LogEntry>(line) {
                    ring.push_back(e);
                }
            }
        }
    }

    fn today_path(&self) -> PathBuf {
        // 파일명 날짜도 로컬(한국시간) 기준 — 로그 항목 시각과 어긋나지 않게.
        self.dir
            .join(format!(
                "mc-musicbot-{}.jsonl",
                chrono::Local::now().format("%Y%m%d")
            ))
    }

    pub fn write(&self, level: &str, category: &str, message: &str) {
        let entry = LogEntry {
            // 로컬(한국시간) 기준 rfc3339 — 값에 +09:00 오프셋이 포함된다.
            timestamp: chrono::Local::now().to_rfc3339(),
            level: level.to_string(),
            category: category.to_string(),
            message: message.to_string(),
        };
        // 콘솔에도 (봇 창 디버깅용).
        println!(
            "[{}] {} {}: {}",
            chrono::Local::now().format("%H:%M:%S"),
            level,
            category,
            message
        );
        /* **한 줄은 한 번에 쓴다.**
         *
         * 예전에는 `writeln!(f, "{line}")` 였다. 그게 본문과 개행을 **따로** write 한다
         * (`write_fmt` 가 조각마다 부른다). append 모드에서 write 하나하나는 원자적이지만
         * **둘이 한 쌍으로는 아니다.** 그래서 두 스레드가 겹치면 이렇게 됐다.
         *
         *     A 본문 → B 본문 → A 개행 → B 개행
         *     결과:  {A}{B}
         *            (빈 줄)
         *
         * 운영 로그에서 실제로 매일 5~28줄이 이 모양으로 깨져 있었다(2026-09-02 이후 전부).
         * 겹친 두 레코드의 시각 차는 100마이크로초 안팎이었다. JSONL 은 한 줄이 한 레코드라는
         * 약속이라, 깨진 줄은 파싱에서 통째로 빠지고 그 사고 기록이 조용히 사라진다.
         *
         * 개행까지 담은 버퍼 하나를 `write_all` 로 한 번에 보내고, 그 위에 자물쇠를 덧댄다.
         * 자물쇠까지 두는 이유는 큰 줄이 여러 write 로 쪼개질 여지를 없애기 위해서다 —
         * 로그 한 줄이 길어질 수 있는 곳(yt-dlp stderr 등)이 실제로 있다. */
        if let Ok(mut line) = serde_json::to_string(&entry) {
            line.push('\n');
            let _guard = self.file.lock();
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.today_path())
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        let mut ring = self.ring.lock().unwrap();
        if ring.len() >= RING_CAP {
            ring.pop_front();
        }
        ring.push_back(entry);
    }

    pub fn info(&self, category: &str, message: &str) {
        self.write("Info", category, message);
    }
    pub fn warn(&self, category: &str, message: &str) {
        self.write("Warn", category, message);
    }
    pub fn error(&self, category: &str, message: &str) {
        self.write("Error", category, message);
    }

    pub fn recent(&self, count: usize) -> Vec<LogEntry> {
        let ring = self.ring.lock().unwrap();
        ring.iter().rev().take(count).cloned().collect()
    }

    /// 보관 일수를 넘긴 로그 파일 삭제.
    pub fn prune(&self, retention_days: i64) {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days.max(1));
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(date_part) = name
                    .strip_prefix("mc-musicbot-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                {
                    if let Ok(d) = chrono::NaiveDate::parse_from_str(date_part, "%Y%m%d") {
                        if d.and_hms_opt(0, 0, 0)
                            .map(|dt| dt.and_utc() < cutoff)
                            .unwrap_or(false)
                        {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **회귀 가드: 여러 스레드가 동시에 찍어도 한 줄에 한 레코드여야 한다.**
    ///
    /// 예전 `writeln!` 은 본문과 개행을 따로 써서, 겹치면 `{A}{B}` + 빈 줄이 남았다.
    /// 그 줄은 JSONL 파싱에서 통째로 빠진다 — 사고 기록이 조용히 사라진다는 뜻이다.
    /// 운영 로그에서 매일 5~28줄이 그 모양이었다.
    #[test]
    fn concurrent_writes_never_merge_two_records_onto_one_line() {
        let dir = std::env::temp_dir().join(format!("mc-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let service = std::sync::Arc::new(LogService::new(dir.clone()));

        // 줄 길이를 섞는다. 길이가 다양해야 쪼개지는 경우가 드러난다.
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let service = service.clone();
                std::thread::spawn(move || {
                    for i in 0..200 {
                        let body = "가".repeat(1 + (i % 50));
                        service.info("Test", &format!("{t}-{i} {body}"));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }

        let text = std::fs::read_to_string(service.today_path()).unwrap();
        let mut seen = 0usize;
        for (index, line) in text.lines().enumerate() {
            if line.is_empty() {
                panic!("{index}번째 줄이 비었다 — 개행이 본문과 따로 나갔다는 뜻이다");
            }
            serde_json::from_str::<LogEntry>(line).unwrap_or_else(|_| {
                panic!("{index}번째 줄이 레코드 하나가 아니다: {}", &line[..line.len().min(160)])
            });
            seen += 1;
        }
        assert_eq!(seen, 8 * 200, "쓴 만큼 줄이 남아야 한다");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
