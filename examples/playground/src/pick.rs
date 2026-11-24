//! The "Layout/Pointer pick" playground story: live `Tx::element_at_in_with` overlay comparing
//! `Pick::default()`, `Pick::interactive(true)`, `Pick::drop_targets(true)`, and both over a
//! passive full-size layer, showing `Picked::rect()` and `Picked::local()`.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, Pick, Picked, Tx, col, el, row, text};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PickMode {
    All,
    #[default]
    Interactive,
    DropTargets,
    Both,
}

const MODES: [(PickMode, &str, &str); 4] = [
    (PickMode::All, "mode-all", "All"),
    (PickMode::Interactive, "mode-interactive", "Interactive"),
    (PickMode::DropTargets, "mode-drops", "Drop targets"),
    (PickMode::Both, "mode-both", "Both"),
];

impl PickMode {
    pub(crate) fn pick(self) -> Pick {
        match self {
            Self::All => Pick::default(),
            Self::Interactive => Pick::new().interactive(true),
            Self::DropTargets => Pick::new().drop_targets(true),
            Self::Both => Pick::new().interactive(true).drop_targets(true),
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Interactive,
            Self::Interactive => Self::DropTargets,
            Self::DropTargets => Self::Both,
            Self::Both => Self::All,
        }
    }
}

pub(crate) struct PointerPick {
    pub(crate) mode: PickMode,
    pub(crate) cursor: Option<[f32; 2]>,
    pub(crate) hit: Option<Picked>,
    pub(crate) clicks: u32,
    pub(crate) pending_probe: Option<&'static str>,
}

impl Default for PointerPick {
    fn default() -> Self {
        Self {
            mode: PickMode::Interactive,
            cursor: None,
            hit: None,
            clicks: 0,
            pending_probe: None,
        }
    }
}

impl PointerPick {
    pub(crate) fn with_probe(mode: PickMode, key: &'static str) -> Self {
        Self { mode, pending_probe: Some(key), ..Self::default() }
    }
}

pub(crate) fn poll(s: &mut PointerPick, _t: f64, tx: &mut Tx) -> bool {
    if let Some(target_key) = s.pending_probe.take() {
        let mode = s.mode;
        tx.read_elements::<PointerPick>(move |s, elements, tx| {
            let Some(e) = elements.iter().find(|e| e.key.as_deref() == Some(target_key)) else { return };
            let x = e.rect[0] + e.rect[2] * 0.5;
            let y = e.rect[1] + e.rect[3] * 0.5;
            s.cursor = Some([x, y]);
            tx.element_at_in_with("pick-targets", x, y, mode.pick(), |s: &mut PointerPick, picked, _| {
                s.hit = picked;
            });
            tx.wake_after(std::time::Duration::ZERO);
        });
    }
    false
}

