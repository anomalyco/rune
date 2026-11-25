//! AppKit activation policy, overlay windows, and visibility without activation.

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2::{ClassType, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSPanel, NSWindow, NSResponder, NSScreen, NSView};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use crate::{KeyPolicy, OverlayInput, OverlayLevel, OverlayOptions, OverlaySupport};
use std::{cell::{Cell, RefCell}, rc::Rc};

#[doc(hidden)]
pub mod appearance;
pub(crate) mod cursor;
pub(crate) mod document;
pub(crate) mod drop;
mod geometry;
mod hold;
pub(crate) mod help;
pub(crate) mod launch;
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
pub(crate) use presentation::Prewarm;
#[cfg(test)]
pub(crate) use surface::layer_occluded;
pub(crate) use surface::{PresumeShown, flush_while_launching, launching, pin_contents_top_left, present_with_transaction, surface_occluded, sync_contents_scale, window_occluded};
use geometry::{overlay_rect_on, screen_id};
use pointer::MouseMonitors;
use presence::Presence;
use surface::{apply_window_color_space, occlusion_hidden};

#[allow(unsafe_code)]
mod overlay_panel {
    use super::*;

    pub(super) struct PanelFlags {
        pub(super) keys: bool,
        pub(super) presence: Rc<Cell<Presence>>,
        pub(super) constrain: bool,
    }

    // Initialized nonactivating panel. No application activation call.
    define_class!(
        #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
        #[name = "RuneOverlayPanel"]
        #[ivars = PanelFlags]
        pub(super) struct OverlayPanel;

        impl OverlayPanel {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool { self.ivars().keys }
            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool { false }
            #[unsafe(method(occlusionState))]
            fn occlusion_state(&self) -> usize {
                // wgpu skips nextDrawable for occluded windows. Permit only the explicit
                // alpha-zero prewarm acquisition and a fade in or out, which AppKit reports as
                // occluded while its alpha is near zero; other visibility readbacks stay native.
                if self.ivars().presence.get().draws_occluded() {
                    2
                } else {
                    // SAFETY: `OverlayPanel` inherits from `NSPanel` (`MainThreadOnly`), whose `occlusionState` returns `NSWindowOcclusionState` (`usize`).
                    unsafe { msg_send![super(self), occlusionState] }
                }
            }
            // AppKit keeps windows below the menu bar's level out from under it; an overlay goes
            // exactly where it's placed at any level.
            #[unsafe(method(constrainFrameRect:toScreen:))]
            fn constrain_frame_rect(&self, frame: NSRect, screen: *mut NSScreen) -> NSRect {
                // SAFETY: `OverlayPanel` inherits from `NSPanel` (`MainThreadOnly`); `frame` and `screen` match `constrainFrameRect:toScreen:`.
                if self.ivars().constrain { unsafe { msg_send![super(self), constrainFrameRect: frame, toScreen: screen] } }
                else { frame }
            }
            #[unsafe(method(sendEvent:))]
            fn send_event(&self, event: &NSEvent) {
                // Without its view the panel would swallow this click and every later one.
                if crate::platform::macos::panel_bridge::reattach(self.as_super()) { eprintln!("rune: panel lost its view; reattached on input"); }
                // SAFETY: `OverlayPanel` inherits from `NSPanel` (`MainThreadOnly`); `event` is the live `NSEvent` passed by AppKit.
                unsafe { let _: () = msg_send![super(self), sendEvent: event]; }
            }
        }
    );
}
use overlay_panel::{OverlayPanel, PanelFlags};

/// Whether the user is dragging the window's edge.
#[allow(unsafe_code)]
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
#[allow(unsafe_code)]
pub(crate) fn content_view(window: &winit::window::Window) -> Option<Retained<objc2_app_kit::NSView>> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: winit's content view is alive as long as its window, which the caller holds.
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }
}

pub(crate) fn ns_window(window: &winit::window::Window) -> Option<Retained<NSWindow>> {
    content_view(window)?.window()
}

/// Whether `window` is an `NSPanel` instance (an overlay or an ordinary window adopted by
/// `Panel::adopt_window`).
#[allow(unsafe_code)]
pub(crate) fn is_panel_object(window: &objc2::runtime::AnyObject) -> bool {
    // SAFETY: `window` is a live ObjC instance; `isKindOfClass:` takes a `Class` and returns `BOOL`.
    let panel: objc2::runtime::Bool = unsafe { msg_send![window, isKindOfClass: NSPanel::class()] };
    panel.as_bool()
}

