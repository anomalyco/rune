use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;

use serde_json::{Value, json};
use winit::keyboard::{Key as WKey, KeyCode, ModifiersState, NamedKey};

use super::chord::{chord, typed};
use super::driver::{Backend, Driver, IDLE_NS, Input, Poll, SurfaceTally};
use super::protocol::{Code, Fail, Request, parse};
use super::record;
use super::select::{Candidate, Selector};

#[test]
fn requests_parse_and_bad_lines_fail_with_their_id() {
    let r = parse(r#"{"id":7,"op":"click","args":{"selector":"save"}}"#).unwrap();
    assert_eq!((r.id, r.op.as_str(), r.args), (json!(7), "click", json!({"selector": "save"})));
    assert_eq!(parse(r#"{"id":1,"op":"info"}"#).unwrap().args, json!({}));
    let (id, fail) = parse(r#"{"id":3}"#).unwrap_err();
    assert_eq!((id, fail.code), (json!(3), Code::BadRequest));
    assert_eq!(parse("nope").unwrap_err().1.code, Code::BadRequest);
    assert_eq!(parse(r#"{"op":"info"}"#).unwrap_err().1.code, Code::BadRequest);
    assert_eq!(parse(r#"{"id":2,"op":"info","args":[1]}"#).unwrap_err().1.code, Code::BadRequest);
}

fn candidate<'a>(key: Option<&'a str>, path: &'a str, role: &'a str, text: Option<&'a str>) -> Candidate<'a> {
    Candidate {
        key, path, role, label: None, text, rect: [10.0, 10.0, 50.0, 20.0], visible: [10.0, 10.0, 50.0, 20.0],
        opacity: 1.0, exiting: false, focused: false, enabled: true,
    }
}

#[test]
fn selectors_match_keys_paths_roles_text_and_focus() {
    let vp = (400.0, 300.0);
    let sel = |v: Value| Selector::parse(&v).unwrap();
    let save = candidate(Some("save"), "root/toolbar/save", "button", Some("Save"));
    assert!(sel(json!("save")).matches(&save, vp));
    assert!(!sel(json!("sav")).matches(&save, vp));
    assert!(sel(json!("toolbar/save")).matches(&save, vp));
    assert!(!sel(json!("bar/save")).matches(&save, vp), "a path suffix starts at a segment");
    assert!(sel(json!("role=button")).matches(&save, vp));
    assert!(sel(json!("[role=button]")).matches(&save, vp));
    assert!(!sel(json!("role=textbox")).matches(&save, vp));
    assert!(sel(json!("text=Sav")).matches(&save, vp));
    assert!(!sel(json!({"selector": "text=Sav", "exact": true})).matches(&save, vp));
    assert!(sel(json!({"selector": "role=button", "text": "Save", "exact": true})).matches(&save, vp));
    assert!(sel(json!({"text": "Save"})).matches(&save, vp));
    assert!(!sel(json!("save:focus")).matches(&save, vp));
    assert!(sel(json!("save:focus")).matches(&Candidate { focused: true, ..candidate(Some("save"), "save", "button", None) }, vp));
    assert!(Selector::parse(&json!(3)).is_err());
    assert!(Selector::parse(&json!({})).is_err());
}

#[test]
fn only_visible_elements_match() {
    let vp = (400.0, 300.0);
    let sel = Selector::parse(&json!("a")).unwrap();
    let a = || candidate(Some("a"), "a", "", None);
    assert!(sel.matches(&a(), vp));
    assert!(!sel.matches(&Candidate { opacity: 0.005, ..a() }, vp));
    assert!(!sel.matches(&Candidate { exiting: true, ..a() }, vp));
    assert!(!sel.matches(&Candidate { visible: [10.0, 10.0, 0.0, 20.0], ..a() }, vp));
    assert!(!sel.matches(&Candidate { visible: [500.0, 10.0, 20.0, 20.0], ..a() }, vp), "outside the window");
    assert!(!sel.matches(&Candidate { visible: [-30.0, 10.0, 20.0, 20.0], ..a() }, vp));
}

#[test]
fn chords_resolve_keys_and_platform_modifiers() {
    let c = chord("Control+Shift+D").unwrap();
    assert_eq!((c.code, c.mods), (Some(KeyCode::KeyD), ModifiersState::CONTROL | ModifiersState::SHIFT));
    assert_eq!((c.key, c.base), (WKey::Character("D".into()), Some('d')));
    let e = chord("Enter").unwrap();
    assert_eq!((e.code, e.key, e.mods), (Some(KeyCode::Enter), WKey::Named(NamedKey::Enter), ModifiersState::empty()));
    let primary = if cfg!(target_os = "macos") { ModifiersState::SUPER } else { ModifiersState::CONTROL };
    assert_eq!(chord("ControlOrMeta+a").unwrap().mods, primary);
    assert_eq!(chord("Meta+k").unwrap().mods, ModifiersState::SUPER);
    assert_eq!(chord("Alt+ArrowLeft").unwrap().key, WKey::Named(NamedKey::ArrowLeft));
    assert_eq!(chord("Control++").unwrap().code, Some(KeyCode::Equal));
    assert!(chord("Control+Nope").is_err());
    assert!(chord("Shift").is_err());
}

#[test]
fn typed_characters_take_us_keys_or_arrive_as_text() {
    let a = typed('a');
    assert_eq!((a.code, a.mods, a.base), (Some(KeyCode::KeyA), ModifiersState::empty(), Some('a')));
    let upper = typed('A');
    assert_eq!((upper.code, upper.mods, upper.key), (Some(KeyCode::KeyA), ModifiersState::SHIFT, WKey::Character("A".into())));
    let bang = typed('!');
    assert_eq!((bang.code, bang.mods, bang.base), (Some(KeyCode::Digit1), ModifiersState::SHIFT, Some('1')));
    assert_eq!(typed(' ').key, WKey::Named(NamedKey::Space));
    assert_eq!(typed('\n').code, Some(KeyCode::Enter));
    let e = typed('é');
    assert_eq!((e.code, e.key), (None, WKey::Character("é".into())));
}

#[test]
fn the_ring_keeps_its_first_rows_and_counts_the_rest() {
    record::install();
    record::run_begin();
    record::turn_begin();
    record::input("main", 5, record::InputKind::Scroll);
    record::presented("main", 10);
    record::presented("hud", 3);
    let rows = record::rows();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0][0], rows[0][1], rows[0][7], rows[0][8]), (1, 0, 5, 2), "seq, surface and the input it reflects");
    assert_eq!((rows[1][1], rows[1][7], rows[1][8]), (1, 0, 0));
    assert!(rows[0][3] <= rows[0][4] && rows[0][4] <= rows[0][5], "begin, cpu end and submit in order");
    for _ in 0..record::RING { record::presented("main", 1); }
    let run = record::run_end().unwrap();
    assert_eq!((run.frames, run.dropped, run.instances_max), (record::RING as u64 + 2, 2, 10));
    assert_eq!(run.json["frames"].as_array().unwrap().len(), record::RING);
    assert_eq!(record::seq("main"), record::RING as u64 + 1);
    // A new run starts empty.
    record::run_begin();
    assert!(record::rows().is_empty());
}

#[test]
fn phases_are_exclusive() {
    use crate::frame::profile::Phase;
    record::install();
    record::run_begin();
    record::turn_begin();
    // Each spin's own wall time, which preemption stretches for the recorder too.
    let spin = |ns: u64| {
        let t = super::clock::now_ns();
        while super::clock::now_ns() - t < ns { std::hint::spin_loop(); }
        (super::clock::now_ns() - t) as f64 / 1e6
    };
    record::phase_enter(Phase::Commit);
    let commit = spin(2_000_000);
    record::phase_enter(Phase::View);
    let view = spin(2_000_000);
    record::phase_exit();
    record::phase_exit();
    record::built();
    let encode = spin(1_000_000);
    record::presented("main", 0);
    record::turn_end();
    let run = record::run_end().unwrap();
    let work = &run.json["work"];
    let ms = |k: &str| work[k].as_f64().unwrap();
    assert!((ms("view_ms") - view).abs() < 0.2, "{work}");
    assert!((ms("layout_ms") - commit).abs() < 0.2, "commit's own time only: {work}");
    assert!((ms("encode_ms") - encode).abs() < 0.2, "{work}");
    let parts = ms("view_ms") + ms("layout_ms") + ms("text_ms") + ms("paint_ms") + ms("encode_ms");
    assert!((parts + ms("other_ms") - ms("wall_ms")).abs() < 0.01, "{work}");
    assert!(ms("cpu_ms") > 0.0 && ms("cpu_ms") <= ms("wall_ms") + 1.0, "{work}");
}

/// A backend whose clock, quiet and elements the test sets.
struct Fake {
    clock: u64,
    quiet: bool,
    activity: u64,
    found: Option<[f32; 4]>,
    delivered: Vec<Input>,
}

impl Backend for Fake {
    fn clock_ns(&self) -> u64 { self.clock }
    fn surface(&mut self, key: Option<&str>) -> Result<String, Fail> {
        match key { None | Some("main") => Ok("main".into()), Some(_) => Err(Fail::new(Code::NoWindow, "no")) }
    }
    fn find(&mut self, _: &str, _: &Selector) -> Option<[f32; 4]> { self.found }
    fn query(&mut self, _: &str, _: &Selector, _: bool) -> Vec<Value> { Vec::new() }
    fn deliver(&mut self, _: &str, input: &Input) -> Result<u64, Fail> {
        self.delivered.push(input.clone());
        Ok(self.clock)
    }
    fn quiet(&self) -> bool { self.quiet }
    fn activity(&self) -> u64 { self.activity }
    fn info(&mut self) -> Value { json!({}) }
    fn window(&mut self, _: &str, _: &str) -> Result<(), Fail> { Ok(()) }
    fn snapshot(&mut self, _: &str, _: &str) -> Result<Value, Fail> { Ok(json!({})) }
    fn step(&mut self, _: &Value) -> Result<Value, Fail> { Ok(json!({})) }
    fn quit(&mut self) {}
    fn tallies(&self) -> Vec<SurfaceTally> { Vec::new() }
}

fn driver() -> (Driver, mpsc::Sender<Request>, mpsc::Receiver<String>) {
    let (tx, rx) = mpsc::channel();
    let (out, replies) = mpsc::channel();
    (Driver::new(rx, out), tx, replies)
}

fn request(tx: &mpsc::Sender<Request>, line: &str) { tx.send(parse(line).unwrap()).unwrap(); }

fn reply(replies: &mpsc::Receiver<String>) -> Value { serde_json::from_str(&replies.try_recv().expect("a reply")).unwrap() }

#[test]
fn idle_needs_300_ms_of_quiet_and_restarts_on_activity() {
    record::install();
    let (mut d, tx, replies) = driver();
    let mut b = Fake { clock: 1_000, quiet: false, activity: 0, found: None, delivered: Vec::new() };
    request(&tx, r#"{"id":1,"op":"wait","args":{"until":"idle"}}"#);
    assert!(matches!(d.poll(&mut b), Poll::Wait(_)));
    b.quiet = true;
    b.clock = 2_000;
    assert_eq!(d.poll(&mut b), Poll::Wait(2_000 + IDLE_NS), "one timer: the moment quiet has lasted 300 ms");
    b.clock = 2_000 + IDLE_NS / 2;
    b.activity = 1; // a frame landed meanwhile
    assert_eq!(d.poll(&mut b), Poll::Wait(2_000 + IDLE_NS / 2 + IDLE_NS));
    b.clock += IDLE_NS / 2;
    assert!(matches!(d.poll(&mut b), Poll::Wait(_)));
    assert!(replies.try_recv().is_err());
    b.clock = 2_000 + IDLE_NS / 2 + IDLE_NS;
    assert_eq!(d.poll(&mut b), Poll::Idle);
    assert!(reply(&replies)["ok"]["at_ns"].is_u64());
}

#[test]
fn quiet_starts_over_at_an_input() {
    record::install();
    let (mut d, tx, replies) = driver();
    let mut b = Fake { clock: 1_000, quiet: true, activity: 0, found: None, delivered: Vec::new() };
    assert_eq!(d.poll(&mut b), Poll::Idle);
    b.clock += 10 * IDLE_NS; // long quiet before the request
    request(&tx, r#"{"id":1,"op":"wheel","args":{"dy":10,"until":"idle"}}"#);
    let at = b.clock;
    assert_eq!(d.poll(&mut b), Poll::Wait(at), "looks again once the input is known");
    assert_eq!(d.poll(&mut b), Poll::Wait(at + IDLE_NS));
    assert!(replies.try_recv().is_err());
    b.clock = at + IDLE_NS;
    assert_eq!(d.poll(&mut b), Poll::Idle);
    assert_eq!(reply(&replies)["ok"]["input_ns"].as_array().map(Vec::len), Some(1));
}

#[test]
fn a_click_waits_for_its_selector_then_presses_and_releases_a_frame_apart() {
    record::install();
    let (mut d, tx, replies) = driver();
    let mut b = Fake { clock: 0, quiet: true, activity: 0, found: None, delivered: Vec::new() };
    request(&tx, r#"{"id":"c","op":"click","args":{"selector":"save","timeout_ms":1000}}"#);
    assert!(matches!(d.poll(&mut b), Poll::Wait(_)));
    b.found = Some([10.0, 20.0, 40.0, 10.0]);
    b.activity = 1;
    assert!(matches!(d.poll(&mut b), Poll::Wait(_)), "the release waits for the press's frame");
    assert_eq!(b.delivered, [Input::Move(30.0, 25.0), Input::Button(crate::PointerButton::Primary, true)]);
    record::presented("main", 0);
    assert_eq!(d.poll(&mut b), Poll::Idle);
    assert_eq!(b.delivered.last(), Some(&Input::Button(crate::PointerButton::Primary, false)));
    let r = reply(&replies);
    assert_eq!((r["id"].as_str(), r["ok"]["input_ns"].as_array().map(Vec::len)), (Some("c"), Some(2)));
    // A selector that never shows fails at its timeout.
    request(&tx, r#"{"id":2,"op":"click","args":{"selector":"gone","timeout_ms":5}}"#);
    b.found = None;
    d.poll(&mut b);
    b.clock += 10_000_000;
    d.poll(&mut b);
    assert_eq!(reply(&replies)["err"]["code"], "not-found");
}

#[test]
fn scroll_steps_once_a_frame_at_its_speed() {
    record::install();
    let (mut d, tx, replies) = driver();
    let mut b = Fake { clock: 0, quiet: true, activity: 0, found: Some([0.0, 0.0, 100.0, 100.0]), delivered: Vec::new() };
    request(&tx, r#"{"id":1,"op":"scroll","args":{"selector":"list","by":100,"speed":3000}}"#);
    let mut frames = 0;
    while d.poll(&mut b) != Poll::Idle {
        b.clock += 1_000_000_000 / 120;
        record::presented("main", 0);
        frames += 1;
        assert!(frames < 100);
    }
    let wheel: Vec<f32> = b.delivered.iter().filter_map(|i| if let Input::Wheel(_, dy) = i { Some(*dy) } else { None }).collect();
    assert!((wheel.iter().sum::<f32>() + 100.0).abs() < 0.1, "content moves up by 100 points: {wheel:?}");
    assert!(wheel.len() >= 3 && wheel[1..wheel.len() - 1].iter().all(|d| (d + 25.0).abs() < 0.1), "3000 pt/s at 120 Hz: {wheel:?}");
    assert_eq!(reply(&replies)["ok"]["input_ns"].as_array().unwrap().len(), wheel.len());
}

#[test]
fn unknown_ops_and_bad_args_fail_fast() {
    record::install();
    let (mut d, tx, replies) = driver();
    let mut b = Fake { clock: 0, quiet: true, activity: 0, found: None, delivered: Vec::new() };
    for line in [r#"{"id":1,"op":"dance"}"#, r#"{"id":2,"op":"click"}"#, r#"{"id":3,"op":"press","args":{"chord":"Nope+"}}"#,
        r#"{"id":4,"op":"wait"}"#, r#"{"id":5,"op":"click","args":{"x":1,"y":1,"window":"other"}}"#] {
        request(&tx, line);
    }
    d.poll(&mut b);
    let codes: VecDeque<Value> = (0..5).map(|_| reply(&replies)["err"]["code"].clone()).collect();
    assert_eq!(codes, [json!("bad-request"), json!("bad-request"), json!("bad-request"), json!("bad-request"), json!("no-window")]);
}

struct Counter {
    n: u32,
    field: crate::TextEdit,
}

fn counter_view(s: &Counter, _: &mut crate::Cx<Counter>) -> crate::El {
    let style = crate::TextStyle::new(14.0, 400.0).lh(20.0);
    crate::col().w_full().h_full().padding(8.0).gap(8.0)
        .child(crate::el().key("inc").size(80.0, 30.0).fill(crate::oklch(0.6, 0.15, 250.0)).on_press(|s: &mut Counter| s.n += 1))
        .child(crate::text(format!("n={}", s.n)).key("count"))
        .child(crate::text_input(&s.field, style).single_line().key("field").size(200.0, 24.0).on_edit(|s: &mut Counter| &mut s.field))
}

/// A fake strobe on loopback drives a headless app: click, type, waits, a run and quit.
#[test]
fn headless_bridge_serves_a_fake_strobe() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let snap = std::env::temp_dir().join(format!("rune-strobe-{}.png", std::process::id()));
    let snap_path = snap.to_string_lossy().replace('\\', "/");
    let strobe = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        let mut out = stream;
        let mut next = || -> Value { serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap() };
        let hello = next();
        let mut ask = |line: Value| -> Value {
            writeln!(out, "{line}").unwrap();
            next()
        };
        let first = ask(json!({"id": 100, "op": "wait", "args": {"until": "inc"}}));
        assert_eq!((first["ok"]["frame_seq"].as_u64(), first["ok"]["already"].as_bool()), (Some(1), Some(true)), "{first}");
        assert!(first["ok"]["at_ns"].as_u64() > hello["now_ns"].as_u64(), "the first frame commits after the hello");
        let info = ask(json!({"id": 1, "op": "info"}));
        let begin = ask(json!({"id": 2, "op": "run.begin"}));
        let click = ask(json!({"id": 3, "op": "click", "args": {"selector": "inc", "until": "text=n=1", "timeout_ms": 2000}}));
        let typed = ask(json!({"id": 4, "op": "type", "args": {"selector": "field", "text": "Hi!", "until": "frame"}}));
        let field = ask(json!({"id": 5, "op": "query", "args": {"selector": "field"}}));
        let idle = ask(json!({"id": 6, "op": "wait", "args": {"until": "idle", "timeout_ms": 5000}}));
        assert_eq!(idle["ok"]["already"], false, "{idle}");
        let mark = ask(json!({"id": 7, "op": "mark", "args": {"name": "typed"}}));
        let end = ask(json!({"id": 8, "op": "run.end"}));
        let missing = ask(json!({"id": 9, "op": "wait", "args": {"until": "missing", "timeout_ms": 100}}));
        let snap = ask(json!({"id": 10, "op": "snapshot", "args": {"path": snap_path}}));
        let step = ask(json!({"id": 11, "op": "step", "args": {"frames": 3}}));
        let quit = ask(json!({"id": 12, "op": "quit"}));
        let event = next();
        (hello, [info, begin, click, typed, field, idle, mark, end, missing, snap, step, quit], event)
    });
    let spec = crate::AppSpec::new("strobe", (320.0, 200.0), || Counter { n: 0, field: crate::TextEdit::default() }, counter_view);
    let args = crate::tooling::Args { fps: 60.0, every: 1, cols: 10, ..Default::default() };
    super::headless::serve(crate::App::new(spec), args, false, &super::Endpoint { addr, token: "t0k".into() }).unwrap();
    let (hello, r, event) = strobe.join().unwrap();
    assert_eq!((hello["hello"].as_u64(), hello["token"].as_str(), hello["mode"].as_str()), (Some(1), Some("t0k"), Some("headless")));
    for (i, reply) in r.iter().enumerate() {
        assert_eq!(reply["id"], json!(i + 1), "{reply}");
        if i != 8 { assert!(reply.get("ok").is_some(), "{reply}"); }
    }
    let [info, _, click, typed, field, _, mark, end, missing, snap_reply, step, _] = &r;
    assert!(info["ok"]["gpu"]["name"].is_string() && info["ok"]["surfaces"][0]["key"] == "main", "{info}");
    assert_eq!(click["ok"]["input_ns"].as_array().unwrap().len(), 2);
    assert_eq!(typed["ok"]["input_ns"].as_array().unwrap().len(), 3, "one per character");
    assert_eq!(field["ok"]["elements"][0]["text"], "Hi!", "{field}");
    assert!(mark["ok"]["at_ns"].is_u64());
    let run = &end["ok"];
    assert!(run["counts"]["frames"].as_u64().unwrap() > 0 && run["counts"]["commits"].as_u64().unwrap() > 0, "{run}");
    assert!(run["work"]["wall_ms"].as_f64().unwrap() > 0.0, "{run}");
    assert_eq!(run["frame_columns"][8], "input_kind");
    assert!(run["frames"].as_array().unwrap().iter().any(|row| row[8] == 1), "frames reflecting a discrete input");
    assert!(run["frames"].as_array().unwrap().iter().any(|row| row[7].as_u64().unwrap() > 0), "frames reflecting input");
    assert_eq!(run["marks"][0][0], "typed");
    assert_eq!(missing["err"]["code"], "timeout");
    assert_eq!((snap_reply["ok"]["width"].as_u64(), snap_reply["ok"]["height"].as_u64()), (Some(640), Some(400)));
    assert!(std::fs::metadata(&snap).is_ok_and(|m| m.len() > 0));
    let _ = std::fs::remove_file(&snap);
    assert!(step["ok"]["virtual_s"].as_f64().unwrap() > 0.0);
    assert_eq!(event["event"], "quit");
}
