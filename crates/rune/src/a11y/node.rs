//! One element as an AccessKit node: its role, name, value, states, actions and bounds, and
//! the fingerprint of everything those read, which tells an update whether to build it again.

use std::hash::{Hash, Hasher};

use accesskit::{Action as Ak, Affine, Live, Node as AkNode, Role, Toggled};

use super::roles::{self, RoleCtx};
use super::tree::{Walk, element_id, hash_str, mix, rect};
use crate::el::Checked;
use crate::runtime::RuntimeCore;

/// Text named from an element's content stops here.
const NAME_MAX: usize = 400;

/// Which nodes stay visible in the accessibility tree and web mirror while an `El::modal` is
/// open. Active modal members and (for anchored popup modals such as `menu` or `listbox`)
/// their anchor trigger subtrees are in `included`; `outer_modal` marks enclosing outer modals
/// that contain a kept trigger; `ancestor` marks all strict ancestors of `included` nodes.
pub(crate) struct A11yModalMask {
    pub(crate) included: Vec<bool>,
    pub(crate) outer_modal: Vec<bool>,
    pub(crate) ancestor: Vec<bool>,
}

impl A11yModalMask {
    #[inline]
    pub(crate) fn visible(&self, i: usize) -> bool {
        self.included[i] || self.ancestor[i]
    }
}

impl RuntimeCore {
    /// Node `i`'s role in the tree, or `None` when it joins its parent.
    pub(crate) fn a11y_role(&self, i: usize) -> Option<Role> {
        let n = &self.tree.nodes[i];
        let handlers = n.handlers.get();
        let input = n.input.as_deref();
        let text = n.text.as_deref().filter(|_| input.is_none());
        // A group that takes its own keys and no press (a game's surface: arrows, Space) is
        // no button even when it steers by drag: Return or Space doesn't press it. As the web
        // mirror has it, a labelled one stays a group.
        let owns_keys = n.role == "group" && n.focus.key.is_some() && handlers.is_none_or(|h| h.on_press.is_none() && h.on_double_press.is_none());
        let cx = RoleCtx {
            pressable: handlers.is_some_and(|h| h.has_primary()) && !owns_keys,
            focusable: self.takes_focus(n),
            input: input.is_some(),
            multiline: input.is_some_and(|i| !i.single_line),
            secure: n.is_secure(),
            scrolls: n.scroll.is_some() && self.scroll.scrolls.contains_key(&n.id),
            modal: handlers.is_some_and(|h| h.modal.is_some()),
            default_button: n.focus.default,
            labelled: n.label.as_deref().is_some_and(|l| !l.trim().is_empty()),
            keyed: n.key.is_some(),
            has_text: text.is_some_and(|t| !t.source_text().trim().is_empty() || t.redacted),
            image: n.vis.pixels.is_some() || n.vis.surface.is_some(),
        };
        // A control the keyboard reaches (or that declares custom accessibility actions) is always
        // in the tree, whatever its role says; an explicit `role("group")` with a role description
        // also enters the tree as a group.
        let has_custom_actions = handlers.is_some_and(|h| h.custom_a11y_actions.as_deref().is_some_and(|a| !a.is_empty()));
        roles::map(n.role, &cx).or_else(|| {
            let explicit_group_rd = n.role == "group"
                && n.a11y.explicit_group
                && n.a11y.more.as_deref()
                    .and_then(|m| m.role_description.as_deref())
                    .is_some_and(|s| !s.trim().is_empty());
            (input.is_some() || self.can_focus(i) || explicit_group_rd || has_custom_actions).then_some(Role::Group)
        })
    }

    /// Node `i`'s effective role description (`El::role_description` / `El::role_description_static`),
    /// or `None` when unset, empty/whitespace-only, or the element has no valid accessible role
    /// (WAI-ARIA 1.2 `aria-roledescription`).
    pub(crate) fn a11y_role_description(&self, i: usize) -> Option<&str> {
        let n = &self.tree.nodes[i];
        let desc = n.a11y.more.as_deref()?.role_description.as_deref()?;
        if desc.trim().is_empty() {
            return None;
        }
        let role = self.a11y_role(i)?;
        if matches!(role, Role::GenericContainer) {
            return None;
        }
        let has_role = if n.role == "group" {
            n.a11y.explicit_group || !matches!(role, Role::Group | Role::ScrollView)
        } else {
            n.input.is_some() || roles::spec(n.role).is_some()
        };
        has_role.then_some(desc)
    }

