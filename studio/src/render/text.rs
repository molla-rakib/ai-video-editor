//! Renders text layers to bitmaps with cosmic-text (full shaping, so Bangla
//! conjuncts render correctly), using the same fonts the app bundles.

use std::sync::{Arc, Mutex, OnceLock};

use iced::advanced::graphics::text::cosmic_text::{self as ct, Attrs, Buffer, Family, Metrics, Shaping, SwashCache, Weight};
use iced::advanced::graphics::text::font_system;

use super::Frame;
use crate::clip::{Align, Ease, TextLayer, TextReveal};

fn swash_cache() -> &'static Mutex<SwashCache> {
    static CACHE: OnceLock<Mutex<SwashCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SwashCache::new()))
}

/// Non-whitespace byte ranges of `text`, in order — the words a kinetic reveal staggers in.
fn word_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                out.push(s..i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out
}

/// Cache key: everything that affects the bitmap. `local` only matters while a reveal is still
/// in progress; once it's done the key settles (clamped), so the bitmap is cached once and
/// reused for the rest of the clip instead of re-rasterizing every frame.
pub fn key(layer: &TextLayer, px_per_unit: f32, local: f64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    layer.text.hash(&mut h);
    layer.font.hash(&mut h);
    layer.weight.hash(&mut h);
    for v in [layer.size, layer.line_height, layer.letter_spacing, layer.padding, px_per_unit as f64] {
        v.to_bits().hash(&mut h);
    }
    for c in [Some(layer.color), layer.background].into_iter().flatten() {
        for v in [c.0, c.1, c.2, c.3] {
            v.to_bits().hash(&mut h);
        }
    }
    (layer.align as u8).hash(&mut h);
    layer.background.is_some().hash(&mut h);
    serde_json::to_string(&layer.spans).unwrap_or_default().hash(&mut h);
    if let Some(r) = layer.reveal {
        let words = word_ranges(if layer.text.is_empty() { " " } else { &layer.text }).len().max(1);
        let total = (words - 1) as f64 * r.stagger + r.duration;
        // 30 steps/sec is plenty smooth for a reveal that lasts well under a second.
        let quantized = (local.clamp(0.0, total) * 30.0).round();
        quantized.to_bits().hash(&mut h);
        r.duration.to_bits().hash(&mut h);
        r.stagger.to_bits().hash(&mut h);
    }
    h.finish()
}

