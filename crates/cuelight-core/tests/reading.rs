//! How a variable is read: the steps between a value and what a binding
//! makes of it.

use cuelight_core::{Engine, Property, Value};

/// What `layer`'s `x` resolves to.
fn x_of(engine: &Engine, layer: &str) -> f64 {
    engine
        .values()
        .unwrap()
        .into_iter()
        .find(|row| row.name == layer && row.property == Property::X)
        .map(|row| row.value.as_number())
        .unwrap()
}

#[test]
fn a_threshold_reads_as_the_curve_it_stands_for() {
    // The same input, read by a threshold and by the two-key step curve
    // the docs say it is: the two must agree everywhere, the level
    // itself included.
    let show = r##"{ "name": "t", "size": [8, 8], "layers": [
        { "name": "short", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "bindings": [{ "property": "x", "variable": "level", "threshold": 0.5,
                         "scale": 10, "offset": 1 }] },
        { "name": "long", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "bindings": [{ "property": "x", "variable": "level",
                         "curve": [{ "t": 0, "v": 0 }, { "t": 0.5, "v": 1, "ease": "step" }],
                         "scale": 10, "offset": 1 }] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    for level in [-3.0, -0.001, 0.0, 0.25, 0.4999, 0.5, 0.5001, 1.0, 7.0] {
        engine.set_variable("level", Value::Number(level));
        engine.advance_to(engine.time());
        let (short, long) = (x_of(&engine, "short"), x_of(&engine, "long"));
        assert_eq!(short, long, "at {level}");
        assert_eq!(short, if level >= 0.5 { 11.0 } else { 1.0 }, "at {level}");
    }
}

/// A show whose `flash` timeline starts on `lamp` becoming true, read
/// with `extra` on the condition, and whose `x` says whether it ran.
fn flashing(extra: &str) -> Engine {
    let show = format!(
        r##"{{ "name": "t", "size": [8, 8], "layers": [
        {{ "name": "lamp", "type": "shape", "shape": {{ "rect": [0, 0, 1, 1] }}, "fill": "#FFFFFF",
          "timelines": [{{ "name": "flash", "when": {{ "variable": "lamp"{extra} }}, "hold": true,
            "tracks": [{{ "property": "x", "keys": [{{ "t": 0, "v": 5 }}, {{ "t": 1, "v": 9 }}] }}] }}] }}
      ] }}"##
    );
    let mut engine = Engine::new();
    engine.load_show(&show).unwrap();
    engine
}

