//! A window's CPU frame buffer: a top-down 32-bit DIB section the CPU renderer
//! (`render::cpu_boot`) paints in place, and the window's GDI redirection surface it's copied to,
//! one `BitBlt` a frame (`Blit`). Measured against every other way of getting CPU bytes
//! composed on Windows (layered windows, a WARP swap chain on the window or on a DirectComposition
//! visual, a DirectComposition surface), this is the cheapest correct one: no DLL beyond user32
//! and gdi32, about 90 µs to set up, one copy of what changed per frame, and nothing per frame that
//! allocates. The buffer keeps the last frame, which the renderer's damage tracking relies on, and
//! it's made once at the largest size the window is likely to reach (its display's), with one row
//! length: a resize then repaints only what moved and touches no new memory.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP,
    HDC, HGDIOBJ, RGBQUAD, ReleaseDC, SRCCOPY, SelectObject,
};

/// A DIB section and the memory DC it's selected into.
pub(crate) struct Dib {
    bitmap: HBITMAP,
    dc: HDC,
    old: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Dib {
    /// A `width` x `height` buffer: B, G, R, A bytes, `width * 4` per row (`stride`), top row
    /// first; the window's client area is its top-left part. Its pages are untouched until
    /// something writes them. None when GDI refuses (out of memory).
    pub(crate) fn new(width: u32, height: u32) -> Option<Dib> {
        let (width, height) = (width.max(1), height.max(1));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: `info` describes a 32 bpp top-down DIB; GDI writes the bits pointer.
        unsafe {
            let bitmap = CreateDIBSection(std::ptr::null_mut(), &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bitmap.is_null() || bits.is_null() { return None; }
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                DeleteObject(bitmap);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            Some(Dib { bitmap, dc, old, bits: bits.cast(), width, height })
        }
    }

    /// Whether a client area of `size` fits in it.
    pub(crate) fn fits(&self, size: (u32, u32)) -> bool { size.0 <= self.width && size.1 <= self.height }

    /// Bytes from one row to the next.
    pub(crate) fn stride(&self) -> usize { self.width as usize * 4 }

    /// The pixels, for the renderer to paint into.
    pub(crate) fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the section holds width * height * 4 bytes for as long as `self` lives, and
        // `&mut self` keeps any other access out.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.width as usize * self.height as usize * 4) }
    }

    /// Writes every page of the first `rows` rows once, so the first frame's paint doesn't take a
    /// fault per 4 KiB. Meant for a worker while the frame is laid out (`Prefault`).
    fn touch(&self, rows: u32) {
        let len = self.stride() * rows.min(self.height) as usize;
        let mut at = 0;
        while at < len {
            // SAFETY: inside the section; a volatile write of the byte already there.
            unsafe { std::ptr::write_volatile(self.bits.add(at), std::ptr::read_volatile(self.bits.add(at))) };
            at += 4096;
        }
    }

    /// Copies what `blit` says of the buffer to the window's client area of `client` pixels, at
    /// the same place.
    pub(crate) fn present(&self, hwnd: HWND, client: (u32, u32), blit: Blit<'_>) {
        let (width, height) = (client.0.min(self.width), client.1.min(self.height));
        let clip = |r: &[u32; 4]| {
            let (x0, y0, x1, y1) = (r[0].min(width), r[1].min(height), r[2].min(width), r[3].min(height));
            (x1 > x0 && y1 > y0).then_some((x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
        };
        let bounds = |rects: &[[u32; 4]]| rects.iter().fold([u32::MAX, u32::MAX, 0, 0], |u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]);
        let copies: Vec<_> = match blit {
            Blit::Bounds([]) => Vec::new(),
            Blit::Bounds(rects) => clip(&bounds(rects)).into_iter().collect(),
            Blit::All => clip(&[0, 0, width, height]).into_iter().collect(),
        };
        if copies.is_empty() { return; }
        // SAFETY: a window this thread owns; the DC goes back before returning.
        unsafe {
            let window = GetDC(hwnd);
            if window.is_null() { return; }
            for (x, y, w, h) in copies { BitBlt(window, x, y, w, h, self.dc, x, y, SRCCOPY); }
            ReleaseDC(hwnd, window);
        }
    }
}

/// What a frame copies to the window (`Dib::present`).
pub(crate) enum Blit<'a> {
    /// The rects' bounding box, in one copy; nothing when there are none (an unchanged frame).
    /// DWM can compose between two `BitBlt`s: on the visible desktop one copy per rect showed
    /// 1-15 torn frames in 600, one per frame none.
    Bounds(&'a [[u32; 4]]),
    /// The whole client area: the window changed size, and its redirection surface with it.
    All,
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: the DC and bitmap are ours; the bitmap leaves the DC before both go.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteDC(self.dc);
            DeleteObject(self.bitmap);
        }
    }
}

// SAFETY: a DIB section and a memory DC aren't tied to the thread that made them; the buffer moves
// to a worker to be touched (`Prefault`) or freed, never used from two threads at once.
unsafe impl Send for Dib {}

/// A new buffer's pages written on a worker while the first frame is laid out: painting into
/// never-touched memory costs a page fault per 4 KiB (20.7 MB at 2880 x 1800, about 8,000 µs on
/// one thread). `take` waits for the worker; None if no worker could start, when the caller makes
/// another buffer and the frame takes the faults.
pub(crate) struct Prefault(Option<std::thread::JoinHandle<Dib>>);

impl Prefault {
    /// Touches the first `rows` rows, the ones the first frame paints.
    pub(crate) fn start(dib: Dib, rows: u32) -> Prefault {
        let worker = std::thread::Builder::new().name("rune-dib-touch".into()).spawn(move || { dib.touch(rows); dib });
        if let Err(error) = &worker { eprintln!("rune: could not start the frame buffer's worker: {error}"); }
        Prefault(worker.ok())
    }

    pub(crate) fn take(self) -> Option<Dib> {
        self.0.and_then(|w| w.join().ok())
    }
}

/// Frees a buffer on a worker (`VirtualFree` of up to tens of MB), off the UI thread.
pub(crate) fn free_later(dib: Dib) {
    if let Err(error) = std::thread::Builder::new().name("rune-dib-free".into()).spawn(move || drop(dib)) {
        eprintln!("rune: could not free the frame buffer on a worker: {error}");
    }
}
