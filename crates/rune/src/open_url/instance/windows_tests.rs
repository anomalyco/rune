#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

use crate::machine::{Attempt, Seq};

use super::session::{self, Effect, Event, State};
use super::{ACCEPTED, ANSWER_WITHIN, Listener, MAGIC, MAGIC_V1, MAX_MESSAGE, MAX_V1_WIRE, QUITTING, create, io, io_result, wide};

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

/// Generous bound for "this happens" waits on real pipes and threads: passes as soon as it
/// happens, without flaking when the machine is busy.
const WAIT: Duration = Duration::from_secs(30);
/// Long server read timeout for "rejected or cancelled without waiting for the read timeout"
/// checks, paired with a wide `LONG_TIMEOUT / 2` margin.
const LONG_TIMEOUT: Duration = Duration::from_secs(60);

struct PipeHandle(HANDLE);

// SAFETY: a Win32 pipe handle may be transferred to and closed on another thread.
unsafe impl Send for PipeHandle {}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an open handle owned by this wrapper.
        unsafe { CloseHandle(self.0) };
    }
}

fn test_pipe_name(label: &str) -> Vec<u16> {
    let seq = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // SAFETY: returns this process's PID.
    let pid = unsafe { GetCurrentProcessId() };
    wide(&format!(r"\\.\pipe\rune-instance-test-{pid}-{seq}-{label}"))
}

fn start_listener(name: &[u16], timeout: Duration) -> (Listener, mpsc::Receiver<Vec<String>>) {
    let pipe = create(name).expect("create test pipe");
    let (tx, rx) = mpsc::channel();
    let listener = Listener::start(pipe, timeout, move |args| {
        tx.send(args).is_ok()
    })
    .expect("start listener");
    (listener, rx)
}

fn open_client(name: &[u16], within: Duration) -> Option<PipeHandle> {
    let deadline = crate::time::now() + within;
    loop {
        // SAFETY: `name` is NUL-terminated; opens the client end overlapped.
        let h = unsafe {
            CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut())
        };
        if h != INVALID_HANDLE_VALUE {
            return Some(PipeHandle(h));
        }
        // SAFETY: reads this thread's last error.
        if unsafe { GetLastError() } == ERROR_PIPE_BUSY {
            let left = deadline.saturating_duration_since(crate::time::now()).as_millis();
            if left > 0 {
                // SAFETY: `name` is NUL-terminated.
                unsafe { WaitNamedPipeW(name.as_ptr(), left.min(u128::from(u32::MAX - 1)) as u32) };
                continue;
            }
        }
        return None;
    }
}

fn v2_message(arg: &str) -> Vec<u8> {
    let body = arg.as_bytes();
    let mut msg = MAGIC.to_vec();
    msg.extend((body.len() as u32).to_le_bytes());
    msg.extend(body);
    msg
}

fn send_v2(name: &[u16], arg: &str, within: Duration) -> bool {
    let deadline = crate::time::now() + within;
    let Some(client) = open_client(name, within) else { return false };
    let msg = v2_message(arg);
    let mut ans = [0u8; 1];
    let wrote = io(client.0, deadline, |o| {
        // SAFETY: writes `msg`, which outlives the wait in `io`.
        unsafe { WriteFile(client.0, msg.as_ptr(), msg.len() as u32, std::ptr::null_mut(), o) }
    });
    if wrote != Some(msg.len() as u32) {
        return false;
    }
    let read = io(client.0, deadline, |o| {
        // SAFETY: reads one byte into `ans`, which outlives the wait in `io`.
        unsafe { ReadFile(client.0, ans.as_mut_ptr(), 1, std::ptr::null_mut(), o) }
    });
    read == Some(1) && ans[0] == ACCEPTED
}

fn write_all_result(client: &PipeHandle, bytes: &[u8], within: Duration) -> Result<(), u32> {
    let deadline = crate::time::now() + within;
    let mut offset = 0;
    while offset < bytes.len() {
        let chunk = &bytes[offset..];
        let n = io_result(client.0, Some(deadline), None, |o| {
            // SAFETY: writes `chunk`, which outlives the wait in `io_result`.
            unsafe { WriteFile(client.0, chunk.as_ptr(), chunk.len() as u32, std::ptr::null_mut(), o) }
        })?;
        if n == 0 {
            return Err(0);
        }
        offset += n as usize;
    }
    Ok(())
}

fn write_all(client: &PipeHandle, bytes: &[u8], within: Duration) -> bool {
    write_all_result(client, bytes, within).is_ok()
}

