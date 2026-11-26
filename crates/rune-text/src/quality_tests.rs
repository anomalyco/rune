use super::*;
use rune_core::tok::OpticalSize;

#[test]
fn centred_wrapped_lines_centre_each_line_in_the_wrap_width() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let content = "a much longer first line of words that wraps\nshort";
    let width = 400.0;
    let start = text.layout_rich(content, &[], style, 2.0, Some(width));
    let centred = text.layout_aligned(content, &[], style, 2.0, Some(width), Alignment::Center);
    assert!(!Rc::ptr_eq(&start, &centred), "alignment is part of the cache key");
    let offsets = |l: &Layout<Ink>| {
        l.lines().map(|line| (line.metrics().offset, line.metrics().advance - line.metrics().hanging_advance)).collect::<Vec<_>>()
    };
    assert!(offsets(&start).iter().all(|(offset, _)| *offset == 0.0));
    let lines = offsets(&centred);
    assert!(lines.len() >= 2);
    for (offset, advance) in lines {
        assert!((offset - (width - advance) / 2.0).abs() < 1.0, "offset {offset} advance {advance}");
    }
}

/// Inline code between padding spaces in the body font draws one chip at the code's height,
/// with no separate sliver for each padding run.
#[test]
fn inline_code_and_its_padding_share_one_chip() {
    let mut text = TextSystem::default();
    let bg = Color::BLACK.alpha(0.08);
    let content = "say \u{202f}code\u{202f} now";
    let (a, b) = (content.find('c').unwrap(), content.find(" now").unwrap());
    let pad = |s: usize, e: usize| Span { bg: Some(bg), ..Span::new(s, e) };
    let code = Span { mono: true, size: Some(13.0 * 0.9), bg: Some(bg), ..Span::new(a, b - 3) };
    let spans = [pad(4, a), code.clone(), pad(b - 3, b)];
    let style = TextStyle::new(13.0, 400.0);
    let layout = text.layout_rich(content, &spans, style, 2.0, None);
    let mut decos = Vec::new();
    text.decorations(&layout, &spans, 2.0, [0.0, 0.0], &mut decos);
    assert_eq!(decos.len(), 1, "{:?}", decos.iter().map(|d| d.rect).collect::<Vec<_>>());
    let mut alone = Vec::new();
    let code_only = text.layout_rich(content, std::slice::from_ref(&code), style, 2.0, None);
    text.decorations(&code_only, std::slice::from_ref(&code), 2.0, [0.0, 0.0], &mut alone);
    assert_eq!((decos[0].rect[1], decos[0].rect[3]), (alone[0].rect[1], alone[0].rect[3]), "the code's height");
    assert!(decos[0].rect[2] > alone[0].rect[2], "padding widens it");
}

