//! Native application and Dock menus. One App owns the process-wide custom menu at a time:
//! the one whose window last took focus, so with a window per App (`run_windows`) the menu bar
//! holds the key window's items and they act in it. Default AppKit/winit items are preserved.
//! Other native platforms return Unsupported; headless scripts use the same item lookup and
//! action dispatch on every platform.
use std::sync::{Arc, Mutex, Weak};
use crate::machine::{Attempt, Seq};
use crate::{App, AppHandle, Shortcut, Tx, app::Shared};

#[cfg(any(test, target_os = "macos"))]
mod bar_wait;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod default_macos;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod key_equivalent;
#[cfg(test)]
mod key_equivalent_tests;
#[cfg(test)]
mod tests;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod standard;
#[cfg(target_os = "macos")]
mod standard_macos;
#[cfg(target_os = "macos")]
pub(crate) use default_macos::set_layout_direction;
#[cfg(target_os = "macos")]
pub(crate) use standard_macos::{adopt as adopt_standard_view, install as install_standard_menus, settle as settle_standard_menus};
#[cfg(target_os = "macos")]
pub(crate) use key_equivalent::set_key_equivalent;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Location { App, Named(String), Dock }

#[derive(Clone)]
struct ItemMeta {
    id: String,
    title: String,
    location: Location,
    after: Option<String>,
    equivalent: Option<Shortcut>,
    keys_first: bool,
    enabled: bool,
    checked: bool,
    alternate: bool,
    replaces_about: bool,
}

impl ItemMeta {
    fn new(title: String) -> Self {
        Self {
            id: title.clone(),
            title,
            location: Location::App,
            after: None,
            equivalent: None,
            keys_first: false,
            enabled: true,
            checked: false,
            alternate: false,
            replaces_about: false,
        }
    }
}

#[derive(Clone)]
enum EntryKind {
    Action,
    Separator,
    Submenu(Vec<Description>),
}

#[derive(Clone)]
struct Description {
    meta: ItemMeta,
    kind: EntryKind,
}

enum MenuAction<S> {
    Fn(fn(&mut S, &mut Tx)),
    Shared(Arc<dyn Fn(&mut S, &mut Tx) + Send + Sync>),
}

impl<S> Clone for MenuAction<S> {
    fn clone(&self) -> Self {
        match self {
            Self::Fn(f) => Self::Fn(*f),
            Self::Shared(f) => Self::Shared(Arc::clone(f)),
        }
    }
}

impl<S> MenuAction<S> {
    fn call(&self, state: &mut S, tx: &mut Tx) {
        match self {
            Self::Fn(f) => f(state, tx),
            Self::Shared(f) => f(state, tx),
        }
    }
}

enum ItemKind<S> {
    Action(MenuAction<S>),
    Separator,
    Submenu(Vec<MenuItem<S>>),
}

impl<S> Clone for ItemKind<S> {
    fn clone(&self) -> Self {
        match self {
            Self::Action(action) => Self::Action(action.clone()),
            Self::Separator => Self::Separator,
            Self::Submenu(children) => Self::Submenu(children.clone()),
        }
    }
}

/// A custom native item. Defaults to the app menu (after its About item, in the order given),
/// enabled and unchecked, with its title as id.
pub struct MenuItem<S> {
    meta: ItemMeta,
    kind: ItemKind<S>,
}

impl<S> Clone for MenuItem<S> {
    fn clone(&self) -> Self {
        Self { meta: self.meta.clone(), kind: self.kind.clone() }
    }
}

