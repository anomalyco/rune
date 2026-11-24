//! `NSStatusBar` / `NSStatusItem` on macOS. Primary clicks send `TrayEvent::Activate`;
//! secondary clicks and Control-clicks open the flat `NSMenu`, whose picks send
//! `TrayEvent::Menu`. `Tray::new` must run on the main thread; `set_menu`, `set_tooltip` and
//! `Drop` may be called from any thread (AppKit calls dispatch to the main queue when off the
//! main thread).

#![allow(unsafe_code)]

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::AnyObject,
    sel,
};
use objc2_app_kit::{
    NSApplication, NSBitmapFormat, NSBitmapImageRep, NSColorSpace, NSDeviceRGBColorSpace, NSEvent,
    NSEventMask, NSEventModifierFlags, NSEventType, NSImage, NSImageScaling, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusBarButton, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSSize, NSString};

use super::{
    Handler, MacClickAction, TrayError, TrayEvent, TrayIcon, TrayItem, TraySpec,
    classify_mac_click, macos_icon_point_size, premultiply_rgba,
};

const DEFAULT_BAR_THICKNESS: f64 = 22.0;

struct Model {
    tooltip: String,
    menu: Vec<TrayItem>,
}

/// Thin platform seam around `NSStatusBar` / `NSStatusItem` / `NSStatusBarButton`, so headless
/// tests can drive image construction, menu building, click routing, updates and teardown without
/// creating a visible item in the live macOS menu bar.
trait StatusItemHost: 'static {
    fn set_image(&self, image: &NSImage);
    fn set_tooltip(&self, tooltip: Option<&str>);
    fn pop_up_menu(&self, menu: &NSMenu);
    fn remove(&self);
}

struct SystemStatusItem {
    bar: Retained<NSStatusBar>,
    item: Retained<NSStatusItem>,
    button: Retained<NSStatusBarButton>,
    removed: Cell<bool>,
}

impl SystemStatusItem {
    fn new(
        mtm: MainThreadMarker,
        bar: Retained<NSStatusBar>,
        target: &ButtonTarget,
    ) -> Result<Self, TrayError> {
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        let Some(button) = item.button(mtm) else {
            bar.removeStatusItem(&item);
            return Err(TrayError::Backend("NSStatusItem has no button".into()));
        };
        button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        // SAFETY: main-thread AppKit button; `target` is retained in `ITEMS` until `remove`
        // clears both target and action.
        unsafe {
            button.setTarget(Some(target));
            button.setAction(Some(sel!(runeTrayClick:)));
        }
        let _ = button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        Ok(Self { bar, item, button, removed: Cell::new(false) })
    }
}

impl StatusItemHost for SystemStatusItem {
    fn set_image(&self, image: &NSImage) {
        self.button.setImage(Some(image));
    }

    fn set_tooltip(&self, tooltip: Option<&str>) {
        let tip = tooltip.filter(|s| !s.is_empty()).map(NSString::from_str);
        self.button.setToolTip(tip.as_deref());
    }

    fn pop_up_menu(&self, menu: &NSMenu) {
        if self.removed.get() {
            return;
        }
        self.item.setMenu(Some(menu));
        // SAFETY: main-thread status bar button; with `item.menu` set, `performClick:` opens
        // and tracks the status item's menu synchronously with native highlight and placement.
        unsafe { self.button.performClick(None) };
        self.item.setMenu(None);
    }

    fn remove(&self) {
        if self.removed.replace(true) {
            return;
        }
        // SAFETY: main-thread button; clear the non-retaining target before dropping it.
        unsafe {
            self.button.setTarget(None);
            self.button.setAction(None);
        }
        self.item.setMenu(None);
        self.bar.removeStatusItem(&self.item);
    }
}

struct LiveItem {
    host: Rc<dyn StatusItemHost>,
    target: Retained<ButtonTarget>,
    model: Arc<Mutex<Model>>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ITEMS: RefCell<HashMap<u64, LiveItem>> = RefCell::new(HashMap::new());
}

