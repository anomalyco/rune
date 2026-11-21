//! The lenient statement rule the block diagrams (class, ER, state) share: a statement that
//! doesn't read is undone and skipped, along with any `{ … }` body it opened, and one past the
//! size bounds leaves the whole diagram as source.

/// A diagram parser the [`Reader`] drives.
pub(crate) trait Lenient {
    /// How much of the diagram is built, to undo a statement that doesn't read back to.
    type Mark;
    fn mark(&self) -> Self::Mark;
    fn undo(&mut self, mark: Self::Mark);
    /// Reads one statement; `None` when it doesn't read.
    fn read_statement(&mut self, s: &str) -> Option<()>;
    /// Past the size bounds.
    fn over(&self) -> bool;
}

/// Feeds statements to a [`Lenient`] parser.
#[derive(Debug, Default)]
pub(crate) struct Reader {
    /// How many `{ … }` bodies deep the skip is: one opened by a statement that didn't read,
    /// and any opened inside it. 0 while reading.
    skipping: usize,
}

impl Reader {
    /// Reads `s` into `p`. `None` once `p` is past its bounds, so the diagram stays source.
    pub(crate) fn read<P: Lenient>(&mut self, p: &mut P, s: &str) -> Option<()> {
        if self.skipping > 0 {
            if s == "}" {
                self.skipping -= 1;
            } else if s.ends_with('{') {
                self.skipping += 1;
            }
            return Some(());
        }
        let mark = p.mark();
        if p.read_statement(s).is_none() {
            if p.over() {
                return None;
            }
            p.undo(mark);
            if s.ends_with('{') {
                self.skipping = 1;
            }
        }
        Some(())
    }
}

#[cfg(test)]
#[path = "lenient_tests.rs"]
mod tests;
