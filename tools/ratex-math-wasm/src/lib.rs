//! Sandboxed `wasm32-unknown-unknown` LaTeX display-math parser & layout module for `rune-kit`.
//!
//! All security limits (fuel, linear memory, wall-clock deadline, and full binary wire validation)
//! are enforced on the host outside this module. This guest is kept minimal and emits a compact
//! little-endian `RM01` display list for host-side `skrifa` + `tiny-skia` rasterisation.

use core::cell::RefCell;

use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_types::{DisplayItem, PathCommand};

const MAX_INPUT_BYTES: usize = 16 * 1024;
const MAX_ITEMS: usize = 4096;
const MAX_PATH_COMMANDS: usize = 16_384;
const MAX_WIRE_BYTES: usize = 1024 * 1024;

struct State {
    input: Vec<u8>,
    output: Vec<u8>,
}

struct SingleThread<T>(RefCell<T>);

// SAFETY: `wasm32-unknown-unknown` without atomics is strictly single-threaded.
unsafe impl<T> Sync for SingleThread<T> {}

static STATE: SingleThread<State> = SingleThread(RefCell::new(State {
    input: Vec::new(),
    output: Vec::new(),
}));

/// Allocates/resizes the guest input buffer to `len` bytes and returns its linear-memory pointer,
/// or `0` if `len` exceeds the input cap.
#[no_mangle]
pub extern "C" fn alloc_input(len: u32) -> u32 {
    let n = len as usize;
    if n == 0 || n > MAX_INPUT_BYTES {
        return 0;
    }
    let mut st = STATE.0.borrow_mut();
    st.input.clear();
    st.input.resize(n, 0);
    st.input.as_mut_ptr() as u32
}

/// Returns the linear-memory pointer of the most recent `RM01` output buffer.
#[no_mangle]
pub extern "C" fn out_ptr() -> u32 {
    let st = STATE.0.borrow();
    st.output.as_ptr() as u32
}

fn push_f32(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&(v as f32).to_le_bytes());
}

fn push_color(out: &mut Vec<u8>, c: ratex_types::Color) {
    out.push((c.r.clamp(0.0, 1.0) * 255.0).round() as u8);
    out.push((c.g.clamp(0.0, 1.0) * 255.0).round() as u8);
    out.push((c.b.clamp(0.0, 1.0) * 255.0).round() as u8);
    out.push((c.a.clamp(0.0, 1.0) * 255.0).round() as u8);
}

/// Parses and lays out the `len`-byte UTF-8 TeX source written into `alloc_input(len)`.
/// Returns the byte length of the `RM01` binary display list in `out_ptr()`, or `0` on failure.
#[no_mangle]
pub extern "C" fn render_tex(len: u32) -> u32 {
    let mut st = STATE.0.borrow_mut();
    let n = (len as usize).min(st.input.len()).min(MAX_INPUT_BYTES);
    let trimmed = match core::str::from_utf8(&st.input[..n]) {
        Ok(s) => s.trim(),
        Err(_) => return 0,
    };
    if trimmed.is_empty() {
        return 0;
    }
    let nodes = match ratex_parser::parse(trimmed) {
        Ok(nodes) => nodes,
        Err(_) => return 0,
    };
    let opts = LayoutOptions::default();
    let root = layout(&nodes, &opts);
    let dl = to_display_list(&root);
    if dl.items.is_empty() || dl.items.len() > MAX_ITEMS {
        return 0;
    }
    if !dl.width.is_finite()
        || !dl.height.is_finite()
        || !dl.depth.is_finite()
        || dl.width < 0.0
        || dl.height < 0.0
        || dl.depth < 0.0
    {
        return 0;
    }
    let out = &mut st.output;
    out.clear();
    out.extend_from_slice(b"RM01");
    push_f32(out, dl.width);
    push_f32(out, dl.height);
    push_f32(out, dl.depth);
    out.extend_from_slice(&(dl.items.len() as u32).to_le_bytes());

    let mut total_cmds = 0usize;
    for item in &dl.items {
        if out.len() > MAX_WIRE_BYTES {
            out.clear();
            return 0;
        }
        match item {
            DisplayItem::GlyphPath { x, y, scale, font, char_code, color } => {
                let fb = font.as_bytes();
                if fb.is_empty() || fb.len() > 64 {
                    out.clear();
                    return 0;
                }
                out.push(0);
                push_f32(out, *x);
                push_f32(out, *y);
                push_f32(out, *scale);
                out.extend_from_slice(&char_code.to_le_bytes());
                push_color(out, *color);
                out.push(fb.len() as u8);
                out.extend_from_slice(fb);
            }
            DisplayItem::Line { x, y, width, thickness, color, dashed } => {
                out.push(1);
                push_f32(out, *x);
                push_f32(out, *y);
                push_f32(out, *width);
                push_f32(out, *thickness);
                push_color(out, *color);
                out.push(u8::from(*dashed));
            }
            DisplayItem::Rect { x, y, width, height, color } => {
                out.push(2);
                push_f32(out, *x);
                push_f32(out, *y);
                push_f32(out, *width);
                push_f32(out, *height);
                push_color(out, *color);
            }
            DisplayItem::Path { x, y, commands, fill, color } => {
                total_cmds = total_cmds.saturating_add(commands.len());
                if total_cmds > MAX_PATH_COMMANDS {
                    out.clear();
                    return 0;
                }
                out.push(3);
                push_f32(out, *x);
                push_f32(out, *y);
                out.push(u8::from(*fill));
                push_color(out, *color);
                out.extend_from_slice(&(commands.len() as u32).to_le_bytes());
                for cmd in commands {
                    match cmd {
                        PathCommand::MoveTo { x, y } => {
                            out.push(0);
                            push_f32(out, *x);
                            push_f32(out, *y);
                        }
                        PathCommand::LineTo { x, y } => {
                            out.push(1);
                            push_f32(out, *x);
                            push_f32(out, *y);
                        }
                        PathCommand::QuadTo { x1, y1, x, y } => {
                            out.push(2);
                            push_f32(out, *x1);
                            push_f32(out, *y1);
                            push_f32(out, *x);
                            push_f32(out, *y);
                        }
                        PathCommand::CubicTo { x1, y1, x2, y2, x, y } => {
                            out.push(3);
                            push_f32(out, *x1);
                            push_f32(out, *y1);
                            push_f32(out, *x2);
                            push_f32(out, *y2);
                            push_f32(out, *x);
                            push_f32(out, *y);
                        }
                        PathCommand::Close => {
                            out.push(4);
                        }
                    }
                }
            }
        }
    }
    if out.len() > MAX_WIRE_BYTES {
        out.clear();
        return 0;
    }
    out.len() as u32
}
