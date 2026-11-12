//! Windows: a playing clip shown on its own layer under the window's UI
//! (`video::windows::layer`), so playing draws no UI frame per picture.
//!
//! The layer is a DirectComposition visual under the UI's; the UI punches the video's place out of
//! its pixels, so corners, clips, opacity and anything drawn over the video stay the UI's own. A
//! swap chain's present and a DirectComposition commit land on different compositions, so the
//! layer only shows while the video's place is still: while it moves (scrolling, a resize, a
//! fade), the UI draws the pictures, as it does without a layer. Once it has held still for a
//! frame, the decoder presents on the layer at its size; once a picture is there, the visual is
//! moved, committed and waited for, and the UI punches from that frame on.
//!
//! The UI's swap chain composites premultiplied only while a layer shows: DWM then blends every
//! region the UI presents, which costs nothing while it's still but 12-15% of an integrated GPU
//! while it animates (`proof\video-win\layer`).

use crate::native_surface::{SurfaceDraw, place};
use crate::platform::win32::dcomp_handover::Handover;
use crate::time::Instant;
use std::sync::Arc;
use std::time::Duration;
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;

/// How long no layer shows before the UI's swap chain goes back to opaque.
const OPAQUE_AFTER: Duration = Duration::from_secs(2);

/// Who shows one clip's pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// The UI draws them: the first, a paused clip's, one whose place moves, one in an isolated
    /// layer, one the decoder can't layer.
    Drawn,
    /// Its place held still for a frame: the decoder presents on the layer at `size`.
    Placing { size: (u32, u32) },
    /// The layer at `at` shows them under the UI, which punches their place out.
    Layered { size: (u32, u32), at: (i32, i32) },
}

/// What one frame saw of a clip.
#[derive(Clone, Copy, Debug)]
pub(super) struct Seen {
    /// Its place is the last frame's.
    pub still: bool,
    /// It could go on a layer: playing, layerable, drawn in the window's own pass, and the
    /// window composites a layer (premultiplied, handed over, the punch compiled).
    pub ready: bool,
    /// The decoder presented a picture on the layer at the asked size.
    pub showing: bool,
    pub size: (u32, u32),
    pub at: (i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// The decoder presents on the layer at this size, or (None) leaves the pictures to the UI.
    Ask(Option<(u32, u32)>),
    /// Move the layer's visual to `at`, commit, and wait for DWM to take it.
    Show { at: (i32, i32) },
    /// Punch the clip's place out this frame.
    Punch,
}

pub(super) fn step(phase: &mut Phase, seen: Seen) -> Effect {
    match *phase {
        Phase::Drawn if seen.still && seen.ready => {
            *phase = Phase::Placing { size: seen.size };
            Effect::Ask(Some(seen.size))
        }
        Phase::Drawn => Effect::Ask(None),
        Phase::Placing { size } if !seen.still || !seen.ready || seen.size != size => {
            *phase = Phase::Drawn;
            Effect::Ask(None)
        }
        Phase::Placing { size } if seen.showing => {
            *phase = Phase::Layered { size, at: seen.at };
            Effect::Show { at: seen.at }
        }
        Phase::Placing { size } => Effect::Ask(Some(size)),
        Phase::Layered { size, at } if seen.still && seen.ready && seen.showing && seen.size == size && seen.at == at => Effect::Punch,
        Phase::Layered { .. } => {
            *phase = Phase::Drawn;
            Effect::Ask(None)
        }
    }
}

/// The place a clip's frame was drawn at; equal two frames running means it held still.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    image: [f32; 4],
    node: [f32; 4],
    radii: [f32; 4],
    clip: [f32; 4],
    fade: [u32; 4],
    opacity: f32,
}

