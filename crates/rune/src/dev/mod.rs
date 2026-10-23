//! Dev-only knobs control channel. Enable `rune/dev` and opt in with `App::dev_panel`.
//! This module is absent from release builds, even when the feature is selected.
mod transport;
mod tune;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use serde_json::{Value, json};
use crate::{AppSpec, Runtime, Tx, Waker};
use transport::{Bridge, Server};
pub use tune::{Tunable, tune};
pub(crate) use tune::scope;

/// Replay an app action after one settled frame of its source scene.
pub struct Transition<S: 'static> {
    pub name: &'static str,
    pub from: &'static str,
    pub action: fn(&mut S, &mut Tx),
}

impl<S> Copy for Transition<S> {}
impl<S> Clone for Transition<S> {
    fn clone(&self) -> Self { *self }
}

impl<S> Transition<S> {
    pub const fn new(name: &'static str, from: &'static str, action: fn(&mut S, &mut Tx)) -> Self {
        Self { name, from, action }
    }
}

pub struct Config<S: 'static> {
    pub bind: SocketAddr,
    pub transitions: Vec<Transition<S>>,
    /// Advertise the blur toggle only when the app's compositor reads `Runtime::motion_blur_enabled`.
    pub motion_blur: bool,
}

impl<S> Default for Config<S> {
    fn default() -> Self {
        Self { bind: SocketAddr::from(([127, 0, 0, 1], 4793)), transitions: Vec::new(), motion_blur: false }
    }
}

impl<S> Config<S> {
    pub fn transitions(mut self, transitions: impl IntoIterator<Item = Transition<S>>) -> Self {
        self.transitions = transitions.into_iter().collect();
        self
    }
}

pub(crate) struct Session<S: 'static> {
    registry: tune::Registry,
    bridge: Arc<Mutex<Bridge>>,
    server: Server,
    wake: Waker,
    scenes: &'static [(&'static str, fn(&mut S))],
    init: fn() -> S,
    setup: Option<fn(&mut S, Waker)>,
    transitions: Vec<Transition<S>>,
    pending: Option<fn(&mut S, &mut Tx)>,
    state: String,
    selection: u64,
    blur_control: bool,
    pub(crate) blur: bool,
    velocity: bool,
    frame: u64,
    replies: Vec<(Value, std::sync::mpsc::Sender<Value>)>,
    requests: std::collections::VecDeque<transport::Request>,
}

/// Attach to an explicitly constructed runtime (headless tools use the same channel).
pub fn attach<S>(rt: &mut Runtime<S>, spec: &AppSpec<S>, config: &Config<S>, wake: Waker) -> std::io::Result<SocketAddr> {
    use std::io::{Error, ErrorKind};
    if rt.dev.is_some() { return Err(Error::new(ErrorKind::AlreadyExists, "a dev panel is already attached")); }
    let mut names = std::collections::HashSet::new();
    for (name, _) in spec.scenes {
        if !names.insert(*name) { return Err(Error::new(ErrorKind::InvalidInput, "duplicate scene name")); }
    }
    names.clear();
    for transition in &config.transitions {
        if !spec.scenes.iter().any(|(name, _)| *name == transition.from) || !names.insert(transition.name) {
            return Err(Error::new(ErrorKind::InvalidInput, "transition needs a unique name and a registered source scene"));
        }
    }
    let bridge = Arc::new(Mutex::new(Bridge::default()));
    let server = Server::bind(config.bind, bridge.clone(), wake.clone())?;
    let address = server.address;
    rt.dev_clock = Some(crate::dev_clock::DevClock::new(rt.wall_start.map_or(rt.clock, |t| t.elapsed().as_secs_f64()), rt.clock));
    rt.dev = Some(Session {
        registry: Arc::new(Mutex::new(BTreeMap::new())), bridge, server, wake,
        scenes: spec.scenes, transitions: config.transitions.clone(), pending: None, state: String::new(), selection: 0,
        init: spec.init, setup: spec.setup,
        blur_control: config.motion_blur, blur: true, velocity: false, frame: 0, replies: Vec::new(), requests: Default::default(),
    });
    rt.invalidate();
    eprintln!("Rune dev controls: ws://{address} (scripts/dev-panel)");
    Ok(address)
}

impl<S> Session<S> {
    pub(crate) fn scope(&self) -> tune::Registry { self.registry.clone() }

    pub(crate) fn prepare(&mut self) -> bool {
        // Freeze this frame's batch before deciding how to advance time. A command arriving
        // later wakes the next frame instead of inheriting the pre-command idle interval.
        self.requests = std::mem::take(&mut self.bridge.lock().unwrap().incoming);
        self.pending.is_some() || !self.requests.is_empty()
    }

