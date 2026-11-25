//! Inline link popover anchored to `Tx::link_rect()` on both pointer click and keyboard
//! Enter/Space (`rune-markdown--link-popover`).

use super::*;

const LINK_POPOVER_MD: &str = "Mention [@alex](mention:alex) for review, inspect [#2061](ref:2061), or follow [a longer wrapped reference that spans onto a second line inside the column](ref:wrapped) to anchor a preview card beside the link's first drawn line box.";

fn link_card_copy(url: &str) -> (&'static str, &'static str) {
    match url {
        "mention:alex" => ("@alex", "Maintainer · Online — review ready on dev"),
        "ref:2061" => ("Issue #2061", "Expose Tx::link_rect() for activated inline links"),
        "ref:wrapped" => ("Wrapped reference", "Anchored to the first drawn line box of the link"),
        _ => ("Link", "Activated inline link"),
    }
}

pub(super) fn markdown_link_popover(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    if let Some((url, rect)) = &s.link_popover {
        let (title, summary) = link_card_copy(url);
        let coords = format!(
            "rect: [{:.1}, {:.1}, {:.1}, {:.1}]",
            rect[0], rect[1], rect[2], rect[3]
        );
        layer.push(
            el().key("link-popover-anchor")
                .absolute(rect[0], rect[1])
                .size(rect[2], rect[3])
                .a11y_hidden(),
        );
        let card = popover::raised(
            k,
            col()
                .w(260.0)
                .padding(12.0)
                .gap(6.0)
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .justify_between()
                        .child(text(title).font(k.ty.ui_medium).color(k.c.text_strong))
                        .child(
                            Button::new("Close")
                                .variant(ButtonVariant::Ghost)
                                .control_size(ControlSize::Small)
                                .on_press(|s: &mut G| s.link_popover = None)
                                .build(k)
                                .key("link-popover-close"),
                        ),
                )
                .child(text(summary).font(k.ty.small).color(k.c.text_base).wrap())
                .child(text(coords).key("link-popover-coords").font(k.ty.small.mono()).color(k.c.text_weak)),
            k.radius.md,
        )
        .key("link-popover-card")
        .role("dialog")
        .label(format!("{title} preview"))
        .modal(true, |s: &mut G| s.link_popover = None);
        layer.push(popover::anchored(
            k,
            card,
            "link-popover-anchor",
            Placement::BottomStart,
            6.0,
        ));
    }

    let status = match &s.link_popover {
        Some((url, r)) => format!("Open: {url} at [{:.0}, {:.0}, {:.0}, {:.0}]", r[0], r[1], r[2], r[3]),
        None => "Click any link, or Tab to one and press Enter or Space.".to_owned(),
    };

    col()
        .w(380.0)
        .gap(12.0)
        .child(text("Link popover (Tx::link_rect)").font(k.ty.ui_medium).color(k.c.text_strong))
        .child(
            Markdown::new(LINK_POPOVER_MD)
                .on_link_tx(|s: &mut G, url, tx| {
                    if let Some(rect) = tx.link_rect() {
                        s.link_popover = Some((url.to_owned(), rect));
                    }
                })
                .build(k),
        )
        .child(text(status).key("link-popover-status").font(k.ty.small).color(k.c.text_weak))
}

#[cfg(test)]
mod tests {
    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    use crate::{init, stories, view};

    fn settle(rt: &mut Runtime<crate::G>) {
        for _ in 0..30 {
            if !rt.frame(1.0 / 60.0) {
                return;
            }
        }
    }

    #[test]
    fn link_popover_story_opens_beside_link_on_click_and_keyboard() {
        let mut g = init();
        let (_, setup) = stories::SCENES
            .iter()
            .find(|(id, _)| *id == "rune-markdown--link-popover")
            .expect("link-popover scene");
        setup(&mut g);

        let mut rt = Runtime::new(g, view, (640.0, 360.0), Env::new(true, 1.0));
        rt.frame(0.0);
        settle(&mut rt);

        // Pointer click on @alex opens the popover below @alex.
        let alex = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("mention:alex"))
            .expect("@alex link");
        rt.pointer_down(alex.rect[0] + alex.rect[2] * 0.5, alex.rect[1] + alex.rect[3] * 0.5);
        rt.pointer_up(alex.rect[0] + alex.rect[2] * 0.5, alex.rect[1] + alex.rect[3] * 0.5);
        settle(&mut rt);

        let (url, rect) = rt.state.link_popover.as_ref().expect("popover from click");
        assert_eq!(url, "mention:alex");
        assert!((rect[0] - alex.rect[0]).abs() < 0.5 && (rect[1] - alex.rect[1]).abs() < 0.5);
        let card = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("link-popover-card"))
            .expect("popover card");
        assert!((card.rect[1] - (rect[1] + rect[3] + 6.0)).abs() < 1.0);

        // Escape dismisses, then keyboard Enter on #2061 opens beside #2061.
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        settle(&mut rt);
        assert!(rt.state.link_popover.is_none());

        let issue = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("ref:2061"))
            .expect("#2061 link");
        rt.update(|_, tx| tx.focus("ref:2061", true));
        settle(&mut rt);
        rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
        settle(&mut rt);

        let (url, rect) = rt.state.link_popover.as_ref().expect("popover from Enter");
        assert_eq!(url, "ref:2061");
        assert!((rect[0] - issue.rect[0]).abs() < 0.5 && (rect[1] - issue.rect[1]).abs() < 0.5);
    }
}