impl<S> MenuItem<S> {
    pub fn new(title: impl Into<String>, action: fn(&mut S, &mut Tx)) -> Self {
        Self { meta: ItemMeta::new(title.into()), kind: ItemKind::Action(MenuAction::Fn(action)) }
    }
    /// A menu item whose action is a closure capturing per-item data, such as an entry from a
    /// dynamic list. Defaults to the app menu, enabled and unchecked, with its title as id.
    /// The closure runs on the app thread whenever the item is picked. `MenuItem<S>` remains
    /// `Clone`, `Send`, and `Sync` without requiring `S: Send + Sync`.
    ///
    /// ```no_run
    /// # use rune::{App, AppSpec, MenuItem, el};
    /// # struct State { selected: String }
    /// let hosts = ["prod", "staging"];
    /// let mut app = App::new(AppSpec::new("Demo", (400.0, 300.0), || State { selected: String::new() }, |_, _| el()));
    /// for host in hosts {
    ///     let name = host.to_owned();
    ///     app = app.menu_item(
    ///         MenuItem::with_action(host, move |s: &mut State, _| s.selected = name.clone())
    ///             .id(format!("host-{host}"))
    ///             .menu("Session"),
    ///     );
    /// }
    /// ```
    pub fn with_action(
        title: impl Into<String>,
        action: impl Fn(&mut S, &mut Tx) + Send + Sync + 'static,
    ) -> Self {
        Self {
            meta: ItemMeta::new(title.into()),
            kind: ItemKind::Action(MenuAction::Shared(Arc::new(action))),
        }
    }
    /// A submenu holding child items (action items, separators, or nested submenus). Defaults
    /// to the app menu, enabled and unchecked, with its title as id; place it in a top-level
    /// menu with [`MenuItem::menu`], in the Dock menu with [`MenuItem::dock`], or nest it
    /// inside another submenu. Child items inside a submenu cannot set `menu` or `dock` or use
    /// [`MenuItem::about`]; disabling a submenu prevents any descendant item from firing.
    ///
    /// ```no_run
    /// # use rune::{App, AppSpec, MenuItem, el};
    /// # struct State { target: String }
    /// let app = App::new(AppSpec::new("Demo", (400.0, 300.0), || State { target: String::new() }, |_, _| el()))
    ///     .menu_item(
    ///         MenuItem::submenu("Attach", [
    ///             MenuItem::submenu("prod", [
    ///                 MenuItem::with_action("main", |s: &mut State, _| s.target = "prod/main".into())
    ///                     .id("attach-prod-main"),
    ///             ]),
    ///         ])
    ///         .menu("Session"),
    ///     );
    /// ```
    pub fn submenu(
        title: impl Into<String>,
        items: impl IntoIterator<Item = MenuItem<S>>,
    ) -> Self {
        Self {
            meta: ItemMeta::new(title.into()),
            kind: ItemKind::Submenu(items.into_iter().collect()),
        }
    }
    /// Replace the macOS app menu's standard About item (`orderFrontStandardAboutPanel:`)
    /// with this item at the top of the app menu. Enabled and unchecked, with its title as id.
    /// At most one About replacement may appear in a menu snapshot, and it must stay in the
    /// app menu (cannot be moved with `menu` or `dock`, placed with `after`, or marked
    /// `alternate`). Clearing custom items restores AppKit's standard About item. Elsewhere
    /// it behaves like a normal item in headless menus.
    ///
    /// ```no_run
    /// # use rune::{App, AppSpec, MenuItem, el};
    /// # struct State { about: bool }
    /// let app = App::new(AppSpec::new("Demo", (400.0, 300.0), || State { about: false }, |_, _| el()))
    ///     .menu_item(MenuItem::about("About Demo", |s: &mut State, _| s.about = true));
    /// ```
    pub fn about(title: impl Into<String>, action: fn(&mut S, &mut Tx)) -> Self {
        let mut item = Self::new(title, action);
        item.meta.replaces_about = true;
        item
    }
    /// A separator line, placed with `menu` or `dock` like an item. It has no id or title,
    /// can't be fired, and needs no unique id.
    pub fn separator() -> Self {
        Self { meta: ItemMeta::new(String::new()), kind: ItemKind::Separator }
    }
    pub fn id(mut self, id: impl Into<String>) -> Self { self.meta.id = id.into(); self }
    /// Place this item in a named top-level menu. Existing native menus are reused. A menu
    /// Rune makes is titled with the app's `Label::Menu(name)`, so the name stays the same in
    /// every language (`rune::set_labels`).
    pub fn menu(mut self, title: impl Into<String>) -> Self { self.meta.location = Location::Named(title.into()); self }
    pub fn dock(mut self) -> Self { self.meta.location = Location::Dock; self }
    /// Place this item right after `anchor` in its menu: another of this app's items by id, or
    /// a native item by its AppKit action (`"paste:"`, `"selectAll:"`), as Electron's menu
    /// items name the item they follow. Items after the same anchor keep the order given. A
    /// menu without the anchor takes the item at its end, as without one.
    pub fn after(mut self, anchor: impl Into<String>) -> Self { self.meta.after = Some(anchor.into()); self }
    pub fn key_equivalent(mut self, shortcut: Shortcut) -> Self { self.meta.equivalent = Some(shortcut); self }
    /// Let the window's own key handling see this item's key equivalent first. AppKit gives a
    /// menu equivalent to the menu before any view, so by default the item takes the chord
    /// even while a field, a modal or `on_key` wants it. With this, the press goes to the key
    /// window like any other key (the focused element, then modals, then `on_key`), and the
    /// item runs only if no handler takes it. Picking the item in the menu always runs it.
    pub fn keys_first(mut self) -> Self { self.meta.keys_first = true; self }
    pub fn enabled(mut self, enabled: bool) -> Self { self.meta.enabled = enabled; self }
    pub fn checked(mut self, checked: bool) -> Self { self.meta.checked = checked; self }
    /// macOS: the item before it in its menu shows this one in its place while the modifier this
    /// one's key equivalent adds is held (Option, mostly), as AppKit's Close All stands behind
    /// Close Window. Give it the same key with that modifier more. Elsewhere it is an item like
    /// any other.
    pub fn alternate(mut self) -> Self { self.meta.alternate = true; self }

