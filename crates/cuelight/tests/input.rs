//! Keys, presses and the pointer: what a show makes of what a host does
//! to it.

// Test code throughout, so clippy lets it panic as tests do.
#![cfg(test)]

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
    assert_eq!(
        engine
            .press([16.0, 16.0])
            .and_then(|p| p.trigger)
            .as_deref(),
        Some("over")
    );
    // Beside the circle but inside the rect: the rect. A circle is
    // round, so its corner is not part of it.
    assert_eq!(
        engine.press([2.0, 2.0]).and_then(|p| p.trigger).as_deref(),
        Some("under")
    );
    assert_eq!(
        engine.press([9.0, 9.0]).and_then(|p| p.trigger).as_deref(),
        Some("under")
    );
    // Over a layer that is not pressable: the show's own press.
    assert_eq!(
        engine
            .press([50.0, 16.0])
            .and_then(|p| p.trigger)
            .as_deref(),
        Some("anywhere")
    );
}

#[test]
fn asking_what_is_under_a_point_fires_nothing() {
    let engine = engine();
    assert_eq!(
        engine
            .pressed([16.0, 16.0])
            .and_then(|p| p.trigger)
            .as_deref(),
        Some("over")
    );
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
    assert_eq!(
        engine
            .press([16.0, 16.0])
            .and_then(|p| p.trigger)
            .as_deref(),
        Some("under")
    );
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
    assert_eq!(
        engine.press([8.0, 16.0]).and_then(|p| p.trigger).as_deref(),
        Some("hit")
    );
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
    assert_eq!(
        engine
            .press([32.0, 48.0])
            .and_then(|p| p.trigger)
            .as_deref(),
        Some("bar"),
        "along"
    );
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
    assert_eq!(
        engine.press([4.0, 16.0]).and_then(|p| p.trigger).as_deref(),
        Some("pull")
    );
    engine.advance_to(1.0);
    assert_eq!(engine.press([4.0, 16.0]), None, "it has gone");
    assert_eq!(
        engine
            .press([44.0, 16.0])
            .and_then(|p| p.trigger)
            .as_deref(),
        Some("pull")
    );
}

#[test]
fn a_layer_that_is_not_pressable_does_not_stop_a_press() {
    // An opaque panel over the button, drawn on top and not pressable:
    // see-through for a press.
    let show = r##"{ "name": "in", "size": [64, 32], "layers": [
      { "name": "button", "type": "shape", "shape": { "rect": [0, 0, 32, 32] },
        "fill": "#FF0000", "press": { "trigger": "go" } },
      { "name": "panel", "type": "shape", "shape": { "rect": [0, 0, 64, 32] },
        "fill": "#000000" }
    ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    let pressed = engine.press([16.0, 16.0]).unwrap();
    assert_eq!(pressed.trigger.as_deref(), Some("go"));
}

#[test]
fn the_layers_under_a_point_come_topmost_first() {
    use cuelight_core::{LayerPath, Root};
    let engine = engine();
    // The circle over the rect: both are under its middle, the circle
    // first. Only the rect beside it, and nothing over the bare canvas
    // (the blue box is there, but it is a layer too).
    let layer = |i: usize| LayerPath::new(Root::Show, vec![i]);
    assert_eq!(engine.layers_at([16.0, 16.0]), [layer(1), layer(0)]);
    assert_eq!(engine.layers_at([2.0, 2.0]), [layer(0)]);
    assert_eq!(engine.layers_at([50.0, 16.0]), [layer(2)]);
    assert!(engine.layers_at([36.0, 16.0]).is_empty(), "the gap");
}

