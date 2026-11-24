use std::borrow::Cow;

use accesskit::{Action as Ak, ActionRequest, TreeId};
use rune_motion::{Edge, Transition};

use crate::edit::{Key, KeyEvent, Mods, TextEdit};
use crate::el::{GrowthSpec, ScrollSpec};
use crate::text::{Link, Run, TextHighlight};
use crate::tok::TextStyle;
use crate::{AppSpec, Color, Cx, Direction, ElementInfo, Env, Runtime, Tx, col, el, row, text, text_input};

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const GREEN: Color = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
const BLUE: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
const ACCENT: Color = Color { r: 0.2, g: 0.5, b: 0.9, a: 1.0 };
const HL_YELLOW: Color = Color { r: 1.0, g: 1.0, b: 0.0, a: 0.3 };

fn info<'a>(els: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing element {key:?}"))
}

fn elements_focus_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused && !e.exiting).and_then(|e| e.key)
}

#[test]
fn element_info_reports_description_separately_from_help() {
    let view = |_: &(), _: &mut Cx<()>| {
        col().key("root").size(300.0, 200.0)
            .child(el().key("owned").size(40.0, 20.0).description(String::from("2 running · 1 paused")))
            .child(el().key("static").size(40.0, 20.0).description_static("Static explanation"))
            .child(el().key("empty").size(40.0, 20.0).description("").help("Tooltip text"))
            .child(el().key("help-only").size(40.0, 20.0).help("Only a tooltip"))
            .child(el().key("both").size(40.0, 20.0).description("Explicit desc").help("Explicit help"))
            .child(
                text("Click docs or mark")
                    .key("para")
                    .description("Paragraph desc")
                    .link(Link::new(6..10, "docs"))
                    .text_highlight(TextHighlight::new(14..18, HL_YELLOW).key("mk")),
            )
    };
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();

    assert_eq!(info(&els, "owned").description.as_deref(), Some("2 running · 1 paused"));
    assert_eq!(info(&els, "owned").help, None);
    assert_eq!(info(&els, "static").description.as_deref(), Some("Static explanation"));
    assert_eq!(info(&els, "empty").description, None, "empty description must be omitted");
    assert_eq!(info(&els, "empty").help.as_deref(), Some("Tooltip text"));
    assert_eq!(info(&els, "help-only").description, None, "help must stay separate from description");
    assert_eq!(info(&els, "help-only").help.as_deref(), Some("Only a tooltip"));
    assert_eq!(info(&els, "both").description.as_deref(), Some("Explicit desc"));
    assert_eq!(info(&els, "both").help.as_deref(), Some("Explicit help"));
    assert_eq!(info(&els, "para").description.as_deref(), Some("Paragraph desc"));
    assert_eq!(info(&els, "docs").description, None, "inline link must not inherit parent description");
    assert_eq!(info(&els, "mk").description, None, "inline mark must not inherit parent description");
    assert_eq!(ElementInfo::stub("stub").description, None);
}

#[test]
fn element_info_reports_border_color_only_when_bordered() {
    let view = |_: &(), _: &mut Cx<()>| {
        col().key("root").size(300.0, 200.0)
            .child(el().key("plain").size(40.0, 20.0).fill(RED))
            .child(el().key("zero").size(40.0, 20.0).border(0.0, RED))
            .child(el().key("bordered").size(40.0, 20.0).border(2.0, RED))
            .child(el().key("sides").size(40.0, 20.0).border_sides([0.0, 0.0, 1.5, 0.0], GREEN))
            .child(el().key("grad").size(40.0, 20.0).border_gradient(1.0, BLUE, RED))
            .child(
                text("Go docs mark")
                    .key("bordered-text")
                    .border(1.0, RED)
                    .link(Link::new(3..7, "docs"))
                    .text_highlight(TextHighlight::new(8..12, GREEN).key("mk")),
            )
    };
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();

    assert_eq!(info(&els, "plain").border_color, None);
    assert_eq!(info(&els, "zero").border_color, None);
    assert_eq!(info(&els, "bordered").border_color, Some(RED));
    assert_eq!(info(&els, "sides").border_color, Some(GREEN));
    assert_eq!(info(&els, "grad").border_color, Some(BLUE));
    assert_eq!(info(&els, "bordered-text").border_color, Some(RED));
    assert_eq!(info(&els, "docs").border_color, None, "inline link must not inherit parent border_color");
    assert_eq!(info(&els, "mk").border_color, None, "inline mark must not inherit parent border_color");
    assert_eq!(ElementInfo::stub("stub").border_color, None);
}

