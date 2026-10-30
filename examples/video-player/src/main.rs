//! Play a movie or audio file, or a URL. `cargo run -p video-player -- clip.mkv` opens a video
//! looping with sound; tap the picture to pause or resume, drag the scrubber to seek. An audio
//! file (mp3, m4a, wav, opus, …) shows a scrubber and waits for a tap on the bar area to play.
//! With no argument it plays the four-frame test clip muted. `-- --snap out.png --at 0.6`
//! renders the frame at 0.6 s headless.

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, Drag, El, Phase, Player, PlayerState, SurfaceFit, Tx, Waker, col, el, text};
use std::time::Duration;

const WIDTH: f32 = 480.0;
const SCRUB_H: f32 = 22.0;
const KNOB: f32 = 14.0;
const BAR: f32 = 4.0;
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg", "oga", "opus"];

struct Viewer {
    clip: Player,
    /// While dragging the scrubber, the time under the pointer (committed on release).
    scrub: Option<f64>,
}

fn state() -> Viewer {
    let from_arg = rune::extra_args().first().cloned();
    let source = from_arg.clone().unwrap_or_else(|| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4").into()
    });
    let ext = source.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    if AUDIO.contains(&ext.as_str()) {
        return Viewer { clip: Player::audio(source), scrub: None };
    }
    let clip = Player::new(source.into());
    clip.set_looping(true);
    // User-provided movies should be audible; the silent test fixture stays muted.
    if from_arg.is_some() {
        clip.set_muted(false);
    }
    clip.play();
    Viewer { clip, scrub: None }
}

/// The clip calls the waker when it loads, ends or fails, so the label follows.
fn setup(s: &mut Viewer, waker: Waker) {
    s.clip.set_waker(waker);
}

/// Keep the scrubber and time label moving while the clip plays (or while dragging).
fn poll(s: &mut Viewer, _: f64, tx: &mut Tx) -> bool {
    if s.clip.is_playing() || s.scrub.is_some() {
        tx.wake_after(Duration::from_millis(33));
    }
    true
}

fn toggle(s: &mut Viewer) {
    if s.clip.is_playing() { s.clip.pause() } else { s.clip.play() }
}

fn format_time(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

fn scrubber(s: &Viewer) -> El {
    let t = tok::theme(true);
    let duration = s.clip.duration().filter(|d| *d > 0.0);
    let time = s.scrub.unwrap_or_else(|| s.clip.time());
    let progress = duration.map_or(0.0, |d| (time / d) as f32).clamp(0.0, 1.0);
    let travel = WIDTH - KNOB;
    let to_time = move |x: f32| -> f64 {
        let u = ((x - KNOB / 2.0) / travel).clamp(0.0, 1.0) as f64;
        duration.map_or(0.0, |d| u * d)
    };
    let stamp = match duration {
        Some(d) => format!("{} / {}", format_time(time), format_time(d)),
        None => format_time(time),
    };
    col()
        .key("scrubber")
        .w(WIDTH)
        .gap(6.0)
        .child(
            el().key("slider")
                .role("slider")
                .label("Seek")
                .size(WIDTH, SCRUB_H)
                .on_drag(move |s: &mut Viewer, d: &mut Drag, tx| {
                    let cur = s.scrub.unwrap_or_else(|| s.clip.time());
                    let dur = s.clip.duration().unwrap_or(0.0);
                    let u = if dur > 0.0 { (cur / dur) as f32 } else { 0.0 };
                    let knob_x = KNOB / 2.0 + u.clamp(0.0, 1.0) * travel;
                    match d.phase {
                        Phase::Down => {
                            if (d.x - knob_x).abs() <= KNOB / 2.0 + 3.0 {
                                d.data = d.x - knob_x;
                            } else {
                                d.data = 0.0;
                            }
                            let at = to_time(d.x - d.data);
                            s.scrub = Some(at);
                            s.clip.seek(at);
                            tx.instant();
                        }
                        Phase::Move => {
                            let at = to_time(d.x - d.data);
                            s.scrub = Some(at);
                            s.clip.seek(at);
                            tx.instant();
                        }
                        Phase::Up => {
                            if let Some(at) = s.scrub.take() {
                                s.clip.seek(at);
                            }
                        }
                    }
                })
                .child(el().key("track").size(WIDTH, BAR).absolute(0.0, (SCRUB_H - BAR) / 2.0).radius(BAR / 2.0).fill(t.control_track))
                .child(
                    el().key("fill")
                        .size(KNOB / 2.0 + progress * travel, BAR)
                        .absolute(0.0, (SCRUB_H - BAR) / 2.0)
                        .radius(BAR / 2.0)
                        .fill(t.indicator),
                )
                .child(
                    el().key("knob")
                        .size(KNOB, KNOB)
                        .absolute(progress * travel, (SCRUB_H - KNOB) / 2.0)
                        .radius(KNOB / 2.0)
                        .fill(t.indicator),
                ),
        )
        .child(text(stamp).key("time").font(type_::LABEL).color(t.text_secondary))
}

fn view(s: &Viewer, _: &mut Cx<Viewer>) -> El {
    let t = tok::theme(true);
    let label = match s.clip.state() {
        PlayerState::Loading => "Loading".to_string(),
        PlayerState::Playing => "Playing, tap media to pause · drag to seek".to_string(),
        PlayerState::Ready => "Paused, tap media to play · drag to seek".to_string(),
        PlayerState::Ended => "Ended, tap media to replay · drag to seek".to_string(),
        PlayerState::Failed(e) => e,
        _ => String::new(),
    };
    let media = if s.clip.is_audio() {
        el().key("audio")
            .w(WIDTH)
            .h(48.0)
            .radius(10.0)
            .fill(t.control_track)
            .center()
            .on_press(toggle)
            .child(text("Audio").font(type_::BODY).color(t.text_secondary))
    } else {
        let (w, h) = s.clip.natural_size().unwrap_or((16, 9));
        el().key("video")
            .w(WIDTH)
            .h(WIDTH * h as f32 / w as f32)
            .radius(10.0)
            .video(&s.clip, SurfaceFit::Fit)
            .on_press(toggle)
    };
    col()
        .w_full()
        .h_full()
        .center()
        .gap(12.0)
        .fill(t.bg)
        .child(media)
        .child(scrubber(s))
        .child(text(label).font(type_::BODY).color(t.text_secondary))
}

fn main() {
    rune::run(AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        ..AppSpec::new("Video player", (520.0, 400.0), state, view)
    });
}
