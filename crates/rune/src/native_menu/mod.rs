//! Native application and Dock menus. One App owns the process-wide custom menu at a time:
//! the one whose window last took focus, so with a window per App (`run_windows`) the menu bar
//! holds the key window's items and they act in it. Default AppKit/winit items are preserved.
//! Other native platforms return Unsupported; headless scripts use the same item lookup and
//! action dispatch on every platform.
use std::sync::{Arc, Mutex, Weak};
use crate::machine::{Attempt, Seq};
use crate::{App, AppHandle, Shortcut, Tx, app::Shared};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod default_macos;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod key_equivalent;
#[cfg(test)]
mod key_equivalent_tests;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod standard;
#[cfg(target_os = "macos")]
mod standard_macos;
#[cfg(target_os = "macos")]
pub(crate) use default_macos::{ensure as ensure_default_menu, promote as promote_default_menu, set_layout_direction};
#[cfg(target_os = "macos")]
pub(crate) use standard_macos::{adopt as adopt_standard_view, install as install_standard_menus};
#[cfg(target_os = "macos")]
pub(crate) use key_equivalent::set_key_equivalent;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Location { App, Named(String), Dock }

#[derive(Clone)]
struct Description {
    id: String,
    title: String,
    location: Location,
    after: Option<String>,
    equivalent: Option<Shortcut>,
    keys_first: bool,
    enabled: bool,
    checked: bool,
    separator: bool,
}

/// A custom native item. Defaults to the app menu (after its About item, in the order given),
/// enabled and unchecked, with its title as id.
pub struct MenuItem<S> { description: Description, action: fn(&mut S, &mut Tx) }
impl<S> Clone for MenuItem<S> {
    fn clone(&self) -> Self { Self { description: self.description.clone(), action: self.action } }
}
impl<S> MenuItem<S> {
    pub fn new(title: impl Into<String>, action: fn(&mut S, &mut Tx)) -> Self {
        let title = title.into();
        Self { description: Description { id: title.clone(), title, location: Location::App, after: None,
            equivalent: None, keys_first: false, enabled: true, checked: false, separator: false }, action }
    }
    /// A separator line, placed with `menu` or `dock` like an item. It has no id or title,
    /// can't be fired, and needs no unique id.
    pub fn separator() -> Self {
        let mut item = Self::new("", |_, _| {});
        item.description.separator = true;
        item
    }
    pub fn id(mut self, id: impl Into<String>) -> Self { self.description.id = id.into(); self }
    /// Place this item in a named top-level menu. Existing native menus are reused. A menu
    /// Rune makes is titled with the app's `Label::Menu(name)`, so the name stays the same in
    /// every language (`rune::set_labels`).
    pub fn menu(mut self, title: impl Into<String>) -> Self { self.description.location = Location::Named(title.into()); self }
    pub fn dock(mut self) -> Self { self.description.location = Location::Dock; self }
    /// Place this item right after `anchor` in its menu: another of this app's items by id, or
    /// a native item by its AppKit action (`"paste:"`, `"selectAll:"`), as Electron's menu
    /// items name the item they follow. Items after the same anchor keep the order given. A
    /// menu without the anchor takes the item at its end, as without one.
    pub fn after(mut self, anchor: impl Into<String>) -> Self { self.description.after = Some(anchor.into()); self }
    pub fn key_equivalent(mut self, shortcut: Shortcut) -> Self { self.description.equivalent = Some(shortcut); self }
    /// Let the window's own key handling see this item's key equivalent first. AppKit gives a
    /// menu equivalent to the menu before any view, so by default the item takes the chord
    /// even while a field, a modal or `on_key` wants it. With this, the press goes to the key
    /// window like any other key (the focused element, then modals, then `on_key`), and the
    /// item runs only if no handler takes it. Picking the item in the menu always runs it.
    pub fn keys_first(mut self) -> Self { self.description.keys_first = true; self }
    pub fn enabled(mut self, enabled: bool) -> Self { self.description.enabled = enabled; self }
    pub fn checked(mut self, checked: bool) -> Self { self.description.checked = checked; self }
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
    actions: Mutex<(u64, Vec<fn(&mut S, &mut Tx)>)>,
}

impl<S> Default for Hub<S> {
    fn default() -> Self {
        Self { core: Arc::default(), actions: Mutex::new((0, Vec::new())) }
    }
}

