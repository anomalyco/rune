//! `ui.*` props: names Luau interns as atoms, read straight off the stack and applied to an `El`.
//! A prop table is walked with `lua_rawiter`; each key's atom (a small integer Luau stores in the
//! string when it is interned) picks the setter, so a prop costs a switch, not a string compare.
//!
//! The names are layout (`w`, `pad`, `gap`, `align`), box (`fill`, `border`, `radius`,
//! `onPress`), text (`size`, `weight`, `color`) and list props (`count`, `renderRow`); colours are
//! palette names or `ui.hex(...)`.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::rc::Rc;

use mlua::ffi;
use rune::{Align, Color, El, ScrollSpec, TextStyle};

use super::{Attach, Callback, Env};

macro_rules! atoms {
    ($($atom:ident = $name:literal,)*) => {
        /// Prop names, by atom.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(i16)]
        pub(crate) enum Atom { $($atom,)* }

        const NAMES: &[(&str, Atom)] = &[$(($name, Atom::$atom),)*];

        fn by_name(bytes: &[u8]) -> Option<Atom> {
            match bytes {
                $(b if b == $name.as_bytes() => Some(Atom::$atom),)*
                _ => None,
            }
        }
    };
}

atoms! {
    Key = "key",
    W = "w",
    H = "h",
    MinW = "minW",
    MinH = "minH",
    MaxW = "maxW",
    MaxH = "maxH",
    Grow = "grow",
    Shrink = "shrink",
    Pad = "pad",
    Px = "px",
    Py = "py",
    Pt = "pt",
    Pr = "pr",
    Pb = "pb",
    Pl = "pl",
    M = "m",
    Mx = "mx",
    My = "my",
    Mt = "mt",
    Mr = "mr",
    Mb = "mb",
    Ml = "ml",
    Opacity = "opacity",
    Role = "role",
    Label = "label",
    Help = "help",
    Gap = "gap",
    Align = "align",
    Justify = "justify",
    Center = "center",
    Wrap = "wrap",
    Fill = "fill",
    Hover = "hover",
    Press = "press",
    Border = "border",
    BorderWidth = "borderWidth",
    Radius = "radius",
    Clip = "clip",
    ScrollY = "scrollY",
    ScrollX = "scrollX",
    StickBottom = "stickBottom",
    OnPress = "onPress",
    Color = "color",
    Size = "size",
    Weight = "weight",
    LineHeight = "lineHeight",
    Tracking = "tracking",
    Mono = "mono",
    Tabular = "tabular",
    NoWrap = "noWrap",
    Truncate = "truncate",
    Selectable = "selectable",
    TextAlign = "textAlign",
    Count = "count",
    EstimatedRowHeight = "estimatedRowHeight",
    Version = "version",
    RenderRow = "renderRow",
    RowKey = "rowKey",
    RowVersions = "rowVersions",
    OnVisibleRows = "onVisibleRows",
    OnHover = "onHover",
    HoverTransition = "hoverTransition",
    HoverGroup = "hoverGroup",
    HoverGroupColor = "hoverGroupColor",
    RevealOnHover = "revealOnHover",
    Corners = "corners",
    Pin = "pin",
    Shadow = "shadow",
    Selected = "selected",
    Focusable = "focusable",
    Disabled = "disabled",
    HelpSide = "helpSide",
    FadeOut = "fadeOut",
    Dir = "dir",
}

/// Every prop name the built-in nodes read.
pub fn prop_names() -> impl Iterator<Item = &'static str> {
    NAMES.iter().map(|(n, _)| *n)
}

fn by_id(id: c_int) -> Option<Atom> {
    usize::try_from(id).ok().and_then(|i| NAMES.get(i)).map(|(_, a)| *a)
}

/// Luau's `useratom` callback: gives each prop name its atom when the string is first interned.
pub(crate) unsafe extern "C-unwind" fn useratom(_: *mut ffi::lua_State, s: *const c_char, len: usize) -> i16 {
    if len > 18 {
        return -1;
    }
    // SAFETY: Luau passes the new string's bytes and length.
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), len) };
    by_name(bytes).map_or(-1, |a| a as i16)
}

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

