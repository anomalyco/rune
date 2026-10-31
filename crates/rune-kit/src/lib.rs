//! rune-kit: generic, themable components for Rune.
//!
//! Components mirror `@opencode/ui`'s names, variants, sizes and states so screens port from
//! the Solid app by name, but they read only semantic tokens from a [`Kit`]. The OpenCode
//! look is one theme (`rune-theme-opencode`), supplied as data; [`Kit::neutral`] renders the
//! same components on Rune's core neutral tokens.
//!
//! Each component is a builder that ends in `.build(&kit) -> El`. Handlers attach through the
//! builder (`.on_press(|s: &mut S| ..)`) so disabled components drop them. `.state(State::Hover)`
//! forces a visual state for galleries and stories; live hover and press come from the runtime.
//!
//! Overlays (popover, menu, select, tooltip, hover card, toast) return their trigger in place
//! and their content as a separate `El` that the view mounts at the root with [`root`], so it
//! draws above later siblings and escapes clipping ancestors.

mod budget;
pub mod css;
pub mod diff;
pub mod glyphs;
pub mod kit;
mod neutral;
mod palette;
pub mod syntax;

mod accordion;
mod actions;
mod badge;
mod card;
mod checkbox;
mod collapsible;
mod combobox;
mod dialog;
mod feedback;
mod field;
mod keybind;
mod list;
#[cfg(feature = "markdown")]
pub mod markdown;
mod menu;
#[cfg(feature = "mermaid")]
pub mod mermaid;
#[cfg(feature = "markdown")]
pub mod notes;
mod overlay;
pub mod popover;
mod popup_list;
mod radio;
mod rolling_text;
mod segmented;
mod select;
pub mod shell;
mod shortcut_recorder;
mod switch;
mod table;
mod text_input;
mod toast;
mod tree;

pub use accordion::{Accordion, AccordionItem};
#[allow(deprecated)]
pub use actions::ButtonSize;
pub use actions::{Button, ButtonVariant, IconButton, IconButtonVariant};
pub use badge::{Badge, BadgeVariant, Divider};
pub use card::Card;
pub use checkbox::Checkbox;
pub use collapsible::Collapsible;
pub use combobox::{Combobox, ComboboxOption, ComboboxState};
pub use dialog::Dialog;
pub use feedback::{Loader, Progress, ProgressCircle, Spinner};
pub use field::Field;
pub use keybind::{Keybind, KeybindVariant};
pub use kit::{Elevation, Glyphs, Kit, Motion, Palette, Radius, StateColors, Type};
pub use list::{List, ListItem};
#[cfg(feature = "markdown")]
pub use markdown::{Case, Highlight, Markdown, MarkdownStyle};
pub use menu::{Menu, MenuEntry, MenuItem, context_menu};
#[cfg(feature = "markdown")]
pub use notes::Note;
pub use overlay::{HoverCard, Placement, Popover, Tooltip, TooltipAppearance, TooltipStyle, body_text, overlay_text, scrim};
pub use palette::{PaletteFocus, PaletteSeed};
pub use popup_list::{PopupItem, PopupList};
pub use radio::{RadioGroup, RadioItem};
pub use rolling_text::RollingText;
pub use segmented::SegmentedControl;
pub use select::{Select, SelectAppearance};
pub use shell::{Page, PaneState, Sidebar, SidebarHeader, SidebarRow, SidebarSection, SplitView, Toolbar};
pub use shortcut_recorder::{ShortcutRecorder, ShortcutRecorderState, shortcut_error_message};
pub use switch::Switch;
pub use table::{Cell, SortDir, Table, TableColumn, TableState, TableStyle};
pub use text_input::{TextArea, TextField, TextInput};
pub use toast::{Toast, ToastRegion};
pub use tree::{FileNode, FileTree, Tree, TreeItem, TreeKey, TreeModel, TreeRow, TreeStyle, file_icon, natural_cmp};

use rune::{Color, El, Shadow, Tx};

/// A forced visual state (Storybook's `data-state`). `Rest` lets the runtime drive hover/press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Rest,
    Hover,
    Pressed,
    Focus,
    Disabled,
}

/// A control's size, named and measured as AppKit's `NSControl.ControlSize` on macOS 27. A
/// push button is 16, 20, 24, 28 or 36 points tall; a switch's track 16, 20, 24, 28 or 36.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]
pub enum ControlSize {
    Mini,
    Small,
    #[default]
    Regular,
    Large,
    ExtraLarge,
}

type Apply = Box<dyn FnOnce(El) -> El>;

/// Handlers collected by a builder and attached to its root once built (unless disabled).
#[derive(Default)]
pub(crate) struct Acts(Vec<Apply>);

impl Acts {
    pub(crate) fn press<S: 'static>(&mut self, f: impl Fn(&mut S) + 'static) {
        self.0.push(Box::new(move |e: El| e.on_press(f)));
    }
    pub(crate) fn press_tx<S: 'static>(&mut self, f: impl Fn(&mut S, &mut Tx) + 'static) {
        self.0.push(Box::new(move |e: El| e.on_press_tx(f)));
    }
    pub(crate) fn apply(self, mut e: El, enabled: bool) -> El {
        if enabled {
            for f in self.0 {
                e = f(e);
            }
        }
        e
    }
}

/// Paints a list of box-shadow layers on `e`.
pub(crate) fn shadows(k: &Kit, mut e: El, list: &[Shadow]) -> El {
    for s in k.shadow_layers(list) {
        e = e.box_shadow(s);
    }
    e
}

/// The press overlay that, composited over the hover overlay the runtime already shows, gives
/// `pressed` over the fill (the web swaps hover for pressed rather than stacking them).
pub(crate) fn press_over(hover: Color, pressed: Color) -> Color {
    if hover.a >= 0.999 {
        return pressed;
    }
    let a = ((pressed.a - hover.a) / (1.0 - hover.a)).clamp(0.0, 1.0);
    // Same hue overlays (black or white ramps) composite exactly; mixed hues come out close.
    pressed.alpha(a)
}

/// Source-over of straight-alpha colours (for forced states and static layering).
pub(crate) fn over(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let mix = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    Color { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), a }
}

/// Mounts overlays above `base`: `base` fills the window and each overlay follows it in paint
/// and hit order. Place popover/menu/select/tooltip contents and toast regions here.
pub fn root(base: El, overlays: impl IntoIterator<Item = El>) -> El {
    let within = base.hover_within_curves();
    let root = rune::el().key("kit-root").w_full().h_full().child(base.w_full().h_full()).children(overlays);
    match within {
        Some((enter, exit)) => root.hover_motion_within(enter, exit),
        None => root,
    }
}
