//! A window host as the strobe bridge drives it. Pointer input goes in as the winit events the
//! OS sends, through the host's own `window_event`; keys, whose winit events can't be built
//! outside winit, take the same steps its keyboard branch takes.

use super::*;
use crate::strobe::{Input, Surface, protocol::{Code, Fail}};

impl<S: 'static> Host<S> {
    /// A key as `WindowEvent::KeyboardInput` delivers it, with the modifiers the host holds.
    fn strobe_key(&mut self, code: Option<winit::keyboard::KeyCode>, key: &winit::keyboard::Key, base: Option<char>, pressed: bool) {
        let Some(l) = self.win.as_ref() else { return };
        let rt = &mut self.rt;
        let logical = (pressed || rt.wants_key_up()).then(|| map_key(key, self.mods)).flatten();
        let unhandled = crate::native_menu::take_unhandled(logical.as_ref().filter(|_| pressed));
        let mut redraw = false;
        if let Some(key) = code {
            redraw = rt.physical_then(crate::PhysicalInput::Key { key, mods: map_mods(self.mods), pressed, repeat: false }, logical, base, unhandled);
        } else if let Some(ev) = logical {
            if !pressed { redraw = rt.key_up(ev); }
            else if ev.key == Key::Command('v') && rt.accepts_paste() { self.clipboard.read(rt); }
            else { rt.key(ev); }
            redraw |= pressed;
        }
        if redraw { l.window.request_redraw(); }
        self.publish();
    }
}

impl<S: 'static> Surface for Host<S> {
    fn key(&self) -> &str { self.native_handle.key() }
    fn overlay(&self) -> bool { false }

    fn describe(&self) -> serde_json::Value {
        let w = self.win.as_ref();
        let refresh = w.and_then(|w| w.window.current_monitor()).and_then(|m| m.refresh_rate_millihertz()).map(|hz| f64::from(hz) / 1000.0);
        serde_json::json!({
            "key": self.key(), "kind": "window", "hwnd": w.and_then(|w| crate::strobe::window::hwnd(&w.window)),
            "visible": !self.finished && w.is_some_and(|w| w.visible),
            "occluded": w.is_some_and(|w| w.occluded || w.minimized()),
            "size": [self.rt.viewport.0, self.rt.viewport.1], "scale": self.rt.env.scale,
            "present_mode": w.map(|w| format!("{:?}", w.config.present_mode)), "refresh_hz": refresh,
        })
    }

    fn adapter(&self) -> Option<wgpu::AdapterInfo> { self.gpu.as_ref().map(|(gpu, _)| gpu.adapter.get_info()) }
    fn counts(&self) -> crate::SurfaceCounts { self.meter.totals() }
    fn tally(&self) -> crate::strobe::Tally { self.rt.strobe_tally() }
    fn find(&self, sel: &crate::strobe::select::Selector) -> Option<[f32; 4]> {
        self.win.as_ref().filter(|w| w.visible)?;
        crate::strobe::select::find(&self.rt, sel)
    }
    fn query(&self, sel: &crate::strobe::select::Selector, all: bool) -> Vec<serde_json::Value> { crate::strobe::select::query(&self.rt, sel, all) }

    fn quiet(&self) -> bool {
        let Some(w) = self.win.as_ref().filter(|w| w.visible && !self.finished) else { return true };
        #[cfg(target_os = "windows")]
        if w.steady_frame || w.steady_until.is_some_and(|until| Instant::now() < until) { return false; }
        let _ = w;
        self.idle && !self.rt.stale() && !self.rt.live_pending() && !self.rt.has_pending_input()
            && self.frame_anim.is_none() && self.rt.window_frame.is_none()
    }

    fn deliver(&mut self, event_loop: &ActiveEventLoop, input: &Input) -> bool {
        let Some(w) = self.win.as_ref() else { return false };
        let (id, scale) = (w.window.id(), w.window.scale_factor());
        let event = match crate::strobe::window::event(input, scale) {
            Some(event) => event,
            None => {
                let Input::Key { code, key, base, pressed } = input else { return false };
                let _input = crate::input::time::arrived_now();
                self.strobe_key(*code, key, *base, *pressed);
                return true;
            }
        };
        let _input = crate::input::time::arrived(&event);
        self.window_event(event_loop, id, event);
        true
    }

    fn window(&mut self, event_loop: &ActiveEventLoop, action: &str) -> Result<(), Fail> {
        match action {
            "show" => self.native_handle.show(),
            "hide" => self.native_handle.hide(),
            "focus" => self.native_handle.focus(),
            _ => {
                // As the user closing it: the close button's event, so an app that keeps running keeps running.
                let id = self.win.as_ref().ok_or_else(|| Fail::new(Code::NoWindow, "the window isn't open"))?.window.id();
                self.window_event(event_loop, id, WindowEvent::CloseRequested);
            }
        }
        Ok(())
    }

    fn pixels(&mut self) -> Result<(u32, u32, Vec<u8>), Fail> {
        let (Some(w), Some((gpu, renderer))) = (self.win.as_ref(), self.gpu.as_ref()) else { return Err(Fail::new(Code::NoWindow, "the window isn't open")) };
        let size = (w.config.width, w.config.height);
        let px = crate::drag_out::read_pixels(gpu, renderer, size, self.rt.clear_color(), [0, 0, size.0, size.1])
            .ok_or_else(|| Fail::new(Code::Unsupported, format!("can't read back a {:?} surface", renderer.format())))?;
        Ok((size.0, size.1, px))
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.rt.shell_request = Some(Shell::Quit);
        self.shell(event_loop);
    }
}
