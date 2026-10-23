//! AppKit activation policy, overlay windows, and visibility without activation.

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability};
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSPanel, NSWindow, NSResponder, NSScreen};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use crate::{DisplayTarget, KeyPolicy, OverlayInput, OverlayLevel, OverlayOptions, OverlaySupport};
use std::{cell::{Cell, RefCell}, collections::VecDeque, rc::Rc, time::{Duration, Instant}};
pub(crate) use show_overlay as show_without_focus;

struct PanelFlags { keys: bool, warming: Rc<Cell<bool>> }

// Initialized nonactivating panel. No application activation call.
declare_class!(
    struct OverlayPanel;
    unsafe impl ClassType for OverlayPanel {
        #[inherits(NSWindow, NSResponder, NSObject)]
        type Super = NSPanel;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "RuneOverlayPanel";
    }
    impl DeclaredClass for OverlayPanel { type Ivars = PanelFlags; }
    unsafe impl OverlayPanel {
        #[method(canBecomeKeyWindow)]
        fn can_become_key(&self) -> bool { self.ivars().keys }
        #[method(canBecomeMainWindow)]
        fn can_become_main(&self) -> bool { false }
        #[method(occlusionState)]
        fn occlusion_state(&self) -> usize {
            // wgpu skips nextDrawable for occluded windows. Permit only the explicit
            // alpha-zero prewarm acquisition; ordinary visibility readbacks stay native.
            if self.ivars().warming.get() { 2 }
            else { unsafe { msg_send![super(self), occlusionState] } }
        }
    }
);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" { fn CGDisplayBounds(display: u32) -> NSRect; }

pub(crate) fn display_bounds(id: u32) -> [f64; 4] {
    let r = unsafe { CGDisplayBounds(id) };
    [r.origin.x, r.origin.y, r.size.width, r.size.height]
}

pub(crate) fn overlay_rect(at: crate::overlay::Placement, options: &OverlayOptions) -> Option<NSRect> {
    overlay_rect_on(at, options, None)
}

fn screen_id(screen: &NSScreen) -> u32 {
    unsafe {
        let description: *mut objc2::runtime::AnyObject = msg_send![screen, deviceDescription];
        let key = objc2_foundation::NSString::from_str("NSScreenNumber");
        let number: *mut objc2::runtime::AnyObject = msg_send![description, objectForKey: &*key];
        msg_send![number, unsignedIntValue]
    }
}

fn contains_pointer(screen: &NSScreen, pointer: NSPoint) -> bool {
    contains_appkit_rect(screen.frame(), pointer)
}

fn contains_appkit_rect(r: NSRect, pointer: NSPoint) -> bool {
    pointer.x >= r.origin.x && pointer.x < r.origin.x + r.size.width
        && pointer.y > r.origin.y && pointer.y <= r.origin.y + r.size.height
}

fn overlay_rect_on(at: crate::overlay::Placement, options: &OverlayOptions, previous: Option<u32>) -> Option<NSRect> {
    use winit::platform::macos::MonitorHandleExtMacOS;
    let mtm = MainThreadMarker::new()?;
    // SAFETY: AppKit screen and pointer queries on the event thread.
    let screens = NSScreen::screens(mtm);
    let pointer = unsafe { NSEvent::mouseLocation() };
    let screen = screens.iter().find(|screen| match &options.display {
        DisplayTarget::UnderPointer => {
            contains_pointer(screen, pointer)
        }
        DisplayTarget::Id(_) | DisplayTarget::Native(_) => {
            let native = screen_id(screen);
            match options.display {
                DisplayTarget::Id(ref id) => native == id.0.native_id(),
                DisplayTarget::Native(crate::NativeDisplayId::MacOS(id)) => native == id,
                _ => false,
            }
        },
        DisplayTarget::Primary => false,
    }).or_else(|| screens.iter().find(|s| options.display == DisplayTarget::UnderPointer && previous == Some(screen_id(s))))
        .or_else(|| screens.iter().next())?;
    let frame = screen.frame();
    let [x, y, w, h] = crate::overlay_options::local_rect(at, options.placement, (frame.size.width as f32, frame.size.height as f32));
    Some(NSRect::new(NSPoint::new(frame.origin.x + x as f64, frame.origin.y + frame.size.height - (y + h) as f64), NSSize::new(w as f64, h as f64)))
}

