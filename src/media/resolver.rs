//! 입력 URL → TrackRef/CollectionRef 정규화. C# MediaResolver 의 규칙을 그대로 포팅.
//! 공급자별 URL 변형 흡수가 캐시 키 안정성과 직결된다.

use crate::models::ProviderKind;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
pub struct ResolvedTrack {
    pub provider: ProviderKind,
    pub content_id: String,
    pub source_url: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedCollection {
    pub provider: ProviderKind,
    pub collection_id: String,
    pub source_url: String,
}

#[derive(Debug, Clone)]
pub enum Resolved {
    Track(ResolvedTrack),
    Collection(ResolvedCollection),
}

static SC_SLUG_CLEAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9/_-]").unwrap());

fn is_youtube_host(host: &str) -> bool {
    host == "youtube.com" || host.ends_with(".youtube.com")
}
fn is_soundcloud_host(host: &str) -> bool {
    host == "soundcloud.com" || host.ends_with(".soundcloud.com")
}

fn query_param(url: &url_lite::Url, key: &str) -> Option<String> {
    url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// 표준 url crate 없이 가벼운 파서 — http(s) 절대 URL 만 다룬다.
pub mod url_lite {
    #[derive(Debug, Clone)]
    pub struct Url {
        pub scheme: String,
        pub host: String,
        pub path: String,
        pub query: String,
        pub raw: String,
    }
    impl Url {
        pub fn parse(input: &str) -> Option<Url> {
            let input = input.trim();
            let (scheme, rest) = input.split_once("://")?;
            let scheme = scheme.to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return None;
            }
            let (authority, path_query) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, "/"),
            };
            let host = authority
                .split('@')
                .last()?
                .split(':')
                .next()?
                .to_ascii_lowercase();
            if host.is_empty() {
                return None;
            }
            let (path, query) = match path_query.split_once('?') {
                Some((p, q)) => (p.to_string(), q.split('#').next().unwrap_or("").to_string()),
                None => (
                    path_query.split('#').next().unwrap_or("/").to_string(),
                    String::new(),
                ),
            };
            Some(Url {
                scheme,
                host,
                path,
                query,
                raw: input.to_string(),
            })
        }
        pub fn query_pairs(&self) -> impl Iterator<Item = (String, String)> + '_ {
            self.query
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (pair.to_string(), String::new()),
                })
        }
    }
}

pub fn can_resolve(input: &str) -> bool {
    match url_lite::Url::parse(input) {
        Some(u) => u.host == "youtu.be" || is_youtube_host(&u.host) || is_soundcloud_host(&u.host),
        None => false,
    }
}

pub fn resolve(input: &str) -> Result<Resolved, String> {
    let url = url_lite::Url::parse(input).ok_or("절대 http(s) URL 이 아닙니다.")?;
    if url.host == "youtu.be" {
        let id = url.path.trim_matches('/').to_string();
        if id.is_empty() {
            return Err("youtu.be 링크에서 영상 ID 를 찾지 못했습니다.".into());
        }
        return Ok(Resolved::Track(ResolvedTrack {
            provider: ProviderKind::YouTube,
            content_id: id.clone(),
            source_url: format!("https://www.youtube.com/watch?v={id}"),
        }));
    }
    if is_youtube_host(&url.host) {
        let is_music = url.host == "music.youtube.com";
        let provider = if is_music {
            ProviderKind::YouTubeMusic
        } else {
            ProviderKind::YouTube
        };
        // playlist 우선: watch?v=..&list=.. 는 트랙으로 (RD 믹스 제외하고 list 페이지만 컬렉션).
        let v = query_param(&url, "v");
        let list = query_param(&url, "list");
        if url.path.starts_with("/playlist") {
            let list = list.ok_or("playlist URL 에 list 파라미터가 없습니다.")?;
            return Ok(Resolved::Collection(ResolvedCollection {
                provider,
                collection_id: list.clone(),
                source_url: url.raw.clone(),
            }));
        }
        if let Some(id) = v {
            let base = if is_music {
                "https://music.youtube.com/watch?v="
            } else {
                "https://www.youtube.com/watch?v="
            };
            return Ok(Resolved::Track(ResolvedTrack {
                provider,
                content_id: id.clone(),
                source_url: format!("{base}{id}"),
            }));
        }
        if url.path.starts_with("/shorts/") || url.path.starts_with("/embed/") {
            let id = url
                .path
                .trim_start_matches("/shorts/")
                .trim_start_matches("/embed/")
                .split('/')
                .next()
                .unwrap_or("")
                .to_string();
            if !id.is_empty() {
                return Ok(Resolved::Track(ResolvedTrack {
                    provider: ProviderKind::YouTube,
                    content_id: id.clone(),
                    source_url: format!("https://www.youtube.com/watch?v={id}"),
                }));
            }
        }
        return Err("지원하지 않는 YouTube URL 형식입니다.".into());
    }
    if is_soundcloud_host(&url.host) {
        let slug = SC_SLUG_CLEAN
            .replace_all(url.path.trim_matches('/'), "")
            .to_lowercase();
        if slug.is_empty() {
            return Err("SoundCloud URL 에서 트랙 경로를 찾지 못했습니다.".into());
        }
        let parts: Vec<&str> = slug.split('/').filter(|s| !s.is_empty()).collect();
        let source_url = format!("https://soundcloud.com/{}", url.path.trim_matches('/'));
        if parts.len() >= 3 && parts[1] == "sets" {
            return Ok(Resolved::Collection(ResolvedCollection {
                provider: ProviderKind::SoundCloud,
                collection_id: slug.clone(),
                source_url,
            }));
        }
        return Ok(Resolved::Track(ResolvedTrack {
            provider: ProviderKind::SoundCloud,
            // C# 과 동일하게 슬래시 보존 — cache_key 호환 (파일명 정제는 cache.rs 몫).
            content_id: slug.clone(),
            source_url,
        }));
    }
    Err(format!("지원하지 않는 공급자입니다: {}", url.host))
}