pub(crate) fn view(s: &PointerPick, cx: &mut Cx<PointerPick>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let green = tok::accent(4, cx.system_dark());
    let mode = s.mode;

    let stage_origin = cx.layout("pick-targets").map_or([0.0, 0.0], |r| [r[0], r[1]]);
    let mode_bar = row().gap(6.0).center_y()
        .children(MODES.iter().map(|&(m, key, label)| {
            let on = s.mode == m;
            row().key(key).h(26.0).px(10.0).center().radius(radius::SM)
                .fill(if on { accent } else { t.control_track })
                .hover(if on { accent } else { t.hover })
                .role("button").label(format!("Pick mode: {label}")).focusable(true)
                .child(text(label).font(type_::LABEL).color(if on { Color::WHITE } else { t.text }))
                .on_press_tx(move |s: &mut PointerPick, tx: &mut Tx| {
                    s.mode = m;
                    if let Some([x, y]) = s.cursor {
                        tx.element_at_in_with("pick-targets", x, y, m.pick(), |s: &mut PointerPick, picked, _| {
                            s.hit = picked;
                        });
                    }
                })
        }));

    let hit_box = s.hit.as_ref().map(|p| {
        let r = p.rect();
        el().key("pick-highlight")
            .absolute(r[0] - stage_origin[0], r[1] - stage_origin[1])
            .size(r[2], r[3])
            .radius(radius::SM)
            .fill(accent.alpha(0.14))
            .border(1.5, accent)
    });
    let dot = s.cursor.map(|[x, y]| {
        el().key("pick-cursor-dot")
            .absolute(x - stage_origin[0] - 4.0, y - stage_origin[1] - 4.0)
            .size(8.0, 8.0)
            .radius(4.0)
            .fill(accent)
    });

    let targets = el().key("pick-targets").w_full().h(196.0).radius(radius::MD)
        .fill(t.card).outline(1.0, 0.0, t.card_border)
        .child(
            row().w_full().h_full().padding(14.0).gap(12.0)
                .child(
                    col().grow(1.0).h_full().gap(8.0)
                        .child(
                            row().key("pick-row-1").w_full().h(36.0).px(12.0).center_y().justify_between()
                                .radius(radius::SM).fill(t.control_track).hover(t.hover)
                                .role("button").label("Inbox row").focusable(true)
                                .child(text("Inbox · on_press").font(type_::BODY).color(t.text))
                                .child(text(format!("{} clicks", s.clicks)).font(type_::LABEL).color(t.text_secondary))
                                .on_press(|s: &mut PointerPick| s.clicks += 1),
                        )
                        .child(
                            row().key("pick-row-2").w_full().h(36.0).px(12.0).center_y().justify_between()
                                .radius(radius::SM).fill(t.control_track).hover(t.hover)
                                .role("button").label("Archive row").focusable(true)
                                .child(text("Archive · on_press").font(type_::BODY).color(t.text))
                                .child(text("interactive").font(type_::LABEL).color(t.text_secondary))
                                .on_press(|s: &mut PointerPick| s.clicks += 1),
                        )
                        .child(
                            row().key("pick-drop-zone").w_full().grow(1.0).px(12.0).center_y().justify_between()
                                .radius(radius::SM).fill(green.alpha(0.12)).border(1.0, green.alpha(0.6))
                                .child(text("Attachments · on_drop_files").font(type_::BODY).color(t.text))
                                .child(text("drop target").font(type_::LABEL).color(t.text_secondary))
                                .on_drop_files(|_: &mut PointerPick, _, _| {}),
                        ),
                )
                .child(
                    col().w(128.0).h_full().center().child(
                        col().key("pick-rotated").size(104.0, 60.0).center().gap(2.0)
                            .radius(radius::SM).fill(t.control_track).hover(t.hover)
                            .rotate(0.16)
                            .role("button").label("Tilted target").focusable(true)
                            .child(text("Tilted").font(type_::BODY).color(t.text))
                            .child(text("rotate(0.16)").font(type_::LABEL).color(t.text_secondary))
                            .on_press(|s: &mut PointerPick| s.clicks += 1),
                    ),
                ),
        )
        // Full-size passive overlay layer (e.g. a scrollbar overlay) with no handlers.
        .child(
            el().key("pick-passive-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .child(el().key("pick-scrollbar").absolute(452.0, 12.0).size(4.0, 72.0).radius(2.0).fill(t.text_secondary.alpha(0.35))),
        )
        .children(hit_box)
        .children(dot);

    let stage = el().key("pick-stage").w_full()
        .on_pointer_move_local(move |s: &mut PointerPick, p, tx| {
            s.cursor = Some([p.window_x, p.window_y]);
            tx.element_at_in_with("pick-targets", p.window_x, p.window_y, mode.pick(), |s: &mut PointerPick, picked, _| {
                s.hit = picked;
            });
        })
        .child(targets);

    let (target_line, geom_line) = match &s.hit {
        Some(p) => {
            let name = p.key().unwrap_or_else(|| p.path());
            let r = p.rect();
            let l = p.local();
            (
                format!("Picked: {name}  ({})", p.path()),
                format!("rect [{:.0}, {:.0}, {:.0} × {:.0}] pt · local [{:.1}, {:.1}] pt", r[0], r[1], r[2], r[3], l[0], l[1]),
            )
        }
        None => (
            "Move the pointer across the stage to pick elements".to_owned(),
            "Passive layer sits over every row and drop zone".to_owned(),
        ),
    };

    let readout = col().w_full().padding(12.0).gap(4.0).radius(radius::MD)
        .fill(t.card).outline(1.0, 0.0, t.card_border)
        .child(text(target_line).key("pick-readout-target").font(type_::LABEL).color(t.text))
        .child(text(geom_line).key("pick-readout-geom").font(type_::LABEL).color(t.text_secondary));

    col().w_full().h_full().center().padding(20.0).child(
        col().w(464.0).gap(12.0)
            .child(mode_bar)
            .child(stage)
            .child(readout),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn pointer_pick_story_filters_passive_layer_and_reports_geometry() {
        let mut rt = Runtime::new(PointerPick::default(), view, (540.0, 340.0), Env::new(true, 1.0));
        rt.frame(0.0);

        let a11y = rt.accessibility_json().to_string();
        assert!(a11y.contains("Pick mode: Interactive"));
        assert!(a11y.contains("Pick mode: Drop targets"));
        assert!(a11y.contains("Inbox row"));

        // Hover over Inbox row under Interactive mode -> skips passive layer.
        let (rx, ry) = rt.find("pick-row-1").unwrap();
        rt.pointer_move(rx, ry);
        rt.frame(0.0);
        let hit = rt.state.hit.as_ref().expect("interactive pick hits row");
        assert_eq!(hit.key(), Some("pick-row-1"));
        assert!(hit.rect()[2] > 100.0 && hit.rect()[3] > 20.0);
        assert!(hit.local()[0] > 0.0 && hit.local()[1] > 0.0);

        // Switch to All mode -> same point stops at the passive layer.
        let (ax, ay) = rt.find("mode-all").unwrap();
        rt.pointer_down(ax, ay);
        rt.pointer_up(ax, ay);
        rt.frame(0.0);
        assert_eq!(rt.state.hit.as_ref().and_then(Picked::key), Some("pick-passive-layer"));

        // Switch to Drop targets mode and hover over the drop zone.
        let (dx, dy) = rt.find("mode-drops").unwrap();
        rt.pointer_down(dx, dy);
        rt.pointer_up(dx, dy);
        rt.frame(0.0);
        let (zx, zy) = rt.find("pick-drop-zone").unwrap();
        rt.pointer_move(zx, zy);
        rt.frame(0.0);
        assert_eq!(rt.state.hit.as_ref().and_then(Picked::key), Some("pick-drop-zone"));
    }
}
