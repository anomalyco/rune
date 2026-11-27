use std::sync::Arc;

use crate::{ImageCache, ImageLimits, MAX_ALLOC, MAX_PIXELS, MAX_SIDE};

#[test]
fn image_limits_clamps_above_global_caps_and_computes_rav1d_limit() {
    let def = ImageLimits::default();
    assert_eq!(def, ImageLimits::new());
    assert_eq!(def.max_side(), MAX_SIDE);
    assert_eq!(def.max_pixels(), MAX_PIXELS);
    assert_eq!(def.max_alloc(), MAX_ALLOC);
    assert!(def.is_default());
    assert_eq!(def.rav1d_frame_size_limit(), Some(MAX_SIDE * MAX_SIDE));

    // Values above global caps clamp down.
    let clamped = ImageLimits::new()
        .with_max_side(u32::MAX)
        .with_max_pixels(u64::MAX)
        .with_max_alloc(u64::MAX);
    assert_eq!(clamped, def);

    // Effective pixel budget takes the minimum of max_pixels, max_side^2, and max_alloc / 4.
    assert_eq!(ImageLimits::new().with_max_side(100).rav1d_frame_size_limit(), Some(10_000));
    assert_eq!(ImageLimits::new().with_max_pixels(500).rav1d_frame_size_limit(), Some(500));
    assert_eq!(ImageLimits::new().with_max_alloc(1_000).rav1d_frame_size_limit(), Some(250));

    // Zero effective pixel budget returns None so 0 is never passed to rav1d (where 0 = unlimited).
    assert_eq!(ImageLimits::new().with_max_side(0).rav1d_frame_size_limit(), None);
    assert_eq!(ImageLimits::new().with_max_pixels(0).rav1d_frame_size_limit(), None);
    assert_eq!(ImageLimits::new().with_max_alloc(3).rav1d_frame_size_limit(), None);
    assert_eq!(ImageLimits::new().with_max_alloc(4).rav1d_frame_size_limit(), Some(1));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn image_cache_load_with_limits_enforces_budgets_and_isolates_cache_keys() {
    let mut png_bytes = Vec::new();
    let img = image::RgbaImage::from_pixel(32, 16, image::Rgba([20, 40, 60, 255]));
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();

    let cache = ImageCache::new(Arc::new(|| {}));

    // Tighter side, pixel, and byte budgets reject before decoding.
    assert!(cache.load_with_limits(png_bytes.clone(), ImageLimits::new().with_max_side(31)).wait().is_err());
    assert!(cache.load_with_limits(png_bytes.clone(), ImageLimits::new().with_max_pixels(32 * 16 - 1)).wait().is_err());
    assert!(cache.load_with_limits(png_bytes.clone(), ImageLimits::new().with_max_alloc(32 * 16 * 4 - 1)).wait().is_err());

    // Exact budget and default load succeed on the same cache (no cross-contamination).
    let exact = ImageLimits::new().with_max_side(32).with_max_pixels(32 * 16).with_max_alloc(32 * 16 * 4);
    assert_eq!(cache.load_with_limits(png_bytes.clone(), exact).wait(), Ok([32, 16]));
    let full = cache.load(png_bytes.clone());
    let default_limited = cache.load_with_limits(png_bytes.clone(), ImageLimits::default());
    assert_eq!(full.wait(), Ok([32, 16]));
    assert_eq!(full.id(), default_limited.id(), "default ImageLimits shares cache key with load()");

    // load_scaled_with_limits rejects oversized sources and downsamples sources within budget.
    assert!(cache.load_scaled_with_limits(png_bytes.clone(), 8.0, 1.0, ImageLimits::new().with_max_pixels(100)).wait().is_err());
    assert_eq!(cache.load_scaled_with_limits(png_bytes, 8.0, 1.0, exact).wait(), Ok([8, 4]));

    // SVG also respects per-load ImageLimits.
    #[cfg(feature = "svg")]
    {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="#000"/></svg>"##;
        // Default load rasterizes SVG at 2x -> 32x16 = 512 px.
        assert!(cache.load_with_limits(&svg[..], ImageLimits::new().with_max_pixels(511)).wait().is_err());
        assert_eq!(cache.load_with_limits(&svg[..], ImageLimits::new().with_max_pixels(512)).wait(), Ok([32, 16]));
    }
}
