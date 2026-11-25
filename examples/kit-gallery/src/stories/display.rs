//! Card, List, Accordion, Collapsible, Badge, Keybind, Spinner, Loader, Progress and SegmentedControl.

use super::*;

// Card, List, Accordion, Collapsible

fn card(k: &Kit, accent: Option<rune::Color>, icon: Option<&'static rune::Icon>) -> El {
    let mut c = Card::new()
        .title("Card title", icon)
        .description("Small supporting text.")
        .action(Button::new("Action").build(k));
    if let Some(a) = accent {
        c = c.accent(a);
    }
    c.build(k)
}

pub(super) fn card_normal(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, None, None)
}
pub(super) fn card_error(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.danger.icon), Some(&ic::CIRCLE_BAN_SIGN_BOLD))
}
pub(super) fn card_warning(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.warning.icon), Some(&ic::WARNING_BOLD))
}
pub(super) fn card_success(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.success.icon), Some(&ic::CIRCLE_CHECK_BOLD))
}
pub(super) fn card_info(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.info.icon), Some(&ic::HELP_BOLD))
}

fn list(s: &G, items: &[&str], search: bool) -> List {
    let mut l = List::new();
    if search {
        l = l.search(&s.edits[3], "").on_search_edit(|s: &mut G| &mut s.edits[3]);
    }
    for i in items {
        l = l.item(ListItem::new(*i));
    }
    l
}

pub(super) fn list_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two", "Three", "Four"], true).build(k)
}
pub(super) fn list_grouped(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    List::new()
        .search(&s.edits[3], "")
        .item(ListItem::new("Alpha").group("Group A"))
        .item(ListItem::new("Bravo").group("Group A"))
        .item(ListItem::new("Delta").group("Group B"))
        .build(k)
}
pub(super) fn list_empty(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &[], true).build(k)
}
pub(super) fn list_with_add(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two"], true)
        .add_row(
            row()
                .px(8.0)
                .py(6.0)
                .child(text("Add item").font(TextStyle::new(14.0, 500.0).lh(21.0)).color(k.c.text_strong)),
        )
        .build(k)
}
pub(super) fn list_divider(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two", "Three"], false).divider().build(k)
}
pub(super) fn list_active_icon(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["Alpha", "Beta", "Gamma"], false).active_icon(&ic::CHEVRON_RIGHT).build(k)
}
pub(super) fn list_no_search(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two", "Three"], false).build(k)
}

fn accordion_content(k: &Kit, s: &str, padded: bool) -> El {
    let t = text(s).font(TextStyle::new(13.0, 440.0).lh(24.0));
    if padded { row().py(8.0).child(t.color(k.c.text_weak)) } else { row().child(t.color(k.c.std_text)) }
}

/// The web story's `value="first"` arrives as a control arg and renders closed; so does this.
pub(super) fn accordion_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let open = s.picked("acc", "").to_owned();
    let pick = |v: &'static str| {
        move |s: &mut G| {
            let cur = s.picked("acc", "").to_owned();
            s.pick.insert("acc".into(), if cur == v { String::new() } else { v.into() });
        }
    };
    col().w(420.0).gap(8.0).child(
        Accordion::new()
            .item(
                AccordionItem::new("First")
                    .expanded(open == "first")
                    .child(accordion_content(k, "Accordion content.", true))
                    .on_toggle(pick("first")),
            )
            .item(
                AccordionItem::new("Second")
                    .expanded(open == "second")
                    .child(accordion_content(k, "More content.", true))
                    .on_toggle(pick("second")),
            )
            .build(k),
    )
}

pub(super) fn accordion_multiple(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Accordion::new()
        .item(
            AccordionItem::new("First")
                .expanded(!s.is("a1"))
                .child(accordion_content(k, "Accordion content.", false))
                .on_toggle(flip("a1")),
        )
        .item(
            AccordionItem::new("Second")
                .expanded(!s.is("a2"))
                .child(accordion_content(k, "More content.", false))
                .on_toggle(flip("a2")),
        )
        .build(k)
}

/// The web story passes `value="first"` without `multiple` or `collapsible` and renders closed.
pub(super) fn accordion_single(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Accordion::new()
        .item(AccordionItem::new("First").expanded(false).child(accordion_content(k, "Accordion content.", false)))
        .build(k)
}

