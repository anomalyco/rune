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
