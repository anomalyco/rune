//! The focused text field's caret in window points, for views (`Cx::caret`) and tools
//! (`ElementInfo::caret`). It comes from layout, not the drawn frame, so a view that reads it
//! is re-measured in the same commit when an edit moves it.
use super::*;

/// Text selection, links, field click/scroll, caret, IME, spellcheck and clipboard state owned by
/// [`RuntimeCore`].
pub(crate) struct TextInputState {
    /// The selection in `El::selectable` text.
    pub(in crate::runtime) selectable: selectable::phase::TextSel,
    /// `Tx::text_selection`'s snapshot, kept while neither a commit nor the selection changed.
    pub(super) sel_snapshot: std::cell::RefCell<Option<selectable::Snapshot>>,
    /// What each `El::on_selection_change` element (by path) last heard.
    pub(in crate::runtime) sel_heard: selectable::Heard,
    pub(super) links: links::LinkState,
    /// Multi-click and drag selection state for `text_input`.
    pub(in crate::runtime) input_click: input_click::InputClick,
    /// Sideways scroll of focused single-line fields, in points, keyed by node path.
    pub(crate) input_scroll: HashMap<String, f32>,
    /// The focused field's caret (path, window rect) the next view sees through `Cx::caret`.
    pub(in crate::runtime) caret: Option<(String, [f32; 4])>,
    /// The last view read the caret, so it re-runs when the caret moves.
    pub(in crate::runtime) caret_watch: bool,
    /// An edit ran this frame: keep the caret in its scroll viewport after the commit.
    pub(in crate::runtime) caret_follow: bool,
    /// The composition `App::on_ime` currently has open.
    app_ime_comp: app_ime::AppComposition,
    /// The `El::ime_cursor` that has the input method asks for Option as Alt (`El::ime_option_as_alt`).
    pub(crate) ime_option_as_alt: bool,
    /// The shell cancels native composition before enabling the new text focus.
    pub(crate) ime_reset: bool,
    /// The focused spellchecked field's word at the caret (`spell::pause`).
    pub(in crate::runtime) spell_watch: Option<crate::spell::pause::Watch>,
    /// Linux's primary selection (`runtime/input/primary_selection.rs`).
    #[cfg(target_os = "linux")]
    pub(crate) primary: primary_selection::PrimarySelection,
    // Outer None: no read; inner None: app-level paste rather than a focused editor.
    pub(crate) paste_request: Option<Option<String>>,
    /// `Tx::read_clipboard` reads for the shell to start, by id.
    pub(crate) read_requests: Vec<u64>,
    /// Their callbacks, until the clipboard answers.
    pub(in crate::runtime) clipboard_reads: Vec<(u64, crate::el::ReadFn)>,
    /// The plain text and pill ranges of this runtime's last copy that held pills, for web
    /// clipboard pastes where the OS worker does not own the pasteboard.
    pub(in crate::runtime) pill_copy: Option<(zeroize::Zeroizing<String>, Vec<(std::ops::Range<usize>, String)>)>,
    pub(in crate::runtime) next_clipboard_read: u64,
    pub(crate) clear_secret_request: bool,
    /// `Runtime::use_clipboard`: a test clipboard that replaces the OS one.
    clipboard: Option<crate::clipboard::MemoryClipboard>,
}

