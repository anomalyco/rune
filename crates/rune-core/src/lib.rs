//! Foundation primitives for Rune: colour, design tokens, themes, vector shapes, state
//! machines, clocks, off-UI tasks, tracked state reads, growth curves, live values, latest-wins
//! background requests, launch marks, app directories, and preferences, with no GPU or windowing
//! dependencies. Apps normally reach these modules and types through `rune`, which re-exports them.

#[doc(hidden)]
pub mod cold;
pub mod color;
pub mod dirs;
pub mod env;
pub mod growth;
pub mod latest;
pub mod live;
pub mod machine;
#[cfg(target_vendor = "apple")]
#[doc(hidden)]
pub mod main_queue;
pub mod marks;
pub mod prefs;
pub mod shape;
pub mod task;
pub mod theme;
pub mod time;
pub mod tok;
pub mod tracked;

pub use color::Color;
pub use color::oklch;
pub use color::oklcha;
pub use growth::GROWTH;
pub use growth::StreamEdge;
pub use growth::edge;
pub use latest::Latest;
pub use live::Live;
pub use live::LiveText;
pub use live::Tween;
pub use shape::FillRule;
pub use shape::LineCap;
pub use shape::Path;
pub use shape::Shape;
pub use theme::Theme;
pub use time::LocalTime;
pub use tok::Family;
pub use tok::OpticalSize;
pub use tok::TextStyle;
pub use tracked::Reads;
pub use tracked::Tracked;
pub use tracked::tracked_reads;

/// Wakes the shell from another thread (new data arrived); a no-op headless.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Fast non-cryptographic hash map (`FxHashMap`) for runtime node ids and paths.
pub type HashMap<K, V> = rustc_hash::FxHashMap<K, V>;

/// Fast non-cryptographic hash set (`FxHashSet`) for runtime node ids and paths.
pub type HashSet<T> = rustc_hash::FxHashSet<T>;

/// How a scope combines its contents. The completed scope still uses source-over on its parent
/// unless that parent also requests additive composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    SourceOver,
    /// Premultiplied One/One addition, clamped when the isolated scope is composited.
    PlusLighter,
}
