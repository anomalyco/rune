//! What a frame reports, kept readable until the next one.

/// Items a frame reports (settle events, unmounted nodes). [`begin`](Self::begin) drops what the
/// previous frame reported and [`end`](Self::end) marks what this one did, so an item pushed
/// between frames is reported by the next one, and every item by exactly one.
#[derive(Clone, Debug)]
pub(crate) struct Carry<T> {
    items: Vec<T>,
    reported: usize,
}

impl<T> Carry<T> {
    pub(crate) const fn new() -> Self {
        Carry { items: Vec::new(), reported: 0 }
    }

    pub(crate) fn reserve(&mut self, n: usize) {
        self.items.reserve(n);
    }

    /// Starts a frame: drops what the last one reported, keeps what came after it.
    pub(crate) fn begin(&mut self) {
        self.items.drain(..self.reported);
        self.reported = 0;
    }

    /// Ends a frame: everything so far is what it reports.
    pub(crate) fn end(&mut self) {
        self.reported = self.items.len();
    }

    pub(crate) fn push(&mut self, item: T) {
        self.items.push(item);
    }

    /// What the last frame reported, and anything pushed since.
    pub(crate) fn as_slice(&self) -> &[T] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::Carry;

    #[test]
    fn each_item_is_reported_by_exactly_one_frame() {
        let mut c = Carry::new();
        c.push(1);
        c.begin();
        c.push(2);
        c.end();
        assert_eq!(c.as_slice(), [1, 2], "pushed before or during a frame");
        c.push(3);
        assert_eq!(c.as_slice(), [1, 2, 3], "readable until the next frame");
        c.begin();
        assert_eq!(c.as_slice(), [3], "pushed between frames: the next one reports it");
        c.end();
        c.begin();
        c.end();
        assert!(c.as_slice().is_empty());
    }
}
