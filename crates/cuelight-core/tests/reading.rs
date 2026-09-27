//! How a variable is read: the steps between a value and what a binding
//! makes of it.

use cuelight_core::{Engine, Property, Value};

/// What `layer`'s `x` resolves to.
fn x_of(engine: &Engine, layer: &str) -> f64 {
    engine
        .values()
        .unwrap()
        .into_iter()
        .find(|(name, prop, _)| name == layer && *prop == Property::X)
        .map(|(_, _, value)| value.as_number())
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
