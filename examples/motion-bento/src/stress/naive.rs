//! `--naive`: the whole wall built again every frame, as a first port draws it. Every number,
//! bar and colour that moves reruns the view and commits the whole tree.

use motion_bento_chaos::hud::{BUDGET_60, BUDGET_120, GRAPH, GRAPH_MAX, Readout};
use motion_bento_chaos::look::{Lch, color, grid, lch};
use motion_bento_chaos::stress::{self, Cell, HEADER, Metrics, Wall, lh};
use rune::{El, Transition, col, el, row, spacer, text};

use super::{GRAPH_H, GRAPH_W, HUD_W, KNOB, OVER, SWITCH_OFF, grade_color};
use crate::State;
use crate::style::{c, caption, font, nums, spring};

pub fn view(s: &State, wall: &Wall) -> El {
    let mut root = el().w(grid::WIDTH).h(grid::HEIGHT).fill(c(color::BG)).child(header(s, wall));
    for cell in wall.cells() {
        root = root.child(control(s, cell, &wall.metrics));
    }
    if let Some(r) = &s.readout {
        root = root.child(hud(r));
    }
    root
}

fn header(s: &State, wall: &Wall) -> El {
    let load = crate::config().load.as_ref();
    let steps = load.map_or(1, |l| l.levels().len());
    let step = load.map_or(0, |l| l.step_at(s.t));
    let n = wall.cells().len();
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
        .child(caption(format!("Seed {} · every control retargets, ticks and reflows at once · t {:.2} s", crate::config().seed, s.t)))
        .child(
            row()
                .gap(10.0)
                .items_end()
                .child(text(format!("{n}")).font(nums(40.0, 700.0)).color(c(color::TEXT)))
                .child(text("live controls").font(font(16.0, 560.0)).color(c(color::TEXT_2)))
                .child(caption(format!("step {} of {steps} · {}×{} grid", step + 1, wall.cols, wall.rows))),
        )
}

fn control(s: &State, cell: &Cell, m: &Metrics) -> El {
    let [x, y, w, h] = cell.rect;
    let tint = stress::tint(cell.hue);
    let (fill, edge) = if cell.hot { (stress::hot_fill(cell.hue), stress::hot_edge(cell.hue)) } else { (color::TILE, color::EDGE) };
    let travel = spring(s.springs.travel);
    let snap = spring(s.springs.snap);
    let mut bars = row().w_full().h(m.bars_h).gap(m.bar_gap).items_end();
    for &b in &cell.bars {
        bars = bars.child(el().grow(1.0).h(b * m.bars_h).radius(2.0 * m.u).gradient(c(stress::tint_hi(cell.hue)), c(stress::tint_lo(cell.hue))).motion(travel));
    }
    let inset = (m.switch_h - m.knob) / 2.0;
    let switch = el()
        .size(m.switch_w, m.switch_h)
        .radius(m.switch_h / 2.0)
        .fill(c(if cell.on { tint } else { SWITCH_OFF }))
        .motion(snap)
        .child(el().size(m.knob, m.knob).absolute(if cell.on { m.switch_w - inset - m.knob } else { inset }, inset).radius(m.knob / 2.0).fill(c(KNOB)).motion(snap));
    let track = el()
        .grow(1.0)
        .h(m.track_h)
        .radius(m.track_h / 2.0)
        .fill(c(color::WELL))
        .child(el().w_frac(cell.progress).h(m.track_h).radius(m.track_h / 2.0).fill(c(tint)));
    col()
        .key(format!("z{}", cell.id))
        .absolute(x, y)
        .size(w, h)
        .padding(m.pad)
        .gap(m.gap)
        .radius(m.radius)
        .fill(c(fill))
        .border(1.0, c(edge))
        .shadow(m.shadow_y, m.shadow_blur, c(lch(0.0, 0.0, 0.0).alpha(0.35)))
        .clip_rounded(m.radius)
        .motion(spring(s.springs.layout))
        .transition(Transition::scale(0.85).combined(Transition::opacity()))
        .child(
            row()
                .h(lh(m.name))
                .gap(m.gap)
                .center_y()
                .child(el().size(m.dot, m.dot).radius(m.dot / 2.0).fill(c(tint)).shadow(0.0, 2.0 * m.dot, c(tint.alpha(0.8))))
                .child(text(cell.name()).font(font(m.name, 520.0)).color(c(color::TEXT_2)))
                .child(spacer())
                .child(text(cell.status).font(font(m.small, 600.0)).color(c(tint))),
        )
        .child(
            row()
                .h(lh(m.reading))
                .center_y()
                .child(text(cell.reading()).font(nums(m.reading, 640.0)).color(c(color::TEXT)))
                .child(spacer())
                .child(text(cell.percent()).font(nums(m.small, 500.0)).color(c(color::TEXT_3))),
        )
        .child(bars)
        .child(row().w_full().h(m.foot_h).gap(2.0 * m.gap).center_y().child(track).child(switch))
}

fn hud(r: &Readout) -> El {
    let bw = GRAPH_W / GRAPH as f32;
    let mut graph = row().size(GRAPH_W, GRAPH_H).items_end();
    for &ms in &r.graph {
        graph = graph.child(el().w(bw).h((ms / GRAPH_MAX).min(1.0) * GRAPH_H).fill(c(grade_color(ms))));
    }
    let line = |ms: f32, col: Lch| el().size(GRAPH_W, 1.0).absolute(0.0, GRAPH_H - ms / GRAPH_MAX * GRAPH_H).fill(c(col));
    let label = |ms: f32, s: &str| text(s.to_owned()).font(nums(10.0, 560.0)).color(c(color::TEXT_2)).absolute(GRAPH_W + 6.0, GRAPH_H - ms / GRAPH_MAX * GRAPH_H - 7.0);
    let graph = el()
        .size(GRAPH_W + 40.0, GRAPH_H)
        .child(el().size(GRAPH_W, GRAPH_H).absolute(0.0, 0.0).radius(4.0).fill(c(color::WELL)))
        .child(graph.absolute(0.0, 0.0))
        .child(line(BUDGET_120, color::ACCENT))
        .child(line(BUDGET_60, OVER))
        .child(label(BUDGET_120, "120 Hz"))
        .child(label(BUDGET_60, "60 Hz"));
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
                .child(caption(format!("Rune · {}", r.source)))
                .child(text(r.big()).font(nums(36.0, 700.0)).color(c(grade_color(r.ms))))
                .child(text(r.detail()).font(nums(11.0, 500.0)).color(c(color::TEXT_2)))
                .child(text(r.rate()).font(nums(11.0, 500.0)).color(c(color::TEXT_2))),
        )
        .child(graph)
}
