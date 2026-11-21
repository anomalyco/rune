use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::handoff::{ACCEPTED, ANSWER_WITHIN, HANDED_WAIT, MAGIC, MAX_MESSAGE, QUITTING, REACH_FOR, arguments, message, relaunched};

/// `sockaddr_un`'s path holds 108 bytes, its NUL included.
const PATH_MAX: usize = 107;

/// The claimed socket's path and inode (a launch that took it over binds its own there, which
/// `release` leaves alone), and the listener's thread.
static CLAIMED: OnceLock<(PathBuf, u64)> = OnceLock::new();
static LISTENER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
/// `release` asked the listener to stop.
static RELEASED: AtomicBool = AtomicBool::new(false);

pub(super) fn claim(id: &str) -> bool {
    // No runtime folder of the user's own: run unclaimed rather than not at all.
    let Some(path) = socket_path(id) else { return true };
    let handed = crate::relaunch::handed_from().map(|pid| (pid, crate::time::now() + HANDED_WAIT));
    // One launch at a time decides: between finding the socket stale (or its process hung) and
    // binding its own, no other launch looks. The kernel lets go of a killed holder's lock.
    let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_extension("lock")) else { return true };
    if lock.lock().is_err() {
        return true;
    }
    let deadline = crate::time::now() + REACH_FOR;
    loop {
        match UnixListener::bind(&path) {
            Ok(listener) => return claimed(path, listener),
            Err(e) if e.kind() == ErrorKind::AddrInUse => {}
            Err(_) => return true,
        }
        // A launch a quitting process handed on waits for it to let go (`release_instance`).
        if let Some((pid, until)) = handed
            && crate::time::now() < until
            && crate::relaunch::alive(pid)
        {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        match send(&path) {
            Sent::Delivered => return false,
            Sent::Quitting => wait_free(&path),
            // Nobody listens there: the process that bound it is gone. Or one listens and
            // doesn't answer (hung): the launches from now on are this process's.
            Sent::Refused | Sent::Nobody => {
                std::fs::remove_file(&path).ok();
            }
            // Removed between the bind and the connect: bind again.
            Sent::Gone if crate::time::now() < deadline => {}
            Sent::Gone => return true,
        }
    }
}

/// The socket's path: in `$XDG_RUNTIME_DIR` (the user's own, mode 0700), else in a folder of
/// the user's own under the temporary folder. None when neither can be had, or the path is
/// too long for a socket.
fn socket_path(id: &str) -> Option<PathBuf> {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' }).collect();
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute() && d.is_dir()) {
        Some(dir) => dir,
        None => own_temp_dir()?,
    };
    let path = dir.join(format!("rune-instance-{clean}.sock"));
    (path.as_os_str().len() <= PATH_MAX).then_some(path)
}

/// `<temp>/rune-<uid>`, made 0700; none when it's someone else's or others may write to it.
#[allow(unsafe_code)]
fn own_temp_dir() -> Option<PathBuf> {
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("rune-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    (meta.is_dir() && meta.uid() == uid && meta.permissions().mode() & 0o077 == 0).then_some(dir)
}

fn claimed(path: PathBuf, listener: UnixListener) -> bool {
    let inode = std::fs::metadata(&path).map_or(0, |m| m.ino());
    CLAIMED.set((path, inode)).ok();
    let spawned = std::thread::Builder::new().name("rune-instance".into()).spawn(move || {
        for stream in listener.incoming() {
            if RELEASED.load(Ordering::Acquire) {
                return;
            }
            let Ok(mut stream) = stream else { continue };
            // A client that connects and sends nothing doesn't hold the next launch up for long.
            stream.set_read_timeout(Some(Duration::from_secs(1))).ok();
            let Some(args) = read(&mut stream) else { continue };
            let quitting = crate::quit::quitting();
            // A sender that stopped waiting (this process didn't answer in time, and the launch
            // went on without it) has closed its end: the answer fails and the launch is its own.
            let told = stream.write_all(&[if quitting { QUITTING } else { ACCEPTED }]).is_ok();
            drop(stream);
            // A launch told the app quits claims the instance itself.
            if told && !quitting {
                relaunched(args);
            }
        }
    });
    if let Ok(listener) = spawned {
        *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener);
    }
    true
}

/// Stops listening: a launch from now on binds the name itself (the socket is removed if it's
/// still this process's), and one already connected is told the app quits. Returns once the
/// listener has ended.
pub(super) fn release() {
    let Some((path, inode)) = CLAIMED.get() else { return };
    if RELEASED.swap(true, Ordering::AcqRel) {
        return;
    }
    // A launch took the name over while this process didn't answer: the socket there is its,
    // and this listener, which nothing reaches any more, goes with the process.
    if !std::fs::metadata(path).is_ok_and(|m| m.ino() == *inode) {
        return;
    }
    // An empty connection wakes the listener from `accept`; it's made before the socket goes.
    let wake = UnixStream::connect(path).ok();
    std::fs::remove_file(path).ok();
    if let Some(listener) = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|_| wake.is_some()) {
        listener.join().ok();
    }
}

/// One client's message (`MAGIC`, the body's length, the body); none for anything else.
fn read(stream: &mut UnixStream) -> Option<Vec<String>> {
    let mut head = [0u8; MAGIC.len() + 4];
    stream.read_exact(&mut head).ok()?;
    let len = u32::from_le_bytes(head.strip_prefix(MAGIC)?.try_into().ok()?) as usize;
    if len > MAX_MESSAGE {
        return None;
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    arguments(&body)
}

enum Sent {
    /// The running process took the launch.
    Delivered,
    /// The running process quits: it removes the socket once it has closed what it holds.
    Quitting,
    /// No socket there now.
    Gone,
    /// A socket and nobody listening: its process died without removing it.
    Refused,
    /// A process listens and doesn't answer within `ANSWER_WITHIN` (hung, or stopped).
    Nobody,
}

/// Sends this launch's arguments to the process listening at `path` and waits, at most
/// `ANSWER_WITHIN`, for its answer.
fn send(path: &Path) -> Sent {
    let deadline = crate::time::now() + ANSWER_WITHIN;
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => return Sent::Refused,
        Err(e) if e.kind() == ErrorKind::NotFound => return Sent::Gone,
        Err(_) => return Sent::Nobody,
    };
    let left = || Some(deadline.saturating_duration_since(crate::time::now()).max(Duration::from_millis(1)));
    stream.set_write_timeout(left()).ok();
    let mut answer = [0u8; 1];
    let answered = stream.write_all(&message()).is_ok() && {
        crate::marks::mark("instance.sent");
        stream.set_read_timeout(left()).ok();
        stream.read_exact(&mut answer).is_ok()
    };
    crate::marks::mark("instance.answered");
    match (answered, answer[0]) {
        (true, ACCEPTED) => Sent::Delivered,
        (true, QUITTING) => Sent::Quitting,
        _ => Sent::Nobody,
    }
}

/// Waits, at most `HANDED_WAIT`, for a quitting process to remove its socket (or to die with
/// it); a socket still there then is taken over.
fn wait_free(path: &Path) {
    let until = crate::time::now() + HANDED_WAIT;
    while crate::time::now() < until {
        match UnixStream::connect(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return,
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => break,
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    std::fs::remove_file(path).ok();
}
