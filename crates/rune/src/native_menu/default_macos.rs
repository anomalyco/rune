//! The winit 0.30 app menu for an accessory app becoming a regular app, and the standard
//! Edit and Window items. Creating a custom Rune menu uses the same root, so either
//! installation order composes.

#![allow(unsafe_code)]

use std::{cell::RefCell, collections::HashSet};
use super::standard::Equivalent;
use crate::Label;
use objc2::{MainThreadMarker, msg_send, rc::Retained, runtime::Sel, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSProcessInfo, NSString};

thread_local! {
    /// The empty app menu of a menu bar `App::standard_menus` made, filled when the app
    /// first shows its Dock icon.
    static PENDING: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
}

/// Preserve any existing menu, including an app-supplied empty menu. AppKit owns
/// the fallback for the app's lifetime; removing Rune's custom items leaves it alone.
pub(crate) fn ensure(app: &NSApplication) -> Retained<NSMenu> {
    // SAFETY: NSApplication and its menus are main-thread-only. All objects are
    // retained and the standard selectors use AppKit's responder chain, as in winit.
    unsafe {
        if let Some(root) = app.mainMenu() { return root; }
        let root = empty_root(app);
        let menu = PENDING.with(|p| p.borrow_mut().take()).expect("empty_root leaves its app menu");
        add_app_items(app, &menu);
        root
    }
}

/// `ensure` for an app showing its Dock icon: a standard-menus bar gets its app menu now.
pub(crate) fn promote(app: &NSApplication) {
    ensure(app);
    // SAFETY: main thread; the pending menu is retained and still in the menu bar's first slot.
    unsafe {
        if let Some(menu) = PENDING.with(|p| p.borrow_mut().take()) { add_app_items(app, &menu); }
    }
}

/// Lays `menu` and its submenus out the way the app's language reads
/// (`labels::set_direction`): right to left, their items align right, with checkmarks and
/// shortcuts on the other sides, as a menu in a right-to-left localization's.
pub(crate) fn set_layout_direction(menu: &NSMenu) {
    // NSUserInterfaceLayoutDirection: left to right 0, right to left 1.
    let direction: isize = if crate::labels::rtl() { 1 } else { 0 };
    // SAFETY: main-thread menus and items, indices bounded by the item count; the setter
    // (macOS 10.11 and later) takes an NSInteger.
    unsafe {
        let _: () = msg_send![menu, setUserInterfaceLayoutDirection: direction];
        for i in 0..menu.numberOfItems() {
            if let Some(submenu) = menu.itemAtIndex(i).and_then(|item| item.submenu()) { set_layout_direction(&submenu); }
        }
    }
}

/// A menu bar whose first item is an empty app menu, waiting in `PENDING`.
pub(crate) fn empty_root(app: &NSApplication) -> Retained<NSMenu> {
    let mtm = MainThreadMarker::from(app);
    let root = NSMenu::new(mtm);
    let top = NSMenuItem::new(mtm);
    let menu = NSMenu::new(mtm);
    top.setSubmenu(Some(&menu));
    root.addItem(&top);
    app.setMainMenu(Some(&root));
    PENDING.with(|p| *p.borrow_mut() = Some(menu));
    root
}

/// winit's app menu items, appended after any custom items already in `menu`.
unsafe fn add_app_items(app: &NSApplication, menu: &NSMenu) {
    // SAFETY: as `ensure`.
    unsafe {
        let mtm = MainThreadMarker::from(app);
        let services = NSMenu::new(mtm);
        let services_item = item(mtm, "", None, "");
        services_item.setSubmenu(Some(&services));
        let hide_others = item(mtm, "", Some(sel!(hideOtherApplications:)), "h");
        hide_others.setKeyEquivalentModifierMask(NSEventModifierFlags::Option
            | NSEventModifierFlags::Command);
        for entry in [
            item(mtm, "", Some(sel!(orderFrontStandardAboutPanel:)), ""),
            NSMenuItem::separatorItem(mtm),
            services_item,
            item(mtm, "", Some(sel!(hide:)), "h"),
            hide_others,
            item(mtm, "", Some(sel!(unhideAllApplications:)), ""),
            NSMenuItem::separatorItem(mtm),
            item(mtm, "", Some(sel!(terminate:)), "q"),
        ] {
            entry.setTag(APP_ITEM);
            menu.addItem(&entry);
        }
        app.setServicesMenu(Some(&services));
        relabel_app_items(menu);
    }
}

