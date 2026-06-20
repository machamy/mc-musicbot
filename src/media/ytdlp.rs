//! yt-dlp 연동: 검색/메타조회/컬렉션 펼치기/라디오 후보/다운로드.
//! C# ExternalMediaTools + AudioPreparationService 다운로드부 포팅.
//! 다운로드 인자는 실청취 검증된 조합(bestaudio → libopus 재인코딩, output_gain=0 보장).

use crate::models::{CsTimeSpan, ProviderKind, TrackRef};
use serde_json::Value;
use std::process::Stdio;
use tokio::process::Command;

#[derive(Clone)]
pub struct YtDlp {
    pub exe: String,
    pub browser_profile: String,
    pub cookie_file: Option<String>,
}

pub enum AuthMode {
    BrowserProfile,
    CookieFile,
    Public,
}

impl AuthMode {
    pub fn describe(&self) -> &'static str {
        match self {
            AuthMode::BrowserProfile => "browser-profile",
            AuthMode::CookieFile => "cookie-file",
            AuthMode::Public => "public",
        }
    }
}

impl YtDlp {
    /// C# YtDlpAuthArguments.Build 과 동일: 프로필에 ':' 가 있으면 그대로,
    /// 없으면 edge → chrome 순서로 둘 다 시도. 그 다음 쿠키 파일, 마지막은 공개 접근.
    fn auth_chain(&self) -> Vec<(AuthMode, Vec<String>)> {
        let mut chain = Vec::new();
        let profile = self.browser_profile.trim();
        if !profile.is_empty() {
            if profile.contains(':') {
                chain.push((
                    AuthMode::BrowserProfile,
                    vec!["--cookies-from-browser".into(), profile.to_string()],
                ));
            } else {
                chain.push((
                    AuthMode::BrowserProfile,
                    vec!["--cookies-from-browser".into(), format!("edge:{profile}")],
                ));
                chain.push((
                    AuthMode::BrowserProfile,
                    vec!["--cookies-from-browser".into(), format!("chrome:{profile}")],
                ));
            }
        }
        if let Some(file) = &self.cookie_file {
            if !file.trim().is_empty() {
                chain.push((AuthMode::CookieFile, vec!["--cookies".into(), file.clone()]));
            }
        }
        chain.push((AuthMode::Public, Vec::new()));
        chain
    }

