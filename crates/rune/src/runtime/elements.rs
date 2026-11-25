//! Element inspection (`Runtime::elements`, `presented`, `presented_pose`, `find`,
//! `text_offset_at`). Point picking lives in `runtime/pick.rs`.
use super::*;

/// Resolved typography of a text element ([`ElementInfo::text_style`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTextStyle {
    /// The font family actually used after CSS family-list fallback (from the first visible
    /// shaped run, or the first available family in the stack for empty or secure text).
    pub family: Option<String>,
    pub size: f32,
    pub weight: f32,
    /// Resolved line height in points ([`TextStyle::line_px`](crate::tok::TextStyle::line_px),
    /// `1.25 * size` when auto).
    pub line_height: f32,
    pub tracking: f32,
    pub italic: bool,
}

/// What tools see of one element.
#[derive(Clone, Debug)]
pub struct ElementInfo {
    pub trace_name: Option<&'static str>,
    pub trace_namespace: Option<(&'static str, &'static str)>,
    pub loop_phase: Option<f64>,
    pub focusable: bool,
    pub focused: bool,
    pub focus_visible: bool,
    pub inert: bool,
    /// Playing its exit transition, or inside an element that is. Lookups by key treat it
    /// as gone; it stays listed so tools can watch the exit.
    pub exiting: bool,
    pub path: String,
    pub key: Option<String>,
    pub role: &'static str,
    pub label: Option<String>,
    /// What a text element shows (none for a secure field). An element of role `"text"` drawn
    /// from several texts, such as rune-kit's `RollingText`, gives its label.
    pub text: Option<String>,
    pub rect: [f32; 4],
    pub target: [f32; 4],
    pub opacity: f32,
    /// Effective uniform scale, including ancestors and the presence transition.
    pub scale: f32,
    /// Additive blur culling margin in logical points, including scaled ancestor sigmas.
    /// Three times this value bounds filter support; it is not the equivalent Gaussian sigma.
    pub blur: f32,
    pub radius: f32,
    pub fill: Color,
    /// Presented border colour, when this element has a non-zero border width.
    pub border_color: Option<Color>,
    pub text_color: Option<Color>,
    /// Resolved typography for text, rich text and text input elements.
    pub text_style: Option<ResolvedTextStyle>,
    /// Padding plus border `[left, top, right, bottom]` in window points: text draws inside it.
    pub inset: [f32; 4],
    /// Laid-out text `[width, height]` in window points (`None` for non-text elements and secure text).
    pub text_size: Option<[f32; 2]>,
    /// Whether this text element wraps lines ([`El::wrap`]).
    pub wrap: bool,
    /// Whether this text element truncates or line-clamps on purpose
    /// ([`El::truncate`], [`El::truncate_fade`], or [`El::max_lines`]).
    pub truncate: bool,
    /// Whether truncation or line clamping cut or faded the text on the last frame
    /// (`false` for secure text).
    pub text_cut: bool,
    /// Whether this element clips its descendants (a scroll viewport,
    /// [`El::clip_soft`]/[`El::clip_rounded`]/[`El::clip_corners`]/[`El::clip_fade`], or an active scene mask).
    pub clips: bool,
    /// Axes this element scrolls along (`[x, y]`).
    pub scroll_axes: [bool; 2],
    /// Whether this element paints above siblings ([`El::raise`]).
    pub raised: bool,
    pub hover: f32,
    pub press: f32,
    pub moving: bool,
    pub parent: Option<usize>,
    /// The part of `rect` its ancestors' clips leave visible.
    pub visible: [f32; 4],
    /// Words of this text are still fading in.
    pub revealing: bool,
    /// Inside a growth row whose height is moving: its visible part is being revealed.
    pub in_growing_row: bool,
    /// This node is riding a scroll: a wheel or trackpad, its momentum, or a glide
    /// (`Tx::scroll_to`, `scroll_to_with`, `scroll_to_bottom`).
    pub scrolling: bool,
    /// How far its scroll viewports' offsets carry it this frame, x and y in the window's points
    /// (scaled and turned as `rect` is): `rect` sits this much left and up of where it would
    /// unscrolled. Motion checks add it back, so they judge an element against the content it
    /// scrolls with.
    pub scrolled: [f32; 2],
    /// Invisible estimated extent of a virtual list, not a drawable surface.
    pub virtual_extent: bool,
    /// Property-specific intentional cuts for 1:1 live bindings (never smoothed bindings).
    pub live_intent: u32,
    /// On the focused text field: its caret line in window points (see [`Cx::caret`]).
    pub caret: Option<[f32; 4]>,
    /// On the focused text field, unless secure: its selection's anchor and focus, UTF-8 bytes
    /// of its text (equal for a caret).
    pub edit_range: Option<[usize; 2]>,
    /// Dragging it starts an OS drag ([`El::drag_out`]).
    pub drag_out: bool,
    /// Its tooltip and accessibility help ([`El::help`]).
    pub help: Option<String>,
    /// Its explicit accessibility description ([`El::description`], [`El::description_static`]),
    /// omitting empty strings. Separate from [`Self::help`].
    pub description: Option<String>,
    /// The part of this [`El::selectable`] text that is selected, as copying would take it.
    pub selected: Option<String>,
    /// Its outline depth ([`El::level`]).
    pub level: Option<u32>,
    /// Whether it is open ([`El::expanded`]); `None` when it can't open.
    pub expanded: Option<bool>,
    /// A selected item of a list, tree, table or group ([`El::focus_selected`]).
    pub item_selected: bool,
    /// How far [`El::sticky_top`], [`El::sticky_bottom`] or [`El::sticky_left`] moved it from
    /// where scrolling put it this frame, in points: down for a stuck header, up for a stuck
    /// footer, right for a stuck left element, 0 when it isn't stuck. `rect` already includes it.
    pub stuck: f32,
    /// True when this visible [`El::system_material`](crate::El::system_material) element
    /// exceeded the window's native material cap ([`App::max_native_materials`](crate::App::max_native_materials),
    /// default [`MAX_NATIVE_MATERIALS`](crate::MAX_NATIVE_MATERIALS)) on the last frame and drew
    /// its fallback fill instead.
    pub material_fallback: bool,
}

