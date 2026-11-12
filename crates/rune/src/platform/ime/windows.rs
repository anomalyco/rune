//! Windows: what winit's IMM32 handling leaves out, from a subclass of the window's procedure.
//! - **Clauses:** a composition's clause boundaries (`GCS_COMPCLAUSE`) and which clause is being
//!   converted (`GCS_COMPATTR`), read as each `WM_IME_COMPOSITION` arrives, before winit turns
//!   it into a preedit with only the target range.
//! - **Reconversion and document feed:** `WM_IME_REQUEST`'s `IMR_RECONVERTSTRING` and
//!   `IMR_DOCUMENTFEED` answered with the focused field's paragraph (the runtime publishes it
//!   each frame), and `IMR_CONFIRMRECONVERTSTRING`'s range kept for the runtime to select, so the
//!   composition the input method starts replaces it.
//!
//! The input method sends these synchronously from inside the message loop; the runtime isn't
//! reachable there, so each answer comes from what it published, and each finding waits for it
//! in a cell, taken in order before the preedit it belongs to.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::Ime::{
    ATTR_TARGET_CONVERTED, ATTR_TARGET_NOTCONVERTED, GCS_COMPATTR, GCS_COMPCLAUSE, GCS_COMPSTR, IMR_CONFIRMRECONVERTSTRING,
    IMR_DOCUMENTFEED, IMR_RECONVERTSTRING, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, RECONVERTSTRING,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_IME_COMPOSITION, WM_IME_REQUEST, WM_NCDESTROY};

use crate::edit::{Clause, ImeText};

const SUBCLASS: usize = 0x5255_4e49; // "RUNI"

/// One window's published text and the findings waiting for the runtime.
#[derive(Default)]
struct Window {
    text: Option<ImeText>,
    /// The last composition string read and its clauses.
    clauses: Option<(String, Vec<Clause>)>,
    /// A reconversion's range of the field's text, in bytes.
    select: Option<Range<usize>>,
}

thread_local! {
    static WINDOWS: RefCell<HashMap<usize, Window>> = RefCell::new(HashMap::new());
}

fn key(window: &winit::window::Window) -> Option<(HWND, usize)> {
    let hwnd = crate::platform::win32::hwnd(window)?;
    Some((hwnd, hwnd as usize))
}

/// Publishes the focused field's text for the input method, subclassing the window the first
/// time.
pub(crate) fn sync(window: &winit::window::Window, text: Option<ImeText>) {
    let Some((hwnd, k)) = key(window) else { return };
    WINDOWS.with_borrow_mut(|all| {
        if !all.contains_key(&k) {
            // SAFETY: this thread's live window; the subclass removes its entry when the window goes.
            if unsafe { SetWindowSubclass(hwnd, Some(procedure), SUBCLASS, 0) } == 0 { return; }
        }
        all.entry(k).or_default().text = text;
    });
}

/// The clauses Windows gave composition `preedit`, if it's the one last read.
pub(crate) fn take_clauses(window: &winit::window::Window, preedit: &str) -> Vec<Clause> {
    let Some((_, k)) = key(window) else { return Vec::new() };
    WINDOWS.with_borrow_mut(|all| match all.get_mut(&k).and_then(|w| w.clauses.take()) {
        Some((text, clauses)) if text == preedit => clauses,
        _ => Vec::new(),
    })
}