/// The key at stack slot `idx` as an atom, or its text when it isn't a prop name.
pub(crate) unsafe fn key_atom<'a>(state: *mut ffi::lua_State, idx: c_int) -> Result<Atom, &'a str> {
    // SAFETY: the caller passes a valid slot.
    unsafe {
        if ffi::lua_type(state, idx) != ffi::LUA_TSTRING {
            return Err("(non-string key)");
        }
        let mut atom: c_int = -1;
        let p = ffi::lua_tostringatom(state, idx, &mut atom);
        if let Some(a) = by_id(atom) {
            return Ok(a);
        }
        let bytes = CStr::from_ptr(p).to_bytes();
        by_name(bytes).ok_or(std::str::from_utf8(bytes).unwrap_or("?"))
    }
}

/// A size prop: points, or the parent's whole side.
#[derive(Clone, Copy)]
pub(crate) enum Size {
    Points(f32),
    Full,
}

/// Padding sides a prop set on its own (`pt`, `px`, ...), which `pad` leaves alone.
const TOP: u8 = 1;
const RIGHT: u8 = 2;
const BOTTOM: u8 = 4;
const LEFT: u8 = 8;

/// A node's props, read straight onto its element in the arena: most set it as they're read.
/// The rest wait here for the whole table, so the result doesn't depend on the order Luau walks
/// it: the press handler needs the key, `pad` yields to the sides set beside it, a text node's
/// style and colour land together, and `center`, `corners`, `selected` and `disabled` overrule
/// the props they overlap.
#[derive(Clone)]
pub(crate) struct Props {
    pub(crate) key: Option<String>,
    pub(crate) on_hover: Option<Rc<Callback>>,
    pad: Option<f32>,
    sides: u8,
    role: Option<&'static str>,
    center: bool,
    corners: Option<[f32; 4]>,
    selected: bool,
    pub(crate) disabled: bool,
    text: TextStyle,
    color: Option<Color>,
    truncate: bool,
    fade_out: bool,
    single_line: bool,
    wrap: bool,
    selectable: bool,
    text_align: Option<Align>,
    border: (Option<f32>, Option<Color>),
    pub(crate) scroll: Option<(bool, bool)>,
    stick_bottom: bool,
    /// A prop whose effect depends on more than its value and the colours (`shadow`): the node
    /// can't be a template.
    pub(crate) uncacheable: bool,
}

impl Props {
    /// A node's props before any are read: text starts from the host's text style.
    pub(crate) fn new(env: &Env) -> Self {
        Props {
            key: None,
            on_hover: None,
            pad: None,
            sides: 0,
            role: None,
            center: false,
            corners: None,
            selected: false,
            disabled: false,
            text: env.text,
            color: None,
            truncate: false,
            fade_out: false,
            single_line: false,
            wrap: false,
            selectable: false,
            text_align: None,
            border: (None, None),
            scroll: None,
            stick_bottom: false,
            uncacheable: false,
        }
    }

    /// Applies what waited for the whole table: the press and hover handlers (`press` is the
    /// node's `onPress`), then the props that depend on others, the text style and the key.
    pub(crate) fn finish(self, e: &mut El, env: &Env, is_text: bool, press: Option<Rc<Callback>>, attach: &'static Attach) {
        let press = press.filter(|_| !self.disabled);
        let key: Option<Rc<str>> = (press.is_some() || self.on_hover.is_some()).then(|| self.key.as_deref().map(Into::into)).flatten();
        if let Some(cb) = press {
            (attach.press)(e, cb, key.clone(), Rc::new([]));
        }
        if let Some(cb) = self.on_hover {
            with(e, |e| (attach.hover)(e, cb, key));
        }
        match (self.pad, self.sides) {
            (Some(p), 0) => _ = e.set_padding(p),
            (Some(p), s) => {
                if s & TOP == 0 {
                    e.set_pt(p);
                }
                if s & RIGHT == 0 {
                    e.set_pr(p);
                }
                if s & BOTTOM == 0 {
                    e.set_pb(p);
                }
                if s & LEFT == 0 {
                    e.set_pl(p);
                }
            }
            (None, _) => {}
        }
        if let Some(r) = self.role {
            e.set_role(r);
        }
        if self.center {
            e.set_center();
        }
        if let Some(r) = self.corners {
            with(e, |e| e.corners(r));
        }
        if self.selected {
            if let Some(c) = env.color("selected") {
                e.set_fill(c);
            }
            with(e, |e| e.focus_selected(true));
        }
        if self.disabled {
            with(e, |e| e.disabled(true));
        }
        if is_text {
            e.set_font(self.text).set_color(self.color.unwrap_or_else(|| env.text_color()));
            if self.truncate {
                e.set_truncate();
            }
            if self.fade_out {
                with(e, El::truncate_fade);
            }
            if self.single_line {
                with(e, El::single_line);
            }
            if self.wrap {
                e.set_wrap();
            }
            if self.selectable {
                e.set_selectable(true);
            }
            if let Some(a) = self.text_align {
                e.set_align(a);
            }
        } else if let Some(c) = self.color {
            e.set_color(c);
        }
        if self.border != (None, None) {
            let c = self.border.1.unwrap_or_else(|| env.color("separator").unwrap_or(Color::TRANSPARENT));
            e.set_border(self.border.0.unwrap_or(1.0), c);
        }
        match self.scroll {
            Some((true, true)) => e.set_scroll_xy(),
            Some((true, false)) => e.set_scroll_x(),
            Some((false, _)) => e.set_scroll(ScrollSpec { stick_bottom: self.stick_bottom }),
            None => e,
        };
        if let Some(k) = self.key {
            e.set_key(k);
        }
    }