/// A pill's chip has room of its own: its label never wraps inside while it fits a line, no
/// other glyph or chip overlaps it at any such width, its icon fits inside it, and a caret at its edge sits outside.
#[test]
fn pill_chips_keep_room_and_never_overlap_at_wraps() {
    static DOT: Icon = Icon { name: "dot", view: 24.0, body: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>" };
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let content = "ask @code-reviewer about @src/main.rs and @notes today please";
    let pills: Vec<PillChip> = ["@code-reviewer", "@src/main.rs", "@notes"]
        .iter()
        .enumerate()
        .map(|(k, label)| {
            let start = content.find(label).unwrap() as u32;
            PillChip { start, end: start + label.len() as u32, icon: (k == 1).then_some(&DOT), image: None, bg: None, fg: None, ring: None, radius: None, bare: None }
        })
        .collect();
    let scale = 2.0;
    let bare = text.layout_blocks(content, &[], &[], &[], style, scale, None, Alignment::Start);
    let one = text.layout_blocks(content, &[], &[], &pills, style, scale, None, Alignment::Start);
    let pm = PillMetrics::new(14.0);
    let room = (pm.lead(false) + pm.lead(true) + pm.lead(false) + 3.0 * pm.trail()) * scale;
    assert!((one.full_width() - bare.full_width() - room).abs() < 4.0, "{} vs {}", one.full_width(), bare.full_width());

    let glyph_boxes = |layout: &Layout<Ink>| {
        let mut out = Vec::new();
        for (li, line) in layout.lines().enumerate() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let mut x = gr.offset();
                for cluster in gr.run().visual_clusters() {
                    let w = cluster.advance();
                    out.push((li, cluster.text_range(), x, x + w));
                    x += w;
                }
            }
        }
        out
    };
    for width in (123..=420).step_by(9) {
        let max = width as f32 * scale;
        let layout = text.layout_blocks(content, &[], &[], &pills, style, scale, Some(max), Alignment::Start);
        let (mut chips, mut icons) = (Vec::new(), Vec::new());
        pill_chips(&layout, &pills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut chips, &mut icons);
        assert_eq!(chips.len(), 3, "each pill draws one chip at width {width}");
        assert_eq!(icons.len(), 1);
        let (_, px, at, _) = icons[0];
        let chip = chips[1].rect;
        assert!(at[0] >= chip[0] && at[0] + px as f32 <= chip[0] + chip[2] + 0.5, "icon inside its chip at width {width}");
        let line_of = |byte: usize| layout.lines().position(|l| l.text_range().contains(&byte)).unwrap();
        for p in &pills {
            assert_eq!(line_of(p.start as usize), line_of(p.end as usize - 1), "no wrap inside a pill at width {width}");
        }
        for (k, c) in chips.iter().enumerate() {
            let r = c.rect;
            let fits = r[0] + r[2] <= max + 0.5 || r[0] <= pm.margin * scale + 0.5;
            assert!(fits, "chip {k} {r:?} overflows {max} at width {width}");
            for (j, d) in chips.iter().enumerate().filter(|&(j, _)| j != k) {
                let same_row = (r[1] - d.rect[1]).abs() < 1.0;
                assert!(!same_row || r[0] + r[2] <= d.rect[0] + 0.5 || d.rect[0] + d.rect[2] <= r[0] + 0.5, "chips {k} and {j} overlap at width {width}");
            }
        }
        for (li, range, x0, x1) in glyph_boxes(&layout) {
            for (p, c) in pills.iter().zip(&chips) {
                let inside = (p.start as usize) <= range.start && range.end <= p.end as usize;
                let row = line_of(p.start as usize) == li;
                let ws = content[range.clone()].trim().is_empty();
                if row && !inside && !ws {
                    assert!(x1 <= c.rect[0] + 0.5 || c.rect[0] + c.rect[2] <= x0 + 0.5, "glyph {:?} overlaps a chip at width {width}", &content[range.clone()]);
                }
                if inside {
                    assert!(x0 >= c.rect[0] && x1 <= c.rect[0] + c.rect[2] + 0.5, "label {:?} {x0}..{x1} inside its chip {:?} at width {width}: {:?}", &content[range.clone()], c.rect, layout.lines().map(|l| l.text_range()).collect::<Vec<_>>());
                }
            }
        }
        for (p, c) in pills.iter().zip(&chips) {
            for (at, before) in [(p.start as usize, true), (p.end as usize, false)] {
                let cur = parley::editing::Cursor::from_byte_index(&layout, at, parley::Affinity::Downstream);
                let x = cursor_geometry(&cur, &layout, 1.0).x0 as f32;
                let line = line_of(at.min(content.len() - 1));
                if line == line_of(p.start as usize) {
                    let outside = if before { x <= c.rect[0] + 0.5 } else { x >= c.rect[0] + c.rect[2] - 0.5 };
                    assert!(outside, "caret at {at} ({x}) inside chip {:?} at width {width}", c.rect);
                }
            }
        }
    }
}

/// A pill wider than its line wraps like the text it labels (as a browser wraps a span) instead
/// of running past the line: one chip per line, each inside the line, over its part of the label.
#[test]
fn a_pill_wider_than_its_line_wraps_inside_it() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let label = "@Migrate config loader to the new schema";
    let content = format!("as in {label} did");
    let start = content.find(label).unwrap() as u32;
    let pills = [PillChip { start, end: start + label.len() as u32, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, bare: None }];
    let scale = 2.0;
    for width in (90..=200).step_by(7) {
        let max = width as f32 * scale;
        let layout = text.layout_blocks(&content, &[], &[], &pills, style, scale, Some(max), Alignment::Start);
        let (mut chips, mut icons) = (Vec::new(), Vec::new());
        pill_chips(&layout, &pills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut chips, &mut icons);
        let rows: Vec<_> = layout.lines().filter(|l| { let r = l.text_range(); r.start < pills[0].end as usize && (pills[0].start as usize) < r.end }).collect();
        assert!(rows.len() >= 2, "the label wraps at width {width}");
        assert_eq!(chips.len(), rows.len(), "one chip per line at width {width}");
        assert!(layout.width() <= max + 0.5, "the text stays inside {max} at width {width}: {}", layout.width());
        for c in &chips {
            let r = c.deco.rect;
            assert!(r[0] >= -0.5 && r[0] + r[2] <= max + 0.5, "chip {r:?} inside {max} at width {width}");
        }
    }
}

#[test]
fn live_layouts_do_not_accumulate_between_commits() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let stable = text.layout_rich("stable label", &[], style, 2.0, None);
    for tick in 0..256 { text.layout_live(&tick.to_string(), style, 2.0); }
    assert_eq!(text.layouts.len(), 1);
    assert!(Rc::ptr_eq(&stable, &text.layout_rich("stable label", &[], style, 2.0, None)));
}

#[test]
fn a_view_shown_again_after_a_while_does_not_reshape() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let row = text.layout_rich("Back to the list", &[], style, 2.0, None);
    for _ in 0..8 { text.end_commit(); }
    let shaped = text.shaped;
    assert!(Rc::ptr_eq(&row, &text.layout_rich("Back to the list", &[], style, 2.0, None)));
    assert_eq!(text.shaped, shaped);
}

