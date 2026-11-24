//! Explicit state machine for the bounded oversized (> `KEEP_CHUNK_MAX`) staging chunk pool.
//!
//! One-shot large uploads (such as decoding a single big image) are unmapped and dropped on
//! encode without remapping, retaining 0 bytes. Only recurring oversized uploads (within
//! `OVERSIZED_IDLE_FLUSHES` flushes and up to `OVERSIZED_KEEP_BYTES`) enter the pool, which is
//! bounded to `OVERSIZED_KEEP_CHUNKS` entries / `OVERSIZED_KEEP_BYTES` total bytes and evicted
//! after `OVERSIZED_IDLE_FLUSHES` consecutive flushes without an oversized write or on trim.

/// Allocation granularity for oversized staging chunks (64 KiB, avoiding power-of-two bloat).
pub(crate) const OVERSIZED_ALIGN: u64 = 64 << 10;
/// Maximum retained oversized chunks (covers `IN_FLIGHT = 2` pipelining).
pub(crate) const OVERSIZED_KEEP_CHUNKS: usize = 2;
/// Maximum total bytes retained across oversized free chunks (32 MiB).
pub(crate) const OVERSIZED_KEEP_BYTES: u64 = 32 << 20;
/// Consecutive flushes without an oversized write before evicting the oversized pool.
pub(crate) const OVERSIZED_IDLE_FLUSHES: u32 = 60;

/// Lifecycle state of the oversized staging pool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// No oversized write seen recently; the pool holds nothing.
    #[default]
    Cold,
    /// One poolable oversized write was seen (sent as one-shot); counting non-oversized flushes.
    SeenOnce { idle_flushes: u32 },
    /// Recurring oversized writes are active and chunks are recycled into `oversized_free`.
    Pooled,
    /// Pool is armed/populated, counting consecutive flushes without an oversized write.
    IdleCounting { idle_flushes: u32 },
}

impl State {
    /// Whether the pool is tracking idle flushes or holding oversized chunks.
    pub(crate) fn is_active(self) -> bool {
        !matches!(self, Self::Cold)
    }

    /// Whether returning oversized chunks may enter `oversized_free`.
    pub(crate) fn accepts_returns(self) -> bool {
        matches!(self, Self::Pooled | Self::IdleCounting { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// An oversized write (`size > KEEP_CHUNK_MAX`) is being staged.
    /// `poolable` is true iff its aligned allocation fits within `OVERSIZED_KEEP_BYTES`.
    OversizedWrite { poolable: bool },
    /// A flush/submit completed; `had_oversized` is true iff it encoded an oversized chunk.
    Flush { had_oversized: bool },
    /// Explicit trim (`Staging::trim` or `Staging::trim_oversized`, e.g. at window rest).
    Trim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Unmap and drop on encode without remapping (zero retention).
    OneShot,
    /// Remap on submit and return to `oversized_free` within bounds.
    Recycle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// How to handle this oversized chunk in `stage`/`encode`.
    Chunk(Disposition),
    /// Drop all retained oversized chunks.
    Evict,
}

/// Exact 64 KiB-aligned allocation size for an oversized write.
pub(crate) fn alloc_size(size: u64) -> u64 {
    size.next_multiple_of(OVERSIZED_ALIGN)
}

/// Whether an oversized free chunk of `chunk_size` bytes can be reused for a write of
/// `write_size` bytes without pinning more than 2x slack.
pub(crate) fn fits_reusable(chunk_size: u64, write_size: u64) -> bool {
    chunk_size >= write_size && chunk_size <= write_size.saturating_mul(2)
}

/// Pure transition function for the oversized staging pool.
pub(crate) fn step(state: &mut State, event: Event) -> Option<Effect> {
    match (*state, event) {
        (_, Event::OversizedWrite { poolable: false }) => Some(Effect::Chunk(Disposition::OneShot)),
        (State::Cold, Event::OversizedWrite { poolable: true }) => {
            *state = State::SeenOnce { idle_flushes: 0 };
            Some(Effect::Chunk(Disposition::OneShot))
        }
        (State::SeenOnce { .. } | State::Pooled | State::IdleCounting { .. }, Event::OversizedWrite { poolable: true }) => {
            *state = State::Pooled;
            Some(Effect::Chunk(Disposition::Recycle))
        }
        (State::Cold, Event::Flush { .. }) => None,
        (State::SeenOnce { .. }, Event::Flush { had_oversized: true }) => {
            *state = State::SeenOnce { idle_flushes: 0 };
            None
        }
        (State::Pooled | State::IdleCounting { .. }, Event::Flush { had_oversized: true }) => {
            *state = State::Pooled;
            None
        }
        (State::SeenOnce { idle_flushes }, Event::Flush { had_oversized: false }) => {
            let next = idle_flushes + 1;
            if next >= OVERSIZED_IDLE_FLUSHES {
                *state = State::Cold;
            } else {
                *state = State::SeenOnce { idle_flushes: next };
            }
            None
        }
        (State::Pooled, Event::Flush { had_oversized: false }) => {
            *state = State::IdleCounting { idle_flushes: 1 };
            None
        }
        (State::IdleCounting { idle_flushes }, Event::Flush { had_oversized: false }) => {
            let next = idle_flushes + 1;
            if next >= OVERSIZED_IDLE_FLUSHES {
                *state = State::Cold;
                Some(Effect::Evict)
            } else {
                *state = State::IdleCounting { idle_flushes: next };
                None
            }
        }
        (State::Cold, Event::Trim) => None,
        (State::SeenOnce { .. } | State::Pooled | State::IdleCounting { .. }, Event::Trim) => {
            *state = State::Cold;
            Some(Effect::Evict)
        }
    }
}

#[cfg(test)]
#[path = "oversized_tests.rs"]
mod tests;
