//! A 360×80 window that draws a frame as fast as the loop turns, then prints instructions per
//! frame and CPU time per frame on the drawing thread. `FRAMES` (2,000) are measured after `WARMUP` (300).

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::count::Counter;

pub trait Draw {
    fn frame(&mut self);
}

pub fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

struct Run<F> {
    make: F,
    draw: Option<(Arc<Window>, Box<dyn Draw>)>,
    counter: Option<Counter>,
    frames: u64,
    warmup: u64,
    measured: u64,
    instructions: (u64, u64, u64),
    started: Option<Instant>,
}

pub fn run(label: &str, make: impl FnMut(Arc<Window>) -> Box<dyn Draw> + 'static) {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut run = Run {
        make,
        draw: None,
        counter: Counter::new(),
        frames: 0,
        warmup: env("WARMUP", 300),
        measured: env("FRAMES", 2000),
        instructions: (0, 0, 0),
        started: None,
    };
    event_loop.run_app(&mut run).expect("run");
    let n = run.measured as f64;
    let wall = run.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    println!(
        "{label}: frames={} kinstr_user={:.2} kinstr_kernel={:.2} cpu_us={:.2} fps={:.0}",
        run.measured,
        run.instructions.0 as f64 / n / 1e3,
        run.instructions.1 as f64 / n / 1e3,
        run.instructions.2 as f64 / n / 1e3,
        n / wall
    );
}

impl<F: FnMut(Arc<Window>) -> Box<dyn Draw>> ApplicationHandler for Run<F> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.draw.is_none() {
            let attrs = Window::default_attributes().with_inner_size(winit::dpi::PhysicalSize::new(360, 80)).with_title("floor");
            let window = Arc::new(event_loop.create_window(attrs).expect("window"));
            self.draw = Some((window.clone(), (self.make)(window)));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some((_, draw)) = self.draw.as_mut() else { return };
        let timed = self.frames >= self.warmup;
        if timed && self.started.is_none() {
            self.started = Some(Instant::now());
        }
        let before = self.counter.as_ref().map(Counter::read).unwrap_or_default();
        draw.frame();
        if timed && let Some(c) = &self.counter {
            let after = c.read();
            self.instructions.0 += after.0 - before.0;
            self.instructions.1 += after.1 - before.1;
            self.instructions.2 += after.2 - before.2;
        }
        self.frames += 1;
        if self.frames >= self.warmup + self.measured {
            event_loop.exit();
        }
    }
}
