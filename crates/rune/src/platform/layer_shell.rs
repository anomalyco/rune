//! A Wayland layer-shell surface for overlays: `zwlr_layer_shell_v1` on winit's own display
//! connection, in a private event queue with its own seat. The surface reserves no space;
//! input regions and the rendered buffer share a commit. Global pointer observation and
//! capture exclusion are unavailable. wgpu draws through the raw `wl_surface`.

use std::os::fd::{OwnedFd, RawFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::protocol::{wl_seat::{self, WlSeat}, wl_pointer::{self, WlPointer}, wl_keyboard::{self, WlKeyboard}, wl_touch::{self, WlTouch}};
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};

use super::layer_shell_machine::{activation as activation_machine, connection, exclusive_zone, frame_callback, keyboard, pointer, readiness, repeat, text_input as text_input_machine, touch};
use crate::el::Align;
use crate::overlay::{Placement, ScreenEdge};
use crate::runtime::Waker;
use crate::{OverlayInput, OverlayOptions, OverlayPlacement};

mod activation;
mod compose;
mod dead;
mod globals;
mod sync;
mod text_input;
pub(crate) use activation::PendingActivationToken;
pub(crate) use dead::display_error;
use dead::classify_turn_error;
pub(crate) use globals::SharedGlobalsHandle;
pub(crate) use sync::{HideSync, SyncPoll};

pub(crate) enum Input {
    Move(f32, f32), Leave, Button(bool), Secondary(bool), ExtraButton(crate::PointerButton, bool),
    Wheel(crate::input::host_events::WheelInput),
    Focus(bool), Key(crate::KeyEvent), KeyRepeat(crate::KeyEvent), KeyUp(crate::KeyEvent),
    Physical(crate::PhysicalInput, Option<crate::KeyEvent>),
    Ime(crate::Ime),
}

/// Whether the pointer is over the surface: the serial of its enter, which setting a cursor
/// needs, and the cursor the surface set since (none: the one it arrived with).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Over {
    #[default]
    Outside,
    Inside { serial: u32, cursor: Option<crate::el::Cursor> },
}

impl Over {
    fn inside(self) -> bool { matches!(self, Over::Inside { .. }) }
}

#[derive(Default)]
struct State {
    /// How far the compositor has got with the surface: its first configure is what makes it
    /// drawable, and closing it is final.
    readiness: readiness::State,
    /// Tracks whether the compositor has answered the last presented frame's `wl_surface.frame`
    /// callback so the overlay never blocks in Mesa's FIFO wait when the surface isn't painted.
    frame: frame_callback::State,
    /// Pending `wl_surface.frame` callback proxy, kept so it can be destroyed on replacement,
    /// close, or drop instead of retaining a `QueueHandle` reference to the event queue.
    frame_callback: Option<(u64, WlCallback)>,
    input: Vec<Input>,
    surface: Option<WlSurface>,
    over: Over,
    keyboard_focus: keyboard::FocusState,
    want_pointer: bool,
    want_keys: bool,
    capabilities: Option<wl_seat::Capability>,
    pointer: Option<WlPointer>,
    pointer_axis: pointer::State,
    /// A finger on the surface acts as the primary button, so a tap is a click.
    touch: Option<WlTouch>,
    finger: touch::State,
    /// `wp_cursor_shape_manager_v1` when the compositor has it, and its device for `pointer`.
    shapes: Option<WpCursorShapeManagerV1>,
    shape: Option<WpCursorShapeDeviceV1>,
    keyboard: Option<WlKeyboard>,
    xkb: Option<xkbcommon::xkb::State>,
    compose: compose::Slot,
    mod_state: Option<keyboard::ModState>,
    repeat_seq: crate::machine::Seq,
    repeat: repeat::State,
    text_input: Option<ZwpTextInputV3>,
    text_input_state: text_input_machine::State,
    /// The serial of the user's last key, pointer button, touch press, or keyboard focus enter on
    /// the surface, which an activation token request presents as proof of interaction.
    input_serial: activation_machine::InputSerial,
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: wayland_client::WEnum::Value(caps) } = event {
            state.capabilities = Some(caps);
            state.update_seat(seat, qh);
        }
    }
}
impl State {
    fn close_surface(&mut self, conn: &Connection) {
        readiness::step(&mut self.readiness, readiness::Event::Closed);
        frame_callback::step(&mut self.frame, frame_callback::Event::Reset);
        if let Some((_, cb)) = self.frame_callback.take() {
            conn.backend().destroy_object(&cb.id()).ok();
        }
    }

    fn step_keyboard_focus(&mut self, event: keyboard::FocusEvent) {
        for effect in keyboard::step(&mut self.keyboard_focus, event) {
            let keyboard::FocusEffect::Focus(focused) = effect;
            self.input.push(Input::Focus(focused));
        }
    }

    fn step_repeat(&mut self, event: repeat::Event) {
        let _ = repeat::step(&mut self.repeat_seq, &mut self.repeat, event);
    }

    fn reset_compose(&mut self) {
        self.compose.reset();
    }

    fn step_pointer(&mut self, event: pointer::Event) {
        for effect in pointer::step(&mut self.pointer_axis, event) {
            let pointer::Effect::Wheel(wheel) = effect;
            self.input.push(Input::Wheel(wheel));
        }
    }

    fn flush_pointer_frame(&mut self, framed: bool) {
        let mods = self.xkb.as_ref().map_or_else(crate::Mods::default, key_mods);
        self.step_pointer(pointer::Event::Frame { mods, framed });
    }
    fn emit_key(&mut self, key: u32, pressed: bool, repeat: bool) {
        use winit::platform::scancode::PhysicalKeyExtScancode;
        let Some(xkb) = &self.xkb else { return };
        let physical = match crate::PhysicalKeyCode::from_scancode(key) {
            winit::keyboard::PhysicalKey::Code(code) => Some(code),
            _ => None,
        };
        let mods = key_mods(xkb);
        let compose_action = keyboard::compose_action(pressed, repeat, mods);
        let (text, fallback) = key_event(xkb, self.compose.for_key(compose_action), key, physical, pressed, repeat);
        if let Some(code) = physical {
            let consumed_mods = crate::window::consumed_from(false, mods, text.as_deref(), unmodified_text(xkb, key).as_deref());
            self.input.push(Input::Physical(
                crate::PhysicalInput::Key { key: code, mods, pressed, repeat, text, consumed_mods },
                fallback,
            ));
        } else if let Some(key) = fallback {
            self.input.push(match (pressed, repeat) {
                (true, true) => Input::KeyRepeat(key),
                (true, false) => Input::Key(key),
                (false, _) => Input::KeyUp(key),
            });
        }
    }

