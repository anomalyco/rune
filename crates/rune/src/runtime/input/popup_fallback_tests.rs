//! Tests for `App::on_popup_menu`, `PopupMenuRequest`, `Tx::resolve_popup_menu`, and the
//! `PopupFallbackState` state machine.

use super::{PopupFallbackEffect, PopupFallbackEvent, PopupFallbackState, step};
use crate::{
    App, AppSpec, Cx, El, Env, Key, KeyEvent, OverlayOptions, OverlaySpec, PointerPress,
    PopupAnchor, PopupEntry, PopupItem, PopupMenu, PopupMenuRequest, Runtime, TextEdit,
    TextStyle, Tx, col, el, text, text_input,
};

#[test]
fn popup_fallback_state_machine_covers_every_transition_and_stale_resolves() {
    let run = |state: &mut PopupFallbackState, seq: &mut u64, ev: PopupFallbackEvent| -> Vec<PopupFallbackEffect> {
        step(state, seq, ev).into_iter().collect()
    };
    let mut state = PopupFallbackState::Idle;
    let mut seq = 0;

    // Stale resolve / dismiss / supersede / post-commit while Idle change nothing.
    for ev in [
        PopupFallbackEvent::Resolve { picked: Some("a".into()), defer: false },
        PopupFallbackEvent::Resolve { picked: None, defer: false },
        PopupFallbackEvent::PostCommit,
        PopupFallbackEvent::Supersede,
        PopupFallbackEvent::Dismiss,
    ] {
        assert!(run(&mut state, &mut seq, ev).is_empty());
        assert_eq!(state, PopupFallbackState::Idle);
    }

    // Open -> immediate Resolve(Some(id)) -> Idle; duplicate Open while Open is ignored.
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Open { anchor_key: Some("btn".into()), field_path: None, queued_behind: 0 }),
        vec![PopupFallbackEffect::Deliver],
    );
    assert_eq!(seq, 1);
    assert!(run(&mut state, &mut seq, PopupFallbackEvent::Open { anchor_key: None, field_path: None, queued_behind: 0 }).is_empty());
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Resolve { picked: Some("dark".into()), defer: false }),
        vec![PopupFallbackEffect::Resolve(Some("dark".into()))],
    );
    assert_eq!(state, PopupFallbackState::Idle);
    // Second resolve after already resolved changes nothing.
    assert!(run(&mut state, &mut seq, PopupFallbackEvent::Resolve { picked: Some("light".into()), defer: false }).is_empty());

    // Open -> deferred pick -> PostCommit -> Resolve(Some(id)); second pick while deferred ignored.
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Open { anchor_key: None, field_path: Some("0/field".into()), queued_behind: 0 }),
        vec![PopupFallbackEffect::Deliver],
    );
    assert!(run(&mut state, &mut seq, PopupFallbackEvent::Resolve { picked: Some("paste".into()), defer: true }).is_empty());
    assert!(matches!(state, PopupFallbackState::DeferredPick { token: 2, ref id, .. } if id == "paste"));
    assert!(run(&mut state, &mut seq, PopupFallbackEvent::Resolve { picked: Some("copy".into()), defer: true }).is_empty());
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::PostCommit),
        vec![PopupFallbackEffect::Resolve(Some("paste".into()))],
    );
    assert_eq!(state, PopupFallbackState::Idle);

    // Dismissal (None) never defers even when defer=true, and overrides a DeferredPick.
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Open { anchor_key: None, field_path: None, queued_behind: 0 }),
        vec![PopupFallbackEffect::Deliver],
    );
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Resolve { picked: None, defer: true }),
        vec![PopupFallbackEffect::Resolve(None)],
    );
    assert_eq!(state, PopupFallbackState::Idle);

    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Open { anchor_key: None, field_path: None, queued_behind: 0 }),
        vec![PopupFallbackEffect::Deliver],
    );
    assert!(run(&mut state, &mut seq, PopupFallbackEvent::Resolve { picked: Some("ok".into()), defer: true }).is_empty());
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Dismiss),
        vec![PopupFallbackEffect::Resolve(None)],
    );
    assert_eq!(state, PopupFallbackState::Idle);

    // Supersede while Open resolves the open one with None.
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Open { anchor_key: None, field_path: None, queued_behind: 0 }),
        vec![PopupFallbackEffect::Deliver],
    );
    assert_eq!(
        run(&mut state, &mut seq, PopupFallbackEvent::Supersede),
        vec![PopupFallbackEffect::Resolve(None)],
    );
    assert_eq!(state, PopupFallbackState::Idle);
}