// --- Deterministic state-machine tests (injected clock, no threads or pipes) ---

fn step_vec(seq: &mut Seq, state: &mut State, timeout: Duration, event: Event<'_>) -> Vec<Effect> {
    session::step(seq, state, timeout, event).into_iter().collect()
}

fn connect_read(seq: &mut Seq, state: &mut State, timeout: Duration, now: std::time::Instant) -> (Attempt, std::time::Instant) {
    let effects = step_vec(seq, state, timeout, Event::Connected { now });
    match effects.as_slice() {
        [Effect::Read { attempt, deadline, .. }] => (*attempt, *deadline),
        _ => panic!("expected Read, got {effects:?}"),
    }
}

#[test]
fn session_silent_client_times_out_and_keeps_serving() {
    let t0 = crate::time::now();
    let timeout = Duration::from_secs(1);
    let (mut seq, mut state) = (Seq::default(), State::default());

    let (attempt, deadline) = connect_read(&mut seq, &mut state, timeout, t0);
    assert_eq!(deadline, t0 + timeout);

    let effects = step_vec(&mut seq, &mut state, timeout, Event::TimedOut { attempt, err: WAIT_TIMEOUT });
    assert_eq!(effects, vec![Effect::Disconnect, Effect::Connect]);
    assert_eq!(state, State::Connecting);

    // Next client is served normally.
    let t1 = t0 + timeout;
    let (a2, _) = connect_read(&mut seq, &mut state, timeout, t1);
    assert_eq!(step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt: a2, bytes: &v2_message("test://ok") }), vec![Effect::Hold]);
    let effects = step_vec(&mut seq, &mut state, timeout, Event::Held { now: t1, stopping: false, quitting: false });
    let [Effect::WriteAnswer { attempt: a3, byte: ACCEPTED, cancellable: true, .. }] = effects.as_slice() else {
        panic!("expected WriteAnswer, got {effects:?}")
    };
    let a3 = *a3;
    let effects = step_vec(&mut seq, &mut state, timeout, Event::WroteAnswer { attempt: a3, bytes: 1 });
    assert!(matches!(effects.as_slice(), [Effect::DrainAnswer { attempt, .. }] if *attempt == a3));
    let effects = step_vec(&mut seq, &mut state, timeout, Event::DrainEof { attempt: a3 });
    assert_eq!(effects, vec![Effect::Deliver(vec!["test://ok".into()]), Effect::Disconnect, Effect::Connect]);
}

#[test]
fn session_stale_attempt_changes_nothing() {
    let t0 = crate::time::now();
    let timeout = Duration::from_secs(1);
    let (mut seq, mut state) = (Seq::default(), State::default());

    let (first, _) = connect_read(&mut seq, &mut state, timeout, t0);
    let _ = step_vec(&mut seq, &mut state, timeout, Event::TimedOut { attempt: first, err: WAIT_TIMEOUT });
    let (second, _) = connect_read(&mut seq, &mut state, timeout, t0 + timeout);
    assert_ne!(first, second);

    // A stale timeout or read from the earlier connection is ignored.
    assert!(step_vec(&mut seq, &mut state, timeout, Event::TimedOut { attempt: first, err: WAIT_TIMEOUT }).is_empty());
    assert!(step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt: first, bytes: b"garbage" }).is_empty());
    assert!(step_vec(&mut seq, &mut state, timeout, Event::TimedOut { attempt: Attempt(999), err: WAIT_TIMEOUT }).is_empty());
    assert!(matches!(state, State::Reading { attempt, .. } if attempt == second));
}

#[test]
fn session_slowloris_keeps_one_total_deadline() {
    let t0 = crate::time::now();
    let timeout = Duration::from_millis(120);
    let (mut seq, mut state) = (Seq::default(), State::default());

    let (attempt, initial_deadline) = connect_read(&mut seq, &mut state, timeout, t0);
    assert_eq!(initial_deadline, t0 + timeout);

    // Every trickled byte keeps the same attempt and the original total deadline.
    for &byte in MAGIC {
        let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &[byte] });
        let [Effect::Read { attempt: next_a, deadline: next_d, .. }] = effects.as_slice() else {
            panic!("expected Read, got {effects:?}")
        };
        assert_eq!(*next_a, attempt);
        assert_eq!(*next_d, initial_deadline, "chunked reads must never extend the total deadline");
    }

    let effects = step_vec(&mut seq, &mut state, timeout, Event::TimedOut { attempt, err: WAIT_TIMEOUT });
    assert_eq!(effects, vec![Effect::Disconnect, Effect::Connect]);
}