    fn fire_repeat(&mut self, attempt: crate::machine::Attempt, now: crate::time::Instant) {
        for effect in repeat::step(&mut self.repeat_seq, &mut self.repeat, repeat::Event::Due { attempt, now }) {
            match effect {
                repeat::Effect::Repeat { key } => {
                    if self.keyboard_focus.focused() {
                        self.emit_key(key, true, true);
                    }
                }
                repeat::Effect::WakeAt(_) => {}
            }
        }
    }

    fn update_seat(&mut self, seat: &WlSeat, qh: &QueueHandle<Self>) {
        let caps = self.capabilities.unwrap_or_else(wl_seat::Capability::empty);
        let pointer = self.want_pointer && caps.contains(wl_seat::Capability::Pointer);
        let keyboard = self.want_keys && caps.contains(wl_seat::Capability::Keyboard);
        if pointer && self.pointer.is_none() {
            let p = seat.get_pointer(qh, ());
            self.shape = self.shapes.as_ref().map(|shapes| shapes.get_pointer(&p, qh, ()));
            self.pointer = Some(p);
        }
        let touch = self.want_pointer && caps.contains(wl_seat::Capability::Touch);
        if touch && self.touch.is_none() { self.touch = Some(seat.get_touch(qh, ())); }
        if keyboard && self.keyboard.is_none() { self.keyboard = Some(seat.get_keyboard(qh, ())); }
        if !pointer && let Some(p) = self.pointer.take() {
            if let Some(shape) = self.shape.take() { shape.destroy(); }
            if p.version() >= 3 { p.release(); } else { destroy_proxy(&p); }
            self.over = Over::Outside;
            self.step_pointer(pointer::Event::Reset);
        }
        if !touch && let Some(t) = self.touch.take() {
            if t.version() >= 3 { t.release(); } else { destroy_proxy(&t); }
            touch::step(&mut self.finger, touch::Event::Gone);
        }
        if !keyboard && let Some(k) = self.keyboard.take() {
            if k.version() >= 3 { k.release(); } else { destroy_proxy(&k); }
            self.xkb = None;
            self.compose.clear();
            self.mod_state = None;
            self.step_repeat(repeat::Event::KeyboardGone);
            self.step_keyboard_focus(keyboard::FocusEvent::KeyboardGone);
        }
    }
}
fn pointer_axis(axis: wl_pointer::Axis) -> Option<pointer::Axis> {
    match axis {
        wl_pointer::Axis::VerticalScroll => Some(pointer::Axis::Vertical),
        wl_pointer::Axis::HorizontalScroll => Some(pointer::Axis::Horizontal),
        _ => None,
    }
}

