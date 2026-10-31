//! Live tuning for any Rune app, in the look of a web knobs panel.
//! Knobs live in groups, each tied to the keyed element it tunes. ⌘⇧K opens the panel on the
//! last group used. [`side`] makes it a right-hand side panel that slides in and pushes the page
//! over, every group a section in one scroll; [`layer`] floats it over the window with a tab per
//! group. Hovering a group's tab or header outlines its part of the window. The
//! target button picks a part by pointing at it. With the panel focused (click in it), ↑/↓ pick
//! a knob, ←/→ step it (⇧ for 10×), Tab or [ ] switch groups. Each group has Random, Reset, Undo, Copy (`name = value` lines)
//! and presets, both declared in code and saved from the panel. Esc closes.
//!
//! Values are plain `name -> f32`; the app saves `values`, `saved` and `kept` with its other
//! state. [`Knobs::change`] makes an undoable change ([`Knobs::undo`], [`Knobs::redo`]);
//! [`Knobs::keep`] saves the values Revert goes back to and [`Knobs::differs`] marks.
//! The panel is a dev tool: without the `panel` feature, `layer` draws nothing, `key` handles
//! nothing and `get` returns each knob's default, so a shipped build runs on the constants.
//!
//! ```ignore
//! const RIM: Knob = Knob::new("Rim width", 0.0, 3.0, 1.0);
//! const GROUPS: &[Group] = &[Group::new("Panel", "panel", &[RIM])];
//! panel.border(knobs.get(&RIM), ..);
//! root.child(rune_knobs::layer(&s.knobs, GROUPS, Style::dark(), |s: &mut State| &mut s.knobs))
//! // in on_key: if rune_knobs::key(&mut s.knobs, ev) { return true }
//! ```

use std::collections::{BTreeMap, BTreeSet};

use rune::KeyEvent;

#[cfg(feature = "panel")]
mod icons;
#[cfg(feature = "panel")]
mod rows;
#[cfg(feature = "panel")]
mod side;
#[cfg(feature = "panel")]
mod side_list;
mod style;
mod tree;
#[cfg(feature = "panel")]
mod view;

#[cfg(feature = "panel")]
pub use side::side;
pub use style::Style;
#[cfg(feature = "panel")]
pub use view::actions;
#[cfg(feature = "panel")]
pub use view::inline;
#[cfg(feature = "panel")]
pub use view::layer;

/// The spring the side panel slides in and out on by default; send ⌘⇧K's commit on
/// [`Knobs::slide`] (`tx.motion(s.knobs.slide())`), so the page moves over with the panel.
pub const SLIDE: rune::Spring = rune::Spring::new(0.3, 0.0);
/// [`SLIDE`] as knobs, for a story that tunes it.
pub const SLIDE_TIME: Knob = Knob::new("Knobs slide", 0.1, 0.8, 0.3).label("Panel slide time");
pub const SLIDE_BOUNCE: Knob = Knob::new("Knobs slide bounce", 0.0, 0.4, 0.0).label("Panel slide bounce");
/// How the side panel lays its groups out, for comparing: a tree of pages and their parts,
/// what's on screen first, or a tab per page.
pub const LAYOUT: Knob = Knob::choice("Knobs layout", &["Tree", "On screen", "Tabs"], 0).label("Panel layout").about("How this panel lists its parts: a folding tree, what's on screen first, or a tab per page");
/// How long a section takes to fold or open.
pub const FOLD_TIME: Knob = Knob::new("Knobs fold", 0.05, 0.6, 0.2).label("Section fold time").about("How long a section of this panel takes to fold or open");
/// The side panel's width until its edge is dragged, and the range it can be dragged in.
pub const SIDE_W: f32 = 340.0;
pub const SIDE_MIN: f32 = 280.0;
pub const SIDE_MAX: f32 = 640.0;

/// A button that does one thing, with the key that does it: see [`actions`].
pub struct Action<S> {
    pub cap: &'static str,
    pub label: &'static str,
    pub run: fn(&mut S, &mut rune::Tx),
}

