//! macOS: through the vendored winit's `NSTextInputClient` (`platform::macos::set_ime_text`
//! and the rest), which reads `setMarkedText:`'s clause attributes and answers `selectedRange`
//! and `attributedSubstringForProposedRange:` from the text published here. Also hooks
//! `WinitView`'s `keyDown:` and `insertText:replacementRange:` so text an input method inserts
//! directly without marked text (such as full-width CJK punctuation `，` for `,`) arrives as
//! `Ime::Commit` instead of falling through as the raw ASCII key.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::{ClassType, msg_send, runtime::{AnyClass, AnyObject, Bool, Imp, Sel}, sel};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSAttributedString, NSRange, NSString};
use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{DirectCommit, KeyDown};
use crate::edit::{Clause, ImeText};

type KeyDownImp = unsafe extern "C" fn(*mut AnyObject, Sel, *mut NSEvent);
type InsertTextImp = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, NSRange);

static HOOKED: AtomicBool = AtomicBool::new(false);
static ORIG_KEY_DOWN: OnceLock<usize> = OnceLock::new();
static ORIG_INSERT_TEXT: OnceLock<usize> = OnceLock::new();

thread_local! {
    /// Where the published text starts in its field, in bytes.
    static START: Cell<usize> = const { Cell::new(0) };
    static DIRECT: RefCell<DirectCommit> = const { RefCell::new(DirectCommit::new()) };
    /// Views (`ns_view` pointer) that currently have a non-`None` `OptionAsAlt` mode.
    static OPTION_AS_ALT: RefCell<HashMap<usize, crate::OptionAsAlt>> = RefCell::new(HashMap::new());
}

struct KeyDownGuard;

impl Drop for KeyDownGuard {
    fn drop(&mut self) {
        DIRECT.with_borrow_mut(DirectCommit::end);
    }
}

fn view_ptr(window: &winit::window::Window) -> Option<usize> {
    let Ok(handle) = window.window_handle() else { return None };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
    Some(h.ns_view.as_ptr() as usize)
}

fn install() {
    if HOOKED.load(Ordering::Acquire) { return; }
    let Some(class) = AnyClass::get(c"WinitView") else { return };
    let (Some(kd), Some(it)) = (
        class.instance_method(sel!(keyDown:)),
        class.instance_method(sel!(insertText:replacementRange:)),
    ) else { return };
    if HOOKED.swap(true, Ordering::AcqRel) { return; }
    // SAFETY: `WinitView` directly defines both selectors with these exact Objective-C ABIs,
    // and both replacements forward to the original IMPs saved here.
    unsafe {
        let orig_kd = kd.set_implementation(std::mem::transmute::<KeyDownImp, Imp>(key_down));
        ORIG_KEY_DOWN.set(orig_kd as usize).ok();
        let orig_it = it.set_implementation(std::mem::transmute::<InsertTextImp, Imp>(insert_text));
        ORIG_INSERT_TEXT.set(orig_it as usize).ok();
    }
}

fn extract_string(string: *mut AnyObject) -> Option<String> {
    if string.is_null() { return None; }
    // SAFETY: AppKit passes an `NSString` or `NSAttributedString` to `insertText:replacementRange:`.
    unsafe {
        let is_attributed: Bool = msg_send![string, isKindOfClass: NSAttributedString::class()];
        if is_attributed.as_bool() {
            let attr = &*(string as *const NSAttributedString);
            return Some(attr.string().to_string());
        }
        let is_string: Bool = msg_send![string, isKindOfClass: NSString::class()];
        if is_string.as_bool() {
            let s = &*(string as *const NSString);
            return Some(s.to_string());
        }
    }
    None
}