    /// Splits the tree into a non-generic `Description` and pre-order leaf actions matching
    /// [`walk_leaves`].
    fn split(self, actions: &mut Vec<MenuAction<S>>) -> Description {
        let kind = match self.kind {
            ItemKind::Action(action) => {
                actions.push(action);
                EntryKind::Action
            }
            ItemKind::Separator => EntryKind::Separator,
            ItemKind::Submenu(items) => {
                let children = items.into_iter().map(|child| child.split(actions)).collect();
                EntryKind::Submenu(children)
            }
        };
        Description { meta: self.meta, kind }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuError {
    Unsupported,
    NotRunning,
    InUse,
    Invalid(String),
    UnknownItem(String),
    AmbiguousTitle(String),
    Disabled(String),
    Backend(String),
}
impl std::fmt::Display for MenuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("native menus are unsupported on this platform"),
            Self::NotRunning => f.write_str("the menu's app is not running"),
            Self::InUse => f.write_str("another Rune app owns the native menu"),
            Self::Invalid(message) => write!(f, "invalid menu: {message}"),
            Self::UnknownItem(item) => write!(f, "unknown menu item: {item}"),
            Self::AmbiguousTitle(item) => write!(f, "ambiguous menu title: {item}; use its id"),
            Self::Disabled(item) => write!(f, "menu item is disabled: {item}"),
            Self::Backend(message) => write!(f, "native menu: {message}"),
        }
    }
}
impl std::error::Error for MenuError {}

/// Whether an App's window is running, and whether it runs with no native menu bar.
enum Presence {
    /// No window of the App runs: `set_menu` says NotRunning and a click goes nowhere.
    Absent,
    /// A window runs. Headless runs the menu actions on the state thread with no bar.
    Running { headless: bool },
}
impl Presence {
    fn running(&self) -> bool { matches!(self, Self::Running { .. }) }
    fn headless(&self) -> bool { matches!(self, Self::Running { headless: true }) }
}

