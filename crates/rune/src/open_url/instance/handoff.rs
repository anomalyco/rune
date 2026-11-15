use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Opens every message, so a stray client's bytes aren't taken for arguments: a length and
/// the arguments follow, and the sender waits for one byte back (`ACCEPTED`, `QUITTING`).
pub(super) const MAGIC: &[u8] = b"rune-instance 2\0";
/// The running process took the launch.
pub(super) const ACCEPTED: u8 = b'A';
/// The running process quits: the launch claims the instance once it lets go.
pub(super) const QUITTING: u8 = b'Q';
/// How long a launch waits for the running process to answer before it takes the name for
/// held by nobody. A healthy process answers in 63-82 µs (p50-p95, Windows, release); this
/// leaves room for one that waits for a busy CPU, and is what a launch pays while a killed
/// process's pipe lingers and its pid can't be read.
pub(super) const ANSWER_WITHIN: Duration = Duration::from_millis(50);
/// The most a launch's arguments may take.
pub(super) const MAX_MESSAGE: usize = 1 << 20;
/// How long a later launch keeps trying to reach the first process.
pub(super) const REACH_FOR: Duration = Duration::from_secs(5);
/// How long a launch handed on by a quitting process waits for it to let go of the name.
pub(super) const HANDED_WAIT: Duration = Duration::from_secs(10);

/// This launch's message: `MAGIC`, the body's length (u32, little-endian), and the body (the
/// arguments, NUL-separated).
pub(super) fn message() -> Vec<u8> {
    let body = std::env::args().skip(1).collect::<Vec<_>>().join("\0").into_bytes();
    frame(&body)
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut message = MAGIC.to_vec();
    message.extend((body.len() as u32).to_le_bytes());
    message.extend_from_slice(body);
    message
}

// A backwards-compatible non-link argument. Only a client requesting UI delivery adds it;
// existing clients still send precisely the old payload and get the old one-byte answer.
#[cfg(feature = "deferred-quit")]
const PREFIX: &str = "--rune-ui-delivery-until=";
#[cfg(feature = "deferred-quit")]
pub(super) fn ui_message(until: Instant) -> Vec<u8> {
    let expiry = std::time::SystemTime::now() + until.saturating_duration_since(Instant::now());
    let micros = expiry.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_micros();
    frame(std::iter::once(format!("{PREFIX}{micros}")).chain(std::env::args().skip(1)).collect::<Vec<_>>().join("\0").as_bytes())
}
#[cfg(feature = "deferred-quit")]
pub(super) fn ui_answer(args: &[String]) -> Option<u8> {
    use crate::deferred_quit::{ACK_WITHIN, activate, hooks};
    hooks()?;
    let micros: u64 = args.first()?.strip_prefix(PREFIX)?.parse().ok()?;
    let left = std::time::UNIX_EPOCH.checked_add(Duration::from_micros(micros))?.duration_since(std::time::SystemTime::now()).unwrap_or_default().min(ACK_WITHIN);
    Some(if activate(args[1..].to_vec(), Instant::now()+left) { b'R' } else { b'U' })
}

/// A message's body as the launch's arguments; none when it isn't UTF-8.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(super) fn arguments(body: &[u8]) -> Option<Vec<String>> {
    Some(std::str::from_utf8(body).ok()?.split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect())
}
type Relaunch = Box<dyn Fn() -> bool + Send>;

/// Sends a launch without links to the loop as a reopen; set once the loop runs.
static RELAUNCH: Mutex<Option<Relaunch>> = Mutex::new(None);

/// Launches handed to the loop (a reopen, or links for `App::on_open_url`) that no shown
/// window has served yet; once the loop has ended (`exited`), a launch starts the app again
/// instead.
struct Waiting {
    launches: Vec<(Instant, Vec<String>)>,
    exited: bool,
}

static WAITING: Mutex<Waiting> = Mutex::new(Waiting { launches: Vec::new(), exited: false });
/// Whether `WAITING` holds a launch: the loop's per-turn check costs a load.
static PENDING: AtomicBool = AtomicBool::new(false);
/// A launch older than this when the loop ends was served by a window that went later (an app
/// that runs on without windows, then quits): it doesn't start the app again.
const UNSERVED_FOR: Duration = Duration::from_secs(5);

/// Whether a launch waits for a shown window to serve it.
pub(crate) fn launches_waiting() -> bool {
    PENDING.load(Ordering::Acquire)
}

/// The loop goes on with a window shown: the launches so far reached it.
pub(crate) fn served() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).launches.clear();
    PENDING.store(false, Ordering::Release);
}

/// The loop ended: each launch no window served starts the app again once this process lets
/// go of the instance, as does every launch from now on.
pub(crate) fn exited() {
    let launches = {
        let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
        waiting.exited = true;
        std::mem::take(&mut waiting.launches)
    };
    for (at, args) in launches {
        if at.elapsed() < UNSERVED_FOR {
            crate::relaunch::spawn_with(args);
        }
    }
}

/// The loop runs: a launch without links calls `relaunch` from now on (`WakeEvent::Reopen`).
pub(crate) fn on_relaunch(relaunch: impl Fn() -> bool + Send + 'static) {
    *RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(relaunch));
}

/// A later launch's arguments: its links to `App::on_open_url`, or, without links, a reopen.
/// One that comes while the app quits, or that the loop hadn't taken when it ended, starts the
/// app again with them once this process has exited, as `Tx::relaunch` does: the windows it
/// would reach are closing, and the launch would be lost.
pub(super) fn relaunched(args: Vec<String>) {
    if crate::quit::quitting() {
        return crate::relaunch::spawn_with(args);
    }
    let links: Vec<String> = args.iter().filter(|a| super::is_link(a)).cloned().collect();
    {
        let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
        if waiting.exited {
            drop(waiting);
            return crate::relaunch::spawn_with(args);
        }
        waiting.launches.push((Instant::now(), args));
        PENDING.store(true, Ordering::Release);
    }
    if links.is_empty() {
        let sent = RELAUNCH.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|relaunch| relaunch());
        if !sent {
            exited();
        }
        return;
    }
    for link in links {
        super::super::incoming::receive(link);
    }
}
