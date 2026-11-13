//! `UiArena`: Luau builds `rune::El`s directly. `ui.row(props, children)` and the other
//! constructors are plain C functions: they read props off the Luau stack, build the `El` into
//! this render's arena, and return a light-userdata handle (an index), so a node costs no Lua
//! table, no userdata and no tree diff. Rune's keyed elements and memo cache do the incremental
//! work: `view:Memo` wraps `rune::memo::memo_with_hash`, and `ui.virtualList` is
//! `El::virtual_rows`. A node whose props table is frozen (`table.freeze`) is decoded once and
//! copied after that ([`Templates`]).

pub(crate) mod host;
pub(crate) mod nodes;
pub(crate) mod props;
pub(crate) mod rows;
pub(crate) mod view;

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::os::raw::c_int;
use std::rc::{Rc, Weak};

use mlua::ffi;
use rune::{Color, El};
use rustc_hash::FxHashSet;

use crate::script::Inner;

/// Maps a colour name to a colour, for the current light or dark appearance. Names that start
/// with `#` are hex (`#rrggbb`, `#rrggbbaa`) and never reach it.
pub type Palette = Rc<dyn Fn(&str, bool) -> Option<Color>>;

/// Rune's own `tok::theme` colours under kebab-case names: `bg`, `card`, `card-border`, `text`,
/// `text-secondary`, `separator`, `hover`, `press`, `control-off`, `indicator`.
pub fn default_palette() -> Palette {
    let themes = [rune::tok::theme(false), rune::tok::theme(true)];
    Rc::new(move |name, dark| {
        let t = &themes[usize::from(dark)];
        Some(match name {
            "bg" => t.bg,
            "card" => t.card,
            "card-border" => t.card_border,
            "text" => t.text,
            "text-secondary" => t.text_secondary,
            "separator" => t.separator,
            "hover" => t.hover,
            "press" => t.press,
            "control-off" => t.control_off,
            "control-track" => t.control_track,
            "indicator" => t.indicator,
            _ => return None,
        })
    })
}

/// A Luau registry reference. Dropping it queues the slot for release at the VM's next entry,
/// since an element can drop while its VM is busy.
pub(crate) struct LuaRef {
    pub(crate) r: c_int,
    garbage: Weak<RefCell<Vec<c_int>>>,
}

impl Drop for LuaRef {
    fn drop(&mut self) {
        if let Some(g) = self.garbage.upgrade() {
            g.borrow_mut().push(self.r);
        }
    }
}

/// A function a node handed the host: a press handler, a row builder.
pub(crate) struct Callback {
    pub(crate) func: LuaRef,
    pub(crate) vm: u64,
    pub(crate) script: Weak<Inner>,
}

/// What a render needs from its script.
pub(crate) struct Env {
    pub(crate) script: Weak<Inner>,
    pub(crate) vm: u64,
    /// This render, unique on the thread ([`next_render`]).
    pub(crate) render: u64,
    pub(crate) salt: u64,
    /// The script's version: it moves whenever script code runs.
    pub(crate) version: u64,
    pub(crate) dark: bool,
    /// A `view:Memo` build failed this render, even if the script caught the error.
    pub(crate) memo_failed: Cell<bool>,
    pub(crate) palette: Palette,
    pub(crate) shadows: Option<crate::options::Shadows>,
    /// The text style `ui.text` starts from, and the weights `weight` names.
    pub(crate) text: rune::TextStyle,
    pub(crate) weights: [f32; 4],
    pub(crate) garbage: Weak<RefCell<Vec<c_int>>>,
    pub(crate) prefix: Rc<str>,
    pub(crate) nodes: Rc<[host::HostNode]>,
    pub(crate) warned: Rc<RefCell<FxHashSet<String>>>,
    /// Colours this render resolved, by name: a render uses a handful, many times each.
    pub(crate) colors: RefCell<Vec<(Box<str>, Color)>>,
    /// The VM's nodes decoded from frozen props tables.
    pub(crate) templates: Rc<RefCell<Templates>>,
    /// While a frozen props table decodes: each colour it resolved, so its template knows
    /// when a theme change has outdated it.
    pub(crate) recording: RefCell<Option<Vec<(Box<str>, Color)>>>,
}

