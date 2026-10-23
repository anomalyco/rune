//! AppKit activation policy, overlay windows, and visibility without activation.

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability};
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSPanel, NSWindow, NSResponder, NSScreen};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use crate::{DisplayTarget, KeyPolicy, OverlayInput, OverlayLevel, OverlayOptions, OverlaySupport};
use std::{cell::{Cell, RefCell}, rc::Rc};
pub(crate) use show_overlay as show_without_focus;

// The launcher's initialized NonactivatingPanel subclass. No application activation call.
declare_class!(
    struct KeyPanel;
    unsafe impl ClassType for KeyPanel {
        #[inherits(NSWindow, NSResponder, NSObject)]
        type Super = NSPanel;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "RuneOverlayKeyPanel";
    }
    impl DeclaredClass for KeyPanel {}
    unsafe impl KeyPanel {
        #[method(canBecomeKeyWindow)]
        fn can_become_key(&self) -> bool { true }
        #[method(canBecomeMainWindow)]
        fn can_become_main(&self) -> bool { false }
    }
);

pub(crate) fn overlay_rect(at: crate::overlay::Placement, options: &OverlayOptions) -> Option<NSRect> {
    use winit::platform::macos::MonitorHandleExtMacOS;
    let mtm = MainThreadMarker::new()?;
    // SAFETY: AppKit screen and pointer queries on the event thread.
    let screens = NSScreen::screens(mtm);
    let pointer = unsafe { NSEvent::mouseLocation() };
    let screen = screens.iter().find(|screen| match &options.display {
        DisplayTarget::UnderPointer => {
            let r = screen.frame();
            pointer.x >= r.origin.x && pointer.y >= r.origin.y && pointer.x < r.origin.x + r.size.width && pointer.y < r.origin.y + r.size.height
        }
        DisplayTarget::Id(id) => unsafe {
            let description: *mut objc2::runtime::AnyObject = msg_send![*screen, deviceDescription];
            let key = objc2_foundation::NSString::from_str("NSScreenNumber");
            let number: *mut objc2::runtime::AnyObject = msg_send![description, objectForKey: &*key];
            let native: u32 = msg_send![number, unsignedIntValue];
            native == id.0.native_id()
        },
        DisplayTarget::Primary => false,
    }).or_else(|| screens.iter().next())?;
    let frame = screen.frame();
    let [x, y, w, h] = crate::overlay_options::local_rect(at, options.placement, (frame.size.width as f32, frame.size.height as f32));
    Some(NSRect::new(NSPoint::new(frame.origin.x + x as f64, frame.origin.y + frame.size.height - (y + h) as f64), NSSize::new(w as f64, h as f64)))
}

#[derive(Default)]
pub(crate) struct PointerState {
    regions: RefCell<Vec<[f32; 4]>>,
    pending: Cell<Option<(f32, f32)>>,
    captured: Cell<bool>,
}

impl PointerState {
    pub(crate) fn hit(&self, x: f32, y: f32) -> bool {
        self.captured.get() || self.regions.borrow().iter().any(|r| crate::overlay_options::contains(*r, x, y))
    }
    fn observe(&self, panel: &NSPanel, regions: bool, wake: Option<&crate::Waker>) {
        // SAFETY: monitors and frame publication run on AppKit's main thread.
        let pointer = unsafe { NSEvent::mouseLocation() };
        let frame = panel.frame();
        let (x, y) = ((pointer.x - frame.origin.x) as f32, (frame.origin.y + frame.size.height - pointer.y) as f32);
        if regions {
            panel.setIgnoresMouseEvents(!self.hit(x, y));
        }
        self.pending.set(Some((x, y)));
        if let Some(wake) = wake { wake(); }
    }
}

