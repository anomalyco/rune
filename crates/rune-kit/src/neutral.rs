//! A plain theme derived from Rune's core neutral tokens (`rune::tok`), so the kit renders
//! without any brand. Real apps use a theme crate (e.g. `rune-theme-opencode`).

use rune::{Color, Shadow, TextStyle, oklch, oklcha, tok};

use crate::kit::{Elevation, Kit, Motion, Palette, Radius, StateColors, Type};

fn ring(c: Color) -> Shadow {
    Shadow { spread: 0.5, color: c, ..Shadow::default() }
}

fn drop(y: f32, blur: f32, c: Color) -> Shadow {
    Shadow { y, blur, color: c, ..Shadow::default() }
}

impl Kit {
    pub fn neutral(dark: bool) -> Kit {
        let t = tok::theme(dark);
        let accent = tok::accent(0, dark);
        let state = |h: f32| StateColors {
            bg: oklcha(if dark { 0.3 } else { 0.95 }, 0.04, h, 1.0),
            fg: oklch(if dark { 0.75 } else { 0.5 }, 0.15, h),
            border: oklch(if dark { 0.4 } else { 0.85 }, 0.08, h),
            icon: oklch(if dark { 0.75 } else { 0.5 }, 0.15, h),
        };
        let white = Color::WHITE;
        let black = Color::BLACK;
        let c = Palette {
            page: t.bg,
            bg_base: t.card,
            bg_deep: t.bg,
            layer_01: t.bg,
            layer_02: t.control_track,
            layer_03: t.control_off,
            layer_04: t.indicator,
            bg_inverse: t.text,
            bg_contrast: t.text,
            bg_button_neutral: t.card,
            bg_icon_button_contrast: t.text,
            bg_accent: accent,
            submit_highlight: white.alpha(0.2),
            submit_hover: white.alpha(0.12),
            submit_fg: t.card,
            submit_icon: t.card,
            text_base: t.text,
            text_muted: t.text_secondary,
            text_faint: t.text_secondary.alpha(0.8),
            text_inverse: t.card,
            text_contrast: t.card,
            text_accent: accent,
            icon_base: t.text,
            icon_muted: t.text_secondary,
            icon_faint: t.text_secondary.alpha(0.7),
            icon_inverse: t.card,
            icon_contrast: t.card,
            icon_accent: accent,
            border_muted: t.separator,
            border_base: t.card_border,
            border_strong: t.separator,
            border_focus: accent,
            hover: t.hover,
            pressed: t.press,
            contrast_hover: white.alpha(0.1),
            contrast_pressed: white.alpha(0.2),
            scrim: black.alpha(0.3),
            depth_top: white,
            depth_bottom: white.alpha(0.0),
            white,
            black,
            knob: t.knob,
            knob_on: t.knob,
            success: state(150.0),
            warning: state(80.0),
            danger: state(25.0),
            info: state(256.0),
            surface_base: t.control_track,
            surface_weak: t.control_track,
            surface_hover: t.hover,
            surface_raised: t.card,
            surface_raised_base: t.card,
            surface_raised_hover: t.hover,
            surface_float: t.text,
            surface_disabled: t.control_track,
            border_weak: t.separator,
            border_hover: t.card_border,
            border_active: accent,
            border_disabled: t.separator,
            text_strong: t.text,
            text_weak: t.text_secondary,
            text_invert_strong: t.card,
            icon_weaker: t.text_secondary,
            std_border: t.card_border,
            std_icon: t.text_secondary,
            std_icon_invert: t.card,
            std_icon_strong: t.text,
            std_text: t.text,
        };
        let elevation = Elevation {
            raised: vec![drop(1.0, 2.0, t.shadow), ring(t.card_border)],
            floating: vec![drop(4.0, 8.0, t.shadow), ring(t.card_border)],
            overlay: vec![drop(8.0, 16.0, t.shadow), ring(t.card_border)],
            button_neutral: vec![drop(1.0, 1.5, t.shadow), ring(t.card_border)],
            button_contrast: vec![drop(1.0, 1.5, t.shadow)],
            elements: vec![drop(0.5, 0.5, t.edge)],
            switch_off: vec![],
            switch_on: vec![],
            md: vec![drop(4.0, 8.0, t.shadow_soft)],
            knob: vec![drop(1.0, 3.0, t.knob_shadow)],
        };
        let ui = TextStyle::new(13.0, 400.0).lh(20.0);
        Kit {
            dark,
            c,
            elevation,
            glyph: crate::glyphs::DEFAULT,
            ty: Type {
                ui,
                ui_medium: TextStyle { weight: 500.0, ..ui },
                ui_compact: ui.lh(16.0),
                small: TextStyle::new(11.0, 400.0).lh(12.0),
                small_medium: TextStyle::new(11.0, 500.0).lh(12.0),
                body: TextStyle::new(14.0, 400.0).lh(21.0),
                body_medium: TextStyle::new(14.0, 500.0).lh(21.0),
                text_small: TextStyle::new(13.0, 400.0).lh(19.5),
                text_small_medium: TextStyle::new(13.0, 500.0).lh(19.5),
                text_xsmall: TextStyle::new(12.0, 400.0).lh(18.0),
                text_xsmall_medium: TextStyle::new(12.0, 500.0).lh(18.0),
                mono: TextStyle::new(12.0, 400.0).mono().lh(18.0),
            },
            radius: Radius { xs: 2.0, sm: 4.0, md: 6.0, lg: 8.0, xl: 10.0 },
            motion: Motion {
                control: tok::motion::SNAPPY,
                overlay: tok::motion::SNAPPY,
                disclosure: tok::motion::DEFAULT,
            },
            blend_over: None,
            ring: None,
            invalid_text: false,
        }
    }
}
