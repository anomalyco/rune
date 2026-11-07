//! The stress HUD built once, its numbers and graph bars live values.

use motion_bento_chaos::hud::{BUDGET_60, BUDGET_120, GRAPH, GRAPH_MAX, Readout};
use motion_bento_chaos::look::{Lch, color, grid};
use motion_bento_chaos::stress::HEADER;
use rune::{Color, El, Live, LiveText, col, el, row, text};

use super::{GRAPH_H, GRAPH_W, HUD_W, OVER, grade_color};
use crate::style::{c, caption, nums};

pub struct Hud {
    source: &'static str,
    big: LiveText,
    big_color: Live<Color>,
    detail: LiveText,
    rate: LiveText,
    bars: [(Live<[f32; 4]>, Live<Color>); GRAPH],
}

impl Hud {
    pub fn new() -> Hud {
        Hud {
            source: "",
            big: LiveText::new(""),
            big_color: Live::new(c(color::GOOD)),
            detail: LiveText::new(""),
            rate: LiveText::new(""),
            bars: std::array::from_fn(|_| (Live::new([0.0; 4]), Live::new(c(color::GOOD)))),
        }
    }

    pub fn set(&mut self, r: &Readout) {
        self.source = r.source;
        self.big.set(r.big());
        self.big_color.set(c(grade_color(r.ms)));
        self.detail.set(r.detail());
        self.rate.set(r.rate());
        let bw = GRAPH_W / GRAPH as f32;
        for (i, (rect, fill)) in self.bars.iter().enumerate() {
            let ms = r.graph.get(i).copied().unwrap_or(0.0);
            let h = (ms / GRAPH_MAX).min(1.0) * GRAPH_H;
            rect.set([i as f32 * bw, GRAPH_H - h, bw, h]);
            fill.set(c(grade_color(ms)));
        }
    }

    pub fn view(&self) -> El {
        let mut graph = el().size(GRAPH_W, GRAPH_H).absolute(0.0, 0.0);
        for (rect, fill) in &self.bars {
            graph = graph.child(el().absolute(0.0, 0.0).size(0.0, 0.0).fill(c(color::GOOD)).live_fill(fill).live_rect(rect));
        }
        let line = |ms: f32, col: Lch| el().size(GRAPH_W, 1.0).absolute(0.0, GRAPH_H - ms / GRAPH_MAX * GRAPH_H).fill(c(col));
        let label = |ms: f32, s: &str| text(s.to_owned()).font(nums(10.0, 560.0)).color(c(color::TEXT_2)).absolute(GRAPH_W + 6.0, GRAPH_H - ms / GRAPH_MAX * GRAPH_H - 7.0);
        let graph = el()
            .size(GRAPH_W + 40.0, GRAPH_H)
            .child(el().size(GRAPH_W, GRAPH_H).absolute(0.0, 0.0).radius(4.0).fill(c(color::WELL)))
            .child(graph)
            .child(line(BUDGET_120, color::ACCENT))
            .child(line(BUDGET_60, OVER))
            .child(label(BUDGET_120, "120 Hz"))
            .child(label(BUDGET_60, "60 Hz"));
        let small = |placeholder: &str, live: &LiveText| text(placeholder.to_owned()).font(nums(11.0, 500.0)).color(c(color::TEXT_2)).live_text(live);
        row()
            .key("hud")
            .absolute(grid::WIDTH - grid::PAD - HUD_W, grid::PAD - 8.0)
            .size(HUD_W, HEADER)
            .padding(12.0)
            .gap(14.0)
            .radius(14.0)
            .fill(c(color::TILE))
            .border(1.0, c(color::ACCENT.alpha(0.4)))
            .raise()
            .child(
                col()
                    .w(HUD_W - GRAPH_W - 40.0 - 14.0 - 24.0)
                    .gap(1.0)
                    .child(caption(format!("Rune · {}", self.source)))
                    .child(text("00.0 ms").font(nums(36.0, 700.0)).color(c(color::GOOD)).live_text_color(&self.big_color).live_text(&self.big))
                    .child(small("build + layout + paint · p95 00.0", &self.detail))
                    .child(small("≈ 000 fps at 120 Hz · 000 missed / s", &self.rate)),
            )
            .child(graph)
    }
}