pub(super) fn collapsible_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Collapsible::new("Details", !s.is("c"))
        .child(
            row().pt(8.0).child(
                text("Optional details sit here.").font(TextStyle::new(13.0, 440.0).lh(24.0)).color(k.c.text_weak),
            ),
        )
        .on_toggle(flip("c"))
        .build(k)
}

pub(super) fn collapsible_ghost(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Collapsible::new("Ghost trigger", s.is("c"))
        .ghost()
        .child(
            row().pt(8.0).child(text("Ghost content.").font(TextStyle::new(13.0, 440.0).lh(24.0)).color(k.c.text_weak)),
        )
        .on_toggle(flip("c"))
        .build(k)
}

// Badge, Keybind, Spinner, Loader, Progress, SegmentedControl

pub(super) fn badge_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Badge::new("Label").build(k))
}
pub(super) fn badge_appearances(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().gap(8.0).center_y().child(Badge::new("Standard").standard().build(k)).child(Badge::new("Compact").build(k))
}
pub(super) fn badge_subtle(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(8.0)
        .center_y()
        .child(Badge::new("1").variant(BadgeVariant::Subtle).build(k))
        .child(Badge::new("12").variant(BadgeVariant::Subtle).build(k))
}
pub(super) fn badge_high_contrast(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().gap(8.0).center_y().child(Badge::new("Label").build(k)).child(Badge::new("Label").high_contrast().build(k))
}
pub(super) fn badge_accent(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Badge::new("New").variant(BadgeVariant::Accent).build(k))
}

pub(super) fn keybind_playground(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Keybind::new(&["⌘"]).build(k))
}
pub(super) fn keybind_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(24.0)
        .center_y()
        .child(Keybind::new(&["⌘"]).build(k))
        .child(Keybind::new(&["⌘"]).variant(KeybindVariant::Ghost).build(k))
}
pub(super) fn keybind_multiple(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(24.0)
        .center_y()
        .child(Keybind::new(&["⌘", "K"]).build(k))
        .child(Keybind::new(&["⌘", "K"]).variant(KeybindVariant::Ghost).build(k))
        .child(Keybind::new(&["Ctrl", "B"]).build(k))
        .child(Keybind::new(&["Ctrl", "B"]).variant(KeybindVariant::Ghost).build(k))
}
pub(super) fn keybind_all(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let line = |name: &str, v: KeybindVariant| {
        row()
            .gap(24.0)
            .center_y()
            .child(
                text(name)
                    .w(50.0)
                    .font(TextStyle::new(11.0, 440.0).lh(24.0))
                    .color(rune::theme::hex("#808080").unwrap_or_default()),
            )
            .child(Keybind::new(&["⌘"]).variant(v).build(k))
            .child(Keybind::new(&["⌘", "K"]).variant(v).build(k))
            .child(Keybind::new(&["⌘", "⇧", "P"]).variant(v).build(k))
            .child(Keybind::new(&["Ctrl", "Shift", "B"]).variant(v).build(k))
    };
    col().gap(16.0).child(line("Neutral", KeybindVariant::Neutral)).child(line("Ghost", KeybindVariant::Ghost))
}

/// Keyboard symbols next to capitals, in both families at 13 and 15 pt, then keycaps.
pub(super) fn keybind_symbols(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    const SAMPLE: &str = "⌘K ⇧P ⏎ ⌫ ⎋ ⌥⌃ ←↑ HP";
    let mut c = col().gap(10.0);
    for mono in [false, true] {
        for size in [13.0, 15.0] {
            let font = TextStyle::new(size, 500.0).lh(size * 1.5);
            let font = if mono { font.mono() } else { font };
            let label = format!("{} {size}", if mono { "Plex Mono" } else { "Inter" });
            c = c.child(
                row()
                    .gap(16.0)
                    .center_y()
                    .child(text(label).w(96.0).font(TextStyle::new(11.0, 440.0).lh(16.0)).color(k.c.text_faint))
                    .child(text(SAMPLE).font(font).color(k.c.text_base)),
            );
        }
        let mut kk = k.clone();
        if mono {
            kk.ty.small_medium = kk.ty.small_medium.mono();
        }
        let mut caps = row().gap(12.0).center_y().child(
            text(if mono { "Plex Mono caps" } else { "Inter caps" })
                .w(96.0)
                .font(TextStyle::new(11.0, 440.0).lh(16.0))
                .color(k.c.text_faint),
        );
        let sets: [&[&str]; 6] = [&["⌘", "⇧", "P"], &["⇧", "⏎"], &["⌫"], &["⎋"], &["⌥", "⌃", "K"], &["←", "↑"]];
        for keys in sets {
            caps = caps.child(Keybind::new(keys).build(&kk));
        }
        c = c.child(caps);
    }
    c
}

