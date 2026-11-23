//! Shared connection slot for X11 queries and one-shot client messages (`pointer_position`,
//! `output_crtc` and `activation::activate`). Kept free of `x11rb` except for the shared
//! connection wrapper and error classification so its reuse and reconnect tests run on every host.

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

/// Maps an `x11rb` result that may also allocate an XID into `QueryOutcome`: X11 protocol
/// errors and missing extensions leave the connection healthy, while transport errors or an
/// exhausted XID range mean the connection must be dropped.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn classify_reply_or_id<T>(result: Result<Option<T>, x11rb::errors::ReplyOrIdError>) -> QueryOutcome<T> {
    use x11rb::errors::{ConnectionError, ReplyOrIdError};
    match result {
        Ok(Some(value)) => QueryOutcome::Answer(value),
        Ok(None)
        | Err(ReplyOrIdError::X11Error(_))
        | Err(ReplyOrIdError::ConnectionError(ConnectionError::UnsupportedExtension)) => QueryOutcome::NoAnswer,
        Err(ReplyOrIdError::ConnectionError(_) | ReplyOrIdError::IdsExhausted) => QueryOutcome::Disconnected,
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(super) struct X11Connection {
    pub(super) connection: x11rb::rust_connection::RustConnection,
    pub(super) screen: usize,
    pub(super) root: u32,
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
impl X11Connection {
    fn connect() -> Option<Self> {
        use x11rb::connection::Connection as _;
        let (connection, screen) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        Some(Self { connection, screen, root })
    }

    pub(super) fn screen_info(&self, screen: Option<usize>) -> Option<&x11rb::protocol::xproto::Screen> {
        use x11rb::connection::Connection as _;
        let roots = &self.connection.setup().roots;
        screen.and_then(|s| roots.get(s)).or_else(|| roots.get(self.screen))
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
static SHARED: QueryCache<X11Connection> = QueryCache::new();

/// Runs one X11 request on the process's shared lazily-opened X11 connection, dropping and
/// reconnecting once on a transport error.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(super) fn with_connection<T>(run: impl FnMut(&X11Connection) -> QueryOutcome<T>) -> Option<T> {
    SHARED.query(X11Connection::connect, run)
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
