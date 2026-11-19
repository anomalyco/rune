//! Optional timing hook for text shaping and line-breaking phases (`Phase::Text` in `rune`).
//!
//! `rune` registers its frame-profile / budget / interaction hooks once; when unset, entering a
//! scope is one atomic load.

use std::sync::OnceLock;

/// Opaque token returned by an enter hook and passed to its exit hook on drop.
/// Zero means the scope was not timed and the exit hook is skipped.
pub type Token = u64;

type EnterFn = fn() -> Token;
type ExitFn = fn(Token);

static HOOKS: OnceLock<(EnterFn, ExitFn)> = OnceLock::new();

/// Installs the enter/exit hooks called around text shaping and line breaking. The first call
/// wins; later calls change nothing.
#[doc(hidden)]
pub fn set_text_phase_hooks(enter: fn() -> Token, exit: fn(Token)) {
    HOOKS.get_or_init(|| (enter, exit));
}

pub(crate) struct TextScope(Option<(ExitFn, Token)>);

impl TextScope {
    #[inline(always)]
    pub(crate) fn enter() -> Self {
        let Some(&(enter, exit)) = HOOKS.get() else { return Self(None) };
        let token = enter();
        Self((token != 0).then_some((exit, token)))
    }
}

impl Drop for TextScope {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some((exit, token)) = self.0 {
            exit(token);
        }
    }
}
