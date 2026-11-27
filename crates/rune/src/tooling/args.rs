//! The tooling arguments: `Args` and its parser.

use super::*;

#[derive(Clone, Default, Debug)]
pub struct Args {
    #[cfg(feature = "hot")]
    pub(crate) snap_on_patch: Option<PathBuf>,
    #[cfg(feature = "hot")]
    pub(crate) hot_control: Option<PathBuf>,
    pub snap: Option<PathBuf>,
    pub scene: Option<String>,
    pub at: Option<f64>,
    pub size: Option<(f32, f32)>,
    pub scale: Option<f32>,
    pub theme: Option<String>,
    pub scroll_bars: Option<crate::ScrollBars>,
    pub script: Option<PathBuf>,
    pub frames: Option<usize>,
    pub start: f64,
    pub fps: f64,
    pub out: Option<PathBuf>,
    pub tree: Option<PathBuf>,
    pub trace: Option<PathBuf>,
    /// Trace the native window's submission clock instead of a virtual headless clock.
    pub native: bool,
    /// Trace every element, keyed or not (paths name the unkeyed ones).
    pub trace_all: bool,
    pub strip: Option<PathBuf>,
    pub crops: Vec<[f32; 4]>,
    pub every: usize,
    pub cols: usize,
    pub list_scenes: bool,
    pub first_pixels: bool,
    pub window: Option<String>,
    pub list_windows: bool,
    /// Draw where macOS will put the traffic lights over headless frames, and report elements
    /// that overlap them.
    pub chrome_overlay: bool,
    /// Stream the dumped frames into ffmpeg, which writes this video.
    pub(crate) video: Option<PathBuf>,
    /// How long to dump frames for, when `frames` isn't given.
    pub(crate) duration: Option<Length>,
    /// `--at mark:NAME±S` and `--start mark:NAME±S`: a script mark's time plus an offset.
    pub(crate) at_mark: Option<String>,
    pub(crate) start_mark: Option<String>,
    /// `--keys [CORNER]`: keycaps for script keys and chords over dumped frames.
    pub(crate) keys: Option<crate::input::keycaps::Corner>,
    /// `--keys-theme light|dark`; None (`auto`) follows the app's theme.
    pub(crate) keys_theme: Option<bool>,
    /// `--pointer`: a ripple where the script presses.
    pub(crate) pointer: bool,
    /// `--desktop BACKDROP`: every visible window composited at its frame over this image.
    pub(crate) desktop: Option<PathBuf>,
    /// `--playground [NAME]`: open the playground at launch, at the story named.
    pub(crate) playground: Option<Option<String>>,
    /// `--variant N|NAME`: the variant `--playground` opens its story on.
    pub(crate) variant: Option<String>,
    /// `--crop KEY`: crop `--snap` to the element with this key or path; with `--playground`,
    /// on the stage unless `whole_window`.
    pub(crate) crop_key: Option<String>,
    /// `--whole-window`: `--snap` with `--playground` takes the whole window, not just the stage.
    pub(crate) whole_window: bool,
    /// `--settle`: run up to 3 zero-dt passes at the `--snap`/`--tree` capture frame so layout
    /// callbacks (`on_layout`, `on_layout_tx`, `on_screen_layout`, `on_drawn_layout`,
    /// `drawn_rect_into`, `on_visible`) deliver and re-commit before capture.
    pub(crate) settle: bool,
    /// `--html OUT.html`: write the scene as a static page (`tooling::html`).
    pub(crate) html: Option<PathBuf>,
    /// `--html-widths 360,640,1000`: the widths `--html` lays the page out at, each the
    /// narrowest its `@media` rules apply to.
    pub(crate) html_widths: Vec<f32>,
    /// `--html-template FILE`: the page `--html` fills at `<!-- rune:css -->` and `<!-- rune:body -->`.
    pub(crate) html_template: Option<PathBuf>,
    /// `--realtime`: a headless run waits for the wall clock to reach each frame's time and
    /// prints `rune-mark NAME T` as the script passes each mark, so a live server's replies and a
    /// harness's actions keep pace with its script. It stands in for a window, so `App::on_ready`
    /// runs after its first frame.
    pub(crate) realtime: bool,
    /// `--window-script FILE`: a real window replays the script's input on the wall clock from
    /// its first drawn frame, through the same `script_input` a headless run uses, and drops the
    /// system's pointer events while it does (`window::host::window_script`). Tooling, like
    /// `--script`: only a command-line argument turns it on.
    pub(crate) window_script: Option<PathBuf>,
    /// `--cpu-boot DIR`: also paint the `--snap` frame on the CPU (`render::cpu_boot`), on
    /// `--cpu-boot-threads N` threads (6 by default).
    pub(crate) cpu_boot: Option<PathBuf>,
    pub(crate) cpu_boot_threads: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Length {
    Seconds(f64),
    /// To the script's `end` mark, or one second after its last event.
    Auto,
}

/// Retain the whole expression until the script is loaded: a mark's name can end in `-1`.
pub(crate) fn mark_time(v: &str) -> Option<String> {
    v.strip_prefix("mark:").map(str::to_owned)
}

/// Whether this build reads Rune's developer flags from the process command line and starts
/// interaction timing from an ambient `OTEL_EXPORTER_OTLP_ENDPOINT`. Always on in debug builds,
/// and in release builds compiled with Rune's `tooling` (or `hot`) feature.
#[inline]
pub(crate) const fn dev_tooling_enabled() -> bool {
    cfg!(all(not(target_arch = "wasm32"), any(feature = "tooling", feature = "hot", debug_assertions)))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn set_extra_args(argv: Vec<String>) {
    EXTRA.set(argv).ok();
}

#[cfg(not(target_arch = "wasm32"))]
fn process_argv() -> Vec<String> {
    decode_argv(std::env::args_os().skip(1))
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn decode_argv(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

impl Args {
    /// Static metadata for every command-line flag Rune's tooling parser accepts in this build.
    ///
    /// Alias of [`crate::tooling::flags`].
    ///
    /// ```
    /// use rune::tooling::{Args, flags};
    ///
    /// assert_eq!(Args::flags(), flags());
    /// ```
    pub const fn flags() -> &'static [super::flags::FlagSpec] {
        super::flags::flags()
    }

    /// Default tooling arguments, with no flags active.
    pub(crate) fn defaults() -> Args {
        Args { fps: 60.0, every: 1, cols: 10, cpu_boot_threads: 6, ..Default::default() }
    }

    pub fn parse() -> Args {
        #[cfg(target_arch = "wasm32")]
        {
            EXTRA.set(Vec::new()).ok();
            Args::defaults()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let argv = process_argv();
            let (args, extra) = Args::parse_process_argv(argv, dev_tooling_enabled());
            EXTRA.set(extra).ok();
            args
        }
    }

    /// Parses `argv` (without the program name); unrecognised arguments become `extra_args`.
    pub fn parse_from(argv: Vec<String>) -> Args {
        let (args, extra) = Args::parse_split(argv);
        EXTRA.set(extra).ok();
        args
    }

    /// The process's arguments, leaving `extra_args` unset so an app's own `App::args` can still
    /// set it.
    fn parse_process() -> Args {
        #[cfg(target_arch = "wasm32")]
        { Args::defaults() }
        #[cfg(not(target_arch = "wasm32"))]
        { Args::parse_process_argv(process_argv(), dev_tooling_enabled()).0 }
    }

    /// Splits a process command line into Rune's flags and app arguments. When `dev_tooling` is
    /// false (a release build without Rune's `tooling` or `hot` feature), Rune parses no process
    /// flags and leaves every argument untouched for the app (`extra_args`).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn parse_process_argv(argv: Vec<String>, dev_tooling: bool) -> (Args, Vec<String>) {
        if dev_tooling {
            Args::parse_split(argv)
        } else {
            (Args::defaults(), argv)
        }
    }

    fn parse_split(argv: Vec<String>) -> (Args, Vec<String>) {
        #[cfg(feature = "hot")]
        crate::hot::enable_from_env();
        let mut a = Args::defaults();
        let mut extra = Vec::new();
        let mut i = 0;
        let val = |i: &mut usize| -> String {
            *i += 1;
            argv.get(*i).cloned().unwrap_or_else(|| die(&format!("{} needs a value", argv[*i - 1])))
        };
        use super::flags::{FlagId, lookup_flag};
        while i < argv.len() {
            match lookup_flag(argv[i].as_str()) {
                #[cfg(feature = "hot")]
                Some(FlagId::Hot) => crate::hot::enable(),
                #[cfg(feature = "hot")]
                Some(FlagId::SnapOnPatch) => a.snap_on_patch = Some(val(&mut i).into()),
                #[cfg(feature = "hot")]
                Some(FlagId::HotControl) => a.hot_control = Some(val(&mut i).into()),
                Some(FlagId::Snap) => a.snap = Some(val(&mut i).into()),
                Some(FlagId::Scene) => a.scene = Some(val(&mut i)),
                Some(FlagId::Playground) => a.playground = Some(crate::playground::arg(&argv, &mut i)),
                Some(FlagId::Variant) => a.variant = Some(val(&mut i)),
                Some(FlagId::WholeWindow) => a.whole_window = true,
                Some(FlagId::Settle) => a.settle = true,
                Some(FlagId::At) => {
                    let v = val(&mut i);
                    a.at_mark = mark_time(&v);
                    a.at = a.at_mark.is_none().then(|| num(&v));
                }
                Some(FlagId::Size) => a.size = Some(parse_size(&val(&mut i)).unwrap_or_else(|e| die(&e))),
                Some(FlagId::Scale) => a.scale = Some(parse_scale(&val(&mut i)).unwrap_or_else(|e| die(&e))),
                Some(FlagId::Theme) => a.theme = Some(val(&mut i)),
                Some(FlagId::ScrollBars) => a.scroll_bars = Some(parse_scroll_bars(&val(&mut i)).unwrap_or_else(|e| die(&e))),
                Some(FlagId::Script) => a.script = Some(val(&mut i).into()),
                Some(FlagId::Frames) => a.frames = Some(num(&val(&mut i)) as usize),
                Some(FlagId::Start) => {
                    let v = val(&mut i);
                    a.start_mark = mark_time(&v);
                    if a.start_mark.is_none() { a.start = num(&v); }
                }
                Some(FlagId::Fps) => a.fps = num(&val(&mut i)),
                Some(FlagId::Out) => a.out = Some(val(&mut i).into()),
                Some(FlagId::Video) => a.video = Some(val(&mut i).into()),
                Some(FlagId::Duration) => {
                    let v = val(&mut i);
                    a.duration = Some(if v == "auto" { Length::Auto } else { Length::Seconds(num(&v)) });
                }
                Some(FlagId::Keys) => {
                    let corner = argv.get(i + 1).and_then(|v| crate::input::keycaps::Corner::parse(v));
                    if corner.is_some() { i += 1; }
                    a.keys = Some(corner.unwrap_or_default());
                }
                Some(FlagId::KeysTheme) => {
                    let v = val(&mut i);
                    a.keys_theme = crate::input::keycaps::parse_theme(&v).unwrap_or_else(|| die("--keys-theme is auto, light or dark"));
                }
                Some(FlagId::Pointer) => a.pointer = true,
                Some(FlagId::Desktop) => a.desktop = Some(val(&mut i).into()),
                Some(FlagId::Tree) => a.tree = Some(val(&mut i).into()),
                Some(FlagId::Html) => a.html = Some(val(&mut i).into()),
                Some(FlagId::HtmlTemplate) => a.html_template = Some(val(&mut i).into()),
                Some(FlagId::HtmlWidths) => {
                    let v = val(&mut i);
                    a.html_widths = v.split(',').map(|w| positive(w).unwrap_or_else(|| die(&format!("--html-widths is a list of widths above 0: {v}")))).collect();
                }
                Some(FlagId::Trace) => a.trace = Some(val(&mut i).into()),
                Some(FlagId::Native) => a.native = true,
                Some(FlagId::Realtime) => a.realtime = true,
                Some(FlagId::WindowScript) => a.window_script = Some(val(&mut i).into()),
                Some(FlagId::TraceAll) => a.trace_all = true,
                Some(FlagId::Strip) => a.strip = Some(val(&mut i).into()),
                Some(FlagId::Crop) => {
                    let v = val(&mut i);
                    match v.split(',').map(|s| s.trim().parse::<f32>()).collect::<Result<Vec<_>, _>>() {
                        Ok(r) if r.len() == 4 => a.crops.push([r[0], r[1], r[2], r[3]]),
                        _ if v.contains(',') => die("--crop is x,y,w,h in points, or an element's key or path"),
                        _ => a.crop_key = Some(v),
                    }
                }
                Some(FlagId::Cols) => a.cols = (num(&val(&mut i)) as usize).max(1),
                Some(FlagId::Every) => a.every = (num(&val(&mut i)) as usize).max(1),
                Some(FlagId::ListScenes) => a.list_scenes = true,
                Some(FlagId::FirstPixels) => a.first_pixels = true,
                Some(FlagId::Window) => a.window = Some(val(&mut i)),
                Some(FlagId::ListWindows) => a.list_windows = true,
                Some(FlagId::Chrome) => a.chrome_overlay = true,
                Some(FlagId::CpuBoot) => a.cpu_boot = Some(val(&mut i).into()),
                Some(FlagId::CpuBootThreads) => a.cpu_boot_threads = (num(&val(&mut i)) as usize).max(1),
                Some(FlagId::Help) => {
                    eprintln!("{HELP}");
                    std::process::exit(0);
                }
                None => {
                    #[cfg(not(feature = "hot"))]
                    if matches!(argv[i].as_str(), "--hot" | "--snap-on-patch" | "--hot-control") {
                        die("live editing requires the rune/hot feature; use scripts/rune-dev");
                    }
                    extra.push(argv[i].clone());
                }
            }
            i += 1;
        }
        a.validate().unwrap_or_else(|e| die(e));
        // Parsed here, before any window opens: an unreadable or malformed file ends the run now.
        if a.window_script.is_some() { super::read_window_script(&a); }
        (a, extra)
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.variant.is_some() && self.playground.is_none() { return Err("--variant needs --playground"); }
        if self.crop_key.is_some() && self.snap.is_none() { return Err("--crop KEY needs --snap"); }
        if self.whole_window && (self.playground.is_none() || self.snap.is_none()) { return Err("--whole-window needs --playground and --snap"); }
        if self.settle && self.snap.is_none() && self.tree.is_none() { return Err("--settle needs --snap or --tree"); }
        if self.cpu_boot.is_some() && self.snap.is_none() { return Err("--cpu-boot needs --snap"); }
        if self.window_script.is_some() && self.headless() { return Err("--window-script replays in a real window: it does not combine with a headless run (--script, --snap, --tree, ...)"); }
        Ok(())
    }

    pub fn headless(&self) -> bool {
        #[cfg(feature = "hot")]
        if self.snap_on_patch.is_some() { return true; }
        self.snap.is_some()
            || self.tree.is_some()
            || self.html.is_some()
            || (!self.native
                && (self.script.is_some() || self.trace.is_some() || self.frames.is_some() || self.duration.is_some() || self.out.is_some()))
            || self.video.is_some()
            || self.strip.is_some()
            || self.list_scenes
            || self.first_pixels
            || self.list_windows
            || self.desktop.is_some()
    }
}

/// The flags an app started again never carries over, each with its value (the next argument, or
/// after `=`): they drive input or act for the person, so a relaunch (`Tx::relaunch`, an updater's
/// restart) takes the real pointer and keys instead of repeating them. Flags that isolate a run
/// (`--harness-writes`, `--dirs-root`) or trace it stay. A program that restarts the app itself
/// strips them with [`without_relaunch_drops`] rather than keeping its own list.
pub const RELAUNCH_DROPS: &[&str] = &["--window-script"];

/// `args` without each [`RELAUNCH_DROPS`] flag and its value, given as `--flag VALUE` or
/// `--flag=VALUE`, wherever it stands (a bare `--` ends nothing, as Rune's parser reads it). Every
/// other argument stays, byte for byte and in order.
pub fn without_relaunch_drops(args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if RELAUNCH_DROPS.iter().any(|f| a == *f) { it.next(); continue; }
        if a.to_str().is_some_and(|a| RELAUNCH_DROPS.iter().any(|f| a.strip_prefix(f).is_some_and(|rest| rest.starts_with('=')))) { continue; }
        out.push(a);
    }
    out
}

/// Whether this process runs headless: its arguments ask for an offscreen render, a scripted
/// run, a trace or a listing rather than a window, as `Args::headless` decides. Parsed from the
/// process's arguments once, so it answers before `run` (to skip a tray, global shortcuts or
/// network work in a scripted run, say), with `--help` and a malformed flag handled as `run`
/// handles them. In a release build without Rune's `tooling` feature, process arguments are not
/// parsed and this returns `false`. An app that passes `App::args` asks
/// `Args::parse_from(..).headless()` instead.
pub fn is_headless() -> bool {
    static HEADLESS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HEADLESS.get_or_init(|| Args::parse_process().headless())
}

pub(super) const HELP: &str = "usage: <app> [window]
  --snap out.png        render offscreen (no window) at --at seconds
  --scene NAME          start from a named scene (--list-scenes)
  --playground [NAME]   open App::playground at launch, at a story; headless too
  --variant N|NAME      open --playground's story on a variant (1 is the first)
                        (--snap with --playground takes only the story's stage by default;
                        --out, --video and --strip frames are the whole window)
  --whole-window        --snap with --playground takes the whole window: list, stage and side
  --crop KEY            crop --snap to the element with that key or path (with --playground,
                        one on the stage, or anywhere in the window with --whole-window)
  --at SECONDS          virtual time of the snapshot/tree (default: end of frames or script);
                        mark:NAME or mark:NAME+0.5 for a script mark's time (also --start)
  --settle              settle layout callbacks (and App::settle_ready) before --snap/--tree capture
  --size WxH            viewport in points      --scale N   device scale (default 2)
  --theme dark|light    system appearance
  --scroll-bars MODE    scroll bars: when-scrolling (headless default), always, automatic
  --script FILE.json    timestamped input, replayed headless (move/down/up/tap/drag, keys,
                        {\"focus\":false}); a key with \"repeat\":true is a held key's auto-repeat;
                        {\"type\":\"mark\",\"name\":N} marks a time;
                        {\"type\":\"shortcut\",\"chord\":\"cmd+space\"} presses a chord through its
                        registered global shortcut handler, never on_key;
                        an object adds \"displays\" and \"pointer\" for place() (rune-placed, --tree)
                        menu-pick with an \"id\" and menu-cancel answer a pop-up menu;
                        file-dialog with a \"path\" and file-dialog-cancel a file dialog
  --frames N --fps 60 --start S --out DIR     dump N frames from S seconds
  --video out.mp4       stream frames into ffmpeg (.mp4 H.264, .mov ProRes 4444, .webm VP9);
                        --frames N or --duration SECONDS|auto long (default auto: to the
                        script's \"end\" mark, or 1 s after its last event)
  --keys [CORNER]       draw keycaps for script keys and chords on frames (bottom-left default;
                        bottom, bottom-right, top-left, top, top-right); never in --tree
  --keys-theme auto|light|dark   cap colours (default auto: match the app's background)
  --pointer             draw a ripple where the script presses
  --desktop BACKDROP.png  draw every visible window at its placed frame, with corners and
                        shadows, over the backdrop (scaled to fill). The output is --size
                        points, else the script's primary display, else the backdrop's pixels
                        at --scale; script x/y are output points, keys go to the key window;
                        --window KEY picks the window --scene and --tree describe
  --strip out.png --crop x,y,w,h --every K    tile dumped frames into one image
                        (repeat --crop to stack several regions per frame; --cols N per row)
  --tree out.json       element tree with animated values
  --html out.html       the page as static HTML and CSS, at rest, for a web app's first paint
                        (no GPU); --html-widths 360,640,1000 lays it out at each width and
                        folds them into @media rules; --html-template FILE fills
                        <!-- rune:css --> and <!-- rune:body --> in FILE
  --trace out.jsonl     per-frame rects/opacity in motion-lint JSONL, with the tracks each frame
                        sampled (tk) and why it was animating (quality.why)
  --trace-all           trace unkeyed elements too, by path
  --native             trace native submissions; --frames N stops after N frames
  --realtime            headless: hold each frame until the wall clock reaches its time, and
                        print \"rune-mark NAME T\" as the script passes each mark,
                        and run on_ready after the first frame, as a window does
  --window-script FILE.json   a real window replays the script's input on the wall clock from its
                        first drawn frame (the same events --script takes) and ignores the
                        system's pointer while it does: the --window KEY window, else the first;
                        prints \"rune-script-window KEY\", \"rune-mark NAME T ELAPSED\", then
                        \"rune-script-done T ELAPSED\", or \"rune-script-error T ELAPSED WHY\" and
                        stops replaying (also when no frame is drawn within 10 s); a relaunch
                        drops it. It cannot show: the system's pointer messages (WM_MOUSEMOVE,
                        WM_MOUSEWHEEL) and their DPI and delta scaling, the system cursor and the
                        system's own hover tracking, the custom caption buttons, overlay windows,
                        the low-level mouse hook and press_point, or an engine's native windows
                        (its dialogs, DevTools)
  --first-pixels        report when the first GPU-complete offscreen pixels are readable
  --window KEY         select a window for headless tools (run_windows, App::window)
  --list-windows       list window keys using a deterministic virtual display
  --chrome              draw where macOS puts the traffic lights; list elements overlapping them
  --cpu-boot DIR        also paint the --snap frame on the CPU, as a window's first frame before
                        its GPU: DIR gets draw.bin, cpu.png and gpu.png, and the run prints
                        rune-cpu-boot {json} (stage times, drift); --cpu-boot-threads N (6)
  other arguments are passed to the app (rune::tooling::extra_args)";

pub(crate) fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

pub(crate) fn num(s: &str) -> f64 {
    s.trim().parse().unwrap_or_else(|_| die(&format!("not a number: {s}")))
}

fn positive(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// `--size WxH`, in points.
pub(crate) fn parse_size(v: &str) -> Result<(f32, f32), String> {
    v.split_once('x').and_then(|(w, h)| Some((positive(w)?, positive(h)?)))
        .ok_or_else(|| format!("--size is WxH, both above 0: {v}"))
}

pub(crate) fn parse_scale(v: &str) -> Result<f32, String> {
    positive(v).ok_or_else(|| format!("--scale is a number above 0: {v}"))
}

pub(crate) fn parse_scroll_bars(v: &str) -> Result<crate::ScrollBars, String> {
    match v.trim() {
        "always" => Ok(crate::ScrollBars::Always),
        "when-scrolling" => Ok(crate::ScrollBars::WhenScrolling),
        "automatic" | "auto" => Ok(crate::ScrollBars::Automatic),
        _ => Err(format!("--scroll-bars is when-scrolling, always or automatic: {v}")),
    }
}
