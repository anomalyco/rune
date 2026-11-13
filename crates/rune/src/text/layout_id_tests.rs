use super::*;

/// A layout built where a freed one stood is a new layout, whatever its address.
#[test]
fn an_address_stamped_again_never_brings_its_old_id_back() {
    let mut ids = LayoutIds::default();
    let first = ids.stamp(0x1000);
    let second = ids.stamp(0x1000);
    assert_ne!(first, second);
    assert_eq!(ids.by_address[&0x1000], second);
}

#[test]
fn stamps_are_bounded_and_keep_the_newest() {
    let mut ids = LayoutIds::default();
    let mut last = 0;
    for a in 0..MAX_STAMPED as usize + 100 {
        last = ids.stamp(a);
    }
    assert!(ids.by_address.len() <= MAX_STAMPED);
    assert_eq!(ids.by_address[&(MAX_STAMPED as usize + 99)], last);
}
