//! Now playing: the cover is a live mesh gradient under a huge title and a moving waveform. A
//! slab of fluted glass slides in over all of it and the queue settles onto the glass.

use rune::{Cx, El, col, el, row, spacer};

use crate::gfx::{FLUTED, MESH};
use crate::kit::*;
use crate::State;

const QUEUE: [(&str, &str, &str, u32); 5] = [
    ("Low Light", "Kaito Hayes", "3:41", 7),
    ("Glass Hours", "Mira Sol", "4:02", 1),
    ("Afterimage", "Sunday Static", "2:57", 5),
    ("Paper Moons", "Kaito Hayes", "3:18", 4),
    ("Slow Signal", "Ode Park", "5:06", 6),
];

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t_ = s.t;
    let mut root = el().w(W).h(H).fill(c(0x120a1e));
    root = root.child(fx(MESH, [7.0, 0.55, 3.0, 0.22]).absolute(0.0, 0.0).size(W, H));

    // A waveform across the whole width, so the glass has something moving behind it.
    let mut wave = row().absolute(40.0, 400.0).w(W - 40.0).h(80.0).gap(4.0).items_end();
    for i in 0..112 {
        let x = i as f32;
        let h = 10.0 + 34.0 * (0.5 + 0.5 * (x * 0.37 + t_ as f32 * 5.1).sin()) * (0.55 + 0.45 * (x * 0.11 - t_ as f32 * 2.3).sin().abs());
        wave = wave.child(el().w(4.0).h(h).radius(2.0).fill(ca(0xffffff, if x < 44.0 { 0.85 } else { 0.5 })));
    }
    root = root.child(wave);

    let title = col().absolute(48.0, 120.0)
        .gap(6.0)
        .child(ta("NOW PLAYING", 12.0, 600.0, 0xffffff, 0.75))
        .child(t("Night Drive", 128.0, 600.0, 0xffffff))
        .child(ta("Kaito Hayes · Low Light Sessions", 20.0, 500.0, 0xffffff, 0.8));
    root = root.child(title);

    let progress = 0.38 + 0.02 * t_ as f32;
    root = root.child(
        row().absolute(48.0, 496.0).w(470.0).gap(12.0).center_y()
            .child(mono("1:42", 12.0, 0xffffff))
            .child(el().w(380.0).h(4.0).radius(2.0).fill(ca(0xffffff, 0.25)).child(el().absolute(0.0, 0.0).h(4.0).w(380.0 * progress).radius(2.0).fill(c(0xffffff))))
            .child(mono("3:58", 12.0, 0xffffff)),
    );

    // The slab slides in from the right edge on a spring.
    let x = track(t_, W + 10.0, &[(0.35, 548.0, sp(0.95, 0.12))]);
    let gw = W - 548.0;
    let mut queue = col().absolute(x, 0.0).w(gw).h(H).pt(54.0).px(34.0).gap(4.0);
    let head = ease(t_, 0.95, 0.4);
    queue = queue.child(
        row().w_full().mb(14.0).offset(0.0, 10.0 * (1.0 - head))
            .child(ta("Up next", 22.0, 600.0, 0xffffff, head))
            .child(spacer())
            .child(ta("Shuffle", 13.0, 500.0, 0xffffff, 0.7 * head)),
    );
    for (i, (name, artist, dur, pal)) in QUEUE.iter().enumerate() {
        let k = track(t_, 0.0, &[(1.05 + i as f64 * 0.07, 1.0, sp(0.5, 0.2))]);
        queue = queue.child(
            row().w_full().h(68.0).gap(14.0).center_y().offset(24.0 * (1.0 - k), 0.0)
                .child(fx(MESH, [*pal as f32, 0.4, i as f32 + 5.0, 0.0]).size(48.0 * k.clamp(0.0, 1.0), 48.0 * k.clamp(0.0, 1.0)).radius(10.0))
                .child(col().gap(4.0).grow(1.0).child(ta(*name, 15.0, 600.0, 0xffffff, k)).child(ta(*artist, 13.0, 400.0, 0xffffff, 0.7 * k)))
                .child(mono_a(*dur, 12.0, 0xffffff, 0.8 * k)),
        );
    }
    root.child(fx(FLUTED, [18.0, 16.0, 0.12, 0.3]).effect_backdrop(true).absolute(x, 0.0).size(gw + 10.0, H))
        .child(el().absolute(x, 0.0).w(1.0).h(H).fill(ca(0xffffff, 0.35)))
        .child(queue)
}
