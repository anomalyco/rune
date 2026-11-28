use std::path::PathBuf;
use std::sync::Arc;

use parley::fontique::Blob;

use super::{BlobCache, load_blob, preload_paths, try_cached_blob, try_cached_paths};

fn bundled_font(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rune/assets").join(name)
}

#[test]
fn blob_cache_bounds_entries_and_bytes() {
    let mut cache = BlobCache::new(2, 1024);
    let a = PathBuf::from("a.ttf");
    let b = PathBuf::from("b.ttf");
    let c = PathBuf::from("c.ttf");
    let small = Blob::new(Arc::new(vec![1u8; 100]));

    cache.insert(a.clone(), Some(small.clone()));
    cache.insert(b.clone(), Some(small.clone()));
    assert_eq!(cache.len(), 2);
    assert!(cache.get(&a).is_some());
    assert!(cache.get(&b).is_some());

    // Third entry evicts oldest (a) to respect max_entries.
    cache.insert(c.clone(), Some(small.clone()));
    assert_eq!(cache.len(), 2);
    assert!(cache.get(&a).is_none());
    assert!(cache.get(&b).is_some());
    assert!(cache.get(&c).is_some());

    // Large entry exceeding max_bytes is skipped rather than clearing valid entries.
    let huge = Blob::new(Arc::new(vec![2u8; 2048]));
    cache.insert(PathBuf::from("huge.ttf"), Some(huge));
    assert_eq!(cache.len(), 2);

    // Entry fitting within max_bytes evicts oldest entries until under byte budget.
    let big = Blob::new(Arc::new(vec![3u8; 950]));
    cache.insert(PathBuf::from("big.ttf"), Some(big));
    assert_eq!(cache.len(), 1);
    assert!(cache.bytes() <= 1024);
}

#[test]
fn blob_cache_loads_bundled_font_and_hits_memory_cache() {
    let path = bundled_font("Inter-Regular.ttf");
    let first = load_blob(&path).expect("bundled Inter-Regular.ttf must map");
    assert!(!first.as_ref().is_empty());

    let cached = try_cached_blob(&path).expect("path must now be cached").expect("cached blob");
    assert_eq!(cached.id(), first.id(), "cached lookup must return the same shared Blob handle");

    let missing = PathBuf::from("definitely-nonexistent-rune-font-file.ttf");
    assert!(load_blob(&missing).is_none());
    assert_eq!(try_cached_blob(&missing).map(|o| o.is_some()), Some(false));

    let preloaded = preload_paths([path.clone(), missing.clone()]);
    assert_eq!(preloaded.len(), 1);
    assert_eq!(preloaded[0].0, path);

    let all_cached = try_cached_paths(&[path.clone(), missing]).expect("both paths are cached");
    assert_eq!(all_cached.len(), 1);
    assert_eq!(all_cached[0].1.id(), first.id());
}
