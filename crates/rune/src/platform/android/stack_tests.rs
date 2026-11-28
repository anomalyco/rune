//! End-to-end host tests for the Android in-activity page stack (`open_window`, `App::window`,
//! `WindowHandle` non-winit routing, Escape-first Back navigation, `on_close_request`,
//! `on_close`, `Tx::close_window`, and environment sync across pages).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::tok::type_;
use crate::{
    App, AppSpec, CloseResponse, ColorScheme, Key, KeyEvent, Mods, TextEdit, TouchPhase, Tx,
    WindowHandle, WindowOptions, WindowSpec, col, el, open_window, text_input,
};

use super::stack::{BackOutcome, PageStack};

fn make_stack() -> (PageStack, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = Arc::clone(&wakes);
    let stack = PageStack::new(Arc::new(move || {
        wakes_cb.fetch_add(1, Ordering::Relaxed);
    }));
    (stack, wakes)
}

fn clear_thread_opened() {
    let _ = crate::windows::take_opened();
}

#[test]
fn open_window_pushes_page_over_root_and_back_pops_running_on_close() {
    clear_thread_opened();
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let prefs_handle_slot: Arc<Mutex<Option<WindowHandle>>> = Arc::new(Mutex::new(None));

    #[derive(Default)]
    struct MainState {
        events: Arc<Mutex<Vec<&'static str>>>,
        prefs_handle: Arc<Mutex<Option<WindowHandle>>>,
    }
    #[derive(Default)]
    struct PrefsState {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    let ev_main = Arc::clone(&events);
    let handle_slot = Arc::clone(&prefs_handle_slot);
    let main_app = App::new(AppSpec::new(
        "Main",
        (360.0, 800.0),
        MainState::default,
        |_, _| {
            col().w_full().h_full().child(
                el().key("gear")
                    .size(48.0, 48.0)
                    .focusable(true)
                    .on_press(|s: &mut MainState| {
                        let ev_close = Arc::clone(&s.events);
                        let ev_created = Arc::clone(&s.events);
                        let ev_window = Arc::clone(&s.events);
                        let ev_ready = Arc::clone(&s.events);
                        let spec = WindowSpec::new(
                            "prefs",
                            AppSpec::new(
                                "Preferences",
                                (360.0, 800.0),
                                PrefsState::default,
                                |_, _| col().w_full().h_full().child(el().key("prefs_body").size(100.0, 40.0)),
                            ),
                        )
                        .state(PrefsState { events: ev_close })
                        .on_created(move |_| ev_created.lock().unwrap().push("prefs:created"))
                        .on_window(move |_| ev_window.lock().unwrap().push("prefs:window"))
                        .on_ready(move |_| ev_ready.lock().unwrap().push("prefs:ready"))
                        .on_close(|p: &mut PrefsState, _| {
                            p.events.lock().unwrap().push("prefs:close");
                        });
                        let handle = open_window(spec);
                        *s.prefs_handle.lock().unwrap() = Some(handle);
                    }),
            )
        },
    ))
    .init(move || MainState { events: ev_main, prefs_handle: handle_slot })
    .on_visibility(|s: &mut MainState, visible, _| {
        s.events
            .lock()
            .unwrap()
            .push(if visible { "main:visible" } else { "main:hidden" });
    });

    let (mut stack, _) = make_stack();
    stack.set_viewport((360.0, 800.0), 3.0);
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);
    stack.step_top_frame(0.0);
    assert_eq!(stack.visible_keys(), ["main"]);

    // Tap the gear button -> open_window pushes Preferences full-screen over Main.
    assert!(stack.tap_top("gear"));
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);
    let prefs_handle = prefs_handle_slot.lock().unwrap().clone().expect("prefs handle recorded");
    assert!(!prefs_handle.is_ready(), "not ready before first frame");
    assert_eq!(prefs_handle.scale(), Some(3.0));

    // First frame on Preferences sets ready and fires on_ready.
    stack.step_top_frame(1.0 / 60.0);
    assert!(prefs_handle.is_ready());

    // Back pops Preferences, runs on_close, drops it, and returns to Main.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.visible_keys(), ["main"]);
    assert!(!stack.has_live_key("prefs"), "non-keep_running page is dropped on pop");
    assert!(!prefs_handle.is_ready());
    assert_eq!(prefs_handle.scale(), None);

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "main:hidden",
            "prefs:created",
            "prefs:window",
            "prefs:ready",
            "prefs:close",
            "main:visible",
        ],
    );

    // Re-tapping gear opens a fresh Preferences page cleanly (old key was freed).
    assert!(stack.tap_top("gear"));
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);
}