struct CoreState {
    descriptions: Vec<Description>,
    /// Incremented whenever `descriptions` is cleared without holding the typed actions lock, so
    /// the typed table discards stale actions before its next read or write.
    clear_gen: u64,
    /// Each new set of items is a new attempt: a click on an item from an earlier set, or a
    /// guard that drained one, is stale. It is the version of `descriptions`, so it outlives a run.
    revision: Seq,
    presence: Presence,
    errors: Vec<MenuError>,
}

impl CoreState {
    fn clear(&mut self) {
        self.descriptions.clear();
        self.clear_gen += 1;
    }

    fn check_set(&self, descriptions: &[Description]) -> Result<(), MenuError> {
        if !cfg!(target_os = "macos") && !self.presence.headless() { return Err(MenuError::Unsupported); }
        if !self.presence.running() { return Err(MenuError::NotRunning); }
        validate(descriptions)
    }
}

struct HubCore {
    state: Mutex<CoreState>,
}

impl Default for HubCore {
    fn default() -> Self {
        Self {
            state: Mutex::new(CoreState {
                descriptions: Vec::new(),
                clear_gen: 0,
                revision: Seq::default(),
                presence: Presence::Absent,
                errors: Vec::new(),
            }),
        }
    }
}

impl HubCore {
    fn take_errors(&self) -> Vec<MenuError> {
        std::mem::take(&mut self.state.lock().unwrap().errors)
    }
}

pub(crate) struct Hub<S> {
    core: Arc<HubCore>,
    actions: Mutex<(u64, Vec<MenuAction<S>>)>,
}

impl<S> Default for Hub<S> {
    fn default() -> Self {
        Self { core: Arc::default(), actions: Mutex::new((0, Vec::new())) }
    }
}

impl<S> Hub<S> {
    fn push(&self, item: MenuItem<S>) {
        let mut new_actions = Vec::new();
        let description = item.split(&mut new_actions);
        let mut state = self.core.state.lock().unwrap();
        let mut actions = self.actions.lock().unwrap();
        if actions.0 != state.clear_gen {
            actions.1.clear();
            actions.0 = state.clear_gen;
        }
        state.descriptions.push(description);
        actions.1.extend(new_actions);
        state.revision.next();
    }

    fn set(&self, descriptions: Vec<Description>, new_actions: Vec<MenuAction<S>>) -> Result<(), MenuError> {
        let mut state = self.core.state.lock().unwrap();
        state.check_set(&descriptions)?;
        let mut actions = self.actions.lock().unwrap();
        state.descriptions = descriptions;
        *actions = (state.clear_gen, new_actions);
        state.revision.next();
        Ok(())
    }

    fn resolve_action(&self, revision: Attempt, id: &str) -> Option<MenuAction<S>> {
        let state = self.core.state.lock().unwrap();
        if !state.presence.running() || state.revision.current() != revision { return None; }
        let (index, _) = lookup(&state, id).ok()?;
        let actions = self.actions.lock().unwrap();
        (actions.0 == state.clear_gen).then(|| actions.1.get(index).cloned()).flatten()
    }
}

fn validate(descriptions: &[Description]) -> Result<(), MenuError> {
    let mut ids = std::collections::HashSet::new();
    let mut has_about = false;
    validate_entries(descriptions, false, &mut ids, &mut has_about)
}