fn pointer_axis_source(source: wl_pointer::AxisSource) -> Option<pointer::AxisSource> {
    match source {
        wl_pointer::AxisSource::Wheel => Some(pointer::AxisSource::Wheel),
        wl_pointer::AxisSource::Finger => Some(pointer::AxisSource::Finger),
        wl_pointer::AxisSource::Continuous => Some(pointer::AxisSource::Continuous),
        wl_pointer::AxisSource::WheelTilt => Some(pointer::AxisSource::WheelTilt),
        _ => None,
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, wl_pointer: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                // Until the surface sets a cursor, the pointer keeps the one it arrived with.
                let inside = state.surface.as_ref() == Some(&surface);
                state.step_pointer(pointer::Event::Reset);
                state.over = if inside { Over::Inside { serial, cursor: None } } else { Over::Outside };
                if inside { state.input.push(Input::Move(surface_x as f32, surface_y as f32)); }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } if state.over.inside() => state.input.push(Input::Move(surface_x as f32, surface_y as f32)),
            wl_pointer::Event::Leave { surface, .. } if state.surface.as_ref() == Some(&surface) => {
                state.over = Over::Outside;
                state.step_pointer(pointer::Event::Reset);
                state.input.push(Input::Leave);
            }
            wl_pointer::Event::Button { serial, button, state: wayland_client::WEnum::Value(button_state), .. } if state.over.inside() => {
                let pressed = button_state == wl_pointer::ButtonState::Pressed;
                state.input_serial.step(activation_machine::InputEvent::PointerButton { serial, inside: true, pressed });
                match pointer::button_from_linux(button) {
                    crate::PointerButton::Primary => {
                        state.input.push(Input::Button(pressed));
                        if pressed && let Some(xkb) = &state.xkb {
                            state.input.push(Input::Physical(crate::PhysicalInput::Modifiers(key_mods(xkb)), None));
                        }
                    }
                    crate::PointerButton::Secondary => state.input.push(Input::Secondary(pressed)),
                    other => state.input.push(Input::ExtraButton(other, pressed)),
                }
            }
            wl_pointer::Event::Axis { axis: wayland_client::WEnum::Value(axis), value, .. } if state.over.inside() => {
                if let Some(axis) = pointer_axis(axis) {
                    state.step_pointer(pointer::Event::Axis { axis, value });
                    if wl_pointer.version() < 5 { state.flush_pointer_frame(false); }
                }
            }
            wl_pointer::Event::AxisSource { axis_source: wayland_client::WEnum::Value(source) } if state.over.inside() => {
                if let Some(source) = pointer_axis_source(source) { state.step_pointer(pointer::Event::AxisSource(source)); }
            }
            wl_pointer::Event::AxisDiscrete { axis: wayland_client::WEnum::Value(axis), discrete } if state.over.inside() => {
                if let Some(axis) = pointer_axis(axis) { state.step_pointer(pointer::Event::AxisDiscrete { axis, discrete }); }
            }
            wl_pointer::Event::AxisValue120 { axis: wayland_client::WEnum::Value(axis), value120 } if state.over.inside() => {
                if let Some(axis) = pointer_axis(axis) { state.step_pointer(pointer::Event::AxisValue120 { axis, value120 }); }
            }
            wl_pointer::Event::AxisStop { axis: wayland_client::WEnum::Value(axis), .. } if state.over.inside() => {
                if let Some(axis) = pointer_axis(axis) { state.step_pointer(pointer::Event::AxisStop { axis }); }
            }
            wl_pointer::Event::Frame if state.over.inside() => state.flush_pointer_frame(true),
            _ => {}
        }
    }
}
impl Dispatch<WlTouch, ()> for State {
    fn event(state: &mut Self, _: &WlTouch, event: wl_touch::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use touch::Event as Touch;
        let event = match event {
            wl_touch::Event::Down { serial, surface, id, x, y, .. } if state.surface.as_ref() == Some(&surface) => {
                state.input_serial.step(activation_machine::InputEvent::TouchDown { serial, ours: true });
                Touch::Down { id, x: x as f32, y: y as f32 }
            }
            wl_touch::Event::Motion { id, x, y, .. } => Touch::Motion { id, x: x as f32, y: y as f32 },
            wl_touch::Event::Up { id, .. } => Touch::Up { id },
            wl_touch::Event::Cancel => Touch::Cancel,
            _ => return,
        };
        for effect in touch::step(&mut state.finger, event) {
            state.input.push(match effect {
                touch::Effect::Move(x, y) => Input::Move(x, y),
                touch::Effect::Press => Input::Button(true),
                touch::Effect::Release => Input::Button(false),
                touch::Effect::Leave => Input::Leave,
            });
        }
    }
}
impl Dispatch<WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use xkbcommon::xkb;
        match event {
            wl_keyboard::Event::Enter { serial, surface, .. } => {
                state.step_repeat(repeat::Event::Stop);
                state.reset_compose();
                let ours = state.surface.as_ref() == Some(&surface);
                state.input_serial.step(activation_machine::InputEvent::KeyboardEnter { serial, ours });
                let focus_event = if ours {
                    keyboard::FocusEvent::EnterOurs
                } else {
                    keyboard::FocusEvent::EnterOther
                };
                state.step_keyboard_focus(focus_event);
            }
            wl_keyboard::Event::Leave { surface, .. } if state.surface.as_ref() == Some(&surface) => {
                state.step_repeat(repeat::Event::Stop);
                state.reset_compose();
                state.step_keyboard_focus(keyboard::FocusEvent::LeaveOurs);
            }
            wl_keyboard::Event::Keymap { format: wayland_client::WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                if let Some(map) = read_keymap_fd(fd, size) {
                    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                    state.xkb = xkb::Keymap::new_from_string(&context, map, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::COMPILE_NO_FLAGS).map(|map| {
                        let mut xkb_state = xkb::State::new(&map);
                        if let Some(m) = state.mod_state {
                            xkb_state.update_mask(m.depressed, m.latched, m.locked, 0, 0, m.group);
                        }
                        xkb_state
                    });
                    state.compose.on_keymap(state.xkb.is_some());
                    if let Some(held) = state.repeat.key()
                        && !state.xkb.as_ref().is_some_and(|xkb| xkb.get_keymap().key_repeats(xkb::Keycode::new(held + 8)))
                    {
                        state.step_repeat(repeat::Event::Stop);
                    }
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                state.mod_state = Some(keyboard::ModState { depressed: mods_depressed, latched: mods_latched, locked: mods_locked, group });
                if let Some(xkb) = &mut state.xkb {
                    xkb.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    state.input.push(Input::Physical(crate::PhysicalInput::Modifiers(key_mods(xkb)), None));
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.step_repeat(repeat::Event::RepeatInfo { rate, delay_ms: delay });
            }
            wl_keyboard::Event::Key { serial, key, state: wayland_client::WEnum::Value(key_state), .. } if state.keyboard_focus.focused() => {
                match key_state {
                    wl_keyboard::KeyState::Pressed => {
                        state.input_serial.step(activation_machine::InputEvent::Key { serial, focused: true, pressed: true });
                        state.emit_key(key, true, false);
                        let repeats = state.xkb.as_ref().is_some_and(|xkb| xkb.get_keymap().key_repeats(xkb::Keycode::new(key + 8)));
                        state.step_repeat(repeat::Event::Press { key, repeats, now: crate::time::now() });
                    }
                    wl_keyboard::KeyState::Released => {
                        state.emit_key(key, false, false);
                        state.step_repeat(repeat::Event::Release { key });
                    }
                    wl_keyboard::KeyState::Repeated => {
                        state.emit_key(key, true, true);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Reads a Wayland `wl_keyboard.keymap` fd from offset `0` without touching the open file
/// description's file offset. Compositors such as Hyprland and niri share one open file
/// description across clients and binds, so sequential `read()` advances the shared offset and
/// leaves later readers at EOF.
fn read_keymap_fd(fd: OwnedFd, size: u32) -> Option<String> {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::from(fd);
    keyboard::decode_keymap(size, |buf, offset| file.read_exact_at(buf, offset))
}

fn key_mods(state: &xkbcommon::xkb::State) -> crate::Mods {
    use xkbcommon::xkb;
    let active = |name: &str| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
    crate::input::key_table::mods_from_keys(active(xkb::MOD_NAME_SHIFT), active(xkb::MOD_NAME_CTRL), active(xkb::MOD_NAME_ALT), active(xkb::MOD_NAME_LOGO))
}

/// The text `key` types in its current layout with no modifiers (its first shift level).
fn unmodified_text(state: &xkbcommon::xkb::State, key: u32) -> Option<String> {
    use xkbcommon::xkb;
    let code = xkb::Keycode::new(key + 8);
    let sym = *state.get_keymap().key_get_syms_by_level(code, state.key_get_layout(code), 0).first()?;
    Some(xkb::keysym_to_utf8(sym)).filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
}

fn feed_compose(compose: &mut xkbcommon::xkb::compose::State, sym: xkbcommon::xkb::Keysym) -> keyboard::ComposeOutcome {
    use xkbcommon::xkb::compose::{FeedResult, Status};
    match compose.feed(sym) {
        FeedResult::Ignored => keyboard::ComposeOutcome::Nothing,
        FeedResult::Accepted => match compose.status() {
            Status::Nothing => keyboard::ComposeOutcome::Nothing,
            Status::Composing => keyboard::ComposeOutcome::Composing,
            Status::Composed => {
                let text = compose.utf8();
                compose.reset();
                keyboard::ComposeOutcome::Composed(text)
            }
            Status::Cancelled => {
                compose.reset();
                keyboard::ComposeOutcome::Cancelled
            }
        },
    }
}

fn key_event(
    state: &xkbcommon::xkb::State,
    compose: Option<&mut xkbcommon::xkb::compose::State>,
    key: u32,
    physical: Option<crate::PhysicalKeyCode>,
    pressed: bool,
    repeat: bool,
) -> (Option<String>, Option<crate::KeyEvent>) {
    use xkbcommon::xkb;
    let code = xkb::Keycode::new(key + 8);
    let sym = state.key_get_one_sym(code);
    let mods = key_mods(state);
    let outcome = match (compose, keyboard::compose_action(pressed, repeat, mods)) {
        (Some(compose), keyboard::ComposeAction::Feed) => feed_compose(compose, sym),
        (Some(compose), keyboard::ComposeAction::ResetAndBypass) => {
            compose.reset();
            keyboard::ComposeOutcome::Nothing
        }
        (_, keyboard::ComposeAction::Bypass) | (None, _) => keyboard::ComposeOutcome::Nothing,
    };
    let state_utf8 = state.key_get_utf8(code);
    let sym_utf8 = xkb::keysym_to_utf8(sym);
    let text = keyboard::physical_text(pressed, &outcome, &state_utf8, &sym_utf8);
    let logical = keyboard::key_event_with_compose(outcome, named_keysym(sym.raw()), xkb::keysym_to_utf32(sym), state_utf8, mods, physical);
    (text, logical)
}

/// The named key an XKB keysym is, or None for one that types (or nothing Rune names).
fn named_keysym(symbol: u32) -> Option<crate::Key> {
    Some(match symbol {
        0xff1b => crate::Key::Escape, 0xff0d | 0xff8d => crate::Key::Enter, 0xff08 => crate::Key::Backspace,
        0xffff | 0xff9f => crate::Key::Delete, 0xff09 | 0xfe20 | 0xff89 => crate::Key::Tab,
        0xff50 | 0xff95 => crate::Key::Home, 0xff57 | 0xff9c => crate::Key::End,
        0xff55 | 0xff9a => crate::Key::PageUp, 0xff56 | 0xff9b => crate::Key::PageDown,
        0xff51 | 0xff96 => crate::Key::Left, 0xff52 | 0xff97 => crate::Key::Up,
        0xff53 | 0xff98 => crate::Key::Right, 0xff54 | 0xff99 => crate::Key::Down,
        0xffbe..=0xffd5 => crate::Key::Function((symbol - 0xffbe + 1) as u8),
        0xff91..=0xff94 => crate::Key::Function((symbol - 0xff91 + 1) as u8),
        0xff67 => crate::Key::ContextMenu,
        0xff63 | 0xff9e => crate::Key::Insert, 0xff61 | 0xff15 => crate::Key::PrintScreen,
        0xff14 => crate::Key::ScrollLock, 0xff13 | 0xff6b => crate::Key::Pause,
        0x1008_ff14 => crate::Key::Media(crate::MediaKey::Play),
        0x1008_ff31 => crate::Key::Media(crate::MediaKey::Pause),
        0x1008_ff17 => crate::Key::Media(crate::MediaKey::Next),
        0x1008_ff16 => crate::Key::Media(crate::MediaKey::Previous),
        0x1008_ff15 => crate::Key::Media(crate::MediaKey::Stop),
        0x1008_ff97 => crate::Key::Media(crate::MediaKey::FastForward),
        0x1008_ff3e => crate::Key::Media(crate::MediaKey::Rewind),
        _ => return None,
    })
}

impl Dispatch<WlRegion, ()> for State {
    fn event(_: &mut Self, _: &WlRegion, _: <WlRegion as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpCursorShapeDeviceV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeDeviceV1, _: <WpCursorShapeDeviceV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// The cursor-shape-v1 shape for `c`, as `os_cursor` maps winit's. An image is the arrow.
fn shape(c: crate::el::Cursor) -> Shape {
    use crate::el::Cursor;
    match c {
        Cursor::Default | Cursor::Hidden | Cursor::Image(_) => Shape::Default,
        Cursor::Pointer => Shape::Pointer,
        Cursor::Text => Shape::Text,
        Cursor::Grab => Shape::Grab,
        Cursor::Grabbing => Shape::Grabbing,
        Cursor::NotAllowed => Shape::NotAllowed,
        Cursor::Move => Shape::Move,
        Cursor::ResizeColumn => Shape::ColResize,
        Cursor::ResizeRow => Shape::RowResize,
        Cursor::ResizeEw => Shape::EwResize,
        Cursor::ResizeNs => Shape::NsResize,
        Cursor::ResizeNwse => Shape::NwseResize,
        Cursor::ResizeNesw => Shape::NeswResize,
        Cursor::ResizeN => Shape::NResize,
        Cursor::ResizeE => Shape::EResize,
        Cursor::ResizeS => Shape::SResize,
        Cursor::ResizeW => Shape::WResize,
        Cursor::Alias => Shape::Alias,
        Cursor::Copy => Shape::Copy,
        Cursor::Crosshair => Shape::Crosshair,
        Cursor::Cell => Shape::Cell,
        Cursor::ZoomIn => Shape::ZoomIn,
        Cursor::ZoomOut => Shape::ZoomOut,
        Cursor::Help => Shape::Help,
        Cursor::Progress => Shape::Progress,
    }
}
impl Dispatch<WlCallback, u64> for State {
    fn event(state: &mut Self, _: &WlCallback, event: wl_callback::Event, id: &u64, _: &Connection, _: &QueueHandle<Self>) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            if state.frame_callback.as_ref().is_some_and(|(pending, _)| pending == id) {
                state.frame_callback = None;
            }
            frame_callback::step(&mut state.frame, frame_callback::Event::Done(*id));
        }
    }
}
impl Dispatch<WlSurface, ()> for State {
    fn event(state: &mut Self, _: &WlSurface, event: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            readiness::step(&mut state.readiness, readiness::Event::Scale(factor.max(1)));
        }
    }
}
impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(state: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), conn: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                readiness::step(&mut state.readiness, readiness::Event::Configure { width, height });
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.close_surface(conn);
            }
            _ => {}
        }
    }
}

/// Wakes the event loop when an XWayland overlay's own X connection becomes readable.
/// Wayland layer surfaces wake through winit's `set_wake_on_wayland_read` instead of a
/// background reader thread.
pub(crate) struct Watch {
    stop: Arc<AtomicBool>,
    /// How many times the overlay has dispatched, so the watcher waits for the loop to read
    /// what woke it instead of spinning on a socket that stays readable until then.
    turns: Arc<(Mutex<u64>, Condvar)>,
    #[cfg(test)]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    #[allow(unsafe_code)]
    pub(crate) fn spawn(fd: RawFd, wake: Waker) -> Watch {
        let stop = Arc::new(AtomicBool::new(false));
        let turns = Arc::new((Mutex::new(0u64), Condvar::new()));
        let (stopped, seen) = (stop.clone(), turns.clone());
        let thread = std::thread::Builder::new().name("rune-layer-watch".into()).spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                // SAFETY: one pollfd for a display socket. winit's outlives the event loop; an
                // XWayland surface drops its watch before closing its connection, so at worst a
                // reused descriptor wakes the loop once more before this thread stops.
                let ret = unsafe { libc::poll(&mut poll, 1, 250) };
                let result = if ret < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        connection::WatchPoll::Interrupted
                    } else {
                        connection::WatchPoll::Error
                    }
                } else if ret == 0 {
                    connection::WatchPoll::TimedOut
                } else {
                    connection::WatchPoll::Ready {
                        readable: poll.revents & libc::POLLIN != 0,
                        hangup_or_error: poll.revents & (libc::POLLHUP | libc::POLLERR) != 0,
                        invalid: poll.revents & libc::POLLNVAL != 0,
                    }
                };
                match connection::watch_action(result) {
                    connection::WatchAction::Continue => {}
                    connection::WatchAction::Stop => break,
                    connection::WatchAction::WakeAndStop => {
                        if !stopped.load(Ordering::Acquire) { wake(); }
                        break;
                    }
                    connection::WatchAction::Wake => {
                        let (count, turned) = &*seen;
                        let before = *count.lock().unwrap_or_else(|e| e.into_inner());
                        wake();
                        turned.wait_timeout_while(
                            count.lock().unwrap_or_else(|e| e.into_inner()),
                            Duration::from_millis(100),
                            |n| *n == before && !stopped.load(Ordering::Acquire),
                        ).ok();
                    }
                }
            }
        }).ok();
        #[cfg(not(test))]
        drop(thread);
        Watch {
            stop,
            turns,
            #[cfg(test)]
            thread,
        }
    }

    #[cfg(test)]
    pub(crate) fn join(mut self) {
        if let Some(handle) = self.thread.take() {
            handle.join().ok();
        }
    }

    pub(crate) fn turned(&self) {
        let (count, turned) = &*self.turns;
        *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        turned.notify_all();
    }
}

