use super::*;

/// Characters the macOS system UI font (`.AppleSystemUIFont`) already covers must skip
/// `public_family` so `SFNS.ttf`'s descriptors are never read and discarded.
#[test]
fn system_font_chars_skip_public_family_scan() {
    let cyrillic: Vec<char> = "абвгдежзийклмноп".chars().collect();
    let mut scans = 0usize;
    let files = fallback_family_files_with(&cyrillic, None, |font| {
        scans += 1;
        public_family(font)
    });
    assert!(files.is_empty(), "system-font chars should add no fallback files: {files:?}");
    assert_eq!(scans, 0, "system-font chars must not call public_family");
}

/// Multiple characters from the same hidden fallback family (e.g. Japanese
/// `.Hiragino Kaku Gothic Interface`) resolve its public family at most once per call.
#[test]
fn hidden_fallback_family_deduplicates_public_family_scan() {
    let japanese: Vec<char> = "あいうえおかきくけこ".chars().collect();
    let mut scans = 0usize;
    let files = fallback_family_files_with(&japanese, Some("ja"), |font| {
        scans += 1;
        public_family(font)
    });
    assert!(!files.is_empty(), "Japanese fallback should find system font files");
    assert!(scans <= 1, "expected at most 1 public_family scan for one hidden family, got {scans}");
    assert_eq!(files, fallback_family_files(&['あ'], Some("ja")));
}

/// `PublicFamilyCache` caches `Some` and `None` lookups while enforcing both its entry and
/// byte caps and ignoring duplicate or oversized entries.
#[test]
fn public_family_cache_bounds_entries_and_bytes() {
    let mut cache = PublicFamilyCache::new();
    cache.insert("/System/Library/Fonts/a.ttc".into(), Some("Hiragino Sans".into()));
    cache.insert("/System/Library/Fonts/b.ttf".into(), None);
    assert_eq!(cache.get("/System/Library/Fonts/a.ttc"), Some(Some("Hiragino Sans".into())));
    assert_eq!(cache.get("/System/Library/Fonts/b.ttf"), Some(None));
    assert_eq!(cache.get("/System/Library/Fonts/missing.ttf"), None);

    // Duplicate keys do not grow entries or bytes.
    let bytes_before = cache.bytes;
    let len_before = cache.entries.len();
    cache.insert("/System/Library/Fonts/a.ttc".into(), Some("Hiragino Sans".into()));
    assert_eq!(cache.entries.len(), len_before);
    assert_eq!(cache.bytes, bytes_before);

    // Oversized entries are skipped without evicting existing ones.
    let huge = "x".repeat(MAX_PUBLIC_FAMILY_BYTES + 1);
    cache.insert(huge.clone(), Some("Too Big".into()));
    assert_eq!(cache.get(&huge), None);
    assert_eq!(cache.entries.len(), len_before);

    // Entry cap evicts the oldest entries first.
    for i in 0..(MAX_PUBLIC_FAMILY_ENTRIES + 10) {
        cache.insert(format!("/f/{i}.ttf"), Some(format!("Family {i}")));
        assert!(cache.entries.len() <= MAX_PUBLIC_FAMILY_ENTRIES);
        assert!(cache.bytes <= MAX_PUBLIC_FAMILY_BYTES);
    }
    assert_eq!(cache.get("/System/Library/Fonts/a.ttc"), None);

    // Byte cap evicts older entries even when entry count is below the entry cap.
    let mut byte_cache = PublicFamilyCache::new();
    let big_val = "v".repeat(2048);
    for i in 0..20 {
        byte_cache.insert(format!("/big/{i}.ttc"), Some(big_val.clone()));
        assert!(byte_cache.entries.len() <= MAX_PUBLIC_FAMILY_ENTRIES);
        assert!(byte_cache.bytes <= MAX_PUBLIC_FAMILY_BYTES);
    }
    assert!(byte_cache.entries.len() < 20);
    assert_eq!(byte_cache.get("/big/0.ttc"), None);
    assert_eq!(byte_cache.get("/big/19.ttc"), Some(Some(big_val)));
}
