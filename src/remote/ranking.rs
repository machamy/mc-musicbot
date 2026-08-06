use super::{QueueScore, QueueSortMode, VotePoints};
use crate::models::{PlaybackRequestKind, QueueItem};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// 정렬을 한 곳에 고정한다. 수동 우선순위는 어느 모드에서든 관리자의 명시적 예외다.
///
/// 점수는 서버가 정한 점수표(`points`, §10.1)로 계산한다 — 여기에 배수를 다시 박으면
/// 화면과 정렬이 서로 다른 말을 하게 된다.
pub fn sort_queue(
    items: &mut [QueueItem],
    scores: &HashMap<String, QueueScore>,
    mode: QueueSortMode,
    points: &VotePoints,
) {
    match mode {
        QueueSortMode::Score => {
            items.sort_by(|left, right| compare_score(left, right, scores, points))
        }
        QueueSortMode::Fifo => items.sort_by(|left, right| compare_fifo(left, right, scores)),
        QueueSortMode::Fair => {
            // 라운드는 항목 쌍이 아니라 전체 대기열을 봐야 정해지므로 비교 전에 한 번만 계산한다.
            let rounds = request_rounds(items, scores);
            items.sort_by(|left, right| compare_fair(left, right, scores, &rounds));
        }
    }
}

/// 사람별로 자기 곡을 `original_order` 순으로 줄 세워 0-based 라운드를 매긴다.
/// 공평제 정렬에도 쓰고, "누구의 몇 번째 곡" 표시에도 그대로 쓴다.
pub fn request_rounds(
    items: &[QueueItem],
    scores: &HashMap<String, QueueScore>,
) -> HashMap<String, i32> {
    let mut ordered: Vec<(&str, Option<u64>, i64)> = items
        .iter()
        .map(|item| {
            (
                item.id.as_str(),
                requester_of(item, scores),
                original_order(&item.id, scores),
            )
        })
        .collect();
    ordered.sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(right.0)));

    let mut next_round: HashMap<Option<u64>, i32> = HashMap::new();
    let mut rounds = HashMap::with_capacity(ordered.len());
    for (item_id, requester, _) in ordered {
        let slot = next_round.entry(requester).or_insert(0);
        rounds.insert(item_id.to_string(), *slot);
        *slot += 1;
    }
    rounds
}

/// 계산한 라운드를 점수 행에 반영한다. 응답 JSON에 그대로 실어 보내기 위한 편의 함수.
pub fn apply_rounds(items: &[QueueItem], scores: &mut HashMap<String, QueueScore>) {
    let rounds = request_rounds(items, scores);
    for (item_id, round) in rounds {
        if let Some(score) = scores.get_mut(&item_id) {
            score.round = round;
        }
    }
}

fn compare_score(
    left: &QueueItem,
    right: &QueueItem,
    scores: &HashMap<String, QueueScore>,
    points: &VotePoints,
) -> Ordering {
    compare_manual(left, right, scores)
        .then_with(|| {
            let total = |item: &QueueItem| {
                scores
                    .get(&item.id)
                    .map(|score| score.total_score(points))
                    .unwrap_or(0)
            };
            total(right).cmp(&total(left))
        })
        .then_with(|| compare_tail(left, right, scores))
}

fn compare_fifo(
    left: &QueueItem,
    right: &QueueItem,
    scores: &HashMap<String, QueueScore>,
) -> Ordering {
    compare_manual(left, right, scores).then_with(|| compare_tail(left, right, scores))
}

fn compare_fair(
    left: &QueueItem,
    right: &QueueItem,
    scores: &HashMap<String, QueueScore>,
    rounds: &HashMap<String, i32>,
) -> Ordering {
    compare_manual(left, right, scores)
        .then_with(|| {
            let left_round = rounds.get(&left.id).copied().unwrap_or(i32::MAX);
            let right_round = rounds.get(&right.id).copied().unwrap_or(i32::MAX);
            left_round.cmp(&right_round)
        })
        .then_with(|| {
            // 아직 한 곡도 못 튼 사람이 가장 오래 기다린 사람이다 — 빈 문자열이 제일 앞에 온다.
            let left_played = last_played(&left.id, scores);
            let right_played = last_played(&right.id, scores);
            left_played.cmp(right_played)
        })
        .then_with(|| compare_tail(left, right, scores))
}

/// 우선순위 없음(`None`)은 **0점**이다. `Option<i32>` 를 그대로 비교하면 Rust 의
/// `None < Some(_)` 때문에 붐따가 준 음수 우선순위(`Some(-1_000_000)`, §10.3 `Bottom`)가
/// 우선순위 없는 곡보다 **위**로 올라온다 — "맨 뒤로"가 "맨 앞으로"가 되는 사고다.
/// 핀(양수) > 보통(0) > 붐따(음수) 순이 되도록 여기서 한 번에 푼다.
fn manual_rank(item: &QueueItem, scores: &HashMap<String, QueueScore>) -> i32 {
    scores
        .get(&item.id)
        .and_then(|score| score.manual_priority)
        .unwrap_or(0)
}

