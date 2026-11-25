//! The browser's clipboard: reads and writes go through the Clipboard API and DOM paste events.

use super::*;

impl Client {
    pub(super) fn read_for(&self, purpose: Purpose) {
        // Start the permission-sensitive operation before yielding the input handler.
        let forms = purpose.forms();
        let promise = crate::platform::web::platform::read_clipboard(forms != Forms::Text, forms == Forms::All, MAX_IMAGE_BYTES as u32);
        let (sender, wake) = (self.sender.clone(), self.wake.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let content = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(value) => {
                    let field = |name: &str| js_sys::Reflect::get(&value, &name.into()).ok().and_then(|v| v.as_string());
                    let html = field("html").filter(|h| !h.is_empty());
                    let text = field("text");
                    let image = web_png(js_sys::Reflect::get(&value, &"png".into()).ok());
                    (text.is_some() || html.is_some() || image.is_some()).then(|| {
                        let mut content = ClipboardContent::new(text.unwrap_or_default());
                        if let Some(html) = html { content = content.with_html(html); }
                        if let Some(image) = image { content = content.with_image(image); }
                        content
                    })
                }
                Err(_) => {
                    web_sys::console::warn_1(&"Clipboard read denied; use the browser's Paste command.".into());
                    None
                }
            };
            if let Some(reply) = purpose.reply(content) && sender.send(reply).is_ok() { wake(); }
        });
    }
    /// A DOM paste or drop into the focused field: the text, HTML and browser file handles come
    /// with the event, and when it carried an image, `png` reads its bytes, only if the target
    /// takes images. A target that takes neither images nor files hears nothing of a paste
    /// that carried only those.
    #[inline(always)]
    pub fn paste_event<S: 'static>(&self, rt: &mut Runtime<S>, content: ClipboardContent, png: Option<impl FnOnce() -> js_sys::Promise>) {
        self.paste_event_core(rt.core_mut(), content, png);
    }
    fn paste_event_core(&self, rt: &mut crate::runtime::RuntimeCore, content: ClipboardContent, png: Option<impl FnOnce() -> js_sys::Promise>) {
        let purpose = Purpose::paste(rt, rt.paste_target());
        // A paste or drop of only an image or files never pastes empty text into a target that
        // takes neither: it would replace the selection with nothing.
        let has_text = |c: &ClipboardContent| !c.text().is_empty() || (!c.is_rich() && png.is_none());
        let content = match purpose.forms() {
            Forms::Text if !has_text(&content) => return,
            Forms::Text => ClipboardContent::new(content.text()),
            Forms::Rich if content.is_empty() && png.is_some() => return,
            Forms::Rich => match content.text_forms() { Some(content) => content, None => return },
            Forms::All => content,
        };
        let Some(png) = png.filter(|_| purpose.forms() == Forms::All) else { return rt.paste_content(content) };
        let promise = png();
        let (sender, wake) = (self.sender.clone(), self.wake.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let image = web_png(wasm_bindgen_futures::JsFuture::from(promise).await.ok());
            let content = match image { Some(image) => content.with_image(image), None => content };
            if let Some(reply) = purpose.reply(Some(content)) && sender.send(reply).is_ok() { wake(); }
        });
    }
    pub(crate) fn flush_core(&self, rt: &mut crate::runtime::RuntimeCore) {
        if rt.edit.has_clipboard() { return flush_memory(rt); }
        #[cfg(test)]
        if let Some(m) = &self.memory { return flush_to_memory(rt, m); }
        rt.edit.clear_secret_request = false;
        if let Some(copy) = rt.copy_request.take() {
            if copy.is_secret() {
                if let Some(slot) = copy.slot() {
                    slot.set(SecretState::Failed("secret clipboard writes are unsupported on web".into()));
                    self.changed.store(true, Ordering::SeqCst);
                    (self.wake)();
                }
            } else {
                let generation = self.latest_write.get().wrapping_add(1);
                self.latest_write.set(generation);
                if copy.has_pending_image() {
                    let copy_reply = copy.copy_image_id().map(|id| CopyReply {
                        id,
                        sender: self.sender.clone(),
                        wake: self.wake.clone(),
                    });
                    let latest = self.latest_write.clone();
                    let latest_check = self.latest_write.clone();
                    let encode_err: std::rc::Rc<std::cell::Cell<Option<CopyImageError>>> = std::rc::Rc::new(std::cell::Cell::new(None));
                    let err_cell = encode_err.clone();
                    let err_check = encode_err.clone();
                    // Kept in an `Option` inside `Closure::wrap` so a first-message CSP worker
                    // fallback can re-extract from the same `ClipboardWrite` if needed.
                    let mut copy_slot = Some(copy);
                    let extract = wasm_bindgen::closure::Closure::wrap(Box::new(move |want_raw: bool| -> wasm_bindgen::JsValue {
                        let obj = js_sys::Object::new();
                        if latest.get() != generation {
                            err_cell.set(Some(CopyImageError::Superseded));
                            if js_sys::Reflect::set(&obj, &"error".into(), &"copy superseded".into()).is_err() {
                                return wasm_bindgen::JsValue::NULL;
                            }
                            return obj.into();
                        }
                        let Some(mut cur) = copy_slot.as_ref().cloned() else {
                            err_cell.set(Some(CopyImageError::Failed("clipboard image already consumed".into())));
                            return wasm_bindgen::JsValue::NULL;
                        };
                        if want_raw {
                            match cur.extract_web_pixels() {
                                Ok((w, h, bytes, premul)) => {
                                    let ok = js_sys::Reflect::set(&obj, &"width".into(), &wasm_bindgen::JsValue::from(w)).is_ok()
                                        && js_sys::Reflect::set(&obj, &"height".into(), &wasm_bindgen::JsValue::from(h)).is_ok()
                                        && js_sys::Reflect::set(&obj, &"bytes".into(), &bytes).is_ok()
                                        && js_sys::Reflect::set(&obj, &"premul".into(), &wasm_bindgen::JsValue::from(premul)).is_ok();
                                    if !ok {
                                        err_cell.set(Some(CopyImageError::Failed("failed to build clipboard pixel payload".into())));
                                        return wasm_bindgen::JsValue::NULL;
                                    }
                                }
                                Err(err) => {
                                    copy_slot = None;
                                    let msg = err.to_string();
                                    err_cell.set(Some(err));
                                    if js_sys::Reflect::set(&obj, &"error".into(), &msg.into()).is_err() {
                                        return wasm_bindgen::JsValue::NULL;
                                    }
                                }
                            }
                        } else {
                            copy_slot = None;
                            match cur.encode_image() {
                                Ok(()) => {
                                    let png = cur.content().image().map_or(&[][..], ClipboardImage::png);
                                    let arr = js_sys::Uint8Array::from(png);
                                    if js_sys::Reflect::set(&obj, &"png".into(), &arr).is_err() {
                                        err_cell.set(Some(CopyImageError::Failed("failed to build clipboard PNG payload".into())));
                                        return wasm_bindgen::JsValue::NULL;
                                    }
                                }
                                Err(err) => {
                                    let msg = err.to_string();
                                    err_cell.set(Some(err));
                                    if js_sys::Reflect::set(&obj, &"error".into(), &msg.into()).is_err() {
                                        return wasm_bindgen::JsValue::NULL;
                                    }
                                }
                            }
                        }
                        obj.into()
                    }) as Box<dyn FnMut(bool) -> wasm_bindgen::JsValue>);
                    let check_outcome = wasm_bindgen::closure::Closure::wrap(Box::new(move |code: String| -> bool {
                        if code == "too_large" {
                            err_check.set(Some(CopyImageError::TooLarge));
                            return false;
                        }
                        if !code.is_empty() {
                            err_check.set(Some(CopyImageError::Failed(code)));
                            return false;
                        }
                        if latest_check.get() != generation {
                            err_check.set(Some(CopyImageError::Superseded));
                            return false;
                        }
                        true
                    }) as Box<dyn FnMut(String) -> bool>);
                    use wasm_bindgen::JsCast;
                    let promise = crate::platform::web::platform::write_clipboard_image(
                        extract.as_ref().unchecked_ref(),
                        check_outcome.as_ref().unchecked_ref(),
                        MAX_IMAGE_BYTES as u32,
                    );
                    wasm_bindgen_futures::spawn_local(async move {
                        let outcome = wasm_bindgen_futures::JsFuture::from(promise).await;
                        drop(extract);
                        drop(check_outcome);
                        match outcome {
                            Ok(_) => if let Some(r) = copy_reply { r.send(Ok(())); },
                            Err(err) => {
                                let typed = encode_err.take().unwrap_or_else(|| CopyImageError::Failed(js_error_message(&err)));
                                if let Some(r) = copy_reply {
                                    r.send(Err(typed));
                                } else if matches!(typed, CopyImageError::Failed(_)) {
                                    web_sys::console::warn_1(&"Clipboard write denied by the browser.".into());
                                }
                            }
                        }
                    });
                } else {
                    // The browser's Clipboard API takes plain text, HTML and PNG; RTF and files stay behind.
                    let content = copy.content();
                    let png = content.image().map_or(&[][..], ClipboardImage::png);
                    let promise = crate::platform::web::platform::write_clipboard(content.text(), content.html().unwrap_or(""), png);
                    wasm_bindgen_futures::spawn_local(async move {
                        if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() {
                            web_sys::console::warn_1(&"Clipboard write denied by the browser.".into());
                        }
                    });
                }
            }
        }
        if let Some(target) = rt.edit.paste_request.take() { self.read_for(Purpose::paste(rt, target)); }
        for id in std::mem::take(&mut rt.edit.read_requests) { self.read_for(Purpose::Read(id)); }
    }
}

/// A `Uint8Array` of PNG bytes from the browser.
fn web_png(value: Option<wasm_bindgen::JsValue>) -> Option<ClipboardImage> {
    use wasm_bindgen::JsCast;
    let bytes = value?.dyn_into::<js_sys::Uint8Array>().ok()?;
    if bytes.length() as usize > MAX_IMAGE_BYTES { return None; }
    ClipboardImage::from_png(bytes.to_vec())
}

fn js_error_message(err: &wasm_bindgen::JsValue) -> String {
    js_sys::Reflect::get(err, &"message".into())
        .ok()
        .and_then(|v| v.as_string())
        .or_else(|| err.as_string())
        .unwrap_or_else(|| "Clipboard write denied by the browser.".into())
}

