use std::cell::Cell;
use std::rc::Rc;

use super::*;

struct FakeConn {
    id: usize,
    dropped: Rc<Cell<usize>>,
}

impl Drop for FakeConn {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

#[test]
fn reuses_one_connection_across_queries() {
    let cache = QueryCache::new();
    let connects = Cell::new(0usize);
    let dropped = Rc::new(Cell::new(0usize));
    let connect = || {
        connects.set(connects.get() + 1);
        Some(FakeConn { id: connects.get(), dropped: dropped.clone() })
    };

    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer((c.id, 10))), Some((1, 10)));
    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer((c.id, 20))), Some((1, 20)));
    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(1));
    assert_eq!(connects.get(), 1);
    assert_eq!(dropped.get(), 0);
}

#[test]
fn keeps_connection_when_query_has_no_answer() {
    let cache = QueryCache::new();
    let connects = Cell::new(0usize);
    let dropped = Rc::new(Cell::new(0usize));
    let connect = || {
        connects.set(connects.get() + 1);
        Some(FakeConn { id: connects.get(), dropped: dropped.clone() })
    };

    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(1));
    assert_eq!(cache.query::<usize>(connect, |_| QueryOutcome::NoAnswer), None);
    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(1));
    assert_eq!(connects.get(), 1);
    assert_eq!(dropped.get(), 0);
}

#[test]
fn drops_and_reconnects_once_when_cached_connection_breaks() {
    let cache = QueryCache::new();
    let connects = Cell::new(0usize);
    let dropped = Rc::new(Cell::new(0usize));
    let connect = || {
        connects.set(connects.get() + 1);
        Some(FakeConn { id: connects.get(), dropped: dropped.clone() })
    };

    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(1));
    // Connection 1 breaks (for instance, the X server restarted); the query drops it and
    // reconnects once so the caller gets an answer from connection 2.
    let answer = cache.query(connect, |c| {
        if c.id == 1 { QueryOutcome::Disconnected } else { QueryOutcome::Answer(c.id) }
    });
    assert_eq!(answer, Some(2));
    assert_eq!(connects.get(), 2);
    assert_eq!(dropped.get(), 1);
    // Subsequent queries reuse connection 2.
    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(2));
    assert_eq!(connects.get(), 2);
}

#[test]
fn fresh_connection_failure_does_not_retry_immediately() {
    let cache = QueryCache::new();
    let connects = Cell::new(0usize);
    let dropped = Rc::new(Cell::new(0usize));
    let connect = || {
        connects.set(connects.get() + 1);
        Some(FakeConn { id: connects.get(), dropped: dropped.clone() })
    };

    assert_eq!(cache.query::<usize>(connect, |_| QueryOutcome::Disconnected), None);
    assert_eq!(connects.get(), 1);
    assert_eq!(dropped.get(), 1);
}

#[test]
fn recovers_once_server_returns_after_disconnect() {
    let cache = QueryCache::new();
    let connects = Cell::new(0usize);
    let server_up = Cell::new(true);
    let dropped = Rc::new(Cell::new(0usize));
    let connect = || {
        connects.set(connects.get() + 1);
        server_up.get().then(|| FakeConn { id: connects.get(), dropped: dropped.clone() })
    };

    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(1));
    server_up.set(false);
    assert_eq!(cache.query::<usize>(connect, |_| QueryOutcome::Disconnected), None);
    assert_eq!(dropped.get(), 1);
    assert_eq!(connects.get(), 2);

    server_up.set(true);
    assert_eq!(cache.query(connect, |c| QueryOutcome::Answer(c.id)), Some(3));
    assert_eq!(connects.get(), 3);
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[test]
fn classify_reply_distinguishes_protocol_errors_from_disconnects() {
    use x11rb::errors::{ConnectionError, ReplyError};
    use x11rb::protocol::ErrorKind;
    use x11rb::x11_utils::X11Error;

    assert_eq!(classify_reply::<u32>(Ok(Some(7))), QueryOutcome::Answer(7));
    assert_eq!(classify_reply::<u32>(Ok(None)), QueryOutcome::NoAnswer);
    let x11_error = X11Error {
        error_kind: ErrorKind::Unknown(0),
        error_code: 0,
        sequence: 1,
        bad_value: 0,
        minor_opcode: 0,
        major_opcode: 0,
        extension_name: None,
        request_name: None,
    };
    assert_eq!(classify_reply::<u32>(Err(ReplyError::X11Error(x11_error))), QueryOutcome::NoAnswer);
    assert_eq!(
        classify_reply::<u32>(Err(ReplyError::ConnectionError(ConnectionError::UnsupportedExtension))),
        QueryOutcome::NoAnswer,
    );
    let io_error = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed");
    assert_eq!(
        classify_reply::<u32>(Err(ReplyError::ConnectionError(ConnectionError::IoError(io_error)))),
        QueryOutcome::Disconnected,
    );
    assert_eq!(
        classify_reply::<u32>(Err(ReplyError::ConnectionError(ConnectionError::UnknownError))),
        QueryOutcome::Disconnected,
    );
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[test]
fn classify_reply_or_id_distinguishes_protocol_errors_from_disconnects() {
    use x11rb::errors::{ConnectionError, ReplyOrIdError};
    use x11rb::protocol::ErrorKind;
    use x11rb::x11_utils::X11Error;

    assert_eq!(classify_reply_or_id::<u32>(Ok(Some(7))), QueryOutcome::Answer(7));
    assert_eq!(classify_reply_or_id::<u32>(Ok(None)), QueryOutcome::NoAnswer);
    let x11_error = X11Error {
        error_kind: ErrorKind::Unknown(0),
        error_code: 0,
        sequence: 1,
        bad_value: 0,
        minor_opcode: 0,
        major_opcode: 0,
        extension_name: None,
        request_name: None,
    };
    assert_eq!(classify_reply_or_id::<u32>(Err(ReplyOrIdError::X11Error(x11_error))), QueryOutcome::NoAnswer);
    assert_eq!(
        classify_reply_or_id::<u32>(Err(ReplyOrIdError::ConnectionError(ConnectionError::UnsupportedExtension))),
        QueryOutcome::NoAnswer,
    );
    assert_eq!(classify_reply_or_id::<u32>(Err(ReplyOrIdError::IdsExhausted)), QueryOutcome::Disconnected);
    assert_eq!(
        classify_reply_or_id::<u32>(Err(ReplyOrIdError::ConnectionError(ConnectionError::UnknownError))),
        QueryOutcome::Disconnected,
    );
}
