//! `ColorPicker`: a saturation and brightness square, a hue strip, a hex field, suggested
//! swatches and an eyedropper toggle, all working in [`Hsv`].

mod bind;
mod hsv;

use std::cell::Cell;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use rune::{Color, Drag, El, Key, KeyEvent, Phase, Shadow, TextEdit, TextStyle, Tx, col, el, row, text};

use self::bind::{Bind, ChangeFn, Erased, LensFn, erase_lens, state, typed};
pub use self::hsv::Hsv;
use crate::{IconButton, IconButtonVariant, Kit, State, TextInput, glyphs};

type ButtonFn = Box<dyn FnOnce(IconButton) -> IconButton>;
type EditFn = Box<dyn FnOnce(TextInput) -> TextInput>;

struct HexField {
    edit: TextEdit,
    lens: LensFn,
    bind_edit: EditFn,
}

/// A colour picker: drag or arrow through a square of every saturation (across) and brightness
/// (up) of the current hue, drag or step a hue strip under it, type a hex code, press a
/// suggested swatch, or toggle an eyedropper whose picking the app does itself.
///
/// Escape during a drag puts back the colour it started from. The hue strip restores it
/// exactly; the square restores saturation and brightness to 1/4095 (they share the drag's one
/// `f32` of scratch), so at a rounding edge the hex code can come back one step off.
///
/// The picker works in [`Hsv`] so that dragging through grays and black keeps the hue and
/// saturation the square shows. Every change, from any part, goes to [`Self::on_change`].
///
/// For assistive technology the square is a slider whose number is the saturation and whose
/// value text names both saturation and brightness (Up and Down move the brightness), the
/// strip is a slider in degrees, and the swatches are a radio group.
///
/// ```
/// # use rune::TextEdit;
/// # use rune_kit::{ColorPicker, Hsv, Kit};
/// struct App { accent: Hsv, accent_hex: TextEdit, picking: bool }
/// # let s = App { accent: Hsv::from_hex("#3366ff").unwrap(), accent_hex: TextEdit::new("#3366ff"), picking: false };
/// # let kit = Kit::neutral(true);
/// let suggested = ["#e5484d", "#30a46c", "#0090ff"].map(|c| Hsv::from_hex(c).unwrap());
/// let el = ColorPicker::new(s.accent)
///     .label("Accent color")
///     .size(240.0, 150.0)
///     .swatches(suggested)
///     .on_change(|s: &mut App, hsv, _tx| s.accent = hsv)
///     .hex_field(&s.accent_hex, |s: &mut App| &mut s.accent_hex)
///     .eyedropper(s.picking, |s: &mut App, _tx| s.picking = !s.picking)
///     .build(&kit);
/// # let _ = el;
/// ```
pub struct ColorPicker {
    hsv: Hsv,
    label: Option<String>,
    width: f32,
    height: f32,
    swatches: Vec<Hsv>,
    disabled: bool,
    change: Option<ChangeFn>,
    hex: Option<HexField>,
    eyedropper: Option<(bool, ButtonFn)>,
    bind: Option<Rc<dyn Bind>>,
}

/// One keyboard or assistive step of the square and strip: 1% saturation or brightness, 1°.
const STEP: f32 = 0.01;
const STRIP_H: f32 = 12.0;
const THUMB: f32 = 14.0;
const SWATCH: f32 = 20.0;
const ROW_H: f32 = 28.0;

impl ColorPicker {
    /// A picker showing `hsv`, 240 points wide with a 150 point square.
    pub fn new(hsv: Hsv) -> ColorPicker {
        ColorPicker {
            hsv,
            label: None,
            width: 240.0,
            height: 150.0,
            swatches: Vec::new(),
            disabled: false,
            change: None,
            hex: None,
            eyedropper: None,
            bind: None,
        }
    }

    /// The visible label over the picker, and the accessible name of its group.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The picker's width and the square's height, in points (at least 48 each).
    pub fn size(mut self, width: f32, height: f32) -> Self {
        let pts = |x: f32, d: f32| if x.is_finite() { x.max(48.0) } else { d };
        self.width = pts(width, 240.0);
        self.height = pts(height, 150.0);
        self
    }

