use super::QueueScore;
use crate::models::{PlaybackRequestKind, QueueItem};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// 점수·등록순 정렬을 한 곳에 고정한다. 수동 우선순위는 관리자의 명시적 예외다.
pub fn sort_queue(items: &mut [QueueItem], scores: &HashMap<String, QueueScore>) {
    items.sort_by(|left, right| compare_items(left, right, scores));
}

fn compare_items(
    left: &QueueItem,
    right: &QueueItem,
    scores: &HashMap<String, QueueScore>,
) -> Ordering {
    let left_score = scores.get(&left.id);
    let right_score = scores.get(&right.id);

    let left_manual = left_score.and_then(|score| score.manual_priority);
    let right_manual = right_score.and_then(|score| score.manual_priority);
    right_manual
        .cmp(&left_manual)
        .then_with(|| {
            let left_total = left_score.map(QueueScore::total_score).unwrap_or(0);
            let right_total = right_score.map(QueueScore::total_score).unwrap_or(0);
            right_total.cmp(&left_total)
        })
        .then_with(|| {
            let left_order = left_score
                .map(|score| score.original_order)
                .unwrap_or(i64::MAX);
            let right_order = right_score
                .map(|score| score.original_order)
                .unwrap_or(i64::MAX);
            left_order.cmp(&right_order)
        })
        .then_with(|| left.id.cmp(&right.id))
}

/// 현재 정렬에서 요청자별 가장 위의 사용자 요청 한 곡만 대기 점수 증가 대상으로 고른다.
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
            requester_user_id: None,
            wait_score: wait,
            like_count: likes,
            super_like_count: supers,
            manual_priority: None,
            original_order: order,
        }
    }

    #[test]
    fn score_then_registration_order_is_deterministic() {
        let mut items = vec![item("a", 1), item("b", 2), item("c", 3)];
        let scores = HashMap::from([
            ("a".into(), score("a", 1, 1, 0, 0)),
            ("b".into(), score("b", 1, 0, 1, 1)),
            ("c".into(), score("c", 2, 0, 0, 2)),
        ]);
        sort_queue(&mut items, &scores);
        assert_eq!(
            items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a", "c"]
        );
    }

    #[test]
    fn only_top_item_per_requester_ages() {
        let items = vec![item("a1", 1), item("b1", 2), item("a2", 1)];
        assert_eq!(wait_score_targets(&items), vec!["a1", "b1"]);
    }

    #[test]
    fn manual_priority_is_an_explicit_override() {
        let mut items = vec![item("popular", 1), item("forced", 2)];
        let mut popular = score("popular", 20, 5, 5, 0);
        let mut forced = score("forced", 0, 0, 0, 1);
        forced.manual_priority = Some(1);
        popular.manual_priority = None;
        let scores = HashMap::from([("popular".into(), popular), ("forced".into(), forced)]);
        sort_queue(&mut items, &scores);
        assert_eq!(items[0].id, "forced");
    }
}
