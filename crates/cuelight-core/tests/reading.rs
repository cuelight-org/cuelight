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
