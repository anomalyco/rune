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

use super::nodes::Children;
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

#[derive(Clone, Copy)]
pub(crate) enum Cross {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy)]
pub(crate) enum Main {
    Start,
    Center,
    End,
    Between,
}

/// A node's decoded props. The element is set up from it in one function, setter after setter
/// on one value: an `El` is several hundred bytes and Rune's setters take it by value, and a
/// straight run of them is what the compiler keeps in place.
pub(crate) struct Style {
    pub(crate) key: Option<String>,
    w: Option<Size>,
    h: Option<Size>,
    min_w: Option<f32>,
    max_w: Option<f32>,
    min_h: Option<f32>,
    max_h: Option<f32>,
    grow: Option<f32>,
    shrink: bool,
    pad: Option<f32>,
    pt: Option<f32>,
    pr: Option<f32>,
    pb: Option<f32>,
    pl: Option<f32>,
    mt: Option<f32>,
    mr: Option<f32>,
    mb: Option<f32>,
    ml: Option<f32>,
    opacity: Option<f32>,
    role: Option<&'static str>,
    label: Option<String>,
    help: Option<String>,
    gap: Option<f32>,
    align: Option<Cross>,
    justify: Option<Main>,
    center: bool,
    flex_wrap: bool,
    fill: Option<Color>,
    hover: Option<Color>,
    press: Option<Color>,
    radius: Option<f32>,
    clip: bool,
    text: TextStyle,
    color: Option<Color>,
    border: (Option<f32>, Option<Color>),
    pub(crate) scroll: Option<(bool, bool)>,
    stick_bottom: bool,
    truncate: bool,
    wrap: bool,
    selectable: bool,
    text_align: Option<Align>,
    instant_hover: bool,
    hover_group: bool,
    group_color: Option<Color>,
    reveal: bool,
    corners: Option<[f32; 4]>,
    pin: Option<[Option<f32>; 4]>,
    shadow: Option<String>,
    selected: bool,
    focusable: bool,
    pub(crate) disabled: bool,
    help_above: bool,
    fade_out: bool,
    single_line: bool,
    pub(crate) on_hover: Option<Rc<Callback>>,
}

impl Style {
    /// A node's props before any are read: text starts from the host's text style.
    pub(crate) fn new(env: &Env) -> Self {
        Style { text: env.text, ..Style::default() }
    }
}

impl Default for Style {
    fn default() -> Self {
        Style {
            key: None,
            w: None,
            h: None,
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
            grow: None,
            shrink: false,
            pad: None,
            pt: None,
            pr: None,
            pb: None,
            pl: None,
            mt: None,
            mr: None,
            mb: None,
            ml: None,
            opacity: None,
            role: None,
            label: None,
            help: None,
            gap: None,
            align: None,
            justify: None,
            center: false,
            flex_wrap: false,
            fill: None,
            hover: None,
            press: None,
            radius: None,
            clip: false,
            text: TextStyle::new(13.0, 400.0),
            color: None,
            border: (None, None),
            scroll: None,
            stick_bottom: false,
            truncate: false,
            wrap: false,
            selectable: false,
            text_align: None,
            instant_hover: false,
            hover_group: false,
            group_color: None,
            reveal: false,
            corners: None,
            pin: None,
            shadow: None,
            selected: false,
            focusable: false,
            disabled: false,
            help_above: false,
            fade_out: false,
            single_line: false,
            on_hover: None,
        }
    }
}

