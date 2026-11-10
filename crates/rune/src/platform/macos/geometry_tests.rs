use super::*;

#[test]
fn overlay_appkit_top_row_belongs_to_secondary_display() {
    let secondary = NSRect::new(NSPoint::new(-1920.0, 0.0), NSSize::new(1920.0, 1080.0));
    assert!(contains_appkit_rect(secondary, NSPoint::new(-100.0, 1080.0)));
    assert!(!contains_appkit_rect(secondary, NSPoint::new(-100.0, 0.0)));
    assert!(!contains_appkit_rect(secondary, NSPoint::new(0.0, 500.0)));
    let below = NSRect::new(NSPoint::new(-1920.0, -1080.0), NSSize::new(1920.0, 1080.0));
    assert!(contains_appkit_rect(below, NSPoint::new(-100.0, 0.0)));
}
