//! A physical shortcut editor. The app owns registration and can reject a proposed change.
use std::rc::Rc;
use rune::{El, Mods, PhysicalInput, PhysicalKeyCode, Shortcut, ShortcutError, Tx, col, row, text};
use crate::{Keybind, Kit};

/// A recorder's value and its capture, as one explicit machine (Desktop's keybind capture,
/// `app/src/settings/keybinds/machine.ts`: idle or capturing). Only a capture has an error, and
/// only an error has the chord it refused.
#[derive(Clone, Debug, Default)]
pub struct ShortcutRecorderState {
    shortcut: Option<Shortcut>,
    phase: Recording,
    /// The modifiers held now, which a capture shows as it waits for a key.
    held: Mods,
    unregistered: bool,
}

#[derive(Clone, Debug, Default)]
enum Recording {
    #[default]
    Idle,
    /// Waiting for a chord; `refusal` is the last one refused, until a new chord starts, and
    /// `finishing` is the key that ended or refused it, whose repeats and release are swallowed.
    Capturing { refusal: Option<Refusal>, finishing: Option<PhysicalKeyCode> },
    /// The capture is over, and the key that ended it still swallows its repeats and release.
    Swallowing { key: PhysicalKeyCode },
}

impl Recording {
    fn finishing(&self) -> Option<PhysicalKeyCode> {
        match self {
            Recording::Capturing { finishing, .. } => *finishing,
            Recording::Swallowing { key } => Some(*key),
            Recording::Idle => None,
        }
    }
    fn finishing_mut(&mut self) -> Option<&mut Option<PhysicalKeyCode>> {
        match self {
            Recording::Capturing { finishing, .. } => Some(finishing),
            Recording::Idle | Recording::Swallowing { .. } => None,
        }
    }
    fn refusal_mut(&mut self) -> Option<&mut Option<Refusal>> {
        match self {
            Recording::Capturing { refusal, .. } => Some(refusal),
            Recording::Idle | Recording::Swallowing { .. } => None,
        }
    }
}
/// Why a chord was refused, and the chord (None for a refused clear or a key that can't be one).
#[derive(Clone, Debug)]
struct Refusal {
    error: ShortcutError,
    chord: Option<Shortcut>,
}

impl ShortcutRecorderState {
    pub fn new(shortcut: Option<Shortcut>) -> Self { Self { shortcut, ..Self::default() } }
    pub fn shortcut(&self) -> Option<&Shortcut> { self.shortcut.as_ref() }
    pub fn is_recording(&self) -> bool { matches!(self.phase, Recording::Capturing { .. }) }
    fn refusal(&self) -> Option<&Refusal> {
        match &self.phase {
            Recording::Capturing { refusal, .. } => refusal.as_ref(),
            Recording::Idle | Recording::Swallowing { .. } => None,
        }
    }
    pub fn error(&self) -> Option<&ShortcutError> { self.refusal().map(|r| &r.error) }
    /// The chord `on_change` just refused, until another chord starts.
    pub fn refused(&self) -> Option<&Shortcut> { self.refusal().and_then(|r| r.chord.as_ref()) }
    /// The shortcut isn't registered (it failed at launch, say): recording the same chord
    /// proposes it through `on_change` again instead of being a no-op, until a change succeeds.
    pub fn mark_unregistered(&mut self) { self.unregistered = true; }
    pub fn begin(&mut self) { self.phase = Recording::Capturing { refusal: None, finishing: None }; }
    /// Ends the capture, keeping the side of a key that still swallows its repeats and release.
    pub fn cancel(&mut self) {
        self.phase = match self.phase.finishing() {
            Some(key) => Recording::Swallowing { key },
            None => Recording::Idle,
        };
        self.held = Mods::default();
    }
    pub fn keycaps(&self) -> Vec<String> {
        if let Some(refused) = self.refused() { refused.keycaps() }
        else if self.is_recording() { rune::modifier_keycaps(self.held) }
        else { self.shortcut.as_ref().map(Shortcut::keycaps).unwrap_or_default() }
    }

