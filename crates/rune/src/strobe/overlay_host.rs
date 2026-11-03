//! An overlay as the strobe bridge drives it: pointer input as the winit events its window
//! hears, keys as its keyboard branch takes them (only while it takes keys on demand).

use super::*;
use crate::strobe::{Input, Surface, protocol::{Code, Fail}};

impl<T: 'static> Surface for Overlay<T> {
    fn key(&self) -> &str { self.name }
    fn overlay(&self) -> bool { true }

    fn describe(&self) -> serde_json::Value {
        let live = self.live.as_ref();
        #[allow(irrefutable_let_patterns)]
        let window = live.and_then(|l| if let Backing::Window(w) = &l.backing { Some(w) } else { None });
        let refresh = window.and_then(|w| w.current_monitor()).and_then(|m| m.refresh_rate_millihertz()).map(|hz| f64::from(hz) / 1000.0);
        serde_json::json!({
            "key": self.name, "kind": "overlay", "hwnd": window.and_then(|w| crate::strobe::window::hwnd(w)),
            "visible": self.visible, "occluded": self.is_occluded(),
            "size": [self.at.size.0, self.at.size.1], "scale": live.map_or(self.rt.env.scale, |l| l.scale),
            "present_mode": live.map(|l| format!("{:?}", l.config.present_mode)), "refresh_hz": refresh,
        })
    }

    fn adapter(&self) -> Option<wgpu::AdapterInfo> { self.gpu.as_ref().map(|gpu| gpu.adapter.get_info()) }
    fn counts(&self) -> crate::SurfaceCounts { self.meter.totals() }
    fn tally(&self) -> crate::strobe::Tally { self.rt.strobe_tally() }
    fn find(&self, sel: &crate::strobe::select::Selector) -> Option<[f32; 4]> {
        if !self.visible { return None; }
        crate::strobe::select::find(&self.rt, sel)
    }
    fn query(&self, sel: &crate::strobe::select::Selector, all: bool) -> Vec<serde_json::Value> { crate::strobe::select::query(&self.rt, sel, all) }

    fn quiet(&self) -> bool {
        !self.drawing() || (self.idle && !self.dirty && !self.rt.stale() && !self.rt.live_pending() && !self.rt.has_pending_input())
    }

    fn deliver(&mut self, event_loop: &ActiveEventLoop, input: &Input) -> bool {
        #[allow(irrefutable_let_patterns)]
        let Some(Live { backing: Backing::Window(window), scale, .. }) = self.live.as_ref() else { return false };
        let (id, scale) = (window.id(), f64::from(*scale));
        let Some(event) = crate::strobe::window::event(input, scale) else {
            let Input::Key { code, key, base, pressed } = input else { return false };
            if self.options.keys != KeyPolicy::OnDemand { return false; }
            let _input = crate::input::time::arrived_now();
            let fallback = (*pressed || self.rt.wants_key_up()).then(|| crate::window::map_key(key, self.mods)).flatten();
            if let Some(code) = *code {
                self.dirty |= self.rt.physical_then(crate::PhysicalInput::Key {
                    key: code, mods: crate::window::map_mods(self.mods), pressed: *pressed, repeat: false,
                }, fallback, *base, None);
            } else if let Some(key) = fallback {
                if !*pressed { self.dirty |= self.rt.key_up(key); } else { self.rt.key(key); self.dirty = true; }
            }
            return true;
        };
        let _input = crate::input::time::arrived(&event);
        self.window_event(event_loop, id, event);
        true
    }

    fn window(&mut self, _event_loop: &ActiveEventLoop, action: &str) -> Result<(), Fail> {
        let handle = OverlayHandle { shared: self.shared.clone() };
        match action {
            "show" => handle.show(),
            "hide" | "close" => handle.hide(),
            _ => return Err(Fail::new(Code::Unsupported, "an overlay takes no focus")),
        }
        Ok(())
    }

    fn pixels(&mut self) -> Result<(u32, u32, Vec<u8>), Fail> {
        let (Some(live), Some(renderer), Some(gpu)) = (self.live.as_ref(), self.renderer.as_ref(), self.gpu.as_ref()) else {
            return Err(Fail::new(Code::NoWindow, "the overlay isn't shown"));
        };
        let size = (live.config.width, live.config.height);
        let px = crate::drag_out::read_pixels(gpu, renderer, size, self.rt.clear_color(), [0, 0, size.0, size.1])
            .ok_or_else(|| Fail::new(Code::Unsupported, format!("can't read back a {:?} surface", renderer.format())))?;
        Ok((size.0, size.1, px))
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) { event_loop.exit(); }
}