/// The app menu items `add_app_items` made, titled with the app's labels (`rune::set_labels`).
unsafe fn relabel_app_items(menu: &NSMenu) {
    let name = app_name();
    for entry in (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).filter(|i| i.tag() == APP_ITEM) {
        let label = match entry.action().and_then(|a| a.name().to_str().ok()) {
            Some("orderFrontStandardAboutPanel:") => Label::About(&name),
            Some("hide:") => Label::Hide(&name),
            Some("hideOtherApplications:") => Label::HideOthers,
            Some("unhideAllApplications:") => Label::ShowAll,
            Some("terminate:") => Label::Quit(&name),
            _ if !entry.isSeparatorItem() && entry.submenu().is_some() => Label::Services,
            _ => continue,
        };
        entry.setTitle(&NSString::from_str(&crate::labels::label(label)));
    }
}

/// The app's name in its menu items (About, Hide, Quit), as macOS shows it elsewhere (Dock, Force
/// Quit, the menu bar): the bundle's `CFBundleDisplayName`, else its `CFBundleName`, else the
/// process name, which for a bundled app is its executable's file name (`opencode-desktop`).
fn app_name() -> String {
    use objc2::{class, runtime::AnyObject};
    // SAFETY: NSBundle reads, on the main thread; an Info.plist name key holds a string.
    unsafe {
        let bundle: Option<Retained<AnyObject>> = msg_send![class!(NSBundle), mainBundle];
        if let Some(bundle) = bundle {
            for key in ["CFBundleDisplayName", "CFBundleName"] {
                let value: Option<Retained<NSString>> = msg_send![&*bundle, objectForInfoDictionaryKey: &*NSString::from_str(key)];
                if let Some(value) = value.map(|v| v.to_string()).filter(|v| !v.trim().is_empty()) { return value; }
            }
        }
    }
    NSProcessInfo::processInfo().processName().to_string()
}

/// Retitles the menus and items Rune made with the app's current labels (`rune::set_labels`),
/// after its language changed.
pub(crate) fn relabel(app: &NSApplication) {
    // SAFETY: main thread; setting titles of retained menus and items.
    unsafe {
        let Some(root) = app.mainMenu() else { return };
        for (index, top) in (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).enumerate() {
            let Some(menu) = top.submenu() else { continue };
            if index == 0 { relabel_app_items(&menu); }
            if let Some(name) = identifier(&top) {
                let title = NSString::from_str(&crate::labels::label(Label::Menu(&name)));
                top.setTitle(&title);
                menu.setTitle(&title);
            }
            for entry in (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).filter(|i| i.tag() == STANDARD) {
                if let Some(standard) = entry.action().and_then(|a| a.name().to_str().ok()).and_then(super::standard::by_action) {
                    entry.setTitle(&NSString::from_str(&crate::labels::label(standard.label())));
                }
            }
            for entry in (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).filter(|i| i.tag() == SYSTEM && !i.isSeparatorItem()) {
                if let Some(label) = entry.action().and_then(|a| system_label(a.name().to_str().ok()?)) {
                    entry.setTitle(&NSString::from_str(&crate::labels::label(label)));
                }
            }
        }
    }
}

/// The name a top-level menu Rune made goes by (`MenuItem::menu`), else its title.
pub(crate) fn name(top: &NSMenuItem) -> String {
    identifier(top).unwrap_or_else(|| top.title().to_string())
}

/// Marks the identifier of a top-level menu Rune made, ahead of its name.
const NAMED: &str = "rune.menu.";

fn identifier(top: &NSMenuItem) -> Option<String> {
    // SAFETY: NSMenuItem implements NSUserInterfaceItemIdentification; main thread.
    let id: Option<Retained<NSString>> = unsafe { msg_send![top, identifier] };
    id.and_then(|id| id.to_string().strip_prefix(NAMED).map(str::to_owned))
}

/// Whether the menu bar is still the one winit installs for a regular app.
pub(crate) fn is_winit_default(app: &NSApplication) -> bool {
    let Some(root) = app.mainMenu() else { return false };
    let first = (root.numberOfItems() > 0).then(|| root.itemAtIndex(0)).flatten().and_then(|i| i.submenu());
    let actions: Vec<Option<String>> = first.map(|m| (0..m.numberOfItems()).filter_map(|i| m.itemAtIndex(i))
        .map(|i| i.action().map(|a| a.name().to_string_lossy().into_owned())).collect()).unwrap_or_default();
    let actions: Vec<Option<&str>> = actions.iter().map(Option::as_deref).collect();
    super::standard::is_winit_default(root.numberOfItems() as usize, &actions)
}

/// Tags the items `add_standard` makes.
const STANDARD: isize = 0x5255_4e45;
/// Tags the app menu items `add_app_items` makes.
const APP_ITEM: isize = 0x5255_4e46;
/// Tags the Edit menu's system items `system_edit_items` makes.
const SYSTEM: isize = 0x5255_4e47;

