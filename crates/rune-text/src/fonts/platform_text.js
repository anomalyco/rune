// Text Rune's fonts can't draw, shaped, measured and drawn by the browser (`text/platform_face.rs`):
// an offscreen 2D canvas with the system's fonts, shaping and colour emoji, at zero bytes.
let context = null;
// The state last set: setting a font costs a parse each time.
let last = '';

function setup(font, rtl, lang) {
    context ??= new OffscreenCanvas(64, 64).getContext('2d', { willReadFrequently: true });
    const c = context;
    const state = `${font}|${rtl}|${lang}`;
    if (state === last) return c;
    last = state;
    c.font = font;
    c.direction = rtl ? 'rtl' : 'ltr';
    if ('lang' in c) c.lang = lang || 'inherit';
    c.textAlign = 'left';
    c.textBaseline = 'alphabetic';
    c.fontKerning = 'normal';
    return c;
}

// The width of each prefix of `text` that ends at the UTF-16 offsets `ends`, in px.
export function measurePrefixes(font, text, rtl, lang, ends) {
    const c = setup(font, rtl, lang);
    return Float64Array.from(ends, (end) => c.measureText(text.slice(0, end)).width);
}

// `text` drawn with its pen at x = `phase` (0 to 1 px) on the baseline: a 20-byte header of
// little-endian i32s (width, height, left of the pen, top above the baseline, 1 for a colour
// image) and the pixels, one coverage byte each, or premultiplied RGBA for a colour image
// (emoji). Empty when nothing is drawn.
export function rasterize(font, text, rtl, lang, phase) {
    const c = setup(font, rtl, lang);
    const m = c.measureText(text);
    const left = Math.floor(phase - m.actualBoundingBoxLeft) - 1;
    const right = Math.ceil(phase + m.actualBoundingBoxRight) + 1;
    const top = Math.ceil(m.actualBoundingBoxAscent) + 1;
    const bottom = Math.ceil(m.actualBoundingBoxDescent) + 1;
    const w = right - left, h = top + bottom;
    if (!(w > 2 && h > 2) || w > 4096 || h > 4096) return new Uint8Array(0);
    const canvas = c.canvas;
    if (canvas.width < w || canvas.height < h) {
        canvas.width = Math.max(canvas.width, w);
        canvas.height = Math.max(canvas.height, h);
        // A resized canvas forgets its state.
        last = '';
        setup(font, rtl, lang);
    }
    c.clearRect(0, 0, w, h);
    c.fillStyle = '#fff';
    c.fillText(text, phase - left, top);
    const rgba = c.getImageData(0, 0, w, h).data;
    let color = false, any = false;
    for (let i = 0; i < rgba.length; i += 4) {
        const a = rgba[i + 3];
        if (a === 0) continue;
        any = true;
        if (a > 48 && Math.min(rgba[i], rgba[i + 1], rgba[i + 2]) < 200) { color = true; break; }
    }
    if (!any) return new Uint8Array(0);
    const out = new Uint8Array(20 + (color ? w * h * 4 : w * h));
    const head = new DataView(out.buffer);
    [w, h, left, top, color ? 1 : 0].forEach((v, i) => head.setInt32(i * 4, v, true));
    if (color) {
        for (let i = 0; i < rgba.length; i += 4) {
            const a = rgba[i + 3];
            out[20 + i] = (rgba[i] * a + 127) / 255;
            out[21 + i] = (rgba[i + 1] * a + 127) / 255;
            out[22 + i] = (rgba[i + 2] * a + 127) / 255;
            out[23 + i] = a;
        }
    } else {
        for (let i = 0, p = 20; i < rgba.length; i += 4, p++) out[p] = rgba[i + 3];
    }
    return out;
}
