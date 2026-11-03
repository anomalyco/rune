//! Requests as jobs: each op becomes inputs to deliver, frames or time to wait for, and an
//! `until` to meet, advanced whenever the UI thread hands the driver a turn (a window's event
//! loop) or a frame (headless). One job runs at a time, in request order.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use serde_json::{Value, json};
use winit::keyboard::{Key as WKey, KeyCode, ModifiersState};

use super::chord::{Stroke, chord, typed};
use super::protocol::{Code, Fail, Request, num, reply, text};
use super::record;
use super::select::{Selector, centre};

/// How long quiet must last for `until: "idle"`.
pub(crate) const IDLE_NS: u64 = 300_000_000;
/// How long a step waits for the frame after an input before going on without it (a hidden or
/// covered surface presents none).
const FRAME_CAP_NS: u64 = 250_000_000;
const DEFAULT_TIMEOUT_MS: f64 = 30_000.0;

/// One input as an OS would deliver it to a surface, in its points.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Input {
    Move(f32, f32),
    Button(crate::PointerButton, bool),
    /// A precise wheel delta in points; positive moves content right and down (Rune's sign).
    Wheel(f32, f32),
    Mods(ModifiersState),
    Key { code: Option<KeyCode>, key: WKey, base: Option<char>, pressed: bool },
}

/// What a surface counts at a moment, for a run's differences.
#[derive(Clone, Debug, Default)]
pub(crate) struct SurfaceTally {
    pub key: String,
    pub counts: crate::SurfaceCounts,
    pub tally: super::Tally,
}

/// What the driver needs of the windows (or the headless runtime) it drives.
pub(crate) trait Backend {
    /// The clock waits and timeouts run on: the host's in a window, the virtual one headless.
    fn clock_ns(&self) -> u64;
    /// The surface an op's `window` names (the main window when absent).
    fn surface(&mut self, key: Option<&str>) -> Result<String, Fail>;
    fn find(&mut self, surface: &str, sel: &Selector) -> Option<[f32; 4]>;
    fn query(&mut self, surface: &str, sel: &Selector, all: bool) -> Vec<Value>;
    /// Delivers `input` and returns the host time it entered the runtime.
    fn deliver(&mut self, surface: &str, input: &Input) -> Result<u64, Fail>;
    /// Nothing animates, waits or asked for a frame on any surface.
    fn quiet(&self) -> bool;
    /// Counts work that ends quiet: a presented frame, or headless a frame that committed or moved.
    fn activity(&self) -> u64;
    fn info(&mut self) -> Value;
    fn window(&mut self, surface: &str, action: &str) -> Result<(), Fail>;
    fn snapshot(&mut self, surface: &str, path: &str) -> Result<Value, Fail>;
    fn step(&mut self, args: &Value) -> Result<Value, Fail>;
    fn quit(&mut self);
    fn tallies(&self) -> Vec<SurfaceTally>;
}

enum Step {
    Resolve(Selector),
    At(f32, f32),
    /// An input at the job's point; `stamp` puts its time in the reply.
    Input(Act, bool),
    Frame,
    Delay(u64),
    Scroll { left: f32, speed: f32 },
}

enum Act {
    Move,
    Button(crate::PointerButton, bool),
    Wheel(f32, f32),
    Mods(ModifiersState),
    Key(Stroke, bool),
}

enum Until {
    Selector(Selector),
    Frame,
    Idle,
}

struct Job {
    id: Value,
    surface: String,
    steps: VecDeque<Step>,
    until: Option<Until>,
    deadline: u64,
    /// A wait op replies `{at_ns, frame_seq}`; inputs reply `{input_ns, frame_seq}`.
    wait: bool,
    at: Option<(f32, f32)>,
    inputs: Vec<u64>,
    /// When the current gate step began, and the surface's frame then.
    gate: Option<(u64, u64)>,
    /// `activity` when the selector last didn't match.
    looked: Option<u64>,
    /// The until has been looked at once (a wait met at the first look was met already).
    looked_once: bool,
    until_seq: Option<u64>,
}

/// Where a job stands after a turn.
enum Progress {
    Done(Result<Value, Fail>),
    /// Waiting, until at most this schedule-clock time.
    Wait(u64),
}

/// What the UI thread should do next.
#[derive(Debug, PartialEq)]
pub(crate) enum Poll {
    /// No request is waiting.
    Idle,
    /// A job waits; poll again after a frame or by this schedule-clock time.
    Wait(u64),
}