/// A tunable number. `step` snaps it (0 is continuous); `labels` make it a choice between
/// `min`, `min + 1`, … shown as a segmented control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knob {
    /// The knob's key: values and presets are stored under it, so it never changes.
    pub name: &'static str,
    /// What the panel calls it, in plain words, when that differs from `name`.
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub step: f32,
    pub labels: &'static [&'static str],
    /// One plain line on what it changes, shown while it's hovered or the keyboard is on it.
    pub about: &'static str,
    /// One short line per option of a choice, shown while that option is hovered or picked.
    pub hints: &'static [&'static str],
}

/// What an app says about a knob from one table instead of on the knob: a plainer name, one
/// line on what it changes, and a line per option, keyed by the option's label so a reordered
/// knob keeps them right. Hand the table to [`describe`]; a note wins over the knob's own label,
/// about and hints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub name: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub hints: &'static [(&'static str, &'static str)],
}

impl Note {
    pub const fn new(name: &'static str, about: &'static str) -> Self {
        Self { name, label: "", about, hints: &[] }
    }
    pub const fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }
    pub const fn hints(mut self, hints: &'static [(&'static str, &'static str)]) -> Self {
        self.hints = hints;
        self
    }
}

static NOTES: std::sync::OnceLock<&'static [Note]> = std::sync::OnceLock::new();

/// Sets the app's notes on its knobs, once; later calls are ignored.
pub fn describe(notes: &'static [Note]) {
    let _ = NOTES.set(notes);
}

fn note(name: &str) -> Option<&'static Note> {
    NOTES.get()?.iter().find(|n| n.name == name)
}

impl Knob {
    pub const fn new(name: &'static str, min: f32, max: f32, default: f32) -> Self {
        Self { name, label: "", min, max, default, step: 0.0, labels: &[], about: "", hints: &[] }
    }
    /// Shows the knob as `label` and keeps storing it under its name.
    pub const fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }
    /// Says in one plain line what it changes.
    pub const fn about(mut self, about: &'static str) -> Self {
        self.about = about;
        self
    }
    /// One line per option, in order.
    pub const fn hints(mut self, hints: &'static [&'static str]) -> Self {
        self.hints = hints;
        self
    }
    /// What the panel calls it.
    pub fn title(&self) -> &'static str {
        match note(self.name) {
            Some(n) if !n.label.is_empty() => n.label,
            _ if self.label.is_empty() => self.name,
            _ => self.label,
        }
    }
    /// The line on what it changes: its note's, or its own.
    pub fn describe(&self) -> &'static str {
        note(self.name).map(|n| n.about).filter(|a| !a.is_empty()).unwrap_or(self.about)
    }
    /// The line on option `i` of a choice, or "" when there isn't one.
    pub fn hint(&self, i: usize) -> &'static str {
        let own = self.hints.get(i).copied().unwrap_or("");
        let label = self.labels.get(i).copied().unwrap_or("");
        note(self.name).and_then(|n| n.hints.iter().find(|(l, _)| *l == label)).map(|(_, h)| *h).filter(|h| !h.is_empty()).unwrap_or(own)
    }
    pub const fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }
    /// One of `labels`, stored as its index.
    pub const fn choice(name: &'static str, labels: &'static [&'static str], default: usize) -> Self {
        Self { name, label: "", min: 0.0, max: (labels.len().saturating_sub(1)) as f32, default: default as f32, step: 1.0, labels, about: "", hints: &[] }
    }
    fn clamp(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        if self.step > 0.0 { (self.min + ((v - self.min) / self.step).round() * self.step).min(self.max) } else { v }
    }
}

/// Values a group starts from, by knob name; knobs it leaves out take their defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub values: &'static [(&'static str, f32)],
}

/// Knobs for one part of the window: `region` is the key of the element they tune.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Group {
    pub name: &'static str,
    pub region: &'static str,
    pub knobs: &'static [Knob],
    pub presets: &'static [Preset],
    /// What the defaults' chip says ("Default"), when they are a version with a name.
    pub default_label: &'static str,
    /// The page this group is a part of ("Item page"), which the side panel folds as one;
    /// empty for a group that stands alone.
    pub under: &'static str,
}