/// Whether `window`'s content view currently lives in an `NSPanel`.
pub(crate) fn is_panel(window: &winit::window::Window) -> bool {
    ns_window(window).is_some_and(|w| is_panel_object(&w))
}

/// Whether key-value observing (or another runtime subclass that hides itself from `-class`)
/// sits on `object`. Placing a Rune per-instance subclass above KVO's crashes Foundation when
/// an observer is removed.
#[allow(unsafe_code)]
pub(crate) unsafe fn kvo_observed(object: &objc2::runtime::AnyObject) -> bool {
    // SAFETY: caller guarantees `object` is a live ObjC instance; `-class` takes no arguments.
    let reported: &objc2::runtime::AnyClass = unsafe { msg_send![object, class] };
    let actual = object.class();
    !std::ptr::eq(reported, actual) || actual.name().to_bytes().starts_with(b"NSKVONotifying_")
}

/// Whether any class in `object`'s runtime superclass chain starts with `prefix`.
pub(crate) fn has_subclass(object: &objc2::runtime::AnyObject, prefix: &[u8]) -> bool {
    std::iter::successors(Some(object.class()), |c| c.superclass())
        .any(|c| c.name().to_bytes().starts_with(prefix))
}

pub(crate) fn configure_window(window: &winit::window::Window, options: &crate::MacWindowOptions) {
    crate::platform::macos::appearance::install();
    if let Some(window) = ns_window(window) { configure_native(&window, options); }
}

#[allow(unsafe_code)]
fn configure_native(window: &NSWindow, options: &crate::MacWindowOptions) {
    if let Some(shadow) = options.shadow { window.setHasShadow(shadow); }
    if let Some(level) = options.level { window.setLevel(level); }
    apply_window_color_space(window, options.color_space);
    // SAFETY: every caller runs on the AppKit main thread.
    unsafe {
        // AppKit saves nothing of a window it could not restore (`launch::ignore_saved_windows`).
        if launch::ignores_saved_windows() { let _: () = msg_send![window, setRestorable: false]; }
        if let Some(bits) = options.collection_behavior {
            let _: () = msg_send![window, setCollectionBehavior: bits];
        }
    }
    if let Some(exclude) = options.exclude_from_capture { set_sharing(window, exclude); }
    if let Some(path) = options.represented_file.as_deref() {
        document::apply_represented_file(window, Some(path), options.title_from_file, None);
    }
    if options.document_edited {
        document::apply_document_edited(window, true);
    }
}

/// `sharingType` none keeps the window out of captures; read-only lets recorders see it.
/// Captures follow every change, but on recent macOS the `sharingType` getter and
/// CGWindowList's sharing state can keep reporting none after the first switch back, so
/// neither confirms a toggle.
#[allow(unsafe_code)]
fn set_sharing(window: &NSWindow, exclude: bool) {
    // SAFETY: every caller runs on the AppKit main thread.
    unsafe { let _: () = msg_send![window, setSharingType: if exclude { 0usize } else { 1usize }]; }
}

/// Sets or clears capture exclusion on an ordinary macOS window.
pub(crate) fn set_exclude_from_capture(window: &winit::window::Window, exclude: bool) {
    if let Some(window) = ns_window(window) { set_sharing(&window, exclude); }
}

