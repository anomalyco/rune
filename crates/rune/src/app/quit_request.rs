//! Application-level quit consent (`QuitResponse` and `App::on_quit_request`).

use crate::el::Tx;
use super::App;

/// Decision returned by [`App::on_quit_request`] and
/// [`WindowSpec::on_quit_request`](crate::WindowSpec::on_quit_request) when macOS asks whether
/// the application should terminate (Dock Quit, the default Quit menu item / unhandled ⌘Q,
/// logout, restart, or shutdown).
///
/// ```no_run
/// use rune::{App, AppSpec, QuitResponse, el};
///
/// App::new(AppSpec::new("Notes", (480.0, 320.0), || (true, false), |_, _| el()))
///     .on_quit_request(|(dirty, confirm), _| {
///         if *dirty { *confirm = true; QuitResponse::Defer } else { QuitResponse::Quit }
///     })
///     .run();
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitResponse {
    /// Allow the application quit to proceed (running [`App::on_quit`] and [`App::on_close`]
    /// as usual, and letting a macOS logout or shutdown continue).
    Quit,
    /// Cancel this quit request, keep the app running, and commit any state or [`Tx`] changes
    /// made in the hook (for example showing an unsaved-changes confirmation dialog). Quit later
    /// with [`Tx::quit`] or [`AppHandle::quit`](crate::AppHandle::quit) without asking again.
    Defer,
}

impl<S: 'static> App<S> {
    /// Intercepts an application-level quit request on macOS (the Dock's Quit, the default
    /// application Quit menu item / unhandled ⌘Q, logout, restart, and shutdown) before
    /// [`quitting`](crate::quitting) becomes `true` or [`App::on_quit`] runs.
    ///
    /// Runs inside a normal update transaction on each live non-finished host (including hidden
    /// [`App::keep_running`] and [`App::windowless`] hosts; overlays and finished hosts are
    /// skipped) in registration order until one returns [`QuitResponse::Defer`] or all return
    /// [`QuitResponse::Quit`]. Returning [`QuitResponse::Defer`] keeps the app running, commits
    /// any state or [`Tx`] changes, and requests a redraw so the app's confirmation UI can appear
    /// normally; on macOS this cancels an in-progress logout or shutdown (macOS shows its
    /// standard system notice). Returning [`QuitResponse::Quit`] lets the quit proceed through
    /// [`App::on_quit`] and [`App::on_close`].
    ///
    /// Programmatic quit or relaunch ([`Tx::quit`], [`AppHandle::quit`](crate::AppHandle::quit),
    /// [`Tx::relaunch`], [`AppHandle::relaunch`](crate::AppHandle::relaunch)), closing the last
    /// main window (already gated by [`App::on_close_request`]), teardown, startup/display errors,
    /// panics, and Force Quit bypass this hook. On macOS, `NSSupportsSuddenTermination` or
    /// `NSSupportsAutomaticTermination` in `Info.plist` may let the system terminate an idle app
    /// without calling `applicationShouldTerminate:`. Apps that do not register this hook keep
    /// the existing behaviour (`NSTerminateNow` immediately).
    ///
    /// On Windows, Linux, and the web this builder compiles as a no-op: per-window close
    /// requests (the close button, Alt+F4) use [`App::on_close_request`]; Windows session
    /// shutdown (`WM_QUERYENDSESSION` / `ShutdownBlockReasonCreate`), Linux portal inhibition
    /// (`org.freedesktop.portal.Inhibit`), and the web's `beforeunload` are not interactive
    /// quit-dialog hooks. Headless scripts deliver it with `{"quit_request": true}`.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, QuitResponse, el};
    ///
    /// App::new(AppSpec::new("Notes", (480.0, 320.0), || (true, false), |_, _| el()))
    ///     .on_quit_request(|(dirty, confirm), _| {
    ///         if *dirty { *confirm = true; QuitResponse::Defer } else { QuitResponse::Quit }
    ///     })
    ///     .run();
    /// ```
    pub fn on_quit_request(mut self, f: fn(&mut S, &mut Tx) -> QuitResponse) -> Self {
        self.on_quit_request = Some(f);
        self
    }
}
