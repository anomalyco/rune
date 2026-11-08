//! [`ScriptError`]: what a failed script reports instead of panicking.

use std::fmt;

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The source doesn't parse. A reload with a syntax error keeps the running VM.
    Syntax,
    /// The script raised an error, or called something with the wrong arguments.
    Runtime,
    /// One entry into the VM ran past the watchdog limit (250 ms by default).
    Timeout,
    /// The VM reached its memory limit.
    Memory,
    /// The script has no entry module, or the entry module has no such function.
    Missing,
}

/// A script error, with the Luau traceback when there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub kind: ErrorKind,
    pub message: String,
}

/// The message the watchdog raises; [`ScriptError::from`] recognises it.
pub(crate) const WATCHDOG: &str = "script ran longer than its time limit (watchdog)";

impl ScriptError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        ScriptError { kind, message: message.into() }
    }

    /// The first line of the message: `init.luau:12: attempt to index nil`.
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or_default()
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScriptError {}

impl From<mlua::Error> for ScriptError {
    fn from(err: mlua::Error) -> Self {
        match err {
            mlua::Error::SyntaxError { message, .. } => ScriptError::new(ErrorKind::Syntax, message),
            mlua::Error::MemoryError(message) => ScriptError::new(ErrorKind::Memory, message),
            mlua::Error::CallbackError { traceback, cause } => {
                let inner = ScriptError::from((*cause).clone());
                ScriptError::new(inner.kind, format!("{}\n{traceback}", inner.message))
            }
            mlua::Error::WithContext { context, cause } => {
                let inner = ScriptError::from((*cause).clone());
                ScriptError::new(inner.kind, format!("{context}: {}", inner.message))
            }
            other => {
                let message = other.to_string();
                let kind = if message.contains(WATCHDOG) { ErrorKind::Timeout } else { ErrorKind::Runtime };
                ScriptError::new(kind, message)
            }
        }
    }
}
