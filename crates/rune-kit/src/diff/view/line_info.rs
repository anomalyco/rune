//! [`FoldRow::LineInfo`]: folded unchanged lines as Pierre's `line-info-basic` separator draws
//! them in OpenCode Desktop's review.

use rune::{Anim, Cursor, El, Icon, Span, Tx, col, icon, rich, row, text};

use super::View;
use crate::diff::state::{DiffState, EXPAND_STEP, Expand, GAP_H};

// Pierre's separator glyphs (`diffs-icon-expand`, `diffs-icon-expand-all`). Its "up" button
// shows the chevron as drawn, pointing down into the lines it opens; its "down" button flips it.
static LINE_INFO_UP: Icon = Icon {
    name: "kit-diff-line-info-up",
    view: 16.0,
    body: r#"<path fill="currentColor" d="M3.47 5.47a.75.75 0 0 1 1.06 0L8 8.94l3.47-3.47a.75.75 0 1 1 1.06 1.06l-4 4a.75.75 0 0 1-1.06 0l-4-4a.75.75 0 0 1 0-1.06"/>"#,
};
static LINE_INFO_DOWN: Icon = Icon {
    name: "kit-diff-line-info-down",
    view: 16.0,
    body: r#"<g transform="matrix(1 0 0 -1 0 16)"><path fill="currentColor" d="M3.47 5.47a.75.75 0 0 1 1.06 0L8 8.94l3.47-3.47a.75.75 0 1 1 1.06 1.06l-4 4a.75.75 0 0 1-1.06 0l-4-4a.75.75 0 0 1 0-1.06"/></g>"#,
};
static LINE_INFO_ALL: Icon = Icon {
    name: "kit-diff-line-info-all",
    view: 16.0,
    body: r#"<path fill="currentColor" d="M11.47 9.47a.75.75 0 1 1 1.06 1.06l-4 4a.75.75 0 0 1-1.06 0l-4-4a.75.75 0 1 1 1.06-1.06L8 12.94zM7.526 1.418a.75.75 0 0 1 1.004.052l4 4a.75.75 0 1 1-1.06 1.06L8 3.06 4.53 6.53a.75.75 0 1 1-1.06-1.06l4-4z"/>"#,
};

/// The diff's background between the buttons and right of them
/// (Pierre's `border-right: 2px solid var(--diffs-bg)`, and 1 px under and over two stacked ones).
const LINE_INFO_GAP: f32 = 2.0;

/// How a folded range of unchanged lines draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FoldRow {
    /// Round buttons centred in the gutter, then "N unchanged lines" (pressing it opens them
    /// all past [`EXPAND_STEP`]) and the next hunk's header.
    #[default]
    Summary,
    /// Pierre's `line-info-basic` separator, as OpenCode Desktop's review draws it: buttons
    /// that fill the gutter, then "N unmodified lines" in the interface face, underlined under
    /// the pointer, opening [`EXPAND_STEP`] lines at each end when pressed. A range of more
    /// than [`EXPAND_STEP`] lines between hunks gets two half-height buttons, one opening lines
    /// down from the hunk above and one up from the hunk below; a shorter one gets one that
    /// opens it all; the first and last ranges get one toward their hunk. Shift on a button
    /// opens the whole range. The chevrons are faint until the pointer is on them.
    LineInfo,
}

impl<S: 'static> View<S> {
    /// The unchanged lines between two hunks as [`FoldRow::LineInfo`] draws them.
    pub(super) fn line_info(&self, st: &DiffState, f: usize, gap: usize) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let hidden = st.hidden(f, gap).unwrap_or(0);
        let (first, last) = (gap == 0, gap == file.hunks.len());
        // Pierre's words, which OpenCode Desktop shows untranslated.
        let count = if hidden == 1 { "1 unmodified line".to_owned() } else { format!("{hidden} unmodified lines") };
        let mut inner = row().key("inner").h(GAP_H).min_w(0.0);
        if hidden > 0 && file.expandable() {
            let steps: Vec<(Expand, &'static Icon, String)> = match (first, last) {
                (true, _) => vec![(Expand::Up, &LINE_INFO_DOWN, format!("Show {} more lines above", hidden.min(EXPAND_STEP)))],
                (_, true) => vec![(Expand::Down, &LINE_INFO_UP, format!("Show {} more lines below", hidden.min(EXPAND_STEP)))],
                _ if hidden <= EXPAND_STEP => vec![(Expand::All, &LINE_INFO_ALL, format!("Show {hidden} hidden lines"))],
                _ => vec![
                    (Expand::Down, &LINE_INFO_UP, format!("Show {EXPAND_STEP} more lines below")),
                    (Expand::Up, &LINE_INFO_DOWN, format!("Show {EXPAND_STEP} more lines above")),
                ],
            };
            let w = self.gutter_w(st, f) - LINE_INFO_GAP;
            let h = if steps.len() == 2 { (GAP_H - LINE_INFO_GAP) / 2.0 } else { GAP_H };
            let buttons = steps.into_iter().map(|(how, glyph, label)| {
                let get = self.d.get_mut.clone();
                row()
                    .key(format!("{how:?}"))
                    .w(w)
                    .h(h)
                    .fill(l.separator)
                    .hover_group()
                    .hover_motion(Anim::INSTANT, Anim::INSTANT)
                    .cursor(Cursor::Pointer)
                    .role("button")
                    .label(label)
                    .child(icon(glyph, 16.0).color(l.faint).group_color(l.text).absolute((w - 16.0) / 2.0, (h - 16.0) / 2.0))
                    .on_press_tx(move |s: &mut S, tx: &mut Tx| get(s).expand(f, gap, if tx.mods().shift { Expand::All } else { how }))
            });
            inner = inner.child(col().key("expand").w(w + LINE_INFO_GAP).h(GAP_H).gap(LINE_INFO_GAP).fill(l.bg).children(buttons));
            // The words open the range from both ends; the pointer anywhere on the row underlines them.
            let get = self.d.get_mut.clone();
            let words = col()
                .key("words")
                .role("button")
                .label(count.clone())
                .cursor(Cursor::Pointer)
                .on_press_tx(move |s: &mut S, _| {
                    let st = get(s);
                    let ends: &[Expand] = match (first, last) {
                        (true, _) => &[Expand::Up],
                        (_, true) => &[Expand::Down],
                        _ => &[Expand::Down, Expand::Up],
                    };
                    for &how in ends {
                        st.expand(f, gap, how);
                    }
                })
                .child(text(count.clone()).key("count").font(l.separator_text).color(l.faint).group_opacity(1.0, 0.0))
                .child(
                    rich(count.clone(), vec![Span::new(0, count.len()).underline(l.faint)])
                        .key("underlined")
                        .font(l.separator_text)
                        .color(l.faint)
                        .group_opacity(0.0, 1.0)
                        .absolute(0.0, 0.0),
                );
            inner = inner.child(row().key("content").h(GAP_H).px(l.adv).items_center().hover_group().hover_motion(Anim::INSTANT, Anim::INSTANT).cursor(Cursor::Pointer).child(words));
        } else {
            // With nothing to open, the count starts 1ch into the gutter.
            inner = inner.child(row().key("content").h(GAP_H).px(l.adv).items_center().child(text(count.clone()).key("count").font(l.separator_text).color(l.faint)));
        }
        if let Some(w) = self.view_w(st, false) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        row().w_full().h(GAP_H).fill(l.separator).role("group").label(count).child(inner)
    }
}
