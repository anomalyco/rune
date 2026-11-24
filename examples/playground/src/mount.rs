//! The "Lifecycle/On mount" playground story: toggling a tooltip panel whose `El::on_mount`
//! records mount state and mounts a chained badge in the same frame before drawing, with no
//! one-frame lag.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, Transition, Tx, col, row, text};

#[derive(Default)]
pub(crate) struct MountDemo {
    pub(crate) show_tip: bool,
    pub(crate) rebuilds: usize,
    pub(crate) mounts: usize,
    pub(crate) child_mounts: usize,
    pub(crate) status: String,
}

impl MountDemo {
    pub(crate) fn shown() -> Self {
        Self { show_tip: true, ..Self::default() }
    }
}

pub(crate) fn view(s: &MountDemo, cx: &mut Cx<MountDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let ok = tok::accent(4, cx.system_dark());

    let toggle_label = if s.show_tip { "Hide tooltip" } else { "Show tooltip" };
    let button = |key: &'static str, label: &'static str, on_press: fn(&mut MountDemo)| {
        row().key(key).h(28.0).px(12.0).center().radius(radius::SM)
            .fill(t.control_track).hover(t.hover)
            .role("button").label(label).focusable(true)
            .child(text(label).font(type_::LABEL).color(t.text))
            .on_press(on_press)
    };

    let tip = s.show_tip.then(|| {
        let ready = !s.status.is_empty();
        col().key("mount-tip").w(340.0).padding(14.0).gap(8.0).radius(radius::MD)
            .fill(t.card)
            .border(1.5, if ready { accent } else { t.card_border })
            .transition(Transition::opacity())
            .role("status").label("Mounted tooltip")
            .on_mount(|s: &mut MountDemo, _tx: &mut Tx| {
                s.mounts += 1;
                s.status = format!("Mounted (#{}) in same frame", s.mounts);
            })
            .child(
                row().w_full().center_y().justify_between()
                    .child(
                        text(if ready { s.status.as_str() } else { "Pending mount..." })
                            .key("mount-tip-status")
                            .font(type_::BODY)
                            .color(if ready { t.text } else { t.text_secondary }),
                    )
                    .children(ready.then(|| {
                        row().key("mount-chain-badge").h(22.0).px(8.0).center().radius(11.0).fill(ok)
                            .on_mount(|s: &mut MountDemo, _| s.child_mounts += 1)
                            .child(
                                text(format!("chain #{}", s.child_mounts))
                                    .key("mount-chain-text")
                                    .font(type_::LABEL)
                                    .color(Color::WHITE),
                            )
                    })),
            )
            .child(
                text(format!(
                    "panel mounts: {} · badge mounts: {} · rebuilds: {}",
                    s.mounts, s.child_mounts, s.rebuilds
                ))
                .key("mount-counts")
                .font(type_::LABEL)
                .color(t.text_secondary),
            )
    });

    col().w_full().h_full().center().padding(20.0).gap(12.0)
        .child(
            row().gap(8.0)
                .child(button("toggle-tip", toggle_label, |s| {
                    s.show_tip = !s.show_tip;
                    if !s.show_tip {
                        s.status.clear();
                    }
                }))
                .child(button("rebuild-tip", "Rebuild view", |s| s.rebuilds += 1)),
        )
        .children(tip)
}
