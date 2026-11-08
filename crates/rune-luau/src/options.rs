//! [`ScriptOptions`]: how a host runs its scripts.

use std::any::Any;
use std::rc::Rc;
use std::time::Duration;

use mlua::{Lua, Table};
use rune::{Shadow, TextStyle, Tx};

use crate::script::LogLevel;
use crate::ui::host::HostNode;
use crate::ui::{Palette, default_palette};
use crate::vm::Host;

/// Receives log lines as they happen.
pub(crate) type LogFn = dyn Fn(LogLevel, &str);

/// Binds a host's API into a new VM.
pub(crate) type SetupFn = dyn Fn(&Host<'_>) -> mlua::Result<()>;

/// Runs once a new VM's entry module has returned.
pub(crate) type StartFn = dyn Fn(&Lua, &Table, u64) -> mlua::Result<()>;

/// Wraps an entry into the VM that comes from the UI (a press, a hover, a list's visible rows)
/// with the app's state and the transaction, when the UI hands one over.
pub(crate) type AroundFn = dyn Fn(&mut dyn Any, Option<&mut Tx>, &mut dyn FnMut(Option<&mut Tx>));

/// Wraps an entry that only reads the app's state: a virtual list building a row.
pub(crate) type AroundReadFn = dyn Fn(&dyn Any, &mut dyn FnMut());

/// Maps an elevation name (`raised`, `floating`, `overlay`) to its shadows, for the current light
/// or dark appearance.
pub type Shadows = Rc<dyn Fn(&str, bool) -> Option<Vec<Shadow>>>;

/// How a script is hosted. Build with [`ScriptOptions::new`] and its setters.
#[derive(Clone)]
pub struct ScriptOptions {
    pub(crate) palette: Palette,
    pub(crate) shadows: Option<Shadows>,
    pub(crate) setup: Option<Rc<SetupFn>>,
    pub(crate) start: Option<Rc<StartFn>>,
    pub(crate) around: Option<Rc<AroundFn>>,
    pub(crate) around_read: Option<Rc<AroundReadFn>>,
    pub(crate) nodes: Vec<HostNode>,
    pub(crate) time_limit: Duration,
    pub(crate) memory_limit: usize,
    pub(crate) on_log: Option<Rc<LogFn>>,
    pub(crate) text: TextStyle,
    /// `weight = "regular"`, `"medium"`, `"semibold"` and `"bold"`.
    pub(crate) weights: [f32; 4],
}

impl Default for ScriptOptions {
    fn default() -> Self {
        ScriptOptions {
            palette: default_palette(),
            shadows: None,
            setup: None,
            start: None,
            around: None,
            around_read: None,
            nodes: Vec::new(),
            time_limit: Duration::from_millis(250),
            memory_limit: 64 << 20,
            on_log: None,
            text: TextStyle::new(13.0, 400.0),
            weights: [400.0, 500.0, 600.0, 700.0],
        }
    }
}

impl ScriptOptions {
    pub fn new() -> Self {
        ScriptOptions::default()
    }

    /// Colour names scripts may use (`fill = "card"`); [`default_palette`] otherwise.
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }

    /// What `shadow = "<name>"` draws. Without it the prop draws nothing.
    pub fn shadows(mut self, shadows: Shadows) -> Self {
        self.shadows = Some(shadows);
        self
    }

    /// Binds the host's modules, globals, signals and async functions into each new VM.
    pub fn setup(mut self, setup: impl Fn(&Host<'_>) -> mlua::Result<()> + 'static) -> Self {
        self.setup = Some(Rc::new(setup));
        self
    }

    /// Runs with each new VM, the table its entry module returned and the VM's id (what
    /// [Script::vm_id](crate::Script::vm_id) returns once it runs), after the module ran and
    /// before `loadHotState`: a host that calls a setup function the module hands it does it
    /// here, so hot state lands on what that setup made. An error fails the start, and a reload
    /// keeps the running VM.
    pub fn start(mut self, start: impl Fn(&Lua, &Table, u64) -> mlua::Result<()> + 'static) -> Self {
        self.start = Some(Rc::new(start));
        self
    }

    /// Wraps every entry the UI makes (a press, `onHover`, a list's `onVisibleRows`): the hook
    /// gets the app's state and the transaction, and must call `run` once. A host whose
    /// bindings reach its own state sets it up around `run`, then carries out what the script
    /// asked for in the same transaction.
    pub fn around(mut self, around: impl Fn(&mut dyn Any, Option<&mut Tx>, &mut dyn FnMut(Option<&mut Tx>)) + 'static) -> Self {
        self.around = Some(Rc::new(around));
        self
    }

    /// Wraps the entries the UI makes while it only reads the app's state: a virtual list's
    /// rows and row keys. The hook must call `run` once.
    pub fn around_read(mut self, around: impl Fn(&dyn Any, &mut dyn FnMut()) + 'static) -> Self {
        self.around_read = Some(Rc::new(around));
        self
    }

    /// Adds `ui.<name>`, a node the host builds.
    pub fn node(mut self, node: HostNode) -> Self {
        self.nodes.push(node);
        self
    }

    /// The text style `ui.text` starts from (13 pt, weight 400), and the weights `regular`,
    /// `medium`, `semibold` and `bold` name (400, 500, 600, 700): an app whose type runs on a
    /// variable font names its own.
    pub fn text(mut self, base: TextStyle, weights: [f32; 4]) -> Self {
        self.text = base;
        self.weights = weights;
        self
    }

    /// The longest one entry into the VM may run before the watchdog stops it (250 ms).
    pub fn time_limit(mut self, limit: Duration) -> Self {
        self.time_limit = limit;
        self
    }

    /// The VM's heap limit in bytes (64 MB).
    pub fn memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// Receives `print`, `warn` and errors as they happen, besides [`Script::log`](crate::Script::log).
    pub fn on_log(mut self, f: impl Fn(LogLevel, &str) + 'static) -> Self {
        self.on_log = Some(Rc::new(f));
        self
    }
}
