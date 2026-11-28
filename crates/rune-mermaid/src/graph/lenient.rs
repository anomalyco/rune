//! The lenient statement rule the block diagrams (class, ER, state) share: a statement that
//! doesn't read is undone and skipped, along with any `{ … }` body it opened, and one past the
//! size bounds leaves the whole diagram as source.

use crate::error::{ParseFail, Skipped};

/// A diagram parser the [`Reader`] drives.
pub(crate) trait Lenient {
    /// How much of the diagram is built, to undo a statement that doesn't read back to.
    type Mark;
    fn mark(&self) -> Self::Mark;
    fn undo(&mut self, mark: Self::Mark);
    /// Reads one statement; `None` when it doesn't read.
    fn read_statement(&mut self, s: &str) -> Option<()>;
    /// Past the size or nesting bounds.
    fn limit(&self) -> Option<ParseFail>;
}

/// Feeds statements to a [`Lenient`] parser.
#[derive(Debug, Default)]
pub(crate) struct Reader {
    /// How many `{ … }` bodies deep the skip is: one opened by a statement that didn't read,
    /// and any opened inside it. 0 while reading.
    skipping: usize,
    skipped: Skipped,
}

impl Reader {
    /// Reads `s` (from 1-based `line`) into `p`. `Err` once `p` is past its bounds, so the
    /// diagram stays source.
    pub(crate) fn read<P: Lenient>(&mut self, p: &mut P, line: usize, s: &str) -> Result<(), ParseFail> {
        if self.skipping > 0 {
            if s == "}" {
                self.skipping -= 1;
            } else if s.ends_with('{') {
                self.skipping += 1;
            }
            return Ok(());
        }
        let mark = p.mark();
        if p.read_statement(s).is_none() {
            if let Some(fail) = p.limit() {
                return Err(fail);
            }
            p.undo(mark);
            self.skipped.record(line, s);
            if s.ends_with('{') {
                self.skipping = 1;
            }
        }
        Ok(())
    }

    pub(crate) fn into_no_items(self) -> ParseFail {
        self.skipped.into_no_items()
    }
}

#[cfg(test)]
#[path = "lenient_tests.rs"]
mod tests;
