//! [`Sources`]: a script's files, held in memory so a script runs the same from a folder, from
//! files built into the binary, or on the web, where there is no file system.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{ErrorKind, ScriptError};

/// The file a script starts from.
pub const ENTRY: &str = "init.luau";

/// Configuration files `require` reads aliases from.
const CONFIGS: [&str; 2] = [".luaurc", ".config.luau"];

/// A script's files by path relative to its folder (`init.luau`, `views/row.luau`,
/// `.luaurc`). `require` follows Luau's require-by-string rules among them and never reaches
/// outside the folder: from `init.luau`, `require("@self/row")` is `row.luau`, and
/// `require("./row")` would be a sibling of the folder, so it fails.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    files: BTreeMap<String, Arc<str>>,
    parent_config: Option<Arc<str>>,
    folder: Option<String>,
    entry: Option<String>,
}

impl Sources {
    pub fn new() -> Self {
        Sources::default()
    }

    /// One file, `init.luau`.
    pub fn single(source: impl Into<Arc<str>>) -> Self {
        Sources::new().with(ENTRY, source)
    }

    /// Adds or replaces the file at `path` (`/`-separated, relative to the script's folder).
    pub fn with(mut self, path: impl AsRef<str>, source: impl Into<Arc<str>>) -> Self {
        self.insert(path, source);
        self
    }

    /// Adds or replaces the file at `path`.
    pub fn insert(&mut self, path: impl AsRef<str>, source: impl Into<Arc<str>>) {
        self.files.insert(normalize(path.as_ref()), source.into());
    }

    /// The `.luaurc` of the folder holding the script's folder, as an editor finds it above a
    /// plugin: its aliases resolve for every module. Aliases may only lead back into the
    /// script's folder; built-in modules (`@rune/ui`) resolve before aliases.
    pub fn parent_config(mut self, luaurc: impl Into<Arc<str>>) -> Self {
        self.parent_config = Some(luaurc.into());
        self
    }

    /// The module the script starts from, [`ENTRY`] (`init.luau`) unless set: a host with two
    /// entry points in one folder (`app.luau` beside `init.luau`) starts each as its own script.
    pub fn entry(mut self, path: impl AsRef<str>) -> Self {
        self.entry = Some(normalize(path.as_ref()));
        self
    }

    /// The module the script starts from.
    pub fn entry_path(&self) -> &str {
        self.entry.as_deref().unwrap_or(ENTRY)
    }

    /// The script folder's own name, so an alias in [`Sources::parent_config`] that leads to
    /// `./<name>` resolves into it.
    pub fn folder(mut self, name: impl Into<String>) -> Self {
        self.folder = Some(name.into());
        self
    }

    /// Every `.luau` file and `.luaurc` under `dir`, recursively, with the `.luaurc` of the
    /// folder above it as [`Sources::parent_config`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_dir(dir: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Sources) -> std::io::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if path.is_dir() {
                    walk(root, &path, out)?;
                } else if (name.ends_with(".luau") || CONFIGS.contains(&name))
                    && let Ok(rel) = path.strip_prefix(root)
                {
                    let rel: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                    out.insert(rel.join("/"), std::fs::read_to_string(&path)?);
                }
            }
            Ok(())
        }
        let dir = dir.as_ref();
        let mut out = Sources::new();
        out.folder = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        walk(dir, dir, &mut out)?;
        if let Some(parent) = dir.parent()
            && let Ok(luaurc) = std::fs::read_to_string(parent.join(".luaurc"))
        {
            out.parent_config = Some(luaurc.into());
        }
        Ok(out)
    }

    /// The file at `path`.
    pub fn get(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|s| &**s)
    }

    /// Every file's path, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Compiles every `.luau` file to Luau bytecode (`-O2`, with type information for native
    /// code). Runs on any thread; a watched script compiles off the UI thread.
    pub fn compile(&self) -> Result<Compiled, ScriptError> {
        self.compile_reusing(None)
    }

    /// [`Sources::compile`], reusing `previous`'s bytecode for every module whose source is
    /// the same: a save recompiles only the files it changed.
    pub fn compile_from(&self, previous: &Compiled) -> Result<Compiled, ScriptError> {
        self.compile_reusing(Some(previous))
    }

    fn compile_reusing(&self, previous: Option<&Compiled>) -> Result<Compiled, ScriptError> {
        let compiler = compiler();
        let entry = self.entry_path();
        if !self.files.contains_key(entry) {
            return Err(ScriptError::new(ErrorKind::Missing, format!("the script has no {entry}")));
        }
        let mut modules = BTreeMap::new();
        let mut configs = BTreeMap::new();
        for (path, source) in &self.files {
            if !path.ends_with(".luau") || path.ends_with(".config.luau") {
                configs.insert(path.clone(), source.clone());
                continue;
            }
            if let Some(same) = previous.and_then(|p| p.modules.get(path)).filter(|m| m.source == *source) {
                modules.insert(path.clone(), same.clone());
                continue;
            }
            let code = compiler
                .compile(source.as_bytes())
                .map_err(|e| ScriptError::new(ErrorKind::Syntax, format!("{path}{}", ScriptError::from(e).message)))?;
            modules.insert(path.clone(), Module { code: code.into(), native: is_native(source), source: source.clone() });
        }
        Ok(Compiled { modules, configs, parent_config: self.parent_config.clone(), folder: self.folder.clone(), entry: entry.to_owned() })
    }
}

/// [`Sources`] compiled to bytecode, ready to load into a VM.
#[derive(Clone, Debug)]
pub struct Compiled {
    modules: BTreeMap<String, Module>,
    configs: BTreeMap<String, Arc<str>>,
    parent_config: Option<Arc<str>>,
    pub(crate) folder: Option<String>,
    pub(crate) entry: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Module {
    pub(crate) code: Arc<[u8]>,
    /// The file starts with `--!native`: compile it to native code where the platform allows.
    pub(crate) native: bool,
    /// What it was compiled from, so a later compile of the same text reuses it.
    source: Arc<str>,
}

impl Compiled {
    pub(crate) fn module(&self, path: &str) -> Option<&Module> {
        self.modules.get(path)
    }

    /// Whether folder `dir` ("" for the script's own) holds any file.
    pub(crate) fn is_dir(&self, dir: &str) -> bool {
        dir.is_empty() || {
            let prefix = format!("{dir}/");
            self.modules.keys().chain(self.configs.keys()).any(|k| k.starts_with(&prefix))
        }
    }

    /// The configuration file in folder `dir` ("" for the script's own), or above the script's
    /// folder for `None`.
    pub(crate) fn config(&self, dir: Option<&str>) -> Option<&str> {
        let Some(dir) = dir else { return self.parent_config.as_deref() };
        CONFIGS.iter().find_map(|name| {
            let path = if dir.is_empty() { (*name).to_owned() } else { format!("{dir}/{name}") };
            self.configs.get(&path).map(|s| &**s)
        })
    }
}

pub(crate) fn compiler() -> mlua::chunk::Compiler {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    crate::web::init();
    mlua::chunk::Compiler::new().set_optimization_level(2).set_type_info_level(1).set_debug_level(1)
}

/// Whether a `--!native` hot comment opens the file (before any code).
fn is_native(source: &str) -> bool {
    source.lines().map(str::trim).take_while(|l| l.is_empty() || l.starts_with("--")).any(|l| l.starts_with("--!native"))
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").trim_start_matches('/').to_owned()
}
