//! Main-thread AppKit adapter. Adds one optional selector to winit's existing delegate;
//! never replaces the delegate or any existing method. The hook returns nil after teardown.

#![allow(unsafe_code)]

use super::{Description, EntryKind, Location, MenuError};
use std::{cell::{Cell, RefCell}, ffi::{c_char, c_void}, sync::Arc};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel, rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, Sel}};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSString};

type Sink = Arc<dyn Fn(&str) + Send + Sync>;
struct Action { id: String, enabled: bool, keys_first: bool, sink: Sink }
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneNativeMenuTarget"]
    #[ivars = Action]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(runeMenuAction:))]
        fn perform(&self, _sender: &NSMenuItem) {
            let Action { id, enabled, keys_first, sink } = self.ivars();
            if *enabled && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (id, sink) = (id.clone(), sink.clone());
                let run: Arc<dyn Fn() + Send + Sync> = Arc::new(move || sink(&id));
                if !*keys_first || !super::standard_macos::defer_key(run.clone()) { run(); }
            })).is_err() { eprintln!("rune: native menu callback panicked"); }
        }
        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, _sender: &NSMenuItem) -> bool { self.ivars().enabled }
    }
);

thread_local! {
    static OWNED: Cell<bool> = const { Cell::new(false) };
    static DOCK: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    /// The jump list's Dock menu (`set_jump_list`), shown when the app's menus put nothing there.
    static JUMP: RefCell<Option<(Retained<NSMenu>, Vec<Retained<MenuTarget>>)>> = const { RefCell::new(None) };
    static HOOKED: Cell<*const AnyClass> = const { Cell::new(std::ptr::null()) };
}
unsafe extern "C" {
    fn class_addMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> Bool;
}
extern "C" fn dock_menu(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> *mut AnyObject {
    DOCK.with(|slot| slot.try_borrow().ok().and_then(|s| s.as_ref().map(|m| &**m as *const NSMenu as *mut AnyObject)))
        .or_else(|| JUMP.with(|slot| slot.try_borrow().ok().and_then(|s| s.as_ref().map(|(m, _)| &**m as *const NSMenu as *mut AnyObject))))
        .unwrap_or(std::ptr::null_mut())
}

/// The jump list as the Dock menu: its tasks, then each category under a disabled title. A pick
/// arrives as a link the app opens (`App::on_open_url`), as a Windows jump list entry does.
pub(crate) fn set_jump_menu(list: &crate::jump_list::JumpList) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    if install_dock_hook(&app).is_err() { return; }
    let sink: Sink = Arc::new(|argument: &str| crate::open_url::incoming::receive(argument.to_owned()));
    let menu = NSMenu::new(mtm);
    let mut targets = Vec::new();
    // SAFETY: AppKit calls on the main thread; each item's target is kept in JUMP for as long as
    // the menu holds the item.
    unsafe {
        let mut add = |title: &str, argument: Option<&str>| {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), argument.map(|_| sel!(runeMenuAction:)), &NSString::from_str(""));
            if let Some(argument) = argument {
                let target: Retained<MenuTarget> = msg_send![super(mtm.alloc().set_ivars(Action { id: argument.to_owned(), enabled: true, keys_first: false, sink: sink.clone() })), init];
                item.setTarget(Some(&target));
                targets.push(target);
            } else {
                item.setEnabled(false);
            }
            menu.addItem(&item);
        };
        for task in &list.tasks { add(&task.title, Some(&task.argument)); }
        for (name, items) in &list.categories {
            if menu.numberOfItems() > 0 { menu.addItem(&NSMenuItem::separatorItem(mtm)); }
            add(name, None);
            for item in items { add(&item.title, Some(&item.argument)); }
        }
    }
    JUMP.with(|slot| *slot.borrow_mut() = Some((menu, targets)));
}
fn install_dock_hook(app: &NSApplication) -> Result<(), MenuError> {
    // SAFETY: AppKit calls and Objective-C runtime mutation occur only on the main thread.
    // The added selector has the exact NSObject*, SEL, NSApplication* -> NSMenu* ABI.
    unsafe {
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let delegate = delegate.as_ref().ok_or_else(|| MenuError::Backend("missing application delegate".into()))?;
        let class = delegate.class();
        if HOOKED.with(|slot| std::ptr::eq(slot.get(), class)) { return Ok(()); }
        let selector = sel!(applicationDockMenu:);
        let exists: bool = msg_send![delegate, respondsToSelector: selector];
        if exists { return Err(MenuError::Backend("application delegate already supplies a Dock menu".into())); }
        if !class_addMethod(class, selector, dock_menu as *const c_void, c"@@:@".as_ptr()).as_bool() {
            return Err(MenuError::Backend("could not install Dock menu hook".into()));
        }
        HOOKED.with(|slot| slot.set(class));
        // Refresh AppKit's optional-selector cache, retaining the exact winit delegate.
        let _: () = msg_send![app, setDelegate: delegate];
    }
    Ok(())
}

