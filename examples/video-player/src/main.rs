//! Play a movie or audio file, or a URL. `cargo run -p video-player -- clip.mov` opens a video
//! looping and muted; tap to pause or resume. Drag the scrub bar to preview keyframes with
//! `Player::seek_fast` and land on the exact frame with `Player::seek` on release. An audio file
//! (mp3, m4a, wav) waits for a tap, since audio is audible. With no argument it plays the
//! four-frame test clip. `-- --snap out.png --at 0.6` renders the frame at 0.6 s headless. Pass
//! `--output-device <uid>` to route audio to a specific device UID on macOS.
//!
//! Keys: space or hardware media keys (play/pause, play, pause, stop, next/fast-forward, previous/rewind) control playback, left and right step one frame while paused (or seek 5 s while
//! playing), comma and period step one frame back or forward, m mutes, v turns the volume to 0
//! and back, d toggles the chosen output device and system default, l loops, 0 goes to the start,
//! h hides the clip (it stops drawing, as when scrolled away) and shows it again.

#![forbid(unsafe_code)]

use rune::tok::{self, type_};
use rune::{App, AppSpec, Cx, Drag, El, Key, KeyEvent, MediaKey, Phase, Player, PlayerState, SurfaceFit, Tx, Waker, col, el, row, text};
use std::time::Duration;

const WIDTH: f32 = 480.0;
const SCRUB_H: f32 = 20.0;
const KNOB: f32 = 12.0;
const TRACK_H: f32 = 4.0;
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg"];

struct Viewer {
    clip: Player,
    hidden: bool,
    preferred_output: Option<String>,
    scrub_target: Option<f64>,
    last_time: f64,
    last_presented: Option<f64>,
    last_seeking: bool,
}

fn parse_args(args: &[String]) -> (Option<String>, Option<String>) {
    let (mut source, mut output_device) = (None, None);
    let mut i = 0;
    while i < args.len() {
        if let Some(uid) = args[i].strip_prefix("--output-device=") {
            output_device = (!uid.is_empty()).then(|| uid.to_string());
        } else if args[i] == "--output-device" {
            if let Some(uid) = args.get(i + 1) {
                output_device = (!uid.is_empty()).then(|| uid.clone());
                i += 1;
            }
        } else if source.is_none() {
            source = Some(args[i].clone());
        }
        i += 1;
    }
    (source, output_device)
}

fn state() -> Viewer {
    let (source, preferred_output) = parse_args(rune::extra_args());
    let source = source
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4").into());
    let ext = source.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    let clip = if AUDIO.contains(&ext.as_str()) {
        Player::audio(source)
    } else {
        let clip = Player::new(source.into());
        clip.set_looping(true);
        clip.play();
        clip
    };
    if let Some(uid) = preferred_output.as_deref() {
        clip.set_output_device(Some(uid));
    }
    Viewer { clip, hidden: false, preferred_output, scrub_target: None, last_time: 0.0, last_presented: None, last_seeking: false }
}

/// The clip calls the waker when it loads, ends or fails, so the label follows.
fn setup(s: &mut Viewer, waker: Waker) {
    s.clip.set_waker(waker);
}

/// Scrub bar and seeking status: poll about 30 times a second while playing or waiting on a seek,
/// and stop asking for frames once idle.
fn poll(s: &mut Viewer, _: f64, tx: &mut Tx) -> bool {
    let playing = s.clip.is_playing();
    let seeking = s.clip.is_seeking();
    if playing || seeking {
        tx.wake_after(Duration::from_millis(33));
    }
    let time = s.clip.time();
    let presented = s.clip.presented_time();
    let changed = (time - s.last_time).abs() > 1e-3 || presented != s.last_presented || seeking != s.last_seeking;
    s.last_time = time;
    s.last_presented = presented;
    s.last_seeking = seeking;
    changed
}

fn toggle(s: &mut Viewer) {
    if s.clip.is_playing() { s.clip.pause() } else { s.clip.play() }
}