impl Group {
    pub const fn new(name: &'static str, region: &'static str, knobs: &'static [Knob]) -> Self {
        Self { name, region, knobs, presets: &[], default_label: DEFAULT, under: "" }
    }
    /// Files the group under a page: names must still be unique across all groups.
    pub const fn under(mut self, page: &'static str) -> Self {
        self.under = page;
        self
    }
    /// The page it's on: its own name when it stands alone.
    pub fn page(&self) -> &'static str {
        if self.under.is_empty() { self.name } else { self.under }
    }
    pub const fn presets(mut self, presets: &'static [Preset]) -> Self {
        self.presets = presets;
        self
    }
    /// Names the defaults' chip, as knobs' `defaultLabel`: "Native" when the defaults are
    /// what the platform does.
    pub const fn default_label(mut self, label: &'static str) -> Self {
        self.default_label = label;
        self
    }
}

/// A preset saved from the panel.
pub type Saved = Vec<(String, BTreeMap<String, f32>)>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Knobs {
    /// The layer is showing.
    pub open: bool,
    /// Values that were changed; the rest are at their defaults.
    pub values: BTreeMap<String, f32>,
    /// Presets saved from the panel, by group name, in the order they were saved.
    pub saved: BTreeMap<String, Saved>,
    /// The values as last kept with [`Knobs::keep`]: [`Knobs::revert`] goes back to them and
    /// [`Knobs::differs`] marks a knob moved off them. Knobs left out were kept at their
    /// defaults. The app saves it with `values`.
    pub kept: BTreeMap<String, f32>,
    /// The group the panel shows; the last one used, and the first group until one is picked.
    pub group: Option<String>,
    /// Picking a region with the target button: the panel shows no group meanwhile.
    pub picking: bool,
    /// What the app is set to while other values are tried (a storybook sets it as it opens),
    /// by knob name: choice rows mark that option with a dot. Knobs left out get no mark.
    pub settled: Option<BTreeMap<String, f32>>,
    /// The side panel's width as dragged (0 until it is); the app saves it.
    pub width: f32,
    /// Side panel sections folded, and ones opened, by group or page name, while the panel
    /// is open. Opening it again folds everything but the group last opened (see [`show`]).
    ///
    /// [`show`]: Knobs::show
    pub folded: BTreeSet<String>,
    pub unfolded: BTreeSet<String>,
    /// What has been typed to filter the side panel: the filter field's text, kept in step with
    /// it on every edit. Set it with [`Knobs::set_query`] so the field follows.
    pub query: String,
    /// The filter field is editing: `/` (or a click on it) starts, Esc or Return stops. While
    /// it does, the field owns the keys it edits with.
    pub filtering: bool,
    /// The filter field's model: caret, selection, undo. `query` mirrors its text.
    edit: rune::TextEdit,
    /// The page the side panel's Tabs layout shows, once one is picked.
    pub tab: Option<String>,
    /// The side panel's keyboard row.
    at: Option<tree::Node>,
    /// The long choice showing all its options under its stepper.
    expanded: Option<String>,
    /// The group whose region is outlined: under the pointer while picking, or its tab hovered.
    hover: Option<String>,
    /// The active knob for the keyboard, and whether the keyboard is driving the panel.
    cursor: usize,
    kbd: bool,
    /// The slider under the pointer, and the one being dragged.
    hot: Option<String>,
    /// The option of a choice under the pointer: its knob and index.
    hot_opt: Option<(String, usize)>,
    drag: Option<String>,
    /// The preset each group last applied.
    active: BTreeMap<String, String>,
    undo: Vec<BTreeMap<String, f32>>,
    redo: Vec<BTreeMap<String, f32>>,
    seed: u64,
}

impl Knobs {
    /// The side panel's spring, from [`SLIDE_TIME`] and [`SLIDE_BOUNCE`].
    pub fn slide(&self) -> rune::Spring {
        rune::Spring::new(self.get(&SLIDE_TIME), self.get(&SLIDE_BOUNCE))
    }