#[test]
fn stale_layouts_stay_within_their_budget_newest_kept() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let labels: Vec<String> = (0..1200).map(|i| format!("label {i:04}")).collect();
    for label in &labels {
        text.layout_rich(label, &[], style, 2.0, None);
        text.end_commit();
    }
    // Two more, so even the last label is stale.
    for _ in 0..2 { text.end_commit(); }
    let cost: usize = text.layouts.values().map(|e| e.cost as usize).sum();
    assert!(cost <= STALE_BYTES, "{cost}");
    assert!(cost > STALE_BYTES * 9 / 10, "{cost}");
    let key = |label: &str| text_hash(label, &[], style, 2.0, None);
    assert!(text.layouts.contains_key(&key(&labels[1199])));
    assert!(!text.layouts.contains_key(&key(&labels[0])));
}

/// A view's labels all share the commit it was last shown in.
#[test]
fn a_view_over_the_budget_keeps_a_budget_of_its_layouts() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0);
    let labels: Vec<String> = (0..1200).map(|i| format!("label {i:04}")).collect();
    let show = |text: &mut TextSystem| for label in &labels { text.layout_rich(label, &[], style, 2.0, None); };
    show(&mut text);
    for _ in 0..2 {
        text.end_commit();
        assert_eq!(text.layouts.len(), 1200, "the current and previous commits are never evicted");
    }
    text.end_commit();
    let cost: usize = text.layouts.values().map(|e| e.cost as usize).sum();
    assert!(cost <= STALE_BYTES, "{cost}");
    assert!(cost > STALE_BYTES * 9 / 10, "{cost}");
    let kept = text.layouts.len();
    let shaped = text.shaped;
    show(&mut text);
    assert_eq!((text.shaped - shaped) as usize, 1200 - kept, "shown again, only what was dropped reshapes");
    for _ in 0..2 {
        text.end_commit();
        assert_eq!(text.layouts.len(), 1200, "the current and previous commits are never evicted");
    }
}

#[test]
fn optical_size_default_keeps_static_font_glyphs_byte_identical() {
    let mut a = TextSystem::default(); let mut b = TextSystem::default();
    for scale in [1.0, 2.0] {
        for size in [11.0, 13.0, 24.0] {
            let style = TextStyle::new(size, 400.0);
            let mut qa = Vec::new(); let mut qb = Vec::new();
            a.glyphs("Hello ffi 0123456789", style, scale, [0.0, 0.0], &mut qa);
            b.glyphs("Hello ffi 0123456789", style.optical_sizing(OpticalSize::FontDefault), scale, [0.0, 0.0], &mut qb);
            assert_eq!(qa.iter().map(|q| (q.rect, q.uv)).collect::<Vec<_>>(), qb.iter().map(|q| (q.rect, q.uv)).collect::<Vec<_>>());
            assert_eq!(a.atlas.data, b.atlas.data);
        }
    }
}

#[test]
fn moving_glyphs_reuse_four_phases_and_land_without_atlas_work() {
    let mut text = TextSystem::default();
    let layout = text.layout_rich("lHEx ", &[], TextStyle::new(13.0, 400.0), 2.0, None);
    let mut quads = Vec::new();
    // Negative origins exercise phase carry as well as both sides of zero.
    for step in 0..256 {
        quads.clear();
        text.glyphs_for_layout(&layout, 2.0, [-2.0 + step as f32 / 64.0, 12.37], false, &mut quads);
        assert_eq!(quads.len(), 4, "dilation must not turn whitespace into empty draw instances");
        assert!(quads.iter().all(|g| g.rect[0].fract() == 0.0 && g.rect[1].fract() == 0.0));
    }
    let entries = text.atlas.map.len();
    assert!(entries <= 5 * 4, "four glyphs plus cached whitespace have at most four x phases");
    let mut track = rune_motion::Animated::new(-2.0f32).with_class(rune_motion::PropClass::Spatial);
    track.animate_to(8.0, rune_motion::Spring::new(0.3, 0.0), 0.0);
    let end = track.settle_time();
    assert!(end.is_finite());
    text.atlas.dirty = None;
    quads.clear();
    text.glyphs_for_layout(&layout, 2.0, [8.0, 12.37], false, &mut quads);
    let landed: Vec<_> = quads.iter().map(|q| (q.rect, q.uv)).collect();
    for frame in 0..120 {
        let x = track.sample(end + frame as f64 / 120.0);
        assert_eq!(x, 8.0, "the analytic tail lands on its target exactly");
        quads.clear();
        text.glyphs_for_layout(&layout, 2.0, [x, 12.37], false, &mut quads);
        assert!(quads.iter().map(|q| (q.rect, q.uv)).eq(landed.iter().copied()));
    }
    assert_eq!(text.atlas.map.len(), entries);
    assert!(text.atlas.dirty.is_none());
}

#[test]
fn glyphs_at_the_same_device_ppem_share_a_mask() {
    let mut text = TextSystem::default();
    text.set_font_smoothing(FontSmoothing::On);
    let small = text.layout_rich("H", &[], TextStyle::new(12.0, 400.0), 2.0, None);
    let large = text.layout_rich("H", &[], TextStyle::new(24.0, 400.0), 1.0, None);
    let mut a = Vec::new(); let mut b = Vec::new();
    // CoreGraphics dilates by device ppem alone, so the two share a mask.
    text.glyphs_for_layout(&small, 2.0, [0.0, 0.0], false, &mut a);
    text.glyphs_for_layout(&large, 1.0, [0.0, 0.0], false, &mut b);
    assert_eq!(a[0].uv, b[0].uv, "same device ppem and ink, same CoreGraphics mask");
}

