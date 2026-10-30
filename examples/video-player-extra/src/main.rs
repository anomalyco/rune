//! Full-chrome video player lab. Title bar, play bar, keyboard shortcuts with a toast stack,
//! seek hover preview, speed menu, volume, fullscreen chrome that auto-hides. Click the video
//! to play/pause. Audio / video / subtitle tracks via the hamburger menu (Linux backends).
//!
//! Pick one Linux backend (`gstreamer` or `libmpv`); macOS ignores both.
//!
//! ```text
//! cargo run -p video-player-extra --features gstreamer -- clip.mkv
//! cargo run -p video-player-extra --features libmpv -- clip.mkv
//! ```

#[cfg(all(target_os = "linux", not(any(feature = "gstreamer", feature = "libmpv"))))]
compile_error!("pick a Linux Player backend: --features gstreamer  or  --features libmpv");
#[cfg(all(feature = "gstreamer", feature = "libmpv"))]
compile_error!("enable only one of --features gstreamer or --features libmpv");

use rune::tok::type_;
use rune::{
    AppSpec, Cx, Drag, El, FileDialogOptions, FileDialogs, Icon, Key, KeyEvent, MediaTrackKind, Phase, Pixels, Player, PlayerState,
    ScrollSpec, SurfaceFit, Tx, Waker, col, el, row, spacer, text,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Dialog, IconButton, IconButtonVariant, Keybind, Kit, Menu, MenuEntry, MenuItem, Placement,
    Toast, root,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WIN: (f32, f32) = (860.0, 520.0);
const TITLE_H: f32 = 40.0;
const PLAY_H: f32 = 64.0;
const SCRUB_H: f32 = 18.0;
const KNOB: f32 = 12.0;
const BAR: f32 = 4.0;
const VOL_W: f32 = 88.0;
const PREVIEW_W: f32 = 160.0;
const CHROME_IDLE: f64 = 2.5;
const TOAST_LIFE: f64 = 1.6;
const TOAST_MAX: usize = 3;
const EDGE: f32 = 56.0;
const RATES: &[f32] = &[0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg", "oga", "opus"];

static PLAY: Icon = Icon {
    name: "play",
    view: 16.0,
    body: r#"<path d="M5 3.5L13 8L5 12.5V3.5Z" fill="currentColor"/>"#,
};
static PAUSE: Icon = Icon {
    name: "pause",
    view: 16.0,
    body: r#"<path d="M4 3H7V13H4V3ZM9 3H12V13H9V3Z" fill="currentColor"/>"#,
};
static MENU: Icon = Icon {
    name: "menu",
    view: 16.0,
    body: r#"<path d="M2.5 4H13.5M2.5 8H13.5M2.5 12H13.5" stroke="currentColor" stroke-linecap="round"/>"#,
};
static VOLUME: Icon = Icon {
    name: "volume",
    view: 16.0,
    body: r#"<path d="M2 6H5L8 3.5V12.5L5 10H2V6Z" fill="currentColor"/><path d="M10 6C11 6.8 11.5 7.8 11.5 8.5C11.5 9.2 11 10.2 10 11" stroke="currentColor" fill="none"/>"#,
};
static MUTE: Icon = Icon {
    name: "mute",
    view: 16.0,
    body: r#"<path d="M2 6H5L8 3.5V12.5L5 10H2V6Z" fill="currentColor"/><path d="M10 6L14 10M14 6L10 10" stroke="currentColor" stroke-linecap="round"/>"#,
};
static FULLSCREEN: Icon = Icon {
    name: "fullscreen",
    view: 16.0,
    body: r#"<path d="M3 6V3H6M10 3H13V6M13 10V13H10M6 13H3V10" stroke="currentColor" stroke-linecap="round" fill="none"/>"#,
};

/// One help row: either keycap chords (alternatives separated by `/`) or a plain label.
enum HelpKeys {
    /// Alternatives of chords, e.g. `Alt+←` / `Alt+→` → `&[&["Alt", "←"], &["Alt", "→"]]`.
    Caps(&'static [&'static [&'static str]]),
    Label(&'static str),
}

struct Shortcut {
    keys: HelpKeys,
    action: &'static str,
}

const HELP: &[Shortcut] = &[
    Shortcut { keys: HelpKeys::Caps(&[&["Space"]]), action: "Play / pause" },
    Shortcut { keys: HelpKeys::Label("Click video"), action: "Play / pause" },
    Shortcut { keys: HelpKeys::Caps(&[&["←"], &["→"]]), action: "Seek ±10 s" },
    Shortcut { keys: HelpKeys::Caps(&[&["Alt", "←"], &["Alt", "→"]]), action: "Seek ±3 s" },
    Shortcut { keys: HelpKeys::Caps(&[&["Ctrl", "←"], &["Ctrl", "→"]]), action: "Seek ±60 s" },
    Shortcut { keys: HelpKeys::Caps(&[&["↑"], &["↓"]]), action: "Volume ±5%" },
    Shortcut { keys: HelpKeys::Caps(&[&["M"]]), action: "Mute / unmute" },
    Shortcut { keys: HelpKeys::Caps(&[&["["], &["]"]]), action: "Slower / faster" },
    Shortcut { keys: HelpKeys::Caps(&[&["F"], &["Enter"]]), action: "Fullscreen" },
    Shortcut { keys: HelpKeys::Label("Double-click video"), action: "Fullscreen" },
    Shortcut { keys: HelpKeys::Caps(&[&["Esc"]]), action: "Exit fullscreen" },
    Shortcut { keys: HelpKeys::Caps(&[&["L"]]), action: "Loop on / off" },
    Shortcut { keys: HelpKeys::Caps(&[&["0–9"]]), action: "Jump to 0%…90%" },
    Shortcut { keys: HelpKeys::Caps(&[&["O"]]), action: "Open file (system picker)" },
    Shortcut { keys: HelpKeys::Caps(&[&["?"], &["H"]]), action: "Keyboard help" },
];

const MEDIA_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "m4v", "mpg", "mpeg", "mp3", "m4a", "aac", "wav", "aif", "aiff", "flac", "ogg",
    "oga", "opus",
];

struct ToastItem {
    id: u64,
    title: String,
    /// Keycap labels for the chord that fired this action (empty when not from a key).
    keys: Vec<String>,
    until: f64,
}

struct Preview {
    pixels: Pixels,
    want: Arc<Mutex<Option<f64>>>,
    busy: Arc<AtomicBool>,
}

struct Viewer {
    path: String,
    clip: Player,
    scrub: Option<f64>,
    hover_seek: Option<(f32, f64)>,
    preview: Preview,
    menu_open: bool,
    help_open: bool,
    /// Result of a system file picker, applied on the next `poll`.
    pending_open: Arc<Mutex<Option<Result<PathBuf, String>>>>,
    picking_file: Arc<AtomicBool>,
    speed_open: bool,
    /// Which tracks submenu is open (Subtitles / Audio / Video).
    track_menu: Option<MediaTrackKind>,
    fullscreen: bool,
    chrome_until: f64,
    /// Last fullscreen chrome visibility; poll dirty-checks against this so idle hide commits.
    chrome_shown: bool,
    /// Pointer is over the title bar / play bar (real hover — not a guessed Y band).
    title_hover: bool,
    play_hover: bool,
    /// Pointer is over a fullscreen edge strip (reveals chrome while it stays hidden).
    edge_hover: bool,
    viewport: (f32, f32),
    toasts: Vec<ToastItem>,
    toast_seq: u64,
    now: f64,
    waker: Option<Waker>,
}

fn is_audio(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    AUDIO.contains(&ext.as_str())
}

fn display_name(path: &str) -> String {
    if path.contains("://") {
        path.rsplit('/').next().unwrap_or(path).to_string()
    } else {
        Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string()
    }
}

fn open_clip(path: &str) -> Player {
    if is_audio(path) {
        return Player::audio(path);
    }
    let clip = Player::new(path.into());
    clip.set_looping(true);
    clip.set_muted(false);
    clip.set_volume(1.0);
    clip.play();
    clip
}

fn new_preview() -> Preview {
    Preview { pixels: Pixels::new(), want: Arc::new(Mutex::new(None)), busy: Arc::new(AtomicBool::new(false)) }
}

fn state() -> Viewer {
    let from_arg = rune::extra_args().first().cloned();
    let path = from_arg.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/rune/tests/fixtures/rgbw.mp4")
            .to_string_lossy()
            .into_owned()
    });
    let clip = open_clip(&path);
    Viewer {
        path,
        clip,
        scrub: None,
        hover_seek: None,
        preview: new_preview(),
        menu_open: false,
        help_open: false,
        pending_open: Arc::new(Mutex::new(None)),
        picking_file: Arc::new(AtomicBool::new(false)),
        speed_open: false,
        track_menu: None,
        fullscreen: false,
        chrome_until: CHROME_IDLE,
        chrome_shown: true,
        title_hover: false,
        play_hover: false,
        edge_hover: false,
        viewport: WIN,
        toasts: Vec::new(),
        toast_seq: 0,
        now: 0.0,
        waker: None,
    }
}

fn setup(s: &mut Viewer, waker: Waker) {
    s.clip.set_waker(waker.clone());
    s.waker = Some(waker);
}

fn bump_chrome(s: &mut Viewer) {
    s.chrome_until = s.now + CHROME_IDLE;
}

fn toast(s: &mut Viewer, title: impl Into<String>, keys: &[&str]) {
    let title = title.into();
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_string()).collect();
    if let Some(t) = s.toasts.iter_mut().find(|t| t.title == title && t.keys == keys) {
        t.until = s.now + TOAST_LIFE;
        return;
    }
    s.toast_seq += 1;
    s.toasts.insert(0, ToastItem { id: s.toast_seq, title, keys, until: s.now + TOAST_LIFE });
    s.toasts.truncate(TOAST_MAX);
}

fn toggle_play(s: &mut Viewer, keys: &[&str]) {
    if s.clip.is_playing() {
        s.clip.pause();
        if !keys.is_empty() {
            toast(s, "Paused", keys);
        }
    } else {
        s.clip.play();
        if !keys.is_empty() {
            toast(s, "Playing", keys);
        }
    }
    bump_chrome(s);
}

fn seek_by(s: &mut Viewer, delta: f64, keys: &[&str]) {
    let dur = s.clip.duration().unwrap_or(f64::INFINITY);
    let at = (s.clip.time() + delta).clamp(0.0, dur);
    s.clip.seek(at);
    let label = if delta < 0.0 { format!("{delta:.0}s") } else { format!("+{delta:.0}s") };
    toast(s, label, keys);
    bump_chrome(s);
}

fn set_volume(s: &mut Viewer, volume: f32, keys: &[&str]) {
    let v = volume.clamp(0.0, 1.0);
    s.clip.set_volume(v);
    if v > 0.0 {
        s.clip.set_muted(false);
    }
    toast(s, format!("Volume {:.0}%", v * 100.0), keys);
    bump_chrome(s);
}

fn nudge_volume(s: &mut Viewer, delta: f32, keys: &[&str]) {
    set_volume(s, s.clip.volume() + delta, keys);
}

fn toggle_mute(s: &mut Viewer, keys: &[&str]) {
    let on = !s.clip.muted();
    s.clip.set_muted(on);
    toast(s, if on { "Muted" } else { "Unmuted" }, keys);
    bump_chrome(s);
}

fn set_rate(s: &mut Viewer, rate: f32, keys: &[&str]) {
    s.clip.set_rate(rate);
    toast(s, format!("Speed {rate:.2}×"), keys);
    bump_chrome(s);
}

fn step_rate(s: &mut Viewer, dir: i32, keys: &[&str]) {
    let cur = s.clip.rate();
    let i = RATES.iter().position(|r| (*r - cur).abs() < 1e-3).unwrap_or(2) as i32;
    let next = RATES[(i + dir).clamp(0, RATES.len() as i32 - 1) as usize];
    set_rate(s, next, keys);
}

fn toggle_loop(s: &mut Viewer, keys: &[&str]) {
    let on = !s.clip.looping();
    s.clip.set_looping(on);
    toast(s, if on { "Loop on" } else { "Loop off" }, keys);
    bump_chrome(s);
}

fn toggle_fullscreen(s: &mut Viewer, _tx: &mut Tx, keys: &[&str]) {
    // Chrome auto-hide mode only — borderless window fullscreen needs a separate framework API.
    s.fullscreen = !s.fullscreen;
    bump_chrome(s);
    toast(s, if s.fullscreen { "Fullscreen" } else { "Windowed" }, keys);
}

fn jump_percent(s: &mut Viewer, pct: u8, keys: &[&str]) {
    let Some(d) = s.clip.duration().filter(|d| *d > 0.0) else { return };
    s.clip.seek(d * (pct as f64 / 100.0));
    toast(s, format!("{pct}%"), keys);
    bump_chrome(s);
}

fn load_path(s: &mut Viewer, path: String) {
    s.path = path.clone();
    s.clip = open_clip(&path);
    if let Some(w) = &s.waker {
        s.clip.set_waker(w.clone());
    }
    s.scrub = None;
    s.hover_seek = None;
    s.preview = new_preview();
    s.menu_open = false;
    s.speed_open = false;
    s.track_menu = None;
    toast(s, display_name(&path), &["O"]);
}

/// Opens the OS file picker off the UI thread (Cocoa deadlocks if the window thread `block_on`s it).
fn start_system_open(s: &mut Viewer) {
    s.menu_open = false;
    if s.picking_file.swap(true, Ordering::SeqCst) {
        return;
    }
    let pending = s.pending_open.clone();
    let picking = s.picking_file.clone();
    let waker = s.waker.clone();
    // Only seed a real directory — a missing/URL "parent" makes the XDG portal fail, and rfd
    // then returns cancel (None) with no error after zenity is also missing.
    let start = Path::new(&s.path).parent().filter(|p| p.is_dir()).map(Path::to_path_buf);
    async_std::task::spawn(async move {
        let mut options = FileDialogOptions { title: "Open media".into(), directory: start, ..Default::default() };
        options = options.filter("Media", MEDIA_EXTS);
        let result = FileDialogs::native().open(options).await;
        picking.store(false, Ordering::SeqCst);
        match result {
            Ok(Some(path)) => *pending.lock().unwrap() = Some(Ok(path)),
            Ok(None) => {}
            Err(e) => *pending.lock().unwrap() = Some(Err(e.0)),
        }
        if let Some(w) = waker {
            w();
        }
    });
}

fn request_preview(s: &Viewer, at: f64) {
    if s.clip.is_audio() {
        return;
    }
    // Snap to ~100 ms so the worker skips near-duplicate seeks while the pointer moves.
    let at = (at * 10.0).round() / 10.0;
    {
        let mut want = s.preview.want.lock().unwrap();
        if *want == Some(at) {
            return;
        }
        *want = Some(at);
    }
    if s.preview.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let want = s.preview.want.clone();
    let busy = s.preview.busy.clone();
    let pixels = s.preview.pixels.clone();
    let clip = s.clip.clone();
    let waker = s.waker.clone();
    let _ = std::thread::Builder::new().name("preview".into()).spawn(move || {
        loop {
            let Some(at) = ({
                let mut slot = want.lock().unwrap();
                let next = slot.take();
                if next.is_none() {
                    busy.store(false, Ordering::SeqCst);
                }
                next
            }) else {
                return;
            };
            if let Ok(frame) = clip.decode_frame_at(at) {
                let mut rgba = frame.planes[0].clone();
                for px in rgba.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                pixels.set(frame.width, frame.height, rgba);
                if let Some(w) = &waker {
                    w();
                }
            }
        }
    });
}

fn format_time(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

fn chrome_visible(s: &Viewer) -> bool {
    if !s.fullscreen {
        return true;
    }
    if s.menu_open || s.help_open || s.speed_open || s.track_menu.is_some() || s.scrub.is_some() {
        return true;
    }
    // Idle timer, hovering the bars, or parked on an edge strip (reveal while hidden).
    s.now < s.chrome_until || s.title_hover || s.play_hover || s.edge_hover
}

fn poll(s: &mut Viewer, t: f64, tx: &mut Tx) -> bool {
    s.now = t;
    let mut changed = false;
    let pending = s.pending_open.lock().unwrap().take();
    if let Some(result) = pending {
        match result {
            Ok(path) => load_path(s, path.to_string_lossy().into_owned()),
            Err(e) => toast(s, e, &[]),
        }
        changed = true;
    }
    let before = s.toasts.len();
    s.toasts.retain(|toast| toast.until > t);
    changed |= before != s.toasts.len();

    let show = chrome_visible(s);
    if show != s.chrome_shown {
        s.chrome_shown = show;
        // Bars / edge strips unmount without a hover-leave; clear so stale hover can't pin chrome.
        if show {
            s.edge_hover = false;
        } else {
            s.title_hover = false;
            s.play_hover = false;
        }
        changed = true;
    }
    if !s.fullscreen {
        s.edge_hover = false;
        s.title_hover = false;
        s.play_hover = false;
    }
    // Wake until the idle deadline so chrome can actually disappear after CHROME_IDLE.
    if s.fullscreen && s.now < s.chrome_until {
        let ms = ((s.chrome_until - s.now) * 1000.0).ceil().clamp(16.0, 250.0) as u64;
        tx.wake_after(Duration::from_millis(ms));
    }

    if s.clip.is_playing()
        || s.scrub.is_some()
        || s.hover_seek.is_some()
        || !s.toasts.is_empty()
        || s.picking_file.load(Ordering::SeqCst)
    {
        tx.wake_after(Duration::from_millis(33));
    }
    changed
}

fn on_key(s: &mut Viewer, ev: &KeyEvent, tx: &mut Tx) -> bool {
    bump_chrome(s);
    if s.help_open {
        if matches!(ev.key, Key::Escape) {
            s.help_open = false;
            return true;
        }
        return false;
    }
    match &ev.key {
        Key::Text(t) if t == " " => {
            toggle_play(s, &["Space"]);
            true
        }
        Key::Left => {
            let d = if ev.mods.ctrl { -60.0 } else if ev.mods.alt { -3.0 } else { -10.0 };
            let keys: &[&str] = if ev.mods.ctrl {
                &["Ctrl", "←"]
            } else if ev.mods.alt {
                &["Alt", "←"]
            } else {
                &["←"]
            };
            seek_by(s, d, keys);
            true
        }
        Key::Right => {
            let d = if ev.mods.ctrl { 60.0 } else if ev.mods.alt { 3.0 } else { 10.0 };
            let keys: &[&str] = if ev.mods.ctrl {
                &["Ctrl", "→"]
            } else if ev.mods.alt {
                &["Alt", "→"]
            } else {
                &["→"]
            };
            seek_by(s, d, keys);
            true
        }
        Key::Up => {
            nudge_volume(s, 0.05, &["↑"]);
            true
        }
        Key::Down => {
            nudge_volume(s, -0.05, &["↓"]);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("m") => {
            toggle_mute(s, &["M"]);
            true
        }
        Key::Text(t) if t == "[" => {
            step_rate(s, -1, &["["]);
            true
        }
        Key::Text(t) if t == "]" => {
            step_rate(s, 1, &["]"]);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("f") => {
            toggle_fullscreen(s, tx, &["F"]);
            true
        }
        Key::Enter => {
            toggle_fullscreen(s, tx, &["Enter"]);
            true
        }
        Key::Escape if s.fullscreen => {
            s.fullscreen = false;
            toast(s, "Windowed", &["Esc"]);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("l") => {
            toggle_loop(s, &["L"]);
            true
        }
        Key::Text(t) if t.eq_ignore_ascii_case("o") => {
            start_system_open(s);
            true
        }
        Key::Text(t) if t == "?" || t.eq_ignore_ascii_case("h") => {
            s.help_open = true;
            s.menu_open = false;
            true
        }
        Key::Text(t) if t.len() == 1 && t.as_bytes()[0].is_ascii_digit() => {
            let pct = (t.as_bytes()[0] - b'0') * 10;
            jump_percent(s, pct, &[t.as_str()]);
            true
        }
        _ => false,
    }
}

fn track_submenu(s: &Viewer, kind: MediaTrackKind) -> MenuItem {
    let open = s.track_menu == Some(kind);
    let tracks = s.clip.tracks(kind);
    let current = s.clip.current_track(kind);
    let title = match kind {
        MediaTrackKind::Subtitle => "Subtitles",
        MediaTrackKind::Audio => "Audio track",
        MediaTrackKind::Video => "Video track",
    };
    let mut entries = Vec::new();
    if kind == MediaTrackKind::Subtitle {
        entries.push(MenuEntry::item(
            MenuItem::new("Off").checked(current.is_none()).on_select(|s: &mut Viewer| {
                s.clip.set_track(MediaTrackKind::Subtitle, None);
                s.track_menu = None;
                s.menu_open = false;
                toast(s, "Subtitles off", &[]);
            }),
        ));
    }
    if tracks.is_empty() {
        entries.push(MenuEntry::item(MenuItem::new("No tracks yet").disabled(true)));
    } else {
        for t in tracks {
            let index = t.index;
            let checked = current == Some(index);
            let label = t.label();
            entries.push(MenuEntry::item(MenuItem::new(label.clone()).checked(checked).on_select(move |s: &mut Viewer| {
                s.clip.set_track(kind, Some(index));
                s.track_menu = None;
                s.menu_open = false;
                toast(s, label.clone(), &[]);
            })));
        }
    }
    MenuItem::new(title)
        .submenu(entries, open)
        .on_select(move |s: &mut Viewer| {
            s.track_menu = if s.track_menu == Some(kind) { None } else { Some(kind) };
            bump_chrome(s);
        })
}

fn title_bar(s: &Viewer, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let trigger = IconButton::new(&MENU)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .label("Menu")
        .on_press(|s: &mut Viewer| {
            s.menu_open = !s.menu_open;
            s.speed_open = false;
            s.track_menu = None;
            bump_chrome(s);
        })
        .build(k);
    let menu = Menu::new("hamburger")
        .trigger(trigger)
        .open(s.menu_open)
        .placement(Placement::BottomStart)
        .on_close(|s: &mut Viewer| {
            s.menu_open = false;
            s.track_menu = None;
        })
        .item(MenuItem::new("Open…").shortcut("O").on_select(|s: &mut Viewer| start_system_open(s)))
        .item(MenuItem::new("Keyboard shortcuts").shortcut("?").on_select(|s: &mut Viewer| {
            s.menu_open = false;
            s.help_open = true;
        }))
        .separator()
        .group_label("Tracks")
        .item(track_submenu(s, MediaTrackKind::Subtitle))
        .item(track_submenu(s, MediaTrackKind::Audio))
        .item(track_submenu(s, MediaTrackKind::Video))
        .separator()
        .item(MenuItem::new("Quit").on_select_tx(|_: &mut Viewer, tx| tx.quit()));
    let menu_el = menu.build(k, layer);
    row()
        .key("title-bar")
        .w_full()
        .h(TITLE_H)
        .px(12.0)
        .items_center()
        .gap(10.0)
        .fill(c.layer_01.alpha(0.92))
        .window_drag()
        .on_hover_change(|s: &mut Viewer, inside, _| s.title_hover = inside)
        .child(menu_el)
        .child(text(display_name(&s.path)).key("title").font(k.ty.ui_medium).color(c.text_base).truncate().grow(1.0))
}

fn seek_bar(s: &Viewer, width: f32, k: &Kit) -> El {
    let c = &k.c;
    let duration = s.clip.duration().filter(|d| *d > 0.0);
    let progress = duration.map_or(0.0, |d| (s.scrub.unwrap_or_else(|| s.clip.time()) / d) as f32).clamp(0.0, 1.0);
    let travel = (width - KNOB).max(1.0);
    let to_time = move |x: f32| -> f64 {
        let u = ((x - KNOB / 2.0) / travel).clamp(0.0, 1.0) as f64;
        duration.map_or(0.0, |d| u * d)
    };
    let mut bar = el()
        .key("seek")
        .role("slider")
        .label("Seek")
        .size(width, SCRUB_H)
        .w_full()
        .on_hover_change(|s: &mut Viewer, inside, _| {
            if !inside {
                s.hover_seek = None;
            }
        })
        .on_pointer_move(move |s: &mut Viewer, p| {
            let at = to_time(p.x);
            // Skip tiny moves so hover doesn't rebuild the tree every pixel.
            let moved = s.hover_seek.map_or(true, |(x, t)| (x - p.x).abs() >= 2.0 || (t - at).abs() >= 0.05);
            if !moved {
                return;
            }
            s.hover_seek = Some((p.x, at));
            request_preview(s, at);
        })
        .on_drag(move |s: &mut Viewer, d: &mut Drag, tx| {
            match d.phase {
                Phase::Down | Phase::Move => {
                    let at = to_time(d.x);
                    s.scrub = Some(at);
                    s.hover_seek = Some((d.x, at));
                    s.clip.seek(at);
                    request_preview(s, at);
                    tx.instant();
                }
                Phase::Up => {
                    if let Some(at) = s.scrub.take() {
                        s.clip.seek(at);
                    }
                }
            }
            bump_chrome(s);
        })
        .child(el().key("track").size(width, BAR).absolute(0.0, (SCRUB_H - BAR) / 2.0).radius(BAR / 2.0).fill(c.border_muted))
        .child(
            el().key("fill")
                .size(KNOB / 2.0 + progress * travel, BAR)
                .absolute(0.0, (SCRUB_H - BAR) / 2.0)
                .radius(BAR / 2.0)
                .fill(c.text_base),
        )
        .child(
            el().key("knob")
                .size(KNOB, KNOB)
                .absolute(progress * travel, (SCRUB_H - KNOB) / 2.0)
                .radius(KNOB / 2.0)
                .fill(c.text_base),
        );
    if let Some((x, at)) = s.hover_seek {
        let line_x = x.clamp(0.0, width);
        bar = bar.child(el().key("hover-line").size(1.0, SCRUB_H).absolute(line_x, 0.0).fill(c.text_muted));
        let preview_x = (x - PREVIEW_W / 2.0).clamp(0.0, (width - PREVIEW_W).max(0.0));
        let thumb = if s.clip.is_audio() {
            el().key("preview-thumb").size(PREVIEW_W, 90.0).radius(6.0).fill(c.layer_02).center().child(text(format_time(at)).font(k.ty.small).color(c.text_base))
        } else {
            col()
                .key("preview-thumb")
                .w(PREVIEW_W)
                .gap(4.0)
                .child(el().key("frame").w(PREVIEW_W).h(90.0).radius(6.0).fill(c.layer_02).pixels_fit(&s.preview.pixels, SurfaceFit::Fit))
                .child(text(format_time(at)).font(k.ty.small).color(c.text_base))
        };
        bar = bar.child(el().key("preview").absolute(preview_x, -108.0).child(thumb));
    }
    bar
}

fn volume_slider(s: &Viewer, k: &Kit) -> El {
    let c = &k.c;
    let vol = if s.clip.muted() { 0.0 } else { s.clip.volume() };
    let travel = VOL_W - KNOB;
    el()
        .key("volume")
        .role("slider")
        .label("Volume")
        .size(VOL_W, SCRUB_H)
        .on_drag(move |s: &mut Viewer, d: &mut Drag, tx| {
            let u = ((d.x - KNOB / 2.0) / travel).clamp(0.0, 1.0);
            if matches!(d.phase, Phase::Down | Phase::Move) {
                set_volume(s, u, &[]);
                tx.instant();
            }
        })
        .child(el().key("track").size(VOL_W, BAR).absolute(0.0, (SCRUB_H - BAR) / 2.0).radius(BAR / 2.0).fill(c.border_muted))
        .child(
            el().key("fill")
                .size(KNOB / 2.0 + vol * travel, BAR)
                .absolute(0.0, (SCRUB_H - BAR) / 2.0)
                .radius(BAR / 2.0)
                .fill(c.text_base),
        )
        .child(
            el().key("knob")
                .size(KNOB, KNOB)
                .absolute(vol * travel, (SCRUB_H - KNOB) / 2.0)
                .radius(KNOB / 2.0)
                .fill(c.text_base),
        )
}

fn play_bar(s: &Viewer, width: f32, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let playing = s.clip.is_playing();
    let play = IconButton::new(if playing { &PAUSE } else { &PLAY })
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Large)
        .label(if playing { "Pause" } else { "Play" })
        .on_press(|s: &mut Viewer| toggle_play(s, &[]))
        .build(k);
    let mute = IconButton::new(if s.clip.muted() || s.clip.volume() <= 0.0 { &MUTE } else { &VOLUME })
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .label("Mute")
        .on_press(|s: &mut Viewer| toggle_mute(s, &[]))
        .build(k);
    let rate_label = format!("{:.2}×", s.clip.rate());
    let rate_btn = Button::new(&rate_label)
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut Viewer| {
            s.speed_open = !s.speed_open;
            s.menu_open = false;
            bump_chrome(s);
        })
        .build(k);
    let mut speed = Menu::new("speed").trigger(rate_btn).open(s.speed_open).placement(Placement::TopStart).on_close(|s: &mut Viewer| s.speed_open = false);
    for &rate in RATES {
        let checked = (s.clip.rate() - rate).abs() < 1e-3;
        speed = speed.item(MenuItem::new(format!("{rate:.2}×")).checked(checked).on_select(move |s: &mut Viewer| {
            s.speed_open = false;
            set_rate(s, rate, &[]);
        }));
    }
    let speed_el = speed.build(k, layer);
    let fs = IconButton::new(&FULLSCREEN)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .label("Fullscreen")
        .on_press_tx(|s: &mut Viewer, tx| toggle_fullscreen(s, tx, &[]))
        .build(k);
    let stamp = match s.clip.duration() {
        Some(d) => format!("{} / {}", format_time(s.scrub.unwrap_or_else(|| s.clip.time())), format_time(d)),
        None => format_time(s.clip.time()),
    };
    let seek_w = (width - 48.0).max(160.0);
    col()
        .key("play-bar")
        .w_full()
        .h(PLAY_H)
        .px(12.0)
        .py(8.0)
        .gap(6.0)
        .fill(c.layer_01.alpha(0.92))
        .on_hover_change(|s: &mut Viewer, inside, _| s.play_hover = inside)
        .child(seek_bar(s, seek_w, k))
        .child(
            row()
                .key("controls")
                .w_full()
                .items_center()
                .gap(8.0)
                .child(play)
                .child(text(stamp).font(k.ty.small).color(c.text_muted))
                .child(spacer())
                .child(speed_el)
                .child(mute)
                .child(volume_slider(s, k))
                .child(fs),
        )
}

fn stage(s: &Viewer, k: &Kit) -> El {
    let c = &k.c;
    if s.clip.is_audio() {
        return el()
            .key("audio-stage")
            .w_full()
            .grow(1.0)
            .center()
            .fill(c.bg_base)
            .on_press(|s: &mut Viewer| toggle_play(s, &[]))
            .on_double_press_tx(|s: &mut Viewer, tx| toggle_fullscreen(s, tx, &[]))
            .child(text("Audio").font(type_::TITLE).color(c.text_muted));
    }
    el()
        .key("video-stage")
        .w_full()
        .grow(1.0)
        .fill(c.bg_base)
        .child(
            el()
                .key("video")
                .w_full()
                .h_full()
                .video(&s.clip, SurfaceFit::Fit)
                .on_press(|s: &mut Viewer| toggle_play(s, &[]))
                // Each click still toggles play; the pair cancels out and this enters/exits fullscreen.
                .on_double_press_tx(|s: &mut Viewer, tx| toggle_fullscreen(s, tx, &[])),
        )
}

fn toasts(s: &Viewer, k: &Kit) -> El {
    // Sit below the title bar when chrome is up; flush to the top edge when it isn't.
    let top = if chrome_visible(s) { TITLE_H + 12.0 } else { 16.0 };
    let mut stack = col().key("toast-stack").gap(8.0).pin(None, Some(top), Some(16.0), None);
    for t in &s.toasts {
        let mut toast = Toast::new(format!("toast-{}", t.id), t.title.clone());
        if !t.keys.is_empty() {
            toast = toast.description(t.keys.join(" · "));
        }
        stack = stack.child(toast.build(k).w(220.0));
    }
    stack
}

fn help_keys(keys: &HelpKeys, k: &Kit) -> El {
    match keys {
        HelpKeys::Label(label) => text(*label).font(k.ty.ui).color(k.c.text_base),
        HelpKeys::Caps(alts) => {
            let mut caps = row().key("caps").gap(6.0).items_center();
            for (i, chord) in alts.iter().enumerate() {
                if i > 0 {
                    caps = caps.child(text("/").font(k.ty.small).color(k.c.text_faint));
                }
                caps = caps.child(Keybind::new(chord).build(k));
            }
            caps
        }
    }
}

fn help_dialog(s: &Viewer, k: &Kit, layer: &mut Vec<El>) {
    let mut list = col().key("help-list").w_full().gap(8.0);
    for (i, row_s) in HELP.iter().enumerate() {
        list = list.child(
            row()
                .key(format!("help-{i}"))
                .w_full()
                .gap(16.0)
                .items_center()
                .child(help_keys(&row_s.keys, k).w(200.0))
                .child(text(row_s.action).font(k.ty.ui).color(k.c.text_muted).grow(1.0)),
        );
    }
    // Fixed viewport height so the list actually scrolls (grow alone was sizing to content and clipping).
    let body = col()
        .key("help-body")
        .w_full()
        .h(360.0)
        .px(16.0)
        .pb(16.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(list);
    Dialog::new("help", "Keyboard shortcuts")
        .description("One binding table drives keys and this sheet.")
        .open(s.help_open)
        .width(520.0)
        .fit()
        .on_close(|s: &mut Viewer| s.help_open = false)
        .child(body)
        .build(k, layer);
}

fn status_line(s: &Viewer, k: &Kit) -> El {
    let label = match s.clip.state() {
        PlayerState::Loading => "Loading…".into(),
        PlayerState::Failed(e) => e,
        PlayerState::Ended => "Ended".into(),
        _ => String::new(),
    };
    if label.is_empty() {
        el().key("status").h(0.0)
    } else {
        text(label).key("status").font(k.ty.small).color(k.c.danger.fg).px(12.0).py(4.0)
    }
}

fn edge_strip(key: &str, top: bool) -> El {
    let strip = el()
        .key(key)
        .w_full()
        .h(EDGE)
        .on_hover_change(|s: &mut Viewer, inside, _| {
            s.edge_hover = inside;
            if inside {
                bump_chrome(s);
            }
        });
    if top {
        strip.pin(Some(0.0), Some(0.0), Some(0.0), None)
    } else {
        strip.pin(Some(0.0), None, Some(0.0), Some(0.0))
    }
}

fn view(s: &Viewer, cx: &mut Cx<Viewer>) -> El {
    let k = rune_theme_opencode::kit(cx.system_dark());
    let mut overlays = Vec::new();
    help_dialog(s, k, &mut overlays);
    if !s.toasts.is_empty() {
        overlays.push(toasts(s, k));
    }
    let show = chrome_visible(s);
    // Layout-pinned strips (not Y math against a possibly stale viewport) reveal chrome when
    // the pointer rests on a screen edge while the bars are hidden.
    if s.fullscreen && !show {
        overlays.push(edge_strip("edge-top", true));
        overlays.push(edge_strip("edge-bot", false));
    }
    let width = s.viewport.0;
    let mut body = col()
        .key("window")
        .w_full()
        .h_full()
        .fill(k.c.bg_base)
        .on_layout(|s: &mut Viewer, r| {
            let next = (r[2].max(1.0), r[3].max(1.0));
            if (s.viewport.0 - next.0).abs() > 0.5 || (s.viewport.1 - next.1).abs() > 0.5 {
                s.viewport = next;
            }
        })
        .on_pointer_move(|s: &mut Viewer, _| {
            // Any move in fullscreen re-shows chrome; idle timeout hides it again.
            if s.fullscreen {
                bump_chrome(s);
            }
        });
    if show {
        body = body.child(title_bar(s, k, &mut overlays));
    }
    body = body.child(stage(s, k));
    if show {
        body = body.child(play_bar(s, width, k, &mut overlays));
    }
    body = body.child(status_line(s, k));
    root(body, overlays)
}

fn main() {
    rune::run(AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Video player extra", WIN, state, view)
    });
}
