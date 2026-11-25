use rune::render::KIND_GLYPH;
use rune::{Cx, El, Env, Runtime, col, icon};

use super::*;

fn glyph_at(rt: &Runtime<()>, key: &str, child_key: Option<&str>) -> ([f32; 2], [u32; 3]) {
    let els = rt.elements();
    let root = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"));
    let target = match child_key {
        Some(child) => els
            .iter()
            .find(|e| e.key.as_deref() == Some(child) && e.path.starts_with(&root.path))
            .unwrap_or_else(|| panic!("no {child} under {key}")),
        None => root,
    };
    let cx = target.rect[0] + target.rect[2] * 0.5;
    let cy = target.rect[1] + target.rect[3] * 0.5;
    let inst = rt
        .instances
        .iter()
        .find(|i| {
            i.params[1] == KIND_GLYPH
                && (i.rect[0] + i.rect[2] * 0.5 - cx).abs() < 1.5
                && (i.rect[1] + i.rect[3] * 0.5 - cy).abs() < 1.5
        })
        .unwrap_or_else(|| panic!("no glyph instance for {key} ({:?})", target.rect));
    let origin = if inst.uv[2] == 0.0 { [inst.uv[0], inst.uv[1]] } else { [inst.uv[0] + 1.0, inst.uv[1] + 1.0] };
    ([target.rect[2], target.rect[3]], [origin[0].to_bits(), origin[1].to_bits(), inst.params[2].to_bits()])
}

#[test]
fn indeterminate_draws_dash_and_custom_icon_overrides() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col()
            .gap(8.0)
            .child(icon(k.glyph.check, 10.0).color(k.c.std_icon).key("ref-check"))
            .child(icon(k.glyph.dash, 10.0).color(k.c.std_icon).key("ref-dash"))
            .child(icon(k.glyph.close, 14.0).color(k.c.std_icon).key("ref-custom"))
            .child(Checkbox::new(true).label("Checked").build(&k).key("checked"))
            .child(Checkbox::new(false).indeterminate(true).label("Indeterminate").build(&k).key("indeterminate"))
            .child(Checkbox::new(true).indeterminate(true).label("Both").build(&k).key("both"))
            .child(Checkbox::new(true).icon(k.glyph.close).label("Custom checked").build(&k).key("custom-checked"))
            .child(Checkbox::new(false).indeterminate(true).icon(k.glyph.close).label("Custom indeterminate").build(&k).key("custom-indeterminate"))
            .child(Checkbox::new(false).label("Unchecked").build(&k).key("unchecked"))
    }

    let mut rt = Runtime::new((), view, (200.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let ref_check = glyph_at(&rt, "ref-check", None);
    let ref_dash = glyph_at(&rt, "ref-dash", None);
    let ref_custom = glyph_at(&rt, "ref-custom", None);
    assert_ne!(ref_check.1, ref_dash.1, "check and dash must occupy distinct atlas slots");

    assert_eq!(glyph_at(&rt, "checked", Some("mark")), ref_check, "checked draws k.glyph.check at 10pt");
    assert_eq!(glyph_at(&rt, "indeterminate", Some("mark")), ref_dash, "indeterminate draws k.glyph.dash at 10pt");
    assert_eq!(glyph_at(&rt, "both", Some("mark")), ref_dash, "indeterminate takes precedence over checked");
    assert_eq!(glyph_at(&rt, "custom-checked", Some("mark")), ref_custom, "custom .icon() overrides checked");
    assert_eq!(
        glyph_at(&rt, "custom-indeterminate", Some("mark")),
        ref_custom,
        "custom .icon() overrides indeterminate"
    );

    let els = rt.elements();
    let unchecked = els.iter().find(|e| e.key.as_deref() == Some("unchecked")).unwrap();
    let mark = els.iter().find(|e| e.key.as_deref() == Some("mark") && e.path.starts_with(&unchecked.path)).unwrap();
    assert_eq!(mark.opacity, 0.0, "unchecked hides its mark");
}

#[test]
fn kit_checked_colours_hold_at_rest_and_under_the_pointer() {
    const FILL: Color = Color { r: 0.1, g: 0.2, b: 0.9, a: 1.0 };
    const BORDER: Color = Color { r: 0.1, g: 0.2, b: 0.7, a: 1.0 };
    const MARK: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    fn kit() -> Kit {
        let mut k = Kit::neutral(true);
        k.metrics.checkbox = Some(crate::CheckboxColors { fill: FILL, border: BORDER, mark: MARK });
        k
    }
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let (base, k) = (Kit::neutral(true), kit());
        col()
            .child(Checkbox::new(true).label("On").build(&k).key("on"))
            .child(Checkbox::new(true).state(State::Hover).label("Hover").build(&k).key("hover"))
            .child(Checkbox::new(false).label("Off").build(&k).key("off"))
            .child(Checkbox::new(false).label("Off default").build(&base).key("off-default"))
            .child(Checkbox::new(true).label("On default").build(&base).key("on-default"))
    }

    let mut rt = Runtime::new((), view, (200.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let els = rt.elements();
    let part = |root: &str, key: &str| {
        let path = &els.iter().find(|e| e.key.as_deref() == Some(root)).unwrap_or_else(|| panic!("no {root}")).path;
        els.iter().find(|e| e.key.as_deref() == Some(key) && e.path.starts_with(path)).unwrap_or_else(|| panic!("no {key} in {root}")).clone()
    };
    let (k, base) = (kit(), Kit::neutral(true));

    let (control, mark) = (part("on", "control"), part("on", "mark"));
    assert_eq!((control.fill, control.border_color, mark.text_color), (FILL, Some(BORDER), Some(MARK)));

    let (control, mark) = (part("hover", "control"), part("hover", "mark"));
    assert_eq!(control.fill, crate::over(k.c.hover, FILL), "hover keeps the kit's fill under the overlay");
    assert_ne!(control.fill, k.c.surface_hover);
    assert_eq!((control.border_color, mark.text_color), (Some(BORDER), Some(MARK)));

    // Unchecked boxes and kits without checked colours look as they did.
    let (off, off_default) = (part("off", "control"), part("off-default", "control"));
    assert_eq!((off.fill, off.border_color), (off_default.fill, off_default.border_color));
    assert_eq!((off.fill, off.border_color), (Color::TRANSPARENT, Some(base.c.border_weak)));
    let (control, mark) = (part("on-default", "control"), part("on-default", "mark"));
    assert_eq!((control.fill, control.border_color), (base.c.surface_weak, Some(base.c.std_border)));
    assert_eq!(mark.text_color, Some(base.c.std_icon));
}
