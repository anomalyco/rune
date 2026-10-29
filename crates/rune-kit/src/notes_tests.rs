use rune::{Cx, El, Env, Runtime, col};

use super::{Note, Piece, cut};
use crate::Kit;

const SAMPLE_NOTE: &str = "\
Lead summary line.

> [!note]
> Attention callout paragraph.

```facts
Latency: {verified} 11.9 ms
Mode: `Rgba16Float`
```

```compare
Before: Software rasterized overlay loop.
After: Single-pass GPU uber-quad exposure.
```

```steps
{built} Shared shutter pass · verified
{proposed} Live preview window
```

```size
app +120 −30 · tests +45 −0 · 4 files
```

```rust title=\"shutter.rs\" {2-3}
fn add_sample(c: Vec4, w: f32) -> Vec4 {
+   c * w
}
```";

#[test]
fn cut_extracts_all_structured_note_blocks() {
    let pieces = cut(SAMPLE_NOTE);
    assert_eq!(pieces.len(), 7);
    assert!(matches!(&pieces[0], Piece::Markdown(s) if s.contains("Lead summary")));
    assert!(matches!(&pieces[1], Piece::Callout { label: None, body } if body.contains("Attention")));
    assert!(matches!(&pieces[2], Piece::Facts(rows) if rows.len() == 2));
    assert!(matches!(&pieces[3], Piece::Compare(sides) if sides.len() == 2));
    assert!(matches!(&pieces[4], Piece::Steps(steps) if steps.len() == 2 && steps[0].status == "built"));
    assert!(matches!(&pieces[5], Piece::Size(sz) if sz.parts.len() == 2 && sz.files.as_deref() == Some("4 files")));
    assert!(matches!(
        &pieces[6],
        Piece::CodeFence { title: Some(t), marks, .. } if t == "shutter.rs" && marks == &[2..=3]
    ));
}

fn host_view(src: &String, _cx: &mut Cx<String>) -> El {
    let k = Kit::neutral(true);
    col().w(600.0).padding(16.0).child(Note::new(src).key("doc").build(&k))
}

#[test]
fn note_mounts_and_lays_out_all_primitives_headlessly() {
    let mut rt = Runtime::new(SAMPLE_NOTE.to_owned(), host_view, (640.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for idx in 0..7 {
        let key = format!("doc/p{idx}");
        let ([_, _, w, h], opacity) = rt.presented(&key).unwrap_or_else(|| panic!("missing piece {key}"));
        assert!(w > 100.0 && h > 10.0, "{key} has empty bounds {w}x{h}");
        assert_eq!(opacity, 1.0);
    }
}
