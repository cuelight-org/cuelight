//! Shapes drawn in whole pixels: a pixel lit where its centre is inside
//! the shape, and nowhere else, for a show on its own pixel grid that
//! asks for hard edges.
//!
//! A shape becomes strips of whole pixels, one run of lit pixels per
//! row, drawn as rectangles on pixel boundaries, so nothing is left for
//! the renderer to smooth.

use cuelight_core::PathElement;

/// Straight pieces a curve is followed by: close enough for a pixel's
/// centre to land on the right side at the sizes a pixel grid draws.
const CURVE_STEPS: usize = 16;

/// The outlines of `elements` as closed polygons of straight edges.
pub(crate) fn flatten(elements: &[PathElement]) -> Vec<Vec<[f64; 2]>> {
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    let mut at = [0.0, 0.0];
    for element in elements {
        match *element {
            PathElement::MoveTo(p) => {
                if current.len() > 2 {
                    out.push(std::mem::take(&mut current));
                }
                current.clear();
                current.push(p);
                at = p;
            }
            PathElement::LineTo(p) => {
                current.push(p);
                at = p;
            }
            PathElement::QuadTo(c, p) => {
                for step in 1..=CURVE_STEPS {
                    let t = step as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    current.push([
                        u * u * at[0] + 2.0 * u * t * c[0] + t * t * p[0],
                        u * u * at[1] + 2.0 * u * t * c[1] + t * t * p[1],
                    ]);
                }
                at = p;
            }
            PathElement::CubicTo(c1, c2, p) => {
                for step in 1..=CURVE_STEPS {
                    let t = step as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    let b = |a: f64, b: f64, c: f64, d: f64| {
                        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
                    };
                    current.push([b(at[0], c1[0], c2[0], p[0]), b(at[1], c1[1], c2[1], p[1])]);
                }
                at = p;
            }
            PathElement::Close => {
                if current.len() > 2 {
                    out.push(std::mem::take(&mut current));
                }
                current.clear();
            }
        }
    }
    if current.len() > 2 {
        out.push(current);
    }
    out
}

/// A circle as a polygon, fine enough that no pixel's centre falls on
/// the wrong side of it.
pub(crate) fn circle(cx: f64, cy: f64, radius: f64) -> Vec<[f64; 2]> {
    let steps = ((radius * 8.0).ceil() as usize).clamp(16, 1024);
    (0..steps)
        .map(|i| {
            let a = i as f64 / steps as f64 * std::f64::consts::TAU;
            [cx + radius * a.cos(), cy + radius * a.sin()]
        })
        .collect()
}

/// The pixels `polygons` cover under the non-zero rule, a pixel counted
/// when its centre is inside, as runs `[x0, y, x1]`: row `y`, columns
/// `x0` up to but not including `x1`.
pub(crate) fn runs(polygons: &[Vec<[f64; 2]>]) -> Vec<[i64; 3]> {
    let edges: Vec<[f64; 4]> = polygons
        .iter()
        .flat_map(|points| {
            // Each point with the next, the last with the first.
            points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .map(|(a, b)| [a[0], a[1], b[0], b[1]])
        })
        .filter(|[_, y0, _, y1]| y0 != y1)
        .collect();
    let Some((top, bottom)) = edges.iter().fold(None, |range: Option<(f64, f64)>, e| {
        let (lo, hi) = (e[1].min(e[3]), e[1].max(e[3]));
        Some(range.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))))
    }) else {
        return Vec::new();
    };
    let mut out: Vec<[i64; 3]> = Vec::new();
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for row in top.floor() as i64..bottom.ceil() as i64 {
        let sample = row as f64 + 0.5;
        crossings.clear();
        for &[x0, y0, x1, y1] in &edges {
            let (up, lo, hi) = if y0 < y1 { (1, y0, y1) } else { (-1, y1, y0) };
            // Half open, so a vertex on the sample line counts once.
            if sample < lo || sample >= hi {
                continue;
            }
            crossings.push((x0 + (sample - y0) / (y1 - y0) * (x1 - x0), up));
        }
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for pair in crossings.windows(2) {
            let &[(x0, up), (x1, _)] = pair else {
                continue;
            };
            winding += up;
            if winding == 0 {
                continue;
            }
            // Every pixel whose centre lies between the two crossings.
            let from = (x0 - 0.5).ceil() as i64;
            let to = (x1 - 0.5).ceil() as i64;
            if to > from {
                // A run touching the last one on this row joins it.
                match out.last_mut() {
                    Some([_, y, x1]) if *y == row && *x1 == from => *x1 = to,
                    _ => out.push([from, row, to]),
                }
            }
        }
    }
    out
}

/// Runs as a path of rectangles on pixel boundaries.
pub(crate) fn as_path(runs: &[[i64; 3]]) -> Vec<PathElement> {
    let mut out = Vec::with_capacity(runs.len() * 5);
    for &[x0, y, x1] in runs {
        let (x0, x1, y0, y1) = (x0 as f64, x1 as f64, y as f64, y as f64 + 1.0);
        out.push(PathElement::MoveTo([x0, y0]));
        out.push(PathElement::LineTo([x1, y0]));
        out.push(PathElement::LineTo([x1, y1]));
        out.push(PathElement::LineTo([x0, y1]));
        out.push(PathElement::Close);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_is_lit_where_its_centre_is_inside() {
        // A square from 0.4 to 3.6: the centres 0.5, 1.5, 2.5 and 3.5 are
        // inside, so four pixels a row, rows 0 to 3.
        let square = vec![[0.4, 0.4], [3.6, 0.4], [3.6, 3.6], [0.4, 3.6]];
        assert_eq!(
            runs(&[square]),
            [[0, 0, 4], [0, 1, 4], [0, 2, 4], [0, 3, 4]]
        );
        // Moved so its right edge falls short of a centre, it loses that
        // column; moved on past the next centre, it has stepped a pixel.
        let nudged = vec![[0.45, 0.4], [3.45, 0.4], [3.45, 3.6], [0.45, 3.6]];
        assert_eq!(runs(&[nudged])[0], [0, 0, 3]);
        let stepped = vec![[1.4, 0.4], [4.6, 0.4], [4.6, 3.6], [1.4, 3.6]];
        assert_eq!(runs(&[stepped])[0], [1, 0, 5]);
    }

    #[test]
    fn a_circle_is_a_disc_of_whole_pixels() {
        let disc = runs(&[circle(5.0, 5.0, 3.0)]);
        // Symmetric about the centre, widest through it.
        let widest = disc.iter().map(|[x0, _, x1]| x1 - x0).max().unwrap();
        assert_eq!(widest, 6);
        assert_eq!(disc.first().map(|r| r[1]), Some(2));
        assert_eq!(disc.last().map(|r| r[1]), Some(7));
    }

    #[test]
    fn curves_are_followed_and_holes_left_empty() {
        let ring = [
            vec![[0.0, 0.0], [6.0, 0.0], [6.0, 6.0], [0.0, 6.0]],
            vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]],
        ];
        let lit = runs(&ring);
        assert!(
            lit.contains(&[0, 2, 2]) && lit.contains(&[4, 2, 6]),
            "{lit:?}"
        );
        let path = [
            PathElement::MoveTo([0.0, 0.0]),
            PathElement::QuadTo([8.0, 0.0], [8.0, 8.0]),
            PathElement::Close,
        ];
        assert_eq!(flatten(&path)[0].len(), 1 + CURVE_STEPS);
    }
}