fn on_key(s: &mut Viewer, event: &KeyEvent, _: &mut Tx) -> bool {
    let clip = &s.clip;
    match &event.key {
        Key::Text(t) if t == " " => toggle(s),
        Key::Media(MediaKey::PlayPause) => toggle(s),
        Key::Media(MediaKey::Play) => clip.play(),
        Key::Media(MediaKey::Pause) => clip.pause(),
        Key::Media(MediaKey::Stop) => {
            clip.pause();
            clip.seek(0.0);
        }
        Key::Media(MediaKey::Next | MediaKey::FastForward) => clip.seek(clip.time() + 5.0),
        Key::Media(MediaKey::Previous | MediaKey::Rewind) => clip.seek((clip.time() - 5.0).max(0.0)),
        Key::Text(t) if t == "m" => clip.set_muted(!clip.muted()),
        Key::Text(t) if t == "v" => clip.set_volume(if clip.volume() > 0.0 { 0.0 } else { 1.0 }),
        Key::Text(t) if t == "d" => {
            let next = if clip.output_device().is_some() { None } else { s.preferred_output.as_deref() };
            clip.set_output_device(next);
        }
        Key::Text(t) if t == "l" => clip.set_looping(!clip.looping()),
        Key::Text(t) if t == "0" => clip.seek(0.0),
        Key::Text(t) if t == "h" => s.hidden = !s.hidden,
        Key::Text(t) if t == "," && !clip.is_audio() => clip.step(-1),
        Key::Text(t) if t == "." && !clip.is_audio() => clip.step(1),
        Key::Left if !clip.is_playing() && !clip.is_audio() => clip.step(-1),
        Key::Right if !clip.is_playing() && !clip.is_audio() => clip.step(1),
        Key::Left => clip.seek(clip.time() - 5.0),
        Key::Right => clip.seek(clip.time() + 5.0),
        _ => return false,
    }
    true
}

