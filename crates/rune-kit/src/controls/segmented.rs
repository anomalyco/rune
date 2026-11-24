//! SegmentedControl (`@opencode/ui` navigation/segmented-control): equal segments over a layer
//! with a sliding raised indicator.

use rune::{El, Icon, Orientation, TextStyle, Tx, el, icon, row, text};

use super::actions::{compose_help, format_shortcut};
use crate::{Kit, State};

/// How wide the control is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Width {
    Points(f32),
    /// The parent's width.
    Fill,
    /// Equal segments, each as wide as the widest segment needs.
    Fit,
}

#[derive(Default)]
struct Segment {
    icon: Option<&'static Icon>,
    trailing: Option<El>,
    help: Option<String>,
    shortcut: Option<String>,
    description: Option<String>,
    controls: Option<String>,
}

type Pick = Box<dyn Fn(usize) -> Box<dyn FnOnce(El) -> El>>;

/// A horizontal row of mutually exclusive segments with a sliding raised indicator.
///
/// Builds a WAI-ARIA `radiogroup` of `radio` segments by default, or a `tablist` of `tab`
/// segments with [`Self::as_tabs`].
///
/// ```
/// # use rune_kit::{Badge, Kit, SegmentedControl};
/// # use rune_theme_opencode::icons;
/// # let k = Kit::neutral(true);
/// # let _ =
/// SegmentedControl::new(&["Comments", "Checks"], Some(0))
///     .icons(&[Some(&icons::COMMENT), Some(&icons::CIRCLE_CHECK)])
///     .trailing(0, Badge::new("3").build(&k))
///     .help(0, "Review comments")
///     .shortcut(0, "cmd+1")
///     .fit()
///     .build(&k)
/// # ;
/// ```
pub struct SegmentedControl {
    labels: Vec<String>,
    selected: Option<usize>,
    width: Width,
    states: Vec<State>,
    segments: Vec<Segment>,
    as_tabs: bool,
    pick: Option<Pick>,
    label: Option<String>,
}

impl SegmentedControl {
    pub fn new<S: AsRef<str>>(labels: &[S], selected: Option<usize>) -> SegmentedControl {
        SegmentedControl {
            labels: labels.iter().map(|l| l.as_ref().to_owned()).collect(),
            selected,
            width: Width::Points(232.0),
            states: Vec::new(),
            segments: Vec::new(),
            as_tabs: false,
            pick: None,
            label: None,
        }
    }

    fn segment_mut(&mut self, i: usize) -> Option<&mut Segment> {
        if i >= self.labels.len() {
            return None;
        }
        if self.segments.len() <= i {
            self.segments.resize_with(i + 1, Segment::default);
        }
        Some(&mut self.segments[i])
    }

