use super::*;
use wasm_bindgen::{JsCast, JsValue, prelude::*};
use winit::event::{ElementState, TouchPhase};
use winit::keyboard::ModifiersState;

#[wasm_bindgen(module = "/src/platform/web/bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = isMac)]
    fn is_mac() -> bool;
    fn mount(canvas: &web_sys::HtmlCanvasElement, callback: &js_sys::Function, embedded: bool, fit_height: bool, document: bool) -> JsValue;
    fn sync(handle: &JsValue, key: &str, text: &str, base: usize, anchor: u32, focus: u32, secure: bool);
    #[wasm_bindgen(js_name = syncCopy)]
    fn sync_copy(handle: &JsValue, text: &str);
    #[wasm_bindgen(js_name = syncA11y)]
    fn sync_a11y(handle: &JsValue, json: &str);
    #[wasm_bindgen(js_name = syncScrollX)]
    fn sync_scroll_x(handle: &JsValue, path: &str, x: f32, rtl: bool);
    fn activate(handle: &JsValue);
    #[wasm_bindgen(js_name = syncPage)]
    fn sync_page(handle: &JsValue, height: f32, focused: bool);
    #[wasm_bindgen(js_name = presented)]
    fn presented_js(handle: &JsValue);
    fn destroy(handle: &JsValue);
    #[wasm_bindgen(js_name = pastedImage)]
    fn pasted_image(handle: &JsValue, max: u32) -> js_sys::Promise;
    #[wasm_bindgen(js_name = announceA11y)]
    fn announce_a11y(handle: &JsValue, text: &str, assertive: bool);
    #[wasm_bindgen(js_name = focusA11y)]
    fn focus_a11y(handle: &JsValue, json: &str);
    #[wasm_bindgen(js_name = scheduleA11y)]
    fn schedule_a11y(handle: &JsValue);
    #[wasm_bindgen(js_name = scheduleA11yIdle)]
    fn schedule_a11y_idle(handle: &JsValue);
    #[wasm_bindgen(js_name = syncSecondary)]
    fn sync_secondary(handle: &JsValue, claim: &str);
    #[wasm_bindgen(js_name = syncWheel)]
    fn sync_wheel(handle: &JsValue, used: bool);
    #[wasm_bindgen(js_name = syncCaret)]
    fn sync_caret(handle: &JsValue, x: f32, y: f32, w: f32, h: f32);
    #[wasm_bindgen(js_name = cancelIme)]
    fn cancel_ime(handle: &JsValue);
    #[wasm_bindgen(js_name = syncFamilies)]
    fn sync_families(handle: &JsValue, sans: &str, mono: &str);
    #[wasm_bindgen(js_name = showMenu)]
    fn show_menu(handle: &JsValue, json: &str, x: f32, y: f32, min_width: f32, dark: bool);
    #[wasm_bindgen(js_name = hideMenu)]
    fn hide_menu(handle: &JsValue);
}

/// How the canvas sits in the page.
#[derive(Clone, Copy)]
pub(super) struct Mode {
    /// Inline at its parent's width.
    pub embedded: bool,
    /// As tall as its content: vertical wheel and pans belong to the page.
    pub fit_height: bool,
    /// The app is the page (`App::web_document`).
    pub document: bool,
}

pub(super) struct Bridge {
    handle: JsValue,
    _callback: Closure<dyn FnMut(String)>,
    dispose_viewport: js_sys::Function,
    mods: ModifiersState,
    is_mac: bool,
    control_click: bool,
    mirror: crate::runtime::web_runtime::Mirror,
    a11y: crate::runtime::web_a11y::A11yMirror,
    copy_cache: String,
    fit_height: bool,
    document: bool,
    /// The page height and control focus the page last heard.
    page: Option<(f32, bool)>,
    /// The horizontal offsets and RTL flags the mirror's containers were last set to.
    scroll_x: Vec<(String, f32, bool)>,
    /// The IME area the page's text field was last put at.
    caret: Option<[f32; 4]>,
    /// When the mirror was last rebuilt, for the scroll throttle (`sync_input`).
    a11y_at: Option<crate::time::Instant>,
    /// `Tx::announce`'s calls the page's live regions have said.
    announced: u32,
    /// The mirror's first build (`sync_input`): not yet asked for, waiting for idle time, built.
    a11y_first: First,
    /// The slot families the mirror's text was last set in.
    families: (String, String),
}