#[test]
fn atlas_capacity_never_overwrites_current_or_pinned_placements() {
    let mut atlas = Atlas::new();
    let side = ATLAS_SIZE - 2;
    let bitmap = vec![137; (side * side) as usize];
    for page in 0..MAX_OVERFLOW_PAGES {
        let g = atlas.put(side, side, &bitmap, 0, 0).unwrap();
        assert_eq!(g.page as usize, page);
    }
    assert!(atlas.put(side, side, &bitmap, 0, 0).is_none());
    assert_eq!(atlas.resets(), 0, "capacity must not invalidate emitted quads");
    // Simulate the next frame retaining all eight pages in text flights.
    atlas.begin_frame((1 << MAX_OVERFLOW_PAGES) - 1);
    assert!(atlas.put(side, side, &bitmap, 0, 0).is_none());
    assert_eq!(atlas.resets(), 0, "pinned pages cannot be recycled either");
    for page in 0..MAX_OVERFLOW_PAGES {
        assert_eq!(atlas.page_data(page).unwrap()[ATLAS_SIZE as usize + 1], 137);
    }
    atlas.begin_frame(0);
    assert!(atlas.put(side, side, &bitmap, 0, 0).is_some(), "failure is retryable next frame");
}

#[test]
fn atlas_eviction_preserves_cached_empty_outlines() {
    let mut atlas = Atlas::new();
    let key = GlyphKey { font: 0, index: 0, glyph: 0, size: 0, coords: 0, subpx: 0, subpy: 0, dilation: 0, apple: 0, skew: 0, bold: false };
    atlas.map.insert(key, super::atlas::fill::Residency::Blank);
    atlas.icons.insert((0, 0), None);
    atlas.evict_page(0);
    assert!(atlas.map.contains_key(&key));
    assert!(atlas.icons.contains_key(&(0, 0)));
}

#[test]
fn colour_budget_retires_unused_scales_without_flushing_active_images() {
    let mut text = TextSystem::default();
    let key = GlyphKey { font: 0, index: 0, glyph: 0, size: 0, coords: 0, subpx: 0, subpy: 0, dilation: 0, apple: 0, skew: 0, bold: false };
    let pixels = rune_image::pixels::Pixels::new();
    pixels.set(1, 1, vec![255; 4]);
    text.color_pixels = vec![pixels.clone(), pixels];
    let active = ColorGlyph { index: 2, used: true, left: 0, top: 0, w: 4096, h: 4096 };
    text.color_glyphs.insert(key, ColorGlyph { index: 1, used: false, ..active });
    let active_key = GlyphKey { glyph: 1, ..key };
    text.color_glyphs.insert(active_key, active);
    text.color_bytes = COLOR_GLYPH_BUDGET + 1;
    text.begin_frame();
    assert!(!text.color_glyphs.contains_key(&key));
    assert_eq!(text.color_pixels.len(), 1);
    assert_eq!(text.color_glyphs[&active_key].index, 1);
    assert_eq!(text.color_glyph(1).rgba(), &[255; 4]);
    // An over-budget visible working set is reused, not flushed every frame.
    text.color_glyphs.get_mut(&active_key).unwrap().used = true;
    text.begin_frame();
    assert_eq!(text.color_glyphs.len(), 1);
    text.begin_frame();
    assert!(text.color_glyphs.is_empty(), "unused working set can be retired");
    assert_eq!(text.color_bytes, 0);
}

#[test]
fn four_hundred_scales_never_exhaust_the_atlas_or_poison_glyphs() {
    const ICON: Icon = Icon {
        name: "box",
        view: 16.0,
        body: r#"<rect x="2" y="2" width="12" height="12" fill="currentColor"/>"#,
    };
    let mut text = TextSystem::default();
    // Unplaceable request must not poison the icon cache with `Some(None)`.
    assert!(text.icon_quad(&ICON, ATLAS_SIZE, [0.0, 0.0]).is_none());
    assert!(!text.atlas.icons.contains_key(&(&ICON as *const Icon as usize, ATLAS_SIZE)));

    let sample = "The quick brown fox jumps over the lazy dog 0123456789";
    let mut quads = Vec::new();
    for i in 0..400 {
        text.begin_frame();
        quads.clear();
        let size = 18.0 + (i as f32) * 0.45;
        text.glyphs(sample, TextStyle::new(size, 400.0), 2.0, [0.0, 0.0], &mut quads);
        let expected = sample.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(quads.len(), expected, "missing glyph at step {i} (size={size})");
        let icon_px = 24 + (i % 96) as u32;
        assert!(text.icon_quad(&ICON, icon_px, [0.0, 0.0]).is_some(), "missing icon at step {i}");
    }
    assert!(text.atlas.resets() >= 4, "400 scales must cycle the atlas multiple times (resets={})", text.atlas.resets());
    assert!(text.atlas.page_count() <= MAX_STEADY_PAGES, "steady single-page frames stay within 2 pages");

    // Fresh string at scale 1.0 after 400 scales must have every glyph present and non-empty.
    text.begin_frame();
    let fresh = "Sphinx of black quartz, judge my vow! 9876543210";
    quads.clear();
    text.glyphs(fresh, TextStyle::new(16.0, 400.0), 1.0, [0.0, 0.0], &mut quads);
    let expected_fresh = fresh.chars().filter(|c| !c.is_whitespace()).count();
    assert_eq!(quads.len(), expected_fresh, "every glyph in the fresh string at scale 1 must be present");
    for (idx, q) in quads.iter().enumerate() {
        let page = (q.uv[1] as u32 / ATLAS_SIZE) as usize;
        let (ax, ay) = (q.uv[0] as u32, q.uv[1] as u32 % ATLAS_SIZE);
        let (w, h) = (q.rect[2] as u32, q.rect[3] as u32);
        let buf = if page == 0 { &text.atlas.data } else { &text.atlas.pages[page - 1].data };
        let mut sum = 0u64;
        for row in 0..h {
            let start = ((ay + row) * ATLAS_SIZE + ax) as usize;
            sum += buf[start..start + w as usize].iter().map(|&b| b as u64).sum::<u64>();
        }
        assert!(sum > 0, "fresh glyph {idx} must have nonzero coverage in page {page}");
    }
    // Whitespace is still cached as `Some(None)` so spaces don't re-enter swash every frame.
    assert!(text.atlas.map.values().any(|v| v.mask().is_none()), "empty space glyph is cached as None");
}

