//! AppKit activation policy, overlay windows, and visibility without activation.

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use objc2::{ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability};
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSPanel, NSWindow, NSResponder, NSScreen, NSView};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use crate::{KeyPolicy, OverlayInput, OverlayLevel, OverlayOptions, OverlaySupport};
use std::{cell::{Cell, RefCell}, rc::Rc, time::Instant};

#[doc(hidden)]
pub mod appearance;
pub(crate) mod cursor;
pub(crate) mod drop;
mod geometry;
mod hold;
pub(crate) mod help;
#[doc(hidden)]
pub mod panel_bridge;
mod pointer;
pub(crate) mod presence;
mod presentation;
mod surface;
pub(crate) mod traffic_lights;
pub(crate) mod window_shape;

pub(crate) use geometry::{backing_scale, content_origin, display_bounds, outer_size, overlay_rect, place_window, pointer_display, set_window_frame, window_geometry};
pub(crate) use pointer::PointerState;
#[cfg(test)]
pub(crate) use surface::layer_occluded;
pub(crate) use surface::{PresumeShown, pin_contents_top_left, present_with_transaction, surface_occluded, window_occluded};
use geometry::{overlay_rect_on, screen_id};
use pointer::MouseMonitors;
use presence::Presence;
use surface::occlusion_hidden;

struct PanelFlags { keys: bool, presence: Rc<Cell<Presence>>, constrain: bool }

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
            // alpha-zero prewarm acquisition and a fade in or out, which AppKit reports as
            // occluded while its alpha is near zero; other visibility readbacks stay native.
            if self.ivars().presence.get().draws_occluded() { 2 }
            else { unsafe { msg_send![super(self), occlusionState] } }
        }
        // AppKit keeps windows below the menu bar's level out from under it; an overlay goes
        // exactly where it's placed at any level.
        #[method(constrainFrameRect:toScreen:)]
        fn constrain_frame_rect(&self, frame: NSRect, screen: *mut NSScreen) -> NSRect {
            if self.ivars().constrain { unsafe { msg_send![super(self), constrainFrameRect: frame, toScreen: screen] } }
            else { frame }
        }
        #[method(sendEvent:)]
        fn send_event(&self, event: &NSEvent) {
            // Without its view the panel would swallow this click and every later one.
            if crate::platform::macos::panel_bridge::reattach(self.as_super()) { eprintln!("rune: panel lost its view; reattached on input"); }
            unsafe { let _: () = msg_send![super(self), sendEvent: event]; }
        }
    }
);

/// Whether the user is dragging the window's edge.
pub(crate) fn in_live_resize(window: &winit::window::Window) -> bool {
    // SAFETY: a property read on the main thread.
    ns_window(window).is_some_and(|window| unsafe { msg_send![&*window, inLiveResize] })
}

/// Regular (Dock icon and app menu) or accessory (menu-bar only). Called on the main thread.
pub(crate) fn set_dock_icon(visible: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let policy = if visible { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
    let app = NSApplication::sharedApplication(mtm);
    if visible { crate::native_menu::promote_default_menu(&app); }
    app.setActivationPolicy(policy);
}

/// winit's content view. Once a [`Panel`] adopts it, its window is the panel.
pub(crate) fn content_view(window: &winit::window::Window) -> Option<Retained<objc2_app_kit::NSView>> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit's content view is alive as long as its window, which the caller holds.
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }
}

pub(crate) fn ns_window(window: &winit::window::Window) -> Option<Retained<NSWindow>> {
    content_view(window)?.window()
}

pub(crate) fn configure_window(window: &winit::window::Window, options: &crate::MacWindowOptions) {
    crate::platform::macos::appearance::install();
    if let Some(window) = ns_window(window) { configure_native(&window, options); }
}

fn configure_native(window: &NSWindow, options: &crate::MacWindowOptions) {
    if let Some(shadow) = options.shadow { window.setHasShadow(shadow); }
    if let Some(level) = options.level { window.setLevel(level); }
    // SAFETY: every caller runs on the AppKit main thread.
    unsafe {
        if let Some(bits) = options.collection_behavior {
            let _: () = msg_send![window, setCollectionBehavior: bits];
        }
    }
    if let Some(exclude) = options.exclude_from_capture { set_sharing(window, exclude); }
}