    fn input(&mut self, event: &PhysicalInput) -> Outcome {
        match *event {
            PhysicalInput::FocusLost => {
                let active = !matches!(self.phase, Recording::Idle);
                self.cancel();
                self.phase = Recording::Idle;
                if active { Outcome::Consumed } else { Outcome::Pass }
            }
            PhysicalInput::Modifiers(mods) => {
                self.hold(mods);
                if self.is_recording() { Outcome::Consumed } else { Outcome::Pass }
            }
            PhysicalInput::Key { key, mods, pressed, repeat } => {
                self.hold(mods);
                if self.phase.finishing() == Some(key) {
                    if !pressed {
                        match self.phase.finishing_mut() {
                            Some(finishing) => *finishing = None,
                            None => self.phase = Recording::Idle,
                        }
                    }
                    return Outcome::Consumed;
                }
                if !self.is_recording() { return Outcome::Pass; }
                if !pressed || repeat || is_modifier(key) { return Outcome::Consumed; }
                if let Some(finishing) = self.phase.finishing_mut() { *finishing = Some(key); }
                if key == PhysicalKeyCode::Escape { self.cancel(); return Outcome::Consumed; }
                if key == PhysicalKeyCode::Backspace { return Outcome::Change(None); }
                match Shortcut::from_physical_key(key, mods) {
                    Ok(shortcut) => Outcome::Change(Some(shortcut)),
                    Err(error) => { self.refuse(error, None); Outcome::Consumed }
                }
            }
        }
    }

    /// Pressing another modifier starts a new chord: the refused one and its error go.
    fn hold(&mut self, mods: Mods) {
        let held = self.held;
        if (mods.ctrl && !held.ctrl) || (mods.alt && !held.alt) || (mods.shift && !held.shift)
            || (mods.cmd && !held.cmd) || (mods.logo && !held.logo) {
            if let Some(refusal) = self.phase.refusal_mut() { *refusal = None; }
        }
        self.held = mods;
    }

    /// `chord` was refused with `error`: the capture goes on, showing both.
    fn refuse(&mut self, error: ShortcutError, chord: Option<Shortcut>) {
        if let Some(refusal) = self.phase.refusal_mut() { *refusal = Some(Refusal { error, chord }); }
    }
}

enum Outcome { Pass, Consumed, Change(Option<Shortcut>) }
fn is_modifier(key: PhysicalKeyCode) -> bool {
    use PhysicalKeyCode::*;
    matches!(key, ShiftLeft | ShiftRight | ControlLeft | ControlRight | AltLeft | AltRight | SuperLeft | SuperRight | Fn | FnLock)
}
type Access<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut ShortcutRecorderState>;
type Change<S> = Rc<dyn Fn(&mut S, Option<&Shortcut>, &mut Tx) -> Result<(), ShortcutError>>;
type Message = Box<dyn Fn(&ShortcutError, Option<&Shortcut>) -> String>;

/// The recorder's default error text, for `error_message` closures to fall back on:
/// "⇧⌘K is already in use.", or "Add ⌘, ⌃ or ⌥." for a chord without a modifier. The refused
/// chord is None when clearing was refused.
pub fn shortcut_error_message(error: &ShortcutError, shortcut: Option<&Shortcut>) -> String {
    let Some(shortcut) = shortcut else {
        return if matches!(error, ShortcutError::InvalidAccelerator(_)) { "That key can't be part of a shortcut.".into() }
            else { "The shortcut couldn't be cleared.".into() };
    };
    let chord = shortcut.keycaps().join(if cfg!(target_os = "macos") { "" } else { "+" });
    match error {
        ShortcutError::InvalidAccelerator(_) if shortcut.require_modifier().is_err() =>
            if cfg!(target_os = "macos") { "Add ⌘, ⌃ or ⌥.".into() } else { "Add Ctrl, Alt or Super.".into() },
        ShortcutError::Conflict => format!("{chord} is already in use."),
        ShortcutError::Unsupported => "This desktop doesn't support global shortcuts.".into(),
        ShortcutError::PermissionDenied => format!("Permission to use {chord} was declined."),
        _ => format!("{chord} couldn't be registered."),
    }
}

