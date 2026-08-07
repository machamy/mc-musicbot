//! TJ 노래방 **공식 차트**를 직접 가져온다 (V3 §15.2c).
//!
//! 전에는 `ytsearch50:TJ노래방 발라드` 처럼 유튜브 검색으로 흉내를 냈다. 그러면
//! 순위가 TJ 순위가 아니라 **유튜브 검색 순위**라 "TJ 발라드 차트"라는 이름이 거짓말이 된다.
//! 지금은 TJ 가 자기 사이트에 쓰는 API 를 그대로 부른다.
//!
//! ```text
//! POST https://www.tjmedia.com/legacy/api/topAndHot100
//!      chartType=TOP|HOT & strType=N & searchStartDate=어제 & searchEndDate=오늘
//! → { "resultCode": "99", "resultMsg": "성공",
//!     "resultData": { "itemsTotalCount": 100,
//!                     "items": [ { "rank": "1", "pro": 52788,
//!                                  "indexTitle": "晩餐歌", "indexSong": "tuki." }, ... ] } }
//! ```
//!
//! **`resultCode` 는 성공일 때 `"99"` 다.** 흔한 규약과 반대라 실패로 오해하기 쉽다.
//!
//! TJ 는 순위와 곡 정보만 주고 **재생할 주소는 주지 않는다.** 그래서 곡마다 유튜브에서
//! 반주 영상을 한 번 찾아 TJ 곡번호에 붙여 저장한다(`remote_tj_tracks`). 곡번호는 TJ 가
//! 영구히 쓰는 값이라 한 번 찾으면 다시 찾지 않는다 — 이게 없으면 차트 한 장을 열 때마다
//! 검색이 100번 나간다.

use crate::media::ytdlp::YtDlp;
use crate::models::{ProviderKind, TrackRef};
use crate::remote::store::RemoteStore;

/// TJ 차트 주소 접두사. `tj:hot` 또는 `tj:top:3`.
pub const TJ_CHART_PREFIX: &str = "tj:";

const TJ_API: &str = "https://www.tjmedia.com/legacy/api/topAndHot100";

/// 한 번에 이만큼씩만 새로 찾는다. 차트 한 장이 100곡인데 처음 열 때 100번 검색하면
/// 몇 분이 걸리고 그동안 화면이 멈춘 것처럼 보인다. 못 찾은 나머지는 다음 조회나
/// 유휴 시간 프리페치가 이어서 채운다 — **부분이라도 바로 보여주는 쪽을 고른다.**
const RESOLVE_PER_FETCH: usize = 25;

/// 못 찾은 곡을 몇 번까지 다시 시도할지. 반주 영상이 아예 없는 곡도 있어서 무한정
/// 재시도하면 매번 그 곡들에 검색을 낭비한다.
const MAX_MISS: i64 = 3;

/// TJ 차트 한 장의 정체. `tj:hot` / `tj:top:N` 을 뜯은 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TjChart {
    /// `HOT` 이면 strType 을 무시하고 전체 인기 100곡이 나온다(실측).
    pub hot: bool,
    pub str_type: u8,
}

impl TjChart {
    /// `tj:hot` · `tj:top:3` 을 읽는다. 우리 차트가 아니면 `None`.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix(TJ_CHART_PREFIX)?.trim();
        if rest.eq_ignore_ascii_case("hot") {
            return Some(Self {
                hot: true,
                str_type: 0,
            });
        }
        let number = rest.strip_prefix("top:")?.trim();
        Some(Self {
            hot: false,
            str_type: number.parse().ok()?,
        })
    }

    fn chart_type(self) -> &'static str {
        if self.hot { "HOT" } else { "TOP" }
    }

    /// 사람이 읽는 분류 이름. 곡 내용으로 확인한 매핑이다(2026-08-07).
    /// TJ 가 이 숫자의 뜻을 공개하지 않아서, 새 번호를 넣을 때는 **실제 곡을 보고** 정한다.
    pub fn label(self) -> &'static str {
        if self.hot {
            return "TJ 인기 100";
        }
        match self.str_type {
            2 => "팝송",
            3 => "J-POP",
            4 => "발라드",
            5 => "댄스",
            6 => "트로트",
            7 => "인디·어쿠스틱",
            _ => "기타",
        }
    }
}

/// TJ 가 준 한 줄. 아직 재생 주소가 없다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TjEntry {
    pub rank: u32,
    /// TJ 곡번호. 재생 주소 캐시의 열쇠다.
    pub number: i64,
    pub title: String,
    pub artist: String,
}

