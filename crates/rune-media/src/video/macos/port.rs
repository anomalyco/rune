//! `AVPlayer` + `AVPlayerItemVideoOutput` construction and the `turn::Port` adapter (`Av`).

#![allow(unsafe_code)]

use super::{Decoded, Native, Objects, Player, asset, park};
use crate::native_surface::macos::PixelBuffer;
use crate::native_surface::{ByteFrame, NativeSurface};
use crate::video::phases::Item;
use crate::video::turn::Port;
use crate::video::{Kind, MediaSource, Meta, Shared};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_av_foundation::{AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVGetCurrentHostTime, CVGetHostClockFrequency, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{NSDictionary, NSString};
use rune_core::machine::Attempt;
use rune_core::time::Instant;
use std::sync::Arc;

fn create_player(source: &MediaSource, headers: &[(String, String)], kind: Kind, decoded: &Decoded, mtm: MainThreadMarker) -> Result<Objects, String> {
    let asset = asset::player_asset(source, headers, decoded)?;
    // SAFETY: `asset` is a live `AVURLAsset` and `mtm` proves we are on the main thread.
    let item = unsafe { AVPlayerItem::playerItemWithAsset(&asset, mtm) };
    let output = (kind == Kind::Video).then(|| {
        let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
        // SAFETY: the three `kCVPixelBuffer*` keys are immutable static `CFString` constants and values are live `CFType`s.
        let attributes = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey],
                &[format.as_ref(), AsRef::<CFType>::as_ref(&*empty), CFBoolean::new(true).as_ref()],
            )
        };
        // SAFETY: `CFDictionary` and `NSDictionary` are toll-free bridged, and the reference does not outlive `attributes`.
        let attributes = unsafe { &*(CFRetained::as_ptr(&attributes).as_ptr() as *const NSDictionary<NSString, AnyObject>) };
        // SAFETY: `attributes` is a valid pixel-buffer dictionary, `output` is freshly allocated, and `item` is a live `AVPlayerItem` on the main thread.
        unsafe {
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(AVPlayerItemVideoOutput::alloc(), Some(attributes));
            output.setSuppressesPlayerRendering(true);
            item.addOutput(&output);
            output
        }
    });
    // SAFETY: `item` is a live `AVPlayerItem` and `mtm` proves we are on the main thread.
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
    Ok(Objects { player, item, output })
}

/// Seconds on the host clock (`CACurrentMediaTime`'s) for an `Instant`.
fn host_seconds(at: Instant) -> f64 {
    let now = rune_core::time::now();
    let host = CVGetCurrentHostTime() as f64 / CVGetHostClockFrequency();
    if at >= now { host + (at - now).as_secs_f64() } else { host - (now - at).as_secs_f64() }
}

/// A clip's native side for one turn, with its locks held. Reads and does what it's asked.
pub(super) struct Av<'a> {
    shared: &'a Arc<Shared>,
    native: &'a mut Native,
    /// Filled by the caller that already checked, or by the turn when it first needs to know.
    mtm: Option<MainThreadMarker>,
    /// Video, during a pull: the surface and the presentation time its frame is for.
    target: Option<(&'a NativeSurface, Instant)>,
    /// A finished poster load's frame.
    poster: Option<ByteFrame>,
    /// A poster the turn asked for, started once the control lock is released.
    pub(super) asked: Option<(f64, Attempt)>,
}

impl<'a> Av<'a> {
    pub(super) fn new(
        shared: &'a Arc<Shared>,
        native: &'a mut Native,
        mtm: Option<MainThreadMarker>,
        target: Option<(&'a NativeSurface, Instant)>,
    ) -> Self {
        Self { shared, native, mtm, target, poster: None, asked: None }
    }

    fn player(&mut self) -> &mut Player {
        self.native.player.as_mut().unwrap()
    }

    fn objects(&self) -> &Objects {
        self.native.player.as_ref().unwrap().objects.as_ref().unwrap()
    }
}

impl Port for Av<'_> {
    fn on_main(&mut self) -> bool {
        self.mtm = self.mtm.or_else(MainThreadMarker::new);
        self.mtm.is_some()
    }

    fn take_poster(&mut self) -> Option<(Attempt, Result<Meta, String>)> {
        let (attempt, done) = self.native.posters.lock().unwrap().done.take()?;
        Some((attempt, done.map(|(bytes, meta)| {
            self.poster = bytes;
            meta
        })))
    }

    fn show_poster(&mut self, next_id: &mut dyn FnMut() -> u64) -> Result<(), String> {
        let (Some(bytes), Some((surface, _))) = (self.poster.take(), self.target) else { return Ok(()) };
        surface.push_bytes(bytes, next_id())
    }

    fn ask_poster(&mut self, at: f64, attempt: Attempt) {
        self.asked = Some((at, attempt));
    }