    /// Suggested colours, drawn as a row of swatches (a radio group) under the hex field. The
    /// one whose code matches the current colour shows as chosen.
    pub fn swatches(mut self, swatches: impl IntoIterator<Item = Hsv>) -> Self {
        self.swatches = swatches.into_iter().collect();
        self
    }

    /// Dims the picker and strips its handlers.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new colour whenever the user changes it: on every drag move (the
    /// square's and strip's live values), each key step, a swatch press, and a whole hex code
    /// typed into [`Self::hex_field`].
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, Hsv, &mut Tx) + 'static) -> Self {
        self.change = Some(Rc::new(move |s: &mut Erased, hsv, tx: &mut Tx| f(state::<S>(s), hsv, tx)));
        self.bind = Some(typed::<S>());
        self
    }

    /// A hex code field, editing `edit` through `lens`. A whole `#rgb` or `#rrggbb` code
    /// typed there changes the colour; Return writes it back as `#rrggbb`, or puts back the
    /// current code if what was typed isn't one. The picker's own changes (square, strip,
    /// swatches) rewrite the field. When the app changes the colour itself (an eyedropper's
    /// pick, an undo), it calls [`ColorPicker::sync_hex`] so the field follows.
    pub fn hex_field<S: 'static>(
        mut self,
        edit: &TextEdit,
        lens: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    ) -> Self {
        let lens = Rc::new(lens);
        let typed_lens = Rc::clone(&lens);
        let bind_edit: EditFn = Box::new(move |t: TextInput| t.on_edit(move |s: &mut S| typed_lens(s)));
        self.hex = Some(HexField { edit: edit.clone(), lens: erase_lens::<S>(move |s| lens(s)), bind_edit });
        self.bind = Some(typed::<S>());
        self
    }

    /// An eyedropper toggle button beside the hex field, shown pressed while `active`. `f` runs
    /// on its press: the app decides what picking means (a press on an image or a canvas it
    /// draws), sets the colour it picks in its own state, and calls [`ColorPicker::sync_hex`]
    /// if it has a hex field.
    pub fn eyedropper<S: 'static>(mut self, active: bool, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.eyedropper = Some((active, Box::new(move |b: IconButton| b.on_press_tx(f))));
        self
    }

    /// Writes `hsv`'s `#rrggbb` code into a hex field's `edit`, unless it already holds a code
    /// for that colour (so a typed `#abc` stays as typed).
    pub fn sync_hex(edit: &mut TextEdit, hsv: Hsv) {
        if Hsv::from_hex(&edit.text).map(Hsv::hex) != Some(hsv.hex()) {
            edit.replace_text(hsv.hex());
        }
    }

    /// Builds the picker with the kit's tokens.
    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let (w, h) = (self.width, self.height);
        let hsv = Hsv { h: clamp_hue(self.hsv.h), s: unit(self.hsv.s), v: unit(self.hsv.v) };
        let live = self.bind.as_ref().filter(|_| !self.disabled).map(|b| Wiring {
            bind: Rc::clone(b),
            live: Rc::new(Cell::new(hsv)),
            change: self.change.clone(),
            lens: self.hex.as_ref().map(|x| Rc::clone(&x.lens)),
        });

        let thumb = |key: &str, x: f32, y: f32| {
            el().key(key.to_owned())
                .size(THUMB, THUMB)
                .absolute(x - THUMB * 0.5, y - THUMB * 0.5)
                .radius(THUMB * 0.5)
                .fill(hsv.to_color())
                .border(2.0, Color::WHITE)
                .box_shadow(Shadow { blur: 2.0, color: Color::BLACK.alpha(0.45), ..Shadow::default() })
                .a11y_hidden()
        };
        let layer =
            |key: &str| el().key(key.to_owned()).size(w, h).absolute(0.0, 0.0).radius(k.radius.sm).a11y_hidden();
        let pure = Hsv { h: hsv.h, s: 1.0, v: 1.0 }.to_color();
        let mut square = el()
            .key("square")
            .role("slider")
            .label("Saturation and brightness")
            .range_value(f64::from(pct(hsv.s)), 0.0, 100.0)
            .range_step(1.0)
            .a11y_value(format!("Saturation {}%, brightness {}%", pct(hsv.s), pct(hsv.v)))
            .size(w, h)
            .radius(k.radius.sm)
            .fill(pure)
            .child(layer("whites").linear_gradient(FRAC_PI_2, [(0.0, Color::WHITE), (1.0, Color::WHITE.alpha(0.0))]))
            .child(layer("blacks").gradient(Color::BLACK.alpha(0.0), Color::BLACK))
            .child(thumb("square-thumb", hsv.s * w, (1.0 - hsv.v) * h));
        square = k.focus_ring(square, 2.0, 2.0);

        let hue_stops: Vec<(f32, Color)> =
            (0..=6).map(|i| (i as f32 / 6.0, Hsv { h: i as f32 * 60.0, s: 1.0, v: 1.0 }.to_color())).collect();
        let mut hue = el()
            .key("hue")
            .role("slider")
            .label("Hue")
            .range_value(f64::from(hsv.h.round()), 0.0, 360.0)
            .range_step(1.0)
            .a11y_value(format!("{}°", hsv.h.round() as i32))
            .size(w, STRIP_H)
            .radius(STRIP_H * 0.5)
            .linear_gradient(FRAC_PI_2, hue_stops)
            .child(thumb("hue-thumb", hsv.h / 360.0 * w, STRIP_H * 0.5).fill(pure));
        hue = k.focus_ring(hue, 2.0, 2.0);

        if let Some(wire) = &live {
            square = wire.square(square);
            hue = wire.hue(hue);
        } else {
            square = square.focusable(!self.disabled).disabled(self.disabled);
            hue = hue.focusable(!self.disabled).disabled(self.disabled);
        }

        let mut root = col().role("group").gap(10.0).w(w).items_start();
        if let Some(l) = &self.label {
            root = root.label(l.clone()).child(
                text(l.clone())
                    .key("label")
                    .font(TextStyle { line_height: 16.0, ..k.ty.small })
                    .color(if self.disabled { c.text_muted } else { c.text_base })
                    .a11y_hidden(),
            );
        }
        root = root.child(square).child(el().key("hue-pad").w(w).py((THUMB - STRIP_H) * 0.5).child(hue));

        if self.hex.is_some() || self.eyedropper.is_some() {
            let mut tools = row().key("tools").w(w).h(ROW_H).gap(8.0).center_y().child(
                el().key("preview")
                    .size(ROW_H, ROW_H)
                    .radius(k.radius.sm)
                    .fill(hsv.to_color())
                    .border(1.0, c.border_weak)
                    .a11y_hidden(),
            );
            let button_w = if self.eyedropper.is_some() { ROW_H + 8.0 } else { 0.0 };
            if let Some(hex) = self.hex {
                let mut field = TextInput::new(&hex.edit)
                    .label("Hex")
                    .placeholder("#rrggbb")
                    .height(ROW_H)
                    .width(Some((w - ROW_H - 8.0 - button_w).max(48.0)))
                    .disabled(self.disabled);
                if let Some(wire) = &live {
                    field = (hex.bind_edit)(field);
                    field = wire.hex(field, hsv, Rc::clone(&hex.lens));
                }
                tools = tools.child(field.build(k).key("hex"));
            }
            if let Some((active, bind_press)) = self.eyedropper {
                let mut b = IconButton::new(&glyphs::EYEDROPPER)
                    .variant(IconButtonVariant::Ghost)
                    .control_size(crate::ControlSize::Large)
                    .label("Eyedropper")
                    .help("Pick a color");
                if active && !self.disabled {
                    b = b.state(State::Pressed);
                }
                b = b.disabled(self.disabled);
                if !self.disabled {
                    b = bind_press(b);
                }
                tools = tools.child(b.build(k).key("eyedropper").checked(active));
            }
            root = root.child(tools);
        }

        if !self.swatches.is_empty() {
            let n = self.swatches.len();
            let current = hsv.hex();
            let mut group = row()
                .key("swatches")
                .role("radiogroup")
                .label("Suggested colors")
                .focus_group()
                .gap(6.0)
                .w(w)
                .flex_wrap();
            for (i, sw) in self.swatches.into_iter().enumerate() {
                let code = sw.hex();
                let chosen = code == current;
                let mut e = el()
                    .key(format!("swatch{i}"))
                    .role("radio")
                    .label(code)
                    .size(SWATCH, SWATCH)
                    .radius(k.radius.sm)
                    .fill(sw.to_color())
                    .border(1.0, c.border_weak)
                    .position_in_set(i + 1, n)
                    .focusable(!self.disabled)
                    .focus_selected(chosen)
                    .checked(chosen)
                    .disabled(self.disabled);
                if chosen {
                    e = e.outline(2.0, 2.0, c.border_strong);
                }
                e = k.focus_ring(e, 2.0, 2.0);
                if let Some(wire) = &live {
                    e = wire.swatch(e, sw.keep_hue(hsv));
                }
                group = group.child(e);
            }
            root = root.child(group);
        }
        k.disabled_dim(root.disabled(self.disabled), self.disabled)
    }
}