/// One `El::matched` handover completed on the last frame: `from` left the tree at `from_rect`
/// as `to` took over on the same frame at `to_rect`.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchedHandover {
    /// Element tree path of the leaving source (`ElementInfo::path`).
    pub from: String,
    /// Element tree path of the arriving target (`ElementInfo::path`).
    pub to: String,
    /// Drawn rect `[x, y, w, h]` of `from` on its last frame, in window points.
    pub from_rect: [f32; 4],
    /// Drawn rect `[x, y, w, h]` of `to` on the handover frame, in window points.
    pub to_rect: [f32; 4],
}

/// What an element presents beyond its rect and opacity, as of the last frame
/// ([`Runtime::presented_pose`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    rotation: f32,
    scale: f32,
    shadows: Vec<crate::el::Shadow>,
    moving: bool,
}

impl Pose {
    /// The element's own presented `El::rotate`, in radians clockwise. Ancestors' turns are not
    /// included.
    pub fn rotation(&self) -> f32 { self.rotation }
    /// Effective uniform scale, ancestors included (as [`ElementInfo::scale`]).
    pub fn scale(&self) -> f32 { self.scale }
    /// The element's shadows (`El::box_shadow`) at their current sprung geometry and colour.
    pub fn shadows(&self) -> &[crate::el::Shadow] { &self.shadows }
    /// A spring or decorative loop still moves its rect, scale, rotation, icon turn or shadows,
    /// or an ancestor's. An active repeating loop does not settle.
    pub fn moving(&self) -> bool { self.moving }
}

impl types::TreeState {
    /// Authored geometry goals, before scroll and spring sampling, in parent space.
    pub(crate) fn trace_goals(&self) -> impl Iterator<Item = (&str, [f32; 7])> {
        self.nodes.iter().map(|n| (n.path.as_str(), [n.local.x, n.local.y, n.local.w, n.local.h, n.vis.offset[0], n.vis.offset[1], n.static_scale]))
    }

    /// Elements that draw nothing and hold nothing: no paint of their own, no text, no field,
    /// no focus, no children. Hit and hover zones, spacers. A trace flags them: nobody can see
    /// them come, go or move.
    pub(crate) fn blank_leaves(&self) -> HashSet<&str> {
        let mut parents = vec![false; self.nodes.len()];
        for n in &self.nodes {
            if let Some(p) = n.parent { parents[p] = true; }
        }
        self.nodes.iter().enumerate().filter(|&(i, n)| {
            let v = &n.vis;
            !parents[i] && n.text.is_none() && n.input.is_none() && !n.focus.enabled && n.material.is_none()
                && v.fill.a == 0.0 && (v.border_width() == 0.0 || v.border().a == 0.0 && v.border_bottom.is_none_or(|c| c.a == 0.0) && n.focus.border.is_none_or(|c| c.a == 0.0))
                && v.shadows.is_empty() && v.surface.is_none() && v.icon.is_none() && v.gradient.is_none() && v.radial.is_none()
                && v.outline.is_none() && v.shape.is_none() && v.grid.is_none() && v.pixels.is_none() && v.effect.is_none()
                && v.hover().a == 0.0 && v.press().a == 0.0
        }).map(|(_, n)| n.path.as_str()).collect()
    }

    /// Video elements' playback state by element path, for `--tree`.
    pub(crate) fn videos(&self) -> HashMap<String, crate::video::Snapshot> {
        self.nodes.iter()
            .filter_map(|n| Some((n.path.clone(), n.vis.surface.as_ref()?.0.video()?.snapshot())))
            .collect()
    }