    pub fn get(&self, k: &Knob) -> f32 {
        if !cfg!(feature = "panel") {
            return k.default;
        }
        k.clamp(self.values.get(k.name).copied().unwrap_or(k.default))
    }
    pub fn set(&mut self, k: &Knob, v: f32) {
        self.values.insert(k.name.to_owned(), k.clamp(v));
    }

    /// Opens on the last group used, or the first.
    /// The side panel's width, open or not.
    pub fn side_w(&self) -> f32 {
        if self.width > 0.0 { self.width.clamp(SIDE_MIN, SIDE_MAX) } else { SIDE_W }
    }
    /// How much room the side panel takes from the page: its width while open, else 0.
    pub fn side_room(&self) -> f32 {
        if cfg!(feature = "panel") && self.open { self.side_w() } else { 0.0 }
    }
    /// The side panel's fold spring, from [`FOLD_TIME`].
    pub fn fold_spring(&self) -> rune::Spring {
        rune::Spring::new(self.get(&FOLD_TIME), 0.0)
    }
    /// Whether side panel section `name` is folded, given whether it would be by default.
    pub fn is_folded(&self, name: &str, by_default: bool) -> bool {
        if self.folded.contains(name) {
            true
        } else if self.unfolded.contains(name) {
            false
        } else {
            by_default
        }
    }
    /// Folds or opens a side panel section, and remembers it.
    pub fn set_folded(&mut self, name: &str, fold: bool) {
        if fold {
            self.unfolded.remove(name);
            self.folded.insert(name.to_owned());
        } else {
            self.folded.remove(name);
            self.unfolded.insert(name.to_owned());
        }
    }

