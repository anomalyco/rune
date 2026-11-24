//! `Cx::selection_rects`: the line boxes of the [`El::selectable`](crate::El::selectable)
//! text selection, for a view that places something over it (a Quote button). It is a layout
//! read like `Cx::layout`, under a key no element can have, so it settles, replays from
//! `memo` and re-runs the view the same way; the boxes ride along with the rects the view is
//! given.
use super::*;

/// The layout-read key `Cx::selection_rects` reads under.
pub(crate) const SELECTION: &str = "\u{1}selection";

/// The prefix of a `Cx::field_selection_rects` layout-read key, which no element key starts with.
const FIELD_SELECTION: &str = "\u{1}field_selection:";

/// The layout-read key `Cx::field_selection_rects(key)` is noted under.
pub(crate) fn field_selection_key(key: &str) -> String {
    format!("{FIELD_SELECTION}{key}")
}

/// The field key a `Cx::field_selection_rects` read key stands for, or `None` for other reads.
pub(crate) fn field_selection_read_key(key: &str) -> Option<&str> {
    key.strip_prefix(FIELD_SELECTION)
}

/// The focused non-secure text field's path and selection line boxes, when any view read
/// `Cx::field_selection_rects`.
pub(crate) type FieldSelection = Option<(Rc<str>, Rc<[[f32; 4]]>)>;

/// Whether focused field path `path` matches `key`, mirroring [`Cx::caret`] (the field's own
/// key or an ancestor's key) plus key paths such as `"composer/input"`.
pub(crate) fn field_path_matches(path: &str, key: &str) -> bool {
    if key.is_empty() { return false; }
    path == key
        || path.split('/').any(|seg| element_path::segment_matches_key(seg, key))
        || (key.contains('/') && element_path::keyed_suffix_matches(path, key))
}

/// The selection line boxes of `sel` when its focused field matches `key`, else empty.
pub(crate) fn field_selection_of<'a>(sel: &'a FieldSelection, key: &str) -> &'a [[f32; 4]] {
    match sel {
        Some((path, boxes)) if field_path_matches(path, key) => boxes,
        _ => &[],
    }
}

impl cx::CxCore {
    pub(crate) fn selection_rects(&self) -> Vec<[f32; 4]> {
        layout_read::note_read(&mut self.layout_reads.borrow_mut(), SELECTION, None);
        crate::memo::read_selection(&self.layout.selection);
        self.layout.selection.to_vec()
    }

    #[track_caller]
    pub(crate) fn field_selection_rects(&self, key: &str) -> Vec<[f32; 4]> {
        let read_key = field_selection_key(key);
        layout_read::note_read_at(
            &mut self.layout_reads.borrow_mut(),
            &read_key,
            None,
            Some(std::panic::Location::caller()),
        );
        let boxes = match &self.layout.field_selection {
            Some((path, boxes)) if field_path_matches(path, key) => boxes.clone(),
            _ => Rc::default(),
        };
        crate::memo::read_field_selection(key, &boxes);
        boxes.to_vec()
    }
}

impl<S> Cx<S> {
    /// The line boxes of the text selection, `[x, y, w, h]` each in window points after
    /// scrolling, in reading order: one per line of each selected text, as its highlight draws
    /// them. Empty (and allocating nothing) when nothing is selected or only a caret is placed.
    /// Boxes aren't clipped to their scroll viewport, so a view can tell when the selection has
    /// scrolled out of sight. Only mounted texts have boxes: the rows of a virtual list that
    /// scrolled away have none. Like [`Cx::layout`], boxes are layout points: an ancestor's
    /// [`El::scale`](crate::El::scale) isn't applied.
    ///
    /// A view that reads it re-runs when a box appears, goes, or moves by more than half a
    /// point: in the same commit when layout moved it, the next frame when the selection
    /// changed or scrolling moved it. While a drag changes the selection that's every frame,
    /// so a toolbar that waits for the release
    /// ([`SelectionChange::dragging`](crate::SelectionChange::dragging)) should only read it
    /// once shown. A `memo` that reads it is rebuilt when the boxes move.
    #[inline(always)]
    pub fn selection_rects(&self) -> Vec<[f32; 4]> {
        self.core.selection_rects()
    }

