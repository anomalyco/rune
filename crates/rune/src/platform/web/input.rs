use super::*;
use wasm_bindgen::{JsCast, JsValue, prelude::*};
use winit::event::{ElementState, MouseButton, TouchPhase};
use winit::keyboard::ModifiersState;

#[wasm_bindgen(module = "/src/platform/web/bridge.js")]
extern "C" {
    fn mount(canvas: &web_sys::HtmlCanvasElement, callback: &js_sys::Function, embedded: bool, fit_height: bool) -> JsValue;
    fn sync(handle: &JsValue, key: &str, text: &str, base: usize, anchor: u32, focus: u32, secure: bool);
    #[wasm_bindgen(js_name = syncCopy)]
    fn sync_copy(handle: &JsValue, text: &str);
    #[wasm_bindgen(js_name = syncA11y)]
    fn sync_a11y(handle: &JsValue, json: &str);
    fn activate(handle: &JsValue);
    fn destroy(handle: &JsValue);
    #[wasm_bindgen(js_name = pastedImage)]
    fn pasted_image(handle: &JsValue, max: u32) -> js_sys::Promise;
}

pub(super) struct Bridge {
    handle: JsValue,
    _callback: Closure<dyn FnMut(String)>,
    dispose_viewport: js_sys::Function,
    mods: ModifiersState,
    touch: Option<Touch>,
    mirror: crate::runtime::web_runtime::Mirror,
    a11y: crate::runtime::web_a11y::A11yMirror,
    copy_cache: String,
    fit_height: bool,
}
struct Touch {
    id: u64,
    scroll: Option<crate::NodeId>,
    origin: (f32, f32),
    last: (f32, f32),
    at: Instant,
    velocity: f32,
    pan: bool,
    /// The pan moves a horizontal viewport (picked when it starts).
    horizontal: bool,
}

impl Bridge {
    pub(super) fn new<S: 'static>(canvas: &web_sys::HtmlCanvasElement,
        running: std::rc::Weak<RefCell<Option<Running<S>>>>, window: Arc<Window>,
        embedded: bool, fit_height: bool) -> Self {
        let callback = Closure::new(move |event: String| {
            let Some(running) = running.upgrade() else { return };
            let event = zeroize::Zeroizing::new(event);
            if let Ok(mut event) = serde_json::from_str(&event) {
                // A DOM event fired synchronously by page JavaScript Rune called mid-frame (a
                // same-tab router tearing the route down) is dropped rather than re-entering.
                let Ok(mut running) = running.try_borrow_mut() else { return };
                if let Some(r) = &mut *running { r.input_event(&mut event, &window); }
                window.request_redraw();
            }
        });
        let dispose_viewport = platform::viewport(canvas, callback.as_ref().unchecked_ref(), embedded);
        let handle = mount(canvas, callback.as_ref().unchecked_ref(), embedded, fit_height);
        Self { handle, _callback: callback, dispose_viewport, mods: ModifiersState::empty(),
            touch: None, mirror: Default::default(), a11y: Default::default(),
            copy_cache: String::new(), fit_height }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.dispose_viewport.call0(&JsValue::NULL);
        destroy(&self.handle);
    }
}