/// Reads the pasted image bytes from the page's clipboard bridge.
pub(super) fn pasted_image_promise(handle: &JsValue) -> js_sys::Promise {
    pasted_image(handle, crate::clipboard::MAX_IMAGE_BYTES as u32)
}

impl Bridge {
    pub(super) fn new(canvas: &web_sys::HtmlCanvasElement,
        running: std::rc::Weak<RefCell<Option<Running>>>, window: Arc<Window>,
        mode: Mode) -> Self {
        let Mode { embedded, fit_height, document } = mode;
        let callback = Closure::new(move |event: String| {
            let Some(running) = running.upgrade() else { return };
            let event = zeroize::Zeroizing::new(event);
            if let Ok(mut event) = serde_json::from_str(&event) {
                // A DOM event fired synchronously by page JavaScript Rune called mid-frame (a
                // same-tab router tearing the route down) is dropped rather than re-entering.
                let Ok(mut running) = running.try_borrow_mut() else { return };
                if let Some(r) = &mut *running && r.input_event(&mut event, &window) { window.request_redraw(); }
            }
        });
        // A page's canvas is sized by its style and reports no safe area: the page lays out
        // around the notch itself.
        let dispose_viewport = platform::viewport(canvas, callback.as_ref().unchecked_ref(), embedded || document);
        let handle = mount(canvas, callback.as_ref().unchecked_ref(), embedded, fit_height, document);
        Self { handle, _callback: callback, dispose_viewport, mods: ModifiersState::empty(), is_mac: is_mac(), control_click: false,
            mirror: Default::default(), a11y: Default::default(),
            copy_cache: String::new(), fit_height, document, page: None, scroll_x: Vec::new(), caret: None, a11y_at: None, announced: 0, a11y_first: First::Unasked, families: Default::default() }
    }

    /// The first frame is on screen: a page swaps its static export for the live mirror.
    pub(super) fn presented(&self) { presented_js(&self.handle); }

    /// Opens `show` as the page's own menu, its top-left at its window point.
    pub(super) fn show_menu(&self, show: &crate::popup_menu::Show, dark: bool) {
        show_menu(&self.handle, &show.menu.to_json().to_string(), show.at.0, show.at.1, show.min_width, dark);
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.dispose_viewport.call0(&JsValue::NULL).ok();
        destroy(&self.handle);
    }
}

