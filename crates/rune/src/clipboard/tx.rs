//! `Tx`'s clipboard requests, carried out by the shell's clipboard worker after the handler.
use std::rc::Rc;
use crate::Tx;

pub(crate) type ReadFn = Box<dyn FnOnce(crate::states::States<'_>, &crate::ClipboardContent, &mut Tx)>;
pub(crate) type CopyImageFn = Box<dyn FnOnce(crate::states::States<'_>, Result<(), crate::CopyImageError>, &mut Tx)>;

/// A `Tx::read_clipboard` callback. `Tx` is `Clone`; clones share it and the runtime takes it once.
#[derive(Clone)]
pub(crate) struct ClipboardRead(Rc<std::cell::Cell<Option<ReadFn>>>);
impl ClipboardRead {
    pub(crate) fn take(&self) -> Option<ReadFn> { self.0.take() }
}
impl std::fmt::Debug for ClipboardRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("ClipboardRead") }
}

/// A `Tx::copy_image_then` callback. `Tx` is `Clone`; clones share it and the runtime takes it once.
#[derive(Clone)]
pub(crate) struct CopyImageCallback(Rc<std::cell::Cell<Option<CopyImageFn>>>);
impl CopyImageCallback {
    pub(crate) fn take(&self) -> Option<CopyImageFn> { self.0.take() }
}
impl std::fmt::Debug for CopyImageCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("CopyImageCallback") }
}

impl Tx {
    fn set_copy(&mut self, write: crate::ClipboardWrite, cb: Option<CopyImageCallback>) {
        if let Some(old) = self.copy_image_callback.take() {
            self.superseded_copy_images.push(old);
        }
        self.copy = Some(write);
        self.copy_image_callback = cb;
    }

    /// Request a clipboard write from the shell after this handler finishes.
    pub fn copy(&mut self, text: impl Into<String>) {
        self.set_copy(crate::ClipboardWrite::new(crate::ClipboardContent::new(text)), None);
    }

    /// Copy several forms at once: plain text for every app, HTML and RTF for apps that paste
    /// formatting (Mail, Notes, Pages, Google Docs), an image, and files for a file manager.
    /// macOS writes every form. Windows and Linux write one: the files if there are any, else
    /// the image, else the HTML beside the text, else the text. The web writes text, HTML and
    /// the image. Forms a platform can't hold are skipped silently.
    ///
    /// ```
    /// # use std::path::PathBuf;
    /// # use rune::{ClipboardContent, ClipboardImage, Tx};
    /// # fn _copy(tx: &mut Tx, image: ClipboardImage, path: PathBuf) {
    /// tx.copy_rich(ClipboardContent::new("Ship it").with_html("<b>Ship</b> it"));
    /// tx.copy_rich(ClipboardContent::default().with_image(image));
    /// tx.copy_rich(ClipboardContent::default().with_files([path]));
    /// # }
    /// ```
    pub fn copy_rich(&mut self, content: crate::ClipboardContent) {
        self.set_copy(crate::ClipboardWrite::new(content), None);
    }

    /// Copy an image an [`ImageCache`](crate::ImageCache) decoded, as a browser's Copy Image
    /// does: the pixels it holds, at full resolution, as PNG. Every format the cache draws
    /// copies, with no second decode: PNG, JPEG, WebP, GIF and AVIF (an animation's first
    /// frame) and SVG at its intrinsic size (see [`Image::to_rgba`](crate::Image::to_rgba)). The
    /// PNG is encoded on the clipboard worker, off the event thread (on the web, asynchronously
    /// inside a `ClipboardItem` `Promise<Blob>` created during the user gesture). An image still
    /// decoding, one that failed, or one whose PNG passes
    /// [`MAX_IMAGE_BYTES`](crate::clipboard::MAX_IMAGE_BYTES) copies nothing and leaves the
    /// clipboard as it was. Use [`Tx::copy_image_then`] to hear the outcome.
    ///
    /// ```
    /// # use rune::{Image, Tx};
    /// # fn _copy(tx: &mut Tx, image: &Image) {
    /// tx.copy_image(image);
    /// # }
    /// ```
    pub fn copy_image(&mut self, image: &crate::Image) {
        self.set_copy(crate::ClipboardWrite::image(image.clone()), None);
    }

    /// Like [`Tx::copy_image`], and calls `on_result` on a later frame with `Ok(())` once the
    /// image is on the clipboard or `Err(CopyImageError)` when it could not be copied (still
    /// decoding, decode failed, over 64 megapixels or
    /// [`MAX_IMAGE_BYTES`](crate::clipboard::MAX_IMAGE_BYTES), superseded by a newer clipboard
    /// write, or rejected by the OS/browser clipboard).
    ///
    /// ```
    /// # use rune::{Image, Tx};
    /// # struct App { copy_error: Option<String> }
    /// # fn _copy(tx: &mut Tx, image: &Image) {
    /// tx.copy_image_then(image, |s: &mut App, res, _| {
    ///     s.copy_error = res.err().map(|e| e.to_string());
    /// });
    /// # }
    /// ```
    pub fn copy_image_then<S: 'static>(
        &mut self,
        image: &crate::Image,
        on_result: impl FnOnce(&mut S, Result<(), crate::CopyImageError>, &mut Tx) + 'static,
    ) {
        let f: CopyImageFn = Box::new(move |s, result, tx| {
            if let Some(s) = s.cast::<S>() { on_result(s, result, tx); }
        });
        let cb = CopyImageCallback(Rc::new(std::cell::Cell::new(Some(f))));
        self.set_copy(crate::ClipboardWrite::image(image.clone()), Some(cb));
    }

    /// Exclude this text from clipboard histories and clear it after `clear_after` if unchanged.
    /// The handle reports whether the copy still holds the clipboard, and can clear it early.
    pub fn copy_secret(&mut self, text: impl Into<String>, clear_after: std::time::Duration) -> crate::SecretCopy {
        let (write, copy) = crate::ClipboardWrite::secret(text.into(), clear_after);
        self.set_copy(write, None);
        copy
    }

    /// Clear the secret Rune copied last, from any window, if the clipboard still holds it
    /// (on lock, say). A secret copy asked for earlier in this handler is dropped.
    pub fn clear_secret_clipboard(&mut self) {
        if self.copy.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy = None; }
        self.clear_secret = true;
    }

    /// Read the OS clipboard asynchronously and paste into the focused field or root `on_paste`.
    pub fn request_paste(&mut self) { self.paste = true; }

    /// Read the clipboard off the event thread and hand what it holds to `f`, as an update,
    /// once it arrives: an empty [`ClipboardContent`](crate::ClipboardContent) when it holds
    /// nothing readable. HTML is sanitised, and on macOS made from RTF when that's all there
    /// is. Nothing is pasted anywhere. The web asks the browser, which may prompt or refuse.
    ///
    /// ```
    /// # use rune::Tx;
    /// # struct Composer { preview: Option<String> }
    /// # fn _read(tx: &mut Tx) {
    /// tx.read_clipboard(|s: &mut Composer, content, _| s.preview = content.html().map(Into::into));
    /// # }
    /// ```
    pub fn read_clipboard<S: 'static>(&mut self, f: impl FnOnce(&mut S, &crate::ClipboardContent, &mut Tx) + 'static) {
        let f: ReadFn = Box::new(move |s, content, tx| {
            if let Some(s) = s.cast::<S>() { f(s, content, tx); }
        });
        self.clipboard_reads.push(ClipboardRead(Rc::new(std::cell::Cell::new(Some(f)))));
    }
}
