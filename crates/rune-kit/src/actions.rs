//! Button and IconButton (`@opencode/ui` actions/button, actions/icon-button).

use rune::{Color, El, Icon, Spring, TextStyle, Tx, icon, row, text};

use crate::{Acts, ControlSize, Kit, RollingText, State, over, press_over, shadows};

/// The older button sizes. A `Button` is 24, 28 or 32 points tall and an `IconButton` 20, 24
/// or 28. For a `Button`, `Small` is `ControlSize::Regular`, `Normal` is `ControlSize::Large`
/// and `Large` sits between `Large` (28) and `ExtraLarge` (36). For an `IconButton` the names
/// match: `Small`, `Regular` and `Large`.
#[deprecated(note = "use `control_size(ControlSize::..)`: for a Button, Small → Regular, Normal → Large, Large → Large or ExtraLarge; for an IconButton, Small → Small, Normal → Regular, Large → Large")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    Small,
    Normal,
    Large,
}

#[allow(deprecated)]
impl Default for ButtonSize {
    fn default() -> ButtonSize {
        ButtonSize::Normal
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    #[default]
    Neutral,
    Danger,
    Warning,
    Outline,
    Contrast,
    Submit,
    Ghost,
    GhostMuted,
    GhostFaint,
    Loading,
}

/// How a button's surface paints: fill, sheen, ring/shadows, overlays and ink.
struct Skin {
    fill: Color,
    sheen: Option<[Color; 2]>,
    shadows: Vec<rune::Shadow>,
    border: Option<(f32, Color)>,
    hover: Color,
    pressed: Color,
    ink: Color,
    icon: Color,
    disabled_opacity: f32,
}

fn skin(k: &Kit, v: ButtonVariant) -> Skin {
    let c = &k.c;
    let e = &k.elevation;
    let neutral = |ink: Color| Skin {
        fill: c.bg_button_neutral,
        sheen: None,
        shadows: e.button_neutral.clone(),
        border: None,
        hover: c.hover,
        pressed: c.pressed,
        ink,
        icon: ink,
        disabled_opacity: 0.5,
    };
    let ghost = |ink: Color, icon: Color| Skin {
        fill: Color::TRANSPARENT,
        sheen: None,
        shadows: Vec::new(),
        border: None,
        hover: c.hover,
        pressed: c.pressed,
        ink,
        icon,
        disabled_opacity: 0.5,
    };
    match v {
        ButtonVariant::Neutral => neutral(c.text_base),
        ButtonVariant::Danger => neutral(c.danger.fg),
        ButtonVariant::Warning => neutral(c.warning.fg),
        ButtonVariant::Outline => Skin { border: Some((1.0, c.border_muted)), ..ghost(c.text_base, c.text_base) },
        ButtonVariant::Contrast => Skin {
            fill: c.bg_contrast,
            sheen: Some([k.light_on(0.2, c.bg_contrast), k.light(0.0)]),
            shadows: e.button_contrast.clone(),
            border: None,
            hover: k.on(c.contrast_hover, c.bg_contrast),
            pressed: k.on(c.contrast_pressed, c.bg_contrast),
            ink: c.text_contrast,
            icon: c.text_contrast,
            disabled_opacity: 0.4,
        },
        ButtonVariant::Submit => Skin {
            fill: c.bg_icon_button_contrast,
            sheen: Some([k.on(c.submit_highlight, c.bg_icon_button_contrast), k.light(0.0)]),
            shadows: e.button_contrast.clone(),
            border: None,
            hover: k.on(c.submit_hover, c.bg_icon_button_contrast),
            pressed: k.on(c.contrast_pressed, c.bg_icon_button_contrast),
            ink: c.submit_fg,
            icon: c.submit_fg,
            disabled_opacity: 0.4,
        },
        ButtonVariant::Ghost => ghost(c.text_base, c.text_base),
        ButtonVariant::GhostMuted => ghost(c.text_muted, c.icon_muted),
        ButtonVariant::GhostFaint => ghost(c.text_faint, c.icon_faint),
        ButtonVariant::Loading => Skin {
            fill: c.layer_02,
            sheen: None,
            shadows: Vec::new(),
            border: Some((0.5, c.border_muted)),
            hover: Color::TRANSPARENT,
            pressed: Color::TRANSPARENT,
            ink: c.text_base,
            icon: c.text_base,
            disabled_opacity: 1.0,
        },
    }
}

impl Skin {
    /// An open menu's trigger: the pressed fill alone. Hover and press overlays on top of it would
    /// draw it lighter than pressed, and brighter still while clicked to close.
    fn hold_pressed(&mut self) {
        self.fill = self.pressed;
        (self.hover, self.pressed) = (Color::TRANSPARENT, Color::TRANSPARENT);
    }
}

/// The press that opens a menu draws the trigger pressed with the hover and press overlays. The
/// menu takes the hover as it opens and the press lets go, so the held fill lands at once: easing
/// in from clear, the trigger dimmed to nearly rest on release and brightened again.
fn held_motion(e: El) -> El {
    e.motion(Spring::NONE)
}

/// Paints `skin` on `e` for `state`. Rest leaves hover/press to the runtime.
fn paint(k: &Kit, mut e: El, s: Skin, state: State, interactive: bool) -> El {
    e = e.focusable(interactive && state != State::Disabled).focus_ring(2.0, 2.5, k.c.border_focus);
    let fill = match state {
        State::Hover => over(s.hover, s.fill),
        State::Pressed => over(s.pressed, s.fill),
        _ => s.fill,
    };
    e = shadows(k, e.fill(fill), &s.shadows);
    if let Some([top, bottom]) = s.sheen {
        e = e.gradient(top, bottom);
    }
    if let Some((w, c)) = s.border {
        e = e.border(w, c);
    }
    if state == State::Rest && interactive {
        e = e.hover(s.hover).press(press_over(s.hover, s.pressed));
    }
    match state {
        State::Focus if interactive => e.outline(2.0, 2.5, k.c.border_focus),
        _ => e,
    }
}

/// A size set with `control_size`, or with the deprecated `size`, which keeps its old metrics.
#[derive(Clone, Copy)]
#[allow(deprecated)]
enum Size {
    Control(ControlSize),
    Legacy(ButtonSize),
}

/// A push button's height, horizontal padding, corner radius and icon size, as macOS 27's
/// `NSButton` push buttons measure. Large and ExtraLarge are capsules there; here they take
/// the theme's larger radii.
fn push_metrics(k: &Kit, size: ControlSize) -> (f32, f32, f32, f32) {
    match size {
        ControlSize::Mini => (16.0, 8.0, k.radius.sm, 12.0),
        ControlSize::Small => (20.0, 10.0, k.radius.sm, 14.0),
        ControlSize::Regular => (24.0, 12.0, k.radius.md, 16.0),
        ControlSize::Large => (28.0, 14.0, k.radius.lg, 16.0),
        ControlSize::ExtraLarge => (36.0, 18.0, k.radius.xl, 16.0),
    }
}

/// What a [`Button`] draws for its label.
enum Label {
    Text,
    Rolling(String),
    Custom(Box<dyn FnOnce(TextStyle, Color) -> El>),
}

/// An accelerator as a menu shows it, the same key on every layout; any other text as given.
fn format_shortcut(s: &str) -> String {
    let s = s.trim();
    if let Ok(sc) = s.parse::<rune::Shortcut>() {
        sc.menu_keycaps().join(if cfg!(target_os = "macos") { "" } else { "+" })
    } else {
        s.to_owned()
    }
}

fn compose_help(help: Option<&str>, label: Option<&str>, shortcut: Option<&str>) -> Option<String> {
    let sc = shortcut.map(format_shortcut).filter(|s| !s.is_empty());
    match (help, label, sc) {
        (Some(h), _, Some(s)) if !h.is_empty() => Some(format!("{h}  {s}")),
        (None, Some(l), Some(s)) if !l.is_empty() => Some(format!("{l}  {s}")),
        (_, _, Some(s)) => Some(s),
        (Some(h), _, None) if !h.is_empty() => Some(h.to_owned()),
        _ => None,
    }
}

/// A text button: `Button::new("Save").variant(ButtonVariant::Contrast).build(&kit)`.
pub struct Button {
    label: String,
    look: Label,
    variant: ButtonVariant,
    size: Size,
    icon: Option<&'static Icon>,
    state: State,
    expanded: bool,
    help: Option<String>,
    shortcut: Option<String>,
    acts: Acts,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Button {
        Button {
            label: label.into(),
            look: Label::Text,
            variant: ButtonVariant::Neutral,
            size: Size::Control(ControlSize::Regular),
            icon: None,
            state: State::Rest,
            expanded: false,
            help: None,
            shortcut: None,
            acts: Acts::default(),
        }
    }
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    /// The button's size, as AppKit's: Regular (24 points tall) unless set.
    pub fn control_size(mut self, s: ControlSize) -> Self {
        self.size = Size::Control(s);
        self
    }
    #[deprecated(note = "use `control_size`: Small → Regular, Normal → Large, Large → Large or ExtraLarge")]
    #[allow(deprecated)]
    pub fn size(mut self, s: ButtonSize) -> Self {
        self.size = Size::Legacy(s);
        self
    }
    pub fn icon(mut self, i: &'static Icon) -> Self {
        self.icon = Some(i);
        self
    }
    /// Draw the label as a [`RollingText`](crate::RollingText) keyed `key`: when the text
    /// changes ("Import 3 items" → "Importing…") only the changed characters roll, and the
    /// button's width springs with the label on its spring. `key` must be unique in the window
    /// and stay the same between builds. The accessible label is still the text.
    pub fn rolling(mut self, key: impl Into<String>) -> Self {
        self.look = Label::Rolling(key.into());
        self
    }
    /// Draw the label with `f`, given the font and ink colour the button would set its text
    /// in: a label with styled spans, a count, or any element. The text given to
    /// [`Button::new`] stays the accessible label.
    ///
    /// ```ignore
    /// Button::new("Import 3 items")
    ///     .label_with(|font, ink| row().gap(4.0).child(text("Import").font(font).color(ink))
    ///         .child(Badge::new("3").build(&k)))
    ///     .build(&k)
    /// ```
    pub fn label_with(mut self, f: impl FnOnce(TextStyle, Color) -> El + 'static) -> Self {
        self.look = Label::Custom(Box::new(f));
        self
    }
    pub fn state(mut self, s: State) -> Self {
        self.state = s;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        if d {
            self.state = State::Disabled;
        }
        self
    }
    /// The trigger of an open menu or popover (`data-expanded`): ghost variants hold the pressed fill,
    /// without hover or press overlays.
    pub fn expanded(mut self, on: bool) -> Self {
        self.expanded = on;
        self
    }
    /// Tooltip text shown after the hover delay (`El::help`).
    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.help = Some(text.into());
        self
    }
    /// A shortcut hint appended to the button's tooltip (e.g. `"Lock  ⌘L"`), formatted like
    /// [`MenuItem::shortcut`](crate::MenuItem::shortcut). Accepts display strings (`"⌘L"`) or
    /// accelerator strings (`"cmd+l"`).
    pub fn shortcut(mut self, s: impl Into<String>) -> Self {
        self.shortcut = Some(s.into());
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        #[allow(deprecated)]
        let (h, px, radius, icon_size, gap, control) = match self.size {
            Size::Legacy(ButtonSize::Small) => (24.0, 9.0, k.radius.sm, 16.0, 6.0, None),
            Size::Legacy(ButtonSize::Normal) => (28.0, 11.0, k.radius.md, 16.0, 6.0, None),
            Size::Legacy(ButtonSize::Large) => (32.0, 15.0, k.radius.md, 16.0, 6.0, None),
            Size::Control(c) => {
                let (h, px, radius, icon_size) = push_metrics(k, c);
                (h, px, radius, icon_size, if c == ControlSize::Mini { 4.0 } else { 6.0 }, Some(c))
            }
        };
        let mut s = skin(k, self.variant);
        let ghostly =
            matches!(self.variant, ButtonVariant::Ghost | ButtonVariant::GhostMuted | ButtonVariant::GhostFaint);
        let held = self.expanded && ghostly && self.state != State::Disabled;
        if held {
            s.hold_pressed();
            if self.variant == ButtonVariant::GhostMuted {
                (s.ink, s.icon) = (k.c.text_base, k.c.icon_base);
            }
        }
        let mut font = k.ty.ui_medium;
        if self.variant == ButtonVariant::GhostMuted {
            font = k.ty.ui;
        }
        // AppKit sets small buttons in 11 points and mini ones in 9.
        let small = if self.variant == ButtonVariant::GhostMuted { k.ty.small } else { k.ty.small_medium };
        match control {
            Some(ControlSize::Small) => font = small,
            Some(ControlSize::Mini) => font = TextStyle { size: 9.0, ..small },
            _ => {}
        }
        let (ink, icon_c) = (s.ink, s.icon);
        let disabled_alpha = s.disabled_opacity;
        let interactive = self.variant != ButtonVariant::Loading;
        let enabled = self.state != State::Disabled && interactive;
        let tip = compose_help(self.help.as_deref(), Some(&self.label), self.shortcut.as_deref());
        let mut e = row().role("button").label(self.label.clone()).h(h).px(px).gap(gap).center().radius(radius);
        if let Some(t) = tip {
            e = e.help(t);
        }
        e = paint(k, e, s, self.state, interactive);
        if held {
            e = held_motion(e);
        }
        if let Some(i) = self.icon {
            let mut ic = icon(i, icon_size).key("icon").color(icon_c);
            if matches!(self.look, Label::Rolling(_)) { ic = ic.width_leads(); }
            e = e.child(ic);
        }
        e = match self.look {
            Label::Text => e.child(text(self.label).key("label").font(font).color(ink)),
            // The button's edges spring on the label's own curve, so the padding holds while
            // the label grows or shrinks.
            Label::Rolling(key) => e.motion(RollingText::SPRING).width_leads()
                .child(RollingText::new(key, self.label).font(font).color(ink).build(k)),
            Label::Custom(f) => e.child(f(font, ink)),
        };
        if self.state == State::Disabled { e = e.dim(disabled_alpha); }
        self.acts.apply(e, enabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IconButtonVariant {
    #[default]
    Neutral,
    Contrast,
    Submit,
    Ghost,
    GhostMuted,
}

/// A square icon-only button: `IconButton::new(&icons::PLUS).build(&kit)`.
pub struct IconButton {
    icon: &'static Icon,
    variant: IconButtonVariant,
    size: Size,
    state: State,
    expanded: bool,
    label: Option<String>,
    help: Option<String>,
    shortcut: Option<String>,
    acts: Acts,
}

impl IconButton {
    pub fn new(i: &'static Icon) -> IconButton {
        IconButton {
            icon: i,
            variant: IconButtonVariant::Neutral,
            size: Size::Control(ControlSize::Regular),
            state: State::Rest,
            expanded: false,
            label: None,
            help: None,
            shortcut: None,
            acts: Acts::default(),
        }
    }
    pub fn variant(mut self, v: IconButtonVariant) -> Self {
        self.variant = v;
        self
    }
    /// A square as tall as a push button of this size: Regular (24 points) unless set.
    pub fn control_size(mut self, s: ControlSize) -> Self {
        self.size = Size::Control(s);
        self
    }
    #[deprecated(note = "use `control_size`: Small → Small, Normal → Regular, Large → Large")]
    #[allow(deprecated)]
    pub fn size(mut self, s: ButtonSize) -> Self {
        self.size = Size::Legacy(s);
        self
    }
    pub fn state(mut self, s: State) -> Self {
        self.state = s;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        if d {
            self.state = State::Disabled;
        }
        self
    }
    pub fn expanded(mut self, on: bool) -> Self {
        self.expanded = on;
        self
    }
    /// Accessible name (the web's `aria-label`).
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// Tooltip text shown after the hover delay (`El::help`). Also supplies the accessible
    /// label when [`Self::label`] was not set.
    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.help = Some(text.into());
        self
    }
    /// A shortcut hint appended to the button's tooltip (e.g. `"Lock  ⌘L"`), formatted like
    /// [`MenuItem::shortcut`](crate::MenuItem::shortcut). Accepts display strings (`"⌘L"`) or
    /// accelerator strings (`"cmd+l"`).
    pub fn shortcut(mut self, s: impl Into<String>) -> Self {
        self.shortcut = Some(s.into());
        self
    }
    pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        #[allow(deprecated)]
        let size = match self.size {
            Size::Legacy(ButtonSize::Small) => ControlSize::Small,
            Size::Legacy(ButtonSize::Normal) => ControlSize::Regular,
            Size::Legacy(ButtonSize::Large) => ControlSize::Large,
            Size::Control(c) => c,
        };
        // The older sizes' radii, so Small, Regular and Large draw as they did.
        let (side, radius, glyph) = match size {
            ControlSize::Mini => (16.0, k.radius.sm, 12.0),
            ControlSize::Small => (20.0, k.radius.sm, 16.0),
            ControlSize::Regular => (24.0, k.radius.md, 16.0),
            ControlSize::Large => (28.0, k.radius.md, 16.0),
            ControlSize::ExtraLarge => (36.0, k.radius.lg, 18.0),
        };
        let mut s = match self.variant {
            IconButtonVariant::Neutral => skin(k, ButtonVariant::Neutral),
            IconButtonVariant::Contrast | IconButtonVariant::Submit => {
                let mut s = skin(k, ButtonVariant::Submit);
                s.icon = k.c.submit_icon;
                s
            }
            IconButtonVariant::Ghost => skin(k, ButtonVariant::Ghost),
            IconButtonVariant::GhostMuted => {
                let mut s = skin(k, ButtonVariant::Ghost);
                s.icon = k.c.icon_muted;
                s
            }
        };
        if self.state == State::Pressed && self.variant == IconButtonVariant::GhostMuted {
            s.icon = k.c.icon_base;
        }
        let held = self.expanded && matches!(self.variant, IconButtonVariant::Ghost | IconButtonVariant::GhostMuted);
        if held {
            s.hold_pressed();
            s.icon = if self.variant == IconButtonVariant::GhostMuted { k.c.icon_base } else { s.icon };
        }
        let ink = s.icon;
        let disabled_alpha = s.disabled_opacity;
        let enabled = self.state != State::Disabled;
        let tip = compose_help(self.help.as_deref(), self.label.as_deref(), self.shortcut.as_deref());
        let mut e = row().role("button").size(side, side).center().radius(radius);
        if let Some(l) = self.label.or(self.help) {
            e = e.label(l);
        }
        if let Some(t) = tip {
            e = e.help(t);
        }
        e = paint(k, e, s, self.state, true).child(icon(self.icon, glyph).key("icon").color(ink));
        if held {
            e = held_motion(e);
        }
        if self.state == State::Disabled { e = e.dim(disabled_alpha); }
        self.acts.apply(e, enabled)
    }
}
