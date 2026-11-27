//! Sandboxed SVG and SVGZ decoding and rasterization.
//!
//! Enforces bounded SVGZ decompression, entity-aware XML nesting depth limits, pre-expansion
//! `<use>` fan-out limits, raster-only `data:` URL header validation (refusing all external
//! filesystem/FIFO/UNC references), output raster side clamping, and post-parse node and pixel
//! budgets before `resvg::render` allocates pixmaps.

use crate::ImageError;

#[cfg(feature = "svg")]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, OnceLock};

    use resvg::{tiny_skia, usvg};

    use crate::{ImageError, ImageLimits, MAX_SIDE, gzip, header};

    const SVG_NS: &str = "http://www.w3.org/2000/svg";
    const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

    /// Maximum raw or decompressed SVG byte length: 32 MiB.
    pub(super) const MAX_SVG_BYTES: usize = 32 << 20;
    /// Maximum nested XML container elements inside an SVG's root element (513 open elements in
    /// all, matching Blink's 512-element HTML/SVG tree bound).
    pub(super) const MAX_SVG_DEPTH: usize = 512;
    /// Maximum renderable or expanded SVG nodes accepted before `resvg::render`.
    pub(super) const MAX_SVG_NODES: usize = 100_000;
    /// Maximum output raster side for `svg_rgba` and `svg_rgba_cover` (stays well within
    /// [`MAX_SIDE`] and [`MAX_ALLOC`] even with mip chains and intermediate render layers).
    pub(super) const MAX_SVG_SIDE: u32 = 4096;

    /// Options for `usvg` that resolve nothing outside the SVG's own bytes: non-`data:` `<image>`
    /// and `<feImage>` `href`s never touch the filesystem, FIFOs, devices or UNC paths, and
    /// `data:` URLs accept validated raster formats only (`image/svg+xml` and `text/plain` SVG
    /// fall-through are refused, and raster headers must satisfy [`MAX_SIDE`] and [`MAX_ALLOC`]).
    fn svg_options(rejected: Arc<OnceLock<ImageError>>) -> usvg::Options<'static> {
        use usvg::{ImageHrefResolver, ImageKind, Options};
        let check_and_wrap = move |kind: ImageKind, data: &[u8]| -> Option<ImageKind> {
            match header::check(data) {
                Ok(()) => Some(kind),
                Err(err) => {
                    rejected.set(err).ok();
                    None
                }
            }
        };
        Options {
            image_href_resolver: ImageHrefResolver {
                resolve_string: Box::new(|_, _| None),
                resolve_data: Box::new(move |mime, data, _| match mime {
                    "image/png" => check_and_wrap(ImageKind::PNG(Arc::clone(&data)), &data),
                    "image/jpg" | "image/jpeg" => check_and_wrap(ImageKind::JPEG(Arc::clone(&data)), &data),
                    "image/gif" => check_and_wrap(ImageKind::GIF(Arc::clone(&data)), &data),
                    "image/webp" => check_and_wrap(ImageKind::WEBP(Arc::clone(&data)), &data),
                    "text/plain" => {
                        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
                            check_and_wrap(ImageKind::PNG(Arc::clone(&data)), &data)
                        } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
                            check_and_wrap(ImageKind::JPEG(Arc::clone(&data)), &data)
                        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
                            check_and_wrap(ImageKind::GIF(Arc::clone(&data)), &data)
                        } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
                            check_and_wrap(ImageKind::WEBP(Arc::clone(&data)), &data)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }),
            },
            ..Options::default()
        }
    }

    fn svg_tree(bytes: &[u8]) -> Result<usvg::Tree, ImageError> {
        if bytes.len() > MAX_SVG_BYTES {
            return Err(ImageError("SVG is too large".into()));
        }
        let unzipped;
        let raw = if bytes.starts_with(&[0x1f, 0x8b]) {
            unzipped = gzip::decompress(bytes, MAX_SVG_BYTES)?;
            unzipped.as_slice()
        } else {
            bytes
        };
        let text = std::str::from_utf8(raw).map_err(|_| ImageError("SVG is not UTF-8".into()))?;
        if svg_exceeds_depth(text, MAX_SVG_DEPTH) {
            return Err(ImageError(format!("SVG exceeds maximum nesting depth of {MAX_SVG_DEPTH}")));
        }
        let xml_opt = usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: MAX_SVG_NODES as u32,
        };
        let doc = usvg::roxmltree::Document::parse_with_options(text, xml_opt)
            .map_err(|e| ImageError(format!("SVG: {e}")))?;
        check_xml_use_budget(&doc)?;
        let rejected = Arc::new(OnceLock::new());
        let opts = svg_options(Arc::clone(&rejected));
        let tree = usvg::Tree::from_xmltree(&doc, &opts).map_err(|e| ImageError(format!("SVG: {e}")))?;
        if let Some(err) = rejected.get() {
            return Err(err.clone());
        }
        Ok(tree)
    }

    /// Iterative byte scan for open XML element depth, run before `roxmltree` so deeply nested
    /// input (including markup expanded through reachable DTD entities) cannot overflow the stack
    /// inside the recursive XML parser or `usvg`/`resvg`.
    pub(super) fn svg_exceeds_depth(text: &str, max_nested: usize) -> bool {
        let limit = max_nested + 1;
        let mut entities: HashMap<&[u8], &[u8]> = HashMap::new();
        let mut frames: Vec<(&[u8], usize)> = vec![(text.as_bytes(), 0)];
        let mut depth = 0usize;
        let mut entity_refs = 0usize;
        let mut expanded_bytes = 0usize;

        while let Some((b, mut i)) = frames.pop() {
            if frames.is_empty() {
                entity_refs = 0;
            }
            while i < b.len() {
                let next_delim = b[i..].iter().position(|&c| c == b'<' || (c == b'&' && !entities.is_empty()));
                let Some(rel) = next_delim else { break };
                i += rel;
                if b[i] == b'&' {
                    if let Some((name, after)) = parse_entity_ref(b, i)
                        && let Some(&val) = entities.get(name)
                    {
                        i = after;
                        // Match roxmltree::LoopDetector (depth 10, 255 nested refs) and bound total
                        // expanded bytes so entity amplification cannot stall the scan.
                        if frames.len() >= 10 || entity_refs >= 255 {
                            return true;
                        }
                        expanded_bytes = expanded_bytes.saturating_add(val.len());
                        if expanded_bytes > MAX_SVG_BYTES {
                            return true;
                        }
                        entity_refs += 1;
                        frames.push((b, i));
                        frames.push((val, 0));
                        break;
                    }
                    i += 1;
                    continue;
                }
                if b[i..].starts_with(b"<!--") {
                    i += 4;
                    let Some(end) = b[i..].windows(3).position(|w| w == b"-->") else { return false };
                    i += end + 3;
                } else if b[i..].starts_with(b"<![CDATA[") {
                    i += 9;
                    let Some(end) = b[i..].windows(3).position(|w| w == b"]]>") else { return false };
                    i += end + 3;
                } else if b[i..].starts_with(b"<?") {
                    i += 2;
                    let Some(end) = b[i..].windows(2).position(|w| w == b"?>") else { return false };
                    i += end + 2;
                } else if b[i..].starts_with(b"<!DOCTYPE") && frames.is_empty() {
                    i = scan_doctype(b, i + 9, &mut entities);
                } else if b[i..].starts_with(b"<!") {
                    i = skip_decl(b, i + 2);
                } else if b[i..].starts_with(b"</") {
                    i += 2;
                    depth = depth.saturating_sub(1);
                    let mut quote = 0u8;
                    while i < b.len() {
                        let c = b[i];
                        i += 1;
                        if quote != 0 {
                            if c == quote {
                                quote = 0;
                            }
                        } else if c == b'"' || c == b'\'' {
                            quote = c;
                        } else if c == b'>' {
                            break;
                        }
                    }
                } else {
                    i += 1;
                    let (mut quote, mut last_non_ws, mut closed) = (0u8, 0u8, false);
                    while i < b.len() {
                        let c = b[i];
                        i += 1;
                        if quote != 0 {
                            if c == quote {
                                quote = 0;
                            }
                        } else if c == b'"' || c == b'\'' {
                            quote = c;
                            last_non_ws = c;
                        } else if c == b'>' {
                            closed = true;
                            break;
                        } else if !c.is_ascii_whitespace() {
                            last_non_ws = c;
                        }
                    }
                    if !closed {
                        return false;
                    }
                    if last_non_ws != b'/' {
                        depth += 1;
                        if depth > limit {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn parse_entity_ref(b: &[u8], start: usize) -> Option<(&[u8], usize)> {
        let rest = b.get(start + 1..)?;
        let cap = rest.len().min(256);
        let semi = rest[..cap].iter().position(|&c| c == b';')?;
        let name = &rest[..semi];
        if name.is_empty() || name[0] == b'#' {
            return None;
        }
        Some((name, start + 1 + semi + 1))
    }

    fn skip_decl(b: &[u8], mut i: usize) -> usize {
        let (mut decl, mut quote) = (1usize, 0u8);
        while i < b.len() && decl > 0 {
            let c = b[i];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'<' {
                decl += 1;
            } else if c == b'>' {
                decl -= 1;
            }
            i += 1;
        }
        i
    }

    /// Walks a `<!DOCTYPE ...>` declaration and records internal general `<!ENTITY name "val">`
    /// definitions so the depth scan counts entity-expanded markup only when reached in the body.
    fn scan_doctype<'a>(b: &'a [u8], mut i: usize, entities: &mut HashMap<&'a [u8], &'a [u8]>) -> usize {
        let mut quote = 0u8;
        let mut in_subset = false;
        while i < b.len() {
            if !in_subset {
                let c = b[i];
                i += 1;
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'[' {
                    in_subset = true;
                } else if c == b'>' {
                    return i;
                }
                continue;
            }
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= b.len() {
                return i;
            }
            if b[i] == b']' {
                i += 1;
                while i < b.len() && b[i] != b'>' {
                    i += 1;
                }
                return (i + 1).min(b.len());
            }
            if b[i..].starts_with(b"<!--") {
                i += 4;
                let Some(end) = b[i..].windows(3).position(|w| w == b"-->") else { return b.len() };
                i += end + 3;
            } else if b[i..].starts_with(b"<?") {
                i += 2;
                let Some(end) = b[i..].windows(2).position(|w| w == b"?>") else { return b.len() };
                i += end + 2;
            } else if b[i..].starts_with(b"<!ENTITY")
                && b.get(i + 8).is_some_and(u8::is_ascii_whitespace)
            {
                i = scan_entity_decl(b, i + 8, entities);
            } else if b[i..].starts_with(b"<!") {
                i = skip_decl(b, i + 2);
            } else {
                i += 1;
            }
        }
        i
    }

    fn scan_entity_decl<'a>(b: &'a [u8], mut i: usize, entities: &mut HashMap<&'a [u8], &'a [u8]>) -> usize {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] == b'%' {
            return skip_decl(b, i);
        }
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
            i += 1;
        }
        let name = &b[name_start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if let Some(&q @ (b'"' | b'\'')) = b.get(i) {
            i += 1;
            let val_start = i;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            if i < b.len() {
                let val = &b[val_start..i];
                if !name.is_empty() {
                    entities.entry(name).or_insert(val);
                }
                i += 1;
            }
        }
        skip_decl(b, i)
    }

    /// Bounds `<use>` fan-out and combined nesting depth iteratively on the parsed XML document
    /// before `usvg::Tree::from_xmltree` expands `<use>` subtrees.
    fn check_xml_use_budget(doc: &usvg::roxmltree::Document<'_>) -> Result<(), ImageError> {
        use usvg::roxmltree::NodeId;

        let mut id_map: HashMap<&str, NodeId> = HashMap::new();
        for node in doc.descendants() {
            if let Some(id) = node.attribute("id") {
                id_map.entry(id).or_insert(node.id());
            }
        }
        let mut memo: HashMap<NodeId, (usize, usize)> = HashMap::new();
        let mut visiting: HashSet<NodeId> = HashSet::new();

        enum Step {
            Enter(NodeId),
            Exit(NodeId, Option<NodeId>),
        }

        let use_target = |node: usvg::roxmltree::Node<'_, '_>| -> Option<NodeId> {
            let tag = node.tag_name();
            if tag.name() != "use" || !matches!(tag.namespace(), None | Some(SVG_NS)) {
                return None;
            }
            let href = node.attribute((XLINK_NS, "href")).or_else(|| node.attribute("href"))?;
            let id = href.trim().strip_prefix('#')?;
            id_map.get(id).copied()
        };

        let mut stack: Vec<Step> = vec![Step::Enter(doc.root().id())];
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(id) => {
                    if memo.contains_key(&id) || !visiting.insert(id) {
                        continue;
                    }
                    let Some(node) = doc.get_node(id) else { continue };
                    let target = use_target(node).filter(|&t| t != id && !visiting.contains(&t));
                    stack.push(Step::Exit(id, target));
                    if let Some(t) = target {
                        if !memo.contains_key(&t) {
                            stack.push(Step::Enter(t));
                        }
                    } else {
                        for child in node.children().filter(|n| n.is_element()) {
                            if !memo.contains_key(&child.id()) {
                                stack.push(Step::Enter(child.id()));
                            }
                        }
                    }
                }
                Step::Exit(id, target) => {
                    visiting.remove(&id);
                    let Some(node) = doc.get_node(id) else { continue };
                    let is_elem = usize::from(node.is_element());
                    let (mut count, mut max_child_depth) = (is_elem, 0usize);
                    if let Some(t) = target {
                        if let Some(&(c, d)) = memo.get(&t) {
                            count = count.saturating_add(c);
                            max_child_depth = max_child_depth.max(d);
                        }
                    } else {
                        for child in node.children().filter(|n| n.is_element()) {
                            if let Some(&(c, d)) = memo.get(&child.id()) {
                                count = count.saturating_add(c);
                                max_child_depth = max_child_depth.max(d);
                            }
                        }
                    }
                    let depth = is_elem + max_child_depth;
                    if count > MAX_SVG_NODES {
                        return Err(ImageError(format!("SVG exceeds maximum node count of {MAX_SVG_NODES}")));
                    }
                    if depth > MAX_SVG_DEPTH + 1 {
                        return Err(ImageError(format!("SVG exceeds maximum nesting depth of {MAX_SVG_DEPTH}")));
                    }
                    memo.insert(id, (count, depth));
                }
            }
        }
        Ok(())
    }

    fn fit_to_rect(r: tiny_skia::IntRect, bounds: tiny_skia::IntRect) -> Option<tiny_skia::IntRect> {
        tiny_skia::IntRect::from_ltrb(
            r.left().max(bounds.left()),
            r.top().max(bounds.top()),
            r.right().min(bounds.right()),
            r.bottom().min(bounds.bottom()),
        )
    }

    /// Walks the resolved `usvg::Tree` iteratively before `resvg::render` to enforce
    /// [`MAX_SVG_NODES`], [`MAX_SIDE`] on every pattern tile and intermediate layer, and
    /// [`MAX_ALLOC`] across the output canvas, pattern tiles, isolated layers, masks, clips,
    /// embedded images, and nested SVGs.
    fn check_tree_budget(
        tree: &usvg::Tree,
        root_transform: tiny_skia::Transform,
        canvas_w: u32,
        canvas_h: u32,
        limits: ImageLimits,
    ) -> Result<(), ImageError> {
        let max_alloc = limits.max_alloc() as f64;
        let max_side = f64::from(limits.max_side());
        let mut nodes = 0usize;
        let mut bytes = f64::from(canvas_w) * f64::from(canvas_h) * 4.0;
        if !bytes.is_finite() || bytes > max_alloc {
            return Err(ImageError("SVG exceeds decoded pixel budget".into()));
        }

        let max_bbox_for = |w: u32, h: u32| -> Option<tiny_skia::IntRect> {
            let wi = i32::try_from(w).ok()?;
            let hi = i32::try_from(h).ok()?;
            tiny_skia::IntRect::from_xywh(
                wi.checked_mul(-2)?,
                hi.checked_mul(-2)?,
                w.checked_mul(5)?,
                h.checked_mul(5)?,
            )
        };

        let mut add_bytes = |add: f64| -> Result<(), ImageError> {
            bytes += add;
            if !bytes.is_finite() || bytes > max_alloc {
                return Err(ImageError("SVG exceeds decoded pixel budget".into()));
            }
            Ok(())
        };

        let mut stack: Vec<(&usvg::Group, tiny_skia::Transform, u32, u32)> =
            vec![(tree.root(), root_transform, canvas_w, canvas_h)];

        while let Some((group, transform, cur_w, cur_h)) = stack.pop() {
            let max_bbox = max_bbox_for(cur_w, cur_h);
            for node in group.children() {
                nodes += 1;
                if nodes > MAX_SVG_NODES {
                    return Err(ImageError(format!("SVG exceeds maximum node count of {MAX_SVG_NODES}")));
                }
                match node {
                    usvg::Node::Group(g) => {
                        let g_ts = transform.pre_concat(g.transform());
                        let (mut next_w, mut next_h) = (cur_w, cur_h);
                        if g.should_isolate()
                            && let Some(bbox) = g.layer_bounding_box().transform(g_ts)
                            && let Some(bounds) = max_bbox
                        {
                            let raw_ibbox = if g.filters().is_empty() {
                                let w = bbox.width().ceil();
                                let h = bbox.height().ceil();
                                if !w.is_finite() || !h.is_finite() {
                                    return Err(ImageError("SVG layer size is invalid".into()));
                                }
                                tiny_skia::IntRect::from_xywh(
                                    bbox.x().floor() as i32 - 2,
                                    bbox.y().floor() as i32 - 2,
                                    (w as u32).saturating_add(4),
                                    (h as u32).saturating_add(4),
                                )
                            } else {
                                Some(bbox.to_int_rect())
                            };
                            if let Some(ibbox) = raw_ibbox.and_then(|r| fit_to_rect(r, bounds)) {
                                if ibbox.width() > MAX_SIDE || ibbox.height() > MAX_SIDE {
                                    return Err(ImageError("SVG layer exceeds maximum side".into()));
                                }
                                let layer_bytes = f64::from(ibbox.width()) * f64::from(ibbox.height()) * 4.0;
                                add_bytes(layer_bytes)?;
                                next_w = ibbox.width();
                                next_h = ibbox.height();
                                let mut clip_opt = g.clip_path();
                                let mut clip_steps = 0usize;
                                while let Some(clip) = clip_opt {
                                    clip_steps += 1;
                                    if clip_steps > MAX_SVG_DEPTH {
                                        return Err(ImageError("SVG clip path nesting is too deep".into()));
                                    }
                                    add_bytes(layer_bytes)?;
                                    stack.push((clip.root(), g_ts.pre_concat(clip.transform()), next_w, next_h));
                                    clip_opt = clip.clip_path();
                                }
                                let mut mask_opt = g.mask();
                                let mut mask_steps = 0usize;
                                while let Some(mask) = mask_opt {
                                    mask_steps += 1;
                                    if mask_steps > MAX_SVG_DEPTH {
                                        return Err(ImageError("SVG mask nesting is too deep".into()));
                                    }
                                    add_bytes(layer_bytes)?;
                                    stack.push((mask.root(), g_ts, next_w, next_h));
                                    mask_opt = mask.mask();
                                }
                                for filter in g.filters() {
                                    for prim in filter.primitives() {
                                        if let usvg::filter::Kind::Image(img) = prim.kind() {
                                            stack.push((img.root(), g_ts, next_w, next_h));
                                        }
                                    }
                                }
                            }
                        }
                        stack.push((g, g_ts, next_w, next_h));
                    }
                    usvg::Node::Path(p) => {
                        if !p.is_visible() {
                            continue;
                        }
                        let patterns = [
                            p.fill().and_then(|f| match f.paint() {
                                usvg::Paint::Pattern(pat) => Some(pat),
                                _ => None,
                            }),
                            p.stroke().and_then(|s| match s.paint() {
                                usvg::Paint::Pattern(pat) => Some(pat),
                                _ => None,
                            }),
                        ];
                        for pat in patterns.into_iter().flatten() {
                            let ts2 = transform.pre_concat(pat.transform());
                            let (sx, sy) = ts2.get_scale();
                            let rect = pat.rect();
                            let pw = (f64::from(rect.width()) * f64::from(sx)).round();
                            let ph = (f64::from(rect.height()) * f64::from(sy)).round();
                            if !pw.is_finite() || !ph.is_finite() || pw > max_side || ph > max_side {
                                return Err(ImageError("SVG pattern tile is too large".into()));
                            }
                            if pw >= 1.0 && ph >= 1.0 {
                                add_bytes(pw * ph * 4.0)?;
                                stack.push((pat.root(), tiny_skia::Transform::from_scale(sx, sy), pw as u32, ph as u32));
                            }
                        }
                    }
                    usvg::Node::Image(img) => {
                        if !img.is_visible() {
                            continue;
                        }
                        match img.kind() {
                            usvg::ImageKind::SVG(sub) => {
                                add_bytes(f64::from(cur_w) * f64::from(cur_h) * 4.0)?;
                                stack.push((sub.root(), transform, cur_w, cur_h));
                            }
                            usvg::ImageKind::PNG(_)
                            | usvg::ImageKind::JPEG(_)
                            | usvg::ImageKind::GIF(_)
                            | usvg::ImageKind::WEBP(_) => {
                                let sz = img.size();
                                let (iw, ih) = (f64::from(sz.width()), f64::from(sz.height()));
                                if !iw.is_finite() || !ih.is_finite() || iw > max_side || ih > max_side {
                                    return Err(ImageError("SVG embedded image is too large".into()));
                                }
                                add_bytes(iw * ih * 4.0)?;
                            }
                        }
                    }
                    usvg::Node::Text(t) => {
                        stack.push((t.flattened(), transform, cur_w, cur_h));
                    }
                }
            }
        }
        Ok(())
    }

    fn demultiply_rgba(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect()
    }

    pub(super) fn svg_rgba_with_size(bytes: &[u8], px: u32) -> Result<(u32, u32, [f32; 2], Vec<u8>), ImageError> {
        svg_rgba_with_size_and_limits(bytes, px, ImageLimits::new())
    }

    pub(super) fn svg_rgba_with_size_and_limits(
        bytes: &[u8],
        px: u32,
        limits: ImageLimits,
    ) -> Result<(u32, u32, [f32; 2], Vec<u8>), ImageError> {
        let tree = svg_tree(bytes)?;
        let size = tree.size();
        let intrinsic = [size.width(), size.height()];
        let px = if px == 0 {
            (size.width().max(size.height()) * 2.0).ceil().clamp(1.0, MAX_SVG_SIDE as f32) as u32
        } else {
            px.min(MAX_SVG_SIDE)
        };
        let k = px as f32 / size.width().max(size.height());
        let (w, h) = (
            ((size.width() * k).round() as u32).clamp(1, px),
            ((size.height() * k).round() as u32).clamp(1, px),
        );
        if !limits.fits(w, h, 4) {
            return Err(ImageError("SVG exceeds decoded pixel budget".into()));
        }
        let t = tiny_skia::Transform::from_scale(k, k);
        check_tree_budget(&tree, t, w, h, limits)?;
        let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| ImageError("empty SVG".into()))?;
        resvg::render(&tree, t, &mut pixmap.as_mut());
        Ok((w, h, intrinsic, demultiply_rgba(&pixmap)))
    }

    pub(super) fn svg_rgba(bytes: &[u8], px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
        let (w, h, _, rgba) = svg_rgba_with_size(bytes, px)?;
        Ok((w, h, rgba))
    }

    pub(super) fn svg_rgba_cover(bytes: &[u8], px: u32) -> Result<Vec<u8>, ImageError> {
        if px == 0 || px > MAX_SVG_SIDE {
            return Err(ImageError("invalid SVG raster size".into()));
        }
        let tree = svg_tree(bytes)?;
        let size = tree.size();
        let k = px as f32 / size.width().min(size.height());
        let t = tiny_skia::Transform::from_row(
            k,
            0.0,
            0.0,
            k,
            (px as f32 - size.width() * k) / 2.0,
            (px as f32 - size.height() * k) / 2.0,
        );
        check_tree_budget(&tree, t, px, px, ImageLimits::new())?;
        let mut pixmap = tiny_skia::Pixmap::new(px, px).ok_or_else(|| ImageError("empty SVG".into()))?;
        resvg::render(&tree, t, &mut pixmap.as_mut());
        Ok(demultiply_rgba(&pixmap))
    }
}

