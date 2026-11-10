//! `ui.virtualList({ count, estimatedRowHeight, renderRow, rowKey?, version?, key, ... })`: a
//! vertical scroll viewport over `El::virtual_rows`. Layout asks for rows only as they enter the
//! mounted window, and each row is memoized by its key and the list's `version`, so scrolling
//! calls `renderRow(i)` (1-based) only for rows coming in, and a frame that scrolls nothing new
//! into the window calls no Luau at all. Without `rowKey`, row `i` is keyed by its index.
//! `rowVersions` gives each row a version of its own beside the list's, so a row rebuilds only
//! when the list's or its own moves. With `scrollY = false` the list has no viewport of its own: it is a stack inside a scrolling
//! column, under or among other content, and windows against that column's viewport.

use std::os::raw::c_int;
use std::rc::{Rc, Weak};

use mlua::ffi;
use rune::{El, GrowthSpec, col};

use super::props::{self, Atom, Props, Value};
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
    /// `rowVersions`: each row's own version, mixed with `version`, so a row rebuilds when
    /// either moves. Rows past its end take `version` alone.
    pub(crate) row_versions: Option<Rc<[u64]>>,
    /// What `virtual_rows_version` gets: rows keyed by index never change keys; rows keyed by
    /// `rowKey` change keys when `version` does.
    pub(crate) keys_version: Option<u64>,
    pub(crate) id: Rc<str>,
    pub(crate) visible: Option<Rc<Callback>>,
    pub(crate) gap: f32,
    /// The rows stack in the list's own element, inside a scrolling ancestor, rather than in a
    /// viewport of their own.
    pub(crate) inline: bool,
}

pub(crate) unsafe extern "C-unwind" fn ui_virtual_list(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: argument 1 is the props table on this C function's stack.
    unsafe {
        if ffi::lua_type(state, 1) != ffi::LUA_TTABLE {
            return guard(state, || Err("ui.virtualList needs a props table".into()));
        }
        guard(state, || {
            top(|frame| {
                let mut p = Props::new(&frame.env);
                let mut list = col();
                list.set_w_full();
                let (mut count, mut estimate, mut version, mut key) = (0usize, 32.0f32, None, None::<String>);
                let (mut render, mut row_key, mut visible, mut gap, mut own) = (None, None, None, 0.0f32, true);
                let mut row_versions: Option<Vec<u64>> = None;
                let mut iter = 0;
                while {
                    iter = ffi::lua_rawiter(state, 1, iter);
                    iter >= 0
                } {
                    let v = props::read(state, -1);
                    match (props::key_atom(state, -2), v) {
                        (Ok(Atom::Count), Value::Num(n)) => count = n.max(0.0) as usize,
                        (Ok(Atom::EstimatedRowHeight), Value::Num(n)) => estimate = n.max(1.0) as f32,
                        (Ok(Atom::RowVersions), Value::Other("table")) => row_versions = Some(versions(state)?),
                        (Ok(Atom::Version), _) => version = Some(version_hash(state, -1)?),
                        (Ok(Atom::RenderRow), Value::Func) => render = Some(frame.env.reference(state, -1)),
                        (Ok(Atom::RowKey), Value::Func) => row_key = Some(frame.env.reference(state, -1)),
                        (Ok(Atom::OnVisibleRows), Value::Func) => visible = Some(frame.env.reference(state, -1)),
                        (Ok(Atom::Gap), Value::Num(n)) => gap = n as f32,
                        (Ok(Atom::ScrollY), Value::Bool(b)) => own = b,
                        (Ok(Atom::Key), Value::Str(s)) => key = Some(s.to_owned()),
                        (Ok(Atom::Key), Value::Num(n)) => key = Some(props::format_num(n)),
                        (Ok(atom @ (Atom::Count | Atom::EstimatedRowHeight | Atom::RenderRow | Atom::RowKey | Atom::RowVersions | Atom::OnVisibleRows | Atom::Gap)), other) => {
                            return Err(format!("ui.virtualList {atom:?} can't be a {}", props::name(other)));
                        }
                        (Ok(Atom::OnPress), _) => frame.env.warn_once("ui.virtualList takes no onPress".into()),
                        (Ok(atom), _) => p.decode_at(&mut list, state, atom, -1, &frame.env, false)?,
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
                    row_versions: row_versions.map(|vs| vs.into_iter().map(|v| mix(v, version)).collect()),
                    render,
                    key: row_key,
                    visible,
                    gap,
                    id: format!("{}{key}", env.prefix).into(),
                    inline: !own,
                };
                if own {
                    p.scroll = Some((false, true));
                }
                p.finish(&mut list, env, false, None, frame.attach);
                list.set_key(key);
                let el = (frame.attach.rows)(list, spec);
                let handle = frame.put(el)?;
                ffi::lua_pushlightuserdata(state, handle);
                Ok(1)
            })
            .unwrap_or_else(|| Err("ui nodes can only be built while rendering".into()))
        })
    }
}

/// The versions in the list on top of the stack, one per row.
unsafe fn versions(state: *mut ffi::lua_State) -> Result<Vec<u64>, String> {
    // SAFETY: the top slot holds a table; each entry is pushed, hashed and popped.
    unsafe {
        let at = ffi::lua_absindex(state, -1);
        let len = ffi::lua_objlen(state, at);
        (1..=len)
            .map(|i| {
                ffi::lua_rawgeti(state, at, i as ffi::lua_Integer);
                let v = version_hash(state, -1);
                ffi::lua_settop(state, -2);
                v.map_err(|e| format!("ui.virtualList rowVersions {i}: {e}"))
            })
            .collect()
    }
}

/// Makes `list` (the node its props built) a virtual list: the rows' stack inside it as its
/// viewport, or itself when the list is inline.
pub(crate) fn attach<S: 'static>(list: El, spec: Spec) -> El {
    let keys = spec.clone();
    let rows = spec.clone();
    let (stack, viewport) = if spec.inline { (list, None) } else { (col().key("rows").w_full(), Some(list)) };
    let mut stack = stack.gap(spec.gap).growth(GrowthSpec::default()).virtual_rows(
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
                    let version = rows.row_versions.as_ref().and_then(|vs| vs.get(i).copied()).unwrap_or(rows.version);
                    rune::memo::memo_with_hash(id.clone(), version, || script.row::<S>(&rows, i).key(id))
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
    match viewport {
        Some(viewport) => viewport.child(stack),
        None => stack,
    }
}

fn upgrade(script: &Weak<Inner>) -> Option<Rc<Inner>> {
    script.upgrade()
}
