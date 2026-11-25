//! Side-by-side comparison of default CommonMark soft breaks vs `Markdown::hard_breaks(true)`
//! for user-typed chat messages (`rune-markdown--hard-breaks`).

use super::*;

const CHAT_SAMPLE: &str = concat!(
    "First line of a chat message\n",
    "Second line typed with Shift+Return\n",
    "Third line with *italic\nacross lines* and [a link\nlabel](https://rune.dev)\n\n",
    "- List item line one\n",
    "  List item line two\n\n",
    "> Quoted line one\n",
    "> Quoted line two\n\n",
    "Inline `code\nspan` stays on one line, and fences keep their lines:\n\n",
    "```rust\nlet a = 1;\nlet b = 2;\n```\n",
);

pub(super) fn markdown_hard_breaks(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let panel = |key: &str, label: &str, hard_breaks: bool| {
        col()
            .key(key)
            .w(280.0)
            .gap(6.0)
            .child(caption(k, label))
            .child(
                col()
                    .w_full()
                    .padding(12.0)
                    .radius(k.radius.md)
                    .fill(c.layer_01)
                    .border(1.0, c.border_weak)
                    .child(
                        Markdown::new(CHAT_SAMPLE)
                            .hard_breaks(hard_breaks)
                            .selectable(true)
                            .copy_source(true)
                            .build(k)
                            .key("md"),
                    ),
            )
    };
    row()
        .gap(16.0)
        .child(panel("hb-default", "Default (CommonMark soft breaks)", false))
        .child(panel("hb-on", "hard_breaks(true) (chat Shift+Return)", true))
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use crate::{init, stories, view};

    #[test]
    fn hard_breaks_story_renders_taller_with_hard_breaks_on() {
        let mut g = init();
        let (_, setup) = stories::SCENES
            .iter()
            .find(|(id, _)| *id == "rune-markdown--hard-breaks")
            .expect("hard-breaks scene");
        setup(&mut g);

        let mut rt = Runtime::new(g, view, (680.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.0);

        let default_p = rt
            .elements()
            .into_iter()
            .find(|e| e.path.ends_with("hb-default/#1/md/b0"))
            .expect("default paragraph");
        let hard_p = rt
            .elements()
            .into_iter()
            .find(|e| e.path.ends_with("hb-on/#1/md/b0"))
            .expect("hard_breaks paragraph");
        assert!(
            hard_p.rect[3] > default_p.rect[3] * 1.4,
            "hard_breaks(true) paragraph should be taller: default={:?} hard={:?}",
            default_p.rect,
            hard_p.rect,
        );
    }
}
