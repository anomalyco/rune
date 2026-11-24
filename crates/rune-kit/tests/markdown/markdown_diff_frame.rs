//! Public API tests for `rune_kit::DiffFrame`, `Markdown::diff_frame`, and `Markdown::wrap_anywhere`.

use std::cell::RefCell;
use std::rc::Rc;

use rune::{Cx, El, Env, Runtime, col, text};
use rune_kit::{DiffFrame, Kit, Markdown};

#[test]
fn diff_frame_public_api_receives_index_source_and_open_state() {
    let seen = Rc::new(RefCell::new(Vec::<(usize, String, bool)>::new()));
    let log = seen.clone();
    let doc = "```diff\n-old\n+new\n```\n\n```rust\nlet a = 1;\n```\n\n```patch\n-x\n+y";
    let _ = col().child(
        Markdown::new(doc)
            .streaming(true)
            .diff_frame(move |card, frame: &DiffFrame<'_>| {
                log.borrow_mut().push((frame.index, frame.source.to_owned(), frame.open));
                col().key(format!("diff-{}", frame.index)).child(text("Patch")).child(card)
            })
            .build(&Kit::neutral(true)),
    );
    assert_eq!(
        seen.take(),
        vec![
            (0, "-old\n+new".to_owned(), false),
            (1, "-x\n+y".to_owned(), true),
        ]
    );
}

#[test]
fn wrap_anywhere_public_api_wraps_long_unbroken_token() {
    let token = "https://example.org/".to_owned() + &"segment".repeat(20);
    let view = |src: &String, _: &mut Cx<String>| -> El {
        col().w_full().child(col().w(240.0).child(Markdown::new(src.clone()).wrap_anywhere(true).build(&Kit::neutral(true)).key("md")))
    };
    let mut rt = Runtime::new(token, view, (480.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let b0 = rt.elements().into_iter().find(|e| e.path.ends_with("md/b0")).unwrap();
    assert!(b0.rect[3] > 40.0, "expected wrapped paragraph height > 40 pt, got {:?}", b0.rect);
}
