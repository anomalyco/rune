//! The pipe owns the instance. This separate, short-lived kernel object announces
//! that its owner will no longer take launches. It survives a stopped listener
//! during ExitProcess, needs no UI turn and grants no right to open a workspace.
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::sync::Mutex;

use windows_sys::Win32::System::Threading::{CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};

static GOODBYE: Mutex<Option<OwnedHandle>> = Mutex::new(None);

fn name(pipe:&[u16])->Vec<u16>{
    // Use the same user, session and app identity as the pipe. Local kernel
    // object names may not contain the pipe's namespace separator.
    "Local\\".encode_utf16().chain(pipe.iter().copied().take_while(|&c|c!=0).map(|c|if c==b'\\' as u16{b'-' as u16}else{c})).chain("-closing\0".encode_utf16()).collect()
}

pub(crate) fn announce(){
    let Some(pipe)=super::CLAIMED.get() else{return};
    // SAFETY: the name is NUL-terminated, with the current user's default DACL.
    let event=unsafe{CreateEventW(std::ptr::null(),1,1,name(pipe).as_ptr())};
    if !event.is_null(){
        // SAFETY: each successful create here returns an owned handle.
        *GOODBYE.lock().unwrap_or_else(|e|e.into_inner())=Some(unsafe{OwnedHandle::from_raw_handle(event)});
    }
}

pub(super) fn announced(pipe:&[u16])->bool{
    // Only the presence matters. Never retain this reference across a wait: the
    // old owner's last handle must be able to go away when its claim does.
    let raw=unsafe{OpenEventW(SYNCHRONIZATION_SYNCHRONIZE,0,name(pipe).as_ptr())};
    if raw.is_null(){return false}
    drop(unsafe{OwnedHandle::from_raw_handle(raw)});
    true
}

pub(super) fn released(){GOODBYE.lock().unwrap_or_else(|e|e.into_inner()).take();}
