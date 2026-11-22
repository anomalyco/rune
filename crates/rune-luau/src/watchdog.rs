//! The watchdog: every entry into a VM gets a budget (250 ms by default) of the thread's own CPU
//! time, and Luau's interrupt safepoints (loop back edges, calls) raise an error once it is spent,
//! so `while true do end` stops that script without touching the rest of the app. The check reads
//! the clocks every 256 safepoints, which keeps it off the profile of ordinary code.
//!
//! CPU time, not the wall clock: on a loaded machine the thread may wait for a CPU, and a host call
//! may wait on the disk or another thread, for longer than the budget, and neither is the script's
//! doing. A wall-clock deadline charged that time to the script and stopped plugins that ran fine
//! (a plugin's start-up dying while a build ran, 22 Nov). A wall-clock backstop of
//! [`WALL_BACKSTOP`] budgets still bounds an entry that a starved CPU stretches out. Where the
//! thread has no CPU clock (the browser, Windows on ARM, the first 20 ms of a Windows process while
//! the clock's rate is measured), or the clock can't be read mid-entry, the budget is wall time, as
//! before.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::os::raw::c_int;
use std::time::Duration;

use mlua::{Lua, ffi};
use rune::time::Instant;

use crate::error::WATCHDOG;

/// The wall-clock bound on one entry, in budgets.
const WALL_BACKSTOP: u32 = 20;

#[derive(Clone, Copy)]
struct Deadline {
    /// The thread's CPU time (ns) at which the budget is spent; `None` without a CPU clock.
    cpu: Option<f64>,
    /// The budget in wall time: the deadline without a CPU clock, or when it can't be read.
    budget: Option<Instant>,
    /// The wall-clock bound with a CPU clock: [`WALL_BACKSTOP`] budgets.
    backstop: Option<Instant>,
}

thread_local! {
    static DEADLINE: Cell<Option<Deadline>> = const { Cell::new(None) };
    static TICKS: Cell<u32> = const { Cell::new(0) };
}

/// Installs the interrupt on `lua`. It never yields and never calls back into Rust code that
/// could hold the VM, so it is safe at every safepoint.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: `lua_callbacks` returns the VM's callback table, which lives as long as the VM; the
    // interrupt slot is a plain function pointer mlua leaves unset.
    unsafe { lua.exec_raw::<()>((), |state| (*ffi::lua_callbacks(state)).interrupt = Some(interrupt)) }
}
/// Arms the budget for one entry into a VM, unless an outer entry already did; dropping the
/// guard disarms what it armed.
pub(crate) fn arm(limit: Duration) -> Armed {
    let outer = DEADLINE.get().is_some();
    if !outer {
        let at = rune::time::now();
        let cpu = rune::thread_cpu_ns().map(|now| now + limit.as_nanos() as f64);
        // A limit too large to add is no wall-clock limit (`None`), never a panic.
        let budget = at.checked_add(limit);
        let backstop = cpu.and_then(|_| at.checked_add(limit.saturating_mul(WALL_BACKSTOP)));
        DEADLINE.set(Some(Deadline { cpu, budget, backstop }));
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

/// Whether deadline `d` has passed: its CPU budget spent or its backstop, or without a CPU reading
/// its budget in wall time. Never panics: it runs inside Luau's interrupt.
fn spent(d: Deadline) -> bool {
    let now = rune::time::now();
    let past = |at: Option<Instant>| at.is_some_and(|at| now >= at);
    match (d.cpu, d.cpu.and_then(|_| rune::thread_cpu_ns())) {
        (Some(cpu), Some(used)) => used >= cpu || past(d.backstop),
        _ => past(d.budget),
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
    if DEADLINE.get().is_some_and(spent) {
        // SAFETY: raised at a safepoint inside a protected call that this crate made; no Rust
        // frame with destructors lies between here and that call.
        unsafe { ffi::luaL_error(state, c"%s".as_ptr(), WATCHDOG_C.as_ptr()) }
    }
}

const WATCHDOG_C: &std::ffi::CStr = c"script ran longer than its time limit (watchdog)";

const _: () = assert!(WATCHDOG_C.to_bytes().len() == WATCHDOG.len());