fn validate_entries<'a>(
    descriptions: &'a [Description],
    nested: bool,
    ids: &mut std::collections::HashSet<&'a str>,
    has_about: &mut bool,
) -> Result<(), MenuError> {
    for d in descriptions {
        if nested {
            if d.meta.location != Location::App {
                return Err(MenuError::Invalid("submenu item cannot set menu or dock".into()));
            }
            if d.meta.replaces_about {
                return Err(MenuError::Invalid("about item must be in the app menu".into()));
            }
        } else {
            if matches!(&d.meta.location, Location::Named(title) if title.trim().is_empty()) {
                return Err(MenuError::Invalid("empty menu title".into()));
            }
            if d.meta.replaces_about {
                if d.meta.location != Location::App { return Err(MenuError::Invalid("about item must be in the app menu".into())); }
                if d.meta.after.is_some() { return Err(MenuError::Invalid("about item cannot use after".into())); }
                if d.meta.alternate { return Err(MenuError::Invalid("about item cannot be an alternate".into())); }
                if *has_about { return Err(MenuError::Invalid("duplicate about item".into())); }
                *has_about = true;
            }
        }
        match &d.kind {
            EntryKind::Separator => {}
            EntryKind::Action => {
                if d.meta.id.trim().is_empty() || d.meta.title.trim().is_empty() {
                    return Err(MenuError::Invalid("empty id or title".into()));
                }
                if !ids.insert(d.meta.id.as_str()) {
                    return Err(MenuError::Invalid(format!("duplicate id {}", d.meta.id)));
                }
            }
            EntryKind::Submenu(children) => {
                if d.meta.title.trim().is_empty() {
                    return Err(MenuError::Invalid("empty submenu title".into()));
                }
                validate_entries(children, true, ids, has_about)?;
            }
        }
    }
    Ok(())
}

impl<S: 'static> App<S> {
    /// Add a custom native item at startup. Deferred failures (including Unsupported on
    /// Windows/Linux) are available through AppHandle::take_menu_errors().
    pub fn menu_item(self, item: MenuItem<S>) -> Self {
        self.shared.menus.push(item);
        self
    }
}
impl<S: 'static> AppHandle<S> {
    /// Replace all this App's custom items, preserving native defaults. Validates and queues
    /// the new snapshot; enabled/checked changes reach AppKit on the next event-loop drain.
    /// Native installation failures are retained by take_menu_errors(). On Windows/Linux this
    /// returns Unsupported without changing anything (except inside a headless run). On macOS
    /// the menus and standard items Rune made also take the app's current labels
    /// (`rune::set_labels`), so an app calls this again after its language changes.
    pub fn set_menu(&self, items: impl IntoIterator<Item = MenuItem<S>>) -> Result<(), MenuError> {
        let mut actions = Vec::new();
        let descriptions: Vec<_> = items.into_iter().map(|i| i.split(&mut actions)).collect();
        self.shared.menus.set(descriptions, actions)?;
        self.shared.core.wake();
        Ok(())
    }
    #[inline]
    pub fn take_menu_errors(&self) -> Vec<MenuError> {
        self.shared.menus.core.take_errors()
    }
}

struct LeafMatch<'a> {
    index: usize,
    id: &'a str,
    enabled: bool,
}

fn walk_leaves<'a>(
    entries: &'a [Description],
    ancestors_enabled: bool,
    next_index: &mut usize,
    visit: &mut impl FnMut(usize, &'a Description, bool),
) {
    for d in entries {
        match &d.kind {
            EntryKind::Separator => {}
            EntryKind::Action => {
                let index = *next_index;
                *next_index += 1;
                visit(index, d, ancestors_enabled && d.meta.enabled);
            }
            EntryKind::Submenu(children) => {
                walk_leaves(children, ancestors_enabled && d.meta.enabled, next_index, visit);
            }
        }
    }
}

