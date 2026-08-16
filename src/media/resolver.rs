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

/* `https://` 를 빼고 붙여 넣는 사람이 많다.
 *
 * 브라우저 주소창은 `youtu.be/...` 로 보여 주고, 그걸 그대로 복사하는 게 자연스럽다.
 * 그런데 그러면 링크가 아니라 **검색어**로 취급돼서 엉뚱한 결과가 나왔다. 사람 눈에는
 * "링크를 못 알아본다" 로 보인다.
 *
 * **아무 글자에나 `https://` 를 붙이면 안 된다.** 그러면 평범한 검색어까지 주소로
 * 오해한다. 그래서 **우리가 아는 호스트로 시작하고 공백이 없을 때만** 붙인다.
 */
fn with_scheme(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.contains("://") || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    const HOSTS: [&str; 6] = [
        "youtu.be/",
        "youtube.com/",
        "www.youtube.com/",
        "m.youtube.com/",
        "music.youtube.com/",
        "soundcloud.com/",
    ];
    let lower = trimmed.to_ascii_lowercase();
    HOSTS
        .iter()
        .any(|host| lower.starts_with(host))
        .then(|| format!("https://{trimmed}"))
}

/// 붙여 넣은 그대로도, `https://` 를 뺀 것도 같은 주소로 본다.
fn parse_lenient(input: &str) -> Option<url_lite::Url> {
    url_lite::Url::parse(input)
        .or_else(|| with_scheme(input).and_then(|full| url_lite::Url::parse(&full)))
}

pub fn can_resolve(input: &str) -> bool {
    match parse_lenient(input) {
        Some(u) => u.host == "youtu.be" || is_youtube_host(&u.host) || is_soundcloud_host(&u.host),
        None => false,
    }
}

pub fn resolve(input: &str) -> Result<Resolved, String> {
    let url = parse_lenient(input).ok_or("절대 http(s) URL 이 아닙니다.")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track(input: &str) -> ResolvedTrack {
        match resolve(input).unwrap_or_else(|e| panic!("{input} → {e}")) {
            Resolved::Track(t) => t,
            other => panic!("{input} → 트랙이 아님: {other:?}"),
        }
    }

    /// **공유 버튼이 주는 주소가 기본형이다.** 유튜브 앱·웹의 "공유" 는 `youtu.be/ID?si=...`
    /// 를 준다. 사람은 그걸 그대로 붙여 넣으므로 이 꼴이 안 되면 사실상 안 되는 것이다.
    #[test]
    fn a_shared_youtu_be_link_resolves() {
        let t = track("https://youtu.be/YXIz7U42pgk?si=cL2kCHU1unBrHNbI");
        assert_eq!(t.content_id, "YXIz7U42pgk");
        assert_eq!(t.provider, ProviderKind::YouTube);
        assert_eq!(t.source_url, "https://www.youtube.com/watch?v=YXIz7U42pgk");
    }

    /// 사람이 실제로 붙여 넣는 꼴들. **하나라도 빠지면 "링크가 안 먹힌다" 가 된다.**
    #[test]
    fn the_shapes_people_actually_paste() {
        for input in [
            "https://youtu.be/YXIz7U42pgk?si=cL2kCHU1unBrHNbI",
            "https://www.youtube.com/watch?v=YXIz7U42pgk&t=42s",
            "https://m.youtube.com/watch?v=YXIz7U42pgk",
            "https://music.youtube.com/watch?v=YXIz7U42pgk",
            "  https://youtu.be/YXIz7U42pgk  ",
            "youtu.be/YXIz7U42pgk?si=cL2kCHU1unBrHNbI",
            "www.youtube.com/watch?v=YXIz7U42pgk",
        ] {
            assert!(can_resolve(input), "이 꼴을 링크로 못 알아봐요: {input:?}");
            assert_eq!(track(input).content_id, "YXIz7U42pgk", "입력: {input:?}");
        }
    }

    /// **평범한 검색어를 주소로 오해하면 안 된다.** 그게 더 큰 사고다 —
    /// 검색이 통째로 안 되는 것이니까.
    #[test]
    fn ordinary_searches_are_not_urls() {
        for input in [
            "아이유 밤편지",
            "newjeans ditto",
            "youtube 인기곡",           // 호스트처럼 생겼지만 슬래시가 없다
            "youtu.be 링크 주세요",      // 공백이 있으면 검색어다
            "노래방 youtube.com 추천",
            "",
        ] {
            assert!(!can_resolve(input), "검색어를 주소로 봤어요: {input:?}");
        }
    }

    #[test]
    fn plain_forms_still_resolve() {
        assert_eq!(track("https://youtu.be/YXIz7U42pgk").content_id, "YXIz7U42pgk");
        assert_eq!(
            track("https://www.youtube.com/watch?v=YXIz7U42pgk").content_id,
            "YXIz7U42pgk"
        );
        assert!(can_resolve("https://youtu.be/YXIz7U42pgk?si=abc"));
    }
}
