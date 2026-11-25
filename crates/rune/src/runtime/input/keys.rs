//! Keyboard, IME, focused-field editing and clipboard paste routing.
use super::*;

impl RuntimeCore {
    /// Offer a logical key that already passed `held_gate` to the app, the focused
    /// input's handler, focus traversal and the focused editor, in that order.
    pub(in crate::runtime) fn route_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, repeat: bool, one: &mut Tx) -> bool {
        let physical_key = one.physical_key;
        one.by_keys = true;
        one.focused = self.focused_key();
        if matches!(ev.key, crate::Key::Modifier { .. }) { return !repeat && self.route_modifier(app, ev, one); }
        if matches!(ev.key, crate::Key::Media(_)) && !self.kbd.media_keys { return false; }
        // Any other key while Ctrl and Shift are down makes them a shortcut, not a direction.
        self.kbd.disarm_direction_chord();
        one.key_repeat = repeat;
        let shown = self.kbd.focus.keyboard() || self.kbd.focus.hovered();
        if focus::navigates(ev) { self.kbd.focus.step(focus_machine::FocusEvent::Navigated); }
        // With no focused element, app-owned navigation (e.g. a toolbar)
        // gets first refusal before Rune starts a new Tab traversal.
        let app_first = self.kbd.focus.path().is_none();
        // Return and Escape that reach a field mid-composition belong to the input method: they
        // confirm or cancel it, and submit, blur and press nothing.
        if matches!(ev.key, crate::Key::Enter | crate::Key::Escape) && self.composing(app) { return true; }
        if self.element_key_preview(app, ev, one) { return true; }
        if self.element_key(app, ev, one) { return true; }
        one.physical_key = None;
        if self.default_button_key(app, ev, one) { return true; }
        one.physical_key = physical_key;
        if self.playground_nav(app, ev, one) { return true; }
        if app_first && !app.playground_typing() && app.has_on_key() && Tx::took(app.on_key(ev, one), one) { return true; }
        if self.focused_input().is_some() {
            self.edit.input_click.clear_last();
        }
        if self.run_input_key(app, ev, one) {
            return true;
        }
        one.physical_key = None;
        let read_only = self.focused_input_read_only();
        // Inside a list item or code block, Tab (and Shift+Tab in lists) edits the document by default.
        if ev.key == crate::edit::Key::Tab
            && !ev.mods.cmd
            && !ev.mods.alt
            && !ev.mods.ctrl
            && !read_only
            && self.focused_input().is_some_and(|i| {
                !i.single_line
                    && !i.edit.is_secure()
                    && (i.edit.selection_in_list() || (!ev.mods.shift && i.edit.active_block() == crate::edit::Block::Code))
            })
        {
            let layout = self.focused_layout();
            if self.with_focused(app, |e| e.key_in(ev, layout.as_ref().map(TextLayout::view)), one) {
                return true;
            }
        }
        if self.focus_key(app, ev, shown, one) { return true; }
        if ev.edit_command() == Some('v') && let Some(target) = self.paste_target() {
            // Capture the target in event order, before another queued click can
            // change focus. An empty/image clipboard still consumes the command.
            self.edit.paste_request = Some(Some(target));
            return true;
        }
        if let Some(cmd) = ev.edit_command().filter(|c| matches!(c, 'c' | 'x'))
            && self.copy_or_cut_focused(app, cmd == 'x', one)
        {
            return true;
        }
        let input_handles = match &ev.key {
            crate::edit::Key::Function(_) | crate::edit::Key::ContextMenu | crate::edit::Key::Tab | crate::edit::Key::PageUp | crate::edit::Key::PageDown
            | crate::edit::Key::Insert | crate::edit::Key::PrintScreen | crate::edit::Key::ScrollLock | crate::edit::Key::Pause
            | crate::edit::Key::Media(_) => false,
            crate::edit::Key::Command(_) => match ev.edit_command() {
                Some('a') => true,
                Some('z') => !read_only,
                Some('y') => !read_only && !cfg!(target_os = "macos"),
                Some('b' | 'i' | 'u') => !read_only && self.focused_input().is_some_and(|i| i.edit.format_chord(ev).is_some()),
                _ => false,
            },
            // Alt+Space is the window menu's, so a field leaves it to the shell.
            _ if ev.is_window_menu_chord() => false,
            _ => !ev.is_command_chord() || ev.control_binding().is_some(),
        };
        if input_handles {
            if read_only {
                if self.focused_input().is_some_and(|i| i.access.is_some()) {
                    let allows_ro = match &ev.key {
                        crate::edit::Key::Left | crate::edit::Key::Right | crate::edit::Key::Up | crate::edit::Key::Down
                        | crate::edit::Key::Home | crate::edit::Key::End | crate::edit::Key::Escape => true,
                        crate::edit::Key::Command(_) => ev.edit_command() == Some('a'),
                        _ => matches!(ev.control_binding(), Some('a' | 'e' | 'b' | 'f' | 'p' | 'n')),
                    };
                    if !allows_ro {
                        one.live_only();
                        return true;
                    }
                    let layout = self.focused_layout();
                    if self.with_focused(app, |e| e.key_in_read_only(ev, layout.as_ref().map(TextLayout::view)), one) {
                        return true;
                    }
                }
            } else {
                let layout = self.focused_layout();
                if self.with_focused(app, |e| e.key_in(ev, layout.as_ref().map(TextLayout::view)), one) {
                    return true;
                }
            }
        }
        if self.focused_input().is_none() && self.caret_browse_key(app, ev, one) {
            return true;
        }
        one.physical_key = physical_key;
        if !app_first && !app.playground_typing() && app.has_on_key() && Tx::took(app.on_key(ev, one), one) {
            if self.has_secure_focus() { log_event!(self, "key [redacted]"); } else { log_event!(self, "key {:?}", ev.key); }
            return true;
        }
        one.physical_key = None;
        if ev.edit_command() == Some('v') && self.app_paste_handler().is_some() {
            self.edit.paste_request = Some(None);
            return true;
        }
        // Selected text takes the Cmd+A and Cmd+C nothing else took, as AppKit's responder chain
        // gives a key equivalent to the menu bar before the text view: an app's shortcut wins.
        if !repeat && self.selection_key(app, ev, one) { return true; }
        if !repeat && self.context_menu_key(app, ev, one) { return true; }
        self.page_scroll(app, ev) || self.playground_key(app, ev, one)
    }

    /// Copy or cut the focused text field's selection, shared by keyboard `Cmd/Ctrl+C/X` and
    /// the field's context menu (`field_menu_pick`).
    ///
    /// Runs `El::on_input_copy` first when the field is not secure and has a non-empty selection.
    /// Returning `Some(content)` writes that `ClipboardContent` via `Tx::copy_rich`; returning
    /// `None` (or with no hook) builds plain text via `El::pill_copy_text` (when set) or
    /// `TextEdit::selected`, and attaches `TextEdit::selected_html` for rich fields. Cut deletes
    /// the selection and records undo after reading what to copy.
    pub(in crate::runtime) fn copy_or_cut_focused(&mut self, app: &mut dyn AppDriver, cut: bool, one: &mut Tx) -> bool {
        if cut && self.focused_input_read_only() {
            return false;
        }
        let Some((copy_hook, pill_copy_text)) = self
            .focused_input()
            .map(|i| (i.copy.clone(), i.pill_copy_text.clone()))
        else {
            return false;
        };
        let focused_secure = self.focused_index().is_some_and(|i| {
            std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
                .any(|k| self.tree.nodes[k].is_secure())
        });
        self.with_focused_state(app, |mut state, access, tx| {
            let (secure, has_selection) = {
                let edit = access(state.reborrow());
                edit.sync_invariants();
                let (a, b) = edit.range();
                (focused_secure || edit.is_secure(), a != b)
            };
            if secure || !has_selection {
                return EditOutcome::default();
            }
            let custom = copy_hook
                .as_ref()
                .and_then(|hook| hook(state.reborrow(), cut, tx));
            if let Some(content) = custom {
                tx.copy_rich(content);
            } else {
                let edit = access(state.reborrow());
                edit.sync_invariants();
                if focused_secure || edit.is_secure() {
                    return EditOutcome::default();
                }
                let (a, b) = edit.range();
                if a != b {
                    let text = match &pill_copy_text {
                        Some(f) => edit.selected_with_pills(|id| f(id)),
                        None => edit.selected().to_owned(),
                    };
                    let html = edit
                        .is_rich()
                        .then(|| edit.selected_html())
                        .filter(|h| !h.is_empty());
                    match html {
                        Some(html) => tx.copy_rich(crate::ClipboardContent::new(text).with_html(html)),
                        None => tx.copy(text),
                    }
                }
            }
            if cut {
                let edit = access(state);
                if !focused_secure && !edit.is_secure() {
                    edit.insert("");
                }
            }
            EditOutcome::default()
        }, one)
    }

    /// Offer `ev` to the focused text field's `El::on_input_key` handler. If it consumes the key
    /// and changes the field's text or caret, keep the caret in view after the commit, as an
    /// editor-handled edit does.
    /// Whether the currently focused element (if mounted) is marked disabled (`El::disabled`
    /// or `El::aria_disabled`) on itself or any ancestor.
    pub(in crate::runtime) fn focused_input_disabled(&self) -> bool {
        self.focused_index().is_some_and(|i| self.node_disabled(i))
    }

    pub(in crate::runtime) fn run_input_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, one: &mut Tx) -> bool {
        if self.focused_input_disabled() { return false; }
        let Some((f, access)) = self.focused_input().and_then(|input| Some((input.key.clone()?, input.access.clone()))) else {
            return false;
        };
        let before = (!self.edit.caret_follow && self.edit.caret_follow_check.is_none()).then(|| {
            let path = self.kbd.focus.owned()?;
            Some(match access {
                Some(access) => selection::caret::CaretFollowCheck::new(path, access(app.states())),
                None => selection::caret::CaretFollowCheck::new(path, &self.focused_input()?.edit),
            })
        }).flatten();
        if !Tx::took(f(app.states(), ev, one), one) {
            return false;
        }
        if !one.live_only && let Some(check) = before {
            self.edit.caret_follow_check = Some(check);
        }
        true
    }

    /// Page Up and Page Down scroll by most of a viewport, and Home and End to the top and
    /// bottom, as an NSScrollView does, when nothing else took the key. The scroll view is the
    /// nearest one around the focused element, else around the pointer, else the first in the
    /// tree, skipping any whose content fits (nothing to scroll): a chat's short composer passes
    /// the keys to the message list beside it. A view that overflows keeps them even at the edge
    /// a key moves toward, where it does nothing, as an NSTextView does; an app that wants
    /// another view paged there routes the key itself with `Tx::scroll_page`.
    pub(in crate::runtime) fn page_scroll(&mut self, app: &dyn AppDriver, ev: &KeyEvent) -> bool {
        use crate::Key::*;
        if ev.mods.cmd || ev.mods.alt || ev.mods.ctrl { return false; }
        // A focused field in a virtual row that scrolled out has no node: page its list.
        let focused = self.kbd.focus.path().and_then(|path| {
            std::iter::successors(Some(path), |p| p.rsplit_once('/').map(|(head, _)| head))
                .find_map(|p| self.tree.nodes.iter().position(|n| n.path == p))
        });
        if let Some(f) = focused && self.kbd.focus.is(&self.tree.nodes[f].path) && self.pageable(f) && self.scroller_key(f, ev) { return true; }
        if !matches!(ev.key, PageUp | PageDown | Home | End) { return false; }
        // A step's key (`Play::press`) pages only the stage, never the playground around it.
        let staged = app.playground().is_some_and(|p| p.pressing());
        let pageable = |i: usize| self.pageable(i) && (!staged || self.under(i, crate::playground::is_stage));
        let around = |from: usize| std::iter::successors(Some(from), |&i| self.tree.nodes[i].parent).find(|&i| pageable(i));
        let i = focused.and_then(around)
            .or_else(|| self.pointer.pos.and_then(|(x, y)| self.scroll_at(x, y)).and_then(|id| self.tree.node_index.get(&id).copied()).and_then(around))
            .or_else(|| (0..self.tree.nodes.len()).find(|&i| pageable(i)));
        let Some(i) = i else { return false };
        if !self.page_to(i, |offset, max, page| match ev.key { PageUp => offset - page, PageDown => offset + page, Home => 0.0, _ => max }) { return false; }
        log_event!(self, "page {:?}", ev.key);
        true
    }

    /// Up and Down by a line of 40 points, and Space (Shift+Space back) by a page, on a scroll
    /// view that has focus itself, as a browser scrolls a focused scroller (`tabindex` on an
    /// `overflow: auto` region) when nothing in it took the key.
    fn scroller_key(&mut self, i: usize, ev: &KeyEvent) -> bool {
        use crate::Key::*;
        const LINE: f64 = 40.0;
        let space = ev.key == Text(" ".into());
        if ev.mods.shift && !space { return false; }
        let moved = match ev.key {
            Up => self.page_to(i, |offset, _, _| offset - LINE),
            Down => self.page_to(i, |offset, _, _| offset + LINE),
            _ if space && ev.mods.shift => self.page_to(i, |offset, _, page| offset - page),
            _ if space => self.page_to(i, |offset, _, page| offset + page),
            _ => return false,
        };
        if moved { log_event!(self, "scroll {:?}", ev.key); }
        moved
    }

    /// A focused scroll view that presses nothing passes Space on to scroll itself.
    pub(in crate::runtime) fn scrolls_on_space(&self, i: usize) -> bool {
        self.tree.nodes[i].handlers.on_press.is_none() && self.pageable(i)
    }

    /// Whether node `i` is a live scroll view whose content overflows it.
    pub(in crate::runtime) fn pageable(&self, i: usize) -> bool {
        self.tree.nodes[i].scroll.is_some() && !self.subtree_exiting(i) && self.allowed(i)
            && self.scroll.scrolls.get(&self.tree.nodes[i].id).is_some_and(|st| st.max > 0.5)
    }

    /// Moves scroll view `i` to `to(offset, max, page)`, clamped, a page being 90% of its
    /// height. The hand moved it: any fling or glide stops, and a `stick_bottom` view that
    /// lands on its end follows it again.
    #[inline]
    pub(in crate::runtime) fn page_to(&mut self, i: usize, to: impl FnOnce(f64, f64, f64) -> f64) -> bool {
        self.page_to_with(i, crate::ScrollSource::Direct, to)
    }

    #[inline]
    pub(in crate::runtime) fn page_to_with(&mut self, i: usize, source: crate::ScrollSource, to: impl FnOnce(f64, f64, f64) -> f64) -> bool {
        let mut to = Some(to);
        self.page_to_dyn(i, source, &mut |offset, max, page| to.take().expect("page_to runs once")(offset, max, page))
    }

    fn page_to_dyn(&mut self, i: usize, source: crate::ScrollSource, to: &mut dyn FnMut(f64, f64, f64) -> f64) -> bool {
        self.release_hover_hold();
        self.drop_scroll_holds();
        let (id, page, stick) = (self.tree.nodes[i].id, f64::from(self.tree.nodes[i].local.h * 0.9), self.tree.nodes[i].scroll.is_some_and(|s| s.stick_bottom));
        let Some(st) = self.scroll.scrolls.get_mut(&id) else { return false };
        let before = st.offset;
        st.offset = to(st.offset, st.max, page).clamp(0.0, st.max);
        st.moved();
        if (st.offset - before).abs() > 1e-6 { st.note_source(source); }
        st.land(stick);
        true
    }

    pub(in crate::runtime) fn focused_layout(&mut self) -> Option<TextLayout> {
        let path = self.kbd.focus.owned()?;
        let s = self.env.scale;
        let n = self.node(&path)?;
        // The field shows its edit's display text: the layout it drew is that text's. A secret
        // draws its mask, so the keys move through a layout of what it holds.
        if let Some(l) = n.text_layout.clone().filter(|_| n.text.as_ref().is_some_and(|t| !t.redacted)) {
            return Some(l);
        }
        let (mut spec, w) = (n.text.clone()?, n.target[2] - n.target[0] - n.inset[0] - n.inset[2]);
        spec.replace_content(n.input.as_ref()?.edit.display().0);
        Some(TextLayout::Single(self.text.layout_for_spec(&spec.as_text_input(s, false), s, spec.wrap.then_some(w * s), line_alignment(&spec))))
    }

    #[inline]
    pub(crate) fn with_focused(&mut self, app: &mut dyn AppDriver, f: impl FnOnce(&mut crate::edit::TextEdit) -> EditOutcome, tx: &mut Tx) -> bool {
        let mut f = Some(f);
        self.with_focused_state_dyn(app, &mut |state, access, _| f.take().expect("focused edit runs once")(access(state)), tx)
    }

    /// [`Self::with_focused`] for an edit that needs the app state (an app's paste hook).
    #[inline]
    pub(in crate::runtime) fn with_focused_state(&mut self, app: &mut dyn AppDriver, f: impl FnOnce(crate::states::States<'_>, &crate::el::EditFn, &mut Tx) -> EditOutcome, tx: &mut Tx) -> bool {
        let mut f = Some(f);
        self.with_focused_state_dyn(app, &mut |state, access, tx| f.take().expect("focused edit runs once")(state, access, tx), tx)
    }

    fn with_focused_state_dyn(
        &mut self,
        app: &mut dyn AppDriver,
        f: &mut dyn FnMut(crate::states::States<'_>, &crate::el::EditFn, &mut Tx) -> EditOutcome,
        tx: &mut Tx,
    ) -> bool {
        if self.focused_input_disabled() { return false; }
        let Some(path) = self.kbd.focus.owned() else { return false };
        let Some((access, submit, change, selection_change, animate_layout, single_line, secure, revealed_override, line_home_end_override)) =
            self.focused_input().map(|i| (
                i.access.clone(),
                i.submit.clone(),
                i.change.clone(),
                i.selection_change.clone(),
                i.animate_layout,
                i.single_line,
                i.edit.is_secure(),
                i.revealed_override,
                i.line_home_end_override,
            ))
        else {
            return false;
        };
        let Some(access) = access else { return false };
        self.edit.input_click.clear_last();
        {
            let edit = access(app.states());
            edit.set_single_line(single_line);
            if secure && !edit.is_secure() { edit.set_secure(true); }
            if let Some(rev) = revealed_override { edit.set_revealed(rev); }
            if let Some(line_home_end) = line_home_end_override { edit.set_line_home_end(line_home_end); }
        }
        let before = change.as_ref().map(|_| zeroize::Zeroizing::new(access(app.states()).text.clone()));
        let before_sel = selection_change.as_ref().map(|_| {
            let e = access(app.states());
            (e.anchor, e.focus)
        });
        let mut out = f(app.states(), &access, tx);
        // Submit and Escape's blur act once per press; held keys still type, move and delete.
        if tx.key_repeat { out.submit = false; out.blur = false; }
        // Typing is the user's own hand: no animation.
        tx.commit = tx.commit.snap(!animate_layout);
        self.edit.caret_follow = true;
        if let Some(change) = change
            && before.as_ref().map(|s| s.as_str()) != Some(access(app.states()).text.as_str())
        {
            change(app.states(), tx);
            access(app.states()).mark_edited();
        }
        if let Some(selection_change) = selection_change {
            let after_sel = {
                let e = access(app.states());
                (e.anchor, e.focus)
            };
            if before_sel != Some(after_sel) {
                selection_change(app.states(), tx);
            }
        }
        if let Some(c) = out.copy {
            tx.copy(c);
        }
        if out.blur {
            self.set_focus(app, None, self.kbd.focus.keyboard());
        }
        if out.submit
            && let Some(sub) = submit
        {
            log_event!(self, "submit {path}");
            tx.commit = tx.commit.animated();
            sub(app.states(), tx);
        } else if out.submit {
            // A field with nothing to submit passes Return to the default button, as in a sheet.
            self.press_default_button(app, tx);
        }
        true
    }

    /// Whether the focused field has an input method composition (preedit) open.
    pub(in crate::runtime) fn composing(&mut self, app: &mut dyn AppDriver) -> bool {
        let Some(access) = self.focused_input().map(|i| i.access.clone()) else { return self.app_ime_composing(app) };
        match access {
            Some(access) => access(app.states()).preedit().is_some(),
            None => self.focused_input().is_some_and(|i| i.edit.preedit().is_some()),
        }
    }

    pub(crate) fn node_index(&self, path: &str) -> Option<usize> {
        self.tree.nodes.iter().enumerate().position(|(i, n)| n.path == path && !self.subtree_exiting(i) && !self.subtree_unmounted(i))
    }

    pub(crate) fn node(&self, path: &str) -> Option<&Node> {
        self.node_index(path).map(|i| &self.tree.nodes[i])
    }

    /// `El::on_key_preview` handlers on the focused element's allowed ancestors and the focused
    /// element itself, outermost first (DOM capture-phase `keydown` / WPF `PreviewKeyDown`).
    /// Runs for focused text fields too (including one scrolled out of a virtual list), stops at
    /// an active modal or inert boundary, and never runs on key-up.
    pub(in crate::runtime) fn element_key_preview(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let i = match self.focused_index() {
            Some(i) if self.can_focus(i) || (self.active_modal() == Some(i) && self.reachable(i)) => i,
            None if self.focused_input().is_some() => {
                let Some(path) = self.kbd.focus.path() else { return false };
                let Some(anc) = std::iter::successors(path.rsplit_once('/').map(|(head, _)| head), |p| p.rsplit_once('/').map(|(head, _)| head))
                    .find_map(|p| self.node_index(p))
                else {
                    return false;
                };
                if !self.reachable(anc) { return false; }
                anc
            }
            _ => return false,
        };
        let mut handlers: Vec<_> = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent)
            .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
            .filter_map(|j| self.tree.nodes[j].focus.key_preview.clone())
            .collect();
        handlers.reverse();
        handlers.into_iter().any(|f| Tx::took(f(app.states(), ev, tx), tx))
    }

    /// `El::on_key` handlers on the focused element and its ancestors, innermost first. A
    /// focused text field's keys stay its own.
    pub(in crate::runtime) fn element_key(&mut self, app: &mut dyn AppDriver, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(i) = self.focused_index() else { return false };
        if !self.can_focus(i) || self.focused_input().is_some() { return false; }
        let handlers: Vec<_> = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent)
            .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
            .filter_map(|j| self.tree.nodes[j].focus.key.clone())
            .collect();
        let rtl = std::mem::replace(&mut tx.key_rtl, self.tree.nodes[i].flow == crate::el::Flow::Rtl);
        let took = handlers.into_iter().any(|f| Tx::took(f(app.states(), ev, tx), tx));
        tx.key_rtl = rtl;
        took
    }

    pub(in crate::runtime) fn focused_input(&self) -> Option<&InputSpec> {
        let path = self.kbd.focus.path()?;
        self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.commit.virtual_inputs.get(path).map(Box::as_ref))
    }

    /// Whether node `i` or any of its ancestors marks it read-only (`El::read_only(true)`),
    /// or its `InputSpec` carries `read_only`.
    pub(crate) fn is_node_read_only(&self, i: usize) -> bool {
        if self.tree.nodes[i].input.as_ref().is_some_and(|inp| inp.read_only) {
            return true;
        }
        std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
            .any(|k| self.tree.nodes[k].a11y.more.as_ref().is_some_and(|m| m.read_only))
    }

    /// Whether the currently focused text input is read-only (`El::read_only(true)` on the field
    /// or an ancestor, including a focused field scrolled out of a virtual list).
    pub(crate) fn focused_input_read_only(&self) -> bool {
        match self.focused_index() {
            Some(i) => self.is_node_read_only(i),
            None => self.focused_input().is_some_and(|inp| inp.read_only),
        }
    }

    /// What [`Tx::focused`] reports: the focused link's id, else the focused element's key,
    /// else its path's last segment (a field in a virtual row that scrolled out has no node).
    pub(in crate::runtime) fn focused_key(&self) -> Option<String> {
        let path = self.kbd.focus.path()?;
        if let Some(id) = self.focused_index().and_then(|i| self.edit.focused_link_id(&self.tree.nodes[i])) { return Some(id); }
        Some(self.focused_index().and_then(|i| self.tree.nodes[i].key.clone())
            .unwrap_or_else(|| {
                let last = path.rsplit('/').next().unwrap_or(path);
                element_path::decode_key_segment(last).map_or_else(|| last.to_owned(), std::borrow::Cow::into_owned)
            }))
    }

    /// The focused element's index. Unlike [`Runtime::node`], this finds it while it exits.
    #[inline]
    pub(crate) fn focused_index(&self) -> Option<usize> {
        self.kbd.focused_index(&self.tree.nodes)
    }

    pub fn key(&mut self, ev: KeyEvent) {
        self.queue_key(ev, false);
    }

    /// Opt into hardware media transport keys (`Key::Media`). Off by default.
    #[inline]
    pub(crate) fn set_media_keys(&mut self, enabled: bool) { self.kbd.media_keys = enabled; }

    /// Whether this runtime accepts hardware media transport keys (`Key::Media`).
    #[inline]
    pub(crate) fn media_keys(&self) -> bool { self.kbd.media_keys }

    /// [`Runtime::key`]; `stage` routes it to the open playground's stage, for `Play::press`.
    pub(in crate::runtime) fn queue_key(&mut self, ev: KeyEvent, stage: bool) {
        if matches!(ev.key, crate::Key::Media(_)) && !self.kbd.media_keys { return; }
        self.dismiss_help();
        // A key ends a middle-click autoscroll, and does nothing else.
        if self.pointer.autoscroll_key() { return; }
        // Escape cancels a held drag first, or releases an active pointer lock, and goes no further.
        if ev.key == crate::Key::Escape && (self.cancel_drag() || self.release_pointer_lock()) { return; }
        self.frame.queue.push(Action::Key { ev, repeat: false, stage });
    }

    /// Queue an auto-repeat of a held key, as the OS sends after the first press. Handlers see
    /// `Tx::key_repeat` true for it.
    pub fn repeat_key(&mut self, ev: KeyEvent) {
        if matches!(ev.key, crate::Key::Media(_)) && !self.kbd.media_keys { return; }
        self.frame.queue.push(Action::Key { ev, repeat: true, stage: false });
    }

    pub fn ime(&mut self, ev: Ime) {
        self.frame.queue.push(Action::Ime(ev));
    }

    /// The window went inactive mid-composition: what the composition shows becomes text, in
    /// event order, as a browser keeps it, and the field keeps focus. The platform's input
    /// method is told to drop its own copy (`ime_reset`), so it doesn't come back in the next
    /// field.
    pub fn finish_composition(&mut self) {
        self.frame.queue.push(Action::FinishIme);
        self.edit.ime_reset = true;
    }

    pub fn paste(&mut self, text: String) {
        self.frame.queue.push(Action::Paste(crate::ClipboardContent::new(text), None));
    }
    /// [`Runtime::paste`] with several forms, as from another app's clipboard: its HTML is
    /// sanitised as a real paste's is. For tests and scripts.
    pub fn paste_content(&mut self, content: crate::ClipboardContent) {
        self.frame.queue.push(Action::Paste(content.untrusted(), None));
    }

    pub(in crate::runtime) fn app_paste_handler(&self) -> Option<crate::el::PasteFn> {
        self.tree.nodes.get(self.active_modal().unwrap_or_else(|| self.app_root()))?.handlers.on_paste.clone()
    }
    #[cfg(test)]
    pub(crate) fn accepts_paste(&self) -> bool {
        self.paste_target().is_some() || self.app_paste_handler().is_some()
    }
    pub(crate) fn paste_target(&self) -> Option<String> {
        if self.focused_input_read_only() || self.focused_input_disabled() { return None; }
        self.focused_input().filter(|i| i.access.is_some()).and(self.kbd.focus.owned())
    }
    /// What the paste target hears: every form, images too, for a field with
    /// `on_input_paste` or a root `on_paste_content` handler; text, HTML and RTF for a rich
    /// [`TextEdit`](crate::TextEdit); only the clipboard's text for anything else (including a
    /// plain field or a plain `on_paste`).
    pub(crate) fn paste_forms(&self, target: Option<&str>) -> crate::clipboard::Forms {
        use crate::clipboard::Forms;
        match target {
            Some(path) => match self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.commit.virtual_inputs.get(path).map(Box::as_ref)) {
                Some(input) if input.paste.is_some() => Forms::All,
                Some(input) if input.edit.is_rich() && !input.edit.is_secure() => Forms::Rich,
                _ => Forms::Text,
            },
            None if self.app_paste_handler().is_some_and(|p| p.rich) => Forms::All,
            None => Forms::Text,
        }
    }
    pub(crate) fn paste_result(&mut self, target: Option<String>, content: crate::ClipboardContent) {
        self.frame.queue.push(Action::Paste(content, Some(target)));
    }
    pub(crate) fn read_result(&mut self, id: u64, content: crate::ClipboardContent) {
        self.frame.queue.push(Action::ClipboardRead(id, content));
    }
    pub(crate) fn copy_image_result(&mut self, id: u64, result: Result<(), crate::CopyImageError>) {
        self.frame.queue.push(Action::CopyImageDone(id, result));
    }
    /// Queues a `Tx::open_url_then` callback with what came of its open.
    pub(crate) fn url_opened(&mut self, f: crate::open_url::OpenedFn, result: Result<(), crate::open_url::OpenUrlError>) {
        self.frame.queue.push(Action::UrlOpened(f, result));
    }

    /// The focused field's text around its selection, for the platform's input method.
    pub(crate) fn ime_text(&self) -> Option<crate::edit::ImeText> {
        if self.focused_input_read_only() || self.focused_input_disabled() { return None; }
        self.focused_input().and_then(|i| i.edit.ime_text())
    }

    /// The focused field's open composition and its cursor range, for a script's tree.
    pub(crate) fn focused_preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        self.focused_input().filter(|i| !i.edit.is_secure()).and_then(|i| i.edit.preedit())
    }

    /// Whether the focused editor requires secure native keyboard input.
    pub fn has_secure_focus(&self) -> bool {
        self.focused_input().is_some_and(|i| i.edit.is_secure())
    }

    #[inline]
    pub fn has_focus(&self) -> bool {
        self.kbd.has_focus()
    }
}

#[cfg(test)]
#[path = "key_preview_tests.rs"]
mod key_preview_tests;