/// Nodes decoded from frozen props tables, by table and node kind. A frozen table can't change,
/// so a node built from one again starts from a copy of the first, its props already applied:
/// no table walk, no prop decoding, and the element's style shared until it is written. A script
/// gets this with `local STYLE = table.freeze({ ... })` passed as a node's props, or as the style
/// under a small table of its own props (`ui.row(STYLE, { key = id }, children)`): then only
/// those are decoded, onto a copy of the style's.
#[derive(Default)]
pub(crate) struct Templates(rustc_hash::FxHashMap<(usize, u8), Template>);

/// How many templates a VM keeps; past it the cache starts over.
const TEMPLATES: usize = 1024;

pub(crate) struct Template {
    /// The finished node, without children, words or handlers.
    el: El,
    /// The node before its props were finished, and those props, for props laid over it.
    under: El,
    props: props::Props,
    /// The appearance and colours it was decoded with.
    dark: bool,
    colors: Vec<(Box<str>, Color)>,
    /// The render it last held in: a render checks its colours once.
    checked: Cell<u64>,
    /// Keeps the table alive, so its address isn't taken by another table.
    _table: LuaRef,
}

/// A template as a node starts from.
pub(crate) enum Reuse {
    /// The finished node: no props of its own.
    Whole(El),
    /// The node and its pending props, to decode the node's own props onto and finish.
    Under(El, props::Props),
}

impl Templates {
    /// A copy of the template for `key`, if it holds under `env`: finished, or ready for
    /// `layered` props.
    pub(crate) fn get(&self, key: (usize, u8), env: &Env, layered: bool) -> Option<Reuse> {
        let t = self.0.get(&key)?;
        if t.checked.get() != env.render {
            let holds = t.dark == env.dark && t.colors.iter().all(|(name, c)| env.resolve(name).ok() == Some(*c));
            if !holds {
                return None;
            }
            t.checked.set(env.render);
        }
        Some(if layered { Reuse::Under(t.under.clone(), t.props.clone()) } else { Reuse::Whole(t.el.clone()) })
    }

    /// Keeps a node built from the frozen table at stack slot `idx` as `key`'s template: `under`
    /// with its pending `props`, and `el`, the two finished.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn put(&mut self, state: *mut ffi::lua_State, idx: c_int, key: (usize, u8), env: &Env, el: El, under: El, props: props::Props, colors: Vec<(Box<str>, Color)>) {
        if self.0.len() >= TEMPLATES {
            self.0.clear();
        }
        // SAFETY: the caller passes the props table's slot.
        let r = unsafe { ffi::lua_ref(state, idx) };
        let table = LuaRef { r, garbage: env.garbage.clone() };
        self.0.insert(key, Template { el, under, props, dark: env.dark, colors, checked: Cell::new(env.render), _table: table });
    }
}

/// Functions that need the window's state type, picked when a render starts.
pub(crate) struct Attach {
    /// Runs the callback on press with a `press` event naming the element's key, and any
    /// fields a host node adds (a switch's `checked`).
    pub(crate) press: fn(&mut El, Rc<Callback>, Option<Rc<str>>, Fields),
    /// Runs the callback on a middle click, with a `middle-press` event.
    pub(crate) middle: fn(El, Rc<Callback>, Option<Rc<str>>) -> El,
    /// Runs the callback as the pointer enters and leaves, with a `hover` event.
    pub(crate) hover: fn(El, Rc<Callback>, Option<Rc<str>>) -> El,
    pub(crate) rows: fn(El, rows::Spec) -> El,
    pub(crate) reload: fn(El, Weak<Inner>) -> El,
}