struct MouseMonitors { tokens: Vec<Retained<NSObject>> }
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
        let panel = unsafe { if options.keys == KeyPolicy::OnDemand {
            let panel: Retained<KeyPanel> = msg_send_id![super(mtm.alloc().set_ivars(())),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::NSBackingStoreBuffered,
                defer: false];
            Retained::cast::<NSPanel>(panel)
        } else {
            NSPanel::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        }};
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
        panel.setLevel(if options.level == OverlayLevel::AboveStatus { 26 } else { 25 });
        if options.exclude_from_capture {
            // NSWindowSharingNone; capture clients should additionally filter our window ID.
            unsafe { let _: () = msg_send![&*panel, setSharingType: 0usize]; }
        }
        panel.setOpaque(false);
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(!interactive || options.input == OverlayInput::Regions);
        panel.setAcceptsMouseMovedEvents(true);
        ns_window.setContentView(None);
        panel.setContentView(Some(&view));
        let pointer = Rc::default();
        let filter = (options.input == OverlayInput::Regions).then(|| Rc::clone(&pointer));
        let bridge = crate::macos_panel_bridge::Bridge::new(&ns_window, &panel, &view, filter);
        if bridge.is_none() {
            panel.setContentView(None);
            ns_window.setContentView(Some(&view));
            return None;
        }
        panel.setDelegate(unsafe { ns_window.delegate() }.as_deref());
        Some(Panel { bridge, monitors: RefCell::new(None), pointer, options: options.clone(), wake, panel, window: ns_window, view })
    }

    /// On the primary screen (the one with the menu bar), in AppKit's bottom-left coordinates.
    pub fn place(&self, at: crate::overlay::Placement, options: &OverlayOptions) {
        let Some(rect) = overlay_rect(at, options) else { return };
        if self.panel.frame() != rect {
            self.window.setFrame_display(rect, false);
            self.panel.setFrame_display(rect, true);
        }
    }

    pub fn show(&self) {
        self.start_monitors();
        // SAFETY: an AppKit call on the main thread.
        unsafe { self.panel.orderFrontRegardless() };
    }

    /// The moved view, for a new wgpu surface (winit's window handle would look for the view
    /// in winit's window, where it no longer is).
    pub fn raw_handles(&self) -> (winit::raw_window_handle::RawDisplayHandle, winit::raw_window_handle::RawWindowHandle) {
        use winit::raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
        let view = std::ptr::NonNull::from(&*self.view).cast();
        (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(view)))
    }

    pub fn hide(&self) {
        self.monitors.borrow_mut().take();
        self.pointer.captured.set(false);
        self.pointer.pending.set(None);
        self.panel.orderOut(None);
    }

    pub fn window_number(&self) -> u32 { unsafe { self.panel.windowNumber() as u32 } }
    pub fn capture_exclusion(&self) -> OverlaySupport {
        if self.options.exclude_from_capture { OverlaySupport::Requested } else { OverlaySupport::Unknown }
    }
    pub fn global_pointer(&self) -> OverlaySupport {
        if (self.options.follow_pointer || self.options.input == OverlayInput::Regions)
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
    pub fn set_regions(&self, regions: &[[f32; 4]]) {
        if self.options.input != OverlayInput::Regions { return; }
        if self.pointer.regions.borrow().as_slice() == regions { return; }
        *self.pointer.regions.borrow_mut() = regions.to_vec();
        self.pointer.observe(&self.panel, true, None);
    }

    fn start_monitors(&self) {
        if self.monitors.borrow().is_some() || !(self.options.follow_pointer || self.options.input == OverlayInput::Regions) { return; }
        // NSEvent masks: moved, left/right/other dragged, left down/up. Observation never
        // suppresses an event. A local down owns its drag until a local/global release.
        let mask = (1usize << 5) | (1 << 6) | (1 << 7) | (1 << 27) | (1 << 1) | (1 << 2);
        let (pointer, panel, wake) = (self.pointer.clone(), self.panel.clone(), self.wake.clone());
        let regions = self.options.input == OverlayInput::Regions;
        let global = block2::RcBlock::new(move |event: *mut NSEvent| {
            unsafe { let kind: usize = msg_send![event, type]; if kind == 2 { pointer.captured.set(false); } }
            pointer.observe(&panel, regions, wake.as_ref());
        });
        let (pointer, panel, wake) = (self.pointer.clone(), self.panel.clone(), self.wake.clone());
        let local = block2::RcBlock::new(move |event: *mut NSEvent| -> *mut NSEvent {
            unsafe {
                let kind: usize = msg_send![event, type];
                let window: *mut NSWindow = msg_send![event, window];
                if kind == 1 && window == (&*panel as *const NSPanel).cast_mut().cast() {
                    let point: NSPoint = msg_send![event, locationInWindow];
                    let frame = panel.frame();
                    if !regions || pointer.hit(point.x as f32, (frame.size.height - point.y) as f32) { pointer.captured.set(true); }
                }
                if kind == 2 { pointer.captured.set(false); }
            }
            pointer.observe(&panel, regions, wake.as_ref());
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
        self.pointer.observe(&self.panel, regions, self.wake.as_ref());
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
