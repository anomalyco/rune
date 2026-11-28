use crate::{Cx, El, Env, Runtime, col, el, memo, text};

struct State {
    large: bool,
    tick: u32,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    if s.large {
        col().key("doc").size(800.0, 600.0).children((0..1500).map(|i| {
            memo(format!("row-{i}"), i, move || {
                el().key(format!("r{i}")).h(20.0).child(text(format!("Line number {i} in document")))
            })
        }))
    } else {
        col()
            .key("start")
            .size(800.0, 600.0)
            .child(text(format!("Start page ({})", s.tick)).key("title"))
    }
}

#[test]
fn runtime_rest_shrinks_retained_tables_prunes_exited_paths_and_preserves_paint_keep() {
    let mut rt = Runtime::new(State { large: true, tick: 0 }, view, (800.0, 600.0), Env::new(true, 1.0));
    #[cfg(debug_assertions)]
    rt.set_paint_keep_strict(true);
    rt.a11y_activate("Test");
    rt.frame(0.0);
    let _ = rt.a11y_update();
    assert!(rt.tree.nodes.len() > 1500);
    assert!(rt.tree.path_ids.len() > 1500);

    // Switch to the small start page and let exits finish.
    rt.update(|s, _| s.large = false);
    for _ in 0..120 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    let _ = rt.a11y_update();
    assert!(rt.tree.nodes.len() < 10);

    rt.rest();
    assert!(rt.tree.path_ids.len() <= 16, "exited paths pruned: {}", rt.tree.path_ids.len());
    assert!(rt.tree.nodes.capacity() <= 128, "nodes capacity shrank: {}", rt.tree.nodes.capacity());
    assert!(rt.tree.spare_nodes.capacity() <= 128, "spare_nodes shrank: {}", rt.tree.spare_nodes.capacity());
    assert!(rt.paint.inh_scratch.capacity() <= 128, "inh_scratch shrank: {}", rt.paint.inh_scratch.capacity());
    assert!(rt.text.cached_layout_count() <= 2, "only start-page layout kept: {}", rt.text.cached_layout_count());

    // Subsequent frame must render cleanly with zero PaintKeep self-check mismatches (strict mode).
    let before_inst = rt.instances.len();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.instances.len(), before_inst);

    // Mutating the start page after rest still commits and paints cleanly.
    rt.update(|s, _| s.tick += 1);
    rt.frame(1.0 / 60.0);
    assert!(!rt.instances.is_empty());
}

fn solid_pixels(rgba: [u8; 4]) -> crate::Pixels {
    let p = crate::Pixels::new();
    p.set(8, 8, rgba.repeat(64));
    p
}