/// TJ 가 준 원본 JSON 을 뜯는다. HTTP 는 부르지 않아서 테스트가 쉽다.
pub fn parse_response(body: &str) -> Result<Vec<TjEntry>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| format!("TJ 응답을 읽지 못했어요: {error}"))?;
    // 성공 코드가 "99" 다. 뒤집힌 규약이라 여기서 한 번만 다룬다.
    let code = value.get("resultCode").and_then(|v| v.as_str()).unwrap_or("");
    if code != "99" {
        let message = value
            .get("resultMsg")
            .and_then(|v| v.as_str())
            .unwrap_or("알 수 없는 이유");
        return Err(format!("TJ 가 거절했어요: {message} (코드 {code})"));
    }
    let items = value
        .get("resultData")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| "TJ 응답에 곡 목록이 없어요.".to_string())?;

    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        // rank 는 문자열로 온다("1"). 숫자로 오는 날이 와도 받아 준다.
        let rank = item
            .get("rank")
            .and_then(|v| v.as_str().and_then(|s| s.trim().parse().ok()).or_else(|| v.as_u64().map(|n| n as u32)))
            .unwrap_or(0);
        let number = item
            .get("pro")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())));
        let title = item.get("indexTitle").and_then(|v| v.as_str()).unwrap_or("").trim();
        let artist = item.get("indexSong").and_then(|v| v.as_str()).unwrap_or("").trim();
        let (Some(number), false) = (number, title.is_empty()) else {
            continue; // 곡번호나 제목이 없으면 재생도 못 하고 캐시도 못 한다.
        };
        entries.push(TjEntry {
            rank,
            number,
            title: title.to_string(),
            artist: artist.to_string(),
        });
    }
    if entries.is_empty() {
        return Err("TJ 가 빈 목록을 줬어요.".to_string());
    }
    Ok(entries)
}

/// 이 곡의 **원곡**을 찾을 때 쓰는 검색어.
///
/// TJ 는 순위만 빌려 오고 **트는 것은 원곡(AR)이다.** 반주(MR)를 틀면 노래방 기기가
/// 되지 노래봇이 아니다. 그래서 `TJ노래방` 같은 말을 일부러 **안** 붙인다 —
/// 붙이면 `[TJ노래방] 곡 - 가수 / TJ Karaoke` 반주 영상이 1순위로 잡힌다.
pub fn search_query(entry: &TjEntry) -> String {
    if entry.artist.is_empty() {
        entry.title.clone()
    } else {
        format!("{} {}", entry.title, entry.artist)
    }
}

/// 반주·커버처럼 원곡이 아닌 결과를 걸러낸다.
///
/// 검색어에서 노래방을 뺐어도 곡에 따라 반주 영상이 먼저 잡힌다(특히 옛날 가요).
/// 제목만 보고도 확실히 아닌 것들은 여기서 버리고 다음 후보를 쓴다.
/// **원곡을 못 찾는 것보다 반주를 트는 게 더 나쁘다** — 사람들은 노래를 들으려고 넣는다.
pub fn looks_like_karaoke(title: &str) -> bool {
    let lowered = title.to_lowercase();
    const MARKERS: [&str; 10] = [
        "노래방",
        "karaoke",
        "instrumental",
        "inst.",
        "(mr)",
        "[mr]",
        "mr 버전",
        "반주",
        "가라오케",
        "カラオケ",
    ];
    MARKERS.iter().any(|marker| lowered.contains(marker))
}

/// TJ 차트를 가져와 재생 가능한 트랙 목록으로 만든다.
///
/// 이미 아는 곡은 캐시에서 즉시 나오고, 모르는 곡만 최대 [`RESOLVE_PER_FETCH`] 개를 찾는다.
/// 그래서 **처음 한 번은 목록이 짧게 나올 수 있고**, 다시 열거나 프리페치가 돌면 채워진다.
pub async fn fetch(
    chart: TjChart,
    limit: usize,
    store: &RemoteStore,
    ytdlp: &YtDlp,
    client: &reqwest::Client,
    resolve_budget: usize,
) -> Result<Vec<TrackRef>, String> {
    let entries = request(chart, client).await?;
    let mut tracks = Vec::new();
    let mut spent = 0usize;

    for entry in entries.into_iter().take(limit) {
        if let Some(track) = store.tj_track(entry.number) {
            tracks.push(track);
            continue;
        }
        if store.tj_miss_count(entry.number) >= MAX_MISS || spent >= resolve_budget {
            continue;
        }
        spent += 1;
        match resolve_original(&entry, ytdlp).await {
            Some(track) => {
                store.save_tj_track(entry.number, &entry.title, &entry.artist, Some(&track));
                tracks.push(track);
            }
            None => store.save_tj_track(entry.number, &entry.title, &entry.artist, None),
        }
    }

    if tracks.is_empty() {
        return Err("TJ 차트에서 재생할 수 있는 곡을 아직 못 찾았어요. 잠시 뒤에 다시 열어 주세요.".into());
    }
    Ok(tracks)
}

