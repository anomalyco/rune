//! A pricing page. The Pro card hums with a charged border; hovering charges it up, and Upgrade
//! throws an electric border round it with a burst of sparks.

use rune::{Cx, El, col, el, row, spacer};

use crate::gfx::{ELECTRIC, GLOW, SPARKLES};
use crate::kit::*;
use crate::State;

const HOVER: f64 = 0.55;
const TAP: f64 = 1.25;

fn feature(s: &str, on: bool) -> El {
    row().gap(10.0).center_y()
        .child(el().size(16.0, 16.0).radius(8.0).center().fill(if on { ca(0x54b3fd, 0.18) } else { ca(0xffffff, 0.06) }).child(dot(if on { 0x54b3fd } else { 0x8a8a93 }, 5.0)))
        .child(t(s, 13.5, 400.0, 0xc9c9d0))
}

fn plan(name: &str, price: &str, blurb: &str, feats: [&str; 4], cta: &str, pro: bool, press: f32) -> El {
    let mut c_ = col().w(268.0).h(318.0).padding(24.0).gap(8.0).radius(20.0)
        .fill(if pro { c(0x16181f) } else { c(0x121215) })
        .border(1.0, if pro { ca(0x54b3fd, 0.35) } else { hair() })
        .child(row().w_full().center_y().child(t(name, 15.0, 600.0, TEXT)).child(spacer()).child(if pro {
            el().px(8.0).py(3.0).radius(8.0).fill(ca(0x54b3fd, 0.16)).child(t("Popular", 11.5, 600.0, 0x8fd0ff))
        } else {
            el()
        }))
        .child(row().mt(6.0).items_end().gap(4.0).child(t(price, 40.0, 600.0, TEXT)).child(t("/mo", 14.0, 500.0, MUTED).mb(8.0)))
        .child(t(blurb, 13.0, 400.0, MUTED).mb(10.0));
    for (i, f) in feats.iter().enumerate() {
        c_ = c_.child(feature(f, pro || i < 2));
    }
    c_.child(spacer()).child(
        row().w_full().h(40.0).radius(11.0).center().scale(1.0 - 0.06 * press)
            .fill(if pro { c(0xeef4ff) } else { ca(0xffffff, 0.07) })
            .child(t(cta, 14.0, 600.0, if pro { 0x0e1320 } else { TEXT })),
    )
}

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t_ = s.t;
    let hover = ease(t_, HOVER, 0.35);
    let hit = ease(t_, TAP, 0.08);
    let press = pressed(t_, TAP);
    let x0 = (W - 3.0 * 268.0 - 2.0 * 22.0) / 2.0;
    let y0 = 176.0;
    let px_ = x0 + 268.0 + 22.0;
    let lift = track(t_, 0.0, &[(HOVER, 1.0, sp(0.45, 0.3))]);

    let header = col().absolute(0.0, 54.0).w(W).items_center().gap(10.0)
        .child(t("PRICING", 12.0, 600.0, 0x54b3fd))
        .child(t("Plans that scale with you", 32.0, 600.0, TEXT))
        .child(t("Start free. Upgrade when your team needs more.", 15.0, 400.0, MUTED));

    let pro_y = y0 - 8.0 * lift;
    el().w(W).h(H).fill(c(0x0a0a0c))
        .child(header)
        .child(plan("Hobby", "$0", "For side projects and prototypes.", ["One project", "Preview deploys", "Community support", "Analytics"], "Get started", false, 0.0).absolute(x0, y0))
        .child(plan("Team", "$60", "For studios shipping together.", ["Unlimited projects", "SSO and audit log", "Analytics", "Priority support"], "Contact us", false, 0.0).absolute(x0 + 2.0 * (268.0 + 22.0), y0))
        .child(fx(GLOW, [1.0, 13.0, 0.35 + 0.5 * hover + 0.8 * hit, 22.0]).absolute(px_, pro_y).size(268.0, 318.0).radius(20.0).effect_bleed(60.0))
        .child(plan("Pro", "$20", "For apps people use every day.", ["Unlimited projects", "Custom domains", "Team analytics", "Priority builds"], "Upgrade", true, press).absolute(px_, pro_y))
        .child(fx(ELECTRIC, [0.5 + 0.7 * hover + 0.4 * hit, if t_ >= TAP { 6.0 } else { 0.0 }, if t_ >= TAP { 0.0 } else { 1.0 }, 13.0]).absolute(px_, pro_y).size(268.0, 318.0).radius(20.0).effect_bleed(40.0))
        .child(fx(SPARKLES, [TAP as f32 + 0.05, 14.0, 10.0, 13.0]).absolute(px_ + 24.0, pro_y + 254.0).size(220.0, 40.0).radius(11.0).effect_bleed(60.0))
        .child(pointer_at(t_, [960.0, 560.0], &[(0.12, px_ + 180.0, pro_y + 140.0), (0.75, px_ + 140.0, y0 + 272.0)], &[TAP]))
}