#[test]
fn back_on_page_runs_escape_first_then_on_close_request_then_pops() {
    clear_thread_opened();
    let close_req_count = Arc::new(AtomicUsize::new(0));
    let close_count = Arc::new(AtomicUsize::new(0));
    let dirty = Arc::new(AtomicBool::new(true));

    struct Prefs {
        note: TextEdit,
        dirty: Arc<AtomicBool>,
        close_req_count: Arc<AtomicUsize>,
        close_count: Arc<AtomicUsize>,
        confirm_shown: bool,
    }
    impl Default for Prefs {
        fn default() -> Self {
            Self {
                note: TextEdit::new(""),
                dirty: Arc::new(AtomicBool::new(false)),
                close_req_count: Arc::new(AtomicUsize::new(0)),
                close_count: Arc::new(AtomicUsize::new(0)),
                confirm_shown: false,
            }
        }
    }

    let main_app = App::new(AppSpec::new("Main", (360.0, 800.0), || (), |_, _| el().w_full().h_full()));
    let (mut stack, _) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);
    stack.step_top_frame(0.0);

    let _handle = open_window(
        WindowSpec::new(
            "prefs",
            AppSpec::new(
                "Preferences",
                (360.0, 800.0),
                Prefs::default,
                |s, _| {
                    col().w_full().h_full().child(
                        text_input(&s.note, type_::BODY)
                            .key("note")
                            .size(240.0, 44.0)
                            .on_edit(|s: &mut Prefs| &mut s.note),
                    )
                },
            ),
        )
        .state(Prefs {
            note: TextEdit::new("draft"),
            dirty: Arc::clone(&dirty),
            close_req_count: Arc::clone(&close_req_count),
            close_count: Arc::clone(&close_count),
            confirm_shown: false,
        })
        .on_close_request(|s: &mut Prefs, _| {
            s.close_req_count.fetch_add(1, Ordering::Relaxed);
            if s.dirty.load(Ordering::Relaxed) {
                s.confirm_shown = true;
                CloseResponse::Defer
            } else {
                CloseResponse::Close
            }
        })
        .on_close(|s: &mut Prefs, _| {
            s.close_count.fetch_add(1, Ordering::Relaxed);
        }),
    );
    assert!(stack.pump());
    stack.step_top_frame(0.0);
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);

    // Focus the text input on the stacked page.
    assert!(stack.tap_top("note"));
    assert!(stack.top_has_focus());

    // Back #1: Key::Escape blurs the focused text input and is consumed -> stays on prefs,
    // without calling on_close_request or on_close.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert!(!stack.top_has_focus());
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);
    assert_eq!(close_req_count.load(Ordering::Relaxed), 0);
    assert_eq!(close_count.load(Ordering::Relaxed), 0);

    // Back #2: Escape is NOT consumed -> consults on_close_request -> returns Defer -> stays.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);
    assert_eq!(close_req_count.load(Ordering::Relaxed), 1);
    assert_eq!(close_count.load(Ordering::Relaxed), 0);

    // Back #3: dirty cleared -> on_close_request returns Close -> runs on_close and pops.
    dirty.store(false, Ordering::Relaxed);
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.visible_keys(), ["main"]);
    assert_eq!(close_req_count.load(Ordering::Relaxed), 2);
    assert_eq!(close_count.load(Ordering::Relaxed), 1);

    // Back #4: on root with nothing consuming Escape -> Unhandled (activity goes back).
    assert_eq!(stack.handle_back(), BackOutcome::Unhandled);
}

