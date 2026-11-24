use rune::{Align, Cx, El, Env, Key, KeyEvent, Mods, Runtime, Side, TextEdit, col, icon, text};
use rune_kit::{Kit, PopupItem, PopupList, TextInput, root};

#[derive(Default)]
struct State {
    composer: TextEdit,
    open: bool,
    count: usize,
    active: Option<usize>,
    hovered: Vec<usize>,
    chosen: Vec<String>,
    animate: bool,
}

fn item_for(k: &Kit, i: usize) -> PopupItem {
    PopupItem::new(format!("cmd-{i}"), format!("/command-{i:02}"))
        .description(format!("Action {i}"))
        .leading(icon(&rune_kit::glyphs::SPARKLE, 14.0).color(k.c.icon_muted))
        .trailing(text("⏎").font(k.ty.small_medium).color(k.c.text_faint))
        .disabled(i == 1)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let field = TextInput::new(&s.composer)
        .placeholder("Type a command…")
        .animate_edit_layout()
        .on_edit(|s: &mut State| &mut s.composer)
        .on_key(|s: &mut State, ev, _| match ev.key {
            Key::Down => {
                if s.count > 0 {
                    s.active = Some((s.active.unwrap_or(0) + 1) % s.count);
                }
                true
            }
            Key::Up => {
                if s.count > 0 {
                    s.active = Some(s.active.unwrap_or(0).checked_sub(1).unwrap_or(s.count - 1));
                }
                true
            }
            _ => false,
        })
        .build(&k)
        .key("composer");
    let items: Vec<_> = (0..s.count).map(|i| item_for(&k, i)).collect();
    PopupList::new("commands", "composer")
        .anchored("composer", Side::Bottom, Align::Start, 4.0)
        .open(s.open)
        .animate_filtering(s.animate)
        .items(items)
        .active(s.active)
        .empty_message("No matching commands")
        .label("Commands")
        .on_active(|s: &mut State, i| {
            s.active = Some(i);
            s.hovered.push(i);
        })
        .on_choose(|s: &mut State, id, _| {
            s.chosen.push(id.into());
        })
        .build(&k, &mut layer);
    root(col().padding(16.0).child(field), layer)
}

