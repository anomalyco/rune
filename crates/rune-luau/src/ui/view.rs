//! The `view` a render function receives: `view:Memo(key, version, build)` and
//! `view:Handler(fn)`, plus `view.dark`; and `ui.each(items, key, version, build)`, a memo per
//! item of a list.
//!
//! `Memo` is a C function: it hashes `version` straight off the stack (a number's bits, a
//! string's bytes) and calls `rune::memo::memo_with_hash`. On a hit Rune hands back the subtree
//! it built before, and `build` is never called.

use std::hash::{Hash, Hasher};
use std::os::raw::c_int;

use mlua::ffi;
use rune::El;

use super::props::{self, Value};
use super::{guard, top};

pub(crate) unsafe extern "C-unwind" fn view_memo(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: arguments are (view, key, version, build) on this C function's stack.
    unsafe {
        let num;
        let key = match props::read(state, 2) {
            Value::Str(s) => s,
            Value::Num(n) => {
                num = props::format_num(n);
                num.as_str()
            }
            other => {
                let message = format!("view:Memo key must be a string or number, got {}", props::name(other));
                return guard(state, || Err(message));
            }
        };
        let version = match version_hash(state, 3) {
            Ok(v) => v,
            Err(message) => return guard(state, || Err(message)),
        };
        if ffi::lua_type(state, 4) != ffi::LUA_TFUNCTION {
            return guard(state, || Err("view:Memo needs a build function".into()));
        }
        let Some((salt, prefix)) = top(|f| (f.env.salt, f.env.prefix.clone())) else {
            return guard(state, || Err("view:Memo can only be called while rendering".into()));
        };
        let mut failed: Option<String> = None;
        // Memo ids are the window's: under the script's prefix, two scripts' "panel" memos are
        // two entries. The subtree's root takes the memo's id, as `rune::memo` keys it.
        let key = format!("{prefix}{key}");
        let id = key.clone();
        let el = rune::memo::memo_with_hash(key, mix(version, salt), || build(state, &mut failed).key(id));
        if let Some(message) = failed {
            // The salt bump after this render keeps the empty placeholder the memo stored from
            // being reused, even if the script catches the error.
            top(|f| f.env.memo_failed.set(true));
            drop(el);
            return guard(state, || Err(message));
        }
        guard(state, || {
            let handle = top(|f| f.put(el)).ok_or("view:Memo can only be called while rendering")??;
            ffi::lua_pushlightuserdata(state, handle);
            Ok(1)
        })
    }
}

/// Calls the build function at slot 4 under `lua_pcall`, so an error in it comes back here
/// instead of unwinding through Rune's memo code.
fn build(state: *mut ffi::lua_State, failed: &mut Option<String>) -> El {
    // SAFETY: pushes a copy of the function, calls it protected and pops what it left.
    unsafe {
        ffi::lua_pushvalue(state, 4);
        let status = ffi::lua_pcall(state, 0, 1, 0);
        built(state, status, failed, "view:Memo build")
    }
}

/// The node a protected build call (`status`) left on the stack, which it pops; on an error,
/// an empty element and the message in `failed`.
unsafe fn built(state: *mut ffi::lua_State, status: c_int, failed: &mut Option<String>, what: &str) -> El {
    // SAFETY: the call's one result is on top of the stack.
    unsafe {
        if status != ffi::LUA_OK {
            *failed = Some(match props::read(state, -1) {
                Value::Str(s) => s.to_owned(),
                _ => format!("{what} failed"),
            });
            ffi::lua_settop(state, -2);
            return rune::el();
        }
        let handle = ffi::lua_tolightuserdata(state, -1);
        ffi::lua_settop(state, -2);
        if handle.is_null() {
            *failed = Some(format!("{what} must return a ui node"));
            return rune::el();
        }
        match top(|f| f.take(handle)) {
            Some(Ok(el)) => el,
            Some(Err(message)) => {
                *failed = Some(message);
                rune::el()
            }
            None => rune::el(),
        }
    }
}

