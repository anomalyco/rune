//! [`Script`]: one Luau VM a Rune app hosts, from `init.luau` to the `El`s it renders.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use mlua::{LightUserData, Value};
use rune::time::Instant;
use rune::{Cx, El, Tx};
use rustc_hash::FxHashSet;

use crate::card::{error_card, row_error};
use crate::options::ScriptOptions;
use crate::sources::{Compiled, Sources};
use crate::task::{self, Sched};
use crate::ui::host::HostNode;
use crate::ui::rows::Spec;
use crate::ui::view::mix;
use crate::ui::{Callback, Env, Event, attach, top, with_frame};
use crate::vm::{Base, Setup, Vm};
use crate::watchdog;
use crate::workers::Shared;
use crate::{Data, ErrorKind, ScriptError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: LogLevel,
    pub message: String,
}

/// Counters for proof and profiling.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    /// Entries from Rust into the VM: renders, rows, handlers, polls that resumed tasks.
    pub vm_calls: u64,
    /// Time spent inside those entries.
    pub vm_time: Duration,
    /// Surface renders that ran the script (memo misses).
    pub renders: u64,
    /// Rows `ui.virtualList` built by calling `renderRow`.
    pub rows: u64,
    pub reloads: u64,
    pub last_reload: Option<Reload>,
}

/// How long a reload took.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reload {
    /// Parsing and compiling every file to bytecode (off the UI thread when watching).
    pub compile: Duration,
    /// Saving hot state, starting the new VM, running `init.luau`, restoring hot state.
    pub swap: Duration,
}

/// A hosted Luau script: one VM. Keep it in the window's state and call [`Script::poll`] from
/// `AppSpec::poll` and [`Script::view`] from the view.
///
/// ```ignore
/// fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool { s.script.poll(now, tx) }
/// fn view(s: &State, cx: &mut Cx<State>) -> El { s.script.view(cx) }
/// ```
pub struct Script {
    pub(crate) inner: Rc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) weak: Weak<Inner>,
    name: Rc<str>,
    pub(crate) prefix: Rc<str>,
    pub(crate) options: ScriptOptions,
    vm: RefCell<Option<Rc<Vm>>>,
    pub(crate) sources: RefCell<Sources>,
    pub(crate) failed: RefCell<Option<ScriptError>>,
    pub(crate) version: Cell<u64>,
    pub(crate) salt: Cell<u64>,
    pub(crate) dark: Cell<bool>,
    pub(crate) clock: Cell<f64>,
    /// The earliest task deadline on the frame clock, or infinity.
    deadline: Cell<f64>,
    pub(crate) metrics: Cell<Metrics>,
    log: RefCell<VecDeque<LogLine>>,
    warned: Rc<RefCell<FxHashSet<String>>>,
    nodes: Rc<[HostNode]>,
    pub(crate) shared: Arc<Shared>,
    /// A VM base built ahead of the next reload ([`Script::prewarm`]).
    spare: RefCell<Option<Base>>,
    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    pub(crate) watch: RefCell<Option<crate::watch::Watch>>,
}

impl Script {
    /// Compiles and starts `sources` with default options.
    pub fn new(name: &str, sources: Sources) -> Self {
        Script::with_options(name, sources, ScriptOptions::new())
    }

    /// Compiles and starts `sources`. A script that fails to start still exists: its surfaces
    /// show the error, and [`Script::error`] returns it.
    pub fn with_options(name: &str, sources: Sources, options: ScriptOptions) -> Self {
        let nodes: Rc<[HostNode]> = options.nodes.clone().into();
        let inner = Rc::new_cyclic(|weak| Inner {
            weak: weak.clone(),
            name: name.into(),
            prefix: format!("luau:{name}:").into(),
            options,
            vm: RefCell::new(None),
            sources: RefCell::new(sources.clone()),
            failed: RefCell::new(None),
            version: Cell::new(1),
            salt: Cell::new(1),
            dark: Cell::new(true),
            clock: Cell::new(0.0),
            deadline: Cell::new(f64::INFINITY),
            metrics: Cell::new(Metrics::default()),
            log: RefCell::new(VecDeque::new()),
            warned: Rc::default(),
            nodes,
            shared: Arc::default(),
            spare: RefCell::new(None),
            #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
            watch: RefCell::new(None),
        });
        let started = sources.compile().and_then(|c| inner.swap(c).map(|_| ()));
        if let Err(err) = started {
            inner.fail(err);
        }
        Script { inner }
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Gives the script the window's waker, so finished host work and `task.delay` deadlines
    /// set outside a handler can ask for a frame. Call it from `AppSpec::setup`.
    pub fn set_waker(&self, waker: rune::Waker) {
        self.inner.shared.set_waker(waker);
    }

    /// Runs what is due: finished host calls, `task.wait`/`task.delay` deadlines up to `now`
    /// (`AppSpec::poll`'s clock) and deferred tasks, then asks for a frame at the next deadline.
    /// Returns true when script code ran. With nothing due it returns at once and never enters
    /// the VM, so an idle script costs nothing.
    pub fn poll(&self, now: f64, tx: &mut Tx) -> bool {
        self.inner.poll(now, tx)
    }

    /// The `render` surface: what `init.luau`'s `render(view)` returns.
    pub fn view<S: 'static>(&self, cx: &Cx<S>) -> El {
        self.surface(cx, "render")
    }

