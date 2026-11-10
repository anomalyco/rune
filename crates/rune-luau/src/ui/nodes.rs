//! The `ui.*` constructors, and host nodes an app adds to them.

use std::os::raw::c_int;
use std::rc::Rc;

use mlua::ffi;
use rune::{El, col, el, row, spacer, text};

use super::props::{self, Atom, Props, Value};
use super::{Callback, Env, Frame, Reuse, Slots, guard, top};

/// The built-in constructors, as `(name, C function)`.
pub(crate) const BUILTINS: &[(&str, ffi::lua_CFunction)] = &[
    ("row", ui_row),
    ("col", ui_col),
    ("box", ui_box),
    ("text", ui_text),
    ("spacer", ui_spacer),
    ("virtualList", super::rows::ui_virtual_list),
    ("each", super::view::ui_each),
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
    unsafe { container(state, row, 0) }
}

unsafe extern "C-unwind" fn ui_col(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, col, 1) }
}

unsafe extern "C-unwind" fn ui_box(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, el, 2) }
}

unsafe extern "C-unwind" fn ui_spacer(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls this as a C function with its arguments on `state`'s stack.
    unsafe { container(state, spacer, 3) }
}


/// What a node starts from: a box (and which kind, for its template) or a text.
enum Base {
    Box(fn() -> El, u8),
    Text(String),
}

/// Where a node's props tables sit on the stack, 0 for none: its style, decoded once when
/// frozen, and its own props laid over it.
#[derive(Clone, Copy)]
struct Layers {
    style: c_int,
    props: c_int,
}

impl Layers {
    const NONE: Layers = Layers { style: 0, props: 0 };

    fn one(at: c_int) -> Layers {
        Layers { style: at, props: 0 }
    }
}

/// `ui.row(props?, children?)`, `ui.row(style, props?, children?)`, or `ui.row(children)`.
unsafe fn container(state: *mut ffi::lua_State, base: fn() -> El, kind: u8) -> c_int {
    // SAFETY: every call below reads or pushes on this C function's own stack.
    unsafe {
        let (layers, children) = match (ffi::lua_type(state, 1), ffi::lua_type(state, 2), ffi::lua_type(state, 3)) {
            (ffi::LUA_TTABLE, ffi::LUA_TTABLE, ffi::LUA_TTABLE) => (Layers { style: 1, props: 2 }, 3),
            (ffi::LUA_TTABLE, ffi::LUA_TNIL, ffi::LUA_TTABLE) => (Layers::one(1), 3),
            (ffi::LUA_TTABLE, ffi::LUA_TTABLE, _) if keyed(state, 2) => (Layers { style: 1, props: 2 }, 0),
            (ffi::LUA_TTABLE, ffi::LUA_TTABLE, _) => (Layers::one(1), 2),
            (ffi::LUA_TTABLE, _, _) if keyed(state, 1) => (Layers::one(1), 0),
            (ffi::LUA_TTABLE, _, _) => (Layers::NONE, 1),
            (ffi::LUA_TNIL | ffi::LUA_TNONE, ffi::LUA_TTABLE, _) => (Layers::NONE, 2),
            _ => (Layers::NONE, 0),
        };
        guard(state, || build(state, Base::Box(base, kind), layers, children))
    }
}

/// `ui.text(content, props?)` or `ui.text(content, style, props?)`, and values after them (or
/// straight after `content`), which `{1}`, `{2}`... in `content` stand for.
unsafe extern "C-unwind" fn ui_text(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: as for `container`.
    unsafe {
        let (layers, values) = match (ffi::lua_type(state, 2), ffi::lua_type(state, 3)) {
            (ffi::LUA_TTABLE, ffi::LUA_TTABLE) => (Layers { style: 2, props: 3 }, 4),
            (ffi::LUA_TTABLE, ffi::LUA_TNIL) => (Layers::one(2), 4),
            (ffi::LUA_TTABLE, _) => (Layers::one(2), 3),
            (ffi::LUA_TNIL, ffi::LUA_TTABLE) => (Layers::one(3), 4),
            (ffi::LUA_TNIL, _) => (Layers::NONE, 3),
            _ => (Layers::NONE, 2),
        };
        let content = match words(state, values) {
            Ok(content) => content,
            Err(message) => return guard(state, || Err(message)),
        };
        guard(state, move || build(state, Base::Text(content), layers, 0))
    }
}

