//! SVG documents as vector artwork: what usvg makes of a file, reduced to
//! paths with solid fills and strokes.

use cuelight::{Engine, ResolvedGradient, ResolvedGradientKind, Transform, Vector, VectorPath};
use cuelight_core::PathElement;
use std::sync::{Arc, Mutex};
use usvg::tiny_skia_path::PathSegment;

/// Artwork converted from an SVG document, and what it asked for and did
/// not get.
#[derive(Debug, Clone, PartialEq)]
pub struct Artwork {
    pub vector: Vector,
    /// Font families the document named that the show does not ship:
    /// their text is not drawn. A show problem, and the same on every
    /// machine, which is the point of drawing with the show's fonts.
    pub missing_fonts: Vec<String>,
}

/// The fonts an SVG's text is drawn with: a show's own, and no others.
///
/// Text in artwork becomes paths at load, and which paths depends on the
/// font. Taking whatever the machine happens to have installed means a
/// show that draws its text here drops it there, and a browser has
/// nothing to take at all. A show carries its fonts, so those are the
/// ones to draw with.
pub struct SvgFonts {
    fontdb: Arc<usvg::fontdb::Database>,
    /// The family text gets when it names none: the first font the show
    /// registered, so a show with one font need not say which.
    default_family: Option<String>,
}

impl SvgFonts {
    /// The outline fonts registered with `engine`, which are the show's
    /// own: `assets/fonts` after a load, or whatever a host registered.
    ///
    /// Families are matched by the name inside the font file, as an
    /// SVG's `font-family` names them, not by the name the show
    /// registered the file under.
    pub fn of(engine: &Engine) -> SvgFonts {
        let mut fontdb = usvg::fontdb::Database::new();
        for (_, bytes) in engine.outline_fonts() {
            fontdb.load_font_source(usvg::fontdb::Source::Binary(Arc::new(bytes.to_vec())));
        }
        let default_family = fontdb
            .faces()
            .next()
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()));
        SvgFonts {
            fontdb: Arc::new(fontdb),
            default_family,
        }
    }

    /// Parsing options that draw text with these fonts, and a list that
    /// fills with every family they could not answer for.
    fn options(&self) -> (usvg::Options<'static>, Asked) {
        let asked: Asked = Arc::new(Mutex::new(Vec::new()));
        let noted = asked.clone();
        let chosen = usvg::FontResolver::default_font_selector();
        let mut options = usvg::Options {
            fontdb: self.fontdb.clone(),
            ..usvg::Options::default()
        };
        if let Some(family) = &self.default_family {
            options.font_family = family.clone();
        }
        options.font_resolver.select_font = Box::new(move |font, fontdb| {
            let found = chosen(font, fontdb);
            if found.is_none() {
                // What the document asked for, as it wrote it, so the
                // show can be told which font it does not ship.
                let mut noted = noted.lock().unwrap_or_else(|e| e.into_inner());
                for family in font.families() {
                    let name = match family {
                        usvg::FontFamily::Named(name) => name.clone(),
                        other => format!("{other:?}").to_lowercase(),
                    };
                    if !noted.contains(&name) {
                        noted.push(name);
                    }
                }
            }
            found
        });
        (options, asked)
    }
}

/// Families the document asked for and did not get, filled while it is
/// parsed.
type Asked = Arc<Mutex<Vec<String>>>;

