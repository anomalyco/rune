use super::*;

/// A layout built where a freed one stood is a new layout, whatever its address.
#[test]
fn an_address_stamped_again_never_brings_its_old_id_back() {
    let mut ids = LayoutIds::default();
    let first = ids.stamp(0x1000);
    let second = ids.stamp(0x1000);
    assert_ne!(first, second);
    assert_eq!(ids.of_at(0x1000), Some(second));
}

#[test]
fn stamps_are_bounded_and_keep_the_newest() {
    let mut ids = LayoutIds::default();
    let mut last = 0;
    for a in 0..MAX_STAMPED as usize + 100 {
        last = ids.stamp(a);
    }
    assert!(ids.by_address.len() <= MAX_STAMPED);
    assert_eq!(ids.of_at(MAX_STAMPED as usize + 99), Some(last));
}

/// A lookup never stamps: it adds nothing, and neither a miss nor a hit moves any other id.
#[test]
fn a_lookup_never_stamps_a_layout() {
    let mut ids = LayoutIds::default();
    let id = ids.stamp(0x2000);
    let before = (ids.by_address.len(), ids.of_at(0x2000));
    assert_eq!(ids.of_at(0x2001), None);
    assert_eq!(ids.of_at(0x2000), Some(id));
    assert_eq!((ids.by_address.len(), ids.of_at(0x2000)), before);
}

/// Past the bound, repeated lookups across every still layout leave the table as it was: the
/// kept ids stay, and none is stamped again or pushed out.
#[test]
fn lookups_at_capacity_neither_evict_nor_restamp() {
    let mut ids = LayoutIds::default();
    for a in 0..MAX_STAMPED as usize + 1 {
        ids.stamp(a);
    }
    let kept = |ids: &LayoutIds| -> Vec<Option<LayoutId>> { (0..MAX_STAMPED as usize + 1).map(|a| ids.of_at(a)).collect() };
    assert!(kept(&ids).iter().filter(|id| id.is_some()).count() >= MAX_STAMPED / 2);
    let before = kept(&ids);
    for _ in 0..8 {
        for a in 0..MAX_STAMPED as usize + 1 {
            let _ = ids.of_at(a);
        }
    }
    assert_eq!(ids.by_address.len(), before.iter().filter(|id| id.is_some()).count());
    assert!(kept(&ids) == before, "a lookup never stamps or evicts");
}
