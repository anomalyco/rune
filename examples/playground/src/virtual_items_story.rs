//! The "Lists/Virtual items canvas" playground story: `El::virtual_items` under a `Live` camera
//! (`live_offset` / `live_scale`) mounting only items near the viewport, with `mount_budget`,
//! nearest-first `mount_priority`, and focus pinning without re-running `view`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rune::tok::{self, radius, type_};
use rune::{Cx, Drag, El, Live, LiveText, Phase, Story, Tx, VirtualItems, col, el, row, text};

const GRID_COLS: usize = 50;
const GRID_ROWS: usize = 50; // 2,500 positioned canvas nodes
const CELL_W: f32 = 148.0;
const CELL_H: f32 = 84.0;
const VIEW_W: f32 = 520.0;
const VIEW_H: f32 = 280.0;

struct CanvasItems {
    ver: Cell<u64>,
    cam_x: Cell<f32>,
    cam_y: Cell<f32>,
    scale: Cell<f32>,
    budget: Cell<Option<usize>>,
    dark: Cell<bool>,
    visible: RefCell<Vec<usize>>,
    stats: LiveText,
}

impl CanvasItems {
    fn new() -> Rc<Self> {
        let s = Rc::new(Self {
            ver: Cell::new(1),
            cam_x: Cell::new(0.0),
            cam_y: Cell::new(0.0),
            scale: Cell::new(1.0),
            budget: Cell::new(Some(12)),
            dark: Cell::new(true),
            visible: RefCell::new(Vec::new()),
            stats: LiveText::new(""),
        });
        s.recompute();
        s
    }

    fn recompute(&self) {
        let scale = self.scale.get().max(0.25);
        let left = (-self.cam_x.get()) / scale - CELL_W;
        let top = (-self.cam_y.get()) / scale - CELL_H;
        let right = left + VIEW_W / scale + CELL_W * 2.0;
        let bottom = top + VIEW_H / scale + CELL_H * 2.0;

        let c0 = (left / CELL_W).floor().max(0.0) as usize;
        let c1 = ((right / CELL_W).ceil().max(0.0) as usize).min(GRID_COLS);
        let r0 = (top / CELL_H).floor().max(0.0) as usize;
        let r1 = ((bottom / CELL_H).ceil().max(0.0) as usize).min(GRID_ROWS);

        let mut next = Vec::new();
        for r in r0..r1 {
            for c in c0..c1 {
                next.push(r * GRID_COLS + c);
            }
        }
        let changed = *self.visible.borrow() != next;
        if changed {
            let count = next.len();
            *self.visible.borrow_mut() = next;
            self.ver.set(self.ver.get() + 1);
            let budget = self.budget.get().map_or("all".to_owned(), |n| format!("{n}/frame"));
            self.stats.set(format!(
                "2,500 nodes · {count} in window · budget {budget} · cam ({:.0}, {:.0}) @ {:.0}%",
                -self.cam_x.get(),
                -self.cam_y.get(),
                scale * 100.0
            ));
        }
    }

    fn center_world(&self) -> (f32, f32) {
        let scale = self.scale.get().max(0.25);
        ((-self.cam_x.get() + VIEW_W * 0.5) / scale, (-self.cam_y.get() + VIEW_H * 0.5) / scale)
    }
}

impl VirtualItems for CanvasItems {
    fn version(&self) -> u64 {
        self.ver.get()
    }

    fn keys(&self) -> Vec<String> {
        self.visible.borrow().iter().map(|i| format!("node-{i}")).collect()
    }

    fn build(&self, key: &str) -> El {
        let idx: usize = key.trim_start_matches("node-").parse().unwrap_or(0);
        let c = idx % GRID_COLS;
        let r = idx / GRID_COLS;
        let t = tok::theme(self.dark.get());
        let accent = tok::accent(idx % 6, self.dark.get());
        col()
            .size(CELL_W - 14.0, CELL_H - 14.0)
            .absolute(c as f32 * CELL_W + 7.0, r as f32 * CELL_H + 7.0)
            .padding(8.0)
            .gap(4.0)
            .radius(radius::SM)
            .fill(t.card)
            .border(1.0, t.card_border)
            .focus_border(accent)
            .focusable(true)
            .role("button")
            .label(format!("Node {r},{c}"))
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .center_y()
                    .child(text(format!("Node #{idx:04}")).font(type_::LABEL).color(t.text))
                    .child(el().size(8.0, 8.0).radius(4.0).fill(accent)),
            )
            .child(text(format!("grid ({c}, {r})")).font(type_::LABEL).color(t.text_secondary))
    }

    fn mount_budget(&self) -> Option<usize> {
        self.budget.get()
    }

    fn mount_priority(&self, key: &str) -> f32 {
        let idx: usize = key.trim_start_matches("node-").parse().unwrap_or(0);
        let x = (idx % GRID_COLS) as f32 * CELL_W + CELL_W * 0.5;
        let y = (idx / GRID_COLS) as f32 * CELL_H + CELL_H * 0.5;
        let (cx, cy) = self.center_world();
        let (dx, dy) = (x - cx, y - cy);
        dx * dx + dy * dy
    }
}