fn scrub_bar(s: &Viewer, t: &tok::Theme) -> El {
    let duration = s.clip.duration().filter(|d| *d > 0.0).unwrap_or(0.0);
    let shown_time = s.scrub_target.unwrap_or_else(|| s.clip.time()).clamp(0.0, duration.max(0.0));
    let progress = if duration > 0.0 { (shown_time / duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let travel = WIDTH - KNOB;
    let step = if duration > 0.0 { (duration / 20.0).clamp(0.1, 5.0) } else { 1.0 };
    let to_time = move |x: f32, dur: f64| f64::from(((x - KNOB / 2.0) / travel).clamp(0.0, 1.0)) * dur;
    let value_text = if duration > 0.0 {
        format!("{shown_time:.2} / {duration:.2} s")
    } else {
        format!("{shown_time:.2} s")
    };
    el().key("scrub")
        .role("slider")
        .label("Playback position")
        .range_value(shown_time, 0.0, duration.max(1e-6))
        .range_step(step)
        .a11y_value(value_text)
        .on_step(move |s: &mut Viewer, steps, _| {
            let dur = s.clip.duration().filter(|d| *d > 0.0).unwrap_or(0.0);
            if dur > 0.0 {
                let next = (s.clip.time() + steps as f64 * step).clamp(0.0, dur);
                s.clip.seek(next);
            }
        })
        .on_set_value(|s: &mut Viewer, value, _| s.clip.seek(value))
        .size(WIDTH, SCRUB_H)
        .on_drag(move |s: &mut Viewer, d: &mut Drag, tx| {
            let Some(dur) = s.clip.duration().filter(|d| *d > 0.0) else { return };
            let target = to_time(d.x, dur);
            match d.phase {
                Phase::Down => {
                    d.data = s.clip.time() as f32;
                    s.scrub_target = Some(target);
                    s.clip.seek_fast(target);
                }
                Phase::Move => {
                    s.scrub_target = Some(target);
                    s.clip.seek_fast(target);
                    tx.instant();
                }
                Phase::Up => {
                    s.scrub_target = None;
                    let final_time = if d.cancelled() { f64::from(d.data) } else { target };
                    s.clip.seek(final_time);
                }
            }
        })
        .child(
            el().key("scrub-track")
                .size(WIDTH, TRACK_H)
                .absolute(0.0, (SCRUB_H - TRACK_H) / 2.0)
                .radius(TRACK_H / 2.0)
                .fill(t.control_track)
                .a11y_hidden(),
        )
        .child(
            el().key("scrub-fill")
                .size(KNOB / 2.0 + progress * travel, TRACK_H)
                .absolute(0.0, (SCRUB_H - TRACK_H) / 2.0)
                .radius(TRACK_H / 2.0)
                .fill(t.indicator)
                .a11y_hidden(),
        )
        .child(
            el().key("scrub-knob")
                .size(KNOB, KNOB)
                .absolute(progress * travel, (SCRUB_H - KNOB) / 2.0)
                .radius(KNOB / 2.0)
                .fill(t.indicator)
                .a11y_hidden(),
        )
}

fn view(s: &Viewer, _: &mut Cx<Viewer>) -> El {
    let t = tok::theme(true);
    let base_label = match s.clip.state() {
        PlayerState::Loading => "Loading".to_string(),
        PlayerState::Playing => "Playing, tap to pause".to_string(),
        PlayerState::Ready => "Paused, tap to play".to_string(),
        PlayerState::Ended => "Ended, tap to replay".to_string(),
        PlayerState::Failed(e) => e,
        _ => String::new(),
    };
    let label = if s.clip.is_seeking() {
        format!("{base_label} · Seeking")
    } else if let Some(dur) = s.clip.duration().filter(|d| *d > 0.0) {
        let shown = s.scrub_target.unwrap_or_else(|| s.clip.time());
        format!("{base_label} · {shown:.2} / {dur:.2} s")
    } else {
        base_label
    };
    let mut root = col().w_full().h_full().center().gap(12.0).fill(t.bg);
    if !s.clip.is_audio() {
        let (w, h) = s.clip.natural_size().unwrap_or((16, 9));
        let frame = el().key("video").w(WIDTH).h(WIDTH * h as f32 / w as f32).radius(10.0).on_press(toggle);
        let media = if s.hidden { frame } else { frame.video(&s.clip, SurfaceFit::Fit) };
        root = root.child(media);
    }
    root = root
        .child(scrub_bar(s, &t))
        .child(text(label).font(type_::BODY).color(t.text_secondary).on_press(toggle));
    if !s.clip.is_audio() {
        let mut readout = Vec::new();
        if let Some(pts) = s.clip.presented_time() {
            if let Some(fd) = s.clip.frame_duration() {
                let index = (pts / fd + 1e-6).floor().max(0.0) as u64;
                readout.push(format!("Frame {index} · {pts:.3} s"));
            } else {
                readout.push(format!("Shown {pts:.3} s"));
            }
        }
        if let Some(fd) = s.clip.frame_duration() {
            readout.push(format!("{:.2} fps ({:.1} ms)", 1.0 / fd, fd * 1000.0));
        }
        let step_btn = |key: &'static str, title: &'static str, a11y: &'static str, delta: i32| {
            el().key(key)
                .role("button")
                .label(a11y)
                .px(8.0)
                .py(4.0)
                .radius(6.0)
                .fill(t.control_track)
                .on_press(move |s: &mut Viewer| s.clip.step(delta))
                .child(text(title).font(type_::LABEL).color(t.text))
        };
        let mut controls = row().gap(8.0).items_center()
            .child(step_btn("step-back", "-1f", "Previous frame", -1))
            .child(step_btn("step-forward", "+1f", "Next frame", 1));
        if !readout.is_empty() {
            controls = controls.child(text(readout.join(" · ")).font(type_::LABEL).color(t.text_secondary));
        }
        root = root.child(controls);
    }
    if let Some(uid) = s.clip.output_device() {
        root = root.child(text(format!("Output: {uid}")).font(type_::LABEL).color(t.text_secondary));
    } else if s.preferred_output.is_some() {
        root = root.child(text("Output: system default").font(type_::LABEL).color(t.text_secondary));
    }
    root
}

fn main() {
    let spec = AppSpec { setup: Some(setup), poll: Some(poll), on_key: Some(on_key), ..AppSpec::new("Video player", (520.0, 360.0), state, view) };
    App::new(spec).media_keys().run();
}

#[cfg(test)]
mod tests;