impl Style {
    /// `el` set up with every prop, its children and its press handler. A text node also takes
    /// the text style and colour.
    pub(crate) fn apply(&mut self, el: El, env: &Env, is_text: bool, kids: Option<Children<'_>>, press: Option<Rc<Callback>>, attach: &'static Attach) -> El {
        let mut e = el;
        let s = self;
        if let Some(kids) = kids {
            e = e.children(kids);
        }
        if let Some(cb) = press.filter(|_| !s.disabled) {
            e = (attach.press)(e, cb, s.key.as_deref().map(Into::into), Rc::new([]));
        }
        if let Some(cb) = s.on_hover.take() {
            e = (attach.hover)(e, cb, s.key.as_deref().map(Into::into));
        }
        if let Some(w) = s.w {
            e = match w {
                Size::Points(n) => e.w(n),
                Size::Full => e.w_full(),
            };
        }
        if let Some(h) = s.h {
            e = match h {
                Size::Points(n) => e.h(n),
                Size::Full => e.h_full(),
            };
        }
        if let Some(n) = s.min_w {
            e = e.min_w(n);
        }
        if let Some(n) = s.max_w {
            e = e.max_w(n);
        }
        if let Some(n) = s.min_h {
            e = e.min_h(n);
        }
        if let Some(n) = s.max_h {
            e = e.max_h(n);
        }
        if let Some(n) = s.grow {
            e = e.grow(n);
        }
        if s.shrink {
            e = e.shrink();
        }
        if let Some(n) = s.pad {
            e = e.padding(n);
        }
        if let Some(n) = s.pt {
            e = e.pt(n);
        }
        if let Some(n) = s.pr {
            e = e.pr(n);
        }
        if let Some(n) = s.pb {
            e = e.pb(n);
        }
        if let Some(n) = s.pl {
            e = e.pl(n);
        }
        if let Some(n) = s.mt {
            e = e.mt(n);
        }
        if let Some(n) = s.mr {
            e = e.mr(n);
        }
        if let Some(n) = s.mb {
            e = e.mb(n);
        }
        if let Some(n) = s.ml {
            e = e.ml(n);
        }
        if let Some(n) = s.opacity {
            e = e.opacity(n);
        }
        if let Some(r) = s.role {
            e = e.role(r);
        }
        if let Some(l) = s.label.take() {
            e = e.label(l);
        }
        if let Some(h) = s.help.take() {
            e = e.help(h);
        }
        if let Some(n) = s.gap {
            e = e.gap(n);
        }
        if let Some(a) = s.align {
            e = match a {
                Cross::Start => e.items_start(),
                Cross::Center => e.items_center(),
                Cross::End => e.items_end(),
                Cross::Stretch => e.items_stretch(),
            };
        }
        if let Some(j) = s.justify {
            e = match j {
                Main::Start => e.justify_start(),
                Main::Center => e.justify_center(),
                Main::End => e.justify_end(),
                Main::Between => e.justify_between(),
            };
        }
        if s.center {
            e = e.center();
        }
        if s.flex_wrap {
            e = e.flex_wrap();
        }
        if let Some(c) = s.fill {
            e = e.fill(c);
        }
        if let Some(c) = s.hover {
            e = e.hover(c);
        }
        if let Some(c) = s.press {
            e = e.press(c);
        }
        if let Some(n) = s.radius {
            e = e.radius(n);
        }
        if let Some(r) = s.corners {
            e = e.corners(r);
        }
        if s.clip {
            e = e.clip_soft(0.0);
        }
        if s.instant_hover {
            e = e.hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT);
        }
        if s.hover_group {
            e = e.hover_group();
        }
        if let Some(c) = s.group_color {
            e = e.group_color(c);
        }
        if s.reveal {
            e = e.group_opacity(0.0, 1.0);
        }
        if let Some([l, t, r, b]) = s.pin {
            e = e.pin(l, t, r, b);
        }
        if let Some(shadows) = s.shadow.as_deref().and_then(|name| env.shadows.as_ref().and_then(|f| f(name, env.dark))) {
            // In CSS order, the first on top.
            e = shadows.into_iter().rev().fold(e, El::box_shadow);
        }
        if s.selected {
            if let Some(c) = env.color("selected") {
                e = e.fill(c);
            }
            e = e.focus_selected(true);
        }
        if s.focusable {
            e = e.focusable(true);
        }
        if s.disabled {
            e = e.disabled(true);
        }
        if s.help_above {
            e = e.help_above();
        }
        if !is_text && let Some(c) = s.color {
            e = e.color(c);
        }
        if is_text {
            e = e.font(s.text).color(s.color.unwrap_or_else(|| env.text_color()));
            if s.truncate {
                e = e.truncate();
            }
            if s.fade_out {
                e = e.truncate_fade();
            }
            if s.single_line {
                e = e.single_line();
            }
            if s.wrap {
                e = e.wrap();
            }
            if s.selectable {
                e = e.selectable(true);
            }
            if let Some(a) = s.text_align {
                e = e.align(a);
            }
        }
        if s.border != (None, None) {
            let c = s.border.1.unwrap_or_else(|| env.color("separator").unwrap_or(Color::TRANSPARENT));
            e = e.border(s.border.0.unwrap_or(1.0), c);
        }
        match s.scroll {
            Some((true, true)) => e = e.scroll_xy(),
            Some((true, false)) => e = e.scroll_x(),
            Some((false, _)) => e = e.scroll(ScrollSpec { stick_bottom: s.stick_bottom }),
            None => {}
        }
        if let Some(k) = s.key.take() {
            e = e.key(k);
        }
        e
    }

    /// Decodes one prop, or says what was wrong with its value.
    pub(crate) fn decode(&mut self, atom: Atom, v: Value<'_>, env: &Env, is_text: bool) -> Result<(), String> {
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
            Atom::W => self.w = Some(size(v)?),
            Atom::H => self.h = Some(size(v)?),
            Atom::MinW => self.min_w = Some(num(v)?),
            Atom::MaxW => self.max_w = Some(num(v)?),
            Atom::MinH => self.min_h = Some(num(v)?),
            Atom::MaxH => self.max_h = Some(num(v)?),
            Atom::Grow => self.grow = Some(num(v)?),
            Atom::Shrink => self.shrink = flag(v)?,
            Atom::Pad => self.pad = Some(num(v)?),
            Atom::Px => (self.pl, self.pr) = (Some(num(v)?), Some(num(v)?)),
            Atom::Py => (self.pt, self.pb) = (Some(num(v)?), Some(num(v)?)),
            Atom::Pt => self.pt = Some(num(v)?),
            Atom::Pr => self.pr = Some(num(v)?),
            Atom::Pb => self.pb = Some(num(v)?),
            Atom::Pl => self.pl = Some(num(v)?),
            Atom::M => {
                let n = Some(num(v)?);
                (self.mt, self.mr, self.mb, self.ml) = (n, n, n, n);
            }
            Atom::Mx => (self.ml, self.mr) = (Some(num(v)?), Some(num(v)?)),
            Atom::My => (self.mt, self.mb) = (Some(num(v)?), Some(num(v)?)),
            Atom::Mt => self.mt = Some(num(v)?),
            Atom::Mr => self.mr = Some(num(v)?),
            Atom::Mb => self.mb = Some(num(v)?),
            Atom::Ml => self.ml = Some(num(v)?),
            Atom::Opacity => self.opacity = Some(num(v)?),
            Atom::Role => {
                let s = string(v)?;
                self.role = Some(role(s).ok_or_else(|| format!("unknown role {s}"))?);
            }
            Atom::Label => self.label = Some(string(v)?.to_owned()),
            Atom::Help => self.help = Some(string(v)?.to_owned()),
            Atom::Gap => self.gap = Some(num(v)?),
            Atom::Align => {
                self.align = Some(match v {
                    Value::Str("start") => Cross::Start,
                    Value::Str("center") => Cross::Center,
                    Value::Str("end") => Cross::End,
                    Value::Str("stretch") => Cross::Stretch,
                    other => return Err(format!("align must be start, center, end or stretch, got {}", self::name(other))),
                })
            }
            Atom::Justify => {
                self.justify = Some(match v {
                    Value::Str("start") => Main::Start,
                    Value::Str("center") => Main::Center,
                    Value::Str("end") => Main::End,
                    Value::Str("between") => Main::Between,
                    other => return Err(format!("justify must be start, center, end or between, got {}", self::name(other))),
                })
            }
            Atom::Center => self.center = flag(v)?,
            Atom::Wrap if is_text => self.wrap = flag(v)?,
            Atom::Wrap => self.flex_wrap = flag(v)?,
            Atom::Fill => self.fill = Some(color(v)?),
            Atom::Hover => self.hover = Some(color(v)?),
            Atom::Press => self.press = Some(color(v)?),
            Atom::Border => self.border.1 = Some(color(v)?),
            Atom::BorderWidth => self.border.0 = Some(num(v)?),
            Atom::Radius => self.radius = Some(num(v)?),
            Atom::Clip => self.clip = flag(v)?,
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
            Atom::HoverTransition => {
                self.instant_hover = match v {
                    Value::Str("instant") => true,
                    Value::Str("default") | Value::Nil => false,
                    other => return Err(format!("hoverTransition must be default or instant, got {}", self::name(other))),
                }
            }
            Atom::HoverGroup => self.hover_group = flag(v)?,
            Atom::HoverGroupColor => self.group_color = Some(color(v)?),
            Atom::RevealOnHover => self.reveal = flag(v)?,
            Atom::Shadow => {
                self.shadow = match v {
                    Value::Str(s) => Some(s.to_owned()),
                    Value::Nil => None,
                    other => return Err(format!("shadow must be an elevation name, got {}", self::name(other))),
                }
            }
            Atom::Selected => self.selected = flag(v)?,
            Atom::Focusable => self.focusable = flag(v)?,
            Atom::Disabled => self.disabled = flag(v)?,
            Atom::HelpSide => self.help_above = matches!(v, Value::Str("top")),
            Atom::FadeOut => self.fade_out = flag(v)?,
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
            Atom::OnPress | Atom::OnHover | Atom::Count | Atom::EstimatedRowHeight | Atom::Version | Atom::RenderRow | Atom::RowKey | Atom::OnVisibleRows => {
                env.warn_once(format!("{name} has no effect on this node"));
            }
        }
        Ok(())
    }

    /// Decodes the prop whose value is at stack slot `idx`, reading the table-valued ones
    /// (`corners`, `pin`) off the stack.
    pub(crate) unsafe fn decode_at(&mut self, state: *mut ffi::lua_State, atom: Atom, idx: c_int, env: &Env, is_text: bool) -> Result<(), String> {
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
                self.pin = Some(unsafe { numbers::<4>(state, idx, &["left", "top", "right", "bottom"]) });
                Ok(())
            }
            _ => self.decode(atom, v, env, is_text),
        }
    }
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
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() }
}
