use rune::render::{KIND_GRADIENT, KIND_RECT};
use rune::{Cx, El, Env, Runtime, col};
use rune_kit::*;

const SIZES: [ControlSize; 5] =
    [ControlSize::Mini, ControlSize::Small, ControlSize::Regular, ControlSize::Large, ControlSize::ExtraLarge];

fn view(_: &(), cx: &mut Cx<()>) -> El {
    let k = Kit::neutral(cx.system_dark());
    col().padding(10.09375).gap(1.1).children(SIZES.into_iter().flat_map(|size| {
        [false, true].map(|on| Switch::new(on).label("Switch").hide_label().control_size(size).build(&k))
    }))
}

/// The knob of every compact switch size sits as far from the top of its track as from the
/// bottom on the device grid, at scales where its 2 pt inset is a fraction of a pixel.
#[test]
fn compact_switch_knobs_sit_centred_on_device_pixels() {
    for scale in [1.2, 1.25, 1.5, 1.75] {
        let mut rt = Runtime::new((), view, (120.0, 240.0), Env::new(true, scale));
        rt.frame(0.0);
        let els = rt.elements();
        // The painted rect (device px edges) drawn for an element's rect.
        let drawn = |r: [f32; 4]| {
            let want = [r[0] * scale, r[1] * scale, (r[0] + r[2]) * scale, (r[1] + r[3]) * scale];
            rt.instances.iter()
                .filter(|i| i.params[1] == KIND_RECT || i.params[1] == KIND_GRADIENT)
                .map(|i| [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]])
                .find(|e| (0..4).all(|k| (e[k] - want[k]).abs() <= 1.01))
                .unwrap_or_else(|| panic!("{scale}×: nothing drawn at {want:?}"))
        };
        let thumbs: Vec<_> = els.iter().filter(|e| e.path.ends_with("/thumb")).collect();
        assert_eq!(thumbs.len(), SIZES.len() * 2);
        for thumb in thumbs {
            let track = els.iter().find(|e| thumb.path.strip_suffix("/thumb") == Some(e.path.as_str())).unwrap();
            let (t, k) = (drawn(track.rect), drawn(thumb.rect));
            assert!(t.iter().chain(&k).all(|v| v.fract() == 0.0), "{scale}× {}: {t:?} {k:?}", thumb.path);
            assert_eq!(k[1] - t[1], t[3] - k[3], "{scale}× {}: track {t:?}, knob {k:?}", thumb.path);
        }
    }
}
