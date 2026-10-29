//! Bind groups kept across frames, keyed on the resources they bind.

/// This frame's bind groups, indexed by `Prepared::bind`, plus last frame's for reuse.
/// A frame keeps at most its own groups and the previous frame's, so the set stays bounded.
pub(crate) struct BindSet<K> {
    now: Vec<(K, wgpu::BindGroup)>,
    last: Vec<(K, wgpu::BindGroup)>,
}

impl<K> BindSet<K> {
    pub(crate) const fn new() -> Self {
        BindSet { now: Vec::new(), last: Vec::new() }
    }

    /// Start a frame: last frame's groups become reusable, the older ones are dropped.
    pub(crate) fn begin(&mut self) {
        self.last.clear();
        std::mem::swap(&mut self.now, &mut self.last);
    }

    /// The index of a group whose key passes `is`, reusing this or last frame's before calling `make`.
    pub(crate) fn find(&mut self, is: impl Fn(&K) -> bool, make: impl FnOnce() -> (K, wgpu::BindGroup)) -> usize {
        if let Some(i) = self.now.iter().position(|(k, _)| is(k)) {
            return i;
        }
        let entry = match self.last.iter().position(|(k, _)| is(k)) {
            Some(i) => self.last.swap_remove(i),
            None => make(),
        };
        self.now.push(entry);
        self.now.len() - 1
    }

    pub(crate) fn get(&self, i: usize) -> &wgpu::BindGroup {
        &self.now[i].1
    }
}