pub(crate) struct Driver {
    rx: Receiver<Request>,
    out: Sender<String>,
    queue: VecDeque<Request>,
    job: Option<Job>,
    /// When quiet began: on the schedule clock and the host's.
    idle_since: Option<(u64, u64)>,
    activity: u64,
    base: Vec<SurfaceTally>,
    pub(crate) closed: bool,
}

impl Driver {
    pub(crate) fn new(rx: Receiver<Request>, out: Sender<String>) -> Self {
        Driver { rx, out, queue: VecDeque::new(), job: None, idle_since: None, activity: 0, base: Vec::new(), closed: false }
    }

    pub(crate) fn send(&self, line: String) { let _ = self.out.send(line); }

    /// A job runs or a request waits for one.
    pub(crate) fn busy(&self) -> bool { self.job.is_some() || !self.queue.is_empty() }

    /// Blocks until a request comes; false once the connection closed.
    pub(crate) fn wait_request(&mut self) -> bool {
        match self.rx.recv() {
            Ok(request) => { self.queue.push_back(request); true }
            Err(_) => { self.closed = true; false }
        }
    }

    /// Runs queued requests as far as they go now.
    pub(crate) fn poll(&mut self, b: &mut dyn Backend) -> Poll {
        loop {
            match self.rx.try_recv() {
                Ok(request) => { self.queue.push_back(request); continue; }
                Err(TryRecvError::Disconnected) => self.closed = true,
                Err(TryRecvError::Empty) => {}
            }
            break;
        }
        if self.closed { self.queue.clear(); self.job = None; return Poll::Idle; }
        let (now, activity, quiet) = (b.clock_ns(), b.activity().wrapping_add(record::inputs()), b.quiet());
        // Quiet starts over at any frame of work since the last turn.
        if activity != self.activity || !quiet { self.idle_since = None; }
        self.activity = activity;
        if quiet && self.idle_since.is_none() { self.idle_since = Some((now, super::clock::now_ns())); }
        loop {
            if self.job.is_none() {
                let Some(request) = self.queue.pop_front() else { return Poll::Idle };
                match self.start(request, b) {
                    Ok(job) => self.job = Some(job),
                    Err(line) => { if let Some(line) = line { self.send(line); } continue; }
                }
            }
            let Some(job) = self.job.as_mut() else { continue };
            match job.advance(b, self.idle_since) {
                Progress::Done(result) => {
                    let line = reply(&job.id, result);
                    self.job = None;
                    self.send(line);
                }
                Progress::Wait(at) => return Poll::Wait(at),
            }
        }
    }

    /// Answers `request` at once (the reply line, if any), or returns the job that will.
    fn start(&mut self, request: Request, b: &mut dyn Backend) -> Result<Job, Option<String>> {
        let Request { id, op, args } = request;
        let now_reply = |result: Result<Value, Fail>| Err(Some(reply(&id, result)));
        let surface = match b.surface(text(&args, "window")) {
            Ok(surface) => surface,
            Err(_) if matches!(op.as_str(), "info" | "mark" | "run.begin" | "run.end" | "quit" | "step") => String::new(),
            Err(fail) => return now_reply(Err(fail)),
        };
        let result = match op.as_str() {
            "info" => Ok(b.info()),
            "query" => (|| {
                let sel = Selector::parse(args.get("selector").ok_or_else(|| Fail::bad("query needs a \"selector\""))?)?;
                let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
                Ok(json!({ "elements": b.query(&surface, &sel, all) }))
            })(),
            "window" => match text(&args, "action") {
                Some(action @ ("show" | "hide" | "focus" | "close")) => b.window(&surface, action).map(|()| json!({})),
                _ => Err(Fail::bad("window needs an \"action\": show, hide, focus or close")),
            },
            "mark" => match text(&args, "name") {
                Some(name) => Ok(json!({ "at_ns": record::mark(name) })),
                None => Err(Fail::bad("mark needs a \"name\"")),
            },
            "run.begin" => {
                self.base = b.tallies();
                Ok(json!({ "at_ns": record::run_begin() }))
            }
            "run.end" => self.run_end(b),
            "snapshot" => match text(&args, "path") {
                Some(path) => b.snapshot(&surface, path),
                None => Err(Fail::bad("snapshot needs a \"path\"")),
            },
            "step" => b.step(&args),
            "quit" => {
                self.send(reply(&id, Ok(json!({}))));
                b.quit();
                return Err(None);
            }
            _ => return Job::of(id.clone(), &op, &args, surface, b.clock_ns()).map_err(|fail| Some(reply(&id, Err(fail)))),
        };
        now_reply(result)
    }