    /// A hash of everything [`Self::a11y_node`] and its text's lines and links read of node
    /// `i`, besides its children and the text inside it ([`Self::a11y_content_prints`]).
    pub(super) fn a11y_print(&self, i: usize, role: Role, w: &Walk) -> u64 {
        let n = &self.tree.nodes[i];
        let mut h = rustc_hash::FxHasher::default();
        (role as u8).hash(&mut h);
        n.role.hash(&mut h);
        n.key.hash(&mut h);
        n.label.hash(&mut h);
        n.help.hash(&mut h);
        n.a11y.level.hash(&mut h);
        n.a11y.expanded.hash(&mut h);
        n.a11y.checked.hash(&mut h);
        self.node_disabled(i).hash(&mut h);
        self.has_image_menu(i).hash(&mut h);
        n.a11y.live.hash(&mut h);
        n.a11y.explicit_group.hash(&mut h);
        if let Some(m) = n.a11y.more.as_deref() {
            (&m.description, &m.value, &m.set, &m.shortcut, m.invalid, m.read_only, m.sorted, m.busy,
                m.row_count, m.column_count, m.row_index, m.column_index).hash(&mut h);
            (&m.role_description, m.multiselectable, m.required, m.has_popup, m.orientation, m.auto_complete, m.current, m.list_start).hash(&mut h);
            [m.labelled_by.as_deref(), m.described_by.as_deref()].map(|k| k.and_then(|k| self.a11y_keyed(i, k)).map(|j| self.a11y_words(j))).hash(&mut h);
            if let Some(r) = m.range { [r.value, r.min, r.max, r.step.unwrap_or(f64::NAN)].map(f64::to_bits).hash(&mut h); }
            self.a11y_active_descendant(i).map(|j| self.tree.nodes[j].id.0).hash(&mut h);
            m.controls.as_deref().and_then(|k| self.a11y_related_target(i, k)).map(|j| self.tree.nodes[j].id.0).hash(&mut h);
            m.owns.as_deref().and_then(|k| self.a11y_related_target(i, k)).map(|j| self.tree.nodes[j].id.0).hash(&mut h);
        }
        n.text_cut(self.env.scale).hash(&mut h);
        let secure = n.is_secure();
        let long_field = self.a11y_uses_long_field(i);
        (secure, long_field).hash(&mut h);
        let long_key = long_field.then(|| {
            n.text.as_deref().and_then(|t| t.shape.map(|(_, k)| k))
                .or_else(|| n.text_layout.as_ref().and_then(crate::text::TextLayout::content_key))
        }).flatten();
        if let Some(t) = n.text.as_deref() {
            if !secure {
                match long_key {
                    Some(k) => (k, t.content.len()).hash(&mut h),
                    None => {
                        t.content.hash(&mut h);
                        if t.source.is_some() { t.source_text().hash(&mut h); }
                    }
                }
            }
            (
                t.redacted,
                t.selectable,
                t.flags.has(crate::el::TextFlags::NO_LINK_MENU),
                t.cold.on_link.is_some(),
                t.cold.on_link_secondary.is_some(),
                !t.cold.links.is_empty() && self.ancestor_has_secondary_press(i),
                t.cold.links.len(),
            ).hash(&mut h);
            for l in &t.cold.links { (l.start, l.end, &l.id, &l.href, l.role, &l.label).hash(&mut h); }
            for p in &t.cold.pills {
                if let Some(b) = p.bare.as_deref().filter(|b| b.label.is_some()) { (p.start, &b.label, &b.help).hash(&mut h); }
            }
        }
        // A text's lines come from its layout and where it scrolled; a long field's drawn lines
        // live under `lines_box_id`, whose own fingerprint tracks its layout and scroll band.
        if !long_field && !secure {
            n.text_layout.as_ref().map(crate::text::TextLayout::address).hash(&mut h);
        }
        if !secure {
            n.elided.map(|e| (e.head, e.tail)).hash(&mut h);
        }
        if let Some(input) = n.input.as_deref() {
            let e = &input.edit;
            if !secure {
                if long_key.is_none() || e.preedit().is_some() {
                    if long_field {
                        // During IME composition on a long field, `t.content` is the display string and
                        // `long_key` already hashed it; only hash `e.text.len()` + preedit metadata.
                        e.text.len().hash(&mut h);
                    } else {
                        e.text.hash(&mut h);
                    }
                }
                (e.anchor, e.focus, e.preedit().map(|p| (p.0.len(), p.1))).hash(&mut h);
                if !long_field {
                    self.edit.input_scroll.get(&n.path).map(|v| v.to_bits()).hash(&mut h);
                }
            }
            (e.focused, &input.placeholder, input.single_line, input.access.is_some(), self.is_node_read_only(i)).hash(&mut h);
        }
        let handlers = n.handlers.get();
        (handlers.is_some_and(|h| h.on_press.is_some()), handlers.is_some_and(|h| h.on_secondary_press.is_some()),
            handlers.is_some_and(|h| h.modal.is_some()), handlers.is_some_and(|h| h.has_primary())).hash(&mut h);
        if let Some(actions) = handlers.and_then(|h| h.custom_a11y_actions.as_deref()) {
            actions.len().hash(&mut h);
            for action in actions {
                (&action.id, &action.label).hash(&mut h);
            }
        } else {
            0usize.hash(&mut h);
        }
        (n.focus.selected, n.focus.step.is_some(), n.focus.set_value.is_some(), n.focus.default, n.focus.caret_browsing, self.focus_group(i).is_some()).hash(&mut h);
        if n.input.is_none() && self.is_caret_browsing_node(n) {
            self.a11y_caret_selection(i)
                .map(|s| (s.anchor.node.0, s.anchor.character_index, s.focus.node.0, s.focus.character_index))
                .hash(&mut h);
        }
        (self.allowed(i), self.can_focus(i), n.inert, w.delegated_a11y && roles::composite_item(role)).hash(&mut h);
        if let Some(st) = self.scroll.scrolls.get(&n.id).filter(|_| n.scroll.is_some()) {
            [st.offset.to_bits(), st.max.to_bits(), st.x.offset.to_bits(), st.x.max.to_bits()].hash(&mut h);
            [st.drawn_x, st.drawn.y()].map(f32::to_bits).hash(&mut h);
        }
        // Its box in its viewport's content space, which scrolling leaves alone.
        let r = rect(self.paint.drawn[i], w.shift);
        [r.x0, r.y0, r.x1, r.y1].map(f64::to_bits).hash(&mut h);
        w.scrolled.hash(&mut h);
        self.env.scale.to_bits().hash(&mut h);
        self.paint.drawn_scale.get(i).map(|v| v.to_bits()).hash(&mut h);
        (n.vis.pixels.is_some(), n.vis.surface.is_some()).hash(&mut h);
        // A grid's rows, each kept while unchanged.
        if let Some(g) = n.vis.grid.as_deref() {
            g.rows.iter().for_each(|row| (std::rc::Rc::as_ptr(row) as usize).hash(&mut h));
            g.caret.hash(&mut h);
        }
        h.finish()
    }

