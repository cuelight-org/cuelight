//! Outline fonts: layout from font metrics, resolved as glyph runs.

// Test code throughout, so clippy lets it panic as tests do.
#![cfg(test)]
#![cfg(feature = "outline-fonts")]

use cuelight::{Engine, ResolvedShape};

const FONT: &[u8] = include_bytes!("fonts/cuelight_test_sans.ttf");

// The test font: 1000 units per em, ascent 1069, descent -293, advances
// A 639, 1 572, space 260. At size 100 a unit is 0.1 px.
const SHOW: &str = r##"{
  "name": "outline", "size": [800, 400],
  "fonts": {
    "big": { "file": "sans", "size": 100, "color": "#FF8000" },
    "edged": { "file": "sans", "size": 50, "border": { "color": "#000000", "width": 2 } },
    "cast": { "file": "sans", "size": 100, "color": "#FFFFFF",
              "border": { "color": "#FF0000", "width": 2 },
              "shadow": { "color": "#00000080", "offset": [3, 2] } }
  },
  "variables": { "speed": 11 },
  "layers": [
    { "name": "a", "type": "text", "font": "big", "text": "A1", "x": 10, "y": 20, "align": "top_left" },
    { "name": "boxed", "type": "text", "font": "big", "text": "1", "size": [200, 300], "x": 400 },
    { "name": "speed", "type": "text", "font": "edged", "text": "0", "x": 780, "y": 380,
      "anchor": "bottom_right", "bindings": [{ "property": "text", "variable": "speed" }] },
    { "name": "lines", "type": "text", "font": "big", "text": "A\n1", "align": "right", "size": [700, 300] },
    { "name": "cast", "type": "text", "font": "cast", "text": "A", "x": 10, "y": 20, "align": "top_left" }
  ]
}"##;

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(SHOW).unwrap();
    engine
}

type Run = (f64, Vec<(f64, f64)>, Option<([u8; 4], f64)>, [u8; 4]);

