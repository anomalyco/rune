//! One Luau VM: a script's sandboxed globals (`task`, `Signal`, `require`), its `@rune/ui`
//! module, the host's own bindings, and the module its `init.luau` returned.

use std::cell::RefCell;
use std::os::raw::c_int;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::chunk::ChunkMode;
use mlua::{Function, IntoLua, Lua, MultiValue, Table, Value, ffi};
use rustc_hash::FxHashMap;

use crate::require::{Loading, Requirer, module_loader};
use crate::sources::{Compiled, compiler};
use crate::task::{self, Sched};
use crate::ui::host::{HostNode, ui_host};
use crate::ui::nodes::BUILTINS;
use crate::ui::props::useratom;
use crate::ui::view::{view_handler, view_memo};
use crate::workers::Shared;
use crate::{Completer, LogLevel, ScriptError};

const PRELUDE: &str = include_str!("prelude.luau");

/// The prelude, compiled once per thread.
fn prelude_bytecode() -> mlua::Result<Rc<[u8]>> {
    thread_local! {
        static CODE: RefCell<Option<Rc<[u8]>>> = const { RefCell::new(None) };
    }
    if let Some(code) = CODE.with_borrow(Clone::clone) {
        return Ok(code);
    }
    let code: Rc<[u8]> = compiler().compile(PRELUDE)?.into();
    CODE.set(Some(code.clone()));
    Ok(code)
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Vm {
    pub(crate) lua: Lua,
    /// Unique across every VM this process made: callbacks from an earlier VM are refused.
    pub(crate) id: u64,
    pub(crate) module: Table,
    pub(crate) view: Table,
    pub(crate) garbage: Rc<RefCell<Vec<c_int>>>,
    /// Nodes decoded from frozen props tables, reused while this VM runs.
    pub(crate) templates: Rc<RefCell<crate::ui::Templates>>,
    signals: Rc<RefCell<FxHashMap<String, Value>>>,
    fire: Function,
}

/// What a host binds into each new VM, in [`ScriptOptions::setup`](crate::ScriptOptions::setup).
/// It runs again for each new VM: in every reload, or ahead of one ([`Script::prewarm`](crate::Script::prewarm)).
pub struct Host<'a> {
    lua: &'a Lua,
    shared: &'a Arc<Shared>,
    signals: &'a Rc<RefCell<FxHashMap<String, Value>>>,
    prelude: &'a Table,
}

impl Host<'_> {
    pub fn lua(&self) -> &Lua {
        self.lua
    }

    /// Makes `require("@name")` return `value` (`@app/sessions`, say).
    pub fn register_module(&self, name: &str, value: impl IntoLua) -> mlua::Result<()> {
        self.lua.register_module(name, value)
    }

    /// Sets global `name` before the sandbox freezes the globals.
    pub fn global(&self, name: &str, value: impl IntoLua) -> mlua::Result<()> {
        self.lua.globals().raw_set(name, value)
    }

    /// A `Signal` (Connect, Once, Wait) the host fires with
    /// [`Script::fire`](crate::Script::fire) as `name`. Expose it to the script in a module.
    pub fn signal(&self, name: &str) -> mlua::Result<Value> {
        let signal: Value = self.prelude.get::<Function>("signal")?.call(())?;
        self.signals.borrow_mut().insert(name.to_owned(), signal.clone());
        Ok(signal)
    }

    /// A new `ScriptSignal`, which the script fires itself (Fire, Version), for a host's
    /// `CreateSignal`. Scripts also get one from `require("@rune/signal").new()`.
    pub fn script_signal(&self) -> mlua::Result<Function> {
        self.prelude.get::<Function>("scriptSignal")
    }

    /// A function a task calls like a plain one, which `start` finishes later: `start` gets
    /// the arguments and a [`Completer`], and the task resumes with what the completer is given.
    /// Called outside a task (while rendering), it raises an error.
    pub fn async_function(
        &self,
        name: &str,
        start: impl Fn(&Lua, MultiValue, Completer) -> mlua::Result<()> + 'static,
    ) -> mlua::Result<Function> {
        let shared = self.shared.clone();
        let submit = self.lua.create_function(move |lua, args: MultiValue| {
            let done = shared.job();
            let id = done.id;
            start(lua, args, done)?;
            Ok(id)
        })?;
        self.prelude.get::<Function>("async")?.call((submit, name))
    }
}

pub(crate) struct Setup<'a> {
    pub(crate) shared: &'a Arc<Shared>,
    pub(crate) memory_limit: usize,
    pub(crate) nodes: &'a [HostNode],
    pub(crate) host: Option<&'a crate::options::SetupFn>,
    pub(crate) log: Rc<dyn Fn(LogLevel, String)>,
}

/// A VM before its script: the Luau state with the prelude, `@rune/ui` and the host's bindings,
/// none of which depend on the script's files, so it can be built ahead of a reload
/// ([`Script::prewarm`](crate::Script::prewarm)).
pub(crate) struct Base {
    lua: Lua,
    prelude: Table,
    view: Table,
    signals: Rc<RefCell<FxHashMap<String, Value>>>,
}

