//! What the translated module calls: little-endian memory access, `memory.grow`, `memory.copy`
//! and `memory.fill`, and the `i8x16` lanes. A trap (an access out of bounds, an
//! `unreachable`) panics, which [`Highlighter`](crate::Highlighter) turns into no highlights.

pub type V128 = [u8; 16];

/// WebAssembly's page size.
pub const PAGE: usize = 65536;

/// Pages a module may grow to (wasm32's 4 GiB).
const MAX_PAGES: usize = 65536;

#[cold]
#[inline(never)]
pub fn trap() -> ! {
    panic!("highlights: wasm trap")
}

#[inline(always)]
pub fn bitmask(v: V128) -> i32 {
    v.iter().enumerate().fold(0, |r, (i, b)| r | (((b >> 7) as i32) << i))
}

#[inline(always)]
pub fn lanes(a: V128, b: V128, f: impl Fn(u8, u8) -> bool) -> V128 {
    std::array::from_fn(|i| if f(a[i], b[i]) { 0xff } else { 0 })
}

#[inline(always)]
pub fn map2(a: V128, b: V128, f: impl Fn(u8, u8) -> u8) -> V128 {
    std::array::from_fn(|i| f(a[i], b[i]))
}

#[inline(always)]
fn at(addr: i32, offset: u64) -> usize {
    addr as u32 as usize + offset as usize
}

/// `N` bytes of memory at `at`. Out of bounds traps: one cold call per access rather than an
/// inlined panic with its own location, which would double the code's size.
#[inline(always)]
fn read<const N: usize>(mem: &[u8], at: usize) -> [u8; N] {
    let mut b = [0u8; N];
    b.copy_from_slice(mem.get(at..at + N).unwrap_or_else(|| trap()));
    b
}

#[inline(always)]
fn write<const N: usize>(mem: &mut [u8], at: usize, v: [u8; N]) {
    mem.get_mut(at..at + N).unwrap_or_else(|| trap()).copy_from_slice(&v);
}

impl super::Wasm {
    #[inline(always)]
    pub fn ld8u(&self, a: i32, off: u64) -> i32 {
        read::<1>(&self.mem, at(a, off))[0] as i32
    }

    #[inline(always)]
    pub fn ld16u(&self, a: i32, off: u64) -> i32 {
        u16::from_le_bytes(read(&self.mem, at(a, off))) as i32
    }

    #[inline(always)]
    pub fn ld16s(&self, a: i32, off: u64) -> i32 {
        i16::from_le_bytes(read(&self.mem, at(a, off))) as i32
    }

    #[inline(always)]
    pub fn ld32(&self, a: i32, off: u64) -> i32 {
        i32::from_le_bytes(read(&self.mem, at(a, off)))
    }

    #[inline(always)]
    pub fn ld64(&self, a: i32, off: u64) -> i64 {
        i64::from_le_bytes(read(&self.mem, at(a, off)))
    }

    #[inline(always)]
    pub fn ld128(&self, a: i32, off: u64) -> V128 {
        read(&self.mem, at(a, off))
    }

    #[inline(always)]
    pub fn st8(&mut self, a: i32, off: u64, v: i32) {
        write(&mut self.mem, at(a, off), [v as u8]);
    }

    #[inline(always)]
    pub fn st16(&mut self, a: i32, off: u64, v: i32) {
        write(&mut self.mem, at(a, off), (v as u16).to_le_bytes());
    }

    #[inline(always)]
    pub fn st32(&mut self, a: i32, off: u64, v: i32) {
        write(&mut self.mem, at(a, off), v.to_le_bytes());
    }

    #[inline(always)]
    pub fn st64(&mut self, a: i32, off: u64, v: i64) {
        write(&mut self.mem, at(a, off), v.to_le_bytes());
    }

    #[inline(always)]
    pub fn st128(&mut self, a: i32, off: u64, v: V128) {
        write(&mut self.mem, at(a, off), v);
    }

    /// `memory.grow`: the old size in pages, or -1.
    pub fn grow(&mut self, pages: i32) -> i32 {
        let old = self.mem.len() / PAGE;
        let new = old + pages as u32 as usize;
        if new > MAX_PAGES {
            return -1;
        }
        self.mem.resize(new * PAGE, 0);
        old as i32
    }

    pub fn copy(&mut self, dst: i32, src: i32, n: i32) {
        let (d, s, n) = (dst as u32 as usize, src as u32 as usize, n as u32 as usize);
        if d.max(s) + n > self.mem.len() {
            trap();
        }
        self.mem.copy_within(s..s + n, d);
    }

    pub fn fill(&mut self, dst: i32, v: i32, n: i32) {
        let (d, n) = (dst as u32 as usize, n as u32 as usize);
        self.mem.get_mut(d..d + n).unwrap_or_else(|| trap()).fill(v as u8);
    }
}
