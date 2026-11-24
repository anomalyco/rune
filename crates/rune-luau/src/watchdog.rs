//! The watchdog: every entry into a VM gets a budget (250 ms by default) of the thread's own CPU
//! time, and Luau's interrupt safepoints (loop back edges, calls) raise an error once it is spent,
//! so `while true do end` stops that script without touching the rest of the app. The check reads
//! the clocks every 256 safepoints, which keeps it off the profile of ordinary code.
//!
//! CPU time, not the wall clock: on a loaded machine the thread may wait for a CPU, and a host call
//! may wait on the disk or another thread, for longer than the budget, and neither is the script's
//! doing. A wall-clock deadline charged that time to the script and stopped plugins that ran fine
//! (a plugin's start-up dying while a build ran, 22 Nov). A wall-clock backstop of
//! [`WALL_BACKSTOP`] budgets still bounds an entry that a starved CPU stretches out, and says so on
//! stderr when it is what stops a script (wall and CPU time, file and line). Where the
//! thread has no CPU clock (the browser, Windows on ARM, the first 20 ms of a Windows process while
//! the clock's rate is measured), or the clock can't be read mid-entry, the budget is wall time, as
//! before.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::io::Write;
use std::os::raw::c_int;
use std::time::Duration;

use mlua::{Lua, ffi};
use rune::time::Instant;

use crate::error::WATCHDOG;

/// The wall-clock bound on one entry, in budgets.
const WALL_BACKSTOP: u32 = 20;

#[derive(Clone, Copy)]
struct Deadline {
    /// The thread's CPU clock when the entry was armed; `None` without one. Converted at each
    /// check, so an entry armed before Windows knows the cycles' rate is charged from its start
    /// once it does (the first plugin's start-up included).
    mark: Option<rune::ThreadCpuMark>,
    /// The budget in nanoseconds of CPU time.
    limit_ns: f64,
    /// The budget in wall time: the deadline while there's no CPU reading.
    budget: Option<Instant>,
    /// The wall-clock bound with a CPU clock: [`WALL_BACKSTOP`] budgets.
    backstop: Option<Instant>,
    /// When the entry was armed: what a backstop stop reports.
    armed: Instant,
    /// The backstop's stop was logged (a script that catches it is stopped again at each check).
    told: bool,
}

/// What stopped an entry.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    /// Its CPU budget, or its wall-clock budget without a CPU clock.
    Budget,
    /// The wall-clock backstop, with CPU budget left: the thread was starved, not looping.
    Backstop,
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
        let mark = rune::thread_cpu_mark();
        // A limit too large to add is no wall-clock limit (`None`), never a panic.
        let budget = at.checked_add(limit);
        let backstop = mark.and_then(|_| at.checked_add(limit.saturating_mul(WALL_BACKSTOP)));
        DEADLINE.set(Some(Deadline { mark, limit_ns: limit.as_nanos() as f64, budget, backstop, armed: at, told: false }));
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

/// What stops deadline `d`'s entry, if anything yet: its CPU budget spent, its backstop, or without
/// a CPU reading its budget in wall time. Never panics: it runs inside Luau's interrupt.
fn stop(d: Deadline) -> Option<Stop> {
    let now = rune::time::now();
    let past = |at: Option<Instant>| at.is_some_and(|at| now >= at);
    match d.mark.and_then(rune::thread_cpu_since) {
        Some(used) if used >= d.limit_ns => Some(Stop::Budget),
        Some(_) => past(d.backstop).then_some(Stop::Backstop),
        None => past(d.budget).then_some(Stop::Budget),
    }
}

/// One line on stderr when the backstop, not the CPU budget, stops a script: its wall and CPU time
/// and where it was, so a backstop that fires on a real machine is seen and its factor can be tuned.
fn tell_backstop(state: *mut ffi::lua_State, d: Deadline) {
    // SAFETY: a zeroed `lua_Debug` is a valid out-parameter; level 0 is the function running at
    // this safepoint, and the strings it points to live while it does.
    let at = unsafe {
        let mut ar: ffi::lua_Debug = std::mem::zeroed();
        if ffi::lua_getinfo(state, 0, c"sl".as_ptr(), &mut ar) != 0 && !ar.short_src.is_null() {
            format!("{}:{}", std::ffi::CStr::from_ptr(ar.short_src).to_string_lossy(), ar.currentline)
        } else {
            "?".to_owned()
        }
    };
    let wall = rune::time::now().saturating_duration_since(d.armed).as_secs_f64() * 1e3;
    let cpu = d.mark.and_then(rune::thread_cpu_since).map_or(0.0, |ns| ns / 1e6);
    // Not `eprintln!`: it panics when stderr is a closed pipe, and this runs inside Luau's interrupt.
    writeln!(std::io::stderr().lock(), "luau: watchdog backstop stopped {at} after {wall:.3} ms wall with {cpu:.3} ms CPU charged (starved, not looping)").ok();
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
    let Some(d) = DEADLINE.get() else { return };
    let Some(why) = stop(d) else { return };
    if why == Stop::Backstop && !d.told {
        tell_backstop(state, d);
        DEADLINE.set(Some(Deadline { told: true, ..d }));
    }
    // SAFETY: raised at a safepoint inside a protected call that this crate made; no Rust
    // frame with destructors lies between here and that call.
    unsafe { ffi::luaL_error(state, c"%s".as_ptr(), WATCHDOG_C.as_ptr()) }
}

const WATCHDOG_C: &std::ffi::CStr = c"script ran longer than its time limit (watchdog)";

const _: () = assert!(WATCHDOG_C.to_bytes().len() == WATCHDOG.len());
