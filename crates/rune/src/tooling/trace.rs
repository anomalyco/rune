//! Motion-lint JSONL trace writer and headless element-tree JSON snapshots.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use crate::time::Instant;

use super::*;

pub(crate) struct Trace {
    out: Out,
    next_id: u32,
    ids: HashMap<String, u32>,
    described: HashMap<u32, (String, &'static str, &'static str)>,
    states: HashMap<u32, ([i64; 6], u32, [i64; 4])>,
    /// Each element's `ElementInfo::scrolled` last written, in hundredths of a point (none: 0).
    scrolled: HashMap<u32, [i64; 2]>,
    effects: HashMap<u32, [String; 2]>,
    goals: HashMap<u32, [i64; 7]>,
    channels: rune_motion::TraceChannels,
    path: PathBuf,
    /// Trace every element, not only keyed and named ones (`--trace-all`).
    pub(crate) all: bool,
    /// Lines waiting for present times; `None` writes each frame at once.
    presents: Option<crate::frame::present_time::Presents>,
    /// The native surface's frame counts and `App::dev_stats`, printed when the trace closes.
    surface: Option<(std::sync::Arc<std::sync::Mutex<crate::SurfaceStats>>, Option<fn() -> String>)>,
    /// Last refresh rate written to the header or a frame line, rounded to 0.01 Hz.
    hz: f64,
}

/// A native frame's timing beyond its submission time.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameTiming {
    /// The display link's target, ms on the trace's clock.
    pub target_ms: Option<f64>,
    /// The drawable the frame presented, when present times are recorded.
    pub drawable: Option<u64>,
    /// The previous frame asked for this one right away (it wasn't idle).
    pub continuous: bool,
    /// The display's current refresh rate, when known.
    pub hz: Option<f64>,
}

/// Where the trace's lines go. Off the web a thread owns the file; the web has no threads and
/// no disk to wait on.
#[cfg(not(target_arch = "wasm32"))]
type Out = TraceOut;
#[cfg(target_arch = "wasm32")]
type Out = std::io::BufWriter<std::fs::File>;

/// The trace file's writer: each finished line goes over a channel to a thread that owns the
/// file and writes it as it arrives, so the thread that draws never waits on the disk. Written
/// from the frame's own thread, a line waited 60 to 220 ms on the growing file's lock in traced
/// runs (about 5 KiB a frame through an 8 KiB buffer: a write call every frame or two), and the
/// trace then showed a frame gap that the app did not have. A process that dies loses only the
/// lines the thread had not written yet; `flush` waits until the file holds every line.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct TraceOut {
    /// The line being built: `writeln!` arrives in pieces.
    line: Vec<u8>,
    lines: Option<std::sync::mpsc::Sender<TraceMsg>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
enum TraceMsg {
    Lines(Vec<u8>),
    /// Answered when every line sent before it is written.
    Flush(std::sync::mpsc::SyncSender<()>),
}