impl Drop for Watch {
    /// The thread notices immediately when parked, or within one poll timeout; joining would
    /// stall hiding.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let (count, turned) = &*self.turns;
        let _guard = count.lock().unwrap_or_else(|e| e.into_inner());
        turned.notify_all();
    }
}

/// What the overlay needs to know after the compositor talked to the surface.
pub(crate) struct Changes {
    pub size: Option<(u32, u32)>,
    pub input: Vec<Input>,
    pub scale: Option<i32>,
    pub closed: bool,
}

pub(crate) struct LayerSurface {
    namespace: String,
    regions: std::cell::RefCell<Option<Vec<[i32; 4]>>>,
    seat: Option<WlSeat>,
    options: OverlayOptions,
    queue: EventQueue<State>,
    state: State,
    layer: ZwlrLayerSurfaceV1,
    surface: WlSurface,
    display: NonNull<std::ffi::c_void>,
    seen_drain_gen: u64,
    /// Registry global name of the `wl_output` this layer surface was bound to, if targeted.
    /// Removing that global ends the surface cleanly even if the compositor sends no `closed`
    /// before the initial `configure`.
    output_name: Option<u32>,
    /// Why the surface closed when a fatal display error ended it; `None` for compositor `closed`.
    close_reason: Option<String>,
    /// Shared per-display registry and singleton globals; declared last so per-surface fields drop
    /// before releasing our reference to the shared connection.
    globals: Arc<globals::SharedGlobals>,
}

