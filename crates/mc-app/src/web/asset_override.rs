//! 자산 덮어쓰기 — **봇을 안 끄고** 리모컨 화면만 갈아 끼우는 통로 (§43).
//!
//! ## 왜 있나
//!
//! 최근 33커밋 중 6개(17%)가 JS·CSS 만 고친 것이었다. 그런데 배포 단위가 exe 하나라
//! 그 여섯 번도 전부 봇을 껐다 켜야 했고, 그때마다 듣고 있던 사람의 노래가 끊겼다.
//!
//! 프로세스를 넷으로 쪼개는 큰 공사(PLAN-04)가 없애려던 중단의 상당 부분이 이것인데,
//! **이 파일 하나로 그 몫만 먼저 없앤다.** 자산은 상태가 없고 요청마다 새로 읽히므로,
//! 프로세스 경계를 나눌 필요 없이 "지금 내보낼 바이트" 만 바꿔치면 끝난다.
//!
//! ## 규칙 — 새 경로를 만들지 못한다
//!
//! **덮어쓸 수 있는 이름은 이미 있는 자산의 이름뿐이다.** 호출부(`assets.rs`)가
//! 화이트리스트(`mc-assets::get`)로 이름을 먼저 해석하고, 그 이름으로만 여기에 묻는다.
//! 그래서 이 폴더에 `secret.txt` 를 넣어도 밖으로 나가지 않는다 —
//! "파일 서버가 아니라 **자산 교체**" 라는 것이 이 설계의 핵심이다.
//!
//! 경로 조작도 구조적으로 막힌다. 이름을 이어 붙여 여는 것이 아니라 폴더를 한 번 훑어
//! **이름이 정확히 일치하는 것만** 표에 담는다. `..` 도 하위 폴더도 표에 못 들어간다.
//!
//! ## 왜 감시가 아니라 폴링인가
//!
//! `SHUTDOWN.request` 와 같은 이유다(`shutdown.rs`). 파일 감시 API 는 플랫폼마다 다르게
//! 놓치고, 편집기가 "임시 파일에 쓰고 이름 바꾸기" 를 하면 이벤트가 두세 번 튄다.
//! 2초에 한 번 폴더 목록만 보는 편이 훨씬 예측 가능하고, 자산 배포는 초 단위로 급하지 않다.
//!
//! **읽기는 지문이 바뀔 때만 한다.** 지문은 `(이름, 크기, 수정시각)` 이라 폴더가 그대로면
//! 디스크를 안 건드린다. 평소(폴더 없음)에는 실패하는 `read_dir` 한 번이 전부다.
//!
//! ## 왜 전역 하나가 아니라 [`Overrides`] 인가
//!
//! 상태(현재 한 벌 + 마지막 지문)를 값에 담아 두면 테스트가 **자기 것만** 쓴다.
//! 전역 하나에 얹으면 같은 프로세스에서 병렬로 도는 테스트끼리 서로의 한 벌을 덮어써서,
//! 혼자 돌리면 초록이고 다 같이 돌리면 빨간 테스트가 된다. 운영에서 쓰는 전역은
//! [`global`] 하나뿐이다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// 자산을 덮어쓸 폴더 이름. 데이터 루트 밑에 둔다.
pub const OVERRIDE_DIR: &str = "assets-override";

/// 폴더를 다시 보는 주기.
const POLL: Duration = Duration::from_secs(2);

/// 덮어쓴 파일 하나의 최대 크기. 제일 큰 자산(`portal.js`)이 400KB 대라 넉넉하다.
///
/// **상한이 필요한 이유**: 이 바이트는 통째로 메모리에 올라가고 요청마다 복사된다.
/// 실수로 동영상을 떨어뜨려도 서버가 죽지 않아야 한다.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 폴더 지문 한 줄 — `(이름, 크기, 수정시각 ms)`.
type Print = (String, u64, Option<u64>);

/// 지금 내보낼 덮어쓰기 한 벌.
#[derive(Default, Debug)]
pub struct Snapshot {
    files: BTreeMap<String, Arc<Vec<u8>>>,
    /// 이 한 벌의 짧은 지문. 비어 있으면 덮어쓰기가 없다는 뜻이다.
    tag: String,
}