    /// Signature of a live region's spoken content and politeness (`Polite` or `Assertive`),
    /// independent of layout geometry. `None` when the element is not an active live root.
    pub(super) fn a11y_live_sig(&self, i: usize, content_print: u64) -> Option<u64> {
        let n = &self.tree.nodes[i];
        let live = n.a11y.live.or_else(|| roles::default_live(n.role))?;
        if live == crate::LiveRegion::Off { return None; }
        let mut h = rustc_hash::FxHasher::default();
        live.hash(&mut h);
        let secure = n.is_secure();
        secure.hash(&mut h);
        n.label.hash(&mut h);
        if let Some(m) = n.a11y.more.as_deref() {
            m.value.hash(&mut h);
            m.labelled_by.as_deref().and_then(|k| self.a11y_keyed(i, k)).map(|j| self.a11y_words(j)).hash(&mut h);
            if let Some(r) = m.range { r.value.to_bits().hash(&mut h); }
        }
        if !secure {
            if let Some(input) = n.input.as_deref() {
                input.edit.text.hash(&mut h);
            } else if let Some(t) = n.text.as_deref() {
                t.source_text().hash(&mut h);
            }
        }
        content_print.hash(&mut h);
        Some(h.finish())
    }

    /// Per node, a hash of the text its name could be taken from (`a11y_content_name`): its
    /// own text or label, and its descendants', folded bottom-up. Zero for nodes whose role
    /// doesn't read it.
    pub(super) fn a11y_content_prints(&self) -> Vec<u64> {
        let mut sub = vec![0u64; self.tree.nodes.len()];
        for j in (0..self.tree.nodes.len()).rev() {
            let n = &self.tree.nodes[j];
            let own = if n.a11y.hidden || n.role == "list-marker" || n.vis.opacity <= 0.01 || n.is_secure() { 0 }
                else if let Some(l) = n.label.as_deref() { hash_str(l) }
                else { n.text.as_deref().filter(|_| n.input.is_none()).map_or(0, |t| hash_str(t.source_text())) };
            sub[j] = mix(sub[j], own);
            if let Some(p) = n.parent { sub[p] = mix(sub[p], sub[j]); }
        }
        sub
    }