/// An SVG scaled to fit `px` square, keeping its aspect ratio, as straight-alpha sRGB RGBA8.
/// `px == 0` rasterizes at twice the drawing's own size; every raster side is capped at 4096 px.
#[cfg(feature = "svg")]
#[doc(hidden)]
pub fn svg_rgba(bytes: &[u8], px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    imp::svg_rgba(bytes, px)
}

#[cfg(not(feature = "svg"))]
#[doc(hidden)]
pub fn svg_rgba(_: &[u8], _: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

/// Like [`svg_rgba`], also returning the SVG document's resolved intrinsic CSS-pixel dimensions
/// `[width, height]` from `usvg::Tree::size`.
#[cfg(feature = "svg")]
pub(crate) fn svg_rgba_with_size(bytes: &[u8], px: u32) -> Result<(u32, u32, [f32; 2], Vec<u8>), ImageError> {
    imp::svg_rgba_with_size(bytes, px)
}

#[cfg(not(feature = "svg"))]
pub(crate) fn svg_rgba_with_size(_: &[u8], _: u32) -> Result<(u32, u32, [f32; 2], Vec<u8>), ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

#[cfg(feature = "svg")]
pub(crate) fn svg_rgba_with_size_and_limits(
    bytes: &[u8],
    px: u32,
    limits: crate::ImageLimits,
) -> Result<(u32, u32, [f32; 2], Vec<u8>), ImageError> {
    imp::svg_rgba_with_size_and_limits(bytes, px, limits)
}

#[cfg(not(feature = "svg"))]
pub(crate) fn svg_rgba_with_size_and_limits(
    _: &[u8],
    _: u32,
    _: crate::ImageLimits,
) -> Result<(u32, u32, [f32; 2], Vec<u8>), ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

/// An SVG centre-cropped to fill `px` square (`object-fit: cover`), as straight-alpha sRGB RGBA8,
/// decoded through the same sandboxed `usvg` options as [`crate::ImageCache`].
#[cfg(feature = "svg")]
pub fn svg_rgba_cover(bytes: &[u8], px: u32) -> Result<Vec<u8>, ImageError> {
    imp::svg_rgba_cover(bytes, px)
}

#[cfg(not(feature = "svg"))]
pub fn svg_rgba_cover(_: &[u8], _: u32) -> Result<Vec<u8>, ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

#[cfg(all(test, feature = "svg"))]
#[path = "svg_tests.rs"]
mod tests;
