//! Parts: elements of vector artwork a show moves on their own.

// Test code throughout, so clippy lets it panic as tests do.
#![cfg(test)]

use cuelight::{Engine, ResolvedShape, Vector, VectorPath};
use cuelight_core::PathElement;

/// A wolf: a body square, a head square inside a `head` group, and a
/// jaw square inside the head, each 10 by 10 in a 100 by 100 picture.
fn wolf() -> Vector {
    let square = |x: f64, y: f64, ids: &[&str]| VectorPath {
        elements: vec![
            PathElement::MoveTo([x, y]),
            PathElement::LineTo([x + 10.0, y]),
            PathElement::LineTo([x + 10.0, y + 10.0]),
            PathElement::LineTo([x, y + 10.0]),
            PathElement::Close,
        ],
        fill: Some([255, 255, 255, 255]),
        stroke: None,
        ids: ids.iter().map(|s| (*s).to_owned()).collect(),
        gradient: None,
    };
    Vector {
        width: 100.0,
        height: 100.0,
        paths: vec![
            square(0.0, 50.0, &["body"]),
            square(50.0, 0.0, &["head"]),
            square(50.0, 20.0, &["head", "jaw"]),
        ],
    }
}

/// The first point of each path drawn for the layer, in canvas units.
fn firsts(engine: &Engine, name: &str) -> Vec<[f64; 2]> {
    engine
        .resolved_layers()
        .unwrap()
        .iter()
        .filter(|l| l.name == name)
        .map(|l| match &l.shape {
            ResolvedShape::Path { elements, .. } => match elements[0] {
                PathElement::MoveTo(p) => p,
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        })
        .collect()
}

const SHOW: &str = r##"{ "name": "parts", "size": [200, 200],
  "variables": { "wag": 0, "open": 0 },
  "layers": [
    { "name": "wolf", "type": "image", "image": "wolf", "x": 100, "y": 100, "parts": [
        { "id": "jaw", "pivot": [50, 20], "bindings": [{ "property": "rotation", "variable": "open" }] },
        { "id": "head", "pivot": [50, 0], "bindings": [{ "property": "x", "variable": "wag" }] },
        { "id": "body", "visible": false } ] } ] }"##;

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_vector("wolf", wolf()).unwrap();
    engine.load_show(SHOW).unwrap();
    engine
}

#[test]
fn a_part_moves_its_element_in_artwork_coordinates_around_its_pivot() {
    let mut engine = engine();
    // At rest: the head and jaw where the SVG put them, the body hidden.
    assert_eq!(firsts(&engine, "wolf"), [[150.0, 100.0], [150.0, 120.0]]);
    // The jaw turns a quarter clockwise around (50, 20): its first
    // corner, at the pivot, stays; the square swings to the left.
    engine.set_variable("open", 90.0);
    let drawn = firsts(&engine, "wolf");
    assert_eq!(drawn[0], [150.0, 100.0]);
    assert!((drawn[1][0] - 150.0).abs() < 1e-9 && (drawn[1][1] - 120.0).abs() < 1e-9);
    let all: Vec<PathElement> = engine
        .resolved_layers()
        .unwrap()
        .iter()
        .filter_map(|l| match &l.shape {
            ResolvedShape::Path { elements, .. } => Some(elements.clone()),
            _ => None,
        })
        .nth(1)
        .unwrap();
    // The corner that was 10 to the right of the pivot is now 10 below.
    let PathElement::LineTo([x, y]) = all[1] else {
        panic!("{all:?}")
    };
    assert!(
        (x - 150.0).abs() < 1e-9 && (y - 130.0).abs() < 1e-9,
        "{x}, {y}"
    );
}

#[test]
fn a_part_inside_a_part_moves_with_both() {
    let mut engine = engine();
    // Moving the head carries the jaw, whose own pivot moves with it.
    engine.set_variable("wag", 5.0);
    assert_eq!(firsts(&engine, "wolf"), [[155.0, 100.0], [155.0, 120.0]]);
    engine.set_variable("open", 90.0);
    let drawn = firsts(&engine, "wolf");
    assert!((drawn[1][0] - 155.0).abs() < 1e-9 && (drawn[1][1] - 120.0).abs() < 1e-9);
}

#[test]
fn parts_are_layers_of_the_artwork_and_are_written_back_as_parts() {
    let engine = engine();
    let show = engine.show().unwrap();
    let wolf = &show.layers[0];
    assert!(wolf.holds_parts());
    let names: Vec<&str> = wolf.children().iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["jaw", "head", "body"]);
    // Their properties resolve like any layer's, by path.
    let rows = engine.values().unwrap();
    assert!(rows
        .iter()
        .any(|r| r.name == "jaw" && r.property == cuelight_core::Property::Rotation));
    // A document round trip keeps the parts as parts.
    let json = serde_json::to_value(show).unwrap();
    let parts = &json["layers"][0]["parts"];
    assert_eq!(parts[0]["id"], "jaw");
    assert_eq!(parts[0]["pivot"], serde_json::json!([50.0, 20.0]));
    assert!(parts[0].get("type").is_none());
    assert!(parts[0].get("name").is_none());
    // Timelines on a part answer to their triggers like any layer's.
    let with_timeline = SHOW.replace(
        r#"{ "id": "body", "visible": false }"#,
        r#"{ "id": "body", "timelines": [{ "name": "bounce", "trigger": "hop", "tracks": [] }] }"#,
    );
    let mut engine = Engine::new();
    engine.load_show(&with_timeline).unwrap();
    assert!(engine.show().unwrap().triggers().contains("hop"));
}