#[test]
fn back_on_root_leaves_activity_when_focused_button_does_not_consume_escape() {
    clear_thread_opened();
    struct RootState {
        modal_open: bool,
        note: TextEdit,
    }
    let main_app = App::new(AppSpec::new(
        "Root",
        (360.0, 800.0),
        || RootState { modal_open: false, note: TextEdit::new("") },
        |s, _| {
            let mut root = col()
                .w_full()
                .h_full()
                .child(
                    el().key("btn")
                        .size(120.0, 44.0)
                        .focusable(true)
                        .on_press(|_: &mut RootState| {}),
                )
                .child(
                    el().key("open_modal")
                        .size(120.0, 44.0)
                        .focusable(true)
                        .on_press(|s: &mut RootState| s.modal_open = true),
                )
                .child(
                    text_input(&s.note, type_::BODY)
                        .key("field")
                        .size(200.0, 44.0)
                        .on_edit(|s: &mut RootState| &mut s.note),
                );
            if s.modal_open {
                root = root.child(
                    el().key("dialog")
                        .size(200.0, 120.0)
                        .modal(false, |s: &mut RootState| s.modal_open = false),
                );
            }
            root
        },
    ));

    let (mut stack, _) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);
    stack.step_top_frame(0.0);

    // Tapping a plain button gives it focus, but Escape is NOT consumed -> Back leaves activity (#2690).
    assert!(stack.tap_top("btn"));
    assert!(stack.top_has_focus());
    assert_eq!(stack.handle_back(), BackOutcome::Unhandled);

    // Tapping a text input: Escape blurs it -> consumed; next Back leaves activity.
    assert!(stack.tap_top("field"));
    assert!(stack.top_has_focus());
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert!(!stack.top_has_focus());
    assert_eq!(stack.handle_back(), BackOutcome::Unhandled);

    // Opening a modal: Escape dismisses modal -> consumed; next Back leaves activity.
    assert!(stack.tap_top("open_modal"));
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.handle_back(), BackOutcome::Unhandled);
}

#[test]
fn window_handle_close_focus_hide_open_and_tx_close_window_work() {
    clear_thread_opened();
    let close_req_a = Arc::new(AtomicUsize::new(0));
    let close_a = Arc::new(AtomicUsize::new(0));
    let close_b = Arc::new(AtomicUsize::new(0));

    #[derive(Default)]
    struct PageA {
        req: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }
    #[derive(Default)]
    struct PageB {
        closed: Arc<AtomicUsize>,
    }

    let main_app = App::new(AppSpec::new("Main", (360.0, 800.0), || (), |_, _| el().w_full().h_full()));
    let (mut stack, wakes) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);
    stack.step_top_frame(0.0);

    let handle_a = open_window(
        WindowSpec::new(
            "a",
            AppSpec::new("A", (360.0, 800.0), PageA::default, |_, _| el().w_full().h_full()),
        )
        .state(PageA { req: Arc::clone(&close_req_a), closed: Arc::clone(&close_a) })
        .on_close_request(|s: &mut PageA, _| {
            s.req.fetch_add(1, Ordering::Relaxed);
            CloseResponse::Defer
        })
        .on_close(|s: &mut PageA, _| {
            s.closed.fetch_add(1, Ordering::Relaxed);
        }),
    );

    let handle_b = open_window(
        WindowSpec::new(
            "b",
            AppSpec::new(
                "B",
                (360.0, 800.0),
                PageB::default,
                |_, _| {
                    col().w_full().h_full().child(
                        el().key("done")
                            .size(100.0, 44.0)
                            .on_press_tx(|_: &mut PageB, tx: &mut Tx| tx.close_window()),
                    )
                },
            ),
        )
        .state(PageB { closed: Arc::clone(&close_b) })
        .on_close(|s: &mut PageB, _| {
            s.closed.fetch_add(1, Ordering::Relaxed);
        }),
    );

    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "a", "b"]);

    // focus() brings an already-visible buried page to the top and wakes the host.
    let wakes_before = wakes.load(Ordering::Relaxed);
    handle_a.focus();
    assert!(wakes.load(Ordering::Relaxed) > wakes_before);
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "b", "a"]);

    // hide() removes from visible stack without running on_close.
    handle_a.hide();
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "b"]);
    assert_eq!(close_a.load(Ordering::Relaxed), 0);
    // focus() on a hidden page does not reopen it; open() does.
    handle_a.focus();
    assert!(!stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "b"]);
    handle_a.open();
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "b", "a"]);

    // Programmatic WindowHandle::close() bypasses on_close_request, runs on_close, and drops.
    handle_a.close();
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "b"]);
    assert_eq!(close_req_a.load(Ordering::Relaxed), 0, "WindowHandle::close bypasses on_close_request");
    assert_eq!(close_a.load(Ordering::Relaxed), 1);
    assert!(!stack.has_live_key("a"));

    // Tx::close_window() inside a handler on page B closes B and runs on_close.
    assert!(stack.tap_top("done"));
    assert_eq!(stack.visible_keys(), ["main"]);
    assert_eq!(close_b.load(Ordering::Relaxed), 1);
    assert!(!stack.has_live_key("b"));
    // Stale handle close after drop is a harmless no-op.
    handle_b.close();
    assert!(!stack.pump());
}