pub(super) struct Native {
    mtm: MainThreadMarker,
    entries: Vec<(Retained<NSMenu>, Retained<NSMenuItem>)>,
    /// Top-level menus it made, in their menu bars.
    tops: Vec<(Retained<NSMenu>, Retained<NSMenuItem>)>,
    targets: Vec<Retained<MenuTarget>>,
}
impl Native {
    pub(super) fn new() -> Result<Self, MenuError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| MenuError::Backend("menus require the main thread".into()))?;
        if OWNED.with(|owned| owned.replace(true)) { return Err(MenuError::InUse); }
        Ok(Self { mtm, entries: Vec::new(), tops: Vec::new(), targets: Vec::new() })
    }
    fn clear(&mut self) {
        for (menu, item) in self.entries.drain(..) {
            // SAFETY: AppKit's target is non-retaining. Clear targets recursively through any
            // nested submenus before releasing our targets, even if a currently tracking menu
            // keeps removed items alive.
            unsafe { clear_item_targets(&item); menu.removeItem(&item); }
        }
        // A menu the standard items joined stays for them.
        for (root, top) in self.tops.drain(..) { super::default_macos::remove_top_if_empty(&root, &top); }
        self.targets.clear();
        DOCK.with(|slot| *slot.borrow_mut() = None);
        super::default_macos::sync_standard(&NSApplication::sharedApplication(self.mtm));
    }
    pub(super) fn set(&mut self, descriptions: &[Description], sink: Sink) -> Result<(), MenuError> {
        // SAFETY: self carries a main-thread marker. All menus/items are owned here or
        // retained from AppKit, indices are bounded, and targets outlive their items.
        unsafe {
        let app = NSApplication::sharedApplication(self.mtm);
        if descriptions.iter().any(|d| d.meta.location == Location::Dock) { install_dock_hook(&app)?; }
        // Construct all items, nested submenus, and targets before removing the previous snapshot.
        let mut new_targets = Vec::new();
        let prepared: Vec<_> = descriptions.iter().map(|d| {
            let item = build_item(self.mtm, d, true, &sink, &mut new_targets);
            (d, item)
        }).collect();
        self.clear();
        // Ensure the default menu bar exists before `adding` releases yieldable keys (including
        // built-in Quit's ⌘Q), so a custom item claiming ⌘Q never collides with `terminate:`.
        if descriptions.iter().any(|d| d.meta.location != Location::Dock) {
            let _ = super::ensure_default_menu(&app);
        }
        super::default_macos::adding(&app, || {
            let mut last_app_item: Option<Retained<NSMenuItem>> = None;
            let mut custom_about = false;
            // Items placed so far by id, and the last one placed after each anchor.
            let mut placed: Vec<(&str, Retained<NSMenuItem>)> = Vec::new();
            let mut chained: Vec<(&str, Retained<NSMenuItem>)> = Vec::new();
            for (d, item) in prepared {
                if d.meta.replaces_about { item.setTag(super::default_macos::ABOUT_REPLACEMENT); }
                let location = &d.meta.location;
                let menu = if *location == Location::Dock {
                    DOCK.with(|slot| slot.borrow_mut().get_or_insert_with(|| NSMenu::new(self.mtm)).clone())
                } else {
                    let root = super::ensure_default_menu(&app);
                    // An app-supplied empty root is preserved. Reserve its first slot for
                    // the app menu even when a named-menu item appears first in the snapshot.
                    if root.numberOfItems() == 0 { self.add_top(&root, "Application"); }
                    match location {
                        Location::App => {
                            if let Some(first) = (root.numberOfItems() > 0).then(|| root.itemAtIndex(0)).flatten().and_then(|i| i.submenu()) { first }
                            else { self.add_top(&root, "Application") }
                        }
                        Location::Named(name) => {
                            let existing = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i))
                                .find(|i| super::default_macos::name(i) == *name).and_then(|i| i.submenu());
                            existing.unwrap_or_else(|| self.add_top(&root, name))
                        }
                        Location::Dock => unreachable!(),
                    }
                };
                let anchor = d.meta.after.as_deref().and_then(|a| anchor_index(&menu, a, &placed, &chained).map(|i| (a, i)));
                if d.meta.replaces_about {
                    menu.insertItem_atIndex(&item, 0);
                    custom_about = true;
                } else if let Some((a, index)) = anchor {
                    menu.insertItem_atIndex(&item, index + 1);
                    chained.retain(|(c, _)| *c != a);
                    chained.push((a, item.clone()));
                } else if *location == Location::App {
                    // App items follow the app menu's About item (custom or standard), where
                    // AppKit apps list Check for Updates… and Settings…; an app menu without
                    // one takes them first.
                    let index = last_app_item.as_ref()
                        .map(|prev| menu.indexOfItem(prev))
                        .filter(|&i| i >= 0)
                        .map(|i| i + 1)
                        .unwrap_or_else(|| {
                            let first = (menu.numberOfItems() > 0).then(|| menu.itemAtIndex(0)).flatten();
                            let about = custom_about || first.and_then(|i| i.action()) == Some(sel!(orderFrontStandardAboutPanel:));
                            isize::from(about)
                        });
                    menu.insertItem_atIndex(&item, index);
                    last_app_item = Some(item.clone());
                } else {
                    menu.addItem(&item);
                }
                if !matches!(d.kind, EntryKind::Separator) { placed.push((&d.meta.id, item.clone())); }
                self.entries.push((menu, item));
            }
        });
        self.targets = new_targets;
        // Each menu reads the app's way; the bar keeps the system's order of its menus.
        if let Some(root) = app.mainMenu() {
            for top in (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).filter_map(|i| i.submenu()) {
                super::set_layout_direction(&top);
            }
        }
        DOCK.with(|slot| {
            if let Some(dock) = slot.borrow().as_ref() {
                super::set_layout_direction(dock);
            }
        });
        Ok(())
        }
    }
    fn add_top(&mut self, root: &Retained<NSMenu>, name: &str) -> Retained<NSMenu> {
        let (top, menu) = super::default_macos::add_top(self.mtm, root, name);
        self.tops.push((root.clone(), top));
        menu
    }
}