#[test]
fn single_frame_exceeding_one_page_places_every_glyph_across_overflow_pages() {
    const ICON: Icon = Icon {
        name: "box",
        view: 16.0,
        body: r#"<rect x="1" y="1" width="14" height="14" fill="currentColor"/>"#,
    };
    let mut text = TextSystem::default();
    text.begin_frame();
    let mut placed = Vec::new();
    // 32 icons around 400×400 px each (~5.1M texels) exceed one 2048×2048 page (4.19M texels) in a single frame.
    for i in 0..32u32 {
        let px = 380 + i;
        let q = text.icon_quad(&ICON, px, [0.0, 0.0]).expect("overflow page must accommodate single-frame burst");
        placed.push((px, q));
    }
    assert!(text.atlas.page_count() >= 2, "single frame exceeding 4M texels must spill to a second page");
    assert_eq!(text.atlas.resets(), 0, "must not evict a page already touched in the current frame");
    for (px, q) in &placed {
        let page = (q.uv[1] as u32 / ATLAS_SIZE) as usize;
        let (ax, ay) = (q.uv[0] as u32, q.uv[1] as u32 % ATLAS_SIZE);
        let buf = if page == 0 { &text.atlas.data } else { &text.atlas.pages[page - 1].data };
        let mid = ((ay + px / 2) * ATLAS_SIZE + ax + px / 2) as usize;
        assert_eq!(buf[mid], 255, "glyph at {px}px on page {page} must retain its coverage");
    }
}