    fn run_end(&mut self, b: &mut dyn Backend) -> Result<Value, Fail> {
        let run = record::run_end().ok_or_else(|| Fail::new(Code::Internal, "the recorder isn't installed"))?;
        let now = b.tallies();
        let mut surfaces = serde_json::Map::new();
        let (mut commits, mut shaped, mut elements) = (0, 0, 0);
        for t in &now {
            let base = self.base.iter().find(|s| s.key == t.key).cloned().unwrap_or_default();
            let (c, z) = (t.counts, base.counts);
            commits += t.tally.commits.saturating_sub(base.tally.commits);
            shaped += t.tally.shaped.saturating_sub(base.tally.shaped);
            elements = elements.max(t.tally.elements);
            surfaces.insert(t.key.clone(), json!({
                "redraws": c.redraws - z.redraws.min(c.redraws), "unpaced": c.unpaced - z.unpaced.min(c.unpaced),
                "presents": c.presents - z.presents.min(c.presents), "skipped": c.skipped - z.skipped.min(c.skipped),
                "occluded": c.occluded - z.occluded.min(c.occluded), "timeouts": c.timeouts - z.timeouts.min(c.timeouts),
                "outdated": c.outdated - z.outdated.min(c.outdated), "turns": c.turns - z.turns.min(c.turns),
                "wakes": c.wakes - z.wakes.min(c.wakes),
            }));
        }
        // `elements` is the main window's tree, which `tallies` lists first.
        if let Some(main) = now.first() { elements = main.tally.elements; }
        let mut out = run.json;
        out["counts"] = json!({
            "frames": run.frames, "commits": commits, "elements": elements, "glyphs_shaped": shaped,
            "instances_max": run.instances_max, "frames_dropped_from_ring": run.dropped,
        });
        out["surfaces"] = Value::Object(surfaces);
        out["surface_keys"] = json!(record::surface_keys());
        Ok(out)
    }
}

/// An op's `until`, if it has one.
fn until(args: &Value) -> Result<Option<Until>, Fail> {
    Ok(match args.get("until") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s == "frame" => Some(Until::Frame),
        Some(Value::String(s)) if s == "idle" => Some(Until::Idle),
        Some(sel) => Some(Until::Selector(Selector::parse(sel)?)),
    })
}

fn button(args: &Value) -> Result<crate::PointerButton, Fail> {
    match text(args, "button").unwrap_or("left") {
        "left" => Ok(crate::PointerButton::Primary),
        "right" => Ok(crate::PointerButton::Secondary),
        "middle" => Ok(crate::PointerButton::Middle),
        other => Err(Fail::bad(format!("unknown button {other:?}; use left, right or middle"))),
    }
}

fn modifiers(args: &Value) -> Result<ModifiersState, Fail> {
    let mut mods = ModifiersState::empty();
    for m in args.get("modifiers").and_then(Value::as_array).into_iter().flatten() {
        mods |= match m.as_str().unwrap_or("") {
            "ctrl" | "control" => ModifiersState::CONTROL,
            "shift" => ModifiersState::SHIFT,
            "alt" => ModifiersState::ALT,
            "meta" => ModifiersState::SUPER,
            other => return Err(Fail::bad(format!("unknown modifier {other:?}; use ctrl, shift, alt or meta"))),
        };
    }
    Ok(mods)
}

