//! Play a movie or audio file, or a URL. `cargo run -p video-player -- clip.mov` opens a video
//! looping and muted; tap to pause or resume. An audio file (mp3, m4a, wav) shows a progress
//! bar and waits for a tap, since audio is audible. With no argument it plays the four-frame
//! test clip. `-- --snap out.png --at 0.6` renders the frame at 0.6 s headless.
//!
//! Keys: space plays or pauses, m mutes, v turns the volume to 0 and back, l loops, left and right seek 5 s, 0 goes to the start,
//! h hides the clip (it stops drawing, as when scrolled away) and shows it again.

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, Key, KeyEvent, Player, PlayerState, SurfaceFit, Tx, Waker, col, el, row, text};
use std::time::Duration;

const WIDTH: f32 = 480.0;
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg"];

struct Viewer {
    clip: Player,
    hidden: bool,
}

fn state() -> Viewer {
    let source = rune::extra_args()
        .first()
        .cloned()
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4").into());
    let ext = source.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    if AUDIO.contains(&ext.as_str()) {
        return Viewer { clip: Player::audio(source), hidden: false };
    }
    let clip = Player::new(source.into());
    clip.set_looping(true);
    clip.play();
    Viewer { clip, hidden: false }
}

/// The clip calls the waker when it loads, ends or fails, so the label follows.
fn setup(s: &mut Viewer, waker: Waker) {
    s.clip.set_waker(waker);
}

/// Audio progress: re-run the view about 30 times a second while it plays, and never otherwise.
fn poll(s: &mut Viewer, _: f64, tx: &mut Tx) -> bool {
    if s.clip.is_audio() && s.clip.is_playing() {
        tx.wake_after(Duration::from_millis(33));
    }
    s.clip.is_audio()
}

fn toggle(s: &mut Viewer) {
    if s.clip.is_playing() { s.clip.pause() } else { s.clip.play() }
}

fn on_key(s: &mut Viewer, event: &KeyEvent, _: &mut Tx) -> bool {
    let clip = &s.clip;
    match &event.key {
        Key::Text(t) if t == " " => toggle(s),
        Key::Text(t) if t == "m" => clip.set_muted(!clip.muted()),
        Key::Text(t) if t == "v" => clip.set_volume(if clip.volume() > 0.0 { 0.0 } else { 1.0 }),
        Key::Text(t) if t == "l" => clip.set_looping(!clip.looping()),
        Key::Text(t) if t == "0" => clip.seek(0.0),
        Key::Text(t) if t == "h" => s.hidden = !s.hidden,
        Key::Left => clip.seek(clip.time() - 5.0),
        Key::Right => clip.seek(clip.time() + 5.0),
        _ => return false,
    }
    true
}

fn view(s: &Viewer, _: &mut Cx<Viewer>) -> El {
    let t = tok::theme(true);
    let label = match s.clip.state() {
        PlayerState::Loading => "Loading".to_string(),
        PlayerState::Playing => "Playing, tap to pause".to_string(),
        PlayerState::Ready => "Paused, tap to play".to_string(),
        PlayerState::Ended => "Ended, tap to replay".to_string(),
        PlayerState::Failed(e) => e,
        _ => String::new(),
    };
    let media = if s.clip.is_audio() {
        let progress = s.clip.duration().filter(|d| *d > 0.0).map_or(0.0, |d| (s.clip.time() / d) as f32);
        row()
            .key("progress")
            .w(WIDTH)
            .h(4.0)
            .radius(2.0)
            .fill(t.control_track)
            .child(el().w(WIDTH * progress.clamp(0.0, 1.0)).h(4.0).radius(2.0).fill(t.indicator))
    } else {
        let (w, h) = s.clip.natural_size().unwrap_or((16, 9));
        let frame = el().key("video").w(WIDTH).h(WIDTH * h as f32 / w as f32).radius(10.0);
        if s.hidden { frame } else { frame.video(&s.clip, SurfaceFit::Fit) }
    };
    col().w_full().h_full().center().gap(12.0).fill(t.bg).on_press(toggle).child(media).child(text(label).font(type_::BODY).color(t.text_secondary))
}

fn main() {
    rune::run(AppSpec { setup: Some(setup), poll: Some(poll), on_key: Some(on_key), ..AppSpec::new("Video player", (520.0, 340.0), state, view) });
}