/// A text's words: argument 1, with `{n}` filled from the `n`th value from stack slot
/// `values` on, straight into the string the element keeps.
unsafe fn words(state: *mut ffi::lua_State, values: c_int) -> Result<String, String> {
    // SAFETY: reads this C function's own arguments.
    let (content, last) = unsafe { (props::read(state, 1), ffi::lua_gettop(state)) };
    let template = match content {
        Value::Str(s) => s,
        Value::Num(n) => return Ok(props::format_num(n)),
        other => return Err(format!("ui.text needs a string, got {}", props::name(other))),
    };
    if last < values {
        return Ok(template.to_owned());
    }
    let mut out = String::with_capacity(template.len() + 8 * (last - values + 1) as usize);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || after.as_bytes().get(digits) != Some(&b'}') {
            out.push('{');
            rest = after;
            continue;
        }
        let n: c_int = after[..digits].parse().unwrap_or(0);
        let at = values + n - 1;
        if n < 1 || at > last {
            return Err(format!("ui.text: {{{n}}} has no value"));
        }
        // SAFETY: `at` is one of this C function's arguments.
        match unsafe { props::read(state, at) } {
            Value::Str(s) => out.push_str(s),
            Value::Num(n) => props::push_num(&mut out, n),
            other => return Err(format!("ui.text: {{{n}}} must be a string or a number, got {}", props::name(other))),
        }
        rest = &after[digits + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Whether the table at `idx` is keyed by names, as props are, rather than a list: its first
/// key is a string. A list's entries come first, even with `nil` before them.
pub(crate) unsafe fn keyed(state: *mut ffi::lua_State, idx: c_int) -> bool {
    // SAFETY: `idx` is a table on this stack; the first pair is popped.
    unsafe {
        if ffi::lua_objlen(state, idx) > 0 || ffi::lua_rawiter(state, idx, 0) < 0 {
            return false;
        }
        let named = ffi::lua_type(state, -2) == ffi::LUA_TSTRING;
        ffi::lua_settop(state, -3);
        named
    }
}

/// Builds a node from its props tables and the children array at `children` (0 for none) and
/// pushes its handle. The element is made in its arena slot first; each prop then sets it there
/// as it is read, and its children move from their slots straight into its list, so it is never
/// copied whole. A frozen style is decoded once per VM: after that the node starts from a copy,
/// and only the props laid over it are read.
unsafe fn build(state: *mut ffi::lua_State, base: Base, layers: Layers, children: c_int) -> Result<c_int, String> {
    top(|frame| {
        let is_text = matches!(base, Base::Text(_));
        let kind = match base {
            Base::Box(_, kind) => kind,
            Base::Text(_) => u8::MAX,
        };
        let Frame { arena, attach, env } = frame;
        // SAFETY: `layers.style` is 0 or a table on this stack.
        let template = (layers.style > 0 && unsafe { ffi::lua_getreadonly(state, layers.style) } != 0)
            // SAFETY: as above.
            .then(|| (unsafe { ffi::lua_topointer(state, layers.style) } as usize, kind));
        let reused = template.and_then(|key| env.templates.borrow().get(key, env, layers.props > 0));
        // The node's props still to apply once its children are in: none for a whole template.
        // SAFETY (each `decode_props`): its table is 0 or a table on this stack; the frame borrow
        // covers no call back into Luau.
        let (slot, pending) = match reused {
            Some(Reuse::Whole(mut el)) => {
                if let Base::Text(s) = base {
                    el.set_content(s);
                }
                (arena.alloc(el)?, None)
            }
            Some(Reuse::Under(mut el, mut p)) => {
                if let Base::Text(s) = base {
                    el.set_content(s);
                }
                let slot = arena.alloc(el)?;
                let press = unsafe { decode_props(state, layers.props, env, is_text, &mut p, arena.get_mut(slot)) }?;
                (slot, Some((p, press)))
            }
            None => {
                let slot = arena.alloc(match base {
                    Base::Box(f, _) => f(),
                    Base::Text(s) => text(s),
                })?;
                let mut p = Props::new(env);
                if template.is_some() {
                    *env.recording.borrow_mut() = Some(Vec::new());
                }
                let decoded = unsafe { decode_props(state, layers.style, env, is_text, &mut p, arena.get_mut(slot)) };
                let colors = env.recording.take();
                let mut press = decoded?;
                // The template is the node without children, words or handlers, finished and not.
                if let (Some(key), Some(colors), None, None, false) = (template, colors, &press, &p.on_hover, p.uncacheable) {
                    let mut under = arena.get_mut(slot).clone();
                    if is_text {
                        under.set_content(String::new());
                    }
                    let mut el = under.clone();
                    p.clone().finish(&mut el, env, is_text, None, *attach);
                    // SAFETY: `layers.style` is the frozen table's slot.
                    unsafe { env.templates.borrow_mut().put(state, layers.style, key, env, el, under, p.clone(), colors) };
                }
                if let Some(own) = unsafe { decode_props(state, layers.props, env, is_text, &mut p, arena.get_mut(slot)) }? {
                    press = Some(own);
                }
                (slot, Some((p, press)))
            }
        };
        if children > 0 {
            let (slots, el) = arena.split(slot);
            let mut failed = None;
            // SAFETY: `children` is a table on this stack.
            el.extend_children(unsafe { Children::new(state, children, slots, env, &mut failed) });
            if let Some(message) = failed {
                return Err(message);
            }
        }
        if let Some((p, press)) = pending {
            p.finish(arena.get_mut(slot), env, is_text, press, *attach);
        }
        // SAFETY: pushes the handle as this C function's result.
        unsafe { ffi::lua_pushlightuserdata(state, arena.handle(slot)) };
        Ok(1)
    })
    .unwrap_or_else(|| Err("ui nodes can only be built while rendering".into()))
}

/// Decodes the props table at `props` (0 for none) onto `el`, and returns the `onPress`
/// function in it.
unsafe fn decode_props(state: *mut ffi::lua_State, props: c_int, env: &Env, is_text: bool, p: &mut Props, el: &mut El) -> Result<Option<Rc<Callback>>, String> {
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
                        Value::Func => p.on_hover = Some(env.reference(state, -1)),
                        Value::Nil => {}
                        other => return Err(format!("onHover must be a function, got {}", props::name(other))),
                    },
                    Ok(atom) => p.decode_at(el, state, atom, -1, env, is_text)?,
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
    let out: Vec<El> = unsafe { Children::new(state, idx, arena.all(), env, &mut failed) }.collect();
    failed.map_or(Ok(out), Err)
}

/// The children in the array at stack slot `idx`, taken from their arena slots as they are
/// read. The first bad entry ends the walk and lands in `failed`.
pub(crate) struct Children<'a> {
    state: *mut ffi::lua_State,
    idx: c_int,
    next: usize,
    len: usize,
    slots: Slots<'a>,
    env: &'a Env,
    failed: &'a mut Option<String>,
}

impl<'a> Children<'a> {
    /// SAFETY: `idx` is a table on `state`'s stack, which the walk leaves as it found it.
    unsafe fn new(state: *mut ffi::lua_State, idx: c_int, slots: Slots<'a>, env: &'a Env, failed: &'a mut Option<String>) -> Self {
        // SAFETY: as this function's contract says.
        let len = unsafe { ffi::lua_objlen(state, idx) };
        Children { state, idx, next: 1, len, slots, env, failed }
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
                    match self.slots.slot(handle) {
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
                    let mut t = text(s);
                    t.set_color(self.env.text_color());
                    Some(t)
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