impl Snapshot {
    pub fn get(&self, name: &str) -> Option<Arc<Vec<u8>>> {
        self.files.get(name).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// 덮어쓰기까지 반영한 자산 버전.
    ///
    /// **내장 버전만 쓰면 안 된다.** 페이지 셸이 `?v=` 에 이 값을 넣고, 그 값이 그대로면
    /// 브라우저가 1년 immutable 로 캐시해 둔 옛 파일을 계속 쓴다 — 파일을 바꿔도 화면이
    /// 안 바뀐다. 그래서 덮어쓴 내용까지 섞어 새 값을 만든다.
    ///
    /// 내장 버전을 **접두사로 남겨 두는** 이유: 새 exe 로 올라가면 덮어쓰기가 그대로여도
    /// 버전이 바뀌어야 한다. 둘 중 하나만 바뀌어도 반드시 값이 달라진다.
    pub fn version_for(&self, builtin: &str) -> String {
        if self.tag.is_empty() {
            builtin.to_string()
        } else {
            format!("{builtin}-{}", self.tag)
        }
    }
}

/// 덮어쓰기 한 벌과 그것을 갱신하는 상태.
pub struct Overrides {
    current: RwLock<Arc<Snapshot>>,
    last: RwLock<Option<Vec<Print>>>,
}

impl Default for Overrides {
    fn default() -> Self {
        Self {
            current: RwLock::new(Arc::new(Snapshot::default())),
            last: RwLock::new(None),
        }
    }
}

impl Overrides {
    /// 지금 한 벌. 폴더가 없으면 빈 것이라 비용이 사실상 0이다.
    pub fn current(&self) -> Arc<Snapshot> {
        self.current
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 한 번 훑어 바뀐 것이 있으면 갈아 끼운다. 바뀌었을 때만 사람이 읽을 설명을 준다.
    ///
    /// `allowed` 는 "이 이름이 우리 자산인가" 를 묻는다 — 화이트리스트를 이 모듈이 따로
    /// 들고 있지 않게 하려는 것이다. 같은 목록이 두 곳에 있으면 반드시 어긋난다.
    pub fn refresh_once(&self, dir: &Path, allowed: &dyn Fn(&str) -> bool) -> Option<String> {
        let print = fingerprint(dir, allowed);
        if self.last.read().ok()?.as_deref() == Some(print.as_slice()) {
            return None;
        }

        let (snapshot, skipped) = load(dir, &print);
        let count = snapshot.files.len();
        let names: Vec<&str> = snapshot.files.keys().map(String::as_str).collect();
        let tag = snapshot.tag.clone();

        let mut message = if count == 0 {
            "자산 덮어쓰기가 없어요. 내장 자산으로 갑니다.".to_string()
        } else {
            format!("자산 {count}개를 덮어썼어요 [{tag}]: {}", names.join(", "))
        };
        if !skipped.is_empty() {
            message.push_str(&format!(" · 건너뜀: {}", skipped.join(", ")));
        }

        if let Ok(mut slot) = self.current.write() {
            *slot = Arc::new(snapshot);
        }
        if let Ok(mut slot) = self.last.write() {
            *slot = Some(print);
        }
        Some(message)
    }
}

/// 운영에서 쓰는 단 하나.
pub fn global() -> &'static Overrides {
    static GLOBAL: OnceLock<Overrides> = OnceLock::new();
    GLOBAL.get_or_init(Overrides::default)
}

/// 폴더 지문. 이게 그대로면 파일을 안 읽는다.
fn fingerprint(dir: &Path, allowed: &dyn Fn(&str) -> bool) -> Vec<Print> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        // **하위 폴더는 안 본다.** 이름이 곧 자산 이름이라 깊이가 있을 이유가 없다.
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !allowed(&name) {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|span| span.as_millis() as u64);
        out.push((name, meta.len(), mtime));
    }
    out.sort();
    out
}