impl Running {
    pub(super) fn sync_input(&mut self, focus: bool) {
        let (reset, caret) = crate::input::host_events::ime_sync_state(self.app.rt_mut());
        if reset {
            cancel_ime(&self.input.handle);
        }
        if self.app.rt().web_context(&mut self.input.mirror) || reset {
            let m = &self.input.mirror;
            sync(&self.input.handle, &m.key, &m.text, m.base, m.anchor, m.focus, m.secure);
        }
        let next_copy = self.app.copy_text().unwrap_or_default();
        if self.input.copy_cache != next_copy {
            self.input.copy_cache = next_copy;
            sync_copy(&self.input.handle, &self.input.copy_cache);
        }
        let rt = self.app.rt();
        // The page's text field sits at the caret in window points, so an input method's
        // candidates open beside it at any zoom.
        if self.input.caret != caret {
            self.input.caret = caret;
            if let Some([x, y, w, h]) = self.input.caret { sync_caret(&self.input.handle, x, y, w, h); }
        }
        let (sans, mono) = rt.font_families();
        if !self.input.document && (self.input.families.0 != sans || self.input.families.1 != mono) {
            self.input.families = (sans.to_owned(), mono.to_owned());
            sync_families(&self.input.handle, sans, mono);
        }
        // While the app scrolls, the mirror follows at most every 150 ms (and once when it stops),
        // not every frame: rebuilding and restyling every item each frame was most of a scroll
        // frame's JavaScript. A page's mirror is in document coordinates and doesn't move.
        let now = crate::time::Instant::now();
        // An app's first mirror is every item a new element the browser lays out (13 ms of layout
        // on OpenCode's Home): it waits for the page's idle time, so a click or key that came
        // with the first frame is handled first. A page's mirror is its document, so it doesn't.
        if !self.input.document && self.input.a11y_first != First::Built {
            if self.input.a11y_first == First::Unasked {
                self.input.a11y_first = First::Waiting;
                schedule_a11y_idle(&self.input.handle);
            }
        } else if !self.input.document && rt.web_a11y_scrolled_only(&self.input.a11y)
            && self.input.a11y_at.is_some_and(|at| now.duration_since(at).as_secs_f64() < 0.15)
        {
            schedule_a11y(&self.input.handle);
        } else if let Some(json) = rt.web_a11y(&mut self.input.a11y) {
            sync_a11y(&self.input.handle, json);
            self.input.a11y_at = Some(now);
        }
        // An app's focus, for a reader: the browser's focus stays on the canvas or its text field.
        if !self.input.document && let Some(focus) = rt.web_a11y_focus(&mut self.input.a11y) {
            focus_a11y(&self.input.handle, focus);
        }
        // `Tx::announce`: the page's own live regions say it, as the native tree's announcer does.
        if rt.host.announcement.said() != self.input.announced {
            self.input.announced = rt.host.announcement.said();
            if let Some((words, how)) = rt.host.announcement.words() {
                announce_a11y(&self.input.handle, &words, how == crate::LiveRegion::Assertive);
            }
        }
        if self.input.document {
            let page = (rt.page_height().ceil(), rt.has_focus());
            if self.input.page != Some(page) {
                self.input.page = Some(page);
                sync_page(&self.input.handle, page.0, page.1);
            }
            // A horizontal viewport Rune scrolled (a wheel or a key on desktop) moves the mirror's
            // container to match; one the browser scrolled already says where it is.
            let handle = &self.input.handle;
            let mut nodes = rt.tree.nodes.iter();
            let next: Vec<_> = rt.page_scroll_x().into_iter().map(|(path, x)| {
                let rtl = nodes.find(|n| n.path == path).is_some_and(|n| n.flow == crate::el::Flow::Rtl);
                (path, x, rtl)
            }).collect();
            for (path, x, rtl) in &next {
                let known = self.input.scroll_x.iter().find(|(p, _, _)| p == path).map(|(_, x, rtl)| (*x, *rtl));
                if known.is_none_or(|(kx, krtl)| (kx - x).abs() > 0.5 || krtl != *rtl) {
                    sync_scroll_x(handle, path, *x, *rtl);
                }
            }
            self.input.scroll_x = next;
        }
        if focus { activate(&self.input.handle); }
    }

    /// The page's field takes the focused field's context from the model now (web_edit_context);
    /// false when a whole frame has to run inside this event instead.
    fn sync_live_edit(&mut self) -> bool {
        let Some(changed) = self.app.web_edit_context(&mut self.input.mirror) else { return false };
        if changed {
            let m = &self.input.mirror;
            sync(&self.input.handle, &m.key, &m.text, m.base, m.anchor, m.focus, m.secure);
        }
        true
    }

