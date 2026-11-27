//! Pipeline caches (`Features::PIPELINE_CACHE`): an `MTLBinaryArchive` holding the GPU code of
//! the pipelines made with the cache. A pipeline the archive holds is made from it without
//! compiling for the GPU; one it lacks is compiled into it first and made from there, so a miss
//! compiles once. Metal still compiles each shader's MSL to AIR when the pipeline is made (the
//! system's shader cache usually answers that); the archive saves the compile from AIR to the
//! GPU's code, per pipeline.
//!
//! `PipelineCache::get_data` serializes the archive behind a header naming the GPU and the
//! macOS build. Data for another GPU, OS build or format, or that Metal refuses, starts an empty
//! archive: stale data costs a compile, never an error. Metal reads and writes archives only
//! through files, so loading and `get_data` go through a temporary file each: make and save
//! caches off the frame.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray, NSError, NSProcessInfo, NSString, NSURL};
use objc2_metal::{MTLBinaryArchive, MTLBinaryArchiveDescriptor, MTLDevice, MTLPipelineOption};
use parking_lot::Mutex;
use wgpu::custom::PipelineCacheInterface;

use super::{Obj, send_sync};

/// What a cache's data starts with, then the 32-byte key of the GPU and OS build it was made on.
const MAGIC: &[u8; 8] = b"RUNEMPC1";

#[derive(Debug)]
pub(crate) struct PipelineCache {
    /// None only if Metal refused even an empty archive: pipelines are then made without one.
    archive: Option<Obj<dyn MTLBinaryArchive>>,
    /// The file the archive was loaded from, removed with the cache.
    file: Option<PathBuf>,
    key: [u8; 32],
    /// What `get_data` returns until a pipeline adds to the archive; held while adding.
    data: Mutex<Option<Vec<u8>>>,
    /// Pipelines the archive lacked, for tests.
    #[cfg(test)]
    pub(super) misses: AtomicU64,
}
send_sync!(PipelineCache);

/// The GPU and macOS build a cache's GPU code is for.
fn key(device: &ProtocolObject<dyn MTLDevice>) -> [u8; 32] {
    let os = NSProcessInfo::processInfo().operatingSystemVersionString();
    *blake3::hash(format!("{}\n{os}", device.name()).as_bytes()).as_bytes()
}

/// A fresh temporary file path.
fn temp() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("rune-pipeline-cache-{}-{}.metalar", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn url(path: &std::path::Path) -> Option<Retained<NSURL>> {
    Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
}

/// An archive over `bytes`, through the file it then reads from; None if Metal refuses them.
fn load(device: &ProtocolObject<dyn MTLDevice>, bytes: &[u8]) -> Option<(Obj<dyn MTLBinaryArchive>, PathBuf)> {
    let path = temp();
    std::fs::write(&path, bytes).ok()?;
    let desc = MTLBinaryArchiveDescriptor::new();
    desc.setUrl(url(&path).as_deref());
    match device.newBinaryArchiveWithDescriptor_error(&desc) {
        Ok(archive) => Some((archive, path)),
        Err(_) => {
            std::fs::remove_file(&path).ok();
            None
        }
    }
}

impl PipelineCache {
    /// The cache a pipeline's descriptor names, when it's this backend's.
    pub(super) fn of(cache: Option<&wgpu::PipelineCache>) -> Option<&PipelineCache> {
        cache.and_then(|cache| cache.as_custom::<PipelineCache>())
    }

    pub(super) fn new(device: &ProtocolObject<dyn MTLDevice>, desc: &wgpu::PipelineCacheDescriptor<'_>) -> Self {
        let key = key(device);
        let archive = desc.data.and_then(|data| data.strip_prefix(MAGIC.as_slice())?.strip_prefix(key.as_slice()));
        if let Some((archive, file)) = archive.and_then(|bytes| load(device, bytes)) {
            let data = desc.data.map(<[u8]>::to_vec);
            return PipelineCache { archive: Some(archive), file: Some(file), key, data: Mutex::new(data), #[cfg(test)] misses: AtomicU64::new(0) };
        }
        let archive = device.newBinaryArchiveWithDescriptor_error(&MTLBinaryArchiveDescriptor::new()).ok();
        PipelineCache { archive, file: None, key, data: Mutex::new(None), #[cfg(test)] misses: AtomicU64::new(0) }
    }

    /// Makes a pipeline through the archive: `archives` points the pipeline's descriptor at it,
    /// `make` makes the pipeline with Metal's options, and `add` puts its GPU code in the archive.
    /// A miss compiles as without a cache, on the calling thread, then adds what Metal compiled.
    pub(super) fn make<T>(&self, archives: impl FnOnce(&NSArray<ProtocolObject<dyn MTLBinaryArchive>>),
        add: impl FnOnce(&ProtocolObject<dyn MTLBinaryArchive>) -> bool,
        make: impl Fn(MTLPipelineOption) -> Result<T, Retained<NSError>>) -> Result<T, Retained<NSError>> {
        let Some(archive) = &self.archive else { return make(MTLPipelineOption::empty()) };
        archives(&NSArray::from_retained_slice(std::slice::from_ref(archive)));
        if let Ok(made) = make(MTLPipelineOption::FailOnBinaryArchiveMiss) { return Ok(made) }
        #[cfg(test)]
        self.misses.fetch_add(1, Ordering::Relaxed);
        let made = make(MTLPipelineOption::empty())?;
        let mut data = self.data.lock();
        if add(archive) { *data = None; }
        Ok(made)
    }
}

impl PipelineCacheInterface for PipelineCache {
    fn get_data(&self) -> Option<Vec<u8>> {
        let archive = self.archive.as_ref()?;
        let mut data = self.data.lock();
        if data.is_none() {
            let path = temp();
            let bytes = url(&path).and_then(|url| archive.serializeToURL_error(&url).ok()).and_then(|()| std::fs::read(&path).ok());
            std::fs::remove_file(&path).ok();
            *data = bytes.map(|bytes| [MAGIC.as_slice(), &self.key, &bytes].concat());
        }
        data.clone()
    }
}

impl Drop for PipelineCache {
    fn drop(&mut self) {
        if let Some(file) = &self.file { std::fs::remove_file(file).ok(); }
    }
}
