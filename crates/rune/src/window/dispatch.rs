//! Native host event dispatch.

use super::*;

impl ApplicationHandler<WakeEvent> for Windows {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        let _handling = crate::quit::handling();
        for host in self.hosts.iter_mut().flatten() { host.new_events(event_loop, cause); }
        // Before resumed there are no hosts to clean up.
        if self.factory.is_none() { self.cleanup(event_loop); }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let _handling = crate::quit::handling();
        let Some(factory) = self.factory.take() else { return };
        crate::marks::mark("loop.resumed");
        let primary = event_loop.primary_monitor();
        let displays: Vec<_> = event_loop.available_monitors().map(|m| Display {
            id: Some(crate::DisplayId::from_monitor(m.clone(), is_wayland(event_loop))),
            name: m.name(), position: (m.position().x, m.position().y),
            size: (m.size().width, m.size().height), scale: m.scale_factor(), primary: primary.as_ref() == Some(&m),
        }).collect();
        let specs = crate::windows::flatten(factory(&displays));
        crate::windows::validate(&specs).expect("window keys");
        let trace_window = self.args.window.clone().or_else(|| specs.first().map(|s| s.key().to_owned()));
        let mut overlays = Vec::new();
        let wayland = is_wayland(event_loop);
        self.hosts = start_all(specs, |slot, spec| {
            let traced = trace_window.as_deref() == Some(spec.key());
            spec.start(self.args.clone(), Start { proxy: self.proxy.clone(), slot, gpu: self.gpu.clone(), wayland, traced, overlays: &mut overlays })
        }, |host| host.resumed(event_loop));
        self.connect_overlays(overlays, event_loop);
        #[cfg(target_os = "macos")]
        {
            let proxy = self.proxy.clone();
            crate::platform::reopen::install(move || proxy.send_event(WakeEvent::Reopen).is_ok());
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let proxy = self.proxy.clone();
            crate::open_url::instance::on_relaunch(move || proxy.send_event(WakeEvent::Reopen).is_ok());
        }
        let proxy = self.proxy.clone();
        crate::quit::install(move |ending| proxy.send_event(WakeEvent::Quit(ending)).is_ok());
        crate::open_url::incoming::settle();
        // A window opened at launch took it; a GPU kept for a later one would sit idle until then.
        crate::render::discard_prewarm();
        self.cleanup(event_loop);
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        let _handling = crate::quit::handling();
        #[cfg(all(feature = "deferred-quit", any(windows, target_os = "macos", target_os = "linux")))]
        if let WakeEvent::Deferred(m) = event { self.deferred_message(event_loop, m); return; }
        let slot = match &event {
            WakeEvent::DisplayTick(i) | WakeEvent::App(i) | WakeEvent::Live(i) | WakeEvent::Window(i, _) | WakeEvent::Focus(i)
                | WakeEvent::Position(i, _) | WakeEvent::Level(i, _) | WakeEvent::Place(i, _) | WakeEvent::Frame(i, _)
                | WakeEvent::A11y(i) | WakeEvent::FrameSlot(i) | WakeEvent::GpuReady(i) => *i,
            WakeEvent::Reopen => {
                #[cfg(all(feature = "deferred-quit", any(windows, target_os = "macos", target_os = "linux")))]
                if self.resume_quit(event_loop) { return; }
                self.reopen(event_loop);
                self.cleanup(event_loop);
                return;
            }
            // A session query (`ending: false`) can still be cancelled: the windows stay up.
            WakeEvent::Quit(ending) => {
                self.quit(event_loop, *ending, *ending);
                self.cleanup(event_loop);
                return;
            }
            WakeEvent::Turn => { #[cfg(not(target_arch = "wasm32"))] self.turn(event_loop); return; }
            #[cfg(all(feature = "deferred-quit", any(windows, target_os = "macos", target_os = "linux")))]
            WakeEvent::Deferred(_) => unreachable!(),
        };
        if let Some(Some(host)) = self.hosts.get_mut(slot) {
            host.user_event(event_loop, event);
        }
        self.cleanup(event_loop);
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let _handling = crate::quit::handling();
        // The last window's close ends a fast-exiting app: the window can go at once.
        #[cfg(all(feature = "deferred-quit", any(windows, target_os = "macos", target_os = "linux")))]
        if crate::deferred_quit::hooks().is_some() && matches!(event, WindowEvent::CloseRequested)
            && let Some(slot) = self.hosts.iter().position(|h| h.as_ref().is_some_and(|h| h.owns(id) && h.close_quits()))
            && self.last_quit_host(slot) {
            self.deferred_step(event_loop, crate::deferred_quit::Event::Quit(Instant::now(), crate::quit::After::Exit));
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(event, WindowEvent::CloseRequested) && crate::quit::fast_exit_hook().is_some()
            && !self.hosts.iter().flatten().any(|h| !h.owns(id) && !h.finished() && h.keeps_loop())
            && let Some(host) = self.hosts.iter_mut().flatten().find(|h| h.owns(id) && h.keeps_loop())
        {
            host.closing_ends_app();
        }
        if let Some(host) = self.hosts.iter_mut().flatten().find(|h| h.owns(id)) {
            let _input = crate::input::time::arrived(&event);
            host.window_event(event_loop, id, event);
        }
        self.cleanup(event_loop);
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _handling = crate::quit::handling();
        // Windows, Linux: a later launch handed to this loop (`single_instance`) is served once
        // the app goes on with a window shown. One that came as the app ended (its last window
        // already closing, the quit not started) isn't: the process starts the app again for it.
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if crate::open_url::instance::launches_waiting() && !event_loop.exiting() {
            let windows: Vec<_> = self.hosts.iter().map(|h| h.as_ref().map(|h| h.reopen_candidate()).unwrap_or_default()).collect();
            if crate::platform::reopen::has_visible(&windows) {
                crate::open_url::instance::served();
            }
        }
        crate::marks::loop_waits();
        crate::cursor::os::make_pending(event_loop);
        let mut animating = false;
        for host in self.hosts.iter_mut().flatten() {
            animating |= host.about_to_wait(event_loop);
        }
        // The frames windows want next (`redraw`), and on Windows the paints the system keeps
        // putting off behind another window's.
        let now = Instant::now();
        for host in self.hosts.iter_mut().flatten() { host.turn_ends(event_loop, now); }
        // Layer surfaces have no redraw events: keep turning while one animates (its vsync
        // paces the loop). Hosts whose only motion is an ambient loop sleep until its next step.
        let deadline = self.hosts.iter().flatten().filter_map(|host| host.wake_deadline()).min();
        #[cfg(all(feature = "deferred-quit", any(windows, target_os = "macos", target_os = "linux")))]
        let deadline = deadline.into_iter().chain(self.deferred_deadline(event_loop, now)).min();
        event_loop.set_control_flow(if animating { ControlFlow::Poll }
            else { deadline.map_or(ControlFlow::Wait, |at| ControlFlow::WaitUntil(crate::time::for_winit(at))) });
    }
    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        let _handling = crate::quit::handling();
        crate::marks::mark("loop.exiting");
        #[cfg(all(feature = "deferred-quit", any(windows, target_os = "macos", target_os = "linux")))]
        if crate::deferred_quit::hooks().is_some() {
            if !matches!(self.deferred.state, crate::deferred_quit::State::Confirmed) {
                // The OS ended the loop, not a durable receipt. Never run an install-on-confirm hook.
                crate::marks::mark("quit.interrupted");
                std::process::exit(2);
            }
            // Prepare and confirmation replace the legacy quit hooks. Running them here could
            // enqueue authoritative writes *after* the receipt that authorized this exit.
            for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
            if let Some(last) = crate::quit::fast_exit_hook() { crate::quit::end_fast(last); }
            self.hosts.clear();
            crate::clipboard::shutdown();
            if crate::quit::send(crate::quit::Event::Exited).contains(&crate::quit::Effect::Relaunch) { crate::relaunch::spawn(); }
            #[cfg(any(target_os="windows",target_os="linux"))] crate::open_url::instance::exited();
            return;
        }
        // Every quit ends here, so the windows still open run `App::on_quit` first, unless the
        // quit's hooks already ran (a Windows logoff, the last window closing).
        self.quit(event_loop, false, true);
        // Rune didn't ask to exit, so the OS is ending the app: macOS Quit, Cmd-Q, the Dock's
        // Quit, logout. Windows close as their close button would, before the clipboard stops.
        if !event_loop.exiting() {
            for host in self.hosts.iter_mut().flatten() { host.terminate(); }
        }
        for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(last) = crate::quit::fast_exit_hook() { crate::quit::end_fast(last); }
        // Surfaces go while winit's display connection is still open (layer surfaces live on it).
        self.hosts.clear();
        crate::marks::mark("windows.dropped");
        crate::clipboard::shutdown();
        if crate::quit::send(crate::quit::Event::Exited).contains(&crate::quit::Effect::Relaunch) {
            crate::relaunch::spawn();
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        crate::open_url::instance::exited();
    }
}