fn runtime(count: usize) -> Runtime<State> {
    let mut composer = TextEdit::new("");
    composer.focused = true;
    let s = State {
        composer,
        open: true,
        count,
        active: Some(0),
        ..State::default()
    };
    let mut rt = Runtime::new(s, view, (480.0, 480.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

#[test]
fn option_changes_5_2_0_3_spring_height_fade_rows_and_toggle_empty_state() {
    let mut rt = runtime(5);
    rt.update(|s, _| s.animate = true);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let find = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let h5 = find(&rt, "commands-content").unwrap().rect[3];
    assert_eq!(find(&rt, "empty").unwrap().opacity, 0.0);
    assert_eq!(find(&rt, "empty").unwrap().rect[3], 0.0);

    // 5 -> 2 items: height springs down and removed rows fade out.
    rt.update(|s, _| s.count = 2);
    rt.frame(1.0 / 60.0);
    let fading = find(&rt, "commands-option-cmd-4").expect("removed row stays mounted while fading out");
    assert!(fading.opacity > 0.0 && fading.opacity < 1.0, "row opacity {}", fading.opacity);
    let mid_5_to_2 = find(&rt, "commands-content").unwrap().rect[3];
    assert!(mid_5_to_2 < h5 && mid_5_to_2 > 64.0, "height springs from {h5}, got {mid_5_to_2}");
    let mut last = mid_5_to_2;
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
        let h = find(&rt, "commands-content").unwrap().rect[3];
        assert!((h - last).abs() < 25.0, "popup height jumped {last} -> {h}");
        last = h;
    }
    let h2 = last;
    assert!((h2 - 64.0).abs() < 1.0, "settled 2-item height {h2}");
    assert_eq!(find(&rt, "empty").unwrap().opacity, 0.0);

    // 2 -> 0 items: height springs to empty state and empty message becomes visible.
    rt.update(|s, _| s.count = 0);
    rt.frame(1.0 / 60.0);
    assert!(find(&rt, "empty").unwrap().opacity > 0.0);
    let mid_2_to_0 = find(&rt, "commands-content").unwrap().rect[3];
    assert!(mid_2_to_0 < h2 && mid_2_to_0 > 36.0, "height springs from {h2} toward empty, got {mid_2_to_0}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let h0 = find(&rt, "commands-content").unwrap().rect[3];
    assert!((h0 - 36.0).abs() < 1.0, "settled empty height {h0}");
    assert_eq!(find(&rt, "empty").unwrap().opacity, 1.0);
    assert!((find(&rt, "empty").unwrap().rect[3] - 28.0).abs() < 1.0);

    // 0 -> 3 items: height springs back up and empty state hides.
    rt.update(|s, _| {
        s.count = 3;
        s.active = Some(0);
    });
    rt.frame(1.0 / 60.0);
    assert!(find(&rt, "empty").unwrap().opacity < 1.0);
    let mid_0_to_3 = find(&rt, "commands-content").unwrap().rect[3];
    assert!(mid_0_to_3 > h0 && mid_0_to_3 < 92.0, "height springs from {h0} toward 3 items, got {mid_0_to_3}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let h3 = find(&rt, "commands-content").unwrap().rect[3];
    assert!((h3 - 92.0).abs() < 1.0, "settled 3-item height {h3}");
    assert_eq!(find(&rt, "empty").unwrap().opacity, 0.0);
}

/// By default a change to the rows lands in its frame, though the composer's edits animate the
/// layout, while opening, closing and the hover highlight keep their motion.
#[test]
fn option_changes_land_at_once_by_default() {
    let find = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let mut rt = runtime(5);
    rt.update(|s, _| s.open = false);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 60.0);
    let opening = find(&rt, "commands-content").unwrap();
    assert!(opening.opacity < 1.0 || opening.scale < 1.0, "the popover enters: {} at {}", opening.opacity, opening.scale);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }

    for (count, height) in [(2, 64.0), (0, 36.0), (3, 92.0)] {
        // The composer's text changes too, in a commit that animates its layout.
        rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
        rt.update(|s, _| {
            s.count = count;
            s.active = (count > 0).then_some(0);
        });
        rt.frame(1.0 / 60.0);
        let h = find(&rt, "commands-content").unwrap().rect[3];
        assert!((h - height).abs() < 0.05, "{count} rows: {h}");
        assert!(rt.elements().iter().all(|e| !e.exiting), "{count} rows: nothing is left fading out");
        for i in 0..count {
            assert_eq!(find(&rt, &format!("commands-option-cmd-{i}")).unwrap().opacity, 1.0, "row {i} is whole");
        }
        let empty = find(&rt, "empty").unwrap();
        assert_eq!(empty.opacity, if count == 0 { 1.0 } else { 0.0 });
    }

    let row = find(&rt, "commands-option-cmd-2").unwrap().rect;
    let fill = |rt: &Runtime<State>| find(rt, "commands-option-cmd-2").unwrap().fill.a;
    rt.pointer_move(row[0] + 10.0, row[1] + 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.active, Some(2));
    let hovered = fill(&rt);
    rt.frame(1.0 / 60.0);
    assert!(fill(&rt) > hovered, "the highlight still eases in: {hovered} → {}", fill(&rt));

    rt.update(|s, _| s.open = false);
    rt.frame(1.0 / 60.0);
    let closing = find(&rt, "commands-content").expect("the popover is still closing");
    assert!(closing.exiting && (closing.opacity < 1.0 || closing.scale < 1.0), "it leaves: {} at {}", closing.opacity, closing.scale);
}

#[test]
fn active_index_scrolls_into_view_and_updates_listbox_accessibility() {
    let mut rt = runtime(40);
    let find = |rt: &Runtime<State>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let listbox = find(&rt, "viewport").unwrap();
    assert_eq!(listbox.role, "listbox");
    assert_eq!(listbox.label.as_deref(), Some("Commands"));
    let first = find(&rt, "commands-option-cmd-0").unwrap();
    assert_eq!(first.role, "option");
    assert!(first.item_selected);

    // Moving active to the last item (via Up key in the outside field) reveals the offscreen row.
    rt.key(KeyEvent { key: Key::Up, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.active, Some(39));
    let last = find(&rt, "commands-option-cmd-39").expect("active virtual row is mounted and revealed");
    assert!(last.visible[3] > 0.0, "last row visible rect {:?}", last.visible);
    assert!(last.item_selected);
    let listbox = find(&rt, "viewport").unwrap();
    assert_eq!(listbox.label.as_deref(), Some("Commands"));
}

#[test]
fn hover_fires_on_active_click_fires_on_choose_and_outside_text_edit_keeps_focus() {
    let mut rt = runtime(5);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    // Typing while open goes straight to the outside TextEdit without moving focus into the popup.
    rt.key(KeyEvent { key: Key::Text("/re".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.composer.text, "/re");
    assert!(rt.state.composer.focused);
    assert!(rt.elements().iter().filter(|e| e.role == "option").all(|e| !e.focusable && !e.focused));

    // Hovering option 2 fires on_active(s, 2).
    let (x2, y2) = rt.find("commands-option-cmd-2").unwrap();
    rt.pointer_move(x2, y2);
    rt.frame(0.0);
    assert_eq!(rt.state.hovered, [2]);
    assert_eq!(rt.state.active, Some(2));
    assert_eq!(
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("viewport")).unwrap().label.as_deref(),
        Some("Commands")
    );

    // Clicking disabled option 1 neither chooses nor blurs the outside TextEdit.
    let (x1, y1) = rt.find("commands-option-cmd-1").unwrap();
    rt.pointer_down(x1, y1);
    rt.pointer_up(x1, y1);
    rt.frame(0.0);
    assert!(rt.state.chosen.is_empty());
    assert!(rt.state.composer.focused);

    // Clicking enabled option 2 fires on_choose and leaves focus in the outside TextEdit so typing continues.
    rt.pointer_down(x2, y2);
    rt.pointer_up(x2, y2);
    rt.frame(0.0);
    assert_eq!(rt.state.chosen, ["cmd-2"]);
    assert!(rt.state.composer.focused);
    rt.key(KeyEvent { key: Key::Text("view".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.composer.text, "/review");
}

#[test]
fn hover_changes_do_not_scroll_now_or_on_the_next_frame() {
    let mut rt = runtime(40);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("commands-option-cmd-8")).unwrap();
    let (x, y) = (row.visible[0] + 10.0, row.visible[1] + 2.0);
    let before = row.rect;
    rt.pointer_move(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.active, Some(8));
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    let after = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("commands-option-cmd-8")).unwrap().rect;
    assert_eq!(before, after, "hovering a partly clipped row must not reveal it");
    assert_eq!(rt.state.hovered, [8]);
}

#[test]
fn pressing_popup_padding_or_empty_state_keeps_the_composer_focused() {
    for count in [0, 3] {
        let mut rt = runtime(count);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let popup = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("commands-content")).unwrap().rect;
        let (x, y) = (popup[0] + 1.0, popup[1] + 1.0);
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
        assert!(rt.state.composer.focused, "padding, count {count}");
        if count == 0 {
            let (x, y) = rt.find("empty").unwrap();
            rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
            assert!(rt.state.composer.focused, "empty state");
        }
    }
}

#[test]
fn ordinary_listbox_selection_does_not_reveal_and_press_still_blurs() {
    fn ordinary(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        col().child(TextInput::new(&s.composer).on_edit(|s: &mut State| &mut s.composer).build(&k))
            .child(col().key("ordinary-list").role("listbox").h(100.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().growth(rune::GrowthSpec::default()).virtualize(28.0).children((0..40).map(|i| {
                    rune::el().key(format!("row-{i}")).role("option").h(28.0)
                        .focus_selected(s.active == Some(i)).on_press(|_: &mut State| {})
                }))))
    }
    let mut s = State::default();
    s.composer.focused = true;
    s.active = Some(39);
    let mut rt = Runtime::new(s, ordinary, (480.0, 480.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let first = |rt: &Runtime<State>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row-0")).unwrap();
    assert!(first(&rt).visible[3] > 0.0, "restoring selection must not scroll");
    rt.update(|s, _| s.active = Some(38)); rt.frame(0.0);
    assert!(first(&rt).visible[3] > 0.0, "programmatic selection must not scroll");
    let (x, y) = rt.find("row-0").unwrap();
    rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    assert!(!rt.state.composer.focused, "ordinary listbox presses still blur the field");
}

#[test]
fn popup_item_label_with_draws_custom_element_and_keeps_plain_a11y_label() {
    #[derive(Default)]
    struct RichState {
        composer: TextEdit,
        active: Option<usize>,
        chosen: Vec<String>,
    }

    fn rich_view(s: &RichState, _: &mut Cx<RichState>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let field = TextInput::new(&s.composer)
            .placeholder("Filter…")
            .on_edit(|s: &mut RichState| &mut s.composer)
            .build(&k)
            .key("composer");
        let match_row = |key: &'static str, head: &'static str, tail: &'static str| {
            rune::row()
                .key(key)
                .center_y()
                .child(text(head).font(k.ty.ui_medium).color(k.c.text_accent))
                .child(text(tail).font(k.ty.ui_compact).color(k.c.text_base))
        };
        let items = [
            PopupItem::new("review", "/review")
                .label_with(match_row("rich-review", "/rev", "iew"))
                .description("Review branch changes")
                .trailing(text("⏎").key("trail-review").font(k.ty.small_medium).color(k.c.text_faint)),
            PopupItem::new("compact", "/compact")
                .label_with(match_row("rich-compact", "/com", "pact"))
                .trailing(text("⌘K").key("trail-compact").font(k.ty.small_medium).color(k.c.text_faint)),
            PopupItem::new("locked", "/locked")
                .label_with(match_row("rich-locked", "/loc", "ked"))
                .disabled(true),
        ];
        PopupList::new("rich", "composer")
            .open(true)
            .width(300.0)
            .items(items)
            .active(s.active)
            .label("Rich commands")
            .on_active(|s: &mut RichState, i| s.active = Some(i))
            .on_choose(|s: &mut RichState, id, _| s.chosen.push(id.to_owned()))
            .build(&k, &mut layer);
        root(col().padding(16.0).child(field), layer)
    }

    let mut composer = TextEdit::new("");
    composer.focused = true;
    let mut rt = Runtime::new(
        RichState { composer, active: Some(0), chosen: Vec::new() },
        rich_view,
        (480.0, 420.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }

    let find = |rt: &Runtime<RichState>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let opt0 = find(&rt, "rich-option-review").expect("rich option with description");
    assert_eq!(opt0.label.as_deref(), Some("/review"), "plain label stays on ElementInfo::label");
    let custom0 = find(&rt, "rich-review").expect("custom label_with element mounts");
    assert!(custom0.target[2] > 0.0 && custom0.target[3] > 0.0);

    // Without description, label_with grows so trailing stays near the right edge.
    let opt1 = find(&rt, "rich-option-compact").expect("rich option without description");
    assert_eq!(opt1.label.as_deref(), Some("/compact"));
    let custom1 = find(&rt, "rich-compact").expect("custom label_with element mounts");
    let trail1 = find(&rt, "trail-compact").expect("trailing hint");
    assert!(
        custom1.target[0] + custom1.target[2] <= trail1.target[0] + 0.5,
        "rich label {:?} does not overlap trailing {:?}",
        custom1.target,
        trail1.target,
    );
    assert!(
        trail1.target[0] > opt1.target[0] + opt1.target[2] * 0.5,
        "trailing {:?} stays near right edge of {:?}",
        trail1.target,
        opt1.target,
    );

    // Clicking disabled rich item does nothing; clicking enabled rich item fires on_choose and keeps focus.
    let (dx, dy) = rt.find("rich-option-locked").unwrap();
    rt.pointer_down(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(0.0);
    assert!(rt.state.chosen.is_empty());
    assert!(rt.state.composer.focused);

    let (x, y) = rt.find("rich-option-compact").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.chosen, ["compact"]);
    assert!(rt.state.composer.focused);
}

#[test]
fn popup_list_build_list_mounts_standalone_listbox_with_handlers_and_empty_state() {
    #[derive(Default)]
    struct InlineState {
        composer: TextEdit,
        count: usize,
        active: Option<usize>,
        hovered: Vec<usize>,
        chosen: Vec<String>,
    }

    fn inline_view(s: &InlineState, _: &mut Cx<InlineState>) -> El {
        let k = Kit::neutral(true);
        let field = TextInput::new(&s.composer)
            .placeholder("Type…")
            .on_edit(|s: &mut InlineState| &mut s.composer)
            .build(&k)
            .key("composer");
        let items = (0..s.count).map(|i| {
            PopupItem::new(format!("id-{i}"), format!("Item {i}")).disabled(i == 1)
        });
        let list = PopupList::new("inline", "")
            // build_list does not gate on open(false); caller owns visibility.
            .open(false)
            .label("Inline completions")
            .max_height(140.0)
            .empty_message("Nothing matches")
            .items(items)
            .active(s.active)
            .on_active(|s: &mut InlineState, i| {
                s.active = Some(i);
                s.hovered.push(i);
            })
            .on_choose(|s: &mut InlineState, id, _| s.chosen.push(id.to_owned()))
            .build_list(&k);
        let panel = col().key("custom-panel").w(280.0).padding(4.0).child(list);
        col().padding(16.0).gap(8.0).child(field).child(panel)
    }

    let mut composer = TextEdit::new("");
    composer.focused = true;
    let mut rt = Runtime::new(
        InlineState { composer, count: 3, active: Some(0), hovered: Vec::new(), chosen: Vec::new() },
        inline_view,
        (480.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let find = |rt: &Runtime<InlineState>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key));
    let listbox = find(&rt, "viewport").expect("standalone listbox viewport");
    assert_eq!(listbox.role, "listbox");
    assert_eq!(listbox.label.as_deref(), Some("Inline completions"));
    assert!(find(&rt, "inline-content").is_none(), "build_list mounts no anchored popover wrapper");

    // Hover and click work on enabled items; disabled item is ignored; composer keeps focus.
    let (x2, y2) = rt.find("inline-option-id-2").unwrap();
    rt.pointer_move(x2, y2);
    rt.frame(0.0);
    assert_eq!(rt.state.hovered, [2]);
    assert_eq!(rt.state.active, Some(2));
    rt.pointer_down(x2, y2);
    rt.pointer_up(x2, y2);
    rt.frame(0.0);
    assert_eq!(rt.state.chosen, ["id-2"]);
    assert!(rt.state.composer.focused, "pressing standalone list preserves focus in outside input");

    let (x1, y1) = rt.find("inline-option-id-1").unwrap();
    rt.pointer_down(x1, y1);
    rt.pointer_up(x1, y1);
    rt.frame(0.0);
    assert_eq!(rt.state.chosen, ["id-2"], "disabled option does not fire on_choose");

    // Filtering down to 0 items lands immediately (default animate_filtering = false) with empty state.
    rt.update(|s, _| s.count = 0);
    rt.frame(1.0 / 60.0);
    let empty = find(&rt, "empty").expect("empty state element");
    assert_eq!(empty.opacity, 1.0);
    assert!((empty.rect[3] - 28.0).abs() < 0.5);
}