    /// Node `i` as an AccessKit node of `role`, without its children.
    pub(super) fn a11y_node(&self, i: usize, role: Role, w: &Walk) -> AkNode {
        let n = &self.tree.nodes[i];
        let handlers = n.handlers.get();
        let input = n.input.as_deref();
        let secure = n.is_secure();
        let text = n.text.as_deref().filter(|_| input.is_none());
        let scroll = self.scroll.scrolls.get(&n.id).filter(|_| n.scroll.is_some());
        let modal = handlers.is_some_and(|h| h.modal.is_some());
        let focusable = self.can_focus(i);
        let mut node = AkNode::new(role);
        // Disabled is what the app says (or an inert subtree); an element that only contains an
        // open modal isn't. Actions go only where input may.
        let enabled = !self.node_disabled(i) && !n.inert;
        let actionable = enabled && self.allowed(i);
        if let Some(key) = &n.key { node.set_author_id(key.as_str()); }
        let more = n.a11y.more.as_deref();
        // `El::labelled_by` and `El::described_by` read the words of the element they name, so
        // the relation holds wherever that element is, in or out of the tree sent.
        let related = |k: Option<&str>| k.and_then(|k| self.a11y_keyed(i, k)).map(|j| self.a11y_words(j)).filter(|w| !w.is_empty());
        let labelled = related(more.and_then(|m| m.labelled_by.as_deref()));
        let label = n.label.as_deref().filter(|l| !l.trim().is_empty()).or(labelled.as_deref());
        if let Some(label) = label { node.set_label(label); }
        // A secret's text never reaches the tree; its label must describe it instead.
        match (text, role) {
            (Some(_), _) if secure => {}
            (Some(t), Role::Label) => node.set_value(t.source_text()),
            (Some(t), _) if label.is_none() => node.set_label(t.source_text()),
            _ => {}
        }
        if label.is_none() && text.is_none() && roles::named_from_content(role) {
            let named = self.a11y_content_name(i);
            if !named.is_empty() { node.set_label(named); }
        }
        let described = related(more.and_then(|m| m.described_by.as_deref()));
        // A tooltip describes the element unless it only repeats its name, or shows only while
        // the text is cut short and it isn't.
        let name = label.or(text.filter(|_| !secure).map(|t| t.source_text()));
        let help = n.help.as_deref()
            .filter(|_| !n.help_cut || n.text_cut(self.env.scale))
            .filter(|h| name.is_none_or(|name| !h.trim().starts_with(name.trim())))
            .map(crate::el::help_words);
        match more.and_then(|m| m.description.as_deref()).or(described.as_deref()).or(help.as_deref()) {
            Some(d) if !d.is_empty() => node.set_description(d),
            _ => {}
        }
        if let Some(rd) = self.a11y_role_description(i) {
            node.set_role_description(rd);
        }
        if let Some(m) = more {
            if m.invalid { node.set_invalid(accesskit::Invalid::True); }
            if m.read_only { node.set_read_only(); }
            match m.sorted {
                Some(true) => node.set_sort_direction(accesskit::SortDirection::Ascending),
                Some(false) => node.set_sort_direction(accesskit::SortDirection::Descending),
                None => {}
            }
            if let Some(j) = self.a11y_active_descendant(i) { node.set_active_descendant(element_id(self.tree.nodes[j].id)); }
            if m.busy { node.set_busy(); }
            if m.multiselectable { node.set_multiselectable(); }
            if m.required { node.set_required(); }
            if let Some(hp) = m.has_popup {
                node.set_has_popup(match hp {
                    crate::HasPopup::Menu => accesskit::HasPopup::Menu,
                    crate::HasPopup::Listbox => accesskit::HasPopup::Listbox,
                    crate::HasPopup::Tree => accesskit::HasPopup::Tree,
                    crate::HasPopup::Grid => accesskit::HasPopup::Grid,
                    crate::HasPopup::Dialog => accesskit::HasPopup::Dialog,
                });
            }
            if let Some(o) = m.orientation {
                node.set_orientation(match o {
                    crate::Orientation::Horizontal => accesskit::Orientation::Horizontal,
                    crate::Orientation::Vertical => accesskit::Orientation::Vertical,
                });
            }
            if let Some(ac) = m.auto_complete {
                node.set_auto_complete(match ac {
                    crate::AutoComplete::Inline => accesskit::AutoComplete::Inline,
                    crate::AutoComplete::List => accesskit::AutoComplete::List,
                    crate::AutoComplete::Both => accesskit::AutoComplete::Both,
                });
            }
            if let Some(cur) = m.current {
                node.set_aria_current(match cur {
                    crate::Current::Page => accesskit::AriaCurrent::Page,
                    crate::Current::Step => accesskit::AriaCurrent::Step,
                    crate::Current::Location => accesskit::AriaCurrent::Location,
                    crate::Current::Date => accesskit::AriaCurrent::Date,
                    crate::Current::Time => accesskit::AriaCurrent::Time,
                    crate::Current::True => accesskit::AriaCurrent::True,
                });
            }
            if let Some(j) = m.controls.as_deref().and_then(|k| self.a11y_related_target(i, k)) {
                node.set_controls(vec![element_id(self.tree.nodes[j].id)]);
            }
            if let Some(j) = m.owns.as_deref().and_then(|k| self.a11y_related_target(i, k)) {
                node.set_owns(vec![element_id(self.tree.nodes[j].id)]);
            }
        }
        if let Some(v) = more.and_then(|m| m.value.as_deref()) { node.set_value(v); }
        if let Some(r) = more.and_then(|m| m.range) {
            node.set_numeric_value(r.value);
            node.set_min_numeric_value(r.min);
            node.set_max_numeric_value(r.max);
            if let Some(step) = r.step { node.set_numeric_value_step(step); }
        }
        if let Some(keys) = more.and_then(|m| m.shortcut.as_deref()) { node.set_keyboard_shortcut(keys); }
        if let Some((pos, _)) = more.and_then(|m| m.set) { node.set_position_in_set(pos.saturating_sub(1)); }
        if let Some(m) = more {
            if let Some(rows) = m.row_count { node.set_row_count(rows); }
            if let Some(cols) = m.column_count { node.set_column_count(cols); }
            if let Some(r) = m.row_index { node.set_row_index(r.saturating_sub(1)); }
            if let Some(c) = m.column_index { node.set_column_index(c.saturating_sub(1)); }
        }
        if let Some(level) = n.a11y.level { node.set_level(level.saturating_sub(1) as usize); }
        if let Some(open) = n.a11y.expanded { node.set_expanded(open); }
        let checked = n.a11y.checked.or_else(|| (role == Role::RadioButton).then(|| if n.focus.selected { Checked::True } else { Checked::False }));
        if let Some(c) = checked {
            node.set_toggled(match c { Checked::False => Toggled::False, Checked::True => Toggled::True, Checked::Mixed => Toggled::Mixed });
        }
        // An item of a group reports whether it's the selected one; elsewhere only a selected
        // item says so (a tree whose container keeps focus).
        if roles::selectable_item(role) && (n.focus.selected || self.focus_group(i).is_some()) { node.set_selected(n.focus.selected); }
        let live = n.a11y.live.or_else(|| roles::default_live(n.role));
        match live {
            Some(crate::LiveRegion::Polite | crate::LiveRegion::Assertive)
                if self.a11y_live_muted(element_id(n.id)) => node.set_live(Live::Off),
            Some(crate::LiveRegion::Polite) => node.set_live(Live::Polite),
            Some(crate::LiveRegion::Assertive) => node.set_live(Live::Assertive),
            Some(crate::LiveRegion::Off) => node.set_live(Live::Off),
            None => {}
        }
        // Per WAI-ARIA 1.2 and Chromium Blink (`AXObject::IsModal`), only `dialog` and
        // `alertdialog` carry the modal state; popup widgets (`menu`, `listbox`, `tree`,
        // `grid`, `treegrid`) do not.
        if modal && matches!(role, Role::Dialog | Role::AlertDialog) { node.set_modal(); }
        if !enabled { node.set_disabled(); }
        // Bounds: a scroll viewport's own box sits in the space its offset moves.
        let mut shift = w.shift;
        if let Some(st) = scroll {
            let [x, y] = self.content_to_window(i, [st.drawn_x, st.drawn.y()]);
            shift = [shift[0] + x, shift[1] + y];
            node.set_transform(Affine::translate((-f64::from(x), -f64::from(y))));
            node.set_clips_children();
            node.set_scroll_y(st.offset);
            node.set_scroll_y_min(0.0);
            node.set_scroll_y_max(st.max.max(0.0));
            if st.x.max > 0.0 {
                node.set_scroll_x(st.x.offset);
                node.set_scroll_x_min(0.0);
                node.set_scroll_x_max(st.x.max);
            }
        }
        node.set_bounds(rect(self.paint.drawn[i], shift));
        // A disabled-but-focusable control (`El::aria_disabled`) can still take assistive focus
        // while activation actions stay gated behind `actionable`.
        if focusable && !n.inert && self.allowed(i) {
            node.add_action(Ak::Focus);
        }
        // What a pointer or keyboard user can do to it, routed to the same handlers.
        if actionable {
            if handlers.is_some_and(|h| h.on_press.is_some()) || input.is_some() || (w.delegated_a11y && roles::composite_item(role)) { node.add_action(Ak::Click); }
            if handlers.is_some_and(|h| h.on_secondary_press.is_some()) || self.has_image_menu(i) { node.add_action(Ak::ShowContextMenu); }
            if let Some(actions) = handlers.and_then(|h| h.custom_a11y_actions.as_deref()).filter(|a| !a.is_empty()) {
                let ids = super::actions::resolve_custom_action_ids(actions);
                node.add_action(Ak::CustomAction);
                node.set_custom_actions(
                    actions
                        .iter()
                        .zip(ids)
                        .map(|(a, id)| accesskit::CustomAction { id, description: a.label.clone() })
                        .collect::<Vec<_>>(),
                );
            }
            if n.a11y.expanded.is_some() && handlers.is_some_and(|h| h.on_press.is_some()) {
                node.add_action(if n.a11y.expanded == Some(true) { Ak::Collapse } else { Ak::Expand });
            }
            if n.focus.step.is_some() {
                node.add_action(Ak::Increment);
                node.add_action(Ak::Decrement);
                if more.and_then(|m| m.range).is_some_and(|r| r.usable_step().is_some()) { node.add_action(Ak::SetValue); }
            }
            if n.focus.set_value.is_some() && more.and_then(|m| m.range).is_none_or(|r| r.usable_bounds().is_some()) {
                node.add_action(Ak::SetValue);
            }
            if input.is_some_and(|inp| inp.access.is_some()) {
                if !self.is_node_read_only(i) {
                    node.add_action(Ak::SetValue);
                    node.add_action(Ak::ReplaceSelectedText);
                }
                if !secure { node.add_action(Ak::SetTextSelection); }
            } else if self.is_caret_browsing_node(n) && !secure {
                node.add_action(Ak::SetTextSelection);
            }
            if scroll.is_some() {
                node.add_action(Ak::ScrollUp);
                node.add_action(Ak::ScrollDown);
                node.add_action(Ak::SetScrollOffset);
                if scroll.is_some_and(|st| st.x.max > 0.0) {
                    node.add_action(Ak::ScrollLeft);
                    node.add_action(Ak::ScrollRight);
                }
            }
        }
        if w.scrolled && self.allowed(i) { node.add_action(Ak::ScrollIntoView); }
        if let Some(input) = input {
            self.a11y_input(i, input, &mut node);
        } else if self.is_caret_browsing_node(n) {
            node.set_read_only();
            if !secure && let Some(sel) = self.a11y_caret_selection(i) {
                node.set_text_selection(sel);
            }
        }
        node
    }