    /// 메타 조회 1회 실행. 30초 타임아웃 — 초과 시 future drop 으로 프로세스가 kill 된다.
    async fn run_json_once(&self, args: &[String]) -> Option<Value> {
        let fut = Command::new(&self.exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(std::time::Duration::from_secs(30), fut)
            .await
            .ok()?
            .ok()?;
        if !out.status.success() {
            return None;
        }
        serde_json::from_slice(&out.stdout).ok()
    }

    /// 메타 조회 — C# 과 동일하게 검색/조회도 인증 체인을 차례로 시도한다
    /// (유튜브 봇 차단 시 로그인 쿠키로 우회, 쿠키 만료 시 공개 접근 폴백).
    async fn run_json(&self, args: &[String]) -> Option<Value> {
        for (_mode, auth_args) in self.auth_chain() {
            let mut full: Vec<String> = auth_args;
            full.extend(args.iter().cloned());
            if let Some(v) = self.run_json_once(&full).await {
                return Some(v);
            }
        }
        None
    }

    fn entry_to_track(entry: &Value, provider: ProviderKind) -> Option<TrackRef> {
        let id = entry.get("id")?.as_str()?.to_string();
        let title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let artist = entry
            .get("artist")
            .or_else(|| entry.get("uploader"))
            .or_else(|| entry.get("channel"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let duration = entry
            .get("duration")
            .and_then(|v| v.as_f64())
            .filter(|d| *d > 0.0)
            .map(CsTimeSpan::from_secs_f64);
        let source_url = entry
            .get("webpage_url")
            .or_else(|| entry.get("url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| match provider {
                ProviderKind::SoundCloud => format!("https://soundcloud.com/{id}"),
                ProviderKind::YouTubeMusic => format!("https://music.youtube.com/watch?v={id}"),
                _ => format!("https://www.youtube.com/watch?v={id}"),
            });
        Some(TrackRef {
            provider,
            content_id: id,
            source_url,
            title,
            artist,
            duration,
            variant_key: None,
        })
    }

    /// ytsearchN — 유튜브 키워드 검색 (기존 호출부 호환용 기본 진입점).
    pub async fn search(&self, query: &str, count: usize) -> Vec<TrackRef> {
        self.search_provider(query, count, ProviderKind::YouTube)
            .await
    }

    /// 공급자별 키워드 검색 — YouTube=ytsearchN, SoundCloud=scsearchN.
    /// flat-playlist 라서 duration/artist 가 빠질 수 있으나 후보 나열엔 충분하다.
    pub async fn search_provider(
        &self,
        query: &str,
        count: usize,
        provider: ProviderKind,
    ) -> Vec<TrackRef> {
        let prefix = match provider {
            ProviderKind::SoundCloud => "scsearch",
            // YouTubeMusic 검색도 일반 ytsearch 로 — 결과 영상 ID 네임스페이스가 동일.
            _ => "ytsearch",
        };
        let target = format!("{prefix}{count}:{query}");
        let args: Vec<String> = vec![
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
            "--".into(),
            target,
        ];
        let Some(json) = self.run_json(&args).await else {
            return Vec::new();
        };
        json.get("entries")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| Self::entry_to_track(e, provider))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 단일 트랙 메타 조회.
    pub async fn inspect_track(&self, url: &str, provider: ProviderKind) -> Option<TrackRef> {
        let args: Vec<String> = vec![
            "--no-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
            "--".into(),
            url.to_string(),
        ];
        let json = self.run_json(&args).await?;
        Self::entry_to_track(&json, provider)
    }

    /// 플레이리스트/세트 펼치기.
    pub async fn expand_collection(&self, url: &str, provider: ProviderKind) -> Vec<TrackRef> {
        let args: Vec<String> = vec![
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            "--no-warnings".into(),
            "--".into(),
            url.to_string(),
        ];
        let Some(json) = self.run_json(&args).await else {
            return Vec::new();
        };
        json.get("entries")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| Self::entry_to_track(e, provider))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 자동추천 라디오/스테이션 후보 (C# BuildStationUrl 과 동일 URL 규칙).
    pub async fn station_candidates(&self, seed: &TrackRef) -> Vec<TrackRef> {
        let url = match seed.provider {
            ProviderKind::YouTube => {
                format!(
                    "https://www.youtube.com/watch?v={0}&list=RD{0}",
                    seed.content_id
                )
            }
            ProviderKind::YouTubeMusic => {
                format!(
                    "https://music.youtube.com/watch?v={0}&list=RDAMVM{0}",
                    seed.content_id
                )
            }
            ProviderKind::SoundCloud => {
                format!("{}/recommended", seed.source_url.trim_end_matches('/'))
            }
        };
        self.expand_collection(&url, seed.provider).await
    }

    /// 곡 다운로드 — 인증 fallback 체인을 따라 시도, 성공 시 실제 파일 경로 반환.
    pub async fn download(
        &self,
        source_url: &str,
        output_template: &str,
        remove_segments: bool,
    ) -> Result<(String, &'static str), String> {
        let mut last_err = String::new();
        for (mode, auth_args) in self.auth_chain() {
            let mut args: Vec<String> = vec![
                "--no-playlist".into(),
                "-f".into(),
                "bestaudio".into(),
                "-x".into(),
                "--audio-format".into(),
                "opus".into(),
                "--audio-quality".into(),
                "128K".into(),
                "--newline".into(),
                "--print".into(),
                "after_move:filepath".into(),
            ];
            // SponsorBlock: 인트로/아웃트로/비음악 구간 컷 (해당 데이터가 있는 영상에만).
            if remove_segments {
                args.push("--sponsorblock-remove".into());
                args.push("music_offtopic,intro,outro".into());
            }
            args.extend(auth_args);
            args.push("-o".into());
            args.push(output_template.to_string());
            args.push("--".into());
            args.push(source_url.to_string());

            let fut = Command::new(&self.exe)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output();
            // 다운로드는 10분 한도 — 행 걸린 yt-dlp 가 재생 명령을 영원히 잡아두지 않게.
            let out = match tokio::time::timeout(std::time::Duration::from_secs(600), fut).await {
                Ok(r) => r.map_err(|e| format!("yt-dlp 실행 실패: {e}"))?,
                Err(_) => {
                    last_err = "yt-dlp 다운로드가 10분을 초과해 중단했습니다.".into();
                    continue;
                }
            };

            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(path) = stdout
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .rev()
                    .find(|l| std::path::Path::new(l).is_file())
                {
                    return Ok((path.to_string(), mode.describe()));
                }
                last_err = "yt-dlp 가 출력 파일 경로를 알려주지 않았습니다.".into();
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
                last_err = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
            }
        }
        Err(last_err)
    }
}
