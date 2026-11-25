use crate::{TextStyle, TextSystem};

#[test]
fn rest_evicts_previous_commit_layouts_while_active_keeps_two_commit_grace() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);

    // Commit 0: shape 500 distinct paragraphs (large document).
    for i in 0..500 {
        let s = format!("Paragraph {i}: verifying active two-commit grace and rest-time eviction.");
        let _ = text.layout_rich(&s, &[], style, 1.0, Some(600.0));
    }
    text.end_commit();
    assert_eq!(text.cached_layout_count(), 500);

    // Commit 1: replace with one small label (start page).
    let _ = text.layout_rich("Open a Markdown file", &[], style, 1.0, Some(600.0));
    text.end_commit();

    // While active (no rest), the two-commit grace keeps the previous view's 500 layouts warm.
    assert_eq!(text.cached_layout_count(), 501, "two-commit grace stays active before rest");

    // Once the window rests, layouts unused by the last commit are evicted and tables shrink.
    text.rest();
    assert_eq!(text.cached_layout_count(), 1, "rest keeps only the last commit's layout");
    assert!(text.layouts.capacity() <= 128, "layouts bucket table shrank: {}", text.layouts.capacity());

    // Calling rest again is a no-op and keeps the current screen's layout warm.
    let cap_after_first = text.layouts.capacity();
    text.rest();
    assert_eq!(text.cached_layout_count(), 1);
    assert_eq!(text.layouts.capacity(), cap_after_first);
}
