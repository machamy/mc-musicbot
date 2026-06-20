//! 자동추천 엔진 — C# BuildRecommendation + GetAutoplayRecommendationAsync 포팅.
//! 1순위 공급자 라디오(RD/RDAMVM/SC recommended) → 2순위 제목+아티스트 검색 → 최후 보루.
//! 차단 규칙에 걸린 후보는 최대 5회 건너뛰며 재시도.

use crate::blacklist::Blacklist;
use crate::logging::LogService;
use crate::media::ytdlp::YtDlp;
use crate::models::TrackRef;
use std::collections::HashSet;
use std::sync::Arc;

/// 자동추천에서 제외할 최대 길이(초). 10분을 넘으면 보통 믹스/루프/긴 라이브 등
/// "노래가 아닌" 영상일 확률이 높아, 사용자 요청으로 다른 곡을 다시 추천받는다.
const MAX_AUTOPLAY_SECS: f64 = 600.0;

/// 길이를 아는 경우에만 과길이 판정 (flat-playlist 가 duration 을 누락하면 통과시켜
/// 멀쩡한 곡까지 버리지 않는다).
fn is_overlong(t: &TrackRef) -> bool {
    matches!(&t.duration, Some(d) if d.as_secs_f64() > MAX_AUTOPLAY_SECS)
}

pub struct AutoplayEngine {
    pub ytdlp: YtDlp,
    pub blacklist: Arc<Blacklist>,
    pub log: Arc<LogService>,
}

impl AutoplayEngine {
    /// 시드 기준으로 신선한(제외 목록에 없는) 추천 한 곡을 찾는다.
    async fn next_candidate(
        &self,
        seed: &TrackRef,
        excluded: &HashSet<String>,
    ) -> Option<TrackRef> {
        let is_fresh = |t: &TrackRef| {
            let key = t.cache_key();
            !key.eq_ignore_ascii_case(&seed.cache_key()) && !excluded.contains(&key)
        };

        // 1순위: 공급자 라디오 믹스.
        let station = self.ytdlp.station_candidates(seed).await;
        self.log.info(
            "Autoplay",
            &format!(
                "1순위 라디오({:?}) 후보 {}곡 수집 (시드 '{}').",
                seed.provider,
                station.len(),
                seed.display_title()
            ),
        );
        if let Some(fresh) = station.iter().find(|t| is_fresh(t)) {
            self.log.info(
                "Autoplay",
                &format!("라디오에서 신선한 후보 채택: '{}'.", fresh.display_title()),
            );
            return Some(fresh.clone());
        }

        // 2순위: 제목+아티스트 검색.
        let mut query = String::new();
        if let Some(t) = &seed.title {
            query.push_str(t);
        }
        if let Some(a) = &seed.artist {
            if !query.is_empty() {
                query.push(' ');
            }
            query.push_str(a);
        }
        if query.trim().is_empty() {
            query = seed.content_id.clone();
        }
        let search = self.ytdlp.search(&query, 5).await;
        self.log.info(
            "Autoplay",
            &format!(
                "2순위 검색('{}') 후보 {}곡 수집.",
                query.trim(),
                search.len()
            ),
        );
        if let Some(fresh) = search.iter().find(|t| is_fresh(t)) {
            self.log.info(
                "Autoplay",
                &format!("검색에서 신선한 후보 채택: '{}'.", fresh.display_title()),
            );
            return Some(fresh.clone());
        }

        // 최후 보루: 시드만 아니면 무엇이든 (단, 과길이 영상은 제외).
        let last = station
            .iter()
            .chain(search.iter())
            .find(|t| !t.cache_key().eq_ignore_ascii_case(&seed.cache_key()) && !is_overlong(t))
            .cloned();
        if let Some(t) = &last {
            self.log.info(
                "Autoplay",
                &format!("최후 보루로 '{}' 채택(신선 후보 없음).", t.display_title()),
            );
        }
        last
    }

    /// 차단 규칙을 통과한 추천 한 곡 (최대 5회 재시도).
    pub async fn recommend(
        &self,
        guild_id: u64,
        seed: &TrackRef,
        excluded: &HashSet<String>,
    ) -> Option<TrackRef> {
        let mut seen = excluded.clone();
        for attempt in 1..=8 {
            self.log.info(
                "Autoplay",
                &format!(
                    "추천 시도 {attempt}/8 (시드 '{}', 제외 {}곡).",
                    seed.display_title(),
                    seen.len()
                ),
            );
            let next = match self.next_candidate(seed, &seen).await {
                Some(n) => n,
                None => {
                    self.log.warn(
                        "Autoplay",
                        &format!(
                            "시드 '{}'({:?})로 추천을 찾지 못함. 라디오 차단/쿠키 만료 가능. 다음 곡 종료까지 자동추천 대기.",
                            seed.display_title(),
                            seed.provider
                        ),
                    );
                    return None;
                }
            };
            if let Some(rule) = self.blacklist.try_get_blocker(guild_id, &next) {
                self.log.info(
                    "Autoplay",
                    &format!(
                        "추천 '{}'({:?}) 차단 규칙에 걸려 건너뜀. ruleId={}, pattern={}.",
                        next.display_title(),
                        next.provider,
                        rule.id,
                        rule.pattern
                    ),
                );
                seen.insert(next.cache_key());
                continue;
            }
            if is_overlong(&next) {
                let secs = next.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);
                self.log.info(
                    "Autoplay",
                    &format!(
                        "추천 '{}'({:?}) 길이 {:.0}분 초과({:.0}초)라 건너뜀 — 다른 곡 재추천.",
                        next.display_title(),
                        next.provider,
                        MAX_AUTOPLAY_SECS / 60.0,
                        secs
                    ),
                );
                seen.insert(next.cache_key());
                continue;
            }
            let dur_txt = next
                .duration
                .map(|d| format!("{:.0}초", d.as_secs_f64()))
                .unwrap_or_else(|| "길이미상".into());
            self.log.info(
                "Autoplay",
                &format!(
                    "추천 확정 '{}'({:?}, {}) ← 시드 '{}' (시도 {attempt}/8).",
                    next.display_title(),
                    next.provider,
                    dur_txt,
                    seed.display_title()
                ),
            );
            return Some(next);
        }
        self.log.warn(
            "Autoplay",
            "차단 규칙/길이 제한으로 최대 시도(8) 내에 통과 가능한 추천을 찾지 못함.",
        );
        None
    }
}