#[test]
fn tx_read_elements_runs_lazily_from_handlers_and_defers_nested_reads() {
    #[derive(Default)]
    struct State {
        log: Vec<String>,
        bordered: Option<Color>,
        nested_seen: usize,
        nest: bool,
    }
    let view = |_: &State, _: &mut Cx<State>| {
        col().key("root").size(240.0, 160.0)
            .child(el().key("plain").size(80.0, 30.0))
            .child(
                el().key("card").size(120.0, 40.0).border(1.5, ACCENT)
                    .on_press_tx(|s: &mut State, tx| {
                        let nest = s.nest;
                        tx.element_at::<State>(10.0, 10.0, |s, picked, _| {
                            s.log.push(format!("pick:{}", picked.and_then(|p| p.key().map(str::to_owned)).unwrap_or_default()));
                        });
                        tx.read_elements::<State>(move |s, elements, tx| {
                            s.log.push(format!("read1:{}", elements.len()));
                            s.bordered = elements.iter().find(|e| e.key.as_deref() == Some("card")).and_then(|e| e.border_color);
                            if nest {
                                tx.read_elements::<State>(|s, elements, _| {
                                    s.nested_seen = elements.len();
                                    s.log.push("nested".into());
                                });
                            }
                        });
                        tx.read_elements::<State>(|s, elements, _| {
                            s.log.push(format!("read2:{}", elements.len()));
                        });
                    }),
            )
            .child(
                el().key("pick-only").size(120.0, 40.0)
                    .on_press_tx(|_: &mut State, tx| {
                        tx.element_at::<State>(10.0, 10.0, |s, _, _| {
                            s.log.push("pick-only".into());
                        });
                    }),
            )
    };

    let mut rt = Runtime::new(State::default(), view, (240.0, 160.0), Env::new(true, 1.0));
    // Unused frames and plain pick reads never build the elements list.
    let (_, builds) = super::elements::count_elements_builds(|| {
        rt.frame(0.0);
        let (x, y) = rt.find("pick-only").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    });
    assert_eq!(builds, 0, "elements() must not be built when read_elements is unused");
    assert_eq!(rt.state.log, vec!["pick-only"]);

    // Two read_elements calls in one handler share a single elements() build and keep FIFO order with element_at.
    rt.state.log.clear();
    rt.state.nest = true;
    let (x, y) = rt.find("card").unwrap();
    let (_, builds) = super::elements::count_elements_builds(|| {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    });
    assert_eq!(builds, 1, "multiple read_elements in one dispatch must share one elements() build");
    assert_eq!(rt.state.bordered, Some(ACCENT));
    assert_eq!(rt.state.log.len(), 3, "nested read_elements must defer to the next frame");
    assert_eq!(rt.state.log[0], "pick:plain");
    assert!(rt.state.log[1].starts_with("read1:"));
    assert!(rt.state.log[2].starts_with("read2:"));
    assert_eq!(rt.state.nested_seen, 0);

    // Next frame runs the deferred nested read.
    let (_, builds) = super::elements::count_elements_builds(|| {
        rt.frame(0.0);
    });
    assert_eq!(builds, 1);
    assert!(rt.state.nested_seen >= 3);
    assert_eq!(rt.state.log.last().map(String::as_str), Some("nested"));
}