/// Fields a host node adds to the events its handlers get.
pub(crate) type Fields = Rc<[(&'static str, crate::Data)]>;

pub(crate) fn attach<S: 'static>() -> &'static Attach {
    const { &Attach { press: press::<S>, middle: middle::<S>, hover: hover::<S>, rows: rows::attach::<S>, reload: reload::<S> } }
}

fn press<S: 'static>(el: &mut El, cb: Rc<Callback>, key: Option<Rc<str>>, fields: Fields) {
    el.set_on_press_tx(move |s: &mut S, tx| {
        if let Some(script) = cb.script.upgrade() {
            let mods = tx.mods();
            let event = Event { fields: fields.clone(), ..Event::press(key.as_deref(), mods.shift, mods.alt, mods.cmd) };
            script.around_write(s, Some(tx), |tx| script.handle(&cb, event, tx));
        }
    });
}

/// `onMiddlePress`: a middle click on the node (`El::on_middle_press`), as a `middle-press` event
/// with the modifier keys.
fn middle<S: 'static>(el: El, cb: Rc<Callback>, key: Option<Rc<str>>) -> El {
    el.on_middle_press(move |s: &mut S, tx: &mut rune::Tx| {
        if let Some(script) = cb.script.upgrade() {
            let mods = tx.mods();
            let event = Event { kind: "middle-press", ..Event::press(key.as_deref(), mods.shift, mods.alt, mods.cmd) };
            script.around_write(s, Some(tx), |tx| script.handle(&cb, event, tx));
        }
    })
}

fn hover<S: 'static>(el: El, cb: Rc<Callback>, key: Option<Rc<str>>) -> El {
    el.on_hover_change(move |s: &mut S, on: bool, tx: &mut rune::Tx| {
        if let Some(script) = cb.script.upgrade() {
            let event = Event { kind: "hover", key: key.as_deref(), mods: None, rows: None, entered: Some(on), fields: Rc::new([]) };
            script.around_write(s, Some(tx), |tx| script.handle(&cb, event, tx));
        }
    })
}

/// A `UiEvent`: what a handler receives.
pub(crate) struct Event<'a> {
    pub(crate) kind: &'static str,
    pub(crate) key: Option<&'a str>,
    pub(crate) mods: Option<(bool, bool, bool)>,
    pub(crate) rows: Option<(usize, usize)>,
    pub(crate) entered: Option<bool>,
    pub(crate) fields: Fields,
}

impl<'a> Event<'a> {
    pub(crate) fn press(key: Option<&'a str>, shift: bool, alt: bool, cmd: bool) -> Self {
        Event { kind: "press", key, mods: Some((shift, alt, cmd)), rows: None, entered: None, fields: Rc::new([]) }
    }

    pub(crate) fn to_table(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Table> {
        let t = lua.create_table_with_capacity(0, 6 + self.fields.len())?;
        t.raw_set("kind", self.kind)?;
        if let Some(key) = self.key {
            t.raw_set("key", key)?;
        }
        if let Some((shift, alt, cmd)) = self.mods {
            t.raw_set("shift", shift)?;
            t.raw_set("alt", alt)?;
            t.raw_set("mod", cmd)?;
        }
        if let Some((first, last)) = self.rows {
            t.raw_set("first", first)?;
            t.raw_set("last", last)?;
        }
        if let Some(entered) = self.entered {
            t.raw_set("entered", entered)?;
        }
        for (name, value) in self.fields.iter() {
            t.raw_set(*name, value.clone())?;
        }
        Ok(t)
    }
}

fn reload<S: 'static>(el: El, script: Weak<Inner>) -> El {
    el.on_press_tx(move |s: &mut S, tx| {
        if let Some(script) = script.upgrade() {
            script.around_write(s, Some(tx), |tx| {
                if let Some(tx) = tx {
                    script.reload_from_card(tx);
                }
            });
        }
    })
}


/// One render's built nodes, by handle.
pub(crate) struct Arena {
    slots: Vec<Option<El>>,
    generation: u32,
}