    /// Decodes one prop onto `e`, or keeps it for `finish`, or says what was wrong with its
    /// value.
    pub(crate) fn decode(&mut self, e: &mut El, atom: Atom, v: Value<'_>, env: &Env, is_text: bool) -> Result<(), String> {
        let name = NAMES[atom as usize].0;
        let num = |v: Value<'_>| match v {
            Value::Num(n) => Ok(n as f32),
            other => Err(format!("{name} must be a number, got {}", self::name(other))),
        };
        let flag = |v: Value<'_>| match v {
            Value::Bool(b) => Ok(b),
            Value::Nil => Ok(false),
            other => Err(format!("{name} must be a boolean, got {}", self::name(other))),
        };
        let color = |v: Value<'_>| match v {
            Value::Str(s) => env.resolve(s),
            other => Err(format!("{name} must be a color token or ui.hex(...), got {}", self::name(other))),
        };
        let string = |v| as_str(atom, v);
        let size = |v: Value<'_>| match v {
            Value::Num(n) => Ok(Size::Points(n as f32)),
            Value::Str("full") => Ok(Size::Full),
            other => Err(format!("{name} must be a number or \"full\", got {}", self::name(other))),
        };
        match atom {
            Atom::Key => {
                self.key = Some(match v {
                    Value::Num(n) => format_num(n),
                    v => string(v)?.to_owned(),
                })
            }
            Atom::W => {
                match size(v)? {
                    Size::Points(n) => e.set_w(n),
                    Size::Full => e.set_w_full(),
                };
            }
            Atom::H => {
                match size(v)? {
                    Size::Points(n) => e.set_h(n),
                    Size::Full => e.set_h_full(),
                };
            }
            Atom::MinW => _ = e.set_min_w(num(v)?),
            Atom::MaxW => _ = e.set_max_w(num(v)?),
            Atom::MinH => _ = e.set_min_h(num(v)?),
            Atom::MaxH => _ = e.set_max_h(num(v)?),
            Atom::Grow => _ = e.set_grow(num(v)?),
            Atom::Shrink => {
                if flag(v)? {
                    e.set_shrink();
                }
            }
            Atom::Pad => self.pad = Some(num(v)?),
            Atom::Px => {
                let n = num(v)?;
                e.set_pl(n).set_pr(n);
                self.sides |= LEFT | RIGHT;
            }
            Atom::Py => {
                let n = num(v)?;
                e.set_pt(n).set_pb(n);
                self.sides |= TOP | BOTTOM;
            }
            Atom::Pt => {
                e.set_pt(num(v)?);
                self.sides |= TOP;
            }
            Atom::Pr => {
                e.set_pr(num(v)?);
                self.sides |= RIGHT;
            }
            Atom::Pb => {
                e.set_pb(num(v)?);
                self.sides |= BOTTOM;
            }
            Atom::Pl => {
                e.set_pl(num(v)?);
                self.sides |= LEFT;
            }
            Atom::M => {
                let n = num(v)?;
                e.set_mt(n).set_mr(n).set_mb(n).set_ml(n);
            }
            Atom::Mx => {
                let n = num(v)?;
                e.set_ml(n).set_mr(n);
            }
            Atom::My => {
                let n = num(v)?;
                e.set_mt(n).set_mb(n);
            }
            Atom::Mt => _ = e.set_mt(num(v)?),
            Atom::Mr => _ = e.set_mr(num(v)?),
            Atom::Mb => _ = e.set_mb(num(v)?),
            Atom::Ml => _ = e.set_ml(num(v)?),
            Atom::Opacity => _ = e.set_opacity(num(v)?),
            Atom::Role => {
                let s = string(v)?;
                self.role = Some(role(s).ok_or_else(|| format!("unknown role {s}"))?);
            }
            Atom::Label => _ = e.set_label(string(v)?),
            Atom::Help => _ = e.set_help(string(v)?),
            Atom::Gap => _ = e.set_gap(num(v)?),
            Atom::Align => {
                match v {
                    Value::Str("start") => e.set_items_start(),
                    Value::Str("center") => e.set_items_center(),
                    Value::Str("end") => e.set_items_end(),
                    Value::Str("stretch") => e.set_items_stretch(),
                    other => return Err(format!("align must be start, center, end or stretch, got {}", self::name(other))),
                };
            }
            Atom::Justify => {
                match v {
                    Value::Str("start") => e.set_justify_start(),
                    Value::Str("center") => e.set_justify_center(),
                    Value::Str("end") => e.set_justify_end(),
                    Value::Str("between") => e.set_justify_between(),
                    other => return Err(format!("justify must be start, center, end or between, got {}", self::name(other))),
                };
            }
            Atom::Center => self.center = flag(v)?,
            Atom::Wrap if is_text => self.wrap = flag(v)?,
            Atom::Wrap => {
                if flag(v)? {
                    e.set_flex_wrap();
                }
            }
            Atom::Fill => _ = e.set_fill(color(v)?),
            Atom::Hover => _ = e.set_hover(color(v)?),
            Atom::Press => _ = e.set_press(color(v)?),
            Atom::Border => self.border.1 = Some(color(v)?),
            Atom::BorderWidth => self.border.0 = Some(num(v)?),
            Atom::Radius => _ = e.set_radius(num(v)?),
            Atom::Clip => {
                if flag(v)? {
                    e.set_clip_soft(0.0);
                }
            }
            Atom::ScrollY | Atom::ScrollX => {
                if flag(v)? {
                    let (x, y) = self.scroll.unwrap_or((false, false));
                    self.scroll = Some(if atom == Atom::ScrollX { (true, y) } else { (x, true) });
                }
            }
            Atom::StickBottom => self.stick_bottom = flag(v)?,
            Atom::Color => self.color = Some(color(v)?),
            Atom::Size => self.text.size = num(v)?,
            Atom::Weight => {
                self.text.weight = match v {
                    Value::Str("regular") => env.weights[0],
                    Value::Str("medium") => env.weights[1],
                    Value::Str("semibold") => env.weights[2],
                    Value::Str("bold") => env.weights[3],
                    Value::Num(n) => n as f32,
                    other => return Err(format!("weight must be regular, medium, semibold or bold, got {}", self::name(other))),
                }
            }
            Atom::HoverTransition => match v {
                Value::Str("instant") => with(e, |e| e.hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)),
                Value::Str("default") | Value::Nil => {}
                other => return Err(format!("hoverTransition must be default or instant, got {}", self::name(other))),
            },
            Atom::HoverGroup => {
                if flag(v)? {
                    with(e, El::hover_group);
                }
            }
            Atom::HoverGroupColor => {
                let c = color(v)?;
                with(e, |e| e.group_color(c));
            }
            Atom::RevealOnHover => {
                if flag(v)? {
                    with(e, |e| e.group_opacity(0.0, 1.0));
                }
            }
            Atom::Shadow => match v {
                Value::Str(s) => {
                    self.uncacheable = true;
                    if let Some(shadows) = env.shadows.as_ref().and_then(|f| f(s, env.dark)) {
                        // In CSS order, the first on top.
                        with(e, |e| shadows.into_iter().rev().fold(e, El::box_shadow));
                    }
                }
                Value::Nil => {}
                other => return Err(format!("shadow must be an elevation name, got {}", self::name(other))),
            },
            Atom::Selected => self.selected = flag(v)?,
            Atom::Focusable => {
                if flag(v)? {
                    with(e, |e| e.focusable(true));
                }
            }
            Atom::Disabled => self.disabled = flag(v)?,
            Atom::HelpSide => {
                if matches!(v, Value::Str("top")) {
                    with(e, El::help_above);
                }
            }
            Atom::FadeOut => self.fade_out = flag(v)?,
            // HTML's `dir`: `ltr` or `rtl` lays the subtree out that way whatever the window's
            // direction, `auto` takes a text's first strong direction.
            Atom::Dir => {
                let dir = match string(v)? {
                    "ltr" => rune::Direction::Ltr,
                    "rtl" => rune::Direction::Rtl,
                    "auto" => rune::Direction::Auto,
                    s => return Err(format!("unknown dir {s}")),
                };
                with(e, |e| e.dir(dir));
            }
            // Tables, read off the stack by `decode_at`.
            Atom::Corners | Atom::Pin => return Err(format!("{name} must be a table, got {}", self::name(v))),
            Atom::LineHeight => self.text = self.text.lh(num(v)?),
            Atom::Tracking => self.text = self.text.tracking(num(v)?),
            Atom::Mono => {
                if flag(v)? {
                    self.text = self.text.mono();
                }
            }
            Atom::Tabular => {
                if flag(v)? {
                    self.text = self.text.features("'tnum' 1");
                }
            }
            Atom::NoWrap => {
                if flag(v)? {
                    self.wrap = false;
                    self.single_line = true;
                }
            }
            Atom::Truncate => self.truncate = flag(v)?,
            Atom::Selectable => self.selectable = flag(v)?,
            Atom::TextAlign => {
                self.text_align = Some(match v {
                    Value::Str("start") => Align::Start,
                    Value::Str("center") => Align::Center,
                    Value::Str("end") => Align::End,
                    other => return Err(format!("textAlign must be start, center or end, got {}", self::name(other))),
                })
            }
            // Read by the node that takes them; elsewhere they mean nothing.
            Atom::OnPress | Atom::OnHover | Atom::Count | Atom::EstimatedRowHeight | Atom::Version | Atom::RenderRow | Atom::RowKey | Atom::RowVersions | Atom::OnVisibleRows => {
                env.warn_once(format!("{name} has no effect on this node"));
            }
        }
        Ok(())
    }

    /// Decodes the prop whose value is at stack slot `idx` onto `e`, reading the table-valued
    /// ones (`corners`, `pin`) off the stack.
    pub(crate) unsafe fn decode_at(&mut self, e: &mut El, state: *mut ffi::lua_State, atom: Atom, idx: c_int, env: &Env, is_text: bool) -> Result<(), String> {
        // SAFETY: the caller passes a valid slot.
        let v = unsafe { read(state, idx) };
        match (atom, v) {
            (Atom::Corners | Atom::Pin, Value::Nil) => Ok(()),
            (Atom::Corners, Value::Other("table")) => {
                // SAFETY: slot `idx` holds a table.
                let r = unsafe { numbers::<4>(state, idx, &[]) };
                self.corners = Some(r.map(|n| n.unwrap_or(0.0)));
                Ok(())
            }
            (Atom::Pin, Value::Other("table")) => {
                // SAFETY: slot `idx` holds a table.
                let [l, t, r, b] = unsafe { numbers::<4>(state, idx, &["left", "top", "right", "bottom"]) };
                e.set_pin(l, t, r, b);
                Ok(())
            }
            _ => self.decode(e, atom, v, env, is_text),
        }
    }

}

/// Runs a by-value builder on `e` in place, for the props Rune has no in-place setter for.
fn with(e: &mut El, f: impl FnOnce(El) -> El) {
    let taken = std::mem::replace(e, rune::el());
    *e = f(taken);
}

/// `N` numbers from the table at `idx`: by name when `names` are given, else items `1..=N`.
unsafe fn numbers<const N: usize>(state: *mut ffi::lua_State, idx: c_int, names: &[&str]) -> [Option<f32>; N] {
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

fn as_str(atom: Atom, v: Value<'_>) -> Result<&str, String> {
    match v {
        Value::Str(s) => Ok(s),
        other => Err(format!("{} must be a string, got {}", NAMES[atom as usize].0, name(other))),
    }
}

fn role(s: &str) -> Option<&'static str> {
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
        "list" => "list",
        "listitem" => "listitem",
        "heading" => "heading",
        "img" => "img",
        "status" => "status",
        "alert" => "alert",
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
    let _ = if n.fract() == 0.0 && n.abs() < 1e15 { write!(out, "{}", n as i64) } else { write!(out, "{n}") };
}