    /// Whether an element path contains secure text, hidden or revealed. Accessibility
    /// adapters should use this alongside [`Runtime::elements`] to report protected content.
    /// Unknown paths (including synthetic flight/link paths) return false.
    pub(crate) fn element_is_secure(&self, path: &str) -> bool {
        self.nodes.iter().find(|n| n.path == path).is_some_and(Node::is_secure)
    }
}

impl RuntimeCore {
    /// Tracks the last tick sampled, by element path and property name: what spent this
    /// frame animating. Tracks of nodes no longer in the tree report their node id.
    pub(crate) fn touched_tracks(&self) -> Vec<(String, &'static str, bool)> {
        let store = self.scene.store();
        let mut out: Vec<_> = store.touched_tracks().map(|(track, node, p)| {
            let path = self.tree.node_by_id(node).map_or_else(|| format!("node:{}", node.0), |n| n.path.clone());
            (path, prop_name(p.0), store.is_active(track))
        }).collect();
        // A growth row's feathered edge (`GrowTrack::mask`) paints as it eases out, after its height
        // has landed: it is a track of its own here.
        out.extend(self.motion.grows.iter().filter(|(_, g)| g.mask > 0.01).filter_map(|(&id, _)| {
            self.tree.node_by_id(id).map(|n| (n.path.clone(), "mask", true))
        }));
        out.sort();
        out.dedup();
        out
    }

    /// What a move of `v` points inside node `i` (a scroll viewport's offset) comes to in the
    /// window: scaled and turned as its children draw.
    pub(crate) fn content_to_window(&self, i: usize, v: [f32; 2]) -> [f32; 2] {
        let Some(h) = self.paint.inh_scratch.get(i) else { return v };
        let [x, y] = v.map(|c| c * h.scale);
        match h.turn.checked_sub(1).and_then(|k| self.paint.turn_scratch.get(k as usize)) {
            Some(t) => [t.mat[0] * x + t.mat[1] * y, t.mat[2] * x + t.mat[3] * y],
            None => [x, y],
        }
    }

    /// `El::matched` handovers completed on the last frame, with both drawn rects in window points.
    pub fn matched_handovers(&self) -> Vec<MatchedHandover> {
        self.motion.handovers().iter().map(|h| {
            let to_rect = self.tree.node_index.get(&h.to_id)
                .and_then(|&i| self.paint.drawn.get(i).copied())
                .unwrap_or(h.start_rect);
            MatchedHandover {
                from: h.from_path.clone(),
                to: h.to_path.clone(),
                from_rect: h.from_rect,
                to_rect,
            }
        }).collect()
    }