#[test]
fn pre_bind_window_handle_close_queued_before_pump_applies_once_bound() {
    clear_thread_opened();
    let closed = Arc::new(AtomicUsize::new(0));
    let main_app = App::new(AppSpec::new("Main", (360.0, 800.0), || (), |_, _| el().w_full().h_full()));
    let (mut stack, _) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);

    let handle = open_window(
        WindowSpec::new("ephemeral", AppSpec::new("E", (360.0, 800.0), Arc::<AtomicUsize>::default, |_, _| el()))
            .state(Arc::clone(&closed))
            .on_close(|c: &mut Arc<AtomicUsize>, _| { c.fetch_add(1, Ordering::Relaxed); }),
    );
    // Called immediately before the host binds the handle.
    handle.close();
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main"]);
    assert!(!stack.has_live_key("ephemeral"));
    assert_eq!(closed.load(Ordering::Relaxed), 1);
}

#[test]
fn app_window_secondary_retains_state_across_close_and_reopen() {
    clear_thread_opened();
    let close_count = Arc::new(AtomicUsize::new(0));
    let seen_counter = Arc::new(AtomicUsize::new(0));

    #[derive(Default)]
    struct Prefs {
        count: usize,
        closes: Arc<AtomicUsize>,
        seen: Arc<AtomicUsize>,
    }

    let mut main_app = App::new(AppSpec::new("Main", (360.0, 800.0), || (), |_, _| el().w_full().h_full()));
    let (prefs_app, prefs_win) = main_app.window(
        WindowSpec::new(
            "prefs",
            AppSpec::new("Prefs", (360.0, 800.0), Prefs::default, |_, _| el().w_full().h_full()),
        )
        .state(Prefs { count: 0, closes: Arc::clone(&close_count), seen: Arc::clone(&seen_counter) })
        .options(WindowOptions { visible: false, ..Default::default() })
        .on_close(|p: &mut Prefs, _| {
            p.closes.fetch_add(1, Ordering::Relaxed);
            p.seen.store(p.count, Ordering::Relaxed);
        }),
    );

    let (mut stack, _) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);
    assert_eq!(stack.visible_keys(), ["main"]);
    assert!(stack.has_live_key("prefs"), "secondary window registered hidden");

    // Open via WindowHandle and update state via AppHandle.
    prefs_win.open();
    prefs_app.update(|p, _| p.count = 42);
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);

    // Back closes it, runs on_close, but retains it because App::window sets keep_running.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.visible_keys(), ["main"]);
    assert!(stack.has_live_key("prefs"));
    assert_eq!(close_count.load(Ordering::Relaxed), 1);
    assert_eq!(seen_counter.load(Ordering::Relaxed), 42);

    // Closing again while already hidden does not double-run on_close.
    prefs_win.close();
    assert!(!stack.pump());
    assert_eq!(close_count.load(Ordering::Relaxed), 1);

    // Reopen via AppHandle preserves state (count is still 42, incremented to 43).
    prefs_app.open();
    prefs_app.update(|p, _| p.count += 1);
    assert!(stack.pump());
    assert_eq!(stack.visible_keys(), ["main", "prefs"]);
    prefs_app.close();
    assert!(stack.pump());
    assert_eq!(close_count.load(Ordering::Relaxed), 2);
    assert_eq!(seen_counter.load(Ordering::Relaxed), 43);
}

