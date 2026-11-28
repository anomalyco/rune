//! Android clipboard backend backed by `dev.rune.RuneClipboard` and `RuneClipboardProvider`.
//!
//! Form selection ([`prepare_form`]) and read-back assembly ([`assemble_content`]) are pure Rust
//! so unit tests verify every clipboard form off-device on every host target.

use super::*;

/// Single primary representation written to Android's `ClipboardManager`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AndroidForm<'a> {
    /// Plain text marked with `ClipDescription.EXTRA_IS_SENSITIVE`.
    Secret(&'a str),
    /// Encoded PNG served through the app's unexported `RuneClipboardProvider`.
    Image(&'a [u8]),
    /// HTML paired with its plain-text fallback (`ClipData.newHtmlText`).
    Html { text: &'a str, html: &'a str },
    /// Plain text (`ClipData.newPlainText`).
    Text(&'a str),
}

/// Chooses the Android `ClipData` form for `copy`.
///
/// Secrets always write plain text only so no HTML or image lingers beside a password.
/// Non-secret writes prefer an image, then HTML beside text, then plain text; a files-only write
/// fails instead of overwriting the clipboard with an empty string.
pub(super) fn prepare_form(copy: &ClipboardWrite) -> Result<AndroidForm<'_>, String> {
    let content = &copy.content;
    if copy.is_secret() {
        return Ok(AndroidForm::Secret(content.text()));
    }
    if let Some(image) = content.image() {
        return Ok(AndroidForm::Image(image.png()));
    }
    if let Some(html) = content.html() {
        return Ok(AndroidForm::Html {
            text: content.text(),
            html,
        });
    }
    if !content.files().is_empty() && content.text().is_empty() {
        return Err("Android clipboard does not support local file paths".into());
    }
    Ok(AndroidForm::Text(content.text()))
}

/// Assembles a [`ClipboardContent`] from the raw forms read out of Android's `ClipboardManager`,
/// capping HTML to `MAX_RICH_BYTES` and validating any PNG payload through [`ClipboardImage::from_png`].
pub(super) fn assemble_content(
    text: Option<String>,
    html: Option<String>,
    png: Option<Vec<u8>>,
) -> Option<ClipboardContent> {
    let html = html.filter(|h| !h.is_empty() && h.len() <= html::MAX_RICH_BYTES);
    let image = png.and_then(ClipboardImage::from_png);
    if text.is_none() && html.is_none() && image.is_none() {
        return None;
    }
    let mut content = ClipboardContent::from(Zeroizing::new(text.unwrap_or_default()));
    if let Some(html) = html {
        content = content.with_html(html);
    }
    if let Some(image) = image {
        content = content.with_image(image);
    }
    Some(content)
}

#[cfg(target_os = "android")]
pub(super) struct AndroidBoard;

#[cfg(target_os = "android")]
pub(super) fn open() -> Option<AndroidBoard> {
    crate::platform::android::bridge::clipboard_available().then_some(AndroidBoard)
}

#[cfg(target_os = "android")]
impl Backend for AndroidBoard {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        use crate::platform::android::bridge;
        match prepare_form(copy)? {
            AndroidForm::Secret(text) => bridge::clipboard_write_text(text, true),
            AndroidForm::Image(png) => bridge::clipboard_write_image(png),
            AndroidForm::Html { text, html } => bridge::clipboard_write_html(text, html),
            AndroidForm::Text(text) => bridge::clipboard_write_text(text, false),
        }
    }

    fn text(&mut self) -> Option<Zeroizing<String>> {
        crate::platform::android::bridge::clipboard_read_text().map(Zeroizing::new)
    }

    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        let (text, html, png) = crate::platform::android::bridge::clipboard_read_content(all)?;
        assemble_content(text, html, png)
    }

    fn revision(&self) -> Option<u64> {
        crate::platform::android::bridge::clipboard_revision()
    }

    fn clear(&mut self) -> Result<(), String> {
        crate::platform::android::bridge::clipboard_clear()
    }
}