    /// The focused text field's selection line boxes, `[x, y, w, h]` each in window points after
    /// scrolling (including a single-line field's horizontal scroll), in reading order, when that
    /// field is keyed `key`, sits inside an element keyed `key`, or matches key path `key`.
    ///
    /// Empty (and allocating nothing) when the field is unfocused, has only a caret, is composing
    /// an IME preedit, or is secure. Like [`Cx::selection_rects`] and [`Cx::caret`], boxes are
    /// unclipped layout points (an ancestor's [`El::scale`](crate::El::scale) isn't applied).
    ///
    /// A view that reads it re-runs when a box appears, goes, or moves by more than half a point:
    /// in the same commit when layout or an edit moved it, and the next frame when scrolling
    /// moved it. A `memo` that reads it is rebuilt when the boxes move.
    ///
    /// ```
    /// use rune::{Cx, El, Env, Runtime, TextEdit, TextStyle, col, text_input};
    ///
    /// fn view(edit: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
    ///     let _boxes = cx.field_selection_rects("composer");
    ///     col().size(240.0, 80.0).child(
    ///         text_input(edit, TextStyle::new(14.0, 400.0)).key("composer").w_full(),
    ///     )
    /// }
    ///
    /// let mut edit = TextEdit::new("hello world");
    /// edit.focused = true;
    /// edit.set_selection(0, 5);
    /// let mut rt = Runtime::new(edit, view, (240.0, 80.0), Env::new(true, 1.0));
    /// rt.frame(0.0);
    /// ```
    #[track_caller]
    #[inline(always)]
    pub fn field_selection_rects(&self, key: &str) -> Vec<[f32; 4]> {
        self.core.field_selection_rects(key)
    }
}

/// Bytes `a..b` of the text at a path.
pub(crate) type SelectedRange = (String, usize, usize);

/// Whether any box of `a` and `b` differs by more than half a point, or one has a box the
/// other lacks.
pub(crate) fn boxes_moved(a: &[[f32; 4]], b: &[[f32; 4]]) -> bool {
    a.len() != b.len() || a.iter().zip(b).any(|(x, y)| layout_read::moved(Some(*x), Some(*y)))
}

impl RuntimeCore {
    /// The selection's line boxes laid out in the last commit's nodes.
    pub(super) fn selection_boxes(&self) -> Vec<[f32; 4]> {
        let mut out = Vec::new();
        for (i, a, b) in self.selection_ranges() { self.text_boxes(&self.tree.nodes, i, a, b, &mut out); }
        out
    }

    /// The selection's ranges by the path of each selected text. A measure reads them before it
    /// runs: reusing a `memo` moves its nodes out of the last commit's, leaving placeholders
    /// the selection can't be found in.
    pub(crate) fn selection_by_path(&self) -> Vec<SelectedRange> {
        self.selection_ranges().into_iter().map(|(i, a, b)| (self.tree.nodes[i].path.clone(), a, b)).collect()
    }

    /// The line boxes of `ranges` laid out in a measure's `nodes`, found there by path.
    pub(super) fn selection_boxes_in(&self, nodes: &[Node], ranges: &[SelectedRange]) -> Vec<[f32; 4]> {
        let mut out = Vec::new();
        for (path, a, b) in ranges {
            if let Some(j) = nodes.iter().position(|n| n.path == *path) { self.text_boxes(nodes, j, *a, *b, &mut out); }
        }
        out
    }

