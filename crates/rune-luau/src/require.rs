//! `require` inside a script's folder, by Luau's require-by-string rules, so the host and
//! `luau-lsp` resolve the same module: Luau's own navigator walks the path, and this is its
//! view of the script's files.
//!
//! - A module's path drops `.luau`, and `x/init.luau` is module `x`: `init.luau` stands for its
//!   folder. `./` and `../` start from the requiring module's parent, so from the script's
//!   `init.luau` they leave the folder and fail; `@self/x` starts from the module itself and
//!   reaches `x.luau` in the script's folder.
//! - `.luaurc` (or `.config.luau`) aliases in the script's folders apply, and so does the one
//!   above the folder ([`Sources::parent_config`](crate::Sources::parent_config)). Built-in
//!   modules (`@rune/ui`, `@rune/signal`, a host's) resolve before any of these.
//! - Nothing outside the folder resolves. Each module runs once per VM, and a cycle is an error
//!   naming the chain.

use std::cell::RefCell;
use std::rc::Rc;
use std::result::Result as StdResult;

use mlua::chunk::ChunkMode;
use mlua::luau::{NavigateError, Require};
use mlua::{Function, Lua, MultiValue};

use crate::sources::{Compiled, ENTRY};

/// Modules running their top level, outermost first.
pub(crate) type Loading = Rc<RefCell<Vec<String>>>;

pub(crate) struct Requirer {
    compiled: Rc<Compiled>,
    loading: Loading,
    /// The current module path, or `None` above the script's folder.
    at: Option<Vec<String>>,
}

impl Requirer {
    pub(crate) fn new(compiled: Rc<Compiled>, loading: Loading) -> Self {
        Requirer { compiled, loading, at: Some(Vec::new()) }
    }

    /// The file of module `at`.
    fn file(&self, at: &[String]) -> StdResult<Option<String>, NavigateError> {
        if at.is_empty() {
            return Ok(self.compiled.module(ENTRY).map(|_| ENTRY.to_owned()));
        }
        let base = at.join("/");
        let found: Vec<String> =
            [format!("{base}.luau"), format!("{base}/init.luau")].into_iter().filter(|p| self.compiled.module(p).is_some()).collect();
        match found.len() {
            0 => Ok(None),
            1 => Ok(found.into_iter().next()),
            _ => Err(NavigateError::Ambiguous),
        }
    }

    fn resolved(&self) -> Option<String> {
        self.at.as_deref().and_then(|at| self.file(at).ok().flatten())
    }
}

impl Require for Requirer {
    fn is_require_allowed(&self, chunk_name: &str) -> bool {
        chunk_name.starts_with('@')
    }

    fn reset(&mut self, chunk_name: &str) -> StdResult<(), NavigateError> {
        let path = chunk_name.strip_prefix('@').ok_or(NavigateError::NotFound)?;
        let path = path.strip_suffix(".luau").unwrap_or(path);
        let mut at: Vec<String> = path.split('/').filter(|c| !c.is_empty()).map(str::to_owned).collect();
        if at.last().is_some_and(|c| c == "init") {
            at.pop();
        }
        self.at = Some(at);
        Ok(())
    }

    fn jump_to_alias(&mut self, _path: &str) -> StdResult<(), NavigateError> {
        // An alias to an absolute path names something outside the script's folder.
        Err(NavigateError::NotFound)
    }

    fn to_parent(&mut self) -> StdResult<(), NavigateError> {
        match &mut self.at {
            Some(at) if at.is_empty() => self.at = None,
            Some(at) => {
                at.pop();
            }
            None => return Err(NavigateError::NotFound),
        }
        Ok(())
    }

    fn to_child(&mut self, name: &str) -> StdResult<(), NavigateError> {
        let Some(at) = &self.at else {
            // Above the folder, only the folder itself is reachable.
            if self.compiled.folder.as_deref() == Some(name) {
                self.at = Some(Vec::new());
                return Ok(());
            }
            return Err(NavigateError::NotFound);
        };
        let mut next = at.clone();
        next.push(name.to_owned());
        if self.file(&next)?.is_none() && !self.compiled.is_dir(&next.join("/")) {
            return Err(NavigateError::NotFound);
        }
        self.at = Some(next);
        Ok(())
    }

    fn has_module(&self) -> bool {
        self.resolved().is_some()
    }

    fn cache_key(&self) -> String {
        self.resolved().unwrap_or_default()
    }

    fn has_config(&self) -> bool {
        self.config_text().is_some()
    }

    fn config(&self) -> std::io::Result<Vec<u8>> {
        self.config_text().map(|s| s.as_bytes().to_vec()).ok_or_else(|| std::io::ErrorKind::NotFound.into())
    }

    fn loader(&self, lua: &Lua) -> mlua::Result<Function> {
        let path = self.resolved().ok_or_else(|| mlua::Error::runtime("module not found"))?;
        module_loader(lua, &self.compiled, &self.loading, &path)
    }
}

impl Requirer {
    /// The configuration of the folder navigation is at: a module path is a folder here.
    fn config_text(&self) -> Option<&str> {
        match &self.at {
            None => self.compiled.config(None),
            Some(at) => self.compiled.config(Some(&at.join("/"))),
        }
    }
}

/// A function that runs module `path` once, refusing to start it again while it is still
/// running (a `require` cycle). A module that opens with `--!native` compiles to native code
/// where the platform allows it.
pub(crate) fn module_loader(lua: &Lua, compiled: &Compiled, loading: &Loading, path: &str) -> mlua::Result<Function> {
    let module = compiled.module(path).ok_or_else(|| mlua::Error::runtime(format!("no module {path}")))?;
    #[cfg(luau_jit)]
    lua.enable_jit(module.native);
    #[cfg(not(luau_jit))]
    let _ = module.native;
    let chunk = lua.load(&*module.code).set_mode(ChunkMode::Binary).set_name(format!("@{path}")).into_function();
    #[cfg(luau_jit)]
    lua.enable_jit(false);
    let chunk = chunk?;
    let loading = loading.clone();
    let path = path.to_owned();
    lua.create_function(move |_, args: MultiValue| {
        if loading.borrow().contains(&path) {
            let mut chain = loading.borrow().clone();
            chain.push(path.clone());
            return Err(mlua::Error::runtime(format!("require cycle: {}", chain.join(" -> "))));
        }
        loading.borrow_mut().push(path.clone());
        let out = chunk.call::<MultiValue>(args);
        loading.borrow_mut().pop();
        out
    })
}