#[test]
fn a_layer_clipped_away_is_not_under_the_point() {
    let show = r##"{ "name": "in", "size": [64, 32], "layers": [
        { "name": "window", "type": "group", "clip": { "rect": [0, 0, 16, 32] },
          "children": [
            { "name": "wide", "type": "shape", "shape": { "rect": [0, 0, 32, 32] },
              "fill": "#FFFFFF" } ] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    assert_eq!(engine.layers_at([8.0, 16.0]).len(), 1);
    assert!(
        engine.layers_at([24.0, 16.0]).is_empty(),
        "outside the window"
    );
}

#[test]
fn a_press_can_open_a_link_beside_or_instead_of_a_trigger() {
    use cuelight_core::{Event, Happened};
    let show = r##"{ "name": "links", "size": [64, 32], "layers": [
      { "name": "site", "type": "shape", "shape": { "rect": [0, 0, 32, 32] },
        "fill": "#FF0000", "press": { "open": "https://example.com/" } },
      { "name": "both", "type": "shape", "shape": { "rect": [32, 0, 32, 32] },
        "fill": "#00FF00", "press": { "trigger": "order", "open": "http://example.com/order" } } ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    engine.drain_trace();
    // Asking says what a press would do; doing it reports the address
    // as an event and traces it, and fires the trigger when there is one.
    let asked = engine.pressed([16.0, 16.0]).unwrap();
    assert_eq!(asked.trigger, None);
    assert_eq!(asked.open.as_deref(), Some("https://example.com/"));
    assert!(engine.drain_events().is_empty());
    let pressed = engine.press([16.0, 16.0]).unwrap();
    assert_eq!(pressed.open.as_deref(), Some("https://example.com/"));
    assert_eq!(
        engine.drain_events(),
        vec![Event::Open {
            url: "https://example.com/".into()
        }]
    );
    assert!(engine
        .drain_trace()
        .iter()
        .any(|t| matches!(&t.what, Happened::Opened { url } if url == "https://example.com/")));
    let pressed = engine.press([48.0, 16.0]).unwrap();
    assert_eq!(pressed.trigger.as_deref(), Some("order"));
    assert_eq!(pressed.open.as_deref(), Some("http://example.com/order"));
    // Only the web, and a press that does something.
    for bad in [
        r##""press": { "open": "file:///etc/passwd" }"##,
        r##""press": { "open": "javascript:alert(1)" }"##,
        r##""press": {}"##,
    ] {
        let show = show.replace(r##""press": { "open": "https://example.com/" }"##, bad);
        assert!(Engine::new().load_show(&show).is_err(), "{bad}");
    }
}

/// A show that follows the pointer, with a value that wanders until a
/// real pointer takes over.
const FOLLOWS: &str = r##"{ "name": "eyes", "size": [100, 50],
  "input": { "pointer": { "x": "px", "y": "py", "over": "over" } },
  "values": { "px": { "timelines": [ { "name": "wander", "autoplay": true,
    "keys": [ { "t": 0, "v": 10 }, { "t": 2, "v": 90 } ] } ] } },
  "layers": [
    { "name": "eye", "type": "shape", "shape": { "circle": [0, 0, 4] }, "fill": "#FFFFFF",
      "bindings": [ { "property": "x", "variable": "px" }, { "property": "y", "variable": "py" } ] }
  ] }"##;

#[test]
fn the_pointer_sets_the_variables_the_show_names() {
    let mut engine = Engine::new();
    engine.load_show(FOLLOWS).unwrap();
    assert!(
        engine.load_warnings().is_empty(),
        "{:?}",
        engine.load_warnings()
    );
    // Before any pointer, the show's own value moves.
    engine.advance_to(1.0);
    assert_eq!(engine.value("px"), Some(50.0.into()));
    engine.point(Some([30.0, 20.0]));
    assert_eq!(engine.value("px"), Some(30.0.into()));
    assert_eq!(engine.value("py"), Some(20.0.into()));
    assert_eq!(engine.value("over"), Some(true.into()));
    // A real pointer has taken over: the value's motion is gone.
    engine.advance_to(1.5);
    assert_eq!(engine.value("px"), Some(30.0.into()));
}

#[test]
fn a_pointer_beside_the_canvas_stops_at_its_edge() {
    let mut engine = Engine::new();
    engine.load_show(FOLLOWS).unwrap();
    engine.point(Some([-20.0, 70.0]));
    assert_eq!(engine.value("px"), Some(0.0.into()));
    assert_eq!(engine.value("py"), Some(50.0.into()));
    assert_eq!(engine.value("over"), Some(false.into()));
    engine.point(Some([140.0, 10.0]));
    assert_eq!(engine.value("px"), Some(100.0.into()));
    assert_eq!(engine.value("py"), Some(10.0.into()));
}

#[test]
fn a_pointer_that_leaves_stays_where_it_was() {
    let mut engine = Engine::new();
    engine.load_show(FOLLOWS).unwrap();
    engine.point(Some([30.0, 20.0]));
    engine.point(None);
    assert_eq!(engine.value("px"), Some(30.0.into()));
    assert_eq!(engine.value("py"), Some(20.0.into()));
    assert_eq!(engine.value("over"), Some(false.into()));
}

#[test]
fn a_show_that_names_no_pointer_is_left_alone() {
    let mut engine = engine();
    engine.point(Some([10.0, 10.0]));
    assert_eq!(engine.value("pointer_x"), None);
}

#[test]
fn a_pointer_that_has_not_moved_sets_nothing_again() {
    let mut engine = Engine::new();
    engine.load_show(FOLLOWS).unwrap();
    engine.point(Some([30.0, 20.0]));
    let _ = engine.drain_trace();
    engine.point(Some([30.0, 20.0]));
    assert!(engine.drain_trace().is_empty());
}

/// Two pressable tiles, one sliding right, under an overlay that is not
/// pressable.
const HOVER: &str = r##"{ "name": "hover", "size": [100, 50],
  "input": { "pointer": { "under": "hovered" } },
  "layers": [
    { "name": "tile_1", "type": "shape", "shape": { "rect": [0, 0, 20, 20] }, "fill": "#FF0000",
      "press": { "trigger": "one" } },
    { "name": "tile_2", "type": "shape", "shape": { "rect": [0, 30, 20, 20] }, "fill": "#00FF00",
      "press": { "trigger": "two" },
      "timelines": [ { "name": "slide", "autoplay": true,
        "tracks": [ { "property": "x", "keys": [ { "t": 0, "v": 0 }, { "t": 1, "v": 50 } ] } ] } ] },
    { "name": "vignette", "type": "shape", "shape": { "rect": [0, 0, 100, 50] }, "fill": "#00000080" },
    { "name": "glow", "type": "shape", "shape": { "rect": [0, 0, 20, 20] }, "fill": "#FFFFFF",
      "bindings": [ { "property": "opacity", "variable": "hovered", "map": { "tile_1": 1 },
                      "default": 0, "transition": { "duration": 0.2 } } ] }
  ] }"##;

