//! Stack value readers, size/role parsers, and range validators for `ui.*` props.

#![allow(unsafe_code)]

use std::ffi::CStr;
use std::os::raw::c_int;

use mlua::ffi;

/// A prop value on the stack. Strings borrow the Lua string, which the table keeps alive.
#[derive(Clone, Copy)]
pub(crate) enum Value<'a> {
    Nil,
    Bool(bool),
    Num(f64),
    Str(&'a str),
    Func,
    Other(&'static str),
}

/// Reads stack slot `idx`.
pub(crate) unsafe fn read<'a>(state: *mut ffi::lua_State, idx: c_int) -> Value<'a> {
    // SAFETY: the caller passes a valid slot; strings stay alive while their table is on the stack.
    unsafe {
        match ffi::lua_type(state, idx) {
            ffi::LUA_TNIL => Value::Nil,
            ffi::LUA_TBOOLEAN => Value::Bool(ffi::lua_toboolean(state, idx) != 0),
            ffi::LUA_TNUMBER => Value::Num(ffi::lua_tonumberx(state, idx, std::ptr::null_mut())),
            ffi::LUA_TSTRING => {
                let mut len = 0;
                let p = ffi::lua_tolstring(state, idx, &mut len);
                Value::Str(std::str::from_utf8(std::slice::from_raw_parts(p.cast::<u8>(), len)).unwrap_or(""))
            }
            ffi::LUA_TFUNCTION => Value::Func,
            t => Value::Other(CStr::from_ptr(ffi::lua_typename(state, t)).to_str().unwrap_or("value")),
        }
    }
}

/// A size prop: points, the parent's whole side, or a percentage fraction of the parent's side.
#[derive(Clone, Copy)]
pub(crate) enum Size {
    Points(f32),
    Full,
    Frac(f32),
}

/// Parses `w` / `h`: points, `"full"`, or a non-negative CSS percentage string like `"40%"`.
pub(crate) fn parse_size(prop: &str, v: Value<'_>) -> Result<Size, String> {
    match v {
        Value::Num(n) => Ok(Size::Points(n as f32)),
        Value::Str("full") => Ok(Size::Full),
        Value::Str(s) => {
            if let Some(pct) = s.strip_suffix('%')
                && let Ok(n) = pct.parse::<f32>()
                && n.is_finite()
                && n >= 0.0
            {
                return Ok(Size::Frac(n / 100.0));
            }
            Err(format!("{prop} must be a number, \"full\", or a percentage like \"40%\", got {s:?}"))
        }
        other => Err(format!("{prop} must be a number, \"full\", or a percentage like \"40%\", got {}", name(other))),
    }
}

/// Finite `f32` prop value.
pub(crate) fn finite(prop: &str, v: Value<'_>) -> Result<f32, String> {
    match v {
        Value::Num(n) if n.is_finite() => Ok(n as f32),
        Value::Num(n) => Err(format!("{prop} must be a finite number, got {n}")),
        other => Err(format!("{prop} must be a number, got {}", name(other))),
    }
}

/// Non-negative finite `f32` prop value.
pub(crate) fn non_neg(prop: &str, v: Value<'_>) -> Result<f32, String> {
    match v {
        Value::Num(n) if n.is_finite() && n >= 0.0 => Ok(n as f32),
        Value::Num(n) => Err(format!("{prop} must be a non-negative finite number, got {n}")),
        other => Err(format!("{prop} must be a number, got {}", name(other))),
    }
}

/// Positive finite `f32` prop value.
pub(crate) fn positive(prop: &str, v: Value<'_>) -> Result<f32, String> {
    match v {
        Value::Num(n) if n.is_finite() && n > 0.0 => Ok(n as f32),
        Value::Num(n) => Err(format!("{prop} must be a positive finite number, got {n}")),
        other => Err(format!("{prop} must be a number, got {}", name(other))),
    }
}

/// Non-negative whole-number `usize` prop value (such as `maxLines`).
pub(crate) fn non_neg_int(prop: &str, v: Value<'_>) -> Result<usize, String> {
    match v {
        Value::Num(n) if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= u32::MAX as f64 => Ok(n as usize),
        Value::Num(n) => Err(format!("{prop} must be a non-negative integer, got {n}")),
        other => Err(format!("{prop} must be a non-negative integer, got {}", name(other))),
    }
}

/// Positive whole-number `usize` prop value (such as `cols`).
pub(crate) fn pos_int(prop: &str, v: Value<'_>) -> Result<usize, String> {
    match v {
        Value::Num(n) if n.is_finite() && n >= 1.0 && n.fract() == 0.0 && n <= u32::MAX as f64 => Ok(n as usize),
        Value::Num(n) => Err(format!("{prop} must be a positive integer, got {n}")),
        other => Err(format!("{prop} must be a positive integer, got {}", name(other))),
    }
}

/// `N` numbers from the table at `idx`: by name when `names` are given, else items `1..=N`.
pub(crate) unsafe fn numbers<const N: usize>(state: *mut ffi::lua_State, idx: c_int, names: &[&str]) -> [Option<f32>; N] {
    let mut out = [None; N];
    // SAFETY: `idx` is a table; each lookup pushes one value, popped right after.
    unsafe {
        let idx = ffi::lua_absindex(state, idx);
        for (i, slot) in out.iter_mut().enumerate() {
            let kind = match names.get(i) {
                Some(name) => {
                    ffi::lua_pushlstring(state, name.as_ptr().cast(), name.len());
                    ffi::lua_rawget(state, idx)
                }
                None => ffi::lua_rawgeti(state, idx, i as ffi::lua_Integer + 1),
            };
            if kind == ffi::LUA_TNUMBER {
                *slot = Some(ffi::lua_tonumberx(state, -1, std::ptr::null_mut()) as f32);
            }
            ffi::lua_settop(state, -2);
        }
    }
    out
}

/// Reads an `{ x, y }` table at `idx`, falling back to `default` for omitted (`nil`) axes and
/// returning a prop error when a present axis is not a finite number.
pub(crate) unsafe fn xy_pair(state: *mut ffi::lua_State, idx: c_int, prop: &str, default: [f32; 2]) -> Result<[f32; 2], String> {
    let mut out = default;
    // SAFETY: `idx` is a table; each lookup pushes one value, popped before returning.
    unsafe {
        let idx = ffi::lua_absindex(state, idx);
        for (i, axis) in ["x", "y"].into_iter().enumerate() {
            ffi::lua_pushlstring(state, axis.as_ptr().cast(), axis.len());
            let kind = ffi::lua_rawget(state, idx);
            let res = match kind {
                ffi::LUA_TNIL => Ok(default[i]),
                ffi::LUA_TNUMBER => {
                    let n = ffi::lua_tonumberx(state, -1, std::ptr::null_mut());
                    if n.is_finite() { Ok(n as f32) } else { Err(format!("{prop}.{axis} must be a finite number, got {n}")) }
                }
                t => {
                    let what = CStr::from_ptr(ffi::lua_typename(state, t)).to_str().unwrap_or("value");
                    Err(format!("{prop}.{axis} must be a finite number, got {what}"))
                }
            };
            ffi::lua_settop(state, -2);
            out[i] = res?;
        }
    }
    Ok(out)
}

/// Maps a WAI-ARIA role name to the static string `El::set_role` expects.
pub(crate) fn role(s: &str) -> Option<&'static str> {
    Some(match s {
        "none" => "none",
        "group" => "group",
        "region" => "region",
        "button" => "button",
        "link" => "link",
        "switch" => "switch",
        "checkbox" => "checkbox",
        "radio" => "radio",
        "tab" => "tab",
        "tablist" => "tablist",
        "list" => "list",
        "listitem" => "listitem",
        "heading" => "heading",
        "img" => "img",
        "status" => "status",
        "timer" => "timer",
        "alert" => "alert",
        "slider" => "slider",
        "spinbutton" => "spinbutton",
        "textbox" => "textbox",
        "progressbar" => "progressbar",
        "dialog" => "dialog",
        "combobox" => "combobox",
        "tooltip" => "tooltip",
        _ => return None,
    })
}

pub(crate) fn name(v: Value<'_>) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Bool(_) => "boolean",
        Value::Num(_) => "number",
        Value::Str(_) => "string",
        Value::Func => "function",
        Value::Other(t) => t,
    }
}

/// A number as Luau prints it: `3`, not `3.0`.
pub(crate) fn format_num(n: f64) -> String {
    let mut s = String::new();
    push_num(&mut s, n);
    s
}

/// [`format_num`] onto the end of `out`.
pub(crate) fn push_num(out: &mut String, n: f64) {
    use std::fmt::Write;
    if n.fract() == 0.0 && n.abs() < 1e15 { write!(out, "{}", n as i64).ok(); } else { write!(out, "{n}").ok(); }
}