    pub fn elements(&self) -> Vec<ElementInfo> {
        #[cfg(test)]
        ELEMENTS_BUILDS.with(|c| c.set(c.get() + 1));
        fn intent(n: &Node) -> u32 {
            n.live.iter().filter(|b| b.enter.is_none()).fold(0, |bits, b| bits | match b.property {
                crate::live::Property::Offset => 1,
                crate::live::Property::Scale | crate::live::Property::Rotation | crate::live::Property::Rect => 1 | 2,
                crate::live::Property::Opacity => 4,
                _ => 0,
            })
        }
        let store = self.scene.store();
        let animating = |id: NodeId| store.is_animating(id, prop::FRAME) || store.is_animating(id, prop::TRAVEL)
            || store.is_animating(id, prop::PRESENCE) || store.is_animating(id, prop::OFFSET) || store.is_animating(id, prop::SCALE) || store.is_animating(id, P_PRESS_SCALE) || store.is_animating(id, P_PRESS_OX) || store.is_animating(id, P_PRESS_OY)
            || store.is_animating(id, P_SCALE) || store.is_animating(id, prop::ROTATION) || paint_pose_springing(store, id)
            || (!self.reduced_motion && self.motion.loop_active(id, store.node_time(id)));
        let sel_ranges = self.selection_ranges();
        let mut elements: Vec<_> = self.tree.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let p = self.scene.presented(n.id)?;
                let exiting = self.subtree_exiting(i);
                let flight = self.motion.flights().iter().find(|f| f.lands(&n.path, n.key.as_deref()));
                let mut moving = flight.is_some() || animating(n.id) || self.motion.grows.get(&n.id).is_some_and(|g| g.moving());
                let drawn = self.paint.drawn.get(i).copied().unwrap_or([p.rect.x, p.rect.y, p.rect.w, p.rect.h]);
                // A flight carries the destination's laid-out text, which can be narrower than
                // its box. Report the box carried along with it, so the element lands on its own
                // frame instead of jumping out to it when the flight ends.
                let rect = flight.map_or(drawn, |f| {
                    let (d, c) = (f.destination, f.presented);
                    let (zx, zy) = (c.w / d.w.max(1e-3), c.h / d.h.max(1e-3));
                    [c.x + (drawn[0] - d.x) * zx, c.y + (drawn[1] - d.y) * zy, drawn[2] * zx, drawn[3] * zy]
                });
                let s = self.env.scale;
                let clip = if self.motion.launches().flying(n.id) { NO_CLIP } else {
                    flight.map_or_else(|| self.screen_clip_of_parent(n.parent), |f| f.clip.map(|v| v * s))
                };
                let (x0, y0) = (rect[0].max(clip[0] / s), rect[1].max(clip[1] / s));
                let (x1, y1) = ((rect[0] + rect[2]).min(clip[2] / s), (rect[1] + rect[3]).min(clip[3] / s));
                let visible = [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)];
                let revealing = flight.is_some() || self.motion.reveal_fading(n.id);
                let mut in_growing_row = false;
                let mut scrolling = false;
                let mut scrolled = [0.0; 2];
                let mut live_intent = intent(n);
                // Held at its landing width this frame: its re-layout is the design.
                if self.motion.holds_landing(n.id) {
                    live_intent |= 1 | 2;
                }
                // A group_opacity swap under an instant hover group is a deliberate cut (Beta's
                // status/close swap), for it and what it contains: opacity and transition intent.
                let up = |from: Option<usize>| std::iter::successors(from, |&pi| self.tree.nodes[pi].parent);
                if let Some(gi) = up(Some(i)).find(|&j| self.tree.nodes[j].vis.group_opacity.is_some()) {
                    let group = up(self.tree.nodes[gi].parent)
                        .map(|pi| &self.tree.nodes[pi])
                        .find(|g| g.vis.tracks_hover_paint() || g.vis_flags.hover_group());
                    if group.and_then(|g| g.hover_motion()).is_some_and(|(e, x)| [e, x].iter().any(|a| matches!(a.curve, rune_motion::Curve::Instant))) {
                        live_intent |= 4 | 16;
                    }
                }
                // Offsets are in points inside the scroll viewport; `rect` is in the window, after
                // every scale and turn above. Below a lift (a flight or launch drawn over the
                // window), the scales holding still above it carry them, unturned.
                let mut lifted: Option<(f32, u32)> = None;
                let mut j = i;
                let mut steps = 0usize;
                loop {
                    let to_window = |at: Option<usize>, v: [f32; 2]| match lifted {
                        Some((k, turn)) => {
                            let [x, y] = v.map(|c| c * k);
                            match turn.checked_sub(1).and_then(|t| self.paint.turn_scratch.get(t as usize)) {
                                Some(t) => [t.mat[0] * x + t.mat[1] * y, t.mat[2] * x + t.mat[3] * y],
                                None => [x, y],
                            }
                        }
                        None => at.map_or(v, |at| self.content_to_window(at, v)),
                    };
                    if j != i && let Some(st) = self.scroll.scrolls.get(&self.tree.nodes[j].id) {
                        let [x, y] = to_window(Some(j), [st.drawn_x, st.drawn.y()]);
                        scrolled = [scrolled[0] + x, scrolled[1] + y];
                    }
                    // A launch, and what it carries, leaves under the offsets it launched against
                    // and takes on the scroll's only as it lands.
                    let id = self.tree.nodes[j].id;
                    let parent = self.tree.nodes[j].parent;
                    let fix = self.motion.launches().carried(&self.scene, id);
                    if fix != [0.0; 2] {
                        let k = parent.and_then(|p| self.paint.inh_scratch.get(p)).map_or(1.0, |h| self.paint.inh_cold_of(h).still.0);
                        scrolled = [scrolled[0] - fix[0] * k, scrolled[1] - fix[1] * k];
                    }
                    if lifted.is_none() && self.motion.flying.iter().any(|f| f.0 == id) {
                        let is_launch = self.motion.launches().launched(id);
                        let in_place = self.tree.nodes[j].vis.flight_in_place && !is_launch;
                        lifted = Some(parent.and_then(|p| self.paint.inh_scratch.get(p)).map_or((1.0, 0), |h| {
                            let c = self.paint.inh_cold_of(h);
                            (if is_launch { c.still.0 } else { c.held.0 }, if in_place { h.turn } else { 0 })
                        }));
                    }
                    steps += 1;
                    if steps >= self.tree.nodes.len() { break; }
                    let Some(pi) = step_up(&self.tree.nodes, j) else { break };
                    let p = &self.tree.nodes[pi];
                    moving |= animating(p.id)
                        || self.motion.stacks.get(&p.id).is_some_and(|s| s.moving)
                        || self.scroll.scrolls.get(&p.id).is_some_and(|s| s.slack.moving() || s.glide().is_some());
                    live_intent |= intent(p);
                    scrolling |= self.scroll.scrolls.get(&p.id).is_some_and(ScrollState::moving);
                    in_growing_row |= self.motion.grows.get(&p.id).is_some_and(|g| g.moving());
                    j = pi;
                }
                let secure = n.is_secure();
                let text_size = (!secure).then(|| {
                    let spec = n.text.as_deref()?;
                    if let (Some(r), Some((_, at))) = (&n.run_layout, &spec.cold.advance_in)
                        && let Some(w) = r.advance(*at, s)
                    {
                        return Some([w, crate::text::spec_height(&r.layout, spec.style, s, spec.exact_lines) / s]);
                    }
                    let laid = n.text_layout.as_ref()?;
                    let l = laid.view();
                    let w = text_width(if spec.wrap { l.width() } else { l.full_width() }) / s;
                    let h = l.spec_height(spec.style, s, spec.exact_lines) / s;
                    Some([w, h])
                }).flatten();
                Some(ElementInfo {
                    trace_name: n.trace_name,
                    trace_namespace: n.trace_namespace,
                    loop_phase: self.motion.loop_track(n.id).map(|l| {
                        if self.reduced_motion && !l.is_finite() { 0.0 }
                        else { l.phase(self.scene.store().node_time(n.id)) }
                    }),
                    focusable: self.can_focus(i),
                    focused: self.kbd.focus.path() == Some(&n.path),
                    focus_visible: self.kbd.focus.keyboard() && self.kbd.focus.path() == Some(&n.path),
                    inert: !self.allowed(i) || exiting,
                    exiting,
                    path: n.path.clone(),
                    key: n.key.clone(),
                    role: n.role,
                    label: n.label.as_deref().map(str::to_owned),
                    text: match &n.text {
                        Some(t) => (!secure).then(|| t.source_text().to_owned()),
                        // A text built from several (a rolling label's glyphs) reads as its label.
                        None => (!secure).then(|| n.label.as_deref().map(str::to_owned)).flatten().filter(|_| n.role == "text"),
                    },
                    rect,
                    target: [n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]],
                    opacity: flight.map_or_else(|| self.paint.inh_scratch.get(i).map_or(p.opacity, |h| h.opacity), |f| f.target_alpha),
                    scale: if flight.is_some() { 1.0 } else { self.paint.inh_scratch.get(i).map_or(p.transform.scale.x, |h| h.scale) },
                    blur: if flight.is_some() { 0.0 } else { self.paint.inh_scratch.get(i).map_or(p.blur, |h| self.paint.inh_cold_of(h).blur) },
                    radius: p.radius.0[0],
                    fill: self.color(n.id, P_FILL, n.vis.fill),
                    border_color: (n.vis.border_width() > 0.0).then(|| self.color(n.id, P_BORDER, n.vis.border())),
                    text_color: paint_tracks::text_color(n)
                        .map(|c| paint_tracks::group_tint(n, self.color(n.id, P_TEXT, c), group_lit(&self.scene, n, self.paint.inh_scratch.get(i).map_or(0.0, |h| h.group)))),
                    text_style: n.text.as_deref().map(|spec| {
                        let family = if secure {
                            self.text.resolved_family(spec.style.family)
                        } else {
                            n.text_layout
                                .as_ref()
                                .and_then(crate::text::resolved_family_of)
                                .or_else(|| self.text.resolved_family(spec.style.family))
                        };
                        ResolvedTextStyle {
                            family,
                            size: spec.style.size,
                            weight: spec.style.weight,
                            line_height: spec.style.line_px(),
                            tracking: spec.style.tracking,
                            italic: spec.style.italic,
                        }
                    }),
                    inset: n.inset,
                    text_size,
                    wrap: n.text.as_ref().is_some_and(|t| t.wrap),
                    truncate: n.text.as_ref().is_some_and(|t| t.truncate || t.truncate_fade || t.max_lines > 0),
                    text_cut: !secure && n.text_cut(s),
                    clips: n.scroll.is_some() || n.vis.clip.is_some() || n.vis.clip_radius.is_some() || !p.mask.is_none(),
                    scroll_axes: [n.scroll.is_some() && n.scroll_axes.x(), n.scroll.is_some() && n.scroll_axes.y()],
                    raised: n.vis_flags.raise(),
                    hover: if (n.focus.highlight.is_some() && self.highlight_visible(i)) || (n.focus.fill.is_some() && self.focus_ring_visible(i)) { 1.0 }
                        else { self.scene.get(n.id, P_HOVER).unwrap_or(0.0) },
                    press: self.scene.get(n.id, P_PRESS).unwrap_or(0.0),
                    moving,
                    parent: n.parent,
                    visible,
                    revealing,
                    in_growing_row,
                    scrolling,
                    scrolled,
                    live_intent,
                    caret: self.caret_rect(&self.tree.nodes, i),
                    edit_range: n.input.as_ref().filter(|inp| inp.edit.focused && !inp.edit.is_secure()).map(|inp| [inp.edit.anchor, inp.edit.focus]),
                    drag_out: n.handlers.drag_out.is_some(),
                    help: n.help.as_deref().filter(|_| !n.help_cut || n.text_cut(s)).map(|h| crate::el::help_words(h).into_owned()),
                    description: n.a11y.more.as_deref().and_then(|m| m.description.as_deref()).filter(|d| !d.is_empty()).map(str::to_owned),
                    selected: n.text.as_ref().filter(|t| t.selectable && !secure).and_then(|t| {
                        let (a, b) = selectable::range_in(&sel_ranges, i)?;
                        t.copy_slice(a as u32, b as u32)
                    }),
                    level: n.a11y.level,
                    expanded: n.a11y.expanded,
                    item_selected: n.focus.selected,
                    stuck: self.scroll.stuck.get(i).copied().unwrap_or(0.0),
                    material_fallback: self.paint.material_fallback(n.id),
                    virtual_extent: self.commit.virtuals.contains_key(&n.path) && n.vis.fill.a == 0.0
                        && n.vis.border_width() == 0.0 && n.vis.shadows.is_empty() && n.text.is_none() && n.vis.icon.is_none(),
                })
            })
            .collect();
        for f in self.motion.flights() {
            let r = if self.reduced_motion { f.source } else { f.presented };
            let c = if self.reduced_motion { f.source_clip } else { f.clip };
            let path = format!("flight-source/{}", f.target_key);
            let t = f.destination;
            elements.push(ElementInfo {
                trace_name: None,
                trace_namespace: None,
                loop_phase: None,
                focusable: false, focused: false, focus_visible: false, inert: true, exiting: false,
                key: Some(path.clone()), path, role: "text", label: None, text: None,
                rect: [r.x, r.y, r.w, r.h], target: [t.x, t.y, t.w, t.h],
                opacity: f.source_alpha, scale: 1.0, blur: 0.0, radius: 0.0, fill: Color::TRANSPARENT, border_color: None, text_color: None, text_style: None,
                inset: [0.0; 4], text_size: None, wrap: false, truncate: false, text_cut: false, clips: false, scroll_axes: [false, false], raised: false,
                hover: 0.0, press: 0.0, moving: true, parent: None,
                visible: [r.x.max(c[0]), r.y.max(c[1]), (r.edges()[2].min(c[2]) - r.x.max(c[0])).max(0.0),
                    (r.edges()[3].min(c[3]) - r.y.max(c[1])).max(0.0)],
                revealing: true, in_growing_row: false, scrolling: false, scrolled: [0.0; 2], virtual_extent: false, live_intent: 0, caret: None, edit_range: None, drag_out: false,
                help: None, description: None, selected: None, level: None, expanded: None, item_selected: false, stuck: 0.0, material_fallback: false,
            });
        }
        self.link_elements(&mut elements);
        self.highlight_elements(&mut elements);
        elements
    }

    /// Index of the first live (non-exiting, mounted) node matching `key` within optional ancestor
    /// `scope`. Matches [`Node::named`] (key, full path, or keyed-ancestor suffix such as
    /// `slider/knob`) and `/`-separated path suffixes.
    pub(crate) fn live_named(&self, scope: Option<&str>, key: &str) -> Option<usize> {
        if key.is_empty() { return None; }
        let has_slash = key.contains('/');
        (0..self.tree.nodes.len()).find(|&i| {
            let n = &self.tree.nodes[i];
            let matched = n.named(key)
                || (has_slash && (n.path.strip_suffix(key).is_some_and(|head| head.ends_with('/')) || keyed_suffix(&n.path, key)));
            matched && !self.subtree_exiting(i) && !self.subtree_unmounted(i)
                && n.parent.is_none_or(|p| in_scope(&self.tree.nodes[p].path, scope))
        })
    }

    /// Centre of the first element whose key is `key`, or whose path ends with `key` when it
    /// contains a `/` (e.g. `slider/knob`), in points. Elements without keys between them don't
    /// count, so `slider/knob` finds a knob inside unkeyed rows inside the slider. Failing that,
    /// the first line box of the link (`El::link`) or highlight (`El::text_highlight`) keyed
    /// `key`. Exiting elements and their children are gone here; [`Runtime::presented`] still
    /// reads them.
    pub fn find(&self, key: &str) -> Option<(f32, f32)> {
        let Some(i) = self.live_named(None, key) else { return self.find_link(key).or_else(|| self.find_highlight(key)) };
        let r = self.paint.drawn.get(i).copied()?;
        Some((r[0] + r[2] * 0.5, r[1] + r[3] * 0.5))
    }
    /// Where a script event aimed at `key` lands, in points: the centre of the part of the element
    /// that is shown, its drawn box inside its ancestors' clips and the window. An element taller
    /// than its scroll viewport, or partly scrolled out, is hit where it can be seen and not at a
    /// centre that lies outside the window. `Err` names the key, its box and what is shown of the
    /// window when no part of the element is shown: an event sent there reaches nothing, and the
    /// script would end as if it had acted. A link or highlight keyed `key`, and an element in a
    /// rotated subtree, keep [`Runtime::find`]'s point.
    pub(crate) fn find_shown(&self, key: &str) -> Result<(f32, f32), String> {
        let Some(i) = self.live_named(None, key) else {
            return self.find_link(key).or_else(|| self.find_highlight(key)).ok_or_else(|| format!("no element with key {key:?}"));
        };
        let Some(r) = self.paint.drawn.get(i).copied() else { return Err(format!("no element with key {key:?}")) };
        if self.paint.inh_scratch.get(i).is_some_and(|h| h.turn != 0) {
            return Ok((r[0] + r[2] * 0.5, r[1] + r[3] * 0.5));
        }
        let edges = [r[0], r[1], r[0] + r[2], r[1] + r[3]];
        let clip = self.tree.nodes[i].parent.and_then(|p| self.paint.inh_scratch.get(p)).map_or(NO_CLIP, |h| h.clip).map(|v| v / self.env.scale);
        let window = [0.0, 0.0, self.viewport.0, self.viewport.1];
        let shown = intersect(intersect(edges, clip), window);
        // An element with no width or height of its own (an anchor) is shown when its line is.
        let across = shown[2] > shown[0] || (r[2] == 0.0 && shown[2] >= shown[0]);
        let down = shown[3] > shown[1] || (r[3] == 0.0 && shown[3] >= shown[1]);
        if across && down {
            return Ok(((shown[0] + shown[2]) * 0.5, (shown[1] + shown[3]) * 0.5));
        }
        let within = intersect(clip, window);
        Err(format!(
            "the element with key {key:?} is not shown: its box [{}, {}, {}, {}] (x, y, width, height) lies outside what is drawn of the window [{}, {}, {}, {}]; scroll it into view first, or aim at a point with \"x\" and \"y\"",
            r[0], r[1], r[2], r[3], within[0], within[1], (within[2] - within[0]).max(0.0), (within[3] - within[1]).max(0.0),
        ))
    }

    /// Index of the first node matching `selector` (key or key path) that is still presented in
    /// `self.scene` (exiting or not; unmounted nodes awaiting `prune_exited` are skipped so
    /// [`Self::presented`] and [`Self::presented_pose`] agree with [`Self::elements`] and paint).
    fn presented_index(&self, selector: &str) -> Option<usize> {
        self.tree.nodes.iter().enumerate().position(|(i, n)| {
            crate::flight::selects(selector, &n.path, n.key.as_deref())
                && self.scene.presented(n.id).is_some()
                && !self.subtree_unmounted(i)
        })
    }

    /// Drawn rect `[x, y, w, h]` in points and effective opacity (ancestors included) of the
    /// element keyed `key` (or key path such as `card-2/title`), exiting or not, as of the last
    /// frame.
    pub fn presented(&self, key: &str) -> Option<([f32; 4], f32)> {
        let i = self.presented_index(key)?;
        Some((*self.paint.drawn.get(i)?, self.paint.inh_scratch.get(i)?.opacity))
    }

    /// Authored key of the non-exiting presented focused element, or the focused inline link's
    /// id, matching `self.elements().into_iter().find(|e| e.focused && !e.exiting).and_then(|e| e.key)`
    /// without building the inspection list.
    pub fn presented_focus_key(&self) -> Option<&str> {
        let path = self.kbd.focus.path()?;
        let (i, n) = self.tree.nodes.iter().enumerate().find(|&(i, n)| {
            n.path == path && !self.subtree_exiting(i) && self.scene.presented(n.id).is_some()
        })?;
        if let Some(spec) = n.text.as_ref().filter(|t| !t.cold.links.is_empty()) {
            let k = self.edit.link_focus()?;
            let link = spec.cold.links.get(k)?;
            return (!self.link_boxes_for(i, k, self.drawn_box(i)).is_empty())
                .then_some(link.id.as_str());
        }
        n.key.as_deref()
    }

    /// UTF-8 byte offset in the text or text field named `key` (a key or key path, as
    /// [`Runtime::find`] resolves it) under window point (`x`, `y`) in points, as of the last
    /// presented frame, following `El::scale`, `El::rotate`, scrolling and single-line field
    /// horizontal scroll. Returns `None` when `key` is missing, exiting, not a text or text
    /// field, or secure, or when `x` or `y` is not finite. Running handlers can use
    /// [`Tx::text_offset_at`](crate::Tx::text_offset_at) without holding a `Runtime`.
    pub fn text_offset_at(&self, key: &str, x: f32, y: f32) -> Option<usize> {
        if !x.is_finite() || !y.is_finite() { return None; }
        let i = self.live_named(None, key)?;
        let n = &self.tree.nodes[i];
        if n.is_secure() { return None; }
        if let Some(inp) = n.input.as_deref() {
            let laid = n.text_layout.as_ref()?;
            let [tx, ty] = self.input_text_point(i, x, y)?;
            return Some(input_click::hit(&inp.edit, laid.view(), tx, ty, self.env.scale, inp.single_line).caret);
        }
        self.text_index_at(i, x, y)
    }

    /// The current tree's laid-out rect of `key` (a key or key path) in window points, after its
    /// viewports' scrolling, for [`Tx::read_layout`](crate::Tx::read_layout). It notes no layout
    /// read, so nothing re-runs when the rect moves.
    fn window_layout_now(&self, key: &str) -> Option<[f32; 4]> {
        if key.is_empty() { return None; }
        self.layout_of(&self.tree.nodes, None, &super::layout_read::window_key(key))
    }

    pub(super) fn run_pick_reads(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        let mut elements: Option<Vec<ElementInfo>> = None;
        for read in std::mem::take(&mut tx.pick_reads) {
            match read {
                crate::el::PickRead::Element { within, x, y, pick, f } => {
                    let picked = self.element_at_within(within.as_deref(), x, y, pick);
                    if let Some(f) = f.take() {
                        f(app.states(), picked, tx);
                        self.commit.dirty = true;
                    }
                }
                crate::el::PickRead::TextOffset { key, x, y, f } => {
                    if !self.paint.text_rows.is_empty() { self.fill_text_near(); }
                    let offset = self.text_offset_at(&key, x, y);
                    if let Some(f) = f.take() {
                        f(app.states(), offset, tx);
                        self.commit.dirty = true;
                    }
                }
                crate::el::PickRead::TextLine { key, byte, f } => {
                    let line = self.text_line(&key, byte);
                    if let Some(f) = f.take() {
                        f(app.states(), line, tx);
                        self.commit.dirty = true;
                    }
                }
                crate::el::PickRead::Elements { f } => {
                    if let Some(f) = f.take() {
                        let list = elements.get_or_insert_with(|| self.elements());
                        f(app.states(), list, tx);
                        self.commit.dirty = true;
                    }
                }
                crate::el::PickRead::Layout { key, f } => {
                    if let Some(f) = f.take() {
                        let rect = self.window_layout_now(&key);
                        f(app.states(), rect, tx);
                        self.commit.dirty = true;
                    }
                }
                crate::el::PickRead::Presented { key, f } => {
                    if let Some(f) = f.take() {
                        let drawn = self.presented(&key);
                        f(app.states(), drawn, tx);
                        self.commit.dirty = true;
                    }
                }
            }
        }
        // A callback may ask again, but cannot keep this dispatch running indefinitely.
        self.frame.pending.pick_reads.append(&mut tx.pick_reads);
    }

    /// The presented rotation, scale and shadows of the element keyed `key`, exiting or not, as
    /// of the last frame, and whether any spring still moves them. It reads the springs on
    /// demand: nothing runs per frame for it. This is a driver/headless read: running view
    /// functions and handlers do not hold a `Runtime`.
    pub fn presented_pose(&self, key: &str) -> Option<Pose> {
        let i = self.presented_index(key)?;
        let n = &self.tree.nodes[i];
        let store = self.scene.store();
        let shadows = n.vis.shadows.iter().enumerate().map(|(k, sh)| {
            let sh = self.shadow_at(n.id, k, sh);
            match P_SHADOWS.get(k) {
                Some(&p) => crate::el::Shadow { color: self.color(n.id, p, sh.color), ..sh },
                None => sh,
            }
        }).collect();
        let moving = self.rect_springing(i)
            || anchor_ancestors(&self.tree.nodes, i).any(|p| {
                let id = self.tree.nodes[p].id;
                paint_pose_springing(store, id)
                    || (!self.reduced_motion && self.motion.loop_active(id, store.node_time(id)))
            });
        Some(Pose {
            rotation: self.scene.presented(n.id).map_or(n.vis.rotate, |p| p.transform.rotate),
            scale: self.paint.inh_scratch.get(i)?.scale,
            shadows,
            moving,
        })
    }

    /// Whether the root's own fill is dark, when it has an opaque one (headless `--keys` caps
    /// follow it, so they flip when the app changes theme itself).
    pub(crate) fn backdrop_dark(&self) -> Option<bool> {
        if self.transparent { return None; }
        let n = self.tree.nodes.get(self.tree.app_root())?;
        let c = self.color(n.id, P_FILL, n.vis.fill);
        (c.a > 0.5).then(|| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b < 0.25)
    }
}

/// Whether `path`'s keyed segments end with `key`'s: `slider/knob` matches
/// `form/#2/slider/#0/knob`, since the `#n` segments stand for elements without keys.
fn keyed_suffix(path: &str, key: &str) -> bool {
    element_path::keyed_suffix_matches(path, key)
}

#[cfg(test)]
thread_local! {
    static ELEMENTS_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn count_elements_builds<R>(f: impl FnOnce() -> R) -> (R, usize) {
    ELEMENTS_BUILDS.with(|c| c.set(0));
    let out = f();
    let count = ELEMENTS_BUILDS.with(|c| c.get());
    (out, count)
}