fn destroy_proxy<P: Proxy>(proxy: &P) {
    if let Some(backend) = proxy.backend().upgrade() {
        backend.destroy_object(&proxy.id()).ok();
    }
}

fn drain_event_queue(queue: &mut EventQueue<State>, state: &mut State) {
    loop {
        match queue.dispatch_pending(state) {
            Ok(0) => break,
            Ok(_) | Err(DispatchError::BadMessage { .. }) => {}
            Err(_) => break,
        }
    }
}

fn wayland_interactivity(mode: keyboard::Interactivity) -> KeyboardInteractivity {
    match mode {
        keyboard::Interactivity::None => KeyboardInteractivity::None,
        keyboard::Interactivity::Exclusive => KeyboardInteractivity::Exclusive,
        keyboard::Interactivity::OnDemand => KeyboardInteractivity::OnDemand,
    }
}

impl LayerSurface {
    /// Creates and commits a Wayland layer-shell surface without waiting for the compositor's
    /// initial `configure`. The caller holds it without a GPU surface until `dispatch()`
    /// delivers the first `configure` (`is_configured()`), a compositor `closed` / output
    /// removal, or a fatal display error. When `hold_keys` is true and `options.keys` is supported
    /// by the bound `zwlr_layer_shell_v1` version, the surface requests keyboard interactivity in
    /// its initial commit so a recreated surface never maps without the keyboard. Returns `None`
    /// when the display isn't Wayland, the compositor lacks layer-shell, or the display connection
    /// fails.
    pub fn new(display: RawDisplayHandle, namespace: &str, at: Placement, interactive: bool, options: &OverlayOptions, output_id: Option<u32>, hold_keys: bool) -> Option<LayerSurface> {
        let RawDisplayHandle::Wayland(display) = display else { return None };
        let want_keys = options.keys.takes_keys();
        if want_keys { compose::warm(); }
        let globals = globals::SharedGlobals::acquire(display, namespace)?;
        let queue = globals.conn().new_event_queue::<State>();
        let qh = queue.handle();
        let output = output_id.and_then(|id| globals.output(id));
        let output_name = output.as_ref().and(output_id);
        let want_pointer = options.input.resolved(interactive) != OverlayInput::None;
        let seat = if want_pointer || want_keys { globals.bind_seat(&qh) } else { None };
        let surface = globals.compositor().create_surface(&qh, ());
        let region = globals.compositor().create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        // Preserve existing overlay-layer semantics. There is no layer above Overlay;
        // relative ordering of peers is compositor policy, unlike AppKit's numeric levels.
        let layer = globals.shell().get_layer_surface(&surface, output.as_ref(), Layer::Overlay, namespace.to_owned(), &qh, ());
        layer.set_keyboard_interactivity(wayland_interactivity(keyboard::interactivity(options.keys, layer.version(), hold_keys)));
        let shapes = globals.cursor_shape_manager();
        let text_input = want_keys
            .then(|| globals.text_input_manager())
            .flatten()
            .zip(seat.as_ref())
            .map(|(manager, seat)| manager.get_text_input(seat, &qh, ()));
        let state = State {
            surface: Some(surface.clone()),
            want_pointer,
            want_keys,
            shapes,
            text_input,
            ..State::default()
        };
        let seen_drain_gen = globals.inner_drain_gen();
        let mut this = LayerSurface {
            namespace: namespace.to_owned(),
            regions: std::cell::RefCell::new(Some(Vec::new())),
            seat,
            options: options.clone(),
            queue,
            state,
            layer,
            surface,
            display: display.display,
            seen_drain_gen,
            output_name,
            close_reason: None,
            globals,
        };
        this.place(at);
        this.surface.commit();
        this.flush_checked();
        if this.state.readiness.closed() { return None; }
        Some(this)
    }