#[test]
fn duplicate_open_window_key_is_ignored_and_env_syncs_across_pages() {
    clear_thread_opened();
    let main_app = App::new(AppSpec::new("Main", (360.0, 800.0), || (), |_, _| el().w_full().h_full()));
    let (mut stack, _) = make_stack();
    stack.set_viewport((360.0, 800.0), 3.0);
    stack.set_dark(true);
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);

    let _h1 = open_window(
        WindowSpec::new("prefs", AppSpec::new("P1", (360.0, 800.0), || (), |_, _| el()))
            .color_scheme(ColorScheme::Light),
    );
    let _h2 = open_window(WindowSpec::new("prefs", AppSpec::new("P2", (360.0, 800.0), || (), |_, _| el())));
    assert!(stack.pump());
    assert_eq!(stack.depth(), 2);
    assert_eq!(stack.top_key(), Some("prefs"));
    // ColorScheme::Light overrides system dark mode.
    assert!(!stack.top_runtime_core().unwrap().env.system_dark);

    // Rotation and inset updates happen while prefs is on top.
    stack.set_viewport((800.0, 360.0), 3.0);
    stack.set_safe_area([0.0, 0.0, 16.0, 32.0]);
    stack.set_keyboard_inset(240.0);
    stack.set_dark(false);

    // Popping prefs syncs the updated viewport, insets, and system dark mode onto Main.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.top_key(), Some("main"));
    let core = stack.top_runtime_core().unwrap();
    assert_eq!(core.viewport, (800.0, 360.0));
    assert_eq!(core.safe_area(), [0.0, 0.0, 16.0, 32.0]);
    assert_eq!(core.keyboard_inset(), 240.0);
    assert!(!core.env.system_dark);
}