    pub(crate) fn before(&mut self, rt: &mut Runtime<S>) {
        let pending = self.pending.take();
        let selection = self.selection;
        let requests = std::mem::take(&mut self.requests);
        for request in requests {
            match self.command(rt, &request.message) {
                Ok(()) => self.replies.push((request.message, request.reply)),
                Err(message) => {
                    // A disconnected requester needs no response; it never rolls back a command.
                    if request.reply.send(json!({"type":"error","message":message,"id":request.message["id"]})).is_err() {}
                }
            }
        }
        // A new scene/replay cancels the old action before it can cause app side effects.
        if self.selection == selection && let Some(action) = pending { rt.update(action); }
        if let Some(mut trace) = rt.scene.take_trace() {
            let clock = rt.dev_clock.as_ref().unwrap();
            trace.set_time_scale(if clock.paused { 0.0 } else { clock.scale as f32 * rt.time_scale });
            rt.scene.set_trace(Some(trace));
        }
    }

    fn scene(&mut self, rt: &mut Runtime<S>, name: &str) -> Result<(), String> {
        let (_, scene) = self.scenes.iter().find(|(n, _)| *n == name).ok_or("unknown scene")?;
        // Scenes are patches of the initial state, exactly as --scene. In particular a
        // default scene may be a no-op; applying it to the previous scene would not reset it.
        let mut next = (self.init)();
        scene(&mut next);
        if let Some(setup) = self.setup { setup(&mut next, self.wake.clone()); }
        rt.update(|state, tx| { *state = next; tx.settle(); });
        self.state = name.to_owned();
        self.selection = self.selection.wrapping_add(1);
        self.pending = None;
        Ok(())
    }

    fn command(&mut self, rt: &mut Runtime<S>, message: &Value) -> Result<(), String> {
        match message["type"].as_str().ok_or("missing message type")? {
            "tree" => Ok(()), // Snapshot follows the committed frame.
            "action" => match message["key"].as_str().ok_or("missing action key")? {
                "rune.step" => { rt.dev_clock.as_mut().unwrap().step(); Ok(()) }
                key => {
                    let name = key.strip_prefix("rune.transition.").ok_or("unknown action")?;
                    let transition = self.transitions.iter().find(|t| t.name == name).ok_or("unknown transition")?;
                    let (from, action) = (transition.from, transition.action);
                    self.scene(rt, from)?;
                    self.pending = Some(action);
                    Ok(())
                }
            },
            "set" => self.set_batch(rt, &[(message["key"].as_str().ok_or("missing key")?.to_owned(), message.get("value").ok_or("missing value")?.clone())]),
            "setBatch" => {
                let changes = message["changes"].as_array().ok_or("missing changes")?;
                if changes.len() > 256 { return Err("too many changes".into()); }
                let changes = changes.iter().map(|c| Ok((c["key"].as_str().ok_or("missing key")?.to_owned(), c.get("value").ok_or("missing value")?.clone()))).collect::<Result<Vec<_>, String>>()?;
                self.set_batch(rt, &changes)
            }
            "reset" => {
                if let Some(key) = message["key"].as_str() { return self.set_batch(rt, &[(key.to_owned(), Value::Null)]); }
                let group = message["group"].as_str();
                let mut changes = self.registry.lock().unwrap().iter()
                    .filter(|(_, e)| group.is_none_or(|g| e.descriptor["group"] == g))
                    .map(|(key, _)| (key.clone(), Value::Null)).collect::<Vec<_>>();
                if group.is_none_or(|g| g == "Playback") {
                    changes.extend(["rune.scale", "rune.paused", "rune.velocity"].map(|k| (k.to_owned(), Value::Null)));
                    if self.blur_control { changes.push(("rune.motionBlur".into(), Value::Null)); }
                }
                if !self.scenes.is_empty() && group.is_none_or(|g| g == "Scenes") {
                    changes.push(("rune.state".into(), Value::Null));
                }
                self.set_batch(rt, &changes)
            }
            _ => Err("unknown message type".into()),
        }
    }

