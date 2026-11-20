//! Submission: values on the shared event, the held frame buffer, staged writes, retirement of
//! what command buffers used, and the callbacks that wait on it.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;

#[cfg(debug_assertions)]
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue, MTLDrawable, MTLEvent, MTLSharedEvent,
};
use objc2_quartz_core::CAMetalDrawable;
use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use parking_lot::MutexGuard;
use wgpu::custom::*;

use super::encode::{MetalCommandBuffer, buffer_to_texture};
use super::resource::{BufferRaw, MetalBuffer, MetalTexture};
use super::surface::MetalOutput;
use super::{Obj, Shared};

/// What a command buffer uses, kept alive until it completes.
pub(super) type Keep = Vec<Arc<dyn Any + Send + Sync>>;

/// Frames the CPU may run ahead of the GPU.
const IN_FLIGHT: usize = 2;

/// Emptied keep lists kept for the next encoders, so a steady frame allocates none.
const SPARE_KEEPS: usize = 8;

enum Callback {
    Map(BufferMapCallback),
    Done(BoxSubmittedWorkDoneCallback),
}

/// A committed command buffer and the resources it references without retaining.
struct Pending {
    value: u64,
    cb: Obj<dyn MTLCommandBuffer>,
    keep: Keep,
    drawable: Option<Obj<dyn CAMetalDrawable>>,
}

/// A frame's command buffer waiting for `present` so `presentDrawable` lands on the same buffer.
struct Held {
    cb: Obj<dyn MTLCommandBuffer>,
    value: u64,
    keep: Keep,
}

#[derive(Default)]
pub(crate) struct State {
    /// The value the last submitted command buffer signals when it completes.
    pub(super) last: u64,
    /// The highest value whose command buffer has reached `Completed` or `Error` and been retired.
    retired: u64,
    /// A frame's command buffer waiting for its drawable's present. While one is held it is the
    /// last assigned value, and every other commit commits it first.
    held: Option<Held>,
    uploads: Option<Uploads>,
    retire: VecDeque<Pending>,
    callbacks: VecDeque<(u64, Callback)>,
    frames: VecDeque<u64>,
    pub(super) spare: Vec<Keep>,
}

/// Writes staged since the last submit, committed ahead of it.
struct Uploads {
    blit: Obj<dyn MTLBlitCommandEncoder>,
    cb: Obj<dyn MTLCommandBuffer>,
    keep: Keep,
}

