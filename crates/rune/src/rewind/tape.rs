//! The recording: one entry per presented frame, each the elements whose rect or opacity changed
//! since the frame before, in a ring that drops frames older than the window (or over its byte
//! budget) into a base state, so any kept frame can be rebuilt.

use std::collections::VecDeque;

use crate::runtime::HashMap;

/// An element's presented state, quantized: x, y, w, h in hundredths of a point and opacity in
/// thousandths.
pub(crate) type Q = [i32; 5];

pub(crate) const FIELDS: usize = 5;
/// The delta header's low bits: which fields follow. Zero says the element went away.
const MASK_BITS: u32 = 5;
/// Elements known before the first compaction (`Tape::compact`).
const COMPACT_AT: usize = 4096;

pub(crate) fn quantize(rect: [f32; 4], opacity: f32) -> Q {
    let q = |v: f32, k: f32| (v * k).round().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    [q(rect[0], 100.0), q(rect[1], 100.0), q(rect[2], 100.0), q(rect[3], 100.0), q(opacity, 1000.0)]
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Mark {
    /// Seconds on the window's clock.
    pub t: f64,
    /// Words this frame's deltas take in `words`.
    pub len: u32,
}

pub(crate) struct Tape {
    pub window: f64,
    pub max_bytes: usize,
    pub paused: bool,
    pub viewport: (f32, f32),
    ids: HashMap<u64, u32>,
    pub names: Vec<String>,
    name_bytes: usize,
    /// Per id: its state as last recorded, and the frame serial it was last seen in.
    last: Vec<Q>,
    seen: Vec<u32>,
    present: Vec<u32>,
    next_present: Vec<u32>,
    serial: u32,
    open: Option<(f64, usize)>,
    pub frames: VecDeque<Mark>,
    pub words: VecDeque<u32>,
    /// Each id's state just before the oldest kept frame (None: not on screen).
    pub base: Vec<Option<Q>>,
    /// Known elements past which the next frame's end forgets those nothing kept refers to.
    compact_at: usize,
}

impl Tape {
    pub(crate) fn new(window: f64, max_bytes: usize) -> Tape {
        Tape {
            window, max_bytes, paused: false, viewport: (0.0, 0.0),
            ids: HashMap::default(), names: Vec::new(), name_bytes: 0,
            last: Vec::new(), seen: Vec::new(), present: Vec::new(), next_present: Vec::new(), serial: 0,
            open: None, frames: VecDeque::new(), words: VecDeque::new(), base: Vec::new(), compact_at: COMPACT_AT,
        }
    }

    /// Starts a frame presented at `t` seconds.
    pub(crate) fn begin(&mut self, t: f64, viewport: (f32, f32)) {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.viewport = viewport;
        self.open = Some((t, self.words.len()));
        self.next_present.clear();
    }

    /// One element as this frame presents it. `name` runs only the first time `node` is seen.
    pub(crate) fn sample(&mut self, node: u64, q: Q, name: impl FnOnce() -> String) {
        let id = match self.ids.get(&node) {
            Some(&id) => id,
            None => {
                let id = self.names.len() as u32;
                let name = name();
                self.name_bytes += name.len();
                self.names.push(name);
                self.ids.insert(node, id);
                self.last.push([i32::MIN; FIELDS]);
                self.seen.push(0);
                self.base.push(None);
                id
            }
        };
        let i = id as usize;
        if self.seen[i] == self.serial { return; }
        // On screen last frame: sampled in the frame before this one.
        let was = self.seen[i] != 0 && self.seen[i] == self.serial.wrapping_sub(1);
        self.seen[i] = self.serial;
        self.next_present.push(id);
        let old = self.last[i];
        let mut mask = 0u32;
        for f in 0..FIELDS {
            if !was || old[f] != q[f] { mask |= 1 << f; }
        }
        if mask == 0 { return; }
        self.words.push_back(id << MASK_BITS | mask);
        for f in 0..FIELDS {
            if mask & (1 << f) != 0 { self.words.push_back(q[f] as u32); }
        }
        self.last[i] = q;
    }

    /// Ends the frame: elements on screen last frame and not this one went away.
    pub(crate) fn end(&mut self) {
        let Some((t, start)) = self.open.take() else { return };
        for &id in &self.present {
            if self.seen[id as usize] != self.serial {
                self.words.push_back(id << MASK_BITS);
            }
        }
        std::mem::swap(&mut self.present, &mut self.next_present);
        let len = (self.words.len() - start) as u32;
        self.frames.push_back(Mark { t, len });
        self.evict(t);
        if self.names.len() >= self.compact_at {
            self.compact();
            self.compact_at = (self.names.len() * 2).max(COMPACT_AT);
        }
    }

    /// Forgets the elements nothing kept refers to: not on screen, with no state before the
    /// oldest kept frame, and in no kept frame. Without it every element a window ever showed
    /// keeps its name and state for good (each session opened and closed adds its own), and
    /// past `max_bytes` those alone evict every frame. The rest are numbered afresh.
    fn compact(&mut self) {
        let mut keep = vec![false; self.names.len()];
        for &id in &self.present { keep[id as usize] = true; }
        for (k, b) in keep.iter_mut().zip(&self.base) { *k |= b.is_some(); }
        Tape::decode(self.words.make_contiguous(), |id, _, _| keep[id as usize] = true);
        let mut map = vec![u32::MAX; keep.len()];
        let mut next = 0u32;
        for (m, k) in map.iter_mut().zip(&keep) {
            if *k { *m = next; next += 1; }
        }
        let kept = |i: &usize| keep[*i];
        let names = std::mem::take(&mut self.names);
        self.names = names.into_iter().enumerate().filter(|(i, _)| kept(i)).map(|(_, n)| n).collect();
        self.name_bytes = self.names.iter().map(String::len).sum();
        self.last = self.last.iter().enumerate().filter(|(i, _)| kept(i)).map(|(_, q)| *q).collect();
        self.seen = self.seen.iter().enumerate().filter(|(i, _)| kept(i)).map(|(_, s)| *s).collect();
        self.base = self.base.iter().enumerate().filter(|(i, _)| kept(i)).map(|(_, b)| *b).collect();
        self.ids.retain(|_, id| { *id = map[*id as usize]; *id != u32::MAX });
        for id in &mut self.present { *id = map[*id as usize]; }
        self.next_present.clear();
        let words = self.words.make_contiguous();
        let mut k = 0;
        while k < words.len() {
            let mask = words[k] & ((1 << MASK_BITS) - 1);
            words[k] = map[(words[k] >> MASK_BITS) as usize] << MASK_BITS | mask;
            k += 1 + mask.count_ones() as usize;
        }
    }

    fn evict(&mut self, now: f64) {
        while self.frames.len() > 1 && (self.frames[0].t < now - self.window || self.bytes() > self.max_bytes) {
            let mark = self.frames.pop_front().expect("a frame");
            let mut k = 0;
            while k < mark.len as usize {
                let head = self.words[k];
                let (id, mask) = ((head >> MASK_BITS) as usize, head & ((1 << MASK_BITS) - 1));
                k += 1;
                if mask == 0 { self.base[id] = None; continue; }
                let mut q = self.base[id].unwrap_or([0; FIELDS]);
                for (f, v) in q.iter_mut().enumerate() {
                    if mask & (1 << f) != 0 { *v = self.words[k] as i32; k += 1; }
                }
                self.base[id] = Some(q);
            }
            self.words.drain(..mark.len as usize);
        }
    }

    /// What the recording holds, roughly: deltas, frame marks, names and per-element state.
    pub(crate) fn bytes(&self) -> usize {
        self.words.len() * 4 + self.frames.len() * std::mem::size_of::<Mark>() + self.name_bytes
            + self.names.len() * (std::mem::size_of::<String>() + 2 * std::mem::size_of::<Q>() + 16)
    }

    /// Decodes a frame's words into `(id, mask, values)` deltas.
    pub(crate) fn decode(words: &[u32], mut each: impl FnMut(u32, u32, &Q)) {
        let mut k = 0;
        while k < words.len() {
            let head = words[k];
            let (id, mask) = (head >> MASK_BITS, head & ((1 << MASK_BITS) - 1));
            k += 1;
            let mut q = [0; FIELDS];
            for (f, v) in q.iter_mut().enumerate() {
                if mask & (1 << f) != 0 { *v = words[k] as i32; k += 1; }
            }
            each(id, mask, &q);
        }
    }
}
