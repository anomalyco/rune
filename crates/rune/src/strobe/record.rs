//! What a run records on the UI thread: one row per frame a surface presents, in a ring
//! allocated once, and the thread's frame work: turn wall time, CPU time and exclusive phases.
//! Every hook is a thread-local flag check while no bridge is connected.

use std::cell::{Cell, RefCell};

use serde_json::{Value, json};

use super::clock::{cpu_ns, now_ns, thread_cpu, thread_id};

/// Rows the ring holds; later frames in a run are counted, not kept.
pub(crate) const RING: usize = 65_536;
/// Columns of a frame row. `input_kind` describes the input `input_ns` stamps.
pub(crate) const COLUMNS: [&str; COLS] = ["seq", "surface", "tid", "begin_ns", "cpu_end_ns", "submit_ns", "present_ns", "input_ns", "input_kind"];
pub(crate) const COLS: usize = 9;

/// `input_kind`: what the input a frame first reflects was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputKind {
    /// A press, release, key or pointer move.
    Discrete = 1,
    /// A wheel or scroll step.
    Scroll = 2,
}

/// The exclusive phases a run reports, in `work`'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part { View, Layout, Text, Paint, Encode }
const PARTS: usize = 5;

impl Part {
    /// The bridge's part for one of the runtime's phase scopes. Commit and Apply (diffing the
    /// tree, mounting and retargeting) count as layout; nested scopes are taken out of them.
    pub(crate) fn of(phase: crate::frame::profile::Phase) -> Part {
        use crate::frame::profile::Phase as P;
        match phase {
            P::View => Part::View,
            P::Commit | P::Layout | P::Apply => Part::Layout,
            P::Text => Part::Text,
            P::Paint => Part::Paint,
        }
    }
}

struct Surface {
    key: Box<str>,
    seq: u64,
    /// The earliest input delivered since its last presented frame (0: none).
    input_ns: u64,
    input_kind: u64,
    /// When its last presented frame committed and was submitted.
    commit_ns: u64,
    submit_ns: u64,
}

/// A surface's latest presented frame: its number and when it committed and was submitted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LastFrame {
    pub seq: u64,
    pub commit_ns: u64,
    pub submit_ns: u64,
}

pub(crate) struct Recorder {
    rows: Box<[[u64; COLS]]>,
    len: usize,
    dropped: u64,
    surfaces: Vec<Surface>,
    presents: u64,
    inputs: u64,
    running: bool,
    t0: u64,
    cpu0: u64,
    turn_start: u64,
    turn_work: bool,
    turn_bridge: u64,
    /// Time blocked for drawables: this turn's, the run's, and since when it's blocked now.
    turn_waited: u64,
    waited: u64,
    wait_since: u64,
    wall: u64,
    /// The bridge's own time and thread CPU since the run began, taken out of the run's.
    bridge_since: Option<(u64, u64)>,
    bridge_cpu: u64,
    stack: [Part; 32],
    depth: usize,
    part_since: u64,
    parts: [u64; PARTS],
    cpu_end: u64,
    /// When the runtime last built a frame, for the frame presented next.
    built_at: u64,
    instances_max: usize,
    marks: Vec<(String, u64)>,
    tid: u64,
    #[cfg(target_os = "macos")]
    present_times: super::present::Presents,
}

