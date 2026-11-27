//! System configuration.
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Mutex, OnceLock};

type ButtonLayout = Option<(String, String)>;

/// The system reads started before a window exists, each on a thread of its own. The first frame
/// takes their answers; a later frame reads again, as it always did.
struct Early {
    dark: Mutex<Option<Receiver<bool>>>,
    button_layout: Mutex<Option<Receiver<ButtonLayout>>>,
    #[cfg(all(not(feature = "crossfont"), feature = "ab_glyph"))]
    title_font: Mutex<Option<Receiver<crate::title::TitleFont>>>,
}

static EARLY: OnceLock<Early> = OnceLock::new();

/// Starts the reads a frame's first drawing needs (the colour scheme and the button layout: a
/// `dbus-send` process each, with 100 ms for the portal's reply; plus the `ab_glyph` titlebar
/// font's `gsettings` and `fc-match` lookups when compiled in) on threads of their own, once per
/// process. Call it when a client-side frame is certain and before the window's first configure:
/// the frame is made in that configure, on the event loop's thread, which otherwise starts those
/// processes one after the other and waits for each.
pub fn read_system_config_early() {
    EARLY.get_or_init(|| Early {
        dark: Mutex::new(start(read_prefer_dark)),
        button_layout: Mutex::new(start(read_button_layout_config)),
        #[cfg(all(not(feature = "crossfont"), feature = "ab_glyph"))]
        title_font: Mutex::new(start(crate::title::read_title_font)),
    });
}

/// Runs `read` on a thread of its own. `None` when the thread could not be made.
pub(crate) fn start<T: Send + 'static>(read: fn() -> T) -> Option<Receiver<T>> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("sctk-adwaita-config".into())
        .spawn(move || {
            let _ = tx.send(read());
        })
        .ok()?;
    Some(rx)
}

/// The answer of a read started early, waited for if it is still running. Once: the next call
/// gets `None` and reads the system as it is then.
pub(crate) fn take<T>(slot: &Mutex<Option<Receiver<T>>>) -> Option<T> {
    let rx = slot.lock().ok()?.take()?;
    rx.recv().ok()
}

/// Query system to see if dark theming should be preferred.
pub(crate) fn prefer_dark() -> bool {
    EARLY.get().and_then(|early| take(&early.dark)).unwrap_or_else(read_prefer_dark)
}

/// Query system configuration for buttons layout.
pub(crate) fn get_button_layout_config() -> ButtonLayout {
    EARLY
        .get()
        .and_then(|early| take(&early.button_layout))
        .unwrap_or_else(read_button_layout_config)
}

/// Query the titlebar font preference and mapped system font file, reusing the early read once.
#[cfg(all(not(feature = "crossfont"), feature = "ab_glyph"))]
pub(crate) fn get_title_font() -> crate::title::TitleFont {
    EARLY
        .get()
        .and_then(|early| take(&early.title_font))
        .unwrap_or_else(crate::title::read_title_font)
}

fn read_prefer_dark() -> bool {
    // outputs something like: `variant       variant          uint32 1`
    let stdout = Command::new("dbus-send")
        .arg("--reply-timeout=100")
        .arg("--print-reply=literal")
        .arg("--dest=org.freedesktop.portal.Desktop")
        .arg("/org/freedesktop/portal/desktop")
        .arg("org.freedesktop.portal.Settings.Read")
        .arg("string:org.freedesktop.appearance")
        .arg("string:color-scheme")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok());

    if matches!(stdout, Some(ref s) if s.is_empty()) {
        log::error!("XDG Settings Portal did not return response in time: timeout: 100ms, key: color-scheme");
    }

    matches!(stdout, Some(s) if s.trim().ends_with("uint32 1"))
}

/// Should be updated to use standard xdg-desktop-portal specs once available
/// https://github.com/flatpak/xdg-desktop-portal/pull/996
fn read_button_layout_config() -> ButtonLayout {
    let config_string = Command::new("dbus-send")
        .arg("--reply-timeout=100")
        .arg("--print-reply=literal")
        .arg("--dest=org.freedesktop.portal.Desktop")
        .arg("/org/freedesktop/portal/desktop")
        .arg("org.freedesktop.portal.Settings.Read")
        .arg("string:org.gnome.desktop.wm.preferences")
        .arg("string:button-layout")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())?;

    let sides_split: Vec<_> = config_string
        // Taking last word
        .rsplit(' ')
        .next()?
        // Split by left/right side
        .split(':')
        // Only two sides
        .take(2)
        .collect();

    match sides_split.as_slice() {
        [left, right] => Some((left.to_string(), right.to_string())),
        _ => None,
    }
}