/// A reconversion's range of the field's text, once.
pub(crate) fn take_select(window: &winit::window::Window) -> Option<Range<usize>> {
    let (_, k) = key(window)?;
    WINDOWS.with_borrow_mut(|all| all.get_mut(&k).and_then(|w| w.select.take()))
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
    match message {
        WM_IME_COMPOSITION if lparam as u32 & GCS_COMPSTR != 0 => {
            // SAFETY: this window's own input context, released at once.
            let clauses = unsafe { read_clauses(hwnd) };
            WINDOWS.with_borrow_mut(|all| { if let Some(w) = all.get_mut(&(hwnd as usize)) { w.clauses = clauses; } });
        }
        WM_IME_REQUEST if matches!(wparam as u32, IMR_RECONVERTSTRING | IMR_DOCUMENTFEED) => {
            let feed = wparam as u32 == IMR_DOCUMENTFEED;
            let answer = WINDOWS.with_borrow(|all| all.get(&(hwnd as usize)).and_then(|w| w.text.clone()));
            if let Some(text) = answer {
                // SAFETY: `lparam` is null (asking the size) or the input method's buffer of that size.
                return unsafe { reconvert_string(&text, feed, lparam as *mut RECONVERTSTRING) };
            }
        }
        WM_IME_REQUEST if wparam as u32 == IMR_CONFIRMRECONVERTSTRING && lparam != 0 => {
            WINDOWS.with_borrow_mut(|all| {
                let Some(w) = all.get_mut(&(hwnd as usize)) else { return };
                let Some(text) = &w.text else { return };
                // SAFETY: the input method's RECONVERTSTRING, as this window wrote it.
                let rs = unsafe { &*(lparam as *const RECONVERTSTRING) };
                let units: Vec<u16> = text.text.encode_utf16().collect();
                let from = (rs.dwCompStrOffset / 2) as usize;
                let to = from + rs.dwCompStrLen as usize;
                if to <= units.len() {
                    let byte = |u: usize| String::from_utf16_lossy(&units[..u]).len();
                    w.select = Some(text.start + byte(from)..text.start + byte(to));
                }
            });
            return 1;
        }
        WM_NCDESTROY => { WINDOWS.with_borrow_mut(|all| { all.remove(&(hwnd as usize)); }); }
        _ => {}
    }
    // SAFETY: the next procedure in this window's chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// The composition string's clauses: byte ranges from `GCS_COMPCLAUSE`'s UTF-16 boundaries,
/// the target from `GCS_COMPATTR`.
unsafe fn read_clauses(hwnd: HWND) -> Option<(String, Vec<Clause>)> {
    // SAFETY: the caller's window; the context is released before returning.
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_null() { return None; }
        let read = |what: u32| -> Vec<u8> {
            let n = ImmGetCompositionStringW(himc, what, std::ptr::null_mut(), 0);
            if n <= 0 { return Vec::new(); }
            let mut buf = vec![0u8; n as usize];
            let got = ImmGetCompositionStringW(himc, what, buf.as_mut_ptr().cast(), n as u32);
            buf.truncate(got.max(0) as usize);
            buf
        };
        let (comp, clause, attr) = (read(GCS_COMPSTR), read(GCS_COMPCLAUSE), read(GCS_COMPATTR));
        ImmReleaseContext(hwnd, himc);
        let units: Vec<u16> = comp.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        let text = String::from_utf16(&units).ok()?;
        let bounds: Vec<usize> = clause.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).collect();
        let byte = |u: usize| String::from_utf16_lossy(&units[..u.min(units.len())]).len();
        let clauses = bounds.windows(2).filter(|w| w[0] < w[1] && w[1] <= units.len()).map(|w| {
            let c = Clause::new(byte(w[0])..byte(w[1]));
            match attr.get(w[0]).map(|&a| a as u32) {
                Some(ATTR_TARGET_CONVERTED | ATTR_TARGET_NOTCONVERTED) => c.target(),
                _ => c,
            }
        }).collect();
        Some((text, clauses))
    }
}

/// Answers `IMR_RECONVERTSTRING` (the selection, else the word at the caret, to convert again)
/// or `IMR_DOCUMENTFEED` (the paragraph around the caret, so conversion reads its context): the
/// size when `rs` is null, else the struct and the paragraph written there.
unsafe fn reconvert_string(text: &ImeText, feed: bool, rs: *mut RECONVERTSTRING) -> LRESULT {
    let units: Vec<u16> = text.text.encode_utf16().collect();
    let head = std::mem::size_of::<RECONVERTSTRING>();
    let size = head + (units.len() + 1) * 2;
    if rs.is_null() { return size as LRESULT; }
    let unit = |byte: usize| text.text[..byte.min(text.text.len())].encode_utf16().count() as u32;
    let (a, b) = match (feed, &text.selection) {
        (true, s) => (unit(s.end), unit(s.end)),
        (false, s) if !s.is_empty() => (unit(s.start), unit(s.end)),
        (false, s) => {
            let w = text.word_at(s.start);
            (unit(w.start), unit(w.end))
        }
    };
    // SAFETY: the input method's buffer holds `size` bytes: the struct, then the string.
    unsafe {
        if ((*rs).dwSize as usize) < size { return 0; }
        *rs = RECONVERTSTRING {
            dwSize: size as u32,
            dwVersion: 0,
            dwStrLen: units.len() as u32,
            dwStrOffset: head as u32,
            dwCompStrLen: b - a,
            dwCompStrOffset: a * 2,
            dwTargetStrLen: b - a,
            dwTargetStrOffset: a * 2,
        };
        let out = rs.cast::<u8>().add(head).cast::<u16>();
        std::ptr::copy_nonoverlapping(units.as_ptr(), out, units.len());
        *out.add(units.len()) = 0;
    }
    size as LRESULT
}
