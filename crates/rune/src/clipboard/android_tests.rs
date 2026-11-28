use super::android::{AndroidForm, assemble_content, prepare_form};
use super::native::Worker;
use super::*;

fn sample_png() -> Vec<u8> {
    ClipboardImage::from_rgba(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255])
        .expect("valid sample PNG")
        .png()
        .to_vec()
}

#[test]
fn android_prepare_form_selects_secret_image_html_text_and_refuses_files_only() {
    let plain = ClipboardWrite::new(ClipboardContent::new("hello"));
    assert_eq!(prepare_form(&plain), Ok(AndroidForm::Text("hello")));

    let html = ClipboardWrite::new(ClipboardContent::new("hi").with_html("<b>hi</b>"));
    assert_eq!(
        prepare_form(&html),
        Ok(AndroidForm::Html { text: "hi", html: "<b>hi</b>" }),
    );

    let png = sample_png();
    let img = ClipboardImage::from_png(png.clone()).unwrap();
    let rich_img = ClipboardWrite::new(
        ClipboardContent::new("caption")
            .with_html("<img src=\"x\">")
            .with_image(img),
    );
    assert_eq!(prepare_form(&rich_img), Ok(AndroidForm::Image(&png)));

    let (secret, _handle) = ClipboardWrite::secret("top-secret".into(), Duration::from_secs(30));
    assert_eq!(prepare_form(&secret), Ok(AndroidForm::Secret("top-secret")));

    let files_only = ClipboardWrite::new(
        ClipboardContent::new("").with_files([std::path::PathBuf::from("/data/local/tmp/a.txt")]),
    );
    assert!(
        prepare_form(&files_only).is_err(),
        "files-only write must error instead of wiping the Android clipboard with empty text",
    );

    let files_with_text = ClipboardWrite::new(
        ClipboardContent::new("fallback").with_files([std::path::PathBuf::from("/tmp/a.txt")]),
    );
    assert_eq!(prepare_form(&files_with_text), Ok(AndroidForm::Text("fallback")));
}

#[test]
fn android_assemble_content_validates_png_caps_html_and_handles_empty() {
    assert_eq!(assemble_content(None, None, None), None);
    assert_eq!(assemble_content(None, Some(String::new()), Some(b"not a png".to_vec())), None);

    let png = sample_png();
    let img_only = assemble_content(None, None, Some(png.clone())).expect("image-only clip");
    assert_eq!(img_only.text(), "");
    assert!(img_only.is_rich());
    assert!(!img_only.has_plain(), "image-only paste must not feed plain text fields");
    assert_eq!(img_only.image().map(ClipboardImage::png), Some(png.as_slice()));
    assert!(img_only.text_forms().is_none());

    let oversized_html = "x".repeat(html::MAX_RICH_BYTES + 1);
    let capped = assemble_content(Some("ok".into()), Some(oversized_html), None).expect("text stays");
    assert_eq!(capped.text(), "ok");
    assert_eq!(capped.html(), None, "oversized HTML is dropped while plain text stays");

    let rich = assemble_content(Some("hi".into()), Some("<b>hi</b><script>bad()</script>".into()), None)
        .expect("rich clip")
        .untrusted();
    assert_eq!(rich.text(), "hi");
    assert_eq!(rich.html(), Some("<b>hi</b>"));
}

