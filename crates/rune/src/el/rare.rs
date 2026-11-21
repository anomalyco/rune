//! What keeps `El` at three cache lines: rarely set fields behind `Cold<ElCold>`, booleans packed
//! into `ElFlags`, and the role as a one-byte `RoleTag`. A plain row, column, box or text
//! allocates none of them.

use std::rc::Rc;

use rune_motion::{Anim, Spring, Transition};

use crate::cold::{Cold, Empty};
use crate::el::{A11y, Anchor, FocusSpec, GrowthSpec, InputSpec, ScrollAxes, ScrollEdges, ScrollSpec, Sticky, WindowDrag};

/// Element fields most elements leave at their defaults.
#[derive(Clone)]
pub(crate) struct ElCold {
    pub(crate) focus: Cold<FocusSpec>,
    pub(crate) looping: Option<Box<rune_motion::LoopSpec>>,
    pub(crate) trace_name: Option<&'static str>,
    pub(crate) trace_namespace: Option<(&'static str, &'static str)>,
    /// A role outside `RoleTag`'s table, read when the tag is `RoleTag::CUSTOM`.
    pub(crate) role: &'static str,
    pub(crate) label: Option<std::borrow::Cow<'static, str>>,
    pub(crate) live: Vec<crate::live::Bound>,
    pub(crate) above: Option<Anchor>,
    pub(crate) motion: Option<Spring>,
    pub(crate) opacity_motion: Option<Box<Anim>>,
    pub(crate) lead: Option<(Spring, Spring)>,
    /// `El::animated_changes`: an ancestor's `El::instant_changes` stops here.
    pub(crate) animated_changes: bool,
    /// `El::ime_option_as_alt`.
    pub(crate) option_as_alt: bool,
    pub(crate) transition: Option<Box<Transition>>,
    pub(crate) matched: Option<Rc<str>>,
    pub(crate) scroll: Option<ScrollSpec>,
    pub(crate) scroll_axes: ScrollAxes,
    /// `El::wheel_scrolls`: the viewport a wheel over this element scrolls, in place of what it
    /// starts on.
    pub(crate) wheel_scrolls: Option<Box<str>>,
    pub(crate) sticky: Option<Box<Sticky>>,
    pub(crate) scroll_edges: ScrollEdges,
    pub(crate) growth: Option<Box<GrowthSpec>>,
    pub(crate) virtual_estimate: Option<f32>,
    /// Rows `El::virtual_rows` builds at layout time, in place of `children`.
    pub(crate) virtual_rows: Option<Rc<crate::el::virtual_rows::RowSource>>,
    /// [`El::virtual_rows_version`](crate::El::virtual_rows_version), hashed.
    pub(crate) virtual_rows_version: Option<u64>,
    /// The memo this element was expanded from (`El::expand_memo`) to mount virtual rows.
    pub(crate) expanded_memo: Option<Rc<crate::memo::Built>>,
    pub(crate) glide: Option<crate::growth::FoldGlide>,
    pub(crate) input: Option<Box<InputSpec>>,
    pub(crate) time_scale: f32,
    pub(crate) help: Option<String>,
    /// `El::help_when_truncated`: the tooltip shows only while the text is cut short.
    pub(crate) help_cut: bool,
    /// `El::help_side`: the side the tooltip shows on; None for under the element.
    pub(crate) help_side: Option<crate::Side>,
    /// `El::help_open`: the tooltip shows whatever the pointer does.
    pub(crate) help_open: bool,
    pub(crate) material: Option<crate::SystemMaterial>,
    /// `El::aspect_fit`'s width over height; 0 when unset.
    pub(crate) aspect_fit: f32,
    /// Outside disc: logical ascent, descent and line height; resolved with this commit's scale.
    pub(crate) list_disc: Option<[f32; 3]>,
    /// `El::level` and `El::expanded`.
    pub(crate) a11y: A11y,
    #[cfg(debug_assertions)]
    pub(crate) scroll_source: Option<&'static std::panic::Location<'static>>,
    #[cfg(debug_assertions)]
    pub(crate) virtual_source: Option<&'static std::panic::Location<'static>>,
}

impl Empty for ElCold {
    const EMPTY: &'static Self = &ElCold {
        focus: Cold::NONE,
        looping: None,
        trace_name: None,
        trace_namespace: None,
        role: "group",
        label: None,
        live: Vec::new(),
        above: None,
        motion: None,
        opacity_motion: None,
        lead: None,
        animated_changes: false,
        option_as_alt: false,
        transition: None,
        matched: None,
        scroll: None,
        scroll_axes: ScrollAxes::Y,
        wheel_scrolls: None,
        sticky: None,
        scroll_edges: ScrollEdges { fade: 0.0, fade_style: crate::el::FadeStyle::SMOOTH, margin: 0.0, anchor: false },
        growth: None,
        virtual_estimate: None,
        virtual_rows: None,
        virtual_rows_version: None,
        expanded_memo: None,
        glide: None,
        input: None,
        time_scale: 1.0,
        help: None,
        help_cut: false,
        help_side: None,
        help_open: false,
        material: None,
        aspect_fit: 0.0,
        list_disc: None,
        a11y: A11y::EMPTY,
        #[cfg(debug_assertions)]
        scroll_source: None,
        #[cfg(debug_assertions)]
        virtual_source: None,
    };
}

/// An element's booleans and small enums, one bit (or two) each.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ElFlags(u16);

