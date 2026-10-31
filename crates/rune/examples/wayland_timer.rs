use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use rune::{App, AppSpec, Cx, El, Tx, col, text};

static POLLS: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct State { updates: u64, polls: u64 }

fn view(s: &State, _: &mut Cx<State>) -> El {
    eprintln!("view updates={} polls={}", s.updates, s.polls);
    col().w_full().h_full().center().gap(16.0)
        .child(text("Move this window to an inactive Wayland workspace"))
        .child(text(format!("Updates: {}   Polls: {}", s.updates, s.polls)))
}

fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    s.polls = POLLS.fetch_add(1, Ordering::Relaxed) + 1;
    tx.wake_after(Duration::from_millis(33));
    false
}

fn main() {
    let app = App::new(AppSpec {
        poll: Some(poll),
        ..AppSpec::new("Wayland timer", (640.0, 160.0), State::default, view)
    });
    let handle = app.handle();
    app.on_ready(move |_| {
        let handle = handle.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            eprintln!("polls={}", POLLS.load(Ordering::Relaxed));
            handle.update(|s, tx| { s.updates += 1; tx.instant(); });
        });
    }).run();
}
