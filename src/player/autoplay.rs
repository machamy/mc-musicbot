//! 자동추천 엔진 — C# BuildRecommendation + GetAutoplayRecommendationAsync 포팅.
//! 1순위 공급자 라디오(RD/RDAMVM/SC recommended) → 2순위 제목+아티스트 검색 → 최후 보루.
//! 차단 규칙에 걸린 후보는 최대 5회 건너뛰며 재시도.
//!
//! 길드에 **기준 곡(자동 재생 시드)** 이 등록돼 있으면 그중 하나를 라운드로빈으로 골라
//! 시드로 쓴다(`recommend_with_seeds`). 등록된 곡이 없으면 지금까지처럼 현재 곡·최근 곡을 쓴다.

use crate::blacklist::Blacklist;
use crate::logging::LogService;
use crate::media::ytdlp::YtDlp;
use crate::models::TrackRef;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// 자동추천에서 제외할 최대 길이(초). 10분을 넘으면 보통 믹스/루프/긴 라이브 등
/// "노래가 아닌" 영상일 확률이 높아, 사용자 요청으로 다른 곡을 다시 추천받는다.
const MAX_AUTOPLAY_SECS: f64 = 600.0;

/// 한 번 추천에 쓸 최대 시도 횟수 (차단·길이 필터에 걸리면 다시 돈다).
const MAX_ATTEMPTS: usize = 8;

/// 길이를 아는 경우에만 과길이 판정 (flat-playlist 가 duration 을 누락하면 통과시켜
/// 멀쩡한 곡까지 버리지 않는다).
fn is_overlong(t: &TrackRef) -> bool {
    matches!(&t.duration, Some(d) if d.as_secs_f64() > MAX_AUTOPLAY_SECS)
}

/// 길드별 기준 곡 라운드로빈 커서. 재시작하면 0부터 다시 돌아도 문제없는 값이라 메모리에만 둔다.
/// (엔진 필드로 두는 편이 깔끔하지만 그러면 `App::new`의 구조체 리터럴까지 손대야 한다.
/// 프로세스당 엔진이 하나뿐이고 길드별로는 확실히 분리되므로 여기서는 모듈 전역으로 충분하다.)
fn seed_cursors() -> &'static Mutex<HashMap<u64, usize>> {
    static CURSORS: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
    CURSORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 이번에 쓸 기준 곡 인덱스를 돌려주고 커서를 한 칸 민다. 길드마다 따로 돈다.
fn next_seed_index(guild_id: u64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut cursors = seed_cursors()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cursor = cursors.entry(guild_id).or_insert(0);
    let index = *cursor % len;
    *cursor = (*cursor + 1) % len;
    index
}

