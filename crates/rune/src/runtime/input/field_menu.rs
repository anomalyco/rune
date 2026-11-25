//! A text field's own menu (right-click, macOS control-click, the Menu key and Shift+F10), as a
//! browser's menu over an editable field: a misspelled word's suggestions and Learn Spelling,
//! then Cut, Copy, Paste and, off macOS, Select All, then the app's Spellcheck setting
//! (`El::on_spellcheck`) and, where there are dictionaries to choose, its Spellcheck Languages
//! (`El::on_spellcheck_languages`). A field with its own `on_secondary_press` shows that instead.
//! The items read the app's names for them (`rune::set_labels`) each time the menu opens.
use super::input_click::hit;
use super::*;
use crate::labels::label;
use crate::popup_menu::{PopupAnchor, Request};
use crate::{Key, Label, Mods, PopupItem, PopupMenu};

/// What a field menu's items act on once one is picked.
pub(crate) struct FieldMenu {
    path: String,
    /// The misspelled word the menu offers suggestions for: its byte range and text.
    word: Option<(usize, usize, String)>,
    suggestions: Vec<String>,
    toggle: Option<(bool, crate::el::WindowFocusFn)>,
    /// The dictionaries checking text when the menu opened, and what keeps a new choice.
    languages: Option<(Vec<String>, crate::el::LanguagesFn)>,
}

/// A pending fallback field menu delivered to `App::on_field_menu`, awaiting
/// `Tx::resolve_field_menu`.
pub(crate) struct PendingFieldMenu {
    pub(crate) token: u64,
    pub(crate) menu: PopupMenu,
    pub(crate) field: FieldMenu,
}

impl RuntimeCore {
    /// Opens field `path`'s menu at window point `at`, pressed at window point `hit_at`, or from
    /// the keyboard (`None`).
    fn field_menu_fallback_active(&self) -> bool {
        #[cfg(test)]
        if self.host.force_field_menu_fallback {
            return true;
        }
        !crate::popup_menu_supported()
    }