impl TextInputState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            selectable: Default::default(),
            sel_snapshot: Default::default(),
            sel_heard: Default::default(),
            links: Default::default(),
            input_click: Default::default(),
            input_scroll: HashMap::default(),
            caret: None,
            caret_watch: false,
            caret_follow: false,
            app_ime_comp: Default::default(),
            ime_option_as_alt: false,
            ime_reset: false,
            spell_watch: None,
            #[cfg(target_os = "linux")]
            primary: Default::default(),
            paste_request: None,
            read_requests: Vec::new(),
            clipboard_reads: Vec::new(),
            pill_copy: None,
            next_clipboard_read: 0,
            clear_secret_request: false,
            clipboard: None,
        }
    }

    #[inline]
    pub(in crate::runtime) fn app_ime_composing(&self) -> bool {
        self.app_ime_comp.composing()
    }

    #[inline]
    pub(in crate::runtime) fn step_app_ime(&mut self, ev: &Ime) {
        self.app_ime_comp.step(ev);
    }

    #[inline]
    pub(in crate::runtime) fn take_app_ime_composing(&mut self) -> Option<String> {
        self.app_ime_comp.take_composing()
    }

    #[inline]
    pub(crate) fn clipboard(&self) -> Option<&crate::clipboard::MemoryClipboard> {
        self.clipboard.as_ref()
    }

    #[inline]
    pub(crate) fn has_clipboard(&self) -> bool {
        self.clipboard.is_some()
    }

    #[inline]
    pub(crate) fn set_clipboard(&mut self, clipboard: crate::clipboard::MemoryClipboard) {
        self.clipboard = Some(clipboard);
    }

    #[inline]
    pub(in crate::runtime) fn single_line_scroll(&self, path: &str) -> f32 {
        self.input_scroll.get(path).copied().unwrap_or(0.0)
    }

    #[inline]
    pub(in crate::runtime) fn set_caret_if_moved(&mut self, next: Option<(&str, [f32; 4])>) -> bool {
        let same = match (&self.caret, next) {
            (Some((path, rect)), Some((next_path, r))) => path.as_str() == next_path && *rect == r,
            (None, None) => true,
            _ => false,
        };
        if same { return false; }
        self.caret = next.map(|(path, r)| (path.to_owned(), r));
        true
    }

    #[inline]
    pub(in crate::runtime) fn take_clipboard_read(&mut self, id: u64) -> Option<crate::el::ReadFn> {
        let i = self.clipboard_reads.iter().position(|(read, _)| *read == id)?;
        Some(self.clipboard_reads.remove(i).1)
    }

    #[inline]
    pub(in crate::runtime) fn has_pill_copy_matching(&self, text: &str, html: &str) -> bool {
        self.pill_copy.as_ref().is_some_and(|(t, pills)| {
            t.as_str() == text && {
                let frag = crate::TextEdit::from_html(html);
                frag.pills().count() == pills.len()
                    && frag.pills().zip(pills).all(|((r1, id1), (r2, id2))| r1 == *r2 && id1 == id2)
            }
        })
    }

    #[inline]
    pub(in crate::runtime) fn set_shift(&mut self, shift: bool) {
        self.selectable.shift = shift;
    }
}