    /// The accessible name of the choice the segments make ("View").
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// Width in points (default 232); `None` fills the parent.
    pub fn width(mut self, w: Option<f32>) -> Self {
        self.width = w.map_or(Width::Fill, Width::Points);
        self
    }
    /// Equal segments, each as wide as the widest segment needs (any language, any font).
    pub fn fit(mut self) -> Self {
        self.width = Width::Fit;
        self
    }
    /// Force a segment's visual state (hover, focus, disabled).
    pub fn segment_state(mut self, i: usize, s: State) -> Self {
        if self.states.len() <= i {
            self.states.resize(i + 1, State::Rest);
        }
        self.states[i] = s;
        self
    }
    /// Leading 14 pt icon for segment `i`, tinted to match the segment's ink and hover colour.
    /// Pair with an empty label (`""`) and [`Self::help`] for an icon-only segment.
    ///
    /// ```
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # use rune_theme_opencode::icons;
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["List", "Grid"], Some(0))
    ///     .icon(0, &icons::BULLET_LIST)
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn icon(mut self, i: usize, icon: &'static Icon) -> Self {
        if let Some(seg) = self.segment_mut(i) {
            seg.icon = Some(icon);
        }
        self
    }
    /// Leading 14 pt icons for segments in order (`None` leaves a segment without an icon).
    /// Entries past the number of segments are ignored.
    ///
    /// ```
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # use rune_theme_opencode::icons;
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["", ""], Some(0))
    ///     .icons(&[Some(&icons::BULLET_LIST), Some(&icons::CODE)])
    ///     .help(0, "List")
    ///     .help(1, "Code")
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn icons(mut self, icons: &[Option<&'static Icon>]) -> Self {
        let count = icons.len().min(self.labels.len());
        if self.segments.len() < count {
            self.segments.resize_with(count, Segment::default);
        }
        for (idx, seg) in self.segments.iter_mut().enumerate() {
            seg.icon = icons.get(idx).copied().flatten();
        }
        self
    }
    /// Trailing element rendered after segment `i`'s label (for example a count badge or status
    /// dot), separated by a 6 pt gap.
    ///
    /// ```
    /// # use rune_kit::{Badge, Kit, SegmentedControl};
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["Comments", "Checks"], Some(0))
    ///     .trailing(0, Badge::new("3").build(&k))
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn trailing(mut self, i: usize, el: El) -> Self {
        if let Some(seg) = self.segment_mut(i) {
            seg.trailing = Some(el);
        }
        self
    }
    /// Tooltip text (`El::help`) for segment `i`, combined with [`Self::shortcut`] using the
    /// same formatting as [`Button`](crate::Button) and [`IconButton`](crate::IconButton).
    ///
    /// Supplies the accessible name when the segment's label is empty, and supplies the
    /// accessible description when it differs from a non-empty label and [`Self::description`]
    /// was not set.
    ///
    /// ```
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["Comments", "Checks"], Some(0))
    ///     .help(0, "Review comments")
    ///     .shortcut(0, "cmd+1")
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn help(mut self, i: usize, text: impl Into<String>) -> Self {
        if let Some(seg) = self.segment_mut(i) {
            seg.help = Some(text.into());
        }
        self
    }
    /// Keyboard shortcut hint for segment `i`, appended to its tooltip (e.g. `"Comments  ⌘1"`)
    /// and exposed to assistive technology via `El::shortcut_hint`. Accepts display strings
    /// (`"⌘1"`) or accelerator strings (`"cmd+1"`).
    ///
    /// ```
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["Comments", "Checks"], Some(0))
    ///     .shortcut(0, "cmd+1")
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn shortcut(mut self, i: usize, s: impl Into<String>) -> Self {
        if let Some(seg) = self.segment_mut(i) {
            seg.shortcut = Some(s.into());
        }
        self
    }
    /// Accessible description (`El::description`, ARIA `aria-description`) for segment `i`, for
    /// example naming the meaning of a trailing status dot. Overrides any tooltip-derived
    /// description; pass `""` to suppress tooltip fallback.
    ///
    /// ```
    /// # use rune::el;
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["General", "Security"], Some(0))
    ///     .trailing(1, el().size(6.0, 6.0).radius(3.0).fill(k.c.danger.fg))
    ///     .description(1, "1 security warning")
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn description(mut self, i: usize, text: impl Into<String>) -> Self {
        if let Some(seg) = self.segment_mut(i) {
            seg.description = Some(text.into());
        }
        self
    }
    /// Wire segment `i`'s ARIA `aria-controls` relation (`El::controls`) to the element keyed
    /// `key`, such as a `role("tabpanel")` in [`Self::as_tabs`] mode. An empty key has no effect.
    ///
    /// ```
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["General", "Network"], Some(0))
    ///     .as_tabs()
    ///     .controls(0, "settings-panel")
    ///     .controls(1, "settings-panel")
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn controls(mut self, i: usize, key: impl Into<String>) -> Self {
        if let Some(seg) = self.segment_mut(i) {
            seg.controls = Some(key.into());
        }
        self
    }
    /// Switch accessibility roles from WAI-ARIA Radio Group (`radiogroup` / `radio` with
    /// `aria-checked`) to WAI-ARIA Tabs (`tablist` with horizontal `aria-orientation` / `tab`
    /// with `aria-selected`), keeping roving focus and visual layout unchanged.
    ///
    /// ```
    /// # use rune_kit::{Kit, SegmentedControl};
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// SegmentedControl::new(&["General", "Network", "Security"], Some(0))
    ///     .label("Settings sections")
    ///     .as_tabs()
    ///     .controls(0, "settings-panel")
    ///     .build(&k)
    /// # ;
    /// ```
    pub fn as_tabs(mut self) -> Self {
        self.as_tabs = true;
        self
    }
    pub fn on_pick<S: 'static>(mut self, f: impl Fn(&mut S, usize, &mut Tx) + Clone + 'static) -> Self {
        self.pick = Some(Box::new(move |i| {
            let f = f.clone();
            Box::new(move |e: El| e.on_press_tx(move |s: &mut S, tx: &mut Tx| f(s, i, tx)))
        }));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let n = self.labels.len().max(1);
        let font = TextStyle { line_height: 13.0, ..k.ty.ui };
        let mut root = row()
            .role(if self.as_tabs { "tablist" } else { "radiogroup" })
            .focus_group()
            .h(28.0)
            .center_y()
            .radius(6.0)
            .fill(c.layer_01)
            .box_shadow(rune::Shadow {
                spread: 0.5,
                color: c.border_base,
                ..Default::default()
            });
        if self.as_tabs {
            root = root.orientation(Orientation::Horizontal);
        }
        if let Some(l) = self.label {
            root = root.label(l);
        }
        root = match self.width {
            Width::Points(w) => root.w(w),
            Width::Fill => root.w_full(),
            Width::Fit => root.equal_columns(n),
        };
        let indicator = || {
            el().key("indicator")
                .h(28.0)
                .radius(6.0)
                .fill(c.bg_base)
                .box_shadow(rune::Shadow { spread: 0.5, color: c.border_strong, ..Default::default() })
                .token_motion(k.motion.control)
                .a11y_hidden()
        };
        match (self.selected, self.width) {
            (Some(sel), Width::Points(w)) => {
                let seg = w / n as f32;
                root = root.child(indicator().w(seg).absolute(seg * sel as f32, 0.0));
            }
            // The indicator shares the selected segment's cell and slides between cells.
            (Some(sel), Width::Fit) => root = root.child(indicator().grid_cell(sel)),
            _ => {}
        }
        let seg_role = if self.as_tabs { "tab" } else { "radio" };
        let mut extras = self.segments.into_iter();
        for (i, l) in self.labels.into_iter().enumerate() {
            let extra = extras.next().unwrap_or_default();
            let st = self.states.get(i).copied().unwrap_or_default();
            let on = self.selected == Some(i);
            let ink = if on || st == State::Hover { c.text_base } else { c.text_muted };
            // An unselected segment lights under the pointer on the first frame and fades as it
            // leaves (the runtime's default hover motion); the selected one already reads raised.
            let hovers = st == State::Rest && !on && self.pick.is_some();
            let has_label = !l.trim().is_empty();
            let help_text = extra.help.as_deref().filter(|h| !h.is_empty());
            let shortcut_raw = extra.shortcut.as_deref().filter(|s| !s.trim().is_empty());
            let tip = compose_help(help_text, has_label.then_some(l.as_str()), shortcut_raw);
            let a11y_name = if has_label { Some(l.as_str()) } else { help_text };
            let formatted_shortcut = shortcut_raw.map(format_shortcut).filter(|s| !s.is_empty());

            let mut seg = row()
                .key(format!("segment{i}"))
                .role(seg_role)
                .focusable(st != State::Disabled)
                .focus_selected(on)
                .disabled(st == State::Disabled)
                .position_in_set(i + 1, n)
                .grow(1.0)
                .shrink()
                .h(28.0)
                .px(12.0)
                .gap(6.0)
                .center()
                .radius(6.0);
            if let Some(name) = a11y_name {
                seg = seg.label(name);
            }
            if let Some(t) = tip {
                seg = seg.help(t);
            }
            if let Some(desc) = extra.description {
                seg = seg.description(desc);
            } else if let Some(h) = help_text {
                if has_label && h.trim() != l.trim() {
                    seg = seg.description(h);
                } else {
                    // Help either names an icon-only segment or repeats the visible label; keep it
                    // from echoing as the description via El::help fallback.
                    seg = seg.description("");
                }
            } else if formatted_shortcut.is_some() {
                // Tooltip only carries label + shortcut; keep it from leaking into description.
                seg = seg.description("");
            }
            if let Some(sc) = formatted_shortcut {
                seg = seg.shortcut_hint(sc);
            }
            if let Some(ctrl) = extra.controls {
                seg = seg.controls(ctrl);
            }
            if let Some(ic) = extra.icon {
                let mut glyph = icon(ic, 14.0).key("icon").color(ink).a11y_hidden();
                if hovers {
                    glyph = glyph.group_color(c.text_base);
                }
                seg = seg.child(glyph);
            }
            if !l.is_empty() {
                let mut label = text(l).key("label").font(font).color(ink).truncate().a11y_hidden();
                if hovers {
                    label = label.group_color(c.text_base);
                }
                seg = seg.child(label);
            }
            if let Some(trailing) = extra.trailing {
                seg = seg.child(trailing);
            }
            let mut seg = k.focus_ring_state(seg, 2.0, -2.0, st == State::Focus);
            if self.width == Width::Fit {
                seg = seg.grid_cell(i);
            }
            if self.width == Width::Fill && on {
                seg = seg.fill(c.bg_base).box_shadow(rune::Shadow {
                    spread: 0.5,
                    color: c.border_strong,
                    ..Default::default()
                });
            }
            seg = k.disabled_dim_alpha(seg, st == State::Disabled, 0.45);
            if st == State::Hover && !on {
                seg = seg.fill(c.hover);
            }
            if hovers {
                seg = k.interactive(seg);
            }
            if st != State::Disabled
                && let Some(p) = &self.pick
            {
                seg = p(i)(seg);
            }
            root = root.child(seg);
        }
        root
    }
}