fn compare_manual(
    left: &QueueItem,
    right: &QueueItem,
    scores: &HashMap<String, QueueScore>,
) -> Ordering {
    manual_rank(right, scores).cmp(&manual_rank(left, scores))
}

/// 모든 모드가 공유하는 마지막 결정자: 등록순 → id.
fn compare_tail(
    left: &QueueItem,
    right: &QueueItem,
    scores: &HashMap<String, QueueScore>,
) -> Ordering {
    original_order(&left.id, scores)
        .cmp(&original_order(&right.id, scores))
        .then_with(|| left.id.cmp(&right.id))
}

fn original_order(item_id: &str, scores: &HashMap<String, QueueScore>) -> i64 {
    scores
        .get(item_id)
        .map(|score| score.original_order)
        .unwrap_or(i64::MAX)
}

fn last_played<'a>(item_id: &str, scores: &'a HashMap<String, QueueScore>) -> &'a str {
    scores
        .get(item_id)
        .and_then(|score| score.last_played_utc.as_deref())
        .unwrap_or("")
}

/// 점수 행의 신청자를 우선하고, 없으면 큐 항목 자신의 신청자를 쓴다.
fn requester_of(item: &QueueItem, scores: &HashMap<String, QueueScore>) -> Option<u64> {
    scores
        .get(&item.id)
        .and_then(|score| score.requester_user_id)
        .or(item.requested_by_user_id)
}