fn completed(cb: &ProtocolObject<dyn MTLCommandBuffer>) -> bool {
    matches!(cb.status(), MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error)
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").field("last", &self.last).field("held", &self.held.is_some()).finish_non_exhaustive()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> { self.state.lock() }

    /// A command buffer that doesn't retain what it uses. Made in a pool of its own, so the
    /// buffer is freed when its last owner here drops it rather than when the thread's pool drains.
    pub(super) fn command_buffer(&self) -> Obj<dyn MTLCommandBuffer> {
        autoreleasepool(|_| self.queue.commandBufferWithUnretainedReferences()).expect("a Metal command buffer")
    }

    /// An empty keep list, reusing one a retired command buffer emptied.
    pub(super) fn keep_list(&self) -> Keep {
        self.lock().spare.pop().unwrap_or_else(|| Vec::with_capacity(32))
    }

    /// Empties `keep`, releasing what it held, and saves it for a later encoder.
    pub(super) fn recycle(&self, mut keep: Keep) {
        keep.clear();
        if keep.capacity() == 0 { return }
        let mut st = self.lock();
        if st.spare.len() < SPARE_KEEPS { st.spare.push(keep); }
    }

    /// Signals `value` on the shared event, commits `cb`, and tracks `cb` with its unretained
    /// resources until Metal marks `cb` completed.
    fn commit_pending(
        &self,
        st: &mut State,
        value: u64,
        cb: Obj<dyn MTLCommandBuffer>,
        keep: Keep,
        drawable: Option<Obj<dyn CAMetalDrawable>>,
    ) {
        cb.encodeSignalEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*self.event), value);
        cb.commit();
        st.retire.push_back(Pending { value, cb, keep, drawable });
    }

    fn flush_held(&self, st: &mut State) {
        if let Some(Held { cb, value, keep }) = st.held.take() {
            self.commit_pending(st, value, cb, keep, None);
        }
    }

    /// Commits a held buffer whose drawable was dropped or never presented.
    pub(super) fn flush(&self) {
        let mut st = self.lock();
        self.flush_held(&mut st);
    }

    fn commit_uploads(&self, st: &mut State) {
        let Some(Uploads { blit, cb, keep }) = st.uploads.take() else { return };
        blit.endEncoding();
        st.last += 1;
        let value = st.last;
        self.commit_pending(st, value, cb, keep, None);
    }

    /// Encodes a staged write into the blit buffer the next submit commits first.
    pub(super) fn upload(
        &self,
        encode: impl FnOnce(&ProtocolObject<dyn MTLBlitCommandEncoder>),
        keep: impl IntoIterator<Item = Arc<dyn Any + Send + Sync>>,
    ) {
        let mut st = self.lock();
        let State { uploads, spare, .. } = &mut *st;
        let uploads = uploads.get_or_insert_with(|| {
            let cb = self.command_buffer();
            let blit = autoreleasepool(|_| cb.blitCommandEncoder()).expect("a Metal blit encoder");
            #[cfg(debug_assertions)]
            {
                cb.setLabel(Some(&NSString::from_str("rune metal uploads")));
                blit.setLabel(Some(&NSString::from_str("rune metal uploads blit")));
            }
            let keep = spare.pop().unwrap_or_else(|| Vec::with_capacity(8));
            Uploads { blit, cb, keep }
        });
        encode(&uploads.blit);
        uploads.keep.extend(keep);
    }

    fn after(&self, callback: Callback) {
        let mut st = self.lock();
        let at = st.last;
        st.callbacks.push_back((at, callback));
    }

    /// Releases what completed command buffers used and runs the callbacks waiting on them.
    /// One at a time, outside the lock: callbacks may call back into the device. Returns the
    /// value it retired up to.
    pub(super) fn retire(&self) -> u64 { self.retire_inner(false) }

    /// Retires completed command buffers from the front of `st.retire`. When `check_unsignaled`
    /// is false, skips `cb.status()` on command buffers whose shared-event value has not yet
    /// signaled (a command buffer cannot complete before its signal); after `waitUntilCompleted`
    /// it checks every entry so an errored buffer that skipped its signal still retires.
    fn retire_inner(&self, check_unsignaled: bool) -> u64 {
        let signaled = if check_unsignaled { u64::MAX } else { self.event.signaledValue() };
        let mut emptied: Option<Keep> = None;
        let (done, has_callbacks) = loop {
            let pending = {
                let mut st = self.lock();
                if let Some(keep) = emptied.take()
                    && st.spare.len() < SPARE_KEEPS
                {
                    st.spare.push(keep);
                }
                if st.retire.front().is_some_and(|p| p.value <= signaled && completed(&p.cb)) {
                    if let Some(p) = st.retire.pop_front() {
                        st.retired = st.retired.max(p.value);
                        Some(p)
                    } else {
                        None
                    }
                } else {
                    let done = st.retired;
                    let has_cb = st.callbacks.front().is_some_and(|(at, _)| *at <= done);
                    break (done, has_cb);
                }
            };
            let Some(Pending { cb, mut keep, drawable, .. }) = pending else { continue };
            // Drop the completed command buffer before any object it referenced without retaining.
            drop(cb);
            drop(drawable);
            keep.clear();
            if keep.capacity() > 0 { emptied = Some(keep); }
        };
        if has_callbacks {
            loop {
                let next = {
                    let mut st = self.lock();
                    if st.callbacks.front().is_some_and(|(at, _)| *at <= done) { st.callbacks.pop_front() } else { None }
                };
                match next {
                    Some((_, Callback::Map(f))) => f(Ok(())),
                    Some((_, Callback::Done(f))) => f(),
                    None => break,
                }
            }
        }
        done
    }

    pub(super) fn poll(&self, poll: wgpu::wgt::PollType<u64>) -> Result<wgpu::PollStatus, wgpu::PollError> {
        let wgpu::wgt::PollType::Wait { submission_index, timeout } = poll else {
            // Empty only as of the value retire ran callbacks up to: work finishing after it
            // still has callbacks to run, so the caller polls again.
            let done = self.retire();
            let st = self.lock();
            let idle = done >= st.last && st.retire.is_empty() && st.held.is_none() && st.callbacks.is_empty();
            return Ok(if idle { wgpu::PollStatus::QueueEmpty } else { wgpu::PollStatus::Poll });
        };
        let last = {
            let mut st = self.lock();
            self.flush_held(&mut st);
            st.last
        };
        let target = submission_index.unwrap_or(last);
        if target > last { return Err(wgpu::PollError::WrongSubmissionIndex(target, last)); }
        if let Some(t) = timeout {
            if t.is_zero() {
                let done = self.retire();
                if done < target { return Err(wgpu::PollError::Timeout); }
                let st = self.lock();
                let empty = st.retire.is_empty() && st.held.is_none() && st.callbacks.is_empty();
                return Ok(if empty { wgpu::PollStatus::QueueEmpty } else { wgpu::PollStatus::WaitSucceeded });
            }
            let ms = u64::try_from(t.as_millis()).unwrap_or(u64::MAX).max(1);
            if self.event.signaledValue() < target && !self.event.waitUntilSignaledValue_timeoutMS(target, ms) {
                return Err(wgpu::PollError::Timeout);
            }
        }
        // Wait until Metal marks every command buffer through `target` completed: the shared event
        // signals on the GPU before Metal finishes completing the command buffer, so retiring or
        // dropping resources on the event alone frees them while the command buffer still tracks them.
        loop {
            let cb = {
                let st = self.lock();
                st.retire.iter().find(|p| p.value <= target && !completed(&p.cb)).map(|p| p.cb.clone())
            };
            let Some(cb) = cb else { break };
            cb.waitUntilCompleted();
        }
        self.retire_inner(true);
        let st = self.lock();
        let empty = st.retire.is_empty() && st.held.is_none() && st.callbacks.is_empty();
        Ok(if empty { wgpu::PollStatus::QueueEmpty } else { wgpu::PollStatus::WaitSucceeded })
    }

    /// Waits until at most `IN_FLIGHT - 1` presented frames are still on the GPU.
    pub(super) fn pace(&self) {
        let oldest = {
            let mut st = self.lock();
            if st.frames.len() < IN_FLIGHT { return }
            st.frames.pop_front()
        };
        if let Some(at) = oldest.filter(|at| self.event.signaledValue() < *at) {
            self.event.waitUntilSignaledValue_timeoutMS(at, 1000);
        }
    }

    pub(super) fn map_after_work(&self, callback: BufferMapCallback) { self.after(Callback::Map(callback)); }

    /// Stamps and commits a buffer recorded outside an encoder, as a submit would.
    pub(super) fn commit(&self, cb: Obj<dyn MTLCommandBuffer>) {
        {
            let mut st = self.lock();
            self.flush_held(&mut st);
            self.commit_uploads(&mut st);
            st.last += 1;
            let value = st.last;
            self.commit_pending(&mut st, value, cb, Vec::new(), None);
        }
        self.retire();
    }

    /// Pending command buffers waiting for completion, for tests.
    #[cfg(test)]
    pub(super) fn pending_cbs(&self) -> Vec<(u64, Obj<dyn MTLCommandBuffer>)> {
        self.lock().retire.iter().map(|p| (p.value, p.cb.clone())).collect()
    }
}

