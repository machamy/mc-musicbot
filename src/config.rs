//! C# BotHostSettings / RuntimeHostConfiguration 와 호환되는 설정 로더.
//! 같은 botsettings.json / musicbot.runtime.json 을 찾아 읽어, C# 포터블의
//! 데이터 루트(.musicbot-data)를 그대로 공유한다 (드롭인 마이그레이션 핵심).

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotSettingsFile {
    pub token: String,
    pub register_guild_id: Option<u64>,
    /// C# botsettings.json stores these without the "Override" suffix.
    pub bot_owner_user_id: Option<u64>,
    pub data_root: Option<String>,
    pub tools_root: Option<String>,
    pub yt_dlp_path: Option<String>,
    pub ffmpeg_path: Option<String>,
    /// Older alternate field names are still accepted for local experiments.
    pub bot_owner_user_id_override: Option<u64>,
    pub data_root_override: Option<String>,
    pub tools_root_override: Option<String>,
    pub yt_dlp_path_override: Option<String>,
    pub ffmpeg_path_override: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimeFile {
    pub bot_owner_user_id: Option<u64>,
    pub data_root: Option<String>,
    pub tools_root: Option<String>,
    pub yt_dlp_path: Option<String>,
    pub ffmpeg_path: Option<String>,
}

/// 해석 완료된 런타임 구성.
#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub register_guild_id: Option<u64>,
    pub bot_owner_user_id: u64,
    pub data_root: PathBuf,
    pub tools_root: PathBuf,
    pub yt_dlp_path: String,
    pub ffmpeg_path: String,
    /// 설정 파일이 발견된 디렉터리 (상대 경로 해석 기준).
    pub config_dir: PathBuf,
    /// 포터블 루트 추정치 (BUILD_ID.txt / assets 탐색용).
    pub portable_root: PathBuf,
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// botsettings.json 후보 경로: exe 폴더 → exe/../bot (C# 포터블의 봇 폴더) → exe 상위 → cwd.
fn candidate_paths(file_name: &str) -> Vec<PathBuf> {
    let exe = exe_dir();
    let mut v = vec![exe.join(file_name)];
    if let Some(parent) = exe.parent() {
        v.push(parent.join("bot").join(file_name));
        v.push(parent.join(file_name));
    }
    v.push(PathBuf::from(file_name));
    v
}

/// `.env` 를 찾아 환경변수로 올린다. 찾는 자리는 `botsettings.json` 과 같다
/// (exe 폴더 → exe/../bot → exe 상위 → cwd). 파일이 없으면 조용히 넘어간다 — `.env` 는 선택이다.
///
/// **이미 설정된 변수는 덮지 않는다.** `START-MK2.cmd` → `bot\remote.env.cmd` 로 이어지는
/// 기존 우선순위를 지켜야 해서다. 프로세스 환경이 `.env` 파일을 이긴다.
///
/// 읽은 파일 경로를 돌려준다. 호출부가 로그에 남겨 "왜 이 값이 먹었는지"를 추적할 수 있게.
pub fn load_env_file() -> Option<PathBuf> {
    candidate_paths(".env")
        .into_iter()
        .find(|path| path.is_file() && dotenvy::from_path(path).is_ok())
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

fn resolve_rel(base: &Path, value: &str) -> PathBuf {
    let p = Path::new(value);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

impl Config {
    pub fn load() -> Result<Config, String> {
        // 1) botsettings.json 탐색.
        let mut settings_path: Option<PathBuf> = None;
        for cand in candidate_paths("botsettings.json") {
            if cand.is_file() {
                settings_path = Some(cand);
                break;
            }
        }
        let settings_path = settings_path.ok_or_else(|| {
            "botsettings.json 을 찾지 못했습니다. exe 옆 또는 <포터블>/bot/ 에 두세요.".to_string()
        })?;
        let config_dir = settings_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let settings: BotSettingsFile = read_json(&settings_path);
        if settings.token.trim().is_empty() {
            return Err(format!(
                "봇 토큰이 비어 있습니다: {}",
                settings_path.display()
            ));
        }

        // 2) musicbot.runtime.json (있으면) — config_dir 와 그 상위에서 탐색.
        let mut runtime = RuntimeFile::default();
        for cand in [
            config_dir.join("musicbot.runtime.json"),
            config_dir.join("..").join("musicbot.runtime.json"),
        ] {
            if cand.is_file() {
                runtime = read_json(&cand);
                break;
            }
        }

        // 3) 경로 해석 — C# 기본값과 동일: dataRoot 기본 ".musicbot-data" (config 기준 상대).
        let data_root = settings
            .data_root_override
            .or(settings.data_root)
            .or(runtime.data_root)
            .map(|v| resolve_rel(&config_dir, &v))
            .unwrap_or_else(|| config_dir.join(".musicbot-data"));
        let tools_root = settings
            .tools_root_override
            .or(settings.tools_root)
            .or(runtime.tools_root)
            .map(|v| resolve_rel(&config_dir, &v))
            .unwrap_or_else(|| {
                // C# 포터블: <portable>/tools
                config_dir
                    .parent()
                    .map(|p| p.join("tools"))
                    .unwrap_or_else(|| data_root.join("tools"))
            });

        let portable_root = config_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| config_dir.clone());

        let yt_dlp_path = settings
            .yt_dlp_path_override
            .or(settings.yt_dlp_path)
            .or(runtime.yt_dlp_path)
            .map(|v| resolve_rel(&config_dir, &v).to_string_lossy().to_string())
            .unwrap_or_else(|| locate_tool(&tools_root, "yt-dlp.exe"));
        let ffmpeg_path = settings
            .ffmpeg_path_override
            .or(settings.ffmpeg_path)
            .or(runtime.ffmpeg_path)
            .map(|v| resolve_rel(&config_dir, &v).to_string_lossy().to_string())
            .unwrap_or_else(|| locate_tool(&tools_root, "ffmpeg.exe"));

        std::fs::create_dir_all(&data_root).map_err(|e| format!("dataRoot 생성 실패: {e}"))?;

        Ok(Config {
            token: settings.token,
            register_guild_id: settings.register_guild_id,
            bot_owner_user_id: settings
                .bot_owner_user_id_override
                .or(settings.bot_owner_user_id)
                .or(runtime.bot_owner_user_id)
                .unwrap_or(0),
            data_root,
            tools_root,
            yt_dlp_path,
            ffmpeg_path,
            config_dir,
            portable_root,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_root.join("musicbot.sqlite")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.data_root.join("cache")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_root.join("logs")
    }
}

/// tools 폴더 → PATH 순서로 도구를 찾는다 (C# ToolLocator 와 같은 우선순위).
fn locate_tool(tools_root: &Path, name: &str) -> String {
    let local = tools_root.join(name);
    if local.is_file() {
        return local.to_string_lossy().to_string();
    }
    // PATH fallback: 이름만 돌려주면 프로세스 spawn 시 PATH 탐색됨.
    name.trim_end_matches(".exe").to_string()
}
