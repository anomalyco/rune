//! Shared still-frame helpers for Linux Player backends (no ffmpeg).
use super::*;

/// Nearest-neighbour shrink so the long edge is at most `max` (scrub previews).
pub(super) fn fit_bgra(frame: ByteFrame, max: u32) -> ByteFrame {
    let long = frame.width.max(frame.height);
    if max == 0 || long <= max {
        return frame;
    }
    let s = max as f64 / long as f64;
    let nw = ((frame.width as f64) * s).round().max(1.0) as u32;
    let nh = ((frame.height as f64) * s).round().max(1.0) as u32;
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    let sw = frame.width as usize;
    let sh = frame.height as usize;
    let src = &frame.planes[0];
    for y in 0..nh as usize {
        let sy = (y as f64 * sh as f64 / nh as f64).floor() as usize;
        let sy = sy.min(sh.saturating_sub(1));
        for x in 0..nw as usize {
            let sx = (x as f64 * sw as f64 / nw as f64).floor() as usize;
            let sx = sx.min(sw.saturating_sub(1));
            let si = (sy * sw + sx) * 4;
            let di = (y * nw as usize + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    ByteFrame {
        width: nw,
        height: nh,
        format: Format::Bgra,
        color: frame.color,
        planes: [out, Vec::new()],
    }
}