    fn make_player(&mut self) -> Result<Instant, String> {
        let shared = self.shared;
        let objects = create_player(&shared.source, &shared.headers, shared.kind, &self.native.decoded, self.mtm.unwrap())?;
        self.native.player = Some(Player::new(objects));
        Ok(rune_core::time::now())
    }

    fn drop_player(&mut self) {
        self.native.player = None;
    }

    fn unpark(&mut self) {
        park::unpark(self.shared);
    }

    fn item(&self) -> Item {
        let item = &self.objects().item;
        // SAFETY: `item` is a live `AVPlayerItem` accessed on the main thread under `shared.native`.
        let status = unsafe { item.status() };
        if status == AVPlayerItemStatus::Failed {
            // SAFETY: `item` is a live `AVPlayerItem` in `Failed` status accessed on the main thread.
            let error = unsafe { item.error() };
            Item::Failed(error.map_or_else(|| "playback failed".into(), |e| e.localizedDescription().to_string()))
        } else if status == AVPlayerItemStatus::ReadyToPlay {
            Item::Ready
        } else {
            Item::Loading
        }
    }

    fn orient(&mut self) {
        let o = self.objects();
        if o.output.is_some() {
            // SAFETY: `o.item` is a live `AVPlayerItem` accessed on the main thread.
            let asset = unsafe { o.item.asset() };
            let orient = asset::orient(&asset);
            self.player().orient = orient;
        }
    }

    fn item_facts(&self) -> Meta {
        let item = &self.objects().item;
        // SAFETY: `item` is a live `AVPlayerItem` accessed on the main thread.
        let size = unsafe { item.presentationSize() };
        // SAFETY: `item` is a live `AVPlayerItem` accessed on the main thread.
        let duration = asset::seconds(unsafe { item.duration() });
        let size = (size.width >= 1.0 && size.height >= 1.0).then(|| (size.width.round() as u32, size.height.round() as u32));
        // SAFETY: `item` is a live `AVPlayerItem` accessed on the main thread.
        let asset = unsafe { item.asset() };
        Meta { duration, size, frame: 1.0 / 30.0, sound: asset::has_sound(&asset) }
    }

    fn alive(&self) -> bool {
        self.native.player.as_ref().is_some_and(|p| p.objects.is_some())
    }

    fn set_muted(&mut self, muted: bool) {
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread.
        unsafe { self.objects().player.setMuted(muted) };
    }

    fn set_volume(&mut self, volume: f32) {
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread.
        unsafe { self.objects().player.setVolume(volume) };
    }

    fn seek_to(&mut self, at: f64) {
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread with valid `CMTime` values.
        unsafe { self.objects().player.seekToTime_toleranceBefore_toleranceAfter(asset::cm(at), asset::zero(), asset::zero()) };
    }

    fn set_rate(&mut self, rate: f32) {
        let player = &self.objects().player;
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread.
        unsafe { if rate > 0.0 { player.setRate(rate) } else { player.pause() } };
    }

    fn restart(&mut self, rate: f32) {
        let player = &self.objects().player;
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread with valid `CMTime` values.
        unsafe {
            player.seekToTime_toleranceBefore_toleranceAfter(asset::zero(), asset::zero(), asset::zero());
            player.setRate(rate);
        }
    }

    fn frame(&mut self, next_id: &mut dyn FnMut() -> u64) -> (f64, Result<Option<f64>, String>) {
        let (surface, target) = self.target.unwrap();
        let p = self.native.player.as_ref().unwrap();
        let output = p.objects.as_ref().unwrap().output.as_ref().unwrap();
        // SAFETY: `output` is a live `AVPlayerItemVideoOutput` attached to this player's item.
        let item_time = unsafe { output.itemTimeForHostTime(host_seconds(target)) };
        let mut shown = item_time;
        let mut landed = Ok(None);
        // SAFETY: `output` is a live `AVPlayerItemVideoOutput` and `item_time` came from `itemTimeForHostTime`.
        if unsafe { output.hasNewPixelBufferForItemTime(item_time) }
            && let Some(buffer) =
                // SAFETY: `output` is live and `&mut shown` is a valid writable `CMTime` out-pointer.
                unsafe { output.copyPixelBufferForItemTime_itemTimeForDisplay(item_time, &mut shown) }
        {
            let id = next_id();
            // SAFETY: AVPlayerItemVideoOutput hands out finished buffers it no longer writes.
            let pushed = unsafe { PixelBuffer::from_raw(Retained::as_ptr(&buffer) as *mut _) }
                .and_then(|b| surface.push_oriented(b, id, p.orient));
            landed = pushed.map(|_| Some(asset::seconds(shown)));
        }
        (asset::seconds(item_time), landed)
    }

    fn current(&self) -> f64 {
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread.
        asset::seconds(unsafe { self.objects().player.currentTime() })
    }

    fn stopped(&mut self) -> bool {
        // SAFETY: `player` is a live `AVPlayer` accessed on the main thread.
        unsafe { self.objects().player.rate() == 0.0 }
    }
}
