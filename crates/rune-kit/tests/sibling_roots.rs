//! Component roots with fixed inner keys can sit side by side without sibling-key repairs.
#![cfg(feature = "markdown")]

use rune::{Cx, El, Env, Runtime, col};
use rune_kit::{Card, Kit, Markdown, MarkdownStyle};

struct Page;

fn cards(_: &Page, _: &mut Cx<Page>) -> El {
    let k = Kit::neutral(true);
    col().w_full().child(Card::new().title("One", None).description("a").build(&k)).child(Card::new().title("Two", None).description("b").build(&k))
}

fn blocks(_: &Page, _: &mut Cx<Page>) -> El {
    let style = MarkdownStyle::neutral(true);
    col().w_full().child(Markdown::new("# One\n\nbody").render(&style)).child(Markdown::new("# Two\n\nbody").render(&style))
}

fn repaired(view: fn(&Page, &mut Cx<Page>) -> El) -> Vec<String> {
    let mut rt = Runtime::new(Page, view, (600.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let all = rt.elements();
    let mut paths: Vec<_> = all.iter().map(|e| e.path.clone()).collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), all.len(), "element paths are unique");
    all.into_iter().filter_map(|e| e.key).filter(|k| k.contains("#dup")).collect()
}

#[test]
fn sibling_cards_have_no_duplicate_keys() {
    assert_eq!(repaired(cards), Vec::<String>::new());
}

#[test]
fn sibling_markdown_blocks_have_no_duplicate_keys() {
    assert_eq!(repaired(blocks), Vec::<String>::new());
}