#[test]
fn the_pointer_says_which_pressable_layer_is_under_it() {
    let mut engine = Engine::new();
    engine.load_show(HOVER).unwrap();
    assert!(
        engine.load_warnings().is_empty(),
        "{:?}",
        engine.load_warnings()
    );
    // The overlay drawn on top is see-through, as it is for a press.
    engine.point(Some([10.0, 10.0]));
    assert_eq!(engine.value("hovered"), Some("tile_1".into()));
    // The highlight bound to it fades in.
    let glow = |engine: &Engine| {
        let layers = engine.resolved_layers().unwrap();
        layers
            .iter()
            .find(|l| l.name == "glow")
            .map_or(0.0, |l| l.opacity)
    };
    engine.advance_to(0.2);
    assert!((glow(&engine) - 1.0).abs() < 1e-9, "{}", glow(&engine));
    engine.point(Some([50.0, 10.0]));
    assert_eq!(engine.value("hovered"), Some("".into()));
    // Gone, nothing is under it.
    engine.point(Some([10.0, 10.0]));
    engine.point(None);
    assert_eq!(engine.value("hovered"), Some("".into()));
}

#[test]
fn a_layer_that_moves_under_a_still_pointer_is_under_it() {
    let mut engine = Engine::new();
    engine.load_show(HOVER).unwrap();
    engine.point(Some([60.0, 40.0]));
    assert_eq!(engine.value("hovered"), Some("".into()));
    // Slid to x 45 to 65 by now: under the pointer that has not moved.
    engine.advance_to(0.9);
    engine.point(Some([60.0, 40.0]));
    assert_eq!(engine.value("hovered"), Some("tile_2".into()));
}

