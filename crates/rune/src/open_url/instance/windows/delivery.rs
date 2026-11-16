//! The UI and the launch race for the same one-shot event. A pipe timeout cannot
//! retract an activation which the UI has already taken, even if the reply was lost.
//! The sender creates this only when it finds an existing owner.
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject};

use crate::frame::host_clock::now_ns;
use crate::deferred_quit::{self as quit, ACK_WITHIN};
use super::super::handoff::{QUITTING, frame};

const PREFIX: &str = "--rune-activation=";
fn name(pid: u32, end: u64) -> Vec<u16> { super::wide(&format!(r"Local\rune-activation-{pid}-{end}")) }

pub(super) struct Request { pub wire: Vec<u8>, token: OwnedHandle }
impl Request {
    pub fn new(until: Instant) -> Option<Self> {
        let pid=std::process::id();
        let end=now_ns().checked_add(until.saturating_duration_since(Instant::now()).as_nanos().try_into().ok()?)?;
        // SAFETY: a NUL-terminated name; initial signalled state, auto reset. Only
        // this launch and its owner hold handles. Never reuse a pre-existing event.
        let raw=unsafe { CreateEventW(std::ptr::null(),0,1,name(pid,end).as_ptr()) };
        if raw.is_null(){return None}
        let collision=unsafe { GetLastError() }==ERROR_ALREADY_EXISTS;
        let token=unsafe { OwnedHandle::from_raw_handle(raw) };
        if collision{return None}
        let args=std::iter::once(format!("{PREFIX}{pid}:{end}")).chain(std::env::args().skip(1)).collect::<Vec<_>>();
        Some(Self{wire:frame(args.join("\0").as_bytes()),token})
    }

    /// Revoke an unserved request before opening fallback. A nonsignalled event
    /// means the UI took the token on its own turn: no second window or link replay.
    pub fn owner_won(&self)->bool {
        unsafe { WaitForSingleObject(self.token.as_raw_handle(),0)==WAIT_TIMEOUT }
    }
}

pub(super) fn answer(args:&[String])->Option<u8>{
    quit::hooks()?;
    let tag=args.first()?.strip_prefix(PREFIX)?;
    let respond=||{
        if quit::exiting(){return Some(QUITTING)}
        let (pid,end)=tag.split_once(':')?;let pid=pid.parse::<u32>().ok()?;let end=end.parse::<u64>().ok()?;
        let left=Duration::from_nanos(end.checked_sub(now_ns())?).min(ACK_WITHIN);
        // SAFETY: this can only open a Rune activation event. No write access or
        // caller-controlled kernel object name; OwnedHandle closes it on all paths.
        let raw=unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE,0,name(pid,end).as_ptr()) };
        if raw.is_null(){return None}
        let token=unsafe { OwnedHandle::from_raw_handle(raw) };
        let on_turn=move || now_ns()<end && unsafe { WaitForSingleObject(token.as_raw_handle(),0)==WAIT_OBJECT_0 };
        let served=quit::activate_if(args[1..].to_vec(),Instant::now()+left,on_turn);
        Some(if served{b'R'}else if quit::exiting(){QUITTING}else{b'U'})
    };
    Some(respond().unwrap_or(b'U'))
}
