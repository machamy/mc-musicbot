//! 자동추천 엔진 — C# BuildRecommendation + GetAutoplayRecommendationAsync 포팅.
//! 1순위 공급자 라디오(RD/RDAMVM/SC recommended) → 2순위 제목+아티스트 검색 → 최후 보루.
//!
//! **시드를 어디서 고르는가**(`AutoplayMode`, §8.2)와 **그 후보 목록에서 무엇을 집는가**
//! (`AutoplayPolicy`, §8.5)는 다른 축이다. 시드 선택은 호출부(`side_effects`)가 하고,
//! 이 모듈은 넘겨받은 시드로 라디오를 돌린 뒤 **정책·아티스트 쿨다운·이력 감쇠·차단 기억**을
//! 적용해 한 곡을 고른다.
//!
//! 무작위는 **재현 가능**하다. `(guild_id, 시드 캐시키, 후보 수, 시각/10분)` 해시를 seed 로 쓰는
//! 결정적 PRNG 라, 같은 상황에서 10분 안에는 같은 답이 나오고 로그만 보면 재현할 수 있다.

use crate::blacklist::Blacklist;
use crate::logging::LogService;
use crate::media::ytdlp::YtDlp;
use crate::models::TrackRef;
use crate::remote::AutoplayPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

/// 자동추천에서 제외할 최대 길이(초). 10분을 넘으면 보통 믹스/루프/긴 라이브 등
/// "노래가 아닌" 영상일 확률이 높아, 사용자 요청으로 다른 곡을 다시 추천받는다.
const MAX_AUTOPLAY_SECS: f64 = 600.0;

/// 한 번 추천에 쓸 최대 시도 횟수 (차단·길이 필터에 걸리면 다시 돈다).
const MAX_ATTEMPTS: usize = 8;

/// 결정적 PRNG 의 시간 칸 크기(초). 같은 상황이면 이 시간 안에는 같은 곡이 나온다 (§8.5).
const RNG_SLOT_SECS: i64 = 600;

/// `popular` 정책이 "무난한 길이"로 보는 구간(초). 2~7분.
const POPULAR_SECS: (f64, f64) = (120.0, 420.0);

/// 이력 감쇠에서 이 아래로 떨어진 후보는 아예 안 뽑는다.
/// 방금 튼 곡이 바로 또 나오면 감쇠가 아니라 고장으로 보인다.
const DECAY_FLOOR: f64 = 0.05;

/// 길이를 아는 경우에만 과길이 판정 (flat-playlist 가 duration 을 누락하면 통과시켜
/// 멀쩡한 곡까지 버리지 않는다).
fn is_overlong(t: &TrackRef) -> bool {
    matches!(&t.duration, Some(d) if d.as_secs_f64() > MAX_AUTOPLAY_SECS)
}