/// Layers placed every way a layer's box can come out.
const BOXES: &str = r##"{ "name": "b", "size": [200, 100], "layers": [
  { "name": "plain", "type": "shape", "shape": { "rect": [0, 0, 20, 10] }, "fill": "#FFFFFF", "x": 5, "y": 7 },
  { "name": "turned", "type": "shape", "shape": { "rect": [0, 0, 20, 10] }, "fill": "#FFFFFF",
    "x": 50, "y": 50, "rotation": 90, "press": { "trigger": "turned" } },
  { "name": "pair", "type": "group", "children": [
    { "name": "a", "type": "shape", "shape": { "rect": [0, 0, 10, 10] }, "fill": "#FFFFFF", "x": 100 },
    { "name": "b", "type": "shape", "shape": { "rect": [0, 0, 10, 10] }, "fill": "#FFFFFF", "x": 130, "y": 20 } ] },
  { "name": "mixed", "type": "group", "children": [
    { "name": "a", "type": "shape", "shape": { "rect": [0, 0, 10, 10] }, "fill": "#FFFFFF", "x": 150 },
    { "name": "b", "type": "shape", "shape": { "rect": [0, 0, 20, 10] }, "fill": "#FFFFFF",
      "x": 170, "y": 40, "rotation": 45 } ] },
  { "name": "cut", "type": "group", "clip": { "rect": [0, 0, 15, 100] }, "children": [
    { "name": "wide", "type": "shape", "shape": { "rect": [0, 80, 40, 10] }, "fill": "#FFFFFF" } ] },
  { "name": "hidden", "type": "shape", "shape": { "rect": [0, 0, 5, 5] }, "fill": "#FFFFFF", "visible": false }
] }"##;

fn bounds(indices: &[usize]) -> Option<cuelight::LayerBounds> {
    let mut engine = Engine::new();
    engine.load_show(BOXES).unwrap();
    engine.bounds(&cuelight_core::LayerPath::new(
        cuelight_core::Root::Show,
        indices.to_vec(),
    ))
}

#[test]
fn a_layer_placed_plainly_has_its_box_on_the_canvas() {
    let plain = bounds(&[0]).unwrap();
    assert_eq!(plain.rect, [5.0, 7.0, 20.0, 10.0]);
    assert_eq!(plain.transform, cuelight::Transform::IDENTITY);
}

#[test]
fn a_turned_layer_has_its_own_box_and_what_turns_it() {
    let turned = bounds(&[1]).unwrap();
    assert_eq!(turned.rect, [0.0, 0.0, 20.0, 10.0]);
    // Its box's far corner, turned a quarter about its place.
    let [x, y] = turned.transform.apply([20.0, 10.0]);
    assert!(
        (x - 40.0).abs() < 1e-9 && (y - 70.0).abs() < 1e-9,
        "{x} {y}"
    );
    // What the box says is what a press there hits.
    let mut engine = Engine::new();
    engine.load_show(BOXES).unwrap();
    let pressed = |at| engine.pressed(at).and_then(|p| p.trigger);
    assert_eq!(pressed([45.0, 60.0]).as_deref(), Some("turned"));
    assert_eq!(pressed([55.0, 60.0]), None, "outside the turned box");
}

#[test]
fn a_group_has_the_box_round_its_children() {
    assert_eq!(bounds(&[2]).unwrap().rect, [100.0, 0.0, 40.0, 30.0]);
    assert_eq!(
        bounds(&[2, 1]).unwrap().rect,
        [130.0, 20.0, 10.0, 10.0],
        "one child"
    );
    // A child placed its own way: the box round both, on the canvas.
    let mixed = bounds(&[3]).unwrap();
    assert_eq!(mixed.transform, cuelight::Transform::IDENTITY);
    let [x, y, w, h] = mixed.rect;
    let half = 10.0 / 2f64.sqrt();
    assert_eq!([x, y], [150.0, 0.0]);
    assert!((w - (20.0 + 2.0 * half)).abs() < 1e-9 && (h - (40.0 + 3.0 * half)).abs() < 1e-9);
}

#[test]
fn a_clip_cuts_the_box_and_a_hidden_layer_has_none() {
    assert_eq!(bounds(&[4]).unwrap().rect, [0.0, 80.0, 15.0, 10.0]);
    assert_eq!(bounds(&[5]), None);
}