fn lookup(state: &CoreState, selector: &str) -> Result<(usize, String), MenuError> {
    if !state.presence.running() { return Err(MenuError::NotRunning); }
    let mut by_id: Option<LeafMatch<'_>> = None;
    let mut by_title: Option<LeafMatch<'_>> = None;
    let mut title_ambiguous = false;
    let mut next_index = 0;
    walk_leaves(&state.descriptions, true, &mut next_index, &mut |index, d, enabled| {
        if by_id.is_none() && d.meta.id == selector {
            by_id = Some(LeafMatch { index, id: &d.meta.id, enabled });
        }
        if d.meta.title == selector {
            if by_title.is_some() {
                title_ambiguous = true;
            } else {
                by_title = Some(LeafMatch { index, id: &d.meta.id, enabled });
            }
        }
    });
    let found = if let Some(found) = by_id {
        found
    } else if title_ambiguous {
        return Err(MenuError::AmbiguousTitle(selector.into()));
    } else {
        by_title.ok_or_else(|| MenuError::UnknownItem(selector.into()))?
    };
    if !found.enabled { return Err(MenuError::Disabled(selector.into())); }
    Ok((found.index, found.id.to_owned()))
}

/// The window host, for a key press it maps to `key`: what to do if no handler takes it, when
/// a menu item (a standard ⌘W or ⌘M, or `MenuItem::keys_first`) is handing Rune that key.
/// The jump list as the Dock menu (`set_jump_list` on macOS).
#[cfg(target_os = "macos")]
pub(crate) fn set_jump_menu(list: &crate::jump_list::JumpList) {
    macos::set_jump_menu(list);
}

pub(crate) fn take_unhandled(key: Option<&crate::KeyEvent>) -> Option<crate::runtime::Unhandled> {
    #[cfg(target_os = "macos")]
    { key.and_then(standard_macos::take_unhandled) }
    #[cfg(not(target_os = "macos"))]
    { let _ = key; None }
}

type Dispatch = Arc<dyn Fn(Attempt, String) + Send + Sync>;

fn enqueue_core(core: &HubCore, dispatch: &dyn Fn(Attempt, String), selector: &str, expected: Option<Attempt>) -> Result<(), MenuError> {
    let state = core.state.lock().unwrap();
    let revision = state.revision.current();
    if expected.is_some_and(|r| r != revision) { return Ok(()); }
    let (_, id) = lookup(&state, selector)?;
    drop(state);
    dispatch(revision, id);
    Ok(())
}

fn dispatch_typed<S: 'static>(shared: &Arc<Shared<S>>, revision: Attempt, id: String) {
    let weak = Arc::downgrade(shared);
    AppHandle { shared: shared.clone() }.update(move |s, tx| {
        let Some(shared) = weak.upgrade() else { return };
        if let Some(action) = shared.menus.resolve_action(revision, &id) {
            action.call(s, tx);
        }
    });
}

#[cfg(test)]
pub(crate) fn fire<S: 'static>(shared: &Arc<Shared<S>>, selector: &str) -> Result<(), MenuError> {
    enqueue_core(&shared.menus.core, &|revision, id| dispatch_typed(shared, revision, id), selector, None)
}

/// Who holds the macOS menu bar's custom items: nobody, or the guard of the App whose window
/// last took focus (`MenuGuard::claim`), with the items it put there.
#[cfg(target_os = "macos")]
enum Bar {
    Free,
    Owned { guard: u64, native: macos::Native },
}

#[cfg(target_os = "macos")]
thread_local! {
    static BAR: std::cell::RefCell<Bar> = const { std::cell::RefCell::new(Bar::Free) };
    static NEXT_GUARD: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    /// The launch's menu bar while it waits for the first window's first frame.
    static BAR_WAIT: std::cell::Cell<bar_wait::BarWait> = const { std::cell::Cell::new(bar_wait::BarWait::new()) };
}

#[cfg(target_os = "macos")]
fn bar_state<R>(f: impl FnOnce(&mut bar_wait::BarWait) -> R) -> R {
    BAR_WAIT.with(|cell| {
        let mut wait = cell.get();
        let out = f(&mut wait);
        cell.set(wait);
        out
    })
}

/// `default_macos::ensure`, once a launch's waiting menu bar is made: winit's default menu is
/// the root it keeps.
#[cfg(target_os = "macos")]
pub(crate) fn ensure_default_menu(app: &objc2_app_kit::NSApplication) -> objc2::rc::Retained<objc2_app_kit::NSMenu> {
    bar_ready();
    default_macos::ensure(app)
}