    /// Returns a lightweight handle that keeps this display's shared layer-shell globals alive
    /// across overlay hide/show cycles without retaining any per-surface resources.
    pub(crate) fn globals_handle(&self) -> SharedGlobalsHandle {
        self.globals.handle()
    }

    pub fn set_placement(&mut self, placement: Option<OverlayPlacement>) {
        self.options.placement = placement;
    }

    pub fn place(&mut self, at: Placement) {
        self.layer.set_exclusive_zone(exclusive_zone(self.options.placement));
        match self.options.placement {
            Some(OverlayPlacement::FullScreen) => {
                self.layer.set_size(0, 0);
                self.layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
                self.layer.set_margin(0, 0, 0, 0);
                return;
            }
            Some(OverlayPlacement::Rect { x, y, .. }) => {
                self.layer.set_size(at.size.0.ceil().max(1.0) as u32, at.size.1.ceil().max(1.0) as u32);
                self.layer.set_anchor(Anchor::Top | Anchor::Left);
                self.layer.set_margin(y.round() as i32, 0, 0, x.round() as i32);
                return;
            }
            _ => {}
        }
        let at = match self.options.placement {
            Some(OverlayPlacement::Edge { edge, margin, align, inset }) => Placement { size: at.size, edge, margin, align, inset },
            _ => at,
        };
        let (margin, inset) = (at.margin.round() as i32, at.inset.round() as i32);
        let (vertical, top, bottom) = match at.edge {
            ScreenEdge::Top => (Anchor::Top, margin, 0),
            ScreenEdge::Bottom => (Anchor::Bottom, 0, margin),
        };
        let (horizontal, left, right) = match at.align {
            Align::Start => (Anchor::Left, inset, 0),
            Align::Center => (Anchor::empty(), 0, 0),
            Align::End => (Anchor::Right, 0, inset),
        };
        self.layer.set_size(at.size.0.ceil() as u32, at.size.1.ceil() as u32);
        self.layer.set_anchor(vertical | horizontal);
        self.layer.set_margin(top, right, bottom, left);
    }

    /// True once the compositor has sent the initial `configure` and the surface is still open.
    pub fn is_configured(&self) -> bool { self.state.readiness.is_ready() }

    /// Sets the surface's pending input region; true when it changed, so it waits for a commit.
    pub fn input_regions(&self, regions: &[[f32; 4]], interactive: bool) -> bool {
        let next = match self.options.input.resolved(interactive) {
            OverlayInput::All => None,
            OverlayInput::Regions => Some(regions.iter().filter_map(|r| {
                let (x, y) = (r[0].floor() as i32, r[1].floor() as i32);
                let (w, h) = (r[2].ceil() as i32 - x, r[3].ceil() as i32 - y);
                (w > 0 && h > 0).then_some([x, y, w, h])
            }).collect()),
            _ => Some(Vec::new()),
        };
        if *self.regions.borrow() == next { return false; }
        if let Some(rects) = &next {
            let region = self.globals.compositor().create_region(&self.queue.handle(), ());
            for &[x, y, w, h] in rects { region.add(x, y, w, h); }
            self.surface.set_input_region(Some(&region));
            region.destroy();
        } else {
            self.surface.set_input_region(None);
        }
        *self.regions.borrow_mut() = next;
        // No commit here: a drawn frame's presentation commits input and pixels together, and a
        // frame that draws nothing commits it alone (`Overlay::unchanged_frame`).
        true
    }

