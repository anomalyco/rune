//! What the whole app hears, not one window: a quit (`crate::quit`) and a reopen.

use super::*;

pub(super) fn quit_hosts(
    hosts: &mut [Option<Box<dyn AnyHost>>],
    ending: bool,
    hide: bool,
    exiting: bool,
    mut send: impl FnMut(crate::quit::Event) -> Vec<crate::quit::Effect>,
) -> bool {
    use crate::quit::{After, Effect, Event};
    if hide {
        for host in hosts.iter_mut().flatten() { host.vanish(); }
        crate::marks::mark("quit.hidden");
    }
    let mut exit = false;
    for effect in send(Event::Start { ending, after: After::Exit }) {
        match effect {
            Effect::RunHooks => {
                for host in hosts.iter_mut().flatten() { host.quit_hook(); }
                crate::marks::mark("quit.hooks");
                send(Event::HooksRan);
            }
            Effect::CloseAndExit => {
                if !exiting {
                    for host in hosts.iter_mut().flatten() { host.terminate(); }
                    exit = true;
                }
                send(Event::WindowsClosed);
            }
            // Only the loop's end relaunches.
            Effect::Relaunch => {}
        }
    }
    exit
}

/// Runs every registered `App::on_display_close` hook once on the UI thread, in registration
/// order, draining them so later exit steps never invoke them again.
pub(super) fn run_display_close(hooks: &mut Vec<crate::app::OnDisplayClose>) {
    for hook in hooks.drain(..) {
        hook();
    }
}

/// Starts the app's quit when its last main window has finished, runs `App::on_display_close`
/// hooks before dropping finished hosts when the loop is ending (unless `exiting` is already
/// true, in which case `exiting_hosts` runs them after `on_quit`), and returns whether the event
/// loop must exit.
pub(super) fn cleanup_hosts(
    hosts: &mut [Option<Box<dyn AnyHost>>],
    on_display_close: &mut Vec<crate::app::OnDisplayClose>,
    run_quit: bool,
    exiting: bool,
    fast_exit: bool,
    mut send: impl FnMut(crate::quit::Event) -> Vec<crate::quit::Effect>,
) -> bool {
    let no_main = !hosts.iter().flatten().any(|h| h.keeps_loop() && !h.finished());
    if no_main && run_quit {
        for host in hosts.iter_mut().flatten() {
            if host.finished() { host.cancel_menus(); }
        }
        // The app ends with its last main window: every window still in the list (including the
        // one that closed this turn) runs `App::on_quit`, then windows still open close as their
        // close button would. A quit already asked to exit and skips their close hooks; a startup
        // failure ends without running quit hooks.
        quit_hosts(hosts, false, true, false, &mut send);
        for host in hosts.iter_mut().flatten() { host.terminate(); }
    }
    if no_main && !exiting {
        run_display_close(on_display_close);
    }
    // The last window closing ends a fast-exiting app: its host isn't dropped first, which
    // tore down its window, swap chain and device (~37 ms) before the process ended anyway.
    let ending = no_main && fast_exit;
    for host in hosts.iter_mut() {
        if let Some(h) = host.as_mut().filter(|h| h.finished()) {
            h.cancel_menus();
            if !ending { *host = None; }
        }
    }
    no_main
}

/// What `Windows::exiting` does once the event loop starts shutting down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExitStep {
    /// Normal fast exit: hooks ran, menus cancelled; call `crate::quit::end_fast` now.
    FastExit,
    /// Orderly teardown (an app that called `orderly_exit`, or an error exit that must return to
    /// `finish_run`): clear hosts, shut down clipboard, and relaunch when `relaunch` is true.
    Orderly { relaunch: bool },
}

/// Whether `Windows::exiting` is ending because of an error rather than a normal quit:
/// - a startup window creation failure (`has_startup_error`),
/// - a fatal Wayland display error captured from the connection (`has_display_error`), or
/// - winit ending the event loop on its own (`loop_exiting && !quitting`, such as an X11 or
///   Wayland `ExitFailure` where Rune never started a quit).
pub(super) fn is_error_exit(
    has_startup_error: bool,
    has_display_error: bool,
    loop_exiting: bool,
    quitting: bool,
) -> bool {
    has_startup_error || has_display_error || (loop_exiting && !quitting)
}

/// Flags describing why `Windows::exiting` was entered.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExitFlags {
    pub(super) has_startup_error: bool,
    pub(super) has_display_error: bool,
    pub(super) loop_exiting: bool,
    pub(super) quitting: bool,
    pub(super) fast_exit: bool,
}