    /// The element node `i`'s `El::active_descendant` names, if it is in the tree.
    pub(crate) fn a11y_active_descendant(&self, i: usize) -> Option<usize> {
        let key = self.tree.nodes[i].a11y.more.as_deref()?.active.as_deref()?;
        // Its own rows first: a row id may also key something elsewhere.
        let inside = |j: &usize| std::iter::successors(self.tree.nodes[*j].parent, |&p| self.tree.nodes[p].parent).any(|p| p == i);
        let own = (i + 1..self.tree.nodes.len())
            .take_while(inside)
            .find(|&j| self.tree.nodes[j].named(key) && !self.subtree_exiting(j));
        own.or_else(|| self.live_named(None, key))
    }

    /// The live, accessible element node `i`'s `El::controls` or `El::owns` names: nearest
    /// scope first (own subtree, then enclosing ancestors), falling back to root-attached
    /// overlays and keyed paths via `live_named`. Never resolves to `i` itself or to a node
    /// omitted from the accessibility tree.
    pub(crate) fn a11y_related_target(&self, i: usize, key: &str) -> Option<usize> {
        if key.is_empty() { return None; }
        let usable = |j: usize| j != i && !self.subtree_exiting(j) && self.a11y_in_tree(j);
        let mut scope = Some(i);
        while let Some(s) = scope {
            let inside = |j: &usize| std::iter::successors(self.tree.nodes[*j].parent, |&p| self.tree.nodes[p].parent).any(|p| p == s);
            if let Some(j) = (s + 1..self.tree.nodes.len())
                .take_while(inside)
                .find(|&j| self.tree.nodes[j].named(key) && usable(j))
            {
                return Some(j);
            }
            scope = self.tree.nodes[s].parent;
        }
        self.live_named(None, key).filter(|&j| usable(j))
    }