/// Simulates the Android clipboard backend with and without `RuneClipboardProvider` declared:
/// text, HTML, sensitive secrets, and image paste always work, while image copy returns
/// `CopyImageError::Failed` without overwriting existing clipboard content when the manifest
/// omits the provider.
#[test]
fn android_form_backend_falls_back_cleanly_when_image_provider_absent() {
    #[derive(Default)]
    struct MockAndroidState {
        text: Option<String>,
        html: Option<String>,
        png: Option<Vec<u8>>,
        sensitive: bool,
        provider_declared: bool,
        rev: u64,
    }
    struct MockAndroidBoard(Arc<Mutex<MockAndroidState>>);
    impl Backend for MockAndroidBoard {
        fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
            let form = prepare_form(copy)?;
            let mut s = self.0.lock().unwrap();
            match form {
                AndroidForm::Secret(t) => {
                    s.text = Some(t.into());
                    s.html = None;
                    s.png = None;
                    s.sensitive = true;
                }
                AndroidForm::Image(png) => {
                    if !s.provider_declared {
                        return Err(
                            "clipboard image provider (dev.rune.test.rune.clipboard) is not declared in AndroidManifest.xml"
                                .into(),
                        );
                    }
                    s.text = None;
                    s.html = None;
                    s.png = Some(png.to_vec());
                    s.sensitive = false;
                }
                AndroidForm::Html { text, html } => {
                    s.text = Some(text.into());
                    s.html = Some(html.into());
                    s.png = None;
                    s.sensitive = false;
                }
                AndroidForm::Text(t) => {
                    s.text = Some(t.into());
                    s.html = None;
                    s.png = None;
                    s.sensitive = false;
                }
            }
            s.rev += 1;
            Ok(())
        }
        fn text(&mut self) -> Option<Zeroizing<String>> {
            self.0.lock().unwrap().text.clone().map(Zeroizing::new)
        }
        fn content(&mut self, all: bool) -> Option<ClipboardContent> {
            let s = self.0.lock().unwrap();
            assemble_content(
                s.text.clone(),
                s.html.clone(),
                if all { s.png.clone() } else { None },
            )
        }
        fn revision(&self) -> Option<u64> {
            Some(self.0.lock().unwrap().rev)
        }
        fn clear(&mut self) -> Result<(), String> {
            let mut s = self.0.lock().unwrap();
            s.text = None;
            s.html = None;
            s.png = None;
            s.sensitive = false;
            s.rev += 1;
            Ok(())
        }
    }

    let state = Arc::new(Mutex::new(MockAndroidState::default()));
    let shared = state.clone();
    let worker = Worker::spawn(move || Some(MockAndroidBoard(shared.clone())));

    // 1. Write plain text first and wait for the worker to commit it before queuing the image.
    worker.send_write(ClipboardWrite::new(ClipboardContent::new("keep me")), None, Some(0));
    let (sync0_tx, sync0_rx) = mpsc::channel();
    worker.sender.send(Request::Read {
        purpose: Purpose::Read(0),
        reply: sync0_tx,
        wake: Arc::new(|| {}),
    }).unwrap();
    let _ = sync0_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    // 2. Attempt image copy while provider_declared == false -> CopyImageError::Failed and text preserved.
    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let img = cache.rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    img.wait().unwrap();
    let (tx_reply, rx_reply) = mpsc::channel();
    let mut img_write = ClipboardWrite::image(img.clone());
    img_write.attach_copy_reply(CopyReply { id: 1, sender: tx_reply, wake: Arc::new(|| {}) });
    worker.send_write(img_write, None, None);
    match rx_reply.recv_timeout(Duration::from_secs(5)).unwrap() {
        Reply::CopyImage { id: 1, result: Err(CopyImageError::Failed(msg)) } => {
            assert!(msg.contains("rune.clipboard"), "unexpected error: {msg}");
        }
        other => panic!("expected Failed when provider absent, got {:?}", matches!(other, Reply::CopyImage { .. })),
    }
    assert_eq!(state.lock().unwrap().text.as_deref(), Some("keep me"));

    // 3. Enable provider -> image copy succeeds and reads back on Forms::All.
    state.lock().unwrap().provider_declared = true;
    let (tx_reply2, rx_reply2) = mpsc::channel();
    let mut img_write2 = ClipboardWrite::image(img);
    img_write2.attach_copy_reply(CopyReply { id: 2, sender: tx_reply2, wake: Arc::new(|| {}) });
    worker.send_write(img_write2, None, None);
    match rx_reply2.recv_timeout(Duration::from_secs(5)).unwrap() {
        Reply::CopyImage { id: 2, result: Ok(()) } => {}
        _ => panic!("expected Ok(()) when provider is declared"),
    }

    let (read_tx, read_rx) = mpsc::channel();
    worker.sender.send(Request::Read {
        purpose: Purpose::Read(7),
        reply: read_tx,
        wake: Arc::new(|| {}),
    }).unwrap();
    match read_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        Reply::Content { purpose: Purpose::Read(7), content } => {
            assert_eq!(content.image().map(ClipboardImage::size), Some((2, 1)));
        }
        _ => panic!("expected PNG image content"),
    }

    // 4. Secret copy sets sensitive flag and clears on clear_now().
    let (secret_write, handle) = ClipboardWrite::secret("otp-123".into(), Duration::from_secs(3600));
    secret_write.bind(Route::System(worker.sender.clone()));
    worker.send_write(secret_write, None, None);
    let (sync_tx, sync_rx) = mpsc::channel();
    worker.sender.send(Request::Read {
        purpose: Purpose::Read(8),
        reply: sync_tx,
        wake: Arc::new(|| {}),
    }).unwrap();
    let _ = sync_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    {
        let s = state.lock().unwrap();
        assert!(s.sensitive);
        assert_eq!(s.text.as_deref(), Some("otp-123"));
        assert!(s.png.is_none());
    }
    handle.clear_now();
    worker.stop();
    assert_eq!(state.lock().unwrap().text, None);
}
