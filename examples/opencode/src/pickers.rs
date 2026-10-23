//! Fixture-backed composer selection. Desktop reference: ModelSelectorPopoverView and
//! ComposerEditorSelect. Selection belongs to the draft/session, not historical messages.
use std::collections::HashMap;

use rune::{Key, KeyEvent, TextEdit};
use serde_json::Value;

use crate::{App, Route, model::{ModelRef, capitalize}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Agent, Model, Effort }

impl Kind {
    pub fn key(self) -> &'static str {
        match self { Self::Agent => "agent", Self::Model => "model", Self::Effort => "effort" }
    }
}

#[derive(Clone, Debug)]
pub struct Model {
    pub key: String,
    pub id: String,
    pub provider: String,
    pub group: String,
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Selection { pub model: ModelRef, pub agent: String }

#[derive(Clone)]
pub struct Item { pub key: String, pub label: String, pub group: String }

#[derive(Default)]
pub struct Pickers {
    pub models: Vec<Model>,
    pub agents: Vec<String>,
    pub open: Option<Kind>,
    pub search: TextEdit,
    pub toolbar_focus: Option<Kind>,
    active: Option<String>,
    searched: String,
    selections: HashMap<String, Selection>,
}

impl Pickers {
    pub fn load(&mut self, fixture: &Value) {
        self.models = fixture["models"].as_array().into_iter().flatten().map(|v| {
            let s = |key: &str| v[key].as_str().unwrap_or("").to_owned();
            Model { key: format!("{}:{}", s("provider"), s("id")), id: s("id"), provider: s("provider"),
                group: s("group"), name: s("name"), variants: v["variants"].as_array().into_iter().flatten()
                    .filter_map(Value::as_str).map(str::to_owned).collect() }
        }).collect();
        self.agents = fixture["agents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    }

    pub fn activate(&mut self, key: String) {
        self.searched.clone_from(&self.search.text);
        self.active = Some(key);
    }
}

impl App {
    fn selection_key(&self) -> &str {
        match &self.route { Route::Session(id) => id, _ => "draft" }
    }

    pub fn selection(&self) -> Selection {
        self.pickers.selections.get(self.selection_key()).cloned().unwrap_or_else(|| Selection {
            model: match &self.route {
                Route::Session(id) => self.session(id),
                _ => self.sessions.first(),
            }.map(|s| s.model.clone()).unwrap_or_default(),
            agent: "build".into(),
        })
    }

    pub fn picker_items(&self) -> Vec<Item> {
        let selection = self.selection();
        match self.pickers.open {
            Some(Kind::Model) => {
                let q = self.pickers.search.text.to_lowercase();
                self.pickers.models.iter().filter(|m| format!("{} {} {}", m.name, m.group, m.id).to_lowercase().contains(&q))
                    .map(|m| Item { key: m.key.clone(), label: m.name.clone(), group: m.group.clone() }).collect()
            }
            Some(Kind::Agent) => self.pickers.agents.iter().map(|a| Item { key: a.clone(), label: capitalize(a), group: String::new() }).collect(),
            Some(Kind::Effort) => {
                let variants = self.pickers.models.iter().find(|m| m.id == selection.model.id && m.provider == selection.model.provider);
                std::iter::once(Item { key: "default".into(), label: "Default".into(), group: String::new() })
                    .chain(variants.into_iter().flat_map(|m| &m.variants).map(|v| Item { key: v.clone(), label: capitalize(v), group: String::new() })).collect()
            }
            None => vec![],
        }
    }

    pub fn picker_current(&self, kind: Kind) -> String {
        let s = self.selection();
        match kind {
            Kind::Model => format!("{}:{}", s.model.provider, s.model.id),
            Kind::Agent => s.agent,
            Kind::Effort => if s.model.variant.is_empty() { "default".into() } else { s.model.variant },
        }
    }

    pub fn picker_active(&self) -> Option<String> {
        if self.pickers.search.text != self.pickers.searched {
            self.picker_items().first().map(|i| i.key.clone())
        } else { self.pickers.active.clone() }
    }

    pub fn open_picker(&mut self, kind: Kind) {
        if self.live() { return; }
        if self.pickers.open == Some(kind) { self.close_picker(true); return; }
        self.pickers.open = Some(kind);
        self.pickers.search = TextEdit::new("");
        self.pickers.search.focused = kind == Kind::Model;
        self.pickers.searched.clear();
        let current = self.picker_current(kind);
        let items = self.picker_items();
        self.pickers.active = items.iter().find(|i| i.key == current).or(items.first()).map(|i| i.key.clone());
        self.composer.focused = false;
        self.pickers.toolbar_focus = None;
    }

    pub fn close_picker(&mut self, restore: bool) {
        self.pickers.open = None;
        self.pickers.search.focused = false;
        self.pickers.search.preedit = None;
        self.pickers.search.break_history_group();
        self.composer.focused = restore;
    }

    pub fn choose_picker(&mut self, key: &str) {
        // Never accept a stale, filtered-out item or a non-option (including empty results).
        if !self.picker_items().iter().any(|i| i.key == key) { return; }
        let mut selected = self.selection();
        match self.pickers.open {
            Some(Kind::Model) => {
                let Some(m) = self.pickers.models.iter().find(|m| m.key == key) else { return };
                if m.id != selected.model.id || m.provider != selected.model.provider {
                    selected.model = ModelRef { id: m.id.clone(), provider: m.provider.clone(), variant: String::new() };
                }
            }
            Some(Kind::Agent) => selected.agent = key.into(),
            Some(Kind::Effort) => selected.model.variant = if key == "default" { String::new() } else { key.into() },
            None => return,
        }
        self.pickers.selections.insert(self.selection_key().to_owned(), selected);
        self.close_picker(true);
    }

    pub fn picker_key(&mut self, ev: &KeyEvent) -> bool {
        if self.pickers.open.is_some() {
            match &ev.key {
                Key::Escape => self.close_picker(true),
                Key::Tab => {
                    let kind = self.pickers.open;
                    self.close_picker(false);
                    self.pickers.toolbar_focus = kind;
                }
                Key::Up | Key::Down if !ev.mods.alt && !ev.mods.cmd => {
                    if self.pickers.search.preedit.is_some() { return false; }
                    let items = self.picker_items();
                    if !items.is_empty() {
                        let active = self.picker_active();
                        let index = items.iter().position(|i| Some(&i.key) == active.as_ref()).unwrap_or(0);
                        let next = if ev.key == Key::Down { (index + 1) % items.len() } else { (index + items.len() - 1) % items.len() };
                        self.pickers.activate(items[next].key.clone());
                    }
                }
                Key::Enter => {
                    if self.pickers.search.preedit.is_none() && let Some(key) = self.picker_active() { self.choose_picker(&key); }
                }
                _ => return self.pickers.open != Some(Kind::Model),
            }
            return true;
        }
        if self.composer.focused { self.pickers.toolbar_focus = None; }
        if ev.key == Key::Tab && (self.composer.focused || self.pickers.toolbar_focus.is_some()) {
            let order = [Kind::Agent, Kind::Model, Kind::Effort];
            let next = self.pickers.toolbar_focus.and_then(|k| order.iter().position(|i| *i == k))
                .map_or(if ev.mods.shift { 2 } else { 0 }, |i| if ev.mods.shift { (i + 3) % 4 } else { i + 1 });
            self.pickers.toolbar_focus = order.get(next).copied();
            self.composer.focused = next == 3;
            return true;
        }
        if matches!(ev.key, Key::Enter | Key::Down | Key::Up)
            && let Some(kind) = self.pickers.toolbar_focus { self.open_picker(kind); return true; }
        if !self.live() {
            if ev.key == Key::Command('\'') { self.open_picker(Kind::Model); return true; }
            let cycle = match ev.key {
                Key::Command('.') => Some((Kind::Agent, ev.mods.shift)),
                Key::Command('d') if ev.mods.shift => Some((Kind::Effort, false)),
                _ => None,
            };
            if let Some((kind, backwards)) = cycle {
                self.open_picker(kind);
                self.picker_key(&KeyEvent { key: if backwards { Key::Up } else { Key::Down }, mods: Default::default() });
                if let Some(key) = self.picker_active() { self.choose_picker(&key); }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    fn runtime() -> Runtime<App> {
        // Deliberately bypass App::new: tests must never probe a server.
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.frame(0.0);
        rt
    }
    fn key(rt: &mut Runtime<App>, key: Key) {
        rt.key(KeyEvent { key, mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }
    fn tap(rt: &mut Runtime<App>, target: &str) {
        let (x, y) = rt.find(target).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn model_search_wrap_empty_ime_and_focus_restore() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        key(&mut rt, Key::Text("draft".into()));
        tap(&mut rt, "model");
        assert_eq!(rt.state.picker_active().as_deref(), Some("anthropic:claude-opus-5-5"));
        key(&mut rt, Key::Up);
        assert_eq!(rt.state.picker_active().as_deref(), Some("google:gemini-4-argon"));
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Text("OPENAI".into()));
        assert_eq!(rt.state.picker_active().as_deref(), Some("openai:gpt-6-astra"));
        assert_eq!(rt.state.composer.text, "draft");
        rt.ime(rune::Ime::Preedit("x".into(), None));
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.pickers.open, Some(Kind::Model));
        rt.ime(rune::Ime::Preedit(String::new(), None));
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().model.id, "gpt-6-astra");
        assert!(rt.state.selection().model.variant.is_empty());
        assert_eq!(rt.state.sends, 0);
        // Exit is still on screen, but keyboard focus already belongs to the composer.
        key(&mut rt, Key::Text("!".into()));
        assert_eq!(rt.state.composer.text, "draft!");
        tap(&mut rt, "model");
        key(&mut rt, Key::Text("no-such-model".into()));
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.pickers.open, Some(Kind::Model));
        key(&mut rt, Key::Escape);
        assert!(rt.state.composer.focused);
    }

    #[test]
    fn agent_effort_tab_pointer_and_session_isolation() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        for expected in [Some(Kind::Agent), Some(Kind::Model), Some(Kind::Effort), None] {
            key(&mut rt, Key::Tab);
            assert_eq!(rt.state.pickers.toolbar_focus, expected);
        }
        assert!(rt.state.composer.focused);
        key(&mut rt, Key::Tab);
        key(&mut rt, Key::Enter);
        key(&mut rt, Key::Down);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().agent, "plan");
        tap(&mut rt, "effort");
        key(&mut rt, Key::Down); // high wraps to default
        key(&mut rt, Key::Down); // low
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().model.variant, "low");
        tap(&mut rt, "model");
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let (x, y) = rt.find("option-anthropic:claude-sonnet-5-5").unwrap();
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.selection().model.id, "claude-sonnet-5-5");
        assert!(rt.state.selection().model.variant.is_empty());
        let old_route = rt.state.route.clone();
        rt.state.route = Route::Draft;
        assert_eq!(rt.state.selection().agent, "build");
        rt.state.route = old_route;
        tap(&mut rt, "model");
        rt.set_viewport((960.0, 700.0), 1.0);
        rt.frame(1.0 / 60.0);
        let elements = rt.elements();
        let menu = elements.iter().find(|e| e.key.as_deref() == Some("picker-model")).unwrap();
        let anchor = elements.iter().find(|e| e.key.as_deref() == Some("model")).unwrap();
        assert!((menu.target[1] + menu.target[3] + 6.0 - anchor.target[1]).abs() < 0.1);
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.pickers.open, None);
        assert!(!rt.state.composer.focused);
    }

    #[test]
    fn filter_reflows_and_exit_alpha_includes_parent() {
        let mut rt = runtime();
        tap(&mut rt, "model");
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let rect = |rt: &Runtime<App>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
        let before = rect(&rt, "picker-model").rect;
        rt.key(KeyEvent { key: Key::Text("OpenAI".into()), mods: Default::default() });
        rt.frame(0.0);
        let filtered = rect(&rt, "picker-model");
        assert!((filtered.rect[3] - before[3]).abs() < 1.0, "filter must retarget from the visible height");
        assert!(filtered.target[3] < filtered.rect[3]);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        key(&mut rt, Key::Escape);
        for _ in 0..8 { rt.frame(1.0 / 60.0); }
        let menu = rect(&rt, "picker-model");
        let scroll = rect(&rt, "picker-scroll");
        assert!(menu.opacity < 0.9 && menu.opacity > 0.0);
        assert!((scroll.opacity - menu.opacity).abs() < 0.001);
        let option = rect(&rt, "option-openai:gpt-6-astra");
        rt.pointer_down(option.rect[0] + 15.0, option.rect[1] + 14.0);
        rt.pointer_up(option.rect[0] + 15.0, option.rect[1] + 14.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selection().model.id, "claude-opus-5-5", "exiting descendants must not accept clicks");
    }

    #[test]
    fn interrupted_reopen_uses_one_layout_curve() {
        let mut rt = runtime();
        tap(&mut rt, "model");
        key(&mut rt, Key::Text("missing".into()));
        for _ in 0..90 { rt.frame(1.0 / 120.0); }
        key(&mut rt, Key::Escape);
        for _ in 0..10 { rt.frame(1.0 / 120.0); }
        tap(&mut rt, "model");
        let mut previous = None;
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            let option = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("option-openai:gpt-6-astra")).unwrap();
            if option.opacity > 0.1 {
                let distance = (option.rect[1] - option.target[1]).abs();
                if let Some(last) = previous { assert!(distance <= last + 0.03, "reopening must not dog-leg: {last} -> {distance}"); }
                previous = Some(distance);
            }
        }
    }
}