fn unit(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

fn clamp_hue(h: f32) -> f32 {
    if h.is_nan() { 0.0 } else { h.clamp(0.0, 360.0) }
}

fn pct(x: f32) -> i32 {
    (x * 100.0).round() as i32
}

/// Rounds to the keyboard's 1% grid.
fn snap(x: f32) -> f32 {
    unit((x / STEP).round() * STEP)
}

/// A drag's starting saturation and value in its one `f32` of scratch: 12 bits each, an
/// integer under 2^24 so it is exact.
fn pack(s: f32, v: f32) -> f32 {
    let q = |x: f32| (unit(x) * 4095.0).round();
    q(s) * 4096.0 + q(v)
}

fn unpack(data: f32) -> (f32, f32) {
    let n = data.max(0.0) as u32;
    ((n / 4096) as f32 / 4095.0, (n % 4096) as f32 / 4095.0)
}

/// The handlers of one build: what changes the colour, and the colour the keyboard last set.
struct Wiring {
    bind: Rc<dyn Bind>,
    live: Rc<Cell<Hsv>>,
    change: Option<ChangeFn>,
    lens: Option<LensFn>,
}

impl Wiring {
    /// A change from the picker's own parts: the hex field follows, then the app hears it.
    fn emitter(&self) -> impl Fn(&mut Erased, Hsv, &mut Tx) + 'static {
        let (change, lens, live) = (self.change.clone(), self.lens.clone(), Rc::clone(&self.live));
        move |s, hsv, tx| {
            live.set(hsv);
            if let Some(lens) = &lens {
                ColorPicker::sync_hex(lens(s), hsv);
            }
            if let Some(f) = &change {
                f(s, hsv, tx);
            }
        }
    }

    fn square(&self, e: El) -> El {
        let emit = Rc::new(self.emitter());
        let live = Rc::clone(&self.live);
        let at = |d: &Drag, base: Hsv| Hsv {
            s: if d.w > 0.0 { unit(d.x / d.w) } else { 0.0 },
            v: if d.h > 0.0 { 1.0 - unit(d.y / d.h) } else { 0.0 },
            ..base
        };
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self.bind.drag(
            e,
            Rc::new(move |s, d, tx| {
                let base = lv.get();
                match d.phase {
                    Phase::Down => {
                        d.data = pack(base.s, base.v);
                        em(s, at(d, base), tx);
                    }
                    Phase::Move => {
                        tx.instant();
                        em(s, at(d, base), tx);
                    }
                    Phase::Up if d.cancelled() => {
                        let (sat, val) = unpack(d.data);
                        em(s, Hsv { s: sat, v: val, ..base }, tx);
                    }
                    Phase::Up => em(s, at(d, base), tx),
                }
            }),
        );
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self.bind.key(
            e,
            Rc::new(move |s, ev: &KeyEvent, tx| {
                if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt {
                    return false;
                }
                let step = if ev.mods.shift { STEP * 10.0 } else { STEP };
                let (ds, dv) = match ev.key {
                    Key::Left => (-step, 0.0),
                    Key::Right => (step, 0.0),
                    Key::Up => (0.0, step),
                    Key::Down => (0.0, -step),
                    Key::PageUp => (0.0, STEP * 10.0),
                    Key::PageDown => (0.0, -STEP * 10.0),
                    _ => return false,
                };
                let cur = lv.get();
                em(s, Hsv { s: snap(cur.s + ds), v: snap(cur.v + dv), ..cur }, tx);
                true
            }),
        );
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self.bind.step(
            e,
            Rc::new(move |s, delta, tx| {
                let cur = lv.get();
                em(s, Hsv { s: snap(cur.s + delta as f32 * STEP), ..cur }, tx);
            }),
        );
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self
            .bind
            .step_limit(e, Rc::new(move |s, max, tx| em(s, Hsv { s: if max { 1.0 } else { 0.0 }, ..lv.get() }, tx)));
        self.bind.set_value(
            e,
            Rc::new(move |s, target, tx| emit(s, Hsv { s: unit(target as f32 / 100.0), ..live.get() }, tx)),
        )
    }

    fn hue(&self, e: El) -> El {
        let emit = Rc::new(self.emitter());
        let live = Rc::clone(&self.live);
        let at = |d: &Drag, base: Hsv| Hsv { h: if d.w > 0.0 { unit(d.x / d.w) * 360.0 } else { 0.0 }, ..base };
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self.bind.drag(
            e,
            Rc::new(move |s, d, tx| {
                let base = lv.get();
                match d.phase {
                    Phase::Down => {
                        d.data = base.h;
                        em(s, at(d, base), tx);
                    }
                    Phase::Move => {
                        tx.instant();
                        em(s, at(d, base), tx);
                    }
                    Phase::Up if d.cancelled() => em(s, Hsv { h: d.data, ..base }, tx),
                    Phase::Up => em(s, at(d, base), tx),
                }
            }),
        );
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self.bind.step(
            e,
            Rc::new(move |s, delta, tx| {
                let cur = lv.get();
                em(s, Hsv { h: clamp_hue(cur.h.round() + delta as f32), ..cur }, tx);
            }),
        );
        let (em, lv) = (Rc::clone(&emit), Rc::clone(&live));
        let e = self
            .bind
            .step_limit(e, Rc::new(move |s, max, tx| em(s, Hsv { h: if max { 360.0 } else { 0.0 }, ..lv.get() }, tx)));
        self.bind
            .set_value(e, Rc::new(move |s, target, tx| emit(s, Hsv { h: clamp_hue(target as f32), ..live.get() }, tx)))
    }

    fn swatch(&self, e: El, color: Hsv) -> El {
        let emit = self.emitter();
        self.bind.press(e, Rc::new(move |s, tx| emit(s, color, tx)))
    }

    /// Typing changes the colour without touching the field; Return tidies the field.
    fn hex(&self, t: TextInput, current: Hsv, lens: LensFn) -> TextInput {
        let change = self.change.clone();
        let typed_lens = Rc::clone(&lens);
        let typed_change = change.clone();
        let on_type = Rc::new(move |s: &mut Erased, tx: &mut Tx| {
            let Some(next) = Hsv::from_hex(&typed_lens(s).text) else { return };
            let next = next.keep_hue(current);
            if next.hex() != current.hex()
                && let Some(f) = &typed_change
            {
                f(s, next, tx);
            }
        });
        let on_submit = Rc::new(move |s: &mut Erased, tx: &mut Tx| {
            let edit = lens(s);
            match Hsv::from_hex(&edit.text) {
                Some(next) => {
                    let next = next.keep_hue(current);
                    edit.replace_text(next.hex());
                    if next.hex() != current.hex()
                        && let Some(f) = &change
                    {
                        f(s, next, tx);
                    }
                }
                None => edit.replace_text(current.hex()),
            }
        });
        self.bind.hex(t, on_type, on_submit)
    }
}

#[cfg(test)]
mod tests;
