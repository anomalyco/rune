//! NSAccessibility for a window: AccessKit subclasses winit's content view at run time to
//! answer for its children, focus and hit tests.

use objc2::runtime::{AnyClass, AnyObject};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{Actions, Activation};

pub(super) struct Adapter(accesskit_macos::SubclassingAdapter, DisplayOptions);

impl Adapter {
    pub(super) fn new(window: &winit::window::Window, activation: Activation, actions: Actions) -> Option<Self> {
        let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
        let view = h.ns_view.as_ptr();
        // AccessKit can't subclass a view key-value observing already subclassed (it would
        // panic): such a view keeps no tree, and says so.
        // SAFETY: `view` is winit's live NSView; `class` takes no arguments.
        let (reported, actual): (&AnyClass, &AnyClass) = unsafe {
            let object = &*(view as *const AnyObject);
            (objc2::msg_send![object, class], object.class())
        };
        if !std::ptr::eq(reported, actual) {
            eprintln!("rune: accessibility unavailable for a window: its view is already observed ({})", actual.name());
            return None;
        }
        let options = DisplayOptions::watch(activation.0.clone());
        // SAFETY: `view` is a valid NSView that outlives the adapter (the window owns it, and
        // hosts drop the adapter before the window).
        Some(Adapter(unsafe { accesskit_macos::SubclassingAdapter::new(view, activation, actions) }, options))
    }

    pub(super) fn update(&mut self, update: accesskit::TreeUpdate) {
        if let Some(events) = self.0.update_if_active(|| update) { events.raise(); }
    }

    pub(super) fn focus(&mut self, focused: bool) {
        if let Some(events) = self.0.update_view_focus_state(focused) { events.raise(); }
    }
}

/// The workspace's accessibility display options (Increase Contrast, Reduce Motion, Reduce
/// Transparency) changed: the runtime reads them again at once, as Windows' `WM_SETTINGCHANGE`
/// does (`windows.rs`), rather than at the window's next activation.
struct DisplayOptions {
    center: Option<objc2::rc::Retained<AnyObject>>,
    token: Option<objc2::rc::Retained<AnyObject>>,
}

impl DisplayOptions {
    fn watch(shared: std::sync::Arc<super::Shared>) -> Self {
        use objc2::{class, msg_send, msg_send_id};
        let block = block2::RcBlock::new(move |_: *mut AnyObject| shared.push(|i| i.settings = true));
        // SAFETY: main thread (the adapter is made there); NSWorkspace's notification center
        // lives for the process; the token is removed in Drop.
        unsafe {
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let center: Option<objc2::rc::Retained<AnyObject>> = msg_send_id![workspace, notificationCenter];
            let Some(center) = center else { return DisplayOptions { center: None, token: None } };
            let name = objc2_foundation::NSString::from_str("NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification");
            let token: Option<objc2::rc::Retained<AnyObject>> = msg_send_id![&*center, addObserverForName: &*name,
                object: std::ptr::null::<AnyObject>(), queue: std::ptr::null::<AnyObject>(), usingBlock: &*block];
            DisplayOptions { center: Some(center), token }
        }
    }
}

impl Drop for DisplayOptions {
    fn drop(&mut self) {
        if let (Some(center), Some(token)) = (&self.center, &self.token) {
            // SAFETY: the token came from this center's addObserverForName.
            unsafe { let _: () = objc2::msg_send![&**center, removeObserver: &**token]; }
        }
    }
}
