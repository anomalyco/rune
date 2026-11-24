//! Layout builders: size, padding, gaps, flex and grid placement, absolute pins, and the
//! pointer, cursor and hover handlers that sit with them.

use super::*;

impl El {
    #[inline]
    pub fn w(mut self, w: f32) -> Self {
        self.set_w(w);
        self
    }
    /// [`El::w`] in place.
    #[inline]
    pub fn set_w(&mut self, w: f32) -> &mut Self {
        self.style.set_width(length(w));
        self
    }
    #[inline]
    pub fn h(mut self, h: f32) -> Self {
        self.set_h(h);
        self
    }
    /// [`El::h`] in place.
    #[inline]
    pub fn set_h(&mut self, h: f32) -> &mut Self {
        self.style.set_height(length(h));
        self
    }
    #[inline]
    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.set_size(w, h);
        self
    }
    /// [`El::size`] in place.
    #[inline]
    pub fn set_size(&mut self, w: f32, h: f32) -> &mut Self {
        self.set_w(w).set_h(h)
    }
    /// Width over height, as CSS `aspect-ratio`: with one side set (or stretched by the parent)
    /// the other follows; a box with both sides set ignores it. A picture that sets only one
    /// side already takes its image's ratio ([`pixels`](Self::pixels)); use this for video or
    /// any other box. Anything not positive removes it.
    #[inline]
    pub fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.set_aspect_ratio(ratio);
        self
    }
    /// [`El::aspect_ratio`] in place.
    #[inline]
    pub fn set_aspect_ratio(&mut self, ratio: f32) -> &mut Self {
        self.style.set_aspect_ratio((ratio.is_finite() && ratio > 0.0).then_some(ratio));
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
        let ratio = if ratio.is_finite() && ratio > 0.0 { ratio } else { 0.0 };
        if ratio > 0.0 || !self.cold.is_empty() {
            self.cold_mut().aspect_fit = ratio;
        }
        self
    }
    #[inline]
    pub fn w_full(mut self) -> Self {
        self.set_w_full();
        self
    }
    /// [`El::w_full`] in place.
    #[inline]
    pub fn set_w_full(&mut self) -> &mut Self {
        self.style.set_width(percent(1.0));
        self
    }
    /// Width as a fraction of the parent's (CSS `width: 40%` is `.w_frac(0.4)`).
    #[inline]
    pub fn w_frac(mut self, f: f32) -> Self {
        self.set_w_frac(f);
        self
    }
    /// [`El::w_frac`] in place.
    #[inline]
    pub fn set_w_frac(&mut self, f: f32) -> &mut Self {
        self.style.set_width(percent(f));
        self
    }
    /// Fit width to content without stretching across a column (CSS `width: fit-content`).
    ///
    /// Keeps `align-self` at `auto`, so a control hugs its content inside a stretching `col()`
    /// while still deferring to a parent's cross-axis alignment (such as `col().items_center()`
    /// or `row().center_y()`).
    ///
    /// ```ignore
    /// row().w_fit().px(12.0).child(text("Badge"))
    /// ```
    #[inline]
    pub fn w_fit(mut self) -> Self {
        self.set_w_fit();
        self
    }
    /// [`El::w_fit`] in place.
    ///
    /// ```ignore
    /// let mut e = row();
    /// e.set_w_fit();
    /// ```
    #[inline]
    pub fn set_w_fit(&mut self) -> &mut Self {
        self.style.set_width(Dimension::fit_content());
        self
    }
    #[inline]
    pub fn h_full(mut self) -> Self {
        self.set_h_full();
        self
    }
    /// [`El::h_full`] in place.
    #[inline]
    pub fn set_h_full(&mut self) -> &mut Self {
        self.style.set_height(percent(1.0));
        self
    }
    /// Height as a fraction of the parent's (CSS `height: 40%` is `.h_frac(0.4)`).
    ///
    /// ```ignore
    /// col().h(200.0).child(el().h_frac(0.4))
    /// ```
    #[inline]
    pub fn h_frac(mut self, f: f32) -> Self {
        self.set_h_frac(f);
        self
    }
    /// [`El::h_frac`] in place.
    ///
    /// ```ignore
    /// let mut e = el();
    /// e.set_h_frac(0.4);
    /// ```
    #[inline]
    pub fn set_h_frac(&mut self, f: f32) -> &mut Self {
        self.style.set_height(percent(f));
        self
    }
    /// Fit height to content without stretching across a row (CSS `height: fit-content`).
    ///
    /// Keeps `align-self` at `auto`, so a child hugs its content height inside a stretching
    /// `row()` while still deferring to a parent's cross-axis alignment (such as `row().center_y()`).
    ///
    /// ```ignore
    /// col().h_fit().py(6.0).child(text("Badge"))
    /// ```
    #[inline]
    pub fn h_fit(mut self) -> Self {
        self.set_h_fit();
        self
    }
    /// [`El::h_fit`] in place.
    ///
    /// ```ignore
    /// let mut e = col();
    /// e.set_h_fit();
    /// ```
    #[inline]
    pub fn set_h_fit(&mut self) -> &mut Self {
        self.style.set_height(Dimension::fit_content());
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
    #[inline]
    pub fn grow(mut self, g: f32) -> Self {
        self.set_grow(g);
        self
    }
    /// [`El::grow`] in place.
    #[inline]
    pub fn set_grow(&mut self, g: f32) -> &mut Self {
        self.style.set_grow(g);
        self
    }
    #[inline]
    pub fn padding(mut self, p: f32) -> Self {
        self.set_padding(p);
        self
    }
    /// [`El::padding`] in place.
    #[inline]
    pub fn set_padding(&mut self, p: f32) -> &mut Self {
        self.style.set_padding_all(p);
        self
    }
    #[inline]
    pub fn px(mut self, p: f32) -> Self {
        self.set_px(p);
        self
    }
    /// [`El::px`] in place.
    #[inline]
    pub fn set_px(&mut self, p: f32) -> &mut Self {
        self.style.set_pad_left(p);
        self.style.set_pad_right(p);
        self
    }
    #[inline]
    pub fn py(mut self, p: f32) -> Self {
        self.set_py(p);
        self
    }
    /// [`El::py`] in place.
    #[inline]
    pub fn set_py(&mut self, p: f32) -> &mut Self {
        self.style.set_pad_top(p);
        self.style.set_pad_bottom(p);
        self
    }
    #[inline]
    pub fn pt(mut self, p: f32) -> Self {
        self.set_pt(p);
        self
    }
    /// [`El::pt`] in place.
    #[inline]
    pub fn set_pt(&mut self, p: f32) -> &mut Self {
        self.style.set_pad_top(p);
        self
    }
    #[inline]
    pub fn pb(mut self, p: f32) -> Self {
        self.set_pb(p);
        self
    }
    /// [`El::pb`] in place.
    #[inline]
    pub fn set_pb(&mut self, p: f32) -> &mut Self {
        self.style.set_pad_bottom(p);
        self
    }
    #[inline]
    pub fn pl(mut self, p: f32) -> Self {
        self.set_pl(p);
        self
    }
    /// [`El::pl`] in place.
    #[inline]
    pub fn set_pl(&mut self, p: f32) -> &mut Self {
        self.style.set_pad_left(p);
        self
    }
    #[inline]
    pub fn ml(mut self, m: f32) -> Self {
        self.set_ml(m);
        self
    }
    /// [`El::ml`] in place.
    #[inline]
    pub fn set_ml(&mut self, m: f32) -> &mut Self {
        self.style.set_margin_left(m);
        self
    }
    #[inline]
    pub fn mr(mut self, m: f32) -> Self {
        self.set_mr(m);
        self
    }
    /// [`El::mr`] in place.
    #[inline]
    pub fn set_mr(&mut self, m: f32) -> &mut Self {
        self.style.set_margin_right(m);
        self
    }
    #[inline]
    pub fn gap(mut self, g: f32) -> Self {
        self.set_gap(g);
        self
    }
    /// [`El::gap`] in place.
    #[inline]
    pub fn set_gap(&mut self, g: f32) -> &mut Self {
        self.style.set_gap(g, g);
        self
    }
    /// Centre children on the cross axis.
    #[inline]
    pub fn center_y(mut self) -> Self {
        self.set_center_y();
        self
    }
    /// [`El::center_y`] in place.
    #[inline]
    pub fn set_center_y(&mut self) -> &mut Self {
        self.style.set_align_items(AlignItems::CENTER);
        self
    }
    #[inline]
    pub fn center(mut self) -> Self {
        self.set_center();
        self
    }
    /// [`El::center`] in place.
    #[inline]
    pub fn set_center(&mut self) -> &mut Self {
        self.style.set_align_items(AlignItems::CENTER);
        self.style.set_justify_content(JustifyContent::CENTER);
        self
    }
    #[inline]
    pub fn justify_between(mut self) -> Self {
        self.set_justify_between();
        self
    }
    /// [`El::justify_between`] in place.
    #[inline]
    pub fn set_justify_between(&mut self) -> &mut Self {
        self.style.set_justify_content(JustifyContent::SPACE_BETWEEN);
        self
    }
    #[inline]
    pub fn justify_end(mut self) -> Self {
        self.set_justify_end();
        self
    }
    /// [`El::justify_end`] in place.
    #[inline]
    pub fn set_justify_end(&mut self) -> &mut Self {
        self.style.set_justify_content(JustifyContent::FLEX_END);
        self
    }
    /// Wrap children onto new lines (CSS `flex-wrap: wrap`).
    #[inline]
    pub fn flex_wrap(mut self) -> Self {
        self.set_flex_wrap();
        self
    }
    /// [`El::flex_wrap`] in place.
    #[inline]
    pub fn set_flex_wrap(&mut self) -> &mut Self {
        self.style.set_flex_wrap(FlexWrap::Wrap);
        self
    }
    /// Align children to the start of the cross axis (CSS `align-items: flex-start`).
    #[inline]
    pub fn items_start(mut self) -> Self {
        self.set_items_start();
        self
    }
    /// [`El::items_start`] in place.
    #[inline]
    pub fn set_items_start(&mut self) -> &mut Self {
        self.style.set_align_items(AlignItems::FLEX_START);
        self
    }
    /// Align children to the end of the cross axis (CSS `align-items: flex-end`).
    #[inline]
    pub fn items_end(mut self) -> Self {
        self.set_items_end();
        self
    }
    /// [`El::items_end`] in place.
    #[inline]
    pub fn set_items_end(&mut self) -> &mut Self {
        self.style.set_align_items(AlignItems::FLEX_END);
        self
    }
    /// Centre children on the cross axis (CSS `align-items: center`; same as `center_y`).
    #[inline]
    pub fn items_center(mut self) -> Self {
        self.set_items_center();
        self
    }
    /// [`El::items_center`] in place.
    #[inline]
    pub fn set_items_center(&mut self) -> &mut Self {
        self.style.set_align_items(AlignItems::CENTER);
        self
    }
    /// Stretch children across the cross axis (CSS `align-items: stretch`).
    #[inline]
    pub fn items_stretch(mut self) -> Self {
        self.set_items_stretch();
        self
    }
    /// [`El::items_stretch`] in place.
    #[inline]
    pub fn set_items_stretch(&mut self) -> &mut Self {
        self.style.set_align_items(AlignItems::STRETCH);
        self
    }
    /// This element's own cross-axis alignment (CSS `align-self`).
    #[inline]
    pub fn self_start(mut self) -> Self {
        self.set_self_start();
        self
    }
    /// [`El::self_start`] in place.
    #[inline]
    pub fn set_self_start(&mut self) -> &mut Self {
        self.style.set_align_self(Some(AlignSelf::FLEX_START));
        self
    }
    #[inline]
    pub fn self_center(mut self) -> Self {
        self.set_self_center();
        self
    }
    /// [`El::self_center`] in place.
    #[inline]
    pub fn set_self_center(&mut self) -> &mut Self {
        self.style.set_align_self(Some(AlignSelf::CENTER));
        self
    }
    #[inline]
    pub fn self_end(mut self) -> Self {
        self.set_self_end();
        self
    }
    /// [`El::self_end`] in place.
    #[inline]
    pub fn set_self_end(&mut self) -> &mut Self {
        self.style.set_align_self(Some(AlignSelf::FLEX_END));
        self
    }
    #[inline]
    pub fn self_stretch(mut self) -> Self {
        self.set_self_stretch();
        self
    }
    /// [`El::self_stretch`] in place.
    #[inline]
    pub fn set_self_stretch(&mut self) -> &mut Self {
        self.style.set_align_self(Some(AlignSelf::STRETCH));
        self
    }
    /// Reset this element's cross-axis alignment to defer to the parent's `align-items`
    /// (CSS `align-self: auto`).
    ///
    /// ```ignore
    /// control.self_start().self_auto()
    /// ```
    #[inline]
    pub fn self_auto(mut self) -> Self {
        self.set_self_auto();
        self
    }
    /// [`El::self_auto`] in place.
    ///
    /// ```ignore
    /// control.set_self_auto();
    /// ```
    #[inline]
    pub fn set_self_auto(&mut self) -> &mut Self {
        self.style.set_align_self(None);
        self
    }
    /// Pack children at the start of the main axis (CSS `justify-content: flex-start`).
    #[inline]
    pub fn justify_start(mut self) -> Self {
        self.set_justify_start();
        self
    }
    /// [`El::justify_start`] in place.
    #[inline]
    pub fn set_justify_start(&mut self) -> &mut Self {
        self.style.set_justify_content(JustifyContent::FLEX_START);
        self
    }
    /// Centre children on the main axis (CSS `justify-content: center`).
    #[inline]
    pub fn justify_center(mut self) -> Self {
        self.set_justify_center();
        self
    }
    /// [`El::justify_center`] in place.
    #[inline]
    pub fn set_justify_center(&mut self) -> &mut Self {
        self.style.set_justify_content(JustifyContent::CENTER);
        self
    }
    /// CSS `justify-content: space-around`.
    pub fn justify_around(mut self) -> Self {
        self.style.set_justify_content(JustifyContent::SPACE_AROUND);
        self
    }
    /// CSS `justify-content: space-evenly`.
    pub fn justify_evenly(mut self) -> Self {
        self.style.set_justify_content(JustifyContent::SPACE_EVENLY);
        self
    }
    #[inline]
    pub fn min_w(mut self, w: f32) -> Self {
        self.set_min_w(w);
        self
    }
    /// [`El::min_w`] in place.
    #[inline]
    pub fn set_min_w(&mut self, w: f32) -> &mut Self {
        self.style.set_min_width(length(w));
        self
    }
    #[inline]
    pub fn max_h(mut self, h: f32) -> Self {
        self.set_max_h(h);
        self
    }
    /// [`El::max_h`] in place.
    #[inline]
    pub fn set_max_h(&mut self, h: f32) -> &mut Self {
        self.style.set_max_height(length(h));
        self
    }
    /// Separate row and column gaps (CSS `row-gap`, `column-gap`).
    #[inline]
    pub fn gaps(mut self, row: f32, column: f32) -> Self {
        self.set_gaps(row, column);
        self
    }
    /// [`El::gaps`] in place.
    #[inline]
    pub fn set_gaps(&mut self, row: f32, column: f32) -> &mut Self {
        self.style.set_gap(column, row);
        self
    }
    #[inline]
    pub fn max_w(mut self, w: f32) -> Self {
        self.set_max_w(w);
        self
    }
    /// [`El::max_w`] in place.
    #[inline]
    pub fn set_max_w(&mut self, w: f32) -> &mut Self {
        self.style.set_max_width(length(w));
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
    #[inline]
    pub fn min_h(mut self, h: f32) -> Self {
        self.set_min_h(h);
        self
    }
    /// [`El::min_h`] in place.
    #[inline]
    pub fn set_min_h(&mut self, h: f32) -> &mut Self {
        self.style.set_min_height(length(h));
        self
    }
    /// Left and right margin together (CSS `margin-inline`).
    #[inline]
    pub fn mx(mut self, m: f32) -> Self {
        self.set_mx(m);
        self
    }
    /// [`El::mx`] in place.
    #[inline]
    pub fn set_mx(&mut self, m: f32) -> &mut Self {
        self.style.set_margin_x(m);
        self
    }
    /// Top and bottom margin together (CSS `margin-block`).
    #[inline]
    pub fn my(mut self, m: f32) -> Self {
        self.set_my(m);
        self
    }
    /// [`El::my`] in place.
    #[inline]
    pub fn set_my(&mut self, m: f32) -> &mut Self {
        self.style.set_margin_y(m);
        self
    }
    #[inline]
    pub fn mt(mut self, m: f32) -> Self {
        self.set_mt(m);
        self
    }
    /// [`El::mt`] in place.
    #[inline]
    pub fn set_mt(&mut self, m: f32) -> &mut Self {
        self.style.set_margin_top(m);
        self
    }
    #[inline]
    pub fn mb(mut self, m: f32) -> Self {
        self.set_mb(m);
        self
    }
    /// [`El::mb`] in place.
    #[inline]
    pub fn set_mb(&mut self, m: f32) -> &mut Self {
        self.style.set_margin_bottom(m);
        self
    }
    #[inline]
    pub fn pr(mut self, p: f32) -> Self {
        self.set_pr(p);
        self
    }
    /// [`El::pr`] in place.
    #[inline]
    pub fn set_pr(&mut self, p: f32) -> &mut Self {
        self.style.set_pad_right(p);
        self
    }
    /// Allow shrinking below content size along the parent's main axis.
    #[inline]
    pub fn shrink(mut self) -> Self {
        self.set_shrink();
        self
    }
    /// [`El::shrink`] in place.
    #[inline]
    pub fn set_shrink(&mut self) -> &mut Self {
        self.style.set_flex_shrink(1.0);
        self.style.set_min_width(length(0.0));
        self
    }
    /// Lay children out in `n` equal columns, each as wide as the widest child needs (CSS
    /// `grid-template-columns: repeat(n, 1fr)`), or sharing the width the element is given.
    /// Children fill the cells in order unless placed with `grid_cell`.
    pub fn equal_columns(mut self, n: usize) -> Self {
        self.style.set_equal_columns(n);
        self
    }
    /// Place this child of an `equal_columns` element in `column` (0-based) of the first row.
    /// Children in the same cell overlap in order, so a moving highlight can sit under a label.
    pub fn grid_cell(mut self, column: usize) -> Self {
        self.style.set_grid_cell(column);
        self
    }
    /// Out of flow, positioned at `left, top` inside the parent's padding box.
    #[inline]
    pub fn absolute(mut self, left: f32, top: f32) -> Self {
        self.set_absolute(left, top);
        self
    }
    /// [`El::absolute`] in place.
    #[inline]
    pub fn set_absolute(&mut self, left: f32, top: f32) -> &mut Self {
        self.style.set_absolute_inset(Rect { left: length(left), top: length(top), right: auto(), bottom: auto() });
        self
    }
    /// Out of flow, pinned to the given edges of the parent's padding box (`None` leaves an
    /// edge free); pinning both opposite edges stretches the element between them.
    #[inline]
    pub fn pin(mut self, left: Option<f32>, top: Option<f32>, right: Option<f32>, bottom: Option<f32>) -> Self {
        self.set_pin(left, top, right, bottom);
        self
    }
    /// [`El::pin`] in place.
    #[inline]
    pub fn set_pin(&mut self, left: Option<f32>, top: Option<f32>, right: Option<f32>, bottom: Option<f32>) -> &mut Self {
        let edge = |v: Option<f32>| v.map_or(auto(), length);
        self.style.set_absolute_inset(Rect { left: edge(left), top: edge(top), right: edge(right), bottom: edge(bottom) });
        self
    }
    /// The pointer's shape while it is over this element or a descendant that sets none.
    /// Applies while the pointer is over a hit target (a pressable, draggable, hoverable or
    /// text element) inside this one.
    pub fn cursor(mut self, cursor: Cursor) -> Self {
        self.handlers.cursor = Some(cursor);
        self
    }
    /// Pressing this element moves the window (or a window-backed overlay; pair with `.hit()`
    /// under `OverlayInput::Regions`), and on ordinary non-panel windows a double press zooms or
    /// minimises it as the system's titlebar setting says (overlays and macOS non-activating
    /// panels ignore the double press). Hit targets inside it keep their presses.
    pub fn window_drag(mut self) -> Self {
        self.flags.set_window_drag(WindowDrag::Drag);
        self
    }
    /// A part of a `.window_drag()` band that keeps its presses: a press, double press or
    /// secondary click on it, or on a bare child of it, neither moves, zooms nor opens the menu
    /// of the window. Controls (`on_press`, `on_drag`, focusable elements, text fields) already
    /// keep theirs; this is for the parts between them, such as the gaps of a toolbar group.
    /// Like any hit target, it takes presses from what lies under it.
    pub fn no_window_drag(mut self) -> Self {
        self.flags.set_window_drag(WindowDrag::NoDrag);
        self
    }
    /// Pressing this element starts an interactive OS/compositor-driven window resize from
    /// `edge` (Win32 sizing borders, X11 `_NET_WM_MOVERESIZE`, Wayland `xdg_toplevel.resize`),
    /// for borderless windows that draw their own frame.
    ///
    /// While hovered, the element shows the matching resize cursor (`Cursor::ResizeNs`,
    /// `Cursor::ResizeEw`, `Cursor::ResizeNwse`, or `Cursor::ResizeNesw`) unless an explicit
    /// [`El::cursor`] is set on it. Interactive children, [`El::no_window_drag`], inert
    /// subtrees, and active modals block the resize just as they block [`El::window_drag`].
    /// Ignored on macOS, on the web, in overlays, and in headless runs (custom hosts and
    /// headless tests observe the edge via
    /// [`Runtime::take_resize_action`](crate::Runtime::take_resize_action)).
    ///
    /// ```
    /// # use rune::{ResizeEdge, el};
    /// # let _ =
    /// el().pin(None, None, Some(0.0), Some(0.0))
    ///     .size(12.0, 12.0)
    ///     .window_resize(ResizeEdge::SouthEast)
    /// # ;
    /// ```
    pub fn window_resize(mut self, edge: crate::ResizeEdge) -> Self {
        self.flags.set_window_drag(WindowDrag::Resize(edge));
        self.cold_mut().window_resize = Some(edge);
        self
    }
    /// The caret of an editor the app draws itself rather than with `text_input`: while this
    /// element is drawn and no text field has focus, the window turns its input method on (dead
    /// keys, CJK, the emoji picker, dictation), places the candidate window at this element's
    /// drawn rect, and hands what it composes to [`App::on_ime`](crate::App::on_ime). Put it on
    /// the caret you draw, only while the editor has the keyboard. A focused field's caret wins,
    /// and a focused secure field keeps the input method off.
    pub fn ime_cursor(mut self) -> Self {
        self.flags.set(ElFlags::IME_CURSOR, true);
        self
    }
    /// With [`Self::ime_cursor`]: on macOS, while this caret has the input method, Option is the
    /// Alt key. A key pressed with it arrives as the key it's on with `mods.alt` (`Tx::key_base`
    /// its character) instead of the character the layout makes with Option, and the input method
    /// composes nothing for it (winit's `OptionAsAlt::Both`): a terminal's Option-as-Meta. It
    /// changes nothing elsewhere, nor while `on` is false.
    ///
    /// To treat only the left or only the right Option key as Alt while leaving the other free for
    /// layout characters, use [`Self::ime_option_as_alt_mode`].
    pub fn ime_option_as_alt(self, on: bool) -> Self {
        self.ime_option_as_alt_mode(crate::OptionAsAlt::from(on))
    }
    /// With [`Self::ime_cursor`]: on macOS, choose which Option key acts as Alt while this caret
    /// has the input method (winit's `OptionAsAlt`: `None`, `OnlyLeft`, `OnlyRight`, `Both`).
    ///
    /// Any key pressed with a selected Option key arrives as the key it is on with `mods.alt`
    /// (`Tx::key_base` its character) instead of the character the keyboard layout composes, and
    /// the input method composes nothing for it. An unselected Option key keeps composing the
    /// layout's characters (for example, right Option on a German layout still types `[` and `@`
    /// while [`OptionAsAlt::Left`](crate::OptionAsAlt::Left) is active). Ignored off macOS,
    /// ignored while `mode` is [`OptionAsAlt::None`](crate::OptionAsAlt::None), and reset to
    /// `None` whenever a text field or another caret takes the input method.
    ///
    /// ```
    /// use rune::{OptionAsAlt, el};
    ///
    /// let caret = el().ime_cursor().ime_option_as_alt_mode(OptionAsAlt::Left);
    /// # let _ = caret;
    /// ```
    pub fn ime_option_as_alt_mode(mut self, mode: crate::OptionAsAlt) -> Self {
        if mode != crate::OptionAsAlt::None || !self.cold.is_empty() {
            self.cold_mut().option_as_alt = mode;
        }
        self
    }
    /// Mark the drawn, ancestor-clipped rectangle as native overlay input. Does not add a handler.
    pub fn hit(mut self) -> Self {
        self.vis_flags.set(VisualFlags::HIT, true);
        self
    }

    /// Observe the pointer moving over this element, in window points, without making a native
    /// overlay input region. [`El::on_pointer_move_local`] gives the element's own points.
    #[track_caller]
    pub fn on_pointer_move<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress) + 'static) -> Self {
        self.handlers.on_pointer_move = Some(Rc::new(move |s, p, _| f(downcast::<S>(s), p.window())));
        #[cfg(debug_assertions)]
        { self.handlers.sites.pointer_move = Some(std::panic::Location::caller()); }
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
    #[track_caller]
    pub fn on_pointer_move_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.on_pointer_move = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.pointer_move = Some(std::panic::Location::caller()); }
        self
    }
    /// [`El::on_pointer_move_local`] for a component that doesn't read the app's state (a scroll
    /// bar that widens under the pointer), so it works in any app.
    #[track_caller]
    pub fn on_pointer_move_any(mut self, f: impl Fn(PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.on_pointer_move = Some(Rc::new(move |_, p, tx| f(p, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.pointer_move = Some(std::panic::Location::caller()); }
        self
    }

    /// Called once when the pointer enters this hit target, not on animation frames.
    #[track_caller]
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.handlers.on_hover = Some(Rc::new(move |s, _| f(downcast::<S>(s))));
        #[cfg(debug_assertions)]
        { self.handlers.sites.hover = Some(std::panic::Location::caller()); }
        self
    }

    /// Called with `true` when the pointer enters this element's visible rect and `false` when
    /// it leaves, descendants included (moving onto a child button is not a leave). It is not a
    /// hit target, so it never takes presses or hover from its children. A held press keeps the
    /// pointer inside until it releases.
    #[track_caller]
    pub fn on_hover_change<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_hover_change = Some(Rc::new(move |s, inside, tx| f(downcast::<S>(s), inside, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.hover_change = Some(std::panic::Location::caller()); }
        self
    }
    /// [`El::on_hover_change`] for a component that doesn't read the app's state.
    #[track_caller]
    pub fn on_hover_change_any(mut self, f: impl Fn(bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_hover_change = Some(Rc::new(move |_, inside, tx| f(inside, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.hover_change = Some(std::panic::Location::caller()); }
        self
    }
    /// [`El::on_hover_change_any`] after any hover handler the element already has, which keeps
    /// running: a component that follows the hover of an element it doesn't build. The change is
    /// live-only when both handlers say so.
    #[doc(hidden)]
    pub fn chain_hover_change_any(mut self, f: impl Fn(bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_hover_change = Some(match self.handlers.get_mut_if_some().and_then(|h| h.on_hover_change.take()) {
            Some(prev) => Rc::new(move |s, inside, tx| {
                prev(s, inside, tx);
                let prev_live = std::mem::take(&mut tx.live_only);
                f(inside, tx);
                tx.live_only &= prev_live;
            }),
            None => Rc::new(move |_, inside, tx| f(inside, tx)),
        });
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

    /// Track whether the pointer is still inside this element's hit bounds and inside the window
    /// during a held press, rather than holding [`Self::on_hover_change`] active until release.
    /// Moving a held press outside this element's clipped (and `press_scale`- /
    /// `press_offset`-compensated) bounds, or receiving a host pointer-leave while captured,
    /// fires `on_hover_change(false)` and retargets hover/press paint tracks off; moving back
    /// inside before release fires `on_hover_change(true)` and restores them without ending
    /// pointer capture or interrupting drags.
    ///
    /// Off by default so ancestors and drag containers keep Rune's drag-friendly retention.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Runtime, col, el};
    /// # #[derive(Default)] struct S { inside: Vec<bool> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// col().size(200.0, 100.0).child(
    ///     el().key("btn").size(80.0, 40.0)
    ///         .hover_tracks_bounds()
    ///         .on_press(|_: &mut S| {})
    ///         .on_hover_change(|s: &mut S, inside, _| s.inside.push(inside)),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # rt.pointer_move(20.0, 20.0);
    /// # rt.pointer_down(20.0, 20.0);
    /// # rt.frame(0.0);
    /// # rt.pointer_move(150.0, 20.0);
    /// # rt.frame(0.0);
    /// # rt.pointer_move(20.0, 20.0);
    /// # rt.frame(0.0);
    /// # assert_eq!(rt.state.inside, [true, false, true]);
    /// ```
    pub fn hover_tracks_bounds(mut self) -> Self {
        self.handlers.hover_tracks_bounds = true;
        self
    }

    #[doc(hidden)]
    pub fn wrap_scroll_area(mut self, fallback_site: (&'static str, u32, u32)) -> (String, String, Self, Self) {
        let key = self.key.clone().unwrap_or_else(|| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            fallback_site.hash(&mut h);
            let v = h.finish();
            let k = format!("scroll-{:08x}", (v ^ (v >> 32)) as u32);
            self.key = Some(k.clone());
            k
        });
        let area = format!("{key}-scroll-area");
        let mut outer = col().key(area.clone());
        outer.style = self.style.take_box_placement();
        if let Some(c) = self.cold.get_mut_if_some() {
            let (fit, above, above_boundary, transition) = (std::mem::replace(&mut c.aspect_fit, 0.0), c.above.take(), c.above_boundary.take(), c.transition.take());
            if fit > 0.0 || above.is_some() || above_boundary.is_some() || transition.is_some() {
                let o = outer.cold_mut();
                (o.aspect_fit, o.above, o.above_boundary, o.transition) = (fit, above, above_boundary, transition);
            }
        }
        outer.vis.cold_mut().clip_to_content = true;
        (area, key, outer, self)
    }

    /// Runs `f` during flattening once this element's full tree path is known, merging any
    /// handlers and overlay children `f` adds into the flattened node. Multiple calls chain in
    /// order. For components like `rune-kit`'s `ScrollBar` whose state and targets scope by tree
    /// path even though elements build bottom-up before ancestors' keys attach.
    #[doc(hidden)]
    pub fn bind_path(mut self, f: impl Fn(&str, &mut PathBind) + 'static) -> Self {
        let cold = self.cold_mut();
        cold.on_path = Some(match cold.on_path.take() {
            Some(prev) => Rc::new(move |path, bind| {
                prev(path, bind);
                f(path, bind);
            }),
            None => Rc::new(f),
        });
        self
    }
}

/// What [`El::bind_path`] can attach once an element's resolved tree path is known: chained
/// pointer, hover and scroll-offset handlers, and overlay children.
#[doc(hidden)]
#[derive(Default)]
pub struct PathBind {
    pub(crate) on_pointer_move: Option<Rc<dyn Fn(PointerEvent, &mut Tx)>>,
    pub(crate) on_hover_change: Option<Rc<dyn Fn(bool, &mut Tx)>>,
    pub(crate) on_scroll_offset: Option<ScrollOffsetFn>,
    #[cfg(debug_assertions)]
    pub(crate) pointer_move_site: Option<&'static std::panic::Location<'static>>,
    #[cfg(debug_assertions)]
    pub(crate) hover_change_site: Option<&'static std::panic::Location<'static>>,
    pub(crate) children: Vec<El>,
}

impl PathBind {
    #[track_caller]
    pub fn on_pointer_move_any(&mut self, f: impl Fn(PointerEvent, &mut Tx) + 'static) -> &mut Self {
        self.on_pointer_move = Some(match self.on_pointer_move.take() {
            Some(prev) => Rc::new(move |p, tx| {
                prev(p, tx);
                let prev_live = std::mem::take(&mut tx.live_only);
                f(p, tx);
                tx.live_only &= prev_live;
            }),
            None => Rc::new(f),
        });
        #[cfg(debug_assertions)]
        { self.pointer_move_site = Some(std::panic::Location::caller()); }
        self
    }

    #[track_caller]
    pub fn on_hover_change_any(&mut self, f: impl Fn(bool, &mut Tx) + 'static) -> &mut Self {
        self.on_hover_change = Some(match self.on_hover_change.take() {
            Some(prev) => Rc::new(move |inside, tx| {
                prev(inside, tx);
                let prev_live = std::mem::take(&mut tx.live_only);
                f(inside, tx);
                tx.live_only &= prev_live;
            }),
            None => Rc::new(f),
        });
        #[cfg(debug_assertions)]
        { self.hover_change_site = Some(std::panic::Location::caller()); }
        self
    }

    pub fn on_scroll_offset(&mut self, f: impl Fn(crate::ScrollPos) + 'static) -> &mut Self {
        self.on_scroll_offset = Some(match self.on_scroll_offset.take() {
            Some(prev) => Rc::new(move |at| {
                prev(at);
                f(at);
            }),
            None => Rc::new(f),
        });
        self
    }

    pub fn child(&mut self, child: El) -> &mut Self {
        self.children.push(child);
        self
    }
}
