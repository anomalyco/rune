//! Event, gesture, layout-watch and pointer-feedback handlers shared by `El` and `Node` behind
//! `Cold<Handlers>`, so elements without callbacks pay one pointer.
use crate::cold::Empty;
use crate::el::{
    A11yActionFn, AnyPressFn, AtBottomFn, Cursor, DragEndFn, DragFn, DragOutFn, DropSpec, LanguagesFn, LayoutFn,
    LayoutTxFn, ModalSpec, PasteFn, PointerFn, PointerMotionFn, PressFn, ScrollOffsetFn, SelectionChangeFn, SelectionCopy,
    VisibleRowsFn, WindowFocusFn,
};

/// One custom accessibility action registered via [`crate::El::a11y_action`].
#[derive(Clone)]
pub(crate) struct CustomA11yAction {
    pub id: String,
    pub label: String,
    pub handler: PressFn,
}

#[derive(Clone)]
pub(crate) struct Handlers {
    pub on_a11y_action: Option<A11yActionFn>,
    /// `El::a11y_action`: custom accessibility actions (VoiceOver's Actions rotor, AT-SPI
    /// Action interface, web mirror action buttons), boxed so elements without them pay one
    /// pointer inside `Cold<Handlers>`.
    pub custom_a11y_actions: Option<Box<Vec<CustomA11yAction>>>,
    pub on_paste: Option<PasteFn>,
    pub on_window_focus: Option<WindowFocusFn>,
    /// `El::on_spellcheck`: the app's spell checking setting and what flips it.
    pub on_spellcheck: Option<(bool, WindowFocusFn)>,
    /// `El::on_spellcheck_languages`: keeps the dictionaries a field's menu chose.
    pub on_spellcheck_languages: Option<LanguagesFn>,
    pub modal: Option<ModalSpec>,
    pub on_press: Option<PressFn>,
    pub on_double_press: Option<PressFn>,
    pub on_secondary_press: Option<PointerFn>,
    /// `El::on_middle_press`.
    pub on_middle_press: Option<PressFn>,
    /// `El::on_any_press`: passive window-wide mouse button-down observer.
    pub on_any_press: Option<AnyPressFn>,
    pub on_hover: Option<PressFn>,
    pub on_hover_change: Option<WindowFocusFn>,
    /// `El::on_focus_change`: this element gained (`true`) or lost (`false`) focus.
    pub on_focus_change: Option<WindowFocusFn>,
    /// `El::on_focus_within_change`: focus entered (`true`) or left (`false`) this element or a descendant.
    pub on_focus_within_change: Option<WindowFocusFn>,
    /// Seconds the pointer must stay inside, then outside, before `on_hover_change` hears it.
    pub hover_delay: [f32; 2],
    /// `El::hover_tracks_bounds`.
    pub hover_tracks_bounds: bool,
    pub on_pointer_move: Option<PointerFn>,
    /// `El::pointer_lock`: declarative relative mouse-look capture.
    pub pointer_lock: Option<PointerMotionFn>,
    /// `on_wheel`, `on_magnify` and the other canvas handlers (`canvas_input.rs`).
    pub gestures: Option<Box<crate::input::canvas_input::CanvasInput>>,
    pub on_drag: Option<DragFn>,
    pub drag_out: Option<DragOutFn>,
    pub drag_threshold: Option<f32>,
    /// `El::touch_drags_at_once`.
    pub touch_at_once: bool,
    /// `El::on_edge_drag`: per-edge registrations (boxed so elements without one pay 8 bytes).
    pub edge_drags: Option<Box<EdgeDrags>>,
    /// `El::press_on_down`.
    pub press_on_down: bool,
    pub on_drag_start: Option<PressFn>,
    pub on_drag_end: Option<DragEndFn>,
    pub drop: Option<Box<DropSpec>>,
    pub on_at_bottom: Option<AtBottomFn>,
    /// `on_far_from_bottom`: its least distance, the key of the frame whose height counts in
    /// place of the viewport's own (`on_far_from_bottom_under`), and its handler.
    pub on_far_from_bottom: Option<(f32, Option<std::rc::Rc<str>>, AtBottomFn)>,
    pub on_visible_rows: Option<VisibleRowsFn>,
    pub on_layout: Option<LayoutTxFn>,
    pub on_screen_layout: Option<LayoutFn>,
    pub on_drawn_layout: Option<LayoutTxFn>,
    /// `El::on_visible`: the element came on screen (`true`) or left it.
    pub on_visible: Option<WindowFocusFn>,
    /// `El::on_mount`: runs once when this element mounts, in the same frame before drawing.
    pub on_mount: Option<PressFn>,
    /// `El::drawn_rect_into`: where each frame draws the element.
    pub drawn_rect: Option<crate::Live<[f32; 4]>>,
    pub on_scroll_offset: Option<ScrollOffsetFn>,
    pub selection_copy: Option<SelectionCopy>,
    pub on_selection_change: Option<SelectionChangeFn>,
    pub cursor: Option<Cursor>,
    pub press_scale: Option<f32>,
    pub press_offset: Option<[f32; 2]>,
    /// Debug builds: where the app set the handlers the commit warnings name.
    #[cfg(debug_assertions)]
    pub sites: HandlerSites,
}