impl Drop for Shared {
    /// Runs what was recorded and waits for Metal to complete every command buffer before
    /// releasing their resources and the device's own buffers, since command buffers don't
    /// retain what they use.
    fn drop(&mut self) {
        {
            let mut st = self.lock();
            self.flush_held(&mut st);
            self.commit_uploads(&mut st);
        }
        loop {
            let cb = {
                let st = self.lock();
                st.retire.iter().find(|p| !completed(&p.cb)).map(|p| p.cb.clone())
            };
            let Some(cb) = cb else { break };
            cb.waitUntilCompleted();
        }
        self.retire_inner(true);
    }
}

#[derive(Debug)]
pub(crate) struct MetalQueue(pub(super) Arc<Shared>);

impl QueueInterface for MetalQueue {
    #[allow(unsafe_code)]
    fn write_buffer(&self, buffer: &DispatchBuffer, offset: wgpu::BufferAddress, data: &[u8]) {
        if data.is_empty() { return }
        let store = &buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").store;
        // Written now if nothing recorded uses the storage, or into a spare if only submitted
        // work does: an encoder recorded earlier and submitted later must read these bytes too.
        // Staged, it pins the storage, since a later swap would copy the rest without this write.
        // An encoder counts in `recording` before it reads any storage, which it does under the
        // store's lock, the one `may_swap` is asked under.
        let idle = || self.0.recording.load(std::sync::atomic::Ordering::Acquire) == 0;
        if store.write_ahead(&self.0, offset, data, idle) { return }
        let target = store.gpu_write();
        let staging = Arc::new(BufferRaw::with_bytes(&self.0, data, Some("rune metal write_buffer")));
        self.0.upload(|blit| {
            // SAFETY: both ranges are in bounds; wgpu checked the target's.
            unsafe { blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(&staging.raw, 0, &target.raw, offset as usize, data.len()) }
        }, [staging.clone() as _, target.clone() as _]);
    }
    fn create_staging_buffer(&self, _: wgpu::BufferSize) -> Option<DispatchQueueWriteBuffer> { None }
    fn validate_write_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: wgpu::BufferSize) -> Option<()> { Some(()) }
    fn write_staging_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchQueueWriteBuffer) {
        super::unsupported!("Queue::write_buffer_with")
    }
    fn write_texture(&self, dst: wgpu::TexelCopyTextureInfo<'_>, data: &[u8], layout: wgpu::TexelCopyBufferLayout, size: wgpu::Extent3d) {
        let data = &data[layout.offset as usize..];
        if data.is_empty() || size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let texture = dst.texture.as_custom::<MetalTexture>().expect("a Metal texture").raw.clone();
        let staging = Arc::new(BufferRaw::with_bytes(&self.0, data, Some("rune metal write_texture")));
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, ..layout };
        self.0.upload(|blit| buffer_to_texture(blit, &staging.raw, layout, &texture, dst.mip_level, dst.origin, size), [staging.clone() as _, texture.clone() as _]);
    }
    fn submit(&self, command_buffers: &mut dyn Iterator<Item = DispatchCommandBuffer>) -> u64 {
        let last = {
            let mut st = self.0.lock();
            self.0.flush_held(&mut st);
            self.0.commit_uploads(&mut st);
            for buffer in command_buffers {
                let Some(recorded) = buffer.as_custom::<MetalCommandBuffer>().and_then(MetalCommandBuffer::take) else { continue };
                self.0.flush_held(&mut st);
                let drawable = recorded.drawable;
                let (cb, keep) = recorded.close();
                st.last += 1;
                let value = st.last;
                if drawable {
                    st.held = Some(Held { cb, value, keep });
                } else {
                    self.0.commit_pending(&mut st, value, cb, keep, None);
                }
            }
            st.last
        };
        self.0.retire();
        last
    }
    fn get_timestamp_period(&self) -> f32 { 1.0 }
    fn on_submitted_work_done(&self, callback: BoxSubmittedWorkDoneCallback) { self.0.after(Callback::Done(callback)) }
    fn compact_blas(&self, _: &DispatchBlas) -> (Option<u64>, DispatchBlas) { super::unsupported!("ray tracing") }
    fn present(&self, detail: &DispatchSurfaceOutputDetail) {
        let Some(output) = detail.as_custom::<MetalOutput>() else { return };
        let Some(drawable) = &output.drawable else { return };
        let mtl_drawable = ProtocolObject::<dyn MTLDrawable>::from_ref(&**drawable);
        let wait_cb = {
            let mut st = self.0.lock();
            let Held { cb, value, keep } = match st.held.take() {
                Some(held) => held,
                None => {
                    let cb = self.0.command_buffer();
                    #[cfg(debug_assertions)]
                    cb.setLabel(Some(&NSString::from_str("rune metal present")));
                    st.last += 1;
                    Held { cb, value: st.last, keep: Vec::new() }
                }
            };
            if !output.transaction {
                cb.presentDrawable(mtl_drawable);
            }
            let wait_cb = output.transaction.then(|| cb.clone());
            self.0.commit_pending(&mut st, value, cb, keep, Some(drawable.clone()));
            st.frames.push_back(value);
            wait_cb
        };
        if let Some(cb) = wait_cb {
            cb.waitUntilScheduled();
            mtl_drawable.present();
        }
    }
}
