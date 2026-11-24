//! Sidebar drag-to-reorder gallery story exercising `ReorderLanding::HalfRow`,
//! `Reorder::cancel_outside_x`, `Reorder::offset`/`pitch`/`shift`, and `SidebarRow::reorder_tx`.

use rune::{El, LiveRegion, Tx, col, row, text};
use rune_kit::shell::{Reorder, ReorderLanding, Sidebar, SidebarRow};
use rune_kit::{Kit, SegmentedControl, Switch};

use crate::G;

const ROW_HEIGHT: f32 = 24.0;
const ROW_GAP: f32 = 2.0;
const CANCEL_X: f32 = 80.0;

pub struct ReorderDemo {
    pub items: Vec<&'static str>,
    pub selected: usize,
    pub landing: ReorderLanding,
    pub cancel_x: bool,
    pub reorder: Reorder,
    pub last_drop: Option<(usize, usize)>,
}

impl Default for ReorderDemo {
    fn default() -> Self {
        Self {
            items: vec!["Home", "Desktop", "Documents", "Downloads", "Projects"],
            selected: 0,
            landing: ReorderLanding::HalfRow,
            cancel_x: true,
            reorder: build_reorder(ReorderLanding::HalfRow, true),
            last_drop: None,
        }
    }
}

impl ReorderDemo {
    fn rebuild(&mut self) {
        self.reorder = build_reorder(self.landing, self.cancel_x);
    }
}

fn build_reorder(landing: ReorderLanding, cancel_x: bool) -> Reorder {
    let r = Reorder::new().gap(ROW_GAP).landing(landing);
    if cancel_x { r.cancel_outside_x(CANCEL_X) } else { r }
}

pub(super) fn reorder(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let demo = &s.reorder;
    let landing_idx = if demo.landing == ReorderLanding::HalfRow { 0 } else { 1 };
    let controls = row()
        .key("reorder-controls")
        .gap(12.0)
        .center_y()
        .child(
            SegmentedControl::new(&["HalfRow", "Nearest"], Some(landing_idx))
                .on_pick(|s: &mut G, i, _| {
                    s.reorder.landing = if i == 0 { ReorderLanding::HalfRow } else { ReorderLanding::Nearest };
                    s.reorder.rebuild();
                })
                .build(k)
                .key("landing-mode"),
        )
        .child(
            Switch::new(demo.cancel_x)
                .label("Cancel outside ±80 pt")
                .on_toggle(|s: &mut G| {
                    s.reorder.cancel_x = !s.reorder.cancel_x;
                    s.reorder.rebuild();
                })
                .build(k)
                .key("cancel-x-toggle"),
        );

    let count = demo.items.len();
    let sb = Sidebar::new().label("Places").inset(6.0).row_gap(ROW_GAP);
    let sb = demo.items.iter().enumerate().fold(sb, |sb, (i, name)| {
        sb.child(
            SidebarRow::new(*name)
                .height(ROW_HEIGHT)
                .selected(demo.selected == i)
                .on_select(move |s: &mut G, _| s.reorder.selected = i)
                .reorder_tx(
                    &demo.reorder,
                    i,
                    count,
                    |s: &mut G| &mut s.reorder.reorder,
                    |s: &mut G, from, to, tx: &mut Tx| {
                        let moved = s.reorder.items.remove(from);
                        s.reorder.items.insert(to, moved);
                        s.reorder.selected = to;
                        s.reorder.last_drop = Some((from, to));
                        tx.announce(format!("Moved {moved} to position {}", to + 1), LiveRegion::Polite);
                    },
                )
                .build(k)
                .key(*name),
        )
    });

    let status_text = match demo.reorder.dragging() {
        Some(from) => {
            let dy = demo.reorder.offset().unwrap_or(0.0);
            let pitch = demo.reorder.pitch().unwrap_or(0.0);
            let shift = demo.reorder.shift(from);
            match demo.reorder.target() {
                Some(to) => format!("Dragging #{from} -> #{to}  dy={dy:.0} pt  pitch={pitch:.0} pt  shift={shift:.0} pt"),
                None => format!("Dragging #{from} -> Cancelled (outside ±{CANCEL_X:.0} pt)  dy={dy:.0} pt"),
            }
        }
        None => match demo.last_drop {
            Some((from, to)) => format!("Last drop: ({from}, {to})"),
            None => "Drag rows vertically; pull sideways past ±80 pt to cancel".to_owned(),
        },
    };

    col()
        .key("sidebar-reorder-story")
        .gap(12.0)
        .child(controls)
        .child(
            col()
                .key("places-frame")
                .w(240.0)
                .h(168.0)
                .py(6.0)
                .fill(c.layer_01)
                .border(1.0, c.border_weak)
                .radius(k.radius.md)
                .child(sb.build(k)),
        )
        .child(text(status_text).key("reorder-status").font(k.ty.small).color(c.text_muted))
}