/// `default_macos::promote`, once a launch's waiting menu bar is made.
#[cfg(target_os = "macos")]
pub(crate) fn promote_default_menu(app: &objc2_app_kit::NSApplication) {
    bar_ready();
    default_macos::promote(app);
}

/// The event loop starts a regular app with winit's default menu turned off: the menu bar is
/// made once the first window's first frame is on its way (`bar_ready`).
#[cfg(target_os = "macos")]
pub(crate) fn bar_waits() {
    bar_state(bar_wait::BarWait::begin);
}

/// The menu bar a launch left waiting, now: winit's default menu, then the standard items its
/// window starts asked for. A window's own items follow as its guard drains. Called once the
/// first window's first frame is on its way, at the end of a launch that drew none, and by
/// whatever needs the menu bar before that (`ensure_default_menu`). True when this call made it.
#[cfg(target_os = "macos")]
pub(crate) fn bar_ready() -> bool {
    let Some(owed) = bar_state(bar_wait::BarWait::end) else { return false };
    winit::platform::macos::install_default_menu();
    if let Some(opt_in) = owed.standard { standard_macos::install(opt_in); }
    true
}

/// Lists `window` in the Window menu now, as AppKit does when a window is ordered in. The
/// launch's first window was ordered in before its menu bar was made (`bar_ready`): AppKit
/// would list it once the app is active, after the launch's later windows, where it was the
/// first of the list.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn list_window(window: &winit::window::Window) {
    let (Some(mtm), Some(window)) = (objc2::MainThreadMarker::new(), crate::platform::macos::ns_window(window)) else { return };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    // SAFETY: main thread; the window and its title are retained across AppKit's own call for
    // a window's item (it adds the item when the window has none).
    unsafe {
        if window.isExcludedFromWindowsMenu() { return; }
        let _: () = objc2::msg_send![&*app, changeWindowsItem: &*window, title: &*window.title(), filename: false];
    }
}

pub(crate) struct MenuGuard {
    core: Weak<HubCore>,
    dispatch: Dispatch,
    /// Whether the bar holds this guard's items.
    placed: Placed,
    /// This guard's name in `BAR`.
    #[cfg(target_os = "macos")]
    id: u64,
}

/// Where this guard's items stand: not in the bar yet, or the set it last put there.
#[derive(Clone, Copy)]
enum Placed {
    /// Its first drain, or another App's window took the bar since.
    Fresh,
    /// The bar holds the items of this revision.
    Synced(Attempt),
}