#[cfg(not(target_arch = "wasm32"))]
impl TraceOut {
    pub(super) fn new(mut sink: impl Write + Send + 'static) -> TraceOut {
        let (lines, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("rune-trace".into())
            .spawn(move || {
                for msg in rx {
                    match msg {
                        TraceMsg::Lines(bytes) => {
                            sink.write_all(&bytes).ok();
                        }
                        TraceMsg::Flush(written) => {
                            sink.flush().ok();
                            written.send(()).ok();
                        }
                    }
                }
                sink.flush().ok();
            })
            .unwrap_or_else(|e| die(&format!("trace writer thread: {e}")));
        TraceOut { line: Vec::new(), lines: Some(lines), thread: Some(thread) }
    }

    fn send_line(&mut self) {
        if self.line.is_empty() { return; }
        let bytes = std::mem::take(&mut self.line);
        if let Some(lines) = &self.lines { lines.send(TraceMsg::Lines(bytes)).ok(); }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Write for TraceOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.line.extend_from_slice(buf);
        if self.line.last() == Some(&b'\n') { self.send_line(); }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send_line();
        let (written, done) = std::sync::mpsc::sync_channel(1);
        if let Some(lines) = &self.lines && lines.send(TraceMsg::Flush(written)).is_ok() { done.recv().ok(); }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for TraceOut {
    fn drop(&mut self) {
        self.send_line();
        drop(self.lines.take());
        if let Some(thread) = self.thread.take() { thread.join().ok(); }
    }
}

pub(super) fn r2(v: f32) -> f64 {
    r2_f64(f64::from(v))
}

fn r3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn r2_f64(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

impl Trace {
    pub(crate) fn open(path: &PathBuf, size: (f32, f32), fps: f64, scale: f32) -> Trace {
        Self::open_clock(path, size, fps, scale, "virtual")
    }

    pub(crate) fn open_clock(path: &PathBuf, size: (f32, f32), fps: f64, scale: f32, clock: &str) -> Trace {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).ok();
        }
        let file = std::fs::File::create(path).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
        let mut out = Out::new(file);
        let mut header = json!({
            "type": "header", "launch": "rune", "width": size.0, "height": size.1,
            "hz": fps, "scale": scale, "platform": "rune", "units": "pt", "clock": clock,
        });
        if clock == "wall" { header["timestamp"] = "cpu-submit".into(); }
        writeln!(out, "{header}").ok();
        Trace { out, next_id: 1, ids: HashMap::new(), described: HashMap::new(), states: HashMap::new(), scrolled: HashMap::new(), effects: HashMap::new(),
            goals: HashMap::new(), channels: Default::default(), path: path.clone(), all: false, presents: None, surface: None, hz: r2_f64(fps) }
    }

    /// Holds each frame's line until its present time arrives from `source`, then writes
    /// it with `present_ms` and `present_dt_ms`. `start` is the trace clock's zero.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn record_presents(&mut self, source: std::sync::Arc<dyn crate::frame::present_time::Source>, start: Instant) {
        self.presents = Some(crate::frame::present_time::Presents::new(source, start));
    }

    /// Prints the surface's frame counts, and the app's own, when the trace closes.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn record_surface(&mut self, stats: std::sync::Arc<std::sync::Mutex<crate::SurfaceStats>>, app: Option<fn() -> String>) {
        self.surface = Some((stats, app));
    }

    /// Waits up to `wait` for the last frames' present times, writes every held line, then
    /// waits until the file holds every line written so far: a run that ends by a fast exit
    /// drops nothing, so this is where its last frames reach the file.
    pub(crate) fn settle_presents(&mut self, wait: std::time::Duration) {
        #[cfg(target_arch = "wasm32")]
        let _ = wait;
        if let Some(presents) = self.presents.as_mut() {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let until = crate::time::now() + wait;
                while presents.waiting() && crate::time::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            presents.flush(&mut self.out);
        }
        self.out.flush().ok();
    }

