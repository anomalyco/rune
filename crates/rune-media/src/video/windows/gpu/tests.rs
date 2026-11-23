use super::super::inbox::{Decoded, Inbox, Pic, Pick};
use super::*;
use crate::native_surface::{ByteFrame, ColorInfo, Format};

#[test]
fn a_slot_is_writable_once_its_picture_is_drawn_or_never_will_be() {
    // Written as 7, and Rune has drawn up to 6: a draw of 7 or later frees it.
    assert!(!writable(1, 7, 6));
    assert!(writable(1, 7, 7));
    assert!(writable(1, 7, 9));
    // Handed back undrawn: free with no draw.
    assert!(writable(1, UNREAD, 6));
    // The queue, the surface or a draw in flight still holds the picture.
    assert!(!writable(2, UNREAD, 6));
    assert!(!writable(2, 7, 9));
}

/// A ring's bookkeeping without D3D: the value of the picture in each slot, written and freed
/// by the ring's own rules. A picture's time stands for its value.
struct Slots {
    values: [u64; RING],
    written: u64,
}

impl Slots {
    /// `Ring::take` and `Ring::signal`: a picture goes into a slot nothing in `inbox` holds.
    fn write(&mut self, inbox: &Inbox, released: u64) -> Option<f64> {
        let held = |value: u64| inbox.frames.iter().chain(&inbox.unread).any(|f| f.pts == value as f64);
        let slot = self.values.iter().position(|v| writable(if held(*v) { 2 } else { 1 }, *v, released))?;
        self.written += 1;
        self.values[slot] = self.written;
        Some(self.written as f64)
    }

    /// `Decoder::reclaim` and `Ring::unread`.
    fn reclaim(&mut self, inbox: &mut Inbox) {
        for dropped in inbox.unread.drain(..) {
            if let Some(value) = self.values.iter_mut().find(|v| **v == dropped.pts as u64) {
                *value = UNREAD;
            }
        }
    }
}

fn picture(generation: u32, pts: f64) -> Decoded {
    let bytes = ByteFrame { width: 2, height: 2, format: Format::Nv12Video, color: ColorInfo::default(), planes: [vec![0; 4], vec![0; 2]] };
    Decoded { generation, pts, picture: Pic::Cpu(bytes) }
}

#[test]
fn a_burst_of_seeks_that_draws_nothing_leaves_a_texture_for_the_last_seek() {
    let mut slots = Slots { values: [UNREAD; RING], written: 0 };
    let mut inbox = Inbox::default();
    // Rune drew nothing since the burst began, so the release fence stands still.
    let released = 0;
    for seek in 1..=4 * RING as u32 {
        // The decoder lands the seek and decodes its first pictures.
        inbox.flush();
        slots.reclaim(&mut inbox);
        for _ in 0..2 {
            let pts = slots.write(&inbox, released).unwrap_or_else(|| panic!("no texture for seek {seek}"));
            inbox.push(picture(seek, pts));
        }
        // The window is a seek ahead by then: it drops them as an earlier seek's, undrawn.
        assert!(inbox.take(seek + 1, Pick::First).is_none());
        slots.reclaim(&mut inbox);
    }
}
