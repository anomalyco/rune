//! Optional timing and stage hooks for text shaping, line breaking, glyph rasterization,
//! setup warming, and system-font fallback (`rune::FrameStage`).
//!
//! `rune` registers its hooks once; when unset, entering a scope is one atomic load.

use std::sync::OnceLock;

/// Opaque token returned by an enter hook and passed to its exit hook on drop.
/// Zero means the scope was not timed and the exit hook is skipped.
pub type Token = u64;

/// Granular text subsystem boundaries reported to `rune`'s observability hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextStage {
    /// Text shaping and line breaking (`Phase::Text` / `FrameStage::Text`).
    Shape,
    /// Glyph rasterization and atlas placement on an atlas cache miss (`FrameStage::GlyphAtlas`).
    GlyphAtlas,
    /// Text system initialization and family warming (`FrameStage::TextWarm`).
    Warm,
    /// On-demand system font fallback lookup (`FrameStage::SystemFontFallback`).
    SystemFallback,
}

type EnterFn = fn() -> Token;
type ExitFn = fn(Token);
type StageEnterFn = fn(TextStage) -> Token;
type StageExitFn = fn(TextStage, Token);

static HOOKS: OnceLock<(EnterFn, ExitFn)> = OnceLock::new();
static STAGE_HOOKS: OnceLock<(StageEnterFn, StageExitFn)> = OnceLock::new();

/// Installs the enter/exit hooks called around text shaping and line breaking. The first call
/// wins; later calls change nothing.
#[doc(hidden)]
pub fn set_text_phase_hooks(enter: fn() -> Token, exit: fn(Token)) {
    HOOKS.get_or_init(|| (enter, exit));
}

/// Installs the enter/exit hooks called around all [`TextStage`] boundaries (including shaping
/// and line breaking). The first call wins; later calls change nothing.
#[doc(hidden)]
pub fn set_text_stage_hooks(enter: fn(TextStage) -> Token, exit: fn(TextStage, Token)) {
    STAGE_HOOKS.get_or_init(|| (enter, exit));
}

enum ExitKind {
    Stage(StageExitFn, TextStage, Token),
    Legacy(ExitFn, Token),
}

pub(crate) struct TextScope(Option<ExitKind>);

impl TextScope {
    #[inline(always)]
    pub(crate) fn enter() -> Self {
        if let Some(&(enter, exit)) = STAGE_HOOKS.get() {
            let token = enter(TextStage::Shape);
            return Self((token != 0).then_some(ExitKind::Stage(exit, TextStage::Shape, token)));
        }
        let Some(&(enter, exit)) = HOOKS.get() else { return Self(None) };
        let token = enter();
        Self((token != 0).then_some(ExitKind::Legacy(exit, token)))
    }
}

impl Drop for TextScope {
    #[inline(always)]
    fn drop(&mut self) {
        match self.0 {
            Some(ExitKind::Stage(exit, stage, token)) => exit(stage, token),
            Some(ExitKind::Legacy(exit, token)) => exit(token),
            None => {}
        }
    }
}

pub(crate) struct TextStageScope(Option<(StageExitFn, TextStage, Token)>);

impl TextStageScope {
    #[inline(always)]
    pub(crate) fn enter(stage: TextStage) -> Self {
        let Some(&(enter, exit)) = STAGE_HOOKS.get() else { return Self(None) };
        let token = enter(stage);
        Self((token != 0).then_some((exit, stage, token)))
    }
}

impl Drop for TextStageScope {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some((exit, stage, token)) = self.0 {
            exit(stage, token);
        }
    }
}