    /// Handles one DOM event. Returns whether a frame must be drawn: a page's scroll only draws
    /// when it moves the band (the browser's compositor scrolls the pixels Rune already drew).
    pub(super) fn input_event(&mut self, event: &mut serde_json::Value, window: &Arc<Window>) -> bool {
        if event["kind"].as_str() == Some("mods") {
            let next = crate::input::host_events::modifiers_from_flags(
                event["shift"].as_bool().unwrap_or(false),
                event["ctrl"].as_bool().unwrap_or(false),
                event["alt"].as_bool().unwrap_or(false),
                event["meta"].as_bool().unwrap_or(false),
            );
            let redraw = self.app.sync_modifiers(&mut self.input.mods, next);
            if redraw { self.full_redraw = true; }
            return redraw;
        }
        if event["kind"].as_str() == Some("pointer_cancel") {
            let lost_capture = event["lost_capture"].as_bool().unwrap_or(false);
            if !crate::input::host_events::pointer_cancel(
                self.app.rt_mut(),
                &mut self.input.control_click,
                lost_capture,
            ) {
                return false;
            }
            self.full_redraw = true;
            self.cursor.apply(window, crate::el::Cursor::Default, false);
            self.app.frame(0.0);
            self.apply_frame_requests(window);
            self.clipboard.flush_core(self.app.rt_mut());
            self.sync_input(false);
            return true;
        }
        self.full_redraw = true;
        let mut activate = false;
        // What a person typed or pressed through the page's own elements is input too, for
        // `rune::last_input` (an app holds background work while someone types).
        if matches!(event["kind"].as_str(), Some("edit" | "preedit" | "selection" | "key" | "paste" | "a11y_press" | "a11y_link")) {
            crate::input::time::typed();
        }
        match event["kind"].as_str() {
            Some("viewport") => {
                let insets = std::array::from_fn(|i| event["insets"][i].as_f64().unwrap_or(0.0).max(0.0) as f32);
                let h = event["h"].as_f64().unwrap_or(0.0).max(0.0) as f32;
                // A phone reports the visual viewport on every scroll frame as its toolbar moves;
                // when neither the safe area nor the largest viewport moved, there's nothing to do.
                let moved = LARGE_VIEWPORT.replace(h) != h;
                if INSETS.replace(insets) == insets && !moved { return false; }
                if moved { self.resize_surface(window, window.inner_size(), window.scale_factor() as f32); }
                self.app.rt_mut().invalidate();
            }
            Some("dpr") => {
                let size = window.inner_size();
                let scale = event["scale"].as_f64().unwrap_or_else(|| window.scale_factor()).max(0.5) as f32;
                self.resize_surface(window, size, scale);
            }
            Some("motion") => self.app.rt_mut().set_system_reduced_motion(event["reduced"].as_bool().unwrap_or(false)),
            // The primary pointer can or can't hover (a mouse or a touch screen), read by views
            // through `Cx::hover_none`.
            Some("primary_pointer") => {
                let pointer = match (event["hover"].as_bool(), event["coarse"].as_bool()) {
                    (Some(false), Some(true)) => PrimaryPointer::Touch,
                    (Some(false), _) => PrimaryPointer::NoHover,
                    _ => PrimaryPointer::Hover,
                };
                if PRIMARY_POINTER.replace(pointer) == pointer { return false; }
                self.app.rt_mut().invalidate();
            }
            // The browser's `forced-colors` (a contrast theme) and its system colours, as sRGB bytes:
            // Canvas, CanvasText, Highlight, HighlightText, GrayText, ButtonFace, ButtonText, LinkText.
            Some("contrast") => {
                let colors: Option<Vec<crate::Color>> = event["colors"].as_array().map(|list| list.iter().map(|c| {
                    let d = |i: usize| crate::color::srgb_decode(c[i].as_f64().unwrap_or(0.0) as f32 / 255.0);
                    crate::Color { r: d(0), g: d(1), b: d(2), a: 1.0 }
                }).collect());
                let contrast = colors.filter(|c| c.len() == 8).map(|c| crate::SystemContrast {
                    window: c[0], text: c[1], highlight: c[2], highlight_text: c[3], disabled: c[4], button: c[5], button_text: c[6], link: c[7],
                });
                self.app.rt_mut().set_system_contrast(contrast);
            }
            Some("focus") => {
                let focused = event["focused"].as_bool().unwrap_or(false);
                self.app.window_focus(focused);
                if !focused {
                    self.input.mods = ModifiersState::empty();
                    self.input.control_click = false;
                    self.cursor.apply(window, crate::el::Cursor::Default, false);
                }
            }
            Some("visibility") => {
                let visible = event["visible"].as_bool().unwrap_or(false);
                self.shared.set_visible(visible);
                if !visible {
                    crate::input::host_events::hide_reset(
                        self.app.rt_mut(),
                        &mut self.input.mods,
                        &mut self.input.control_click,
                    );
                    hide_menu(&self.input.handle);
                    self.app.resolve_popup(None).ok();
                    self.cursor.apply(window, crate::el::Cursor::Default, false);
                    // Commit cancellation, FocusLost, FinishIme and poll now: browsers pause rAF
                    // while the tab is hidden.
                    self.app.frame(0.0);
                    self.apply_frame_requests(window);
                    self.clipboard.flush_core(self.app.rt_mut());
                    self.sync_input(false);
                    return false;
                }
                if event["focused"].as_bool().unwrap_or(false) {
                    self.app.window_focus(true);
                }
                return true;
            }
            Some("leave") => {
                if self.app.rt().touching() { return false; }
                self.app.rt_mut().pointer_leave();
                self.cursor.apply(window, crate::el::Cursor::Default, false);
                return true;
            }
            Some(kind @ ("edit" | "selection" | "preedit")) => {
                self.app.web_edit(event);
                // A typed edit is in the model and the page's field: the display's next frame
                // draws it, one frame for every key and press that came before it.
                if kind == "edit" && self.sync_live_edit() { return true; }
            }
            Some("sync") => self.input.mirror.key.clear(), // Composition's final sync was deferred by the DOM.
            Some("paste") => {
                let mut content = crate::ClipboardContent::new(event["text"].as_str().unwrap_or_default());
                if let Some(html) = event["html"].as_str().filter(|html| !html.is_empty()) { content = content.with_html(html); }
                let image = (event["image"].as_bool() == Some(true)).then_some(&self.input.handle);
                self.app.paste_event(&self.clipboard, content, image);
            }
            Some("secure") => {
                if let serde_json::Value::String(text) = event["text"].take() {
                    self.app.rt_mut().web_secure(event["key"].as_str().unwrap_or_default(), text);
                }
            }
            Some("key") => activate = self.app.web_key(event),
            Some("a11y_press" | "a11y_link") => { let _ = self.app.rt_mut().web_a11y_action(event); }
            // The page's menu (`showMenu`) was picked from or dismissed.
            Some("menu") => {
                if self.app.resolve_popup(event["id"].as_str()).is_err() { self.app.resolve_popup(None).ok(); }
            }
            // The browser's menu takes this right-click (bridge.js): its release opens none of Rune's.
            Some("secondary_browser") => { self.app.rt_mut().secondary_abandon(); return false; }
            // The scroll throttle's last word: the mirror where the scroll stopped.
            Some("a11y_due") => {
                self.input.a11y_first = First::Built;
                self.input.a11y_at = None;
                self.sync_input(false);
                return false;
            }
            // The page scrolled: draw the band in view, and hover what is now under the pointer.
            // The redraw that follows the event draws it; nothing commits.
            Some("scroll") => {
                let (moved, hover) = {
                    let rt = self.app.rt_mut();
                    let before = rt.page_scroll_y();
                    let moved = rt.set_page_scroll(event["y"].as_f64().unwrap_or(0.0) as f32);
                    // The pointer keeps its place on screen while the band moves under it.
                    let turned = rt.page_scroll_y() - before;
                    let hover = (turned != 0.0).then(|| rt.pointer().filter(|_| !rt.touching()).map(|(x, y)| (x, y + turned))).flatten();
                    (moved, hover)
                };
                if let Some((x, y)) = hover {
                    self.app.pointer_move(x, y);
                }
                return moved;
            }
            // A page's horizontal viewport scrolled natively, by the browser's own compositor.
            Some("scroll_x") => {
                let path = event["path"].as_str().unwrap_or_default();
                let x = event["x"].as_f64().unwrap_or(0.0) as f32;
                return self.app.rt_mut().set_page_scroll_x(path, x);
            }
            // `WheelEvent.DOM_DELTA_PAGE` (deltaMode === 2), which winit's web backend drops.
            Some("wheel_page") => {
                let mods = crate::input::key_table::web_mods(
                    event["shift"].as_bool().unwrap_or(false),
                    event["ctrl"].as_bool().unwrap_or(false),
                    event["alt"].as_bool().unwrap_or(false),
                    event["meta"].as_bool().unwrap_or(false),
                );
                let dx_pages = event["dx"].as_f64().unwrap_or(0.0) as f32;
                let dy_pages = event["dy"].as_f64().unwrap_or(0.0) as f32;
                let shift = !self.input.is_mac && mods.shift;
                let (dx, dy) = crate::input::host_events::wheel_page_delta(self.app.rt(), dx_pages, dy_pages, shift);
                let (redraw, wheel_used) = crate::input::host_events::web_wheel(
                    self.app.rt_mut(),
                    dx,
                    dy,
                    mods,
                    crate::GesturePhase::Moved,
                    false,
                    self.input.document,
                    self.input.fit_height,
                );
                if let Some(used) = wheel_used { sync_wheel(&self.input.handle, used); }
                return redraw;
            }
            // The pointer is over mirrored text or a link, which takes it from the canvas.
            Some("hover") => {
                let (x, y, offset) = {
                    let rt = self.app.rt();
                    if rt.touching() { return false; }
                    let (x, y) = crate::input::host_events::window_point(
                        rt,
                        event["x"].as_f64().unwrap_or(0.0) as f32,
                        event["y"].as_f64().unwrap_or(0.0) as f32,
                    );
                    (x, y, rt.page_window().0)
                };
                // The mirror's pointer is in window points; the canvas draws the band.
                self.app.pointer_move(x, y + offset);
                return true;
            }
            _ => {}
        }
        // Commit within the DOM event: clipboard, link and soft-keyboard APIs need activation.
        self.app.frame(0.0);
        self.apply_frame_requests(window);
        self.clipboard.flush_core(self.app.rt_mut());
        self.sync_input(activate);
        true
    }