    /// Opens the panel. Coming from shut, every section starts folded, a list of all the parts,
    /// except the group last used if it was left open (and so its page).
    pub fn show(&mut self) {
        if !self.open {
            let keep = self.group.clone().filter(|g| self.unfolded.contains(g));
            self.folded.clear();
            self.unfolded.clear();
            self.unfolded.extend(keep);
        }
        self.open = true;
        self.picking = false;
        self.kbd = false;
        self.hover = None;
    }
    /// Starts picking a region of the window with the pointer.
    pub fn pick_region(&mut self) {
        self.picking = true;
        self.hover = None;
    }
    /// The group the panel shows, if not picking.
    pub fn current(&self, groups: &[Group]) -> Option<Group> {
        if self.picking {
            return None;
        }
        self.group.as_deref().and_then(|n| groups.iter().find(|g| g.name == n)).or(groups.first()).copied()
    }
    /// Puts the pointer over knob `name` (and option `option` of a choice), for a still or a test.
    pub fn hover(&mut self, name: &str, option: Option<usize>) {
        self.hot = Some(name.to_owned());
        self.hot_opt = option.map(|j| (name.to_owned(), j));
    }
    /// Sets the filter's text, in the field and in `query`.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_owned();
        self.sync_edit();
    }
    /// Brings the field's text to `query`, if the app changed `query` itself.
    pub(crate) fn sync_edit(&mut self) {
        if self.edit.text != self.query {
            self.edit.replace_text(self.query.clone());
        }
    }
    /// Starts filtering, as `/` does: the field takes focus with its text selected, so typing
    /// replaces it.
    pub fn filter(&mut self, query: &str) {
        self.set_query(query);
        self.start_filter();
    }
    pub(crate) fn start_filter(&mut self) {
        self.sync_edit();
        self.filtering = true;
        self.edit.focused = true;
        self.edit.set_selection(0, self.edit.text.len());
    }
    /// Stops filtering; focus goes back to the panel.
    pub(crate) fn stop_filter(&mut self) {
        self.filtering = false;
        self.edit.focused = false;
    }
    /// The field's model, for [`rune::El::on_edit`].
    #[cfg(feature = "panel")]
    pub(crate) fn edit_mut(&mut self) -> &mut rune::TextEdit {
        &mut self.edit
    }
    /// The field's model as the view draws it.
    #[cfg(feature = "panel")]
    pub(crate) fn edit(&self) -> &rune::TextEdit {
        &self.edit
    }
    /// The field changed: `query` follows.
    #[cfg(feature = "panel")]
    pub(crate) fn edited(&mut self) {
        self.query = self.edit.text.clone();
    }
    pub fn close(&mut self) {
        self.open = false;
        self.picking = false;
        self.hover = None;
        self.kbd = false;
        self.stop_filter();
        self.hot = None;
        self.hot_opt = None;
        self.drag = None;
    }
    pub fn toggle(&mut self) {
        if self.open { self.close() } else { self.show() }
    }
    /// Shows `group`'s knobs.
    pub fn pick(&mut self, group: &str) {
        if self.group.as_deref() != Some(group) {
            self.cursor = 0;
        }
        self.group = Some(group.to_owned());
        self.set_folded(group, false);
        self.picking = false;
        self.hover = None;
    }

    /// Keys for the panel while it has focus: ↑/↓ pick a knob, ←/→ step it (⇧ for 10×; a choice
    /// moves one option), Tab, ⇧Tab, `[` and `]` switch groups, Esc closes. True if taken.
    pub fn panel_key(&mut self, groups: &[Group], ev: &KeyEvent) -> bool {
        use rune::Key;
        // Focus left the filter field without the panel hearing of it.
        if self.filtering && !self.edit.focused {
            self.filtering = false;
        }
        if ev.mods.cmd || ev.mods.alt || ev.mods.ctrl || self.filtering {
            return false;
        }
        if ev.key == Key::Escape {
            self.close();
            return true;
        }
        let Some(g) = self.current(groups).or(groups.first().copied()) else { return false };
        let n = g.knobs.len();
        match &ev.key {
            Key::Up | Key::Down if n > 0 => {
                let at = self.cursor.min(n - 1) as i32 + if ev.key == Key::Down { 1 } else { -1 };
                self.cursor = at.rem_euclid(n as i32) as usize;
            }
            Key::Left | Key::Right if n > 0 => self.step(&g.knobs[self.cursor.min(n - 1)], ev.key == Key::Right, ev.mods.shift),
            Key::Tab | Key::Text(_) => {
                let back = match &ev.key {
                    Key::Tab => ev.mods.shift,
                    Key::Text(t) if t == "[" => true,
                    Key::Text(t) if t == "]" => false,
                    _ => return false,
                };
                let i = groups.iter().position(|x| x.name == g.name).unwrap_or(0) as i32;
                let next = (i + if back { -1 } else { 1 }).rem_euclid(groups.len() as i32) as usize;
                self.pick(groups[next].name);
            }
            _ => return false,
        }
        self.kbd = true;
        true
    }

    /// One step of `d` up or down (⇧ for 10×; a choice moves one option), undoable.
    pub(crate) fn step(&mut self, d: &Knob, up: bool, big: bool) {
        let unit = if !d.labels.is_empty() { 1.0 } else if d.step > 0.0 { d.step } else { (d.max - d.min) / 100.0 };
        let unit = if big && d.labels.is_empty() { unit * 10.0 } else { unit };
        let before = self.get(d);
        self.checkpoint();
        self.set(d, before + if up { unit } else { -unit });
        if self.get(d) == before && self.undo.last() == Some(&self.values) {
            self.undo.pop();
        }
    }

    /// Anything Reset would change in `g`.
    pub fn can_reset(&self, g: &Group) -> bool {
        g.knobs.iter().any(|k| (self.get(k) - k.default).abs() > 1e-4) || self.active.get(g.name).is_some_and(|a| self.drifted(g, a))
    }

    /// Remembers the values for Undo, once per change: call it before a change the panel makes
    /// itself (a drag calls it once, as it starts). A new change drops what Redo had.
    pub fn checkpoint(&mut self) {
        if self.undo.last() != Some(&self.values) {
            self.undo.push(self.values.clone());
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }
    /// Sets `k` to `v` so Undo can take it back.
    pub fn change(&mut self, k: &Knob, v: f32) {
        if (self.get(k) - k.clamp(v)).abs() > 1e-6 {
            self.checkpoint();
            self.set(k, v);
        }
    }
    /// Takes back the last change. True if there was one.
    pub fn undo(&mut self) -> bool {
        let Some(v) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.values, v));
        true
    }
    /// Puts back the change Undo took. True if there was one.
    pub fn redo(&mut self) -> bool {
        let Some(v) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.values, v));
        true
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What `k` was when the values were last kept.
    pub fn kept_value(&self, k: &Knob) -> f32 {
        k.clamp(self.kept.get(k.name).copied().unwrap_or(k.default))
    }
    /// `k` has moved off its kept value.
    pub fn differs(&self, k: &Knob) -> bool {
        (self.get(k) - self.kept_value(k)).abs() > 1e-4
    }
    /// Keeps every value as it is now, as what Revert goes back to.
    pub fn keep(&mut self) {
        self.kept = self.values.clone();
    }
    /// Keeps `knobs` as they are now, the rest as they were kept.
    pub fn keep_some(&mut self, knobs: &[Knob]) {
        for k in knobs {
            match self.values.get(k.name) {
                Some(v) => self.kept.insert(k.name.to_owned(), *v),
                None => self.kept.remove(k.name),
            };
        }
    }
    /// Puts `knobs` back to their kept values, undoably.
    pub fn revert(&mut self, knobs: &[Knob]) {
        if !knobs.iter().any(|k| self.differs(k)) {
            return;
        }
        self.checkpoint();
        for k in knobs {
            match self.kept.get(k.name) {
                Some(v) => self.values.insert(k.name.to_owned(), *v),
                None => self.values.remove(k.name),
            };
        }
    }

    /// Each of `g`'s knobs to a random value in its range.
    pub fn random(&mut self, g: &Group) {
        self.checkpoint();
        if self.seed == 0 {
            self.seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
        }
        for k in g.knobs {
            // xorshift64
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            let f = (self.seed >> 40) as f32 / (1u64 << 24) as f32;
            self.set(k, k.min + f * (k.max - k.min));
        }
    }

    /// Back to the active preset, or, already there, to the defaults.
    /// Puts one knob back to its default, undoably.
    pub fn reset_knob(&mut self, d: &Knob) {
        if self.values.contains_key(d.name) {
            self.checkpoint();
            self.values.remove(d.name);
        }
    }

    pub fn reset(&mut self, g: &Group) {
        let at = self.active.get(g.name).cloned();
        match at {
            Some(name) if self.drifted(g, &name) => self.apply(g, &name),
            _ => {
                self.checkpoint();
                for k in g.knobs {
                    self.values.remove(k.name);
                }
                self.active.remove(g.name);
            }
        }
    }

    /// A preset's values for every knob in `g`, defaults filled in; "Default" is the defaults.
    pub fn preset(&self, g: &Group, name: &str) -> Option<BTreeMap<String, f32>> {
        let mut out: BTreeMap<String, f32> = g.knobs.iter().map(|k| (k.name.to_owned(), k.default)).collect();
        if let Some(p) = g.presets.iter().find(|p| p.name == name) {
            out.extend(p.values.iter().map(|(n, v)| ((*n).to_owned(), *v)));
        } else if let Some((_, v)) = self.saved.get(g.name).and_then(|s| s.iter().find(|(n, _)| n == name)) {
            out.extend(v.iter().filter(|(n, _)| g.knobs.iter().any(|k| k.name == *n)).map(|(n, v)| (n.clone(), *v)));
        } else if name != DEFAULT {
            return None;
        }
        Some(out)
    }
    pub fn apply(&mut self, g: &Group, name: &str) {
        let Some(vals) = self.preset(g, name) else { return };
        self.checkpoint();
        for k in g.knobs {
            let v = vals.get(k.name).copied().unwrap_or(k.default);
            if v == k.default {
                self.values.remove(k.name);
            } else {
                self.set(k, v);
            }
        }
        if name == DEFAULT {
            self.active.remove(g.name);
        } else {
            self.active.insert(g.name.to_owned(), name.to_owned());
        }
    }
    /// The preset `g` last applied ("Default" if none).
    pub fn active(&self, g: &Group) -> &str {
        self.active.get(g.name).map_or(DEFAULT, |s| s.as_str())
    }
    /// The values have moved off preset `name`.
    pub fn drifted(&self, g: &Group, name: &str) -> bool {
        let Some(vals) = self.preset(g, name) else { return false };
        g.knobs.iter().any(|k| (self.get(k) - k.clamp(vals[k.name])).abs() > 1e-4)
    }
    /// Saves `g`'s values as a new preset and makes it active.
    pub fn save(&mut self, g: &Group) -> String {
        let list = self.saved.entry(g.name.to_owned()).or_default();
        let n = (1..).find(|n| !list.iter().any(|(s, _)| *s == format!("Saved {n}"))).unwrap_or(1);
        let name = format!("Saved {n}");
        let vals = g.knobs.iter().map(|k| (k.name.to_owned(), self.get(k))).collect();
        self.saved.entry(g.name.to_owned()).or_default().push((name.clone(), vals));
        self.active.insert(g.name.to_owned(), name.clone());
        name
    }
    pub fn delete(&mut self, g: &Group, name: &str) {
        if let Some(list) = self.saved.get_mut(g.name) {
            list.retain(|(n, _)| n != name);
        }
        if self.active.get(g.name).is_some_and(|a| a == name) {
            self.active.remove(g.name);
        }
    }

    /// `name = value` lines for `g`'s knobs; a choice adds its label.
    pub fn code(&self, g: &Group) -> String {
        let line = |k: &Knob| {
            let v = self.get(k);
            match k.labels.get(v as usize) {
                Some(label) => format!("{} = {} ({label})", k.name, trim(v)),
                None => format!("{} = {}", k.name, trim(v)),
            }
        };
        g.knobs.iter().map(line).collect::<Vec<_>>().join("\n")
    }
}