impl Base {
    pub(crate) fn new(setup: Setup<'_>) -> Result<Base, ScriptError> {
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        crate::web::init();
        let lua = Lua::new();
        // SAFETY: the callback table lives as long as the VM; `useratom` is a plain function.
        unsafe { lua.exec_raw::<()>((), |state| (*ffi::lua_callbacks(state)).useratom = Some(useratom))? };
        lua.set_memory_limit(setup.memory_limit)?;
        lua.set_compiler(compiler());
        crate::watchdog::install(&lua)?;
        lua.set_app_data(Sched::new(setup.shared.clone(), 0.0));

        let (task, forget) = task::install(&lua)?;
        let (wait, park, job) = task::tokens();
        #[cfg(luau_jit)]
        lua.enable_jit(false);
        let prelude: Table = lua
            .load(&*prelude_bytecode()?)
            .set_mode(ChunkMode::Binary)
            .set_name("=rune-luau")
            .call((task.clone(), wait, park, job, forget))?;
        let globals = lua.globals();
        globals.raw_set("task", task)?;
        let signal = lua.create_table()?;
        signal.raw_set("new", prelude.get::<Function>("scriptSignal")?)?;
        lua.register_module("@rune/signal", signal)?;
        for (name, level) in [("print", LogLevel::Info), ("warn", LogLevel::Warn)] {
            let log = setup.log.clone();
            let f = lua.create_function(move |_, args: MultiValue| {
                let parts: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| v.type_name().to_owned())).collect();
                log(level, parts.join("\t"));
                Ok(())
            })?;
            globals.raw_set(name, f)?;
        }

        let ui = lua.create_table()?;
        for (name, f) in BUILTINS {
            // SAFETY: each builtin follows the C function protocol and raises only through `guard`.
            ui.raw_set(*name, unsafe { lua.create_c_function(*f)? })?;
        }
        for (i, node) in setup.nodes.iter().enumerate() {
            // SAFETY: pushes `ui_host` with the node's index as its one upvalue.
            let f: Function = unsafe {
                lua.exec_raw((), |state| {
                    ffi::lua_pushinteger(state, i as ffi::lua_Integer);
                    ffi::lua_pushcclosurek(state, ui_host, c"ui.host".as_ptr(), 1, None);
                })?
            };
            ui.raw_set(node.name.as_str(), f)?;
        }
        lua.register_module("@rune/ui", &ui)?;
        let view = lua.create_table()?;
        // SAFETY: as for the builtins.
        unsafe {
            view.raw_set("Memo", lua.create_c_function(view_memo)?)?;
            view.raw_set("Handler", lua.create_c_function(view_handler)?)?;
        }
        view.raw_set("dark", false)?;

        let signals = Rc::new(RefCell::new(FxHashMap::default()));
        if let Some(host) = setup.host {
            host(&Host { lua: &lua, shared: setup.shared, signals: &signals, prelude: &prelude })?;
        }
        Ok(Base { lua, prelude, view, signals })
    }
}

impl Vm {
    /// A new VM from `base` running `compiled`, sandboxed, before its entry module runs; its
    /// task clock starts at `clock`.
    pub(crate) fn new(base: Base, compiled: &Rc<Compiled>, clock: f64) -> Result<(Vm, Function), ScriptError> {
        let Base { lua, prelude, view, signals } = base;
        if let Some(mut sched) = lua.app_data_mut::<Sched>() {
            sched.set_clock(clock);
        }
        let loading: Loading = Rc::default();
        lua.globals().raw_set("require", lua.create_require_function(Requirer::new(compiled.clone(), loading.clone()))?)?;
        lua.sandbox(true)?;
        let entry = module_loader(&lua, compiled, &loading, &compiled.entry)?;
        let fire = prelude.get::<Function>("fire")?;
        let vm = Vm {
            module: lua.create_table()?,
            lua,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            view,
            garbage: Rc::default(),
            templates: Rc::default(),
            signals,
            fire,
        };
        Ok((vm, entry))
    }

    /// Takes what `init.luau` returned: a table of functions, or one function (`render`).
    pub(crate) fn set_module(&mut self, value: Value) -> Result<(), ScriptError> {
        self.module = match value {
            Value::Table(t) => t,
            Value::Function(f) => {
                let t = self.lua.create_table()?;
                t.raw_set("render", f)?;
                t
            }
            other => {
                return Err(ScriptError::new(
                    crate::ErrorKind::Missing,
                    format!("the entry module must return a table of functions or a render function, not a {}", other.type_name()),
                ));
            }
        };
        Ok(())
    }

    /// The exported function `name`, if the module has one.
    pub(crate) fn export(&self, name: &str) -> Option<Function> {
        self.module.raw_get::<Option<Function>>(name).ok().flatten()
    }

    /// The function behind registry reference `r`.
    pub(crate) fn function(&self, r: c_int) -> mlua::Result<Function> {
        // SAFETY: pushes the registry slot `r`, which this VM's `lua_ref` filled.
        unsafe { self.lua.exec_raw((), |state| { ffi::lua_rawgeti(state, ffi::LUA_REGISTRYINDEX, r as ffi::lua_Integer); }) }
    }

    /// Releases registry slots of elements dropped since the last entry.
    pub(crate) fn collect(&self) {
        let refs = std::mem::take(&mut *self.garbage.borrow_mut());
        if refs.is_empty() {
            return;
        }
        // SAFETY: each slot came from this VM's `lua_ref` and is released once. Releasing
        // can't fail short of memory, and then the slots only stay taken.
        let released: mlua::Result<()> = unsafe {
            self.lua.exec_raw::<()>((), |state| {
                for r in &refs {
                    ffi::lua_unref(state, *r);
                }
            })
        };
        released.ok();
    }

    /// Fires the host signal `name` with `args`; false when there is no such signal.
    pub(crate) fn fire(&self, name: &str, args: Vec<crate::Data>) -> mlua::Result<bool> {
        let Some(signal) = self.signals.borrow().get(name).cloned() else { return Ok(false) };
        let mut values = MultiValue::new();
        values.push_back(signal);
        for a in args {
            values.push_back(a.into_lua(&self.lua)?);
        }
        self.fire.call::<()>(values)?;
        Ok(true)
    }
}
