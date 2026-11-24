//! Rune's own `SplitView` scene showing `SplitView::flat_main` and `SidebarToggle::labels`.

use rune::{El, col, row, text};
use rune_kit::shell::{self, Sidebar, SidebarRow, SidebarSection, SidebarToggle, SplitView, Toolbar};
use rune_kit::{IconButton, IconButtonVariant, Kit, Switch};

use crate::G;

pub(super) fn flat_main(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let flat = !s.is("split-card");
    let localized = !s.is("split-english");
    let picked = s.picked("split-file", "split.rs");
    let files = ["split.rs", "pane.rs", "toolbar.rs", "sidebar.rs"];
    let rows = files.into_iter().map(|name| {
        SidebarRow::new(name)
            .selected(picked == name)
            .on_select(move |s: &mut G, _| {
                s.pick.insert("split-file".into(), name.into());
            })
            .build(k)
            .key(name)
    });
    let sidebar = Sidebar::new()
        .label("Explorer")
        .section(SidebarSection::new("shell").rows(rows))
        .build(k);
    let mut toggle = SidebarToggle::new();
    if localized {
        toggle = toggle.labels("Afficher le panneau", "Masquer le panneau");
    }
    let main = col()
        .key("split-main-body")
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .child(text(picked).font(k.ty.body_medium).color(c.text_base))
        .child(
            text("Toggle flat_main to compare hairline pane dividers with the rounded card, and hover the sidebar toggle to inspect its localized tooltip.")
                .font(k.ty.small)
                .color(c.text_muted)
                .wrap(),
        )
        .child(
            row()
                .gap(16.0)
                .center_y()
                .child(
                    Switch::new(flat)
                        .label("Flat main pane")
                        .on_toggle(|s: &mut G| s.flip("split-card"))
                        .build(k)
                        .key("toggle-flat"),
                )
                .child(
                    Switch::new(localized)
                        .label("Custom toggle labels")
                        .on_toggle(|s: &mut G| s.flip("split-english"))
                        .build(k)
                        .key("toggle-labels"),
                ),
        );
    let inspector = col()
        .key("split-inspector-body")
        .w_full()
        .padding(14.0)
        .gap(8.0)
        .child(text("Inspector").font(k.ty.ui_medium).color(c.text_base))
        .child(
            text(if flat { "Mode: flat_main(true)" } else { "Mode: rounded card" })
                .font(k.ty.small)
                .color(c.text_muted),
        )
        .child(
            text(if localized {
                "Toggle: Afficher / Masquer"
            } else {
                "Toggle: Show / Hide sidebar"
            })
            .font(k.ty.small)
            .color(c.text_muted),
        );
    let toolbar = Toolbar::new().title("Workspace").trailing(
        IconButton::new(&shell::INSPECTOR)
            .variant(IconButtonVariant::GhostMuted)
            .expanded(s.split_inspector.is_open())
            .label(if s.split_inspector.is_open() { "Hide inspector" } else { "Show inspector" })
            .on_press(|s: &mut G| s.split_inspector.toggle())
            .build(k)
            .key("inspector-toggle"),
    );
    let shell = SplitView::new(main)
        .sidebar(&s.split_sidebar, |s: &mut G| &mut s.split_sidebar, sidebar)
        .inspector(&s.split_inspector, |s: &mut G| &mut s.split_inspector, inspector)
        .toolbar(toolbar)
        .sidebar_toggle(toggle)
        .flat_main(flat)
        .build(k);
    col()
        .key("split-frame")
        .w(672.0)
        .h(340.0)
        .border(1.0, c.border_weak)
        .clip_soft(0.0)
        .child(shell)
}