    /// Pushes the line boxes of bytes `a..b` of node `j`'s text onto `out`.
    fn text_boxes(&self, nodes: &[Node], j: usize, a: usize, b: usize, out: &mut Vec<[f32; 4]>) {
        let n = &nodes[j];
        let (Some(spec), Some(laid)) = (&n.text, &n.text_layout) else { return };
        let flow = laid.view();
        let len = spec.content.len();
        let (a, b) = (a.min(len), b.min(len));
        if a >= b { return; }
        let (a, b) = n.elided.map_or((a, b), |e| e.range(a, b));
        let [x, y, w, h] = self.laid_out(nodes, j);
        let s = self.env.scale;
        let [ox, oy] = links::origin(n, spec, flow, [x, y, x + w, y + h], s, 1.0);
        for (bb, _) in flow.range_boxes(a, b, true) {
            let (x0, y0) = (ox + bb.x0 as f32 / s, oy + bb.y0 as f32 / s);
            out.push([x0, y0, (bb.x1 - bb.x0) as f32 / s, (bb.y1 - bb.y0) as f32 / s]);
        }
    }

    /// The focused non-secure text field's path and selection line boxes in `nodes`.
    pub(super) fn field_selection_boxes_in(&self, nodes: &[Node]) -> FieldSelection {
        let live_tree = std::ptr::eq(nodes, self.tree.nodes.as_slice());
        let i = nodes.iter().enumerate().position(|(i, n)| {
            (!live_tree || !self.subtree_exiting(i))
                && n.input.as_ref().is_some_and(|inp| inp.edit.focused)
        })?;
        let boxes = self.field_selection_boxes_of(nodes, i);
        if boxes.is_empty() { return None; }
        Some((Rc::from(nodes[i].path.as_str()), boxes.into()))
    }

    /// Selection line boxes of focused non-secure text field `i` in `nodes`.
    fn field_selection_boxes_of(&self, nodes: &[Node], i: usize) -> Vec<[f32; 4]> {
        let n = &nodes[i];
        if n.is_secure() { return Vec::new(); }
        let (Some(inp), Some(spec), Some(laid)) = (n.input.as_ref(), n.text.as_ref(), n.text_layout.as_ref()) else {
            return Vec::new();
        };
        if !inp.edit.focused || inp.edit.preedit().is_some() { return Vec::new(); }
        let (a, b) = inp.edit.range();
        if a == b { return Vec::new(); }
        let (da, db) = inp.edit.display_selection_range(a, b);
        if da >= db { return Vec::new(); }
        let [ox, oy] = self.field_text_origin(nodes, i, inp, spec, laid);
        let flow = laid.view();
        let s = self.env.scale;
        let mut out = Vec::new();
        for (bb, _) in flow.range_boxes(da, db, true) {
            out.push([
                ox + bb.x0 as f32 / s,
                oy + bb.y0 as f32 / s,
                (bb.x1 - bb.x0) as f32 / s,
                (bb.y1 - bb.y0) as f32 / s,
            ]);
        }
        out
    }

    /// Once a frame: the selection moved since the view last read it (a drag, a click, code,
    /// scrolling). That isn't layout feedback, so the view re-runs as for a state change.
    pub(super) fn selection_read_moved(&self) -> bool {
        let seen = &self.commit.layout_seen.rects;
        seen.contains_key(SELECTION) && boxes_moved(&seen.selection, &self.selection_boxes())
    }

    /// Once a frame (or after scroll requests): a watched field selection moved since the view
    /// last read it. That isn't layout feedback, so the view re-runs as for a state change.
    pub(in crate::runtime) fn field_selection_read_moved(&self) -> bool {
        let seen = &self.commit.layout_seen;
        let mut keys = seen.reads.iter().filter_map(|r| field_selection_read_key(&r.key)).peekable();
        if keys.peek().is_none() { return false; }
        let now = self.field_selection_boxes_in(&self.tree.nodes);
        keys.any(|k| boxes_moved(field_selection_of(&seen.rects.field_selection, k), field_selection_of(&now, k)))
    }
}
