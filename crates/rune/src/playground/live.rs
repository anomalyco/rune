//! [`Live`] and [`LiveCore`]: a window's playground while it runs, and the requests that move it.

use super::*;

/// An own-state story (`Story<P>`, type-erased) together with its live stage while open.
pub(super) struct OwnedStory {
    pub(super) story: Box<dyn OwnStory>,
    pub(super) stage: Option<Box<dyn Any>>,
}

/// Precomputed `S`-independent story metadata used by the list, side column, navigation and
/// layout without touching the app's state type.
pub(super) struct StoryMeta {
    pub(super) path: &'static str,
    pub(super) group: &'static str,
    pub(super) name: &'static str,
    pub(super) layout: Stage,
    pub(super) frame: Option<(f32, f32)>,
    pub(super) has_triggers: bool,
    pub(super) triggers: Vec<(char, &'static str)>,
    pub(super) variants: Vec<&'static str>,
    pub(super) side_keys: bool,
    pub(super) is_live_page: bool,
}

/// Inputs [`Live::new`] extracts from a typed `Playground<S>` and `AppSpec<S>` to build
/// [`LiveCore`] once, non-generically.
pub(super) struct LiveInit {
    pub(super) scenes: Vec<&'static str>,
    pub(super) pages: Vec<(&'static str, Option<Box<dyn OwnStory>>)>,
    pub(super) triggers: Vec<(usize, char, &'static str)>,
    pub(super) side_keys: Vec<usize>,
    pub(super) stages: Vec<(usize, Stage)>,
    pub(super) variants: Vec<(usize, &'static str)>,
    pub(super) sizes: Vec<(&'static str, (f32, f32))>,
    pub(super) shortcut: Option<Shortcut>,
    pub(super) menu: bool,
    pub(super) bar: bool,
    pub(super) show: Option<fn(&str) -> bool>,
    pub(super) folds: Folds,
    pub(super) widths: Widths,
    pub(super) group_order: Vec<&'static str>,
}

/// Narrow boundary through which non-generic [`LiveCore`] request handling reaches the typed
/// scene state and real app state `S`.
pub(super) trait SceneOps {
    fn real(&self) -> &dyn Any;
    fn keep(&mut self);
    fn clear_scene(&mut self);
    fn build_scene(&mut self, selected: usize, variant: usize, clock: crate::SceneClock);
    fn trigger_scene(&mut self, selected: usize, key: char, now: f64);
}

static REPLAY_ONLY: &[(char, &str)] = &[('r', "Replay")];

fn order_triggers(mut keys: Vec<(char, &'static str)>) -> Vec<(char, &'static str)> {
    let replay = keys.iter().position(|(k, _)| *k == 'r').map_or(('r', "Replay"), |i| keys.remove(i));
    keys.insert(0, replay);
    keys
}

/// Non-generic playground UI state, story metadata, navigation, chrome and own-state stages.
///
/// Compiled once in `librune` instead of per app state type `S`; [`Live<S>`] wraps this beside
/// the typed scene/page hooks.
pub(crate) struct LiveCore {
    pub(super) scene_count: usize,
    pub(super) stories: Vec<StoryMeta>,
    /// Each story's variant, by its index into scenes then pages, until the playground closes.
    pub(super) variant: Vec<usize>,
    pub(super) sizes: Vec<(&'static str, (f32, f32))>,
    pub(super) shortcut: Option<Shortcut>,
    /// The View menu's item takes the shortcut, so keys here leave it alone.
    pub(super) menu: bool,
    pub(super) bar: bool,
    /// Stories, as indices into scenes then pages, in the order the list shows them.
    pub(super) order: Vec<usize>,
    /// Into scenes then pages.
    pub(super) selected: usize,
    /// A group header is the list's selection rather than `selected`'s row.
    pub(super) header: Option<&'static str>,
    /// Groups whose stories the list hides, until the playground closes.
    pub(super) folded: Vec<&'static str>,
    /// [`Playground::folds`].
    pub(super) folds: Folds,
    /// [`Playground::widths`].
    pub(super) widths: Widths,
    /// The column that has the keyboard.
    pub(super) keyboard: Keyboard,
    /// Whether the playground is showing. While closed, `Live::stage.scene` is `None`.
    pub(super) open: bool,
    /// Own-state stories by page index (`i - scene_count`); `None` for live pages.
    pub(super) owned: Vec<Option<OwnedStory>>,
    pub(super) controls: Controls,
    pub(super) dark: Option<bool>,
    pub(super) size: usize,
    pub(super) reduce_motion: bool,
    pub(super) pending: Vec<Request>,
    /// Keys the stage's steps pressed with [`Play::press`], until the runtime routes them.
    pub(super) pressed: Vec<KeyEvent>,
    /// A key a step pressed is being routed: only the stage pages.
    pub(crate) pressing: bool,
    /// How many keys steps have pressed, which a motion gate's run reads as input.
    pub(crate) played: u64,
    /// The clock at the last commit or poll, which triggers start from.
    pub(super) now: f64,
    /// Bumped by every select and replay, so the new stage's elements start fresh: scroll
    /// offsets, focus and animations don't carry over from the last one.
    pub(super) generation: u32,
    /// The time scale the app had when the playground opened, which every new stage starts
    /// from and Back restores.
    pub(crate) app_time_scale: f32,
}

impl LiveCore {
    pub(super) fn new(init: LiveInit) -> Self {
        let LiveInit { scenes, pages, triggers, side_keys, stages, variants, sizes, shortcut, menu, bar, show, folds, mut widths, group_order } = init;
        let scene_count = scenes.len();
        let mut stories = Vec::with_capacity(scene_count + pages.len());
        for (i, path) in scenes.into_iter().enumerate() {
            let (group, name) = split(path, "Scenes");
            let layout = stages.iter().rev().find(|(s, _)| *s == i).map_or(Stage::Center, |(_, stage)| *stage);
            let keys: Vec<(char, &'static str)> = triggers.iter().filter(|(s, _, _)| *s == i).map(|(_, k, l)| (*k, *l)).collect();
            let has_triggers = !keys.is_empty();
            stories.push(StoryMeta {
                path,
                group,
                name,
                layout,
                frame: None,
                has_triggers,
                triggers: order_triggers(keys),
                variants: variants.iter().filter(|(s, _)| *s == i).map(|(_, label)| *label).collect(),
                side_keys: side_keys.contains(&i),
                is_live_page: false,
            });
        }
        let mut owned = Vec::with_capacity(pages.len());
        for (path, own) in pages {
            let (group, name) = split(path, "Pages");
            match own {
                Some(story) => {
                    let keys = story.triggers();
                    let has_triggers = !keys.is_empty();
                    stories.push(StoryMeta {
                        path,
                        group,
                        name,
                        layout: story.layout(),
                        frame: story.frame(),
                        has_triggers,
                        triggers: order_triggers(keys),
                        variants: story.variants(),
                        side_keys: story.side_keys(),
                        is_live_page: false,
                    });
                    owned.push(Some(OwnedStory { story, stage: None }));
                }
                None => {
                    stories.push(StoryMeta {
                        path,
                        group,
                        name,
                        layout: Stage::Center,
                        frame: None,
                        has_triggers: false,
                        triggers: Vec::new(),
                        variants: Vec::new(),
                        side_keys: false,
                        is_live_page: true,
                    });
                    owned.push(None);
                }
            }
        }
        let controls = Controls { save_widths: widths.save.take(), ..Controls::default() };
        let count = stories.len();
        let mut core = LiveCore {
            scene_count,
            stories,
            variant: vec![0; count],
            sizes,
            shortcut,
            menu,
            bar,
            order: Vec::new(),
            selected: 0,
            header: None,
            folded: Vec::new(),
            folds,
            widths,
            keyboard: Keyboard::default(),
            open: false,
            owned,
            controls,
            dark: None,
            size: 0,
            reduce_motion: false,
            pending: Vec::new(),
            pressed: Vec::new(),
            pressing: false,
            played: 0,
            now: 0.0,
            generation: 0,
            app_time_scale: 1.0,
        };
        let listed: Vec<usize> = (0..count).filter(|&i| i >= scene_count || show.is_none_or(|show| show(core.path(i)))).collect();
        core.order = core.tree_order(&listed, &group_order);
        core.selected = core.order.first().copied().unwrap_or(0);
        if core.folds.start { core.folded = core.all_groups(); }
        core.load_widths();
        core
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn count(&self) -> usize {
        self.stories.len()
    }

    pub(super) fn path(&self, i: usize) -> &'static str {
        self.stories[i].path
    }

    pub(super) fn group(&self, i: usize) -> &'static str {
        self.stories[i].group
    }

    pub(super) fn name(&self, i: usize) -> &'static str {
        self.stories[i].name
    }

    pub(super) fn own(&self) -> Option<(&dyn OwnStory, &dyn Any)> {
        let i = self.selected.checked_sub(self.scene_count)?;
        let own = self.owned.get(i)?.as_ref()?;
        Some((own.story.as_ref(), own.stage.as_deref()?))
    }

    pub(super) fn own_slot_mut(&mut self, i: usize) -> Option<&mut OwnedStory> {
        let page = i.checked_sub(self.scene_count)?;
        self.owned.get_mut(page)?.as_mut()
    }

    pub(super) fn is_live_page(&self) -> bool {
        self.stories.get(self.selected).is_some_and(|s| s.is_live_page)
    }

    #[inline(always)]
    pub(super) fn is_own_page(&self) -> bool {
        self.selected.checked_sub(self.scene_count).and_then(|i| self.owned.get(i)).is_some_and(Option::is_some)
    }

    /// How the stage lays out the open story.
    pub(super) fn layout(&self) -> Stage {
        self.stories.get(self.selected).map_or(Stage::Center, |s| s.layout)
    }

    /// An overlay story's dashed frame size, if the open story has one.
    pub(super) fn frame(&self) -> Option<(f32, f32)> {
        self.stories.get(self.selected).and_then(|s| s.frame)
    }

    /// Whether story `i` has triggers of its own, beside R, which the list marks.
    pub(super) fn has_triggers(&self, i: usize) -> bool {
        self.stories.get(i).is_some_and(|s| s.has_triggers)
    }

    /// The open story's triggers, R's first.
    pub(super) fn triggers(&self) -> &[(char, &'static str)] {
        match self.stories.get(self.selected) {
            Some(story) => &story.triggers,
            None => REPLAY_ONLY,
        }
    }

    pub(crate) fn request(&mut self, requests: Vec<Request>) {
        self.pending.extend(requests);
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn pending_swaps_stage(&self) -> bool {
        self.pending.iter().any(Request::swaps_stage)
    }

    /// Whether the only requests that swap the stage are replays, which keep its story.
    pub(crate) fn pending_replays_only(&self) -> bool {
        self.pending.iter().filter(|r| r.swaps_stage()).all(|r| matches!(r, Request::Replay | Request::Variant(_)))
    }

    pub(super) fn apply_requests(&mut self, clock: f64, ops: &mut dyn SceneOps) -> bool {
        self.now = clock;
        self.load_folds();
        self.load_widths();
        let mut changed = false;
        for request in std::mem::take(&mut self.pending) {
            changed = true;
            match request {
                Request::Open(name) => self.open_story(name.as_deref(), ops),
                Request::OpenAt(name, variant) => {
                    let name = self.resolve(&name, variant.as_deref());
                    self.open_story(name.as_deref(), ops);
                }
                Request::Close => self.close(ops),
                Request::Toggle if self.open => self.close(ops),
                Request::Toggle => {
                    self.open = true;
                    if self.folds.start { self.reveal(self.selected); }
                    self.select(self.selected, ops);
                }
                Request::Select(i) => {
                    self.reveal(i);
                    self.select(i, ops);
                }
                Request::Replay => self.replay(ops),
                Request::Variant(n) if n < self.variants().len() => {
                    self.variant[self.selected] = n;
                    self.replay(ops);
                }
                Request::Variant(_) => {}
                Request::Keep => ops.keep(),
                Request::Trigger(key) => self.trigger(key, ops),
                Request::Dark(dark) => self.dark = Some(dark),
                Request::Size(i) => self.size = i,
                Request::ReduceMotion(on) => self.reduce_motion = on,
                Request::Header(group) => self.header = Some(group),
                Request::Fold(group, fold) => self.fold(group, fold),
            }
        }
        // Opened on a story whose group is folded, the header stands in for its row.
        self.settle();
        changed
    }

    /// Folds a group, unfolds it, or toggles it. A filter shows every match, so folds wait
    /// until it's cleared. A selection whose row goes moves to the header that hid it.
    pub(super) fn fold(&mut self, group: &'static str, fold: Option<bool>) {
        if !self.query().is_empty() { return; }
        let folded = self.folded.contains(&group);
        match fold.unwrap_or(!folded) {
            true if !folded => self.refold(|f| f.push(group)),
            false => self.refold(|f| f.retain(|g| *g != group)),
            true => {}
        }
        self.settle();
    }

    /// Story `i`'s row becomes the list's selection, its groups unfolded.
    pub(super) fn reveal(&mut self, i: usize) {
        self.header = None;
        if i < self.count() {
            let groups = self.groups_of(i);
            self.refold(|f| f.retain(|g| !groups.contains(g)));
        }
    }

    /// The open story's stage made again, then its `'r'` trigger.
    pub(super) fn replay(&mut self, ops: &mut dyn SceneOps) {
        if self.is_live_page() { return; }
        if let Some(own) = self.own_slot_mut(self.selected) { own.stage = None; }
        self.select(self.selected, ops);
    }

    pub(super) fn close(&mut self, ops: &mut dyn SceneOps) {
        ops.keep();
        ops.clear_scene();
        self.open = false;
        self.header = None;
        self.moved(Moved::Closed);
        self.controls.filter = crate::TextEdit::default();
        self.variant.iter_mut().for_each(|v| *v = 0);
        for own in self.owned.iter_mut().flatten() { own.stage = None; }
    }

    /// Shows story `i`. A stage made new, as a scene's always is, runs its `'r'` trigger.
    /// Closed, only the selection moves: nothing is staged until the playground opens.
    pub(super) fn select(&mut self, i: usize, ops: &mut dyn SceneOps) {
        ops.keep();
        if let Some(own) = self.own_slot_mut(self.selected)
            && let Some(stage) = own.stage.as_mut()
        {
            own.story.stop(stage.as_mut());
        }
        self.selected = i.min(self.count().saturating_sub(1));
        if !self.open { return; }
        ops.clear_scene();
        self.generation += 1;
        let scene_clock = crate::SceneClock { now: self.now, dt: 1.0 / 60.0 };
        if self.selected < self.scene_count {
            ops.build_scene(self.selected, self.variant_at(), scene_clock);
        } else {
            let variant = self.variant_at();
            let Some(own) = self.own_slot_mut(self.selected) else { return };
            if own.stage.is_some() { return; }
            let stage = crate::with_scene_clock(scene_clock, || own.story.init(ops.real(), variant));
            own.stage = Some(stage);
        }
        self.trigger('r', ops);
    }

    /// Queues the open story's trigger on `key`, to run at the next poll from now.
    pub(super) fn trigger(&mut self, key: char, ops: &mut dyn SceneOps) {
        if !self.open { return; }
        let now = self.now;
        if self.selected < self.scene_count {
            ops.trigger_scene(self.selected, key, now);
            return;
        }
        if let Some(own) = self.own_slot_mut(self.selected)
            && let Some(stage) = own.stage.as_mut()
        {
            own.story.trigger(stage.as_mut(), key, now);
        }
    }

    /// Whether `ev` is the shortcut that opens and closes the playground, where no menu item
    /// takes it first.
    pub(crate) fn is_shortcut(&self, ev: &KeyEvent) -> bool {
        !self.menu && self.shortcut.as_ref().is_some_and(|s| s.matches_key(ev))
    }
}

/// The `S`-dependent scene hooks, live page views and active scene stage owned beside [`LiveCore`].
pub(super) struct TypedStage<S: 'static> {
    /// Live-page views by page index (`i - scene_count`); `None` for own-state stories.
    pub(super) live_pages: Vec<Option<View<S>>>,
    /// By the scene they're for.
    pub(super) triggers: Vec<(usize, Trigger<S>)>,
    pub(super) sides: Vec<(usize, Side<S>)>,
    pub(super) variants: Vec<(usize, Variant<S>)>,
    pub(super) init: fn() -> S,
    pub(super) fresh: Option<fn(&S) -> S>,
    pub(super) keep: Option<fn(&S, &mut S)>,
    pub(super) typing: Option<fn(&S) -> bool>,
    /// [`Playground::dark`].
    pub(super) app_dark: Option<fn(&S) -> bool>,
    pub(super) scenes: &'static [(&'static str, fn(&mut S))],
    pub(super) poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
    /// The open scene's stage; always `None` while closed or showing a page.
    pub(super) scene: Option<Box<Staged<S>>>,
}

struct SceneSession<'a, S: 'static> {
    stage: &'a mut TypedStage<S>,
    real: &'a mut S,
}

impl<S: 'static> SceneOps for SceneSession<'_, S> {
    fn real(&self) -> &dyn Any { self.real }

    fn keep(&mut self) {
        if let (Some(keep), Some(scene)) = (self.stage.keep, self.stage.scene.as_deref()) {
            keep(&scene.state, self.real);
        }
    }

    fn clear_scene(&mut self) {
        self.stage.scene = None;
    }

    fn build_scene(&mut self, selected: usize, variant: usize, clock: crate::SceneClock) {
        let (_, f) = self.stage.scenes[selected];
        let (init, fresh) = (self.stage.init, self.stage.fresh);
        let variant = self.stage.scene_variant(selected, variant);
        let state = crate::with_scene_clock(clock, || {
            let mut s = fresh.map_or_else(init, |fresh| fresh(self.real));
            f(&mut s);
            if let Some(variant) = variant { variant(&mut s); }
            s
        });
        self.stage.scene = Some(Box::new(Staged::new(state)));
    }

    fn trigger_scene(&mut self, selected: usize, key: char, now: f64) {
        if let Some(scene) = &mut self.stage.scene
            && let Some((_, t)) = self.stage.triggers.iter().find(|(s, t)| *s == selected && t.key == key)
        {
            scene.script.push((now, t.queued()));
        }
    }
}

/// A window's playground while it runs.
pub(crate) struct Live<S: 'static> {
    pub(super) core: LiveCore,
    pub(super) stage: TypedStage<S>,
    /// `compose` and `apply_requests`, which the runtime reaches through these so that only an app
    /// that makes a playground (`Live::new`) links the playground's views and requests.
    compose: fn(&Self, &S, fn(&S, &mut Cx<S>) -> El, &mut Cx<S>, (f32, f32)) -> El,
    apply_requests: fn(&mut Self, f64, &mut S) -> bool,
    /// Trait-object casts initialized only in `Live::new`, keeping `Live<S>: PlaygroundDriver`
    /// dead-code-eliminated for apps without a playground.
    as_driver: fn(&Self) -> &dyn crate::runtime::driver::PlaygroundDriver,
    as_driver_mut: fn(&mut Self) -> &mut dyn crate::runtime::driver::PlaygroundDriver,
}

impl<S: 'static> std::ops::Deref for Live<S> {
    type Target = LiveCore;
    #[inline(always)]
    fn deref(&self) -> &LiveCore { &self.core }
}

impl<S: 'static> Live<S> {
    pub(super) fn new(playground: Playground<S>, spec: &crate::AppSpec<S>, menu: bool) -> Self {
        let Playground { pages, triggers, sides, stages, sizes, shortcut, bar, enabled: _, scenes: show, fresh, keep, variants, typing, dark: app_dark, folds, widths, group_order } = playground;
        let scene_names: Vec<&'static str> = spec.scenes.iter().map(|(name, _)| *name).collect();
        let mut live_pages = Vec::with_capacity(pages.len());
        let mut page_specs = Vec::with_capacity(pages.len());
        for Page { name, kind } in pages {
            match kind {
                Kind::Live(view) => { live_pages.push(Some(view)); page_specs.push((name, None)); }
                Kind::Own(own) => { live_pages.push(None); page_specs.push((name, Some(own))); }
            }
        }
        let mut typed_triggers = Vec::new();
        let mut trigger_meta = Vec::new();
        for (name, t) in triggers {
            if let Some(s) = open::resolve_scene(&scene_names, name) {
                trigger_meta.push((s, t.key, t.label));
                typed_triggers.push((s, t));
            }
        }
        let mut typed_sides = Vec::new();
        let mut side_keys = Vec::new();
        for (name, side) in sides {
            if let Some(s) = open::resolve_scene(&scene_names, name) {
                if matches!(side, Side::Keys(..)) { side_keys.push(s); }
                typed_sides.push((s, side));
            }
        }
        let stages = stages.into_iter().filter_map(|(name, stage)| Some((open::resolve_scene(&scene_names, name)?, stage))).collect();
        let mut typed_variants = Vec::new();
        let mut variant_meta = Vec::new();
        for (name, label, apply) in variants {
            if let Some(s) = open::resolve_scene(&scene_names, name) {
                variant_meta.push((s, label));
                typed_variants.push((s, apply));
            }
        }
        let core = LiveCore::new(LiveInit {
            scenes: scene_names,
            pages: page_specs,
            triggers: trigger_meta,
            side_keys,
            stages,
            variants: variant_meta,
            sizes,
            shortcut,
            menu,
            bar,
            show,
            folds,
            widths,
            group_order,
        });
        Live {
            core,
            stage: TypedStage {
                live_pages,
                triggers: typed_triggers,
                sides: typed_sides,
                variants: typed_variants,
                init: spec.init,
                fresh,
                keep,
                typing,
                app_dark,
                scenes: spec.scenes,
                poll: spec.poll,
                scene: None,
            },
            compose: Self::compose,
            apply_requests: Self::apply_requests,
            as_driver: |live| live,
            as_driver_mut: |live| live,
        }
    }

    /// The open scene's stage.
    pub(super) fn scene(&self) -> Option<&Staged<S>> {
        self.stage.scene.as_deref()
    }

    pub(super) fn live_page(&self) -> Option<&View<S>> {
        let i = self.core.selected.checked_sub(self.core.scene_count)?;
        self.stage.live_pages.get(i)?.as_ref()
    }

    /// The window's root: the app's view above the bar, or the playground around its stage.
    pub(crate) fn view(&self, app: &S, view: fn(&S, &mut Cx<S>) -> El, cx: &mut Cx<S>, viewport: (f32, f32)) -> El {
        (self.compose)(self, app, view, cx, viewport)
    }

    /// Applies the requests handlers made since the last commit; true when anything changed.
    pub(crate) fn apply(&mut self, clock: f64, real: &mut S) -> bool { (self.apply_requests)(self, clock, real) }

    fn apply_requests(&mut self, clock: f64, real: &mut S) -> bool {
        let mut ops = SceneSession { stage: &mut self.stage, real };
        self.core.apply_requests(clock, &mut ops)
    }

    pub(crate) fn driver(&self) -> &dyn crate::runtime::driver::PlaygroundDriver {
        (self.as_driver)(self)
    }

    pub(crate) fn driver_mut(&mut self) -> &mut dyn crate::runtime::driver::PlaygroundDriver {
        (self.as_driver_mut)(self)
    }
}

impl<S: 'static> crate::runtime::driver::PlaygroundDriver for Live<S> {
    fn is_open(&self) -> bool { self.core.is_open() }
    fn is_shortcut(&self, ev: &KeyEvent) -> bool { self.core.is_shortcut(ev) }
    fn request(&mut self, requests: Vec<Request>) { self.core.request(requests); }
    fn has_pending(&self) -> bool { self.core.has_pending() }
    fn pending_swaps_stage(&self) -> bool { self.core.pending_swaps_stage() }
    fn pending_replays_only(&self) -> bool { self.core.pending_replays_only() }
    fn polls(&self) -> bool { Live::polls(self) }
    fn poll_stage(&mut self, clock: f64, tx: &mut Tx) -> bool { Live::poll_stage(self, clock, tx) }
    fn take_pressed(&mut self) -> Vec<KeyEvent> { self.core.take_pressed() }
    fn pressing(&self) -> bool { self.core.pressing }
    fn set_pressing(&mut self, pressing: bool) { self.core.pressing = pressing; }
    fn app_time_scale(&self) -> f32 { self.core.app_time_scale }
    fn set_app_time_scale(&mut self, scale: f32) { self.core.app_time_scale = scale; }
    fn region(&self) -> Region { self.core.region() }
    fn region_at(&self, x: f32, width: f32) -> Region { self.core.region_at(x, width) }
    fn saw_focus(&self, focus: Option<&str>) -> bool { self.core.saw_focus(focus) }
    fn moved(&mut self, moved: Moved) -> bool { self.core.moved(moved) }
    fn nav(&self, ev: &KeyEvent, region: Region) -> Nav { self.core.nav(ev, region) }
    fn side_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool { Live::side_key(self, ev, tx) }
    fn clear_filter(&mut self) -> bool { self.core.clear_filter() }
    fn first_shown(&self) -> Option<usize> { self.core.first_shown() }
    fn first_match(&self) -> Option<usize> { self.core.first_match() }
}