/// The standard items, into menus of their titles (made if absent) in the menu bar, or in a
/// new one whose empty app menu waits for `promote`. An action a menu already has is skipped.
/// Never adds Quit.
pub(crate) fn add_standard(app: &NSApplication) {
    let mtm = MainThreadMarker::from(app);
    // SAFETY: main-thread AppKit calls on retained menus; nil-target items keep no target.
    unsafe {
        let root = app.mainMenu().unwrap_or_else(|| empty_root(app));
        for standard in &super::standard::ITEMS {
            let menu = top(mtm, &root, standard.menu);
            let action = Sel::register(&std::ffi::CString::new(standard.action).expect("selector"));
            if (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).any(|i| i.action() == Some(action)) { continue; }
            if standard.separator_before && menu.numberOfItems() > 0 { menu.addItem(&NSMenuItem::separatorItem(mtm)); }
            let entry = item(mtm, &crate::labels::label(standard.label()), Some(action), &standard.key.to_string());
            let mut flags = NSEventModifierFlags::Command;
            if standard.shift { flags |= NSEventModifierFlags::Shift; }
            entry.setKeyEquivalentModifierMask(flags);
            entry.setTag(STANDARD);
            menu.addItem(&entry);
        }
        system_edit_items(mtm, &top(mtm, &root, "Edit"));
        system_window_items(mtm, &top(mtm, &root, "Window"));
        // AppKit's windows menu: it lists the app's windows and takes the items macOS adds there
        // (Fill, Center, Move & Resize, Full-Screen Tile), as Electron's `windowMenu` role does.
        if let Some(window) = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).find(|i| name(i) == "Window").and_then(|i| i.submenu()) {
            app.setWindowsMenu(Some(&window));
        }
    }
    sync_standard(app);
}

/// The items AppKit puts at the end of an Edit menu the app has when it finishes launching,
/// which Rune's, made at the first window, misses: Start Dictation and Emoji & Symbols (⌃⌘Space),
/// after a separator, as in every Mac app's Edit menu. AppKit implements both actions. Skipped
/// when the menu already has one.
unsafe fn system_edit_items(mtm: MainThreadMarker, edit: &NSMenu) {
    let has = |action: Sel| (0..edit.numberOfItems()).filter_map(|i| edit.itemAtIndex(i)).any(|i| i.action() == Some(action));
    if has(sel!(startDictation:)) || has(sel!(orderFrontCharacterPalette:)) { return; }
    let dictation = item(mtm, &crate::labels::label(Label::StartDictation), Some(sel!(startDictation:)), "");
    let emoji = item(mtm, &crate::labels::label(Label::EmojiAndSymbols), Some(sel!(orderFrontCharacterPalette:)), " ");
    emoji.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Control);
    for entry in [NSMenuItem::separatorItem(mtm), dictation, emoji] {
        entry.setTag(SYSTEM);
        edit.addItem(&entry);
    }
}

/// The Window menu's Zoom, after Minimize, and Bring All to Front at its end, as Electron's
/// `windowMenu` role and AppKit's template give a Mac app; AppKit puts its tiling items after
/// Zoom and its window list after the end. AppKit implements both actions. Each is skipped when
/// the menu already has it.
unsafe fn system_window_items(mtm: MainThreadMarker, window: &NSMenu) {
    let items = || (0..window.numberOfItems()).filter_map(|i| window.itemAtIndex(i));
    if !items().any(|i| i.action() == Some(sel!(performZoom:))) {
        let zoom = item(mtm, &crate::labels::label(Label::Zoom), Some(sel!(performZoom:)), "");
        zoom.setTag(SYSTEM);
        let after = items().position(|i| i.action() == Some(sel!(performMiniaturize:))).map_or(0, |i| i + 1);
        window.insertItem_atIndex(&zoom, after as isize);
    }
    if !items().any(|i| i.action() == Some(sel!(arrangeInFront:))) {
        let front = item(mtm, &crate::labels::label(Label::BringAllToFront), Some(sel!(arrangeInFront:)), "");
        for entry in [NSMenuItem::separatorItem(mtm), front] {
            entry.setTag(SYSTEM);
            window.addItem(&entry);
        }
    }
}
/// Moves the `system_edit_items` back to the end of the Edit menu after the app added its own.
unsafe fn system_edit_last(edit: &NSMenu) {
    let items: Vec<_> = (0..edit.numberOfItems()).filter_map(|i| edit.itemAtIndex(i)).collect();
    let first = items.iter().position(|i| i.tag() == SYSTEM);
    if first.is_none_or(|first| items[first..].iter().all(|i| i.tag() == SYSTEM)) { return; }
    for entry in items.iter().filter(|i| i.tag() == SYSTEM) {
        edit.removeItem(entry);
        edit.addItem(entry);
    }
}

