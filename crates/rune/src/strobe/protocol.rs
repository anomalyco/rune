//! The bridge's wire format: newline-delimited JSON requests, replies, events and the hello.

use serde_json::{Value, json};

/// One request: `{"id": 7, "op": "click", "args": {...}}`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Request {
    pub id: Value,
    pub op: String,
    pub args: Value,
}

/// Why a request failed, as the contract's error codes name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Code {
    BadRequest,
    NotFound,
    Timeout,
    Unsupported,
    NoWindow,
    Internal,
}

impl Code {
    fn name(self) -> &'static str {
        match self {
            Code::BadRequest => "bad-request",
            Code::NotFound => "not-found",
            Code::Timeout => "timeout",
            Code::Unsupported => "unsupported",
            Code::NoWindow => "no-window",
            Code::Internal => "internal",
        }
    }
}

/// A failed request's code and message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fail {
    pub code: Code,
    pub message: String,
}

impl Fail {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Self { Fail { code, message: message.into() } }
    pub(crate) fn bad(message: impl Into<String>) -> Self { Fail::new(Code::BadRequest, message) }
}

/// Parses one line. A line that isn't a request fails with whatever id it carried.
pub(crate) fn parse(line: &str) -> Result<Request, (Value, Fail)> {
    let value: Value = serde_json::from_str(line).map_err(|e| (Value::Null, Fail::bad(format!("not JSON: {e}"))))?;
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    if !id.is_number() && !id.is_string() { return Err((id, Fail::bad("a request needs an \"id\""))); }
    let Some(op) = value.get("op").and_then(Value::as_str) else { return Err((id, Fail::bad("a request needs an \"op\""))) };
    let args = match value.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(args @ Value::Object(_)) => args.clone(),
        Some(_) => return Err((id, Fail::bad("\"args\" is an object"))),
    };
    Ok(Request { id, op: op.to_owned(), args })
}

pub(crate) fn ok(id: &Value, value: Value) -> String { json!({ "id": id, "ok": value }).to_string() }

pub(crate) fn err(id: &Value, fail: &Fail) -> String {
    json!({ "id": id, "err": { "code": fail.code.name(), "message": fail.message } }).to_string()
}

pub(crate) fn reply(id: &Value, result: Result<Value, Fail>) -> String {
    match result {
        Ok(value) => ok(id, value),
        Err(fail) => err(id, &fail),
    }
}

/// The first line after connecting.
pub(crate) fn hello(token: &str, mode: &str, now_ns: u64) -> String {
    json!({
        "hello": 1, "token": token, "pid": std::process::id(), "toolkit": "rune",
        "toolkit_version": env!("CARGO_PKG_VERSION"), "mode": mode, "debug_build": cfg!(debug_assertions),
        "clock": super::clock::NAME, "now_ns": now_ns,
    }).to_string()
}

pub(crate) fn closed(window: &str) -> String { json!({ "event": "closed", "window": window }).to_string() }

pub(crate) fn quit() -> String { json!({ "event": "quit" }).to_string() }

/// `args[key]` as a number.
pub(crate) fn num(args: &Value, key: &str) -> Option<f64> { args.get(key).and_then(Value::as_f64) }

/// `args[key]` as a string.
pub(crate) fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> { args.get(key).and_then(Value::as_str) }