/// Builds one `NSMenuItem` (leaf action, separator, or recursive submenu), appending every
/// created `MenuTarget` to `targets`. `ancestors_enabled` is false whenever an enclosing submenu
/// is disabled, so nested targets refuse to validate or fire even via key equivalents.
unsafe fn build_item(
    mtm: MainThreadMarker,
    d: &Description,
    ancestors_enabled: bool,
    sink: &Sink,
    targets: &mut Vec<Retained<MenuTarget>>,
) -> Retained<NSMenuItem> {
    // SAFETY: caller holds a main-thread marker; every MenuTarget is pushed into `targets`
    // and kept alive until `clear_item_targets` detaches it.
    unsafe {
        match &d.kind {
            EntryKind::Separator => NSMenuItem::separatorItem(mtm),
            EntryKind::Action => {
                let effective_enabled = ancestors_enabled && d.meta.enabled;
                let target: Retained<MenuTarget> = msg_send![super(mtm.alloc().set_ivars(Action {
                    id: d.meta.id.clone(),
                    enabled: effective_enabled,
                    keys_first: d.meta.keys_first,
                    sink: sink.clone(),
                })), init];
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &NSString::from_str(&d.meta.title),
                    Some(sel!(runeMenuAction:)),
                    &NSString::from_str(""),
                );
                item.setTarget(Some(&target));
                item.setEnabled(effective_enabled);
                item.setState(if d.meta.checked { 1 } else { 0 });
                item.setAlternate(d.meta.alternate);
                super::set_key_equivalent(&item, d.meta.equivalent.as_ref());
                targets.push(target);
                item
            }
            EntryKind::Submenu(children) => {
                let effective_enabled = ancestors_enabled && d.meta.enabled;
                let title = NSString::from_str(&d.meta.title);
                let submenu = NSMenu::new(mtm);
                submenu.setTitle(&title);
                submenu.setAutoenablesItems(false);
                let mut placed: Vec<(&str, Retained<NSMenuItem>)> = Vec::new();
                let mut chained: Vec<(&str, Retained<NSMenuItem>)> = Vec::new();
                for child in children {
                    let ns_child = build_item(mtm, child, effective_enabled, sink, targets);
                    let anchor = child.meta.after.as_deref()
                        .and_then(|a| anchor_index(&submenu, a, &placed, &chained).map(|i| (a, i)));
                    if let Some((a, index)) = anchor {
                        submenu.insertItem_atIndex(&ns_child, index + 1);
                        chained.retain(|(c, _)| *c != a);
                        chained.push((a, ns_child.clone()));
                    } else {
                        submenu.addItem(&ns_child);
                    }
                    if !matches!(child.kind, EntryKind::Separator) {
                        placed.push((&child.meta.id, ns_child));
                    }
                }
                let item = NSMenuItem::new(mtm);
                item.setTitle(&title);
                item.setSubmenu(Some(&submenu));
                item.setEnabled(effective_enabled);
                item.setState(if d.meta.checked { 1 } else { 0 });
                item.setAlternate(d.meta.alternate);
                item
            }
        }
    }
}

