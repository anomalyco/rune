//! Debug-only warnings for app code that works but runs slow: a `virtualize` list that builds
//! every row to mount a few, a callback that blocks the main thread, a `poll` that says it
//! changed what shows when nothing did, a window that keeps drawing while idle, a slow first
//! frame, something that commits the view on most frames of a scroll, an app stepping its own
//! scroll glide, and (mistakes rather than slowness) a `memo` whose root already had another
//! key, a `memo` id reused in the same view build, or `reveal_selected` on a `virtual_rows`
//! list. Each warns once per site, on stderr; `RUNE_PERF_WARN=0` silences them. Release builds
//! contain none of it. `budget` is the exception: an opt-in line per slow commit, in any build.

pub(crate) mod budget;
mod commits;
mod idle;
pub(crate) mod launch;
pub(crate) mod runq;

#[cfg(debug_assertions)]
pub(crate) use idle::Drawn;
pub(crate) use idle::IdleMeter;
pub(crate) use commits::CommitMeter;

/// A window's commit watch: what the commit underway is for (`RUNE_COMMIT_BUDGET_MS` prints
/// it), and the commit warnings' meters.
#[derive(Default)]
pub(crate) struct Watch {
    /// `CommitWhy` bits for the commit underway; 0 for one outside a frame's checks.
    pub cause: u16,
    pub meter: CommitMeter,
    /// Interaction timing (`tooling::interaction`), when it's on.
    pub interaction: crate::tooling::interaction::Tracker,
}

#[cfg(debug_assertions)]
use crate::time::Instant;
#[cfg(debug_assertions)]
use std::{collections::HashSet, sync::{Mutex, OnceLock}};

/// Blocked wall time (ms; time waiting for a core doesn't count) a callback may take before it warns, and how many times its CPU time
/// that wall time must also be. A callback that computes for 50 ms is slow but not blocked.
#[cfg(debug_assertions)]
const STALL_MS: f64 = 8.0;
#[cfg(debug_assertions)]
const STALL_CPU_RATIO: f64 = 4.0;

/// Rows a `virtualize` list must build, and how many times what it mounts, before it warns.
#[cfg(debug_assertions)]
const OVERBUILD_ROWS: usize = 200;
#[cfg(debug_assertions)]
const OVERBUILD_RATIO: usize = 4;

#[cfg(debug_assertions)]
fn enabled_for(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|v| v != "0")
}

#[cfg(debug_assertions)]
fn enabled() -> bool {
    #[cfg(test)]
    if let Some(on) = FAKE_ENABLED.get() { return on; }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| enabled_for(rune_core::env::var_os("RUNE_PERF_WARN").as_deref()))
}

/// Initializes the cached `RUNE_PERF_WARN` flag up front so the first timed callback does not
/// heap-allocate while reading the environment variable, and starts Windows' thread cycle-rate
/// calibration early without waiting.
#[inline(always)]
pub(crate) fn warm() {
    #[cfg(debug_assertions)]
    {
        let _ = enabled();
        #[cfg(windows)]
        if enabled() {
            let _ = crate::frame::host_clock::cycles_per_ns();
        }
    }
}

/// Prints `message` the first time `site` warns.
#[cfg(debug_assertions)]
#[inline]
fn warn_once(site: String, message: impl FnOnce() -> String) {
    let mut message = Some(message);
    warn_once_dyn(site, &mut || message.take().expect("warn_once message runs once")());
}

#[cfg(debug_assertions)]
fn warn_once_dyn(site: String, message: &mut dyn FnMut() -> String) {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    #[cfg(test)]
    let site = format!("{:?} {site}", std::thread::current().id());
    if !enabled() || !WARNED.lock().unwrap().get_or_insert_default().insert(site) { return }
    let message = format!("rune: {}. (debug builds only; RUNE_PERF_WARN=0 silences)", message());
    #[cfg(test)]
    LOG.with_borrow_mut(|log| log.push(message.clone()));
    eprintln!("{message}");
}