#[derive(Default)]
pub(crate) struct PointerState {
    input: Cell<OverlayInput>,
    regions: RefCell<Vec<[f32; 4]>>,
    pending: Cell<Option<(f32, f32)>>,
    captured: Cell<bool>,
    last: Cell<Option<(f32, f32)>>,
    inside: Cell<bool>,
    display: Cell<Option<u32>>,
    follow: Cell<bool>,
    clicks: RefCell<VecDeque<(bool, f32, f32)>>,
    release_since: Cell<Option<(Instant, (f32, f32))>>,
    recovered_release: Cell<Option<(f32, f32)>>,
}

impl PointerState {
    fn trace(&self, panel: &NSPanel, source: &str, event: Option<&NSEvent>) {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ENABLED.get_or_init(|| std::env::var_os("RUNE_OVERLAY_TRACE_INPUT").is_some()) { return; }
        let cursor = unsafe { NSEvent::mouseLocation() };
        let frame = panel.frame();
        let local = (cursor.x - frame.origin.x, frame.origin.y + frame.size.height - cursor.y);
        let kind: usize = event.map_or(0, |event| unsafe { msg_send![event, type] });
        let event_point: NSPoint = event.map_or(NSPoint::new(0.0, 0.0), |event| unsafe { msg_send![event, locationInWindow] });
        let buttons: usize = unsafe { msg_send![NSEvent::class(), pressedMouseButtons] };
        eprintln!("overlay-input source={source} window={} kind={kind} event={event_point:?} cursor={cursor:?} local={local:?} frame={frame:?} input={:?} buttons={buttons} captured={} ignore={} regions={:?}",
            unsafe { panel.windowNumber() }, self.input.get(), self.captured.get(), unsafe { panel.ignoresMouseEvents() }, self.regions.borrow());
    }
    pub(crate) fn hit(&self, x: f32, y: f32) -> bool {
        self.captured.get() || match self.input.get() {
            OverlayInput::All => true,
            OverlayInput::Regions => self.regions.borrow().iter().any(|r| crate::overlay_options::contains(*r, x, y)),
            _ => false,
        }
    }
    fn record_click(&self, down: bool, x: f32, y: f32) {
        if !self.hit(x, y) { return; }
        if down { self.captured.set(true); self.release_since.set(None); }
        self.clicks.borrow_mut().push_back((down, x, y));
    }
    fn released(&self) {
        // Native callbacks can queue another Down before winit dispatches this Up.
        self.captured.set(self.clicks.borrow().iter().any(|(down, _, _)| *down));
        self.release_since.set(None);
    }
    fn observe_release(&self, buttons: usize, now: Instant, point: (f32, f32)) {
        if !self.captured.get() || buttons & 1 != 0 || self.clicks.borrow().iter().any(|(down, _, _)| !down) {
            self.release_since.set(None);
            return;
        }
        let (since, released_at) = self.release_since.get().unwrap_or_else(|| {
            self.release_since.set(Some((now, point)));
            (now, point)
        });
        if now.duration_since(since) < Duration::from_millis(100) { return; }
        self.captured.set(false);
        self.clicks.borrow_mut().clear();
        self.release_since.set(None);
        self.recovered_release.set(Some(released_at));
    }
    fn observe(&self, panel: &NSPanel, wake: Option<&crate::Waker>) {
        // SAFETY: monitors and frame publication run on AppKit's main thread.
        let pointer = unsafe { NSEvent::mouseLocation() };
        let buttons: usize = unsafe { msg_send![NSEvent::class(), pressedMouseButtons] };
        let frame = panel.frame();
        let (x, y) = ((pointer.x - frame.origin.x) as f32, (frame.origin.y + frame.size.height - pointer.y) as f32);
        self.observe_release(buttons, Instant::now(), (x, y));
        let ignore = !self.hit(x, y);
        if unsafe { panel.ignoresMouseEvents() } != ignore {
            panel.setIgnoresMouseEvents(ignore);
            self.trace(panel, "flip", None);
        }
        let inside = x >= 0.0 && y >= 0.0 && x < frame.size.width as f32 && y < frame.size.height as f32;
        let was_inside = self.inside.replace(inside);
        let display_changed = self.follow.get() && MainThreadMarker::new().is_some_and(|mtm| {
            NSScreen::screens(mtm).iter().find(|s| contains_pointer(s, pointer))
                .is_some_and(|s| self.display.replace(Some(screen_id(s))) != Some(screen_id(s)))
        });
        if (inside || was_inside || self.captured.get() || display_changed) && self.last.replace(Some((x, y))) != Some((x, y)) {
            self.pending.set(Some((x, y)));
            if let Some(wake) = wake { wake(); }
        }
    }
}

