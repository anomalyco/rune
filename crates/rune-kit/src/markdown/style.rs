//! [`MarkdownStyle`]: how markdown looks, and its builders.

use rune::{Icon, Spring};

use super::*;
use crate::PaletteSeed;

/// How markdown looks. [`MarkdownStyle::from_kit`] takes every colour and font from the kit's
/// tokens, [`MarkdownStyle::neutral`] and [`MarkdownStyle::from_seed`] need no kit; each builder
/// overrides one thing.
#[derive(Clone, Debug)]
pub struct MarkdownStyle {
    pub(super) text: Color,
    pub(super) muted: Color,
    pub(super) accent: Color,
    pub(super) link_underline: Color,
    pub(super) code_bg: Color,
    pub(super) code_block_fill: Color,
    pub(super) code_block_border: Color,
    pub(super) quote_rule: Color,
    pub(super) rule: Color,
    pub(super) body: TextStyle,
    pub(super) headings: [TextStyle; 3],
    pub(super) code_block: TextStyle,
    pub(super) inline_code_scale: f32,
    pub(super) inline_code_weight: f32,
    pub(super) inline_code_pad: f32,
    pub(super) code_block_pad: [f32; 2],
    pub(super) list_indent: f32,
    pub(super) list_gap: f32,
    pub(super) strong_weight: f32,
    pub(super) table_outline: Option<Color>,
    pub(super) table_rule: Color,
    pub(super) table_head_fill: Color,
    pub(super) table_head_text: Color,
    pub(super) table_head_rule: Option<Color>,
    pub(super) table_head_weight: Option<f32>,
    pub(super) table_min_column: f32,
    pub(super) table_tabular: bool,
    /// Cells' font before tabular figures and the header's weight (`None`: the body's).
    pub(super) table_font: Option<TextStyle>,
    /// Cell padding (x, y), and whether the first column has none on its left.
    pub(super) table_pad: [f32; 2],
    pub(super) table_flush_left: bool,
    pub(super) table_frame: Option<TableFrame>,
    pub(super) table_body: Option<TableBody>,
    /// Header cell padding (x, y) when it differs from the body's.
    pub(super) table_head_pad: Option<[f32; 2]>,
    pub(super) table_full_width: bool,
    /// Space above and below a table (collapsed with its neighbours' like CSS margins).
    pub(super) table_margin: [f32; 2],
    pub(super) diff_add: Color,
    pub(super) diff_remove: Color,
    pub(super) diff_add_text: Color,
    pub(super) diff_remove_text: Color,
    pub(super) diff_hues: [Color; 2],
    pub(super) diff_hunk: Color,
    pub(super) syntax: SyntaxTheme,
    pub(super) highlight: bool,
    pub(super) selection: Color,
    pub(super) highlight_bg: Color,
    pub(super) highlight_fg: Option<Color>,
    pub(super) highlight_ring: Option<(Color, f32)>,
    pub(super) highlight_current_bg: Color,
    pub(super) highlight_current_fg: Option<Color>,
    pub(super) highlight_current_ring: Option<(Color, f32)>,
    pub(super) task: TaskPaint,
    pub(crate) diagram: DiagramPaint,
}

/// Colours, icon and motion of a GFM task list checkbox (matching [`crate::Checkbox`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct TaskPaint {
    pub(super) fill_on: Color,
    pub(super) border_on: Color,
    pub(super) border_off: Color,
    pub(super) hover: Color,
    pub(super) focus_ring: Color,
    pub(super) mark: Color,
    pub(super) check: &'static Icon,
    pub(super) radius: f32,
    pub(super) motion: Spring,
}

/// A table's rounded frame (see [`MarkdownStyle::table_frame`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct TableFrame {
    pub(super) fill: Color,
    pub(super) ring: Color,
    pub(super) pad: f32,
}

/// The card a table's body rows sit on (see [`MarkdownStyle::table_body`]).
#[derive(Clone, Debug)]
pub(super) struct TableBody {
    pub(super) fill: Color,
    pub(super) radius: f32,
    pub(super) shadows: Vec<rune::Shadow>,
}

