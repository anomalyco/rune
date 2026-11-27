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

#![forbid(unsafe_code)]

mod budget;
mod containers;
mod controls;
pub mod diff;
mod feedback;
mod grip;
pub mod image_zoom;
pub mod line_selection;
mod list_nav;
#[cfg(feature = "markdown")]
pub mod markdown;
#[cfg(feature = "math")]
pub mod math;
#[cfg(feature = "mermaid")]
pub mod mermaid;
#[cfg(feature = "markdown")]
pub mod notes;
mod overlays;
mod scroll_bar;
pub mod search;
pub mod shell;
pub mod syntax;
mod tokens;
mod tree;
pub mod zoom_pan;

pub use containers::accordion::Accordion;
pub use containers::accordion::AccordionItem;
pub use containers::badge::Badge;
pub use containers::badge::BadgeVariant;
pub use containers::badge::Divider;
pub use containers::card::Card;
pub use containers::collapsible::Collapsible;
pub use containers::keybind::Keybind;
pub use containers::keybind::KeybindVariant;
pub use containers::list::List;
pub use containers::list::ListItem;
pub use containers::rolling_text::RollingText;
pub use containers::table::Cell;
pub use containers::table::SortDir;
pub use containers::table::Table;
pub use containers::table::TableColumn;
pub use containers::table::TableState;
pub use containers::table::TableStyle;
pub use controls::actions::Button;
#[allow(deprecated)]
pub use controls::actions::ButtonSize;
pub use controls::actions::ButtonVariant;
pub use controls::actions::IconButton;
pub use controls::actions::IconButtonVariant;
pub use controls::checkbox::Checkbox;
pub use controls::color_picker::ColorPicker;
pub use controls::color_picker::Hsv;
pub use controls::field::Field;
pub use controls::number_field::NumberField;
pub use controls::number_field::NumberFieldState;
pub use controls::radio::RadioGroup;
pub use controls::radio::RadioItem;
pub use controls::radio::RadioSurface;
pub use controls::segmented::SegmentedControl;
pub use controls::shortcut_recorder::ShortcutRecorder;
pub use controls::shortcut_recorder::ShortcutRecorderState;
pub use controls::shortcut_recorder::shortcut_error_message;
pub use controls::slider::Slider;
pub use controls::switch::Switch;
pub use controls::text_input::TextArea;
pub use controls::text_input::TextField;
pub use controls::text_input::TextInput;
pub use feedback::Loader;
pub use feedback::Progress;
pub use feedback::ProgressCircle;
pub use feedback::Spinner;
pub use image_zoom::ImageZoom;
pub use image_zoom::Preview;
pub use image_zoom::Stage;
pub use image_zoom::StageZoom;
#[cfg(feature = "markdown")]
pub use markdown::Case;
#[cfg(feature = "markdown")]
pub use markdown::CodeFrame;
#[cfg(feature = "mermaid")]
pub use markdown::DiagramFrame;
#[cfg(feature = "markdown")]
pub use markdown::DiffFrame;
#[cfg(feature = "markdown")]
pub use markdown::Highlight;
#[cfg(feature = "markdown")]
pub use markdown::ImageFrame;
#[cfg(feature = "markdown")]
pub use markdown::Markdown;
#[cfg(feature = "markdown")]
pub use markdown::MarkdownStyle;
#[cfg(feature = "markdown")]
pub use markdown::QuoteFrame;
#[cfg(feature = "markdown")]
pub use markdown::TableShrink;
#[cfg(feature = "markdown")]
pub use notes::Note;
pub use overlays::combobox::Combobox;
pub use overlays::combobox::ComboboxEffect;
pub use overlays::combobox::ComboboxEvent;
pub use overlays::combobox::ComboboxOption;
pub use overlays::combobox::ComboboxPhase;
pub use overlays::combobox::ComboboxState;
pub use overlays::dialog::Dialog;
pub use overlays::hover_card::HoverCard;
pub use overlays::menu::Menu;
pub use overlays::menu::MenuEntry;
pub use overlays::menu::MenuItem;
pub use overlays::menu::context_menu;
pub use overlays::overlay::Placement;
pub use overlays::overlay::Popover;
pub use overlays::overlay::Tooltip;
pub use overlays::overlay::TooltipAppearance;
pub use overlays::overlay::TooltipStyle;
pub use overlays::overlay::body_text;
pub use overlays::overlay::overlay_text;
pub use overlays::overlay::scrim;
pub use overlays::overlay::scrim_tx;
pub use overlays::popover;
pub use overlays::popup_list::PopupItem;
pub use overlays::popup_list::PopupList;
pub use overlays::select::Select;
pub use overlays::select::SelectAppearance;
pub use overlays::toast::Toast;
pub use overlays::toast::ToastRegion;
pub use overlays::toast::stack as toast_stack;
pub use overlays::toast::stack::ToastEffect;
pub use overlays::toast::stack::ToastEntry;
pub use overlays::toast::stack::ToastEvent;
pub use overlays::toast::stack::ToastPage;
pub use overlays::toast::stack::ToastPlacement;
pub use overlays::toast::stack::ToastStack;
pub use overlays::toast::stack::ToastStackState;
pub use overlays::toast::stack::ToastTimer;
pub use scroll_bar::ScrollBar;
pub use scroll_bar::ScrollBarStyle;
pub use scroll_bar::scroll_bar;
pub use shell::Page;
pub use shell::PaneState;
pub use shell::Reorder;
pub use shell::ReorderLanding;
pub use shell::Sidebar;
pub use shell::SidebarHeader;
pub use shell::SidebarRow;
pub use shell::SidebarSection;
pub use shell::SplitView;
pub use shell::TabStrip;
pub use shell::Toolbar;
pub use shell::WindowTitlebar;
pub use tokens::css;
pub use tokens::glyphs;
pub use tokens::kit;
pub use tokens::kit::Elevation;
pub use tokens::kit::Glyphs;
pub use tokens::kit::Kit;
pub use tokens::kit::Motion;
pub use tokens::kit::Palette;
pub use tokens::kit::Radius;
pub use tokens::kit::StateColors;
pub use tokens::kit::Type;
pub use tokens::metrics::ButtonMetrics;
pub use tokens::metrics::BySize;
pub use tokens::metrics::CheckboxColors;
pub use tokens::metrics::DialogMetrics;
pub use tokens::metrics::FieldMetrics;
pub use tokens::metrics::IconButtonMetrics;
pub use tokens::metrics::KeybindMetrics;
pub use tokens::metrics::MenuMetrics;
pub use tokens::metrics::Metrics;
pub use tokens::metrics::PopoverMetrics;
pub use tokens::metrics::PopupBackdrop;
pub use tokens::metrics::PopupRowMetrics;
pub use tokens::metrics::ProgressMetrics;
pub use tokens::metrics::SegmentedMetrics;
pub use tokens::metrics::SelectMetrics;
pub use tokens::metrics::StandardSwitchMetrics;
pub use tokens::metrics::SwitchLook;
pub use tokens::metrics::SwitchMetrics;
pub use tokens::metrics::TextAreaMetrics;
pub use tokens::palette::PaletteFocus;
pub use tokens::palette::PaletteSeed;
pub use tree::FileNode;
pub use tree::FileTree;
pub use tree::Tree;
pub use tree::TreeItem;
pub use tree::TreeKey;
pub use tree::TreeModel;
pub use tree::TreeRow;
pub use tree::TreeStyle;
pub use tree::file_icon;
pub use tree::natural_cmp;
pub use zoom_pan::Canvas;
pub use zoom_pan::ZoomPan;

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

/// Source-over of straight-alpha colours (for forced states and static layering), mixed as the
/// window blends the same layers: in sRGB, as a browser does, in a window that blends in sRGB
/// (the default), else in linear light.
pub(crate) fn over(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let srgb = rune::theme::blends_srgb();
    let (enc, dec): (fn(f32) -> f32, fn(f32) -> f32) =
        if srgb { (rune::theme::srgb_encode, rune::theme::srgb_decode) } else { (|c| c, |c| c) };
    let mix = |t: f32, b: f32| dec((enc(t) * top.a + enc(b) * bottom.a * (1.0 - top.a)) / a);
    Color { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), a }
}

/// Mounts overlays above `base`: `base` fills the window and each overlay follows it in paint
/// and hit order. Place dialog/popover/menu/select/tooltip contents and toast regions here.
pub fn root(base: El, overlays: impl IntoIterator<Item = El>) -> El {
    let within = base.hover_within_curves();
    let root = rune::el().key("kit-root").w_full().h_full().child(base.w_full().h_full()).children(overlays);
    match within {
        Some((enter, exit)) => root.hover_motion_within(enter, exit),
        None => root,
    }
}
