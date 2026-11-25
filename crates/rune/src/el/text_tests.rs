use crate::tooling::test_alloc::{ReleaseCheck, watch_release};
use crate::{Link, TextCase, rich, text};

#[test]
fn redacted_label_set_content_wipes_old_text_on_growth_shrink_and_clear() {
    for revealed in [false, true] {
        let mut label = text("old-label-secret").secure().revealed(revealed);

        // 1. Growing past old capacity must wipe the old buffer before release.
        let (old_ptr, old_len) = {
            let t = &label.text.as_ref().unwrap().content;
            (t.as_ptr(), t.len())
        };
        let (_, check) = watch_release(old_ptr, old_len, || {
            label.set_content("a-much-longer-replacement-label-secret-beyond-old-capacity");
        });
        assert_eq!(check, ReleaseCheck::Zeroed, "growing redacted label (revealed={revealed}) must wipe old buffer");
        assert_eq!(
            label.text.as_ref().unwrap().content,
            "a-much-longer-replacement-label-secret-beyond-old-capacity"
        );

        // 2. Shrinking must wipe and drop the old buffer rather than leaving tail bytes in spare capacity.
        let (old_ptr, old_len) = {
            let t = &label.text.as_ref().unwrap().content;
            (t.as_ptr(), t.len())
        };
        let (_, check) = watch_release(old_ptr, old_len, || {
            label.set_content("tiny");
        });
        assert_eq!(check, ReleaseCheck::Zeroed, "shrinking redacted label (revealed={revealed}) must wipe old buffer");
        {
            let t = &label.text.as_ref().unwrap().content;
            assert_eq!((t.as_str(), t.capacity()), ("tiny", 4));
        }

        // 3. Clearing to empty must wipe and free the old buffer.
        let (old_ptr, old_len) = {
            let t = &label.text.as_ref().unwrap().content;
            (t.as_ptr(), t.len())
        };
        let (_, check) = watch_release(old_ptr, old_len, || {
            label.set_content("");
        });
        assert_eq!(check, ReleaseCheck::Zeroed, "clearing redacted label (revealed={revealed}) must wipe old buffer");
        {
            let t = &label.text.as_ref().unwrap().content;
            assert_eq!((t.as_str(), t.capacity()), ("", 0));
        }
    }
}

#[test]
fn redacted_label_set_content_wipes_spare_capacity_source_and_links() {
    // 1. Spare capacity holding trailing bytes of a truncated secret string is also wiped.
    let mut padded = String::from("head-and-hidden-spare-secret-tail");
    padded.truncate(4);
    let mut label = text(padded).secure();
    let (old_ptr, old_cap) = {
        let t = &label.text.as_ref().unwrap().content;
        (t.as_ptr(), t.capacity())
    };
    let (_, check) = watch_release(old_ptr, old_cap, || {
        label.set_content("next");
    });
    assert_eq!(check, ReleaseCheck::Zeroed, "spare capacity in redacted label must be wiped on replace");

    // 2. Authored `TextSpec::source` (from `apply_case` prior to `.secure()`) is wiped on `set_content`.
    let mut cased = text("cased-secret-value").text_case(TextCase::Uppercase);
    cased.text.as_mut().unwrap().apply_case();
    let mut cased = cased.secure();
    let (src_ptr, src_len) = {
        let s = &cased.text.as_ref().unwrap().source.as_ref().unwrap().text;
        (s.as_ptr(), s.len())
    };
    let (_, check) = watch_release(src_ptr, src_len, || {
        cased.set_content("replacement");
    });
    assert_eq!(check, ReleaseCheck::Zeroed, "redacted TextSpec::source must be wiped on set_content");
    assert!(cased.text.as_ref().unwrap().source.is_none());

    // 3. Links attached to a redacted label have id, label and help wiped on `set_content` and `conceal`.
    let make_linked = || {
        rich("secret-text", vec![])
            .link(Link::new(0..6, "https://secret.example/token").href("https://secret.example/href").label("secret-link-label").help("secret-link-help"))
            .secure()
    };
    let mut a = make_linked();
    let (id_ptr, id_len) = {
        let l = &a.text.as_ref().unwrap().cold.links[0];
        (l.id.as_ptr(), l.id.len())
    };
    let (_, check) = watch_release(id_ptr, id_len, || { a.set_content("next"); });
    assert_eq!(check, ReleaseCheck::Zeroed, "redacted link id must be wiped on set_content");

    let mut b = make_linked();
    let (lbl_ptr, lbl_len) = {
        let l = b.text.as_ref().unwrap().cold.links[0].label.as_ref().unwrap();
        (l.as_ptr(), l.len())
    };
    let (_, check) = watch_release(lbl_ptr, lbl_len, || { b.set_content("next"); });
    assert_eq!(check, ReleaseCheck::Zeroed, "redacted link label must be wiped on set_content");

    let mut c = make_linked();
    let (help_ptr, help_len) = {
        let h = c.text.as_ref().unwrap().cold.links[0].help.as_ref().unwrap();
        (h.as_ptr(), h.len())
    };
    let (_, check) = watch_release(help_ptr, help_len, || { c.text.as_mut().unwrap().conceal(); });
    assert_eq!(check, ReleaseCheck::Zeroed, "redacted link help must be wiped on conceal");

    let mut d = make_linked();
    let (href_ptr, href_len) = {
        let h = d.text.as_ref().unwrap().cold.links[0].href.as_ref().unwrap();
        (h.as_ptr(), h.len())
    };
    let (_, check) = watch_release(href_ptr, href_len, || { d.set_content("next"); });
    assert_eq!(check, ReleaseCheck::Zeroed, "redacted link href must be wiped on set_content");
}