/// `sharingType` none keeps the window out of captures; read-only lets recorders see it.
/// Captures follow every change, but on recent macOS the `sharingType` getter and
/// CGWindowList's sharing state can keep reporting none after the first switch back, so
/// neither confirms a toggle.
fn set_sharing(window: &NSWindow, exclude: bool) {
    // SAFETY: every caller runs on the AppKit main thread.
    unsafe { let _: () = msg_send![window, setSharingType: if exclude { 0usize } else { 1usize }]; }
}

/// Orders a window front without making it key or activating the app.
pub(crate) fn show_without_focus(window: &winit::window::Window) {
    let Some(w) = ns_window(window) else { return };
    // SAFETY: an AppKit call on the main thread.
    unsafe { w.orderFrontRegardless() };
}

/// An overlay's non-activating panel. winit's content view (with the Metal layer wgpu draws
/// into) moves into it, so clicks reach Rune through winit while the app never activates and
/// the panel only becomes key on request. winit's own window stays hidden and gets its view back on drop.
pub(crate) struct Panel {
    bridge: Option<crate::platform::macos::panel_bridge::Bridge>,
    monitors: RefCell<Option<MouseMonitors>>,
    pointer: Rc<PointerState>,
    options: OverlayOptions,
    follow_pointer: Cell<bool>,
    pointer_poll: Cell<Option<Instant>>,
    /// Prewarmed, shown or fading, under one fence, and the requested capture exclusion,
    /// applied only while shown. While a fade runs the panel reads as visible, so a fading-out
    /// overlay can still draw its exit.
    presence: Rc<Cell<Presence>>,
    wake: Option<crate::Waker>,
    panel: objc2::rc::Retained<objc2_app_kit::NSPanel>,
    window: objc2::rc::Retained<objc2_app_kit::NSWindow>,
    view: objc2::rc::Retained<objc2_app_kit::NSView>,
}

impl Panel {
    pub fn adopt(window: &winit::window::Window, at: crate::overlay::Placement, interactive: bool, options: &OverlayOptions, wake: Option<crate::Waker>) -> Option<Panel> {
        Self::adopt_kind(window, at, interactive, options, wake, false)
    }

    pub fn adopt_window(window: &winit::window::Window, size: (f32, f32), native: &crate::MacWindowOptions) -> Option<Panel> {
        let options = OverlayOptions { input: OverlayInput::All, keys: KeyPolicy::OnDemand, prewarm: native.launcher, ..Default::default() };
        let panel = Self::adopt_kind(window, crate::windows::PlaceRequest::at(size), true, &options, None, true)?;
        let mut native = native.clone();
        if let Some(exclude) = native.exclude_from_capture.take() { panel.set_capture_exclusion(exclude); }
        configure_native(&panel.panel, &native);
        Some(panel)
    }

