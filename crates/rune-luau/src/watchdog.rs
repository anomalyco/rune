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
//! thread has no CPU clock (the browser) the budget is wall time, as before.

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
    wall: Instant,
}

thread_local! {
    static DEADLINE: Cell<Option<Deadline>> = const { Cell::new(None) };
    static TICKS: Cell<u32> = const { Cell::new(0) };
    /// Proof only: the entry's start and the last safepoint (wall, thread CPU ns).
    static ENTRY: Cell<Option<(Instant, f64)>> = const { Cell::new(None) };
    static LAST: Cell<Option<(Instant, f64)>> = const { Cell::new(None) };
}

/// Proof only: `RUNE_LUAU_WALL_BUDGET=1` gives the old wall-clock budget (before this branch).
fn wall_only() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUNE_LUAU_WALL_BUDGET").is_ok_and(|v| v == "1"))
}

/// Proof only: `RUNE_LUAU_GAP_MS=N` logs every stretch over N ms between two safepoints of one
/// entry (a host call that blocked, a page fault, a wait for a CPU) with its wall and CPU time
/// and the Luau stack where it ended.
fn gap_ms() -> Option<f64> {
    static MS: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *MS.get_or_init(|| std::env::var("RUNE_LUAU_GAP_MS").ok().and_then(|v| v.parse().ok()))
}

fn clocks() -> (Instant, f64) {
    (rune::time::now(), rune::thread_cpu_ns().unwrap_or(0.0))
}

fn stack(state: *mut ffi::lua_State) -> String {
    // SAFETY: called at a safepoint on the VM's own thread; the trace is copied out at once.
    let trace = unsafe { ffi::lua_debugtrace(state) };
    if trace.is_null() {
        return String::new();
    }
    // SAFETY: `lua_debugtrace` returns a NUL-terminated string in a static buffer.
    let s = unsafe { std::ffi::CStr::from_ptr(trace) }.to_string_lossy();
    s.lines().take(4).map(str::trim).collect::<Vec<_>>().join(" <- ")
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
        let at = clocks();
        ENTRY.set(Some(at));
        LAST.set(Some(at));
        let cpu = if wall_only() { None } else { rune::thread_cpu_ns().map(|now| now + limit.as_nanos() as f64) };
        let wall = at.0 + if cpu.is_some() { limit * WALL_BACKSTOP } else { limit };
        DEADLINE.set(Some(Deadline { cpu, wall }));
    }
    Armed(!outer)
}

pub(crate) struct Armed(bool);

impl Drop for Armed {
    fn drop(&mut self) {
        if self.0 {
            DEADLINE.set(None);
            ENTRY.set(None);
            LAST.set(None);
        }
    }
}

/// Whether deadline `d` has passed: its CPU budget spent, or its wall-clock bound.
fn spent(d: Deadline) -> bool {
    d.cpu.is_some_and(|cpu| rune::thread_cpu_ns().is_some_and(|now| now >= cpu)) || rune::time::now() >= d.wall
}

unsafe extern "C-unwind" fn interrupt(state: *mut ffi::lua_State, gc: c_int) {
    if gc >= 0 {
        return;
    }
    if let Some(limit) = gap_ms()
        && let Some(last) = LAST.get()
    {
        let now = clocks();
        let wall = now.0.saturating_duration_since(last.0).as_secs_f64() * 1e3;
        if wall > limit {
            eprintln!("luau-gap: {wall:.3} ms wall, {:.3} ms CPU between two safepoints, ending at {}", (now.1 - last.1) / 1e6, stack(state));
        }
        LAST.set(Some(now));
    }
    let ticks = TICKS.get().wrapping_add(1);
    TICKS.set(ticks);
    if ticks & 255 != 0 {
        return;
    }
    // The deadline stays set past the first error, so a script that catches it with `pcall`
    // gets it again at the next check, until control is back in the host.
    if DEADLINE.get().is_some_and(spent) {
        if let Some(entry) = ENTRY.get() {
            let now = clocks();
            eprintln!(
                "luau-watchdog: {} after {:.3} ms wall, {:.3} ms CPU, at {}",
                if wall_only() { "wall budget" } else { "cpu budget" },
                now.0.saturating_duration_since(entry.0).as_secs_f64() * 1e3,
                (now.1 - entry.1) / 1e6,
                stack(state)
            );
        }
        // SAFETY: raised at a safepoint inside a protected call that this crate made; no Rust
        // frame with destructors lies between here and that call.
        unsafe { ffi::luaL_error(state, c"%s".as_ptr(), WATCHDOG_C.as_ptr()) }
    }
}

const WATCHDOG_C: &std::ffi::CStr = c"script ran longer than its time limit (watchdog)";

const _: () = assert!(WATCHDOG_C.to_bytes().len() == WATCHDOG.len());