impl ElFlags {
    /// This element or anything under it uses `virtualize` or `virtual_rows`.
    pub(crate) const HAS_VIRTUAL: u16 = 1;
    pub(crate) const WIDTH_LEADS: u16 = 1 << 1;
    pub(crate) const ANIMATE_LAYOUT: u16 = 1 << 2;
    pub(crate) const INSTANT_CHANGES: u16 = 1 << 3;
    pub(crate) const VIRTUAL_MOUNT: u16 = 1 << 4;
    pub(crate) const STREAMING: u16 = 1 << 5;
    pub(crate) const WHOLE: u16 = 1 << 6;
    /// `El::ime_cursor`.
    pub(crate) const IME_CURSOR: u16 = 1 << 7;
    pub(crate) const SELECTION_GROUP: u16 = 1 << 8;
    pub(crate) const INERT: u16 = 1 << 9;
    /// The memo placeholder was expanded or given extra children: it shares its cached
    /// children still, but is no longer the cached subtree itself.
    pub(crate) const MEMO_EXTENDED: u16 = 1 << 10;
    const DRAG_SHIFT: u16 = 11;
    const DRAG_MASK: u16 = 0b11 << Self::DRAG_SHIFT;
    /// `El::dir`: none (its parent's), `Ltr`, `Rtl` or `Auto`.
    const DIR_SHIFT: u16 = 13;
    const DIR_MASK: u16 = 0b11 << Self::DIR_SHIFT;
    /// `El::unmirrored`: a right-to-left parent doesn't mirror where it sits.
    pub(crate) const UNMIRRORED: u16 = 1 << 15;

    #[inline(always)]
    pub(crate) fn dir(self) -> Option<crate::Direction> {
        match (self.0 & Self::DIR_MASK) >> Self::DIR_SHIFT {
            1 => Some(crate::Direction::Ltr),
            2 => Some(crate::Direction::Rtl),
            3 => Some(crate::Direction::Auto),
            _ => None,
        }
    }

    #[inline(always)]
    pub(crate) fn set_dir(&mut self, dir: crate::Direction) {
        let bits = match dir {
            crate::Direction::Ltr => 1,
            crate::Direction::Rtl => 2,
            crate::Direction::Auto => 3,
        };
        self.0 = (self.0 & !Self::DIR_MASK) | (bits << Self::DIR_SHIFT);
    }

    #[inline(always)]
    pub(crate) fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    #[inline(always)]
    pub(crate) fn set(&mut self, bit: u16, on: bool) {
        if on { self.0 |= bit } else { self.0 &= !bit }
    }

    #[inline(always)]
    pub(crate) fn window_drag(self) -> WindowDrag {
        match (self.0 & Self::DRAG_MASK) >> Self::DRAG_SHIFT {
            1 => WindowDrag::Drag,
            2 => WindowDrag::NoDrag,
            _ => WindowDrag::None,
        }
    }

    #[inline(always)]
    pub(crate) fn set_window_drag(&mut self, drag: WindowDrag) {
        let bits = match drag {
            WindowDrag::None => 0,
            WindowDrag::Drag => 1,
            WindowDrag::NoDrag => 2,
        };
        self.0 = (self.0 & !Self::DRAG_MASK) | (bits << Self::DRAG_SHIFT);
    }
}

/// An element's accessible role as an index into [`crate::a11y::roles::ROLES`];
/// `CUSTOM` reads `ElCold::role`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RoleTag(u8);

impl RoleTag {
    const fn known(role: &str) -> RoleTag {
        match crate::a11y::roles::index(role) {
            Some(i) => RoleTag(i),
            None => panic!("unknown RoleTag constant"),
        }
    }

    pub(crate) const GROUP: RoleTag = RoleTag::known("group");
    pub(crate) const TEXT: RoleTag = RoleTag::known("text");
    pub(crate) const BUTTON: RoleTag = RoleTag::known("button");
    pub(crate) const ICON: RoleTag = RoleTag::known("icon");
    pub(crate) const IMAGE: RoleTag = RoleTag::known("image");
    pub(crate) const TEXTBOX: RoleTag = RoleTag::known("textbox");
    pub(crate) const SLIDER: RoleTag = RoleTag::known("slider");
    pub(crate) const CUSTOM: RoleTag = RoleTag(u8::MAX);

    /// The tag for `role`, or `None` when it isn't in the canonical role table.
    #[inline]
    pub(crate) fn of(role: &str) -> Option<RoleTag> {
        crate::a11y::roles::ROLES.iter().position(|r| r.name == role).map(|i| RoleTag(i as u8))
    }

    /// The role's name; `CUSTOM` has none of its own.
    #[inline(always)]
    pub(crate) fn name(self) -> Option<&'static str> {
        crate::a11y::roles::ROLES.get(self.0 as usize).map(|r| r.name)
    }
}

impl crate::El {
    /// The accessible role, as [`role`](crate::El::role) set it.
    #[inline]
    pub(crate) fn role_name(&self) -> &'static str {
        self.role.name().unwrap_or(self.cold.role)
    }

    #[inline]
    pub(crate) fn set_role_name(&mut self, role: &'static str) {
        match RoleTag::of(role) {
            Some(tag) => self.role = tag,
            None => {
                self.role = RoleTag::CUSTOM;
                self.cold_mut().role = role;
            }
        }
    }

    /// The rarely set fields, for writing: the first write allocates them.
    #[inline]
    pub(crate) fn cold_mut(&mut self) -> &mut ElCold {
        &mut self.cold
    }
}