    fn adopt_kind(window: &winit::window::Window, at: crate::overlay::Placement, interactive: bool, options: &OverlayOptions, wake: Option<crate::Waker>, ordinary: bool) -> Option<Panel> {
        use objc2_app_kit::{NSBackingStoreType, NSColor, NSWindowCollectionBehavior, NSWindowStyleMask};
        let mtm = MainThreadMarker::new()?;
        crate::platform::macos::appearance::install();
        let view = content_view(window)?;
        let ns_window = view.window()?;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(at.size.0 as f64, at.size.1 as f64));
        // SAFETY: AppKit calls on the main thread; the panel is retained by `Panel`.
        let presence = Rc::new(Cell::new(Presence::new(options.exclude_from_capture)));
        let style = if ordinary { ns_window.styleMask() } else { NSWindowStyleMask::Borderless };
        let panel = unsafe {
            let panel: Retained<OverlayPanel> = msg_send_id![super(mtm.alloc().set_ivars(PanelFlags { keys: options.keys == KeyPolicy::OnDemand, presence: presence.clone(), constrain: ordinary })),
                initWithContentRect: rect,
                styleMask: style | NSWindowStyleMask::NonactivatingPanel,
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
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    // Over other apps' full-screen Spaces too (macOS 13+; earlier ignores the bit).
                    | NSWindowCollectionBehavior::CanJoinAllApplications,
            );
        }
        let native = crate::MacWindowOptions::default().shadow(false)
            .level(match options.level { OverlayLevel::Status => 25, OverlayLevel::AboveStatus => 26, OverlayLevel::AboveOverlays => 27 });
        configure_native(&panel, &native);
        if ordinary {
            panel.setTitle(&ns_window.title());
            panel.setLevel(3);
            panel.setHasShadow(ns_window.hasShadow());
            // SAFETY: both retained windows are owned on the AppKit main thread.
            unsafe {
                panel.setContentMinSize(ns_window.contentMinSize());
                panel.setContentMaxSize(ns_window.contentMaxSize());
            }
            panel.setFrame_display(ns_window.frame(), false);
        }
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
        let pointer = Rc::new(PointerState::new(options.input.resolved(interactive), options.follow_pointer));
        let bridge = if ordinary { crate::platform::macos::panel_bridge::Bridge::new_window(&ns_window, &panel, &view) }
            else { crate::platform::macos::panel_bridge::Bridge::new(&ns_window, &panel, &view, Some(pointer.clone())) };
        if bridge.is_none() {
            panel.setContentView(None);
            ns_window.setContentView(Some(&view));
            return None;
        }
        panel.setDelegate(unsafe { ns_window.delegate() }.as_deref());
        Some(Panel { bridge, monitors: RefCell::new(None), pointer, options: options.clone(), follow_pointer: Cell::new(options.follow_pointer), pointer_poll: Cell::new(None), presence, wake, panel, window: ns_window, view })
    }

    /// On the primary screen (the one with the menu bar), in AppKit's bottom-left coordinates.
    pub fn place(&self, at: crate::overlay::Placement, options: &OverlayOptions) {
        let Some(rect) = self.rect(at, options) else { return };
        if self.panel.frame() != rect {
            self.window.setFrame_display(rect, false);
            self.panel.setFrame_display(rect, true);
            if self.presence.get().visible() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
        }
    }

    pub fn rect(&self, at: crate::overlay::Placement, options: &OverlayOptions) -> Option<NSRect> {
        overlay_rect_on(at, options, self.panel.screen().map(|s| screen_id(&s)))
    }

    /// Reattaches winit's view if something outside Rune took it from the panel, here or in
    /// `sendEvent:`. Clicks recorded while it was gone were never delivered, so native capture
    /// starts over, and a press the runtime still holds is released where the pointer was.
    fn keep_view(&self) {
        // SAFETY: plain readbacks on the main thread; nothing is retained.
        let (content, window): (*const NSView, *const NSWindow) = unsafe {
            (msg_send![&*self.panel, contentView], msg_send![&*self.view, window])
        };
        let attached = std::ptr::eq(content, &*self.view) && std::ptr::eq(window.cast::<NSPanel>(), &*self.panel);
        if !attached && crate::platform::macos::panel_bridge::reattach(&self.panel) { eprintln!("rune: panel lost its view; reattached"); }
        if crate::platform::macos::panel_bridge::take_reattached(&self.panel) { self.pointer.view_lost(); }
    }

    /// Whether none of the panel is on screen: covered, or its displays asleep. Reads as
    /// visible during a prewarm acquisition.
    pub fn occluded(&self) -> bool {
        occlusion_hidden(&self.panel)
    }

    /// The panel's AppKit window level (`NSWindow.Level`), changeable while shown.
    pub fn set_level(&self, level: isize) { self.panel.setLevel(level); }

    /// The moved view, for a new wgpu surface (winit's window handle would look for the view
    /// in winit's window, where it no longer is).
    pub fn raw_handles(&self) -> (winit::raw_window_handle::RawDisplayHandle, winit::raw_window_handle::RawWindowHandle) {
        use winit::raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
        let view = std::ptr::NonNull::from(&*self.view).cast();
        (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(view)))
    }

    pub fn window_number(&self) -> u32 { unsafe { self.panel.windowNumber() as u32 } }
    pub fn capture_exclusion(&self) -> OverlaySupport {
        if self.presence.get().requested() { OverlaySupport::Requested } else { OverlaySupport::Unknown }
    }
    pub fn request_keys(&self) {
        if self.options.keys == KeyPolicy::OnDemand {
            self.panel.makeFirstResponder(Some(&self.view));
            unsafe { self.panel.makeKeyWindow(); }
        }
    }
    pub fn is_key(&self) -> bool { self.panel.isKeyWindow() }
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