#[test]
fn fallback_handles_point_key_layout_wait_zoom_submenus_invalid_ids_and_stale_resolves() {
    #[derive(Default)]
    struct S {
        show_extra: bool,
        requests: Vec<PopupMenuRequest>,
        picks: Vec<Option<String>>,
    }

    fn view(s: &S, cx: &mut Cx<S>) -> El {
        cx.zoom(1.25);
        col().size(400.0, 240.0)
            .child(el().key("btn").absolute(20.0, 30.0).size(100.0, 28.0).on_press_tx(|_: &mut S, tx: &mut Tx| {
                let menu = PopupMenu::picker([("system", "System"), ("dark", "Dark")], "system");
                tx.popup_menu(menu, "btn", |s: &mut S, picked, _| s.picks.push(picked.map(str::to_owned)));
            }))
            .child(el().key("spawn").absolute(140.0, 30.0).size(80.0, 28.0).on_press_tx(|s: &mut S, tx: &mut Tx| {
                s.show_extra = true;
                let menu = PopupMenu::new().item(PopupItem::new("go", "Go"));
                tx.popup_menu(menu, "extra", |s: &mut S, picked, _| s.picks.push(picked.map(str::to_owned)));
            }))
            .children(s.show_extra.then(|| el().key("extra").absolute(240.0, 40.0).size(90.0, 24.0)))
            .child(el().key("pad").absolute(20.0, 80.0).size(200.0, 80.0).on_secondary_press(|_: &mut S, p: PointerPress, tx: &mut Tx| {
                let menu = PopupMenu::new()
                    .item(PopupItem::new("copy", "Copy"))
                    .item(PopupItem::new("paste", "Paste").enabled(false))
                    .separator()
                    .submenu("Share", PopupMenu::new().item(PopupItem::new("mail", "Mail")));
                tx.popup_menu(menu, p, |s: &mut S, picked, _| s.picks.push(picked.map(str::to_owned)));
            }))
    }

    let mut rt = App::new(AppSpec::new("fallback-core", (400.0, 240.0), S::default, view))
        .on_popup_menu(|s: &mut S, req: PopupMenuRequest, _| s.requests.push(req))
        .headless_runtime((400.0, 240.0), Env::new(true, 1.0));
    rt.core_mut().host.force_popup_menu_fallback = true;
    rt.frame(0.0);
    rt.frame(0.0);

    // 1. Key-anchored picker: delivers anchor_rect, min_width, positioned in view points (divided by zoom).
    let (bx, by) = rt.find("btn").unwrap();
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), 1);
    let req = &rt.state.requests[0];
    assert_eq!(req.anchor(), &PopupAnchor::Key("btn".into()));
    assert_eq!(req.anchor_rect(), Some([20.0, 30.0, 100.0, 28.0]));
    assert_eq!(req.point(), PointerPress { x: 20.0, y: 30.0 });
    assert_eq!(req.min_width(), 100.0);
    assert_eq!(req.positioned(), Some("system"));
    rt.update(|_, tx| tx.resolve_popup_menu(Some("dark")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [Some("dark".into())]);
    // Stale resolve after close does nothing.
    rt.update(|_, tx| tx.resolve_popup_menu(Some("system")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [Some("dark".into())]);

    // 2. Point-anchored context menu: rejects disabled/unknown ids, accepts submenu pick.
    rt.secondary_down(60.0, 110.0);
    rt.secondary_up(60.0, 110.0);
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), 2);
    let ctx_req = &rt.state.requests[1];
    assert_eq!(ctx_req.anchor_rect(), None);
    assert_eq!(ctx_req.min_width(), 0.0);
    assert_eq!(ctx_req.positioned(), None);
    assert!((ctx_req.point().x - 60.0).abs() < 0.01 && (ctx_req.point().y - 110.0).abs() < 0.01);
    rt.update(|_, tx| tx.resolve_popup_menu(Some("paste")));
    rt.update(|_, tx| tx.resolve_popup_menu(Some("unknown")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks.len(), 1, "disabled and unknown picks must leave menu open");
    rt.update(|_, tx| tx.resolve_popup_menu(Some("mail")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [Some("dark".into()), Some("mail".into())]);

    // 3. Key-anchored menu whose anchor element mounts in the same handler waits for layout and delivers.
    let (sx, sy) = rt.find("spawn").unwrap();
    rt.pointer_down(sx, sy);
    rt.pointer_up(sx, sy);
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), 3);
    let extra_req = &rt.state.requests[2];
    assert_eq!(extra_req.anchor_rect(), Some([240.0, 40.0, 90.0, 24.0]));
    assert_eq!(extra_req.point(), PointerPress { x: 240.0, y: 64.0 });

    // 4. Removing the anchor element while the menu is open auto-dismisses with None once.
    rt.update(|s, _| s.show_extra = false);
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [Some("dark".into()), Some("mail".into()), None]);
    // Resolving after auto-dismissal changes nothing.
    rt.update(|_, tx| tx.resolve_popup_menu(Some("go")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks.len(), 3);
}

