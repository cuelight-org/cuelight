//! `reveal`: how much of a text or digits layer's text shows.

use cuelight_core::{revealed, Engine, Property};

#[test]
fn a_share_shows_that_many_characters_rounded_down() {
    assert_eq!(revealed("typing", 0.0), 0);
    assert_eq!(revealed("typing", 0.5), 3);
    assert_eq!(revealed("typing", 0.99), 5);
    assert_eq!(revealed("typing", 1.0), 6);
    // Written as a fraction of the count, a share reaches the character
    // it means despite the float that carries it.
    assert_eq!(revealed("0123456789", 0.3), 3);
    assert_eq!(revealed("0123456789", 0.7), 7);
    // Every character counts: spaces and line breaks too.
    assert_eq!(revealed("a b\nc", 0.6), 3);
    // Out of range shows none or all; nothing has no characters.
    assert_eq!(revealed("typing", -1.0), 0);
    assert_eq!(revealed("typing", 4.0), 6);
    assert_eq!(revealed("typing", f64::NAN), 6);
    assert_eq!(revealed("", 0.5), 0);
}

#[test]
fn reveal_is_a_numeric_property_of_text_and_digits_layers() {
    let show = r##"{ "name": "t", "size": [64, 8], "fonts": { "f": { "file": "f" } },
      "variables": { "line": 0.25 },
      "layers": [
        { "name": "typed", "type": "text", "text": "hello", "font": "f", "reveal": 0,
          "timelines": [{ "name": "type", "autoplay": true, "hold": true,
            "tracks": [{ "property": "reveal", "keys": [{ "t": 0, "v": 0 }, { "t": 1, "v": 1 }] }] }] },
        { "name": "score", "type": "digits", "digits": 4, "size": [32, 8], "text": "1234",
          "display": { "segments": { "style": "numeric7", "fill": "#FFFFFF" } },
          "bindings": [{ "property": "reveal", "variable": "line" }] },
        { "name": "whole", "type": "text", "text": "as is", "font": "f" } ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    let reveal = |engine: &Engine, name: &str| {
        engine
            .values()
            .unwrap()
            .into_iter()
            .find(|row| row.name == name && row.property == Property::Reveal)
            .map(|row| row.value.as_number())
            .unwrap_or_else(|| panic!("no reveal on {name}"))
    };
    // Unset, everything shows.
    assert_eq!(reveal(&engine, "whole"), 1.0);
    // Keyframed like any number.
    assert_eq!(reveal(&engine, "typed"), 0.0);
    engine.advance_to(0.5);
    assert!((reveal(&engine, "typed") - 0.5).abs() < 1e-9);
    engine.advance_to(2.0);
    assert_eq!(reveal(&engine, "typed"), 1.0);
    // Bound like any number.
    assert_eq!(reveal(&engine, "score"), 0.25);
    engine.set_variable("line", 0.75);
    assert_eq!(reveal(&engine, "score"), 0.75);
    // A shape has no text to reveal.
    let bad = r##"{ "name": "t", "size": [8, 8], "layers": [
      { "name": "box", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
        "bindings": [{ "property": "reveal", "variable": "line" }] } ] }"##;
    assert!(Engine::new().load_show(bad).is_err());
}