/// One render in progress: its arena and what it builds with.
pub(crate) struct Frame {
    pub(crate) arena: Arena,
    pub(crate) attach: &'static Attach,
    pub(crate) env: Rc<Env>,
}

thread_local! {
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    static GENERATION: Cell<u32> = const { Cell::new(0) };
    static POOL: RefCell<Vec<Vec<Option<El>>>> = const { RefCell::new(Vec::new()) };
    static RENDERS: Cell<u64> = const { Cell::new(0) };
}

/// A number no earlier render on this thread had, for [`Env::render`].
pub(crate) fn next_render() -> u64 {
    let n = RENDERS.get().wrapping_add(1);
    RENDERS.set(n);
    n
}

/// Handles are `generation << 20 | slot + 1`: a handle kept from an earlier render is refused,
/// not taken for whatever sits in its slot now. 32 bits, so they fit wasm32 pointers.
const SLOT_BITS: u32 = 20;
const SLOT_MASK: usize = (1 << SLOT_BITS) - 1;

/// Runs `f` with a fresh frame on top of this thread's stack.
pub(crate) fn with_frame<R>(attach: &'static Attach, env: Rc<Env>, f: impl FnOnce() -> R) -> R {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            if let Some(frame) = FRAMES.with_borrow_mut(Vec::pop) {
                let mut slots = frame.arena.slots;
                slots.clear();
                POOL.with_borrow_mut(|p| p.push(slots));
            }
        }
    }
    let generation = GENERATION.get().wrapping_add(1) & 0xfff;
    GENERATION.set(generation);
    let slots = POOL.with_borrow_mut(Vec::pop).unwrap_or_default();
    FRAMES.with_borrow_mut(|f| f.push(Frame { arena: Arena { slots, generation }, attach, env }));
    let _pop = Pop;
    f()
}

/// Runs `f` on the innermost frame, or returns `None` outside a render.
pub(crate) fn top<R>(f: impl FnOnce(&mut Frame) -> R) -> Option<R> {
    FRAMES.with_borrow_mut(|frames| frames.last_mut().map(f))
}

impl Frame {
    pub(crate) fn put(&mut self, el: El) -> Result<*mut c_void, String> {
        self.arena.put(el)
    }

    pub(crate) fn take(&mut self, handle: *mut c_void) -> Result<El, String> {
        self.arena.take(handle)
    }
}

impl Arena {
    /// Stores `el` and returns its handle.
    pub(crate) fn put(&mut self, el: El) -> Result<*mut c_void, String> {
        let slot = self.alloc(el)?;
        Ok(self.handle(slot))
    }

    /// Stores `el` and returns its slot, where props decode onto it in place.
    pub(crate) fn alloc(&mut self, el: El) -> Result<usize, String> {
        let slot = self.slots.len();
        if slot >= SLOT_MASK {
            return Err("more than a million nodes in one render".into());
        }
        self.slots.push(Some(el));
        Ok(slot)
    }

    /// The handle a script holds for `slot`.
    pub(crate) fn handle(&self, slot: usize) -> *mut c_void {
        (((self.generation as usize) << SLOT_BITS) | (slot + 1)) as *mut c_void
    }

    /// The element `alloc` stored in `slot`.
    pub(crate) fn get_mut(&mut self, slot: usize) -> &mut El {
        self.slots[slot].as_mut().expect("a node keeps its slot until its handle is placed")
    }

    /// The element in `slot`, and the slots before it, where its children wait: a script
    /// builds a node's children before the node.
    pub(crate) fn split(&mut self, slot: usize) -> (Slots<'_>, &mut El) {
        let (before, rest) = self.slots.split_at_mut(slot);
        let el = rest[0].as_mut().expect("a node keeps its slot until its handle is placed");
        (Slots { slots: before, generation: self.generation }, el)
    }

