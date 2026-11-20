use super::{pack_cf_html, sanitize_html as clean, unpack_cf_html};

#[test]
fn formatting_survives() {
    let html = r#"<p class="p1"><b>Bold</b> <i>it</i> <a href="https://example.com/a?b=1">link</a><br/><span style="font-weight: 700; color: #333">x</span></p>"#;
    assert_eq!(clean(html), r#"<p class="p1"><b>Bold</b> <i>it</i> <a href="https://example.com/a?b=1">link</a><br /><span style="font-weight: 700; color: #333">x</span></p>"#);
    assert_eq!(clean("<ul><li>1 &lt; 2 &amp; 3 > 0</li></ul>"), "<ul><li>1 &lt; 2 &amp; 3 > 0</li></ul>");
    assert_eq!(clean("<style>p.p1 {margin: 0; font: 12px Helvetica}</style><p>a</p>"), "<style>p.p1 {margin: 0; font: 12px Helvetica}</style><p>a</p>");
    assert_eq!(clean("a < b"), "a &lt; b");
    assert_eq!(clean(r#"<img src="data:image/png;base64,AAAA" alt='a "q"'>"#), r#"<img src="data:image/png;base64,AAAA" alt="a &quot;q&quot;">"#);
    assert_eq!(clean(r#"<img srcset="data:image/png;base64,AAAA 1x, https://example.com/a.png 2x">"#), r#"<img srcset="data:image/png;base64,AAAA 1x, https://example.com/a.png 2x">"#);
}

#[test]
fn active_content_goes() {
    let cases = [
        ("<p>a<script>alert(1)</script>b</p>", "<p>ab</p>"),
        ("<p>a<SCRIPT type=x>alert(1)</ScRiPt >b</p>", "<p>ab</p>"),
        ("<script/src=x></script>ok", "ok"),
        ("<scr\0ipt>alert(1)</scr\0ipt>ok", "ok"),
        ("<img src=x onerror=alert(1)>", "<img src=\"x\">"),
        ("<img src=x on\0error=alert(1)>", "<img src=\"x\">"),
        ("<img src=x OnError = 'alert(1)' >", "<img src=\"x\">"),
        ("<a href=\"javascript:alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\" JaVa&#x09;Script&colon;alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"jav&#97;script:alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"javascript&#0000000000000000000000000000000000058;alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"javascript&#x00000000000000000000000000000000003a;alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"javascript&unknownentity;alert(1)\">x</a>", "<a>x</a>"),
        ("<a href=\"//evil.example/x\">x</a>", "<a>x</a>"),
        ("<a href='vbscript:x'>x</a>", "<a>x</a>"),
        ("<img src=\"data:image/svg+xml;base64,AAAA\">", "<img>"),
        ("<img src=\"data:image/png.svg;base64,AAAA\">", "<img>"),
        ("<a href=\"data:text/html,<script>\">x</a>", "<a>x</a>"),
        ("<iframe srcdoc=\"<script>\"></iframe>t", "t"),
        ("<svg><a xlink:href=\"javascript:x\"/><script>x</script></svg>t", "t"),
        ("<math><mi href=javascript:x>x</mi></math>t", "t"),
        ("<meta http-equiv=refresh content=\"0;url=javascript:x\"><base href=//evil><link rel=import href=x>t", "t"),
        ("<embed src=x.swf><object data=x>fallback</object>t", "t"),
        ("<div style=\"background:url(javascript:x)\">t</div>", "<div>t</div>"),
        ("<div style=\"width: expression(alert(1))\">t</div>", "<div>t</div>"),
        ("<div style=\"x: ex/**/pression(1)\">t</div>", "<div>t</div>"),
        ("<div style=\"font-family:'/*';background:url(https://evil.example/x);/*'\">t</div>", "<div>t</div>"),
        ("<div style=\"&commat;import 'https://evil.example/x.css'\">t</div>", "<div>t</div>"),
        ("<div style=\"background:u\\72l(x)\">t</div>", "<div>t</div>"),
        ("<div style=\"background:u\0rl(x)\">t</div>", "<div>t</div>"),
        ("<style>@import 'x.css';</style>t", "t"),
        ("<style>a{font-family:'/*'}@import 'x.css';/*'}*/</style>t", "t"),
        ("<style>a{background:&#x75;rl(x)}</style>t", "t"),
        ("<style>p{}</style><script>x</script>t", "<style>p{}</style>t"),
        ("<style>p{}</style ><b>t</b>", "<style>p{}</style><b>t</b>"),
        ("<!--<script>x</script>-->t", "t"),
        ("<!--><script>x</script>t", "t"),
        ("<!---><script>x</script>t", "t"),
        ("<!-- --!><script>x</script>t", "t"),
        ("<![CDATA[<script>x</script>]]>t", "t"),
        ("<!--[if gte mso 9]><xml>x</xml><![endif]-->t", "t"),
        ("<![if !supportLists]>t<![endif]>", "t"),
        ("<!DOCTYPE html><html><body>t</body></html>", "<html><body>t</body></html>"),
        ("<title><!--</title><script>x</script>-->t", "t"),
        ("<textarea><div title=\"</textarea><script>x</script>\"></div></textarea>t", "<div title=\"&lt;/textarea&gt;&lt;script&gt;x&lt;/script&gt;\"></div>t"),
        ("<xmp><script>x</script></xmp>t", "t"),
        ("<plaintext><script>x</script>", ""),
        ("<form action=https://evil.example><button formaction=javascript:y>b</button></form>", "<button>b</button>"),
        ("<details open ontoggle=x>t</details>", "<details open>t</details>"),
        ("<scr<script>ipt>alert(1)</script>", "<scr>ipt>alert(1)"),
        ("<img srcset=\"a.png 1x, javascript:x 2x\">", "<img>"),
        ("t<script>never closed", "t"),
        ("t<img src=x", "t"),
        ("t<a href=\"unterminated>", "t"),
        ("a\0b", "ab"),
    ];
    for (dirty, want) in cases { assert_eq!(clean(dirty), want, "{dirty}"); }
}

#[test]
fn clean_is_stable() {
    for html in [
        "<p><b>x</b><img src=y onload=z></p>",
        "<a href='mailto:a@b.c'>m</a>",
        "<style>a{}</style>",
        "<div title=\"<script>alert(1)</script>\">x</div>",
        "<scr<script>ipt>alert(1)</script>",
    ] {
        let once = clean(html);
        assert_eq!(clean(&once), once, "{html}");
    }
}

#[test]
fn stays_linear_and_panic_free_on_huge_and_nested_inputs() {
    let start = crate::time::now();
    let many_scripts = "<script>alert(1)</script><b>ok</b>".repeat(5_000);
    assert_eq!(clean(&many_scripts), "<b>ok</b>".repeat(5_000));

    let deep = format!("{}text{}", "<div>".repeat(5_000), "</div>".repeat(5_000));
    assert_eq!(clean(&deep), deep);

    let unclosed = "<a href=\"https://example.com\" ".repeat(5_000);
    assert_eq!(clean(&unclosed), "");

    let multibyte = "café 🦀 <b title=\"🎉\">bold</b>".repeat(2_000);
    assert!(clean(&multibyte).contains("🦀"));
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "sanitizer took {:?}", start.elapsed());
}

#[test]
fn cf_html_uses_utf8_byte_offsets_and_tolerates_minus_one_offsets() {
    let fragment = "<b>Héllo 🦀</b> <i>world</i>";
    let packed = pack_cf_html(fragment);
    assert_eq!(unpack_cf_html(packed.as_bytes()).as_deref(), Some(fragment));
    let mut nul_padded = packed.into_bytes();
    nul_padded.extend_from_slice(b"\0\0\0");
    assert_eq!(unpack_cf_html(&nul_padded).as_deref(), Some(fragment));

    // Apps that omit context or fragment offsets write -1 (or -000000001) and rely on the markers or StartHTML.
    let minus_one_context = "Version:0.9\r\nStartHTML:-1\r\nEndHTML:-1\r\nStartFragment:0000000109\r\nEndFragment:0000000124\r\n<!--StartFragment--><b>Hi café</b><!--EndFragment-->";
    assert_eq!(unpack_cf_html(minus_one_context.as_bytes()).as_deref(), Some("<b>Hi café</b>"));

    let minus_one_frag = "Version:0.9\r\nStartHTML:-000000001\r\nEndHTML:-000000001\r\nStartFragment:-000000001\r\nEndFragment:-000000001\r\n<html><body><!--StartFragment--><i>markers</i><!--EndFragment--></body></html>";
    assert_eq!(unpack_cf_html(minus_one_frag.as_bytes()).as_deref(), Some("<i>markers</i>"));

    let html_offsets_only = "Version:0.9\r\nStartHTML:0000000043\r\nEndHTML:0000000059\r\n<p>doc slice</p>";
    assert_eq!(unpack_cf_html(html_offsets_only.as_bytes()).as_deref(), Some("<p>doc slice</p>"));

    let out_of_bounds = "Version:0.9\r\nStartHTML:0000000100\r\nEndHTML:0000099999\r\nStartFragment:0000000500\r\nEndFragment:0000099999\r\n<b>fallback</b>";
    assert_eq!(unpack_cf_html(out_of_bounds.as_bytes()).as_deref(), Some("<b>fallback</b>"));
}
