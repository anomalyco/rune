//! [`Script::watch`]: save a `.luau` file and the script reloads. The OS file watcher's own
//! thread reads and compiles the folder, so the UI thread only swaps the VM, at the next poll.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use rune::time::Instant;

use crate::script::{Inner, LogLevel, Reload};
use crate::sources::{Compiled, Sources};
use crate::{Script, ScriptError};

type Ready = Arc<Mutex<Option<Result<(Sources, Compiled, Duration), ScriptError>>>>;

pub(crate) struct Watch {
    _watcher: notify::RecommendedWatcher,
    ready: Ready,
}

impl Script {
    /// Reloads from `dir` whenever a `.luau` file under it changes. Compiling happens on the
    /// watcher's thread; the swap happens in the next [`Script::poll`].
    pub fn watch(&self, dir: impl AsRef<Path>) -> notify::Result<()> {
        let dir: PathBuf = dir.as_ref().to_owned();
        let ready: Ready = Arc::default();
        let shared = self.inner().shared.clone();
        let slot = ready.clone();
        let root = dir.clone();
        let entry = self.inner().sources.borrow().entry_path().to_owned();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !event.kind.is_modify() && !event.kind.is_create() && !event.kind.is_remove() {
                return;
            }
            if !event.paths.iter().any(|p| p.extension().is_some_and(|e| e == "luau")) {
                return;
            }
            let result = Sources::from_dir(&root).map(|s| s.entry(&entry)).map_err(|e| ScriptError::new(crate::ErrorKind::Missing, e.to_string())).and_then(|sources| {
                let start = Instant::now();
                sources.compile().map(|c| (sources, c, start.elapsed()))
            });
            if let Ok(mut slot) = slot.lock() {
                *slot = Some(result);
            }
            shared.mark();
            shared.wake();
        })?;
        watcher.watch(&dir, RecursiveMode::Recursive)?;
        *self.inner().watch.borrow_mut() = Some(Watch { _watcher: watcher, ready });
        Ok(())
    }
}

/// Swaps in what the watcher compiled, if anything. True when the VM changed.
pub(crate) fn apply(inner: &Inner) -> bool {
    let next = inner.watch.borrow().as_ref().and_then(|w| w.ready.lock().ok().and_then(|mut r| r.take()));
    match next {
        None => false,
        Some(Err(err)) => {
            inner.log(LogLevel::Error, &err.message);
            false
        }
        Some(Ok((sources, compiled, compile))) => match inner.swap(compiled) {
            Ok(report) => {
                inner.set_sources(sources);
                inner.note_reload(Reload { compile, ..report });
                true
            }
            Err(err) => {
                inner.log(LogLevel::Error, &err.message);
                false
            }
        },
    }
}