impl Job {
    fn of(id: Value, op: &str, args: &Value, surface: String, now: u64) -> Result<Job, Fail> {
        let timeout = num(args, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS).max(0.0);
        let mut steps = VecDeque::new();
        // Where a pointer op acts: its selector's centre, else its x and y.
        let target = |steps: &mut VecDeque<Step>, required: bool| -> Result<(), Fail> {
            if let Some(sel) = args.get("selector") {
                steps.push_back(Step::Resolve(Selector::parse(sel)?));
            } else if let (Some(x), Some(y)) = (num(args, "x"), num(args, "y")) {
                steps.push_back(Step::At(x as f32, y as f32));
            } else if required {
                return Err(Fail::bad(format!("{op} needs a \"selector\" or \"x\" and \"y\"")));
            }
            Ok(())
        };
        let wait = op == "wait";
        let until = until(args)?;
        match op {
            "wait" => if until.is_none() { return Err(Fail::bad("wait needs \"until\"")) },
            "click" => {
                target(&mut steps, true)?;
                let (b, mods) = (button(args)?, modifiers(args)?);
                let count = num(args, "count").unwrap_or(1.0).clamp(1.0, 3.0) as usize;
                steps.push_back(Step::Input(Act::Move, false));
                if !mods.is_empty() { steps.push_back(Step::Input(Act::Mods(mods), false)); }
                for k in 0..count {
                    if k > 0 { steps.push_back(Step::Frame); }
                    steps.push_back(Step::Input(Act::Button(b, true), true));
                    steps.push_back(Step::Frame);
                    steps.push_back(Step::Input(Act::Button(b, false), true));
                }
                if !mods.is_empty() { steps.push_back(Step::Input(Act::Mods(ModifiersState::empty()), false)); }
            }
            "move" | "hover" => {
                target(&mut steps, true)?;
                steps.push_back(Step::Input(Act::Move, true));
            }
            "down" | "up" => {
                target(&mut steps, false)?;
                let b = button(args)?;
                if steps.front().is_some() { steps.push_back(Step::Input(Act::Move, false)); }
                steps.push_back(Step::Input(Act::Button(b, op == "down"), true));
            }
            "wheel" => {
                let has_target = args.get("selector").is_some() || num(args, "x").is_some();
                target(&mut steps, false)?;
                if has_target { steps.push_back(Step::Input(Act::Move, false)); }
                let (dx, dy) = (num(args, "dx").unwrap_or(0.0) as f32, num(args, "dy").unwrap_or(0.0) as f32);
                steps.push_back(Step::Input(Act::Wheel(-dx, -dy), true));
            }
            "scroll" => {
                target(&mut steps, true)?;
                let by = num(args, "by").ok_or_else(|| Fail::bad("scroll needs \"by\""))? as f32;
                let speed = num(args, "speed").unwrap_or(3000.0).max(1.0) as f32;
                steps.push_back(Step::Input(Act::Move, false));
                steps.push_back(Step::Scroll { left: by, speed });
            }
            "type" => {
                let text = text(args, "text").ok_or_else(|| Fail::bad("type needs \"text\""))?;
                if args.get("selector").is_some() {
                    target(&mut steps, false)?;
                    steps.push_back(Step::Input(Act::Move, false));
                    steps.push_back(Step::Input(Act::Button(crate::PointerButton::Primary, true), false));
                    steps.push_back(Step::Frame);
                    steps.push_back(Step::Input(Act::Button(crate::PointerButton::Primary, false), false));
                    steps.push_back(Step::Frame);
                }
                let delay = (num(args, "delay_ms").unwrap_or(0.0).max(0.0) * 1e6) as u64;
                for (k, c) in text.chars().enumerate() {
                    if k > 0 && delay > 0 { steps.push_back(Step::Delay(delay)); }
                    push_stroke(&mut steps, typed(c));
                }
            }
            "press" => {
                let input = text(args, "chord").ok_or_else(|| Fail::bad("press needs a \"chord\""))?;
                push_stroke(&mut steps, chord(input).map_err(Fail::bad)?);
            }
            other => return Err(Fail::new(Code::BadRequest, format!("unknown op {other:?}"))),
        }
        let deadline = now.saturating_add((timeout * 1e6) as u64);
        Ok(Job { id, surface, steps, until, deadline, wait, at: None, inputs: Vec::new(), gate: None, looked: None, looked_once: false, until_seq: None })
    }