/// Single-character keycaps upper-cased by default vs authored case kept with `keep_case()`.
pub(super) fn keybind_keep_case(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let line = |name: &str, v: KeybindVariant, keep: bool| {
        let cap = |keys: &[&str]| {
            let kb = Keybind::new(keys).variant(v);
            if keep { kb.keep_case().build(k) } else { kb.build(k) }
        };
        row()
            .gap(16.0)
            .center_y()
            .child(text(name).w(96.0).font(TextStyle::new(11.0, 440.0).lh(16.0)).color(k.c.text_faint))
            .child(cap(&["g"]))
            .child(cap(&["g", "g"]))
            .child(cap(&["G"]))
            .child(cap(&["Shift", "g"]))
            .child(cap(&["Shift", "G"]))
    };
    col()
        .gap(12.0)
        .child(line("Default", KeybindVariant::Neutral, false))
        .child(line("keep_case", KeybindVariant::Neutral, true))
        .child(line("Ghost", KeybindVariant::Ghost, false))
        .child(line("Ghost keep_case", KeybindVariant::Ghost, true))
}

pub(super) fn spinner_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Spinner::new().build(k))
}
pub(super) fn spinner_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(16.0)
        .center_y()
        .child(Spinner::new().size(12.0).build(k))
        .child(Spinner::new().size(20.0).build(k))
        .child(Spinner::new().size(28.0).build(k))
}
pub(super) fn loader_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Loader::new().build(k))
}
pub(super) fn loader_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(16.0)
        .center_y()
        .child(Loader::new().size(12.0).build(k))
        .child(Loader::new().build(k))
        .child(Loader::new().size(24.0).build(k))
}
pub(super) fn progress_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Progress::new(Some(0.6)).label("Progress").show_value().build(k)
}
pub(super) fn progress_no_label(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Progress::new(Some(0.3)).build(k)
}
pub(super) fn progress_indeterminate(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Progress::new(None).label("Loading").build(k)
}

pub(super) fn segmented_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let sel = s.picked("seg", "0").parse().unwrap_or(0);
    row().child(
        SegmentedControl::new(&["Label", "Label", "Label", "Label"], Some(sel))
            .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                s.pick.insert("seg".into(), i.to_string());
            })
            .build(k),
    )
}
pub(super) fn segmented_controlled(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let sel: usize = s.picked("seg", "1").parse().unwrap_or(1);
    col()
        .gap(12.0)
        .items_start()
        .child(
            SegmentedControl::new(&["List", "Grid", "Board"], Some(sel))
                .label("View")
                .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                    s.pick.insert("seg".into(), i.to_string());
                })
                .build(k),
        )
        .child(
            text(format!("Value: {}", ["a", "b", "c"][sel.min(2)]))
                .font(TextStyle::new(12.0, 440.0).lh(24.0))
                .color(rune::theme::hex("#808080").unwrap_or_default()),
        )
}