#[test]
fn queued_batch_supersede_blur_and_window_cancel_resolve_each_menu_once() {
    #[derive(Default)]
    struct S {
        requests: Vec<PopupMenuRequest>,
        picks: Vec<(usize, Option<String>)>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El { el().key("root").size(300.0, 200.0) }

    let mut rt = App::new(AppSpec::new("fallback-order", (300.0, 200.0), S::default, view))
        .on_popup_menu(|s: &mut S, req: PopupMenuRequest, _| s.requests.push(req))
        .headless_runtime((300.0, 200.0), Env::new(true, 1.0));
    rt.core_mut().host.force_popup_menu_fallback = true;
    rt.frame(0.0);

    // Batch of two menus queued together: first opens, second opens after first resolves.
    rt.update(|_, tx| {
        tx.popup_menu(PopupMenu::new().item(PopupItem::new("a", "A")), (10.0, 10.0), |s: &mut S, p, _| s.picks.push((1, p.map(str::to_owned))));
        tx.popup_menu(PopupMenu::new().item(PopupItem::new("b", "B")), (20.0, 20.0), |s: &mut S, p, _| s.picks.push((2, p.map(str::to_owned))));
    });
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), 1);
    assert!(rt.state.picks.is_empty());
    rt.update(|_, tx| tx.resolve_popup_menu(Some("a")));
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [(1, Some("a".into()))]);
    assert_eq!(rt.state.requests.len(), 2, "second queued menu opens after first resolves");

    // A fresh request while menu 2 is already open supersedes menu 2 (dismissing it with None) and opens menu 3.
    rt.update(|_, tx| {
        tx.popup_menu(PopupMenu::new().item(PopupItem::new("c", "C")), (30.0, 30.0), |s: &mut S, p, _| s.picks.push((3, p.map(str::to_owned))));
    });
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [(1, Some("a".into())), (2, None)]);
    assert_eq!(rt.state.requests.len(), 3);

    // Window blur dismisses menu 3 with None.
    rt.window_focus(true);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [(1, Some("a".into())), (2, None), (3, None)]);

    // Window close/cancel dismisses open menu 4 with None.
    rt.window_focus(true);
    rt.update(|_, tx| {
        tx.popup_menu(PopupMenu::new().item(PopupItem::new("d", "D")), (40.0, 40.0), |s: &mut S, p, _| s.picks.push((4, p.map(str::to_owned))));
    });
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), 4);
    rt.cancel_popups();
    rt.frame(0.0);
    assert_eq!(rt.state.picks, [(1, Some("a".into())), (2, None), (3, None), (4, None)]);
    // Invalid request (missing key / duplicate id) answers None immediately without calling on_popup_menu.
    rt.update(|_, tx| {
        tx.popup_menu(PopupMenu::new().item(PopupItem::new("x", "X")), "missing-key", |s: &mut S, p, _| s.picks.push((5, p.map(str::to_owned))));
    });
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), 4);
    assert_eq!(rt.state.picks.last(), Some(&(5, None)));
}