#[test]
fn session_garbage_prefix_and_oversized_v2_rejected_immediately() {
    let t0 = crate::time::now();
    let timeout = LONG_TIMEOUT;
    let (mut seq, mut state) = (Seq::default(), State::default());

    // Garbage prefix rejected on the first chunk without waiting for the deadline.
    let (attempt, _) = connect_read(&mut seq, &mut state, timeout, t0);
    let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: b"not-a-rune-magic" });
    assert_eq!(effects, vec![Effect::Disconnect, Effect::Connect]);

    // Oversized v2 lengths rejected immediately upon reading the header.
    for bad_len in [(MAX_MESSAGE as u32) + 1, u32::MAX] {
        let (attempt, _) = connect_read(&mut seq, &mut state, timeout, t0);
        let mut head = MAGIC.to_vec();
        head.extend(bad_len.to_le_bytes());
        let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &head });
        assert_eq!(effects, vec![Effect::Disconnect, Effect::Connect]);
    }
}

#[test]
fn session_endless_v1_rejected_at_max_wire() {
    let t0 = crate::time::now();
    let timeout = LONG_TIMEOUT;
    let (mut seq, mut state) = (Seq::default(), State::default());

    let (attempt, _) = connect_read(&mut seq, &mut state, timeout, t0);
    let mut fed = 0usize;
    let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: MAGIC_V1 });
    assert!(matches!(effects.as_slice(), [Effect::Read { .. }]));
    fed += MAGIC_V1.len();

    let chunk = vec![b'a'; session::READ_CHUNK];
    while fed + chunk.len() <= MAX_V1_WIRE {
        let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &chunk });
        assert!(matches!(effects.as_slice(), [Effect::Read { .. }]));
        fed += chunk.len();
    }
    let rem = MAX_V1_WIRE - fed;
    if rem > 0 {
        let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &chunk[..rem] });
        assert!(matches!(effects.as_slice(), [Effect::Read { want: 1, .. }]));
    }
    // One byte past MAX_V1_WIRE disconnects immediately.
    let effects = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: b"x" });
    assert_eq!(effects, vec![Effect::Disconnect, Effect::Connect]);
}

#[test]
fn session_unanswered_v2_drain_times_out_without_delivering() {
    let t0 = crate::time::now();
    let timeout = Duration::from_millis(120);
    let (mut seq, mut state) = (Seq::default(), State::default());

    let (attempt, _) = connect_read(&mut seq, &mut state, timeout, t0);
    assert_eq!(step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &v2_message("test://unanswered") }), vec![Effect::Hold]);
    let effects = step_vec(&mut seq, &mut state, timeout, Event::Held { now: t0, stopping: false, quitting: false });
    let [Effect::WriteAnswer { attempt: ans, byte: ACCEPTED, deadline, cancellable: true }] = effects.as_slice() else {
        panic!("expected WriteAnswer, got {effects:?}")
    };
    let (ans, deadline) = (*ans, *deadline);
    assert_eq!(deadline, t0 + timeout);
    assert_eq!(
        step_vec(&mut seq, &mut state, timeout, Event::WroteAnswer { attempt: ans, bytes: 1 }),
        vec![Effect::DrainAnswer { attempt: ans, deadline, cancellable: true }]
    );
    let effects = step_vec(&mut seq, &mut state, timeout, Event::TimedOut { attempt: ans, err: WAIT_TIMEOUT });
    assert_eq!(effects, vec![Effect::AnswerFailed { phase: "flush", err: WAIT_TIMEOUT }, Effect::Disconnect, Effect::Connect]);
}

