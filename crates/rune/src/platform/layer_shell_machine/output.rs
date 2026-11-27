//! Which Wayland `wl_output` global a layer surface belongs to.
//!
//! A surface created with an explicit `output_id` stays pinned to that global name. One created
//! with no output lets the compositor place it and tracks `wl_surface.enter` / `wl_surface.leave`
//! so it knows which output it is on now—or left most recently, when the compositor sends `leave`
//! right before unplugging the output.

/// Output ownership for a Wayland layer surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceOutput {
    /// Created for a specific `wl_output` global name.
    Pinned(u32),
    /// Created without an output: follows `wl_surface.enter` / `wl_surface.leave`.
    Follow {
        /// Outputs the surface is currently entered on, most recent last.
        on: Vec<u32>,
        /// Output most recently left, used if `on` is empty when `global_remove` arrives.
        last_left: Option<u32>,
    },
}

impl Default for SurfaceOutput {
    fn default() -> Self {
        Self::Follow { on: Vec::new(), last_left: None }
    }
}

impl SurfaceOutput {
    /// Creates output tracking for a layer surface: pinned when `output_id` is `Some`, otherwise
    /// following `wl_surface.enter` / `wl_surface.leave`.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn new(output_id: Option<u32>) -> Self {
        match output_id {
            Some(id) => Self::Pinned(id),
            None => Self::default(),
        }
    }

    /// True when the surface was made without an output and needs `wl_output` bindings to follow
    /// `wl_surface.enter` / `wl_surface.leave`.
    #[cfg(test)]
    pub(crate) fn follows_enter_leave(&self) -> bool {
        matches!(self, Self::Follow { .. })
    }

    /// Records `wl_surface.enter` for one of this surface's bound `wl_output` globals.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn enter(&mut self, output: u32) {
        if let Self::Follow { on, .. } = self {
            if let Some(pos) = on.iter().position(|&id| id == output) {
                on.remove(pos);
            }
            on.push(output);
        }
    }

    /// Records `wl_surface.leave` for one of this surface's bound `wl_output` globals.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn leave(&mut self, output: u32) {
        if let Self::Follow { on, last_left } = self {
            if let Some(pos) = on.iter().position(|&id| id == output) {
                on.remove(pos);
            }
            *last_left = Some(output);
        }
    }

    /// Checks this surface's tracked output(s) against the registry's currently live `wl_output`
    /// globals. Prunes any removed outputs from `on`, and returns `true` when the surface's
    /// output is gone (the pinned output disappeared, all currently entered outputs disappeared,
    /// or the surface is on no output and the output it left last disappeared).
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn reconcile_live_outputs(&mut self, is_live: impl Fn(u32) -> bool) -> bool {
        match self {
            Self::Pinned(id) => !is_live(*id),
            Self::Follow { on, last_left } => {
                if on.is_empty() {
                    last_left.is_some_and(|id| !is_live(id))
                } else {
                    let mut i = 0;
                    while i < on.len() {
                        if !is_live(on[i]) {
                            let removed = on.remove(i);
                            *last_left = Some(removed);
                        } else {
                            i += 1;
                        }
                    }
                    on.is_empty()
                }
            }
        }
    }

    /// The output global name this surface is currently associated with, if known.
    #[cfg(test)]
    pub(crate) fn current(&self) -> Option<u32> {
        match self {
            Self::Pinned(id) => Some(*id),
            Self::Follow { on, last_left } => on.last().copied().or(*last_left),
        }
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
