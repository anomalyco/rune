//! Latest-wins property values. Sources outlive views and may be shared by windows.
use std::marker::PhantomData;
use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use portable_atomic::AtomicU128;
use crate::{Color, Spring, Waker};

/// A linear, render-time track. Keep it in app state and bind it like a `Live` value.
/// Starts on first attachment to each runtime; view rebuilds retain its elapsed time.
/// No producer ticks, app wake requests or hidden-window sampling are needed.
pub struct Tween<T: LiveValue> { live: Live<T> }
impl<T: LiveValue> Clone for Tween<T> {
    fn clone(&self) -> Self { Self { live: self.live.clone() } }
}
impl<T: LiveValue> Tween<T> {
    pub fn linear(from: T, to: T, duration: std::time::Duration) -> Self {
        let mut live = Live::new(to);
        Arc::get_mut(&mut live.source).unwrap().linear = Some(Linear {
            from: from.encode().unwrap_or(0), duration: duration.as_secs_f64(),
        });
        Self { live }
    }
}
impl<T: LiveValue> From<&Tween<T>> for Binding<T> {
    fn from(tween: &Tween<T>) -> Self { Self { live: tween.live.clone(), spring: None } }
}

struct Linear { from: u128, duration: f64 }
impl Linear {
    fn sample(&self, to: u128, elapsed: f64) -> (u128, bool) {
        let t = if self.duration == 0.0 { 1.0 } else { (elapsed / self.duration).clamp(0.0, 1.0) };
        if t >= 1.0 { return (to, false); }
        if t == 0.0 { return (self.from, true); }
        let mut bits = 0;
        for lane in 0..4 {
            let a = f32::decode(self.from >> (32 * lane)) as f64;
            let b = f32::decode(to >> (32 * lane)) as f64;
            bits |= ((a * (1.0 - t) + b * t) as f32).encode().unwrap_or(0) << (32 * lane);
        }
        (bits, true)
    }
}

mod sealed { pub trait Sealed {} }

/// The small, atomically published values supported by [`Live`].
pub trait LiveValue: sealed::Sealed + Copy + Send + Sync + 'static {
    /// `None` when any lane is NaN. ±inf clamps to ±`f32::MAX` and -0.0 becomes 0.0, so producers
    /// (audio callbacks) never panic and equal values never wake.
    #[doc(hidden)] fn encode(self) -> Option<u128>;
    #[doc(hidden)] fn decode(bits: u128) -> Self;
}
impl sealed::Sealed for f32 {}
impl LiveValue for f32 {
    fn encode(self) -> Option<u128> {
        if self.is_nan() { return None; }
        let x = if self == 0.0 { 0.0 } else { self.clamp(-f32::MAX, f32::MAX) };
        Some(x.to_bits() as u128)
    }
    fn decode(bits: u128) -> Self { f32::from_bits(bits as u32) }
}
impl sealed::Sealed for [f32; 2] {}
impl LiveValue for [f32; 2] {
    fn encode(self) -> Option<u128> { Some(self[0].encode()? | (self[1].encode()? << 32)) }
    fn decode(bits: u128) -> Self { [f32::decode(bits), f32::decode(bits >> 32)] }
}
impl sealed::Sealed for Color {}
impl sealed::Sealed for [f32; 4] {}
impl LiveValue for [f32; 4] {
    fn encode(self) -> Option<u128> {
        Some(self[0].encode()? | (self[1].encode()? << 32) | (self[2].encode()? << 64) | (self[3].encode()? << 96))
    }
    fn decode(bits: u128) -> Self {
        [f32::decode(bits), f32::decode(bits >> 32), f32::decode(bits >> 64), f32::decode(bits >> 96)]
    }
}
impl LiveValue for Color {
    fn encode(self) -> Option<u128> {
        Some(self.r.encode()? | (self.g.encode()? << 32) | (self.b.encode()? << 64) | (self.a.encode()? << 96))
    }
    fn decode(bits: u128) -> Self {
        Self { r: f32::decode(bits), g: f32::decode(bits >> 32), b: f32::decode(bits >> 64), a: f32::decode(bits >> 96) }
    }
}