fn system_label(action: &str) -> Option<Label<'static>> {
    match action {
        "startDictation:" => Some(Label::StartDictation),
        "orderFrontCharacterPalette:" => Some(Label::EmojiAndSymbols),
        "performZoom:" => Some(Label::Zoom),
        "arrangeInFront:" => Some(Label::BringAllToFront),
        _ => None,
    }
}

/// Adds an app's own items in `add`: AppKit drops the key equivalent of an item added while
/// the menu bar already has it, so the standard items let go of theirs first and take back
/// only those no other item claims (`sync_standard`).
pub(crate) fn adding<R>(app: &NSApplication, add: impl FnOnce() -> R) -> R {
    set_standard_keys(app, |_| false);
    let added = add();
    sync_standard(app);
    added
}

/// After the menu bar's items change: each standard item keeps its key equivalent unless
/// another item in the menu bar has the same one, so an app's own item (Close Tab ⌘W) wins
/// whichever was added first.
pub(crate) fn sync_standard(app: &NSApplication) {
    if let Some(edit) = app.mainMenu().and_then(|root| (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).find(|i| name(i) == "Edit")).and_then(|i| i.submenu()) {
        // SAFETY: main thread; moving retained items within their menu.
        unsafe { system_edit_last(&edit) };
    }
    let mut claimed = HashSet::new();
    // SAFETY: main thread; reading retained menus.
    if let Some(root) = app.mainMenu() { unsafe { claims(&root, &mut claimed) }; }
    set_standard_keys(app, |equivalent| !claimed.contains(equivalent));
}

fn set_standard_keys(app: &NSApplication, keep: impl Fn(&Equivalent) -> bool) {
    let Some(root) = app.mainMenu() else { return };
    for menu in (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)?.submenu()) {
        for entry in (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)).filter(|i| i.tag() == STANDARD) {
            let Some(standard) = entry.action().and_then(|a| a.name().to_str().ok()).and_then(super::standard::by_action) else { continue };
            let equivalent = standard.equivalent();
            entry.setKeyEquivalent(&NSString::from_str(if keep(&equivalent) { &equivalent.key } else { "" }));
        }
    }
}

/// The key equivalents of every item but the standard ones, submenus included.
unsafe fn claims(menu: &NSMenu, out: &mut HashSet<Equivalent>) {
    // SAFETY: as `sync_standard`.
    unsafe {
        for entry in (0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)) {
            if entry.tag() != STANDARD {
                let flags = entry.keyEquivalentModifierMask();
                let has = |f| flags.contains(f);
                out.extend(Equivalent::new(&entry.keyEquivalent().to_string(),
                    has(NSEventModifierFlags::Command), has(NSEventModifierFlags::Shift),
                    has(NSEventModifierFlags::Option), has(NSEventModifierFlags::Control)));
            }
            if let Some(sub) = entry.submenu() { claims(&sub, out); }
        }
    }
}

/// A new top-level menu named `name` in the Mac order (`standard::top_index`): File before the
/// standard Edit, the app's other menus before Window, Help last. Its title is the app's label
/// for the name (`Label::Menu`).
pub(crate) fn add_top(mtm: MainThreadMarker, root: &NSMenu, name: &str) -> (Retained<NSMenuItem>, Retained<NSMenu>) {
    // SAFETY: main thread; every object is retained.
    unsafe {
        let tops: Vec<String> = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).map(|i| self::name(&i)).collect();
        let title = NSString::from_str(&crate::labels::label(Label::Menu(name)));
        let menu = NSMenu::new(mtm);
        menu.setTitle(&title);
        let entry = NSMenuItem::new(mtm);
        entry.setTitle(&title);
        let _: () = msg_send![&entry, setIdentifier: &*NSString::from_str(&format!("{NAMED}{name}"))];
        entry.setSubmenu(Some(&menu));
        root.insertItem_atIndex(&entry, super::standard::top_index(name, &tops) as isize);
        (entry, menu)
    }
}

/// Removes a top-level menu its maker no longer uses, unless other items (the standard ones)
/// still live in it.
pub(crate) fn remove_top_if_empty(root: &NSMenu, top: &NSMenuItem) {
    if top.submenu().is_none_or(|m| m.numberOfItems() == 0) && root.indexOfItem(top) >= 0 { root.removeItem(top); }
}

unsafe fn top(mtm: MainThreadMarker, root: &NSMenu, name: &str) -> Retained<NSMenu> {
    let existing = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i))
        .find(|i| self::name(i) == name).and_then(|i| i.submenu());
    existing.unwrap_or_else(|| add_top(mtm, root, name).1)
}

fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
    // SAFETY: Standard NSMenuItem initializer on the main thread; nil target lets
    // AppKit route the action. NSMenuItem defaults to Command for its key equivalent.
    unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(),
        &NSString::from_str(title), action, &NSString::from_str(key)) }
}