    /// Surface `name`: what the module's function `name(view)` returns. The result is memoized
    /// on the script's version, which moves only when script code runs, so a view rebuilt for
    /// other reasons reuses it without entering the VM.
    pub fn surface<S: 'static>(&self, cx: &Cx<S>, name: &str) -> El {
        let i = &self.inner;
        let dark = cx.system_dark();
        i.dark.set(dark);
        // The surface's root is keyed by its memo id, whatever key the script gave it.
        let id = format!("{}{name}", i.prefix);
        rune::memo::memo_with_hash(id.clone(), mix(i.version.get(), i.salt.get()), || i.render::<S>(name, dark).key(id))
    }

    /// Runs the surface's render function now, outside any memo: the cost of a rebuild.
    pub fn render_uncached<S: 'static>(&self, name: &str, dark: bool) -> El {
        self.inner.render::<S>(name, dark)
    }

    /// Recompiles from `sources` and swaps the VM. `saveHotState()` from the old VM is passed to
    /// `loadHotState(state)` in the new one. A compile or start error keeps the old VM running.
    pub fn reload(&self, sources: Sources) -> Result<Reload, ScriptError> {
        let start = Instant::now();
        let compiled = sources.compile().inspect_err(|e| self.inner.log(LogLevel::Error, &e.message))?;
        let compile = start.elapsed();
        let mut report = self.inner.swap(compiled).inspect_err(|e| self.inner.log(LogLevel::Error, &e.message))?;
        report.compile = compile;
        *self.inner.sources.borrow_mut() = sources;
        self.inner.note_reload(report);
        Ok(report)
    }

    /// Fires the signal the host made with [`Host::signal`] as `name`.
    pub fn fire(&self, name: &str, args: impl IntoIterator<Item = Data>) -> bool {
        let args: Vec<Data> = args.into_iter().collect();
        let fired = self.inner.enter(None, |vm| Ok(vm.fire(name, args)?));
        fired.is_some_and(|f| f.unwrap_or(false))
    }

    /// Calls the module's function `name` with `args` in a new task.
    pub fn call(&self, name: &str, args: impl IntoIterator<Item = Data>) -> Result<(), ScriptError> {
        let args: Vec<Data> = args.into_iter().collect();
        let ran = self.inner.enter(None, |vm| {
            let f = vm.export(name).ok_or_else(|| ScriptError::new(ErrorKind::Missing, format!("no function {name}")))?;
            let thread = vm.lua.create_thread(f)?;
            task::run(&vm.lua, &thread, mlua::Variadic::from_iter(args));
            Ok(())
        });
        ran.unwrap_or_else(|| Err(self.inner.failed.borrow().clone().unwrap_or_else(|| ScriptError::new(ErrorKind::Missing, "not running"))))
    }

    /// Calls the module's function `name` with `args` and returns its result. It runs to the
    /// end at once: like a render, it can't wait.
    pub fn query(&self, name: &str, args: impl IntoIterator<Item = Data>) -> Result<Data, ScriptError> {
        let args: Vec<Data> = args.into_iter().collect();
        let ran = self.inner.enter(None, |vm| {
            let f = vm.export(name).ok_or_else(|| ScriptError::new(ErrorKind::Missing, format!("no function {name}")))?;
            Ok(f.call::<Data>(mlua::Variadic::from_iter(args))?)
        });
        ran.unwrap_or_else(|| Err(self.inner.failed.borrow().clone().unwrap_or_else(|| ScriptError::new(ErrorKind::Missing, "not running"))))
    }

    /// Why the script isn't running, if it isn't.
    pub fn error(&self) -> Option<ScriptError> {
        self.inner.failed.borrow().clone()
    }

    /// Recent `print`, `warn` and error lines (up to 256).
    pub fn log(&self) -> Vec<LogLine> {
        self.inner.log.borrow().iter().cloned().collect()
    }

    pub fn metrics(&self) -> Metrics {
        self.inner.metrics.get()
    }

    /// Bytes the VM's heap holds.
    pub fn memory(&self) -> usize {
        self.inner.vm().map_or(0, |vm| vm.lua.used_memory())
    }

    /// The prefix of every memo id this script's surfaces use, for
    /// `Tx::invalidate_memos_by_prefix`.
    pub fn memo_prefix(&self) -> &str {
        &self.inner.prefix
    }

    /// The script's current sources.
    pub fn sources(&self) -> Sources {
        self.inner.sources.borrow().clone()
    }

    /// The Luau VM, for host bindings beyond [`Host`]. Changes here don't survive a reload.
    pub fn with_lua<R>(&self, f: impl FnOnce(&mlua::Lua) -> R) -> Option<R> {
        self.inner.vm().map(|vm| f(&vm.lua))
    }

    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    pub(crate) fn inner(&self) -> &Rc<Inner> {
        &self.inner
    }
}