struct MouseMonitors { tokens: Vec<Retained<NSObject>> }

pub(crate) struct Prewarm(Rc<Cell<bool>>);
impl Drop for Prewarm { fn drop(&mut self) { self.0.set(false); } }
impl Drop for MouseMonitors {
    fn drop(&mut self) {
        for token in &self.tokens {
            // SAFETY: each token was returned by NSEvent and is removed exactly once on main.
            unsafe { let _: () = msg_send![NSEvent::class(), removeMonitor: &**token]; }
        }
    }
}

/// Regular (Dock icon and app menu) or accessory (menu-bar only). Called on the main thread.
pub(crate) fn set_dock_icon(visible: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let policy = if visible { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
}

fn ns_window(window: &winit::window::Window) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit's content view is alive as long as its window, which the caller holds.
    let view: &objc2_app_kit::NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
}

pub(crate) fn configure_window(window: &winit::window::Window, options: &crate::MacWindowOptions) {
    if let Some(window) = ns_window(window) { configure_native(&window, options); }
}

fn configure_native(window: &NSWindow, options: &crate::MacWindowOptions) {
    if let Some(shadow) = options.shadow { window.setHasShadow(shadow); }
    if let Some(level) = options.level { window.setLevel(level); }
    // SAFETY: all callers are native creation paths on the AppKit main thread.
    unsafe {
        if let Some(bits) = options.collection_behavior {
            let _: () = msg_send![window, setCollectionBehavior: bits];
        }
        if let Some(exclude) = options.exclude_from_capture {
            let _: () = msg_send![window, setSharingType: if exclude { 0usize } else { 1usize }];
        }
    }
}

/// Orders a window front without making it key or activating the app.
#[allow(dead_code)]
pub(crate) fn show_overlay(window: &winit::window::Window) {
    let Some(w) = ns_window(window) else { return };
    // SAFETY: an AppKit call on the main thread.
    unsafe { w.orderFrontRegardless() };
}

/// An overlay's non-activating panel. winit's content view (with the Metal layer wgpu draws
/// into) moves into it, so clicks reach Rune through winit while the app never activates and
/// the panel only becomes key on request. winit's own window stays hidden and gets its view back on drop.
pub(crate) struct Panel {
    bridge: Option<crate::macos_panel_bridge::Bridge>,
    monitors: RefCell<Option<MouseMonitors>>,
    pointer: Rc<PointerState>,
    options: OverlayOptions,
    follow_pointer: Cell<bool>,
    visible: Cell<bool>,
    pointer_poll: Cell<Option<Instant>>,
    warming: Rc<Cell<bool>>,
    wake: Option<crate::Waker>,
    panel: objc2::rc::Retained<objc2_app_kit::NSPanel>,
    window: objc2::rc::Retained<objc2_app_kit::NSWindow>,
    view: objc2::rc::Retained<objc2_app_kit::NSView>,
}