impl<S: 'static> Running<S> {
    pub(super) fn sync_input(&mut self, focus: bool) {
        if self.rt.web_context(&mut self.input.mirror) {
            let m = &self.input.mirror;
            sync(&self.input.handle, &m.key, &m.text, m.base, m.anchor, m.focus, m.secure);
        }
        let next_copy = self.rt.copy_text().unwrap_or_default();
        if self.input.copy_cache != next_copy {
            self.input.copy_cache = next_copy;
            sync_copy(&self.input.handle, &self.input.copy_cache);
        }
        if let Some(json) = self.rt.web_a11y(&mut self.input.a11y) {
            sync_a11y(&self.input.handle, json);
        }
        if focus { activate(&self.input.handle); }
    }

    pub(super) fn input_event(&mut self, event: &mut serde_json::Value, window: &Arc<Window>) {
        let mut activate = false;
        match event["kind"].as_str() {
            Some("viewport") => {
                let insets = std::array::from_fn(|i| event["insets"][i].as_f64().unwrap_or(0.0).max(0.0) as f32);
                if INSETS.replace(insets) != insets { self.rt.invalidate(); }
            }
            Some("dpr") => {
                let size = window.inner_size();
                let scale = event["scale"].as_f64().unwrap_or_else(|| window.scale_factor()).max(0.5) as f32;
                self.resize_surface(size, scale);
            }
            Some("motion") => self.rt.set_system_reduced_motion(event["reduced"].as_bool().unwrap_or(false)),
            Some("focus") => self.rt.window_focus(event["focused"].as_bool().unwrap_or(false)),
            Some("visibility") => {
                self.shared.visible.store(event["visible"].as_bool().unwrap_or(false), Ordering::Release);
                return;
            }
            Some("edit" | "selection" | "preedit") => self.rt.web_edit(event),
            Some("sync") => self.input.mirror.key.clear(), // Composition's final sync was deferred by the DOM.
            Some("paste") => {
                let mut content = crate::ClipboardContent::new(event["text"].as_str().unwrap_or_default());
                if let Some(html) = event["html"].as_str().filter(|html| !html.is_empty()) { content = content.with_html(html); }
                let handle = &self.input.handle;
                let image = (event["image"].as_bool() == Some(true)).then_some(|| pasted_image(handle, crate::clipboard::MAX_IMAGE_BYTES as u32));
                self.clipboard.paste_event(&mut self.rt, content, image);
            }
            Some("secure") => {
                if let serde_json::Value::String(text) = event["text"].take() {
                    self.rt.web_secure(event["key"].as_str().unwrap_or_default(), text);
                }
            }
            Some("key") => activate = self.rt.web_key(event),
            Some("a11y_press" | "a11y_link") => { let _ = self.rt.web_a11y_action(event); }
            _ => {}
        }
        // Commit within the DOM event: clipboard, link and soft-keyboard APIs need activation.
        self.rt.frame(0.0);
        self.apply_frame_requests(window);
        self.clipboard.flush(&mut self.rt);
        self.sync_input(activate);
    }

    pub(super) fn window_input(&mut self, event: &WindowEvent, window: &Arc<Window>) -> bool {
        let scale = window.scale_factor() as f32;
        let mut activate = false;
        let mut immediate = false;
        match event {
            WindowEvent::ModifiersChanged(mods) => { self.input.mods = mods.state(); return false; }
            WindowEvent::CursorMoved { position, .. } if self.input.touch.is_none() => {
                self.rt.pointer_move(position.x as f32 / scale, position.y as f32 / scale);
                self.cursor.apply(window, self.rt.cursor(), true);
            }
            WindowEvent::CursorLeft { .. } if self.input.touch.is_none() => {
                self.rt.pointer_leave();
                self.cursor.apply(window, crate::el::Cursor::Default, false);
            }
            WindowEvent::MouseInput { button, state, .. } if self.input.touch.is_none() => {
                immediate = true;
                let Some((x, y)) = self.rt.pointer() else { return false };
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => { self.rt.pointer_down(x, y); activate = true; }
                    (MouseButton::Left, ElementState::Released) => self.rt.pointer_up(x, y),
                    (MouseButton::Right, ElementState::Pressed) => self.rt.secondary_down(x, y),
                    (MouseButton::Right, ElementState::Released) => self.rt.secondary_up(x, y),
                    (&button, ElementState::Pressed) => self.rt.button_down(x, y, crate::window::pointer_button(button)),
                    (&button, ElementState::Released) => self.rt.button_up(x, y, crate::window::pointer_button(button)),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some((x, y)) = self.rt.pointer() else { return false };
                let (dx, dy) = crate::runtime::wheel_delta(*delta, scale, self.input.mods.shift_key());
                if self.input.mods.control_key() { return self.rt.pinch_wheel(x, y, dy); }
                // On a content-sized embedded canvas, vertical wheel belongs to the page unless
                // a horizontal strip is being scrolled.
                if self.input.fit_height && dx.abs() <= dy.abs() { return false; }
                let precise = matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_));
                self.rt.wheel_event(crate::WheelEvent::new(dx, dy).at(x, y).mods(modifiers(self.input.mods)).precise(precise));
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                immediate = true;
                let pressed = event.state == ElementState::Pressed;
                let mut logical = (pressed || self.rt.wants_key_up()).then(|| crate::window::map_key(&event.logical_key, self.input.mods)).flatten();
                if let Some(key) = &mut logical {
                    key.mods = modifiers(self.input.mods);
                    if key.mods.cmd && let crate::Key::Text(text) = &key.key && text != " " && text.chars().count() == 1 {
                        key.key = crate::Key::Command(text.chars().next().unwrap().to_ascii_lowercase());
                    }
                }
                activate = logical.as_ref().is_some_and(|k| k.key == crate::Key::Tab);
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    self.rt.physical_with_fallback(crate::PhysicalInput::Key { key, mods: modifiers(self.input.mods),
                        pressed: event.state == ElementState::Pressed, repeat: event.repeat }, logical);
                } else if let Some(key) = logical {
                    if pressed { self.rt.key(key); } else { self.rt.key_up(key); }
                }
            }
            WindowEvent::Touch(touch) if !self.input.fit_height => {
                let point = (touch.location.x as f32 / scale, touch.location.y as f32 / scale);
                match touch.phase {
                    TouchPhase::Started if self.input.touch.is_none() => {
                        self.input.touch = Some(Touch { id: touch.id, scroll: self.rt.web_touch_start(point.0, point.1),
                            origin: point, last: point, at: Instant::now(), velocity: 0.0, pan: false, horizontal: false });
                    }
                    TouchPhase::Moved => {
                        let Some(t) = &mut self.input.touch else { return false };
                        if t.id != touch.id { return false; }
                        let (dx, dy) = (point.0 - t.origin.0, point.1 - t.origin.1);
                        if !t.pan && dx.hypot(dy) > 8.0 {
                            t.pan = true;
                            (t.scroll, t.horizontal) = self.rt.web_touch_pan(t.origin.0, t.origin.1, dx.abs() > dy.abs(), t.scroll);
                        }
                        let horizontal = t.horizontal;
                        let along = move |p: (f32, f32)| if horizontal { p.0 } else { p.1 };
                        let now = Instant::now();
                        let dt = now.duration_since(t.at).as_secs_f32();
                        if dt > 0.0 { t.velocity = (along(point) - along(t.last)) / dt; }
                        if t.pan { if let Some(id) = t.scroll { self.rt.web_touch_move(id, along(point) - along(t.origin)); } }
                        t.last = point; t.at = now;
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.input.touch.as_ref().is_none_or(|t| t.id != touch.id) { return false; }
                        let t = self.input.touch.take().unwrap();
                        let velocity = if t.pan && touch.phase == TouchPhase::Ended && t.at.elapsed().as_millis() < 100 { t.velocity } else { 0.0 };
                        self.rt.web_touch_end(t.scroll, velocity);
                        if !t.pan && touch.phase == TouchPhase::Ended {
                            self.rt.pointer_down(point.0, point.1);
                            self.rt.pointer_up(point.0, point.1);
                            activate = true;
                            immediate = true;
                        }
                        self.rt.pointer_leave();
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        // Moves and wheels are coalesced by rAF. Only activation-sensitive input must
        // commit before the DOM handler returns (clipboard, links and keyboard focus).
        if immediate {
            self.rt.frame(0.0);
            self.apply_frame_requests(window);
            self.clipboard.flush(&mut self.rt);
            self.sync_input(activate);
            self.cursor.apply(window, self.rt.cursor(), self.rt.pointer().is_some());
        }
        true
    }
}

fn modifiers(mods: ModifiersState) -> crate::Mods {
    crate::Mods { shift: mods.shift_key(), alt: mods.alt_key(), ctrl: mods.control_key(),
        logo: mods.super_key(), cmd: mods.super_key() || mods.control_key() }
}
