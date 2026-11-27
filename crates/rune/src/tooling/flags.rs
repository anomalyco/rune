//! Static metadata for Rune's tooling command-line flags, shared with `Args::parse_split`.

use super::args::dev_tooling_enabled;

/// How a Rune CLI flag in [`FlagSpec`] accepts a value.
///
/// ```
/// use rune::tooling::{FlagValue, flags};
///
/// let switches = flags().iter().filter(|f| matches!(f.value, FlagValue::None)).count();
/// let with_value = flags().iter().filter(|f| !matches!(f.value, FlagValue::None)).count();
/// assert_eq!(switches + with_value, flags().len());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FlagValue {
    /// Boolean switch; takes no following argument.
    None,
    /// Requires a following argument (for example `"WxH"` or `"OUT.png"`).
    Required(&'static str),
    /// Takes an optional following argument (for example `"NAME"` or `"CORNER"`).
    Optional(&'static str),
}

/// Metadata describing one command-line flag recognised by Rune's tooling parser.
///
/// Returned by [`flags`] and [`Args::flags`](super::Args::flags) so an app that owns its
/// command line (with `clap`, `lexopt`, or a custom parser) and forwards flags through
/// [`App::args`](crate::App::args) can render help text and match Rune's flags without
/// maintaining a separate list.
///
/// ```
/// use rune::tooling::{FlagSpec, FlagValue, flags};
///
/// fn format_flag(flag: &FlagSpec) -> String {
///     let usage = match flag.value {
///         FlagValue::None => flag.name.to_owned(),
///         FlagValue::Required(v) => format!("{} {v}", flag.name),
///         FlagValue::Optional(v) => format!("{} [{v}]", flag.name),
///         _ => flag.name.to_owned(),
///     };
///     format!("{usage} — {}", flag.description)
/// }
///
/// for flag in flags() {
///     assert!(!format_flag(flag).is_empty());
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FlagSpec {
    /// Long flag name including its leading `"--"` (for example `"--snap"`).
    pub name: &'static str,
    /// Optional short alias including its leading `"-"` (for example `Some("-h")`).
    pub short: Option<&'static str>,
    /// Whether the flag takes no value, a required value, or an optional value, with its placeholder.
    pub value: FlagValue,
    /// Short one-line description suitable for CLI help output.
    pub description: &'static str,
    /// Cargo feature required for this flag (`Some("hot")` for live-editing flags, `None` for
    /// standard tooling flags). Flags whose feature is disabled in this build are omitted from
    /// [`flags`].
    pub feature: Option<&'static str>,
    /// Whether the flag may be passed more than once (for example `--crop`).
    pub repeatable: bool,
}

macro_rules! define_flags {
    ($(
        $(#[$meta:meta])*
        $id:ident => {
            name: $name:expr,
            $(short: $short:expr,)?
            value: $value:expr,
            $(feature: $feature:expr,)?
            $(repeatable: $repeatable:expr,)?
            description: $description:expr $(,)?
        }
    ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum FlagId {
            $( $(#[$meta])* $id, )*
        }

        const FLAG_NAMES: &[(FlagId, &'static str, Option<&'static str>)] = &[
            $(
                $(#[$meta])*
                (FlagId::$id, $name, define_flags!(@opt $($short)?)),
            )*
        ];

        const FLAGS: &[FlagSpec] = &[
            $(
                $(#[$meta])*
                FlagSpec {
                    name: $name,
                    short: define_flags!(@opt $($short)?),
                    value: $value,
                    description: $description,
                    feature: define_flags!(@opt $($feature)?),
                    repeatable: define_flags!(@bool $($repeatable)?),
                },
            )*
        ];
    };
    (@opt) => { None };
    (@opt $v:expr) => { Some($v) };
    (@bool) => { false };
    (@bool $v:expr) => { $v };
}

define_flags! {
    #[cfg(feature = "hot")]
    Hot => {
        name: "--hot",
        value: FlagValue::None,
        feature: "hot",
        description: "enable live hot-patching (or set RUNE_HOT=1)",
    },
    #[cfg(feature = "hot")]
    SnapOnPatch => {
        name: "--snap-on-patch",
        value: FlagValue::Required("OUT.png"),
        feature: "hot",
        description: "keep an offscreen renderer alive and write a snapshot on each hot patch",
    },
    #[cfg(feature = "hot")]
    HotControl => {
        name: "--hot-control",
        value: FlagValue::Required("FILE.json"),
        feature: "hot",
        description: "watch a JSON control file for live-edit requests with --snap-on-patch",
    },
    Snap => {
        name: "--snap",
        value: FlagValue::Required("OUT.png"),
        description: "render offscreen (no window) to a PNG at --at seconds",
    },
    Scene => {
        name: "--scene",
        value: FlagValue::Required("NAME"),
        description: "start from a named scene (--list-scenes)",
    },
    ListScenes => {
        name: "--list-scenes",
        value: FlagValue::None,
        description: "print the app's scene names and exit",
    },
    Playground => {
        name: "--playground",
        value: FlagValue::Optional("NAME"),
        description: "open App::playground at launch, optionally at a named story",
    },
    Variant => {
        name: "--variant",
        value: FlagValue::Required("N|NAME"),
        description: "open --playground's story on a variant (1 is the first)",
    },
    WholeWindow => {
        name: "--whole-window",
        value: FlagValue::None,
        description: "capture the whole window instead of only the stage with --playground and --snap",
    },
    Settle => {
        name: "--settle",
        value: FlagValue::None,
        description: "settle layout callbacks (and App::settle_ready) before --snap/--tree capture",
    },
    At => {
        name: "--at",
        value: FlagValue::Required("SECONDS"),
        description: "virtual capture time in seconds or mark:NAME[+-S]",
    },
    Size => {
        name: "--size",
        value: FlagValue::Required("WxH"),
        description: "viewport size in points",
    },
    Scale => {
        name: "--scale",
        value: FlagValue::Required("N"),
        description: "device scale factor (default 2)",
    },
    Theme => {
        name: "--theme",
        value: FlagValue::Required("dark|light"),
        description: "system appearance",
    },
    ScrollBars => {
        name: "--scroll-bars",
        value: FlagValue::Required("MODE"),
        description: "scroll bar mode: when-scrolling, always, or automatic",
    },
    Script => {
        name: "--script",
        value: FlagValue::Required("FILE.json"),
        description: "replay timestamped input events headlessly from a JSON script",
    },
    Frames => {
        name: "--frames",
        value: FlagValue::Required("N"),
        description: "number of frames to capture or trace",
    },
    Start => {
        name: "--start",
        value: FlagValue::Required("SECONDS"),
        description: "start time for frame dumps in seconds or mark:NAME[+-S]",
    },
    Fps => {
        name: "--fps",
        value: FlagValue::Required("N"),
        description: "frame rate for dumped frames and video (default 60)",
    },
    Out => {
        name: "--out",
        value: FlagValue::Required("DIR"),
        description: "directory for dumped PNG frames",
    },
    Video => {
        name: "--video",
        value: FlagValue::Required("OUT.mp4"),
        description: "stream frames into ffmpeg (.mp4, .mov, .webm)",
    },
    Duration => {
        name: "--duration",
        value: FlagValue::Required("SECONDS|auto"),
        description: "recording length in seconds, or auto (to end mark or 1 s after last event)",
    },
    Keys => {
        name: "--keys",
        value: FlagValue::Optional("CORNER"),
        description: "draw keycaps for script keys over frames (default bottom-left)",
    },
    KeysTheme => {
        name: "--keys-theme",
        value: FlagValue::Required("auto|light|dark"),
        description: "keycap overlay colour theme (default auto)",
    },
    Pointer => {
        name: "--pointer",
        value: FlagValue::None,
        description: "draw a ripple where the script presses",
    },
    Desktop => {
        name: "--desktop",
        value: FlagValue::Required("BACKDROP.png"),
        description: "composite every visible window over a backdrop image",
    },
    Strip => {
        name: "--strip",
        value: FlagValue::Required("OUT.png"),
        description: "tile dumped frames into one contact-sheet PNG",
    },
    Crop => {
        name: "--crop",
        value: FlagValue::Required("x,y,w,h|KEY"),
        repeatable: true,
        description: "crop --snap to a key/path or stack x,y,w,h regions in --strip (repeatable)",
    },
    Every => {
        name: "--every",
        value: FlagValue::Required("K"),
        description: "sample every K-th frame for --out and --strip (default 1)",
    },
    Cols => {
        name: "--cols",
        value: FlagValue::Required("N"),
        description: "columns per row in --strip (default 10)",
    },
    Tree => {
        name: "--tree",
        value: FlagValue::Required("OUT.json"),
        description: "write the element tree with animated values as JSON",
    },
    Html => {
        name: "--html",
        value: FlagValue::Required("OUT.html"),
        description: "export the scene as static HTML and CSS",
    },
    HtmlWidths => {
        name: "--html-widths",
        value: FlagValue::Required("W,..."),
        description: "comma-separated viewport widths in points for --html @media rules",
    },
    HtmlTemplate => {
        name: "--html-template",
        value: FlagValue::Required("FILE"),
        description: "HTML template filled at <!-- rune:css --> and <!-- rune:body -->",
    },
    Trace => {
        name: "--trace",
        value: FlagValue::Required("OUT.jsonl"),
        description: "write per-frame rects and opacity in motion-lint JSONL",
    },
    TraceAll => {
        name: "--trace-all",
        value: FlagValue::None,
        description: "include unkeyed elements by path in --trace",
    },
    Native => {
        name: "--native",
        value: FlagValue::None,
        description: "trace native window submissions instead of a virtual headless clock",
    },
    Realtime => {
        name: "--realtime",
        value: FlagValue::None,
        description: "pace headless frames to the wall clock, print marks, and run on_ready",
    },
    WindowScript => {
        name: "--window-script",
        value: FlagValue::Required("FILE.json"),
        description: "replay a JSON input script on the wall clock in a real window",
    },
    FirstPixels => {
        name: "--first-pixels",
        value: FlagValue::None,
        description: "report when the first GPU-complete offscreen pixels are readable",
    },
    Window => {
        name: "--window",
        value: FlagValue::Required("KEY"),
        description: "select a window key for headless tools or --window-script",
    },
    ListWindows => {
        name: "--list-windows",
        value: FlagValue::None,
        description: "list window keys using a deterministic virtual display",
    },
    Chrome => {
        name: "--chrome",
        value: FlagValue::None,
        description: "draw macOS traffic-light positions and report overlapping elements",
    },
    CpuBoot => {
        name: "--cpu-boot",
        value: FlagValue::Required("DIR"),
        description: "also paint the --snap frame on the CPU into DIR",
    },
    CpuBootThreads => {
        name: "--cpu-boot-threads",
        value: FlagValue::Required("N"),
        description: "worker thread count for --cpu-boot (default 6)",
    },
    Help => {
        name: "--help",
        short: "-h",
        value: FlagValue::None,
        description: "print Rune tooling flag usage and exit",
    },
}

pub(crate) fn lookup_flag(arg: &str) -> Option<FlagId> {
    FLAG_NAMES
        .iter()
        .find(|(_, name, short)| *name == arg || *short == Some(arg))
        .map(|(id, _, _)| *id)
}

#[cfg(test)]
pub(crate) const fn flag_names() -> &'static [(FlagId, &'static str, Option<&'static str>)] {
    FLAG_NAMES
}

pub(crate) const fn flags_for(dev_tooling: bool) -> &'static [FlagSpec] {
    if dev_tooling { FLAGS } else { &[] }
}

/// Static metadata for every command-line flag Rune's tooling parser accepts in this build.
///
/// An app that parses its own command line and forwards Rune flags through
/// [`App::args`](crate::App::args) can iterate this table to render help text or register
/// forwarded options without duplicating Rune's flag list. The parser reads the same table,
/// so the metadata and accepted flags stay synchronized.
///
/// Feature-gated flags (such as `--hot`, `--snap-on-patch`, and `--hot-control` behind the
/// `hot` Cargo feature) appear only when that feature is enabled. In a release build without
/// Rune's `tooling` or `hot` feature, Rune parses no process flags and this returns an empty
/// slice (`&[]`).
///
/// ```
/// use rune::tooling::{FlagValue, flags};
///
/// let mut lines = Vec::new();
/// for flag in flags() {
///     let line = match flag.value {
///         FlagValue::None => format!("  {}  {}", flag.name, flag.description),
///         FlagValue::Required(v) => format!("  {} {v}  {}", flag.name, flag.description),
///         FlagValue::Optional(v) => format!("  {} [{v}]  {}", flag.name, flag.description),
///         _ => format!("  {}  {}", flag.name, flag.description),
///     };
///     lines.push(line);
/// }
/// # assert_eq!(lines.len(), flags().len());
/// ```
pub const fn flags() -> &'static [FlagSpec] {
    flags_for(dev_tooling_enabled())
}
