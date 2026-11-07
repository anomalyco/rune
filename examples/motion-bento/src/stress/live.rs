//! The wall as Rune means it to be built. A control's card is built once for each size it takes
//! (`memo`), and placed again only when the wall reshuffles. Everything that moves inside it, its
//! reading, bars, switch, progress and colours, is a live value `poll` sets, so on most frames
//! the view doesn't run and nothing commits.

use motion_bento_chaos::hud::Readout;
use motion_bento_chaos::look::{Springs, color, grid};
use motion_bento_chaos::stress::{self, BARS, Cell, HEADER, HUES, Metrics, Wall, lh};
use rune::{Align, Color, El, Live, LiveText, Transition, col, el, memo, row, spacer, text};

use super::hud::Hud;
use super::{HUD_W, KNOB, SWITCH_OFF};
use crate::State;
use crate::style::{c, caption, font, nums, spring};

/// One hue's colours, worked out once.
struct Palette {
    tint: Color,
    glow: Color,
    hi: Color,
    lo: Color,
    hot_fill: Color,
    hot_edge: Color,
}

/// What moves inside one control.
struct Control {
    reading: LiveText,
    percent: LiveText,
    status: LiveText,
    tint: Live<Color>,
    glow: Live<Color>,
    hi: Live<Color>,
    lo: Live<Color>,
    card: Live<Color>,
    edge: Live<Color>,
    switch: Live<Color>,
    knob: Live<[f32; 2]>,
    bars: [Live<[f32; 4]>; BARS],
    progress: Live<[f32; 4]>,
    shown: (u32, &'static str),
}

impl Control {
    fn new() -> Control {
        let unset = || Live::new(Color::TRANSPARENT);
        Control {
            reading: LiveText::new(""),
            percent: LiveText::new(""),
            status: LiveText::new(""),
            tint: unset(),
            glow: unset(),
            hi: unset(),
            lo: unset(),
            card: Live::new(c(color::TILE)),
            edge: Live::new(c(color::EDGE)),
            switch: Live::new(c(SWITCH_OFF)),
            knob: Live::new([0.0; 2]),
            bars: std::array::from_fn(|_| Live::new([0.0; 4])),
            progress: Live::new([0.0; 4]),
            shown: (u32::MAX, ""),
        }
    }

    fn set(&mut self, cell: &Cell, m: &Metrics, p: &Palette) {
        let [_, _, w, _] = cell.rect;
        self.reading.set(cell.reading());
        let pct = (cell.progress * 100.0).round() as u32;
        if self.shown.0 != pct {
            self.percent.set(cell.percent());
        }
        if self.shown.1 != cell.status {
            self.status.set(cell.status);
        }
        self.shown = (pct, cell.status);
        self.tint.set(p.tint);
        self.glow.set(p.glow);
        self.hi.set(p.hi);
        self.lo.set(p.lo);
        let (card, edge) = if cell.hot { (p.hot_fill, p.hot_edge) } else { (c(color::TILE), c(color::EDGE)) };
        self.card.set(card);
        self.edge.set(edge);
        self.switch.set(if cell.on { p.tint } else { c(SWITCH_OFF) });
        let inset = (m.switch_h - m.knob) / 2.0;
        self.knob.set([if cell.on { m.switch_w - 2.0 * inset - m.knob } else { 0.0 }, 0.0]);
        let inner = w - 2.0 * m.pad;
        let bar_w = (inner - (BARS - 1) as f32 * m.bar_gap) / BARS as f32;
        for (j, (live, &b)) in self.bars.iter().zip(&cell.bars).enumerate() {
            let h = b * m.bars_h;
            live.set([j as f32 * (bar_w + m.bar_gap), m.bars_h - h, bar_w, h]);
        }
        let track = inner - 2.0 * m.gap - m.switch_w;
        self.progress.set([0.0, 0.0, cell.progress * track, m.track_h]);
    }
}

/// The live values the wall's view binds, and what it last built from.
pub struct Bindings {
    palette: Vec<Palette>,
    controls: Vec<Control>,
    t: LiveText,
    pub(super) hud: Hud,
    /// The controls' rects the view last placed: a change needs a commit.
    built: Vec<[f32; 4]>,
}

impl Bindings {
    pub fn new() -> Bindings {
        let palette = HUES
            .iter()
            .map(|&h| Palette {
                tint: c(stress::tint(h)),
                glow: c(stress::tint(h).alpha(0.8)),
                hi: c(stress::tint_hi(h)),
                lo: c(stress::tint_lo(h)),
                hot_fill: c(stress::hot_fill(h)),
                hot_edge: c(stress::hot_edge(h)),
            })
            .collect();
        Bindings { palette, controls: Vec::new(), t: LiveText::new(""), hud: Hud::new(), built: Vec::new() }
    }