/// Colours and sizes diagrams draw with (see [`MarkdownStyle::diagram`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DiagramPaint {
    pub text: Color,
    pub muted: Color,
    pub surface: Color,
    pub surface_border: Color,
    pub node_fill: Color,
    pub node_border: Color,
    pub line: Color,
    pub cluster_fill: Color,
    pub note_fill: Color,
    pub note_border: Color,
    pub font: TextStyle,
    pub max_width: f32,
    pub hover: Color,
    pub hover_neighbours: bool,
}

/// `t` of the way from `base` toward `hue` in Oklab, at `base`'s alpha: a quiet wash that keeps
/// `base`'s lightness close in both themes.
pub(super) fn tint(base: Color, hue: Color, t: f32) -> Color {
    let [a, b] = [base.to_oklab(), hue.to_oklab()];
    Color::from_oklab([0, 1, 2, 3].map(|i| if i == 3 { a[3] } else { a[i] + (b[i] - a[i]) * t }))
}

impl MarkdownStyle {
    pub fn from_kit(k: &Kit) -> MarkdownStyle {
        let c = &k.c;
        let body = TextStyle { line_height: 22.4, ..k.ty.body };
        let heading = |size, lh| TextStyle { size, weight: 600.0, line_height: lh, ..body };
        MarkdownStyle {
            text: c.text_base,
            muted: c.text_muted,
            accent: c.text_accent,
            link_underline: k.alpha(c.text_accent, 0.4),
            code_bg: k.alpha(c.text_base, 0.08),
            code_block_fill: c.layer_01,
            code_block_border: c.border_muted,
            quote_rule: c.layer_03,
            rule: c.border_base,
            body,
            headings: [heading(17.0, 24.0), heading(15.0, 24.0), heading(13.0, 20.0)],
            code_block: TextStyle { size: 13.0, line_height: 20.0, ..k.ty.mono },
            inline_code_scale: 0.9,
            inline_code_weight: 400.0,
            inline_code_pad: 3.5,
            code_block_pad: [12.0, 12.0],
            list_indent: 32.0,
            list_gap: 4.0,
            strong_weight: 670.0,
            table_outline: Some(c.border_muted),
            table_rule: c.border_muted,
            table_head_fill: c.layer_01,
            table_head_text: c.text_base,
            table_head_rule: None,
            table_head_weight: None,
            table_min_column: 120.0,
            table_tabular: true,
            table_font: None,
            table_pad: [10.0, 6.0],
            table_flush_left: false,
            table_frame: None,
            table_body: None,
            table_head_pad: None,
            table_full_width: false,
            table_margin: [12.0, 16.0],
            diff_add: tint(c.layer_01, c.success.icon, if k.dark { 0.11 } else { 0.14 }),
            diff_remove: tint(c.layer_01, c.danger.icon, if k.dark { 0.11 } else { 0.14 }),
            diff_add_text: c.text_base,
            diff_remove_text: c.text_base,
            diff_hues: [c.success.icon, c.danger.icon],
            diff_hunk: c.text_muted,
            syntax: SyntaxTheme::from_kit(k),
            highlight: true,
            selection: k.selection(),
            highlight_bg: if k.dark { k.alpha(c.warning.icon, 0.24) } else { rune::oklcha(0.86, 0.15, 85.0, 0.42) },
            highlight_fg: None,
            highlight_ring: None,
            highlight_current_bg: if k.dark { k.alpha(c.warning.icon, 0.40) } else { rune::oklcha(0.82, 0.17, 75.0, 0.58) },
            highlight_current_fg: None,
            highlight_current_ring: Some((if k.dark { c.warning.icon } else { rune::oklch(0.66, 0.16, 70.0) }, 1.0)),
            task: TaskPaint {
                fill_on: c.surface_weak,
                border_on: c.std_border,
                border_off: c.border_weak,
                hover: c.surface_hover,
                focus_ring: c.border_active,
                mark: c.std_icon,
                check: k.glyph.check,
                radius: k.radius.sm,
                motion: k.motion.control,
            },
            diagram: DiagramPaint {
                text: c.text_base,
                muted: c.text_muted,
                surface: c.layer_01,
                surface_border: c.border_muted,
                node_fill: c.bg_base,
                node_border: k.alpha(c.text_base, 0.32),
                line: k.alpha(c.text_base, 0.55),
                cluster_fill: k.alpha(c.text_base, 0.04),
                note_fill: c.warning.bg,
                note_border: c.warning.border,
                font: TextStyle { size: 13.0, line_height: 18.0, ..k.ty.body },
                max_width: 680.0,
                hover: k.alpha(c.text_base, 0.9),
                hover_neighbours: false,
            },
        }
    }
    /// Rune's neutral light or dark look ([`Kit::neutral`]'s tokens), for apps without a kit.
    pub fn neutral(dark: bool) -> MarkdownStyle {
        MarkdownStyle::from_kit(&Kit::neutral(dark))
    }
    /// Derived from three colours, the way [`Kit::from_palette`] derives a kit.
    pub fn from_seed(seed: PaletteSeed) -> MarkdownStyle {
        MarkdownStyle::from_kit(&Kit::from_palette(seed))
    }
    /// Body, heading (levels 1 to 3 and the muted level 4+) and emphasis colour.
    pub fn text(mut self, c: Color) -> Self {
        self.text = c;
        self
    }
    /// Level 4+ headings and list markers.
    pub fn muted(mut self, c: Color) -> Self {
        self.muted = c;
        self
    }
    /// Link text; the underline is the same colour at 40% unless set with [`Self::link_underline`].
    pub fn link(mut self, c: Color) -> Self {
        self.accent = c;
        self.link_underline = c.alpha(0.4);
        self
    }
    pub fn link_underline(mut self, c: Color) -> Self {
        self.link_underline = c;
        self
    }
    /// The chip behind inline code.
    pub fn inline_code_bg(mut self, c: Color) -> Self {
        self.code_bg = c;
        self
    }
    pub fn code_block(mut self, fill: Color, border: Color) -> Self {
        self.code_block_fill = fill;
        self.code_block_border = border;
        self
    }
    /// Room (x, y) inside a code block's frame; 12 by 12 by default.
    pub fn code_block_padding(mut self, x: f32, y: f32) -> Self {
        self.code_block_pad = [x, y];
        self
    }
    /// The column a list's markers sit centred in (32 by default), and the gap between its
    /// items (4).
    pub fn list(mut self, indent: f32, gap: f32) -> Self {
        self.list_indent = indent;
        self.list_gap = gap;
        self
    }
    pub fn quote_rule(mut self, c: Color) -> Self {
        self.quote_rule = c;
        self
    }
    pub fn rule(mut self, c: Color) -> Self {
        self.rule = c;
        self
    }
    /// Paragraphs, list items and list markers.
    pub fn body_font(mut self, f: TextStyle) -> Self {
        self.body = f;
        self
    }
    /// The font of heading `level` (1 to 3; level 4 and deeper share 3).
    pub fn heading_font(mut self, level: u8, f: TextStyle) -> Self {
        self.headings[(level.clamp(1, 3) - 1) as usize] = f;
        self
    }
    /// Code blocks and diffs. Inline code is set in the app's mono slot, so keep this on
    /// `Family::Mono` for the two to match; for the platform's monospaced font (SF Mono) in
    /// both, name `ui-monospace` in the mono slot of `AppSpec::families`.
    pub fn code_block_font(mut self, f: TextStyle) -> Self {
        self.code_block = f;
        self
    }
    /// Inline code's size as a multiple of the surrounding text, and its weight (the family is
    /// always the mono slot's).
    pub fn inline_code_font(mut self, scale: f32, weight: f32) -> Self {
        self.inline_code_scale = scale;
        self.inline_code_weight = weight;
        self
    }
    /// Room in points between inline code and the edges of its chip. Up to twice a narrow
    /// space's width; what's left of it is the gap between the chip and the text around it.
    pub fn inline_code_pad(mut self, pad: f32) -> Self {
        self.inline_code_pad = pad;
        self
    }
    pub fn strong_weight(mut self, w: f32) -> Self {
        self.strong_weight = w;
        self
    }
    /// The tints behind a diff's added and removed lines (full width).
    pub fn diff(mut self, added: Color, removed: Color) -> Self {
        self.diff_add = added;
        self.diff_remove = removed;
        self
    }
    /// The text colours of a diff's added and removed lines (the body text colour by default).
    pub fn diff_text(mut self, added: Color, removed: Color) -> Self {
        self.diff_add_text = added;
        self.diff_remove_text = removed;
        self
    }
    /// Tints added and removed lines' text `amount` (0 to 1) of the way toward the theme's green
    /// and red, from the text colour set so far; around 0.5 reads clearly in both themes.
    pub fn diff_text_tint(mut self, amount: f32) -> Self {
        let t = amount.clamp(0.0, 1.0);
        self.diff_add_text = tint(self.text, self.diff_hues[0], t);
        self.diff_remove_text = tint(self.text, self.diff_hues[1], t);
        self
    }
    /// The text colour of a diff's `@@` hunk headers and file header lines.
    pub fn diff_hunk(mut self, c: Color) -> Self {
        self.diff_hunk = c;
        self
    }
    /// The colour of one kind of token in highlighted code blocks.
    pub fn syntax(mut self, kind: Syntax, c: Color) -> Self {
        self.syntax = self.syntax.with(kind, c);
        self
    }
    /// Every token colour at once (by default [`SyntaxTheme::from_kit`]).
    pub fn syntax_theme(mut self, theme: SyntaxTheme) -> Self {
        self.syntax = theme;
        self
    }
    /// Whether code blocks are coloured (on by default): fences in a language the highlighter
    /// knows (see [`Markdown::highlight`]). Off, every code block draws in the text colour.
    pub fn syntax_highlight(mut self, on: bool) -> Self {
        self.highlight = on;
        self
    }
    /// The highlight of [`Markdown::selectable`] text (the kit's field selection by default).
    pub fn selection(mut self, c: Color) -> Self {
        self.selection = c;
        self
    }
    /// Diagram nodes (and sequence participants): their fill and outline, and the colour of
    /// edges, arrows and lifelines.
    pub fn diagram(mut self, node_fill: Color, node_border: Color, line: Color) -> Self {
        self.diagram.node_fill = node_fill;
        self.diagram.node_border = node_border;
        self.diagram.line = line;
        self
    }
    /// The card a diagram sits on, and the tint behind flowchart subgraphs.
    pub fn diagram_surface(mut self, fill: Color, border: Color, subgraph: Color) -> Self {
        self.diagram.surface = fill;
        self.diagram.surface_border = border;
        self.diagram.cluster_fill = subgraph;
        self
    }
    /// Sequence diagram notes.
    pub fn diagram_note(mut self, fill: Color, border: Color) -> Self {
        self.diagram.note_fill = fill;
        self.diagram.note_border = border;
        self
    }
    /// The font of diagram labels (13 points of the body face by default).
    pub fn diagram_font(mut self, f: TextStyle) -> Self {
        self.diagram.font = f;
        self
    }
    /// The widest a diagram draws, in points (680 by default). A wider left-to-right flowchart
    /// is laid out top-down instead; anything still wider is drawn smaller, down to 60%.
    pub fn diagram_max_width(mut self, points: f32) -> Self {
        self.diagram.max_width = points.max(120.0);
        self
    }
    /// What lights up while the pointer is over a flowchart or state diagram node: its edges in
    /// `color` (the text colour by default; transparent turns it off), and with `neighbours` its
    /// outline and the outlines of the nodes they reach.
    pub fn diagram_hover(mut self, color: Color, neighbours: bool) -> Self {
        self.diagram.hover = color;
        self.diagram.hover_neighbours = neighbours;
        self
    }
    /// Background fill of standard [`Markdown::highlights`] matches.
    pub fn highlight_bg(mut self, c: Color) -> Self {
        self.highlight_bg = c;
        self
    }
    /// Optional glyph foreground colour of standard [`Markdown::highlights`] matches.
    pub fn highlight_fg(mut self, c: Option<Color>) -> Self {
        self.highlight_fg = c;
        self
    }
    /// Optional inset border ring `(color, width_pt)` of standard [`Markdown::highlights`] matches.
    pub fn highlight_ring(mut self, ring: Option<(Color, f32)>) -> Self {
        self.highlight_ring = ring;
        self
    }
    /// Background fill of [`Highlight::current`] matches.
    pub fn highlight_current_bg(mut self, c: Color) -> Self {
        self.highlight_current_bg = c;
        self
    }
    /// Optional glyph foreground colour of [`Highlight::current`] matches.
    pub fn highlight_current_fg(mut self, c: Option<Color>) -> Self {
        self.highlight_current_fg = c;
        self
    }
    /// Optional inset border ring `(color, width_pt)` of [`Highlight::current`] matches.
    pub fn highlight_current_ring(mut self, ring: Option<(Color, f32)>) -> Self {
        self.highlight_current_ring = ring;
        self
    }
}