    pub(in crate::runtime) fn field_menu(&mut self, app: &mut dyn AppDriver, path: String, at: (f32, f32), hit_at: Option<(f32, f32)>, one: &mut Tx) -> bool {
        let Some(i) = self.live(&path) else { return false };
        if self.node_disabled(i) { return false; }
        let field_key = self.tree.nodes[i].key.clone();
        let Some(input) = self.tree.nodes[i].input.as_deref() else { return false };
        if let (Some(f), Some((hx, hy))) = (input.on_pill_secondary.clone(), hit_at)
            && let Some(pill_id) = self.input_pill_at(i, hx, hy)
            && f(app.states(), &pill_id, crate::PointerPress { x: at.0, y: at.1 }, one)
        {
            return true;
        }
        let selection_change = input.selection_change.clone();
        let Some(access) = input.access.clone() else { return false };
        let (single, secure) = (input.single_line, input.edit.is_secure());
        let read_only = self.is_node_read_only(i);
        let spell = input.spellcheck && !secure && !read_only;
        let layout = self.tree.nodes[i].text_layout.clone();
        let point = hit_at.and_then(|(x, y)| self.input_text_point(i, x, y));
        let s = self.env.scale;
        let toggle = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent).find_map(|j| self.tree.nodes[j].handlers.on_spellcheck.clone());
        let keep = std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent).find_map(|j| self.tree.nodes[j].handlers.on_spellcheck_languages.clone());
        if self.kbd.focus.path() != Some(path.as_str()) {
            self.set_focus(app, Some(path.clone()), false);
        }
        let changed_sel = {
            let e = access(app.states());
            e.set_single_line(single);
            let before = (e.anchor, e.focus);
            if let (Some([tx, ty]), Some(l)) = (point, layout.as_ref().map(TextLayout::view))
                && e.preedit().is_none()
            {
                let caret = hit(e, l, tx, ty, s, single).caret;
                let (a, b) = e.range();
                // A press in the selection keeps it for Cut and Copy. Elsewhere it moves the caret
                // there, and on a misspelled word selects the word, as Chromium does.
                if a == b || caret < a || caret > b {
                    match spell.then(|| crate::spell::misspelled_at(&e.text, caret)).flatten() {
                        Some((wa, wb)) => e.set_selection(wa, wb),
                        None => e.set_caret(caret),
                    }
                }
            }
            (e.anchor, e.focus) != before
        };
        if changed_sel && let Some(selection_change) = selection_change {
            selection_change(app.states(), one);
        }
        let e = access(app.states());
        let (a, b) = e.range();
        let word = spell
            .then(|| crate::spell::misspelled(&e.text, None))
            .filter(|found| found.contains(&(a, b)))
            .map(|_| (a, b, e.text[a..b].to_owned()));
        let suggestions = word.as_ref().map(|w| crate::spell::suggest(&w.2)).unwrap_or_default();
        let mut menu = PopupMenu::new();
        if word.is_some() {
            if suggestions.is_empty() {
                menu = menu.item(PopupItem::new("no-guesses", label(Label::NoGuesses)).enabled(false));
            }
            menu = menu.items(suggestions.iter().enumerate().map(|(n, s)| PopupItem::new(format!("suggestion-{n}"), s.clone())));
            menu = menu.separator().item(PopupItem::new("learn-spelling", label(Label::LearnSpelling))).separator();
        }
        let selected = a != b && !secure;
        menu = menu
            .item(PopupItem::new("cut", label(Label::Cut)).enabled(selected && !read_only))
            .item(PopupItem::new("copy", label(Label::Copy)).enabled(selected))
            .item(PopupItem::new("paste", label(Label::Paste)).enabled(!read_only));
        if !cfg!(target_os = "macos") {
            menu = menu.item(PopupItem::new("select-all", label(Label::SelectAll)));
        }
        let toggle = (!read_only).then_some(toggle).flatten();
        let mut languages = None;
        if let Some((on, _)) = &toggle {
            menu = menu.separator().item(PopupItem::new("spellcheck", label(Label::Spellcheck)).checked(*on));
            // Desktop's dictionary picker (spellcheck.ts), where the platform has dictionaries
            // to choose: the last one stays, as the Spellcheck item is how to stop checking.
            let available = crate::spell::available_languages();
            if let Some(keep) = keep.filter(|_| !available.is_empty()) {
                let current = crate::spell::languages();
                let only = |tag: &String| current.len() == 1 && current[0] == *tag;
                // Named in the app's language, in the order of those names, as Desktop's
                // `nativeLanguageOptions` sorts its `Intl.DisplayNames`.
                let locale = crate::labels::locale();
                let name = |tag: &str| {
                    crate::labels::named(Label::Language(tag))
                        .or_else(|| locale.as_deref().and_then(|locale| crate::spell::language_name_in(tag, locale)))
                        .unwrap_or_else(|| crate::spell::language_name(tag))
                };
                let mut named: Vec<(String, &String)> = available.iter().map(|tag| (name(tag), tag)).collect();
                crate::spell::sort_names_in(&mut named, locale.as_deref().unwrap_or("en"));
                let items = named.into_iter().map(|(name, tag)| {
                    PopupItem::new(format!("language-{tag}"), name).checked(current.contains(tag)).enabled(!only(tag))
                });
                menu = menu.submenu_enabled(label(Label::SpellcheckLanguages), PopupMenu::new().items(items), *on);
                languages = Some((current, keep));
            }
        }
        one.commit = one.commit.instant();
        log_event!(self, "field menu {path}");
        let field = FieldMenu { path: path.clone(), word, suggestions, toggle, languages };
        if self.field_menu_fallback_active() && app.has_on_field_menu() {
            self.host.field_menu_seq = self.host.field_menu_seq.wrapping_add(1);
            let token = self.host.field_menu_seq;
            self.host.deferred_field_menu_pick = None;
            self.host.field_menu_fallback = Some(Box::new(PendingFieldMenu { token, menu: menu.clone(), field }));
            one.field_menu_opened = true;
            self.trace_event("field-menu:open");
            let request = crate::FieldMenuRequest::new(field_key, path, crate::PointerPress { x: at.0, y: at.1 }, menu);
            app.on_field_menu(request, one);
            return true;
        }
        self.host.popups.push_back(Request {
            menu,
            anchor: PopupAnchor::Point(at.0, at.1),
            on_pick: Box::new(|_, _, _| {}),
            field: Some(Box::new(field)),
        });
        true
    }

    /// Handles `Tx::resolve_field_menu` during request latching.
    ///
    /// Dismissals (`None`) cancel immediately. Picks (`Some(id)`) run immediately when the target
    /// field is currently reachable, or defer until right after the commit when an active modal
    /// (such as a closing in-window `rune_kit::Menu`) currently blocks `self.live(&field.path)`.
    pub(in crate::runtime) fn latch_field_menu_resolution(&mut self, app: &mut dyn AppDriver, picked: Option<String>, one: &mut Tx) {
        let Some(pending) = self.host.field_menu_fallback.as_ref() else { return };
        if let Some(id) = picked.as_deref() {
            let Some(item) = pending.menu.find(id) else { return };
            if !item.enabled { return; }
            if self.active_modal().is_some() && self.live(&pending.field.path).is_none() {
                self.host.deferred_field_menu_pick = Some((pending.token, id.to_owned()));
                self.commit.dirty = true;
                return;
            }
        }
        self.host.deferred_field_menu_pick = None;
        self.resolve_field_menu_fallback(app, picked.as_deref(), one);
    }

    /// Resolves a deferred `Tx::resolve_field_menu(Some(id))` right after a commit unmounts the
    /// modal menu that covered the target field, latching its clipboard/focus effects and
    /// committing any state change in the same frame.
    pub(in crate::runtime) fn finish_deferred_field_menu(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        open_urls: Option<&mut Vec<crate::open_url::Link>>,
        open_paths: Option<&mut Vec<crate::open_path::PathRequest>>,
    ) {
        let Some((token, id)) = self.host.deferred_field_menu_pick.take() else { return };
        let Some(pending) = self.host.field_menu_fallback.as_ref() else { return };
        if pending.token != token || self.live(&pending.field.path).is_none() {
            return;
        }
        let mut post_tx = self.transaction(app);
        self.resolve_field_menu_fallback(app, Some(&id), &mut post_tx);
        let wanted_paste = post_tx.paste;
        let mut urls = std::mem::take(&mut post_tx.open_urls);
        let mut paths = std::mem::take(&mut post_tx.open_paths);
        self.latch_requests(app, &mut post_tx);
        if wanted_paste && self.edit.clipboard().is_some() {
            for queued in std::mem::take(&mut self.frame.queue) {
                if !matches!(queued.action, Action::Paste(..)) {
                    self.frame.queue.push_queued(queued);
                    continue;
                }
                if self.run_action(app, queued.action, &mut post_tx) {
                    self.commit.dirty = true;
                }
            }
            urls.extend(std::mem::take(&mut post_tx.open_urls));
            paths.extend(std::mem::take(&mut post_tx.open_paths));
            self.latch_requests(app, &mut post_tx);
        }
        if let Some(dst) = open_urls {
            dst.extend(urls);
        } else {
            post_tx.open_urls.extend(urls);
        }
        if let Some(dst) = open_paths {
            dst.extend(paths);
        } else {
            post_tx.open_paths.extend(paths);
        }
        self.schedule_wakes(post_tx.wakes.drain(..));
        if self.commit.dirty || self.commit.relayout || self.kbd.focus_request.is_some() {
            self.capture_keeps(&mut post_tx);
            self.commit.set_state_unchanged(false);
            self.commit_for(app, &post_tx, CommitWhy::DIRTY);
            self.apply_keeps(app);
        }
        merge(tx, post_tx);
    }

    /// Resolves the pending fallback field menu delivered to `App::on_field_menu` when
    /// `Tx::resolve_field_menu` runs.
    pub(in crate::runtime) fn resolve_field_menu_fallback(&mut self, app: &mut dyn AppDriver, picked: Option<&str>, one: &mut Tx) {
        let Some(pending) = self.host.field_menu_fallback.as_ref() else { return };
        if let Some(id) = picked {
            let Some(item) = pending.menu.find(id) else { return };
            if !item.enabled { return; }
        }
        let Some(pending) = self.host.field_menu_fallback.take() else { return };
        match picked {
            Some(id) => {
                self.trace_event(format_args!("field-menu:pick:{id}"));
                self.field_menu_pick(app, pending.field, id, one);
                self.commit.dirty = true;
            }
            None => {
                self.trace_event("field-menu:cancel");
            }
        }
    }

    /// Does what `picked` in field menu `menu` asks, on its field.
    pub(in crate::runtime) fn field_menu_pick(&mut self, app: &mut dyn AppDriver, menu: FieldMenu, picked: &str, one: &mut Tx) {
        if self.kbd.focus.path() != Some(menu.path.as_str()) && self.live(&menu.path).is_some() {
            self.set_focus(app, Some(menu.path.clone()), false);
        }
        let command = |c| KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } };
        match picked {
            "cut" => {
                self.copy_or_cut_focused(app, true, one);
            }
            "copy" => {
                self.copy_or_cut_focused(app, false, one);
            }
            "select-all" => {
                let ev = command('a');
                let layout = self.focused_layout();
                let ro = self.focused_input_read_only();
                self.with_focused(
                    app,
                    |e| if ro { e.key_in_read_only(&ev, layout.as_ref().map(TextLayout::view)) } else { e.key_in(&ev, layout.as_ref().map(TextLayout::view)) },
                    one,
                );
            }
            "paste" => {
                if !self.focused_input_read_only() {
                    one.request_paste();
                }
            }
            "learn-spelling" => {
                if let Some((_, _, word)) = &menu.word {
                    crate::spell::learn(word);
                    self.commit.dirty = true;
                }
            }
            "spellcheck" => {
                if let Some((on, f)) = menu.toggle {
                    f(app.states(), !on, one);
                }
            }
            id if id.starts_with("language-") => {
                if let (Some((mut current, keep)), Some(tag)) = (menu.languages, id.strip_prefix("language-")) {
                    match current.iter().position(|t| t == tag) {
                        Some(n) => _ = current.remove(n),
                        None => current.push(tag.to_owned()),
                    }
                    crate::spell::set_languages(&current);
                    keep(app.states(), current, one);
                    self.commit.dirty = true;
                }
            }
            id => {
                let n = id.strip_prefix("suggestion-").and_then(|n| n.parse::<usize>().ok());
                if !self.focused_input_read_only()
                    && let (Some((a, b, _)), Some(suggestion)) = (menu.word, n.and_then(|n| menu.suggestions.get(n).cloned()))
                {
                    self.with_focused(
                        app,
                        |e| {
                            e.set_selection(a, b);
                            e.paste(&suggestion)
                        },
                        one,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "field_menu_tests.rs"]
mod tests;