impl Inner {
    pub(crate) fn vm(&self) -> Option<Rc<Vm>> {
        self.vm.borrow().clone()
    }

    fn fail(&self, err: ScriptError) {
        self.log(LogLevel::Error, &err.message);
        *self.failed.borrow_mut() = Some(err);
        self.bump();
    }

    fn bump(&self) {
        self.version.set(self.version.get().wrapping_add(1));
    }

    pub(crate) fn log(&self, level: LogLevel, message: &str) {
        if let Some(f) = &self.options.on_log {
            f(level, message);
        }
        let mut log = self.log.borrow_mut();
        if log.len() == 256 {
            log.pop_front();
        }
        log.push_back(LogLine { level, message: message.to_owned() });
    }

    /// One entry into the VM under the watchdog, counted in the metrics. Errors are logged.
    pub(crate) fn timed<R>(&self, vm: &Vm, f: impl FnOnce(&Vm) -> R) -> R {
        vm.collect();
        let _armed = watchdog::arm(self.options.time_limit);
        let start = Instant::now();
        let out = f(vm);
        let mut m = self.metrics.get();
        m.vm_calls += 1;
        m.vm_time += start.elapsed();
        self.metrics.set(m);
        out
    }

    /// Runs script code that may change state: afterwards the version moves, deferred tasks
    /// run, task errors are logged and the next deadline is armed (on `tx`, or by waking).
    pub(crate) fn enter<R>(&self, tx: Option<&mut Tx>, f: impl FnOnce(&Vm) -> Result<R, ScriptError>) -> Option<Result<R, ScriptError>> {
        let vm = self.vm()?;
        let out = self.timed(&vm, |vm| {
            let out = f(vm);
            task::drain_defers(&vm.lua);
            out
        });
        if let Err(e) = &out {
            self.log(LogLevel::Error, &e.message);
        }
        self.settle(&vm, tx);
        Some(out)
    }

    /// After script code ran: logs task errors, moves the version and arms the next deadline.
    fn settle(&self, vm: &Vm, tx: Option<&mut Tx>) {
        self.bump();
        // One incremental collection step while the VM is warm, so garbage from this batch is
        // swept between frames rather than in the middle of a render. An idle script runs none.
        vm.lua.gc_step().ok();
        let Some((errors, deadline, wait, changed)) = vm.lua.app_data_mut::<Sched>().map(|mut s| {
            let errors = std::mem::take(&mut s.errors);
            let deadline = s.next_deadline();
            (errors, deadline, s.wait_for(deadline), s.take_changed())
        }) else {
            return;
        };
        for e in errors {
            self.log(LogLevel::Error, &e.message);
        }
        self.deadline.set(deadline.unwrap_or(f64::INFINITY));
        match (tx, wait) {
            (Some(tx), Some(wait)) => tx.wake_after(Duration::from_secs_f64(wait)),
            (None, Some(_)) if changed => self.shared.wake(),
            _ => {}
        }
    }

