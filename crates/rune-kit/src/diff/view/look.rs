//! Diff palette, colours, typography, and gutter metrics derived from [`Kit`].

use std::hash::{Hash, Hasher};

use rune::{Color, Icon, TextStyle};

use crate::Kit;
use crate::diff::model::LineKind;
use crate::diff::state::{GAP_H, LINE_H};
use crate::syntax::SyntaxTheme;
use crate::tokens::palette::tint;

/// Colours and type, from the kit's palette so themes restyle them.
pub(super) struct Look {
    pub(super) font: TextStyle,
    pub(super) label: TextStyle,
    pub(super) small: TextStyle,
    /// [`FoldRow::LineInfo`]'s count: the interface face at the code's size, on the row's height.
    pub(super) separator_text: TextStyle,
    /// [`FoldRow::LineInfo`]'s row: the background 2% toward black, 5% toward white in dark
    /// (OpenCode's `--diffs-bg-separator-override`).
    pub(super) separator: Color,
    /// The advance of a mono column.
    pub(super) adv: f32,
    pub(super) bg: Color,
    pub(super) text: Color,
    pub(super) muted: Color,
    pub(super) faint: Color,
    pub(super) border: Color,
    pub(super) header_bg: Color,
    pub(super) hover: Color,
    pub(super) add: Tints,
    pub(super) del: Tints,
    pub(super) gap_bg: Color,
    pub(super) empty_bg: Color,
    pub(super) selected: Color,
    pub(super) selected_gutter: Color,
    pub(super) selection: Color,
    pub(super) accent: Color,
    pub(super) on_accent: Color,
    pub(super) triangle_down: &'static Icon,
    pub(super) triangle_right: &'static Icon,
    pub(super) syntax: SyntaxTheme,
    /// Changes when anything above does, for memoised rows.
    pub(super) version: u64,
}

#[derive(Clone, Copy)]
pub(super) struct Tints {
    pub(super) line: Color,
    pub(super) gutter: Color,
    pub(super) word: Color,
    pub(super) number: Color,
    pub(super) bar: Color,
    pub(super) count: Color,
}

impl Look {
    pub(super) fn new(k: &Kit) -> Look {
        let c = &k.c;
        let bg = c.bg_base;
        let dark = k.dark;
        let tints = |hue: Color, fg: Color| Tints {
            line: tint(bg, hue, if dark { 0.13 } else { 0.1 }),
            gutter: tint(bg, hue, if dark { 0.2 } else { 0.17 }),
            word: tint(bg, hue, if dark { 0.32 } else { 0.27 }),
            number: tint(c.text_muted, hue, 0.7),
            bar: hue,
            count: fg,
        };
        let font = TextStyle { size: 13.0, line_height: LINE_H, ..k.ty.mono };
        let info = c.info.icon;
        let mut look = Look {
            font,
            label: TextStyle { size: 13.0, line_height: 20.0, ..k.ty.ui_medium },
            small: TextStyle { size: 12.0, line_height: 16.0, ..k.ty.mono },
            separator_text: TextStyle { size: font.size, line_height: GAP_H, ..k.ty.ui },
            separator: tint(bg, if dark { c.white } else { Color::BLACK }, if dark { 0.05 } else { 0.02 }),
            adv: 0.6 * font.size,
            bg,
            text: c.text_base,
            muted: c.text_muted,
            faint: c.text_faint,
            border: c.border_weak,
            header_bg: c.layer_01,
            hover: k.alpha(c.text_base, if dark { 0.06 } else { 0.035 }),
            add: tints(c.success.icon, c.success.fg),
            del: tints(c.danger.icon, c.danger.fg),
            gap_bg: tint(bg, info, if dark { 0.1 } else { 0.06 }),
            empty_bg: tint(bg, c.text_base, if dark { 0.03 } else { 0.025 }),
            selected: tint(bg, info, if dark { 0.2 } else { 0.14 }),
            selected_gutter: tint(bg, info, if dark { 0.32 } else { 0.26 }),
            selection: k.selection(),
            accent: info,
            on_accent: c.white,
            triangle_down: k.glyph.triangle_down,
            triangle_right: k.glyph.triangle_right,
            syntax: SyntaxTheme::from_kit(k),
            version: 0,
        };
        let mut h = std::hash::DefaultHasher::new();
        for c in [look.bg, look.text, look.add.bar, look.del.bar, look.accent, look.header_bg] {
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
        }
        look.font.size.to_bits().hash(&mut h);
        look.version = h.finish();
        look
    }

    pub(super) fn tints(&self, kind: LineKind) -> Option<&Tints> {
        match kind {
            LineKind::Added => Some(&self.add),
            LineKind::Removed => Some(&self.del),
            LineKind::Context => None,
        }
    }

    pub(super) fn gutter_fill(&self, kind: LineKind, selected: bool) -> Color {
        match (selected, self.tints(kind)) {
            (true, _) => self.selected_gutter,
            (false, Some(t)) => t.gutter,
            (false, None) => self.bg,
        }
    }

    /// One line-number column for numbers of `digits` digits.
    pub(super) fn number_w(&self, digits: u32) -> f32 {
        (digits as f32 * self.adv + 16.0).ceil()
    }
}