impl<S> Hub<S> {
    fn push(&self, item: MenuItem<S>) {
        let mut state = self.core.state.lock().unwrap();
        let mut actions = self.actions.lock().unwrap();
        if actions.0 != state.clear_gen {
            actions.1.clear();
            actions.0 = state.clear_gen;
        }
        state.descriptions.push(item.description);
        actions.1.push(item.action);
        state.revision.next();
    }

    fn set(&self, descriptions: Vec<Description>, new_actions: Vec<fn(&mut S, &mut Tx)>) -> Result<(), MenuError> {
        let mut state = self.core.state.lock().unwrap();
        state.check_set(&descriptions)?;
        let mut actions = self.actions.lock().unwrap();
        state.descriptions = descriptions;
        *actions = (state.clear_gen, new_actions);
        state.revision.next();
        Ok(())
    }

    fn resolve_action(&self, revision: Attempt, id: &str) -> Option<fn(&mut S, &mut Tx)> {
        let state = self.core.state.lock().unwrap();
        if !state.presence.running() || state.revision.current() != revision { return None; }
        let index = lookup(&state, id).ok()?;
        let actions = self.actions.lock().unwrap();
        (actions.0 == state.clear_gen).then(|| actions.1.get(index).copied()).flatten()
    }
}

fn validate(descriptions: &[Description]) -> Result<(), MenuError> {
    let mut ids = std::collections::HashSet::new();
    for d in descriptions {
        if matches!(&d.location, Location::Named(title) if title.trim().is_empty()) { return Err(MenuError::Invalid("empty menu title".into())); }
        if d.separator { continue; }
        if d.id.trim().is_empty() || d.title.trim().is_empty() { return Err(MenuError::Invalid("empty id or title".into())); }
        if !ids.insert(&d.id) { return Err(MenuError::Invalid(format!("duplicate id {}", d.id))); }
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
        let (descriptions, actions): (Vec<_>, Vec<_>) = items.into_iter().map(|i| (i.description, i.action)).unzip();
        self.shared.menus.set(descriptions, actions)?;
        self.shared.core.wake();
        Ok(())
    }
    #[inline]
    pub fn take_menu_errors(&self) -> Vec<MenuError> {
        self.shared.menus.core.take_errors()
    }
}

fn lookup(state: &CoreState, selector: &str) -> Result<usize, MenuError> {
    if !state.presence.running() { return Err(MenuError::NotRunning); }
    let items = state.descriptions.iter().enumerate().filter(|(_, d)| !d.separator);
    let index = if let Some((index, _)) = items.clone().find(|(_, d)| d.id == selector) { index }
    else {
        let mut matches = items.filter(|(_, d)| d.title == selector);
        let index = matches.next().ok_or_else(|| MenuError::UnknownItem(selector.into()))?.0;
        if matches.next().is_some() { return Err(MenuError::AmbiguousTitle(selector.into())); }
        index
    };
    if !state.descriptions[index].enabled { return Err(MenuError::Disabled(selector.into())); }
    Ok(index)
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
    let index = lookup(&state, selector)?;
    let id = state.descriptions[index].id.clone();
    drop(state);
    dispatch(revision, id);
    Ok(())
}