    pub fn set_input(&mut self, input: OverlayInput) {
        self.options.input = input;
        self.state.want_pointer = input != OverlayInput::None;
        if self.state.want_pointer && self.state.shapes.is_none() {
            self.state.shapes = self.globals.cursor_shape_manager();
        }
        if self.seat.is_none() && self.state.want_pointer {
            self.seat = self.globals.bind_seat(&self.queue.handle());
        }
        if let Some(seat) = &self.seat { self.state.update_seat(seat, &self.queue.handle()); }
    }

    /// Asks for keyboard interactivity: exclusive for `KeyPolicy::Exclusive`, else on demand.
    /// True when an earlier `release_keys` left the compositor's focus on our surface (no
    /// `wl_keyboard.leave` came), so the overlay is focused again at once, for the caller to
    /// deliver; otherwise focus follows the compositor's `enter`.
    pub fn request_keys(&mut self) -> bool {
        let mode = keyboard::interactivity(self.options.keys, self.layer.version(), true);
        if mode == keyboard::Interactivity::None { return false; }
        self.layer.set_keyboard_interactivity(wayland_interactivity(mode));
        self.surface.commit();
        self.queue.flush().ok();
        let effects = keyboard::step(&mut self.state.keyboard_focus, keyboard::FocusEvent::Requested);
        effects.into_iter().any(|keyboard::FocusEffect::Focus(focused)| focused)
    }

    /// Sets keyboard interactivity back to none on the mapped layer, for `release_keys`. Some
    /// compositors send no `wl_keyboard.leave` for it, so the surface stops repeat and drops its
    /// focus itself (`FocusState::Released`); a leave that does arrive later changes nothing,
    /// and a later `request_keys` refocuses if none came. True when it held the keys, for the
    /// caller to deliver the blur at once.
    pub fn release_keys(&mut self) -> bool {
        if !self.options.keys.takes_keys() { return false; }
        self.layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        self.surface.commit();
        self.queue.flush().ok();
        self.state.step_repeat(repeat::Event::Stop);
        self.state.reset_compose();
        let effects = keyboard::step(&mut self.state.keyboard_focus, keyboard::FocusEvent::Released);
        effects.into_iter().any(|keyboard::FocusEffect::Focus(focused)| !focused)
    }

    /// Starts a non-blocking XDG activation token request for a program launched from the overlay
    /// and returns immediately without waiting in a compositor roundtrip; see `activation`.
    pub fn start_activation_token(&self) -> Option<PendingActivationToken> {
        if self.state.readiness.closed() { return None; }
        PendingActivationToken::start(
            &self.globals,
            &self.surface,
            self.seat.as_ref().zip(self.state.input_serial.get()),
            self.state.readiness.closed(),
        )
    }

    pub fn supports_keys(&self) -> bool {
        keyboard::supports_keys(self.options.keys, self.layer.version(), self.seat.is_some())
    }

    pub fn has_keyboard_focus(&self) -> bool {
        self.state.keyboard_focus.focused()
    }

    /// Synchronizes `zwp_text_input_v3` enablement and cursor rectangle with `step`.
    pub fn sync_ime(&mut self, step: crate::input::host_events::ImeStep) -> Option<crate::Ime> {
        if self.state.readiness.closed() { return None; }
        let (sent, ime) = self.state.sync_text_input(step);
        if sent { self.flush_checked(); }
        ime
    }

    /// Shows `want` while the pointer is over the surface, once per change. Without
    /// cursor-shape-v1 only `Cursor::Hidden` applies; others keep the cursor the pointer
    /// arrived with, such as a terminal's I-beam.
    pub fn set_cursor(&mut self, want: crate::el::Cursor) {
        let (Over::Inside { serial, cursor }, Some(pointer)) = (self.state.over, &self.state.pointer) else { return };
        if cursor == Some(want) { return; }
        match (want, &self.state.shape) {
            (crate::el::Cursor::Hidden, _) => pointer.set_cursor(serial, None, 0, 0),
            (want, Some(device)) => device.set_shape(serial, shape(want)),
            (_, None) => return,
        }
        self.state.over = Over::Inside { serial, cursor: Some(want) };
        self.flush_checked();
    }

    /// Requests a `wl_surface.frame` callback on this surface's private queue without committing,
    /// so it commits alongside the buffer in the immediately following `queue.present`.
    pub fn request_frame(&mut self) {
        if self.state.readiness.closed() { return; }
        for effect in frame_callback::step(&mut self.state.frame, frame_callback::Event::Request) {
            let frame_callback::Effect::Request(id) = effect;
            if let Some((_, prev)) = self.state.frame_callback.take() {
                self.globals.conn().backend().destroy_object(&prev.id()).ok();
            }
            let cb = self.surface.frame(&self.queue.handle(), id);
            self.state.frame_callback = Some((id, cb));
        }
    }

    /// True when no `wl_surface.frame` callback is outstanding, so acquiring and presenting will
    /// not block in Mesa's FIFO wait.
    pub fn frame_ready(&self) -> bool {
        self.state.frame.ready()
    }

    /// Commit role state (placement) without a new buffer; wgpu's present commits buffers.
    pub fn commit(&mut self) {
        self.surface.commit();
        self.flush_checked();
    }

    pub fn set_buffer_scale(&mut self, scale: i32) {
        if self.surface.version() >= 3 {
            self.surface.set_buffer_scale(scale);
        }
    }

    /// True while the surface has not been closed by the compositor or a fatal display error.
    pub fn is_live(&self) -> bool {
        !self.state.readiness.closed()
    }

    /// Why the surface closed when a fatal display error ended it; `None` for a clean compositor
    /// `zwlr_layer_surface_v1.closed` event.
    pub fn close_reason(&self) -> Option<&str> {
        self.close_reason.as_deref()
    }

    /// True when the surface is still live and libwayland's private event queue already holds
    /// unread events (for example from a mid-turn `wl_display` read during swapchain acquire,
    /// present, or a roundtrip). Checks the queue in memory without syscalls by immediately
    /// dropping any prepared-read guard.
    pub fn has_pending_events(&self) -> bool {
        !self.state.readiness.closed()
            && self.globals.has_pending_events(self.seen_drain_gen, self.queue.prepare_read().is_none())
    }

    /// Next held-key repeat deadline for the overlay's wake timer, or `None` when idle.
    pub fn repeat_deadline(&self) -> Option<crate::time::Instant> {
        self.state.repeat.deadline()
    }