#[test]
fn pill_chips_apply_custom_radius_clamping_ring_and_leading_images() {
    static FALLBACK: Icon = Icon { name: "fallback", view: 24.0, body: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>" };
    let logo = rune_image::pixels::Pixels::new();
    logo.set(16, 16, vec![255; 16 * 16 * 4]);
    let ring_color = Color { r: 0.2, g: 0.5, b: 0.9, a: 0.8 };
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;

    // Single-line: default radius (5pt), negative radius (0pt), capsule radius (clamped to h/2),
    // scaled ring width, and leading Pixels image taking precedence over icon and reserving room.
    let content = "ping @default @sharp @capsule @logo here";
    let find = |s: &str| {
        let a = content.find(s).unwrap() as u32;
        (a, a + s.len() as u32)
    };
    let (d0, d1) = find("@default");
    let (s0, s1) = find("@sharp");
    let (c0, c1) = find("@capsule");
    let (l0, l1) = find("@logo");
    let pills = [
        PillChip { start: d0, end: d1, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, bare: None },
        PillChip { start: s0, end: s1, icon: None, image: None, bg: None, fg: None, ring: Some((ring_color, 0.0)), radius: Some(-3.0), bare: None },
        PillChip { start: c0, end: c1, icon: None, image: None, bg: None, fg: None, ring: Some((ring_color, 1.5)), radius: Some(999.0), bare: None },
        PillChip { start: l0, end: l1, icon: Some(&FALLBACK), image: Some(logo.clone()), bg: None, fg: None, ring: Some((ring_color, 1.0)), radius: Some(8.0), bare: None },
    ];
    let bare = text.layout_blocks(content, &[], &[], &[], style, scale, None, Alignment::Start);
    let layout = text.layout_blocks(content, &[], &[], &pills, style, scale, None, Alignment::Start);
    let pm = PillMetrics::new(14.0);
    let expected_room = (3.0 * pm.lead(false) + pm.lead(true) + 4.0 * pm.trail()) * scale;
    assert!((layout.full_width() - bare.full_width() - expected_room).abs() < 4.5, "leading image reserves icon room");

    let (mut chips, mut icons, mut images) = (Vec::new(), Vec::new(), Vec::new());
    pill_chips_with_images(&layout, &pills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut chips, &mut icons, &mut images);
    assert_eq!(chips.len(), 4);
    assert_eq!(icons.len(), 0, "leading image takes precedence over monochrome icon");
    assert_eq!(images.len(), 1);

    assert!((chips[0].deco.radius - 10.0).abs() < 0.01);
    assert_eq!(chips[0].ring, None);
    assert_eq!(chips[1].deco.radius, 0.0);
    assert_eq!(chips[1].ring, None);
    let expected_cap = chips[2].deco.rect[2].min(chips[2].deco.rect[3]) * 0.5;
    assert!((chips[2].deco.radius - expected_cap).abs() < 0.01);
    assert_eq!(chips[2].ring, Some((ring_color, 3.0)));
    assert_eq!(chips[3].ring, Some((ring_color, 2.0)));
    let (_, px, at) = &images[0];
    let chip3 = chips[3].deco.rect;
    assert!(at[0] >= chip3[0] && at[0] + *px <= chip3[0] + chip3[2] + 0.5, "logo {at:?} ({px}px) inside chip {chip3:?}");

    // Wrapped long pill carries both custom radius and ring onto every wrapped segment.
    let long_label = "@Migrate config loader to the new schema";
    let wrapped_text = format!("hi {long_label} end");
    let ws = wrapped_text.find(long_label).unwrap() as u32;
    let wpills = [PillChip {
        start: ws,
        end: ws + long_label.len() as u32,
        icon: None,
        image: Some(logo.clone()),
        bg: None,
        fg: None,
        ring: Some((ring_color, 1.0)),
        radius: Some(999.0),
        bare: None,
    }];
    let wlayout = text.layout_blocks(&wrapped_text, &[], &[], &wpills, style, scale, Some(150.0 * scale), Alignment::Start);
    let (mut wchips, mut wicons, mut wimages) = (Vec::new(), Vec::new(), Vec::new());
    pill_chips_with_images(&wlayout, &wpills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut wchips, &mut wicons, &mut wimages);
    assert!(wchips.len() >= 2, "long pill wraps across lines");
    assert_eq!(wimages.len(), 1, "leading image draws only on the first segment");
    for seg in &wchips {
        let cap = seg.deco.rect[2].min(seg.deco.rect[3]) * 0.5;
        assert!((seg.deco.radius - cap).abs() < 0.01, "wrapped segment keeps clamped radius");
        assert_eq!(seg.ring, Some((ring_color, 2.0)), "wrapped segment keeps scaled ring");
    }

    // RTL pill puts leading image on the chip's right (start) side.
    let rtl_text = "שלום דוד היום";
    let rs = rtl_text.find("דוד").unwrap() as u32;
    let rpills = [PillChip {
        start: rs,
        end: rs + "דוד".len() as u32,
        icon: None,
        image: Some(logo),
        bg: None,
        fg: None,
        ring: Some((ring_color, 1.0)),
        radius: Some(999.0),
        bare: None,
    }];
    let rlayout = text.layout_blocks(rtl_text, &[], &[], &rpills, style, scale, None, Alignment::Start);
    let (mut rchips, mut ricons, mut rimages) = (Vec::new(), Vec::new(), Vec::new());
    pill_chips_with_images(&rlayout, &rpills, 14.0, scale, [0.0, 0.0], Color::BLACK, &mut rchips, &mut ricons, &mut rimages);
    assert_eq!(rchips.len(), 1);
    assert_eq!(rimages.len(), 1);
    let rc = rchips[0].deco.rect;
    let (_, rpx, rat) = &rimages[0];
    assert!(rat[0] >= rc[0] && rat[0] + *rpx <= rc[0] + rc[2] + 0.5, "RTL logo inside chip");
    assert!(rat[0] + *rpx * 0.5 > rc[0] + rc[2] * 0.5, "RTL logo sits on the right (start) half of the chip: {rat:?} in {rc:?}");
}

#[test]
fn text_flow_line_span_covers_single_wrapped_multiline_and_blank_lines() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let scale = 2.0;
    let line_px = style.line_px() * scale;

    let wrapped = "alpha beta gamma delta epsilon zeta";
    let single = text.layout_blocks(wrapped, &[], &[], &[], style, scale, Some(90.0 * scale), Alignment::Start);
    assert!(single.len() >= 3);
    let flow = TextFlow::Single(&single);
    assert_eq!(flow.line_span(5, 5, line_px), None, "empty range returns None");
    assert_eq!(flow.line_span(10, 5, line_px), None, "inverted range returns None");
    let first_r = single.get(0).unwrap().text_range();
    let last_r = single.get(single.len() - 1).unwrap().text_range();
    let (t0, b0) = flow.line_span(first_r.start, first_r.end, line_px).unwrap();
    let (t_all, b_all) = flow.line_span(0, wrapped.len(), line_px).unwrap();
    assert!((t_all - t0).abs() < 0.5);
    assert!(b_all > b0 + line_px * 0.5, "multi-line span grows vertically: {b0} -> {b_all}");
    let (tl, bl) = flow.line_span(last_r.start, last_r.end, line_px).unwrap();
    assert!(tl >= b0 - 0.5 && (bl - b_all).abs() < 0.5);

    let doc = "intro\n```\nlet a = 1;\n\nlet b = 2;\n```\noutro";
    let input = TextInput {
        content: doc,
        spans: &[],
        blocks: &[],
        pills: &[],
        span_boxes: &[],
        inline_boxes: &[],
        text_indent: 0.0,
        style,
        shape_key: None,
        wrap_anywhere: false,
        pretty: false,
        redacted: false,
        by_line: false,
        by_paragraph: true,
        dir: TextDir::Plaintext(None),
    };
    let tl = text.layout_text(&input, true, scale, Some(300.0 * scale), Alignment::Start, None);
    let pflow = tl.view();
    let code_start = doc.find("```").unwrap();
    let code_end = doc.rfind("```").unwrap() + 3;
    let intro_span = pflow.line_span(0, 5, line_px).unwrap();
    let intro_with_nl = pflow.line_span(0, 6, line_px).unwrap();
    assert_eq!(intro_span, intro_with_nl, "trailing newline stays on the same line");
    let code_span = pflow.line_span(code_start, code_end, line_px).unwrap();
    let outro_start = doc.find("outro").unwrap();
    let outro_span = pflow.line_span(outro_start, doc.len(), line_px).unwrap();
    assert!(code_span.0 >= intro_span.1 - 0.5);
    assert!(outro_span.0 >= code_span.1 - 0.5);
    assert!((code_span.1 - code_span.0) >= 4.5 * line_px, "code block height: {:?}", code_span);
    let blank_at = doc.find("\n\n").unwrap() + 1;
    let blank_span = pflow.line_span(blank_at, blank_at + 1, line_px).unwrap();
    assert!(blank_span.0 > code_span.0 && blank_span.1 < code_span.1);
}