fn run(engine: &Engine, name: &str) -> Run {
    let layers = engine.resolved_layers().unwrap();
    let layer = layers.iter().find(|l| l.name == name).unwrap();
    match &layer.shape {
        ResolvedShape::GlyphRun {
            size,
            glyphs,
            border,
            ..
        } => (
            *size,
            glyphs.iter().map(|g| (g.x, g.y)).collect(),
            *border,
            layer.color,
        ),
        other => panic!("{name}: {other:?}"),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn glyphs_sit_on_the_baseline_and_advance() {
    let (size, glyphs, border, color) = run(&engine(), "a");
    assert_eq!(size, 100.0);
    assert_eq!(border, None);
    assert_eq!(color, [255, 128, 0, 255]);
    // origin x = 10, then A's advance; baseline = y + ascent
    assert!(
        close(glyphs[0].0, 10.0) && close(glyphs[1].0, 10.0 + 63.9),
        "{glyphs:?}"
    );
    assert!(close(glyphs[0].1, 20.0 + 106.9), "{glyphs:?}");
}

#[test]
fn text_centers_in_its_box() {
    let (_, glyphs, _, _) = run(&engine(), "boxed");
    // "1" is 57.2 wide and one line (136.2) tall, centered in 200x300 at x = 400
    assert!(
        close(glyphs[0].0, 400.0 + (200.0 - 57.2) / 2.0),
        "{glyphs:?}"
    );
    assert!(
        close(glyphs[0].1, (300.0 - 136.2) / 2.0 + 106.9),
        "{glyphs:?}"
    );
}

#[test]
fn anchor_and_bindings_work_like_bitmap_text() {
    let mut engine = engine();
    let (size, glyphs, border, _) = run(&engine, "speed");
    assert_eq!((size, border), (50.0, Some(([0, 0, 0, 255], 2.0))));
    // "11" at size 50 is 57.2 wide; its box's bottom-right is (780, 380)
    assert!(close(glyphs[0].0, 780.0 - 57.2), "{glyphs:?}");
    assert!(close(glyphs[0].1, 380.0 - 68.1 + 53.45), "{glyphs:?}");
    engine.set_variable("speed", 111.0);
    let (_, glyphs, _, _) = run(&engine, "speed");
    assert_eq!(glyphs.len(), 3);
    assert!(close(glyphs[0].0, 780.0 - 85.8), "{glyphs:?}");
}

#[test]
fn lines_align_on_their_own() {
    let (_, glyphs, _, _) = run(&engine(), "lines");
    // right aligned in 700: "A" ends at 700, so does "1", one line lower
    assert!(
        close(glyphs[0].0, 700.0 - 63.9) && close(glyphs[1].0, 700.0 - 57.2),
        "{glyphs:?}"
    );
    assert!(close(glyphs[1].1 - glyphs[0].1, 136.2), "{glyphs:?}");
}

#[test]
fn size_must_match_the_kind_of_font() {
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    let no_size = SHOW.replace(r#""size": 100, "#, "");
    assert!(engine.load_show(&no_size).is_err());
    assert!(engine
        .load_show(&SHOW.replace(r#""size": 100"#, r#""size": 0"#))
        .is_err());
    // unknown fonts are not judged: they may still be registered, as either kind
    let mut engine = Engine::new();
    engine.load_show(&no_size).unwrap();
    assert!(engine.resolved_layers().unwrap().is_empty());
}

#[test]
fn garbage_is_not_a_font() {
    let mut engine = Engine::new();
    assert!(engine.set_outline_font("sans", vec![1u8, 2, 3]).is_err());
    assert!(!engine.has_font("sans"));
}

#[test]
fn a_shadow_is_the_same_text_behind_it() {
    let layers = engine().resolved_layers().unwrap();
    let cast: Vec<_> = layers.iter().filter(|l| l.name == "cast").collect();
    assert_eq!(cast.len(), 2, "a shadow is a draw of its own");

    let glyphs = |layer: &cuelight::ResolvedLayer| match &layer.shape {
        ResolvedShape::GlyphRun { glyphs, border, .. } => (glyphs[0].x, glyphs[0].y, *border),
        other => panic!("{other:?}"),
    };
    let (sx, sy, edge) = glyphs(cast[0]);
    let (tx, ty, _) = glyphs(cast[1]);
    // Behind, moved by the offset, and one color throughout.
    assert!(close(sx - tx, 3.0) && close(sy - ty, 2.0), "{sx} {sy}");
    assert_eq!(cast[0].color, [0, 0, 0, 128]);
    assert_eq!(edge, Some(([0, 0, 0, 128], 2.0)), "the border casts too");
    // The text itself keeps its own colors.
    assert_eq!(cast[1].color, [255, 255, 255, 255]);
    assert_eq!(glyphs(cast[1]).2, Some(([255, 0, 0, 255], 2.0)));
}

#[test]
fn a_shadow_scales_with_the_layer() {
    let mut engine = engine();
    let show = SHOW.replace(
        r#""name": "cast", "type": "text""#,
        r#""name": "cast", "type": "text", "scale": 2"#,
    );
    engine.load_show(&show).unwrap();
    let layers = engine.resolved_layers().unwrap();
    let cast: Vec<_> = layers.iter().filter(|l| l.name == "cast").collect();
    let at = |layer: &cuelight::ResolvedLayer| match &layer.shape {
        ResolvedShape::GlyphRun { glyphs, .. } => (glyphs[0].x, glyphs[0].y),
        other => panic!("{other:?}"),
    };
    let ((sx, sy), (tx, ty)) = (at(cast[0]), at(cast[1]));
    assert!(close(sx - tx, 6.0) && close(sy - ty, 4.0), "{sx} {sy}");
}

#[test]
fn a_shadow_needs_a_color_and_a_finite_offset() {
    let bad = SHOW.replace(r##""color": "#00000080""##, r#""color": "not a color""#);
    assert!(Engine::new().load_show(&bad).is_err());
}

/// The bitmap a layer resolved to: its place, its size, and every
/// alpha value its pixels use.
fn bitmap(engine: &Engine, name: &str) -> ([f64; 4], Vec<u8>) {
    let layers = engine.resolved_layers().unwrap();
    let layer = layers.iter().find(|l| l.name == name).unwrap();
    match &layer.shape {
        ResolvedShape::Bitmap {
            image,
            x,
            y,
            width,
            height,
        } => {
            let mut alphas: Vec<u8> = image.pixels.chunks(4).map(|px| px[3]).collect();
            alphas.sort_unstable();
            alphas.dedup();
            ([*x, *y, *width, *height], alphas)
        }
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn an_outline_font_asked_for_as_pixels_is_drawn_as_a_bitmap_font() {
    let show = r##"{
      "name": "pixels", "size": [200, 100],
      "fonts": {
        "dots": { "file": "sans", "size": 20, "pixels": true, "color": "#FF8000" },
        "smooth": { "file": "sans", "size": 20 }
      },
      "layers": [
        { "name": "hard", "type": "text", "font": "dots", "text": "A1", "x": 10, "y": 20, "align": "top_left" },
        { "name": "soft", "type": "text", "font": "smooth", "text": "A1", "x": 10, "y": 60, "align": "top_left" }
      ]
    }"##;
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(show).unwrap();
    // Hard edges: a pixel is in or out, and the glyphs sit on whole
    // pixels of the canvas.
    let ([x, y, w, h], alphas) = bitmap(&engine, "hard");
    assert_eq!(alphas, [0, 255]);
    assert!(x.fract() == 0.0 && y.fract() == 0.0, "{x}, {y}");
    assert!(w > 10.0 && h > 8.0 && w < 40.0 && h < 30.0, "{w}x{h}");
    // At size 20 the two glyphs advance 12.8 and 11.4: rounded to whole
    // pixels, the raster is 13 wide plus the second glyph's ink.
    assert!((x - 10.0).abs() < 3.0, "{x}");
    // The same font, not asked for as pixels, stays an outline.
    let _ = run(&engine, "soft");
}

#[test]
fn on_a_pixel_grid_outline_fonts_are_pixels_unless_told_otherwise() {
    let show = r##"{
      "name": "grid", "size": [200, 100], "output": { "scaling": "pixel_perfect" },
      "fonts": {
        "plain": { "file": "sans", "size": 20 },
        "outlined": { "file": "sans", "size": 20, "pixels": false }
      },
      "layers": [
        { "name": "plain", "type": "text", "font": "plain", "text": "1", "x": 10, "y": 20 },
        { "name": "outlined", "type": "text", "font": "outlined", "text": "1", "x": 10, "y": 60 }
      ]
    }"##;
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(show).unwrap();
    let (_, alphas) = bitmap(&engine, "plain");
    assert_eq!(alphas, [0, 255]);
    let _ = run(&engine, "outlined");
    // A bitmap font is pixels already; asking is a mistake.
    let mut engine = Engine::new();
    let fnt = "info face=\"b\" size=3\ncommon lineHeight=4 base=3 scaleW=2 scaleH=3 pages=1\npage id=0 file=\"b_0.png\"\nchar id=49 x=0 y=0 width=2 height=3 xoffset=0 yoffset=0 xadvance=3 page=0\n";
    engine
        .set_font(
            "blocks",
            cuelight::BitmapFont::parse(fnt).unwrap(),
            vec![(2, 3, vec![255; 24])],
        )
        .unwrap();
    let bad = r##"{ "name": "b", "size": [8, 8], "fonts": { "f": { "file": "blocks", "pixels": true } } }"##;
    let err = engine.load_show(bad).unwrap_err().to_string();
    assert!(err.contains("remove pixels"), "{err}");
}

#[test]
fn a_shadow_blur_on_an_outline_font_is_said_to_be_drawn_hard() {
    let show = SHOW.replace(
        r##""shadow": { "color": "#00000080", "offset": [3, 2] }"##,
        r##""shadow": { "color": "#00000080", "offset": [3, 2], "blur": 4 }"##,
    );
    assert_ne!(show, SHOW);
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(&show).unwrap();
    let warnings = engine.load_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("\"cast\"") && w.contains("drawn hard")),
        "{warnings:?}"
    );
}

#[test]
fn a_press_hits_the_text_by_its_fonts_metrics() {
    // "A1" at size 100 from (10, 20): the line runs from x 10 to the end
    // of the 1's advance, 10 + 63.9 + 57.2 = 131.1, and from the top of
    // the ascent, y 20, to the bottom of the descent, 20 + 106.9 + 29.3.
    let show = r##"{ "name": "hit", "size": [400, 200],
      "fonts": { "big": { "file": "sans", "size": 100 } },
      "layers": [ { "name": "word", "type": "text", "font": "big", "text": "A1",
                    "x": 10, "y": 20, "align": "top_left", "press": { "trigger": "word" } } ] }"##;
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(show).unwrap();
    let hit = |x: f64, y: f64| engine.pressed([x, y]).and_then(|p| p.trigger);
    assert_eq!(hit(60.0, 80.0).as_deref(), Some("word"), "on the letters");
    assert_eq!(
        hit(130.0, 80.0).as_deref(),
        Some("word"),
        "at the end of the 1"
    );
    // Past the last advance, where a glyph's em square used to reach.
    assert_eq!(hit(150.0, 80.0), None, "right of the word");
    // Below the baseline, where a descender would be.
    assert_eq!(hit(60.0, 150.0).as_deref(), Some("word"), "in the descent");
    assert_eq!(hit(60.0, 160.0), None, "under the line");
    assert_eq!(hit(60.0, 18.0), None, "above the line");
}

#[test]
fn a_press_on_text_half_revealed_hits_only_what_shows() {
    // "A1" half revealed: only the A, from x 10 to 10 + 63.9.
    let show = r##"{ "name": "hit", "size": [400, 200],
      "fonts": { "big": { "file": "sans", "size": 100 } },
      "layers": [ { "name": "word", "type": "text", "font": "big", "text": "A1", "reveal": 0.5,
                    "x": 10, "y": 20, "align": "top_left", "press": { "trigger": "word" } } ] }"##;
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(show).unwrap();
    let hit = |x: f64| engine.pressed([x, 80.0]).and_then(|p| p.trigger);
    assert_eq!(hit(70.0).as_deref(), Some("word"), "on the A");
    assert_eq!(hit(80.0), None, "where the 1 is not drawn yet");
}

#[test]
fn text_in_an_outline_font_is_bounded_by_its_lines() {
    let show = r##"{ "name": "b", "size": [400, 200],
      "fonts": { "big": { "file": "sans", "size": 100 } },
      "layers": [ { "name": "word", "type": "text", "font": "big", "text": "A1",
                    "x": 10, "y": 20, "align": "top_left" } ] }"##;
    let mut engine = Engine::new();
    engine.set_outline_font("sans", FONT).unwrap();
    engine.load_show(show).unwrap();
    let path = cuelight_core::LayerPath::new(cuelight_core::Root::Show, vec![0]);
    let [x, y, w, h] = engine.bounds(&path).unwrap().rect;
    // From the A to the end of the 1's advance, ascent to descent.
    let close = |a: f64, b: f64| (a - b).abs() < 0.01;
    assert!(close(x, 10.0) && close(y, 20.0), "{x} {y}");
    assert!(close(w, 121.1) && close(h, 136.2), "{w} {h}");
}
