//! Regression tests for kit-gallery virtualization, resize relayout, and unique sibling keys.

use std::cell::Cell;

use super::*;

fn duplicate_paths(rt: &rune::Runtime<G>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| e.key.as_deref().is_some_and(|k| k.contains("#dup")) || e.path.contains("#dup"))
        .map(|e| e.path)
        .collect()
}

/// The gallery index mounts only rows in and near the viewport (not all 183 stories), and
/// resizing with or without an open combobox takes Rune's relayout fast path.
#[test]
fn index_virtualizes_stories_and_relayouts_on_resize() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }
    fn counted(s: &G, cx: &mut Cx<G>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        view(s, cx)
    }

    let mut rt = rune::Runtime::new(init(), counted, (800.0, 560.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    // Let virtual_rows finish its initial spread overscan fill before measuring resizes.
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    assert!(
        rt.stats.layout_nodes < 1_000,
        "index should mount only visible stories + overscan, got {} nodes",
        rt.stats.layout_nodes,
    );

    // Horizontal drag resizes do not move the virtual window and should run zero views.
    let v0 = VIEWS.get();
    for w in [840.0, 900.0, 760.0, 820.0] {
        rt.set_viewport((w, 560.0), 1.0);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(VIEWS.get() - v0, 0, "horizontal index resizes should take Relayout without rebuilding view");

    // Opening the top combobox mounts its scrim and content once with no duplicates, and still relayouts on resize.
    rt.update(|s, _| {
        let _ = s.combo.step(rune_kit::ComboboxEvent::Toggle { current: None });
    });
    rt.frame(1.0 / 60.0);
    assert!(duplicate_paths(&rt).is_empty(), "open combobox in index must have no duplicate keys: {:?}", duplicate_paths(&rt));
    assert!(rt.find("model-content").is_some(), "model-content should mount when s.combo is open");

    let v1 = VIEWS.get();
    for w in [860.0, 920.0, 780.0] {
        rt.set_viewport((w, 560.0), 1.0);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(VIEWS.get() - v1, 0, "index resizes with open combobox should take Relayout without rebuilding view");
}

/// Opening the comboboxes and every shared overlay in index produces zero duplicate sibling keys
/// (previously `rune-combobox--searchable` and `rune-combobox--animated-filter` both mounted
/// `model-scrim` and `model-content` under `kit-root`).
#[test]
fn index_open_overlays_have_no_duplicate_sibling_keys() {
    let mut rt = rune::Runtime::new(init(), view, (800.0, 560.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(duplicate_paths(&rt).is_empty(), "closed index duplicates: {:?}", duplicate_paths(&rt));

    // Open all three comboboxes simultaneously: each must use distinct scrim/content keys.
    rt.update(|s, _| {
        let _ = s.combo.step(rune_kit::ComboboxEvent::Toggle { current: None });
        let _ = s.combo_animated.step(rune_kit::ComboboxEvent::Toggle { current: None });
        let _ = s.combo_localized.step(rune_kit::ComboboxEvent::Toggle { current: None });
    });
    rt.frame(1.0 / 60.0);
    let dups = duplicate_paths(&rt);
    assert!(dups.is_empty(), "all three comboboxes open in index had duplicates: {dups:?}");
    assert!(rt.find("model-content").is_some());
    assert!(rt.find("model-animated-content").is_some());
    assert!(rt.find("modele-content").is_some());

    // Close comboboxes and open every overlay key listed in OPENED in index mode.
    rt.update(|s, _| {
        s.combo = Default::default();
        s.combo_animated = Default::default();
        s.combo_localized = Default::default();
    });
    for &(_, key) in stories::OPENED {
        rt.update(|s, _| s.open = Some(key.to_owned()));
        rt.frame(1.0 / 60.0);
        let dups = duplicate_paths(&rt);
        assert!(dups.is_empty(), "index with open={key:?} had duplicate sibling keys: {dups:?}");
    }
}

/// Standing check: no single story scene produces a duplicate sibling key.
#[test]
fn no_story_produces_duplicate_sibling_keys() {
    for &(id, setup) in stories::SCENES {
        let mut g = init();
        setup(&mut g);
        let mut rt = rune::Runtime::new(g, view, (720.0, 480.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        let dups = duplicate_paths(&rt);
        assert!(dups.is_empty(), "story {id:?} produced duplicate sibling keys: {dups:?}");
    }
}

/// Stories whose hover handlers genuinely mutate app state (active menu/select/combobox/popup row,
/// submenu open, HoverCard open/close, `focus_highlight` row focus, or DiffView selectable-side
/// switch). Every other gallery story must commit zero times on a pure pointer sweep when the
/// debug `Tx::live_only` self-check is off.
const STATEFUL_HOVER_STORIES: &[&str] = &[
    "rune-combobox--searchable",
    "rune-combobox--animated-filter",
    "rune-combobox--localized",
    "ui-select--playground",
    "ui-select--grouped",
    "rune-select--states",
    "ui-menu--with-radio",
    "rune-menu--submenu-options",
    "rune-menu--submenu-small-window",
    "rune-menu--field-menu-fallback",
    "rune-hover-card--placements",
    "rune-focus--keyboard",
    "rune-popup-list--composer",
    "rune-popup-list--rich-inline",
    "rune-diff--localized",
    "rune-diff--custom",
];

/// Standing check: sweeping the pointer across any gallery story without pressing must not commit
/// the view unless the story is in `STATEFUL_HOVER_STORIES`. Catches kit hover/pointer handlers
/// that forget `Tx::live_only`.
#[test]
fn pure_hover_sweep_commits_only_on_stateful_hover_stories() {
    for &allowed in STATEFUL_HOVER_STORIES {
        assert!(
            stories::SCENES.iter().any(|&(id, _)| id == allowed),
            "STATEFUL_HOVER_STORIES entry {allowed:?} no longer exists in stories::SCENES"
        );
    }
    let mut unexpected = Vec::new();
    for &(id, setup) in stories::SCENES {
        if STATEFUL_HOVER_STORIES.contains(&id) {
            continue;
        }
        let mut g = init();
        setup(&mut g);
        let mut rt = rune::Runtime::new(g, view, (720.0, 480.0), rune::Env::new(true, 1.0));
        rt.check_live_only(false);
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        let before = rt.stats.commits;
        for row in 0..4 {
            for col in 0..6 {
                rt.pointer_move(40.0 + col as f32 * 120.0, 40.0 + row as f32 * 100.0);
                rt.frame(1.0 / 60.0);
            }
        }
        rt.pointer_move(1000.0, 1000.0);
        rt.frame(1.0 / 60.0);
        let commits = rt.stats.commits - before;
        if commits > 0 {
            unexpected.push((id, commits));
        }
    }
    assert!(
        unexpected.is_empty(),
        "stories with no stateful hover committed on a pure pointer sweep: {unexpected:?}"
    );
}


const PINNED_STORIES: &[&str] = &[
    // Controls and containers (31 stories).
    "ui-segmented-control--basic",
    "ui-segmented-control--controlled",
    "rune-segmented-control--rich",
    "components-collapsible--basic",
    "components-collapsible--ghost",
    "ui-accordion--basic",
    "ui-accordion--multiple",
    "ui-accordion--non-collapsible",
    "components-progress--basic",
    "components-progress--no-label",
    "components-progress--indeterminate",
    "components-list--divider",
    "ui-keybind--playground",
    "ui-keybind--variants",
    "ui-keybind--multiple-keys",
    "ui-keybind--all-examples",
    "rune-keybind--symbols",
    "rune-keybind--keep-case",
    "ui-textarea--playground",
    "ui-textarea--controlled",
    "ui-textarea--with-field",
    "ui-textarea--states",
    "rune-textarea--composer",
    "rune-textarea--code-keys",
    "rune-textarea--focus-caret",
    "ui-switch--basic",
    "ui-switch--appearances",
    "ui-switch--states",
    "ui-switch--hidden-label",
    "rune-switch--sizes",
    "rune-switch--web",
    // Overlays (13 stories: open menus with separators & submenus, select lists, popup list, dialogs, popover).
    "ui-menu--basic",
    "ui-menu--appearances",
    "ui-menu--rich",
    "ui-menu--with-submenu",
    "rune-menu--submenu-options",
    "ui-menu--with-radio",
    "ui-select--playground",
    "ui-select--grouped",
    "rune-select--states",
    "rune-popup-list--composer",
    "ui-dialog--basic",
    "rune-dialog--fit",
    "components-popover--basic",
];

fn fnv_bytes(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x1000_0000_01b3);
    }
}

/// Quantise a point measurement to 0.01 pt (canonicalising -0.0 and non-finite values).
fn fnv_pt(h: &mut u64, v: f32) {
    let q = if v.is_finite() { (f64::from(v) * 100.0).round() as i64 } else { i64::MIN };
    fnv_bytes(h, &q.to_le_bytes());
}

/// Quantise a colour channel to 1e-4 precision.
fn fnv_ch(h: &mut u64, v: f32) {
    let q = if v.is_finite() { (f64::from(v) * 10_000.0).round() as i64 } else { i64::MIN };
    fnv_bytes(h, &q.to_le_bytes());
}

fn fnv_color(h: &mut u64, c: rune::Color) {
    for ch in [c.r, c.g, c.b, c.a] {
        fnv_ch(h, ch);
    }
}

/// Cross-platform layout & chrome paint hash for one settled story frame.
///
/// Bundled Inter / IBM Plex Mono fonts are loaded so Latin shaping agrees across OSes, while
/// platform-dependent glyph rasterisation (`KIND_GLYPH`) and texture-atlas `uv` coordinates
/// are excluded. Every metric-driven geometry property (vertical position & height, corner
/// radius, padding/border insets, fills, borders, text styles, chrome paint rects/radii/borders/
/// shadows/gradients and icon box sizes) is rounded to 0.01 pt (colours to 1e-4) before hashing.
fn hash_story_frame(rt: &rune::Runtime<G>, include_x_width: bool) -> (u64, u64, usize, usize) {
    use rune::render::{KIND_GLYPH, KIND_IMAGE};

    let mut layout_h: u64 = 0xcbf2_9ce4_8422_2325;
    let els = rt.elements();
    for e in &els {
        fnv_bytes(&mut layout_h, e.path.as_bytes());
        fnv_bytes(&mut layout_h, &[0]);
        if let Some(k) = &e.key {
            fnv_bytes(&mut layout_h, k.as_bytes());
        }
        fnv_bytes(&mut layout_h, &[0]);
        fnv_bytes(&mut layout_h, e.role.as_bytes());
        // Vertical layout (y, h), corners, insets (padding + border), fill, border and type style.
        fnv_pt(&mut layout_h, e.rect[1]);
        fnv_pt(&mut layout_h, e.rect[3]);
        // Non-text boxes whose width is fixed or metric-driven (switches, dividers, icons,
        // dialogs, popovers, selects, text areas, fixed segmented controls).
        if include_x_width && e.role != "text" {
            fnv_pt(&mut layout_h, e.rect[0]);
            fnv_pt(&mut layout_h, e.rect[2]);
        }
        fnv_pt(&mut layout_h, e.radius);
        fnv_color(&mut layout_h, e.fill);
        match e.border_color {
            Some(c) => {
                fnv_bytes(&mut layout_h, &[1]);
                fnv_color(&mut layout_h, c);
            }
            None => fnv_bytes(&mut layout_h, &[0]),
        }
        for v in e.inset {
            fnv_pt(&mut layout_h, v);
        }
        if let Some(ts) = &e.text_style {
            fnv_bytes(&mut layout_h, &[1]);
            fnv_pt(&mut layout_h, ts.size);
            fnv_pt(&mut layout_h, ts.weight);
            fnv_pt(&mut layout_h, ts.line_height);
            fnv_pt(&mut layout_h, ts.tracking);
            fnv_bytes(&mut layout_h, &[u8::from(ts.italic)]);
        } else {
            fnv_bytes(&mut layout_h, &[0]);
        }
    }

    let mut paint_h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut chrome_count = 0usize;
    for inst in &rt.instances {
        if inst.params[1] == KIND_GLYPH {
            continue;
        }
        chrome_count += 1;
        fnv_pt(&mut paint_h, inst.params[1]); // kind
        fnv_pt(&mut paint_h, inst.params[0]); // border width
        fnv_pt(&mut paint_h, inst.params[2]); // shadow sigma
        if inst.params[1] == KIND_IMAGE {
            // Icon/image dimensions and tint (omit atlas UV and text-shifted x).
            fnv_pt(&mut paint_h, inst.rect[1]);
            fnv_pt(&mut paint_h, inst.rect[2]);
            fnv_pt(&mut paint_h, inst.rect[3]);
        } else if include_x_width {
            for v in inst.rect {
                fnv_pt(&mut paint_h, v);
            }
        } else {
            fnv_pt(&mut paint_h, inst.rect[1]);
            fnv_pt(&mut paint_h, inst.rect[3]);
        }
        for v in inst.radii {
            fnv_pt(&mut paint_h, v);
        }
        for v in inst.fill {
            fnv_ch(&mut paint_h, v);
        }
        for v in inst.border {
            fnv_ch(&mut paint_h, v);
        }
    }
    (layout_h, paint_h, els.len(), chrome_count)
}

/// Standing guard: pins the default look of every control, container and overlay story across
/// `rune_theme_opencode` and `Kit::neutral` (light and dark) against the committed baseline
/// `golden/default-look.txt`. Runs in an ordinary `cargo test -p kit-gallery` with no window.
///
/// To accept an intentional default-look change, run:
/// `RUNE_BLESS=1 cargo test -p kit-gallery pinned_default_look_baseline`
/// and review the changed lines in `examples/kit-gallery/golden/default-look.txt`.
fn ensure_gallery_fonts(rt: &mut rune::Runtime<G>) {
    static LOAD_FONTS: std::sync::Once = std::sync::Once::new();
    LOAD_FONTS.call_once(|| {
        for &bytes in rune_theme_opencode::FONTS {
            let _ = rt.add_font(bytes);
        }
    });
}

#[test]
fn pinned_default_look_baseline() {
    let baseline_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("golden/default-look.txt");

    let mut saved_families: Option<(String, String)> = None;
    let mut lines = Vec::with_capacity(PINNED_STORIES.len() * 4);
    for &id in PINNED_STORIES {
        let &(_, setup) = stories::SCENES
            .iter()
            .find(|&&(s, _)| s == id)
            .unwrap_or_else(|| panic!("missing story {id:?}"));
        for (theme_name, neutral) in [("opencode", false), ("neutral", true)] {
            for dark in [false, true] {
                let mut g = init();
                g.neutral = neutral;
                g.palette = false;
                setup(&mut g);
                if id == "ui-menu--with-submenu" {
                    g.on.insert("recent".into());
                }
                let mut rt = rune::Runtime::new(g, view, (720.0, 480.0), rune::Env::new(dark, 1.0));
                if saved_families.is_none() {
                    let (sans, mono) = rt.font_families();
                    saved_families = Some((sans.to_owned(), mono.to_owned()));
                }
                ensure_gallery_fonts(&mut rt);
                let (sans, mono) = rune_theme_opencode::FAMILIES;
                rt.set_font_families(sans, mono);
                rt.frame(0.0);
                rt.settle_all();
                rt.frame(0.0);
                // `rune-keybind--symbols` exercises OS fallback symbol glyphs (⌫/⎋/⌥/⌃/⏎)
                // absent from bundled Inter / IBM Plex Mono, whose horizontal advance widths vary
                // by platform font; vertical geometry, radii, padding insets, colours and chrome
                // counts remain pinned identically across Windows, macOS and Linux.
                let include_x_width = id != "rune-keybind--symbols";
                let (lh, ph, ne, nc) = hash_story_frame(&rt, include_x_width);
                lines.push(format!("{id}|{theme_name}|dark={dark}|els={ne}|chrome={nc}|layout={lh:016x}|paint={ph:016x}"));
            }
        }
    }
    if let Some((sans, mono)) = saved_families {
        let mut restore = rune::Runtime::new(init(), view, (16.0, 16.0), rune::Env::new(true, 1.0));
        restore.set_font_families(&sans, &mono);
    }

    let fresh = format!("{}\n", lines.join("\n"));
    if rune::env::var("RUNE_BLESS").is_some_and(|v| v != "0") {
        if let Some(parent) = baseline_path.parent() {
            std::fs::create_dir_all(parent).expect("create golden dir");
        }
        std::fs::write(&baseline_path, &fresh).expect("write default-look baseline");
        return;
    }

    let want = std::fs::read_to_string(&baseline_path).unwrap_or_else(|_| {
        panic!("missing {}; run with RUNE_BLESS=1 to generate", baseline_path.display())
    });
    let want_lines: Vec<&str> = want.lines().collect();
    let got_lines: Vec<&str> = fresh.lines().collect();
    assert_eq!(
        want_lines.len(),
        got_lines.len(),
        "default-look baseline line count changed ({} vs {}); accept intended changes with RUNE_BLESS=1",
        want_lines.len(),
        got_lines.len(),
    );
    let diffs: Vec<String> = want_lines
        .iter()
        .zip(&got_lines)
        .filter(|(a, b)| a != b)
        .map(|(a, b)| format!("  expected: {a}\n  actual:   {b}"))
        .collect();
    assert!(
        diffs.is_empty(),
        "default kit look drifted for {} story/theme combination(s) (accept intentional changes with RUNE_BLESS=1):\n{}",
        diffs.len(),
        diffs.join("\n"),
    );
}

/// Standing check: every overlay story under `Kit::neutral` and `rune_theme_opencode::kit`
/// produces bit-identical element geometry and GPU paint instances whether `Kit::metrics` is
/// left at `Metrics::default()` or explicitly populated with the literal default values from
/// #2362 and #2364. Also asserts each `::kit(&k)` returns those exact literal defaults.
#[test]
fn overlay_default_metrics_match_explicit_kit_defaults() {
    use rune_kit::{DialogMetrics, MenuMetrics, PopoverMetrics, PopupRowMetrics, SelectMetrics};

    const LITERAL_DIALOG: DialogMetrics = DialogMetrics { radius: 6.0, padding: 16.0, title: None };
    const LITERAL_POPUP_ROW: PopupRowMetrics = PopupRowMetrics { height: 28.0, radius: 4.0 };
    const LITERAL_SELECT: SelectMetrics = SelectMetrics { chevron: 16.0, chevron_box: 20.0 };
    const LITERAL_POPOVER: PopoverMetrics = PopoverMetrics { min_width: 200.0, max_width: 320.0 };
    const LITERAL_MENU: MenuMetrics = MenuMetrics {
        padding: 2.0,
        standard_padding: 4.0,
        separator_above: 2.0,
        separator_below: 2.0,
        standard_separator_above: 4.0,
        standard_separator_below: 4.0,
        standard_inner_border: 1.0,
    };

    const OVERLAY_STORIES: &[&str] = &[
        "ui-menu--basic",
        "ui-menu--appearances",
        "ui-menu--with-submenu",
        "rune-menu--submenu-options",
        "ui-menu--with-radio",
        "ui-select--playground",
        "ui-select--grouped",
        "rune-select--states",
        "rune-popup-list--composer",
        "ui-dialog--basic",
        "rune-dialog--fit",
        "components-popover--basic",
    ];

    thread_local! {
        static EXPLICIT: Cell<bool> = const { Cell::new(false) };
    }

    fn pin_view(s: &G, cx: &mut Cx<G>) -> El {
        let base = kit(s, cx.system_dark());
        let mut owned = base.clone();
        if EXPLICIT.get() {
            owned.metrics.dialog = Some(LITERAL_DIALOG);
            owned.metrics.popup_row = Some(LITERAL_POPUP_ROW);
            owned.metrics.select = Some(LITERAL_SELECT);
            owned.metrics.popover = Some(LITERAL_POPOVER);
            owned.metrics.menu = Some(LITERAL_MENU);
        }
        let mut overlays = Vec::new();
        let (_, f) = stories::STORIES
            .iter()
            .find(|(id, _)| *id == s.story)
            .expect("story");
        let body = f(s, &owned, &mut overlays);
        let frame = col().key("frame").fill(owned.c.page).padding(24.0).child(body);
        rune_kit::root(frame, overlays)
    }

    fn snapshot(neutral: bool, dark: bool, id: &str, explicit_metrics: bool) -> (Vec<String>, Vec<[u32; 32]>) {
        EXPLICIT.set(explicit_metrics);
        let (_, setup) = stories::SCENES.iter().find(|(s, _)| *s == id).expect("scene");
        let mut g = init();
        g.neutral = neutral;
        g.palette = false;
        setup(&mut g);
        if id == "ui-menu--with-submenu" {
            g.on.insert("recent".into());
        }
        let mut rt = rune::Runtime::new(g, pin_view, (720.0, 480.0), rune::Env::new(dark, 1.0));
        ensure_gallery_fonts(&mut rt);
        let (sans, mono) = rune_theme_opencode::FAMILIES;
        rt.set_font_families(sans, mono);
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(0.0);
        let els = rt
            .elements()
            .into_iter()
            .map(|e| {
                let ts = e.text_style.as_ref().map(|s| {
                    (s.size.to_bits(), s.weight.to_bits(), s.line_height.to_bits(), s.tracking.to_bits())
                });
                format!(
                    "{}|{:?}|{:?}|{:?}|{:#x}|{:?}|{:?}|{:?}|{:?}",
                    e.path,
                    e.key,
                    e.rect.map(f32::to_bits),
                    e.target.map(f32::to_bits),
                    e.radius.to_bits(),
                    e.inset.map(f32::to_bits),
                    e.fill,
                    e.border_color,
                    ts,
                )
            })
            .collect();
        let insts = rt
            .instances
            .iter()
            .map(|i| {
                let mut words = [0u32; 32];
                for (k, v) in i.rect.iter().chain(&i.radii).chain(&i.fill).chain(&i.border).chain(&i.params).chain(&i.uv).chain(&i.clip).enumerate() {
                    words[k] = v.to_bits();
                }
                words[28..32].copy_from_slice(&i.fade);
                words
            })
            .collect();
        (els, insts)
    }

    for (theme, neutral, dark) in [
        ("neutral-dark", true, true),
        ("neutral-light", true, false),
        ("opencode-dark", false, true),
        ("opencode-light", false, false),
    ] {
        let base = if neutral { Kit::neutral(dark) } else { rune_theme_opencode::kit(dark).clone() };
        assert_eq!(DialogMetrics::kit(&base), LITERAL_DIALOG, "{theme}: DialogMetrics::kit default");
        assert_eq!(PopupRowMetrics::kit(&base), LITERAL_POPUP_ROW, "{theme}: PopupRowMetrics::kit default");
        assert_eq!(SelectMetrics::kit(&base), LITERAL_SELECT, "{theme}: SelectMetrics::kit default");
        assert_eq!(PopoverMetrics::kit(&base), LITERAL_POPOVER, "{theme}: PopoverMetrics::kit default");
        assert_eq!(MenuMetrics::kit(&base), LITERAL_MENU, "{theme}: MenuMetrics::kit default");

        for &id in OVERLAY_STORIES {
            let def = snapshot(neutral, dark, id, false);
            let exp = snapshot(neutral, dark, id, true);
            assert_eq!(def.0, exp.0, "layout/elements diverged for {theme} {id}");
            assert_eq!(def.1, exp.1, "paint instances diverged for {theme} {id}");
        }
    }
}