unsafe extern "C" fn key_down(this: *mut AnyObject, sel: Sel, event: *mut NSEvent) {
    let Some(&orig) = ORIG_KEY_DOWN.get() else { return };
    let view = this as usize;
    let key_down = (!event.is_null()).then(|| {
        // SAFETY: `event` is a non-null `NSEvent` passed by AppKit on the main thread.
        let ev = unsafe { &*event };
        if ev.r#type() != NSEventType::KeyDown { return None; }
        let flags = ev.modifierFlags();
        let alt = flags.contains(NSEventModifierFlags::Option);
        let mode = OPTION_AS_ALT.with_borrow(|views| {
            views.get(&view).copied().unwrap_or(crate::OptionAsAlt::None)
        });
        let option_as_alt = super::option_as_alt_matches(mode, alt, flags.bits());
        Some(KeyDown {
            view,
            chars: ev.characters().map(|s| s.to_string()),
            chars_ignoring_mods: ev.charactersIgnoringModifiers().map(|s| s.to_string()),
            ctrl: flags.contains(NSEventModifierFlags::Control),
            cmd: flags.contains(NSEventModifierFlags::Command),
            alt,
            option_as_alt,
        })
    }).flatten();
    let _guard = key_down.map(|kd| {
        DIRECT.with_borrow_mut(|d| d.begin(kd));
        KeyDownGuard
    });
    // SAFETY: `orig` is the original `-[WinitView keyDown:]` IMP with the exact same ABI.
    let original: KeyDownImp = unsafe { std::mem::transmute(orig) };
    // SAFETY: forwards the exact `this`, `sel`, and `event` received from AppKit.
    unsafe { original(this, sel, event) };
}

unsafe extern "C" fn insert_text(
    this: *mut AnyObject,
    sel: Sel,
    string: *mut AnyObject,
    replacement_range: NSRange,
) {
    let Some(&orig) = ORIG_INSERT_TEXT.get() else { return };
    // Check marked text before winit's `insertText:` runs.
    // SAFETY: `this` is a live `WinitView` on the main thread, implementing `NSTextInputClient`.
    let has_marked_text: bool = unsafe { msg_send![this, hasMarkedText] };
    let inserted = (!has_marked_text).then(|| extract_string(string)).flatten();
    // SAFETY: `orig` is the original `-[WinitView insertText:replacementRange:]` IMP.
    let original: InsertTextImp = unsafe { std::mem::transmute(orig) };
    // SAFETY: forwards the exact arguments received from AppKit.
    unsafe { original(this, sel, string, replacement_range) };
    if let Some(text) = inserted {
        DIRECT.with_borrow_mut(|d| d.record_insert(this as usize, false, &text));
    }
}

fn from_winit_option_as_alt(opt: OptionAsAlt) -> crate::OptionAsAlt {
    match opt {
        OptionAsAlt::None => crate::OptionAsAlt::None,
        OptionAsAlt::OnlyLeft => crate::OptionAsAlt::Left,
        OptionAsAlt::OnlyRight => crate::OptionAsAlt::Right,
        OptionAsAlt::Both => crate::OptionAsAlt::Both,
    }
}

pub(crate) fn sync(window: &winit::window::Window, text: Option<ImeText>) {
    install();
    START.set(text.as_ref().map_or(0, |t| t.start));
    if let Some(view) = view_ptr(window) {
        let mode = from_winit_option_as_alt(window.option_as_alt());
        OPTION_AS_ALT.with_borrow_mut(|views| {
            if mode == crate::OptionAsAlt::None {
                views.remove(&view);
            } else {
                views.insert(view, mode);
            }
        });
    }
    winit::platform::macos::set_ime_text(text.map(|t| (t.text.clone(), t.selection.clone())));
}

pub(crate) fn take_clauses(_window: &winit::window::Window, preedit: &str) -> Vec<Clause> {
    winit::platform::macos::take_ime_clauses(preedit).into_iter().map(|(range, target)| {
        let clause = Clause::new(range);
        if target { clause.target() } else { clause }
    }).collect()
}

pub(crate) fn take_select(_window: &winit::window::Window) -> Option<Range<usize>> {
    let start = START.get();
    winit::platform::macos::take_ime_replacement().map(|r| start + r.start..start + r.end)
}

pub(crate) fn take_direct_commit(window: &winit::window::Window) -> Option<String> {
    let view = view_ptr(window)?;
    DIRECT.with_borrow_mut(|d| d.take(view))
}
