//! Headless runs: a clip's facts and its picture at a virtual time, decoded synchronously on
//! the CPU, so `--snap` and `--frames` are the same every run. No player, no sound.

use super::super::{Kind, MediaSource, Meta};
use super::mf::{self, Decode, Picture};
use crate::native_surface::ByteFrame;
use windows::Win32::Media::MediaFoundation::{IMFSourceReader, MF_SOURCE_READERF_ENDOFSTREAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

pub(crate) struct Still {
    reader: IMFSourceReader,
    video: u32,
    picture: Picture,
    /// The last picture read at or before the time asked, and the one read after it: a later
    /// time reads on from them rather than seeking.
    current: Option<(f64, ByteFrame)>,
    next: Option<(f64, ByteFrame)>,
}

/// Opens `source` and reads its facts; for video, a reader for its pictures too.
pub(super) fn open(source: &MediaSource, kind: Kind) -> Result<(Option<Still>, Meta), String> {
    // SAFETY: COM on this thread for the reader; left initialised, as the thread may open more.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let lib = mf::library()?;
    let opened = mf::open(lib, &lib.resolve(source)?, kind, Decode::Cpu, false)?;
    let meta = Meta { duration: opened.duration, size: opened.picture.map(|p| p.natural), frame: opened.picture.map_or(1.0 / 30.0, |p| p.frame) };
    let still = match (opened.video, opened.picture) {
        (Some(video), Some(picture)) => Some(Still { reader: opened.reader, video, picture, current: None, next: None }),
        _ => None,
    };
    Ok((still, meta))
}

impl Still {
    /// The picture showing at clip second `at`.
    pub(super) fn frame(&mut self, at: f64) -> Result<ByteFrame, String> {
        let shows = |t: f64| t <= at + self.picture.frame * 0.5;
        let reads_on = self.current.as_ref().is_some_and(|(t, _)| shows(*t) && at - t < 1.0);
        if !reads_on {
            mf::seek(&self.reader, at)?;
            (self.current, self.next) = (None, None);
        }
        if let Some((t, _)) = &self.next {
            if !shows(*t) {
                return self.current.as_ref().map(|(_, f)| copy(f)).ok_or_else(|| "no picture at that time".into());
            }
            self.current = self.next.take();
        }
        loop {
            let read = mf::read(&self.reader, self.video)?;
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
        self.current.as_ref().map(|(_, f)| copy(f)).ok_or_else(|| "no picture at that time".into())
    }
}

fn copy(f: &ByteFrame) -> ByteFrame {
    ByteFrame { width: f.width, height: f.height, format: f.format, color: f.color, planes: f.planes.clone() }
}