impl Panel {
    pub fn adopt(window: &winit::window::Window, at: crate::overlay::Placement, interactive: bool, options: &OverlayOptions, wake: Option<crate::Waker>) -> Option<Panel> {
        use objc2_app_kit::{NSBackingStoreType, NSColor, NSPanel, NSView, NSWindowCollectionBehavior, NSWindowStyleMask};
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's content view is alive while its window is, and we retain it.
        let view = unsafe { objc2::rc::Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }?;
        let ns_window = view.window()?;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(at.size.0 as f64, at.size.1 as f64));
        // SAFETY: AppKit calls on the main thread; the panel is retained by `Panel`.
        let warming = Rc::new(Cell::new(false));
        let panel = unsafe {
            let panel: Retained<OverlayPanel> = msg_send_id![super(mtm.alloc().set_ivars(PanelFlags { keys: options.keys == KeyPolicy::OnDemand, warming: warming.clone() })),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::NSBackingStoreBuffered,
                defer: false];
            Retained::cast::<NSPanel>(panel)
        };
        unsafe {
            panel.setReleasedWhenClosed(false);
            panel.setFloatingPanel(true);
            panel.setBecomesKeyOnlyIfNeeded(options.keys == KeyPolicy::None);
            panel.setHidesOnDeactivate(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        }
        let mut native = crate::MacWindowOptions::default().shadow(false)
            .level(match options.level { OverlayLevel::Status => 25, OverlayLevel::AboveStatus => 26, OverlayLevel::AboveOverlays => 27 });
        if options.exclude_from_capture { native = native.exclude_from_capture(true); }
        configure_native(&panel, &native);
        panel.setOpaque(false);
        panel.setIgnoresMouseEvents(options.input.resolved(interactive) != OverlayInput::All);
        panel.setAcceptsMouseMovedEvents(true);
        if options.prewarm {
            unsafe { panel.setAlphaValue(0.0); }
            panel.setIgnoresMouseEvents(true);
        }
        // defer:NO creates the WindowServer identity while hidden. Never publish a sentinel
        // (-1/0) cast to CGWindowID as a successful native creation.
        if unsafe { panel.windowNumber() } <= 0 { return None; }
        ns_window.setContentView(None);
        panel.setContentView(Some(&view));
        let pointer = Rc::new(PointerState { input: Cell::new(options.input.resolved(interactive)), follow: Cell::new(options.follow_pointer), ..Default::default() });
        let bridge = crate::macos_panel_bridge::Bridge::new(&ns_window, &panel, &view, Some(pointer.clone()));
        if bridge.is_none() {
            panel.setContentView(None);
            ns_window.setContentView(Some(&view));
            return None;
        }
        panel.setDelegate(unsafe { ns_window.delegate() }.as_deref());
        Some(Panel { bridge, monitors: RefCell::new(None), pointer, options: options.clone(), follow_pointer: Cell::new(options.follow_pointer), visible: Cell::new(false), pointer_poll: Cell::new(None), warming, wake, panel, window: ns_window, view })
    }

    /// On the primary screen (the one with the menu bar), in AppKit's bottom-left coordinates.
    pub fn place(&self, at: crate::overlay::Placement, options: &OverlayOptions) {
        let Some(rect) = self.rect(at, options) else { return };
        if self.panel.frame() != rect {
            self.window.setFrame_display(rect, false);
            self.panel.setFrame_display(rect, true);
            if self.visible.get() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
        }
    }

    pub fn rect(&self, at: crate::overlay::Placement, options: &OverlayOptions) -> Option<NSRect> {
        overlay_rect_on(at, options, self.panel.screen().map(|s| screen_id(&s)))
    }

    pub fn show(&self) {
        self.visible.set(true);
        self.refresh_monitors();
        self.pointer.observe(&self.panel, self.wake.as_ref());
        // SAFETY: an AppKit call on the main thread.
        if self.options.prewarm { unsafe { self.panel.setAlphaValue(1.0); } }
        else { unsafe { self.panel.orderFrontRegardless() }; }
    }