pub(crate) struct VirtualItemsDemo {
    cam_offset: Live<[f32; 2]>,
    cam_scale: Live<f32>,
    drag_start: [f32; 2],
    items: Rc<CanvasItems>,
}

impl Default for VirtualItemsDemo {
    fn default() -> Self {
        Self {
            cam_offset: Live::new([0.0, 0.0]),
            cam_scale: Live::new(1.0),
            drag_start: [0.0, 0.0],
            items: CanvasItems::new(),
        }
    }
}

impl VirtualItemsDemo {
    fn pan_to(&mut self, x: f32, y: f32) {
        self.items.cam_x.set(x);
        self.items.cam_y.set(y);
        self.cam_offset.set([x, y]);
        self.items.recompute();
    }

    fn set_zoom(&mut self, z: f32) {
        self.items.scale.set(z);
        self.cam_scale.set(z);
        self.items.recompute();
    }
}

pub(crate) fn story() -> Story<VirtualItemsDemo> {
    Story::new("Lists/Virtual items canvas", VirtualItemsDemo::default, view)
        .trigger('j', "Pan +300,+160", |s, tx| {
            tx.live_only();
            let (x, y) = (s.items.cam_x.get() - 300.0, s.items.cam_y.get() - 160.0);
            s.pan_to(x, y);
        })
        .trigger('r', "Reset camera", |s, tx| {
            tx.live_only();
            s.set_zoom(1.0);
            s.pan_to(0.0, 0.0);
        })
        .variant("Default (budget 12)", |_| {})
        .variant("Panned mid-grid", |s| s.pan_to(-1800.0, -960.0))
        .variant("Zoomed out 60%", |s| {
            s.set_zoom(0.6);
            s.pan_to(-900.0, -480.0);
        })
}

fn view(s: &VirtualItemsDemo, cx: &mut Cx<VirtualItemsDemo>) -> El {
    let dark = cx.system_dark();
    s.items.dark.set(dark);
    let t = tok::theme(dark);
    let accent = tok::accent(0, dark);

    let btn = |key: &'static str, label: &'static str, f: fn(&mut VirtualItemsDemo, &mut Tx)| {
        row()
            .key(key)
            .h(26.0)
            .px(10.0)
            .center()
            .radius(radius::SM)
            .fill(t.control_track)
            .hover(t.hover)
            .focusable(true)
            .role("button")
            .label(label)
            .on_press_tx(move |s: &mut VirtualItemsDemo, tx: &mut Tx| f(s, tx))
            .child(text(label).font(type_::LABEL).color(t.text))
    };

    let stage = el()
        .key("viewport")
        .size(VIEW_W, VIEW_H)
        .radius(radius::MD)
        .fill(t.bg)
        .border(1.0, t.card_border)
        .clip_soft(0.0)
        .on_drag(|s: &mut VirtualItemsDemo, d: &mut Drag, tx: &mut Tx| {
            tx.live_only();
            if d.phase == Phase::Down {
                s.drag_start = [s.items.cam_x.get(), s.items.cam_y.get()];
            }
            s.pan_to(s.drag_start[0] + d.dx, s.drag_start[1] + d.dy);
        })
        .child(
            el().key("canvas")
                .w_full()
                .h_full()
                .live_offset(&s.cam_offset)
                .live_scale(&s.cam_scale)
                .virtual_items(s.items.clone()),
        );

    col()
        .w_full()
        .h_full()
        .center()
        .padding(16.0)
        .gap(10.0)
        .child(
            row()
                .w(VIEW_W)
                .gap(8.0)
                .center_y()
                .child(btn("pan-right", "Pan →", |s, tx| {
                    tx.live_only();
                    let (x, y) = (s.items.cam_x.get() - 320.0, s.items.cam_y.get());
                    s.pan_to(x, y);
                }))
                .child(btn("pan-down", "Pan ↓", |s, tx| {
                    tx.live_only();
                    let (x, y) = (s.items.cam_x.get(), s.items.cam_y.get() - 180.0);
                    s.pan_to(x, y);
                }))
                .child(btn("zoom-toggle", "Toggle 60%/100%", |s, tx| {
                    tx.live_only();
                    let next = if s.items.scale.get() < 0.8 { 1.0 } else { 0.6 };
                    s.set_zoom(next);
                }))
                .child(btn("reset", "Reset", |s, tx| {
                    tx.live_only();
                    s.set_zoom(1.0);
                    s.pan_to(0.0, 0.0);
                })),
        )
        .child(stage)
        .child(
            row()
                .w(VIEW_W)
                .justify_between()
                .center_y()
                .child(text("").live_text(&s.items.stats).font(type_::LABEL).color(accent))
                .child(text("Drag canvas to pan · Tab focuses & pins a node").font(type_::LABEL).color(t.text_secondary)),
        )
}