fn dispatch_typed<S: 'static>(shared: &Arc<Shared<S>>, revision: Attempt, id: String) {
    let weak = Arc::downgrade(shared);
    AppHandle { shared: shared.clone() }.update(move |s, tx| {
        let Some(shared) = weak.upgrade() else { return };
        if let Some(action) = shared.menus.resolve_action(revision, &id) {
            action(s, tx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppSpec, el};
    fn app() -> App<Vec<u32>> { App::new(AppSpec::new("menu", (100.0, 100.0), Vec::new, |_, _| el())) }
    fn item(id: &str) -> MenuItem<Vec<u32>> { MenuItem::new("Open Quick Panel", |s: &mut Vec<u32>, _| s.push(1)).id(id) }
    fn drain(app: &App<Vec<u32>>, s: &mut Vec<u32>) {
        for update in app.shared.take().updates { update(s, &mut Tx::default()); }
    }

    #[test]
    fn title_and_id_dispatch_on_the_state_thread_with_reentrant_menu_updates() {
        let app = app().menu_item(item("quick")).menu_item(MenuItem::new("Dock", |s: &mut Vec<u32>, tx| { s.push(2); tx.quit(); }).dock());
        let _guard = MenuGuard::attach(&app.shared, true);
        let handle = app.handle();
        let shared = app.shared.clone();
        std::thread::spawn(move || fire(&shared, "Open Quick Panel").unwrap()).join().unwrap();
        let mut state = Vec::new();
        assert!(state.is_empty());
        drain(&app, &mut state);
        assert_eq!(state, [1]);
        fire(&app.shared, "quick").unwrap();
        drain(&app, &mut state);
        assert_eq!(state, [1, 1]);
        let h = handle.clone();
        handle.update(move |_, _| h.set_menu([item("replacement").checked(true)]).unwrap());
        drain(&app, &mut state);
        assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions[0].id, "replacement");
    }

    #[test]
    fn replacement_disable_and_shutdown_cancel_stale_actions() {
        let app = app().menu_item(item("quick"));
        let guard = MenuGuard::attach(&app.shared, true);
        let handle = app.handle();
        fire(&app.shared, "quick").unwrap();
        handle.set_menu([item("quick").enabled(false).checked(true)]).unwrap();
        let mut state = Vec::new(); drain(&app, &mut state);
        assert!(state.is_empty());
        assert_eq!(fire(&app.shared, "quick"), Err(MenuError::Disabled("quick".into())));
        assert!(app.shared.menus.core.state.lock().unwrap().descriptions[0].checked);
        handle.set_menu([item("quick")]).unwrap();
        fire(&app.shared, "quick").unwrap();
        drop(guard); drain(&app, &mut state);
        assert!(state.is_empty());
        assert_eq!(fire(&app.shared, "quick"), Err(MenuError::NotRunning));
    }

    #[test]
    fn validation_is_atomic_and_ambiguous_titles_require_ids() {
        let app = app().menu_item(item("first")).menu_item(item("second").menu("Tools"));
        let _guard = MenuGuard::attach(&app.shared, true);
        assert_eq!(fire(&app.shared, "Open Quick Panel"), Err(MenuError::AmbiguousTitle("Open Quick Panel".into())));
        assert!(fire(&app.shared, "first").is_ok());
        assert!(matches!(app.handle().set_menu([item("duplicate"), item("duplicate")]), Err(MenuError::Invalid(_))));
        assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions.len(), 2);
        assert!(matches!(fire(&app.shared, "missing"), Err(MenuError::UnknownItem(_))));
        let invalid = self::app().menu_item(item("same")).menu_item(item("same"));
        let _guard = MenuGuard::attach(&invalid.shared, true);
        assert!(matches!(invalid.handle().take_menu_errors().as_slice(), [MenuError::Invalid(_)]));
        assert!(matches!(fire(&invalid.shared, "same"), Err(MenuError::UnknownItem(_))));
    }

    #[test]
    fn separators_need_no_ids_and_never_fire() {
        let file = |title: &str| MenuItem::new(title, |s: &mut Vec<u32>, _| s.push(3)).menu("File");
        let app = app().menu_item(file("New")).menu_item(MenuItem::separator().menu("File"))
            .menu_item(file("Close")).menu_item(MenuItem::separator().menu("File")).menu_item(MenuItem::separator().dock());
        let _guard = MenuGuard::attach(&app.shared, true);
        assert!(app.handle().take_menu_errors().is_empty());
        assert_eq!(fire(&app.shared, ""), Err(MenuError::UnknownItem(String::new())));
        fire(&app.shared, "Close").unwrap();
        let mut state = Vec::new(); drain(&app, &mut state);
        assert_eq!(state, [3]);
        app.handle().set_menu([file("New"), MenuItem::separator().menu("File"), MenuItem::separator().menu("File")]).unwrap();
        assert!(app.shared.menus.core.state.lock().unwrap().descriptions[1].separator);
        assert!(matches!(app.handle().set_menu([MenuItem::separator().menu(" ")]), Err(MenuError::Invalid(_))));
    }

    #[test]
    fn equivalent_and_non_send_state_remain_builder_compatible() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<AppHandle<std::rc::Rc<()>>>();
        let shortcut: Shortcut = "cmd+shift+k".parse().unwrap();
        let item = item("quick").key_equivalent(shortcut.clone());
        assert_eq!(item.description.equivalent, Some(shortcut));
        assert!(!item.description.keys_first && item.keys_first().description.keys_first);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_native_platform_is_a_noop_but_headless_works() {
        let app = app().menu_item(item("quick"));
        let _guard = MenuGuard::attach(&app.shared, false);
        assert_eq!(app.handle().take_menu_errors(), [MenuError::Unsupported]);
        assert_eq!(app.handle().set_menu([item("other")]), Err(MenuError::Unsupported));
        assert_eq!(app.shared.menus.core.state.lock().unwrap().descriptions[0].id, "quick");
    }
}
