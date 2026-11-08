//! The `ui.*` constructors, and host nodes an app adds to them.

use std::os::raw::c_int;
use std::rc::Rc;

use mlua::ffi;
use rune::{El, col, el, row, spacer, text};

use super::props::{self, Atom, Style, Value};
use super::{Arena, Callback, Env, Frame, guard, top};

/// The built-in constructors, as `(name, C function)`.
pub(crate) const BUILTINS: &[(&str, ffi::lua_CFunction)] = &[
    ("row", ui_row),
    ("col", ui_col),
    ("box", ui_box),
    ("text", ui_text),
    ("spacer", ui_spacer),
    ("virtualList", super::rows::ui_virtual_list),
    ("hex", ui_hex),
];

/// `ui.hex("#RRGGBB")` or `ui.hex("#RRGGBBAA")`: a literal colour. It checks the string and
/// returns it; colour props read `#` strings as hex.
unsafe extern "C-unwind" fn ui_hex(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: reads argument 1 and returns it.
    unsafe {
        let ok = matches!(props::read(state, 1), Value::Str(s) if s.starts_with('#') && matches!(s.len(), 7 | 9) && rune::color::hex(s).is_some());
        if !ok {
            return guard(state, || Err("ui.hex needs \"#RRGGBB\" or \"#RRGGBBAA\"".into()));
        }
        ffi::lua_pushvalue(state, 1);
        1
    }
}

unsafe extern "C-unwind" fn ui_row(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, row) }
}

unsafe extern "C-unwind" fn ui_col(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, col) }
}

unsafe extern "C-unwind" fn ui_box(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, el) }
}

unsafe extern "C-unwind" fn ui_spacer(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, spacer) }
}


/// What a node starts from.
enum Base {
    Box(fn() -> El),
    Text(String),
}

/// `ui.row(props?, children?)`, or `ui.row(children)`.
unsafe fn container(state: *mut ffi::lua_State, base: fn() -> El) -> c_int {
    // SAFETY: every call below reads or pushes on this C function's own stack.
    unsafe {
        let (props, children) = match (ffi::lua_type(state, 1), ffi::lua_type(state, 2)) {
            (ffi::LUA_TTABLE, ffi::LUA_TTABLE) => (1, 2),
            (ffi::LUA_TTABLE, _) if is_array(state, 1) => (0, 1),
            (ffi::LUA_TTABLE, _) => (1, 0),
            (ffi::LUA_TNIL | ffi::LUA_TNONE, ffi::LUA_TTABLE) => (0, 2),
            _ => (0, 0),
        };
        guard(state, || build(state, Base::Box(base), props, children))
    }
}

/// `ui.text(content, props?)`.
unsafe extern "C-unwind" fn ui_text(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: as for `container`.
    unsafe {
        let content = match props::read(state, 1) {
            Value::Str(s) => s.to_owned(),
            Value::Num(n) => props::format_num(n),
            other => {
                let message = format!("ui.text needs a string, got {}", props::name(other));
                return guard(state, || Err(message));
            }
        };
        let props_at = if ffi::lua_type(state, 2) == ffi::LUA_TTABLE { 2 } else { 0 };
        guard(state, move || build(state, Base::Text(content), props_at, 0))
    }
}

unsafe fn is_array(state: *mut ffi::lua_State, idx: c_int) -> bool {
    // SAFETY: `idx` is a table on this stack.
    unsafe {
        let t = ffi::lua_rawgeti(state, idx, 1);
        ffi::lua_settop(state, -2);
        t != ffi::LUA_TNIL
    }
}

/// Builds a node from the props table at `props` and the children array at `children` (0 for
/// none), stores it in the arena and pushes its handle. Props decode first and the element is
/// made last, in one run of setters, its children moving from the arena straight into it: an
/// `El` is several hundred bytes, and every hand-off between functions would copy it.
unsafe fn build(state: *mut ffi::lua_State, base: Base, props: c_int, children: c_int) -> Result<c_int, String> {
    top(|frame| {
        let is_text = matches!(base, Base::Text(_));
        let mut style = Style::default();
        // SAFETY: `props` is 0 or a table on this stack; the frame borrow covers no call back
        // into Luau.
        let press = unsafe { decode_props(state, props, &frame.env, is_text, &mut style)? };
        let el = match base {
            Base::Box(f) => f(),
            Base::Text(s) => text(s),
        };
        let Frame { arena, attach, env } = frame;
        let mut failed = None;
        // SAFETY: `children` is 0 or a table on this stack.
        let kids = (children > 0).then(|| unsafe { Children::new(state, children, arena, env, &mut failed) });
        let el = style.apply(el, env, is_text, kids, press, attach);
        if let Some(message) = failed {
            return Err(message);
        }
        let handle = arena.put(el)?;
        // SAFETY: pushes the handle as this C function's result.
        unsafe { ffi::lua_pushlightuserdata(state, handle) };
        Ok(1)
    })
    .unwrap_or_else(|| Err("ui nodes can only be built while rendering".into()))
}