struct Entry {
    key: usize,
    video: Arc<crate::video::Shared>,
    phase: Phase,
    last: Option<Place>,
    /// The last frame drew it inside an isolated layer, where a punch can't reach the window.
    isolated: bool,
    /// Its layer's visual, and the decoder's surface handle it shows: a decoder opened again (a
    /// replaced GPU device) makes a new layer with a new handle.
    visual: Option<(IDCompositionVisual, usize)>,
    /// What `RUNE_VIDEO_LOG` last printed about it.
    logged: Option<(bool, bool, bool, bool, bool, bool, bool, bool)>,
}

/// What a frame does about the window's layers.
#[derive(Default)]
pub(super) struct Frame {
    /// The surfaces to punch out (`Renderer::set_punched_surfaces`).
    pub punched: Vec<usize>,
    /// A clip is on its way to its layer: draw again.
    pub again: bool,
}

#[derive(Default)]
pub(super) struct VideoLayers {
    entries: Vec<Entry>,
    /// The UI's swap chain should composite premultiplied (Some(true)) or opaque (Some(false));
    /// `Host::draw` makes it so before its next frame.
    pending_alpha: Option<bool>,
    /// Since when no clip has wanted a layer.
    unused_since: Option<Instant>,
}

impl VideoLayers {
    /// Before a frame's surfaces are prepared: steps each drawn clip and carries out its effect.
    pub(super) fn frame(&mut self, draws: &[SurfaceDraw], handover: Option<&Handover>, premultiplied: bool, punch_ready: bool) -> Frame {
        let mut out = Frame::default();
        let window_ready = handover.is_some() && premultiplied && punch_ready && enabled();
        let mut wanted = false;
        let mut kept = Vec::with_capacity(draws.len());
        for d in draws {
            let (Some(video), Some(frame)) = (d.surface.video(), d.surface.latest()) else { continue };
            let key = d.surface.key();
            if kept.contains(&key) {
                // Drawn twice in one frame: a layer can be in one place only.
                continue;
            }
            kept.push(key);
            let upright = frame.orient.upright(frame.size);
            let image = place::image_rect(d.rect, upright, d.fit, d.surface.place());
            let here = Place { image, node: d.rect, radii: d.radii, clip: d.clip, fade: d.fade, opacity: d.opacity };
            let entry = match self.entries.iter().position(|e| e.key == key) {
                Some(i) => &mut self.entries[i],
                None => {
                    self.entries.push(Entry { key, video, phase: Phase::Drawn, last: None, isolated: false, visual: None, logged: None });
                    self.entries.last_mut().unwrap()
                }
            };
            let size = (image[2].round().max(1.0) as u32, image[3].round().max(1.0) as u32);
            let at = (image[0].round() as i32, image[1].round() as i32);
            let still = entry.last == Some(here);
            entry.last = Some(here);
            let ask = match entry.phase {
                Phase::Placing { size } | Phase::Layered { size, .. } => Some(size),
                Phase::Drawn => None,
            };
            let status = crate::video::layer(&entry.video, ask);
            let layerable = status.running && !status.refused && !entry.isolated && d.opacity > 0.0;
            wanted |= layerable && still && enabled();
            let seen = Seen { still, ready: layerable && window_ready, showing: status.showing, size, at };
            let before = entry.phase;
            let effect = step(&mut entry.phase, seen);
            if logging() {
                let why = (status.running, status.refused, entry.isolated, handover.is_some(), premultiplied, punch_ready, still, status.showing);
                if entry.phase != before || entry.logged != Some(why) {
                    entry.logged = Some(why);
                    eprintln!(
                        "rune video layer: {before:?} -> {:?} (running {}, refused {}, isolated {}, handed {}, premultiplied {}, punch {}, still {}, showing {}) at {at:?} size {size:?}",
                        entry.phase, why.0, why.1, why.2, why.3, why.4, why.5, why.6, why.7
                    );
                }
            }
            match effect {
                Effect::Ask(next) => {
                    if next != ask {
                        crate::video::layer(&entry.video, next);
                    }
                    // Back from the layer: this frame draws the picture for now, not the one
                    // the surface held when the layer took over.
                    if matches!(before, Phase::Layered { .. }) {
                        crate::video::unlayer(&entry.video, &d.surface);
                    }
                    out.again |= next.is_some();
                }
                Effect::Show { at } => match show(handover, entry, status.handle, at) {
                    Ok(()) => out.punched.push(key),
                    Err(e) => {
                        eprintln!("rune: a clip's layer couldn't show, the window draws it: {e}");
                        entry.phase = Phase::Drawn;
                        crate::video::layer(&entry.video, None);
                    }
                },
                Effect::Punch => out.punched.push(key),
            }
        }
        // Clips no longer drawn leave their layers.
        let before = self.entries.len();
        self.entries.retain(|e| {
            let drawn = kept.contains(&e.key);
            if !drawn {
                release(handover, e);
            }
            drawn
        });
        if self.entries.len() != before
            && let Some(h) = handover
        {
            let _ = h.commit();
        }
        self.follow_alpha(wanted, premultiplied, handover.is_some());
        out
    }

