//! The "Lists/Scroll anchoring" playground story: `El::no_scroll_anchor` on a top loading
//! placeholder or on a `virtual_rows` list, and interior descendant anchoring inside a tall
//! mounted `virtual_rows` row.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, GrowthSpec, ScrollSpec, Story, Tx, col, el, row, text};

pub(crate) struct AnchorDemo {
    pub(crate) older: usize,
    pub(crate) show_detail: bool,
    pub(crate) list_no_anchor: bool,
}

impl Default for AnchorDemo {
    fn default() -> Self {
        Self { older: 0, show_detail: true, list_no_anchor: false }
    }
}

fn row_key(s: &AnchorDemo, i: usize) -> String {
    if i == 0 {
        "loading".to_owned()
    } else if i <= s.older {
        format!("older-{}", s.older - i + 1)
    } else if i == s.older + 1 {
        "tall".to_owned()
    } else {
        format!("msg-{}", i - s.older - 1)
    }
}

fn row_count(s: &AnchorDemo) -> usize {
    1 + s.older + 1 + 14
}

pub(crate) fn anchor_story() -> Story<AnchorDemo> {
    Story::new("Lists/Scroll anchoring", AnchorDemo::default, anchor_demo_view)
        .trigger('p', "Prepend 5 rows", |s, _| s.older += 5)
        .trigger('d', "Toggle detail above passage", |s, _| s.show_detail = !s.show_detail)
        .trigger('a', "Toggle list no_scroll_anchor", |s, _| s.list_no_anchor = !s.list_no_anchor)
        .variant("Placeholder opt-out", |_| {})
        .variant("Detail collapsed", |s| s.show_detail = false)
        .variant("List no_scroll_anchor", |s| s.list_no_anchor = true)
}