    fn advance(&mut self, b: &mut dyn Backend, idle_since: Option<(u64, u64)>) -> Progress {
        loop {
            let now = b.clock_ns();
            let Some(step) = self.steps.front_mut() else { break };
            match step {
                Step::Resolve(sel) => {
                    if self.looked == Some(b.activity()) && now < self.deadline { return Progress::Wait(self.deadline); }
                    match b.find(&self.surface, sel) {
                        Some(rect) => { self.at = Some(centre(rect)); self.looked = None; }
                        None if now >= self.deadline => return Progress::Done(Err(Fail::new(Code::NotFound, "the selector matched nothing visible before the timeout"))),
                        None => { self.looked = Some(b.activity()); return Progress::Wait(self.deadline); }
                    }
                }
                &mut Step::At(x, y) => self.at = Some((x, y)),
                Step::Input(act, stamp) => {
                    let (x, y) = self.at.unwrap_or((0.0, 0.0));
                    let input = match act {
                        Act::Move => Input::Move(x, y),
                        &mut Act::Button(button, pressed) => Input::Button(button, pressed),
                        &mut Act::Wheel(dx, dy) => Input::Wheel(dx, dy),
                        &mut Act::Mods(mods) => Input::Mods(mods),
                        Act::Key(stroke, pressed) => Input::Key { code: stroke.code, key: stroke.key.clone(), base: stroke.base, pressed: *pressed },
                    };
                    let stamp = *stamp;
                    match b.deliver(&self.surface, &input) {
                        Ok(ns) => if stamp {
                            self.inputs.push(ns);
                            record::input(&self.surface, ns, kind(&input));
                        },
                        Err(fail) => return Progress::Done(Err(fail)),
                    }
                }
                Step::Frame => {
                    let seq = record::seq(&self.surface);
                    let (since, at) = *self.gate.get_or_insert((now, seq));
                    if seq <= at && now < since + FRAME_CAP_NS { return Progress::Wait(since + FRAME_CAP_NS); }
                }
                &mut Step::Delay(ns) => {
                    let (since, _) = *self.gate.get_or_insert((now, 0));
                    if now < since + ns { return Progress::Wait(since + ns); }
                }
                Step::Scroll { left, speed } => {
                    let seq = record::seq(&self.surface);
                    // One wheel step a presented frame, as far as `speed` goes in the time since the last.
                    let dt = match self.gate {
                        None => 1.0 / 60.0,
                        Some((since, at)) if seq > at || now >= since + FRAME_CAP_NS => (now - since) as f32 / 1e9,
                        Some((since, _)) => return Progress::Wait(since + FRAME_CAP_NS),
                    };
                    let step = (*speed * dt).min(left.abs()).copysign(*left);
                    *left -= step;
                    let done = left.abs() < 0.01;
                    match b.deliver(&self.surface, &Input::Wheel(0.0, -step)) {
                        Ok(ns) => {
                            self.inputs.push(ns);
                            record::input(&self.surface, ns, record::InputKind::Scroll);
                        }
                        Err(fail) => return Progress::Done(Err(fail)),
                    }
                    if !done {
                        self.gate = Some((now, seq));
                        return Progress::Wait(now + FRAME_CAP_NS);
                    }
                }
            }
            self.steps.pop_front();
            self.gate = None;
            // Quiet starts over at the input: look again on the next turn, which knows it.
            if self.steps.is_empty() && matches!(self.until, Some(Until::Idle)) && !self.inputs.is_empty() && !self.looked_once {
                self.looked_once = true;
                return Progress::Wait(now);
            }
        }
        let now = b.clock_ns();
        let first = !std::mem::replace(&mut self.looked_once, true);
        let last = record::last_frame(&self.surface);
        // When the condition was met, on the host clock: the matching frame's commit, the new
        // frame's submission, or when the quiet began.
        let at = match &self.until {
            None => Some(super::clock::now_ns()),
            Some(Until::Selector(sel)) => {
                if self.looked == Some(b.activity()) && now < self.deadline { return Progress::Wait(self.deadline); }
                let found = b.find(&self.surface, sel).is_some();
                if !found { self.looked = Some(b.activity()); }
                found.then_some(last.commit_ns)
            }
            Some(Until::Frame) => (last.seq > *self.until_seq.get_or_insert(last.seq)).then_some(last.submit_ns),
            Some(Until::Idle) => idle_since.filter(|&(since, _)| now >= since + IDLE_NS).map(|(_, host)| host),
        };
        if let Some(at_ns) = at {
            return Progress::Done(Ok(if self.wait {
                // Met before any frame came since the request arrived.
                json!({ "at_ns": at_ns, "frame_seq": last.seq, "already": first && !matches!(self.until, Some(Until::Idle)) })
            } else {
                json!({ "input_ns": self.inputs, "frame_seq": last.seq })
            }));
        }
        if now >= self.deadline {
            let what = match &self.until { Some(Until::Selector(_)) => "the selector", Some(Until::Frame) => "the frame", _ => "idle" };
            return Progress::Done(Err(Fail::new(Code::Timeout, format!("{what} wasn't met before the timeout"))));
        }
        let idle_at = match (&self.until, idle_since) { (Some(Until::Idle), Some((since, _))) => since + IDLE_NS, _ => self.deadline };
        Progress::Wait(idle_at.min(self.deadline))
    }
}

/// What kind of input a frame reflecting input reports.
fn kind(input: &Input) -> record::InputKind {
    if matches!(input, Input::Wheel(..)) { record::InputKind::Scroll } else { record::InputKind::Discrete }
}

/// A keystroke: its modifiers down, the key down and up, the modifiers up.
fn push_stroke(steps: &mut VecDeque<Step>, stroke: Stroke) {
    let mods = stroke.mods;
    if !mods.is_empty() { steps.push_back(Step::Input(Act::Mods(mods), false)); }
    steps.push_back(Step::Input(Act::Key(stroke.clone(), true), true));
    steps.push_back(Step::Input(Act::Key(stroke, false), false));
    if !mods.is_empty() { steps.push_back(Step::Input(Act::Mods(ModifiersState::empty()), false)); }
}
