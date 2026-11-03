//! The bridge on a native event loop: connecting once the first windows exist, serving requests
//! when the reader wakes the loop, and advancing jobs at the end of each turn, when frames have
//! landed. A job's next deadline joins the loop's own, so waiting never polls.

use std::cell::RefCell;
use std::time::Duration;

use serde_json::{Value, json};
use winit::event::{DeviceId, ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};

use super::driver::{Backend, Driver, Input, Poll, SurfaceTally};
use super::protocol::{Code, Fail};
use super::{Endpoint, Link, Surface, clock, record, select::Selector};
use crate::time::Instant;
use crate::window::{AnyHost, WakeEvent};

struct Bridge {
    driver: Driver,
    link: Option<Link>,
    /// Surfaces alive at the last turn, to report the ones that close.
    alive: Vec<String>,
    proxy: EventLoopProxy<WakeEvent>,
    /// When the waiting job next needs a look, and the presents it last saw.
    next_at: Option<u64>,
    seen: u64,
}

thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

type Hosts = Vec<Option<Box<dyn AnyHost>>>;

/// Connects when `STROBE_BRIDGE` is set. Called once the first windows exist.
pub(crate) fn connect(proxy: &EventLoopProxy<WakeEvent>) {
    let Some(endpoint) = Endpoint::from_env() else { return };
    if BRIDGE.with(|b| b.borrow().is_some()) { return; }
    let wake = proxy.clone();
    match Link::open(&endpoint, "window", Box::new(move || { let _ = wake.send_event(WakeEvent::Strobe); })) {
        Ok((link, requests)) => {
            record::install();
            let driver = Driver::new(requests, link.out.clone());
            let bridge = Bridge { driver, link: Some(link), alive: Vec::new(), proxy: proxy.clone(), next_at: None, seen: 0 };
            BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));
        }
        Err(error) => eprintln!("rune: {error}"),
    }
}

fn with(f: impl FnOnce(&mut Bridge)) {
    BRIDGE.with(|b| if let Some(bridge) = b.borrow_mut().as_mut() { f(bridge) });
}

/// The reader queued a request (or the connection closed).
pub(crate) fn serve(hosts: &mut Hosts, event_loop: &ActiveEventLoop) {
    with(|bridge| poll(bridge, hosts, event_loop));
}

pub(crate) fn turn_begin() { record::turn_begin(); }

/// The turn's work is done: closes its wall time and reports closed surfaces. A job that can
/// go on (a frame landed, or its time came) goes on in a wake the turn sends, not here: a
/// redraw an input asks for while the loop is about to wait waits for the loop's next wake on
/// Windows. Returns the loop's deadline, joined with the job's.
pub(crate) fn turn_end(hosts: &mut Hosts, _event_loop: &ActiveEventLoop, deadline: Option<Instant>) -> Option<Instant> {
    if !record::live() { return deadline; }
    record::turn_end();
    let mut wake = None;
    with(|bridge| {
        record::bridge_begin();
        let live = |h: &&mut Box<dyn AnyHost>| !h.finished();
        let count = hosts.iter_mut().flatten().filter(live).filter(|h| h.strobe_ref().is_some()).count();
        let changed = count != bridge.alive.len()
            || bridge.alive.iter().any(|k| !hosts.iter_mut().flatten().filter(live).any(|h| h.strobe_ref().is_some_and(|s| s.key() == k)));
        if changed {
            let alive: Vec<String> = hosts.iter_mut().flatten().filter(live).filter_map(|h| h.strobe_ref().map(|s| s.key().to_owned())).collect();
            for gone in bridge.alive.iter().filter(|k| !alive.contains(k)) { bridge.driver.send(super::protocol::closed(gone)); }
            bridge.alive = alive;
        }
        if bridge.driver.busy() {
            let now = clock::now_ns();
            let due = bridge.next_at.is_none_or(|at| now >= at);
            if due || record::presents() != bridge.seen {
                let _ = bridge.proxy.send_event(WakeEvent::Strobe);
            } else if let Some(at) = bridge.next_at {
                wake = Some(Instant::now() + Duration::from_nanos(at - now));
            }
        }
        record::bridge_end();
    });
    match (deadline, wake) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn poll(bridge: &mut Bridge, hosts: &mut Hosts, event_loop: &ActiveEventLoop) {
    record::bridge_begin();
    let poll = bridge.driver.poll(&mut Windows { hosts, event_loop });
    bridge.seen = record::presents();
    bridge.next_at = match poll { Poll::Wait(at) => Some(at), Poll::Idle => None };
    record::bridge_end();
}

/// The loop is ending: says so and flushes what's queued.
pub(crate) fn exiting() {
    BRIDGE.with(|b| if let Some(mut bridge) = b.borrow_mut().take() && let Some(link) = bridge.link.take() {
        link.finish(super::protocol::quit());
    });
}

/// The winit event an OS sends for `input`, or None for a key (winit can't build those).
pub(crate) fn event(input: &Input, scale: f64) -> Option<WindowEvent> {
    let device_id = DeviceId::dummy();
    let at = |x: f32, y: f32| winit::dpi::PhysicalPosition::new(f64::from(x) * scale, f64::from(y) * scale);
    Some(match *input {
        Input::Move(x, y) => WindowEvent::CursorMoved { device_id, position: at(x, y) },
        Input::Button(button, pressed) => WindowEvent::MouseInput {
            device_id, state: if pressed { ElementState::Pressed } else { ElementState::Released },
            button: match button {
                crate::PointerButton::Secondary => MouseButton::Right,
                crate::PointerButton::Middle => MouseButton::Middle,
                crate::PointerButton::Back => MouseButton::Back,
                crate::PointerButton::Forward => MouseButton::Forward,
                crate::PointerButton::Other(n) => MouseButton::Other(n),
                _ => MouseButton::Left,
            },
        },
        Input::Wheel(dx, dy) => WindowEvent::MouseWheel { device_id, delta: MouseScrollDelta::PixelDelta(at(dx, dy)), phase: TouchPhase::Moved },
        Input::Mods(mods) => WindowEvent::ModifiersChanged(mods.into()),
        Input::Key { .. } => return None,
    })
}

/// The window's HWND on Windows.
pub(crate) fn hwnd(window: &winit::window::Window) -> Option<i64> {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as i64),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    { let _ = window; None }
}

