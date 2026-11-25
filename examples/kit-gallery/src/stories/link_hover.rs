//! Inline link hover preview card anchored to `Markdown::on_link_hover` (`rune-markdown--link-hover`).

use super::*;

const LINK_HOVER_MD: &str = "Rest the pointer over [@alex](mention:alex), inspect [issue #867](https://github.com/anomalyco/rune/issues/867), or hover [a longer wrapped link that continues onto a second line inside the column](https://rune.dev/docs/links) to anchor a URL preview card to the hit line box.";

fn preview_copy(url: &str) -> (&'static str, &'static str) {
    match url {
        "mention:alex" => ("@alex", "Maintainer · Online — hover card anchored to inline mention"),
        "https://github.com/anomalyco/rune/issues/867" => ("Issue #867", "Markdown link hover callback with hovered URL and rect"),
        "https://rune.dev/docs/links" => ("Wrapped link", "Anchored to whichever line box of the wrapped link the pointer entered"),
        _ => ("Link preview", "Hovered inline link"),
    }
}

pub(super) fn markdown_link_hover(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    if let Some((url, rect)) = &s.link_hover {
        let (title, summary) = preview_copy(url);
        let coords = format!(
            "rect: [{:.1}, {:.1}, {:.1}, {:.1}]",
            rect[0], rect[1], rect[2], rect[3]
        );
        layer.push(
            el().key("link-hover-anchor")
                .absolute(rect[0], rect[1])
                .size(rect[2], rect[3])
                .a11y_hidden(),
        );
        let card = popover::raised(
            k,
            col()
                .w(260.0)
                .padding(12.0)
                .gap(4.0)
                .child(text(title).font(k.ty.ui_medium).color(k.c.text_strong))
                .child(text(summary).font(k.ty.small).color(k.c.text_base).wrap())
                .child(text(url.clone()).key("link-hover-url").font(k.ty.small.mono()).color(k.c.text_weak).truncate())
                .child(text(coords).key("link-hover-coords").font(k.ty.small.mono()).color(k.c.text_weak)),
            k.radius.md,
        )
        .key("link-hover-card")
        .hover_passes_below();
        layer.push(popover::anchored(
            k,
            card,
            "link-hover-anchor",
            Placement::BottomStart,
            6.0,
        ));
    }

    let status = match &s.link_hover {
        Some((url, r)) => format!("Hover: {url} at [{:.0}, {:.0}, {:.0}, {:.0}]", r[0], r[1], r[2], r[3]),
        None => "Move the pointer over any link to preview its URL and rect.".to_owned(),
    };

    col()
        .w(380.0)
        .gap(12.0)
        .child(text("Link hover preview (Markdown::on_link_hover)").font(k.ty.ui_medium).color(k.c.text_strong))
        .child(
            Markdown::new(LINK_HOVER_MD)
                .selectable(true)
                .on_link_hover(|s: &mut G, url, rect, tx| {
                    if tx.pressing() {
                        s.link_hover = None;
                        return;
                    }
                    s.link_hover = url.zip(rect).map(|(u, r)| (u.to_owned(), r));
                })
                .build(k),
        )
        .child(text(status).key("link-hover-status").font(k.ty.small).color(k.c.text_weak))
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use crate::{init, stories, view};

    fn settle(rt: &mut Runtime<crate::G>) {
        for _ in 0..30 {
            if !rt.frame(1.0 / 60.0) {
                return;
            }
        }
    }

    #[test]
    fn link_hover_story_anchors_preview_card_on_hover_and_clears_on_leave() {
        let mut g = init();
        let (_, setup) = stories::SCENES
            .iter()
            .find(|(id, _)| *id == "rune-markdown--link-hover")
            .expect("link-hover scene");
        setup(&mut g);

        let mut rt = Runtime::new(g, view, (640.0, 360.0), Env::new(true, 1.0));
        rt.frame(0.0);
        settle(&mut rt);

        let alex = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("mention:alex"))
            .expect("@alex link");
        rt.pointer_move(alex.rect[0] + alex.rect[2] * 0.5, alex.rect[1] + alex.rect[3] * 0.5);
        settle(&mut rt);

        let (url, rect) = rt.state.link_hover.as_ref().expect("hover preview open");
        assert_eq!(url, "mention:alex");
        assert!((rect[0] - alex.rect[0]).abs() < 0.5 && (rect[1] - alex.rect[1]).abs() < 0.5);
        let card = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("link-hover-card"))
            .expect("preview card");
        assert!((card.rect[1] - (rect[1] + rect[3] + 6.0)).abs() < 1.5);

        rt.pointer_leave();
        settle(&mut rt);
        assert!(rt.state.link_hover.is_none());
    }
}
