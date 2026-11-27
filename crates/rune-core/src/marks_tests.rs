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

#[test]
fn test_scope_isolates_presented_and_on_display_marks_per_thread() {
    let earlier = crate::time::now() - Duration::from_millis(25);
    let later = earlier + Duration::from_millis(10);

    {
        let _scope = test_scope();
        assert!(counting());
        assert!(marks().is_empty());

        on_display(|| earlier);
        on_display(|| later);
        assert_eq!(
            marks().into_iter().find(|(name, _)| *name == "frame.first.on_display").map(|(_, at)| at),
            Some(earlier),
        );

        assert!(presented());
        assert!(!counting());
        loop_waits();
        assert!(marks().iter().any(|(name, _)| *name == "frame.first.presented"));
        assert!(marks().iter().any(|(name, _)| *name == "input.ready"));
    }

    // A second scope starts clean even after an earlier scope recorded every launch mark.
    {
        let _scope = test_scope();
        assert!(counting());
        assert!(marks().is_empty());
    }
}
