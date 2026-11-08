//! The `view` a render function receives: `view:Memo(key, version, build)` and
//! `view:Handler(fn)`, plus `view.dark`.
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
        let key = match props::read(state, 2) {
            Value::Str(s) => s.to_owned(),
            Value::Num(n) => props::format_num(n),
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
        let Some(salt) = top(|f| f.env.salt) else {
            return guard(state, || Err("view:Memo can only be called while rendering".into()));
        };
        let mut failed: Option<String> = None;
        // The subtree's root takes the memo's key, as `rune::memo` keys it.
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
        if ffi::lua_pcall(state, 0, 1, 0) != ffi::LUA_OK {
            *failed = Some(match props::read(state, -1) {
                Value::Str(s) => s.to_owned(),
                _ => "view:Memo build failed".to_owned(),
            });
            ffi::lua_settop(state, -2);
            return rune::el();
        }
        let handle = ffi::lua_tolightuserdata(state, -1);
        ffi::lua_settop(state, -2);
        if handle.is_null() {
            *failed = Some("view:Memo build must return a ui node".into());
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