/// Rich `SegmentedControl`: per-segment icons, trailing count badge and status dot, hover
/// tooltips with keyboard shortcuts, icon-only segments, and `as_tabs()` with `controls()`
/// wired to a `tabpanel`.
pub(super) fn segmented_rich(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let view: usize = s.picked("seg-rich", "0").parse().unwrap_or(0).min(3);
    let icon_view: usize = s.picked("seg-icons", "0").parse().unwrap_or(0).min(2);
    let tab: usize = s.picked("seg-tabs", "0").parse().unwrap_or(0).min(2);
    let tab_copy = [
        "General preferences: language, startup screen and default workspace.",
        "Network settings: proxy configuration and TLS certificate store.",
        "Security warning: 1 pending signing key rotation requires review.",
    ][tab];
    let dot = |color| el().size(6.0, 6.0).radius(3.0).fill(color).a11y_hidden();

    col()
        .gap(16.0)
        .items_start()
        .child(
            col()
                .gap(6.0)
                .items_start()
                .child(text("Icons, trailing badge & status dot, shortcut tooltips").font(k.ty.small_medium).color(k.c.text_weak))
                .child(
                    SegmentedControl::new(&["Comments", "Checks", "Sound", "Versions"], Some(view))
                        .label("Inspector view")
                        .icons(&[Some(&ic::COMMENT), Some(&ic::CIRCLE_CHECK), Some(&glyphs::MIC), Some(&ic::ARCHIVE)])
                        .trailing(0, Badge::new("3").build(k))
                        .trailing(1, dot(k.c.danger.fg))
                        .help(0, "Review comments")
                        .shortcut(0, "cmd+1")
                        .help(1, "CI checks")
                        .shortcut(1, "cmd+2")
                        .description(1, "2 failing checks")
                        .help(2, "Sound alerts")
                        .shortcut(2, "cmd+3")
                        .help(3, "Version history")
                        .shortcut(3, "cmd+4")
                        .fit()
                        .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                            s.pick.insert("seg-rich".into(), i.to_string());
                        })
                        .build(k)
                        .key("seg-rich"),
                ),
        )
        .child(
            col()
                .gap(6.0)
                .items_start()
                .child(text("Icon-only segments (tooltip names each segment)").font(k.ty.small_medium).color(k.c.text_weak))
                .child(
                    SegmentedControl::new(&["", "", ""], Some(icon_view))
                        .label("Layout mode")
                        .icons(&[Some(&ic::BULLET_LIST), Some(&ic::CODE), Some(&ic::TERMINAL)])
                        .help(0, "List")
                        .shortcut(0, "cmd+1")
                        .help(1, "Code")
                        .shortcut(1, "cmd+2")
                        .help(2, "Terminal")
                        .shortcut(2, "cmd+3")
                        .fit()
                        .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                            s.pick.insert("seg-icons".into(), i.to_string());
                        })
                        .build(k)
                        .key("seg-icons"),
                ),
        )
        .child(
            col()
                .w(360.0)
                .gap(8.0)
                .items_start()
                .child(text("Tablist mode (.as_tabs() + .controls())").font(k.ty.small_medium).color(k.c.text_weak))
                .child(
                    SegmentedControl::new(&["General", "Network", "Security"], Some(tab))
                        .label("Settings sections")
                        .icons(&[Some(&ic::SETTINGS_GEAR), Some(&ic::GLOBE), Some(&ic::SHIELD)])
                        .trailing(2, dot(k.c.warning.fg))
                        .description(2, "1 security warning")
                        .controls(0, "seg-tabpanel")
                        .controls(1, "seg-tabpanel")
                        .controls(2, "seg-tabpanel")
                        .as_tabs()
                        .width(None)
                        .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                            s.pick.insert("seg-tabs".into(), i.to_string());
                        })
                        .build(k)
                        .key("seg-tabs"),
                )
                .child(
                    col()
                        .key("seg-tabpanel")
                        .role("tabpanel")
                        .label("Settings section details")
                        .w_full()
                        .padding(10.0)
                        .radius(k.radius.md)
                        .fill(k.c.layer_01)
                        .border(1.0, k.c.border_weak)
                        .child(text(tab_copy).font(k.ty.small).color(k.c.text_base).wrap()),
                ),
        )
}

/// A copy label and countdown, sans and mono. Each press of "Roll" advances one step:
/// "Copy password" → "Copied, clears in 0:30" → "… 0:29", and the countdown "0:30" → "0:29" → ….
pub(super) fn rolling_text_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let step: u32 = s.picked("roll", "0").parse().unwrap_or(0);
    let label = if step == 0 { "Copy password".to_owned() } else { format!("Copied, clears in 0:{:02}", 31 - step.min(31)) };
    let count = format!("0:{:02}", 30 - step.min(30));
    let big = TextStyle::new(28.0, 600.0);
    let line = |name: &str, e: El| row().gap(12.0).center_y().child(text(name).font(k.ty.small).color(k.c.text_faint).w(56.0)).child(e);
    col()
        .gap(14.0)
        .child(
            row()
                .gap(8.0)
                .child(
                    Button::new("Roll")
                        .on_press(|s: &mut G| {
                            let n: u32 = s.picked("roll", "0").parse().unwrap_or(0);
                            s.pick.insert("roll".into(), (n + 1).to_string());
                        })
                        .build(k)
                        .key("roll"),
                )
                .child(Button::new(label.clone()).rolling("roll-button").build(k).key("rolling-button"))
                .child(Button::new("Cancel").build(k).key("cancel")),
        )
        .child(line("Sans", RollingText::new("roll-sans", label.clone()).font(k.ty.body).build(k)))
        .child(line("Mono", RollingText::new("roll-mono", label.clone()).font(k.ty.mono).build(k)))
        .child(line("Centred", row().w(260.0).center().child(RollingText::new("roll-centre", label).font(k.ty.body).build(k))))
        .child(line("Count", RollingText::new("count-sans", count.clone()).font(big).from(Edge::Top).build(k)))
        .child(line("", RollingText::new("count-mono", count).font(big.mono()).from(Edge::Top).build(k)))
}

