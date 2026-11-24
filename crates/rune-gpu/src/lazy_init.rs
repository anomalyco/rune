//! Per-(mip, layer) lazy zero-initialization state machine for Private render-target textures.
//!
//! A newly created Private `RENDER_ATTACHMENT` texture starts with every `(mip, layer)` marked
//! uninitialized instead of queuing an eager `zero_texture` blit. The state machine decides
//! whether an upcoming render-pass attachment, shader sample, copy, or partial write needs a
//! load-action promotion or a clear-only render pass, and transitions to `State::Initialized`
//! once every subresource has been initialized.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rune_core::machine::Effects;

/// Subresource dimensions of a 2D or 2D-array texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    pub mips: u32,
    pub layers: u32,
}

impl Shape {
    pub(crate) fn new(mips: u32, layers: u32) -> Self {
        Self { mips: mips.max(1), layers: layers.max(1) }
    }

    pub(crate) fn count(self) -> usize {
        self.mips as usize * self.layers as usize
    }

    fn index(self, sub: Subresource) -> Option<usize> {
        if sub.mip < self.mips && sub.layer < self.layers {
            Some(sub.layer as usize * self.mips as usize + sub.mip as usize)
        } else {
            None
        }
    }
}

/// One `(mip, layer)` subresource of a texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Subresource {
    pub mip: u32,
    pub layer: u32,
}

/// A rectangular range of mip levels and array layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubresourceRange {
    pub base_mip: u32,
    pub mips: u32,
    pub base_layer: u32,
    pub layers: u32,
}

impl SubresourceRange {
    pub(crate) fn single(mip: u32, layer: u32) -> Self {
        Self { base_mip: mip, mips: 1, base_layer: layer, layers: 1 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadKind {
    Clear,
    Load,
    DontCare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreKind {
    Store,
    Discard,
}

/// Bitset of uninitialized subresources (bit = 1 means uninitialized).
/// Uses an inline `u64` up to 64 subresources and spills to a `Vec<u64>` beyond 64.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UninitBits {
    Inline(u64),
    Heap(Vec<u64>),
}

impl UninitBits {
    fn all_set(count: usize) -> Self {
        if count <= 64 {
            let mask = if count == 64 { u64::MAX } else { (1u64 << count) - 1 };
            Self::Inline(mask)
        } else {
            let words = count.div_ceil(64);
            let mut bits = vec![u64::MAX; words];
            let rem = count % 64;
            if rem != 0 {
                bits[words - 1] = (1u64 << rem) - 1;
            }
            Self::Heap(bits)
        }
    }

    fn get(&self, index: usize) -> bool {
        match self {
            Self::Inline(mask) => index < 64 && ((*mask >> index) & 1) != 0,
            Self::Heap(words) => words.get(index / 64).is_some_and(|w| ((*w >> (index % 64)) & 1) != 0),
        }
    }

    fn clear(&mut self, index: usize) {
        match self {
            Self::Inline(mask) if index < 64 => *mask &= !(1u64 << index),
            Self::Inline(_) => {}
            Self::Heap(words) => {
                if let Some(w) = words.get_mut(index / 64) {
                    *w &= !(1u64 << (index % 64));
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Inline(mask) => *mask == 0,
            Self::Heap(words) => words.iter().all(|&w| w == 0),
        }
    }
}

/// Per-texture lazy-initialization state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Every `(mip, layer)` subresource is initialized.
    Initialized,
    /// At least one subresource is still uninitialized.
    Uninitialized { shape: Shape, uninit: UninitBits },
}

impl State {
    pub(crate) fn new(shape: Shape) -> Self {
        let count = shape.count();
        if count == 0 {
            Self::Initialized
        } else {
            Self::Uninitialized { shape, uninit: UninitBits::all_set(count) }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_uninit(&self, sub: Subresource) -> bool {
        match self {
            Self::Initialized => false,
            Self::Uninitialized { shape, uninit } => shape.index(sub).is_some_and(|i| uninit.get(i)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Render pass attaches `sub` as a color attachment with `load` and `store`.
    Attach { sub: Subresource, load: LoadKind, store: StoreKind },
    /// Sampling, texture-to-buffer readback, or texture-to-texture copy source over `range`.
    Read(SubresourceRange),
    /// Full-mip write overwriting every texel of every subresource in `range`.
    WriteFull(SubresourceRange),
    /// Partial write covering a strict subset of texels in `range`.
    WritePartial(SubresourceRange),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Promote this color attachment's load action to `MTLLoadAction::Clear(0, 0, 0, 0)`.
    PromoteLoadToClear,
    /// Run a clear-only render pass (`Clear(0, 0, 0, 0)`, `Store`) on `sub` before this op.
    ClearSubresource(Subresource),
}

/// Pure transition function for lazy texture initialization.
pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let State::Uninitialized { shape, uninit } = state else {
        return Effects::none();
    };
    let shape = *shape;
    let mut effects = Effects::none();
    match event {
        Event::Attach { sub, load, store } => {
            if let Some(idx) = shape.index(sub) && uninit.get(idx) {
                if load != LoadKind::Clear {
                    effects.push(Effect::PromoteLoadToClear);
                }
                if store == StoreKind::Store {
                    uninit.clear(idx);
                }
            }
        }
        Event::Read(range) | Event::WritePartial(range) => {
            let end_layer = range.base_layer.saturating_add(range.layers).min(shape.layers);
            let end_mip = range.base_mip.saturating_add(range.mips).min(shape.mips);
            for layer in range.base_layer..end_layer {
                for mip in range.base_mip..end_mip {
                    let sub = Subresource { mip, layer };
                    if let Some(idx) = shape.index(sub) && uninit.get(idx) {
                        uninit.clear(idx);
                        effects.push(Effect::ClearSubresource(sub));
                    }
                }
            }
        }
        Event::WriteFull(range) => {
            let end_layer = range.base_layer.saturating_add(range.layers).min(shape.layers);
            let end_mip = range.base_mip.saturating_add(range.mips).min(shape.mips);
            for layer in range.base_layer..end_layer {
                for mip in range.base_mip..end_mip {
                    if let Some(idx) = shape.index(Subresource { mip, layer }) {
                        uninit.clear(idx);
                    }
                }
            }
        }
    }
    if uninit.is_empty() {
        *state = State::Initialized;
    }
    effects
}

/// Thread-safe lazy-initialization tracker with a lock-free fast path once fully initialized.
#[derive(Debug)]
pub(crate) struct LazyInit {
    any_uninit: AtomicBool,
    state: Mutex<State>,
}

impl LazyInit {
    pub(crate) fn new(mips: u32, layers: u32) -> Self {
        let state = State::new(Shape::new(mips, layers));
        let any_uninit = AtomicBool::new(!matches!(state, State::Initialized));
        Self { any_uninit, state: Mutex::new(state) }
    }

    pub(crate) fn has_uninit(&self) -> bool {
        self.any_uninit.load(Ordering::Acquire)
    }

    pub(crate) fn step(&self, event: Event) -> Effects<Effect> {
        if !self.has_uninit() {
            return Effects::none();
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let effects = step(&mut state, event);
        if matches!(*state, State::Initialized) {
            self.any_uninit.store(false, Ordering::Release);
        }
        effects
    }
}

#[cfg(test)]
#[path = "lazy_init_tests.rs"]
mod tests;