#[test]
fn tx_read_elements_works_from_poll_and_update() {
    #[derive(Default)]
    struct State {
        want_poll_read: bool,
        poll_border: Option<Color>,
        update_count: Option<usize>,
    }
    let view = |_: &State, _: &mut Cx<State>| {
        el().key("box").size(80.0, 40.0).border(1.0, GREEN)
    };
    let poll = |s: &mut State, _t: f64, tx: &mut Tx| -> bool {
        if std::mem::take(&mut s.want_poll_read) {
            tx.read_elements::<State>(|s, elements, _| {
                s.poll_border = elements.iter().find(|e| e.key.as_deref() == Some("box")).and_then(|e| e.border_color);
            });
        }
        false
    };
    let mut spec = AppSpec::new("test", (120.0, 80.0), State::default, view);
    spec.poll = Some(poll);
    let mut rt = crate::App::new(spec).headless_runtime((120.0, 80.0), Env::new(true, 1.0));

    // Before the first frame, update_if read_elements sees an empty slice without panicking.
    let stale = rt.update_if(|_, tx| {
        tx.read_elements::<State>(|s, elements, _| s.update_count = Some(elements.len()));
        false
    });
    assert!(stale);
    assert_eq!(rt.state.update_count, Some(0));

    // Present one frame, then trigger a poll read (even though poll returns false).
    rt.frame(0.0);
    rt.state.want_poll_read = true;
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.poll_border, Some(GREEN));
}

