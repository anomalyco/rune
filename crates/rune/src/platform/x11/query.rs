//! Shared connection slot for X11 queries (`pointer_position` and `output_crtc`). Kept free of
//! `x11rb` except for error classification so its reuse and reconnect tests run on every host.

use std::sync::Mutex;

/// The outcome of running one query on a cached X11 connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueryOutcome<T> {
    /// The server answered with a value; keep the connection.
    Answer(T),
    /// The connection is still healthy, but the query had no answer (pointer on another screen,
    /// output with no CRTC, X11 protocol error, or unsupported extension); keep the connection.
    NoAnswer,
    /// The connection itself failed; drop it and reconnect.
    Disconnected,
}

/// One lazily-opened connection shared across X11 queries.
pub(crate) struct QueryCache<C> {
    connection: Mutex<Option<C>>,
}

impl<C> QueryCache<C> {
    pub(crate) const fn new() -> Self {
        Self { connection: Mutex::new(None) }
    }

    pub(crate) fn query<T>(
        &self,
        mut connect: impl FnMut() -> Option<C>,
        mut run: impl FnMut(&C) -> QueryOutcome<T>,
    ) -> Option<T> {
        let mut slot = self.connection.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        });
        let reused = slot.is_some();
        if !reused {
            *slot = connect();
        }
        let conn = slot.as_ref()?;
        match run(conn) {
            QueryOutcome::Answer(value) => Some(value),
            QueryOutcome::NoAnswer => None,
            QueryOutcome::Disconnected => {
                *slot = None;
                if !reused {
                    return None;
                }
                *slot = connect();
                let conn = slot.as_ref()?;
                match run(conn) {
                    QueryOutcome::Answer(value) => Some(value),
                    QueryOutcome::NoAnswer => None,
                    QueryOutcome::Disconnected => {
                        *slot = None;
                        None
                    }
                }
            }
        }
    }
}

/// Maps an `x11rb` query result into `QueryOutcome`: X11 protocol errors and missing extensions
/// leave the connection healthy, while transport errors mean the connection must be dropped.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn classify_reply<T>(result: Result<Option<T>, x11rb::errors::ReplyError>) -> QueryOutcome<T> {
    use x11rb::errors::{ConnectionError, ReplyError};
    match result {
        Ok(Some(value)) => QueryOutcome::Answer(value),
        Ok(None)
        | Err(ReplyError::X11Error(_))
        | Err(ReplyError::ConnectionError(ConnectionError::UnsupportedExtension)) => QueryOutcome::NoAnswer,
        Err(ReplyError::ConnectionError(_)) => QueryOutcome::Disconnected,
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
