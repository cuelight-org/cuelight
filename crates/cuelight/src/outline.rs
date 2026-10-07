//! Outline (TrueType / OpenType) fonts: measuring and placing glyphs.
//!
//! The engine only lays text out; drawing the glyph outlines is the
//! renderer's job (see [`ResolvedShape::GlyphRun`](crate::ResolvedShape)).
//! Glyphs are placed by their advance width alone: no kerning, ligatures or
//! complex shaping yet, so text sets slightly looser than in a browser.
//! That is one function to replace when a shaper comes in.

use crate::engine::{FontData, PlacedGlyph};
use cuelight_core::{Align, TextBox};
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

/// Check that `bytes` hold a font this module can read.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), String> {
    FontRef::new(bytes).map(|_| ()).map_err(|e| e.to_string())
}

/// Text laid out in a box.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    /// Glyphs with their baseline origins, relative to the box's top-left.
    pub glyphs: Vec<PlacedGlyph>,
    /// The box the text was laid out in: the one asked for, else the
    /// text's own size.
    pub container: [f64; 2],
    /// Each line's box as `[x, y, width, height]`, relative to the box's
    /// top-left: from its first glyph to the end of its last shown
    /// glyph's advance, between the font's ascent and descent. A line
    /// with nothing shown has none.
    pub lines: Vec<[f64; 4]>,
}

/// Lay `text` out at `size` canvas pixels per em: lines split on `\n`, the
/// block aligned in `container` (its own size when `None`) and, for
/// multi-line text that is not left aligned, each line within the
/// container's width. `None` when the font cannot be read.
/// How high the ink of `characters` reaches and how far it falls at
/// `size`, measured down from the top of the line they are laid out in,
/// with the line's own height. `None` when the font cannot be read.
///
/// A line reserves room for descenders whether or not the characters use
/// any; the ink is what is actually drawn.
pub(crate) fn ink(font: &FontData, size: f64, characters: &[char]) -> Option<(f64, f64, f64)> {
    let font = FontRef::new(&font.data).ok()?;
    let px = Size::new(size as f32);
    let metrics = font.metrics(px, LocationRef::default());
    let glyph_metrics = font.glyph_metrics(px, LocationRef::default());
    let charmap = font.charmap();
    let (ascent, descent) = (f64::from(metrics.ascent), f64::from(metrics.descent));
    let line = ascent - descent + f64::from(metrics.leading);
    let (mut top, mut bottom) = (f64::MAX, f64::MIN);
    for character in characters {
        let id = charmap.map(*character)?;
        let Some(bounds) = glyph_metrics.bounds(id) else {
            continue;
        };
        if bounds.y_min == bounds.y_max {
            continue;
        }
        // Font coordinates count up from the baseline, a laid out line
        // counts down from its top.
        top = top.min(ascent - f64::from(bounds.y_max));
        bottom = bottom.max(ascent - f64::from(bounds.y_min));
    }
    (top <= bottom).then_some((top, bottom, line))
}

/// Only the first `shown` characters get a glyph (line breaks count);
/// the rest keep their room in the layout.
pub(crate) fn layout(
    font: &FontData,
    text: &str,
    size: f64,
    container: Option<[f64; 2]>,
    align: Align,
    text_box: TextBox,
    shown: usize,
) -> Option<Layout> {
    let font = FontRef::new(&font.data).ok()?;
    let px = Size::new(size as f32);
    let metrics = font.metrics(px, LocationRef::default());
    let glyph_metrics = font.glyph_metrics(px, LocationRef::default());
    let charmap = font.charmap();
    let (ascent, descent) = (f64::from(metrics.ascent), f64::from(metrics.descent));
    let line_height = ascent - descent + f64::from(metrics.leading);
    // Down from the top of a line: where the block starts above the
    // first, where it ends below the last, and what a press on a line
    // hits. The cap height is the font's own, else the top of its H.
    let cap = metrics.cap_height.map(f64::from).or_else(|| {
        let id = charmap.map('H')?;
        glyph_metrics.bounds(id).map(|b| f64::from(b.y_max))
    });
    let (start, end, hit) = match (text_box, cap) {
        (TextBox::Cap, Some(cap)) => (ascent - cap, ascent, (ascent - cap, ascent)),
        _ => (0.0, line_height, (0.0, ascent - descent)),
    };

    // Glyph ids and pen positions per line; a character the font lacks
    // becomes glyph 0, the font's "missing" box.
    let lines: Vec<(Vec<(u32, f64)>, f64)> = text
        .split('\n')
        .map(|line| {
            let mut pen = 0.0;
            let glyphs = line
                .trim_end_matches('\r')
                .chars()
                .map(|c| {
                    let id = charmap.map(c).unwrap_or_default();
                    let at = pen;
                    pen += f64::from(glyph_metrics.advance_width(id).unwrap_or_default());
                    (id.to_u32(), at)
                })
                .collect();
            (glyphs, pen)
        })
        .collect();
    let (block_w, block_h) = if text.is_empty() {
        (0.0, 0.0)
    } else {
        let widest = lines.iter().map(|(_, w)| *w).fold(0.0, f64::max);
        (widest, line_height * (lines.len() - 1) as f64 + end - start)
    };
    let [cw, ch] = container.unwrap_or([block_w, block_h]);
    let (bx, by) = align.offset(block_w, block_h, cw, ch);
    let per_line = lines.len() > 1 && !align.is_left();

    let mut glyphs = Vec::new();
    let mut boxes = Vec::new();
    let mut index = 0;
    for (i, (line, width)) in lines.iter().enumerate() {
        let x0 = if per_line {
            align.offset(*width, 0.0, cw, 0.0).0
        } else {
            bx
        };
        let baseline = by - start + line_height * i as f64 + ascent;
        let count = line.len().min(shown.saturating_sub(index));
        glyphs.extend(line.iter().take(count).map(|&(id, pen)| PlacedGlyph {
            id,
            x: x0 + pen,
            y: baseline,
        }));
        if let Some(&(_, first)) = line.first().filter(|_| count > 0) {
            // The last shown glyph ends where the next one starts, or
            // where the line does.
            let last = line.get(count).map_or(*width, |&(_, pen)| pen);
            boxes.push([
                x0 + first,
                baseline - ascent + hit.0,
                last - first,
                hit.1 - hit.0,
            ]);
        }
        // The break between lines is a character too.
        index += line.len() + 1;
    }
    Some(Layout {
        glyphs,
        container: [cw, ch],
        lines: boxes,
    })
}
