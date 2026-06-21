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
}

const RING_CAP: usize = 2000;

impl LogService {
    pub fn new(dir: PathBuf) -> LogService {
        let _ = std::fs::create_dir_all(&dir);
        let svc = LogService {
            dir,
            ring: Mutex::new(VecDeque::with_capacity(RING_CAP)),
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
        if let Ok(line) = serde_json::to_string(&entry) {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.today_path())
            {
                let _ = writeln!(f, "{line}");
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