/// 이번 추천에 쓸 시드 한 곡. 기준 곡이 없으면 지금까지처럼 넘겨받은 곡을 쓴다.
/// 반환값의 첫 요소는 로그에 찍을 "몇 번째 기준 곡"(0-based)이고, 기준 곡이 없으면 `None`이다.
/// 기준 곡도 없고 참고할 곡도 없으면 통째로 `None` — 이때는 추천 자체를 건너뛴다.
fn pick_seed(
    guild_id: u64,
    fallback_seed: Option<&TrackRef>,
    seeds: &[TrackRef],
) -> Option<(Option<usize>, TrackRef)> {
    if seeds.is_empty() {
        return fallback_seed.map(|seed| (None, seed.clone()));
    }
    let index = next_seed_index(guild_id, seeds.len());
    Some((Some(index), seeds[index].clone()))
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
                "1순위 라디오({:?})에서 후보 {}곡을 모았어요 (시드 '{}').",
                seed.provider,
                station.len(),
                seed.display_title()
            ),
        );
        if let Some(fresh) = station.iter().find(|t| is_fresh(t)) {
            self.log.info(
                "Autoplay",
                &format!("라디오에서 새 후보를 골랐어요: '{}'.", fresh.display_title()),
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
                "2순위 검색('{}')에서 후보 {}곡을 모았어요.",
                query.trim(),
                search.len()
            ),
        );
        if let Some(fresh) = search.iter().find(|t| is_fresh(t)) {
            self.log.info(
                "Autoplay",
                &format!("검색에서 새 후보를 골랐어요: '{}'.", fresh.display_title()),
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
                &format!(
                    "새 후보가 없어서 최후 보루로 '{}'를 골랐어요.",
                    t.display_title()
                ),
            );
        }
        last
    }

    /// 차단 규칙을 통과한 추천 한 곡 (최대 8회 재시도).
    /// 기준 곡을 안 쓰는 경로 — 넘겨받은 곡 하나만 시드로 삼는다.
    pub async fn recommend(
        &self,
        guild_id: u64,
        seed: &TrackRef,
        excluded: &HashSet<String>,
    ) -> Option<TrackRef> {
        self.recommend_with_seeds(guild_id, Some(seed), &[], excluded)
            .await
    }

    /// 등록된 기준 곡(`seeds`)이 있으면 그중 하나를 **라운드로빈**으로 골라 그 곡의 라디오에서 뽑는다.
    /// 곡마다 돌아가며 써야 한 곡 장르로 쏠리지 않는다. 비어 있으면 `fallback_seed`(현재 곡·최근 곡)로
    /// 지금까지와 똑같이 동작한다. 대기열·최근 중복(`excluded`), 라이브, 길이 초과 필터는 그대로다.
    ///
    /// 기준 곡이 있으면 `fallback_seed`가 `None`이어도 된다 — 아직 아무 곡도 안 튼 서버에서
    /// 기준 곡만으로 자동 재생을 시작할 수 있어야 한다.
    pub async fn recommend_with_seeds(
        &self,
        guild_id: u64,
        fallback_seed: Option<&TrackRef>,
        seeds: &[TrackRef],
        excluded: &HashSet<String>,
    ) -> Option<TrackRef> {
        let mut seen = excluded.clone();
        let Some((mut picked, mut seed)) = pick_seed(guild_id, fallback_seed, seeds) else {
            self.log.info(
                "Autoplay",
                "기준 곡도 없고 참고할 최근 곡도 없어서 이번 추천은 건너뛰어요.",
            );
            return None;
        };
        if let Some(index) = picked {
            self.log.info(
                "Autoplay",
                &format!(
                    "등록된 기준 곡 {}곡 중 {}번째 '{}'을 기준으로 추천을 받아요.",
                    seeds.len(),
                    index + 1,
                    seed.display_title()
                ),
            );
        }
        // 기준 곡을 몇 번 갈아탔는지 — 한 곡이 막혔다고 자동 재생이 멈추면 안 된다.
        let mut rotations = 0usize;
        for attempt in 1..=MAX_ATTEMPTS {
            self.log.info(
                "Autoplay",
                &format!(
                    "추천을 {attempt}/{MAX_ATTEMPTS}번째로 시도해요 (시드 '{}', 제외 {}곡).",
                    seed.display_title(),
                    seen.len()
                ),
            );
            let next = match self.next_candidate(&seed, &seen).await {
                Some(n) => n,
                None => {
                    // 아직 안 써 본 기준 곡이 남아 있으면 다음 곡으로 넘어간다.
                    if rotations + 1 < seeds.len() {
                        rotations += 1;
                        let previous = seed.display_title().to_string();
                        // seeds 가 비어 있지 않으므로 여기서 None 이 나올 수 없다.
                        let Some(rotated) = pick_seed(guild_id, fallback_seed, seeds) else {
                            return None;
                        };
                        (picked, seed) = rotated;
                        self.log.info(
                            "Autoplay",
                            &format!(
                                "기준 곡 '{previous}'에서는 후보가 안 나와서 {}번째 '{}'으로 넘어가요.",
                                picked.map(|index| index + 1).unwrap_or(0),
                                seed.display_title()
                            ),
                        );
                        continue;
                    }
                    self.log.warn(
                        "Autoplay",
                        &format!(
                            "시드 '{}'({:?}) 기준으로는 추천을 못 찾았어요. 라디오가 막혔거나 쿠키가 만료됐을 수 있어요. 다음 곡이 끝날 때까지 자동추천을 쉬어요.",
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
                        "추천 '{}'({:?})은 차단 규칙에 걸려서 건너뛰어요. ruleId={}, pattern={}.",
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
                        "추천 '{}'({:?})은 길이 {:.0}분을 넘어서({:.0}초) 건너뛰고 다른 곡을 다시 받아요.",
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
                    "추천을 확정했어요: '{}'({:?}, {}) ← 시드 '{}' (시도 {attempt}/{MAX_ATTEMPTS}).",
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
            &format!(
                "차단 규칙과 길이 제한 때문에 {MAX_ATTEMPTS}번 안에 쓸 만한 추천을 못 찾았어요."
            ),
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProviderKind;

    fn track(id: &str) -> TrackRef {
        TrackRef {
            provider: ProviderKind::YouTube,
            content_id: id.into(),
            source_url: format!("https://example.test/{id}"),
            title: Some(id.into()),
            artist: None,
            duration: None,
            variant_key: None,
        }
    }

    /// 커서는 길드마다 따로 돈다. 다른 테스트와 겹치지 않게 전용 길드 id를 쓴다.
    #[test]
    fn seed_cursor_rotates_per_guild() {
        let (a, b) = (900_101, 900_102);
        let picked: Vec<usize> = (0..7).map(|_| next_seed_index(a, 3)).collect();
        assert_eq!(picked, vec![0, 1, 2, 0, 1, 2, 0]);
        // b 길드는 a 가 아무리 돌았어도 처음부터 시작한다.
        assert_eq!(next_seed_index(b, 2), 0);
        assert_eq!(next_seed_index(b, 2), 1);
        assert_eq!(next_seed_index(b, 2), 0);
        // 곡 수가 줄어도 인덱스가 범위를 벗어나지 않는다.
        assert!(next_seed_index(a, 1) < 1);
        assert_eq!(next_seed_index(a, 0), 0);
    }

    #[test]
    fn seeds_are_used_round_robin_and_fall_back_when_empty() {
        let guild = 900_103;
        let fallback = track("현재곡");
        let seeds = vec![track("기준1"), track("기준2")];

        let (index, chosen) = pick_seed(guild, Some(&fallback), &seeds).unwrap();
        assert_eq!(index, Some(0));
        assert_eq!(chosen.content_id, "기준1");
        let (index, chosen) = pick_seed(guild, Some(&fallback), &seeds).unwrap();
        assert_eq!(index, Some(1));
        assert_eq!(chosen.content_id, "기준2");
        let (index, chosen) = pick_seed(guild, Some(&fallback), &seeds).unwrap();
        assert_eq!(index, Some(0));
        assert_eq!(chosen.content_id, "기준1");

        // 기준 곡이 없으면 지금 동작 그대로 — 넘겨받은 곡을 쓴다.
        let (index, chosen) = pick_seed(guild, Some(&fallback), &[]).unwrap();
        assert_eq!(index, None);
        assert_eq!(chosen.content_id, "현재곡");

        // 아직 아무 곡도 안 튼 서버라도 기준 곡만 있으면 추천을 시작할 수 있다.
        assert!(pick_seed(guild, None, &seeds).is_some());
        // 둘 다 없으면 이번 추천은 건너뛴다.
        assert!(pick_seed(guild, None, &[]).is_none());
    }
}
