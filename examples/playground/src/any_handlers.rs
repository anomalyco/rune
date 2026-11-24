//! The "Controls/State-independent handlers" playground story: one non-generic reusable timeline
//! strip component using `El::on_layout_any`, `El::on_visible_any`, and `El::on_key_any` without
//! naming the host story's state type.

use std::cell::Cell;
use std::rc::Rc;

use rune::tok::{self, Theme, radius, type_};
use rune::{Color, Cx, El, Key, Live, LiveText, ScrollSpec, col, el, row, text};

const TICKS: [&str; 4] = ["00:00", "00:15", "00:30", "00:45"];

/// Component-owned state held behind `Rc`, completely independent of any host app state type.
pub(crate) struct StripModel {
    pub(crate) bounds: LiveText,
    pub(crate) visibility: LiveText,
    pub(crate) selection: LiveText,
    pub(crate) vis_opacity: Live<f32>,
    pub(crate) active_index: Cell<usize>,
    pub(crate) width: Cell<f32>,
    pub(crate) visible: Cell<bool>,
}

impl Default for StripModel {
    fn default() -> Self {
        Self {
            bounds: LiveText::new("Waiting for layout..."),
            visibility: LiveText::new("Offscreen"),
            selection: LiveText::new("Tick 1 / 4 (00:00)"),
            vis_opacity: Live::new(0.45),
            active_index: Cell::new(0),
            width: Cell::new(0.0),
            visible: Cell::new(false),
        }
    }
}

/// Reusable, state-independent timeline strip component (no `<S>` type parameter).
///
/// Uses `El::on_layout_any` to record its bounds and display scale, `El::on_visible_any` to track
/// viewport visibility, and `El::on_key_any` to step Left/Right while letting other keys bubble to
/// the host.
pub(crate) fn timeline_strip(model: &Rc<StripModel>, width: f32, t: Theme, accent: Color) -> El {
    let m_layout = model.clone();
    let m_vis = model.clone();
    let m_key = model.clone();

    col()
        .key("any-timeline")
        .w(width)
        .padding(12.0)
        .gap(8.0)
        .radius(radius::MD)
        .fill(t.card)
        .border(1.5, t.card_border)
        .focus_border(accent)
        .focusable(true)
        .role("group")
        .label("State-independent timeline strip")
        .on_layout_any(move |rect, tx| {
            m_layout.width.set(rect[2]);
            m_layout.bounds.set(format!("{:.0}×{:.0} @ {:.0}%", rect[2], rect[3], tx.scale() * 100.0));
            tx.live_only();
        })
        .on_visible_any(move |on, tx| {
            m_vis.visible.set(on);
            m_vis.visibility.set(if on { "Visible in viewport" } else { "Scrolled offscreen" });
            m_vis.vis_opacity.set(if on { 1.0 } else { 0.45 });
            tx.live_only();
        })
        .on_key_any(move |ev, tx| {
            let cur = m_key.active_index.get();
            let next = match ev.key {
                Key::Left => cur.saturating_sub(1),
                Key::Right => (cur + 1).min(TICKS.len() - 1),
                _ => return false,
            };
            m_key.active_index.set(next);
            m_key.selection.set(format!("Tick {} / {} ({})", next + 1, TICKS.len(), TICKS[next]));
            tx.live_only();
            true
        })
        .child(
            row()
                .w_full()
                .center_y()
                .justify_between()
                .child(text("Reusable timeline (click or Tab, then ← / →)").font(type_::LABEL).color(t.text))
                .child(
                    el()
                        .key("any-vis-dot")
                        .size(10.0, 10.0)
                        .radius(5.0)
                        .fill(accent)
                        .live_opacity(&model.vis_opacity),
                ),
        )
        .child(
            row()
                .w_full()
                .gap(6.0)
                .children(TICKS.iter().enumerate().map(|(idx, tick)| {
                    row()
                        .key(format!("any-tick-{idx}"))
                        .grow(1.0)
                        .h(26.0)
                        .center()
                        .radius(radius::SM)
                        .fill(t.control_track)
                        .child(text(*tick).font(type_::LABEL).color(t.text_secondary))
                })),
        )
}

pub(crate) struct AnyHandlersDemo {
    pub(crate) model: Rc<StripModel>,
    pub(crate) wide: bool,
    pub(crate) top_spacer: bool,
    pub(crate) outer_key: String,
}

impl Default for AnyHandlersDemo {
    fn default() -> Self {
        Self {
            model: Rc::new(StripModel::default()),
            wide: false,
            top_spacer: false,
            outer_key: "None (← / → consumed by component; ↑ / ↓ bubble here)".to_owned(),
        }
    }
}