#[test]
fn session_stop_aborts_silent_read_and_answers_arrived_launch_with_quitting() {
    let t0 = crate::time::now();
    let timeout = LONG_TIMEOUT;
    let (mut seq, mut state) = (Seq::default(), State::default());

    // Stop while reading a silent client aborts immediately without waiting for the deadline.
    let _ = connect_read(&mut seq, &mut state, timeout, t0);
    assert_eq!(step_vec(&mut seq, &mut state, timeout, Event::Stopped), vec![Effect::Disconnect, Effect::Close]);
    assert_eq!(state, State::Stopped);

    // A launch that already arrived before stop receives QUITTING with a bounded deadline, then closes.
    let mut state = State::default();
    let (attempt, _) = connect_read(&mut seq, &mut state, timeout, t0);
    assert_eq!(step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &v2_message("test://quitting") }), vec![Effect::Hold]);
    let effects = step_vec(&mut seq, &mut state, timeout, Event::Held { now: t0, stopping: true, quitting: true });
    let [Effect::WriteAnswer { attempt: ans, byte: QUITTING, deadline, cancellable: false }] = effects.as_slice() else {
        panic!("expected non-cancellable QUITTING WriteAnswer, got {effects:?}")
    };
    let (ans, deadline) = (*ans, *deadline);
    assert_eq!(deadline, t0 + ANSWER_WITHIN);
    // Repeated Stopped while sending QUITTING is ignored; DrainEof finishes and closes without delivering.
    assert!(step_vec(&mut seq, &mut state, timeout, Event::Stopped).is_empty());
    let _ = step_vec(&mut seq, &mut state, timeout, Event::WroteAnswer { attempt: ans, bytes: 1 });
    assert_eq!(step_vec(&mut seq, &mut state, timeout, Event::DrainEof { attempt: ans }), vec![Effect::Disconnect, Effect::Close]);
    assert_eq!(state, State::Stopped);
}

// --- Real named-pipe integration tests ---

#[test]
fn silent_client_does_not_wedge_pipe() {
    let name = test_pipe_name("silent");
    let timeout = Duration::from_millis(120);
    let (listener, rx) = start_listener(&name, timeout);

    // Silent client connects, writes nothing, and keeps its handle open.
    let _silent = open_client(&name, WAIT).expect("silent client connects");

    let start = crate::time::now();
    assert!(send_v2(&name, "test://after-silent", WAIT), "real client must be served despite an open silent client");
    assert!(start.elapsed() < WAIT, "real client must be served within the bound");
    assert_eq!(rx.recv_timeout(WAIT).ok(), Some(vec!["test://after-silent".to_owned()]));

    listener.stop();
}

#[test]
fn stop_aborts_connected_silent_client_promptly() {
    let name = test_pipe_name("stop-silent");
    let pipe = create(&name).expect("create test pipe");
    let (tx, _rx) = mpsc::channel();
    let (reading_tx, reading_rx) = mpsc::channel();
    // Long client timeout: `stop()` must cancel immediately instead of waiting for it.
    let listener = Listener::start_notifying_read(pipe, LONG_TIMEOUT, reading_tx, move |args| {
        tx.send(args).is_ok()
    })
    .expect("start listener");

    let _silent = open_client(&name, WAIT).expect("silent client connects");
    reading_rx.recv_timeout(WAIT).expect("server enters ReadFile on silent client");

    let start = crate::time::now();
    listener.stop();
    assert!(
        start.elapsed() < LONG_TIMEOUT / 2,
        "stop must abort a silent client without waiting for the read timeout"
    );

    // Pipe handle must have been closed so the name can be claimed again immediately.
    let reopened = create(&name).expect("pipe name must be free after stop");
    drop(PipeHandle(reopened));
}

#[test]
fn silent_client_after_valid_message_does_not_wedge_answer() {
    let name = test_pipe_name("silent-answer");
    let timeout = Duration::from_millis(120);
    let (listener, rx) = start_listener(&name, timeout);

    // Hostile client sends a valid v2 launch, then never reads the 1-byte answer and never closes.
    let hostile = open_client(&name, WAIT).expect("hostile client connects");
    assert!(write_all(&hostile, &v2_message("test://unanswered"), WAIT));

    assert!(send_v2(&name, "test://after-unanswered", WAIT), "real client must be served after a client ignores its answer");
    assert_eq!(rx.recv_timeout(WAIT).ok(), Some(vec!["test://after-unanswered".to_owned()]));

    drop(hostile);
    listener.stop();
}

#[test]
fn oversized_v2_length_rejected_immediately() {
    let name = test_pipe_name("oversized-v2");
    // Long timeout: invalid header length must be rejected right away, not at the timeout.
    let (listener, rx) = start_listener(&name, LONG_TIMEOUT);

    for bad_len in [(MAX_MESSAGE as u32) + 1, u32::MAX] {
        let client = open_client(&name, WAIT).expect("client connects");
        let mut head = MAGIC.to_vec();
        head.extend(bad_len.to_le_bytes());
        assert!(write_all(&client, &head, WAIT));

        let start = crate::time::now();
        assert!(send_v2(&name, "test://after-bad-len", WAIT), "pipe must keep serving after oversized length");
        assert!(
            start.elapsed() < LONG_TIMEOUT / 2,
            "oversized length must be rejected before the client timeout"
        );
        assert_eq!(rx.recv_timeout(WAIT).ok(), Some(vec!["test://after-bad-len".to_owned()]));
        drop(client);
    }

    listener.stop();
}