    /// Whether modal node `i` is an anchored popup widget (`menu`, `listbox`, `tree`,
    /// `grid`, or `treegrid`) that keeps its trigger readable in the accessibility tree.
    fn is_anchored_popup_modal(&self, i: usize) -> bool {
        self.has_anchor(i) && self.a11y_role(i).is_some_and(roles::popup_modal)
    }

    /// Accessibility visibility mask while an `El::modal` is active, or `None` when no modal
    /// is open. Modal dialogs keep only their members and ancestors; anchored popup modals
    /// (`menu`, `listbox`, `tree`, `grid`, `treegrid`) also keep their anchor trigger subtree
    /// and any enclosing outer modal readable (inert to actions).
    pub(crate) fn a11y_modal_mask(&self) -> Option<A11yModalMask> {
        let active = self.active_modal()?;
        let n_nodes = self.tree.nodes.len();
        let mut included = (0..n_nodes).map(|i| self.modal_member(i)).collect::<Vec<_>>();
        let mut outer_modal = vec![false; n_nodes];
        if self.is_anchored_popup_modal(active) {
            let mut queue = vec![active];
            let mut followed = vec![false; n_nodes];
            let mut steps = 0usize;
            while let Some(overlay) = queue.pop() {
                if followed[overlay] || steps >= crate::runtime::anchor::MAX_ANCHOR_CHAIN {
                    continue;
                }
                followed[overlay] = true;
                steps += 1;
                let Some(anchor) = self.anchor_of(overlay) else { continue };
                let end = self.subtree_end(anchor);
                for slot in &mut included[anchor..end] {
                    *slot = true;
                }
                let mut p = self.tree.nodes[anchor].parent;
                while let Some(a) = p {
                    let an = &self.tree.nodes[a];
                    let is_modal = an.handlers.get().is_some_and(|h| h.modal.is_some());
                    if is_modal && !self.modal_member(a) {
                        outer_modal[a] = true;
                    }
                    if self.has_anchor(a)
                        && !followed[a]
                        && (!is_modal || self.is_anchored_popup_modal(a))
                    {
                        queue.push(a);
                    }
                    p = an.parent;
                }
            }
        }
        let mut ancestor = vec![false; n_nodes];
        for (i, &inc) in included.iter().enumerate() {
            if !inc { continue; }
            let mut p = self.tree.nodes[i].parent;
            while let Some(j) = p.filter(|&j| !ancestor[j]) {
                ancestor[j] = true;
                p = self.tree.nodes[j].parent;
            }
        }
        Some(A11yModalMask { included, outer_modal, ancestor })
    }