#[test]
fn presented_focus_key_matches_elements_across_navigation_exits_and_links() {
    struct State {
        show_exiting: bool,
        scroll_y: f32,
        virt_edit: TextEdit,
        cut_link: bool,
    }
    let view = |s: &State, _: &mut Cx<State>| {
        let mut root = col().key("root").size(320.0, 260.0).gap(4.0)
            .child(el().key("btn-a").size(80.0, 24.0).focusable(true).role("button").label("A"))
            .child(el().key("btn-b").size(80.0, 24.0).focusable(true).role("button").label("B"))
            .child(el().size(80.0, 24.0).focusable(true).role("button").label("Unkeyed"))
            .child(
                col().w(if s.cut_link { 40.0 } else { 240.0 }).child(
                    text("Read the very long guide link here")
                        .key("links-text")
                        .truncate()
                        .link(Link::new(25..29, "guide-link"))
                        .on_link(|_: &mut State, _, _| {}),
                ),
            )
            .child(
                col().key("scroller").size(200.0, 60.0).scroll(ScrollSpec::default()).child(
                    col().key("vlist").w_full().growth(GrowthSpec::default()).virtual_rows(
                        30,
                        40.0,
                        |_: &State, i| Cow::Owned(format!("row-{i}")),
                        |s: &State, i| {
                            if i == 0 {
                                row().size(180.0, 32.0).child(
                                    text_input(&s.virt_edit, TextStyle::new(13.0, 400.0))
                                        .key("virt-field")
                                        .label("Virtual field")
                                        .size(180.0, 32.0)
                                        .on_edit(|s: &mut State| &mut s.virt_edit),
                                )
                            } else {
                                el().size(180.0, 32.0)
                            }
                        },
                    ),
                ),
            );
        if s.show_exiting {
            root = root.child(
                el().key("leaving")
                    .size(80.0, 24.0)
                    .focusable(true)
                    .role("button")
                    .label("Leaving")
                    .transition(Transition::opacity()),
            );
        }
        root
    };
    let mut rt = Runtime::new(
        State { show_exiting: true, scroll_y: 0.0, virt_edit: TextEdit::new("hi"), cut_link: false },
        view,
        (320.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let assert_agrees = |rt: &Runtime<State>, expected: Option<&str>, ctx: &str| {
        let (fast, builds) = super::elements::count_elements_builds(|| rt.presented_focus_key());
        assert_eq!(builds, 0, "{ctx}: presented_focus_key must never build elements()");
        let slow = elements_focus_key(rt);
        assert_eq!(fast, expected, "{ctx}: presented_focus_key");
        assert_eq!(slow.as_deref(), expected, "{ctx}: elements() focus key");
    };

    // 1. No focus.
    assert_agrees(&rt, None, "initial");

    // 2. Pointer click.
    let (x, y) = rt.find("btn-a").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_agrees(&rt, Some("btn-a"), "pointer click btn-a");

    // 3. Tab to keyed control, then Tab to unkeyed control.
    let tab = || KeyEvent { key: Key::Tab, mods: Mods::default() };
    rt.key(tab());
    rt.frame(0.0);
    assert_agrees(&rt, Some("btn-b"), "Tab to btn-b");

    rt.key(tab());
    rt.frame(0.0);
    assert_agrees(&rt, None, "Tab to unkeyed button");

    // 4. Tab to inline link (has presented line box).
    rt.key(tab());
    rt.frame(0.0);
    assert_agrees(&rt, Some("guide-link"), "Tab to inline link");

    // 5. Truncate the link away so it has no presented line box.
    rt.update(|s, tx| { s.cut_link = true; tx.instant(); });
    rt.frame(0.0);
    assert_agrees(&rt, None, "focused inline link with no presented line box");

    // Restore link width.
    rt.update(|s, tx| { s.cut_link = false; tx.instant(); });
    rt.frame(0.0);

    // 6. Accessibility Focus action.
    let tree = rt.accessibility_tree();
    let (id, _) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("leaving")).expect("leaving a11y node");
    assert!(rt.a11y_action(&ActionRequest { action: Ak::Focus, target_tree: TreeId::ROOT, target_node: *id, data: None }));
    rt.frame(0.0);
    assert_agrees(&rt, Some("leaving"), "a11y Focus on leaving");

    // 7. Focused element starts exiting with no surviving focusable sibling: must return None while exiting.
    {
        let exit_view = |show: &bool, _: &mut Cx<bool>| {
            let mut root = col().key("root").size(160.0, 80.0);
            if *show {
                root = root.child(
                    el().key("leaving").size(80.0, 24.0).focusable(true).role("button").label("Leaving")
                        .transition(Transition::opacity()),
                );
            }
            root
        };
        let mut exit_rt = Runtime::new(true, exit_view, (160.0, 80.0), Env::new(true, 1.0));
        exit_rt.frame(0.0);
        let (x, y) = exit_rt.find("leaving").unwrap();
        exit_rt.pointer_down(x, y);
        exit_rt.pointer_up(x, y);
        exit_rt.frame(0.0);
        assert_eq!(exit_rt.presented_focus_key(), Some("leaving"));
        let leaving_path = exit_rt.elements().into_iter().find(|e| e.key.as_deref() == Some("leaving")).unwrap().path;
        exit_rt.update(|show, tx| { *show = false; tx.animated(); });
        exit_rt.frame(1.0 / 60.0);
        // Even if focus points directly at the exiting path, presented_focus_key matches elements() and returns None.
        exit_rt.set_focus(Some(leaving_path), false);
        assert!(exit_rt.elements().iter().any(|e| e.key.as_deref() == Some("leaving") && e.exiting && e.focused));
        assert_eq!(exit_rt.presented_focus_key(), None);
        assert_eq!(elements_focus_key(&exit_rt), None);
    }

    // 8. Unmounted virtual text editor: focus it, then scroll it far out of the viewport.
    let (x, y) = rt.find("virt-field").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_agrees(&rt, Some("virt-field"), "mounted virtual editor");

    rt.update(|s, tx| {
        s.scroll_y = 900.0;
        tx.scroll_to_instant("scroller", 0.0, s.scroll_y);
    });
    rt.frame(0.0);
    assert!(rt.find("virt-field").is_none(), "virtual field should be unmounted after scrolling far down");
    assert_agrees(&rt, None, "unmounted virtual editor");
}

#[test]
fn element_info_reports_laid_out_text_truncation_inset_clips_scroll_axes_and_raised() {
    struct State {
        show_wipe: bool,
        run: Run,
        secret_edit: TextEdit,
    }
    let view = |s: &State, _: &mut Cx<State>| {
        let style = TextStyle::new(13.0, 400.0);
        let mut root = col().key("root").size(400.0, 400.0).gap(4.0)
            .child(el().key("plain").size(50.0, 20.0))
            .child(el().key("padded").size(100.0, 40.0).pl(8.0).pt(4.0).pr(12.0).pb(6.0).border(2.0, Color::WHITE).border_layout())
            .child(row().key("overflow-box").w(60.0).child(text("A label that is much wider than its box").key("overflow").w(60.0).font(style)))
            .child(row().key("trunc-cut-box").w(60.0).child(text("A label that is much wider than its box").key("trunc-cut").font(style).truncate()))
            .child(row().key("trunc-fit-box").w(240.0).child(text("Short").key("trunc-fit").font(style).truncate()))
            .child(row().key("fade-cut-box").w(60.0).child(text("A label that is much wider than its box").key("fade-cut").font(style).truncate_fade()))
            .child(row().key("fade-fit-box").w(240.0).child(text("Short").key("fade-fit").font(style).truncate_fade()))
            .child(col().w(80.0).child(text("One two three four five six seven eight nine ten").key("wrapped").font(style).wrap()))
            .child(col().w(80.0).child(text("One two three four five six seven eight nine ten").key("clamped").font(style).wrap().max_lines(2)))
            .child(col().w(240.0).child(text("Short line").key("clamped-fit").font(style).wrap().max_lines(2)))
            .child(text("A").key("run-cell").font(style).advance_in(&s.run, 0))
            .child(text("secret-label").key("secure-text").font(style).secure().truncate())
            .child(text_input(&s.secret_edit, style).key("secure-input").size(80.0, 24.0))
            .child(el().key("scroll-y").size(80.0, 40.0).scroll(ScrollSpec::default()))
            .child(el().key("scroll-x").size(80.0, 40.0).scroll(ScrollSpec::default()).scroll_x())
            .child(el().key("scroll-xy").size(80.0, 40.0).scroll(ScrollSpec::default()).scroll_xy())
            .child(el().key("clip-soft").size(80.0, 40.0).clip_soft(8.0))
            .child(el().key("clip-rounded").size(80.0, 40.0).clip_rounded(6.0))
            .child(el().key("raised").size(80.0, 40.0).raise())
            .child(
                col().dir(Direction::Rtl).child(
                    el().key("rtl-pad").size(100.0, 30.0).pl(4.0).pr(12.0)
                        .child(text("مرحبا بالعالم").key("rtl-text").font(style)),
                ),
            )
            .child(
                col().w(200.0).child(
                    text("Visit docs and mark")
                        .key("parent-text")
                        .font(style)
                        .wrap()
                        .padding(6.0)
                        .raise()
                        .link(Link::new(6..10, "lnk"))
                        .text_highlight(TextHighlight::new(15..19, HL_YELLOW).key("hl")),
                ),
            );
        if s.show_wipe {
            root = root.child(
                el().key("wipe-masked").absolute(260.0, 10.0).size(80.0, 40.0).transition(Transition::reveal(Edge::Top)),
            );
        }
        root
    };

    let style = TextStyle::new(13.0, 400.0);
    for scale in [1.0f32, 2.0] {
        let mut secret_edit = TextEdit::new("super-secret-value");
        secret_edit.set_secure(true);
        let mut rt = Runtime::new(
            State { show_wipe: false, run: Run::new("ABCDEF"), secret_edit },
            view,
            (400.0, 400.0),
            Env::new(true, scale),
        );
        rt.frame(0.0);
        // Trigger reveal transition on wipe-masked so p.mask is active mid-flight.
        rt.update(|s, tx| { s.show_wipe = true; tx.animated(); });
        rt.frame(1.0 / 60.0);

        let els = rt.elements();

        // Plain non-text element.
        let plain = info(&els, "plain");
        assert_eq!(plain.inset, [0.0; 4]);
        assert_eq!(plain.text_size, None);
        assert!(!plain.wrap && !plain.truncate && !plain.text_cut && !plain.clips && !plain.raised);
        assert_eq!(plain.scroll_axes, [false, false]);

        // Padding + border_layout inset.
        let padded = info(&els, "padded");
        assert_eq!(padded.inset, [10.0, 6.0, 14.0, 8.0], "scale={scale}: padding + 2pt border");

        // Untruncated overflowing text: text_size wider than box, truncate == false, text_cut == false.
        let overflow = info(&els, "overflow");
        let ov_sz = overflow.text_size.expect("overflow text_size");
        assert!(ov_sz[0] > 150.0 && ov_sz[0] > overflow.rect[2], "scale={scale}: text_size {ov_sz:?} vs rect {:?}", overflow.rect);
        assert!(ov_sz[1] >= style.line_px() - 0.5, "scale={scale}: height {ov_sz:?}");
        assert!(!overflow.wrap && !overflow.truncate && !overflow.text_cut);

        // Truncated text (cut vs fit).
        let tc = info(&els, "trunc-cut");
        assert!(tc.truncate && tc.text_cut, "scale={scale}: trunc-cut should be cut");
        assert!(tc.text_size.is_some());
        let tf = info(&els, "trunc-fit");
        assert!(tf.truncate && !tf.text_cut, "scale={scale}: trunc-fit should not be cut");

        // Truncate fade (cut vs fit).
        let fc = info(&els, "fade-cut");
        assert!(fc.truncate && fc.text_cut, "scale={scale}: fade-cut should report text_cut");
        assert!(fc.text_size.unwrap()[0] > 150.0);
        let ff = info(&els, "fade-fit");
        assert!(ff.truncate && !ff.text_cut, "scale={scale}: fade-fit should not report text_cut");

        // Wrapped and max_lines clamped text.
        let wrapped = info(&els, "wrapped");
        assert!(wrapped.wrap && !wrapped.truncate && !wrapped.text_cut);
        assert!(wrapped.text_size.unwrap()[1] > style.line_px() * 1.5, "scale={scale}: wrapped text should span multiple lines");
        let clamped = info(&els, "clamped");
        assert!(clamped.wrap && clamped.truncate && clamped.text_cut);
        let clamped_fit = info(&els, "clamped-fit");
        assert!(clamped_fit.wrap && clamped_fit.truncate && !clamped_fit.text_cut);

        // Shared run cell reports its own cell width, not the whole "ABCDEF" run.
        let run_cell = info(&els, "run-cell");
        let cell_w = run_cell.text_size.expect("run-cell text_size")[0];
        assert!(cell_w > 3.0 && cell_w < 20.0, "scale={scale}: single glyph cell width should be small, got {cell_w}");

        // Secure text and secure input omit text_size and text_cut.
        let sec_t = info(&els, "secure-text");
        assert_eq!(sec_t.text_size, None, "scale={scale}: secure text must omit text_size");
        assert!(!sec_t.text_cut, "scale={scale}: secure text must report text_cut == false");
        let sec_i = info(&els, "secure-input");
        assert_eq!(sec_i.text_size, None, "scale={scale}: secure input must omit text_size");
        assert!(!sec_i.text_cut);

        // Scroll axes and clips.
        let sy = info(&els, "scroll-y");
        assert!(sy.clips && sy.scroll_axes == [false, true]);
        let sx = info(&els, "scroll-x");
        assert!(sx.clips && sx.scroll_axes == [true, false]);
        let sxy = info(&els, "scroll-xy");
        assert!(sxy.clips && sxy.scroll_axes == [true, true]);
        assert!(info(&els, "clip-soft").clips);
        assert!(info(&els, "clip-rounded").clips);
        assert!(info(&els, "wipe-masked").clips, "scale={scale}: active Transition::reveal mask must set clips");

        // Raised.
        assert!(info(&els, "raised").raised);
        assert!(!plain.raised);

        // RTL inset mirroring and RTL text_size.
        let rtl_pad = info(&els, "rtl-pad");
        assert_eq!(rtl_pad.inset, [12.0, 0.0, 4.0, 0.0], "scale={scale}: RTL swaps left/right inset");
        let rtl_sz = info(&els, "rtl-text").text_size.expect("rtl text_size");
        assert!(rtl_sz[0] > 10.0 && rtl_sz[1] > 5.0);

        // Synthetic inline link and mark do not inherit parent text_size, inset, wrap, or raised.
        for synth in ["lnk", "hl"] {
            let e = info(&els, synth);
            assert_eq!(e.text_size, None, "{synth} must not inherit parent text_size");
            assert_eq!(e.inset, [0.0; 4], "{synth} must not inherit parent inset");
            assert!(!e.wrap && !e.truncate && !e.text_cut && !e.clips && !e.raised);
            assert_eq!(e.scroll_axes, [false, false]);
        }
    }

    // ElementInfo::stub matches plain defaults for all new fields.
    let stub = ElementInfo::stub("check");
    assert_eq!(stub.inset, [0.0; 4]);
    assert_eq!(stub.text_size, None);
    assert!(!stub.wrap && !stub.truncate && !stub.text_cut && !stub.clips && !stub.raised);
    assert_eq!(stub.scroll_axes, [false, false]);
}
