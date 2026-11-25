//! Headless runs: a clip's facts and its picture at a virtual time, decoded synchronously on
//! the CPU, so `--snap` and `--frames` are the same every run. No player, no sound.

use super::super::thumbnails::{DecodedFrame, ThumbnailPrecision};
use super::super::{Kind, MediaSource, Meta, nv12};
use super::mf::{self, Decode, Picture};
use crate::native_surface::ByteFrame;
use windows::Win32::Media::MediaFoundation::MF_SOURCE_READERF_ENDOFSTREAM;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

pub(crate) struct Still {
    reader: mf::Reader,
    video: u32,
    picture: Picture,
    /// The last picture read at or before the time asked, and the one read after it: a later
    /// time reads on from them rather than seeking.
    current: Option<(f64, ByteFrame)>,
    next: Option<(f64, ByteFrame)>,
}

/// Opens `source` and reads its facts; for video, a reader for its pictures too.
#[allow(unsafe_code)]
pub(super) fn open(source: &MediaSource, kind: Kind) -> Result<(Option<Still>, Meta), String> {
    let target = mf::Target::from_source(source)?;
    // SAFETY: COM on this thread for the reader; left initialised, as the thread may open more.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().ok();
    let lib = mf::library()?;
    let opened = mf::open(lib, lib.resolve(&target)?, kind, Decode::Cpu, false)?;
    let meta = Meta { duration: opened.duration, size: opened.picture.map(|p| p.natural), frame: opened.picture.and_then(|p| p.frame_duration), sound: opened.has_sound };
    let still = match (opened.video, opened.picture) {
        (Some(video), Some(picture)) => Some(Still { reader: opened.reader, video, picture, current: None, next: None }),
        _ => None,
    };
    Ok((still, meta))
}

impl Still {
    /// The picture showing at clip second `at` and its presentation timestamp.
    pub(super) fn frame(&mut self, at: f64) -> Result<(ByteFrame, f64), String> {
        let shows = |t: f64| t <= at + 1e-6;
        let reads_on = self.current.as_ref().is_some_and(|(t, _)| shows(*t) && at - t < 1.0);
        if !reads_on {
            mf::seek(&mut self.reader, at)?;
            (self.current, self.next) = (None, None);
        }
        if let Some((t, _)) = &self.next {
            if !shows(*t) {
                return self.current.as_ref().map(|(t, f)| (copy(f), *t)).ok_or_else(|| "no picture at that time".into());
            }
            self.current = self.next.take();
        }
        loop {
            let read = mf::read(&mut self.reader, self.video)?;
            if let Some(sample) = &read.sample {
                let frame = mf::nv12_bytes(sample, &self.picture)?;
                if !shows(read.time) && self.current.is_some() {
                    self.next = Some((read.time, frame));
                    break;
                }
                self.current = Some((read.time, frame));
            }
            if read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                break;
            }
        }
        self.current.as_ref().map(|(t, f)| (copy(f), *t)).ok_or_else(|| "no picture at that time".into())
    }
}

fn copy(f: &ByteFrame) -> ByteFrame {
    ByteFrame { width: f.width, height: f.height, format: f.format, color: f.color, planes: f.planes.clone() }
}


/// Balances `CoInitializeEx` on a thumbnail worker thread after COM objects drop.
#[allow(unsafe_code)]
struct ComGuard(bool);

#[allow(unsafe_code)]
impl ComGuard {
    fn enter() -> Self {
        // SAFETY: initialize MTA COM for this worker thread; matched in `Drop` when `is_ok()`.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

#[allow(unsafe_code)]
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: matches `CoInitializeEx` above; `ThumbnailDecoder::still` dropped first.
            unsafe { CoUninitialize() };
        }
    }
}

/// CPU Media Foundation still-frame decoder for a single thumbnail batch.
pub(in crate::video) struct ThumbnailDecoder {
    still: Still,
    duration: f64,
    // Declared last so `still` (and its `IMFSourceReader`) drops before `CoUninitialize`.
    _com: ComGuard,
}