/// 이 곡의 원곡을 한 번 찾는다. 반주로 보이는 후보는 건너뛴다.
///
/// 후보를 여러 개 받아 보는 이유가 있다. 1위 결과가 반주일 때 그걸 버리고 끝내면
/// 원곡이 있는데도 "못 찾음" 으로 기록돼 [`MAX_MISS`] 까지 재시도만 낭비한다.
async fn resolve_original(entry: &TjEntry, ytdlp: &YtDlp) -> Option<crate::models::TrackRef> {
    let candidates = ytdlp.search(&search_query(entry), RESOLVE_CANDIDATES).await;
    let mut track = candidates.into_iter().find(|candidate| {
        candidate
            .title
            .as_deref()
            .is_none_or(|title| !looks_like_karaoke(title))
    })?;
    // TJ 가 준 제목·가수를 쓴다. 유튜브 제목은 `... (Official MV) [4K]` 처럼 군더더기가 붙어
    // 그대로 두면 대기열이 지저분해진다.
    track.title = Some(entry.title.clone());
    if !entry.artist.is_empty() {
        track.artist = Some(entry.artist.clone());
    }
    Some(track)
}

/// 원곡을 고를 때 훑어볼 후보 수. 1위가 반주여도 다음 것에서 건질 수 있게 한다.
const RESOLVE_CANDIDATES: usize = 5;

