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