#[test]
fn shutdown_runs_on_close_top_to_bottom_and_clears_ready() {
    clear_thread_opened();
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    let o_main = Arc::clone(&order);
    let main_app = App::new(AppSpec::new("Main", (360.0, 800.0), Arc::<Mutex<Vec<&'static str>>>::default, |_, _| el()))
        .init(move || o_main)
        .on_close(|o: &mut Arc<Mutex<Vec<&'static str>>>, _| o.lock().unwrap().push("main"));

    let (mut stack, _) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);

    let h_a = open_window(
        WindowSpec::new("a", AppSpec::new("A", (360.0, 800.0), Arc::<Mutex<Vec<&'static str>>>::default, |_, _| el()))
            .state(Arc::clone(&order))
            .on_close(|o: &mut Arc<Mutex<Vec<&'static str>>>, _| o.lock().unwrap().push("a")),
    );
    let h_b = open_window(
        WindowSpec::new("b", AppSpec::new("B", (360.0, 800.0), Arc::<Mutex<Vec<&'static str>>>::default, |_, _| el()))
            .state(Arc::clone(&order))
            .on_close(|o: &mut Arc<Mutex<Vec<&'static str>>>, _| o.lock().unwrap().push("b")),
    );
    assert!(stack.pump());
    stack.step_top_frame(0.0);
    assert!(h_b.is_ready());
    let _ = h_a;

    stack.shutdown();
    assert_eq!(stack.depth(), 0);
    assert!(!h_b.is_ready());
    assert_eq!(*order.lock().unwrap(), vec!["b", "a", "main"]);
}

#[test]
fn on_key_escape_handler_returning_true_stays_on_page_while_false_pops() {
    clear_thread_opened();
    struct Panel {
        panel_open: bool,
    }
    let main_app = App::new(AppSpec::new("Main", (360.0, 800.0), || (), |_, _| el().w_full().h_full()));
    let (mut stack, _) = make_stack();
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);

    let mut inspector_spec = AppSpec::new("Inspector", (360.0, 800.0), || Panel { panel_open: true }, |_, _| el());
    inspector_spec.on_key = Some(|s: &mut Panel, ev: &KeyEvent, _| {
        if ev.key == Key::Escape && s.panel_open {
            s.panel_open = false;
            return true;
        }
        false
    });
    let _h = open_window(WindowSpec::new("inspector", inspector_spec));
    assert!(stack.pump());
    stack.step_top_frame(0.0);
    assert_eq!(stack.visible_keys(), ["main", "inspector"]);

    // First Back: Escape consumed by on_key to close inner panel -> stays on inspector.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.visible_keys(), ["main", "inspector"]);

    // Second Back: on_key returns false -> Escape unconsumed -> pops inspector.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    assert_eq!(stack.visible_keys(), ["main"]);

    // Third Back: on root with no handler -> Unhandled (activity goes back).
    stack.send_key_top(KeyEvent { key: Key::Tab, mods: Mods::default() });
    assert_eq!(stack.handle_back(), BackOutcome::Unhandled);
}

#[test]
fn touch_input_routes_only_to_top_page_not_covered_root() {
    clear_thread_opened();
    let root_taps = Arc::new(AtomicUsize::new(0));
    let prefs_taps = Arc::new(AtomicUsize::new(0));

    let main_app = App::new(AppSpec::new(
        "Main",
        (360.0, 800.0),
        Arc::<AtomicUsize>::default,
        |_, _| {
            col().w_full().h_full().child(
                el().key("target")
                    .size(200.0, 200.0)
                    .on_press(|c: &mut Arc<AtomicUsize>| { c.fetch_add(1, Ordering::Relaxed); }),
            )
        },
    ))
    .init({ let c = Arc::clone(&root_taps); move || c });

    let (mut stack, _) = make_stack();
    stack.set_viewport((360.0, 800.0), 3.0);
    stack.set_touch_long_press(Some(0.4));
    stack.set_initial(vec![WindowSpec::from_app("main", main_app).into()]);
    stack.step_top_frame(0.0);

    let _h = open_window(
        WindowSpec::new(
            "prefs",
            AppSpec::new(
                "Prefs",
                (360.0, 800.0),
                Arc::<AtomicUsize>::default,
                |_, _| {
                    col().w_full().h_full().child(
                        el().key("target")
                            .size(200.0, 200.0)
                            .on_press(|c: &mut Arc<AtomicUsize>| { c.fetch_add(1, Ordering::Relaxed); }),
                    )
                },
            ),
        )
        .state(Arc::clone(&prefs_taps)),
    );
    assert!(stack.pump());
    stack.step_top_frame(0.0);

    // Send a touch tap at (50, 50): only the top page receives it.
    stack.touch(1, TouchPhase::Started, 50.0, 50.0, 0.1);
    stack.step_top_frame(1.0 / 60.0);
    stack.touch(1, TouchPhase::Ended, 50.0, 50.0, 0.15);
    stack.step_top_frame(1.0 / 60.0);
    assert_eq!(prefs_taps.load(Ordering::Relaxed), 1);
    assert_eq!(root_taps.load(Ordering::Relaxed), 0);

    // Pop prefs, send touch tap at (50, 50): now root receives it.
    assert_eq!(stack.handle_back(), BackOutcome::Consumed);
    stack.step_top_frame(0.0);
    stack.touch(2, TouchPhase::Started, 50.0, 50.0, 0.2);
    stack.step_top_frame(1.0 / 60.0);
    stack.touch(2, TouchPhase::Ended, 50.0, 50.0, 0.25);
    stack.step_top_frame(1.0 / 60.0);
    assert_eq!(root_taps.load(Ordering::Relaxed), 1);
}