/// The event loop's hosts as the driver sees them.
struct Windows<'a> {
    hosts: &'a mut Hosts,
    event_loop: &'a ActiveEventLoop,
}

impl Windows<'_> {
    fn surfaces(&mut self) -> impl Iterator<Item = &mut dyn Surface> {
        self.hosts.iter_mut().flatten().filter(|h| !h.finished()).filter_map(|h| h.strobe())
    }

    fn get(&mut self, key: &str) -> Result<&mut dyn Surface, Fail> {
        self.surfaces().find(|s| s.key() == key).ok_or_else(|| Fail::new(Code::NoWindow, format!("no window or overlay {key:?}")))
    }
}

impl Backend for Windows<'_> {
    fn clock_ns(&self) -> u64 { clock::now_ns() }

    fn surface(&mut self, key: Option<&str>) -> Result<String, Fail> {
        match key {
            Some(key) => self.get(key).map(|s| s.key().to_owned()),
            None => self.surfaces().find(|s| !s.overlay()).map(|s| s.key().to_owned()).ok_or_else(|| Fail::new(Code::NoWindow, "the app has no window")),
        }
    }

    fn find(&mut self, surface: &str, sel: &Selector) -> Option<[f32; 4]> { self.get(surface).ok()?.find(sel) }

    fn query(&mut self, surface: &str, sel: &Selector, all: bool) -> Vec<Value> {
        self.get(surface).map(|s| s.query(sel, all)).unwrap_or_default()
    }

    fn deliver(&mut self, surface: &str, input: &Input) -> Result<u64, Fail> {
        let event_loop = self.event_loop;
        let s = self.get(surface)?;
        // The bridge's work stops where the app's starts.
        record::bridge_end();
        let ns = clock::now_ns();
        let delivered = s.deliver(event_loop, input);
        record::bridge_begin();
        if delivered { Ok(ns) } else { Err(Fail::new(Code::NoWindow, format!("{surface:?} has no window to take input"))) }
    }

    fn quiet(&self) -> bool {
        self.hosts.iter().flatten().filter(|h| !h.finished()).all(|h| h.strobe_ref().is_none_or(Surface::quiet))
    }

    fn activity(&self) -> u64 { record::presents() }

    fn info(&mut self) -> Value {
        let gpu = self.surfaces().find_map(|s| s.adapter());
        let surfaces: Vec<Value> = self.surfaces().map(|s| s.describe()).collect();
        json!({ "gpu": super::gpu_json(gpu), "surfaces": surfaces })
    }

    fn window(&mut self, surface: &str, action: &str) -> Result<(), Fail> {
        let event_loop = self.event_loop;
        self.get(surface)?.window(event_loop, action)
    }

    fn snapshot(&mut self, surface: &str, path: &str) -> Result<Value, Fail> {
        let px = self.get(surface)?.pixels()?;
        super::write_snapshot(path, px)
    }

    fn step(&mut self, _args: &Value) -> Result<Value, Fail> { Err(Fail::new(Code::Unsupported, "step runs headless only")) }

    fn quit(&mut self) {
        let event_loop = self.event_loop;
        match self.surfaces().find(|s| !s.overlay()) {
            Some(main) => main.quit(event_loop),
            None => event_loop.exit(),
        }
    }

    fn tallies(&self) -> Vec<SurfaceTally> {
        self.hosts.iter().flatten().filter_map(|h| h.strobe_ref())
            .map(|s| SurfaceTally { key: s.key().to_owned(), counts: s.counts(), tally: s.tally() }).collect()
    }
}