/// Orders a window front without making it key or activating the app.
pub(crate) fn show_without_focus(window: &winit::window::Window) {
    let Some(w) = ns_window(window) else { return };
    w.orderFrontRegardless();
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
    /// Prewarmed, shown or fading, under one fence, and the requested capture exclusion,
    /// applied only while shown. While a fade runs the panel reads as visible, so a fading-out
    /// overlay can still draw its exit.
    presence: Rc<Cell<Presence>>,
    wake: Option<crate::Waker>,
    ordinary: bool,
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

    #[allow(unsafe_code)]
    fn adopt_kind(window: &winit::window::Window, at: crate::overlay::Placement, interactive: bool, options: &OverlayOptions, wake: Option<crate::Waker>, ordinary: bool) -> Option<Panel> {
        use objc2_app_kit::{NSBackingStoreType, NSColor, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask};
        let mtm = MainThreadMarker::new()?;
        crate::platform::macos::appearance::install();
        let view = content_view(window)?;
        let ns_window = view.window()?;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(at.size.0 as f64, at.size.1 as f64));
        let presence = Rc::new(Cell::new(Presence::new(options.exclude_from_capture)));
        let style = if ordinary { ns_window.styleMask() } else { NSWindowStyleMask::Borderless };
        // SAFETY: `mtm` proves main-thread execution; `OverlayPanel` inherits from `NSPanel`, so
        // `initWithContentRect:styleMask:backing:defer:` and the upcast to `NSPanel` are valid.
        let panel = unsafe {
            let panel: Retained<OverlayPanel> = msg_send![super(mtm.alloc().set_ivars(PanelFlags { keys: options.keys == KeyPolicy::OnDemand, presence: presence.clone(), constrain: ordinary })),
                initWithContentRect: rect,
                styleMask: style | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false];
            panel.into_super()
        };
        // SAFETY: `panel` is a live retained `NSPanel` configured on the AppKit main thread (`mtm` checked above).
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
            panel.setTitleVisibility(ns_window.titleVisibility());
            panel.setTitlebarAppearsTransparent(ns_window.titlebarAppearsTransparent());
            panel.setLevel(3);
            panel.setHasShadow(ns_window.hasShadow());
            panel.setContentMinSize(ns_window.contentMinSize());
            panel.setContentMaxSize(ns_window.contentMaxSize());
            panel.setFrame_display(ns_window.frame(), false);
        }
        panel.setOpaque(false);
        panel.setIgnoresMouseEvents(options.input.resolved(interactive) != OverlayInput::All);
        panel.setAcceptsMouseMovedEvents(true);
        if options.prewarm {
            panel.setAlphaValue(0.0);
            panel.setIgnoresMouseEvents(true);
            // AppKit's default animation scales and fades an NSPanel in over ~280 ms on
            // order-in; a prewarmed panel reveals cached pixels at once (or runs its own fade).
            panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        }
        // defer:NO creates the WindowServer identity while hidden. Never publish a sentinel
        // (-1/0) cast to CGWindowID as a successful native creation.
        if panel.windowNumber() <= 0 { return None; }
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
        panel.setDelegate(ns_window.delegate().as_deref());
        Some(Panel { bridge, monitors: RefCell::new(None), pointer, options: options.clone(), follow_pointer: Cell::new(options.follow_pointer), presence, wake, ordinary, panel, window: ns_window, view })
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

    /// Resizes the panel in place, keeping its top-left corner fixed (for a user-dragged overlay).
    pub fn resize(&self, size: (f32, f32)) {
        let mut rect = self.panel.frame();
        let next = NSSize::new(f64::from(size.0), f64::from(size.1));
        if rect.size == next { return; }
        rect.origin.y += rect.size.height - next.height;
        rect.size = next;
        self.window.setFrame_display(rect, false);
        self.panel.setFrame_display(rect, true);
        if self.presence.get().visible() { self.pointer.observe(&self.panel, self.wake.as_ref()); }
    }

    /// Starts an interactive AppKit window drag on this panel from the current mouse-down event.
    /// Shared by overlay panels (whose hidden source `NSWindow` does not bridge geometry, so the
    /// source frame is synced here) and ordinary adopted panels (`Panel::adopt_window`, whose
    /// `panel_bridge` already forwards `frame` and `performWindowDragWithEvent:` to this panel).
    /// Returns whether the panel's frame moved during the drag.
    pub fn drag_window(&self) -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return false };
        let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else { return false };
        let before = self.panel.frame();
        self.panel.performWindowDragWithEvent(&event);
        let after = self.panel.frame();
        if after != before {
            if !self.ordinary { self.window.setFrame_display(after, false); }
            true
        } else {
            false
        }
    }

    pub fn rect(&self, at: crate::overlay::Placement, options: &OverlayOptions) -> Option<NSRect> {
        overlay_rect_on(at, options, self.panel.screen().map(|s| screen_id(&s)))
    }

    /// Reattaches winit's view if something outside Rune took it from the panel, here or in
    /// `sendEvent:`. Clicks recorded while it was gone were never delivered, so native capture
    /// starts over, and a press the runtime still holds is released where the pointer was.
    #[allow(unsafe_code)]
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

    /// Applies `scheme` to this panel's `NSWindow.appearance`.
    pub fn set_color_scheme(&self, scheme: crate::ColorScheme) {
        crate::platform::macos::appearance::set_window_appearance(&self.panel, scheme);
    }

    /// The moved view, for a new wgpu surface (winit's window handle would look for the view
    /// in winit's window, where it no longer is).
    pub fn raw_handles(&self) -> (winit::raw_window_handle::RawDisplayHandle, winit::raw_window_handle::RawWindowHandle) {
        use winit::raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
        let view = std::ptr::NonNull::from(&*self.view).cast();
        (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(view)))
    }

    pub fn window_number(&self) -> u32 { self.panel.windowNumber() as u32 }
    pub fn capture_exclusion(&self) -> OverlaySupport {
        if self.presence.get().requested() { OverlaySupport::Requested } else { OverlaySupport::Unknown }
    }
    pub fn request_keys(&self) {
        if self.options.keys == KeyPolicy::OnDemand {
            self.panel.makeFirstResponder(Some(&self.view));
            self.panel.makeKeyWindow();
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

thread_local! {
    /// Each window's count (`WindowHandle::set_taskbar`); the Dock shows their sum.
    static BADGES: std::cell::RefCell<std::collections::HashMap<u64, u32>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// `window`'s count for the Dock icon's badge, which is the app's: the sum over its windows, none
/// at 0. Main thread.
#[allow(unsafe_code)]
pub(crate) fn dock_badge(window: &winit::window::Window, count: u32) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let total: u32 = BADGES.with_borrow_mut(|b| {
        b.insert(u64::from(window.id()), count);
        b.values().sum()
    });
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: AppKit's Dock tile, on the main thread; nil clears the badge.
    unsafe {
        let tile: *mut objc2::runtime::AnyObject = msg_send![&app, dockTile];
        if tile.is_null() { return; }
        let label = (total > 0).then(|| objc2_foundation::NSString::from_str(&total.to_string()));
        let label: *const objc2::runtime::AnyObject = label.as_deref().map_or(std::ptr::null(), |l| (l as *const objc2_foundation::NSString).cast());
        let _: () = msg_send![tile, setBadgeLabel: label];
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
pub(crate) mod test_pasteboard {
    use objc2::rc::Retained;
    use objc2::{class, msg_send};
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
    use objc2_foundation::{NSArray, NSString};

    /// A private named pasteboard, released globally when dropped.
    pub(crate) struct Board(Retained<NSPasteboard>);

    impl Drop for Board {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a private named pasteboard created via `pasteboardWithName:` and released once on drop.
            unsafe { msg_send![&*self.0, releaseGlobally] }
        }
    }

    impl std::ops::Deref for Board {
        type Target = NSPasteboard;
        fn deref(&self) -> &NSPasteboard { &self.0 }
    }

    /// Creates a private named pasteboard unique to this call. `+[NSPasteboard pasteboardWithUniqueName]`
    /// stamps names from `mach_absolute_time`, so parallel tests that start within one tick receive
    /// the same `NSPasteboard` instance and overwrite each other's items.
    fn create() -> Option<Board> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = NSString::from_str(&format!("dev.rune.test.{}.{n}", std::process::id()));
        // SAFETY: `+[NSPasteboard pasteboardWithName:]` takes an `NSString` and returns a retained or
        // nil `NSPasteboard`; the private pasteboard is released once in `Board::drop`.
        let pb: Option<Retained<NSPasteboard>> = unsafe { msg_send![class!(NSPasteboard), pasteboardWithName: &*name] };
        pb.map(Board)
    }

    /// Whether this session has a pasteboard server. A headless or SSH-only session (no window
    /// server) has none, and `pasteboardWithName:` returns nil or refuses writes there, so tests
    /// skip with a note instead of panicking.
    pub(crate) fn available() -> bool {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            let probe = std::panic::catch_unwind(|| {
                let Some(pb) = create() else { return false };
                pb.clearContents();
                let item = NSPasteboardItem::new();
                let kind = NSString::from_str("public.utf8-plain-text");
                item.setString_forType(&NSString::from_str("probe"), &kind)
                    && pb.writeObjects(&NSArray::from_retained_slice(&[objc2::runtime::ProtocolObject::from_retained(item)]))
                    && pb.stringForType(&kind).is_some_and(|s| s.to_string() == "probe")
            });
            let ok = probe.unwrap_or(false);
            if !ok { eprintln!("macos: no pasteboard in this session (headless?); skipping pasteboard round-trip tests"); }
            ok
        })
    }

    /// A private named pasteboard unique to this call, or `None` without a pasteboard server.
    pub(crate) fn unique() -> Option<Board> {
        if !available() { return None; }
        create()
    }
}