#[test]
fn inline_icon_stays_on_url_line_with_prefix_and_never_creates_empty_line() {
    const ICON: Icon = Icon { name: "globe", view: 16.0, body: "" };
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(22.4);
    let cases = [
        // Exact parenthesised URL + isolated code/URL marks matching Markdown rendering.
        "Checks green: \u{2066}81/81\u{2069} theme tests pass in \u{2066}packages/ui\u{2069}, and PR #66531 (\u{2066}https://github.com/anomalyco/opencode/pull/66531\u{2069}) is \u{2066}MERGEABLE\u{2069} / \u{2066}CLEAN\u{2069} ok.",
        // Plain URL after whitespace (no opening parenthesis).
        "Checks green: 81/81 theme tests pass in packages/ui, see https://github.com/anomalyco/opencode/pull/66531 now.",
    ];
    for (ci, content) in cases.iter().enumerate() {
        let url_at = content.find("https://").unwrap();
        let pill_start = if content[..url_at].ends_with('\u{2066}') { url_at - '\u{2066}'.len_utf8() } else { url_at };
        let pill_end = pill_start + content[pill_start..].find(char::is_whitespace).unwrap();
        let paren_at = content[..pill_start].ends_with('(').then_some(pill_start - 1);
        let pills = [PillChip {
            start: pill_start as u32,
            end: pill_end as u32,
            icon: Some(&ICON),
            image: None,
            bg: None,
            fg: None,
            ring: None,
            radius: None,
            bare: Some(Box::new(crate::layout::span::BareIcon {
                size: 14.0,
                gap: 4.0,
                radius: 0.0,
                image: None,
                replace: None,
                label: None,
                help: None,
            })),
        }];
        for scale in [1.0f32, 2.0] {
            for (wrap_anywhere, pretty) in [(false, false), (true, false), (true, true)] {
                for width in (180..=440).step_by(11) {
                    let max_w = width as f32 * scale;
                    let layout = ts.layout_keyed(
                        None,
                        content,
                        &[],
                        &[],
                        0.0,
                        &pills,
                        &[],
                        &[],
                        style,
                        scale,
                        Some(max_w),
                        Alignment::Start,
                        wrap_anywhere,
                        pretty,
                        crate::Keep::Owned,
                        parley::BaseDirection::Auto,
                    );
                    // No line may ever collapse to a zero-height / empty orphan line.
                    for (li, line) in layout.lines().enumerate() {
                        let m = line.metrics();
                        assert!(
                            m.line_height > 0.0 && !line.text_range().is_empty(),
                            "case={ci} scale={scale} wrap_anywhere={wrap_anywhere} pretty={pretty} w={width} L{li} was empty/zero-height"
                        );
                        if wrap_anywhere {
                            let used = m.advance - m.hanging_advance;
                            assert!(
                                used <= max_w + 1.5 * scale,
                                "case={ci} scale={scale} pretty={pretty} w={width} L{li} overflowed: used={used} max={max_w}"
                            );
                        }
                    }
                    let url_line_idx = layout
                        .lines()
                        .position(|l| l.text_range().contains(&url_at))
                        .expect("URL start must lie on a line");
                    let url_line = layout.get(url_line_idx).unwrap();
                    let um = url_line.metrics();

                    // Opening '(' stays attached to the icon and URL on the same line.
                    if let Some(p_at) = paren_at {
                        let paren_line_idx = layout
                            .lines()
                            .position(|l| l.text_range().contains(&p_at))
                            .expect("paren must lie on a line");
                        assert_eq!(
                            paren_line_idx, url_line_idx,
                            "case={ci} scale={scale} wrap_anywhere={wrap_anywhere} pretty={pretty} w={width}: '(' on L{paren_line_idx}, URL on L{url_line_idx}"
                        );
                    }

                    // The icon sits inside the URL line's vertical band and immediately before 'h'.
                    let mut icons = Vec::new();
                    bare_icons(&layout, &pills, scale, [0.0, 0.0], &mut icons);
                    assert_eq!(icons.len(), 1);
                    let (_, icon_at, icon_size) = icons[0];
                    assert!(
                        icon_at[1] >= um.block_min_coord - 1.0 && icon_at[1] + icon_size[1] <= um.block_max_coord + 1.0,
                        "case={ci} scale={scale} wrap_anywhere={wrap_anywhere} pretty={pretty} w={width}: icon y={}..{} outside URL line {}..{}",
                        icon_at[1],
                        icon_at[1] + icon_size[1],
                        um.block_min_coord,
                        um.block_max_coord,
                    );
                }
            }
        }
    }
}

