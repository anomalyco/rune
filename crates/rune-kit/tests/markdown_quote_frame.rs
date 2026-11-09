//! `Markdown::quote_frame`: each block quote comes to the app with its text as a selection of
//! all of it copies, so a Copy button on the quote gives what Cmd+C would.
#![cfg(feature = "markdown")]

use std::cell::RefCell;
use std::rc::Rc;

use rune::col;
use rune_kit::{Kit, Markdown};

fn quotes(src: &str) -> Vec<String> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let saw = seen.clone();
    let _ = col().child(
        Markdown::new(src)
            .quote_frame(move |quote, q| {
                saw.borrow_mut().push(q.text.to_owned());
                quote
            })
            .build(&Kit::neutral(true)),
    );
    seen.take()
}

#[test]
fn a_quote_frame_gets_the_quote_as_plain_text() {
    let src = "Before.\n\n> First `para` here.\n>\n> Second para.\n>\n> - one\n> - [x] two\n>   - nested\n> 3. third\n>\n> ```\n> let a = 1;\n> ```\n\nAfter.\n";
    assert_eq!(quotes(src), ["First para here.\n\nSecond para.\n\n- one\n- [x] two\n  - nested\n\n3. third\n\nlet a = 1;"]);
}

#[test]
fn a_quote_in_a_quote_comes_too() {
    assert_eq!(quotes("> outer\n>\n> > inner\n"), ["inner", "outer\n\ninner"]);
}