/// Convert an SVG document into vector artwork in its own units (the
/// viewBox): every path, in paint order, with group transforms applied
/// and group opacities folded into the colors. Kept: solid fills and
/// strokes, linear and radial gradient fills (see [`VectorPath::gradient`];
/// a gradient on a stroke paints as its first stop's color, a pattern is
/// dropped), fill rule, stroke width. Text becomes paths through the
/// show's own fonts (see [`SvgFonts`]). Dropped: raster images, clip
/// paths, masks, filters, stroke joins and dashes.
pub fn convert_svg(bytes: &[u8], fonts: &SvgFonts) -> Result<Artwork, String> {
    let (options, asked) = fonts.options();
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    group(tree.root(), 1.0, &mut Vec::new(), &mut paths);
    let missing_fonts = std::mem::take(&mut *asked.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(Artwork {
        vector: Vector {
            width: f64::from(tree.size().width()),
            height: f64::from(tree.size().height()),
            paths,
        },
        missing_fonts,
    })
}

/// Every path under `group`, with the ids of the elements it is inside
/// (`ids`, outermost first), which a show's `parts` name.
fn group(group: &usvg::Group, opacity: f32, ids: &mut Vec<String>, out: &mut Vec<VectorPath>) {
    let opacity = opacity * group.opacity().get();
    for node in group.children() {
        match node {
            usvg::Node::Group(inner) => {
                let named = !inner.id().is_empty();
                if named {
                    ids.push(inner.id().to_owned());
                }
                self::group(inner, opacity, ids, out);
                if named {
                    ids.pop();
                }
            }
            usvg::Node::Path(path) => {
                if let Some(converted) = convert_path(path, opacity, ids) {
                    out.push(converted);
                }
            }
            usvg::Node::Text(text) => {
                let named = !text.id().is_empty();
                if named {
                    ids.push(text.id().to_owned());
                }
                self::group(text.flattened(), opacity, ids, out);
                if named {
                    ids.pop();
                }
            }
            usvg::Node::Image(_) => {}
        }
    }
}

fn convert_path(path: &usvg::Path, opacity: f32, ids: &[String]) -> Option<VectorPath> {
    if !path.is_visible() {
        return None;
    }
    let transform = path.abs_transform();
    let point = |p: usvg::tiny_skia_path::Point| {
        let mut p = p;
        transform.map_point(&mut p);
        [f64::from(p.x), f64::from(p.y)]
    };
    let elements: Vec<PathElement> = path
        .data()
        .segments()
        .map(|segment| match segment {
            PathSegment::MoveTo(p) => PathElement::MoveTo(point(p)),
            PathSegment::LineTo(p) => PathElement::LineTo(point(p)),
            PathSegment::QuadTo(c, p) => PathElement::QuadTo(point(c), point(p)),
            PathSegment::CubicTo(c1, c2, p) => PathElement::CubicTo(point(c1), point(c2), point(p)),
            PathSegment::Close => PathElement::Close,
        })
        .collect();
    if elements.is_empty() {
        return None;
    }
    let fill = path
        .fill()
        .and_then(|fill| color(fill.paint(), fill.opacity().get() * opacity));
    let gradient = path
        .fill()
        .and_then(|fill| gradient(fill.paint(), fill.opacity().get() * opacity, transform));
    // A stroke's width scales with the transform; uniform enough for the
    // artwork this is for.
    let scale = f64::from((transform.sx * transform.sy - transform.kx * transform.ky).abs()).sqrt();
    let stroke = path.stroke().and_then(|stroke| {
        let color = color(stroke.paint(), stroke.opacity().get() * opacity)?;
        Some((color, f64::from(stroke.width().get()) * scale))
    });
    if fill.is_none() && stroke.is_none() {
        return None;
    }
    let mut ids = ids.to_vec();
    if !path.id().is_empty() {
        ids.push(path.id().to_owned());
    }
    Some(VectorPath {
        elements,
        fill,
        stroke,
        ids,
        gradient,
    })
}

/// A gradient paint as a gradient in the artwork's own units: its points
/// through its own transform and the path's, its stops' opacity folded
/// into their alpha. A focal point off the centre is drawn from the
/// centre, and a spread other than `pad` pads: what the fill supports.
fn gradient(paint: &usvg::Paint, opacity: f32, path: usvg::Transform) -> Option<ResolvedGradient> {
    let (kind, own, stops) = match paint {
        usvg::Paint::LinearGradient(g) => (
            ResolvedGradientKind::Linear {
                from: [f64::from(g.x1()), f64::from(g.y1())],
                to: [f64::from(g.x2()), f64::from(g.y2())],
            },
            g.transform(),
            g.stops(),
        ),
        usvg::Paint::RadialGradient(g) => (
            ResolvedGradientKind::Radial {
                center: [f64::from(g.cx()), f64::from(g.cy())],
                radius: f64::from(g.r().get()),
            },
            g.transform(),
            g.stops(),
        ),
        _ => return None,
    };
    // Its own coordinates into the artwork's: the gradient's transform,
    // then the path's. Carried as it is, so an ellipse stays one.
    let t = path.pre_concat(own);
    let stops: Vec<(f32, [u8; 4])> = stops
        .iter()
        .map(|stop| {
            let c = stop.color();
            let alpha = (stop.opacity().get() * opacity).clamp(0.0, 1.0);
            (
                stop.offset().get(),
                [c.red, c.green, c.blue, (alpha * 255.0).round() as u8],
            )
        })
        .collect();
    (!stops.is_empty()).then_some(ResolvedGradient {
        kind,
        stops,
        space: Transform([
            f64::from(t.sx),
            f64::from(t.ky),
            f64::from(t.kx),
            f64::from(t.sy),
            f64::from(t.tx),
            f64::from(t.ty),
        ]),
        straight_alpha: true,
    })
}

/// A paint as one RGBA color: solid colors as they are, gradients by their
/// first stop, patterns not at all.
fn color(paint: &usvg::Paint, opacity: f32) -> Option<[u8; 4]> {
    let (color, stop_opacity) = match paint {
        usvg::Paint::Color(color) => (*color, 1.0),
        usvg::Paint::LinearGradient(gradient) => {
            let stop = gradient.stops().first()?;
            (stop.color(), stop.opacity().get())
        }
        usvg::Paint::RadialGradient(gradient) => {
            let stop = gradient.stops().first()?;
            (stop.color(), stop.opacity().get())
        }
        usvg::Paint::Pattern(_) => return None,
    };
    let alpha = (opacity * stop_opacity).clamp(0.0, 1.0);
    Some([
        color.red,
        color.green,
        color.blue,
        (alpha * 255.0).round() as u8,
    ])
}
