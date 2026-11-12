//! The app's jump list: what a right-click on its taskbar button (Windows) offers besides its
//! windows, as tasks (New window) and named categories of recent things (Recent projects). Each
//! entry starts the app's own executable with one argument, which a running app receives
//! through `single_instance` like any second launch's arguments, so a link the app opens
//! (`App::on_open_url`) is the natural argument.
//!
//! [`set_jump_list`] returns at once: the list is built and committed on a thread of its own
//! (made on the first call, asleep in between), never on the UI thread, and a list equal to the
//! last one does nothing. Entries the person removed from the list stay out of it, as Windows
//! asks. On Windows the list belongs to the process's AppUserModelID, so call it after the app
//! took its identity.
//!
//! macOS shows the same list as the Dock icon's menu (when the app's own menus put nothing in the
//! Dock): a pick arrives as the link the app opens (`App::on_open_url`), in the running app.
//! Linux's counterpart, the launcher's actions, are static in a `.desktop` file, not built here.

/// One entry: a title, and the single argument the app starts with when it's picked.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JumpItem {
    pub(crate) title: String,
    pub(crate) argument: String,
    pub(crate) description: String,
}

impl JumpItem {
    pub fn new(title: impl Into<String>, argument: impl Into<String>) -> Self {
        Self { title: title.into(), argument: argument.into(), description: String::new() }
    }

    /// The tooltip over the entry.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Tasks first, then each category under its name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct JumpList {
    pub(crate) tasks: Vec<JumpItem>,
    pub(crate) categories: Vec<(String, Vec<JumpItem>)>,
    pub(crate) lead: Option<String>,
}

impl JumpList {
    pub fn new() -> Self { Self::default() }

    pub fn task(mut self, item: JumpItem) -> Self {
        self.tasks.push(item);
        self
    }

    /// One argument every entry starts the app with before its own (Windows): a run's own data
    /// folder, so a pick reaches that run and not the app's usual profile.
    pub fn lead(mut self, argument: impl Into<String>) -> Self {
        self.lead = Some(argument.into());
        self
    }

    /// A category of entries under `name`; an empty one isn't shown.
    pub fn category(mut self, name: impl Into<String>, items: Vec<JumpItem>) -> Self {
        if !items.is_empty() { self.categories.push((name.into(), items)); }
        self
    }
}

/// Replaces the app's jump list with `list`: on Windows off the UI thread, on macOS as the Dock
/// menu (call it on the main thread). Does nothing elsewhere, and for headless runs.
pub fn set_jump_list(list: JumpList) {
    if crate::is_headless() { return; }
    #[cfg(target_os = "windows")]
    crate::platform::win32::jump_list::set(list);
    #[cfg(target_os = "macos")]
    crate::native_menu::set_jump_menu(&list);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = list;
}
