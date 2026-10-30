//! Event, gesture, layout-watch and pointer-feedback handlers shared by `El` and `Node` behind
//! `Cold<Handlers>`, so elements without callbacks pay one pointer.
use crate::cold::Empty;
use crate::el::{
    AtBottomFn, Cursor, DragEndFn, DragFn, DragOutFn, DrawnLayoutFn, DropSpec, LayoutFn, ModalSpec,
    PasteFn, PointerFn, PressFn, ScrollOffsetFn, SelectionChangeFn, SelectionCopy, VisibleRowsFn,
    WindowFocusFn,
};

#[derive(Clone)]
pub(crate) struct Handlers {
    pub on_paste: Option<PasteFn>,
    pub on_window_focus: Option<WindowFocusFn>,
    pub modal: Option<ModalSpec>,
    pub on_press: Option<PressFn>,
    pub on_double_press: Option<PressFn>,
    pub on_secondary_press: Option<PointerFn>,
    pub on_hover: Option<PressFn>,
    pub on_hover_change: Option<WindowFocusFn>,
    /// Seconds the pointer must stay inside, then outside, before `on_hover_change` hears it.
    pub hover_delay: [f32; 2],
    pub on_pointer_move: Option<PointerFn>,
    /// `on_wheel`, `on_magnify` and the other canvas handlers (`canvas_input.rs`).
    pub gestures: Option<Box<crate::canvas_input::CanvasInput>>,
    pub on_drag: Option<DragFn>,
    pub drag_out: Option<DragOutFn>,
    pub drag_threshold: Option<f32>,
    pub on_drag_start: Option<PressFn>,
    pub on_drag_end: Option<DragEndFn>,
    pub drop: Option<Box<DropSpec>>,
    pub on_at_bottom: Option<AtBottomFn>,
    pub on_far_from_bottom: Option<(f32, AtBottomFn)>,
    pub on_visible_rows: Option<VisibleRowsFn>,
    pub on_layout: Option<LayoutFn>,
    pub on_screen_layout: Option<LayoutFn>,
    pub on_drawn_layout: Option<DrawnLayoutFn>,
    pub on_scroll_offset: Option<ScrollOffsetFn>,
    pub selection_copy: Option<SelectionCopy>,
    pub on_selection_change: Option<SelectionChangeFn>,
    pub cursor: Option<Cursor>,
    pub press_scale: Option<f32>,
}

impl Handlers {
    #[inline]
    pub(crate) fn has_primary(&self) -> bool {
        self.on_press.is_some()
            || self.on_double_press.is_some()
            || self.on_hover.is_some()
            || self.on_drag.is_some()
            || self.drag_out.is_some()
    }

    #[inline]
    pub(crate) fn takes_hit(&self) -> bool {
        self.on_pointer_move.is_some()
            || self.on_press.is_some()
            || self.on_double_press.is_some()
            || self.on_secondary_press.is_some()
            || self.on_hover.is_some()
            || self.on_hover_change.is_some()
            || self.on_drag.is_some()
            || self.drag_out.is_some()
            || self.modal.is_some()
            || self.gestures.as_ref().is_some_and(|g| g.takes_gestures())
    }
}

impl Empty for Handlers {
    const EMPTY: &'static Self = &Handlers {
        on_paste: None,
        on_window_focus: None,
        modal: None,
        on_press: None,
        on_double_press: None,
        on_secondary_press: None,
        on_hover: None,
        on_hover_change: None,
        hover_delay: [0.0; 2],
        on_pointer_move: None,
        gestures: None,
        on_drag: None,
        drag_out: None,
        drag_threshold: None,
        on_drag_start: None,
        on_drag_end: None,
        drop: None,
        on_at_bottom: None,
        on_far_from_bottom: None,
        on_visible_rows: None,
        on_layout: None,
        on_screen_layout: None,
        on_drawn_layout: None,
        on_scroll_offset: None,
        selection_copy: None,
        on_selection_change: None,
        cursor: None,
        press_scale: None,
    };
}
