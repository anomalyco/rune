//! Whether interaction timing runs, read once from the environment, and where it sends.
//!
//! On when `RUNE_PERF=1`, or (in debug builds and release builds with Rune's `tooling` feature)
//! when `OTEL_EXPORTER_OTLP_ENDPOINT` names a loopback address (a developer's local collector).
//! `RUNE_PERF=0` turns it off over either. A headless run turns on only for `RUNE_PERF=1`, so
//! agents' snapshots and tests don't send. Release builds without `tooling` ignore an ambient
//! `OTEL_EXPORTER_OTLP_ENDPOINT` unless `RUNE_PERF=1` is set.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// Where `RUNE_PERF=1` sends when no loopback endpoint is set: a local OTLP/HTTP collector.
pub(crate) const DEFAULT_ENDPOINT: &str = "127.0.0.1:27686";

const UNKNOWN: u8 = 0;
const OFF: u8 = 1;
const ON: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);
static ENDPOINT: OnceLock<Option<SocketAddr>> = OnceLock::new();

#[inline(always)]
pub(crate) fn on() -> bool {
    #[cfg(target_arch = "wasm32")]
    return false;
    #[cfg(all(not(target_arch = "wasm32"), test))]
    return FORCED.get();
    #[cfg(all(not(target_arch = "wasm32"), not(test)))]
    match STATE.load(Ordering::Relaxed) {
        ON => true,
        OFF => false,
        _ => decide(false),
    }
}

/// A headless run: on only for an explicit `RUNE_PERF=1`.
pub(crate) fn headless() { decide(true); }

#[cold]
fn decide(headless: bool) -> bool {
    let perf = rune_core::env::var("RUNE_PERF");
    let ambient = crate::tooling::dev_tooling_enabled();
    let otel = (ambient || perf.as_deref().map(str::trim) == Some("1"))
        .then(|| rune_core::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
        .flatten();
    let on = !cfg!(test) && !cfg!(target_arch = "wasm32") && wanted(perf.as_deref(), otel.as_deref(), headless, ambient);
    if on { ENDPOINT.set(endpoint(otel.as_deref())).ok(); }
    STATE.store(if on { ON } else { OFF }, Ordering::Relaxed);
    on
}

/// Whether the settings turn timing on.
pub(crate) fn wanted(perf: Option<&str>, otel: Option<&str>, headless: bool, ambient_otel: bool) -> bool {
    match perf.map(str::trim) {
        Some("0") => false,
        Some("1") => true,
        _ => ambient_otel && !headless && otel.and_then(loopback).is_some(),
    }
}

/// Where spans go: the loopback endpoint set, else [`DEFAULT_ENDPOINT`].
pub(crate) fn endpoint(otel: Option<&str>) -> Option<SocketAddr> {
    otel.and_then(loopback).or_else(|| DEFAULT_ENDPOINT.parse().ok())
}

/// `http://host:port[/path]` or `host:port` on loopback; anything else is `None`.
pub(crate) fn loopback(setting: &str) -> Option<SocketAddr> {
    let rest = setting.trim();
    let rest = rest.strip_prefix("http://").unwrap_or(rest);
    let host = rest.split('/').next()?.replace("localhost", "127.0.0.1");
    host.parse::<SocketAddr>().ok().filter(|a| a.ip().is_loopback())
}

/// The collector, once timing is on.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn collector() -> Option<SocketAddr> { ENDPOINT.get().copied().flatten() }

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Turns timing on for this test thread, sending nowhere.
#[cfg(test)]
pub(crate) fn force_on() { FORCED.set(true); }
