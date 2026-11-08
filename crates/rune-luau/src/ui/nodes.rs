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
        let mut style = Style::new(&frame.env);
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
                    Ok(Atom::OnHover) => match v {
                        Value::Func => style.on_hover = Some(env.reference(state, -1)),
                        Value::Nil => {}
                        other => return Err(format!("onHover must be a function, got {}", props::name(other))),
                    },
                    Ok(atom) => style.decode_at(state, atom, -1, env, is_text)?,
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
