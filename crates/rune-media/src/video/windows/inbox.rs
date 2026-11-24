//! What the UI thread and a clip's decoder thread tell each other, under one lock: the UI's asks
//! (a seek, sound, the layer, a stop) and the decoder's answers (its news, the pictures it
//! decoded, the end). The picture queue's steps are methods here, apart from Media Foundation
//! and D3D, so they can be driven with plain data.

use super::LayerSurface;
use super::decoder::Facts;
use crate::native_surface::ByteFrame;
use crate::native_surface::windows::DecodedPlanes;
use rune_core::time::Instant;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// A seek the UI asked the decoder to land: clip seconds, the generation its pictures carry,
/// and whether to decode forward to `to` (`exact == true`) or keep the first keyframe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SeekAsk {
    pub to: f64,
    pub generation: u32,
    pub exact: bool,
}

/// What the UI asks of the decoder and what the decoder hands back, under one lock.
#[derive(Default)]
pub(super) struct Inbox {
    /// A seek to land.
    pub seek: Option<SeekAsk>,
    pub looping: bool,
    /// Play sound, at this gain (unmuted, and the clip has some).
    pub sound: Option<f32>,
    pub stop: bool,
    /// When a frame last drew the clip.
    pub touched: Option<Instant>,
    pub news: Vec<News>,
    pub frames: VecDeque<Decoded>,
    /// The generation whose last picture is in the queue: the clip ended there.
    pub ended: Option<u32>,
    /// The generation whose video stream reached its end: no more pictures will arrive for it,
    /// though trailing sound may still be playing out.
    pub video_ended: Option<u32>,
    /// Unwrapped end time of the clip's audio when audio plays past the video or duration.
    pub audio_end: Option<f64>,
    /// Pictures dropped before they reached the surface (an earlier seek's, late ones, ones the
    /// layer showed): Rune never draws them, so no draw releases their textures. The decoder
    /// frees those itself (`Decoder::reclaim`).
    pub unread: Vec<Decoded>,
    /// The window shows the clip on its own layer at this size (device pixels): while the clock
    /// runs, the decoder presents each picture there itself (`layer`). None: the UI draws them.
    pub layer: Option<(u32, u32)>,
    /// The layer's id and DirectComposition surface handle, once made, for the window's visual.
    pub layer_surface: Option<LayerSurface>,
    /// The size of the last picture presented on the layer, and the generation it belongs to.
    pub layer_shown: Option<((u32, u32), u32)>,
    /// The newest picture the decoder presented on the layer: its time and its generation. It
    /// outlives the layer, so the window can still ask what the layer showed last once it's gone.
    pub layer_presented: Option<(f64, u32)>,
    /// The clip can't have a layer (a rotated clip, software pictures, no video processor): the
    /// UI keeps drawing it.
    pub layer_refused: bool,
}

pub(super) enum News {
    Opened(Facts),
    Failed(String),
    /// Let go after `PARK` undrawn.
    Parked,
}

pub(super) struct Decoded {
    pub generation: u32,
    /// Seconds from the clip's start, counting on through loops.
    pub pts: f64,
    pub picture: Pic,
}

pub(super) enum Pic {
    Gpu(Arc<DecodedPlanes>, u64),
    Cpu(ByteFrame),
}

/// Which picture a frame takes off the queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pick {
    /// The first of the current seek: nothing of it shows yet.
    First,
    /// The newest whose time has come at this clock position; the ones before it are late.
    Due(f64),
    /// None: the picture on screen holds.
    Hold,
}

impl Inbox {
    /// Drops the pictures of seeks before `generation`, then takes the one `pick` asks for.
    /// What it drops goes to `unread`.
    pub(super) fn take(&mut self, generation: u32, pick: Pick) -> Option<Decoded> {
        while self.frames.front().is_some_and(|f| f.generation != generation) {
            self.unread.extend(self.frames.pop_front());
        }
        match pick {
            Pick::First => self.frames.pop_front(),
            Pick::Due(pos) => {
                let mut newest = None;
                while self.frames.front().is_some_and(|f| f.pts <= pos) {
                    self.unread.extend(newest.replace(self.frames.pop_front()?));
                }
                newest
            }
            Pick::Hold => None,
        }
    }

    /// A seek lands: its pictures replace the queue's, which go to `unread`, and the end is
    /// ahead again.
    pub(super) fn flush(&mut self) {
        self.unread.extend(self.frames.drain(..));
        self.ended = None;
        self.video_ended = None;
        self.audio_end = None;
    }

    /// Queues a decoded picture; true when the queue was empty.
    pub(super) fn push(&mut self, decoded: Decoded) -> bool {
        let was_empty = self.frames.is_empty();
        self.frames.push_back(decoded);
        was_empty
    }

    /// Keeps only the newest picture; the others go to `unread`: the layer showed them, so the
    /// window never takes them.
    pub(super) fn keep_newest(&mut self) {
        let older = self.frames.len().saturating_sub(1);
        self.unread.extend(self.frames.drain(..older));
    }

    /// Queues the picture the decoder is about to present on the layer, as the only one in the
    /// queue (for when the window draws the clip itself again), and notes it as the one the
    /// layer shows. Presenting counts as being drawn, so the decoder doesn't park.
    pub(super) fn push_presented(&mut self, decoded: Decoded, now: Instant) {
        self.layer_presented = Some((decoded.pts, decoded.generation));
        self.push(decoded);
        self.keep_newest();
        self.touched = Some(now);
    }

    /// The newest picture of `generation` the layer showed: its time.
    pub(super) fn presented(&self, generation: u32) -> Option<f64> {
        self.layer_presented.filter(|(_, g)| *g == generation).map(|(pts, _)| pts)
    }
}

pub(super) struct Pipe {
    inbox: Mutex<Inbox>,
    wake: Condvar,
}

impl Pipe {
    pub(super) fn new(inbox: Inbox) -> Arc<Pipe> {
        Arc::new(Pipe { inbox: Mutex::new(inbox), wake: Condvar::new() })
    }
    pub(super) fn lock(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap()
    }
    /// Tell the decoder something changed (a seek, a picture taken, a stop).
    pub(super) fn poke(&self) {
        self.wake.notify_all();
    }
    /// Lands the UI's seeks: takes the newest ask, drops the queue for it and runs `seek` (the
    /// source's own seek, which can take as long as a network round trip) with the inbox
    /// unlocked, since the UI thread locks it in every frame that draws the clip and for every
    /// seek it asks for. A seek asked for meanwhile is landed next, before the caller reads
    /// anything for the one before it, so only the newest is decoded.
    pub(super) fn land_seeks<'a, E>(&'a self, mut inbox: MutexGuard<'a, Inbox>, mut seek: impl FnMut(SeekAsk) -> Result<(), E>) -> Result<MutexGuard<'a, Inbox>, E> {
        while let Some(ask) = inbox.seek.take() {
            inbox.flush();
            drop(inbox);
            seek(ask)?;
            inbox = self.lock();
        }
        Ok(inbox)
    }
    /// The decoder sleeps until it's poked or `patience` passes.
    pub(super) fn wait<'a>(&self, inbox: MutexGuard<'a, Inbox>, patience: Duration) -> MutexGuard<'a, Inbox> {
        self.wake.wait_timeout(inbox, patience).unwrap().0
    }
}

#[cfg(test)]
mod tests;