/// Decodes the props table at `props` (0 for none), and the `onPress` function in it.
unsafe fn decode_props(state: *mut ffi::lua_State, props: c_int, env: &Env, is_text: bool, style: &mut Style) -> Result<Option<Rc<Callback>>, String> {
    let mut press = None;
    if props > 0 {
        // SAFETY: walks the table at `props` with `lua_rawiter`, popping each pair.
        unsafe {
            let mut iter = 0;
            while {
                iter = ffi::lua_rawiter(state, props, iter);
                iter >= 0
            } {
                let v = props::read(state, -1);
                match props::key_atom(state, -2) {
                    Ok(Atom::OnPress) => match v {
                        Value::Func => press = Some(env.reference(state, -1)),
                        Value::Nil => {}
                        other => return Err(format!("onPress must be a function, got {}", props::name(other))),
                    },
                    Ok(atom) => style.decode(atom, v, env, is_text)?,
                    Err(name) => env.warn_once(format!("unknown prop {name}")),
                }
                ffi::lua_settop(state, -3);
            }
        }
    }
    Ok(press)
}

/// Takes the elements of the children array at `idx`. Strings become text; `nil` and `false`
/// entries are skipped, so `cond and ui.text(...)` works.
pub(crate) unsafe fn take_children(state: *mut ffi::lua_State, idx: c_int, frame: &mut Frame) -> Result<Vec<El>, String> {
    let mut failed = None;
    let Frame { arena, env, .. } = frame;
    // SAFETY: as `Children` documents.
    let out: Vec<El> = unsafe { Children::new(state, idx, arena, env, &mut failed) }.collect();
    failed.map_or(Ok(out), Err)
}

/// The children in the array at stack slot `idx`, taken from the arena as they are read. The
/// first bad entry ends the walk and lands in `failed`.
pub(crate) struct Children<'a> {
    state: *mut ffi::lua_State,
    idx: c_int,
    next: usize,
    len: usize,
    arena: &'a mut Arena,
    env: &'a Env,
    failed: &'a mut Option<String>,
}

impl<'a> Children<'a> {
    /// SAFETY: `idx` is a table on `state`'s stack, which the walk leaves as it found it.
    unsafe fn new(state: *mut ffi::lua_State, idx: c_int, arena: &'a mut Arena, env: &'a Env, failed: &'a mut Option<String>) -> Self {
        // SAFETY: as this function's contract says.
        let len = unsafe { ffi::lua_objlen(state, idx) };
        Children { state, idx, next: 1, len, arena, env, failed }
    }
}

impl Iterator for Children<'_> {
    type Item = El;

    fn next(&mut self) -> Option<El> {
        while self.next <= self.len && self.failed.is_none() {
            let i = self.next;
            self.next += 1;
            let state = self.state;
            // SAFETY: reads one array slot, then pops it below.
            let kind = unsafe { ffi::lua_rawgeti(state, self.idx, i as ffi::lua_Integer) };
            let child = match kind {
                ffi::LUA_TLIGHTUSERDATA => {
                    // SAFETY: the slot holds a light userdata.
                    let handle = unsafe { ffi::lua_tolightuserdata(state, -1) };
                    match self.arena.slot(handle) {
                        // Moved straight out of its slot into the parent's list.
                        Ok(slot) if slot.is_some() => slot.take(),
                        Ok(_) => {
                            *self.failed = Some("a node can only be placed once".into());
                            None
                        }
                        Err(message) => {
                            *self.failed = Some(message);
                            None
                        }
                    }
                }
                ffi::LUA_TSTRING | ffi::LUA_TNUMBER => {
                    // SAFETY: reads the slot pushed above.
                    let s = match unsafe { props::read(state, -1) } {
                        Value::Str(s) => s.to_owned(),
                        Value::Num(n) => props::format_num(n),
                        _ => String::new(),
                    };
                    Some(text(s).color(self.env.text_color()))
                }
                ffi::LUA_TNIL => None,
                // SAFETY: reads the slot pushed above.
                ffi::LUA_TBOOLEAN if unsafe { ffi::lua_toboolean(state, -1) } == 0 => None,
                _ => {
                    // SAFETY: reads the slot pushed above.
                    let what = props::name(unsafe { props::read(state, -1) });
                    *self.failed = Some(format!("child {i} is a {what}, not a ui node"));
                    None
                }
            };
            // SAFETY: pops the slot pushed above.
            unsafe { ffi::lua_settop(state, -2) };
            if child.is_some() {
                return child;
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Holes (`nil`, `false`) are rare; counting every slot reserves the list once.
        let left = self.len + 1 - self.next;
        (left, Some(left))
    }
}

type BuildNode = dyn Fn(&mut NodeArgs) -> Result<El, String>;

/// A node an app adds to `ui`: `ui.<name>(props, children)` calls `build` with the props as
/// [`NodeArgs`]. Use it for native views a script places (a code view, a terminal).
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

/// A host node's props and children.
pub struct NodeArgs {
    props: Vec<(String, Prop)>,
    pub children: Vec<El>,
    attach: &'static super::Attach,
}

/// One prop a host node received.
#[derive(Clone)]
pub enum Prop {
    Bool(bool),
    Number(f64),
    String(String),
    /// A function; [`NodeArgs::on_press`] attaches it.
    Function(Handler),
}

/// A script function a host node can run on press.
#[derive(Clone)]
pub struct Handler(Rc<Callback>);

impl NodeArgs {
    pub fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|(k, _)| k == name).map(|(_, v)| v)
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

    /// Runs the script function in prop `name` when `el` is pressed.
    pub fn on_press(&self, el: El, name: &str) -> El {
        match self.get(name) {
            Some(Prop::Function(h)) => (self.attach.press)(el, h.0.clone(), self.string("key").map(Into::into)),
            _ => el,
        }
    }
}

