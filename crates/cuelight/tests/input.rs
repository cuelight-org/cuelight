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