    fn set_batch(&mut self, rt: &mut Runtime<S>, changes: &[(String, Value)]) -> Result<(), String> {
        // Validate the entire batch before changing either app state or registry values.
        let mut normalized = Vec::with_capacity(changes.len());
        {
            let entries = self.registry.lock().unwrap();
            for (key, value) in changes {
                let normalized_value = match key.as_str() {
                    "rune.state" => {
                        let name = value.as_str().or_else(|| value.is_null().then(|| self.scenes.first().map(|s| s.0)).flatten()).ok_or("state needs a scene name")?;
                        if !self.scenes.iter().any(|(n, _)| *n == name) { return Err("unknown scene".into()); }
                        json!(name)
                    }
                    "rune.scale" => {
                        let n = if value.is_null() { 1.0 } else { value.as_f64().ok_or("scale needs a number")? };
                        if !(0.1..=1.0).contains(&n) { return Err("scale must be 0.1–1".into()); }
                        json!(n)
                    }
                    "rune.paused" => json!(if value.is_null() { false } else { value.as_bool().ok_or("pause needs a boolean")? }),
                    "rune.velocity" => json!(if value.is_null() { false } else { value.as_bool().ok_or("velocity needs a boolean")? }),
                    "rune.motionBlur" if self.blur_control => json!(if value.is_null() { true } else { value.as_bool().ok_or("motion blur needs a boolean")? }),
                    _ => entries.get(key).ok_or_else(|| format!("unknown tunable: {key}"))?.validate(value)?,
                };
                normalized.push((key, normalized_value));
            }
        }
        for (key, value) in normalized {
            match key.as_str() {
                "rune.state" => self.scene(rt, value.as_str().unwrap())?,
                "rune.scale" => rt.dev_clock.as_mut().unwrap().scale = value.as_f64().unwrap(),
                "rune.paused" => rt.dev_clock.as_mut().unwrap().paused = value.as_bool().unwrap(),
                "rune.velocity" => self.velocity = value.as_bool().unwrap(),
                "rune.motionBlur" => self.blur = value.as_bool().unwrap(),
                _ => {
                    self.registry.lock().unwrap().get_mut(key).unwrap().value = value;
                    rt.scene.store_mut().retune_springs = true;
                    rt.invalidate();
                }
            }
        }
        Ok(())
    }

    pub(crate) fn after(&mut self, rt: &mut Runtime<S>) -> bool {
        self.frame += 1;
        rt.scene.store_mut().retune_springs = false;
        if self.velocity { rt.dev_velocity_overlay(); }
        let clock = rt.dev_clock.as_ref().unwrap();
        let mut descriptors = vec![
            json!({"key":"rune.scale","kind":"number","label":"Speed","group":"Playback","default":1,"min":0.1,"max":1,"step":0.1,"unit":"×"}),
            json!({"key":"rune.paused","kind":"toggle","label":"Pause","group":"Playback","default":false}),
            json!({"key":"rune.step","kind":"action","label":"Step frame","group":"Playback"}),
            json!({"key":"rune.time","kind":"watch","label":"Time","group":"Playback","unit":"s","decimals":3}),
            json!({"key":"rune.velocity","kind":"toggle","label":"Layout velocity","group":"Playback","default":false}),
        ];
        let mut values = serde_json::Map::from_iter([
            ("rune.scale".into(), json!(clock.scale)), ("rune.paused".into(), json!(clock.paused)),
            ("rune.velocity".into(), json!(self.velocity)),
        ]);
        if !self.scenes.is_empty() {
            descriptors.push(json!({"key":"rune.state","kind":"choice","label":"State","group":"Scenes","default":self.scenes[0].0,"choices":self.scenes.iter().map(|s| s.0).collect::<Vec<_>>()}));
            if !self.state.is_empty() { values.insert("rune.state".into(), json!(self.state)); }
        }
        for transition in &self.transitions {
            descriptors.push(json!({"key":format!("rune.transition.{}",transition.name),"kind":"action","label":transition.name,"group":"Transitions"}));
        }
        if self.blur_control {
            descriptors.push(json!({"key":"rune.motionBlur","kind":"toggle","label":"Motion blur","group":"Playback","default":true}));
            values.insert("rune.motionBlur".into(), json!(self.blur));
        }
        for (key, entry) in self.registry.lock().unwrap().iter() {
            descriptors.push(entry.descriptor.clone());
            values.insert(key.clone(), entry.value.clone());
        }
        let snapshot = json!({"type":"state","protocol":1,"descriptors":descriptors,"values":values,"readouts":{"rune.time":rt.clock},"frame":self.frame});
        {
            let mut bridge = self.bridge.lock().unwrap();
            bridge.latest = snapshot;
            bridge.revision += 1;
        }
        for (request, reply) in self.replies.drain(..) {
            let response = if request["type"] == "tree" {
                json!({"type":"tree","id":request["id"],"frame":self.frame,"elements":rt.elements().into_iter().map(|e| json!({"key":e.key,"text":e.text,"rect":e.rect,"target":e.target,"moving":e.moving})).collect::<Vec<_>>()})
            } else { json!({"type":"ack","id":request["id"],"frame":self.frame}) };
            if reply.send(response).is_err() {} // The client may disconnect before its commit.
        }
        if self.pending.is_some() {
            (self.wake)();
            rt.animating = true; // Replay needs its action frame even when the clock is paused.
        }
        rt.animating
    }
}

impl<S> Runtime<S> {
    /// The actual listening address, including an OS-selected port when configured with port 0.
    pub fn dev_address(&self) -> Option<SocketAddr> { self.dev.as_ref().map(|dev| dev.server.address) }
}

#[cfg(test)]
mod tests;