/// 지문에 든 파일을 실제로 읽어 한 벌을 만든다.
fn load(dir: &Path, print: &[Print]) -> (Snapshot, Vec<String>) {
    let mut files = BTreeMap::new();
    let mut skipped = Vec::new();
    for (name, len, _) in print {
        if *len > MAX_BYTES {
            skipped.push(format!("{name} (너무 큼, {len} 바이트)"));
            continue;
        }
        match std::fs::read(dir.join(name)) {
            Ok(bytes) => {
                files.insert(name.clone(), Arc::new(bytes));
            }
            // 읽다 실패하면 그 파일만 건너뛴다. 편집기가 저장하는 중일 수 있고,
            // 그러면 다음 폴링에서 지문이 또 바뀌어 자연히 다시 읽는다.
            Err(error) => skipped.push(format!("{name} ({error})")),
        }
    }
    let tag = tag_of(&files);
    (Snapshot { files, tag }, skipped)
}

/// 한 벌의 짧은 지문. 내용이 바뀌면 반드시 바뀐다.
fn tag_of(files: &BTreeMap<String, Arc<Vec<u8>>>) -> String {
    if files.is_empty() {
        return String::new();
    }
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for (name, bytes) in files {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update(bytes.as_slice());
    }
    hasher
        .finalize()
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 폴링 태스크를 띄운다. 기동에서 한 번 부른다.
///
/// 폴더가 없는 것이 정상이다 — 그때는 매번 `read_dir` 하나가 실패하고 끝난다.
pub fn spawn_watcher(
    data_root: PathBuf,
    log: Arc<crate::logging::LogService>,
    allowed: fn(&str) -> bool,
) {
    let dir = data_root.join(OVERRIDE_DIR);
    tokio::spawn(async move {
        // 기동 직후 한 번은 즉시 본다. 껐다 켠 뒤에도 덮어쓰기가 살아 있어야 한다.
        // 첫 훑기는 폴더가 없어도 한 번 말한다(= 덮어쓰기 없음). 그 뒤로는 바뀔 때만.
        loop {
            if let Some(message) = global().refresh_once(&dir, &allowed) {
                log.info("Assets", &message);
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    //! 여기서 지켜야 할 것은 넷이다.
    //!   1. **폴더가 없을 때 아무 일도 안 난다** — 운영의 평소 상태다.
    //!   2. **화이트리스트 밖 이름은 절대 안 담긴다** — 이게 뚫리면 파일 서버가 된다.
    //!   3. **내용이 바뀌면 버전이 반드시 바뀐다** — 안 그러면 바꿔도 화면이 그대로다.
    //!   4. **하나가 이상해도 나머지는 산다** — 자산 하나 때문에 화면이 통째로 죽으면 안 된다.

    use super::*;

    fn allowed(name: &str) -> bool {
        matches!(name, "portal.js" | "portal.css" | "core.js" | "sw.js")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "macham-override-{tag}-{}",
            crate::models::uuid_like()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_folder_changes_nothing_and_says_so_once() {
        let overrides = Overrides::default();
        let dir = std::env::temp_dir().join(format!("macham-absent-{}", crate::models::uuid_like()));
        assert!(!dir.exists());

        // 첫 훑기는 "덮어쓰기 없음" 을 한 번 알리고, 그 뒤로는 조용하다.
        assert!(overrides.refresh_once(&dir, &allowed).is_some());
        assert!(
            overrides.refresh_once(&dir, &allowed).is_none(),
            "폴더가 없는데 계속 바뀌었다고 한다 — 로그가 2초마다 도배된다"
        );
        assert!(overrides.current().is_empty());
        assert_eq!(
            overrides.current().version_for("abc"),
            "abc",
            "덮어쓰기가 없으면 내장 버전 그대로여야 한다"
        );
    }

    /// **화이트리스트 밖 파일은 표에 못 들어간다.** 이게 이 모듈의 안전 계약 전부다.
    #[test]
    fn only_known_asset_names_are_picked_up() {
        let overrides = Overrides::default();
        let dir = temp_dir("whitelist");
        std::fs::write(dir.join("portal.css"), b"body{}").unwrap();
        std::fs::write(dir.join("secret.txt"), b"password").unwrap();
        std::fs::write(dir.join(".env"), b"TOKEN=1").unwrap();
        std::fs::write(dir.join("portal.js.map"), b"{}").unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested").join("core.js"), b"nope").unwrap();

        overrides.refresh_once(&dir, &allowed);
        let snap = overrides.current();
        assert!(snap.get("portal.css").is_some(), "아는 이름이 안 담겼다");
        for name in ["secret.txt", ".env", "portal.js.map", "nested", "core.js"] {
            assert!(snap.get(name).is_none(), "{name} 이 담겼다 — 파일 서버가 됐다");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 내용이 바뀌면 버전이 바뀌고, 그대로면 안 바뀐다.
    /// 버전이 안 바뀌면 브라우저가 옛 파일을 1년 immutable 로 붙잡고 있어 화면이 그대로다.
    #[test]
    fn the_version_moves_with_the_bytes_and_with_the_build() {
        let overrides = Overrides::default();
        let dir = temp_dir("version");
        std::fs::write(dir.join("portal.css"), b"body{color:red}").unwrap();
        overrides.refresh_once(&dir, &allowed);
        let first = overrides.current().version_for("base");
        assert_ne!(first, "base", "덮어썼는데 버전이 그대로다");

        // 같은 내용을 다시 써도(수정시각만 바뀜) 버전은 같아야 한다.
        std::fs::write(dir.join("portal.css"), b"body{color:red}").unwrap();
        overrides.refresh_once(&dir, &allowed);
        assert_eq!(
            overrides.current().version_for("base"),
            first,
            "내용이 같은데 버전이 바뀌었다 — 저장할 때마다 전원이 자산을 다시 받는다"
        );

        std::fs::write(dir.join("portal.css"), b"body{color:blue}").unwrap();
        overrides.refresh_once(&dir, &allowed);
        let second = overrides.current().version_for("base");
        assert_ne!(second, first, "내용이 바뀌었는데 버전이 그대로다");

        // 새 exe 로 올라가면(= 내장 버전이 바뀌면) 덮어쓰기가 같아도 버전이 바뀐다.
        assert_ne!(overrides.current().version_for("other"), second);

        // 파일을 지우면 내장 자산으로 돌아간다.
        std::fs::remove_file(dir.join("portal.css")).unwrap();
        overrides.refresh_once(&dir, &allowed);
        assert!(overrides.current().is_empty());
        assert_eq!(overrides.current().version_for("base"), "base");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 너무 큰 파일은 건너뛰고 **나머지는 그대로 산다.**
    #[test]
    fn an_oversized_file_is_skipped_without_taking_the_rest_down() {
        let overrides = Overrides::default();
        let dir = temp_dir("toobig");
        std::fs::write(dir.join("portal.css"), b"ok").unwrap();
        std::fs::write(dir.join("portal.js"), vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();

        let message = overrides
            .refresh_once(&dir, &allowed)
            .expect("바뀌었다고 해야 한다");
        assert!(message.contains("건너뜀"), "건너뛴 사실을 안 알린다: {message}");
        let snap = overrides.current();
        assert!(snap.get("portal.css").is_some(), "멀쩡한 파일까지 날아갔다");
        assert!(snap.get("portal.js").is_none(), "너무 큰 파일이 실렸다");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 덮어쓰기 두 벌이 **같은 파일 집합이라도 이름이 다르면** 버전이 달라야 한다.
    /// 이름을 안 섞으면 `a=X, b=Y` 와 `a=Y, b=X` 가 같은 지문을 받는다.
    #[test]
    fn the_tag_mixes_the_name_not_only_the_bytes() {
        let overrides = Overrides::default();
        let dir = temp_dir("names");
        std::fs::write(dir.join("portal.css"), b"AAA").unwrap();
        std::fs::write(dir.join("core.js"), b"BBB").unwrap();
        overrides.refresh_once(&dir, &allowed);
        let first = overrides.current().version_for("base");

        std::fs::write(dir.join("portal.css"), b"BBB").unwrap();
        std::fs::write(dir.join("core.js"), b"AAA").unwrap();
        overrides.refresh_once(&dir, &allowed);
        assert_ne!(
            overrides.current().version_for("base"),
            first,
            "내용을 맞바꿨는데 버전이 같다 — 이름이 지문에 안 섞였다"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
