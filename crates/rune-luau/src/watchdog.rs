//! The watchdog: every entry into a VM gets a deadline (250 ms by default), and Luau's interrupt
//! safepoints (loop back edges, calls) raise an error once it passes, so `while true do end`
//! stops that script without touching the rest of the app. The check reads the clock every 256
//! safepoints, which keeps it off the profile of ordinary code.

use std::cell::Cell;
use std::os::raw::c_int;
use std::time::Duration;

use mlua::{Lua, ffi};
use rune::time::Instant;

use crate::error::WATCHDOG;

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static TICKS: Cell<u32> = const { Cell::new(0) };
}

/// Installs the interrupt on `lua`. It never yields and never calls back into Rust code that
/// could hold the VM, so it is safe at every safepoint.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: `lua_callbacks` returns the VM's callback table, which lives as long as the VM; the
    // interrupt slot is a plain function pointer mlua leaves unset.
    unsafe { lua.exec_raw::<()>((), |state| (*ffi::lua_callbacks(state)).interrupt = Some(interrupt)) }
}
/// Arms the deadline for one entry into a VM, unless an outer entry already did; dropping the
/// guard disarms what it armed.
pub(crate) fn arm(limit: Duration) -> Armed {
    let outer = DEADLINE.get().is_some();
    if !outer {
        DEADLINE.set(Some(Instant::now() + limit));
    }
    Armed(!outer)
}

pub(crate) struct Armed(bool);

impl Drop for Armed {
    fn drop(&mut self) {
        if self.0 {
            DEADLINE.set(None);
        }
    }
}

unsafe extern "C-unwind" fn interrupt(state: *mut ffi::lua_State, gc: c_int) {
    if gc >= 0 {
        return;
    }
    let ticks = TICKS.get().wrapping_add(1);
    TICKS.set(ticks);
    if ticks & 255 != 0 {
        return;
    }
    // The deadline stays set past the first error, so a script that catches it with `pcall`
    // gets it again at the next check, until control is back in the host.
    if DEADLINE.get().is_some_and(|d| Instant::now() >= d) {
        // SAFETY: raised at a safepoint inside a protected call that this crate made; no Rust
        // frame with destructors lies between here and that call.
        unsafe { ffi::luaL_error(state, c"%s".as_ptr(), WATCHDOG_C.as_ptr()) }
    }
}

const WATCHDOG_C: &std::ffi::CStr = c"script ran longer than its time limit (watchdog)";

const _: () = assert!(WATCHDOG_C.to_bytes().len() == WATCHDOG.len());