#[test]
fn a_condition_is_debounced_like_a_binding() {
    let mut engine = flashing(r#", "debounce": 0.1"#);
    // The first value a reading sees settles at once; the flicker comes
    // after it.
    engine.set_variable("lamp", Value::Number(0.0));
    engine.advance_frame(0.05);
    // A flicker shorter than the debounce is never seen: nothing starts.
    engine.set_variable("lamp", Value::Number(1.0));
    engine.advance_frame(0.05);
    engine.set_variable("lamp", Value::Number(0.0));
    engine.advance_frame(0.05);
    assert_eq!(x_of(&engine, "lamp"), 0.0, "a flicker starts nothing");
    // Held past it, the condition becomes true and the timeline runs.
    engine.set_variable("lamp", Value::Number(1.0));
    engine.advance_frame(0.05);
    assert_eq!(x_of(&engine, "lamp"), 0.0, "not yet held long enough");
    engine.advance_frame(0.1);
    assert!(x_of(&engine, "lamp") > 5.0, "started once the value held");
}

#[test]
fn a_condition_is_bent_through_a_curve() {
    // A curve that is 0 until 3 and 1 from 3 on, written long.
    let mut engine =
        flashing(r#", "curve": [{ "t": 0, "v": 0 }, { "t": 3, "v": 1, "ease": "step" }]"#);
    engine.set_variable("lamp", Value::Number(2.0));
    engine.advance_frame(0.1);
    assert_eq!(x_of(&engine, "lamp"), 0.0, "below the curve's step");
    engine.set_variable("lamp", Value::Number(3.0));
    engine.advance_frame(0.1);
    assert!(x_of(&engine, "lamp") >= 5.0, "at it");
}

#[test]
fn a_reading_is_checked_the_same_wherever_it_is() {
    let bad = |extra: &str| {
        let show = format!(
            r##"{{ "name": "t", "size": [8, 8], "layers": [
            {{ "name": "l", "type": "shape", "shape": {{ "rect": [0, 0, 1, 1] }}, "fill": "#FFFFFF",
              "timelines": [{{ "name": "flash", "when": {{ "variable": "lamp"{extra} }}, "tracks": [] }}] }}
          ] }}"##
        );
        Engine::new().load_show(&show).unwrap_err().to_string()
    };
    assert!(bad(r#", "debounce": -1"#).contains("debounce of 0 or more"));
    assert!(bad(r#", "threshold": 0.5, "curve": [{ "t": 0, "v": 0 }]"#).contains("same job"));
    assert!(
        bad(r#", "curve": [{ "t": 1, "v": 0 }, { "t": 0, "v": 1 }]"#).contains("in order of input")
    );
}

#[test]
fn the_stages_of_a_binding_are_what_the_engine_applies() {
    use cuelight_core::{Show, Value};
    // A text binding with every stage in play: read through a map, bent,
    // scaled, offset, formatted and worded.
    let show = r##"{ "name": "t", "size": [8, 8], "variables": { "gear": "high" },
      "layers": [{ "name": "readout", "type": "text", "font": "f", "text": "",
        "bindings": [{ "property": "text", "variable": "gear",
          "map": { "low": 10, "high": 40 }, "default": 0,
          "curve": [{ "t": 0, "v": 0 }, { "t": 100, "v": 50 }],
          "scale": 2, "offset": 1, "decimals": 1, "suffix": " km/h" }] }],
      "fonts": { "f": { "file": "none" } } }"##;
    let parsed: Show = serde_json::from_str(show).unwrap();
    let binding = &parsed.layers[0].bindings[0];
    let stages = binding.stages(Value::Text("high".into()), &parsed);
    assert_eq!(stages.mapped, Some(Value::Number(40.0)));
    assert_eq!(stages.bent, Some(20.0));
    assert_eq!(stages.scaled, Some(41.0));
    assert_eq!(stages.output, Some(Value::Text("41.0 km/h".into())));
    // And the engine says the same of the layer.
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    let text = engine
        .values()
        .unwrap()
        .into_iter()
        .find(|row| row.property == Property::Text)
        .unwrap()
        .value;
    assert_eq!(Some(text), stages.output);
    // A value the map does not list stops at the default.
    let stages = binding.stages(Value::Text("reverse".into()), &parsed);
    assert_eq!(stages.mapped, Some(Value::Number(0.0)));
    assert_eq!(stages.output, Some(Value::Text("1.0 km/h".into())));
}

#[test]
fn a_transitions_step_response_is_what_the_engine_shows() {
    use cuelight_core::Show;
    // A lamp driven from cold to full power: the binding's opacity at
    // 50 ms is the filament's response at 50 ms.
    let show = r##"{ "name": "t", "size": [8, 8], "variables": { "power": 1 },
      "layers": [{ "name": "bulb", "type": "shape", "shape": { "rect": [0, 0, 1, 1] },
        "fill": "#FFFFFF", "opacity": 0,
        "bindings": [{ "property": "opacity", "variable": "power",
          "transition": { "model": "incandescent" } }] }] }"##;
    let parsed: Show = serde_json::from_str(show).unwrap();
    let transition = parsed.layers[0].bindings[0].transition.as_ref().unwrap();
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    engine.advance_to(0.05);
    let shown = engine
        .values()
        .unwrap()
        .into_iter()
        .find(|row| row.property == Property::Opacity)
        .unwrap()
        .value
        .as_number();
    assert!(shown > 0.0 && shown < 1.0, "{shown}: on its way up");
    assert!((shown - transition.step_response(0.0, 1.0, 0.05)).abs() < 1e-12);
    // An eased one is the ease over its duration.
    let eased: cuelight_core::Transition =
        serde_json::from_str(r#"{ "duration": 2, "ease": "linear" }"#).unwrap();
    assert_eq!(eased.step_response(10.0, 20.0, 0.5), 12.5);
}

#[test]
fn a_condition_follows_a_value_the_show_animates_from_the_instant_it_turns() {
    // No host variable: the condition follows the show's own value, which
    // crosses the mark at exactly half a second, inside the one frame
    // taken. The slide it starts is timed from there, not from the
    // frame's end.
    let show = r##"{ "name": "t", "size": [8, 8],
      "values": { "rise": { "timelines": [{ "name": "up", "autoplay": true, "hold": true,
        "keys": [{ "t": 0, "v": 0 }, { "t": 1, "v": 1 }] }] } },
      "layers": [
        { "name": "lamp", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "timelines": [{ "name": "flash", "when": { "variable": "rise", "threshold": 0.5 }, "hold": true,
            "tracks": [{ "property": "x", "keys": [{ "t": 0, "v": 5 }, { "t": 1, "v": 9 }] }] }] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    engine.advance_to(0.25);
    assert_eq!(x_of(&engine, "lamp"), 0.0);
    engine.advance_to(0.75);
    assert!(
        (x_of(&engine, "lamp") - 6.0).abs() < 1e-5,
        "{}",
        x_of(&engine, "lamp")
    );
}