/// 순위만 가져온다. 프리페치가 "무엇을 채워야 하는지" 알아내는 데도 쓴다.
pub async fn request(chart: TjChart, client: &reqwest::Client) -> Result<Vec<TjEntry>, String> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let response = client
        .post(TJ_API)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("Referer", "https://www.tjmedia.com/")
        .form(&[
            ("chartType", chart.chart_type()),
            ("strType", &chart.str_type.to_string()),
            ("searchStartDate", &yesterday),
            ("searchEndDate", &today),
        ])
        .send()
        .await
        .map_err(|error| format!("TJ 에 닿지 못했어요: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("TJ 가 {} 를 줬어요.", response.status()));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("TJ 응답을 받지 못했어요: {error}"))?;
    parse_response(&body)
}

/// 아직 반주 영상을 못 찾은 곡을 채운다. 유휴 시간 프리페치가 부른다.
/// 한 번에 몇 곡만 채우고 끝낸다 — 백그라운드가 yt-dlp 를 오래 붙들면 사람이 검색할 때 밀린다.
pub async fn resolve_pending(
    chart: TjChart,
    store: &RemoteStore,
    ytdlp: &YtDlp,
    client: &reqwest::Client,
    budget: usize,
) -> usize {
    let Ok(entries) = request(chart, client).await else {
        return 0;
    };
    let mut filled = 0usize;
    for entry in entries {
        if filled >= budget {
            break;
        }
        if store.tj_track(entry.number).is_some() || store.tj_miss_count(entry.number) >= MAX_MISS {
            continue;
        }
        filled += 1;
        let found = resolve_original(&entry, ytdlp).await;
        store.save_tj_track(entry.number, &entry.title, &entry.artist, found.as_ref());
    }
    filled
}

/// 차트를 열 때 한 번에 새로 찾는 곡 수. [`fetch`] 를 부르는 쪽이 그대로 쓰면 된다.
pub const fn default_resolve_budget() -> usize {
    RESOLVE_PER_FETCH
}

/// 저장된 provider 문자열을 되돌린다. 모르는 값은 유튜브로 본다 — TJ 반주는 전부 유튜브다.
pub fn provider_from_str(value: &str) -> ProviderKind {
    match value {
        "YouTubeMusic" => ProviderKind::YouTubeMusic,
        "SoundCloud" => ProviderKind::SoundCloud,
        _ => ProviderKind::YouTube,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_chart_url_forms() {
        assert_eq!(
            TjChart::parse("tj:hot"),
            Some(TjChart {
                hot: true,
                str_type: 0
            })
        );
        assert_eq!(
            TjChart::parse("tj:top:3"),
            Some(TjChart {
                hot: false,
                str_type: 3
            })
        );
        assert_eq!(TjChart::parse("tj:top:3").unwrap().label(), "J-POP");
        assert_eq!(TjChart::parse("tj:hot").unwrap().label(), "TJ 인기 100");
        // 우리 것이 아닌 주소
        assert_eq!(TjChart::parse("ytsearch50:TJ노래방"), None);
        assert_eq!(TjChart::parse("internal:guild-plays"), None);
        assert_eq!(TjChart::parse("tj:top:"), None);
    }

    #[test]
    fn chart_type_switches_on_hot() {
        assert_eq!(TjChart::parse("tj:hot").unwrap().chart_type(), "HOT");
        assert_eq!(TjChart::parse("tj:top:5").unwrap().chart_type(), "TOP");
    }

    /// 실제 응답을 줄인 것. 필드 이름이 바뀌면 여기서 먼저 깨진다.
    const SAMPLE: &str = r#"{
        "resultCode": "99",
        "resultMsg": "성공",
        "resultData": {
            "itemsTotalCount": 3,
            "items": [
                {"rank":"1","pro":52788,"indexTitle":"晩餐歌","indexSong":"tuki."},
                {"rank":"2","pro":68058,"indexTitle":"Pretender","indexSong":"Official髭男dism"},
                {"rank":"3","pro":68553,"indexTitle":"ベテルギウス","indexSong":"優里"}
            ]
        }
    }"#;

    #[test]
    fn reads_rank_number_title_artist() {
        let entries = parse_response(SAMPLE).expect("성공 응답");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].rank, 1);
        assert_eq!(entries[0].number, 52788);
        assert_eq!(entries[0].title, "晩餐歌");
        assert_eq!(entries[0].artist, "tuki.");
        assert_eq!(entries[2].number, 68553);
    }

    /// 회귀: 성공 코드가 "99" 다. 0 이나 "00" 을 성공으로 보면 **항상 실패로 읽는다.**
    #[test]
    fn treats_99_as_success_and_anything_else_as_failure() {
        assert!(parse_response(SAMPLE).is_ok());
        let rejected = r#"{"resultCode":"00","resultMsg":"권한 없음","resultData":{"items":[]}}"#;
        let error = parse_response(rejected).unwrap_err();
        assert!(error.contains("권한 없음"), "이유를 그대로 전해야 한다: {error}");
    }

    #[test]
    fn skips_rows_without_a_song_number() {
        let body = r#"{"resultCode":"99","resultData":{"items":[
            {"rank":"1","indexTitle":"번호 없음","indexSong":"누구"},
            {"rank":"2","pro":100,"indexTitle":"정상","indexSong":"가수"}
        ]}}"#;
        let entries = parse_response(body).expect("한 줄은 살아야 한다");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].number, 100);
    }

    #[test]
    fn empty_list_is_an_error_not_an_empty_chart() {
        let body = r#"{"resultCode":"99","resultData":{"items":[]}}"#;
        assert!(parse_response(body).is_err(), "빈 차트를 조용히 내보내면 안 된다");
    }

    /// TJ 는 **순위만** 빌려 온다. 트는 것은 원곡이라 검색어에 노래방을 붙이면 안 된다.
    /// 붙이면 반주(MR)가 1순위로 잡혀서 노래봇이 노래방 기기가 된다.
    #[test]
    fn search_query_looks_for_the_original_not_the_karaoke_track() {
        let entry = TjEntry {
            rank: 1,
            number: 1,
            title: "좋니".into(),
            artist: "윤종신".into(),
        };
        assert_eq!(search_query(&entry), "좋니 윤종신");
        assert!(!search_query(&entry).contains("노래방"));
        let no_artist = TjEntry {
            artist: String::new(),
            ..entry
        };
        assert_eq!(search_query(&no_artist), "좋니");
    }

    /// 검색어에서 노래방을 빼도 곡에 따라 반주가 먼저 잡힌다. 제목으로 한 번 더 거른다.
    #[test]
    fn rejects_karaoke_results_by_title() {
        for title in [
            "[TJ노래방] 좋니 - 윤종신 / TJ Karaoke",
            "좋니 - 윤종신 (KY.53410) / KY Karaoke",
            "IU - Through the Night (Instrumental)",
            "밤편지 (MR)",
            "夜に駆ける カラオケ",
            "애상 반주",
        ] {
            assert!(looks_like_karaoke(title), "반주로 걸러야 한다: {title}");
        }
        for title in [
            "윤종신 - 좋니 (Official Music Video)",
            "IU(아이유) _ Through the Night(밤편지)",
            "YOASOBI「夜に駆ける」Official Music Video",
            "Radiohead - Creep",
        ] {
            assert!(!looks_like_karaoke(title), "원곡을 버리면 안 된다: {title}");
        }
    }
}
