use std::cell::RefCell;
use std::rc::Rc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::super::{AnyHost, WakeEvent};
use super::{ExitFlags, ExitStep, exiting_hosts, prepare_fast_exit_wayland_with};
use crate::quit::{After, Effect, Event as QuitEvent, Hooks, State as QuitState, step as quit_step};

struct WaylandExitHost {
    name: &'static str,
    events: Rc<RefCell<Vec<String>>>,
}

impl WaylandExitHost {
    fn new(name: &'static str, events: Rc<RefCell<Vec<String>>>) -> Self {
        Self { name, events }
    }
}

impl Drop for WaylandExitHost {
    fn drop(&mut self) {
        self.events.borrow_mut().push(format!("{}:drop", self.name));
    }
}

impl AnyHost for WaylandExitHost {
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn user_event(&mut self, _: &ActiveEventLoop, _: WakeEvent) {}
    fn owns(&self, _: WindowId) -> bool { false }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    fn finished(&self) -> bool { false }
    fn vanish(&mut self) {
        self.events.borrow_mut().push(format!("{}:vanish", self.name));
    }
    fn quit_hook(&mut self) {
        self.events.borrow_mut().push(format!("{}:on_quit", self.name));
    }
    fn release_wayland_surface(&mut self) {
        self.events.borrow_mut().push(format!("{}:release_wayland_surface", self.name));
    }
}

fn step_quit(state: &mut QuitState) -> impl FnMut(QuitEvent) -> Vec<Effect> + '_ {
    move |event| {
        let (next, effects) = quit_step(*state, event);
        *state = next;
        effects
    }
}

#[test]
fn fast_exit_on_wayland_releases_all_host_surfaces_reaps_winit_shuts_down_globals_and_syncs_in_order() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![
        Some(Box::new(WaylandExitHost::new("main", events.clone()))),
        None,
        Some(Box::new(WaylandExitHost::new("overlay", events.clone()))),
    ];
    let mut display_close: Vec<crate::app::OnDisplayClose> = vec![{
        let events = events.clone();
        Box::new(move || events.borrow_mut().push("on_display_close".into()))
    }];
    let mut quit_state = QuitState::Quitting(Hooks::Owed, After::Exit);

    let step = exiting_hosts(
        &mut hosts,
        &mut display_close,
        ExitFlags {
            has_startup_error: false,
            has_display_error: false,
            loop_exiting: true,
            quitting: true,
            fast_exit: true,
        },
        step_quit(&mut quit_state),
    );
    assert_eq!(step, ExitStep::FastExit);

    prepare_fast_exit_wayland_with(
        &mut hosts,
        true,
        || events.borrow_mut().push("reap_winit_windows".into()),
        || events.borrow_mut().push("shutdown_wayland_globals".into()),
        || events.borrow_mut().push("sync_display".into()),
    );

    // Hosts themselves remain undropped (fast exit still skips GPU/app state drop), while every
    // Wayland surface is released, reaped from winit, globals shut down, and synced before _exit.
    assert!(hosts[0].is_some());
    assert!(hosts[2].is_some());
    assert_eq!(
        *events.borrow(),
        vec![
            "main:vanish",
            "overlay:vanish",
            "main:on_quit",
            "overlay:on_quit",
            "on_display_close",
            "main:release_wayland_surface",
            "overlay:release_wayland_surface",
            "reap_winit_windows",
            "shutdown_wayland_globals",
            "sync_display",
        ],
    );
}

#[test]
fn fast_exit_off_wayland_is_a_noop_and_keeps_surfaces_untouched() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut hosts: Vec<Option<Box<dyn AnyHost>>> = vec![
        Some(Box::new(WaylandExitHost::new("main", events.clone()))),
    ];

    prepare_fast_exit_wayland_with(
        &mut hosts,
        false,
        || events.borrow_mut().push("reap_winit_windows".into()),
        || events.borrow_mut().push("shutdown_wayland_globals".into()),
        || events.borrow_mut().push("sync_display".into()),
    );

    assert!(events.borrow().is_empty(), "non-Wayland fast exit must not run Wayland teardown: {:?}", *events.borrow());
}