/// 현재 정렬에서 요청자별 가장 위의 사용자 요청 한 곡만 대기 점수 증가 대상으로 고른다.
/// 점수제 전용 — 공평제는 대기 점수를 순서에 쓰지 않는다.
pub fn wait_score_targets(items: &[QueueItem]) -> Vec<String> {
    let mut seen_requesters = HashSet::new();
    let mut targets = Vec::new();
    for item in items {
        if item.request_kind != PlaybackRequestKind::User {
            continue;
        }
        let Some(user_id) = item.requested_by_user_id else {
            continue;
        };
        if seen_requesters.insert(user_id) {
            targets.push(item.id.clone());
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ProviderKind, QueueItem, TrackRef};

    fn item(id: &str, user_id: u64) -> QueueItem {
        let mut item = QueueItem::new_user(
            TrackRef {
                provider: ProviderKind::YouTube,
                content_id: id.into(),
                source_url: format!("https://example.test/{id}"),
                title: Some(id.into()),
                artist: None,
                duration: None,
                variant_key: None,
            },
            format!("user-{user_id}"),
            Some(user_id),
        );
        item.id = id.into();
        item
    }

    fn score(id: &str, wait: i32, likes: i32, supers: i32, order: i64) -> QueueScore {
        QueueScore {
            item_id: id.into(),
            guild_id: 1,
            wait_score: wait,
            like_count: likes,
            super_like_count: supers,
            original_order: order,
            ..Default::default()
        }
    }

    /// 기본 점수표 (👍1 · ⭐2 · 대기1). 테스트가 서버 설정과 같은 값을 쓰는지 한 곳에서 본다.
    fn points() -> VotePoints {
        VotePoints::default()
    }

    fn ids(items: &[QueueItem]) -> Vec<&str> {
        items.iter().map(|item| item.id.as_str()).collect()
    }

    /// 민수(1) 3곡 · 지훈(2) 1곡 · 수연(3) 1곡. 세 모드가 공유하는 시나리오.
    fn three_people() -> (Vec<QueueItem>, HashMap<String, QueueScore>) {
        let items = vec![
            item("민수1", 1),
            item("민수2", 1),
            item("민수3", 1),
            item("지훈1", 2),
            item("수연1", 3),
        ];
        let scores = HashMap::from([
            ("민수1".into(), score("민수1", 2, 0, 0, 0)),
            ("민수2".into(), score("민수2", 0, 0, 0, 1)),
            ("민수3".into(), score("민수3", 0, 1, 0, 2)),
            ("지훈1".into(), score("지훈1", 1, 1, 0, 3)),
            ("수연1".into(), score("수연1", 0, 0, 2, 4)),
        ]);
        (items, scores)
    }

    #[test]
    fn score_mode_ranks_by_total_then_registration_order() {
        let (mut items, scores) = three_people();
        sort_queue(&mut items, &scores, QueueSortMode::Score, &points());
        // 수연1=4점, 민수1=2점·등록0, 지훈1=2점·등록3, 민수3=1점, 민수2=0점
        assert_eq!(ids(&items), vec!["수연1", "민수1", "지훈1", "민수3", "민수2"]);
    }

    #[test]
    fn fifo_mode_ignores_votes_entirely() {
        let (mut items, scores) = three_people();
        sort_queue(&mut items, &scores, QueueSortMode::Fifo, &points());
        assert_eq!(ids(&items), vec!["민수1", "민수2", "민수3", "지훈1", "수연1"]);
    }

    #[test]
    fn fair_mode_gives_everyone_a_turn_before_seconds() {
        let (mut items, mut scores) = three_people();
        // 민수는 방금 한 곡 재생됐고, 지훈·수연은 아직 한 곡도 못 틀었다.
        for id in ["민수1", "민수2", "민수3"] {
            scores.get_mut(id).unwrap().last_played_utc = Some("2026-08-06T10:00:00+00:00".into());
        }
        sort_queue(&mut items, &scores, QueueSortMode::Fair, &points());
        // 1라운드: 지훈1·수연1(미재생) → 민수1. 그 다음에야 민수의 2·3번째 곡.
        assert_eq!(ids(&items), vec!["지훈1", "수연1", "민수1", "민수2", "민수3"]);
    }

    #[test]
    fn fair_mode_rounds_are_per_person_and_zero_based() {
        let (items, scores) = three_people();
        let rounds = request_rounds(&items, &scores);
        assert_eq!(rounds["민수1"], 0);
        assert_eq!(rounds["민수2"], 1);
        assert_eq!(rounds["민수3"], 2);
        assert_eq!(rounds["지훈1"], 0);
        assert_eq!(rounds["수연1"], 0);
    }

    #[test]
    fn apply_rounds_writes_back_into_scores() {
        let (items, mut scores) = three_people();
        apply_rounds(&items, &mut scores);
        assert_eq!(scores["민수3"].round, 2);
        assert_eq!(scores["수연1"].round, 0);
    }

    #[test]
    fn only_top_item_per_requester_ages() {
        let items = vec![item("a1", 1), item("b1", 2), item("a2", 1)];
        assert_eq!(wait_score_targets(&items), vec!["a1", "b1"]);
    }

    #[test]
    fn manual_priority_is_an_explicit_override_in_every_mode() {
        let mut popular = score("popular", 20, 5, 5, 0);
        popular.manual_priority = None;
        let mut forced = score("forced", 0, 0, 0, 1);
        forced.manual_priority = Some(1);
        let scores = HashMap::from([("popular".into(), popular), ("forced".into(), forced)]);
        for mode in [QueueSortMode::Score, QueueSortMode::Fifo, QueueSortMode::Fair] {
            let mut items = vec![item("popular", 1), item("forced", 2)];
            sort_queue(&mut items, &scores, mode, &points());
            assert_eq!(items[0].id, "forced", "{mode:?} 모드에서 수동 우선순위가 무시됨");
        }
    }

    /// 붐따 `Bottom`(§10.3)이 준 **음수** 우선순위는 꼬리로 가야 한다.
    /// `Option<i32>` 를 그대로 비교하면 `None < Some(-1_000_000)` 이라 미움받은 곡이
    /// 1번으로 올라오는 정반대 동작이 된다 — 세 모드 전부에서 막는다.
    #[test]
    fn boomtta_negative_priority_sinks_to_the_bottom_in_every_mode() {
        let mut pinned = score("pinned", 0, 0, 0, 0);
        pinned.manual_priority = Some(1_000_000);
        let normal = score("normal", 0, 0, 0, 1);
        let mut boomtta = score("boomtta", 30, 9, 9, 2); // 점수는 제일 높지만 붐따당했다.
        boomtta.manual_priority = Some(-1_000_000);
        let scores = HashMap::from([
            ("pinned".into(), pinned),
            ("normal".into(), normal),
            ("boomtta".into(), boomtta),
        ]);
        for mode in [QueueSortMode::Score, QueueSortMode::Fifo, QueueSortMode::Fair] {
            let mut items = vec![item("boomtta", 1), item("pinned", 2), item("normal", 3)];
            sort_queue(&mut items, &scores, mode, &points());
            assert_eq!(
                ids(&items),
                vec!["pinned", "normal", "boomtta"],
                "{mode:?} 모드에서 붐따 곡이 꼬리로 안 갔다"
            );
        }
    }

    /// 우선순위 없음은 0점이다 — 핀(양수) > 보통(없음) > 붐따(음수).
    #[test]
    fn missing_manual_priority_ranks_as_zero() {
        let mut sunk = score("sunk", 0, 0, 0, 0);
        sunk.manual_priority = Some(-1);
        let plain = score("plain", 0, 0, 0, 1);
        let mut lifted = score("lifted", 0, 0, 0, 2);
        lifted.manual_priority = Some(1);
        let scores = HashMap::from([
            ("sunk".into(), sunk),
            ("plain".into(), plain),
            ("lifted".into(), lifted),
        ]);
        let mut items = vec![item("sunk", 1), item("plain", 2), item("lifted", 3)];
        sort_queue(&mut items, &scores, QueueSortMode::Fifo, &points());
        assert_eq!(ids(&items), vec!["lifted", "plain", "sunk"]);
    }
}