#[test]
fn a_part_belongs_only_to_an_artwork_layer_and_names_what_the_artwork_has() {
    let mut engine = Engine::new();
    let stray = r##"{ "name": "s", "size": [8, 8], "layers": [
      { "name": "g", "type": "group", "children": [ { "name": "x", "type": "part", "id": "x" } ] } ] }"##;
    let err = engine.load_show(stray).unwrap_err().to_string();
    assert!(err.contains("is a part"), "{err}");
    // A part naming an id the registered artwork lacks is a warning, not
    // an error: the artwork is a host asset and may still arrive.
    engine.set_vector("wolf", wolf()).unwrap();
    let unknown = SHOW.replace(
        r#"{ "id": "body", "visible": false }"#,
        r#"{ "id": "nose" }"#,
    );
    engine.load_show(&unknown).unwrap();
    let warnings = engine.load_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("\"nose\"")),
        "{warnings:?}"
    );
}

#[test]
fn a_part_strokes_its_paths_in_its_own_space() {
    use cuelight::Transform;
    let outlined = VectorPath {
        elements: vec![
            PathElement::MoveTo([40.0, 40.0]),
            PathElement::LineTo([60.0, 40.0]),
            PathElement::LineTo([60.0, 60.0]),
            PathElement::Close,
        ],
        fill: None,
        stroke: Some(([0, 0, 0, 255], 2.0)),
        ids: vec!["eye".to_owned()],
        gradient: None,
    };
    let art = Vector {
        width: 100.0,
        height: 100.0,
        paths: vec![outlined],
    };
    let stroke = |scale: &str| {
        let show = format!(
            r##"{{ "name": "s", "size": [100, 100], "layers": [
              {{ "name": "face", "type": "image", "image": "face", "parts": [
                {{ "id": "eye", "pivot": [50, 50], {scale} }} ] }} ] }}"##
        );
        let mut engine = Engine::new();
        engine.set_vector("face", art.clone()).unwrap();
        engine.load_show(&show).unwrap();
        match &engine.resolved_layers().unwrap()[0].shape {
            ResolvedShape::Path {
                stroke,
                stroke_space,
                ..
            } => (stroke.unwrap().1, *stroke_space),
            other => panic!("{other:?}"),
        }
    };
    // The width is the artwork's; the part's scale is the space the
    // outline is drawn in, so squashed to blink it squashes the outline.
    assert_eq!(stroke(r#""scale": 1"#), (2.0, None));
    assert_eq!(
        stroke(r#""scale": 2.5"#),
        (2.0, Some(Transform([2.5, 0.0, 0.0, 2.5, 0.0, 0.0])))
    );
    assert_eq!(
        stroke(r#""scale_y": 0.1"#),
        (2.0, Some(Transform([1.0, 0.0, 0.0, 0.1, 0.0, 0.0])))
    );
}

#[test]
fn an_artwork_gradient_is_placed_with_its_path() {
    use cuelight::{ResolvedGradient, ResolvedGradientKind};
    let art = Vector {
        width: 100.0,
        height: 100.0,
        paths: vec![VectorPath {
            elements: vec![
                PathElement::MoveTo([0.0, 0.0]),
                PathElement::LineTo([10.0, 0.0]),
                PathElement::LineTo([10.0, 10.0]),
                PathElement::Close,
            ],
            fill: Some([255, 0, 0, 255]),
            stroke: None,
            ids: vec!["sun".to_owned()],
            gradient: Some(ResolvedGradient {
                kind: ResolvedGradientKind::Radial {
                    center: [5.0, 5.0],
                    radius: 5.0,
                },
                stops: vec![(0.0, [255, 255, 255, 255]), (1.0, [255, 0, 0, 255])],
                space: cuelight::Transform::IDENTITY,
                straight_alpha: false,
            }),
        }],
    };
    let show = r##"{ "name": "g", "size": [400, 400], "layers": [
      { "name": "icon", "type": "image", "image": "icon", "x": 100, "size": [200, 200],
        "parts": [ { "id": "sun", "x": 10, "pivot": [5, 5], "scale": 2 } ] } ] }"##;
    let mut engine = Engine::new();
    engine.set_vector("icon", art).unwrap();
    engine.load_show(show).unwrap();
    let layer = &engine.resolved_layers().unwrap()[0];
    let gradient = layer.gradient.as_ref().unwrap();
    // Kept in its own coordinates, with where it went in its space: the
    // artwork is drawn twice its size at x 100, and the part moves the
    // sun 10 across and doubles it about its centre.
    assert_eq!(
        gradient.kind,
        ResolvedGradientKind::Radial {
            center: [5.0, 5.0],
            radius: 5.0
        }
    );
    let centre = gradient.space.apply([5.0, 5.0]);
    let rim = gradient.space.apply([10.0, 5.0]);
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    assert!(close(centre, [100.0 + 15.0 * 2.0, 10.0]), "{centre:?}");
    assert!(
        (rim[0] - centre[0] - 5.0 * 2.0 * 2.0).abs() < 1e-9,
        "{rim:?}"
    );
}