    /// Sets every live value to the wall at show time `t`. True when the wall's shape changed
    /// (a control came or went, or moved or resized), which only a commit can show.
    pub fn sync(&mut self, wall: &Wall, t: f64, readout: Option<&Readout>) -> bool {
        let cells = wall.cells();
        // Controls are never dropped, so a card a memo kept still binds its own values.
        while self.controls.len() < cells.len() {
            self.controls.push(Control::new());
        }
        for (cell, control) in cells.iter().zip(&mut self.controls) {
            let hue = HUES.iter().position(|&h| h == cell.hue).unwrap_or(0);
            control.set(cell, &wall.metrics, &self.palette[hue]);
        }
        self.t.set(format!("{t:.2} s"));
        if let Some(r) = readout {
            self.hud.set(r);
        }
        let moved = self.built.len() != cells.len() || cells.iter().zip(&self.built).any(|(c, r)| c.rect != *r);
        if moved {
            self.built.clear();
            self.built.extend(cells.iter().map(|c| c.rect));
        }
        moved
    }
}

pub fn view(s: &State, wall: &Wall, b: &Bindings) -> El {
    let mut root = el().w(grid::WIDTH).h(grid::HEIGHT).fill(c(color::BG)).child(header(s, wall, b));
    for (cell, control) in wall.cells().iter().zip(&b.controls) {
        root = root.child(card(&s.springs, cell, &wall.metrics, control));
    }
    if s.readout.is_some() {
        root = root.child(b.hud.view());
    }
    root
}

fn header(s: &State, wall: &Wall, b: &Bindings) -> El {
    let cfg = crate::config();
    let load = cfg.load.as_ref();
    let steps = load.map_or(1, |l| l.levels().len());
    let n = wall.cells().len();
    // The view runs whenever the count changes, so the step it reads is current.
    let step = load.map_or(0, |l| l.step_at(s.t));
    col()
        .absolute(grid::PAD, grid::PAD - 8.0)
        .size(grid::WIDTH - 2.0 * grid::PAD - HUD_W - grid::GAP, HEADER)
        .gap(4.0)
        .child(
            row()
                .gap(10.0)
                .center_y()
                .child(text("Rune").font(font(24.0, 680.0)).color(c(color::ACCENT)))
                .child(text("stress wall").font(font(24.0, 680.0)).color(c(color::TEXT))),
        )
        .child(
            row()
                .gap(4.0)
                .child(caption(format!("Seed {} · every control retargets, ticks and reflows at once · t", cfg.seed)))
                .child(text("00.00 s").font(nums(12.0, 460.0)).color(c(color::TEXT_3)).live_text(&b.t)),
        )
        .child(
            row()
                .gap(10.0)
                .items_end()
                .child(text(format!("{n}")).font(nums(40.0, 700.0)).color(c(color::TEXT)))
                .child(text("live controls").font(font(16.0, 560.0)).color(c(color::TEXT_2)))
                .child(caption(format!("step {} of {steps} · {}×{} grid", step + 1, wall.cols, wall.rows))),
        )
}

/// The card is placed on each commit; what's inside builds once per size.
fn card(springs: &Springs, cell: &Cell, m: &Metrics, b: &Control) -> El {
    let [x, y, w, h] = cell.rect;
    let layout = spring(springs.layout);
    let id = cell.id;
    col()
        .key(format!("z{id}"))
        .absolute(x, y)
        .size(w, h)
        .padding(m.pad)
        .radius(m.radius)
        .fill(c(color::TILE))
        .live_fill(b.card.smooth(layout))
        .border(1.0, c(color::EDGE))
        .live_border(b.edge.smooth(layout))
        .shadow(m.shadow_y, m.shadow_blur, Color::BLACK.alpha(0.35))
        .clip_rounded(m.radius)
        .motion(layout)
        .transition(Transition::scale(0.85).combined(Transition::opacity()))
        .child(memo(format!("c{id}"), (w.to_bits(), h.to_bits()), || inside(springs, cell, m, b)))
}

fn inside(springs: &Springs, cell: &Cell, m: &Metrics, b: &Control) -> El {
    let [_, _, w, _] = cell.rect;
    let (travel, snap) = (spring(springs.travel), spring(springs.snap));
    // Authored colours only until the first latch: the live values own them.
    let ink = c(color::ACCENT);
    let inner = w - 2.0 * m.pad;
    let bar_w = (inner - (BARS - 1) as f32 * m.bar_gap) / BARS as f32;
    let mut bars = el().w_full().h(m.bars_h);
    for live in &b.bars {
        bars = bars.child(
            el().absolute(0.0, 0.0)
                .size(bar_w, m.bars_h)
                .radius(2.0 * m.u)
                .gradient(ink, ink)
                .live_gradient(b.hi.smooth(travel), b.lo.smooth(travel))
                .live_rect(live.smooth(travel)),
        );
    }
    let inset = (m.switch_h - m.knob) / 2.0;
    let switch = el()
        .size(m.switch_w, m.switch_h)
        .radius(m.switch_h / 2.0)
        .fill(c(SWITCH_OFF))
        .live_fill(b.switch.smooth(snap))
        .child(el().size(m.knob, m.knob).absolute(inset, inset).radius(m.knob / 2.0).fill(c(KNOB)).live_offset(b.knob.smooth(snap)));
    let track = el()
        .grow(1.0)
        .h(m.track_h)
        .radius(m.track_h / 2.0)
        .fill(c(color::WELL))
        .child(el().absolute(0.0, 0.0).size(0.0, m.track_h).radius(m.track_h / 2.0).fill(ink).live_fill(&b.tint).live_rect(&b.progress));
    col()
        .w_full()
        .h_full()
        .gap(m.gap)
        .child(
            row()
                .h(lh(m.name))
                .gap(m.gap)
                .center_y()
                .child(
                    el().size(m.dot, m.dot)
                        .radius(m.dot / 2.0)
                        .fill(ink)
                        .live_fill(&b.tint)
                        .shadow(0.0, 2.0 * m.dot, ink)
                        .live_shadow(&b.glow),
                )
                .child(text(cell.name()).font(font(m.name, 520.0)).color(c(color::TEXT_2)))
                .child(spacer())
                .child(text("Boost").font(font(m.small, 600.0)).color(ink).align(Align::End).live_text_color(&b.tint).live_text(&b.status)),
        )
        .child(
            row()
                .h(lh(m.reading))
                .center_y()
                .child(text("0000.00").font(nums(m.reading, 640.0)).color(c(color::TEXT)).live_text(&b.reading))
                .child(spacer())
                .child(text("100%").font(nums(m.small, 500.0)).color(c(color::TEXT_3)).align(Align::End).live_text(&b.percent)),
        )
        .child(bars)
        .child(row().w_full().h(m.foot_h).gap(2.0 * m.gap).center_y().child(track).child(switch))
}