    /// Every slot, for children a host node takes.
    pub(crate) fn all(&mut self) -> Slots<'_> {
        Slots { slots: &mut self.slots, generation: self.generation }
    }

    /// Takes the element behind `handle`; each handle gives its element once.
    pub(crate) fn take(&mut self, handle: *mut c_void) -> Result<El, String> {
        self.all().slot(handle)?.take().ok_or_else(|| "a node can only be placed once".to_owned())
    }
}

/// Arena slots that handles reach.
pub(crate) struct Slots<'a> {
    slots: &'a mut [Option<El>],
    generation: u32,
}

impl Slots<'_> {
    /// The slot behind `handle`.
    pub(crate) fn slot(&mut self, handle: *mut c_void) -> Result<&mut Option<El>, String> {
        let raw = handle as usize;
        let slot = (raw & SLOT_MASK).wrapping_sub(1);
        if (raw >> SLOT_BITS) as u32 != self.generation {
            return Err("a node from an earlier render can't be reused; build it again or use view:Memo".into());
        }
        self.slots.get_mut(slot).ok_or_else(|| "not a ui node".to_owned())
    }
}


impl Env {
    pub(crate) fn resolve(&self, name: &str) -> Result<Color, String> {
        if let Some(c) = self.cached(name) {
            self.note(name, c);
            return Ok(c);
        }
        let c = if name.starts_with('#') {
            rune::color::hex(name).ok_or_else(|| format!("{name} is not a #rrggbb or #rrggbbaa colour"))?
        } else if let Some(c) = (self.palette)(name, self.dark) {
            c
        } else {
            self.warn_once(format!("unknown colour {name}"));
            Color::TRANSPARENT
        };
        self.colors.borrow_mut().push((name.into(), c));
        self.note(name, c);
        Ok(c)
    }

    fn note(&self, name: &str, c: Color) {
        if let Some(r) = self.recording.borrow_mut().as_mut() {
            r.push((name.into(), c));
        }
    }

    fn cached(&self, name: &str) -> Option<Color> {
        self.colors.borrow().iter().find(|(n, _)| &**n == name).map(|(_, c)| *c)
    }

    pub(crate) fn color(&self, name: &str) -> Option<Color> {
        if let Some(c) = self.cached(name) {
            self.note(name, c);
            return Some(c);
        }
        let c = (self.palette)(name, self.dark)?;
        self.colors.borrow_mut().push((name.into(), c));
        self.note(name, c);
        Some(c)
    }

    pub(crate) fn text_color(&self) -> Color {
        self.color("text").unwrap_or(if self.dark { Color::WHITE } else { Color::BLACK })
    }

    /// Logs `message` once per script.
    pub(crate) fn warn_once(&self, message: String) {
        if self.warned.borrow_mut().insert(message.clone())
            && let Some(script) = self.script.upgrade()
        {
            script.log(crate::LogLevel::Warn, &message);
        }
    }

    /// A registry reference to the function at stack slot `idx`.
    pub(crate) unsafe fn reference(&self, state: *mut ffi::lua_State, idx: c_int) -> Rc<Callback> {
        // SAFETY: the caller checked slot `idx` holds a function.
        let r = unsafe { ffi::lua_ref(state, idx) };
        Rc::new(Callback {
            func: LuaRef { r, garbage: self.garbage.clone() },
            vm: self.vm,
            script: self.script.clone(),
        })
    }
}

/// Runs a C function body: its error becomes a Lua error once every Rust value it held is
/// dropped, and a Rust panic becomes an error too, where the target can catch one.
pub(crate) unsafe fn guard(state: *mut ffi::lua_State, body: impl FnOnce() -> Result<c_int, String>) -> c_int {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    let message = match result {
        Ok(Ok(n)) => return n,
        Ok(Err(message)) => message,
        Err(_) => "the host panicked while building this node".to_owned(),
    };
    // SAFETY: the message string is moved onto the Lua stack before raising.
    unsafe {
        ffi::lua_pushlstring(state, message.as_ptr().cast(), message.len());
        drop(message);
        ffi::lua_error(state)
    }
}