/// `ScrollBarStyle::scroll_view`: the `@opencode/ui` ScrollView thumb, hidden at rest and shown while
/// the pointer is over the list or it scrolls.
pub(super) fn scroll_view_thumb(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let rows = (1..=24).map(|i| row().key(format!("row{i}")).h(28.0).px(12.0).center_y()
        .child(text(format!("Row {i}")).font(k.ty.ui_compact).color(k.c.text_base)));
    col().key("list").w(240.0).h(180.0).radius(k.radius.md).fill(k.c.bg_base).border(1.0, k.c.border_weak)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(col().children(rows))
        .scroll_bar_styled(ScrollBarStyle::scroll_view(k))
}

/// Container-owned listbox using `El::on_a11y_action`: rows carry `role("option")` with no
/// per-row `on_press`, so pointer, keyboard and assistive clicks all route through the container.
pub(super) fn list_delegated_a11y(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    const ITEMS: [(&str, &str, bool); 4] = [
        ("opt-readme", "README.md", false),
        ("opt-cargo", "Cargo.toml", false),
        ("opt-lib", "src/lib.rs", false),
        ("opt-locked", "Cargo.lock (locked)", true),
    ];
    const ROW_H: f32 = 30.0;
    let selected = s.picked("delegated-a11y", "opt-readme");
    let selected_label = ITEMS.iter().find(|(id, _, _)| *id == selected).map_or("README.md", |(_, name, _)| *name);
    let rows = ITEMS.iter().map(|&(id, name, disabled)| {
        let active = selected == id;
        let fill = if active { k.c.layer_02 } else { rune::Color::TRANSPARENT };
        let fg = if disabled { k.c.text_faint } else if active { k.c.text_strong } else { k.c.text_base };
        row()
            .key(id)
            .role("option")
            .label(name)
            .focus_selected(active)
            .disabled(disabled)
            .w_full()
            .h(ROW_H)
            .px(10.0)
            .center_y()
            .radius(k.radius.sm)
            .fill(fill)
            .child(text(name).font(k.ty.ui_compact).color(fg))
    });
    col()
        .w(280.0)
        .gap(8.0)
        .child(text("Workspace files").key("list-heading").role("heading").level(3).font(k.ty.ui_medium).color(k.c.text_strong))
        .child(
            col()
                .key("delegated-list")
                .role("listbox")
                .label("Workspace files")
                .focusable(true)
                .active_descendant(selected)
                .w_full()
                .padding(4.0)
                .radius(k.radius.md)
                .fill(k.c.bg_base)
                .border(1.0, k.c.border_weak)
                .on_drag(|s: &mut G, d, _| {
                    if d.phase == rune::Phase::Up && !d.cancelled() {
                        let idx = ((d.y - 4.0) / ROW_H).floor() as isize;
                        if let Some(&(id, _, false)) = usize::try_from(idx).ok().and_then(|i| ITEMS.get(i)) {
                            s.pick.insert("delegated-a11y".into(), id.into());
                        }
                    }
                })
                .on_key(|s: &mut G, ev, _| {
                    let enabled: Vec<&str> = ITEMS.iter().filter(|(_, _, d)| !d).map(|(id, _, _)| *id).collect();
                    let cur = enabled.iter().position(|id| *id == s.picked("delegated-a11y", "opt-readme")).unwrap_or(0);
                    let next = match ev.key {
                        rune::Key::Down => (cur + 1).min(enabled.len() - 1),
                        rune::Key::Up => cur.saturating_sub(1),
                        _ => return false,
                    };
                    s.pick.insert("delegated-a11y".into(), enabled[next].into());
                    true
                })
                .on_a11y_action(|s: &mut G, key, action, _| {
                    if action == rune::accesskit::Action::Click
                        && ITEMS.iter().any(|&(id, _, disabled)| id == key && !disabled)
                    {
                        s.pick.insert("delegated-a11y".into(), key.to_owned());
                        return true;
                    }
                    false
                })
                .children(rows),
        )
        .child(
            text(format!("Selected: {selected_label}"))
                .key("delegated-status")
                .role("status")
                .font(k.ty.small)
                .color(k.c.text_weak),
        )
}
/// Accessibility story exercising `El::aria_disabled` (focusable unavailable button vs truly
/// disabled), a horizontal `tablist` with `El::orientation` and `El::controls`, an editable
/// `combobox` with `El::auto_complete` (`List` / `Inline` / `Both`), and a popup trigger with
/// `El::has_popup`, `El::controls`, `El::owns`, `El::multiselectable`, and `El::required`.
pub(super) fn a11y_aria_properties(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    const TABS: [(&str, &str, &str); 3] = [
        ("tab-overview", "Overview", "aria_disabled keeps a pending button in the Tab order while reading as disabled."),
        ("tab-relations", "Relations", "controls and owns resolve nearest scope first, falling back to root overlays."),
        ("tab-lists", "Lists", "multiselectable, required, has_popup and orientation reach AccessKit and web."),
    ];
    const FRUITS: [(&str, &str); 3] = [
        ("fruit-apple", "Apple"),
        ("fruit-banana", "Banana"),
        ("fruit-cherry", "Cherry"),
    ];
    let pending = !s.is("aria-ready");
    let active_tab = s.picked("aria-tab", "tab-overview");
    let panel_copy = TABS.iter().find(|(id, _, _)| *id == active_tab).map_or(TABS[0].2, |(_, _, body)| *body);
    let popup_open = !s.is("aria-popup-closed");
    let ac_mode = s.picked("aria-ac", "list");
    let ac = match ac_mode {
        "inline" => Some(rune::AutoComplete::Inline),
        "both" => Some(rune::AutoComplete::Both),
        "none" => None,
        _ => Some(rune::AutoComplete::List),
    };

    let mut pending_btn = row()
        .key("aria-pending-save")
        .role("button")
        .h(32.0)
        .px(12.0)
        .center_y()
        .radius(k.radius.sm)
        .fill(if pending { k.c.surface_base } else { k.c.layer_02 })
        .border(1.0, k.c.border_weak)
        .focusable(true)
        .aria_disabled(pending)
        .child(
            text(if pending { "Saving…" } else { "Save" })
                .font(k.ty.ui_medium)
                .color(if pending { k.c.text_muted } else { k.c.text_strong }),
        );
    if !pending {
        pending_btn = pending_btn.on_press(|s: &mut G| s.flip("aria-ready"));
    }

    let tabs = TABS.iter().map(|&(id, label, _)| {
        let selected = active_tab == id;
        row()
            .key(id)
            .role("tab")
            .h(28.0)
            .px(10.0)
            .center_y()
            .radius(k.radius.sm)
            .fill(if selected { k.c.layer_02 } else { rune::Color::TRANSPARENT })
            .focusable(true)
            .focus_selected(selected)
            .controls("aria-tabpanel")
            .on_press(move |s: &mut G| { s.pick.insert("aria-tab".into(), id.into()); })
            .child(text(label).font(k.ty.ui_compact).color(if selected { k.c.text_strong } else { k.c.text_weak }))
    });

    let mut combo = rune::text_input(&s.aria_combo_edit, k.ty.ui_compact)
        .single_line().key("aria-combo").role("combobox").label("Fruit filter")
        .placeholder("Filter fruit…", k.c.text_faint).color(k.c.text_base).caret(k.c.text_base, k.selection())
        .grow(1.0).h(32.0).px(10.0).radius(k.radius.sm).fill(k.c.bg_base).border(1.0, k.c.border_weak)
        .expanded(popup_open).has_popup(rune::HasPopup::Listbox).controls("aria-popup-list")
        .on_edit(|s: &mut G| &mut s.aria_combo_edit);
    if let Some(mode) = ac { combo = combo.auto_complete(mode); }
    let mode_btns = ["list", "inline", "both", "none"].into_iter().map(|m| {
        Button::new(m).variant(if ac_mode == m { ButtonVariant::Neutral } else { ButtonVariant::Ghost })
            .on_press(move |s: &mut G| { s.pick.insert("aria-ac".into(), m.into()); })
            .build(k).key(format!("aria-ac-{m}"))
    });

    let mut root = col()
        .w(360.0)
        .gap(12.0)
        .child(
            col().gap(6.0)
                .child(text("Pending vs disabled buttons").font(k.ty.small_medium).color(k.c.text_weak))
                .child(
                    row().gap(8.0).center_y()
                        .child(pending_btn)
                        .child(
                            Button::new(if pending { "Finish save" } else { "Start save" })
                                .variant(ButtonVariant::Ghost)
                                .on_press(|s: &mut G| s.flip("aria-ready"))
                                .build(k)
                                .key("aria-toggle-pending"),
                        )
                        .child(
                            Button::new("Disabled")
                                .disabled(true)
                                .build(k)
                                .key("aria-hard-disabled"),
                        ),
                ),
        )
        .child(
            col().gap(6.0)
                .child(
                    row()
                        .key("aria-tablist")
                        .role("tablist")
                        .label("ARIA details")
                        .orientation(rune::Orientation::Horizontal)
                        .focus_group()
                        .gap(4.0)
                        .padding(3.0)
                        .radius(k.radius.md)
                        .fill(k.c.bg_base)
                        .border(1.0, k.c.border_weak)
                        .children(tabs),
                )
                .child(
                    col()
                        .key("aria-tabpanel")
                        .role("tabpanel")
                        .label("ARIA detail panel")
                        .w_full()
                        .padding(10.0)
                        .radius(k.radius.md)
                        .fill(k.c.layer_01)
                        .border(1.0, k.c.border_weak)
                        .child(text(panel_copy).font(k.ty.small).color(k.c.text_base).wrap()),
                ),
        )
        .child(
            col().gap(6.0)
                .child(row().gap(6.0).center_y().child(combo))
                .child(row().gap(4.0).center_y().children(mode_btns)),
        )
        .child(
            row()
                .key("aria-popup-owner")
                .role("group")
                .label("Fruit picker")
                .owns("aria-popup-list")
                .gap(8.0)
                .center_y()
                .child(
                    row()
                        .key("aria-popup-trigger")
                        .role("button")
                        .h(32.0)
                        .px(12.0)
                        .center_y()
                        .radius(k.radius.sm)
                        .fill(k.c.surface_base)
                        .border(1.0, k.c.border_weak)
                        .focusable(true)
                        .expanded(popup_open)
                        .has_popup(rune::HasPopup::Listbox)
                        .controls("aria-popup-list")
                        .on_press(|s: &mut G| s.flip("aria-popup-closed"))
                        .child(text(if popup_open { "Hide fruit list" } else { "Show fruit list" }).font(k.ty.ui_compact).color(k.c.text_base)),
                ),
        );

    if popup_open {
        let rows = FRUITS.iter().map(|&(id, label)| {
            let selected = s.is(id) || (id == "fruit-apple" && !s.is("aria-fruit-touched"));
            row()
                .key(id)
                .role("option")
                .label(label)
                .h(28.0)
                .w_full()
                .px(10.0)
                .center_y()
                .radius(k.radius.sm)
                .fill(if selected { k.c.layer_02 } else { rune::Color::TRANSPARENT })
                .focusable(true)
                .focus_selected(selected)
                .on_press(move |s: &mut G| {
                    if !s.is("aria-fruit-touched") {
                        s.on.insert("aria-fruit-touched".into());
                        s.on.insert("fruit-apple".into());
                    }
                    s.flip(id);
                })
                .child(text(if selected { format!("✓ {label}") } else { label.to_owned() }).font(k.ty.ui_compact).color(k.c.text_base))
        });
        root = root.child(
            col()
                .key("aria-popup-list")
                .role("listbox")
                .label("Fruits")
                .multiselectable(true)
                .required(true)
                .orientation(rune::Orientation::Vertical)
                .focus_group()
                .w_full()
                .padding(4.0)
                .gap(2.0)
                .radius(k.radius.md)
                .fill(k.c.bg_base)
                .border(1.0, k.c.border_weak)
                .children(rows),
        );
    }

    root
}

