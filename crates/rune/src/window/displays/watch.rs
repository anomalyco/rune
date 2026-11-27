//! Display change events (`DisplayEvent`) and cross-platform watcher backing `App::on_displays`.

use std::collections::HashMap;

use crate::windows::Display;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod macos;
#[cfg(target_os = "windows")]
#[path = "win32.rs"]
mod win32;

/// A change in the connected displays reported to [`App::on_displays`](crate::App::on_displays).
///
/// Multiple changes observed together are delivered in order: [`Removed`](Self::Removed) first,
/// then [`Changed`](Self::Changed), then [`Added`](Self::Added).
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayEvent {
    /// A display was connected or became active.
    Added(Display),
    /// A previously connected display was disconnected or deactivated.
    Removed(Display),
    /// A connected display's geometry, scale, name, or primary status changed.
    Changed(Display),
}

/// Stable identity key for diffing two snapshots of connected displays.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum DisplayKey<'a> {
    Native(crate::NativeDisplayId),
    Fallback(Option<&'a str>, usize),
}

fn build_keys(displays: &[Display]) -> Vec<DisplayKey<'_>> {
    let mut seen_fallback: HashMap<Option<&str>, usize> = HashMap::new();
    displays
        .iter()
        .map(|d| match d.native_id() {
            Some(id) => DisplayKey::Native(id),
            None => {
                let name = d.name.as_deref();
                let ordinal = *seen_fallback.entry(name).and_modify(|n| *n += 1).or_insert(0);
                DisplayKey::Fallback(name, ordinal)
            }
        })
        .collect()
}

fn display_changed(a: &Display, b: &Display) -> bool {
    a.name != b.name
        || a.position != b.position
        || a.size != b.size
        || a.scale != b.scale
        || a.primary != b.primary
}

/// Computes the ordered display change events transitioning from `previous` to `current`:
/// removals first (in `previous` order), then changes (in `current` order), then additions
/// (in `current` order).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn diff_displays(previous: &[Display], current: &[Display]) -> Vec<DisplayEvent> {
    let prev_keys = build_keys(previous);
    let curr_keys = build_keys(current);

    let prev_index: HashMap<&DisplayKey<'_>, usize> = prev_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    let curr_index: HashMap<&DisplayKey<'_>, usize> = curr_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();

    let mut events = Vec::new();
    // 1. Removals first, so shells can tear down overlays for gone displays first.
    for (i, prev) in previous.iter().enumerate() {
        if !curr_index.contains_key(&prev_keys[i]) {
            events.push(DisplayEvent::Removed(prev.clone()));
        }
    }
    // 2. Changes for displays that stayed connected with updated properties.
    for (i, curr) in current.iter().enumerate() {
        if let Some(&pi) = prev_index.get(&curr_keys[i])
            && display_changed(&previous[pi], curr)
        {
            events.push(DisplayEvent::Changed(curr.clone()));
        }
    }
    // 3. Additions last.
    for (i, curr) in current.iter().enumerate() {
        if !prev_index.contains_key(&curr_keys[i]) {
            events.push(DisplayEvent::Added(curr.clone()));
        }
    }
    events
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
enum PlatformWatch {
    #[default]
    None,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Wayland(crate::platform::wayland::output::OutputWatch),
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    X11,
    #[cfg(target_os = "windows")]
    Win32(win32::Win32DisplayWatch),
    #[cfg(target_os = "macos")]
    MacOS(macos::MacDisplayWatch),
}

/// Event-loop subsystem owning `App::on_displays` hooks, the last reported display snapshot,
/// and the platform-native watcher.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct DisplayWatch {
    hooks: Vec<crate::app::OnDisplays>,
    last: Vec<Display>,
    started: bool,
    platform: PlatformWatch,
}

#[cfg(not(target_arch = "wasm32"))]
impl DisplayWatch {
    /// Registers `hooks` and starts the platform display watch on the first registration.
    pub(crate) fn add_hooks(
        &mut self,
        hooks: Vec<crate::app::OnDisplays>,
        event_loop: &winit::event_loop::ActiveEventLoop,
        initial: Option<&[Display]>,
        proxy: &winit::event_loop::EventLoopProxy<crate::window::WakeEvent>,
    ) {
        if hooks.is_empty() {
            return;
        }
        self.hooks.extend(hooks);
        if self.started {
            return;
        }
        self.started = true;
        self.start_platform(event_loop, initial, proxy);
    }

