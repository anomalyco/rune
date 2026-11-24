//! Shared, bounded XKB compose-table cache and per-surface compose slot for Wayland
//! layer-shell overlays. Compiles each locale's Compose file once off the UI thread and
//! hands each surface its own lightweight `xkb::compose::State` referencing the cached table.

use std::cell::RefCell;
use std::ffi::OsStr;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use xkbcommon::xkb::{self, compose};

use crate::platform::layer_shell_machine::compose_cache::{
    Compiled, ComposePool, Lookup, MAX_LOCALES, compile_for_locale,
};
use crate::platform::layer_shell_machine::keyboard;

/// Uniquely owned compiled XKB compose table moved once from a worker to the UI thread.
struct SendTable(compose::Table);

// SAFETY: libxkbcommon's reference counts are not atomic, so a table may move between threads
// only while one handle owns it. The worker makes its own context, compiles the table and drops
// its context handle, leaving the table the only owner of both; no clone or `compose::State`
// exists until the UI thread unwraps it into its thread-local cache.
#[allow(unsafe_code)]
unsafe impl Send for SendTable {}

#[cfg(test)]
static TABLE_COMPILATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static POOL: RefCell<ComposePool<compose::Table, SendTable>> =
        RefCell::new(ComposePool::new(MAX_LOCALES));
}

fn current_locale() -> String { keyboard::compose_locale(crate::env::var) }

fn compile_table(locale: &str) -> Option<compose::Table> {
    #[cfg(test)]
    TABLE_COMPILATIONS.fetch_add(1, Ordering::Relaxed);
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    compose::Table::new_from_locale(&context, OsStr::new(locale), compose::COMPILE_NO_FLAGS).ok()
}

fn compile_worker(locale: &str, try_c: bool) -> Compiled<SendTable> {
    compile_for_locale(locale, try_c, |loc| compile_table(loc).map(SendTable))
}

fn into_table(sent: SendTable) -> compose::Table { sent.0 }
fn state_from(table: Option<compose::Table>) -> Option<compose::State> {
    table.map(|t| compose::State::new(&t, compose::STATE_NO_FLAGS))
}

/// Starts compiling the current locale's XKB compose table off the UI thread when not cached.
pub(super) fn warm() { warm_locale(&current_locale()); }

fn warm_locale(locale: &str) {
    POOL.with(|pool| { let _ = pool.borrow_mut().warm(locale, into_table, compile_worker); });
}

fn try_state(locale: &str) -> Lookup<compose::State> {
    POOL.with(|pool| match pool.borrow_mut().try_get(locale, into_table, compile_worker, compile_table) {
        Lookup::Ready(table) => Lookup::Ready(state_from(table)),
        Lookup::Pending => Lookup::Pending,
    })
}

fn wait_state(locale: &str) -> Option<compose::State> {
    POOL.with(|pool| state_from(pool.borrow_mut().wait(locale, into_table, compile_table)))
}

/// Per-surface XKB compose slot: avoids blocking `wl_keyboard.keymap` during the first show's
/// background compile, attaching a `compose::State` once ready or on the first composed key.
#[derive(Default)]
pub(super) enum Slot { #[default] None, Pending(String), Ready(compose::State), Unavailable }

impl Slot {
    pub(super) fn on_keymap(&mut self, has_xkb: bool) {
        match self {
            Slot::Ready(compose) => compose.reset(),
            Slot::Unavailable => {}
            Slot::None | Slot::Pending(_) if has_xkb => {
                let locale = match self { Slot::Pending(loc) => loc.clone(), _ => current_locale() };
                *self = match try_state(&locale) {
                    Lookup::Ready(Some(state)) => Slot::Ready(state),
                    Lookup::Ready(None) => Slot::Unavailable,
                    Lookup::Pending => Slot::Pending(locale),
                };
            }
            Slot::None | Slot::Pending(_) => {}
        }
    }

    pub(super) fn reset(&mut self) { if let Slot::Ready(c) = self { c.reset(); } }
    pub(super) fn clear(&mut self) { *self = Slot::None; }

    pub(super) fn for_key(&mut self, action: keyboard::ComposeAction) -> Option<&mut compose::State> {
        if action == keyboard::ComposeAction::Feed && let Slot::Pending(locale) = self {
            let locale = std::mem::take(locale);
            *self = match wait_state(&locale) { Some(s) => Slot::Ready(s), None => Slot::Unavailable };
        }
        match self { Slot::Ready(c) => Some(c), _ => None }
    }
}

#[cfg(test)] pub(super) fn table_compilations() -> usize { TABLE_COMPILATIONS.load(Ordering::Relaxed) }
#[cfg(test)] pub(super) fn state_for_locale(locale: &str) -> Option<compose::State> { wait_state(locale) }
#[cfg(test)] pub(super) fn warm_for_test(locale: &str) { warm_locale(locale); }
#[cfg(test)] pub(super) fn clear_cache_for_test() { POOL.with(|p| p.borrow_mut().clear()); }
