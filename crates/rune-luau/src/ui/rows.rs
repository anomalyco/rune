//! `ui.virtualList({ count, estimatedRowHeight, renderRow, rowKey?, version?, key, ... })`: a
//! vertical scroll viewport over `El::virtual_rows`. Layout asks for rows only as they enter the
//! mounted window, and each row is memoized by its key and the list's `version`, so scrolling
//! calls `renderRow(i)` (1-based) only for rows coming in, and a frame that scrolls nothing new
//! into the window calls no Luau at all. Without `rowKey`, row `i` is keyed by its index.

use std::os::raw::c_int;
use std::rc::{Rc, Weak};

use mlua::ffi;
use rune::{El, GrowthSpec, col};

use super::props::{self, Atom, Style, Value};
use super::view::{mix, version_hash};
use super::{Callback, Event, guard, top};
use crate::script::Inner;

/// A list's rows, as layout asks for them.
#[derive(Clone)]
pub(crate) struct Spec {
    pub(crate) count: usize,
    pub(crate) estimate: f32,
    pub(crate) render: Rc<Callback>,
    pub(crate) key: Option<Rc<Callback>>,
    /// The list's version mixed with the script's salt: rows rebuild when it changes. Without
    /// a `version` prop it is the script's own, which moves whenever script code runs.
    pub(crate) version: u64,
    /// What `virtual_rows_version` gets: rows keyed by index never change keys; rows keyed by
    /// `rowKey` change keys when `version` does.
    pub(crate) keys_version: Option<u64>,
    pub(crate) id: Rc<str>,
    pub(crate) visible: Option<Rc<Callback>>,
    pub(crate) gap: f32,
}

pub(crate) unsafe extern "C-unwind" fn ui_virtual_list(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: argument 1 is the props table on this C function's stack.
    unsafe {
        if ffi::lua_type(state, 1) != ffi::LUA_TTABLE {
            return guard(state, || Err("ui.virtualList needs a props table".into()));
        }
        guard(state, || {
            top(|frame| {
                let mut style = Style::new(&frame.env);
                let (mut count, mut estimate, mut version, mut key) = (0usize, 32.0f32, None, None::<String>);
                let (mut render, mut row_key, mut visible, mut gap) = (None, None, None, 0.0f32);
                let mut iter = 0;
                while {
                    iter = ffi::lua_rawiter(state, 1, iter);
                    iter >= 0
                } {
                    let v = props::read(state, -1);
                    match (props::key_atom(state, -2), v) {
                        (Ok(Atom::Count), Value::Num(n)) => count = n.max(0.0) as usize,
                        (Ok(Atom::EstimatedRowHeight), Value::Num(n)) => estimate = n.max(1.0) as f32,
                        (Ok(Atom::Version), _) => version = Some(version_hash(state, -1)?),
                        (Ok(Atom::RenderRow), Value::Func) => render = Some(frame.env.reference(state, -1)),
                        (Ok(Atom::RowKey), Value::Func) => row_key = Some(frame.env.reference(state, -1)),
                        (Ok(Atom::OnVisibleRows), Value::Func) => visible = Some(frame.env.reference(state, -1)),
                        (Ok(Atom::Gap), Value::Num(n)) => gap = n as f32,
                        (Ok(Atom::Key), Value::Str(s)) => key = Some(s.to_owned()),
                        (Ok(Atom::Key), Value::Num(n)) => key = Some(props::format_num(n)),
                        (Ok(atom @ (Atom::Count | Atom::EstimatedRowHeight | Atom::RenderRow | Atom::RowKey | Atom::OnVisibleRows | Atom::Gap)), other) => {
                            return Err(format!("ui.virtualList {atom:?} can't be a {}", props::name(other)));
                        }
                        (Ok(Atom::OnPress), _) => frame.env.warn_once("ui.virtualList takes no onPress".into()),
                        (Ok(atom), _) => style.decode_at(state, atom, -1, &frame.env, false)?,
                        (Err(name), _) => frame.env.warn_once(format!("unknown prop {name}")),
                    }
                    ffi::lua_settop(state, -3);
                }
                let render = render.ok_or("ui.virtualList needs renderRow")?;
                let key = key.unwrap_or_else(|| "list".to_owned());
                let env = &frame.env;
                let version = mix(version.unwrap_or(env.version), env.salt);
                let spec = Spec {
                    count,
                    estimate,
                    keys_version: if row_key.is_none() { Some(env.salt) } else { Some(version) },
                    version,
                    render,
                    key: row_key,
                    visible,
                    gap,
                    id: format!("{}{key}", env.prefix).into(),
                };
                style.scroll = Some((false, true));
                let viewport = style.apply(col().w_full(), env, false, None, None, frame.attach).key(key);
                let el = (frame.attach.rows)(viewport, spec);
                let handle = frame.put(el)?;
                ffi::lua_pushlightuserdata(state, handle);
                Ok(1)
            })
            .unwrap_or_else(|| Err("ui nodes can only be built while rendering".into()))
        })
    }
}

pub(crate) fn attach<S: 'static>(viewport: El, spec: Spec) -> El {
    let keys = spec.clone();
    let rows = spec.clone();
    let mut stack = col().key("rows").w_full().gap(spec.gap).growth(GrowthSpec::default()).virtual_rows(
        spec.count,
        spec.estimate,
        move |s: &S, i| match &keys.key {
            Some(cb) => upgrade(&cb.script).and_then(|script| script.around_read(s, || script.row_key(cb, i))).unwrap_or_else(|| i.to_string()).into(),
            None => i.to_string().into(),
        },
        move |s: &S, i| {
            let Some(script) = upgrade(&rows.render.script) else { return rune::el() };
            script
                .around_read(s, || {
                    let key = match &rows.key {
                        Some(cb) => script.row_key(cb, i),
                        None => i.to_string(),
                    };
                    // Rune keys the row by `key` whatever its memo id; keying the root by that id
                    // keeps memo from reporting a replaced key.
                    let id = format!("{}/{key}", rows.id);
                    rune::memo::memo_with_hash(id.clone(), rows.version, || script.row::<S>(&rows, i).key(id))
                })
                .unwrap_or_else(rune::el)
        },
    );
    if let Some(v) = spec.keys_version {
        stack = stack.virtual_rows_version(v);
    }
    if let Some(cb) = spec.visible {
        stack = stack.on_visible_rows(move |s: &mut S, range| {
            let Some(script) = cb.script.upgrade() else { return false };
            let event = Event { kind: "visible-rows", key: None, mods: None, rows: Some((range.start + 1, range.end)), entered: None, fields: Rc::new([]) };
            script.around_write(s, None, |tx| script.handle(&cb, event, tx));
            true
        });
    }
    viewport.child(stack)
}

fn upgrade(script: &Weak<Inner>) -> Option<Rc<Inner>> {
    script.upgrade()
}