fn ct_color(c: crate::clip::Rgba) -> ct::Color {
    ct::Color::rgba((c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8, (c.3 * 255.0) as u8)
}

/// cosmic-text only carries one `usize` tag per glyph (`Attrs::metadata`), already used for the
/// style-span index; a kinetic reveal packs a word index in above it. Unpacking is safe on a
/// plain (unpacked) style index too, since those are always far below `WORD_SHIFT`.
const WORD_SHIFT: usize = 1_000_000;
fn pack_word_meta(style_idx: usize, word_index: Option<usize>) -> usize {
    word_index.map_or(0, |w| w + 1) * WORD_SHIFT + style_idx
}
fn unpack_style(meta: usize) -> usize {
    meta % WORD_SHIFT
}
fn unpack_word(meta: usize) -> usize {
    meta / WORD_SHIFT
}

pub fn rasterize(layer: &TextLayer, px_per_unit: f32, local: f64) -> Arc<Frame> {
    let size = (layer.size as f32 * px_per_unit).max(1.0);
    let metrics = Metrics::new(size, size * layer.line_height as f32);
    let attrs = Attrs::new()
        .family(Family::Name(&layer.font))
        .weight(Weight(layer.weight))
        .letter_spacing(layer.letter_spacing as f32 / layer.size.max(1.0) as f32);
    let align = match layer.align {
        Align::Left => ct::Align::Left,
        Align::Center => ct::Align::Center,
        Align::Right => ct::Align::Right,
    };
    let text = if layer.text.is_empty() { " " } else { layer.text.as_str() };
    // Each run with its own attributes; `metadata` is the span's index + 1 (0 = plain), so the
    // laid-out glyphs can be traced back for highlights and underlines.
    let runs = layer.styled_runs();
    fn span_attrs<'a>(base: &Attrs<'a>, i: usize, style: Option<&'a crate::clip::SpanStyle>, size: f32, line_height: f32) -> Attrs<'a> {
        let Some(s) = style else { return base.clone() };
        let mut a = base.clone().metadata(i + 1);
        if let Some(f) = &s.font {
            a = a.family(Family::Name(f));
        }
        if let Some(w) = s.weight {
            a = a.weight(Weight(w));
        }
        if let Some(c) = s.color {
            a = a.color(ct_color(c));
        }
        if let Some(k) = s.scale {
            a = a.metrics(Metrics::new(size * k as f32, size * k as f32 * line_height));
        }
        a
    }

    // A word's alpha multiplier at `local`: 1.0 once its reveal has finished, eased in before that.
    let word_alpha = |reveal: TextReveal, word_index: usize| -> f32 {
        let start = word_index as f64 * reveal.stagger;
        Ease::EaseOut.apply(((local - start) / reveal.duration.max(1e-6)).clamp(0.0, 1.0)) as f32
    };
    // Splits run `r` at the word boundaries it overlaps, tagging each piece with its word index
    // (`None` for the whitespace between words, which needs no fading).
    fn split_by_words(r: &std::ops::Range<usize>, words: &[std::ops::Range<usize>]) -> Vec<(std::ops::Range<usize>, Option<usize>)> {
        let mut out = Vec::new();
        let mut at = r.start;
        for (wi, w) in words.iter().enumerate() {
            let (ws, we) = (w.start.max(r.start), w.end.min(r.end));
            if ws >= we {
                continue;
            }
            if at < ws {
                out.push((at..ws, None));
            }
            out.push((ws..we, Some(wi)));
            at = we;
        }
        if at < r.end {
            out.push((at..r.end, None));
        }
        out
    }

    let mut fs_guard = font_system().write().expect("font system");
    let fs = fs_guard.raw();
    let mut buffer = Buffer::new(fs, metrics);
    buffer.set_size(fs, None, None);
    if layer.reveal.is_none() && (layer.spans.is_empty() || layer.text.is_empty()) {
        buffer.set_text(fs, text, &attrs, Shaping::Advanced, Some(align));
    } else if layer.reveal.is_some() {
        // Every glyph needs its own run here (even unstyled ones), so each word can carry its
        // own packed metadata — cosmic-text discards a color's alpha when drawing glyphs
        // (`SwashCache::with_pixels` replaces it with the glyph's own coverage), so the fade
        // itself is applied as a post-pass over the rasterized pixels, keyed by these word boxes.
        let words = word_ranges(text);
        let mut spans: Vec<(&str, Attrs)> = Vec::new();
        for (i, (r, s)) in runs.iter().enumerate() {
            let style_idx = if s.is_some() { i + 1 } else { 0 };
            for (sub, word_index) in split_by_words(r, &words) {
                let a = span_attrs(&attrs, i, *s, size, layer.line_height as f32).metadata(pack_word_meta(style_idx, word_index));
                spans.push((&layer.text[sub], a));
            }
        }
        buffer.set_rich_text(fs, spans, &attrs, Shaping::Advanced, Some(align));
    } else {
        let spans = runs.iter().enumerate().map(|(i, (r, s))| (&layer.text[r.clone()], span_attrs(&attrs, i, *s, size, layer.line_height as f32)));
        buffer.set_rich_text(fs, spans, &attrs, Shaping::Advanced, Some(align));
    }
    buffer.shape_until_scroll(fs, false);
    // Measure, then lay out again at that width so alignment works across lines.
    let width = buffer.layout_runs().map(|r| r.line_w).fold(0.0f32, f32::max).ceil().max(1.0);
    buffer.set_size(fs, Some(width), None);
    buffer.shape_until_scroll(fs, false);
    let height = buffer.layout_runs().map(|r| r.line_top + r.line_height).fold(0.0f32, f32::max).ceil().max(1.0);

    // Room around the glyphs for overhangs, and for highlight boxes around the biggest styled words.
    let biggest = layer.spans.iter().filter_map(|s| s.style.scale).fold(1.0f64, f64::max) as f32;
    let pad = if layer.background.is_some() { (layer.padding as f32 * px_per_unit).round() } else { (size * biggest * 0.12).ceil() + 2.0 };
    let (w, h) = ((width + pad * 2.0) as u32, (height + pad * 2.0) as u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];

    if let Some(bg) = layer.background {
        fill_rounded(&mut rgba, w, h, (pad * 0.7).min(h as f32 / 2.0), [bg.0, bg.1, bg.2, bg.3]);
    }

    // Highlights behind the styled words, underlines below them: one box per line and span.
    let (ox, oy) = (pad as i32, pad as i32);
    let mut underlines = Vec::new();
    for run in buffer.layout_runs() {
        let mut boxes: Vec<(usize, f32, f32, f32)> = Vec::new(); // (style index, left, right, glyph size)
        for g in run.glyphs.iter().filter(|g| unpack_style(g.metadata) > 0) {
            let style_idx = unpack_style(g.metadata);
            match boxes.last_mut() {
                Some(b) if b.0 == style_idx && (g.x - b.2).abs() < 1.0 + g.font_size * 0.5 => {
                    b.2 = b.2.max(g.x + g.w);
                    b.3 = b.3.max(g.font_size);
                }
                _ => boxes.push((style_idx, g.x, g.x + g.w, g.font_size)),
            }
        }
        for (style_idx, left, right, gsize) in boxes {
            let Some(style) = runs.get(style_idx - 1).and_then(|r| r.1) else { continue };
            if let Some(hl) = style.highlight {
                let (x0, x1) = (left - gsize * 0.08 + pad, right + gsize * 0.08 + pad);
                let (y0, y1) = (run.line_y - gsize * 0.86 + pad, run.line_y + gsize * 0.24 + pad);
                fill_box(&mut rgba, w, h, (x0, y0, x1, y1), gsize * 0.12, [hl.0, hl.1, hl.2, hl.3]);
            }
            if style.underline {
                let c = style.color.unwrap_or(layer.color);
                let y = run.line_y + gsize * 0.1 + pad;
                underlines.push(((left + pad, y, right + pad, y + (gsize * 0.06).max(1.0)), gsize * 0.03, [c.0, c.1, c.2, c.3]));
            }
        }
    }

    // A kinetic reveal's word boxes (buffer-local, pre-padding coords) and their current alpha,
    // grouped the same way as the highlight boxes above but keyed by word index instead of style.
    let mut word_boxes: Vec<(f32, f32, f32, f32, f32)> = Vec::new(); // (x0, y0, x1, y1, alpha)
    if let Some(reveal) = layer.reveal {
        for run in buffer.layout_runs() {
            let mut cur: Option<(usize, f32, f32)> = None; // (word index, left, right)
            let flush = |cur: &mut Option<(usize, f32, f32)>, out: &mut Vec<(f32, f32, f32, f32, f32)>| {
                if let Some((wi, l, r)) = cur.take() {
                    out.push((l, run.line_top, r, run.line_top + run.line_height, word_alpha(reveal, wi)));
                }
            };
            for g in run.glyphs.iter() {
                let word_idx_plus_1 = unpack_word(g.metadata);
                if word_idx_plus_1 == 0 {
                    flush(&mut cur, &mut word_boxes);
                    continue;
                }
                let wi = word_idx_plus_1 - 1;
                match &mut cur {
                    Some((cwi, _, right)) if *cwi == wi && (g.x - *right).abs() < 1.0 + g.font_size * 0.5 => *right = right.max(g.x + g.w),
                    _ => {
                        flush(&mut cur, &mut word_boxes);
                        cur = Some((wi, g.x, g.x + g.w));
                    }
                }
            }
            flush(&mut cur, &mut word_boxes);
        }
    }

    let color = ct_color(layer.color);
    let mut cache = swash_cache().lock().expect("swash cache");
    buffer.draw(fs, &mut cache, color, |x, y, gw, gh, col| {
        let a = col.a() as f32 / 255.0;
        if a <= 0.0 {
            return;
        }
        for yy in 0..gh as i32 {
            for xx in 0..gw as i32 {
                let (px, py) = (x + xx + ox, y + yy + oy);
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                // `with_pixels` discards a glyph's own color alpha (only its coverage mask
                // survives), so the reveal fade is applied here instead, by position.
                let mul = word_boxes
                    .iter()
                    .find(|&&(x0, y0, x1, y1, _)| (px - ox) as f32 >= x0 && (px - ox) as f32 <= x1 && (py - oy) as f32 >= y0 && (py - oy) as f32 <= y1)
                    .map_or(1.0, |&(_, _, _, _, m)| m);
                if mul <= 0.0 {
                    continue;
                }
                let i = ((py as u32 * w + px as u32) * 4) as usize;
                blend(&mut rgba[i..i + 4], [col.r(), col.g(), col.b()], a * mul);
            }
        }
    });
    for (rect, radius, c) in underlines {
        fill_box(&mut rgba, w, h, rect, radius, c);
    }
    Arc::new(Frame::new(w, h, rgba))
}

/// An anti-aliased rounded box from (x0, y0) to (x1, y1), in bitmap pixels.
fn fill_box(rgba: &mut [u8], w: u32, h: u32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32, c: [f32; 4]) {
    let rgb = [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8];
    let (cx, cy, hw, hh) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let r = r.min(hw).min(hh).max(0.0);
    for y in (y0.floor().max(0.0) as u32)..(y1.ceil().max(0.0) as u32).min(h) {
        for x in (x0.floor().max(0.0) as u32)..(x1.ceil().max(0.0) as u32).min(w) {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let qx = (px - cx).abs() - (hw - r);
            let qy = (py - cy).abs() - (hh - r);
            let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                let i = ((y * w + x) * 4) as usize;
                blend(&mut rgba[i..i + 4], rgb, c[3] * cover);
            }
        }
    }
}

