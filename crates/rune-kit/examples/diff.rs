//! A pull request's changes in `DiffView`: three files diffed from their texts and two read
//! from a patch (a rename and a binary file), unified or split, with a review comment under a
//! line and the unchanged lines between hunks folded.
//! `cargo run -p rune-kit --example diff` opens the window; `j`/`k` move between hunks, `]`/`[`
//! between files. The stills in `demo/kit/` are
//! `-- --snap diff-unified-dark.png --size 900x620 --theme dark` and the same with
//! `--scene split` and `--theme light`. `diff-unified-scrolled.png` is
//! `-- --scene scrolled --script crates/rune-kit/examples/diff_scrolled.json --snap diff-unified-scrolled.png --at mark:end --size 900x620 --theme dark`:
//! long lines scrolled sideways, with the line numbers held at the left edge.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::{App, AppSpec, Cx, El, KeyEvent, Tx, col, row, text};
use rune_kit::diff::{Annotation, DiffMode, DiffState, DiffView, FileDiff, LineRange, LineRef, Side, parse};
use rune_kit::{Button, ButtonVariant, Kit, SegmentedControl};

const OLD_RS: &str = "use std::time::Duration;

/// How long to wait before trying a request again.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0 }
    }

    /// The wait before the next attempt.
    pub fn next(&mut self) -> Duration {
        let wait = self.base * 2u32.pow(self.attempt);
        self.attempt += 1;
        wait.min(self.max)
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    pub fn base(&self) -> Duration {
        self.base
    }

    pub fn max(&self) -> Duration {
        self.max
    }

    pub fn is_fresh(&self) -> bool {
        self.attempt == 0
    }

    pub fn exhausted(&self, limit: u32) -> bool {
        self.attempt >= limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_until_the_cap() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        assert_eq!(b.next(), Duration::from_millis(100));
        assert_eq!(b.next(), Duration::from_millis(200));
    }
}
";

const NEW_RS: &str = "use std::time::Duration;

/// How long to wait before trying a request again, with jitter so clients spread out.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    jitter: f32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0, jitter: 0.2 }
    }

    /// The wait before the next attempt, give or take `jitter` of it.
    pub fn next(&mut self, roll: f32) -> Duration {
        let wait = self.base.saturating_mul(2u32.saturating_pow(self.attempt));
        self.attempt += 1;
        wait.min(self.max).mul_f32(1.0 + self.jitter * (roll * 2.0 - 1.0))
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    pub fn base(&self) -> Duration {
        self.base
    }

    pub fn max(&self) -> Duration {
        self.max
    }

    pub fn is_fresh(&self) -> bool {
        self.attempt == 0
    }

    pub fn exhausted(&self, limit: u32) -> bool {
        self.attempt >= limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_until_the_cap() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        b.jitter = 0.0;
        assert_eq!(b.next(0.5), Duration::from_millis(100));
        assert_eq!(b.next(0.5), Duration::from_millis(200));
    }
}
";

const OLD_TS: &str = "export async function fetchWithRetry(url: string, tries = 3): Promise<Response> {
\tfor (let i = 0; i < tries; i++) {
\t\tconst res = await fetch(url)
\t\tif (res.ok) return res
\t}
\tthrow new Error(`gave up on ${url}`)
}
";

const NEW_TS: &str =
    "export async function fetchWithRetry(url: string, tries = 3, signal?: AbortSignal): Promise<Response> {
\tlet last: Response | undefined
\tfor (let i = 0; i < tries; i++) {
\t\tlast = await fetch(url, { signal })
\t\tif (last.ok) return last
\t\tawait sleep(backoff(i))
\t}
\tthrow new Error(`gave up on ${url} after ${tries} tries (${last?.status})`)
}
";

const OLD_TOML: &str = "[retry]\nbase_ms = 100\nmax_ms = 1000\n";
const NEW_TOML: &str = "[retry]\nbase_ms = 100\nmax_ms = 1000\njitter = 0.2\n";

const PATCH: &str = "diff --git a/src/net/retry.rs b/src/net/backoff_policy.rs
similarity index 100%
rename from src/net/retry.rs
rename to src/net/backoff_policy.rs
diff --git a/assets/retry-curve.png b/assets/retry-curve.png
new file mode 100644
index 0000000..4b825dc
Binary files /dev/null and b/assets/retry-curve.png differ
";

const LONG_OLD: &str = "pub fn describe(&self) -> String {
    format!(\"retry {} times starting at {:?}, doubling up to {:?}\", self.limit, self.base, self.max)
}

pub fn jitter_for(&self, attempt: u32) -> Duration {
    self.base * 2u32.pow(attempt)
}

