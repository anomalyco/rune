//! Wayland `zwp_text_input_v3` protocol dispatch and wire request execution for layer-shell
//! overlays. The protocol state machine lives in `layer_shell_machine::text_input`.

use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ContentHint, ContentPurpose, ZwpTextInputV3,
};

use super::super::layer_shell_machine::text_input::{self, Effect, Event};
use super::{Input, State};
use crate::input::host_events::ImeStep;

impl Dispatch<ZwpTextInputV3, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let ev = match event {
            zwp_text_input_v3::Event::Enter { surface } => Event::Enter {
                ours: state.surface.as_ref() == Some(&surface),
            },
            zwp_text_input_v3::Event::Leave { surface } => Event::Leave {
                ours: state.surface.as_ref() == Some(&surface),
            },
            zwp_text_input_v3::Event::PreeditString { text, cursor_begin, cursor_end } => {
                Event::Preedit { text, cursor_begin, cursor_end }
            }
            zwp_text_input_v3::Event::CommitString { text } => Event::Commit { text },
            zwp_text_input_v3::Event::Done { .. } => Event::Done,
            _ => return,
        };
        state.step_text_input(ev);
    }
}

fn apply_wire(ti: &ZwpTextInputV3, effect: &Effect) -> bool {
    match effect {
        Effect::Enable => ti.enable(),
        Effect::Disable => ti.disable(),
        Effect::SetContentTypeNormal => ti.set_content_type(ContentHint::None, ContentPurpose::Normal),
        Effect::SetCursorRectangle(rect) => ti.set_cursor_rectangle(rect.x, rect.y, rect.width, rect.height),
        Effect::Commit => ti.commit(),
        Effect::Ime(_) | Effect::ResetCompose => return false,
    }
    true
}

impl State {
    pub(super) fn step_text_input(&mut self, event: Event) {
        for effect in text_input::step(&mut self.text_input_state, event) {
            if let Some(ti) = &self.text_input && apply_wire(ti, &effect) {
                continue;
            }
            match effect {
                Effect::Ime(ime) => self.input.push(Input::Ime(ime)),
                Effect::ResetCompose => self.reset_compose(),
                Effect::Enable
                | Effect::Disable
                | Effect::SetContentTypeNormal
                | Effect::SetCursorRectangle(_)
                | Effect::Commit => {}
            }
        }
    }

    /// Applies one host `ImeStep` on `zwp_text_input_v3`, returning `(sent_wire, ime_event)` so
    /// the caller flushes only when wire requests were queued and routes any immediate
    /// `Ime::Enabled`/`Disabled` transition to the runtime.
    pub(super) fn sync_text_input(&mut self, step: ImeStep) -> (bool, Option<crate::Ime>) {
        if self.text_input.is_none() {
            return (false, None);
        }
        let mut sent = false;
        let mut out_ime = None;
        for effect in text_input::step(&mut self.text_input_state, Event::Sync(step)) {
            if let Some(ti) = &self.text_input && apply_wire(ti, &effect) {
                sent = true;
                continue;
            }
            match effect {
                Effect::Ime(ime) => out_ime = Some(ime),
                Effect::ResetCompose => self.reset_compose(),
                Effect::Enable
                | Effect::Disable
                | Effect::SetContentTypeNormal
                | Effect::SetCursorRectangle(_)
                | Effect::Commit => {}
            }
        }
        (sent, out_ime)
    }

    /// Disables an active text input while the surface is hidden and draining its queue.
    /// Returns true when wire requests were queued and need a flush.
    pub(super) fn drain_text_input(&mut self) -> bool {
        let mut sent = false;
        for effect in text_input::step(&mut self.text_input_state, Event::Drain) {
            if let Some(ti) = &self.text_input && apply_wire(ti, &effect) {
                sent = true;
            }
        }
        sent
    }

    /// Drops text-input protocol state when the connection died or proxies are tearing down.
    pub(super) fn reset_text_input_gone(&mut self) {
        let _ = text_input::step(&mut self.text_input_state, Event::Gone);
    }
}