/// Opens `source` once on the current worker thread for CPU thumbnail decoding.
pub(in crate::video) fn open_thumbnails(
    source: &MediaSource,
    headers: &[(String, String)],
) -> Result<ThumbnailDecoder, String> {
    if !headers.is_empty() {
        return Err("request headers aren't supported for clips on Windows yet".into());
    }
    let target = mf::Target::from_source(source)?;
    let com = ComGuard::enter();
    let lib = mf::library()?;
    let opened = mf::open(lib, lib.resolve(&target)?, Kind::Video, Decode::Cpu, false)?;
    let (Some(video), Some(picture)) = (opened.video, opened.picture) else {
        return Err("no playable video track".into());
    };
    let still = Still { reader: opened.reader, video, picture, current: None, next: None };
    Ok(ThumbnailDecoder { still, duration: opened.duration, _com: com })
}

impl ThumbnailDecoder {
    pub(in crate::video) fn decode(
        &mut self,
        at: f64,
        max_size: u32,
        precision: ThumbnailPrecision,
    ) -> Result<DecodedFrame, String> {
        let target = if self.duration > 0.0 { at.min(self.duration) } else { at }.max(0.0);
        let (frame, pts) = match precision {
            ThumbnailPrecision::Exact => self.decode_exact(target)?,
            ThumbnailPrecision::NearestKeyframe => self.decode_keyframe(target)?,
        };
        let (uw, uh, rgba) = nv12::byte_frame_to_rgba(&frame, self.still.picture.orient)?;
        let (nw, nh) = self.still.picture.natural;
        let (tw, th) = rune_image::scaled_dimensions(nw.max(1), nh.max(1), max_size);
        let (width, height, rgba) = rune_image::downsample_rgba(uw, uh, rgba, tw, th)
            .map_err(|e| e.to_string())?;
        Ok(DecodedFrame { width, height, rgba, actual_time: pts.max(0.0) })
    }

    fn decode_exact(&mut self, target: f64) -> Result<(ByteFrame, f64), String> {
        match self.still.frame(target) {
            Ok(decoded) => Ok(decoded),
            Err(first_err) if self.duration > 0.0 => {
                let fallback = (self.duration - self.still.picture.frame()).max(0.0);
                if let Ok(Some(decoded)) = self.seek_and_scan_last(fallback, target) {
                    return Ok(decoded);
                }
                if fallback > 0.0
                    && let Ok(Some(decoded)) = self.seek_and_scan_last(0.0, target)
                {
                    return Ok(decoded);
                }
                Err(first_err)
            }
            Err(err) => Err(err),
        }
    }

    fn seek_and_scan_last(&mut self, seek_to: f64, max_time: f64) -> Result<Option<(ByteFrame, f64)>, String> {
        mf::seek(&mut self.still.reader, seek_to)?;
        (self.still.current, self.still.next) = (None, None);
        let shows = |t: f64| t <= max_time + 1e-6;
        loop {
            let read = mf::read(&mut self.still.reader, self.still.video)?;
            if let Some(sample) = &read.sample {
                let frame = mf::nv12_bytes(sample, &self.still.picture)?;
                if !shows(read.time) && self.still.current.is_some() {
                    self.still.next = Some((read.time, frame));
                    break;
                }
                self.still.current = Some((read.time, frame));
            }
            if read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                break;
            }
        }
        Ok(self.still.current.as_ref().map(|(t, f)| (copy(f), *t)))
    }

    fn decode_keyframe(&mut self, target: f64) -> Result<(ByteFrame, f64), String> {
        (self.still.current, self.still.next) = (None, None);
        match self.read_first_after_seek(target) {
            Ok(Some(decoded)) => Ok(decoded),
            first => {
                if self.duration > 0.0 {
                    let fallback = (self.duration - self.still.picture.frame()).max(0.0);
                    if let Ok(Some(decoded)) = self.read_first_after_seek(fallback) {
                        return Ok(decoded);
                    }
                    if fallback > 0.0
                        && let Ok(Some(decoded)) = self.read_first_after_seek(0.0)
                    {
                        return Ok(decoded);
                    }
                }
                match first {
                    Err(err) => Err(err),
                    Ok(None) => Err("no picture at that time".into()),
                    Ok(Some(decoded)) => Ok(decoded),
                }
            }
        }
    }

    fn read_first_after_seek(&mut self, seek_to: f64) -> Result<Option<(ByteFrame, f64)>, String> {
        mf::seek(&mut self.still.reader, seek_to)?;
        loop {
            let read = mf::read(&mut self.still.reader, self.still.video)?;
            if let Some(sample) = &read.sample {
                let frame = mf::nv12_bytes(sample, &self.still.picture)?;
                return Ok(Some((frame, read.time)));
            }
            if read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                return Ok(None);
            }
        }
    }
}
