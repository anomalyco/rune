use std::time::Duration;

use super::*;

#[test]
fn mark_at_records_supplied_timestamp_and_keeps_first_wins() {
    let earlier = crate::time::now() - Duration::from_millis(42);
    let later = earlier + Duration::from_millis(20);

    mark_at("test.mark_at.first_wins", earlier);
    mark_at("test.mark_at.first_wins", later);

    let all = marks();
    let entries: Vec<_> = all
        .iter()
        .filter(|(name, _)| *name == "test.mark_at.first_wins")
        .collect();
    assert_eq!(entries.len(), 1, "mark_at must record each name at most once");
    assert_eq!(entries[0].1, earlier, "mark_at must preserve the exact supplied timestamp");
}