pub(crate) fn view(s: &AnyHandlersDemo, cx: &mut Cx<AnyHandlersDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let strip_w = if s.wide { 396.0 } else { 296.0 };

    let btn = |key: &'static str, label: &'static str, on_press: fn(&mut AnyHandlersDemo)| {
        row()
            .key(key)
            .h(28.0)
            .px(10.0)
            .center()
            .radius(radius::SM)
            .fill(t.control_track)
            .hover(t.hover)
            .role("button")
            .label(label)
            .focusable(true)
            .child(text(label).font(type_::LABEL).color(t.text))
            .on_press(on_press)
    };

    col()
        .w_full()
        .h_full()
        .center()
        .padding(18.0)
        .gap(10.0)
        .on_key(|s: &mut AnyHandlersDemo, ev: &rune::KeyEvent, _| {
            if matches!(ev.key, Key::Up | Key::Down | Key::Enter) {
                s.outer_key = format!("Outer host caught {:?}", ev.key);
                true
            } else {
                false
            }
        })
        .child(
            row()
                .gap(8.0)
                .child(btn(
                    "any-toggle-width",
                    if s.wide { "Compact 296pt" } else { "Wide 396pt" },
                    |s| s.wide = !s.wide,
                ))
                .child(btn(
                    "any-toggle-spacer",
                    if s.top_spacer { "Bring on screen" } else { "Push offscreen" },
                    |s| s.top_spacer = !s.top_spacer,
                )),
        )
        .child(
            col()
                .key("any-readout")
                .w(420.0)
                .padding(10.0)
                .gap(4.0)
                .radius(radius::MD)
                .fill(t.card)
                .border(1.0, t.card_border)
                .child(
                    row()
                        .gap(6.0)
                        .child(text("on_layout_any:").font(type_::LABEL).color(t.text_secondary))
                        .child(text("Waiting...").key("any-bounds-text").w(220.0).h(16.0).font(type_::LABEL).color(t.text).live_text(&s.model.bounds)),
                )
                .child(
                    row()
                        .gap(6.0)
                        .child(text("on_visible_any:").font(type_::LABEL).color(t.text_secondary))
                        .child(text("Offscreen").key("any-vis-text").w(220.0).h(16.0).font(type_::LABEL).color(t.text).live_text(&s.model.visibility)),
                )
                .child(
                    row()
                        .gap(6.0)
                        .child(text("on_key_any:").font(type_::LABEL).color(t.text_secondary))
                        .child(text("Tick 1 / 4 (00:00)").key("any-sel-text").w(220.0).h(16.0).font(type_::LABEL).color(accent).live_text(&s.model.selection)),
                )
                .child(
                    text(s.outer_key.clone())
                        .key("any-outer-key")
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                ),
        )
        .child(
            col()
                .key("any-scroller")
                .size(420.0, 120.0)
                .padding(10.0)
                .gap(10.0)
                .radius(radius::MD)
                .fill(t.bg)
                .border(1.0, t.card_border)
                .scroll(ScrollSpec { stick_bottom: false })
                .children(s.top_spacer.then(|| {
                    col()
                        .w_full()
                        .h(640.0)
                        .center()
                        .child(text("Scroll down to reveal the strip below").font(type_::LABEL).color(t.text_secondary))
                }))
                .child(timeline_strip(&s.model, strip_w, t, accent))
                .child(el().w_full().h(640.0)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, KeyEvent, Mods, Runtime};

    #[test]
    fn story_exercises_layout_visible_and_key_any_without_host_state() {
        let mut rt = Runtime::new(AnyHandlersDemo::default(), view, (480.0, 320.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.model.width.get(), 296.0);
        assert!(rt.state.model.visible.get());

        // Toggle width -> on_layout_any updates without naming host state.
        let (wx, wy) = rt.find("any-toggle-width").unwrap();
        rt.pointer_down(wx, wy);
        rt.pointer_up(wx, wy);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.model.width.get(), 396.0);

        // Push strip offscreen -> on_visible_any reports false once the layout settles.
        let (sx, sy) = rt.find("any-toggle-spacer").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
        }
        assert!(!rt.state.model.visible.get());

        // Bring back on screen -> on_visible_any reports true.
        let (sx, sy) = rt.find("any-toggle-spacer").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.model.visible.get());

        // Focus strip and send Right (consumed by on_key_any) and Up (bubbles to host on_key).
        let (tx, ty) = rt.find("any-timeline").unwrap();
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.model.active_index.get(), 1);
        assert!(!rt.state.outer_key.contains("Right"));

        rt.key(KeyEvent { key: Key::Up, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.outer_key.contains("Up"));
    }
}
