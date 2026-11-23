#![allow(unsafe_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

use super::{ACCEPTED, Listener, MAGIC, MAGIC_V1, MAX_MESSAGE, create, io, wide};

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

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

fn write_all(client: &PipeHandle, bytes: &[u8], within: Duration) -> bool {
    let deadline = crate::time::now() + within;
    let mut offset = 0;
    while offset < bytes.len() {
        let chunk = &bytes[offset..];
        let Some(n) = io(client.0, deadline, |o| {
            // SAFETY: writes `chunk`, which outlives the wait in `io`.
            unsafe { WriteFile(client.0, chunk.as_ptr(), chunk.len() as u32, std::ptr::null_mut(), o) }
        }) else {
            return false;
        };
        if n == 0 {
            return false;
        }
        offset += n as usize;
    }
    true
}

#[test]
fn silent_client_does_not_wedge_pipe() {
    let name = test_pipe_name("silent");
    let timeout = Duration::from_millis(120);
    let (listener, rx) = start_listener(&name, timeout);

    // Silent client connects, writes nothing, and keeps its handle open.
    let _silent = open_client(&name, Duration::from_secs(1)).expect("silent client connects");

    let start = crate::time::now();
    assert!(send_v2(&name, "test://after-silent", Duration::from_secs(2)), "real client must be served despite an open silent client");
    assert!(start.elapsed() < Duration::from_secs(2), "real client must be served within the bound");
    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), Some(vec!["test://after-silent".to_owned()]));

    listener.stop();
}

#[test]
fn stop_aborts_connected_silent_client_promptly() {
    let name = test_pipe_name("stop-silent");
    // Long client timeout: `stop()` must cancel immediately instead of waiting for it.
    let (listener, _rx) = start_listener(&name, Duration::from_secs(10));

    let _silent = open_client(&name, Duration::from_secs(1)).expect("silent client connects");
    std::thread::sleep(Duration::from_millis(20));

    let start = crate::time::now();
    listener.stop();
    assert!(start.elapsed() < Duration::from_secs(1), "stop must abort a silent client without waiting for the read timeout");

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
    let hostile = open_client(&name, Duration::from_secs(1)).expect("hostile client connects");
    assert!(write_all(&hostile, &v2_message("test://unanswered"), Duration::from_secs(1)));

    assert!(send_v2(&name, "test://after-unanswered", Duration::from_secs(2)), "real client must be served after a client ignores its answer");
    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), Some(vec!["test://after-unanswered".to_owned()]));

    drop(hostile);
    listener.stop();
}

#[test]
fn oversized_v2_length_rejected_immediately() {
    let name = test_pipe_name("oversized-v2");
    // Long timeout: invalid header length must be rejected right away, not at the timeout.
    let (listener, rx) = start_listener(&name, Duration::from_secs(5));

    for bad_len in [(MAX_MESSAGE as u32) + 1, u32::MAX] {
        let client = open_client(&name, Duration::from_secs(1)).expect("client connects");
        let mut head = MAGIC.to_vec();
        head.extend(bad_len.to_le_bytes());
        assert!(write_all(&client, &head, Duration::from_secs(1)));

        let start = crate::time::now();
        assert!(send_v2(&name, "test://after-bad-len", Duration::from_secs(1)), "pipe must keep serving after oversized length");
        assert!(start.elapsed() < Duration::from_secs(1), "oversized length must be rejected before the client timeout");
        assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), Some(vec!["test://after-bad-len".to_owned()]));
        drop(client);
    }

    listener.stop();
}

#[test]
fn endless_v1_message_is_bounded_and_pipe_keeps_serving() {
    let name = test_pipe_name("endless-v1");
    let (listener, rx) = start_listener(&name, Duration::from_secs(2));

    let client = open_client(&name, Duration::from_secs(1)).expect("v1 flood client connects");
    assert!(write_all(&client, MAGIC_V1, Duration::from_secs(1)));
    let chunk = vec![b'a'; 16 * 1024];
    let mut disconnected = false;
    // Stream well past MAX_MESSAGE; the server must disconnect once the bound is exceeded.
    for _ in 0..((MAX_MESSAGE / chunk.len()) + 32) {
        if !write_all(&client, &chunk, Duration::from_millis(500)) {
            disconnected = true;
            break;
        }
    }
    assert!(disconnected, "server must disconnect a v1 client that exceeds MAX_MESSAGE");
    drop(client);

    assert!(send_v2(&name, "test://after-flood", Duration::from_secs(1)));
    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), Some(vec!["test://after-flood".to_owned()]));

    listener.stop();
}

#[test]
fn garbage_prefix_rejected_immediately() {
    let name = test_pipe_name("garbage");
    let (listener, rx) = start_listener(&name, Duration::from_secs(5));

    let _bad = open_client(&name, Duration::from_secs(1)).expect("bad client connects");
    assert!(write_all(&_bad, b"not-a-rune-magic", Duration::from_secs(1)));

    let start = crate::time::now();
    assert!(send_v2(&name, "test://after-garbage", Duration::from_secs(1)));
    assert!(start.elapsed() < Duration::from_secs(1), "non-magic prefix must be rejected before the timeout");
    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), Some(vec!["test://after-garbage".to_owned()]));

    listener.stop();
}

#[test]
fn slowloris_client_dropped_at_total_deadline() {
    let name = test_pipe_name("slowloris");
    let timeout = Duration::from_millis(120);
    let (listener, rx) = start_listener(&name, timeout);

    let slow = open_client(&name, Duration::from_secs(1)).expect("slow client connects");
    let trickle = std::thread::spawn(move || {
        // Trickle valid MAGIC prefix bytes slowly past the total deadline.
        for &byte in MAGIC {
            if !write_all(&slow, &[byte], Duration::from_millis(200)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        drop(slow);
    });

    assert!(send_v2(&name, "test://after-slowloris", Duration::from_secs(2)));
    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), Some(vec!["test://after-slowloris".to_owned()]));

    trickle.join().ok();
    listener.stop();
}

#[test]
fn v1_client_message_is_delivered() {
    let name = test_pipe_name("v1");
    let (listener, rx) = start_listener(&name, Duration::from_secs(1));

    let client = open_client(&name, Duration::from_secs(1)).expect("v1 client connects");
    let mut msg = MAGIC_V1.to_vec();
    msg.extend_from_slice(b"test://legacy\0second-arg");
    assert!(write_all(&client, &msg, Duration::from_secs(1)));
    drop(client);

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).ok(),
        Some(vec!["test://legacy".to_owned(), "second-arg".to_owned()])
    );

    listener.stop();
}