pub fn clamp(&self, wait: Duration) -> Duration {
    wait.min(self.max)
}
";
const LONG_NEW: &str = "pub fn describe(&self) -> String {
    format!(\"retry {} times starting at {:?}, doubling up to {:?}, with {:.0}% jitter so clients that failed together spread out when they retry\", self.limit, self.base, self.max, self.jitter * 100.0)
}

pub fn jitter_for(&self, attempt: u32) -> Duration {
    let wait = self.base * 2u32.pow(attempt);
    wait.mul_f64(1.0 + self.jitter * (self.rng.next_f64() - 0.5) * 2.0).min(self.max + self.max.mul_f64(self.jitter))
}

pub fn clamp(&self, wait: Duration) -> Duration {
    wait.min(self.max)
}
";

struct Review {
    diff: DiffState,
    composing: Option<LineRange>,
}

fn init() -> Review {
    let mut files = vec![
        FileDiff::from_texts("src/net/backoff.rs", OLD_RS, NEW_RS),
        FileDiff::from_texts("web/fetch.ts", OLD_TS, NEW_TS),
        FileDiff::from_texts("config/retry.toml", OLD_TOML, NEW_TOML),
    ];
    files.extend(parse(PATCH));
    let mut diff = DiffState::new(files);
    diff.set_annotations(vec![Annotation::new(0, LineRef::new(Side::New, 19), "jitter")]);
    Review { diff, composing: None }
}

fn view(s: &Review, cx: &mut Cx<Review>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let split = s.diff.mode() == DiffMode::Split;
    let modes = SegmentedControl::new(&["Unified", "Split"], Some(split as usize)).width(Some(160.0)).on_pick(
        |s: &mut Review, i, tx: &mut Tx| {
            tx.motion(rune::tok::motion::SNAPPY);
            s.diff.set_mode(if i == 1 { DiffMode::Split } else { DiffMode::Unified });
        },
    );
    let (adds, dels) = s.diff.files().iter().fold((0, 0), |(a, d), f| (a + f.additions(), d + f.deletions()));
    let files = match s.diff.files().len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let bar = row()
        .key("bar")
        .w_full()
        .h(48.0)
        .px(16.0)
        .gap(12.0)
        .items_center()
        .fill(k.c.layer_01)
        .child(text("Add jitter to retry backoff").font(k.ty.ui_medium).color(k.c.text_base))
        .child(text(format!("{files}  +{adds} −{dels}")).font(k.ty.ui).color(k.c.text_muted))
        .child(row().grow(1.0))
        .child(modes.build(&k));
    let kc = k.clone();
    let diff = DiffView::new(|s: &Review| &s.diff, |s: &mut Review| &mut s.diff)
        .annotation(move |s: &Review, a: &Annotation| comment(&kc, s, a))
        .on_comment(|s: &mut Review, lines, _| s.composing = Some(lines))
        .build(s, &k);
    col().w_full().h_full().fill(k.c.bg_base).child(bar).child(diff)
}

/// A review thread under a line.
fn comment(k: &Kit, _: &Review, a: &Annotation) -> El {
    let body = "Jitter keeps a crowd of clients that failed together from retrying together. \
                0.2 matches what the server's rate limiter assumes.";
    let card = col()
        .w_full()
        .padding(12.0)
        .gap(6.0)
        .radius(8.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(text("Reviewer").font(k.ty.ui_medium).color(k.c.text_base))
                .child(text(format!("on line {}", a.at.line)).font(k.ty.ui).color(k.c.text_faint)),
        )
        .child(text(body).font(k.ty.body).color(k.c.text_base).wrap())
        .child(
            row()
                .gap(8.0)
                .child(Button::new("Reply").variant(ButtonVariant::Ghost).build(k))
                .child(Button::new("Resolve").variant(ButtonVariant::Ghost).build(k)),
        );
    col().w_full().px(12.0).py(8.0).fill(k.c.bg_base).child(card)
}

fn on_key(s: &mut Review, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.diff.key(ev, tx)
}

fn main() {
    let spec = AppSpec {
        scenes: &[
            ("split", |s: &mut Review| s.diff.set_mode(DiffMode::Split)),
            ("scrolled", |s: &mut Review| {
                s.diff = DiffState::new(vec![FileDiff::from_texts("src/net/backoff.rs", LONG_OLD, LONG_NEW)])
            }),
        ],
        on_key: Some(on_key),
        families: ("Inter", "ui-monospace, Inter"),
        ..AppSpec::new("Diff", (900.0, 620.0), init, view)
    };
    App::new(spec).modifier_keys().run();
}