fn artist_key(track: &TrackRef) -> Option<String> {
    track
        .artist
        .as_deref()
        .map(|artist| artist.trim().to_lowercase())
        .filter(|artist| !artist.is_empty())
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

// ───────── 결정적 PRNG (§8.5) ─────────

/// splitmix64. 짧고 상태가 하나뿐이라 같은 입력이면 어디서 돌려도 같은 수열이 나온다.
#[derive(Debug, Clone)]
pub struct DeterministicRng(u64);

impl DeterministicRng {
    /// `(guild_id, 시드 캐시키, 후보 수, 시각/10분)` 을 섞은 해시로 시작한다.
    pub fn new(guild_id: u64, seed_key: &str, candidates: usize, slot: i64) -> Self {
        // FNV-1a 로 문자열까지 한 값에 접는다.
        fn fold(hash: u64, bytes: &[u8]) -> u64 {
            let mut hash = hash;
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
            hash
        }
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        hash = fold(hash, &guild_id.to_le_bytes());
        hash = fold(hash, seed_key.as_bytes());
        hash = fold(hash, &(candidates as u64).to_le_bytes());
        hash = fold(hash, &(slot as u64).to_le_bytes());
        Self(hash)
    }

    /// 지금 시각으로 만든 PRNG. 10분 안에는 같은 답이 나온다.
    pub fn now(guild_id: u64, seed_key: &str, candidates: usize) -> Self {
        Self::new(
            guild_id,
            seed_key,
            candidates,
            chrono::Utc::now().timestamp() / RNG_SLOT_SECS,
        )
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `0.0 ..< 1.0`
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ───────── 추천 정책 입력 (§8.5) ─────────

/// 추천 한 번에 적용할 정책 값. 길드 설정에서 그대로 온다.
#[derive(Debug, Clone, Copy)]
pub struct AutoplayTuning {
    pub policy: AutoplayPolicy,
    /// 최근 이 곡 수 안에 나온 아티스트는 뺀다. `0`이면 끔.
    pub artist_cooldown: u32,
    /// 이력 회피가 완전히 풀리는 시간. `0`이면 감쇠 없이 최근 곡을 그냥 제외한다(옛 동작).
    pub recent_decay_hours: u32,
}

impl Default for AutoplayTuning {
    fn default() -> Self {
        Self {
            policy: AutoplayPolicy::Balanced,
            artist_cooldown: 3,
            recent_decay_hours: 24,
        }
    }
}

/// 후보를 거를 때 참고하는 것들. 전부 호출부가 한 번씩만 조회해서 넘긴다 —
/// 추천 한 번에 DB 를 여러 번 왕복할 이유가 없다.
#[derive(Debug, Clone, Copy)]
pub struct AutoplayContext<'a> {
    /// 지금 재생 중·대기열에 있는 곡. **무조건 제외**다.
    pub excluded: &'a HashSet<String>,
    /// `📻 이 곡 말고`로 뺐거나 재생에 실패한 곡 (§8.5-3). 무조건 제외다.
    pub blocked: &'a HashSet<String>,
    /// `cache_key → 마지막 재생 후 지난 시간(시간)`. 최근일수록 강하게 회피한다 (§8.5-2).
    pub recent_ages: &'a HashMap<String, f64>,
    /// 최근 재생 아티스트 (최신순, 소문자). 앞에서 `artist_cooldown`개만 본다 (§8.5-1).
    pub recent_artists: &'a [String],
    pub tuning: AutoplayTuning,
}

impl<'a> AutoplayContext<'a> {
    /// 지금까지와 똑같이 동작하는 컨텍스트 — `excluded` 하나만 보고 정책은 기본값이다.
    pub fn legacy(excluded: &'a HashSet<String>) -> Self {
        static EMPTY_KEYS: OnceLock<HashSet<String>> = OnceLock::new();
        static EMPTY_AGES: OnceLock<HashMap<String, f64>> = OnceLock::new();
        Self {
            excluded,
            blocked: EMPTY_KEYS.get_or_init(HashSet::new),
            recent_ages: EMPTY_AGES.get_or_init(HashMap::new),
            recent_artists: &[],
            tuning: AutoplayTuning::default(),
        }
    }
}

/// 필터를 얼마나 풀었는지. 다 걸러서 후보가 0이 되면 자동재생이 이유 없이 멈춘 것처럼 보인다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relax {
    /// 아티스트 쿨다운 + 이력 감쇠 전부 적용.
    None,
    /// 아티스트 쿨다운을 뺀다.
    NoArtistCooldown,
    /// 이력 감쇠까지 뺀다 (하드 제외·차단·차단규칙·길이만 남는다).
    Everything,
}

impl Relax {
    fn label(self) -> &'static str {
        match self {
            Self::None => "정책 그대로",
            Self::NoArtistCooldown => "아티스트 쿨다운을 풀고",
            Self::Everything => "이력 감쇠까지 풀고",
        }
    }
}

pub struct AutoplayEngine {
    pub ytdlp: YtDlp,
    pub blacklist: Arc<Blacklist>,
    pub log: Arc<LogService>,
}

