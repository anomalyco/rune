//! A photo library of live gradients. Taps send shockwaves through the whole window, on the beat.

use rune::{Cx, El, col, el, row, spacer};

use crate::gfx::{MESH, SHOCK};
use crate::kit::*;
use crate::State;

/// Taps: time and grid cell (column, row). 0.6667 s apart, a beat at 90 BPM.
const TAPS: [(f64, usize, usize); 3] = [(0.5, 2, 1), (1.1667, 0, 2), (1.8333, 4, 0)];

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t_ = s.t;
    let (cols, rows) = (5usize, 3usize);
    let (gx, gy, gap) = (28.0, 92.0, 10.0);
    let tw = (W - 2.0 * gx - gap * (cols as f32 - 1.0)) / cols as f32;
    let th = (H - gy - 24.0 - gap * (rows as f32 - 1.0)) / rows as f32;

    let seg = ["Years", "Months", "Days", "All Photos"];
    let sel = track(t_, 3.0, &[(0.25, 2.0, sp(0.45, 0.25))]);
    let mut seg_row = el().h(34.0).w(4.0 * 92.0 + 8.0).radius(10.0).fill(ca(0xffffff, 0.06))
        .child(el().absolute(4.0 + sel * 92.0, 4.0).size(92.0, 26.0).radius(7.0).fill(ca(0xffffff, 0.14)));
    for (i, name) in seg.iter().enumerate() {
        seg_row = seg_row.child(row().absolute(4.0 + i as f32 * 92.0, 4.0).size(92.0, 26.0).center().child(t(*name, 13.0, 500.0, if (sel - i as f32).abs() < 0.5 { TEXT } else { MUTED })));
    }
    let header = row().absolute(gx, 28.0).w(W - 2.0 * gx).center_y()
        .child(col().gap(3.0).child(t("Library", 24.0, 600.0, TEXT)).child(t("1,284 photos · 3 albums", 13.0, 400.0, MUTED)))
        .child(spacer())
        .child(seg_row);

    let mut root = el().w(W).h(H).fill(c(0x08080a)).child(header);
    for r in 0..rows {
        for cc in 0..cols {
            let i = r * cols + cc;
            let press = TAPS.iter().filter(|(_, x, y)| *x == cc && *y == r).map(|(tp, _, _)| pressed(t_, *tp)).fold(0.0f32, f32::max);
            let x = gx + cc as f32 * (tw + gap);
            let y = gy + r as f32 * (th + gap);
            root = root.child(fx(MESH, [(i % 8) as f32, 0.5, i as f32, 0.25]).key(format!("tile-{i}")).absolute(x, y).size(tw, th).radius(12.0).scale(1.0 - 0.06 * press));
        }
    }
    // Always in the tree (an idle shockwave draws nothing), so no slot enters inside a fade layer,
    // where its backdrop would be the empty layer.
    for (tp, cc, r) in TAPS {
        let x = gx + cc as f32 * (tw + gap) + tw / 2.0;
        let y = gy + r as f32 * (th + gap) + th / 2.0;
        root = root.child(fx(SHOCK, [x, y, tp as f32 + 0.03, 22.0]).key(format!("shock-{cc}-{r}")).effect_backdrop(true).absolute(0.0, 0.0).size(W, H));
    }
    let path: Vec<(f64, f32, f32)> = TAPS.iter().map(|(tp, cc, r)| (tp - 0.42, gx + *cc as f32 * (tw + gap) + tw / 2.0, gy + *r as f32 * (th + gap) + th / 2.0)).collect();
    let taps: Vec<f64> = TAPS.iter().map(|x| x.0).collect();
    root.child(pointer_at(t_, [700.0, 500.0], &path, &taps))
}
