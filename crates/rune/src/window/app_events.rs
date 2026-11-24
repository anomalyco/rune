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

/// Starts the app's quit when its last main window has finished, before dropping finished hosts
/// (or leaving them undropped for a fast exit). Returns whether the event loop must exit.
pub(super) fn cleanup_hosts(
    hosts: &mut [Option<Box<dyn AnyHost>>],
    run_quit: bool,
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