/// Runs the host-level steps of `Windows::exiting` and decides whether the process ends through
/// fast exit or returns from the event loop for orderly teardown / error reporting.
pub(super) fn exiting_hosts(
    hosts: &mut [Option<Box<dyn AnyHost>>],
    on_display_close: &mut Vec<crate::app::OnDisplayClose>,
    flags: ExitFlags,
    mut send: impl FnMut(crate::quit::Event) -> Vec<crate::quit::Effect>,
) -> ExitStep {
    let error_exit = is_error_exit(flags.has_startup_error, flags.has_display_error, flags.loop_exiting, flags.quitting);
    if !error_exit {
        // Every quit ends here, so the windows still open run `App::on_quit` first, unless the
        // quit's hooks already ran (a Windows logoff, the last window closing).
        let _ = quit_hosts(hosts, false, true, flags.loop_exiting, &mut send);
        // Rune didn't ask to exit, so the OS is ending the app: macOS Quit, Cmd-Q, the Dock's
        // Quit, logout. Windows close as their close button would, before the clipboard stops.
        if !flags.loop_exiting {
            for host in hosts.iter_mut().flatten() { host.terminate(); }
        }
    }
    for host in hosts.iter_mut().flatten() { host.cancel_menus(); }
    run_display_close(on_display_close);
    if !error_exit && flags.fast_exit {
        return ExitStep::FastExit;
    }
    let relaunch = !error_exit
        && send(crate::quit::Event::Exited).contains(&crate::quit::Effect::Relaunch);
    ExitStep::Orderly { relaunch }
}

/// Runs the non-blocking orderly teardown steps inside `Windows::exiting` before the event loop
/// returns and `shutdown_prewarm()` begins its bounded wait: drops every host (windows and
/// overlays), removes every live tray icon so nothing stays visible on screen during a slow or
/// hung warm-up drain, shuts down platform helpers, and hands off relaunch / single-instance exit.
pub(super) fn finish_orderly_exiting(
    hosts: &mut Vec<Option<Box<dyn AnyHost>>>,
    relaunch: bool,
    remove_trays: impl FnOnce(),
    shutdown_platform: impl FnOnce(),
    spawn_relaunch: impl FnOnce(),
    mark_instance_exited: impl FnOnce(),
) {
    hosts.clear();
    remove_trays();
    shutdown_platform();
    if relaunch {
        spawn_relaunch();
    }
    mark_instance_exited();
}

impl Windows {
    /// The app starts to quit (`crate::quit`): every open window (and any window whose close this
    /// turn ended the app) runs `App::on_quit`, once per quit. A session ending now (`ending`)
    /// then closes the windows as their close buttons would, before the loop ends, as a macOS
    /// logout does: the OS may end the process next. With `hide`, every window leaves the screen
    /// first, so the quit looks done the moment it starts and the hooks run unseen.
    pub(super) fn quit(&mut self, event_loop: &ActiveEventLoop, ending: bool, hide: bool) {
        if quit_hosts(&mut self.hosts, ending, hide, event_loop.exiting(), crate::quit::send) {
            event_loop.exit();
        }
    }

    /// The app was reopened: every `on_reopen` hook runs, then, unless one handled it or the
    /// app is an accessory, the default brings back a window when none shows (`reopen::target`).
    pub(super) fn reopen(&mut self, event_loop: &ActiveEventLoop) {
        let candidates = |hosts: &[Option<Box<dyn AnyHost>>]| -> Vec<_> {
            hosts.iter().map(|h| h.as_ref().map(|h| h.reopen_candidate()).unwrap_or_default()).collect()
        };
        let has_visible = crate::platform::reopen::has_visible(&candidates(&self.hosts));
        let mut handled = false;
        for host in self.hosts.iter_mut().flatten() {
            handled |= host.reopen_hook(event_loop, has_visible);
        }
        if handled || event_loop.exiting() { return; }
        #[cfg(target_os = "macos")]
        if crate::platform::reopen::accessory() { return; }
        // The default sees what the hooks did, to other windows too.
        for host in self.hosts.iter_mut().flatten() { host.apply_pending(event_loop); }
        if event_loop.exiting() { return; }
        #[cfg(target_os = "macos")]
        crate::platform::reopen::unhide();
        if let Some(Some(host)) = crate::platform::reopen::target(&candidates(&self.hosts)).and_then(|i| self.hosts.get_mut(i)) {
            host.reopen(event_loop);
        }
        // Windows: a launch while the app runs brings its front window forward, restored, as
        // AppKit's activation does on macOS; the launch let this process take the foreground.
        #[cfg(target_os = "windows")]
        crate::open_url::instance::raise_front();
    }
}

#[cfg(test)]
#[path = "app_events_tests.rs"]
mod tests;
