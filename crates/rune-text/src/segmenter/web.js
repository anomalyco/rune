// The browser's dictionary word breaks (`Intl.Segmenter`, the ICU Chrome lays its own text out
// with), for the scripts Rune's web build carries no dictionary for. One segmenter per language,
// made the first time a text in it is laid out.
const segmenters = new Map();

// The word boundaries inside `text`, as UTF-16 offsets; none where the browser has no segmenter.
export function wordBounds(text, language) {
    let segmenter = segmenters.get(language);
    if (segmenter === undefined) {
        segmenter = typeof Intl.Segmenter === 'function' ? new Intl.Segmenter(language, { granularity: 'word' }) : null;
        segmenters.set(language, segmenter);
    }
    if (segmenter === null) return new Uint32Array(0);
    const out = [];
    for (const { index } of segmenter.segment(text)) if (index > 0) out.push(index);
    return Uint32Array.from(out);
}