#[test]
fn wide_unbroken_pill_and_rtl_inline_icon_never_orphan_leading_box() {
    const ICON: Icon = Icon { name: "dot", view: 16.0, body: "" };
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0).lh(22.0);

    // 1. Boxed pill whose label is a single unbroken identifier wider than the column.
    let pill_body = "check @very_long_unbroken_identifier_name_that_exceeds_the_column_width next";
    let ps = pill_body.find('@').unwrap() as u32;
    let pe = (pill_body.find(" next").unwrap()) as u32;
    let pills = [PillChip {
        start: ps,
        end: pe,
        icon: Some(&ICON),
        image: None,
        bg: None,
        fg: None,
        ring: None,
        radius: None,
        bare: None,
    }];
    for wrap_anywhere in [false, true] {
        for width in [140.0f32, 180.0, 220.0] {
            let layout = ts.layout_blocks_inner(pill_body, &[], &[], &pills, style, 1.0, Some(width), Alignment::Start, wrap_anywhere, false);
            for (li, line) in layout.lines().enumerate() {
                assert!(
                    line.metrics().line_height > 0.0 && !line.text_range().is_empty(),
                    "pill wrap_anywhere={wrap_anywhere} w={width} L{li} empty/zero-height"
                );
            }
            let (mut chips, mut icons) = (Vec::new(), Vec::new());
            pill_chips(&layout, &pills, 14.0, 1.0, [0.0, 0.0], Color::BLACK, &mut chips, &mut icons);
            assert_eq!(icons.len(), 1);
            let label_line = layout.lines().find(|l| l.text_range().contains(&(ps as usize))).unwrap();
            let lm = label_line.metrics();
            let icon_y = icons[0].2[1];
            assert!(
                icon_y >= lm.block_min_coord - 1.0 && icon_y <= lm.block_max_coord + 1.0,
                "pill icon y={icon_y} outside label line {}..{}",
                lm.block_min_coord,
                lm.block_max_coord,
            );
        }
    }

    // 2. RTL paragraph containing an isolated LTR URL with a bare inline icon.
    let rtl = "راجع هذا الرابط (\u{2066}https://github.com/anomalyco/opencode/pull/66531\u{2069}) الآن";
    let url_at = rtl.find("https://").unwrap();
    let p_start = url_at - '\u{2066}'.len_utf8();
    let p_end = rtl.find(')').unwrap() + 1;
    let rtl_pills = [PillChip {
        start: p_start as u32,
        end: p_end as u32,
        icon: Some(&ICON),
        image: None,
        bg: None,
        fg: None,
        ring: None,
        radius: None,
        bare: Some(Box::new(crate::layout::span::BareIcon {
            size: 14.0,
            gap: 4.0,
            radius: 0.0,
            image: None,
            replace: None,
            label: None,
            help: None,
        })),
    }];
    for wrap_anywhere in [false, true] {
        for width in [220.0f32, 300.0, 360.0] {
            let layout = ts.layout_blocks_inner(rtl, &[], &[], &rtl_pills, style, 1.0, Some(width), Alignment::Start, wrap_anywhere, false);
            for (li, line) in layout.lines().enumerate() {
                assert!(
                    line.metrics().line_height > 0.0 && !line.text_range().is_empty(),
                    "rtl wrap_anywhere={wrap_anywhere} w={width} L{li} empty/zero-height"
                );
            }
            let url_line = layout.lines().find(|l| l.text_range().contains(&url_at)).unwrap();
            let um = url_line.metrics();
            let mut icons = Vec::new();
            bare_icons(&layout, &rtl_pills, 1.0, [0.0, 0.0], &mut icons);
            assert_eq!(icons.len(), 1);
            let (_, at, sz) = icons[0];
            assert!(
                at[1] >= um.block_min_coord - 1.0 && at[1] + sz[1] <= um.block_max_coord + 1.0,
                "rtl wrap_anywhere={wrap_anywhere} w={width}: icon y={}..{} outside URL line {}..{}",
                at[1],
                at[1] + sz[1],
                um.block_min_coord,
                um.block_max_coord,
            );
        }
    }
}