/// `ui.<host node>(props, children)`: upvalue 1 is the node's index in the script's host nodes.
pub(crate) unsafe extern "C-unwind" fn ui_host(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: reads this C function's arguments and upvalue.
    unsafe {
        let index = ffi::lua_tointegerx(state, ffi::lua_upvalueindex(1), std::ptr::null_mut()) as usize;
        let props_at = if ffi::lua_type(state, 1) == ffi::LUA_TTABLE { 1 } else { 0 };
        let children_at = if ffi::lua_type(state, 2) == ffi::LUA_TTABLE { 2 } else { 0 };
        guard(state, || {
            let args = top(|frame| -> Result<NodeArgs, String> {
                let props = if props_at > 0 { host_props(state, props_at, frame)? } else { Vec::new() };
                let children = if children_at > 0 { take_children(state, children_at, frame)? } else { Vec::new() };
                Ok(NodeArgs { props, children, attach: frame.attach })
            });
            let mut args = args.ok_or("ui nodes can only be built while rendering")??;
            let node = top(|frame| frame.env.nodes.get(index).cloned()).flatten().ok_or("unknown host node")?;
            let el = (node.build)(&mut args).map_err(|e| format!("ui.{}: {e}", node.name))?;
            // Standard layout props still apply, after the host's own.
            build_from(state, el, props_at)
        })
    }
}

unsafe fn build_from(state: *mut ffi::lua_State, el: El, props_at: c_int) -> Result<c_int, String> {
    top(|frame| {
        let mut style = Style::default();
        if props_at > 0 {
            // SAFETY: walks the props table again, decoding only the standard props.
            unsafe {
                let mut iter = 0;
                while {
                    iter = ffi::lua_rawiter(state, props_at, iter);
                    iter >= 0
                } {
                    if let Ok(atom) = props::key_atom(state, -2)
                        && atom != Atom::OnPress
                    {
                        style.decode(atom, props::read(state, -1), &frame.env, false)?;
                    }
                    ffi::lua_settop(state, -3);
                }
            }
        }
        let handle = frame.put(style.apply(el, &frame.env, false, None, None, frame.attach))?;
        // SAFETY: pushes the handle as this C function's result.
        unsafe { ffi::lua_pushlightuserdata(state, handle) };
        Ok(1)
    })
    .unwrap_or_else(|| Err("ui nodes can only be built while rendering".into()))
}
unsafe fn host_props(state: *mut ffi::lua_State, idx: c_int, frame: &Frame) -> Result<Vec<(String, Prop)>, String> {
    let mut out = Vec::new();
    // SAFETY: walks the table at `idx`, popping each value.
    unsafe {
        let mut iter = 0;
        while {
            iter = ffi::lua_rawiter(state, idx, iter);
            iter >= 0
        } {
            let key = match props::read(state, -2) {
                Value::Str(s) => Some(s.to_owned()),
                _ => None,
            };
            let value = match props::read(state, -1) {
                Value::Bool(b) => Some(Prop::Bool(b)),
                Value::Num(n) => Some(Prop::Number(n)),
                Value::Str(s) => Some(Prop::String(s.to_owned())),
                Value::Func => Some(Prop::Function(Handler(frame.env.reference(state, -1)))),
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