    /// After a frame was drawn: which clips it drew inside an isolated layer.
    pub(super) fn drawn(&mut self, isolated: impl Fn(usize) -> Option<bool>) {
        for e in &mut self.entries {
            e.isolated = isolated(e.key).unwrap_or(false);
        }
    }

    /// The window is covered, minimised or goes: every clip's pictures go back to the UI, which
    /// draws them when it next draws, and the decoders stop presenting.
    pub(super) fn release(&mut self, handover: Option<&Handover>) {
        for e in self.entries.drain(..) {
            release(handover, &e);
        }
        if let Some(h) = handover {
            let _ = h.commit();
        }
    }

    /// The alpha the UI's swap chain should switch to before the next frame, once.
    pub(super) fn take_alpha(&mut self) -> Option<bool> {
        self.pending_alpha.take()
    }

    fn follow_alpha(&mut self, wanted: bool, premultiplied: bool, handed: bool) {
        if !handed {
            return;
        }
        let now = Instant::now();
        if wanted {
            self.unused_since = None;
            if !premultiplied {
                self.pending_alpha = Some(true);
            }
            return;
        }
        let since = *self.unused_since.get_or_insert(now);
        if premultiplied && now.saturating_duration_since(since) >= OPAQUE_AFTER {
            self.pending_alpha = Some(false);
        }
    }
}

/// Puts the clip's visual at `at`, made from the layer's surface handle the first time, and waits
/// for DWM to take it, so the frame that punches lands over it.
fn show(handover: Option<&Handover>, entry: &mut Entry, handle: Option<usize>, at: (i32, i32)) -> Result<(), String> {
    let handover = handover.ok_or("the window isn't on DirectComposition")?;
    let handle = handle.ok_or("the layer has no surface yet")?;
    if entry.visual.as_ref().is_none_or(|(_, shown)| *shown != handle) {
        if let Some((old, _)) = entry.visual.take() {
            handover.remove(&old);
        }
        entry.visual = Some((handover.add_under(handle)?, handle));
    }
    let (visual, _) = entry.visual.as_ref().unwrap();
    // SAFETY: COM calls on a visual of the window's DirectComposition device.
    unsafe {
        visual.SetOffsetX2(at.0 as f32).map_err(|e| format!("SetOffsetX: {e}"))?;
        visual.SetOffsetY2(at.1 as f32).map_err(|e| format!("SetOffsetY: {e}"))?;
    }
    handover.commit_and_wait()
}

/// `RUNE_VIDEO_LAYER=0` keeps every clip's pictures in the UI, as without layers.
fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_VIDEO_LAYER").is_none_or(|v| v != "0"))
}

/// `RUNE_VIDEO_LOG` set: each clip's way to its layer.
fn logging() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_VIDEO_LOG").is_some_and(|v| v != "0"))
}

fn release(handover: Option<&Handover>, e: &Entry) {
    crate::video::layer(&e.video, None);
    if let (Some(h), Some((v, _))) = (handover, &e.visual) {
        h.remove(v);
    }
}