impl RuntimeCore {
    /// Any vertical shift from ancestor growth rows whose presented height has not settled to layout.
    pub(in crate::runtime) fn stack_row_dy(&self, nodes: &[Node], i: usize) -> f32 {
        let mut dy = 0.0;
        let applied = std::ptr::eq(nodes, self.tree.nodes.as_slice());
        for r in std::iter::successors(Some(i), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none())) {
            let Some(c) = nodes[r].parent.filter(|&c| nodes[c].growth.is_some()) else { continue };
            let Some(stack) = self.motion.stacks.get(&nodes[c].id) else { continue };
            if applied && let Some(&top) = stack.tops.get(&nodes[r].id) {
                let mut count = 0usize;
                let before_r: f32 = stack.order.iter()
                    .take_while(|&&id| id != nodes[r].id)
                    .filter_map(|id| self.motion.grows.get(id))
                    .map(|g| { count += 1; g.h })
                    .sum::<f32>() + stack.gap * count as f32;
                dy += stack.pad_top + before_r - top;
            } else if stack.moving {
                for j in (c + 1..r).filter(|&j| nodes[j].parent == Some(c)) {
                    if let Some(g) = self.motion.grows.get(&nodes[j].id) {
                        dy += g.h - nodes[j].local.h;
                    }
                }
            }
        }
        dy
    }

    /// Node `i`'s caret line `[x, y, w, h]` in window points at its laid-out place, after
    /// scrolling and sticky shifts, if it is a focused text field.
    pub(in crate::runtime) fn caret_rect(&self, nodes: &[Node], i: usize) -> Option<[f32; 4]> {
        let n = &nodes[i];
        let (inp, spec, laid) = (n.input.as_ref()?, n.text.as_ref()?, n.text_layout.as_ref()?);
        if !inp.edit.focused { return None; }
        let layout = laid.view();
        let s = self.env.scale;
        let t = sticky::anchored_target(nodes, &self.scroll.scrolls, i, self.viewport);
        let y = t[1] + self.stack_row_dy(nodes, i);
        let tw = text_width(layout.full_width()) / s;
        let (x0, x1) = (t[0] + n.inset[0], t[2] - n.inset[2]);
        let mut x = text_x(spec, x0, x1, tw, layout.flow());
        if inp.single_line && let Some(l) = laid.single() {
            let prev = self.edit.single_line_scroll(&n.path);
            x -= single_line_offset(prev, inp, &spec.content, l, s, (x1 - x0) * s) / s;
        }
        let bb = layout.caret(caret_index(&inp.edit, &spec.content), inp.edit.affinity, 1.5 * s);
        let h = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 / s } else { spec.style.line_px() };
        Some([x + bb.x0 as f32 / s, y + n.inset[1] + bb.y0 as f32 / s, 1.5, h])
    }

    /// After an edit, scrolls the focused field's nearest scroll viewport on each axis just far
    /// enough to show the caret line, as a browser does for a textarea, leaving the viewport's
    /// `El::scroll_margin` like CSS `scroll-padding`.
    pub(in crate::runtime) fn follow_caret(&mut self, app: &mut dyn AppDriver) {
        let Some(i) = self.focused_index() else { return };
        let Some(r) = self.caret_rect(&self.tree.nodes, i) else { return };
        let mut scrolled = false;
        for horizontal in [false, true] {
            let Some(v) = std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent.filter(|_| self.tree.nodes[p].above.is_none()))
                .skip(1)
                .find(|&p| self.tree.nodes[p].scroll.is_some() && self.tree.nodes[p].scroll_axes.has(horizontal)) else { continue };
            let a = usize::from(!horizontal);
            let start = scroll_offsets(&self.scroll.scrolls, &self.tree.nodes, v).fold(self.tree.nodes[v].target[a], |p, o| p - o[a]);
            let view = f64::from(self.tree.nodes[v].target[a + 2] - self.tree.nodes[v].target[a]);
            let (at, len) = (f64::from(r[a] - start), f64::from(r[a + 2]));
            let margin = f64::from(self.tree.nodes[v].scroll_edges.margin);
            let delta = scroll_edge::reveal(0.0, at, len, view, margin);
            if delta == 0.0 { continue; }
            let stick = self.tree.nodes[v].scroll.is_some_and(|s| s.stick_bottom);
            let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[v].id) else { continue };
            if horizontal {
                st.x.offset = (st.x.offset + st.x.toward(-delta)).clamp(0.0, st.x.max);
                st.x.motion = XMotion::Rest;
            } else {
                st.offset = (st.offset + delta).clamp(0.0, st.max);
                st.land(stick);
                scrolled = true;
            }
            st.moved();
        }
        if scrolled && self.virtual_window_changed() {
            self.commit(app, &Tx::default());
            self.step_scroll(0.0);
        }
    }

    /// Refreshes the caret the next view sees from `nodes` (the current frame's when `None`).
    /// Returns whether it moved.
    pub(in crate::runtime) fn caret_moved(&mut self, nodes: Option<&[Node]>) -> bool {
        let nodes = nodes.unwrap_or(&self.tree.nodes);
        let next = nodes.iter().position(|n| n.input.as_ref().is_some_and(|inp| inp.edit.focused))
            .and_then(|i| Some((i, self.caret_rect(nodes, i)?)));
        let next = next.map(|(i, r)| (nodes[i].path.as_str(), r));
        self.edit.set_caret_if_moved(next)
    }
}

#[cfg(test)]
#[path = "caret_tests.rs"]
mod tests;
