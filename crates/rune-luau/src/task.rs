//! The `task` library on Rune's frame clock. `spawn` runs a coroutine at once until it yields;
//! `defer` queues one for the end of the current handler or poll; `delay` and `wait` park one
//! until a deadline on the clock `AppSpec::poll` receives (virtual in headless runs), which the
//! host turns into `Tx::wake_after`; a host async call parks one until its [`Completer`] fires.
//! Nothing polls: with no deadline due and no result waiting, a poll returns without entering
//! the VM.
//!
//! [`Completer`]: crate::Completer

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::ffi::c_void;
use std::sync::Arc;

use mlua::thread::ThreadStatus;
use mlua::{Function, LightUserData, Lua, MultiValue, Table, Thread, Value};
use rune::time::Instant;
use rustc_hash::FxHashMap;

use crate::ScriptError;
use crate::workers::Shared;

static WAIT: u8 = 0;
static PARK: u8 = 0;
static JOB: u8 = 0;

fn token(tag: &'static u8) -> LightUserData {
    LightUserData(tag as *const u8 as *mut c_void)
}

/// A deadline, ordered by time then by when it was set.
#[derive(Clone, Copy, PartialEq)]
struct Due(f64, u64);

impl Eq for Due {}

impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Due {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

/// A parked task: resumed with the seconds it waited, or with `delay`'s arguments.
struct Parked {
    thread: Thread,
    since: f64,
    args: Option<MultiValue>,
}

/// One VM's scheduler, kept in the VM's app data.
pub(crate) struct Sched {
    clock: f64,
    read_at: Instant,
    timers: BinaryHeap<Reverse<Due>>,
    parked: FxHashMap<u64, Parked>,
    by_thread: FxHashMap<*const c_void, u64>,
    defers: VecDeque<(Thread, MultiValue)>,
    jobs: FxHashMap<u64, Thread>,
    next: u64,
    /// A deadline was set since the host last read [`Sched::take_changed`].
    changed: bool,
    pub(crate) errors: Vec<ScriptError>,
    pub(crate) shared: Arc<Shared>,
}

impl Sched {
    pub(crate) fn new(shared: Arc<Shared>, clock: f64) -> Self {
        Sched {
            clock,
            read_at: Instant::now(),
            timers: BinaryHeap::new(),
            parked: FxHashMap::default(),
            by_thread: FxHashMap::default(),
            defers: VecDeque::new(),
            jobs: FxHashMap::default(),
            next: 0,
            changed: false,
            errors: Vec::new(),
            shared,
        }
    }

    /// Sets the frame clock to `clock` now (a VM built ahead of time starts on the clock of the
    /// moment it runs).
    pub(crate) fn set_clock(&mut self, clock: f64) {
        self.clock = clock;
        self.read_at = Instant::now();
    }

    /// The frame clock now: the last poll's reading plus the wall time since.
    fn now(&self) -> f64 {
        self.clock + self.read_at.elapsed().as_secs_f64()
    }

    fn park(&mut self, thread: Thread, at: f64, args: Option<MultiValue>) {
        self.next += 1;
        let since = self.now();
        self.by_thread.insert(thread.to_pointer(), self.next);
        self.parked.insert(self.next, Parked { thread, since, args });
        self.timers.push(Reverse(Due(at, self.next)));
        self.changed = true;
    }

    fn forget(&mut self, thread: &Thread) {
        if let Some(seq) = self.by_thread.remove(&thread.to_pointer()) {
            self.parked.remove(&seq);
        }
        let ptr = thread.to_pointer();
        self.defers.retain(|(t, _)| t.to_pointer() != ptr);
        self.jobs.retain(|_, t| t.to_pointer() != ptr);
    }

    /// The earliest deadline still set.
    pub(crate) fn next_deadline(&mut self) -> Option<f64> {
        while let Some(Reverse(Due(at, seq))) = self.timers.peek().copied() {
            if self.parked.contains_key(&seq) {
                return Some(at);
            }
            self.timers.pop();
        }
        None
    }

    pub(crate) fn has_defers(&self) -> bool {
        !self.defers.is_empty()
    }

    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Seconds from now until `deadline` on the frame clock.
    pub(crate) fn wait_for(&self, deadline: Option<f64>) -> Option<f64> {
        deadline.map(|d| (d - self.now()).max(0.0))
    }
}

/// Builds the `task` table (the prelude adds `wait` and `cancel`) and the host half of cancel.
pub(crate) fn install(lua: &Lua) -> mlua::Result<(Table, Function)> {
    let task = lua.create_table()?;
    task.raw_set(
        "spawn",
        lua.create_function(|lua, (target, args): (Value, MultiValue)| {
            let thread = thread_of(lua, target, "task.spawn")?;
            with(lua, |s| s.forget(&thread));
            if thread.status() == ThreadStatus::Resumable {
                run(lua, &thread, args);
            }
            Ok(thread)
        })?,
    )?;
    task.raw_set(
        "defer",
        lua.create_function(|lua, (target, args): (Value, MultiValue)| {
            let thread = thread_of(lua, target, "task.defer")?;
            with(lua, |s| s.defers.push_back((thread.clone(), args)));
            Ok(thread)
        })?,
    )?;
    task.raw_set(
        "delay",
        lua.create_function(|lua, (seconds, target, args): (Option<f64>, Value, MultiValue)| {
            let thread = thread_of(lua, target, "task.delay")?;
            with(lua, |s| {
                let at = s.now() + seconds.unwrap_or(0.0).max(0.0);
                s.park(thread.clone(), at, Some(args));
            });
            Ok(thread)
        })?,
    )?;
    let forget = lua.create_function(|lua, thread: Thread| {
        with(lua, |s| s.forget(&thread));
        Ok(())
    })?;
    Ok((task, forget))
}

/// The tokens the prelude yields: wait, park, job.
pub(crate) fn tokens() -> (LightUserData, LightUserData, LightUserData) {
    (token(&WAIT), token(&PARK), token(&JOB))
}

fn thread_of(lua: &Lua, target: Value, name: &str) -> mlua::Result<Thread> {
    match target {
        Value::Function(f) => lua.create_thread(f),
        Value::Thread(t) => Ok(t),
        other => Err(mlua::Error::runtime(format!("{name} needs a function or a thread, got {}", other.type_name()))),
    }
}

fn with<R>(lua: &Lua, f: impl FnOnce(&mut Sched) -> R) -> Option<R> {
    lua.app_data_mut::<Sched>().map(|mut s| f(&mut s))
}

/// Resumes `thread` with `args` and parks it by what it yielded. An error in the task is
/// recorded, not raised: one failing task doesn't stop the code that started it.
pub(crate) fn run(lua: &Lua, thread: &Thread, args: impl mlua::IntoLuaMulti) {
    match thread.resume::<MultiValue>(args) {
        Ok(out) if thread.status() == ThreadStatus::Resumable => park(lua, thread, out),
        Ok(_) => {}
        Err(err) => {
            let err = ScriptError::from(err);
            with(lua, |s| s.errors.push(err));
        }
    }
}

fn park(lua: &Lua, thread: &Thread, out: MultiValue) {
    let tag = match out.front() {
        Some(Value::LightUserData(p)) => p.0,
        _ => std::ptr::null_mut(),
    };
    // Whole numbers arrive as integers.
    let arg = match out.get(1) {
        Some(Value::Integer(i)) => Some(*i as f64),
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    };
    with(lua, |s| {
        if tag == token(&PARK).0 {
            return;
        }
        if tag == token(&JOB).0 {
            s.jobs.insert(arg.unwrap_or_default() as u64, thread.clone());
            return;
        }
        // `task.wait(seconds)`, or a bare `coroutine.yield()` in a task: resume next frame.
        let wait = if tag == token(&WAIT).0 { arg.unwrap_or(0.0).max(0.0) } else { 0.0 };
        let at = s.now() + wait;
        s.park(thread.clone(), at, None);
    });
}

/// Runs what is due at frame time `now`: finished host calls, then deadlines that passed, then
/// deferred tasks. Tasks parked during this run wait for a later poll. Returns how many tasks
/// resumed.
pub(crate) fn poll(lua: &Lua, now: f64) -> usize {
    let Some((done, due)) = with(lua, |s| {
        s.clock = now;
        s.read_at = Instant::now();
        let done: Vec<_> =
            s.shared.take_completions().into_iter().filter_map(|(id, value)| Some((s.jobs.remove(&id)?, value))).collect();
        let mut due = Vec::new();
        while let Some(Reverse(Due(at, seq))) = s.timers.peek().copied() {
            if at > now {
                break;
            }
            s.timers.pop();
            if let Some(p) = s.parked.remove(&seq) {
                s.by_thread.remove(&p.thread.to_pointer());
                due.push(p);
            }
        }
        (done, due)
    }) else {
        return 0;
    };
    let mut resumed = done.len() + due.len();
    for (thread, value) in done {
        run(lua, &thread, value);
    }
    for p in due {
        match p.args {
            Some(args) => run(lua, &p.thread, args),
            None => run(lua, &p.thread, (now - p.since).max(0.0)),
        }
    }
    resumed += drain_defers(lua);
    resumed
}

/// Runs deferred tasks until none are left, including ones they defer.
pub(crate) fn drain_defers(lua: &Lua) -> usize {
    let mut n = 0;
    while let Some((thread, args)) = with(lua, |s| s.defers.pop_front()).flatten() {
        if thread.status() == ThreadStatus::Resumable {
            run(lua, &thread, args);
        }
        n += 1;
        if n >= 100_000 {
            let err = ScriptError::new(crate::ErrorKind::Runtime, "task.defer: more than 100,000 deferred tasks in one batch");
            with(lua, |s| {
                s.defers.clear();
                s.errors.push(err);
            });
            break;
        }
    }
    n
}
