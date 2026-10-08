//! Components: layers written once and used by name.

// Test code throughout, so clippy lets it panic as tests do.
#![cfg(test)]

use cuelight_core::{Engine, Error};

/// The show `json` loads as, written out again.
fn loaded(json: &str) -> serde_json::Value {
    let mut engine = Engine::new();
    engine.load_show(json).unwrap();
    serde_json::to_value(engine.show().unwrap()).unwrap()
}

fn refused(json: &str) -> String {
    match Engine::new().load_show(json) {
        Err(Error::InvalidShow(message)) => message,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_component_is_a_group_of_its_layers_with_the_inputs_filled_in() {
    let used = loaded(
        r##"{ "name": "s", "size": [64, 64],
          "components": { "lamp": {
            "inputs": ["color", "size", "label"],
            "layers": [
              { "name": "bulb", "type": "shape", "shape": { "circle": [0, 0, "{size}"] }, "fill": "{color}" },
              { "name": "label", "type": "text", "font": "small", "text": "Lamp {label}", "y": "{size}" }
            ] } },
          "fonts": { "small": { "file": "f", "size": 8 } },
          "layers": [
            { "name": "left", "type": "component", "component": "lamp", "x": 10, "opacity": 0.5,
              "with": { "color": "#FF0000", "size": 4, "label": 1 } }
          ] }"##,
    );
    let written = loaded(
        r##"{ "name": "s", "size": [64, 64],
          "fonts": { "small": { "file": "f", "size": 8 } },
          "layers": [
            { "name": "left", "type": "group", "x": 10, "opacity": 0.5, "children": [
              { "name": "bulb", "type": "shape", "shape": { "circle": [0, 0, 4] }, "fill": "#FF0000" },
              { "name": "label", "type": "text", "font": "small", "text": "Lamp 1", "y": 4 }
            ] }
          ] }"##,
    );
    assert_eq!(used, written);
}

#[test]
fn components_are_used_in_scenes_and_inside_each_other() {
    let used = loaded(
        r##"{ "name": "s", "size": [64, 64],
          "components": {
            "dot": { "inputs": ["x"], "layers": [
              { "name": "dot", "type": "shape", "x": "{x}", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF" } ] },
            "pair": { "inputs": ["at"], "layers": [
              { "name": "a", "type": "component", "component": "dot", "with": { "x": "{at}" } },
              { "name": "b", "type": "component", "component": "dot", "y": 2, "with": { "x": "{at}" } } ] }
          },
          "scenes": [ { "name": "one", "layers": [
            { "name": "p", "type": "component", "component": "pair", "with": { "at": 3 } } ] } ] }"##,
    );
    let dot = serde_json::json!({ "name": "dot", "type": "shape", "x": 3.0 });
    let layers = &used["scenes"][0]["layers"][0]["children"];
    for child in [&layers[0]["children"][0], &layers[1]["children"][0]] {
        assert_eq!((&child["name"], &child["x"]), (&dot["name"], &dot["x"]));
    }
    assert_eq!(layers[1]["y"], 2.0);
}

#[test]
fn an_instance_gives_every_input_and_nothing_else() {
    let show = |with: &str| {
        format!(
            r#"{{ "name": "s", "size": [8, 8],
              "components": {{ "c": {{ "inputs": ["a"], "layers": [] }} }},
              "layers": [ {{ "name": "x", "type": "component", "component": "c", "with": {with} }} ] }}"#
        )
    };
    assert_eq!(
        refused(&show("{}")),
        "layers[0]: `with` gives no value for input `a`"
    );
    assert_eq!(
        refused(&show(r#"{ "a": 1, "b": 2 }"#)),
        "layers[0]: component `c` has no input `b`"
    );
}

#[test]
fn a_placeholder_that_is_no_input_is_refused() {
    let message = refused(
        r#"{ "name": "s", "size": [8, 8],
          "components": { "c": { "inputs": ["heading"], "layers": [
            { "name": "t", "type": "text", "font": "f", "text": "{heding}" } ] } } }"#,
    );
    assert_eq!(
        message,
        "components.c: `{heding}` is not one of the component's inputs"
    );
}

#[test]
fn braces_around_anything_but_a_name_are_text() {
    let used = loaded(
        r#"{ "name": "s", "size": [8, 8],
          "fonts": { "f": { "file": "f", "size": 8 } },
          "components": { "c": { "layers": [
            { "name": "t", "type": "text", "font": "f", "text": "{ } {1} {" } ] } },
          "layers": [ { "name": "x", "type": "component", "component": "c" } ] }"#,
    );
    assert_eq!(used["layers"][0]["children"][0]["text"], "{ } {1} {");
}

#[test]
fn a_component_that_uses_itself_is_refused() {
    let message = refused(
        r#"{ "name": "s", "size": [8, 8],
          "components": {
            "a": { "layers": [ { "name": "b", "type": "component", "component": "b" } ] },
            "b": { "layers": [ { "name": "a", "type": "component", "component": "a" } ] } },
          "layers": [ { "name": "x", "type": "component", "component": "a" } ] }"#,
    );
    assert_eq!(
        message,
        "layers[0].children[0].children[0]: component `a` uses itself"
    );
}

#[test]
fn a_tolerant_load_leaves_an_unusable_instance_empty_and_says_where() {
    let mut engine = Engine::new();
    let findings = engine
        .load_show_tolerant(
            r##"{ "name": "s", "size": [8, 8],
              "components": { "c": { "layers": [
                { "name": "dot", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF" } ] } },
              "layers": [
                { "name": "fine", "type": "component", "component": "c" },
                { "name": "lost", "type": "component", "component": "missing" } ] }"##,
        )
        .unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].path, "layers[1]");
    assert_eq!(findings[0].message, "there is no component named `missing`");
    let show = engine.show().unwrap();
    assert_eq!(show.layers.len(), 2);
    assert_eq!(show.layers[1].name, "lost");
}

#[test]
fn the_documented_caption_loads() {
    let used = loaded(
        r#"{ "name": "s", "size": [1280, 720],
          "fonts": { "heading": { "file": "f" }, "body": { "file": "f" } },
          "components": {
            "caption": {
              "inputs": ["heading", "body"],
              "layers": [
                { "name": "heading", "type": "text", "font": "heading", "text": "{heading}" },
                { "name": "body", "type": "text", "font": "body", "y": 40, "text": "{body}" }
              ]
            }
          },
          "layers": [
            { "name": "totality", "type": "component", "component": "caption",
              "x": 716, "y": 546, "opacity": 0,
              "with": { "heading": "Totality", "body": "The moon covers the whole sun." },
              "timelines": [ { "name": "in", "trigger": "c2", "delay": 0.5, "hold": true,
                "tracks": [ { "property": "opacity", "keys": [ { "t": 0, "v": 0 }, { "t": 0.4, "v": 1 } ] } ] } ] }
          ] }"#,
    );
    let caption = &used["layers"][0];
    assert_eq!(caption["type"], "group");
    assert_eq!(
        caption["children"][1]["text"],
        "The moon covers the whole sun."
    );
    assert_eq!(caption["timelines"][0]["trigger"], "c2");
}