#[cfg(debug_assertions)]
type Site = Option<&'static std::panic::Location<'static>>;

/// Where `on_hover`, `on_hover_change`, `on_pointer_move`, `on_layout`, `on_drawn_layout` and
/// `on_visible_rows` were set.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Default)]
pub(crate) struct HandlerSites {
    pub hover: Site,
    pub hover_change: Site,
    pub pointer_move: Site,
    pub pointer_lock: Site,
    pub layout: Site,
    pub drawn_layout: Site,
    pub visible_rows: Site,
}

/// Per-edge [`El::on_edge_drag`] registrations, indexed by [`SwipeEdge::index`].
#[derive(Clone, Default)]
pub(crate) struct EdgeDrags(pub [Option<(f32, DragFn)>; 4]);

impl EdgeDrags {
    pub(crate) fn set(&mut self, edge: crate::el::SwipeEdge, width: f32, f: DragFn) {
        self.0[edge.index()] = Some((width, f));
    }

    pub(crate) fn get(&self, edge: crate::el::SwipeEdge) -> Option<&(f32, DragFn)> {
        self.0[edge.index()].as_ref()
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        self.0.iter().zip(other.0.iter()).all(|(a, b)| match (a, b) {
            (None, None) => true,
            (Some((wa, fa)), Some((wb, fb))) => wa == wb && std::rc::Rc::ptr_eq(fa, fb),
            _ => false,
        })
    }
}

impl Handlers {
    #[inline]
    pub(crate) fn has_primary(&self) -> bool {
        self.on_press.is_some()
            || self.on_double_press.is_some()
            || self.on_hover.is_some()
            || self.on_drag.is_some()
            || self.drag_out.is_some()
            || self.pointer_lock.is_some()
    }

    #[inline]
    pub(crate) fn takes_hit(&self) -> bool {
        self.on_pointer_move.is_some()
            || self.pointer_lock.is_some()
            || self.on_press.is_some()
            || self.on_double_press.is_some()
            || self.on_secondary_press.is_some()
            || self.on_middle_press.is_some()
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
        on_a11y_action: None,
        custom_a11y_actions: None,
        on_paste: None,
        on_window_focus: None,
        on_spellcheck: None,
        on_spellcheck_languages: None,
        modal: None,
        on_press: None,
        on_double_press: None,
        on_secondary_press: None,
        on_middle_press: None,
        on_any_press: None,
        on_hover: None,
        on_hover_change: None,
        on_focus_change: None,
        on_focus_within_change: None,
        hover_delay: [0.0; 2],
        hover_tracks_bounds: false,
        on_pointer_move: None,
        pointer_lock: None,
        gestures: None,
        on_drag: None,
        drag_out: None,
        drag_threshold: None,
        touch_at_once: false,
        edge_drags: None,
        press_on_down: false,
        on_drag_start: None,
        on_drag_end: None,
        drop: None,
        on_at_bottom: None,
        on_far_from_bottom: None,
        on_visible_rows: None,
        on_layout: None,
        on_screen_layout: None,
        on_drawn_layout: None,
        on_visible: None,
        on_mount: None,
        drawn_rect: None,
        on_scroll_offset: None,
        selection_copy: None,
        on_selection_change: None,
        cursor: None,
        press_scale: None,
        press_offset: None,
        #[cfg(debug_assertions)]
        sites: HandlerSites { hover: None, hover_change: None, pointer_move: None, pointer_lock: None, layout: None, drawn_layout: None, visible_rows: None },
    };
}
