//! A secret's wipe on Windows, with the clipboard open: while Rune has it open no other app can
//! write to it, so the check that it still holds Rune's write and the wipe are one step.
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use zeroize::Zeroizing;
use super::{Mark, Wipe};

const CF_UNICODETEXT: u32 = 13;

pub(super) fn clear_if(mark: &Mark) -> Wipe {
    unsafe {
        // As arboard does: another app may have it open for a moment.
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(core::ptr::null_mut()) != 0 { opened = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !opened { return Wipe::Failed; }
        let wipe = if Some(GetClipboardSequenceNumber() as u64) != mark.revision {
            Wipe::NotOurs
        } else {
            match text() {
                Err(()) => Wipe::Failed,
                Ok(Some(text)) if mark.text_is(&text) => if EmptyClipboard() != 0 { Wipe::Done } else { Wipe::Failed },
                Ok(_) => Wipe::NotOurs,
            }
        };
        CloseClipboard();
        wipe
    }
}

/// The clipboard's text, read while Rune has it open.
unsafe fn text() -> Result<Option<Zeroizing<String>>, ()> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 { return Ok(None); }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() { return Err(()); }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() { return Err(()); }
        let units = core::slice::from_raw_parts(ptr, GlobalSize(handle) / 2);
        let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let text = String::from_utf16(&units[..len]).ok().map(Zeroizing::new);
        GlobalUnlock(handle);
        Ok(text)
    }
}