/// Cloneable, thread-safe, coherent latest value. Equal bit patterns do not wake a window.
/// NaN is ignored (`set` returns false; `new` starts at zero), ±inf clamps to ±`f32::MAX`. Opacity is clamped by presentation; scale must be nonnegative.
/// Publication is lock-free on supported desktop CPUs. Window notification is coalesced;
/// its platform callback is not a hard-real-time API.
pub struct Live<T: LiveValue> { pub(crate) source: Arc<Source>, _t: PhantomData<T> }
impl<T: LiveValue> Clone for Live<T> {
    fn clone(&self) -> Self { Self { source: self.source.clone(), _t: PhantomData } }
}
impl<T: LiveValue> Live<T> {
    pub fn new(value: T) -> Self {
        assert!(AtomicU128::is_lock_free(), "Rune Live requires lock-free 128-bit atomics on this CPU");
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self { source: Arc::new(Source {
            id: NEXT.fetch_add(1, Ordering::Relaxed), value: AtomicU128::new(value.encode().unwrap_or(0)),
            listeners: AtomicPtr::new(std::ptr::null_mut()),
            linear: None,
        }), _t: PhantomData }
    }
    /// Returns whether the published bit pattern changed. No allocation, including first use
    /// on a producer thread. Concurrent producers publish whole values, never torn lanes.
    pub fn set(&self, value: T) -> bool {
        let Some(bits) = value.encode() else { return false };
        if self.source.value.swap(bits, Ordering::AcqRel) == bits { return false; }
        self.source.notify();
        true
    }
    pub fn get(&self) -> T { T::decode(self.source.value.load(Ordering::Acquire)) }
    /// Smooth this binding with an interruptible rune-motion spring; the source stays 1:1.
    pub fn smooth(&self, spring: Spring) -> Binding<T> {
        Binding { live: self.clone(), spring: Some(spring) }
    }
}

pub struct Binding<T: LiveValue> { live: Live<T>, spring: Option<Spring> }
impl<T: LiveValue> From<&Live<T>> for Binding<T> {
    fn from(live: &Live<T>) -> Self { Self { live: live.clone(), spring: None } }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Property { Opacity, Offset, Scale, Fill, Rect }
pub(crate) struct Bound {
    pub source: Arc<Source>,
    pub property: Property,
    pub spring: Option<Spring>,
    pub seen: Option<u128>,
    pub started: Option<f64>,
}
impl Bound {
    pub fn new<T: LiveValue>(binding: impl Into<Binding<T>>, property: Property) -> Self {
        let b = binding.into();
        Self { source: b.live.source, property, spring: b.spring, seen: None, started: None }
    }
    pub fn attach(&mut self, now: f64) {
        if self.source.linear.is_some() { self.started.get_or_insert(now); }
    }
    pub fn read(&mut self, now: f64) -> (Option<u128>, bool) {
        let bits = self.source.value.load(Ordering::Acquire);
        let (bits, active) = match &self.source.linear {
            Some(linear) => linear.sample(bits, now - *self.started.get_or_insert(now)),
            None => (bits, false),
        };
        if self.seen == Some(bits) { return (None, active); }
        self.seen = Some(bits);
        (Some(bits), active)
    }
}

pub(crate) struct Wake {
    pub pending: AtomicBool,
    visible: AtomicBool,
    callback: Waker,
}
impl Wake {
    pub fn new(callback: Waker) -> Arc<Self> {
        Arc::new(Self { pending: AtomicBool::new(false), visible: AtomicBool::new(true), callback })
    }
    pub fn visible(&self, visible: bool) {
        let was_visible = self.visible.swap(visible, Ordering::AcqRel);
        if visible && !was_visible && self.pending.load(Ordering::Acquire) { (self.callback)(); }
    }
    fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) && self.visible.load(Ordering::Acquire) {
            (self.callback)();
        }
    }
}

pub(crate) struct Subscription {
    pub source_id: u64,
    pub active: AtomicBool,
    wake: Weak<Wake>,
}
struct Listener { subscription: Arc<Subscription>, next: *mut Listener }

