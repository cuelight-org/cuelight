//! Outline fonts drawn as exact pixels: the glyphs rasterized once, at
//! one size, with hard edges, into a bitmap font that the bitmap text
//! path then draws pixel for pixel.
//!
//! A pixel font is usually shipped as the TTF it was drawn from, and
//! drawn from its outlines it is only exact when it happens to land on
//! the grid: a half-pixel position or a size that is not a whole
//! multiple of its design size smears it. Filled here, a pixel is either
//! in or out, decided at its centre, and the bitmap path never puts a
//! glyph between pixels.

use crate::engine::FontData;
use crate::font::{BitmapFont, Glyph, Rgba};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use std::collections::HashMap;

/// Pixels across the page glyphs are packed on.
const PAGE_WIDTH: u32 = 1024;

/// Rasterize every character `font` has at `size` pixels per em, each
/// glyph with `pad` transparent pixels around it (room for a border to
/// be drawn into), as a bitmap font on one page. `None` when the font
/// cannot be read.
pub(crate) fn rasterize(font: &FontData, size: f64, pad: u32) -> Option<(BitmapFont, Rgba)> {
    let font = FontRef::new(&font.data).ok()?;
    let px = Size::new(size as f32);
    let metrics = font.metrics(px, LocationRef::default());
    let glyph_metrics = font.glyph_metrics(px, LocationRef::default());
    let outlines = font.outline_glyphs();
    let (ascent, descent) = (f64::from(metrics.ascent), f64::from(metrics.descent));
    let line_height = (ascent - descent + f64::from(metrics.leading)).round() as i32;
    let pad = pad as i32;

    // Every glyph filled, then packed on shelves left to right.
    let mut filled: Vec<(char, i32, Ink)> = Vec::new();
    for (codepoint, id) in font.charmap().mappings() {
        let Some(c) = char::from_u32(codepoint) else {
            continue;
        };
        let advance = glyph_metrics
            .advance_width(id)
            .map_or(0.0, f64::from)
            .round() as i32;
        let ink = outlines
            .get(id)
            .and_then(|outline| {
                let mut pen = Flattened::default();
                outline
                    .draw(DrawSettings::unhinted(px, LocationRef::default()), &mut pen)
                    .ok()?;
                fill(&pen.edges, ascent)
            })
            .unwrap_or_default();
        filled.push((c, advance, ink));
    }
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    let mut placed: HashMap<char, (Glyph, Ink)> = HashMap::new();
    for (c, advance, ink) in filled {
        let (w, h) = if ink.width == 0 || ink.height == 0 {
            (0, 0)
        } else {
            (ink.width + 2 * pad as u32, ink.height + 2 * pad as u32)
        };
        if x + w > PAGE_WIDTH && x > 0 {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        let glyph = Glyph {
            x: x as i32,
            y: y as i32,
            width: w as i32,
            height: h as i32,
            xoffset: ink.left - pad,
            yoffset: ink.top - pad,
            xadvance: advance,
            page: 0,
        };
        x += w;
        shelf = shelf.max(h);
        placed.insert(c, (glyph, ink));
    }
    let mut page = Rgba::transparent(PAGE_WIDTH.max(1), (y + shelf).max(1));
    let mut glyphs = HashMap::with_capacity(placed.len());
    for (c, (glyph, ink)) in placed {
        for (i, on) in ink.pixels.iter().enumerate() {
            if *on {
                let (dx, dy) = (i as u32 % ink.width, i as u32 / ink.width);
                page.set(
                    glyph.x + pad + dx as i32,
                    glyph.y + pad + dy as i32,
                    [255, 255, 255, 255],
                );
            }
        }
        glyphs.insert(c, glyph);
    }
    Some((BitmapFont::from_glyphs(line_height, glyphs), page))
}

/// A glyph's filled pixels: `width` by `height` of them, the top-left
/// one `left` pixels right of the pen and `top` pixels down from the top
/// of the line.
#[derive(Debug, Default)]
struct Ink {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    pixels: Vec<bool>,
}

/// An outline flattened to straight edges, in font units scaled to
/// pixels: x right of the pen, y up from the baseline.
#[derive(Default)]
struct Flattened {
    edges: Vec<[f64; 4]>,
    start: (f64, f64),
    at: (f64, f64),
}

impl Flattened {
    fn edge(&mut self, to: (f64, f64)) {
        if self.at != to {
            self.edges.push([self.at.0, self.at.1, to.0, to.1]);
        }
        self.at = to;
    }

    /// A curve as the straight edges that follow it closely enough for a
    /// pixel to fall on the right side.
    fn curve(&mut self, point: impl Fn(f64) -> (f64, f64)) {
        const STEPS: usize = 12;
        for step in 1..=STEPS {
            self.edge(point(step as f64 / STEPS as f64));
        }
    }
}

impl OutlinePen for Flattened {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.start = (f64::from(x), f64::from(y));
        self.at = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.edge((f64::from(x), f64::from(y)));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (
            self.at,
            (f64::from(cx0), f64::from(cy0)),
            (f64::from(x), f64::from(y)),
        );
        self.curve(|t| {
            let u = 1.0 - t;
            (
                u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
                u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
            )
        });
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (
            self.at,
            (f64::from(cx0), f64::from(cy0)),
            (f64::from(cx1), f64::from(cy1)),
            (f64::from(x), f64::from(y)),
        );
        self.curve(|t| {
            let u = 1.0 - t;
            (
                u * u * u * p0.0
                    + 3.0 * u * u * t * p1.0
                    + 3.0 * u * t * t * p2.0
                    + t * t * t * p3.0,
                u * u * u * p0.1
                    + 3.0 * u * u * t * p1.1
                    + 3.0 * u * t * t * p2.1
                    + t * t * t * p3.1,
            )
        });
    }

    fn close(&mut self) {
        let start = self.start;
        self.edge(start);
    }
}

