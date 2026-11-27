//! The playground in an app's tests: [`App::headless_runtime`] attaches it, and
//! [`Runtime::open_playground`] opens a story the way `--playground NAME --variant V` does.

use super::*;
use crate::{App, ElementInfo, Env, Runtime};

impl<S: 'static> App<S> {
    /// A runtime for this app with no window, as a headless render builds it: the state from
    /// `AppSpec::init` (or [`App::init`]'s closure), `AppSpec::setup` given a waker that does
    /// nothing, the app's fonts, `poll` and `on_key`, and its [`App::playground`] attached,
    /// closed, if it's enabled (by default, in debug builds, so in `cargo test`). Nothing is
    /// read from the command line. Call [`Runtime::frame`] to build the first frame.
    pub fn headless_runtime(mut self, size: (f32, f32), env: Env) -> Runtime<S> {
        let playground = self.playground.take();
        let (chrome, titlebar, borderless) = (self.chrome, self.titlebar, self.window_options.borderless);
        crate::credentials::note_headless_runtime();
        let (spec, state, _, runtime) = self.into_parts(None);
        let mut rt = crate::tooling::runtime_for_configured(&spec, state, size, env, std::sync::Arc::new(|| {}), Some(runtime));
        if let Some(playground) = playground.filter(Playground::is_enabled) {
            rt.attach_playground(Live::new(playground, &spec, false));
        }
        rt.headless_chrome(chrome, titlebar, borderless);
        rt
    }
}

impl<S: 'static> Runtime<S> {
    /// Opens the playground at `story` (its `"Group/Name"` path, its name, or its path's
    /// [`slug`]), on `variant` (by number from 1, or by name) if given,
    /// as `--playground NAME --variant V` and [`Tx::open_playground_at`] do. Like a handler's
    /// [`Tx::open_playground`], it applies on a frame; this one takes
    /// two that advance no time, so on return the stage is made and the steps its `'r'`
    /// trigger runs at once have run. Steps queued with [`Play::after`] run as later frames
    /// reach them. Opening the story already open makes a scene's stage again, and a story's
    /// only with a variant.
    ///
    /// Panics without a playground (see [`App::headless_runtime`]), or when it has no such
    /// story or variant, listing those it has.
    pub fn open_playground(&mut self, story: &str, variant: Option<&str>) {
        let live = self.hooks.playground.as_mut().expect(NO_PLAYGROUND);
        live.core.open_at(Some(story), variant).unwrap_or_else(|e| panic!("{e}"));
        self.settle_playground(Request::Open(Some(story.to_owned())));
    }

    /// Runs the open story's trigger on `key`, as its letter or its button in the side column
    /// does: `'r'` makes the stage again and replays. As with [`Runtime::open_playground`],
    /// its first step has run on return, and its [`Play::after`] steps wait for their frames.
    ///
    /// Panics when the playground isn't open, or its story has no trigger on `key`.
    pub fn playground_trigger(&mut self, key: char) {
        let live = self.hooks.playground.as_ref().expect("the playground isn't open: call Runtime::open_playground first");
        let request = live.core.require_trigger(key);
        self.settle_playground(request);
    }

    /// Queues `request` as a handler does, then takes two frames that advance no time: the
    /// first applies it, the second runs the steps it made due now.
    fn settle_playground(&mut self, request: Request) {
        self.update(move |_, tx| tx.playground.push(request));
        self.frame(0.0);
        self.frame(0.0);
    }

    /// The open story's stage state: a scene's `S`, or a [`Story`]'s own state. `None` while
    /// the playground is closed, on a [`Playground::page`] (whose stage is the real state,
    /// `self.state`), or when `P` isn't the stage's type. `self.state` stays the app's real
    /// state throughout.
    pub fn playground_state<P: 'static>(&self) -> Option<&P> {
        self.hooks.playground.as_ref()?.stage_state()?.downcast_ref()
    }

    /// The element keyed or pathed `key` on the open story's stage, as of the last frame: its
    /// rect, text, role and the rest. The playground's list and side column aren't searched;
    /// [`Runtime::find`] and [`Runtime::elements`] see the whole window.
    pub fn playground_element(&self, key: &str) -> Option<ElementInfo> {
        if !self.hooks.playground.as_ref().is_some_and(|p| p.is_open()) { return None; }
        let all = self.elements();
        let i = stage_element(&all, key)?;
        all.into_iter().nth(i)
    }
}

const NO_PLAYGROUND: &str = "this runtime has no playground: build it with App::headless_runtime from an App whose App::playground is enabled";

/// The index in `all` of the element keyed or pathed `key` on the stage, not exiting.
pub(crate) fn stage_element(all: &[ElementInfo], key: &str) -> Option<usize> {
    let named = |i: &usize| !all[*i].exiting && (all[*i].key.as_deref() == Some(key) || all[*i].path == key);
    let in_stage = |i: &usize| std::iter::successors(Some(*i), |&j| all[j].parent).any(|j| all[j].key.as_deref().is_some_and(is_stage));
    (0..all.len()).filter(named).find(in_stage)
}

impl<S: 'static> Live<S> {
    /// The open stage's own state: a scene's, or a story's.
    fn stage_state(&self) -> Option<&dyn Any> {
        if !self.core.open { return None; }
        if let Some(scene) = self.scene() { return Some(&scene.state); }
        self.core.own().map(|(own, stage)| own.state_ref(stage))
    }
}