struct MenuEntry {
    id: String,
    enabled: bool,
}

struct MenuIvars {
    entries: Vec<MenuEntry>,
    on_event: Handler,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneTrayMenuTarget"]
    #[ivars = MenuIvars]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(runeTrayMenuPick:))]
        fn pick(&self, sender: &NSMenuItem) {
            let tag = sender.tag();
            let picked = usize::try_from(tag)
                .ok()
                .and_then(|i| self.ivars().entries.get(i))
                .filter(|entry| entry.enabled)
                .map(|entry| entry.id.clone());
            if let Some(id) = picked {
                dispatch_event(&self.ivars().on_event, TrayEvent::Menu(id));
            }
        }
    }
);

struct ButtonIvars {
    id: u64,
    model: Arc<Mutex<Model>>,
    on_event: Handler,
    showing_menu: Cell<bool>,
    last_menu: RefCell<Option<(Retained<NSMenu>, Retained<MenuTarget>)>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneTrayButtonTarget"]
    #[ivars = ButtonIvars]
    struct ButtonTarget;

    impl ButtonTarget {
        #[unsafe(method(runeTrayClick:))]
        fn click(&self, _sender: Option<&AnyObject>) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let event = NSApplication::sharedApplication(mtm).currentEvent();
            self.handle_click(mtm, event_action(event.as_deref()));
        }
    }
);

struct ShowingMenuGuard<'a>(&'a Cell<bool>);
impl Drop for ShowingMenuGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl ButtonTarget {
    fn handle_click(&self, mtm: MainThreadMarker, action: MacClickAction) {
        let ivars = self.ivars();
        if ivars.showing_menu.get() {
            return;
        }
        match action {
            MacClickAction::Activate => {
                dispatch_event(&ivars.on_event, TrayEvent::Activate { token: None });
            }
            MacClickAction::ShowMenu => {
                let items = ivars.model.lock().unwrap_or_else(|e| e.into_inner()).menu.clone();
                if items.is_empty() {
                    return;
                }
                let (menu, menu_target) = build_menu(mtm, &items, ivars.on_event.clone());
                let previous = ivars.last_menu.borrow_mut().replace((menu.clone(), menu_target));
                if let Some((old_menu, _)) = previous {
                    clear_menu_targets(&old_menu);
                }
                let host = ITEMS.with(|map| map.borrow().get(&ivars.id).map(|live| live.host.clone()));
                let Some(host) = host else { return };
                ivars.showing_menu.set(true);
                let _guard = ShowingMenuGuard(&ivars.showing_menu);
                host.pop_up_menu(&menu);
            }
        }
    }
}

fn dispatch_event(on_event: &Handler, event: TrayEvent) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_event(event))).is_err() {
        eprintln!("rune: tray callback panicked");
    }
}

