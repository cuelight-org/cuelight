//! Parts: elements of vector artwork a show moves on their own.

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
fn a_part_scales_the_strokes_of_its_paths() {
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
            ResolvedShape::Path { stroke, .. } => stroke.unwrap().1,
            other => panic!("{other:?}"),
        }
    };
    assert!((stroke(r#""scale": 1"#) - 2.0).abs() < 1e-9);
    assert!((stroke(r#""scale": 2.5"#) - 5.0).abs() < 1e-9);
    // Squashed to a tenth of its height to blink: the average of the axes.
    assert!((stroke(r#""scale_y": 0.1"#) - 1.1).abs() < 1e-9);
}