impl MenuGuard {
    pub(crate) fn attach<S: 'static>(shared: &Arc<Shared<S>>, headless: bool) -> Self {
        let weak = Arc::downgrade(shared);
        let dispatch: Dispatch = Arc::new(move |revision, id| {
            if let Some(shared) = weak.upgrade() {
                dispatch_typed(&shared, revision, id);
            }
        });
        Self::attach_core(&shared.menus.core, dispatch, headless)
    }

    fn attach_core(core: &Arc<HubCore>, dispatch: Dispatch, headless: bool) -> Self {
        let mut state = core.state.lock().unwrap();
        state.presence = Presence::Running { headless };
        if let Err(error) = validate(&state.descriptions) {
            state.clear();
            state.errors.push(error);
        }
        drop(state);
        let mut guard = Self {
            core: Arc::downgrade(core),
            dispatch,
            placed: Placed::Fresh,
            #[cfg(target_os = "macos")]
            id: NEXT_GUARD.with(|n| n.replace(n.get() + 1)),
        };
        guard.drain();
        guard
    }

    pub(crate) fn fire(&self, selector: &str) -> Result<(), MenuError> {
        let Some(core) = self.core.upgrade() else { return Err(MenuError::NotRunning) };
        enqueue_core(&core, &*self.dispatch, selector, None)
    }

    /// This App's window took focus: its items take the menu bar, so they act in it. An App
    /// without items of its own leaves the bar to the one that has it.
    pub(crate) fn claim(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let Some(core) = self.core.upgrade() else { return };
            let state = core.state.lock().unwrap();
            if state.presence.headless() || state.descriptions.is_empty() { return; }
            drop(state);
            let taken = BAR.with(|bar| match &*bar.borrow() {
                Bar::Owned { guard, .. } => *guard != self.id,
                Bar::Free => true,
            });
            if taken {
                // The previous holder's items leave with its `Native`, before this one's go in.
                BAR.with(|bar| *bar.borrow_mut() = Bar::Free);
                self.placed = Placed::Fresh;
                self.drain();
            }
        }
    }

    pub(crate) fn drain(&mut self) {
        // A launch's menu bar waits for the first frame; this guard's items go in after it.
        #[cfg(target_os = "macos")]
        if bar_state(|wait| wait.waits()) { return; }
        let Some(core) = self.core.upgrade() else { return };
        let state = core.state.lock().unwrap();
        if matches!(self.placed, Placed::Synced(at) if at == state.revision.current()) { return; }
        let (descriptions, revision, headless) = (state.descriptions.clone(), state.revision.current(), state.presence.headless());
        drop(state);
        let result = validate(&descriptions).and_then(|()| {
            if headless { return Ok(()); }
            #[cfg(target_os = "macos")]
            {
                let weak = self.core.clone();
                let dispatch = self.dispatch.clone();
                let sink = Arc::new(move |id: &str| {
                    if let Some(core) = weak.upgrade()
                        && let Err(error) = enqueue_core(&core, &*dispatch, id, Some(revision))
                    {
                        eprintln!("rune: {error}");
                    }
                });
                BAR.with(|bar| {
                    let mut bar = bar.borrow_mut();
                    let (next, result) = match std::mem::replace(&mut *bar, Bar::Free) {
                        // Another window's App has the bar until this one's window takes focus.
                        Bar::Owned { guard, native } if guard != self.id => (Bar::Owned { guard, native }, Ok(())),
                        // Dropping the `Native` takes its items out.
                        Bar::Owned { .. } | Bar::Free if descriptions.is_empty() => (Bar::Free, Ok(())),
                        Bar::Owned { mut native, .. } => {
                            let result = native.set(&descriptions, sink);
                            (Bar::Owned { guard: self.id, native }, result)
                        }
                        Bar::Free => match macos::Native::new() {
                            Ok(mut native) => {
                                let result = native.set(&descriptions, sink);
                                (Bar::Owned { guard: self.id, native }, result)
                            }
                            Err(error) => (Bar::Free, Err(error)),
                        },
                    };
                    *bar = next;
                    result
                })
            }
            #[cfg(not(target_os = "macos"))]
            { if descriptions.is_empty() { Ok(()) } else { Err(MenuError::Unsupported) } }
        });
        // The app's labels may have changed with its language since the menus were made.
        #[cfg(target_os = "macos")]
        if !headless && let Some(mtm) = objc2::MainThreadMarker::new() {
            default_macos::relabel(&objc2_app_kit::NSApplication::sharedApplication(mtm));
        }
        self.placed = Placed::Synced(revision);
        if let Err(error) = result {
            eprintln!("rune: {error}");
            core.state.lock().unwrap().errors.push(error);
        }
    }
}

impl Drop for MenuGuard {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            let mut state = core.state.lock().unwrap();
            state.presence = Presence::Absent;
            state.clear();
        }
        // The bar waits for the next window to take focus (`claim`).
        #[cfg(target_os = "macos")]
        BAR.try_with(|bar| {
            let mut bar = bar.borrow_mut();
            if matches!(&*bar, Bar::Owned { guard, .. } if *guard == self.id) { *bar = Bar::Free; }
        }).ok();
    }
}