#[test]
fn endless_v1_message_is_bounded_and_pipe_keeps_serving() {
    let name = test_pipe_name("endless-v1");
    let (listener, rx) = start_listener(&name, LONG_TIMEOUT);

    let client = open_client(&name, WAIT).expect("v1 flood client connects");
    assert!(write_all(&client, MAGIC_V1, WAIT));
    let chunk = vec![b'a'; 16 * 1024];
    let mut disconnected = false;
    // Stream well past MAX_MESSAGE; the server must disconnect once the bound is exceeded.
    for _ in 0..((MAX_MESSAGE / chunk.len()) + 32) {
        match write_all_result(&client, &chunk, WAIT) {
            Ok(()) => {}
            Err(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED) => {
                disconnected = true;
                break;
            }
            Err(err) => panic!("unexpected write error {err} while flooding v1 pipe (expected disconnect)"),
        }
    }
    assert!(disconnected, "server must disconnect a v1 client that exceeds MAX_MESSAGE");
    drop(client);

    assert!(send_v2(&name, "test://after-flood", WAIT));
    assert_eq!(rx.recv_timeout(WAIT).ok(), Some(vec!["test://after-flood".to_owned()]));

    listener.stop();
}

#[test]
fn garbage_prefix_rejected_immediately() {
    let name = test_pipe_name("garbage");
    let (listener, rx) = start_listener(&name, LONG_TIMEOUT);

    let _bad = open_client(&name, WAIT).expect("bad client connects");
    assert!(write_all(&_bad, b"not-a-rune-magic", WAIT));

    let start = crate::time::now();
    assert!(send_v2(&name, "test://after-garbage", WAIT));
    assert!(
        start.elapsed() < LONG_TIMEOUT / 2,
        "non-magic prefix must be rejected before the timeout"
    );
    assert_eq!(rx.recv_timeout(WAIT).ok(), Some(vec!["test://after-garbage".to_owned()]));

    listener.stop();
}

#[test]
fn slowloris_client_dropped_at_total_deadline() {
    let name = test_pipe_name("slowloris");
    let timeout = Duration::from_millis(120);
    let (listener, rx) = start_listener(&name, timeout);

    let slow = open_client(&name, WAIT).expect("slow client connects");
    let done = Arc::new(AtomicBool::new(false));
    let done_bg = Arc::clone(&done);
    let trickle = std::thread::spawn(move || {
        // Trickle valid MAGIC prefix bytes slowly past the total deadline, and keep the handle
        // open until the real client is served so the server's total deadline must be what drops it.
        for &byte in MAGIC {
            if done_bg.load(Ordering::Acquire) || !write_all(&slow, &[byte], WAIT) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        while !done_bg.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(slow);
    });

    assert!(send_v2(&name, "test://after-slowloris", WAIT));
    assert_eq!(rx.recv_timeout(WAIT).ok(), Some(vec!["test://after-slowloris".to_owned()]));

    done.store(true, Ordering::Release);
    trickle.join().ok();
    listener.stop();
}

#[test]
fn v1_client_message_is_delivered() {
    let name = test_pipe_name("v1");
    let (listener, rx) = start_listener(&name, WAIT);

    let client = open_client(&name, WAIT).expect("v1 client connects");
    let mut msg = MAGIC_V1.to_vec();
    msg.extend_from_slice(b"test://legacy\0second-arg");
    assert!(write_all(&client, &msg, WAIT));
    drop(client);

    assert_eq!(
        rx.recv_timeout(WAIT).ok(),
        Some(vec!["test://legacy".to_owned(), "second-arg".to_owned()])
    );

    listener.stop();
}

#[test]
fn session_stop_while_writing_or_draining_cancellable_answer_closes() {
    let t0 = crate::time::now();
    let timeout = LONG_TIMEOUT;
    let (mut seq, mut state) = (Seq::default(), State::default());

    let (attempt, _) = connect_read(&mut seq, &mut state, timeout, t0);
    let _ = step_vec(&mut seq, &mut state, timeout, Event::ReadBytes { attempt, bytes: &v2_message("test://a") });
    let _ = step_vec(&mut seq, &mut state, timeout, Event::Held { now: t0, stopping: false, quitting: false });
    assert_eq!(
        step_vec(&mut seq, &mut state, timeout, Event::Stopped),
        vec![Effect::AnswerFailed { phase: "write", err: ERROR_OPERATION_ABORTED }, Effect::Disconnect, Effect::Close]
    );
    assert_eq!(state, State::Stopped);
}
