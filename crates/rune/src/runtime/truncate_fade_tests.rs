//! `El::truncate_fade` fades a label only when it runs past its box, wherever the box lands.

use super::*;
use crate::{Cx, El, Env, TextStyle, el, row, text};

/// (zoom, offset before the label, label, room after it)
type S = (f32, f32, &'static str, f32);

fn view(s: &S, _: &mut Cx<S>) -> El {
    let (zoom, offset, label, room) = *s;
    el().w(400.0 / zoom).h(60.0 / zoom).scale(zoom).child(
        row().w(400.0 / zoom).child(el().w(offset).h(1.0)).child(
            el().max_w(room).child(text(label).key("label").font(TextStyle::new(13.0, 500.0)).truncate_fade().shrink()),
        ),
    )
}

/// The widest right-edge fade on the label's glyphs, in device px.
fn fade(rt: &Runtime<S>) -> f32 {
    rt.instances.iter()
        .filter(|i| i.params[1] == crate::render::KIND_GLYPH)
        .map(|i| crate::render::unpack_fade(i.fade).0[2])
        .fold(0.0, f32::max)
}

#[test]
fn a_label_that_fits_never_fades_wherever_it_lands() {
    let mut faded = Vec::new();
    for scale in [1.0, 2.0] {
        for zoom in [1.0, 1.1, 1.25, 1.3, 1.5, 1.6] {
            for label in ["Skill, Shell", "Read, Grep", "Shell"] {
                for k in 0..40 {
                    let offset = 30.0 + k as f32 * 0.137;
                    let mut rt = Runtime::new((zoom, offset, label, 300.0), view, (400.0, 60.0), Env::new(true, scale));
                    rt.frame(0.0);
                    let f = fade(&rt);
                    if f > 0.0 {
                        faded.push(format!("{scale}x, zoom {zoom}, {label:?} after {offset}: fades {f} px"));
                    }
                }
            }
        }
    }
    assert!(faded.is_empty(), "{} labels with room faded: {faded:#?}", faded.len());
}

#[test]
fn a_label_cut_short_still_fades() {
    for scale in [1.0, 2.0] {
        for zoom in [1.0, 1.3] {
            let mut rt = Runtime::new((zoom, 30.0, "Skill, Shell", 40.0), view, (400.0, 60.0), Env::new(true, scale));
            rt.frame(0.0);
            assert!(fade(&rt) > 0.0, "{scale}x, zoom {zoom}");
        }
    }
}