/// Fill the shape `edges` bound, non-zero winding, one pixel at a time:
/// a pixel is in when its centre is. `ascent` turns the font's y, up
/// from the baseline, into pixel rows down from the top of the line.
fn fill(edges: &[[f64; 4]], ascent: f64) -> Option<Ink> {
    // Into pixel space first: x as it is, y flipped and moved down.
    let edges: Vec<[f64; 4]> = edges
        .iter()
        .map(|&[x0, y0, x1, y1]| [x0, ascent - y0, x1, ascent - y1])
        .filter(|[_, y0, _, y1]| y0 != y1)
        .collect();
    if edges.is_empty() {
        return None;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &[x0, y0, x1, y1] in &edges {
        min_x = min_x.min(x0).min(x1);
        max_x = max_x.max(x0).max(x1);
        min_y = min_y.min(y0).min(y1);
        max_y = max_y.max(y0).max(y1);
    }
    let (left, top) = (min_x.floor() as i32, min_y.floor() as i32);
    let (right, bottom) = (max_x.ceil() as i32, max_y.ceil() as i32);
    let (width, height) = ((right - left).max(0) as u32, (bottom - top).max(0) as u32);
    let mut pixels = vec![false; (width * height) as usize];
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for row in 0..height as i32 {
        let sample = f64::from(top + row) + 0.5;
        crossings.clear();
        for &[x0, y0, x1, y1] in &edges {
            // Half-open, so a vertex on the sample row is counted once.
            let (upward, lo, hi) = if y0 < y1 { (1, y0, y1) } else { (-1, y1, y0) };
            if sample < lo || sample >= hi {
                continue;
            }
            let x = x0 + (sample - y0) / (y1 - y0) * (x1 - x0);
            crossings.push((x, upward));
        }
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for pair in crossings.windows(2) {
            winding += pair[0].1;
            if winding == 0 {
                continue;
            }
            // Every pixel whose centre lies between the two crossings.
            let from = (pair[0].0 - 0.5).ceil() as i32;
            let to = (pair[1].0 - 0.5).ceil() as i32;
            for column in from.max(left)..to.min(right) {
                pixels[(row as u32 * width + (column - left) as u32) as usize] = true;
            }
        }
    }
    let any = pixels.iter().any(|on| *on);
    any.then_some(Ink {
        left,
        top,
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square from (1, 1) to (5, 5) in font space, on a line whose
    /// ascent is 8: rows 3 to 7 down from the top, columns 1 to 5.
    #[test]
    fn a_square_fills_the_pixels_whose_centres_it_covers() {
        let edges = [
            [1.0, 1.0, 5.0, 1.0],
            [5.0, 1.0, 5.0, 5.0],
            [5.0, 5.0, 1.0, 5.0],
            [1.0, 5.0, 1.0, 1.0],
        ];
        let ink = fill(&edges, 8.0).unwrap();
        assert_eq!((ink.left, ink.top, ink.width, ink.height), (1, 3, 4, 4));
        assert!(ink.pixels.iter().all(|on| *on));
        // Half a pixel over: the centres decide, so nothing new is in.
        let shifted: Vec<[f64; 4]> = edges
            .iter()
            .map(|&[x0, y0, x1, y1]| [x0 + 0.4, y0, x1 + 0.4, y1])
            .collect();
        let ink = fill(&shifted, 8.0).unwrap();
        assert_eq!((ink.left, ink.width), (1, 5));
        let rows: Vec<Vec<bool>> = ink.pixels.chunks(5).map(<[bool]>::to_vec).collect();
        assert_eq!(rows[0], [true, true, true, true, false]);
    }

    /// A ring: the hole stays empty under non-zero winding when the
    /// inner contour runs the other way.
    #[test]
    fn a_hole_is_left_empty() {
        let outer = [
            [0.0, 0.0, 6.0, 0.0],
            [6.0, 0.0, 6.0, 6.0],
            [6.0, 6.0, 0.0, 6.0],
            [0.0, 6.0, 0.0, 0.0],
        ];
        let inner = [
            [2.0, 2.0, 2.0, 4.0],
            [2.0, 4.0, 4.0, 4.0],
            [4.0, 4.0, 4.0, 2.0],
            [4.0, 2.0, 2.0, 2.0],
        ];
        let edges: Vec<[f64; 4]> = outer.iter().chain(inner.iter()).copied().collect();
        let ink = fill(&edges, 6.0).unwrap();
        assert_eq!((ink.width, ink.height), (6, 6));
        let at = |x: usize, y: usize| ink.pixels[y * 6 + x];
        assert!(at(0, 0) && at(5, 5) && at(1, 3));
        assert!(!at(2, 2) && !at(3, 3));
    }
}
