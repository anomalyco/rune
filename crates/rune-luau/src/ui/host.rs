//! Nodes an app adds to `ui`: `ui.<name>(props?, children?)`, `ui.<name>(content, props?)` or
//! `ui.<name>(props, node)`, built by the app from [`NodeArgs`]; a second props table after the
//! first lays its props over it (`ui.icon("copy", STYLE, { color = ink })`). The standard layout
//! props (`w`, `pad`, `key`, `label`, ...) apply after the app's build, except the ones it read.

use std::cell::Cell;
use std::os::raw::c_int;
use std::rc::Rc;

use mlua::ffi;
use rune::{Color, El};

use super::nodes::{keyed, take_children};
use super::props::{self, Atom, Props, Value};
use super::{Callback, Env, Frame, guard, top};
use crate::Data;

type BuildNode = dyn Fn(&mut NodeArgs) -> Result<El, String>;

/// A node an app adds to `ui`: `ui.<name>(props, children)` calls `build` with the props as
/// [`NodeArgs`]. Use it for native views a script places (a code view, a terminal) and for the
/// app's own controls (a button, a switch).
#[derive(Clone)]
pub struct HostNode {
    pub(crate) name: String,
    pub(crate) build: Rc<BuildNode>,
}

impl HostNode {
    pub fn new(name: impl Into<String>, build: impl Fn(&mut NodeArgs) -> Result<El, String> + 'static) -> Self {
        HostNode { name: name.into(), build: Rc::new(build) }
    }
}

/// A host node's arguments: a leading string (`ui.icon("copy", props)`), its props and its
/// children (an array, or one node: `ui.settingsRow(props, control)`).
pub struct NodeArgs {
    content: Option<String>,
    props: Vec<(Key, Prop)>,
    pub children: Vec<El>,
    attach: &'static super::Attach,
    env: Rc<Env>,
    /// Props the build read, by index in `props` (past 64 all count as read): the standard pass
    /// leaves them alone.
    used: Cell<u64>,
}

/// A prop's name: the bytes of the key string in the props table. The table sits on the Luau
/// stack for the whole host node call, which is a `NodeArgs`'s whole life, and keeps it alive.
struct Key(*const u8, usize);

impl Key {
    fn as_str(&self) -> &str {
        // SAFETY: as the type says; `host_props` only makes keys of UTF-8 strings.
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.0, self.1)) }
    }
}

/// One prop a host node received.
#[derive(Clone)]
pub enum Prop {
    Bool(bool),
    Number(f64),
    String(String),
    /// A table, as plain data.
    Data(Data),
    /// A function; [`NodeArgs::on_press`] attaches it.
    Function(Handler),
}

/// A script function a host node can run on press.
#[derive(Clone)]
pub struct Handler(Rc<Callback>);

/// A script function a host node keeps past its build ([`NodeArgs::press_later`]): each element
/// it attaches to runs it on press, with that element's own event fields.
#[derive(Clone)]
pub struct Press {
    cb: Rc<Callback>,
    attach: &'static super::Attach,
    key: Option<Rc<str>>,
}

impl Press {
    /// Runs the function when `el` is pressed, with `fields` added to its `press` event.
    pub fn attach(&self, el: El, fields: Vec<(&'static str, Data)>) -> El {
        let mut el = el;
        (self.attach.press)(&mut el, self.cb.clone(), self.key.clone(), fields.into());
        el
    }
}

impl NodeArgs {
    /// The node's key, which the standard pass applies: reading it here leaves it there.
    fn key(&self) -> Option<Rc<str>> {
        self.props.iter().rev().find_map(|(k, v)| match v {
            Prop::String(s) if k.as_str() == "key" => Some(s.as_str().into()),
            _ => None,
        })
    }

    /// The leading string argument, if the node was called with one.
    pub fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }

    /// Prop `name`: from the props laid over a style, else from the style.
    pub fn get(&self, name: &str) -> Option<&Prop> {
        let i = self.props.iter().rposition(|(k, _)| k.as_str() == name)?;
        if i < 64 {
            self.used.set(self.used.get() | 1 << i);
        }
        Some(&self.props[i].1)
    }