pub(crate) fn anchor_demo_view(s: &AnchorDemo, cx: &mut Cx<AnchorDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let accent_ok = tok::accent(4, cx.system_dark());

    let btn = |key: &'static str, label: &'static str, active: bool, f: fn(&mut AnchorDemo, &mut Tx)| {
        row().key(key).h(26.0).px(10.0).center().radius(radius::SM)
            .fill(if active { accent } else { t.control_track })
            .hover(if active { accent } else { t.hover })
            .child(text(label).font(type_::LABEL).color(if active { Color::WHITE } else { t.text }))
            .focusable(true).role("button").label(label)
            .on_press_tx(move |s: &mut AnchorDemo, tx: &mut Tx| f(s, tx))
    };

    let list_label = if s.list_no_anchor { "List anchor: off" } else { "List anchor: on" };
    let detail_label = if s.show_detail { "Hide detail" } else { "Show detail" };

    let mut rows = col().key("rows").w_full().gap(6.0).padding(8.0).growth(GrowthSpec::default())
        .virtual_rows(
            row_count(s),
            44.0,
            |s: &AnchorDemo, i| row_key(s, i).into(),
            move |s: &AnchorDemo, i| {
                if i == 0 {
                    row().w_full().h(34.0).px(10.0).center_y().justify_between()
                        .radius(radius::SM).fill(t.control_track)
                        .no_scroll_anchor()
                        .child(text("Loading older messages…").font(type_::LABEL).color(t.text_secondary))
                        .child(text("no_scroll_anchor").font(type_::LABEL).color(t.text_secondary))
                } else if i <= s.older {
                    let n = s.older - i + 1;
                    row().w_full().h(36.0).px(10.0).center_y()
                        .radius(radius::SM).fill(t.card).outline(1.0, 0.0, t.card_border)
                        .child(text(format!("Earlier message #{n}")).font(type_::BODY).color(t.text))
                } else if i == s.older + 1 {
                    col().w_full().padding(10.0).gap(8.0)
                        .radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                        .child(
                            col().key("intro").w_full().h(180.0).padding(8.0).radius(radius::SM).fill(t.control_track)
                                .child(text("Tall message (row 1) — top section").font(type_::LABEL).color(t.text_secondary)),
                        )
                        .children(s.show_detail.then(|| {
                            col().key("detail").w_full().h(110.0).padding(8.0).radius(radius::SM)
                                .fill(t.hover).outline(1.0, 0.0, t.card_border)
                                .child(text("Collapsible tool output (+110 pt)").font(type_::LABEL).color(t.text))
                        }))
                        .child(
                            row().key("passage").w_full().h(40.0).px(10.0).center_y()
                                .radius(radius::SM).fill(accent_ok)
                                .child(text("Current passage — stays anchored").font(type_::BODY).color(Color::WHITE)),
                        )
                        .child(
                            col().key("tail").w_full().h(300.0).padding(8.0).radius(radius::SM).fill(t.control_track)
                                .child(text("Trailing prose below current passage").font(type_::LABEL).color(t.text_secondary)),
                        )
                } else {
                    let n = i - s.older - 1;
                    row().w_full().h(36.0).px(10.0).center_y()
                        .radius(radius::SM).fill(t.card).outline(1.0, 0.0, t.card_border)
                        .child(text(format!("Follow-up message #{n}")).font(type_::BODY).color(t.text))
                }
            },
        );
    if s.list_no_anchor {
        rows = rows.no_scroll_anchor();
    }

    let viewport = col().key("feed").w(440.0).h(260.0)
        .radius(radius::MD).fill(t.bg).outline(1.0, 0.0, t.card_border)
        .scroll(ScrollSpec::default())
        .scroll_anchor()
        .child(rows);

    col().w_full().h_full().center().gap(10.0).padding(16.0)
        .child(
            row().gap(6.0).center_y()
                .child(btn("prepend", "Prepend 5 rows", false, |s, _| s.older += 5))
                .child(btn("jump-passage", "Jump to passage", false, |_, tx| tx.scroll_key_to("tall/passage", 24.0)))
                .child(btn("toggle-detail", detail_label, s.show_detail, |s, _| s.show_detail = !s.show_detail))
                .child(btn("toggle-list-anchor", list_label, s.list_no_anchor, |s, _| s.list_no_anchor = !s.list_no_anchor)),
        )
        .child(viewport)
        .child(
            el().w(440.0).child(
                text(format!(
                    "Prepended: {} · detail: {} · list anchor: {}",
                    s.older,
                    if s.show_detail { "shown" } else { "hidden" },
                    if s.list_no_anchor { "off" } else { "on" },
                ))
                .font(type_::LABEL)
                .color(t.text_secondary),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn scroll_anchor_story_keeps_passage_and_skips_placeholder_and_exposes_a11y() {
        let mut rt = Runtime::new(AnchorDemo::default(), anchor_demo_view, (520.0, 360.0), Env::new(true, 1.0));
        rt.frame(0.0);

        let a11y = rt.accessibility_json().to_string();
        for label in ["Prepend 5 rows", "Jump to passage", "Hide detail", "List anchor: on"] {
            assert!(a11y.contains(label), "missing accessible button label {label:?}");
        }

        let y_of = |rt: &Runtime<AnchorDemo>, k: &str| {
            rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).map(|e| e.rect[1])
        };

        // 1. Prepending with placeholder no_scroll_anchor holds "tall" in place.
        let tall_before = y_of(&rt, "tall").expect("tall row visible");
        rt.update(|s, _| s.older += 5);
        for _ in 0..20 { rt.frame(0.016); }
        let tall_after = y_of(&rt, "tall").expect("tall row stays visible");
        assert!((tall_after - tall_before).abs() < 1.0, "tall moved: {tall_before} -> {tall_after}");

        // 2. Jumping to tall/passage and hiding the 110pt detail block above it keeps passage still.
        rt.update(|_, tx| tx.scroll_key_to("tall/passage", 24.0));
        for _ in 0..20 { rt.frame(0.016); }
        let passage_before = y_of(&rt, "passage").expect("passage visible");
        rt.update(|s, _| s.show_detail = false);
        for _ in 0..20 { rt.frame(0.016); }
        let passage_after = y_of(&rt, "passage").expect("passage stays visible");
        assert!((passage_after - passage_before).abs() < 1.0, "passage moved: {passage_before} -> {passage_after}");

        // 3. With list_no_anchor at offset 0, newly prepended rows appear immediately at the top.
        rt.update(|s, tx| {
            s.list_no_anchor = true;
            tx.scroll_to_instant("feed", 0.0, 0.0);
        });
        for _ in 0..10 { rt.frame(0.016); }
        let loading_before = y_of(&rt, "loading").expect("loading visible at top");
        rt.update(|s, _| s.older += 3);
        for _ in 0..10 { rt.frame(0.016); }
        assert!((y_of(&rt, "loading").unwrap() - loading_before).abs() < 1.0, "loading stays at top");
        assert!(y_of(&rt, "older-8").is_some(), "newest prepended row is revealed at offset 0");
    }
}
