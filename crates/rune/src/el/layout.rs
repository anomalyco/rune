//! Layout builders: size, padding, gaps, flex and grid placement, absolute pins, and the
//! pointer, cursor and hover handlers that sit with them.

use super::*;

impl El {
    pub fn w(mut self, w: f32) -> Self {
        self.style.size.width = length(w);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.style.size.height = length(h);
        self
    }
    pub fn size(self, w: f32, h: f32) -> Self {
        self.w(w).h(h)
    }
    /// Width over height, as CSS `aspect-ratio`: with one side set (or stretched by the parent)
    /// the other follows; a box with both sides set ignores it. A picture that sets only one
    /// side already takes its image's ratio ([`pixels`](Self::pixels)); use this for video or
    /// any other box. Anything not positive removes it.
    pub fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.style.aspect_ratio = (ratio.is_finite() && ratio > 0.0).then_some(ratio);
        self
    }
    /// The largest box at `ratio` (width over height) that fits the room the element is given,
    /// as SwiftUI's `.aspectRatio(ratio, contentMode: .fit)` or CSS `object-fit: contain` for
    /// the element itself. The room is the free space along the parent's main axis and its
    /// whole cross side, within `max_w` and `max_h`; siblings keep theirs, so a caption under
    /// a picture sits under the picture. The parent's alignment places the fitted box. Use it
    /// for a picture or video in a bounded cell: `el().aspect_fit(w / h).pixels_fit(p, Fill)`.
    /// In a column with no bounded height, bound it with `max_h`. It sets both sides, so `w`
    /// and `h` don't apply; anything not positive removes it.
    pub fn aspect_fit(mut self, ratio: f32) -> Self {
        self.aspect_fit = if ratio.is_finite() && ratio > 0.0 { ratio } else { 0.0 };
        self
    }
    pub fn w_full(mut self) -> Self {
        self.style.size.width = percent(1.0);
        self
    }
    /// Width as a fraction of the parent's (CSS `width: 40%` is `.w_frac(0.4)`).
    pub fn w_frac(mut self, f: f32) -> Self {
        self.style.size.width = percent(f);
        self
    }
    pub fn h_full(mut self) -> Self {
        self.style.size.height = percent(1.0);
        self
    }
    /// Take a `g` share of the free space along the parent's main axis (CSS `flex: g 1 0`).
    ///
    /// This also sets flex-basis to 0 and allows shrinking, so the element's own content does
    /// not size it: it is only as big as the space the parent has left over. Inside a parent
    /// with no bounded height (an auto-height column) there is no free space, and a grown
    /// `.scroll(..)` viewport collapses to 0 pt. Bound an ancestor's height (`.h(..)`,
    /// `.h_full()`, or a grown child of a bounded parent), or use `.min_h(..)` for a floor.
    /// A grown column that holds its own flex content also needs `.min_h(0.0)` (see
    /// [`El::min_h`]).
    pub fn grow(mut self, g: f32) -> Self {
        self.style.flex_grow = g;
        self.style.flex_shrink = 1.0;
        self.style.flex_basis = length(0.0);
        self
    }
    pub fn padding(mut self, p: f32) -> Self {
        self.style.padding = Rect { left: length(p), right: length(p), top: length(p), bottom: length(p) };
        self
    }
    pub fn px(mut self, p: f32) -> Self {
        self.style.padding.left = length(p);
        self.style.padding.right = length(p);
        self
    }
    pub fn py(mut self, p: f32) -> Self {
        self.style.padding.top = length(p);
        self.style.padding.bottom = length(p);
        self
    }
    pub fn pt(mut self, p: f32) -> Self {
        self.style.padding.top = length(p);
        self
    }
    pub fn pb(mut self, p: f32) -> Self {
        self.style.padding.bottom = length(p);
        self
    }
    pub fn pl(mut self, p: f32) -> Self {
        self.style.padding.left = length(p);
        self
    }
    pub fn ml(mut self, m: f32) -> Self {
        self.style.margin.left = length(m);
        self
    }
    pub fn mr(mut self, m: f32) -> Self {
        self.style.margin.right = length(m);
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.style.gap = Size { width: length(g), height: length(g) };
        self
    }
    /// Centre children on the cross axis.
    pub fn center_y(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self
    }
    pub fn center(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self.style.justify_content = JustifyContent::CENTER;
        self
    }
    pub fn justify_between(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_BETWEEN;
        self
    }
    pub fn justify_end(mut self) -> Self {
        self.style.justify_content = JustifyContent::FLEX_END;
        self
    }
    /// Wrap children onto new lines (CSS `flex-wrap: wrap`).
    pub fn flex_wrap(mut self) -> Self {
        self.style.flex_wrap = FlexWrap::Wrap;
        self
    }
    /// Align children to the start of the cross axis (CSS `align-items: flex-start`).
    pub fn items_start(mut self) -> Self {
        self.style.align_items = AlignItems::FLEX_START;
        self
    }
    /// Align children to the end of the cross axis (CSS `align-items: flex-end`).
    pub fn items_end(mut self) -> Self {
        self.style.align_items = AlignItems::FLEX_END;
        self
    }
    /// Centre children on the cross axis (CSS `align-items: center`; same as `center_y`).
    pub fn items_center(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self
    }
    /// Stretch children across the cross axis (CSS `align-items: stretch`).
    pub fn items_stretch(mut self) -> Self {
        self.style.align_items = AlignItems::STRETCH;
        self
    }
    /// This element's own cross-axis alignment (CSS `align-self`).
    pub fn self_start(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::FLEX_START);
        self
    }
    pub fn self_center(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::CENTER);
        self
    }
    pub fn self_end(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::FLEX_END);
        self
    }
    pub fn self_stretch(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::STRETCH);
        self
    }
    /// Pack children at the start of the main axis (CSS `justify-content: flex-start`).
    pub fn justify_start(mut self) -> Self {
        self.style.justify_content = JustifyContent::FLEX_START;
        self
    }
    /// Centre children on the main axis (CSS `justify-content: center`).
    pub fn justify_center(mut self) -> Self {
        self.style.justify_content = JustifyContent::CENTER;
        self
    }
    /// CSS `justify-content: space-around`.
    pub fn justify_around(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_AROUND;
        self
    }
    /// CSS `justify-content: space-evenly`.
    pub fn justify_evenly(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_EVENLY;
        self
    }
    pub fn min_w(mut self, w: f32) -> Self {
        self.style.min_size.width = length(w);
        self
    }
    pub fn max_h(mut self, h: f32) -> Self {
        self.style.max_size.height = length(h);
        self
    }
    /// Separate row and column gaps (CSS `row-gap`, `column-gap`).
    pub fn gaps(mut self, row: f32, column: f32) -> Self {
        self.style.gap = Size { width: length(column), height: length(row) };
        self
    }
    pub fn max_w(mut self, w: f32) -> Self {
        self.style.max_size.width = length(w);
        self
    }
    /// Minimum height in points.
    ///
    /// Flex items default to a minimum of their content height (CSS `min-height: auto`), so a
    /// nested flex column with tall content overflows its parent instead of shrinking, and a
    /// scroll viewport inside it never gets to scroll. Put `.min_h(0.0)` on each flex column
    /// between the bounded ancestor and the viewport:
    ///
    /// ```ignore
    /// col().h_full().child(header).child(
    ///     col().grow(1.0).min_h(0.0).child(col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).child(list)),
    /// )
    /// ```
    pub fn min_h(mut self, h: f32) -> Self {
        self.style.min_size.height = length(h);
        self
    }
    /// Left and right margin together (CSS `margin-inline`).
    pub fn mx(mut self, m: f32) -> Self {
        self.style.margin.left = length(m);
        self.style.margin.right = length(m);
        self
    }
    /// Top and bottom margin together (CSS `margin-block`).
    pub fn my(mut self, m: f32) -> Self {
        self.style.margin.top = length(m);
        self.style.margin.bottom = length(m);
        self
    }
    pub fn mt(mut self, m: f32) -> Self {
        self.style.margin.top = length(m);
        self
    }
    pub fn mb(mut self, m: f32) -> Self {
        self.style.margin.bottom = length(m);
        self
    }
    pub fn pr(mut self, p: f32) -> Self {
        self.style.padding.right = length(p);
        self
    }
    /// Allow shrinking below content size along the parent's main axis.
    pub fn shrink(mut self) -> Self {
        self.style.flex_shrink = 1.0;
        self.style.min_size.width = length(0.0);
        self
    }
    /// Lay children out in `n` equal columns, each as wide as the widest child needs (CSS
    /// `grid-template-columns: repeat(n, 1fr)`), or sharing the width the element is given.
    /// Children fill the cells in order unless placed with `grid_cell`.
    pub fn equal_columns(mut self, n: usize) -> Self {
        self.style.display = Display::Grid;
        self.style.grid_template_columns = vec![fr(1.0); n.max(1)];
        self
    }
    /// Place this child of an `equal_columns` element in `column` (0-based) of the first row.
    /// Children in the same cell overlap in order, so a moving highlight can sit under a label.
    pub fn grid_cell(mut self, column: usize) -> Self {
        self.style.grid_column = Line { start: line(column as i16 + 1), end: span(1) };
        self.style.grid_row = Line { start: line(1), end: span(1) };
        self
    }
    /// Out of flow, positioned at `left, top` inside the parent's padding box.
    pub fn absolute(mut self, left: f32, top: f32) -> Self {
        self.style.position = Position::Absolute;
        self.style.inset = Rect { left: length(left), top: length(top), right: auto(), bottom: auto() };
        self
    }
    /// Out of flow, pinned to the given edges of the parent's padding box (`None` leaves an
    /// edge free); pinning both opposite edges stretches the element between them.
    pub fn pin(mut self, left: Option<f32>, top: Option<f32>, right: Option<f32>, bottom: Option<f32>) -> Self {
        let edge = |v: Option<f32>| v.map_or(auto(), length);
        self.style.position = Position::Absolute;
        self.style.inset = Rect { left: edge(left), top: edge(top), right: edge(right), bottom: edge(bottom) };
        self
    }
    /// The pointer's shape while it is over this element or a descendant that sets none.
    /// Applies while the pointer is over a hit target (a pressable, draggable, hoverable or
    /// text element) inside this one.
    pub fn cursor(mut self, cursor: Cursor) -> Self {
        self.handlers.cursor = Some(cursor);
        self
    }
    /// Pressing this element moves the window, as a title bar does, and a double press zooms or
    /// minimises it as the system's titlebar setting says. Hit targets inside it (buttons,
    /// fields) keep their presses.
    pub fn window_drag(mut self) -> Self {
        self.window_drag = true;
        self
    }
    /// The caret of an editor the app draws itself rather than with `text_input`: while this
    /// element is drawn and no text field has focus, the window turns its input method on (dead
    /// keys, CJK, the emoji picker, dictation), places the candidate window at this element's
    /// drawn rect, and hands what it composes to [`App::on_ime`](crate::App::on_ime). Put it on
    /// the caret you draw, only while the editor has the keyboard. A focused field's caret wins,
    /// and a focused secure field keeps the input method off.
    pub fn ime_cursor(mut self) -> Self {
        self.ime_cursor = true;
        self
    }
    /// Mark the drawn, ancestor-clipped rectangle as native overlay input. Does not add a handler.
    pub fn hit(mut self) -> Self {
        self.vis.hit = true;
        self
    }

    /// Observe the pointer moving over this element, in window points, without making a native
    /// overlay input region. [`El::on_pointer_move_local`] gives the element's own points.
    pub fn on_pointer_move<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress) + 'static) -> Self {
        self.handlers.on_pointer_move = Some(Rc::new(move |s, p, _| f(downcast::<S>(s), p.window())));
        self
    }
    /// [`El::on_pointer_move`] with the point in this element's own layout points as well as
    /// the window's (see [`PointerEvent`]), and a [`Tx`]: a canvas tracks the pointer in its own
    /// coordinates at any zoom, and a handler that only moves a [`Live`](crate::Live) value
    /// (a crosshair) says so with [`Tx::live_only`].
    ///
    /// ```ignore
    /// .on_pointer_move_local(|s: &mut Canvas, p, tx| {
    ///     s.crosshair.set([p.x, p.y]);
    ///     tx.live_only();
    /// })
    /// ```
    pub fn on_pointer_move_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.on_pointer_move = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }

    /// Called once when the pointer enters this hit target, not on animation frames.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.handlers.on_hover = Some(Rc::new(move |s, _| f(downcast::<S>(s))));
        self
    }

    /// Called with `true` when the pointer enters this element's visible rect and `false` when
    /// it leaves, descendants included (moving onto a child button is not a leave). It is not a
    /// hit target, so it never takes presses or hover from its children. A held press keeps the
    /// pointer inside until it releases.
    pub fn on_hover_change<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_hover_change = Some(Rc::new(move |s, inside, tx| f(downcast::<S>(s), inside, tx)));
        self
    }

    /// Delay [`Self::on_hover_change`]: `true` arrives once the pointer has rested inside for
    /// `enter`, and `false` once it has stayed out for `leave`. A pass shorter than the delay
    /// reports nothing, and a held button never starts the wait (a press during it cancels it;
    /// the wait starts again when the pointer next moves). Losing window focus ends a shown hover
    /// and cancels a wait; the pointer's next move over the window, still unfocused, starts the
    /// wait again, so tooltips and hover cards open in a window behind another. A sidebar that
    /// peeks open when the pointer rests at the window's edge uses `hover_delays(40 ms, 0)`.
    pub fn hover_delays(mut self, enter: std::time::Duration, leave: std::time::Duration) -> Self {
        let d = [enter.as_secs_f32(), leave.as_secs_f32()];
        if d != [0.0; 2] || !self.handlers.is_empty() {
            self.handlers.hover_delay = d;
        }
        self
    }
}