fn event_action(event: Option<&NSEvent>) -> MacClickAction {
    let Some(event) = event else { return MacClickAction::Activate };
    let is_right = matches!(event.r#type(), NSEventType::RightMouseUp | NSEventType::RightMouseDown);
    let control = event.modifierFlags().contains(NSEventModifierFlags::Control);
    classify_mac_click(is_right, control)
}

fn valid_thickness(thickness: f64) -> f64 {
    if thickness.is_finite() && thickness > 0.0 { thickness } else { DEFAULT_BAR_THICKNESS }
}

fn accessibility_label(spec: &TraySpec) -> &str {
    if !spec.title.is_empty() { &spec.title } else { &spec.tooltip }
}

fn non_empty(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

/// Builds a premultiplied sRGB `NSBitmapImageRep` wrapped in an `NSImage` sized in points to
/// `thickness` high while preserving the pixel buffer's aspect ratio.
fn make_image(icon: &TrayIcon, thickness: f64, description: &str) -> Result<Retained<NSImage>, TrayError> {
    let Some((pt_w, pt_h)) = macos_icon_point_size(icon.width, icon.height, thickness) else {
        return Err(TrayError::Backend("tray icon dimensions must be non-zero".into()));
    };
    let w = isize::try_from(icon.width)
        .map_err(|_| TrayError::Backend("tray icon width overflow".into()))?;
    let h = isize::try_from(icon.height)
        .map_err(|_| TrayError::Backend("tray icon height overflow".into()))?;
    let row_bytes = w
        .checked_mul(4)
        .ok_or_else(|| TrayError::Backend("tray icon row size overflow".into()))?;
    let expected = usize::try_from(h)
        .ok()
        .and_then(|rows| usize::try_from(row_bytes).ok()?.checked_mul(rows));
    if expected != Some(icon.rgba.len()) {
        return Err(TrayError::Backend("tray icon size doesn't match its pixels".into()));
    }
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: null planes asks NSBitmapImageRep to allocate its own width × height × 4 buffer;
        // NSDeviceRGBColorSpace is an immutable AppKit constant.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                w,
                h,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                NSBitmapFormat::empty(),
                row_bytes,
                32,
            )
        }
        .ok_or_else(|| TrayError::Backend("could not allocate tray bitmap".into()))?;
        let data = rep.bitmapData();
        let stride = rep.bytesPerRow();
        if data.is_null() || stride < row_bytes {
            return Err(TrayError::Backend("tray bitmap has no pixel buffer".into()));
        }
        let src_row_bytes = icon.width as usize * 4;
        for (y, row) in icon.rgba.chunks_exact(src_row_bytes).enumerate() {
            // SAFETY: `rep` owns `h` rows of `stride >= src_row_bytes` bytes starting at `data`.
            let dst = unsafe {
                std::slice::from_raw_parts_mut(data.offset(y as isize * stride), src_row_bytes)
            };
            premultiply_rgba(row, dst);
        }
        let rep = rep
            .bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())
            .unwrap_or(rep);
        let point_size = NSSize::new(pt_w, pt_h);
        rep.setSize(point_size);
        let image = NSImage::initWithSize(NSImage::alloc(), point_size);
        image.addRepresentation(&rep);
        image.setTemplate(icon.template);
        if !description.is_empty() {
            image.setAccessibilityDescription(Some(&NSString::from_str(description)));
        }
        Ok(image)
    })
}

fn build_menu(
    mtm: MainThreadMarker,
    items: &[TrayItem],
    on_event: Handler,
) -> (Retained<NSMenu>, Retained<MenuTarget>) {
    let entries: Vec<MenuEntry> = items
        .iter()
        .filter_map(|item| match item {
            TrayItem::Action { id, enabled, .. } => {
                Some(MenuEntry { id: id.clone(), enabled: *enabled })
            }
            TrayItem::Separator => None,
        })
        .collect();
    // SAFETY: main-thread NSObject initialization immediately following set_ivars.
    let target: Retained<MenuTarget> =
        unsafe { msg_send![super(mtm.alloc().set_ivars(MenuIvars { entries, on_event })), init] };
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    crate::native_menu::set_layout_direction(&menu);
    let target_obj: &AnyObject = &target;
    let mut tag = 0isize;
    for item in items {
        match item {
            TrayItem::Action { label, enabled, .. } => {
                // SAFETY: main-thread NSMenuItem; `target` is kept alive in `ButtonTarget::last_menu`
                // until replaced or removed, and `clear_menu_targets` clears it before release.
                let ns_item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        mtm.alloc(),
                        &NSString::from_str(label),
                        Some(sel!(runeTrayMenuPick:)),
                        &NSString::from_str(""),
                    )
                };
                // SAFETY: `target_obj` responds to `runeTrayMenuPick:`.
                unsafe { ns_item.setTarget(Some(target_obj)) };
                ns_item.setTag(tag);
                tag += 1;
                ns_item.setEnabled(*enabled);
                menu.addItem(&ns_item);
            }
            TrayItem::Separator => {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
        }
    }
    (menu, target)
}