impl AutoplayEngine {
    /// 시드의 라디오 + 검색 결과를 한 목록으로 모은다. **순서가 유사도 순위**라 그대로 유지한다.
    async fn gather_candidates(&self, seed: &TrackRef) -> Vec<TrackRef> {
        let seed_key = seed.cache_key();
        let mut seen: HashSet<String> = HashSet::new();
        let mut candidates: Vec<TrackRef> = Vec::new();
        let push = |track: &TrackRef, seen: &mut HashSet<String>, out: &mut Vec<TrackRef>| {
            let key = track.cache_key();
            if key.eq_ignore_ascii_case(&seed_key) || !seen.insert(key) {
                return;
            }
            out.push(track.clone());
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
        for track in &station {
            push(track, &mut seen, &mut candidates);
        }

        // 2순위: 제목+아티스트 검색. 라디오가 얇을 때만 부른다 — yt-dlp 를 괜히 한 번 더 돌리지 않는다.
        if candidates.len() < 5 {
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
            for track in &search {
                push(track, &mut seen, &mut candidates);
            }
        }
        candidates
    }

    /// 후보 목록에서 한 곡을 고른다. 필터를 다 통과한 후보가 없으면 단계적으로 풀어 본다 —
    /// 좁은 취향 때문에 자동재생이 멈추는 것보다 조금 다른 곡이 나오는 게 낫다.
    fn choose(
        &self,
        guild_id: u64,
        seed: &TrackRef,
        candidates: &[TrackRef],
        ctx: &AutoplayContext<'_>,
        policy: AutoplayPolicy,
    ) -> Option<TrackRef> {
        for relax in [Relax::None, Relax::NoArtistCooldown, Relax::Everything] {
            let weighted = self.weigh(guild_id, candidates, ctx, policy, relax);
            if weighted.is_empty() {
                continue;
            }
            let mut rng = DeterministicRng::now(guild_id, &seed.cache_key(), weighted.len());
            let picked = weighted_pick(&weighted, &mut rng)?;
            let track = candidates[picked].clone();
            self.log.info(
                "Autoplay",
                &format!(
                    "정책 {}({} 후보 {}곡)에서 '{}'를 골랐어요.",
                    policy.as_str(),
                    relax.label(),
                    weighted.len(),
                    track.display_title()
                ),
            );
            return Some(track);
        }
        None
    }

    /// 살아남은 후보의 `(원본 인덱스, 가중치)`. 가중치가 클수록 잘 뽑힌다.
    fn weigh(
        &self,
        guild_id: u64,
        candidates: &[TrackRef],
        ctx: &AutoplayContext<'_>,
        policy: AutoplayPolicy,
        relax: Relax,
    ) -> Vec<(usize, f64)> {
        let cooldown = if relax == Relax::None {
            ctx.tuning.artist_cooldown as usize
        } else {
            0
        };
        let blocked_artists: HashSet<&str> = ctx
            .recent_artists
            .iter()
            .take(cooldown)
            .map(String::as_str)
            .collect();
        let window = policy.window();

        let mut kept: Vec<(usize, f64)> = Vec::new();
        // 정책 창(상위 N곡)은 **살아남은 후보 기준**으로 센다. 하드 제외된 곡이 창을 다 잡아먹으면
        // `similar` 이 매번 빈손이 된다.
        let mut rank = 0usize;
        for (index, track) in candidates.iter().enumerate() {
            let key = track.cache_key();
            if ctx.excluded.contains(&key) || ctx.blocked.contains(&key) || is_overlong(track) {
                continue;
            }
            if let Some(rule) = self.blacklist.try_get_blocker(guild_id, track) {
                self.log.info(
                    "Autoplay",
                    &format!(
                        "추천 후보 '{}'는 차단 규칙에 걸려서 뺐어요. ruleId={}, pattern={}.",
                        track.display_title(),
                        rule.id,
                        rule.pattern
                    ),
                );
                continue;
            }
            if !blocked_artists.is_empty()
                && artist_key(track).is_some_and(|artist| blocked_artists.contains(artist.as_str()))
            {
                continue;
            }
            let decay = if relax == Relax::Everything {
                1.0
            } else {
                decay_factor(ctx.recent_ages.get(&key).copied(), ctx.tuning.recent_decay_hours)
            };
            if decay < DECAY_FLOOR {
                continue;
            }
            if let Some(window) = window {
                if rank >= window {
                    rank += 1;
                    continue;
                }
            }
            let base = policy_weight(policy, rank, track);
            rank += 1;
            let weight = base * decay;
            if weight > 0.0 {
                kept.push((index, weight));
            }
        }
        kept
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
    /// 정책·쿨다운·감쇠를 안 쓰는 옛 호출부용 얇은 껍데기다.
    pub async fn recommend_with_seeds(
        &self,
        guild_id: u64,
        fallback_seed: Option<&TrackRef>,
        seeds: &[TrackRef],
        excluded: &HashSet<String>,
    ) -> Option<TrackRef> {
        let ctx = AutoplayContext::legacy(excluded);
        self.recommend_with_context(guild_id, fallback_seed, seeds, &ctx)
            .await
    }

    /// 추천 본체 (§8.2 + §8.5).
    ///
    /// 시드를 라운드로빈으로 고르고, 그 라디오 후보에 **정책 · 아티스트 쿨다운 · 이력 감쇠 ·
    /// 차단 기억**을 적용해 한 곡을 집는다. 한 시드에서 못 구하면 다음 시드로 넘어가고,
    /// 넘어갈 때마다 **정책을 한 단계 느슨하게** 한다 (§8.5-4).
    pub async fn recommend_with_context(
        &self,
        guild_id: u64,
        fallback_seed: Option<&TrackRef>,
        seeds: &[TrackRef],
        ctx: &AutoplayContext<'_>,
    ) -> Option<TrackRef> {
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
        let mut policy = ctx.tuning.policy;
        // 기준 곡을 몇 번 갈아탔는지 — 한 곡이 막혔다고 자동 재생이 멈추면 안 된다.
        let mut rotations = 0usize;
        for attempt in 1..=MAX_ATTEMPTS {
            self.log.info(
                "Autoplay",
                &format!(
                    "추천을 {attempt}/{MAX_ATTEMPTS}번째로 시도해요 (시드 '{}', 정책 {}, 제외 {}곡).",
                    seed.display_title(),
                    policy.as_str(),
                    ctx.excluded.len() + ctx.blocked.len()
                ),
            );
            let candidates = self.gather_candidates(&seed).await;
            if let Some(next) = self.choose(guild_id, &seed, &candidates, ctx, policy) {
                let dur_txt = next
                    .duration
                    .map(|d| format!("{:.0}초", d.as_secs_f64()))
                    .unwrap_or_else(|| "길이미상".into());
                self.log.info(
                    "Autoplay",
                    &format!(
                        "추천을 확정했어요: '{}'({:?}, {}) ← 시드 '{}' (시도 {attempt}/{MAX_ATTEMPTS}, 정책 {}).",
                        next.display_title(),
                        next.provider,
                        dur_txt,
                        seed.display_title(),
                        policy.as_str()
                    ),
                );
                return Some(next);
            }

            // 아직 안 써 본 기준 곡이 남아 있으면 다음 곡으로 넘어가면서 정책을 한 단계 푼다.
            if rotations + 1 < seeds.len() {
                rotations += 1;
                let previous = seed.display_title().to_string();
                let Some(rotated) = pick_seed(guild_id, fallback_seed, seeds) else {
                    return None;
                };
                (picked, seed) = rotated;
                let loosened = policy.loosened();
                self.log.info(
                    "Autoplay",
                    &format!(
                        "기준 곡 '{previous}'에서는 후보가 안 나와서 {}번째 '{}'으로 넘어가요 (정책 {} → {}).",
                        picked.map(|index| index + 1).unwrap_or(0),
                        seed.display_title(),
                        policy.as_str(),
                        loosened.as_str()
                    ),
                );
                policy = loosened;
                continue;
            }

            // 시드가 하나뿐이어도 정책은 풀어 볼 수 있다. 더 풀 게 없으면 그때 포기한다.
            let loosened = policy.loosened();
            if loosened != policy {
                self.log.info(
                    "Autoplay",
                    &format!(
                        "정책 {}로는 후보가 없어서 {}로 풀어서 다시 골라요.",
                        policy.as_str(),
                        loosened.as_str()
                    ),
                );
                policy = loosened;
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
        self.log.warn(
            "Autoplay",
            &format!(
                "차단 규칙과 길이 제한 때문에 {MAX_ATTEMPTS}번 안에 쓸 만한 추천을 못 찾았어요."
            ),
        );
        None
    }
}

/// 최근 재생 이력 회피 강도 (§8.5-2). `1.0`이면 회피 없음, `0.0`이면 사실상 금지.
/// 하루 지난 곡은 다시 나와도 괜찮으니 `decay_hours` 를 넘으면 회피가 완전히 풀린다.
///
/// **`decay_hours == 0` 은 "회피 창이 무제한"이다** (§23.1 `0 = 무제한`).
/// 이 값은 회피가 *풀리는 데 걸리는 시간*이라, 무제한이면 영원히 안 풀린다 —
/// `auditRetentionDays = 0` 이 "영원히 안 지움"인 것과 같은 방향이고,
/// 관리 콘솔도 `무제한 · 한 번 튼 곡은 계속 피해요` 라고 같은 말을 한다.
/// (횟수 계열인 `autoplay_artist_cooldown` 의 `0` 은 반대로 "끔"이다 — 그쪽은 *막을 곡 수*라
/// 0이면 아무도 안 막는다. 두 값의 `0` 이 다른 방향인 건 세는 대상이 달라서다.)
fn decay_factor(age_hours: Option<f64>, decay_hours: u32) -> f64 {
    let Some(age) = age_hours else {
        return 1.0; // 최근에 튼 적이 없는 곡.
    };
    if decay_hours == 0 {
        return 0.0;
    }
    (age / decay_hours as f64).clamp(0.0, 1.0)
}

/// 정책별 기본 가중치. `rank` 는 살아남은 후보 안에서의 순위(0-based)다.
fn policy_weight(policy: AutoplayPolicy, rank: usize, track: &TrackRef) -> f64 {
    match policy {
        // 상위 3곡 중 균등 — 시드와 제일 비슷하게.
        AutoplayPolicy::Similar => 1.0,
        // 상위 10곡 중 앞쪽이 더 잘 뽑히게.
        AutoplayPolicy::Balanced => (10 - rank.min(9)) as f64,
        // 전체 균등.
        AutoplayPolicy::Explore => 1.0,
        // 조회수는 flat-playlist 에서 안 오므로 **길이**로 "무난함"을 본다.
        // 2~7분이면 노래일 확률이 높고, 앞 순위일수록 조금 더 잘 뽑힌다.
        AutoplayPolicy::Popular => {
            let length_bonus = match track.duration.map(|d| d.as_secs_f64()) {
                Some(secs) if secs >= POPULAR_SECS.0 && secs <= POPULAR_SECS.1 => 3.0,
                Some(_) => 0.5,
                None => 1.0,
            };
            length_bonus * (1.0 + 1.0 / (rank as f64 + 1.0))
        }
    }
}

/// 가중 무작위. 가중치 합이 0이면 `None`.
fn weighted_pick(weighted: &[(usize, f64)], rng: &mut DeterministicRng) -> Option<usize> {
    let total: f64 = weighted.iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        return None;
    }
    let mut roll = rng.next_f64() * total;
    for (index, weight) in weighted {
        roll -= weight;
        if roll <= 0.0 {
            return Some(*index);
        }
    }
    weighted.last().map(|(index, _)| *index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CsTimeSpan, ProviderKind};

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

    fn track_by(id: &str, artist: &str) -> TrackRef {
        TrackRef {
            artist: Some(artist.into()),
            ..track(id)
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

    /// **무작위는 재현 가능해야 한다** (§8.5). 같은 (길드·시드·후보수·시간칸)이면 같은 답이 나오고,
    /// 하나라도 다르면 달라진다. 안 그러면 왜 그 곡이 나왔는지 추적을 못 한다.
    #[test]
    fn the_random_pick_is_reproducible() {
        let sample = |guild: u64, seed: &str, count: usize, slot: i64| {
            let mut rng = DeterministicRng::new(guild, seed, count, slot);
            (0..5).map(|_| rng.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(sample(1, "youtube:aaa", 20, 100), sample(1, "youtube:aaa", 20, 100));
        assert_ne!(sample(1, "youtube:aaa", 20, 100), sample(2, "youtube:aaa", 20, 100));
        assert_ne!(sample(1, "youtube:aaa", 20, 100), sample(1, "youtube:bbb", 20, 100));
        assert_ne!(sample(1, "youtube:aaa", 20, 100), sample(1, "youtube:aaa", 21, 100));
        assert_ne!(sample(1, "youtube:aaa", 20, 100), sample(1, "youtube:aaa", 20, 101));

        // 0.0 ~ 1.0 을 벗어나면 가중 무작위가 통째로 망가진다.
        let mut rng = DeterministicRng::new(7, "seed", 3, 9);
        for _ in 0..200 {
            let value = rng.next_f64();
            assert!((0.0..1.0).contains(&value), "{value} 가 범위를 벗어났다");
        }
    }

    /// 하루 지난 곡은 회피가 완전히 풀리고, 방금 튼 곡은 사실상 안 나온다 (§8.5-2).
    #[test]
    fn recent_history_fades_instead_of_banning_forever() {
        assert_eq!(decay_factor(None, 24), 1.0);
        assert_eq!(decay_factor(Some(0.0), 24), 0.0);
        assert!(decay_factor(Some(6.0), 24) - 0.25 < 1e-9);
        assert_eq!(decay_factor(Some(24.0), 24), 1.0);
        assert_eq!(decay_factor(Some(100.0), 24), 1.0);
        // 감쇠를 끄면 옛 동작 — 최근 목록에 있으면 그냥 제외.
        assert_eq!(decay_factor(Some(100.0), 0), 0.0);
    }

    /// 정책 창과 가중치: `similar` 은 앞 3곡만, `balanced` 는 앞쪽이 더 무겁다.
    #[test]
    fn policy_weights_follow_the_spec_table() {
        assert!(policy_weight(AutoplayPolicy::Balanced, 0, &track("a")) > policy_weight(AutoplayPolicy::Balanced, 9, &track("a")));
        assert_eq!(policy_weight(AutoplayPolicy::Explore, 0, &track("a")), 1.0);
        assert_eq!(policy_weight(AutoplayPolicy::Similar, 2, &track("a")), 1.0);

        // popular 은 2~7분 곡을 확실히 선호한다.
        let song = TrackRef {
            duration: Some(CsTimeSpan::from_secs_f64(210.0)),
            ..track("노래")
        };
        let long_mix = TrackRef {
            duration: Some(CsTimeSpan::from_secs_f64(590.0)),
            ..track("믹스")
        };
        assert!(
            policy_weight(AutoplayPolicy::Popular, 0, &song)
                > policy_weight(AutoplayPolicy::Popular, 0, &long_mix)
        );
    }

    /// 가중 무작위가 가중치를 실제로 따르는지. 0 가중치는 절대 안 뽑혀야 한다.
    #[test]
    fn weighted_pick_respects_the_weights() {
        let weighted = vec![(0usize, 0.0), (1, 1.0)];
        let mut rng = DeterministicRng::new(1, "s", 2, 0);
        for _ in 0..50 {
            assert_eq!(weighted_pick(&weighted, &mut rng), Some(1));
        }
        assert!(weighted_pick(&[], &mut rng).is_none());
        assert!(weighted_pick(&[(0, 0.0)], &mut rng).is_none());
    }

    /// 아티스트 쿨다운은 **최근 N곡 안**의 가수만 막는다. N을 넘어선 가수는 다시 나올 수 있다.
    #[test]
    fn artist_cooldown_only_covers_the_recent_window() {
        let recent = ["아이브".to_string(), "뉴진스".to_string(), "에스파".to_string()];
        let blocked: HashSet<&str> = recent.iter().take(2).map(String::as_str).collect();
        assert!(blocked.contains(artist_key(&track_by("a", "아이브")).unwrap().as_str()));
        assert!(blocked.contains(artist_key(&track_by("b", " 뉴진스 ")).unwrap().as_str()));
        assert!(!blocked.contains(artist_key(&track_by("c", "에스파")).unwrap().as_str()));
        // 아티스트가 없는 곡은 쿨다운에 걸리지 않는다.
        assert!(artist_key(&track("d")).is_none());
    }
}
