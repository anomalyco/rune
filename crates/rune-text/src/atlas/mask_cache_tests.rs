use super::*;

static ICON: Icon = Icon {
    name: "corner",
    view: 16.0,
    body: r#"<path d="M2 2h9v4H6v8H2z" fill="currentColor"/>"#,
};

const ASKS: [((u32, f32), Facing); 3] = [((18, 18.0), Facing::Upright), ((18, 18.0), Facing::Mirrored), ((17, 16.5), Facing::Upright)];

/// Each of [`ASKS`]' slots as the atlas holds it, border included, in the order asked.
fn slots(text: &mut TextSystem) -> Vec<Vec<u8>> {
    ASKS.iter().map(|&(size, facing)| {
        let before = text.atlas.icons.len();
        assert!(text.icon_quad_sized(&ICON, size, [0.0, 0.0], facing).is_some());
        assert_eq!(text.atlas.icons.len(), before + 1, "each ask is an atlas entry of its own");
        let short = ((size.0 as f32 - size.1) * 64.0).round() as u32;
        let key = (&ICON as *const Icon as usize, size.0 | (short << 24) | (u32::from(facing == Facing::Mirrored) << 31));
        let g = text.atlas.icons[&key].expect("placed");
        let page = text.atlas.page_data(g.page as usize).expect("its page");
        (0..g.h).flat_map(|row| {
            let at = ((g.y + row) * ATLAS_SIZE + g.x) as usize;
            page[at..at + g.w as usize].iter().copied()
        }).collect()
    }).collect()
}

/// The icons a launch drew are in what it saves, and the next launch's atlas holds the same
/// texels for them: upright, mirrored and at a fractional size.
#[test]
fn a_saved_icon_is_placed_texel_for_texel() {
    let mut first = TextSystem::default();
    let drawn = slots(&mut first);
    assert_ne!(drawn[0], drawn[1], "the mirrored drawing differs, or the test proves nothing about facing");
    let cache = decode(&first.mask_snapshot(7).encode(), Some(7)).expect("this build's cache");
    assert_eq!(cache.len(), ASKS.len());
    let mut second = TextSystem::default();
    second.masks.offered = Some(Arc::new(cache));
    assert_eq!(slots(&mut second), drawn);
    // And what the second launch saves still holds them (they are its masks in use).
    let again = decode(&second.mask_snapshot(7).encode(), Some(7)).expect("this build's cache");
    assert_eq!(again.len(), ASKS.len());
}

/// The texels come from the cache, not from a second drawing: a cache whose bytes are changed
/// shows through.
#[test]
fn a_cached_icon_is_not_drawn_again() {
    let mut first = TextSystem::default();
    let drawn = slots(&mut first);
    let mut cache = decode(&first.mask_snapshot(7).encode(), Some(7)).expect("this build's cache");
    cache.data.fill(0x5a);
    let mut second = TextSystem::default();
    second.masks.offered = Some(Arc::new(cache));
    let placed = slots(&mut second);
    for (placed, drawn) in placed.iter().zip(&drawn) {
        assert_eq!(placed.len(), drawn.len());
        assert_eq!(placed.iter().filter(|&&t| t == 0x5a).count(), placed.len() - border(placed.len()));
    }
}

/// Texels in a square slot's one-texel border.
fn border(texels: usize) -> usize {
    let side = texels.isqrt();
    4 * side - 4
}

/// Another build's cache (another link stamp) is no cache, and an icon whose saved mask is of
/// another canvas is drawn afresh.
#[test]
fn another_builds_icons_are_drawn_afresh() {
    let mut first = TextSystem::default();
    let drawn = slots(&mut first);
    let bytes = first.mask_snapshot(7).encode();
    assert!(decode(&bytes, Some(8)).is_none());
    let mut cache = decode(&bytes, Some(7)).expect("this build's cache");
    cache.data.fill(0x5a);
    for entry in cache.entries.values_mut() { entry.w -= 1; }
    let mut second = TextSystem::default();
    second.masks.offered = Some(Arc::new(cache));
    assert_eq!(slots(&mut second), drawn);
}