    /// Handles what the compositor sent since the last call (winit reads the socket) and fires
    /// any due held-key repeat.
    pub fn dispatch(&mut self) -> Changes {
        let armed = self.state.repeat.attempt();
        self.turn();
        if !self.state.readiness.closed() && let Some(attempt) = armed {
            self.state.fire_repeat(attempt, crate::time::now());
        }
        Changes {
            size: self.state.readiness.take_size(),
            input: std::mem::take(&mut self.state.input),
            scale: self.state.readiness.take_scale(),
            closed: self.state.readiness.closed(),
        }
    }

    /// Handles what the compositor sent to a hidden surface and drops its input, keeping size
    /// and scale for the next `dispatch`; true once the compositor closed it. libwayland frees
    /// an undispatched event without closing its fd, so a queue left alone leaks every
    /// `wl_keyboard.keymap` (each virtual keyboard a paste tool makes sends a few).
    pub fn drain(&mut self) -> bool {
        self.turn();
        self.state.step_repeat(repeat::Event::Stop);
        self.state.reset_compose();
        self.state.step_pointer(pointer::Event::Reset);
        if !self.state.readiness.closed() && self.state.drain_text_input() {
            self.flush_checked();
        }
        self.state.input.clear();
        self.state.readiness.closed()
    }

    fn turn(&mut self) {
        let was_closed = self.state.readiness.closed();
        if was_closed {
            return;
        }
        let inner_err = self.globals.dispatch_inner_queue().err().map(DispatchError::Backend);
        let dispatch_err = self.queue.dispatch_pending(&mut self.state).err().or(inner_err);
        self.seen_drain_gen = self.globals.inner_drain_gen();
        if !self.state.readiness.closed()
            && let Some(id) = self.output_name
            && !self.globals.has_output(id)
        {
            self.state.close_surface(self.globals.conn());
        }
        let flush_err = self.queue.flush().err();
        let last_err = self.globals.conn().backend().last_error();
        let (kind, reason) = classify_turn_error(dispatch_err, flush_err, last_err);
        match connection::turn_action(was_closed, kind, self.state.readiness.closed()) {
            connection::TurnAction::Continue
            | connection::TurnAction::StopClosed
            | connection::TurnAction::AlreadyClosed => {}
            connection::TurnAction::FailAndClose => self.fail_connection(reason),
        }
    }

    fn flush_checked(&mut self) {
        if self.state.readiness.closed() { return; }
        let flush_err = self.queue.flush().err();
        let (kind, reason) = classify_turn_error(None, flush_err, self.globals.conn().backend().last_error());
        if kind.is_some_and(connection::is_fatal) {
            self.fail_connection(reason);
        }
    }

    fn fail_connection(&mut self, reason: Option<String>) {
        let was_closed = self.state.readiness.closed();
        readiness::step(&mut self.state.readiness, readiness::Event::Closed);
        frame_callback::step(&mut self.state.frame, frame_callback::Event::Reset);
        if let Some((_, cb)) = self.state.frame_callback.take() {
            self.globals.conn().backend().destroy_object(&cb.id()).ok();
        }
        self.state.step_repeat(repeat::Event::Stop);
        self.state.reset_text_input_gone();
        self.state.input.clear();
        if !was_closed {
            let msg = match reason {
                Some(reason) => format!("the display connection failed: {reason}"),
                None => "the display connection failed".to_owned(),
            };
            eprintln!("rune: Wayland layer surface '{}' closed: {msg}", self.namespace);
            self.close_reason = Some(msg);
        }
    }

    pub fn raw_handles(&self) -> (RawDisplayHandle, RawWindowHandle) {
        let surface = NonNull::new(self.surface.id().as_ptr().cast()).expect("live wl_surface");
        (
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.display)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
        )
    }
}

impl Drop for LayerSurface {
    fn drop(&mut self) {
        self.state.want_pointer = false;
        self.state.want_keys = false;
        self.state.surface = None;
        self.state.input_serial.step(activation_machine::InputEvent::Reset);
        self.state.reset_text_input_gone();
        // Drain pending events before destroying proxies so any queued `wl_keyboard.keymap` fd is
        // received and closed via `OwnedFd` rather than dropped open by libwayland.
        let _ = self.globals.dispatch_inner_queue();
        drain_event_queue(&mut self.queue, &mut self.state);
        let backend = self.globals.conn().backend();
        if let Some((_, cb)) = self.state.frame_callback.take() { backend.destroy_object(&cb.id()).ok(); }
        if let Some(ti) = self.state.text_input.take() { ti.destroy(); }
        if let Some(shape) = self.state.shape.take() { shape.destroy(); }
        // `shapes` (`wp_cursor_shape_manager_v1`) is owned by `SharedGlobals`: drop the handle
        // without destroying the shared global.
        self.state.shapes = None;
        if let Some(pointer) = self.state.pointer.take() {
            if pointer.version() >= 3 { pointer.release(); } else { backend.destroy_object(&pointer.id()).ok(); }
        }
        if let Some(touch) = self.state.touch.take() {
            if touch.version() >= 3 { touch.release(); } else { backend.destroy_object(&touch.id()).ok(); }
        }
        if let Some(keyboard) = self.state.keyboard.take() {
            if keyboard.version() >= 3 { keyboard.release(); } else { backend.destroy_object(&keyboard.id()).ok(); }
        }
        if let Some(seat) = self.seat.take() {
            if seat.version() >= 5 { seat.release(); } else { backend.destroy_object(&seat.id()).ok(); }
        }
        self.layer.destroy();
        self.surface.destroy();
        // Drain again after destroying every per-surface proxy so `EventQueueInner` holds no
        // queued events (each retains `QueueHandle` in a cycle) and no proxy can enqueue more.
        let _ = self.globals.dispatch_inner_queue();
        drain_event_queue(&mut self.queue, &mut self.state);
        self.queue.flush().ok();
    }
}

#[cfg(test)]
#[path = "layer_shell/configure_tests.rs"]
mod configure_tests;

#[cfg(test)]
#[path = "layer_shell_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "layer_shell/keys_tests.rs"]
mod keys_tests;
