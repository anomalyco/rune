//! Running script functions a host holds: a host that takes callbacks from a script (a panel's
//! `render`, a command's `run`) renders, runs and queries them through these, under the same
//! watchdog, metrics and error handling as the script's own entries.

use std::time::Duration;

use mlua::{FromLuaMulti, Function, IntoLuaMulti, Table, Value};
use rune::{El, Tx};

use crate::card::error_card;
use crate::script::{Inner, LogLevel, Reload, take_root};
use crate::sources::{Compiled, Sources};
use crate::task;
use crate::ui::view::mix;
use crate::ui::{attach, with_frame};
use crate::{ErrorKind, Script, ScriptError};

impl Script {
    /// Moves whenever script code runs (a handler, a task, a signal), and on a reload. A host
    /// mixes it into the versions it passes [`Script::render_fn`], so a surface rebuilds after
    /// the script ran and not otherwise.
    pub fn version(&self) -> u64 {
        self.inner.version.get()
    }

    /// The running VM's id, unique in the process; `None` when the script isn't running. It
    /// changes with every VM a reload starts, and [`ScriptOptions::start`](crate::ScriptOptions::start)
    /// receives the new one.
    pub fn vm_id(&self) -> Option<u64> {
        self.inner.vm().map(|vm| vm.id)
    }

    /// The table the entry module returned.
    pub fn module(&self) -> Option<Table> {
        self.inner.vm().map(|vm| vm.module.clone())
    }

    /// Renders `f(view, args...)`, memoized as `id` on `version`: while `version` holds (and
    /// the script hasn't reloaded) the subtree is reused without entering the VM. A render
    /// error shows the error card, with Reload.
    pub fn render_fn<S: 'static>(&self, id: &str, version: u64, dark: bool, f: &Function, args: impl IntoLuaMulti) -> El {
        let i = &self.inner;
        i.dark.set(dark);
        let id = format!("{}{id}", i.prefix);
        rune::memo::memo_with_hash(id.clone(), mix(version, i.salt.get()), || i.render_with::<S>(dark, f, args).key(id))
    }

    /// Runs `f(args...)` as a task, so it may wait: a command's `run`, a signal from the host.
    /// With `tx`, the deadline it sets arms on the transaction.
    pub fn spawn_fn(&self, f: &Function, args: impl IntoLuaMulti, tx: Option<&mut Tx>) -> Result<(), ScriptError> {
        let ran = self.inner.enter(tx, |vm| {
            let thread = vm.lua.create_thread(f.clone())?;
            task::run(&vm.lua, &thread, args);
            Ok(())
        });
        ran.unwrap_or_else(|| Err(self.inner.not_running()))
    }

    /// Calls `f(args...)` and returns its result, at once and without letting it wait: a
    /// query a host makes while it draws (a pill's `show`, a link handler's `match`). It leaves
    /// the script's version where it was.
    pub fn eval_fn<R: FromLuaMulti>(&self, f: &Function, args: impl IntoLuaMulti) -> Result<R, ScriptError> {
        let Some(vm) = self.inner.vm() else { return Err(self.inner.not_running()) };
        let out = self.inner.timed(&vm, |_| f.call::<R>(args));
        self.inner.note_schedule(&vm);
        out.map_err(|e| {
            let e = ScriptError::from(e);
            self.inner.log(LogLevel::Error, &e.message);
            e
        })
    }

    /// Builds, ahead of the next reload, the part of a VM that doesn't depend on the script's
    /// files: the Luau state, the prelude, `@rune/ui` and the host's
    /// [`ScriptOptions::setup`](crate::ScriptOptions::setup). The reload then only loads the
    /// modules and runs them, so its swap on the UI thread is shorter. Call it once a reload is
    /// coming (a file was saved and compiles off the UI thread meanwhile); with one built
    /// already it does nothing. `setup` runs here rather than in the reload.
    pub fn prewarm(&self) {
        self.inner.prewarm();
    }

    /// [`Script::reload`] with sources the host compiled itself, off the UI thread (`compile` is
    /// how long that took, for the report).
    pub fn reload_compiled(&self, sources: Sources, compiled: Compiled, compile: Duration) -> Result<Reload, ScriptError> {
        let mut report = self.inner.swap(compiled).inspect_err(|e| self.inner.log(LogLevel::Error, &e.message))?;
        report.compile = compile;
        *self.inner.sources.borrow_mut() = sources;
        self.inner.note_reload(report);
        Ok(report)
    }
}

impl Inner {
    pub(crate) fn not_running(&self) -> ScriptError {
        self.failed.borrow().clone().unwrap_or_else(|| ScriptError::new(ErrorKind::Missing, "not running"))
    }

    /// Runs `f` inside the host's [`ScriptOptions::around`](crate::ScriptOptions::around) hook.
    pub(crate) fn around_write<S: 'static>(&self, s: &mut S, tx: Option<&mut Tx>, f: impl FnOnce(Option<&mut Tx>)) {
        let Some(hook) = self.options.around.clone() else { return f(tx) };
        let mut f = Some(f);
        hook(s, tx, &mut |tx| {
            if let Some(f) = f.take() {
                f(tx);
            }
        });
    }

    /// Runs `f` inside the host's [`ScriptOptions::around_read`](crate::ScriptOptions::around_read)
    /// hook; `None` if the hook didn't run it.
    pub(crate) fn around_read<S: 'static, R>(&self, s: &S, f: impl FnOnce() -> R) -> Option<R> {
        let Some(hook) = self.options.around_read.clone() else { return Some(f()) };
        let (mut f, mut out) = (Some(f), None);
        hook(s, &mut || {
            if let Some(f) = f.take() {
                out = Some(f());
            }
        });
        out
    }

    fn render_with<S: 'static>(&self, dark: bool, f: &Function, args: impl IntoLuaMulti) -> El {
        let attach = attach::<S>();
        let card = |message: &str| error_card(message, &self.options.palette, dark, attach, self.weak.clone());
        if let Some(err) = self.failed.borrow().as_ref() {
            return card(&err.message);
        }
        let Some(vm) = self.vm() else { return card("the script isn't running") };
        let env = self.env(&vm, dark);
        let built = with_frame(attach, env.clone(), || {
            self.timed(&vm, |vm| {
                vm.view.raw_set("dark", dark)?;
                let mut args = args.into_lua_multi(&vm.lua)?;
                args.push_front(Value::Table(vm.view.clone()));
                take_root(f.call::<Value>(args)?)
            })
        });
        let mut m = self.metrics.get();
        m.renders += 1;
        self.metrics.set(m);
        self.note_schedule(&vm);
        if env.memo_failed.get() {
            self.salt.set(self.salt.get().wrapping_add(1));
        }
        built.unwrap_or_else(|err| {
            let err = ScriptError::from(err);
            self.log(LogLevel::Error, &err.message);
            self.salt.set(self.salt.get().wrapping_add(1));
            card(&err.message)
        })
    }
}