thread_local! {
    static LIVE: Cell<bool> = const { Cell::new(false) };
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static REC: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

/// Allocates the ring on this (the UI) thread: from here on its hooks record.
pub(crate) fn install() {
    REC.with(|r| *r.borrow_mut() = Some(Recorder::new()));
    LIVE.set(true);
}

/// Whether a bridge is connected on this thread.
#[inline(always)]
pub(crate) fn live() -> bool { LIVE.get() }

fn with<R>(f: impl FnOnce(&mut Recorder) -> R) -> Option<R> {
    REC.with(|r| r.borrow_mut().as_mut().map(f))
}

impl Recorder {
    fn new() -> Self {
        Recorder {
            rows: vec![[0; COLS]; RING].into_boxed_slice(), len: 0, dropped: 0, surfaces: Vec::new(), presents: 0, inputs: 0,
            running: false, t0: 0, cpu0: 0, turn_start: now_ns(), turn_work: false, turn_bridge: 0, turn_waited: 0, waited: 0, wait_since: 0, wall: 0,
            bridge_since: None, bridge_cpu: 0, stack: [Part::View; 32], depth: 0, part_since: 0, parts: [0; PARTS],
            cpu_end: 0, built_at: 0, instances_max: 0, marks: Vec::new(), tid: thread_id(),
            #[cfg(target_os = "macos")]
            present_times: super::present::Presents::new(RING),
        }
    }

    fn surface(&mut self, key: &str) -> usize {
        match self.surfaces.iter().position(|s| &*s.key == key) {
            Some(i) => i,
            None => { self.surfaces.push(Surface { key: key.into(), seq: 0, input_ns: 0, input_kind: 0, commit_ns: 0, submit_ns: 0 }); self.surfaces.len() - 1 }
        }
    }

    fn enter(&mut self, part: Part, now: u64) {
        if let Some(&top) = self.stack[..self.depth].last() { self.parts[top as usize] += now.saturating_sub(self.part_since); }
        if self.depth < self.stack.len() { self.stack[self.depth] = part; self.depth += 1; }
        self.part_since = now;
        self.turn_work = true;
    }

    fn exit(&mut self, now: u64) {
        if self.depth == 0 { return; }
        let top = self.stack[self.depth - 1];
        self.parts[top as usize] += now.saturating_sub(self.part_since);
        self.depth -= 1;
        self.part_since = now;
    }

    fn unwind(&mut self, now: u64) { while self.depth > 0 { self.exit(now); } }
}

/// A runtime phase scope began. Only while a run records.
#[inline(always)]
pub(crate) fn phase_enter(phase: crate::frame::profile::Phase) {
    if RECORDING.get() { with(|r| r.enter(Part::of(phase), now_ns())); }
}

#[inline(always)]
pub(crate) fn phase_exit() {
    if RECORDING.get() { with(|r| r.exit(now_ns())); }
}

/// An event-loop turn (or a headless frame) begins.
pub(crate) fn turn_begin() {
    if !live() { return; }
    with(|r| { r.turn_start = now_ns(); r.turn_work = false; r.turn_bridge = 0; r.turn_waited = 0; });
}

/// The turn ends: its wall time counts when it built a frame or ran a phase.
pub(crate) fn turn_end() {
    if !live() { return; }
    with(|r| {
        let now = now_ns();
        r.unwind(now);
        if r.running && r.turn_work { r.wall += now.saturating_sub(r.turn_start).saturating_sub(r.turn_bridge).saturating_sub(r.turn_waited); }
        r.turn_work = false;
        r.turn_bridge = 0;
        r.turn_waited = 0;
        r.turn_start = now;
    });
}

/// The bridge's own work starts; [`bridge_end`] takes it out of the turn and the run.
pub(crate) fn bridge_begin() {
    with(|r| r.bridge_since = Some((now_ns(), thread_cpu())));
}

pub(crate) fn bridge_end() {
    with(|r| {
        let Some((t, c)) = r.bridge_since.take() else { return };
        r.turn_bridge += now_ns().saturating_sub(t);
        if r.running { r.bridge_cpu += thread_cpu().saturating_sub(c); }
    });
}

/// The UI thread blocks for a drawable (the present queue's pacing): that time isn't work.
#[inline(always)]
pub(crate) fn wait_begin() {
    if RECORDING.get() { with(|r| r.wait_since = now_ns()); }
}

#[inline(always)]
pub(crate) fn wait_end() {
    if !RECORDING.get() { return; }
    with(|r| {
        if r.wait_since == 0 { return; }
        let waited = now_ns().saturating_sub(std::mem::take(&mut r.wait_since));
        r.turn_waited += waited;
        r.waited += waited;
        // The phase it blocked inside doesn't count it either.
        r.part_since += waited;
    });
}

/// The runtime built (committed) the frame: encoding starts.
#[inline(always)]
pub(crate) fn built() {
    if !live() { return; }
    with(|r| {
        let now = now_ns();
        r.built_at = now;
        if r.running { r.enter(Part::Encode, now); }
    });
}

/// Encoding is done and the frame goes to the GPU.
#[inline(always)]
pub(crate) fn encoded() {
    if RECORDING.get() { with(|r| r.cpu_end = now_ns()); }
}

/// `key`'s frame was presented, drawing `instances` instances.
pub(crate) fn presented(key: &str, instances: usize) {
    if !live() { return; }
    with(|r| {
        let now = now_ns();
        if r.depth > 0 && r.stack[r.depth - 1] == Part::Encode { r.exit(now); }
        let i = r.surface(key);
        let built = std::mem::take(&mut r.built_at);
        let s = &mut r.surfaces[i];
        s.seq += 1;
        (s.commit_ns, s.submit_ns) = (if built != 0 { built } else { now }, now);
        let (seq, input, kind) = (s.seq, std::mem::take(&mut s.input_ns), std::mem::take(&mut s.input_kind));
        r.presents += 1;
        if !r.running { return; }
        r.turn_work = true;
        r.instances_max = r.instances_max.max(instances);
        let begin = r.turn_start;
        let cpu_end = if r.cpu_end >= begin && r.cpu_end != 0 { r.cpu_end } else { now };
        r.cpu_end = 0;
        if r.len < RING {
            r.rows[r.len] = [seq, i as u64, r.tid, begin, cpu_end, now, 0, input, kind];
            #[cfg(target_os = "macos")]
            r.present_times.clear_row(r.len);
            r.len += 1;
        } else {
            r.dropped += 1;
        }
    });
}

/// macOS: the frame `key` just presented went out in `drawable` of the window's present-time
/// `log`, which fills its row's `present_ns` at the run's end.
#[cfg(target_os = "macos")]
pub(crate) fn drawable(key: &str, drawable: Option<u64>, log: &std::sync::Arc<crate::frame::present_time::metal::Log>) {
    if !live() { return; }
    with(|r| {
        let i = r.surface(key);
        let row = (r.running && r.len > 0 && r.rows[r.len - 1][1] == i as u64 && r.rows[r.len - 1][0] == r.surfaces[i].seq).then(|| r.len - 1);
        r.present_times.note(i, row, drawable, log);
    });
}

/// An input reached `key`'s runtime at `ns`.
pub(crate) fn input(key: &str, ns: u64, kind: InputKind) {
    with(|r| {
        r.inputs += 1;
        let i = r.surface(key);
        let s = &mut r.surfaces[i];
        if s.input_ns == 0 { (s.input_ns, s.input_kind) = (ns, kind as u64); }
    });
}

/// Frames `key` has presented since the bridge connected.
pub(crate) fn seq(key: &str) -> u64 { last_frame(key).seq }

pub(crate) fn last_frame(key: &str) -> LastFrame {
    with(|r| r.surfaces.iter().find(|s| &*s.key == key).map(|s| LastFrame { seq: s.seq, commit_ns: s.commit_ns, submit_ns: s.submit_ns }))
        .flatten().unwrap_or_default()
}

/// Inputs the bridge has delivered.
pub(crate) fn inputs() -> u64 { with(|r| r.inputs).unwrap_or(0) }

/// Frames every surface has presented.
pub(crate) fn presents() -> u64 { with(|r| r.presents).unwrap_or(0) }

pub(crate) fn mark(name: &str) -> u64 {
    let now = now_ns();
    with(|r| if r.running { r.marks.push((name.to_owned(), now)); });
    now
}

/// Clears the ring and counters and starts recording.
pub(crate) fn run_begin() -> u64 {
    with(|r| {
        r.len = 0;
        r.dropped = 0;
        r.wall = 0;
        r.waited = 0;
        r.parts = [0; PARTS];
        r.instances_max = 0;
        r.marks.clear();
        r.bridge_cpu = 0;
        #[cfg(target_os = "macos")]
        r.present_times.discard();
        r.running = true;
        r.t0 = now_ns();
        r.cpu0 = thread_cpu();
        // This request's own time from here on is the bridge's.
        if r.bridge_since.is_some() { r.bridge_since = Some((r.t0, r.cpu0)); }
        RECORDING.set(true);
        r.t0
    }).unwrap_or(0)
}

/// What a run measured, without the per-surface parts the caller adds.
pub(crate) struct Run {
    pub json: Value,
    pub instances_max: usize,
    pub frames: u64,
    pub dropped: u64,
}

/// Stops recording and reports the run: `work`, `frame_columns`, `frames` and `marks`.
pub(crate) fn run_end() -> Option<Run> {
    with(|r| {
        let (t1, cpu1) = (now_ns(), thread_cpu());
        r.running = false;
        RECORDING.set(false);
        r.unwind(t1);
        let bridge = r.bridge_cpu + r.bridge_since.map_or(0, |(_, c)| cpu1.saturating_sub(c));
        #[cfg(target_os = "macos")]
        { let len = r.len; r.present_times.fill(&mut r.rows[..len]); }
        let cpu_ms = cpu_ns(cpu1.saturating_sub(r.cpu0).saturating_sub(bridge)) / 1e6;
        let ms = |ns: u64| ns as f64 / 1e6;
        let wall = ms(r.wall);
        let parts: f64 = r.parts.iter().map(|&p| ms(p)).sum();
        let work = json!({
            "wall_ms": wall, "cpu_ms": cpu_ms,
            "view_ms": ms(r.parts[Part::View as usize]), "layout_ms": ms(r.parts[Part::Layout as usize]),
            "text_ms": ms(r.parts[Part::Text as usize]), "paint_ms": ms(r.parts[Part::Paint as usize]),
            "encode_ms": ms(r.parts[Part::Encode as usize]), "other_ms": (wall - parts).max(0.0),
            "acquire_ms": ms(r.waited),
        });
        let frames: Vec<Value> = r.rows[..r.len].iter().map(|row| json!(row)).collect();
        let marks: Vec<Value> = r.marks.iter().map(|(name, at)| json!([name, at])).collect();
        Run {
            instances_max: r.instances_max, frames: r.len as u64 + r.dropped, dropped: r.dropped,
            json: json!({ "t0_ns": r.t0, "t1_ns": t1, "work": work, "frame_columns": COLUMNS, "frames": frames, "marks": marks }),
        }
    })
}

/// The surface keys rows name by index, in index order.
pub(crate) fn surface_keys() -> Vec<String> {
    with(|r| r.surfaces.iter().map(|s| s.key.to_string()).collect()).unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn rows() -> Vec<[u64; COLS]> { with(|r| r.rows[..r.len].to_vec()).unwrap_or_default() }