/// Click (or activate with Enter/Space) to record. Escape cancels; Backspace clears.
/// `on_change` must register the proposed binding before releasing the previous registration:
/// returning an error keeps the old value and shows the refused chord with a message
/// (`error_message` words it). An unchanged chord is a no-op unless the state was marked
/// unregistered (`ShortcutRecorderState::mark_unregistered`). A chord without ⌘, ⌃ or ⌥ (Ctrl,
/// Alt or Super elsewhere) other than F1–F24 is refused before `on_change` unless
/// `allow_unmodified`.
pub struct ShortcutRecorder<S> {
    key: String,
    state: ShortcutRecorderState,
    access: Access<S>,
    change: Option<Change<S>>,
    message: Message,
    width: f32,
    disabled: bool,
    unmodified: bool,
    label: Option<String>,
}
impl<S: 'static> ShortcutRecorder<S> {
    pub fn new(key: impl Into<String>, state: &ShortcutRecorderState,
        access: impl for<'a> Fn(&'a mut S) -> &'a mut ShortcutRecorderState + 'static) -> Self {
        Self { key: key.into(), state: state.clone(), access: Rc::new(access), change: None, message: Box::new(shortcut_error_message), width: 240.0, disabled: false, unmodified: false, label: None }
    }
    /// The accessible name: what the shortcut does ("Show window"). Assistive technology then
    /// reads the recorded keys as its value. Without it the keys (or "Record shortcut") name it.
    pub fn label(mut self, l: impl Into<String>) -> Self { self.label = Some(l.into()); self }
    pub fn on_change(mut self, change: impl Fn(&mut S, Option<&Shortcut>, &mut Tx) -> Result<(), ShortcutError> + 'static) -> Self {
        self.change = Some(Rc::new(change)); self
    }
    /// Words the error shown under the recorder, given the refused chord (None when clearing
    /// was refused). The default, `shortcut_error_message`, is sentence-case and names the
    /// chord: "⌃J is already in use."
    pub fn error_message(mut self, message: impl Fn(&ShortcutError, Option<&Shortcut>) -> String + 'static) -> Self {
        self.message = Box::new(message); self
    }
    /// Propose chords without a modifier too, such as Space or ⇧A. Registering one globally
    /// also needs `App::allow_unmodified_shortcuts`.
    pub fn allow_unmodified(mut self) -> Self { self.unmodified = true; self }
    pub fn width(mut self, width: f32) -> Self { self.width = width; self }
    pub fn disabled(mut self, disabled: bool) -> Self { self.disabled = disabled; self }
    pub fn build(self, k: &Kit) -> El {
        let labels = self.state.keycaps();
        let placeholder = if self.state.is_recording() { "Press shortcut" } else { "Record shortcut" };
        let border = if self.state.error().is_some() { k.c.danger.border }
            else if self.state.is_recording() { k.c.border_focus } else { k.c.border_base };
        let keys = if labels.is_empty() { placeholder.to_owned() } else { labels.join(" ") };
        let mut control = row().key(&self.key).role("button")
            .w(self.width).h(32.0).px(8.0).center_y().radius(k.radius.md)
            .fill(k.c.bg_base).border(1.0, border).focusable(!self.disabled).disabled(self.disabled)
            .focus_ring(2.0, 2.0, k.c.border_focus);
        control = match self.label {
            Some(name) => control.label(name).a11y_value(keys),
            None => control.label(keys),
        };
        if self.state.is_recording() {
            control = control.description("Press a shortcut. Escape cancels; Backspace clears.");
        }
        control = control.child((if labels.is_empty() { text(placeholder).font(k.ty.ui_compact).color(k.c.text_faint) }
            else { Keybind::new(&labels).build(k) }).a11y_hidden());
        if !self.disabled {
            let access = self.access.clone();
            control = control.on_press(move |s: &mut S| access(s).begin());
        }
        {
            let access = self.access;
            let change = self.change;
            let (disabled, unmodified) = (self.disabled, self.unmodified);
            control = control.on_physical_input(move |s: &mut S, event, tx| {
                if disabled && !matches!(event, PhysicalInput::FocusLost) { return false; }
                match access(s).input(event) {
                    Outcome::Pass => false,
                    Outcome::Consumed => true,
                    Outcome::Change(next) => {
                        let bare = next.as_ref().filter(|_| !unmodified).map(Shortcut::require_modifier);
                        let result = if let Some(Err(error)) = bare { Err(error) }
                            else if access(s).shortcut == next && !access(s).unregistered { Ok(()) }
                            else { change.as_ref().map_or(Ok(()), |f| f(s, next.as_ref(), tx)) };
                        let state = access(s);
                        match result {
                            Ok(()) => { state.shortcut = next; state.unregistered = false; state.cancel(); }
                            Err(error) => state.refuse(error, next),
                        }
                        true
                    }
                }
            });
        }
        let mut root = col().key(format!("{}-field", self.key)).gap(4.0).child(control);
        if let Some(error) = self.state.error() {
            root = root.child(text((self.message)(error, self.state.refused())).key("error").role("alert")
                .font(k.ty.small_medium).color(k.c.danger.fg));
        }
        root
    }
}
