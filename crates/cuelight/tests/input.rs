//! Keys and presses: what a show makes of what a host does to it.

use cuelight::{Engine, ResolvedShape};

/// A show with two pressable boxes, one over the other, and keys.
const SHOW: &str = r##"{ "name": "in", "size": [64, 32],
  "input": { "keys": { "ArrowRight": "next", "ArrowLeft": "prev", " ": "next" },
             "press": "anywhere" },
  "layers": [
    { "name": "under", "type": "shape", "shape": { "rect": [0, 0, 32, 32] },
      "fill": "#FF0000", "press": { "trigger": "under" } },
    { "name": "over", "type": "shape", "shape": { "circle": [16, 16, 8] },
      "fill": "#00FF00", "press": { "trigger": "over" } },
    { "name": "quiet", "type": "shape", "shape": { "rect": [40, 0, 24, 32] },
      "fill": "#0000FF" }
  ] }"##;

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.load_show(SHOW).unwrap();
    engine
}

#[test]
fn a_key_fires_what_the_show_says_it_means() {
    let mut engine = engine();
    assert_eq!(engine.key("ArrowRight").as_deref(), Some("next"));
    assert_eq!(engine.key(" ").as_deref(), Some("next"), "the space bar");
    assert_eq!(engine.key("ArrowLeft").as_deref(), Some("prev"));
    // A key the show says nothing about does nothing, and a name is
    // matched as written.
    assert_eq!(engine.key("Enter"), None);
    assert_eq!(engine.key("arrowright"), None);
}

#[test]
fn a_press_finds_the_topmost_layer_under_it() {
    let mut engine = engine();
    // The circle is drawn over the rect, and wins where they overlap.
    assert_eq!(engine.press([16.0, 16.0]).as_deref(), Some("over"));
    // Beside the circle but inside the rect: the rect. A circle is
    // round, so its corner is not part of it.
    assert_eq!(engine.press([2.0, 2.0]).as_deref(), Some("under"));
    assert_eq!(engine.press([9.0, 9.0]).as_deref(), Some("under"));
    // Over a layer that is not pressable: the show's own press.
    assert_eq!(engine.press([50.0, 16.0]).as_deref(), Some("anywhere"));
}

#[test]
fn asking_what_is_under_a_point_fires_nothing() {
    let engine = engine();
    assert_eq!(engine.pressed([16.0, 16.0]).as_deref(), Some("over"));
    // Only what a layer says, never the show's fallback: a host asking
    // wants to know whether there is something there.
    assert_eq!(engine.pressed([50.0, 16.0]), None);
}

#[test]
fn a_layer_that_is_not_shown_is_not_pressed() {
    let show = SHOW.replace(
        r##""fill": "#00FF00", "press": { "trigger": "over" }"##,
        r##""fill": "#00FF00", "visible": false, "press": { "trigger": "over" }"##,
    );
    let mut engine = Engine::new();
    engine.load_show(&show).unwrap();
    assert_eq!(engine.press([16.0, 16.0]).as_deref(), Some("under"));
}

#[test]
fn a_press_is_cut_off_by_the_clip_over_it() {
    // The pressable box is twice the window it shows through, so half
    // of it is not there to press.
    let show = r##"{ "name": "in", "size": [64, 32], "layers": [
        { "name": "window", "type": "group", "clip": { "rect": [0, 0, 16, 32] },
          "children": [
            { "name": "wide", "type": "shape", "shape": { "rect": [0, 0, 32, 32] },
              "fill": "#FFFFFF", "press": { "trigger": "hit" } } ] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    assert_eq!(engine.press([8.0, 16.0]).as_deref(), Some("hit"));
    assert_eq!(engine.press([24.0, 16.0]), None, "outside the window");
}

#[test]
fn a_press_follows_a_layer_that_turned() {
    // A long bar turned a quarter turn about its middle: pressing where
    // it now is hits it, and where it was does not.
    let show = r##"{ "name": "in", "size": [64, 64], "layers": [
        { "name": "bar", "type": "shape", "shape": { "rect": [-20, -4, 40, 8] },
          "x": 32, "y": 32, "rotation": 90, "fill": "#FFFFFF",
          "press": { "trigger": "bar" } }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    assert!(matches!(
        engine.resolved_layers().unwrap()[0].shape,
        ResolvedShape::Rect { .. }
    ));
    assert_eq!(engine.press([32.0, 48.0]).as_deref(), Some("bar"), "along");
    assert_eq!(engine.press([48.0, 32.0]), None, "across");
}

#[test]
fn a_press_moves_with_what_it_presses() {
    // The lever slides right over a second; pressing where it started
    // stops working once it has left.
    let show = r##"{ "name": "in", "size": [64, 32], "layers": [
        { "name": "lever", "type": "shape", "shape": { "rect": [0, 0, 8, 32] },
          "fill": "#FFFFFF", "press": { "trigger": "pull" },
          "timelines": [{ "name": "slide", "autoplay": true,
            "hold": true,
            "tracks": [{ "property": "x", "keys": [{ "t": 0, "v": 0 },
                                                   { "t": 1, "v": 40 }] }] }] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    assert_eq!(engine.press([4.0, 16.0]).as_deref(), Some("pull"));
    engine.advance_to(1.0);
    assert_eq!(engine.press([4.0, 16.0]), None, "it has gone");
    assert_eq!(engine.press([44.0, 16.0]).as_deref(), Some("pull"));
}