/// Source-over blend of one color onto a straight-alpha pixel.
fn blend(dst: &mut [u8], rgb: [u8; 3], a: f32) {
    let da = dst[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    for k in 0..3 {
        let s = rgb[k] as f32;
        let d = dst[k] as f32;
        dst[k] = ((s * a + d * da * (1.0 - a)) / out_a).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round() as u8;
}

fn fill_rounded(rgba: &mut [u8], w: u32, h: u32, r: f32, c: [f32; 4]) {
    let rgb = [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8];
    for y in 0..h {
        for x in 0..w {
            // Signed distance to the rounded rectangle, for an anti-aliased edge.
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let qx = (px - w as f32 / 2.0).abs() - (w as f32 / 2.0 - r);
            let qy = (py - h as f32 / 2.0).abs() - (h as f32 / 2.0 - r);
            let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                let i = ((y * w + x) * 4) as usize;
                blend(&mut rgba[i..i + 4], rgb, c[3] * cover);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Rgba, SpanChange};

    fn count(f: &Frame, test: impl Fn([u8; 4]) -> bool) -> usize {
        f.rgba.chunks_exact(4).filter(|p| test([p[0], p[1], p[2], p[3]])).count()
    }

    #[test]
    fn styled_words_render_with_their_own_look() {
        crate::fonts::ensure_loaded();
        let plain = TextLayer { text: "Make it fast".into(), size: 80.0, ..Default::default() };
        let red = |p: [u8; 4]| p[3] > 200 && p[0] > 200 && p[1] < 60 && p[2] < 60;
        let yellow = |p: [u8; 4]| p[3] > 200 && p[0] > 200 && p[1] > 150 && p[2] < 80;
        let base = rasterize(&plain, 1.0, 0.0);
        assert_eq!(count(&base, red) + count(&base, yellow), 0);

        let mut styled = plain.clone();
        styled.style_range(8, 12, &SpanChange::Color(Some(Rgba::hex(0xFF2020))));
        styled.style_range(5, 7, &SpanChange::Highlight(Some(Rgba::hex(0xFFD000))));
        let f = rasterize(&styled, 1.0, 0.0);
        assert!(count(&f, red) > 300, "the colored word is drawn in its color");
        assert!(count(&f, yellow) > 1500, "the highlight box is drawn behind its word");
        assert_eq!((f.width, f.height), (base.width, base.height), "color and highlight don't change the layout");

        // A bigger word makes the layer bigger; an underline adds ink below the baseline.
        let mut big = plain.clone();
        big.style_range(8, 12, &SpanChange::Scale(Some(2.0)));
        let b = rasterize(&big, 1.0, 0.0);
        assert!(b.width > base.width && b.height > base.height, "{}x{} vs {}x{}", b.width, b.height, base.width, base.height);
        let mut under = plain.clone();
        under.style_range(0, 4, &SpanChange::Underline(true));
        let ink = |p: [u8; 4]| p[3] > 128;
        assert!(count(&rasterize(&under, 1.0, 0.0), ink) > count(&base, ink) + 200);
        assert_ne!(key(&plain, 1.0, 0.0), key(&styled, 1.0, 0.0), "styles are part of the cache key");
    }

    #[test]
    fn kinetic_reveal_fades_words_in_one_after_another() {
        crate::fonts::ensure_loaded();
        let layer = TextLayer { text: "One Two Three".into(), size: 80.0, reveal: Some(TextReveal { duration: 0.3, stagger: 0.1 }), ..Default::default() };
        let ink = |f: &Frame| -> usize { f.rgba.chunks_exact(4).filter(|p| p[3] > 128).count() };
        let total = 0.1 * 2.0 + 0.3; // the last word's reveal finishes here
        let none_yet = rasterize(&layer, 1.0, 0.0);
        let partial = rasterize(&layer, 1.0, 0.15);
        let full = rasterize(&layer, 1.0, total);
        assert_eq!(ink(&none_yet), 0, "nothing has started revealing yet");
        assert!(ink(&partial) > 0 && ink(&partial) < ink(&full), "midway: more ink than the start, less than the end ({} vs {} vs {})", ink(&none_yet), ink(&partial), ink(&full));
        // The cache key settles once the whole reveal is done, so later frames reuse the bitmap.
        assert_eq!(key(&layer, 1.0, total), key(&layer, 1.0, total + 5.0), "the key settles after the reveal finishes");
        assert_ne!(key(&layer, 1.0, 0.0), key(&layer, 1.0, total), "but differs while it's in progress");
    }
}