fn clear_menu_targets(menu: &NSMenu) {
    for i in 0..menu.numberOfItems() {
        let Some(item) = menu.itemAtIndex(i) else { continue };
        // SAFETY: main-thread menu item; clearing target/action prevents sends after release.
        unsafe {
            item.setTarget(None);
            item.setAction(None);
        }
    }
}

fn alloc_button_target(
    mtm: MainThreadMarker,
    id: u64,
    model: Arc<Mutex<Model>>,
    on_event: Handler,
) -> Retained<ButtonTarget> {
    let ivars = ButtonIvars {
        id,
        model,
        on_event,
        showing_menu: Cell::new(false),
        last_menu: RefCell::new(None),
    };
    // SAFETY: main-thread NSObject initialization immediately following set_ivars.
    unsafe { msg_send![super(mtm.alloc().set_ivars(ivars)), init] }
}

fn install_native(
    id: u64,
    model: Arc<Mutex<Model>>,
    target: Retained<ButtonTarget>,
    host: Rc<dyn StatusItemHost>,
    image: &NSImage,
) -> Native {
    host.set_image(image);
    let initial_tip = model.lock().unwrap_or_else(|e| e.into_inner()).tooltip.clone();
    host.set_tooltip(non_empty(&initial_tip));
    ITEMS.with(|map| {
        map.borrow_mut().insert(id, LiveItem { host, target, model: model.clone() });
    });
    Native { id, model }
}

fn apply_tooltip(id: u64) {
    let entry = ITEMS.with(|map| {
        map.borrow().get(&id).map(|live| (live.host.clone(), live.model.clone()))
    });
    let Some((host, model)) = entry else { return };
    let tip = model.lock().unwrap_or_else(|e| e.into_inner()).tooltip.clone();
    host.set_tooltip(non_empty(&tip));
}

fn remove_item(id: u64) {
    let Some(live) = ITEMS.with(|map| map.borrow_mut().remove(&id)) else { return };
    if let Some((menu, _)) = live.target.ivars().last_menu.borrow_mut().take() {
        clear_menu_targets(&menu);
    }
    live.host.remove();
}

pub(super) struct Native {
    id: u64,
    model: Arc<Mutex<Model>>,
}

impl Native {
    pub(super) fn new(spec: TraySpec, on_event: Handler) -> Result<Self, TrayError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            TrayError::Backend("macOS tray must be created on the main thread".into())
        })?;
        let bar = NSStatusBar::systemStatusBar();
        let thickness = valid_thickness(bar.thickness());
        let image = make_image(&spec.icon, thickness, accessibility_label(&spec))?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let model = Arc::new(Mutex::new(Model { tooltip: spec.tooltip, menu: spec.menu }));
        let target = alloc_button_target(mtm, id, model.clone(), on_event);
        let host = Rc::new(SystemStatusItem::new(mtm, bar, &target)?);
        Ok(install_native(id, model, target, host, &image))
    }

    pub(super) fn set_menu(&self, menu: Vec<TrayItem>) {
        self.model.lock().unwrap_or_else(|e| e.into_inner()).menu = menu;
    }

    pub(super) fn set_tooltip(&self, tooltip: String) {
        self.model.lock().unwrap_or_else(|e| e.into_inner()).tooltip = tooltip;
        let id = self.id;
        if MainThreadMarker::new().is_some() {
            apply_tooltip(id);
        } else {
            crate::platform::main_queue::on_main(move || apply_tooltip(id));
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        let id = self.id;
        if MainThreadMarker::new().is_some() {
            remove_item(id);
        } else {
            crate::platform::main_queue::on_main(move || remove_item(id));
        }
    }
}

#[cfg(test)]
#[path = "macos_tests.rs"]
mod tests;