/// Detaches targets and actions recursively through any nested submenus before releasing
/// `Native::targets`.
unsafe fn clear_item_targets(item: &NSMenuItem) {
    // SAFETY: main-thread AppKit items; indices are bounded by `numberOfItems`.
    unsafe {
        if let Some(submenu) = item.submenu() {
            for i in 0..submenu.numberOfItems() {
                if let Some(child) = submenu.itemAtIndex(i) {
                    clear_item_targets(&child);
                }
            }
        }
        item.setTarget(None);
        item.setAction(None);
    }
}

/// Where `MenuItem::after`'s `anchor` is in `menu`: the last item already placed after it, else
/// this app's item with that id, else a native item with that AppKit action.
fn anchor_index(menu: &NSMenu, anchor: &str, placed: &[(&str, Retained<NSMenuItem>)], chained: &[(&str, Retained<NSMenuItem>)]) -> Option<isize> {
    let index_of = |item: &NSMenuItem| Some(menu.indexOfItem(item)).filter(|&i| i >= 0);
    let find = |list: &[(&str, Retained<NSMenuItem>)]| list.iter().filter(|(a, _)| *a == anchor).find_map(|(_, item)| index_of(item));
    find(chained).or_else(|| find(placed)).or_else(|| {
        (0..menu.numberOfItems()).find(|&i| menu.itemAtIndex(i).and_then(|item| item.action()).is_some_and(|a| a.name().to_str() == Ok(anchor)))
    })
}
impl Drop for Native {
    fn drop(&mut self) {
        self.clear();
        OWNED.with(|owned| owned.set(false));
    }
}