    fn id(&mut self, key: &str) -> u32 {
        if let Some(&id) = self.ids.get(key) {
            return id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("trace element id overflow");
        self.ids.insert(key.to_owned(), id);
        id
    }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn frame<S>(&mut self, rt: &mut Runtime<S>, t_ms: f64, cpu_ms: f64) {
        self.frame_timed_core(rt.core_mut(), t_ms, cpu_ms, FrameTiming::default());
    }

    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn frame_timed<S>(&mut self, rt: &mut Runtime<S>, t_ms: f64, cpu_ms: f64, timing: FrameTiming) {
        self.frame_timed_core(rt.core_mut(), t_ms, cpu_ms, timing);
    }

    pub(crate) fn frame_timed_core(&mut self, rt: &mut crate::runtime::RuntimeCore, t_ms: f64, cpu_ms: f64, timing: FrameTiming) {
        let els = rt.elements();
        let settled = rt.settled;
        let mut metas = Vec::new();
        let mut changed = Vec::new();
        let mut scrolled = Vec::new();
        let mut seen = HashSet::new();
        // A virtual list's invisible estimated extent changes when rows are measured or the
        // window rebases. Record the actual row surfaces, not that bookkeeping rectangle.
        let all = self.all;
        let traced = |e: &crate::runtime::ElementInfo| (all || e.key.is_some() || e.trace_name.is_some()) && !e.virtual_extent;
        let blank = rt.tree.blank_leaves();
        for e in els.iter().filter(|e| traced(e)) {
            let id = self.id(&e.path);
            let name = e.trace_name.unwrap_or(&e.path);
            let (class, host) = e.trace_namespace.unwrap_or(("rune", "rune"));
            seen.insert(id);
            if self.described.get(&id).is_none_or(|old| old.0 != name || old.1 != class || old.2 != host) {
                self.described.insert(id, (name.to_owned(), class, host));
                let mut meta = json!({ "id": id, "cls": class, "nm": name, "host": host, "role": e.role });
                if let Some(p) = e.parent.and_then(|p| els.get(p)).filter(|p| traced(p)) {
                    meta["pv"] = self.id(&p.path).into();
                }
                metas.push(meta);
            }
            let effects = [format!("{name}.scale"), format!("{name}.blur")];
            for (name, value, unit) in [(&effects[0], e.scale, "ratio"), (&effects[1], e.blur, "pt")] {
                self.channels.sample(name, "position", value);
                self.channels.owner(name, id, unit);
            }
            if let Some(old) = self.effects.insert(id, effects.clone()) {
                for name in old.iter().filter(|n| !effects.contains(n)) { self.channels.remove(name); }
            }
            let [x, y, w, h] = e.rect;
            // A settle commit is a deliberate cut (everything lands at once); a growth row reveals
            // its content by growing; reveals fade words in.
            use rune_motion::{Intent as I, flags as F};
            let clipped = e.rect.iter().zip(e.visible.iter()).any(|(a, b)| (a - b).abs() > 0.5);
            let mut intent = e.live_intent;
            if settled {
                intent |= I::POSITION | I::SIZE | I::TRANSITION | I::REVEAL;
            }
            // Reduced motion deliberately cuts geometry. Opacity still needs to fade;
            // do not declare transition/opacity intent for the whole reduced-motion tree.
            if rt.reduced_motion {
                intent |= I::POSITION | I::SIZE;
            }
            if e.revealing {
                intent |= I::REVEAL;
            }
            if e.in_growing_row {
                intent |= I::SIZE | I::REVEAL;
            }
            let flags = if e.moving || e.in_growing_row { F::ANIMATED } else { 0 }
                | if e.scrolling { F::SCROLLING } else { 0 }
                | if clipped { F::CLIPPED } else { 0 }
                | if blank.contains(e.path.as_str()) { F::BLANK } else { 0 }
                | if e.loop_phase.is_some() { F::LOOPING } else { 0 }
                | intent << 8;
            let q = [
                (f64::from(x) * 100.0).round() as i64,
                (f64::from(y) * 100.0).round() as i64,
                (f64::from(w) * 100.0).round() as i64,
                (f64::from(h) * 100.0).round() as i64,
                (f64::from(e.opacity) * 1000.0).round() as i64,
                (f64::from(e.radius) * 100.0).round() as i64,
            ];
            let sc = e.scrolled.map(|v| (f64::from(v) * 100.0).round() as i64);
            if self.scrolled.get(&id).copied().unwrap_or_default() != sc {
                scrolled.push(json!([id, r2(e.scrolled[0]), r2(e.scrolled[1])]));
                if sc == [0; 2] { self.scrolled.remove(&id); } else { self.scrolled.insert(id, sc); }
            }
            let state = (q, flags, e.visible.map(|v| (v * 100.0).round() as i64));
            if self.states.get(&id) != Some(&state) {
                self.states.insert(id, state);
                let mut row = vec![
                    json!(id),
                    json!(r2(x)),
                    json!(r2(y)),
                    json!(r2(w)),
                    json!(r2(h)),
                    json!((f64::from(e.opacity) * 1000.0).round() / 1000.0),
                    json!(r2(e.radius)),
                    json!(flags),
                ];
                if clipped {
                    row.extend(e.visible.map(|v| json!(r2(v))));
                }
                changed.push(Value::Array(row));
            }
        }
        let gone: Vec<u32> = self.states.keys().copied().filter(|id| !seen.contains(id)).collect();
        // A settle unmounts what it replaces at once: a cut (transition | opacity) at its last place.
        let mut cuts = Vec::new();
        for id in &gone {
            self.described.remove(id);
            self.scrolled.remove(id);
            if let Some(names) = self.effects.remove(id) {
                for name in names { self.channels.remove(&name); }
            }
            if let Some((q, ..)) = self.states.remove(id)
                && settled
            {
                let [x, y, w, h] = [q[0], q[1], q[2], q[3]].map(|v| v as f64 / 100.0);
                cuts.push(json!([id, x, y, w, h, rune_motion::Intent::TRANSITION | rune_motion::Intent::OPACITY, 0]));
            }
        }
        let mut handovers = Vec::new();
        for h in rt.matched_handovers() {
            let from_id = self.ids.get(&h.from).copied();
            let to_id = self.ids.get(&h.to).copied();
            if from_id.is_none() && to_id.is_none() {
                continue;
            }
            handovers.push(json!({
                "from": from_id,
                "to": to_id,
                "from_rect": h.from_rect.map(r2),
                "to_rect": h.to_rect.map(r2),
            }));
        }
        if !gone.is_empty() {
            self.ids.retain(|_, id| seen.contains(id));
        }

        let mut line = json!({ "t": (t_ms * 1000.0).round() / 1000.0, "n": 1 });
        // A fresh destination ends a trajectory, even when it came from streamed data rather
        // than a key. Keep this separate from input: it never excuses a jump or lost velocity.
        let mut goals = Vec::new();
        for (path, goal) in rt.tree.trace_goals() {
            let Some(&id) = self.ids.get(path) else { continue };
            let q = goal.map(|v| (f64::from(v) * 10000.0).round() as i64);
            if let Some(old) = self.goals.insert(id, q) {
                let scale = u8::from(old[6] != q[6]) * 15;
                let position = u8::from(old[0] != q[0] || old[4] != q[4])
                    | u8::from(old[1] != q[1] || old[5] != q[5]) * 2;
                let own = position | scale | u8::from(old[2] != q[2]) * 4 | u8::from(old[3] != q[3]) * 8;
                if own != 0 { goals.push(json!([id, own, position | scale])); }
            }
        }
        self.goals.retain(|id, _| seen.contains(id));
        if !goals.is_empty() { line["retarget"] = goals.into(); }
        // motion-lint's touch count: motion under a held press follows the hand.
        if rt.pressing() {
            line["k"] = 1.into();
        }
        let (edges, position_error) = rt.scene.sampling_error();
        let retarget = rt.scene.store_mut().take_retarget_audit();
        line["quality"] = json!({
            "cpu_ms": cpu_ms, "scene_edges": edges, "position_error_px": position_error,
            "retarget_lanes": retarget.lanes, "velocity_error_pt_s": retarget.max_velocity_error,
            "animating": rt.animating,
            "why": crate::runtime::AnimSource::names(if rt.animating { rt.motion.anim_sources() } else { 0 }),
        });
        // What the motion store spent this frame on, by traced element id (or path when untraced).
        let touched = rt.touched_tracks();
        if !touched.is_empty() {
            line["tk"] = touched.into_iter().map(|(path, prop, active)| {
                let who = self.ids.get(&path).map_or_else(|| json!(path), |&id| json!(id));
                json!([who, prop, u8::from(active)])
            }).collect::<Vec<_>>().into();
        }
        let events = std::mem::take(&mut rt.events);
        if !events.is_empty() {
            line["ev"] = events.into();
        }
        if !metas.is_empty() {
            line["m"] = metas.into();
        }
        if !changed.is_empty() {
            line["e"] = changed.into();
        }
        if !gone.is_empty() {
            line["g"] = gone.into();
        }
        if !handovers.is_empty() {
            line["handover"] = handovers.into();
        }
        // How far scrolling carries each element, when that changed: [id, x, y] in points.
        if !scrolled.is_empty() {
            line["sc"] = scrolled.into();
        }
        if !cuts.is_empty() {
            line["ai"] = cuts.into();
        }
        if let Some(channels) = rt.frame.trace_channels() {
            for (&name, &(kind, value)) in channels { self.channels.sample(name, kind, value); }
        }
        let mut line = line.to_string();
        line.pop(); // Append scalar fields to the same frame object.
        self.channels.write_json(&mut line);
        if let Some(target) = timing.target_ms {
            line.push_str(&format!(r#","target_ms":{target}"#));
        }
        if let Some(hz) = timing.hz.filter(|hz| hz.is_finite() && *hz > 0.0).map(r2_f64).filter(|&hz| hz > 0.0 && hz != self.hz) {
            self.hz = hz;
            line.push_str(&format!(r#","hz":{hz}"#));
        }
        if let Some(presents) = self.presents.as_mut() {
            presents.push(line, timing.drawable, timing.continuous, &mut self.out);
            return;
        }
        line.push('}');
        writeln!(self.out, "{line}").ok();
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        if let Some((stats, app)) = &self.surface {
            eprintln!("rune: frames {}", crate::frame::surface_stats::read(stats).summary());
            if let Some(app) = app { eprintln!("rune: app {}", app()); }
        }
        if self.presents.is_none() { return; }
        self.settle_presents(crate::frame::present_time::SETTLE);
        self.out.flush().ok();
        if let Some(summary) = std::fs::read_to_string(&self.path).ok().as_deref().and_then(crate::frame::present_time::Summary::of_trace) {
            eprintln!("{summary}");
        }
    }
}

#[cfg(test)]
#[inline(always)]
pub(crate) fn tree_json<S>(rt: &Runtime<S>, real_t: f64) -> Value {
    tree_json_core(rt.core(), real_t)
}

fn shadow_json(s: &crate::el::Shadow) -> Value {
    json!({
        "x": r2(s.x),
        "y": r2(s.y),
        "blur": r2(s.blur),
        "spread": r2(s.spread),
        "color": s.color.hex(),
        "inset": s.inset,
    })
}

pub(crate) fn tree_json_core(rt: &crate::runtime::RuntimeCore, real_t: f64) -> Value {
    let videos = rt.tree.videos();
    let shadows = rt.presented_shadows_by_path();
    // The pointer's shape (`Runtime::cursor`) and the element it is read from.
    let cursor = rt.cursor().css_name();
    let cursor_path = rt.cursor_path();
    let els: Vec<Value> = rt
        .elements()
        .iter()
        .map(|e| {
            let mut v = json!({
                "path": e.path,
                "role": e.role,
                "rect": e.rect.map(r2),
                "opacity": (e.opacity * 1000.0).round() / 1000.0,
                "layerOpacity": r3(e.layer_opacity),
                "scale": (e.scale * 10000.0).round() / 10000.0,
                "blur": r2(e.blur),
                "backdropBlur": r2(e.backdrop_blur),
                "radius": r2(e.radius),
                "focusable": e.focusable, "focused": e.focused,
                "focusVisible": e.focus_visible, "inert": e.inert,
                "loopPhase": e.loop_phase,
                "fill": e.fill.hex(),
            });
            if let Some(k) = &e.key {
                v["key"] = k.clone().into();
            }
            if let Some(l) = &e.label {
                v["label"] = l.clone().into();
            }
            if let Some(t) = &e.text {
                v["text"] = t.clone().into();
            }
            if let Some(c) = e.border_color {
                v["borderColor"] = c.hex().into();
            }
            if let Some(c) = e.text_color {
                v["color"] = c.hex().into();
            }
            if let Some(ts) = &e.text_style {
                let mut style = json!({
                    "size": r2(ts.size),
                    "weight": r2(ts.weight),
                    "lineHeight": r2(ts.line_height),
                    "tracking": r2(ts.tracking),
                    "italic": ts.italic,
                });
                if let Some(family) = &ts.family {
                    style["family"] = family.clone().into();
                }
                v["textStyle"] = style;
            }
            if let Some(h) = &e.help {
                v["help"] = h.clone().into();
            }
            if let Some(d) = &e.description {
                v["description"] = d.clone().into();
            }
            if let Some(s) = &e.selected {
                v["selected"] = s.clone().into();
            }
            if let Some(l) = e.level {
                v["level"] = l.into();
            }
            if let Some(x) = e.expanded {
                v["expanded"] = x.into();
            }
            if e.item_selected {
                v["itemSelected"] = true.into();
            }
            let secure = rt.tree.element_is_secure(&e.path);
            if secure {
                v["secure"] = true.into();
            }
            if e.rect.iter().zip(e.target.iter()).any(|(a, b)| (a - b).abs() > 0.01) {
                v["target"] = e.target.map(r2).to_vec().into();
            }
            if e.moving {
                v["moving"] = true.into();
            }
            if e.exiting {
                v["exiting"] = true.into();
            }
            if e.hover > 0.001 {
                v["hover"] = r2(e.hover).into();
            }
            if e.press > 0.001 {
                v["press"] = r2(e.press).into();
            }
            if e.drag_out {
                v["dragOut"] = true.into();
            }
            if e.stuck.abs() > 0.001 {
                v["stuck"] = r2(e.stuck).into();
            }
            if e.material_fallback {
                v["materialFallback"] = true.into();
            }
            if let Some(sh) = shadows.get(&e.path) {
                v["shadows"] = sh.iter().map(shadow_json).collect::<Vec<_>>().into();
            }
            if let Some(c) = e.caret && !secure {
                v["caret"] = c.map(r2).to_vec().into();
            }
            if let Some(r) = e.edit_range {
                v["selection"] = r.to_vec().into();
            }
            if let Some(video) = videos.get(&e.path) {
                v["video"] = video_json(video);
            }
            // On the one element the pointer's shape is read from: its own cursor, else the
            // nearest ancestor's, else `"default`".
            if cursor_path == Some(e.path.as_str()) {
                v["cursor"] = cursor.into();
            }
            v
        })
        .collect();
    let mut tree = json!({
        "t": real_t,
        "anim_t": rt.t,
        "viewport": [rt.viewport.0, rt.viewport.1],
        "scale": rt.env.scale,
        "dark_system": rt.env.system_dark,
        "reduced_motion": rt.reduced_motion,
        "time_scale": rt.time_scale,
        "animating": rt.animating,
        "elements": els,
        "warnings": rt.commit.key_warnings().unwrap_or_default(),
    });
    if let Some(menu) = rt.popup_json() { tree["popupMenu"] = menu; }
    if let Some(dialog) = rt.file_dialog_json() { tree["fileDialog"] = dialog; }
    // Where the input method's candidate window opens: the window's IME area, in view points.
    if let Some(area) = rt.ime_area { tree["imeArea"] = area.map(r2).to_vec().into(); }
    // The focused field's open composition, not yet part of its text.
    if let Some((text, cursor)) = rt.focused_preedit() {
        tree["preedit"] = json!({ "text": text, "cursor": cursor.map(|(a, b)| [a, b]) });
    }
    if let Some(title) = rt.host.window_title.current() { tree["title"] = title.into(); }
    if let Some((text, rect)) = rt.help_tooltip() { tree["tooltip"] = json!({ "text": text, "rect": rect.map(r2) }); }
    // Where the pointer is and the shape the shell shows for it: `"default`" over an element
    // that sets none (and inherits none) and outside the window.
    if let Some((x, y)) = rt.pointer() { tree["pointer"] = json!({ "at": [r2(x), r2(y)], "cursor": cursor }); }
    tree["accessibility"] = crate::a11y::dump::json(&rt.accessibility_tree(), false);
    tree
}

fn video_json(v: &crate::video::Snapshot) -> Value {
    use crate::PlayerState;
    let (state, error) = match &v.state {
        PlayerState::Loading => ("loading", None),
        PlayerState::Ready => ("ready", None),
        PlayerState::Playing => ("playing", None),
        PlayerState::Ended => ("ended", None),
        PlayerState::Failed(e) => ("failed", Some(e.clone())),
        _ => ("unknown", None),
    };
    let mut out = json!({
        "kind": match v.kind { crate::video::Kind::Video => "video", crate::video::Kind::Audio => "audio" },
        "state": state,
        "time": (v.time * 1000.0).round() / 1000.0,
        "duration": v.duration.map(|d| (d * 1000.0).round() / 1000.0),
        "size": v.size.map(|(w, h)| [w, h]),
        "rate": v.rate,
        "looping": v.looping,
        "muted": v.muted,
    });
    if let Some(e) = error { out["error"] = e.into(); }
    out
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