const DEFAULT: &str = "Default";

fn trim(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// ⌘⇧K toggles the layer and Esc closes it. Returns true when it took the key.
pub fn key(k: &mut Knobs, ev: &KeyEvent) -> bool {
    if !cfg!(feature = "panel") {
        return false;
    }
    match ev.key {
        rune::Key::Command('k' | 'K') if ev.mods.shift => k.toggle(),
        rune::Key::Escape if k.open => k.close(),
        _ => return false,
    }
    true
}

/// [`Knobs::panel_key`] as the keys of a playground story's side column, for
/// `rune::Story::side_keys` beside [`inline`]: ↑ ↓ pick a knob, ← → step it (⇧ for 10×), `[`
/// and `]` switch groups. It leaves Esc and Tab to the playground, which leaves the column.
pub fn side_key<S: 'static>(groups: &'static [Group], lens: fn(&mut S) -> &mut Knobs) -> impl Fn(&mut S, &KeyEvent, &mut rune::Tx) -> bool + 'static {
    move |s, ev, tx| {
        if matches!(ev.key, rune::Key::Escape | rune::Key::Tab) {
            return false;
        }
        let taken = lens(s).panel_key(groups, ev);
        if taken {
            tx.instant();
        }
        taken
    }
}

/// Without the `panel` feature the layer is an empty element.
#[cfg(not(feature = "panel"))]
pub fn layer<S: 'static>(_: &Knobs, _: &'static [Group], _: Style, _: fn(&mut S) -> &mut Knobs) -> rune::El {
    rune::el().key("knobs-layer")
}

#[cfg(not(feature = "panel"))]
pub fn inline<S: 'static>(_: &Knobs, _: &'static [Group], _: Style, _: fn(&mut S) -> &mut Knobs, _: bool) -> rune::El {
    rune::el().key("knobs-panel")
}

/// Without the `panel` feature the page shows alone.
#[cfg(not(feature = "panel"))]
pub fn side<S: 'static>(page: rune::El, _: &Knobs, _: &'static [Group], _: Style, _: fn(&mut S) -> &mut Knobs) -> rune::El {
    page
}

#[cfg(not(feature = "panel"))]
pub fn actions<S: 'static>(_: Style, _: &[Action<S>]) -> rune::El {
    rune::el().key("knobs-actions")
}

#[cfg(all(test, feature = "panel"))]
#[path = "tests.rs"]
mod tests;