#[test]
fn hidden_rest_drops_pixels_across_elements_memos_pills_menus_and_virtual_lists() {
    use std::sync::Arc;
    use crate::{GrowthSpec, Image, ImageMenu, InlineImage, PillStyle, Pixels, ScrollSpec, TextEdit, TextStyle, text_input};
    use crate::image::ImageCache;

    struct ImgState {
        direct: Vec<Pixels>,
        memo_px: Option<Pixels>,
        clipped_memo_px: Option<Pixels>,
        pill_px: Option<Pixels>,
        inline_px: Option<Pixels>,
        pill_fn_px: Option<Pixels>,
        menu_img: Option<Image>,
        virtual_px: Option<Pixels>,
        edit: TextEdit,
    }

    fn img_view(s: &ImgState, _: &mut Cx<ImgState>) -> El {
        let mut root = col().key("root").size(200.0, 200.0);
        for (i, px) in s.direct.iter().enumerate() {
            root = root.child(el().key(format!("thumb-{i}")).size(16.0, 16.0).pixels(px));
        }
        if let Some(px) = &s.memo_px {
            let px = px.clone();
            root = root.child(memo("memo-thumb", 1u64, move || el().key("m").size(16.0, 16.0).pixels(&px)));
        }
        if let Some(px) = &s.clipped_memo_px {
            let px = px.clone();
            root = root.child(
                el().key("zero-clip")
                    .size(0.0, 0.0)
                    .child(memo("clipped-memo", 1u64, move || el().key("cm").size(0.0, 0.0).pixels(&px))),
            );
        }
        if let Some(px) = &s.pill_px {
            root = root.child(text("p").key("pill-el").pill_with_image(0..1, PillStyle::default(), px.clone()));
        }
        if let Some(px) = &s.inline_px {
            root = root.child(text("i").key("inline-el").inline_image(0..1, InlineImage::new(px.clone(), 8.0, 8.0, 0.0)));
        }
        if let Some(px) = &s.pill_fn_px {
            let px = px.clone();
            root = root.child(
                text_input(&s.edit, TextStyle::new(14.0, 400.0))
                    .key("field")
                    .size(100.0, 24.0)
                    .pill_image(move |_| Some(px.clone())),
            );
        }
        if let Some(img) = &s.menu_img {
            root = root.child(el().key("menu-el").size(16.0, 16.0).pixels(img.pixels()).image_menu(ImageMenu::new(img)));
        }
        if let Some(px) = &s.virtual_px {
            let rows: Vec<El> = (0..40)
                .map(|i| {
                    let row = el().key(format!("vrow-{i}")).h(30.0);
                    if i == 39 { row.pixels(px) } else { row.child(text(format!("row {i}"))) }
                })
                .collect();
            root = root.child(
                col().key("vscroll")
                    .size(100.0, 60.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(col().key("vlist").growth(GrowthSpec::default()).virtualize(30.0).children(rows)),
            );
        }
        root
    }

    let cache = ImageCache::new(Arc::new(|| {}));
    let menu_img = cache.rgba(8, 8, [10, 20, 30, 255].repeat(64));
    let direct: Vec<Pixels> = (0..6).map(|i| solid_pixels([i as u8 * 30, 100, 200, 255])).collect();
    let memo_px = solid_pixels([200, 40, 20, 255]);
    let clipped_memo_px = solid_pixels([20, 200, 40, 255]);
    let pill_px = solid_pixels([20, 40, 200, 255]);
    let inline_px = solid_pixels([200, 200, 20, 255]);
    let pill_fn_px = solid_pixels([200, 20, 200, 255]);
    let virtual_px = solid_pixels([20, 200, 200, 255]);

    let mut weaks: Vec<(&'static str, std::sync::Weak<_>)> = direct
        .iter()
        .map(|p| ("direct", Arc::downgrade(&p.source)))
        .collect();
    weaks.extend([
        ("memo", Arc::downgrade(&memo_px.source)),
        ("clipped_memo", Arc::downgrade(&clipped_memo_px.source)),
        ("pill", Arc::downgrade(&pill_px.source)),
        ("inline", Arc::downgrade(&inline_px.source)),
        ("pill_fn", Arc::downgrade(&pill_fn_px.source)),
        ("menu_img", Arc::downgrade(&menu_img.pixels().source)),
        ("virtual", Arc::downgrade(&virtual_px.source)),
    ]);

    let state = ImgState {
        direct,
        memo_px: Some(memo_px),
        clipped_memo_px: Some(clipped_memo_px),
        pill_px: Some(pill_px),
        inline_px: Some(inline_px),
        pill_fn_px: Some(pill_fn_px),
        menu_img: Some(menu_img),
        virtual_px: Some(virtual_px),
        edit: TextEdit::new(""),
    };
    let mut rt = Runtime::new(state, img_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.images.pages.is_empty(), "visible frame populated image atlas pages");

    // Visible idle rest must keep visible images resident.
    rt.rest();
    assert!(!rt.images.pages.is_empty(), "visible rest keeps displayed image pages");
    assert!(Arc::strong_count(&rt.state.direct[0].source) > 1, "visible tree keeps its Pixels clones");

    // Hide + rest releases Rune's Pixels references and CPU atlas pages without needing another frame.
    rt.set_live_visible(false);
    rt.rest();
    assert!(rt.images.pages.is_empty(), "hidden rest drops CPU image atlas pages");
    assert!(rt.stale(), "hidden rest marks view dirty so the next show rebuilds images");

    // App drops all of its own Pixels/Image handles while hidden (without even calling update/frame).
    rt.state.direct.clear();
    rt.state.memo_px = None;
    rt.state.clipped_memo_px = None;
    rt.state.pill_px = None;
    rt.state.inline_px = None;
    rt.state.pill_fn_px = None;
    rt.state.menu_img = None;
    rt.state.virtual_px = None;
    drop(cache);

    for (label, weak) in &weaks {
        assert!(weak.upgrade().is_none(), "hidden window must not pin {label} Pixels after app drops it");
    }

    // Keeping a Pixels across hide -> show with unchanged state rebuilds and re-uploads cleanly.
    let kept = solid_pixels([12, 180, 90, 255]);
    rt.state.direct.push(kept);
    rt.set_live_visible(true);
    rt.frame(0.0);
    assert!(!rt.images.pages.is_empty());
    assert!(rt.instances.iter().any(|i| i.params[1] == crate::render::KIND_IMAGE));

    rt.set_live_visible(false);
    rt.rest();
    assert!(rt.images.pages.is_empty());
    assert_eq!(Arc::strong_count(&rt.state.direct[0].source), 1, "only app state holds Pixels while hidden");

    rt.set_live_visible(true);
    rt.frame(0.0);
    assert!(!rt.images.pages.is_empty(), "showing again rebuilds and re-uploads kept Pixels");
    assert!(rt.instances.iter().any(|i| i.params[1] == crate::render::KIND_IMAGE));
}

#[test]
fn hidden_state_change_via_update_or_poll_drops_pixels_without_rebuilding_view() {
    use std::sync::Arc;
    use crate::Pixels;

    struct Thumbs {
        items: Vec<Pixels>,
        clear_in_poll: bool,
    }
    fn thumb_view(s: &Thumbs, _: &mut Cx<Thumbs>) -> El {
        let mut root = col().key("grid").size(200.0, 200.0);
        for (i, px) in s.items.iter().enumerate() {
            root = root.child(el().key(format!("t{i}")).size(20.0, 20.0).pixels(px));
        }
        root
    }

    // 1. App drops its Pixels through Runtime::update while hidden (even without rest()).
    let thumbs: Vec<Pixels> = (0..4).map(|i| solid_pixels([i as u8 * 40, 80, 160, 255])).collect();
    let weaks: Vec<_> = thumbs.iter().map(|p| Arc::downgrade(&p.source)).collect();
    let mut rt = Runtime::new(Thumbs { items: thumbs, clear_in_poll: false }, thumb_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.images.pages.is_empty());

    rt.set_live_visible(false);
    let commits_before = rt.stats.commits;
    rt.update(|s, _| s.items.clear());
    assert_eq!(rt.stats.commits, commits_before, "hidden update does not rebuild the view");
    assert!(rt.images.pages.is_empty(), "hidden state change drops CPU atlas pages");
    for (i, w) in weaks.iter().enumerate() {
        assert!(w.upgrade().is_none(), "thumbnail {i} freed immediately on hidden update");
    }

    // 2. App drops its Pixels inside poll during hidden_wake while hidden.
    let thumbs2: Vec<Pixels> = (0..4).map(|i| solid_pixels([160, i as u8 * 40, 80, 255])).collect();
    let weaks2: Vec<_> = thumbs2.iter().map(|p| Arc::downgrade(&p.source)).collect();
    rt.set_live_visible(true);
    rt.update(|s, _| {
        s.items = thumbs2;
        s.clear_in_poll = false;
    });
    rt.frame(0.0);
    assert!(!rt.images.pages.is_empty());

    rt.poll = Some(|s, _, _| {
        if s.clear_in_poll && !s.items.is_empty() {
            s.items.clear();
            return true;
        }
        false
    });
    rt.set_live_visible(false);
    rt.state.clear_in_poll = true;
    let commits_before = rt.stats.commits;
    assert!(rt.hidden_wake(), "poll reported state change");
    assert_eq!(rt.stats.commits, commits_before, "hidden poll does not rebuild the view");
    assert!(rt.images.pages.is_empty(), "hidden poll state change drops CPU atlas pages");
    for (i, w) in weaks2.iter().enumerate() {
        assert!(w.upgrade().is_none(), "thumbnail {i} freed immediately on hidden poll");
    }
}

#[test]
fn hidden_rest_without_images_leaves_memos_and_dirty_untouched() {
    let mut rt = Runtime::new(State { large: false, tick: 0 }, view, (800.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.stale());

    rt.set_live_visible(false);
    rt.rest();
    assert!(!rt.stale(), "image-free hidden rest must not force a rebuild on show");

    // Also with memos (still no images).
    let mut rt_m = Runtime::new(State { large: true, tick: 0 }, view, (800.0, 600.0), Env::new(true, 1.0));
    rt_m.frame(0.0);
    let memos_before = rt_m.commit.memo_len();
    assert!(memos_before > 0);
    rt_m.set_live_visible(false);
    rt_m.rest();
    assert!(!rt_m.stale(), "image-free memoized hidden rest must not mark view dirty");
    assert_eq!(rt_m.commit.memo_len(), memos_before, "image-free memos stay cached across hidden rest");
}
