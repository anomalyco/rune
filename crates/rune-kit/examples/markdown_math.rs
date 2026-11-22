//! Inline and display math in Markdown (`math` feature): equations sit on the text's
//! baseline in paragraphs, headings, lists and tables, prices and shell variables stay text, and
//! TeX that fails shows its source. `cargo run -p rune-kit --example markdown_math --features markdown,math`
//! opens the window; the stills: `-- --snap math.png --theme dark --size 640x900 --scale 2 --at 2 --realtime` (math renders
//! on a background thread, so the snapshot waits for the wall clock).
use rune::{AppSpec, Cx, El, ScrollSpec, Tx, Waker, col};
use rune_kit::{Kit, Markdown, math};

const DOC: &str = r#"## Euler's $e^{i\pi} + 1 = 0$

Using math.exp(x) computes $e^x$, where $e \approx 2.71828$ (Euler's number). For logits $z_i$, softmax is $\frac{e^{z_i}}{\sum_j e^{z_j}}$, and costs stay at $0.60 to $12.50 with $HOME unchanged.

- A sum $\sum_{k=1}^{n} k = \frac{n(n+1)}{2}$ in a list item
- Sub and superscripts: $x_1^2 + x_2^2 = r^2$, and \(\sqrt{a^2 + b^2}\)
- Shell stays shell: `echo $HOME` and $5.00/$6.00

> A long quoted line wraps around its math: the area $A = \pi r^2$, the volume $V = \frac{4}{3}\pi r^3$, and the series $\sum_{n=0}^{\infty} \frac{x^n}{n!} = e^x$ all stay whole.

| Quantity | Value |
|---|---|
| Golden ratio | $\varphi = \frac{1 + \sqrt{5}}{2}$ |
| Broken TeX | $\notacommand{x}$ |

$$
\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}
$$
"#;

struct State {
    math: math::Client,
}

fn view(_: &State, cx: &mut Cx<State>) -> El {
    let k = Kit::neutral(cx.system_dark());
    col().w_full().h_full().fill(k.c.page).child(
        col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().padding(24.0).child(Markdown::new(DOC).build(&k))),
    )
}

fn setup(s: &mut State, wake: Waker) {
    s.math.set_waker(wake);
}

fn poll(s: &mut State, _: f64, _: &mut Tx) -> bool {
    s.math.changed()
}

fn main() {
    let init = || State { math: math::Client::default() };
    rune::run(AppSpec { setup: Some(setup), poll: Some(poll), ..AppSpec::new("Math", (640.0, 760.0), init, view) });
}
