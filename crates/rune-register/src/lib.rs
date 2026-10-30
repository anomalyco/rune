//! Lists a feature adds itself to from its own file, so new scenes, stories and knob groups don't
//! all edit one central list. A list is a distributed slice of [`Part`]s: declare it once with
//! [`slice!`], add to it from any module with [`register!`], and read it with [`collect`], which
//! orders parts by module path so the result doesn't depend on link order.
//!
//! ```
//! use rune_register::{collect, register, slice};
//!
//! #[derive(Clone, Copy)]
//! pub struct Scene(&'static str);
//!
//! slice!(pub SCENES: Scene);
//!
//! mod chat {
//!     rune_register::register!(super::SCENES: super::Scene = [super::Scene("chat-1")]);
//! }
//!
//! # #[cfg(not(target_family = "wasm"))]
//! fn main() {
//!     let names: Vec<&str> = collect(&SCENES).iter().map(|s| s.0).collect();
//!     assert_eq!(names, ["chat-1"]);
//! }
//! # #[cfg(target_family = "wasm")]
//! # fn main() {}
//! ```
//!
//! # Targets
//!
//! Registration uses [linkme](https://docs.rs/linkme), which collects statics in a linker section.
//! That works on Linux, macOS, iOS, tvOS, Android, Windows, FreeBSD and a few others (this crate
//! was checked for `aarch64-apple-ios`). It does not work on `wasm32`: linkme has no wasm support
//! and fails there with "distributed_slice is not implemented for this platform".
//!
//! So on `target_family = "wasm"` the macros fall back to an empty list: [`slice!`] declares a
//! zero-length slice, and [`register!`] still type-checks its items but adds them nowhere, so
//! [`collect`] returns nothing. An app on the web that needs its lists must build them another
//! way there. Any other target linkme lacks stops the build with linkme's compile error rather
//! than silently returning an empty list.
//!
//! A part is only collected if its crate is linked in, so a crate that only registers and is
//! never referenced must be used by name, for example `use my_crate as _;`.

#[doc(hidden)]
pub use linkme;

/// One module's contribution to a list.
pub struct Part<T: 'static> {
    pub module: &'static str,
    pub items: &'static [T],
}

/// Every part's items: parts by module path, each part's items in its own order.
pub fn collect<T: Copy>(parts: &[Part<T>]) -> Vec<T> {
    let mut parts: Vec<&Part<T>> = parts.iter().collect();
    parts.sort_by_key(|p| p.module);
    parts.iter().flat_map(|p| p.items.iter().copied()).collect()
}

/// `own` with `more` folded in by name: an item named like one of `own` joins it, any other is new
/// and goes before `own`'s last item (the list's footer).
pub fn merge<T: Copy>(
    own: &[T],
    more: &[T],
    name: impl Fn(&T) -> &'static str,
    join: impl Fn(&mut T, &T),
) -> Vec<T> {
    let (last, own) = own.split_last().map_or((None, own), |(l, rest)| (Some(*l), rest));
    let mut out: Vec<T> = own.to_vec();
    for m in more {
        match out.iter_mut().find(|o| name(o) == name(m)) {
            Some(o) => join(o, m),
            None => out.push(*m),
        }
    }
    out.extend(last);
    out
}

/// Declares a list: `slice!(pub SCENES: Scene);` makes `SCENES` a slice of `Part<Scene>`.
#[macro_export]
macro_rules! slice {
    ($vis:vis $list:ident : $ty:ty) => {
        #[cfg(not(target_family = "wasm"))]
        #[$crate::linkme::distributed_slice]
        #[linkme(crate = $crate::linkme)]
        $vis static $list: [$crate::Part<$ty>];

        #[cfg(target_family = "wasm")]
        $vis static $list: [$crate::Part<$ty>; 0] = [];
    };
}

/// Adds this module's items to a list: `register!(LIST: Type = [item, …]);`.
#[macro_export]
macro_rules! register {
    ($list:path : $ty:ty = [$($item:expr),* $(,)?]) => {
        const _: () = {
            #[cfg_attr(not(target_family = "wasm"), $crate::linkme::distributed_slice($list))]
            #[cfg_attr(not(target_family = "wasm"), linkme(crate = $crate::linkme))]
            #[cfg_attr(target_family = "wasm", allow(dead_code))]
            static PART: $crate::Part<$ty> = $crate::Part {
                module: module_path!(),
                items: &[$($item),*],
            };
        };
    };
}

#[cfg(test)]
mod tests;
