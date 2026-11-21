//! Strict, zero-trust host decoder for the `RM01` binary display list emitted by the sandboxed
//! `ratex-math.wasm` guest.
//!
//! Every byte, length, offset, font name, coordinate, scale, and command count from the guest is
//! treated as untrusted input and validated against hard host bounds before rasterisation.

pub(super) const MAX_WIRE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_ITEMS: usize = 4096;
pub(super) const MAX_PATH_COMMANDS: usize = 16_384;
const MAX_FONT_NAME_LEN: usize = 64;
const MAX_COORD_EM: f32 = 16_384.0;
const MAX_GLYPH_SCALE: f32 = 100.0;
const MIN_GLYPH_SCALE: f32 = 0.01;

#[derive(Debug, Clone)]
pub(super) struct DisplayList {
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) depth: f32,
    pub(super) items: Vec<DisplayItem>,
}

#[derive(Debug, Clone)]
pub(super) enum DisplayItem {
    Glyph {
        x: f32,
        y: f32,
        scale: f32,
        char_code: u32,
        color: [u8; 4],
        font: String,
    },
    Line {
        x: f32,
        y: f32,
        width: f32,
        thickness: f32,
        color: [u8; 4],
        dashed: bool,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [u8; 4],
    },
    Path {
        x: f32,
        y: f32,
        fill: bool,
        color: [u8; 4],
        commands: Vec<PathCmd>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32(&mut self) -> Option<f32> {
        let b = self.take(4)?;
        let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        v.is_finite().then_some(v)
    }

    fn coord(&mut self) -> Option<f32> {
        let v = self.f32()?;
        (v.abs() <= MAX_COORD_EM).then_some(v)
    }

    fn non_neg_coord(&mut self) -> Option<f32> {
        let v = self.coord()?;
        (v >= 0.0).then_some(v)
    }

    fn rgba(&mut self) -> Option<[u8; 4]> {
        let b = self.take(4)?;
        Some([b[0], b[1], b[2], b[3]])
    }
}

/// Decodes and validates an untrusted `RM01` byte slice from the math guest.
/// Returns `None` on any framing error, trailing garbage, out-of-range coordinate, or cap breach.
pub(super) fn decode(bytes: &[u8]) -> Option<DisplayList> {
    if bytes.len() < 20 || bytes.len() > MAX_WIRE_BYTES {
        return None;
    }
    let mut c = Cursor::new(bytes);
    if c.take(4)? != b"RM01" {
        return None;
    }
    let width = c.non_neg_coord()?;
    let height = c.non_neg_coord()?;
    let depth = c.non_neg_coord()?;
    if width <= 0.0 && (height + depth) <= 0.0 {
        return None;
    }
    let count = c.u32()? as usize;
    if count == 0 || count > MAX_ITEMS {
        return None;
    }
    let mut items = Vec::with_capacity(count.min(256));
    let mut total_cmds = 0usize;

    for _ in 0..count {
        match c.u8()? {
            0 => {
                let x = c.coord()?;
                let y = c.coord()?;
                let scale = c.f32()?;
                if !(MIN_GLYPH_SCALE..=MAX_GLYPH_SCALE).contains(&scale) {
                    return None;
                }
                let char_code = c.u32()?;
                char::from_u32(char_code)?;
                let color = c.rgba()?;
                let name_len = c.u8()? as usize;
                if name_len == 0 || name_len > MAX_FONT_NAME_LEN {
                    return None;
                }
                let name_bytes = c.take(name_len)?;
                if !name_bytes.iter().all(|b| b.is_ascii_graphic()) {
                    return None;
                }
                let font = std::str::from_utf8(name_bytes).ok()?.to_owned();
                items.push(DisplayItem::Glyph { x, y, scale, char_code, color, font });
            }
            1 => {
                let x = c.coord()?;
                let y = c.coord()?;
                let width = c.non_neg_coord()?;
                let thickness = c.non_neg_coord()?;
                let color = c.rgba()?;
                let dashed = match c.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                items.push(DisplayItem::Line { x, y, width, thickness, color, dashed });
            }
            2 => {
                let x = c.coord()?;
                let y = c.coord()?;
                let width = c.non_neg_coord()?;
                let height = c.non_neg_coord()?;
                let color = c.rgba()?;
                items.push(DisplayItem::Rect { x, y, width, height, color });
            }
            3 => {
                let x = c.coord()?;
                let y = c.coord()?;
                let fill = match c.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                let color = c.rgba()?;
                let cmd_count = c.u32()? as usize;
                total_cmds = total_cmds.checked_add(cmd_count)?;
                if total_cmds > MAX_PATH_COMMANDS {
                    return None;
                }
                let mut commands = Vec::with_capacity(cmd_count.min(256));
                for _ in 0..cmd_count {
                    match c.u8()? {
                        0 => commands.push(PathCmd::MoveTo(c.coord()?, c.coord()?)),
                        1 => commands.push(PathCmd::LineTo(c.coord()?, c.coord()?)),
                        2 => commands.push(PathCmd::QuadTo(c.coord()?, c.coord()?, c.coord()?, c.coord()?)),
                        3 => commands.push(PathCmd::CubicTo(
                            c.coord()?, c.coord()?, c.coord()?, c.coord()?, c.coord()?, c.coord()?,
                        )),
                        4 => commands.push(PathCmd::Close),
                        _ => return None,
                    }
                }
                items.push(DisplayItem::Path { x, y, fill, color, commands });
            }
            _ => return None,
        }
    }
    if c.pos != bytes.len() {
        return None;
    }
    Some(DisplayList { width, height, depth, items })
}