    /// After a render, which has no `Tx`: a `task.delay` or `task.defer` it set asks for a
    /// frame, whose poll runs it.
    pub(crate) fn note_schedule(&self, vm: &Vm) {
        let Some((changed, deadline, defers)) =
            vm.lua.app_data_mut::<Sched>().map(|mut s| (s.take_changed(), s.next_deadline(), s.has_defers()))
        else {
            return;
        };
        if changed || defers {
            self.deadline.set(if defers { self.clock.get() } else { deadline.unwrap_or(f64::INFINITY) });
            self.shared.mark();
            self.shared.wake();
        }
    }

    fn poll(&self, now: f64, tx: &mut Tx) -> bool {
        self.clock.set(now);
        #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
        let reloaded = crate::watch::apply(self);
        #[cfg(not(all(feature = "watch", not(target_arch = "wasm32"))))]
        let reloaded = false;
        let arrived = self.shared.take_dirty();
        if !arrived && now < self.deadline.get() {
            return reloaded;
        }
        let Some(vm) = self.vm() else { return reloaded };
        let resumed = self.timed(&vm, |vm| task::poll(&vm.lua, now));
        self.settle(&vm, Some(tx));
        if resumed == 0 {
            // Nothing ran: the version needn't have moved.
            self.version.set(self.version.get().wrapping_sub(1));
        }
        reloaded || resumed > 0
    }

    /// Runs a handler as a task with its `UiEvent`, so it may wait.
    pub(crate) fn handle(&self, cb: &Callback, event: Event<'_>, tx: Option<&mut Tx>) {
        if self.vm().is_none_or(|vm| vm.id != cb.vm) {
            return;
        }
        let r = cb.func.r;
        self.enter(tx, |vm| {
            let f = vm.function(r)?;
            let thread = vm.lua.create_thread(f)?;
            task::run(&vm.lua, &thread, event.to_table(&vm.lua)?);
            Ok(())
        });
    }

    pub(crate) fn reload_from_card(&self, _tx: &mut Tx) {
        let sources = self.sources.borrow().clone();
        let start = Instant::now();
        let swapped = sources.compile().and_then(|c| {
            let compile = start.elapsed();
            self.swap(c).map(|r| Reload { compile, ..r })
        });
        match swapped {
            Ok(report) => self.note_reload(report),
            Err(err) => self.fail(err),
        }
    }

    #[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
    pub(crate) fn set_sources(&self, sources: Sources) {
        *self.sources.borrow_mut() = sources;
    }

    pub(crate) fn note_reload(&self, report: Reload) {
        let mut m = self.metrics.get();
        m.reloads += 1;
        m.last_reload = Some(report);
        self.metrics.set(m);
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let (compile, swap) = (ms(report.compile), ms(report.swap));
        self.log(LogLevel::Info, &format!("reloaded in {:.2} ms: compile {compile:.2} ms, swap {swap:.2} ms", compile + swap));
    }

    /// Starts a VM from `compiled`, carrying hot state over from the running one, and swaps it
    /// in. On error the running VM stays.
    pub(crate) fn swap(&self, compiled: Compiled) -> Result<Reload, ScriptError> {
        let start = Instant::now();
        let old = self.vm();
        let saved = old.as_ref().and_then(|vm| {
            let save = vm.export("saveHotState")?;
            self.timed(vm, |_| save.call::<Data>(())).inspect_err(|e| self.log(LogLevel::Warn, &format!("saveHotState: {e}"))).ok()
        });
        let base = match self.spare.take() {
            Some(base) => base,
            None => self.base()?,
        };
        let (mut vm, entry) = Vm::new(base, &Rc::new(compiled), self.clock.get())?;
        let module = self.timed(&vm, |_| entry.call::<Value>(()))?;
        vm.set_module(module)?;
        if let Some(start) = self.options.start.clone() {
            self.timed(&vm, |vm| start(&vm.lua, &vm.module, vm.id))?;
        }
        if let (Some(saved), Some(load)) = (saved, vm.export("loadHotState")) {
            self.timed(&vm, |_| load.call::<()>(saved))?;
        }
        *self.vm.borrow_mut() = Some(Rc::new(vm));
        *self.failed.borrow_mut() = None;
        self.warned.borrow_mut().clear();
        self.salt.set(self.salt.get().wrapping_add(1));
        if let Some(vm) = self.vm() {
            self.settle(&vm, None);
        }
        drop(old);
        Ok(Reload { compile: Duration::ZERO, swap: start.elapsed() })
    }