    pub(super) fn window_input(&mut self, event: &WindowEvent, window: &Arc<Window>) -> bool {
        let scale = window.scale_factor() as f32;
        let mut activate = false;
        let mut immediate = false;
        match event {
            WindowEvent::ModifiersChanged(mods) => {
                return self.app.sync_modifiers(&mut self.input.mods, mods.state());
            }
            WindowEvent::CursorMoved { position, .. } if !self.app.rt().touching() => {
                let (x, y) = crate::input::host_events::physical_point(self.app.rt(), position.x, position.y);
                self.app.pointer_move(x, y);
                self.cursor.apply(window, self.app.rt().cursor(), true);
            }
            WindowEvent::CursorLeft { .. } if !self.app.rt().touching() => {
                self.app.rt_mut().pointer_leave();
                self.cursor.apply(window, crate::el::Cursor::Default, false);
            }
            WindowEvent::MouseInput { button, state, .. } if !self.app.rt().touching() => {
                immediate = true;
                let Some((x, y)) = self.app.rt().pointer() else { return false };
                let mods = crate::window::map_mods(self.input.mods);
                let action = self.app.mouse_button(
                    crate::input::host_events::MouseButtonInput {
                        x,
                        y,
                        button: crate::window::pointer_button(*button),
                        pressed: *state == ElementState::Pressed,
                        mods,
                        ctrl_click_is_secondary: self.input.is_mac,
                        // Queued after `web_focus_field` below so the text-field fast path still
                        // sees `Action::Focus` as its sole activation-sensitive action.
                        queue_mods_on_primary_down: false,
                    },
                    &mut self.input.control_click,
                );
                match action {
                    crate::input::host_events::ButtonAction::PrimaryDown => {
                        activate = true;
                        // A press into a text field: the field takes focus now (a key may follow
                        // before the next frame, and a phone's keyboard opens only inside the
                        // press); the frame waits for the display's.
                        if self.app.web_focus_field() && self.sync_live_edit() {
                            self.app.rt_mut().physical_input(crate::PhysicalInput::Modifiers(mods));
                            super::input::activate(&self.input.handle);
                            return true;
                        }
                        self.app.rt_mut().physical_input(crate::PhysicalInput::Modifiers(mods));
                    }
                    crate::input::host_events::ButtonAction::PrimaryUp => {
                        // Only a release with a handler runs its frame inside the event. A field's
                        // own selection step (a move while held) runs as a press into it does.
                        immediate = self.app.rt().web_release_needs_frame() && !(self.app.web_focus_field() && self.sync_live_edit());
                    }
                    crate::input::host_events::ButtonAction::SecondaryDown => {
                        // The browser's own menu opens where Rune's doesn't (bridge.js).
                        sync_secondary(&self.input.handle, self.app.rt().secondary_claim());
                    }
                    crate::input::host_events::ButtonAction::SecondaryUp
                    | crate::input::host_events::ButtonAction::OtherDown(_)
                    | crate::input::host_events::ButtonAction::OtherUp(_) => {}
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                // On macOS, AppKit swaps Shift+mouse-wheel before the browser sees it while
                // leaving Shift+trackpad vertical. Elsewhere, browsers that keep deltaX == 0
                // rely on wheel_delta to swap vertical into horizontal (it never double-swaps
                // when dx != 0).
                let shift = !self.input.is_mac && self.input.mods.shift_key();
                let (dx, dy) = crate::runtime::wheel_delta(*delta, scale, shift, crate::runtime::LINE_POINTS);
                let precise = matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_));
                // Browsers give WheelEvent no gesture phase (always Moved), matching unphased
                // desktop mouse wheels so WheelLock uses its idle timeout.
                let phase = crate::input::host_events::gesture_phase(*phase);
                let (redraw, wheel_used) = crate::input::host_events::web_wheel(
                    self.app.rt_mut(),
                    dx,
                    dy,
                    crate::window::map_mods(self.input.mods),
                    phase,
                    precise,
                    self.input.document,
                    self.input.fit_height,
                );
                if let Some(used) = wheel_used { sync_wheel(&self.input.handle, used); }
                if !redraw { return false; }
            }
            // A page's Tab is the browser's, through the mirror's links and buttons.
            WindowEvent::KeyboardInput { event, .. }
                if self.input.document && event.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Tab) => return false,
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                // Browsers expose no synchronous layout key_without_modifiers, so `Tx::key_base`
                // stays `None` on web by design; physical US positions arrive in `physical_key`.
                if !self.app.keyboard(&self.clipboard, event, self.input.mods) { return false; }
                immediate = true;
                // Any key that puts Rune's focus in a field (Tab, or a shortcut such as the
                // prompt's) hands the page's own field the browser's focus, so a screen reader is
                // in a real text field with its text, not on the canvas's stand-in for it.
                activate = event.state == ElementState::Pressed;
            }
            // On a canvas as tall as its content the browser takes vertical pans and cancels the
            // touch; taps and horizontal drags stay Rune's.
            WindowEvent::Touch(touch) => {
                self.app.touch(touch);
                // A tap is a user activation: links, the clipboard and focus act before the handler
                // returns. In a page only a tap that left something focused needs it; anything else
                // would focus the canvas and drop the reader's selection.
                if matches!(touch.phase, TouchPhase::Ended) {
                    activate = !self.input.document || self.app.rt().has_focus();
                    immediate = true;
                }
            }
            _ => return false,
        }
        // Moves and wheels are coalesced by rAF. Only activation-sensitive input must
        // commit before the DOM handler returns (clipboard, links and keyboard focus).
        if immediate {
            self.app.frame(0.0);
            self.apply_frame_requests(window);
            self.clipboard.flush_core(self.app.rt_mut());
            self.sync_input(activate);
            let rt = self.app.rt();
            self.cursor.apply(window, rt.cursor(), rt.pointer().is_some());
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum First { Unasked, Waiting, Built }