    pub fn prewarm(&self) -> Prewarm {
        debug_assert!(self.options.prewarm);
        unsafe { self.panel.setAlphaValue(0.0); }
        self.panel.setIgnoresMouseEvents(true);
        if !self.panel.isVisible() { unsafe { self.panel.orderFrontRegardless() }; }
        self.warming.set(true);
        Prewarm(self.warming.clone())
    }

    /// The moved view, for a new wgpu surface (winit's window handle would look for the view
    /// in winit's window, where it no longer is).
    pub fn raw_handles(&self) -> (winit::raw_window_handle::RawDisplayHandle, winit::raw_window_handle::RawWindowHandle) {
        use winit::raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
        let view = std::ptr::NonNull::from(&*self.view).cast();
        (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(view)))
    }

    pub fn hide(&self) {
        self.visible.set(false);
        self.pointer_poll.set(None);
        self.monitors.borrow_mut().take();
        self.pointer.captured.set(false);
        self.pointer.pending.set(None);
        self.pointer.last.set(None);
        self.pointer.inside.set(false);
        self.pointer.clicks.borrow_mut().clear();
        self.pointer.release_since.set(None);
        self.pointer.recovered_release.set(None);
        if self.options.prewarm {
            unsafe { self.panel.setAlphaValue(0.0); self.panel.resignKeyWindow(); }
            self.panel.setIgnoresMouseEvents(true);
        } else { self.panel.orderOut(None); }
    }

    pub fn window_number(&self) -> u32 { unsafe { self.panel.windowNumber() as u32 } }
    pub fn capture_exclusion(&self) -> OverlaySupport {
        if self.options.exclude_from_capture { OverlaySupport::Requested } else { OverlaySupport::Unknown }
    }
    pub fn global_pointer(&self) -> OverlaySupport {
        if (self.follow_pointer.get() || self.pointer.input.get() == OverlayInput::Regions)
            && self.monitors.borrow().as_ref().is_none_or(|m| m.tokens.len() != 2) { OverlaySupport::Failed }
        else { OverlaySupport::Supported }
    }
    pub fn request_keys(&self) {
        if self.options.keys == KeyPolicy::OnDemand {
            self.panel.makeFirstResponder(Some(&self.view));
            unsafe { self.panel.makeKeyWindow(); }
        }
    }
    pub fn take_pointer(&self) -> Option<(f32, f32)> { self.pointer.pending.take() }
    pub fn pointer_captured(&self) -> bool { self.pointer.captured.get() }
    pub fn take_recovered_release(&self) -> Option<(f32, f32)> { self.pointer.recovered_release.take() }
    pub fn take_click(&self, down: bool) -> Option<(f32, f32)> {
        let mut clicks = self.pointer.clicks.borrow_mut();
        if clicks.front().is_some_and(|(pressed, _, _)| *pressed == down) {
            clicks.pop_front().map(|(_, x, y)| (x, y))
        } else { None }
    }
    pub fn pointer_released(&self) {
        self.pointer.released();
        self.refresh_monitors();
        if self.visible.get() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
    }
    pub fn pointer_deadline(&self) -> Option<Instant> { self.pointer_poll.get() }
    pub fn poll_pointer(&self, now: Instant) {
        if self.pointer_poll.get().is_some_and(|at| at <= now) {
            self.pointer_poll.set(Some(now + Duration::from_secs_f64(1.0 / 120.0)));
            self.pointer.trace(&self.panel, "poll", None);
            self.pointer.observe(&self.panel, None);
        }
    }
    pub fn observe_pointer(&self) {
        if self.visible.get() { self.pointer.observe(&self.panel, None); }
    }
    pub fn set_input(&self, input: OverlayInput) {
        self.pointer.input.set(input);
        self.refresh_monitors();
        if self.visible.get() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
    }
    pub fn set_follow_pointer(&self, follow: bool) { self.follow_pointer.set(follow); self.pointer.follow.set(follow); self.refresh_monitors(); }
    fn refresh_monitors(&self) {
        if !self.visible.get() { return; }
        if self.pointer.input.get() == OverlayInput::Regions {
            if self.pointer_poll.get().is_none() { self.pointer_poll.set(Some(Instant::now())); }
        } else { self.pointer_poll.set(None); }
        if self.follow_pointer.get() || self.pointer.input.get() == OverlayInput::Regions || self.pointer.captured.get() { self.start_monitors(); }
        else { self.monitors.borrow_mut().take(); }
    }
    pub fn set_regions(&self, regions: &[[f32; 4]]) {
        if self.pointer.regions.borrow().as_slice() == regions { return; }
        *self.pointer.regions.borrow_mut() = regions.to_vec();
        self.pointer.trace(&self.panel, "regions", None);
        if self.visible.get() { self.pointer.observe(&self.panel, None); }
    }

    fn start_monitors(&self) {
        if self.monitors.borrow().is_some() || !(self.follow_pointer.get() || self.pointer.input.get() == OverlayInput::Regions || self.pointer.captured.get()) { return; }
        // Monitors wake the loop; position/button state is sampled there. A local down
        // owns its drag until winit has delivered Up, including outside the hit region.
        let movement = (1usize << 5) | (1 << 6) | (1 << 7) | (1 << 27);
        let mask = movement | (1 << 1) | (1 << 2);
        let (pointer, panel, wake) = (self.pointer.clone(), self.panel.clone(), self.wake.clone());
        let global = block2::RcBlock::new(move |event: *mut NSEvent| {
            pointer.trace(&panel, "global", unsafe { event.as_ref() });
            if let Some(wake) = &wake { wake(); }
        });
        let (pointer, panel, wake) = (self.pointer.clone(), self.panel.clone(), self.wake.clone());
        let local = block2::RcBlock::new(move |event: *mut NSEvent| -> *mut NSEvent {
            pointer.trace(&panel, "local", unsafe { event.as_ref() });
            unsafe {
                let kind: usize = msg_send![event, type];
                let window: *mut NSWindow = msg_send![event, window];
                if (kind == 1 || kind == 2) && window == (&*panel as *const NSPanel).cast_mut().cast() {
                    let point: NSPoint = msg_send![event, locationInWindow];
                    let frame = panel.frame();
                    let (x, y) = (point.x as f32, (frame.size.height - point.y) as f32);
                    pointer.record_click(kind == 1, x, y);
                }
            }
            if let Some(wake) = &wake { wake(); }
            event
        });
        let mut tokens = Vec::new();
        unsafe {
            let token: Option<Retained<NSObject>> = msg_send_id![NSEvent::class(), addGlobalMonitorForEventsMatchingMask: mask, handler: &*global];
            tokens.extend(token);
            let token: Option<Retained<NSObject>> = msg_send_id![NSEvent::class(), addLocalMonitorForEventsMatchingMask: mask, handler: &*local];
            tokens.extend(token);
        }
        *self.monitors.borrow_mut() = Some(MouseMonitors { tokens });
        self.pointer.observe(&self.panel, self.wake.as_ref());
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.monitors.get_mut().take();
        self.panel.setDelegate(None);
        self.bridge.take();
        self.panel.orderOut(None);
        self.panel.setContentView(None);
        self.window.setContentView(Some(&self.view));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_handlerless_hit_releases_native_ownership_in_every_input_mode() {
        fn view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
            crate::el().child(crate::el().absolute(10.0, 10.0).size(20.0, 20.0).hit())
        }
        for mode in [OverlayInput::Regions, OverlayInput::None] {
            let mut rt = crate::Runtime::new((), view, (100.0, 100.0), crate::Env::default());
            rt.native_overlay = true;
            rt.frame(0.0);
            let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
            pointer.regions.borrow_mut().extend_from_slice(rt.hit_regions());
            pointer.record_click(true, 20.0, 20.0);
            pointer.clicks.borrow_mut().pop_front(); // winit dispatches Down
            rt.pointer_down(20.0, 20.0);
            assert!(!rt.pointer_captured(), "a handlerless hit owns only the native press");
            assert!(pointer.captured.get());
            pointer.record_click(false, 50.0, 50.0);
            pointer.input.set(mode);
            pointer.clicks.borrow_mut().pop_front(); // winit dispatches Up
            pointer.released(); // acknowledged even when Runtime input is disabled
            assert!(!pointer.captured.get(), "native release cannot depend on Runtime capture or mode");
            assert!(!pointer.hit(50.0, 50.0));
        }
    }

    #[test]
    fn overlay_lost_up_recovers_once_after_grace_using_release_position() {
        let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
        pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
        pointer.record_click(true, 20.0, 20.0);
        let now = Instant::now();
        pointer.observe_release(1, now, (40.0, 40.0));
        pointer.observe_release(0, now, (50.0, 50.0));
        pointer.observe_release(0, now + Duration::from_millis(99), (60.0, 60.0));
        assert!(pointer.captured.get());
        pointer.observe_release(0, now + Duration::from_millis(100), (70.0, 70.0));
        assert!(!pointer.captured.get());
        assert!(pointer.clicks.borrow().is_empty());
        assert_eq!(pointer.recovered_release.take(), Some((50.0, 50.0)));
        pointer.observe_release(0, now + Duration::from_secs(1), (80.0, 80.0));
        assert_eq!(pointer.recovered_release.take(), None);
    }
    #[test]
    fn overlay_queued_up_and_next_down_keep_their_ownership_until_dispatched() {
        let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
        pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
        pointer.record_click(true, 20.0, 20.0);
        pointer.record_click(false, 100.0, 100.0);
        pointer.observe_release(0, Instant::now(), (100.0, 100.0));
        pointer.observe_release(0, Instant::now() + Duration::from_secs(1), (100.0, 100.0));
        assert!(pointer.captured.get());
        assert_eq!(pointer.recovered_release.get(), None);
        pointer.clicks.borrow_mut().pop_front(); // Down dispatched
        pointer.clicks.borrow_mut().pop_front(); // Up dispatched
        pointer.record_click(true, 20.0, 20.0); // next native Down queued
        pointer.released();
        assert!(pointer.captured.get());
        pointer.clicks.borrow_mut().clear();
        pointer.released();
        assert!(!pointer.captured.get());
    }
    #[test]
    fn overlay_rejected_down_cannot_poison_later_click_and_owned_up_survives_exit() {
        let pointer = PointerState { input: Cell::new(OverlayInput::Regions), ..Default::default() };
        pointer.regions.borrow_mut().push([10.0, 10.0, 30.0, 30.0]);
        pointer.record_click(true, 1.0, 1.0);
        pointer.record_click(true, 20.0, 20.0);
        pointer.record_click(false, 100.0, 100.0);
        assert_eq!(*pointer.clicks.borrow(), VecDeque::from([(true, 20.0, 20.0), (false, 100.0, 100.0)]));
        // Physical Up must not turn off native hit testing before winit dispatches it.
        assert!(pointer.hit(100.0, 100.0));
        pointer.captured.set(false);
        assert!(!pointer.hit(100.0, 100.0));
    }
    #[test]
    fn overlay_appkit_top_row_belongs_to_secondary_display() {
        let secondary = NSRect::new(NSPoint::new(-1920.0, 0.0), NSSize::new(1920.0, 1080.0));
        assert!(contains_appkit_rect(secondary, NSPoint::new(-100.0, 1080.0)));
        assert!(!contains_appkit_rect(secondary, NSPoint::new(-100.0, 0.0)));
        assert!(!contains_appkit_rect(secondary, NSPoint::new(0.0, 500.0)));
        let below = NSRect::new(NSPoint::new(-1920.0, -1080.0), NSSize::new(1920.0, 1080.0));
        assert!(contains_appkit_rect(below, NSPoint::new(-100.0, 0.0)));
    }
}