    /// The part of a new VM that doesn't depend on the script's files.
    fn base(&self) -> Result<Base, ScriptError> {
        let weak = self.weak.clone();
        let log: Rc<dyn Fn(LogLevel, String)> = Rc::new(move |level, message| {
            if let Some(i) = weak.upgrade() {
                i.log(level, &message);
            }
        });
        Base::new(Setup { shared: &self.shared, memory_limit: self.options.memory_limit, nodes: &self.options.nodes, host: self.options.setup.as_deref(), log })
    }

    pub(crate) fn prewarm(&self) {
        if self.spare.borrow().is_none() {
            let base = self.base().ok();
            *self.spare.borrow_mut() = base;
        }
    }

    pub(crate) fn env(&self, vm: &Vm, dark: bool) -> Rc<Env> {
        Rc::new(Env {
            script: self.weak.clone(),
            vm: vm.id,
            render: crate::ui::next_render(),
            salt: self.salt.get(),
            version: self.version.get(),
            dark,
            memo_failed: Cell::new(false),
            palette: self.options.palette.clone(),
            shadows: self.options.shadows.clone(),
            text: self.options.text,
            weights: self.options.weights,
            garbage: Rc::downgrade(&vm.garbage),
            templates: vm.templates.clone(),
            recording: RefCell::new(None),
            prefix: self.prefix.clone(),
            nodes: self.nodes.clone(),
            warned: self.warned.clone(),
            colors: RefCell::new(Vec::new()),
        })
    }

    pub(crate) fn render<S: 'static>(&self, name: &str, dark: bool) -> El {
        let attach = attach::<S>();
        let card = |message: &str| error_card(message, &self.options.palette, dark, attach, self.weak.clone());
        if let Some(err) = self.failed.borrow().as_ref() {
            return card(&err.message);
        }
        let Some(vm) = self.vm() else { return card("the script isn't running") };
        let Some(render) = vm.export(name) else {
            return card(&format!("init.luau exports no {name} function"));
        };
        let env = self.env(&vm, dark);
        let built = with_frame(attach, env.clone(), || {
            self.timed(&vm, |vm| {
                vm.view.raw_set("dark", dark)?;
                let out = render.call::<Value>(vm.view.clone())?;
                take_root(out)
            })
        });
        let mut m = self.metrics.get();
        m.renders += 1;
        self.metrics.set(m);
        self.note_schedule(&vm);
        if env.memo_failed.get() {
            self.salt.set(self.salt.get().wrapping_add(1));
        }
        match built {
            Ok(el) => el,
            Err(err) => {
                let err = ScriptError::from(err);
                self.log(LogLevel::Error, &err.message);
                self.salt.set(self.salt.get().wrapping_add(1));
                card(&err.message)
            }
        }
    }

    pub(crate) fn row<S: 'static>(&self, spec: &Spec, i: usize) -> El {
        let Some(vm) = self.vm().filter(|vm| vm.id == spec.render.vm) else { return rune::el() };
        let env = self.env(&vm, self.dark.get());
        let r = spec.render.func.r;
        let built = with_frame(attach::<S>(), env, || {
            self.timed(&vm, |vm| {
                let out = vm.function(r)?.call::<Value>(i + 1)?;
                take_root(out)
            })
        });
        let mut m = self.metrics.get();
        m.rows += 1;
        self.metrics.set(m);
        self.note_schedule(&vm);
        built.unwrap_or_else(|err| {
            let err = ScriptError::from(err);
            self.log(LogLevel::Error, &err.message);
            row_error(&err.message)
        })
    }

    pub(crate) fn row_key(&self, cb: &Callback, i: usize) -> String {
        let Some(vm) = self.vm().filter(|vm| vm.id == cb.vm) else { return i.to_string() };
        let r = cb.func.r;
        let key = self.timed(&vm, |vm| vm.function(r)?.call::<Value>(i + 1));
        match key {
            Ok(Value::String(s)) => s.to_string_lossy(),
            Ok(Value::Integer(n)) => n.to_string(),
            Ok(Value::Number(n)) => crate::ui::props::format_num(n),
            _ => i.to_string(),
        }
    }
}

/// The element behind the handle a render or row function returned.
pub(crate) fn take_root(out: Value) -> mlua::Result<El> {
    match out {
        Value::LightUserData(LightUserData(p)) => top(|f| f.take(p)).unwrap_or_else(|| Err("no render in progress".into())).map_err(mlua::Error::runtime),
        Value::Nil => Ok(rune::el()),
        other => Err(mlua::Error::runtime(format!("a render function must return a ui node, not a {}", other.type_name()))),
    }
}
