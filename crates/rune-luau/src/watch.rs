//! [`Script::watch`]: save a `.luau` file and the script reloads. The OS file watcher's own
//! thread reads the folder and compiles the files that changed, while the UI thread builds the
//! next VM's base ([`Script::prewarm`]); the next poll after that only swaps the VM.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use rune::time::Instant;

use crate::script::{Inner, LogLevel, Reload};
use crate::sources::{Compiled, Sources};
use crate::{Script, ScriptError};

type Ready = Arc<Mutex<Slot>>;

#[derive(Default)]
pub(crate) struct Slot {
    /// A save is compiling.
    saving: bool,
    ready: Option<Result<(Sources, Compiled, Duration), ScriptError>>,
}

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
        let mut last: Option<(Sources, Compiled)> = None;
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !event.kind.is_modify() && !event.kind.is_create() && !event.kind.is_remove() {
                return;
            }
            if !event.paths.iter().any(|p| p.extension().is_some_and(|e| e == "luau")) {
                return;
            }
            let sources = Sources::from_dir(&root).map(|s| s.entry(&entry)).map_err(|e| ScriptError::new(crate::ErrorKind::Missing, e.to_string()));
            // One save is often several events (truncate, write): the same files compile once.
            if let (Ok(sources), Some((seen, _))) = (&sources, &last)
                && sources == seen
            {
                return;
            }
            if let Ok(mut slot) = slot.lock() {
                slot.saving = true;
            }
            shared.mark();
            shared.wake();
            let start = Instant::now();
            let result = sources.and_then(|sources| {
                let compiled = match &last {
                    Some((_, previous)) => sources.compile_from(previous),
                    None => sources.compile(),
                }?;
                last = Some((sources.clone(), compiled.clone()));
                Ok((sources, compiled, start.elapsed()))
            });
            if let Ok(mut slot) = slot.lock() {
                *slot = Slot { saving: false, ready: Some(result) };
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
    let (saving, next) = inner.watch.borrow().as_ref().and_then(|w| w.ready.lock().ok().map(|mut s| (s.saving, s.ready.take()))).unwrap_or_default();
    match next {
        None => {
            if saving {
                inner.prewarm();
            }
            false
        }
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