    /// The names of the props the build read.
    fn read(&self) -> impl Iterator<Item = &str> {
        let used = self.used.get();
        self.props.iter().enumerate().filter(move |(i, _)| *i >= 64 || used & (1 << i) != 0).map(|(_, (k, _))| k.as_str())
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        match self.get(name) {
            Some(Prop::Number(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Prop::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        matches!(self.get(name), Some(Prop::Bool(true)))
    }

    /// A table prop as plain data (a string or number prop too).
    pub fn data(&self, name: &str) -> Option<Data> {
        match self.get(name)? {
            Prop::Data(d) => Some(d.clone()),
            Prop::String(s) => Some(Data::String(s.clone())),
            Prop::Number(n) => Some(Data::Number(*n)),
            Prop::Bool(b) => Some(Data::Bool(*b)),
            Prop::Function(_) => None,
        }
    }

    /// The colour prop `name` names (a palette name or `ui.hex`), resolved for this render.
    pub fn color(&self, name: &str) -> Option<Color> {
        self.string(name).and_then(|c| self.env.resolve(c).ok())
    }

    /// Palette colour `name`, resolved for this render.
    pub fn palette(&self, name: &str) -> Option<Color> {
        self.env.color(name)
    }

    /// Whether the render is for the dark appearance.
    pub fn dark(&self) -> bool {
        self.env.dark
    }

    /// The script function in prop `name`.
    pub fn handler(&self, name: &str) -> Option<Handler> {
        match self.get(name) {
            Some(Prop::Function(h)) => Some(h.clone()),
            _ => None,
        }
    }

    /// Runs the script function in prop `name` when `el` is pressed.
    pub fn on_press(&self, el: El, name: &str) -> El {
        self.on_press_with(el, name, Vec::new())
    }

    /// Runs the script function in prop `name` when `el` is pressed, with `fields` added to its
    /// event (`checked` for a switch).
    pub fn on_press_with(&self, el: El, name: &str, fields: Vec<(&'static str, Data)>) -> El {
        let key = self.key();
        match self.handler(name) {
            Some(h) => {
                let mut el = el;
                (self.attach.press)(&mut el, h.0, key, fields.into());
                el
            }
            None => el,
        }
    }

    /// The script function in prop `name`, kept to run on a press of elements the node builds
    /// later, such as the rows of a virtual list it makes.
    pub fn press_later(&self, name: &str) -> Option<Press> {
        let key = self.key();
        self.handler(name).map(|h| Press { cb: h.0, attach: self.attach, key })
    }

    /// Runs the script function in prop `name` as the pointer enters and leaves `el`.
    pub fn on_hover(&self, el: El, name: &str) -> El {
        let key = self.key();
        match self.handler(name) {
            Some(h) => (self.attach.hover)(el, h.0, key),
            None => el,
        }
    }
}

/// `ui.<host node>(...)`: upvalue 1 is the node's index in the script's host nodes.
pub(crate) unsafe extern "C-unwind" fn ui_host(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: reads this C function's arguments and upvalue.
    unsafe {
        let index = ffi::lua_tointegerx(state, ffi::lua_upvalueindex(1), std::ptr::null_mut()) as usize;
        let lead = ffi::lua_type(state, 1) == ffi::LUA_TSTRING;
        let content = lead.then(|| match props::read(state, 1) {
            Value::Str(s) => s.to_owned(),
            _ => String::new(),
        });
        let first = if lead { 2 } else { 1 };
        let props_at = if ffi::lua_type(state, first) == ffi::LUA_TTABLE { first } else { 0 };
        // Props laid over a style: `ui.icon("copy", STYLE, { color = ink })`.
        let over_at = if props_at > 0 && ffi::lua_type(state, first + 1) == ffi::LUA_TTABLE && keyed(state, first + 1) { first + 1 } else { 0 };
        let next = if over_at > 0 { over_at + 1 } else { first + 1 };
        let children_at = match ffi::lua_type(state, next) {
            ffi::LUA_TTABLE | ffi::LUA_TLIGHTUSERDATA => next,
            _ => 0,
        };
        guard(state, || {
            let args = top(|frame| -> Result<NodeArgs, String> {
                let mut props = if props_at > 0 { host_props(state, props_at, frame)? } else { Vec::new() };
                if over_at > 0 {
                    props.extend(host_props(state, over_at, frame)?);
                }
                let children = match children_at {
                    0 => Vec::new(),
                    at if ffi::lua_type(state, at) == ffi::LUA_TLIGHTUSERDATA => vec![frame.take(ffi::lua_tolightuserdata(state, at))?],
                    at => take_children(state, at, frame)?,
                };
                Ok(NodeArgs { content, props, children, attach: frame.attach, env: frame.env.clone(), used: Cell::new(0) })
            });
            let mut args = args.ok_or("ui nodes can only be built while rendering")??;
            let node = top(|frame| frame.env.nodes.get(index).cloned()).flatten().ok_or("unknown host node")?;
            let el = (node.build)(&mut args).map_err(|e| format!("ui.{}: {e}", node.name))?;
            // Standard layout props still apply, after the host's own.
            build_from(state, el, [props_at, over_at], &args)
        })
    }
}

unsafe fn build_from(state: *mut ffi::lua_State, el: El, tables: [c_int; 2], args: &NodeArgs) -> Result<c_int, String> {
    top(|frame| {
        let Frame { arena, attach, env } = frame;
        let slot = arena.alloc(el)?;
        let mut p = Props::new(env);
        for props_at in tables.into_iter().filter(|&at| at > 0) {
            let el = arena.get_mut(slot);
            // SAFETY: walks the props table again, decoding the standard props the build left.
            unsafe {
                let mut iter = 0;
                while {
                    iter = ffi::lua_rawiter(state, props_at, iter);
                    iter >= 0
                } {
                    let own = matches!(props::read(state, -2), Value::Str(k) if args.read().any(|u| u == k));
                    match props::key_atom(state, -2) {
                        Ok(Atom::OnPress | Atom::OnHover) => {}
                        Ok(atom) if !own => p.decode_at(el, state, atom, -1, env, false)?,
                        _ => {}
                    }
                    ffi::lua_settop(state, -3);
                }
            }
        }
        p.finish(arena.get_mut(slot), env, false, None, *attach);
        let handle = arena.handle(slot);
        // SAFETY: pushes the handle as this C function's result.
        unsafe { ffi::lua_pushlightuserdata(state, handle) };
        Ok(1)
    })
    .unwrap_or_else(|| Err("ui nodes can only be built while rendering".into()))
}

unsafe fn host_props(state: *mut ffi::lua_State, idx: c_int, frame: &Frame) -> Result<Vec<(Key, Prop)>, String> {
    let mut out = Vec::new();
    // SAFETY: walks the table at `idx`, popping each value.
    unsafe {
        let mut iter = 0;
        while {
            iter = ffi::lua_rawiter(state, idx, iter);
            iter >= 0
        } {
            let key = match props::read(state, -2) {
                Value::Str(s) => Some(Key(s.as_ptr(), s.len())),
                _ => None,
            };
            let value = match props::read(state, -1) {
                Value::Bool(b) => Some(Prop::Bool(b)),
                Value::Num(n) => Some(Prop::Number(n)),
                Value::Str(s) => Some(Prop::String(s.to_owned())),
                Value::Func => Some(Prop::Function(Handler(frame.env.reference(state, -1)))),
                Value::Other("table") => Some(Prop::Data(crate::data::from_stack(state, -1))),
                Value::Nil | Value::Other(_) => None,
            };
            if let (Some(k), Some(v)) = (key, value) {
                out.push((k, v));
            }
            ffi::lua_settop(state, -3);
        }
    }
    Ok(out)
}