pub(crate) struct Source {
    linear: Option<Linear>,
    id: u64,
    value: AtomicU128,
    listeners: AtomicPtr<Listener>,
}
impl Source {
    // Append-only immutable list: no reclamation can race a publisher. One cell per
    // source/runtime pair (not per view rebuild); all cells reclaimed with the source.
    pub fn subscribe(&self, wake: &Arc<Wake>) -> Arc<Subscription> {
        let mut head = self.listeners.load(Ordering::Acquire);
        let mut p = head;
        while !p.is_null() {
            // SAFETY: nodes are published with release and only freed by exclusive Drop.
            let node = unsafe { &*p };
            if node.subscription.wake.ptr_eq(&Arc::downgrade(wake)) {
                node.subscription.active.store(true, Ordering::Release);
                return node.subscription.clone();
            }
            p = node.next;
        }
        let subscription = Arc::new(Subscription { source_id: self.id, active: AtomicBool::new(true), wake: Arc::downgrade(wake) });
        let p = Box::into_raw(Box::new(Listener { subscription: subscription.clone(), next: head }));
        loop {
            match self.listeners.compare_exchange_weak(head, p, Ordering::Release, Ordering::Acquire) {
                Ok(_) => return subscription,
                Err(current) => {
                    head = current;
                    // SAFETY: p is unpublished, owned exclusively by this call.
                    unsafe { (*p).next = head; }
                }
            }
        }
    }
    fn notify(&self) {
        let mut p = self.listeners.load(Ordering::Acquire);
        while !p.is_null() {
            // SAFETY: this source is alive through the caller's Arc; list is immutable.
            let node = unsafe { &*p };
            if node.subscription.active.load(Ordering::Acquire)
                && let Some(wake) = node.subscription.wake.upgrade() { wake.notify(); }
            p = node.next;
        }
    }
    pub fn id(&self) -> u64 { self.id }
}
impl Drop for Source {
    fn drop(&mut self) {
        let mut p = *self.listeners.get_mut();
        while !p.is_null() {
            // SAFETY: last source Arc is gone, so no publisher/subscriber can access it.
            let node = unsafe { Box::from_raw(p) };
            p = node.next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn concurrent_producers_never_tear_color_lanes() {
        let live = Live::new(Color::WHITE);
        std::thread::scope(|scope| {
            for writer in 0..4 {
                let live = &live;
                scope.spawn(move || {
                    for i in 0..20_000 {
                        let n = (i * 4 + writer) as f32;
                        live.set(Color { r: n, g: n, b: n, a: n });
                    }
                });
            }
            for _ in 0..80_000 {
                let c = live.get();
                assert_eq!([c.r; 3], [c.g, c.b, c.a]);
            }
        });
    }

    #[test]
    fn subscriptions_coalesce_and_respect_visibility_and_lifetime() {
        let live = Live::new(0.0);
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let wake = Wake::new(Arc::new(move || { c.fetch_add(1, Ordering::Relaxed); }));
        let subscription = live.source.subscribe(&wake);
        assert!(Arc::ptr_eq(&subscription, &live.source.subscribe(&wake)));
        assert!(!live.set(0.0));
        for n in 1..100 { live.set(n as f32); }
        assert_eq!(count.load(Ordering::Relaxed), 1);
        wake.pending.store(false, Ordering::Release);
        wake.visible(false);
        live.set(100.0);
        assert_eq!(count.load(Ordering::Relaxed), 1);
        wake.visible(true);
        assert_eq!(count.load(Ordering::Relaxed), 2);
        wake.pending.store(false, Ordering::Release);
        subscription.active.store(false, Ordering::Release);
        live.set(101.0);
        assert!(!wake.pending.load(Ordering::Acquire));
        subscription.active.store(true, Ordering::Release);
        drop(wake);
        live.set(102.0);
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}

#[cfg(test)]
mod nonfinite_tests {
    use super::*;

    #[test]
    fn non_finite_values_never_panic() {
        let v = Live::new(0.5f32);
        assert!(!v.set(f32::NAN));
        assert_eq!(v.get(), 0.5);
        assert!(v.set(f32::INFINITY));
        assert_eq!(v.get(), f32::MAX);
        assert!(v.set(f32::NEG_INFINITY));
        assert_eq!(v.get(), -f32::MAX);
        assert!(v.set(0.0));
        assert!(!v.set(-0.0), "-0.0 equals 0.0 and must not wake");
        assert_eq!(v.get().to_bits(), 0.0f32.to_bits());
        assert_eq!(Live::new(f32::NAN).get(), 0.0);
        let p = Live::new([1.0f32, 2.0]);
        assert!(!p.set([f32::NAN, 3.0]), "a NaN lane rejects the whole value");
        assert_eq!(p.get(), [1.0, 2.0]);
        let r = Live::new([0.0f32; 4]);
        assert!(r.set([0.0, 0.0, f32::INFINITY, 1.0]));
        assert_eq!(r.get(), [0.0, 0.0, f32::MAX, 1.0]);
        let c = Live::new(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
        assert!(!c.set(Color { r: f32::NAN, g: 0.0, b: 0.0, a: 1.0 }));
        assert_eq!(c.get().r, 1.0);
    }
}