#[cfg(all(test, debug_assertions))]
thread_local! {
    static LOG: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The warnings this thread printed since the last call.
#[cfg(all(test, debug_assertions))]
pub(crate) fn take_log() -> Vec<String> { LOG.take() }

#[cfg(all(test, debug_assertions))]
thread_local! {
    static FAKE_ENABLED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Overrides `enabled()` on this thread until dropped, so tests can verify `RUNE_PERF_WARN=0`
/// without racing the process-wide `OnceLock`.
#[cfg(all(test, debug_assertions))]
pub(super) struct FakeEnabled {
    previous: Option<bool>,
}

#[cfg(all(test, debug_assertions))]
impl FakeEnabled {
    pub(super) fn new(on: bool) -> Self {
        Self { previous: FAKE_ENABLED.replace(Some(on)) }
    }
}

#[cfg(all(test, debug_assertions))]
impl Drop for FakeEnabled {
    fn drop(&mut self) {
        FAKE_ENABLED.set(self.previous);
    }
}

#[cfg(all(test, debug_assertions))]
#[derive(Clone, Copy)]
struct FakeSample {
    wall_ms: f64,
    cpu_ms: Option<f64>,
    wait_ms: Option<f64>,
}

#[cfg(all(test, debug_assertions))]
thread_local! {
    static FAKE_STALL: std::cell::Cell<Option<FakeSample>> = const { std::cell::Cell::new(None) };
}

/// Replaces `Stall`'s wall, CPU and run-queue readings on this thread with synthetic counters
/// until dropped, so tests do not depend on OS scheduling or host load.
#[cfg(all(test, debug_assertions))]
pub(super) struct FakeStallClock {
    previous: Option<FakeSample>,
}

#[cfg(all(test, debug_assertions))]
impl FakeStallClock {
    pub(super) fn new() -> Self {
        let previous = FAKE_STALL.replace(Some(FakeSample { wall_ms: 0.0, cpu_ms: Some(0.0), wait_ms: Some(0.0) }));
        Self { previous }
    }

    pub(super) fn advance(wall_ms: f64, cpu_ms: f64, wait_ms: Option<f64>) {
        FAKE_STALL.with(|cell| {
            if let Some(mut sample) = cell.get() {
                sample.wall_ms += wall_ms;
                sample.cpu_ms = sample.cpu_ms.map(|a| a + cpu_ms);
                sample.wait_ms = match (sample.wait_ms, wait_ms) {
                    (Some(a), Some(b)) => Some(a + b),
                    _ => None,
                };
                cell.set(Some(sample));
            }
        });
    }

    pub(super) fn advance_without_cpu(wall_ms: f64) {
        FAKE_STALL.with(|cell| {
            if let Some(mut sample) = cell.get() {
                sample.wall_ms += wall_ms;
                sample.cpu_ms = None;
                cell.set(Some(sample));
            }
        });
    }
}

#[cfg(all(test, debug_assertions))]
impl Drop for FakeStallClock {
    fn drop(&mut self) {
        FAKE_STALL.set(self.previous);
    }
}

/// A `virtualize` list at `path`, set up at `site`, was handed `built` rows and mounts
/// `mounted` of them.
#[cfg(debug_assertions)]
pub(crate) fn overbuilt(path: &str, site: Option<&std::panic::Location>, built: usize, mounted: usize) {
    if built <= OVERBUILD_ROWS || built <= OVERBUILD_RATIO * mounted { return }
    let site = site.map_or_else(String::new, |s| format!(" (.virtualize at {s})"));
    warn_once(format!("overbuild {path}"), || format!(
        "virtualized list {path:?}{site} built {built} rows to mount {mounted}: build only the \
         mounted rows with `.virtual_rows(count, estimated_row_height, key, build)`"
    ));
}

/// A `virtual_rows` list at `path`, set up at `site`, set `.reveal_selected(true)`, which
/// only inspects eager child rows of `El::virtualize`.
#[cfg(debug_assertions)]
pub(crate) fn reveal_selected_on_virtual_rows(path: &str, site: Option<&std::panic::Location>) {
    let site = site.map_or_else(String::new, |s| format!(" (.virtual_rows at {s})"));
    warn_once(format!("reveal_selected_virtual_rows {path}"), || format!(
        "virtual_rows list {path:?}{site} has `.reveal_selected(true)`, which only inspects eager \
         child rows of `El::virtualize` and does nothing here: call `Tx::scroll_into_view` or \
         `Tx::scroll_into_view_in` from the handler that moves selection"
    ));
}

/// A `virtual_rows_append_only` list at `path`, set up at `site`, changed an existing row's key
/// at `index` from `expected` to `actual` without bumping `virtual_rows_version`.
#[cfg(debug_assertions)]
pub(crate) fn virtual_rows_append_broken(
    path: &str,
    site: Option<&std::panic::Location>,
    index: usize,
    expected: &str,
    actual: &str,
) {
    let site = site.map_or_else(String::new, |s| format!(" (.virtual_rows_append_only at {s})"));
    warn_once(format!("append-only {path}"), || format!(
        "append-only virtual list {path:?}{site} changed row {index} from {expected:?} to \
         {actual:?}: bump `.virtual_rows_version(..)` on non-append changes, or remove \
         `.virtual_rows_append_only()`"
    ));
}

/// `memo(id, ..)`, called at `site`, built a root already keyed `existing` and renamed it `id`.
#[cfg(debug_assertions)]
pub(crate) fn memo_key_replaced(id: &str, existing: &str, site: &std::panic::Location) {
    if existing == id { return }
    warn_once(format!("memo key {site}"), || format!(
        "`memo({id:?}, ..)` at {site} replaced its root's key {existing:?} with {id:?}: look it up \
         by {id:?}, or key the memo with the id you want"
    ));
}

/// A memo or region with resolved cache identity `id`, called at `site`, was already used in the
/// same view build and fell back to building uncached.
#[cfg(debug_assertions)]
pub(crate) fn memo_id_reused(id: &str, site: &std::panic::Location) {
    warn_once(format!("memo id {site}"), || format!(
        "memo id {id:?} at {site} was already used in this build, so this call built uncached: \
         give each memo in the same scope a distinct id"
    ));
}

#[cfg(debug_assertions)]
enum Stamp {
    Real(Instant, crate::frame::profile::ThreadCpuMark),
    #[cfg(test)]
    Fake(FakeSample),
}

/// Times one app callback on the main thread: [`Stall::end`] warns when it spent most of its
/// time blocked rather than computing.
pub(crate) struct Stall {
    #[cfg(debug_assertions)]
    start: Option<Stamp>,
}

impl Stall {
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(crate) fn start() -> Self {
        Stall {
            #[cfg(debug_assertions)]
            start: if enabled() {
                #[cfg(test)]
                if let Some(sample) = FAKE_STALL.get() {
                    return Stall { start: Some(Stamp::Fake(sample)) };
                }
                // Record the raw cycle/nanosecond mark rather than converted milliseconds: on
                // Windows the cycle rate is unknown for the first 20 ms, and a mark taken then
                // still converts at the end once the rate is known.
                crate::frame::profile::thread_cpu_mark().map(|mark| Stamp::Real(crate::time::now(), mark))
            } else {
                None
            },
        }
    }

    /// `what` names the callback in the message, as "`poll`" or "`on_press` on \"#0/save\"";
    /// it warns once per `site`.
    #[inline(always)]
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    pub(crate) fn end(self, what: impl FnOnce() -> (String, String)) {
        #[cfg(debug_assertions)]
        match self.start {
            Some(Stamp::Real(wall, mark)) => {
                let wall = wall.elapsed().as_secs_f64() * 1000.0;
                if wall > STALL_MS {
                    let mut what = Some(what);
                    end_slow(wall, mark, &mut || what.take().expect("stall description runs once")());
                }
            }
            #[cfg(test)]
            Some(Stamp::Fake(before)) => {
                let mut what = Some(what);
                end_fake(before, &mut || what.take().expect("stall description runs once")());
            }
            None => {}
        }
    }

    /// Non-generic [`Self::end`] for a callback identified by a static `name`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(crate) fn end_named(self, name: &'static str) {
        #[cfg(not(debug_assertions))]
        let _ = name;
        #[cfg(debug_assertions)]
        match self.start {
            Some(Stamp::Real(wall, mark)) => {
                let wall = wall.elapsed().as_secs_f64() * 1000.0;
                if wall > STALL_MS {
                    end_slow(wall, mark, &mut || {
                        (name.to_owned(), if name.contains(' ') { name.to_owned() } else { format!("`{name}`") })
                    });
                }
            }
            #[cfg(test)]
            Some(Stamp::Fake(before)) => {
                end_fake(before, &mut || {
                    (name.to_owned(), if name.contains(' ') { name.to_owned() } else { format!("`{name}`") })
                });
            }
            None => {}
        }
    }
}

#[cfg(all(test, debug_assertions))]
fn end_fake(before: FakeSample, what: &mut dyn FnMut() -> (String, String)) {
    let Some(after) = FAKE_STALL.get() else { return };
    let wall = after.wall_ms - before.wall_ms;
    if wall > STALL_MS {
        let cpu = before.cpu_ms.zip(after.cpu_ms).map(|(a, b)| b - a);
        let waited = before.wait_ms.zip(after.wait_ms).map(|(a, b)| b - a);
        end_slow_measured(wall, cpu, waited, false, what);
    }
}

#[cfg(debug_assertions)]
#[cold]
#[inline(never)]
fn end_slow(wall: f64, mark: crate::frame::profile::ThreadCpuMark, what: &mut dyn FnMut() -> (String, String)) {
    let cpu = crate::frame::profile::thread_cpu_since(mark).map(|ns| ns / 1e6);
    end_slow_measured(wall, cpu, None, runq::overloaded(), what);
}

#[cfg(debug_assertions)]
#[cold]
#[inline(never)]
fn end_slow_measured(wall: f64, cpu: Option<f64>, waited: Option<f64>, loaded: bool, what: &mut dyn FnMut() -> (String, String)) {
    // Without a thread CPU reading (Windows before its 20 ms cycle-rate window, Windows on ARM,
    // or a failed OS call), computing cannot be told from blocking: stay quiet.
    let Some(cpu) = cpu else { return };
    let Some(blocked) = runq::blocked_ms(wall, cpu, waited, loaded) else { return };
    let (site, name) = what();
    warn_once(format!("stall {site}"), || {
        let queued = waited.filter(|w| *w >= 1.0).map_or_else(String::new, |w| format!(", waited {w:.0} ms for a core"));
        format!(
            "{name} blocked {blocked:.0} ms (CPU {cpu:.0} ms{queued}) on the main thread: move IO off it \
             with `rune::Latest` or a thread"
        )
    });
}

/// Runs `f`, an app callback named `name` (a call site of its own), under a [`Stall`].
#[inline(always)]
pub(crate) fn timed<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let stall = Stall::start();
    let out = f();
    stall.end_named(name);
    out
}

/// A handler's site: its name and the element's path with the digits taken out, so the rows of
/// one list share it.
#[cfg(debug_assertions)]
pub(crate) fn handler_site(name: &str, path: &str) -> String {
    format!("{name} {}", path.chars().filter(|c| !c.is_ascii_digit()).collect::<String>())
}

/// Warns once in debug builds when a turn hits its follow-up round cap (`MAX_POLL_ROUNDS`)
/// with work still waiting, deferring continuation instead of spinning.
#[cfg(debug_assertions)]
pub(crate) fn poll_round_cap(rounds: usize) {
    warn_once("poll_round_cap".into(), || {
        format!(
            "turn hit MAX_POLL_ROUNDS ({rounds}) with queued work still waiting; deferring continuation to the next wake"
        )
    });
}

#[cfg(not(debug_assertions))]
#[inline(always)]
pub(crate) fn poll_round_cap(_rounds: usize) {}

#[cfg(all(test, debug_assertions))]
mod commits_tests;
#[cfg(all(test, debug_assertions))]
mod tests;