    fn start_platform(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        initial: Option<&[Display]>,
        proxy: &winit::event_loop::EventLoopProxy<crate::window::WakeEvent>,
    ) {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            let _ = proxy;
            if crate::window::is_wayland(event_loop) {
                if let Ok(display) = winit::raw_window_handle::HasDisplayHandle::display_handle(event_loop)
                    && let Some(watch) = crate::platform::wayland::output::OutputWatch::new(display.as_raw())
                {
                    let baseline = super::read_from_wayland_watch(event_loop, &watch);
                    self.last = if baseline.is_empty() {
                        initial.map_or_else(|| super::collect(event_loop), <[Display]>::to_vec)
                    } else {
                        super::remember(&baseline);
                        baseline
                    };
                    self.platform = PlatformWatch::Wayland(watch);
                    return;
                }
            } else {
                self.platform = PlatformWatch::X11;
            }
            self.last = initial.map_or_else(|| super::collect(event_loop), <[Display]>::to_vec);
            return;
        }
        #[cfg(target_os = "windows")]
        {
            let wake_proxy = proxy.clone();
            let wake = std::sync::Arc::new(move || {
                wake_proxy.send_event(crate::window::WakeEvent::DisplaysChanged).ok();
            });
            if let Some(watch) = win32::Win32DisplayWatch::new(wake) {
                self.platform = PlatformWatch::Win32(watch);
            }
            self.last = initial.map_or_else(|| super::collect(event_loop), <[Display]>::to_vec);
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let wake_proxy = proxy.clone();
            let wake = std::sync::Arc::new(move || {
                wake_proxy.send_event(crate::window::WakeEvent::DisplaysChanged).ok();
            });
            if let Some(watch) = macos::MacDisplayWatch::new(wake) {
                self.platform = PlatformWatch::MacOS(watch);
            }
            self.last = initial.map_or_else(|| super::collect(event_loop), <[Display]>::to_vec);
            return;
        }
        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))
        )))]
        {
            let _ = proxy;
            self.last = initial.map_or_else(|| super::collect(event_loop), <[Display]>::to_vec);
        }
    }

    /// Whether a Wayland output watch is live and needs winit's `wake_on_wayland_read` enabled.
    pub(crate) fn needs_wayland_wake(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        return matches!(self.platform, PlatformWatch::Wayland(_));
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        false
    }

    /// Syscall-free check whether the Wayland output watch already has unread events queued.
    pub(crate) fn has_pending_wayland_events(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let PlatformWatch::Wayland(watch) = &self.platform {
            return watch.has_pending_events();
        }
        false
    }

    /// Driven from `Windows::about_to_wait` to dispatch Wayland output watch events and inspect
    /// X11's winit-cached monitor list without extra threads or roundtrips.
    pub(crate) fn poll(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.hooks.is_empty() {
            return;
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        match &mut self.platform {
            PlatformWatch::Wayland(watch) => {
                watch.dispatch_pending();
                let current = super::read_from_wayland_watch(event_loop, watch);
                self.apply_snapshot(current);
            }
            PlatformWatch::X11 => {
                let current = super::read(event_loop);
                self.apply_snapshot(current);
            }
            PlatformWatch::None => {}
        }
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        let _ = event_loop;
    }

    /// Driven from `WakeEvent::DisplaysChanged` (Windows / macOS) to re-query displays and notify
    /// hooks of any removals, changes, or additions.
    pub(crate) fn refresh(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        #[cfg(target_os = "windows")]
        if let PlatformWatch::Win32(watch) = &self.platform {
            watch.clear_queued();
        }
        #[cfg(target_os = "macos")]
        if let PlatformWatch::MacOS(watch) = &self.platform {
            watch.clear_queued();
        }
        if self.hooks.is_empty() {
            return;
        }
        let current = super::read(event_loop);
        self.apply_snapshot(current);
    }

    /// Diffs `current` against `self.last`; when anything changed, updates `rune::displays()`
    /// first and invokes every registered hook with removals, then changes, then additions.
    pub(crate) fn apply_snapshot(&mut self, current: Vec<Display>) {
        let events = diff_displays(&self.last, &current);
        if events.is_empty() {
            return;
        }
        super::remember(&current);
        self.last = current;
        for event in &events {
            for hook in &mut self.hooks {
                hook(event, &self.last);
            }
        }
    }

    /// Tears down the platform watcher and drops all hooks strictly before `App::on_display_close`
    /// and before winit closes the native display connection.
    pub(crate) fn shutdown(&mut self) {
        self.platform = PlatformWatch::None;
        self.hooks.clear();
        self.started = false;
    }

    #[cfg(test)]
    pub(crate) fn test_with_hook(
        initial: Vec<Display>,
        hook: impl FnMut(&DisplayEvent, &[Display]) + 'static,
    ) -> Self {
        Self {
            hooks: vec![Box::new(hook)],
            last: initial,
            started: true,
            platform: PlatformWatch::None,
        }
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