/// `ui.each(items, key, version, build)`: a list of nodes, `build(item, index)` for each item,
/// each a memo of its own (as `view:Memo`) on the item's key and version, so a render rebuilds
/// only the items whose version moved. `key` and `version` are field names or functions of
/// `(item, index)`. Returns the nodes as a list, to pass as children.
pub(crate) unsafe extern "C-unwind" fn ui_each(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: arguments are (items, key, version, build) on this C function's stack; slot 5 is
    // the result list and slot 6 the item being built, popped after each.
    unsafe {
        let pick = |i| matches!(ffi::lua_type(state, i), ffi::LUA_TSTRING | ffi::LUA_TFUNCTION);
        if ffi::lua_type(state, 1) != ffi::LUA_TTABLE || !pick(2) || !pick(3) || ffi::lua_type(state, 4) != ffi::LUA_TFUNCTION {
            return guard(state, || Err("ui.each needs items, a key and a version (field names or functions) and a build function".into()));
        }
        ffi::lua_settop(state, 4);
        let len = ffi::lua_objlen(state, 1);
        ffi::lua_createtable(state, len as c_int, 0);
        guard(state, || {
            // An item's key is its node's key and its memo's id; memos nest under the surface's,
            // which carries the script's prefix, so two scripts' keys never meet.
            let salt = top(|f| f.env.salt).ok_or("ui.each can only be called while rendering")?;
            for i in 1..=len {
                ffi::lua_rawgeti(state, 1, i as ffi::lua_Integer);
                let key = match field(state, 2, i)? {
                    Value::Str(s) => s.to_owned(),
                    Value::Num(n) => props::format_num(n),
                    other => return Err(format!("ui.each: item {i}'s key must be a string or a number, got {}", props::name(other))),
                };
                ffi::lua_settop(state, 6);
                field(state, 3, i)?;
                let version = version_hash(state, 7).map_err(|e| format!("ui.each: item {i}: {e}"))?;
                ffi::lua_settop(state, 6);
                let mut failed: Option<String> = None;
                let id = key.clone();
                let el = rune::memo::memo_with_hash(key, mix(version, salt), || item(state, i, &mut failed).key(id));
                if let Some(message) = failed {
                    top(|f| f.env.memo_failed.set(true));
                    return Err(message);
                }
                let handle = top(|f| f.put(el)).ok_or("ui.each can only be called while rendering")??;
                ffi::lua_settop(state, 5);
                ffi::lua_pushlightuserdata(state, handle);
                ffi::lua_rawseti(state, 5, i as ffi::lua_Integer);
            }
            Ok(1)
        })
    }
}

/// Pushes the item at slot 6's key or version: its field named by the string at `by`, or what
/// the function at `by` returns for `(item, i)`.
unsafe fn field<'a>(state: *mut ffi::lua_State, by: c_int, i: usize) -> Result<Value<'a>, String> {
    // SAFETY: slot 6 is the item; `by` is a string or a function.
    unsafe {
        if ffi::lua_type(state, by) == ffi::LUA_TSTRING {
            if ffi::lua_type(state, 6) != ffi::LUA_TTABLE {
                return Err(format!("ui.each: item {i} is a {}, not a table with fields", props::name(props::read(state, 6))));
            }
            ffi::lua_pushvalue(state, by);
            ffi::lua_rawget(state, 6);
        } else {
            ffi::lua_pushvalue(state, by);
            ffi::lua_pushvalue(state, 6);
            ffi::lua_pushinteger(state, i as ffi::lua_Integer);
            if ffi::lua_pcall(state, 2, 1, 0) != ffi::LUA_OK {
                return Err(match props::read(state, -1) {
                    Value::Str(s) => s.to_owned(),
                    _ => format!("ui.each: item {i}'s key or version failed"),
                });
            }
        }
        Ok(props::read(state, -1))
    }
}

/// Calls the build function (slot 4) for the item at slot 6, protected.
fn item(state: *mut ffi::lua_State, i: usize, failed: &mut Option<String>) -> El {
    // SAFETY: pushes the function and its arguments, calls it protected and pops what it left.
    unsafe {
        ffi::lua_pushvalue(state, 4);
        ffi::lua_pushvalue(state, 6);
        ffi::lua_pushinteger(state, i as ffi::lua_Integer);
        let status = ffi::lua_pcall(state, 2, 1, 0);
        built(state, status, failed, "ui.each build")
    }
}

/// `view:Handler(fn)`: returns `fn`. Handlers are plain functions; this keeps scripts written
/// against hosts that wrap them working.
pub(crate) unsafe extern "C-unwind" fn view_handler(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: returns argument 2 as is.
    unsafe {
        ffi::lua_pushvalue(state, 2);
        1
    }
}

/// A memo version straight off the stack: nil, a boolean, a number's bits or a string's bytes.
pub(crate) unsafe fn version_hash(state: *mut ffi::lua_State, idx: c_int) -> Result<u64, String> {
    let mut h = rustc_hash::FxHasher::default();
    // SAFETY: reads slot `idx`.
    match unsafe { props::read(state, idx) } {
        Value::Nil => 0u8.hash(&mut h),
        Value::Bool(b) => (1u8, b).hash(&mut h),
        Value::Num(n) => (2u8, n.to_bits()).hash(&mut h),
        Value::Str(s) => (3u8, s).hash(&mut h),
        other => return Err(format!("a memo version must be nil, a boolean, a number or a string, got {}", props::name(other))),
    }
    Ok(h.finish())
}

pub(crate) fn mix(version: u64, salt: u64) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    (version, salt).hash(&mut h);
    h.finish()
}
