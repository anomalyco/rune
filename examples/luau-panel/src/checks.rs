//! Behaviour the panel doesn't exercise, shown on small scripts: the watchdog, a failing render,
//! `require` by Luau's rules, a cycle, numbers crossing to Rust, and a host signal and async
//! call. Printed by `luau-bench` and `luau_bench()` next to the performance numbers.

use std::time::Duration;

use rune::time::Instant;
use rune_luau::mlua::Value;
use rune_luau::{Data, ErrorKind, Script, ScriptOptions, Sources};

use crate::State;

pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

pub fn run() -> Vec<Check> {
    vec![watchdog(), render_error(), require_rules(), require_cycle(), numbers(), host_signal_and_async()]
}

fn check(name: &'static str, ok: bool, detail: impl Into<String>) -> Check {
    Check { name, ok, detail: detail.into() }
}

/// `while true do end` stops at the 250 ms limit with a timeout, and the script keeps working.
fn watchdog() -> Check {
    let script = Script::new("spin", Sources::single("return { spin = function() while true do end end, ok = function() return 1 end }"));
    let start = Instant::now();
    let err = script.query("spin", []).err();
    let took = start.elapsed();
    let still = script.query("ok", []).ok().and_then(|d| d.as_i64());
    let ok = err.as_ref().is_some_and(|e| e.kind == ErrorKind::Timeout) && took < Duration::from_millis(260) && still == Some(1);
    check("watchdog", ok, format!("stopped after {:.1} ms ({:?}); the next call returned {still:?}", took.as_secs_f64() * 1e3, err.map(|e| e.kind)))
}

/// A render that raises shows the error card instead of panicking, and the log has the error.
fn render_error() -> Check {
    let script = Script::new("broken", Sources::single("local ui = require('@rune/ui')\nreturn function() local t = nil; return ui.text(t.field) end"));
    let el = script.render_uncached::<State>("render", true);
    drop(el);
    let logged = script.log().iter().any(|l| l.message.contains("attempt to index nil"));
    check("render error", logged, script.log().last().map(|l| l.message.lines().next().unwrap_or_default().to_owned()).unwrap_or_default())
}

/// Luau's rules: from `init.luau` (which stands for its folder), `@self/x` is `x.luau` in the
/// folder and `./x` would be beside the folder, so it fails; from `views/row.luau`, `./util`
/// is `views/util.luau`, and the folder's `.luaurc` alias `@lib` resolves.
fn require_rules() -> Check {
    let sources = Sources::new()
        .with("init.luau", "local row = require('@self/views/row')\nlocal escaped = pcall(require, './views/row')\nreturn { probe = function() return row .. '|' .. tostring(escaped) end }")
        .with("views/row.luau", "return 'row+' .. require('./util') .. '+' .. require('@lib/util')")
        .with("views/util.luau", "return 'util'")
        .with(".luaurc", r#"{ "aliases": { "lib": "./views" } }"#);
    let script = Script::new("req", sources);
    let got = script.query("probe", []).map(|d| d.as_str().unwrap_or_default().to_owned()).unwrap_or_else(|e| e.message);
    check("require", got == "row+util+util|false", got)
}

fn require_cycle() -> Check {
    let sources = Sources::new().with("init.luau", "return require('@self/a')").with("a.luau", "return require('./b')").with("b.luau", "return require('./a')");
    let script = Script::new("cycle", sources);
    let err = script.error().map(|e| e.message).unwrap_or_default();
    check("require cycle", err.contains("require cycle: init.luau -> a.luau -> b.luau -> a.luau"), err.lines().next().unwrap_or_default().to_owned())
}

/// Whole numbers come across as integers, through `Data` and through mlua's serde.
fn numbers() -> Check {
    let script = Script::new("nums", Sources::single("return { values = function() return { small = 3, big = 2^40, half = 2.5 } end }"));
    let data = script.query("values", []).unwrap_or_default();
    let via_data = (data.get("small").as_i64(), data.get("big").as_i64(), data.get("half").as_f64());
    let serde = script.with_lua(|lua| {
        use rune_luau::mlua::LuaSerdeExt;
        let t: Value = lua.load("return { small = 3, big = 2^40 }").eval().unwrap_or(Value::Nil);
        let small = lua.from_value::<std::collections::HashMap<String, i64>>(t.clone()).map_err(|e| e.to_string());
        small.map(|m| (m.get("small").copied(), m.get("big").copied()))
    });
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Values {
        small: i64,
        big: i64,
        half: f64,
    }
    let decoded = data.decode::<Values>().map_err(|e| e.to_string());
    let ok = via_data == (Some(3), Some(1 << 40), Some(2.5)) && decoded == Ok(Values { small: 3, big: 1 << 40, half: 2.5 });
    check("numbers", ok, format!("Data: {via_data:?}; Data::decode: {decoded:?}; mlua serde into i64: {serde:?}"))
}

/// A host signal fires into a connection, and a host async call resumes its task.
fn host_signal_and_async() -> Check {
    let options = ScriptOptions::new().setup(|host| {
        let signal = host.signal("changed")?;
        let double = host.async_function("double", |_, args, done| {
            let n = match args.front() {
                Some(Value::Integer(i)) => *i as f64,
                Some(Value::Number(n)) => *n,
                _ => 0.0,
            };
            rune_luau::spawn_blocking(move || done.complete(n * 2.0));
            Ok(())
        })?;
        let module = host.lua().create_table()?;
        module.raw_set("changed", signal)?;
        module.raw_set("double", double)?;
        host.register_module("@app/host", module)
    });
    let src = "local host = require('@app/host')\nlocal seen = {}\nhost.changed:Connect(function(v) seen.signal = v end)\nreturn { start = function() seen.async = host.double(21) end, seen = function() return seen end }";
    let script = Script::with_options("host", Sources::single(src), options);
    script.fire("changed", [Data::from("hello")]);
    script.call("start", []).ok();
    // The async result arrives on a worker thread; poll until the task resumes.
    let mut rt = rune::Runtime::new((), |_: &(), _: &mut rune::Cx<()>| rune::el(), (10.0, 10.0), rune::Env::new(true, 1.0));
    let mut now = 0.0;
    for _ in 0..200 {
        now += 1.0 / 120.0;
        rt.update_if(|_, tx| script.poll(now, tx));
        if script.query("seen", []).ok().is_some_and(|d| d.get("async") != &Data::Nil) {
            break;
        }
        // On the web the result arrives in a microtask, after this synchronous loop.
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(Duration::from_millis(1));
    }
    let seen = script.query("seen", []).unwrap_or_default();
    let async_done = seen.get("async").as_i64() == Some(42) || cfg!(target_arch = "wasm32");
    let ok = seen.get("signal").as_str() == Some("hello") && async_done;
    check("host signal and async", ok, format!("{seen:?} {:?}", script.log().iter().map(|l| l.message.clone()).collect::<Vec<_>>()))
}