#[test]
fn selection_menu_and_field_menu_route_through_on_popup_menu_while_on_field_menu_keeps_precedence() {
    #[derive(Default)]
    struct S {
        edit: TextEdit,
        popup_menu: Option<PopupMenuRequest>,
        field_menus: usize,
        popup_menus: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = col().size(400.0, 240.0).gap(12.0)
            .child(text("Selectable line").key("sel").selectable(true))
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 32.0).on_edit(|s: &mut S| &mut s.edit));
        if let Some(req) = &s.popup_menu {
            // Mount a modal menu covering the view to verify #2082 post-commit modal deferral on Paste/Cut/Copy.
            let mut surface = col().key("modal-menu").role("menu").focus_group().absolute(req.point().x, req.point().y)
                .modal_tx(true, |s: &mut S, tx: &mut Tx| {
                    s.popup_menu = None;
                    tx.resolve_popup_menu(None);
                });
            for entry in req.menu().entries() {
                if let PopupEntry::Item(item) = entry {
                    let id = item.id().to_owned();
                    surface = surface.child(el().key(format!("pm-{id}")).size(120.0, 24.0).focusable(item.is_enabled()).on_press_tx(move |s: &mut S, tx: &mut Tx| {
                        s.popup_menu = None;
                        tx.resolve_popup_menu(Some(&id));
                    }));
                }
            }
            root = root.child(surface);
        }
        root
    }

    // 1. Only on_popup_menu registered: handles BOTH selectable text Copy AND text-field Cut/Paste through modal!
    let board = crate::clipboard::memory();
    let mut rt = App::new(AppSpec::new("unified", (400.0, 240.0), || S { edit: TextEdit::new("hello world"), ..S::default() }, view))
        .on_popup_menu(|s: &mut S, req: PopupMenuRequest, _| {
            s.popup_menus += 1;
            s.popup_menu = Some(req);
        })
        .headless_runtime((400.0, 240.0), Env::new(true, 1.0));
    rt.use_clipboard(&board);
    rt.core_mut().host.force_popup_menu_fallback = true;
    rt.frame(0.0);

    // Select text and right-click selectable text -> Copy menu delivered to on_popup_menu.
    let (_, sy) = rt.find("sel").unwrap();
    rt.pointer_down(4.0, sy);
    rt.pointer_move(60.0, sy);
    rt.pointer_up(60.0, sy);
    rt.frame(0.0);
    rt.secondary_down(25.0, sy);
    rt.secondary_up(25.0, sy);
    rt.frame(0.0);
    assert_eq!(rt.state.popup_menus, 1, "selection_menu routes to on_popup_menu");
    assert!(rt.state.popup_menu.as_ref().unwrap().menu().find("copy").is_some_and(|i| i.is_enabled()));
    let (cx, cy) = rt.find("pm-copy").unwrap();
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(0.0);
    assert!(rt.state.popup_menu.is_none());
    assert!(board.content().is_some_and(|c| !c.text().is_empty()), "selection Copy copied text");

    // Text field without on_field_menu routes through on_popup_menu; Cut & Paste succeed across modal unmount.
    rt.update(|s, _| { s.edit.focused = true; s.edit.set_selection(0, 5); });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    assert_eq!(rt.state.popup_menus, 2, "field_menu routes to on_popup_menu when on_field_menu is unset");
    let (cutx, cuty) = rt.find("pm-cut").unwrap();
    rt.pointer_down(cutx, cuty);
    rt.pointer_up(cutx, cuty);
    rt.frame(0.0);
    assert_eq!(board.content().as_ref().map(|c| c.text()), Some("hello"));
    assert_eq!(rt.state.edit.text, " world");

    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    let (px, py) = rt.find("pm-paste").unwrap();
    rt.pointer_down(px, py);
    rt.pointer_up(px, py);
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "hello world");

    // 2. When BOTH on_field_menu and on_popup_menu are registered, on_field_menu keeps precedence for fields.
    let mut both = App::new(AppSpec::new("both", (400.0, 240.0), || S { edit: TextEdit::new("abc"), ..S::default() }, view))
        .on_field_menu(|s: &mut S, _, _| s.field_menus += 1)
        .on_popup_menu(|s: &mut S, _, _| s.popup_menus += 1)
        .headless_runtime((400.0, 240.0), Env::new(true, 1.0));
    both.core_mut().host.force_field_menu_fallback = true;
    both.core_mut().host.force_popup_menu_fallback = true;
    both.update(|s, _| s.edit.focused = true);
    both.frame(0.0);
    both.key(KeyEvent::from(Key::ContextMenu));
    both.frame(0.0);
    assert_eq!(both.state.field_menus, 1, "on_field_menu keeps precedence for text fields");
    assert_eq!(both.state.popup_menus, 0);
}

