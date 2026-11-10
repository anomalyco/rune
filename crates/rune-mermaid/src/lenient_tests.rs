use super::{Lenient, Reader};

/// Keeps statements that are words; anything else doesn't read, after adding a junk item.
#[derive(Default)]
struct Words {
    items: Vec<String>,
    limit: usize,
}

impl Lenient for Words {
    type Mark = usize;
    fn mark(&self) -> usize {
        self.items.len()
    }
    fn undo(&mut self, mark: usize) {
        self.items.truncate(mark);
    }
    fn read_statement(&mut self, s: &str) -> Option<()> {
        self.items.push(s.to_owned());
        (s.chars().all(|c| c.is_alphanumeric() || c == '{' || c == '}' || c == ' ') && !s.starts_with('!')).then_some(())
    }
    fn over(&self) -> bool {
        self.limit > 0 && self.items.len() > self.limit
    }
}

fn read(src: &[&str], limit: usize) -> Option<Vec<String>> {
    let (mut r, mut p) = (Reader::default(), Words { limit, ..Words::default() });
    for s in src {
        r.read(&mut p, s)?;
    }
    Some(p.items)
}

#[test]
fn an_unreadable_statement_is_undone_with_its_body_and_the_bodies_inside_it() {
    let src = ["a", "!bad {", "inner {", "x", "}", "y", "}", "b", "!also bad", "c"];
    assert_eq!(read(&src, 0).unwrap(), ["a", "b", "c"]);
}

#[test]
fn past_the_bounds_the_whole_diagram_stays_source() {
    assert!(read(&["a", "b", "!c", "d"], 2).is_none());
}
