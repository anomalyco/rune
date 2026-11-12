//! Luau scripts that build Rune UI.
//!
//! A [`Script`] is one sandboxed Luau VM. Its `init.luau` returns a module whose `render(view)`
//! builds nodes with `@rune/ui`:
//!
//! ```luau
//! --!strict
//! local ui = require("@rune/ui")
//! local count = 0
//! return {
//!     render = function(view)
//!         return ui.col({ pad = 16, gap = 8 }, {
//!             ui.text(`Pressed {count} times`),
//!             ui.row({ key = "inc", px = 10, py = 4, radius = 6, fill = "control-off",
//!                 onPress = function() count += 1 end }, { "Press" }),
//!         })
//!     end,
//! }
//! ```
//!
//! - **Nodes** are built straight into `rune::El`s by C functions, with no Lua tables or tree
//!   diff in between. `view:Memo(key, version, build)` reuses a subtree without calling `build`;
//!   `ui.virtualList` is `El::virtual_rows`, calling `renderRow(i)` only for rows entering the
//!   window.
//! - **`task`** (`spawn`, `defer`, `delay`, `wait`, `cancel`) runs on the frame clock the app's
//!   `poll` receives; an idle script never enters its VM. A host's **`Signal`** has `Connect`,
//!   `Once` and `Wait`; a `ScriptSignal` adds `Fire` and `Version`. Connections run as tasks.
//! - **Errors never panic**: a failed render shows an error card, a failed task is logged, and a
//!   watchdog stops any entry that runs past 250 ms.
//! - **Hot reload**: [`Script::reload`] (or `Script::watch` with the `watch` feature) swaps the
//!   VM, carrying `saveHotState()` into `loadHotState(state)`.
//!
//! Luau runs with native code generation on Windows and Linux (x86-64, arm64), and as its
//! interpreter on macOS, iOS and the web. On `wasm32-unknown-unknown`, Luau's C++ builds with
//! WASI SDK: `scripts/web-build.sh` sets that up when `WASI_SDK_PATH` is set.

mod card;
mod data;
mod error;
mod functions;
mod options;
mod require;
mod script;
mod sources;
mod task;
mod ui;
mod vm;
mod watchdog;
#[cfg(all(feature = "watch", not(target_arch = "wasm32")))]
mod watch;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod web;
mod workers;

pub use data::Data;
pub use error::ErrorKind;
pub use error::ScriptError;
pub use mlua;
pub use options::ScriptOptions;
pub use options::Shadows;
pub use script::LogLevel;
pub use script::LogLine;
pub use script::Metrics;
pub use script::Reload;
pub use script::Script;
pub use sources::Compiled;
pub use sources::ENTRY;
pub use sources::Sources;
pub use sources::precompiled;
pub use ui::Palette;
pub use ui::default_palette;
pub use ui::host::Handler;
pub use ui::host::HostNode;
pub use ui::host::NodeArgs;
pub use ui::host::Press;
pub use ui::host::Prop;
pub use ui::props::prop_names;
pub use vm::Host;
pub use workers::Completer;
pub use workers::spawn_blocking;

/// Whether this build compiles `--!native` modules to native code (the `native-code` feature, on
/// Windows and Linux) rather than interpreting them.
pub const NATIVE_CODE: bool = cfg!(luau_jit);