#[test]
fn platform_gating_opt_in_and_overlay_wiring() {
    #[derive(Default)]
    struct S { requests: usize, picks: Vec<Option<String>> }
    fn view(_: &S, _: &mut Cx<S>) -> El { el().key("root").size(200.0, 100.0) }

    // On popup_menu_supported() platforms (macOS, Windows) without test seam, on_popup_menu never runs.
    if crate::popup_menu_supported() {
        let mut rt = App::new(AppSpec::new("native", (200.0, 100.0), S::default, view))
            .on_popup_menu(|s: &mut S, _, _| s.requests += 1)
            .headless_runtime((200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.popup_menu(PopupMenu::new().item(PopupItem::new("a", "A")), (10.0, 10.0), |_: &mut S, _, _| {}));
        rt.frame(0.0);
        assert_eq!(rt.state.requests, 0, "on_popup_menu must never run when popup_menu_supported() is true");
        assert!(rt.popup_menu().is_some(), "native menu stays queued for native Presenter");
    }

    // On Linux (where !popup_menu_supported() && !wasm32), on_popup_menu activates WITHOUT test seam.
    if !crate::popup_menu_supported() && !cfg!(target_arch = "wasm32") {
        let mut rt = App::new(AppSpec::new("linux-real", (200.0, 100.0), S::default, view))
            .on_popup_menu(|s: &mut S, _, tx| {
                s.requests += 1;
                tx.resolve_popup_menu(Some("ok"));
            })
            .headless_runtime((200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.popup_menu(PopupMenu::new().item(PopupItem::new("ok", "OK")), (10.0, 10.0), |s: &mut S, p, _| s.picks.push(p.map(str::to_owned))));
        rt.frame(0.0);
        assert_eq!(rt.state.requests, 1, "Linux activates on_popup_menu natively without test seam");
        assert_eq!(rt.state.picks, [Some("ok".into())]);
    }

    // Opt-in: without on_popup_menu, Presenter::open_core on Linux behaves as before (answers None).
    let mut plain: Runtime<S> = App::new(AppSpec::new("plain", (200.0, 100.0), S::default, view))
        .headless_runtime((200.0, 100.0), Env::new(true, 1.0));
    plain.frame(0.0);
    plain.update(|_, tx| tx.popup_menu(PopupMenu::new().item(PopupItem::new("a", "A")), (10.0, 10.0), |s: &mut S, p, _| s.picks.push(p.map(str::to_owned))));
    plain.frame(0.0);
    assert!(plain.popup_menu().is_some());
    assert!(plain.state.picks.is_empty());

    // ConfiguredOverlay wires on_popup_menu.
    let overlay = OverlaySpec::new("ov", (200.0, 100.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .on_popup_menu(|_: &mut (), _: PopupMenuRequest, _: &mut Tx| {});
    assert!(overlay.runtime.on_popup_menu.is_some());
}

#[test]
fn default_link_context_menu_from_2339_routes_through_on_popup_menu_with_zero_special_code() {
    #[derive(Default)]
    struct S {
        opened: Vec<String>,
        popup: Option<PopupMenuRequest>,
    }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        col().size(400.0, 120.0).child(
            text("Visit Rune docs")
                .key("link-line")
                .link(crate::Link::new(6..15, "https://example.com/rune"))
                .on_link(|s: &mut S, id: &str, _| s.opened.push(id.into())),
        )
    }

    let board = crate::clipboard::memory();
    let mut rt = App::new(AppSpec::new("link-fallback", (400.0, 120.0), S::default, view))
        .on_popup_menu(|s: &mut S, req: PopupMenuRequest, _| s.popup = Some(req))
        .headless_runtime((400.0, 120.0), Env::new(true, 1.0));
    rt.use_clipboard(&board);
    rt.core_mut().host.force_popup_menu_fallback = true;
    rt.frame(0.0);

    let (_, ly) = rt.find("link-line").unwrap();
    rt.secondary_down(60.0, ly);
    rt.secondary_up(60.0, ly);
    rt.frame(0.0);
    let req = rt.state.popup.as_ref().expect("default link menu routed to on_popup_menu");
    assert!(req.menu().find("open-link").is_some());
    assert!(req.menu().find("copy-link").is_some());
    rt.update(|s, tx| { s.popup = None; tx.resolve_popup_menu(Some("copy-link")); });
    rt.frame(0.0);
    assert_eq!(board.content().as_ref().map(|c| c.text()), Some("https://example.com/rune"));

    rt.secondary_down(60.0, ly);
    rt.secondary_up(60.0, ly);
    rt.frame(0.0);
    rt.update(|s, tx| { s.popup = None; tx.resolve_popup_menu(Some("open-link")); });
    rt.frame(0.0);
    assert_eq!(rt.state.opened, ["https://example.com/rune"]);
}