    /// Whether node `j` appears in the accessibility tree (not hidden, exiting, unmounted,
    /// transparent, inside a secure ancestor, occluded by a modal, or role-omitted).
    pub(super) fn a11y_in_tree(&self, j: usize) -> bool {
        let n = &self.tree.nodes[j];
        if n.is_moved_stub() || n.a11y.hidden || n.vis.opacity <= 0.01 || self.is_exiting(n.id) || !self.scene.contains(n.id) {
            return false;
        }
        let mut p = n.parent;
        while let Some(a) = p {
            let an = &self.tree.nodes[a];
            if an.is_moved_stub() || an.a11y.hidden || an.vis.opacity <= 0.01 || an.is_secure() || self.is_exiting(an.id) || !self.scene.contains(an.id) {
                return false;
            }
            p = an.parent;
        }
        if self.active_modal().is_some()
            && !self.modal_member(j)
            && !self.a11y_modal_mask().is_some_and(|m| m.visible(j))
        {
            return false;
        }
        self.a11y_role(j).is_some()
    }

    /// Debug builds: warns once per site about a control a screen reader would say only by its
    /// role ("button", "edit"): focusable, shown, and named by nothing (no `El::label`, no
    /// `El::labelled_by` that finds words, no text of its own or, for a role named from its
    /// content, inside it; a text field's placeholder isn't a name). Runs after a commit, every
    /// sixteenth, so it costs a debug build little and a release build nothing.
    /// `RUNE_A11Y_WARN=0` silences it.
    #[cfg(debug_assertions)]
    pub(crate) fn a11y_warn_nameless(&self) {
        use std::{collections::HashSet, sync::{Mutex, OnceLock}};
        static ON: OnceLock<bool> = OnceLock::new();
        static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
        if !*ON.get_or_init(|| std::env::var_os("RUNE_A11Y_WARN").is_none_or(|v| v != "0")) { return; }
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let field = n.input.is_some();
            if n.path.is_empty() || n.a11y.hidden || n.inert || !(field || roles::warn_nameless(n.role)) || !self.can_focus(i) { continue; }
            if n.label.as_deref().is_some_and(|l| !l.trim().is_empty()) { continue; }
            let more = n.a11y.more.as_deref();
            if more.and_then(|m| m.labelled_by.as_deref()).and_then(|k| self.a11y_keyed(i, k)).is_some_and(|j| !self.a11y_words(j).is_empty()) { continue; }
            if !field && (n.text.as_deref().is_some_and(|t| !t.source_text().trim().is_empty()) || !self.a11y_content_name(i).is_empty()) { continue; }
            let site: String = n.path.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
            if !WARNED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(site) { continue; }
            let what = if field { "text field" } else { n.role };
            eprintln!("rune: a focusable {what} has no name a screen reader can say (give it El::label or El::labelled_by): {} (debug builds only; RUNE_A11Y_WARN=0 silences)", n.path);
        }
    }
    /// The element keyed `key` nearest node `i`, which isn't leaving: the first inside its
    /// parent, else inside its grandparent, and so on out, as a component's own `"title"` is
    /// the one a control inside it means. What `El::labelled_by` and `El::described_by` name.
    pub(crate) fn a11y_keyed(&self, i: usize, key: &str) -> Option<usize> {
        let mut scope = self.tree.nodes[i].parent;
        while let Some(s) = scope {
            let inside = |j: &usize| std::iter::successors(self.tree.nodes[*j].parent, |&p| self.tree.nodes[p].parent).any(|p| p == s);
            if let Some(j) = (s + 1..self.tree.nodes.len())
                .take_while(inside)
                .find(|&j| j != i && self.tree.nodes[j].named(key) && !self.subtree_exiting(j))
            {
                return Some(j);
            }
            scope = self.tree.nodes[s].parent;
        }
        None
    }

    /// Node `j`'s words as a name: its label, else its text, else the text inside it.
    pub(crate) fn a11y_words(&self, j: usize) -> String {
        let n = &self.tree.nodes[j];
        if n.is_secure() { return String::new(); }
        n.label.as_deref().filter(|l| !l.trim().is_empty()).map(str::to_owned)
            .or_else(|| n.text.as_deref().filter(|_| n.input.is_none()).map(|t| t.source_text().trim().to_owned()))
            .unwrap_or_else(|| self.a11y_content_name(j))
    }

    /// The text inside node `i`, labels first, for a role a reader names from its content.
    fn a11y_content_name(&self, i: usize) -> String {
        let mut out = String::new();
        let mut j = i + 1;
        let mut skip: Option<usize> = None;
        // Its subtree is contiguous: a node after it is inside it while its parent is.
        let mut inside = vec![i];
        while j < self.tree.nodes.len() {
            let Some(p) = self.tree.nodes[j].parent else { break };
            while inside.last().is_some_and(|&a| a != p) { inside.pop(); }
            if inside.is_empty() { break; }
            inside.push(j);
            let n = &self.tree.nodes[j];
            if skip.is_some_and(|s| inside.contains(&s)) { j += 1; continue; }
            skip = None;
            if n.a11y.hidden || n.role == "list-marker" || n.vis.opacity <= 0.01 || n.is_secure() {
                skip = Some(j);
            } else if let Some(l) = n.label.as_deref().filter(|l| !l.trim().is_empty()) {
                push_words(&mut out, l);
                skip = Some(j);
            } else if let Some(t) = n.text.as_deref().filter(|_| n.input.is_none()) {
                push_words(&mut out, t.source_text());
            }
            if out.len() >= NAME_MAX { break; }
            j += 1;
        }
        out
    }
}

fn push_words(out: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() { return; }
    if !out.is_empty() { out.push(' '); }
    let room = NAME_MAX.saturating_sub(out.len());
    let mut end = text.len().min(room);
    while !text.is_char_boundary(end) { end -= 1; }
    out.push_str(&text[..end]);
}
