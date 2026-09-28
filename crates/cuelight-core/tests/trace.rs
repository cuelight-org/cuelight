//! The trace: what happened inside the show and why, each at its own
//! instant; and `explain`, the precedence answered per property.

use cuelight_core::{
    Cause, Engine, Firing, Happened, Influence, LayerPath, Property, Root, TimelineOwner, Value,
    Which,
};

/// A chain of two timelines linked by `on_end`, and a third that the
/// same trigger starts.
const CHAIN: &str = r##"{ "name": "t", "size": [8, 8], "layers": [
    { "name": "box", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
      "x": 3,
      "bindings": [{ "property": "x", "variable": "nudge" }],
      "timelines": [
        { "name": "a", "trigger": "go", "on_end": "d1",
          "tracks": [{ "property": "x", "keys": [{ "t": 0, "v": 0 }, { "t": 0.7, "v": 10 }] }] },
        { "name": "b", "trigger": "d1", "hold": true,
          "tracks": [{ "property": "x", "keys": [{ "t": 0, "v": 10 }, { "t": 0.7, "v": 20 }] }] },
        { "name": "also", "trigger": "go",
          "tracks": [{ "property": "y", "keys": [{ "t": 0, "v": 0 }, { "t": 5, "v": 1 }] }] }
      ] }
  ] }"##;

fn box_layer() -> LayerPath {
    LayerPath::new(Root::Show, vec![0])
}

#[test]
fn a_chain_is_traced_with_its_causes_and_instants() {
    let mut engine = Engine::new();
    engine.load_show(CHAIN).unwrap();
    engine.drain_trace();
    engine.trigger("go");
    // One long frame: everything inside it is still at its own instant.
    engine.advance_to(2.0);
    let trace = engine.drain_trace();
    let what: Vec<String> = trace
        .iter()
        .map(|t| format!("{:.3} {}", t.at, t.what))
        .collect();
    assert_eq!(
        what,
        [
            "0.000 fired \"go\" by the host",
            "0.000 started timeline \"a\" of layer show/0 on \"go\", fired by the host",
            "0.000 started timeline \"also\" of layer show/0 on \"go\", fired by the host",
            "0.700 ended timeline \"a\" of layer show/0",
            "0.700 fired \"d1\" at the end of timeline \"a\" of layer show/0",
            "0.700 started timeline \"b\" of layer show/0 on \"d1\", fired at the end of timeline \"a\" of layer show/0",
            "1.400 ended timeline \"b\" of layer show/0, holding",
        ]
    );
    // And as data, not only as words.
    let Happened::Started { timeline, by } = &trace[5].what else {
        panic!("{:?}", trace[5]);
    };
    assert_eq!(timeline.name, "b");
    assert_eq!(timeline.owner, TimelineOwner::Layer(box_layer()));
    let Cause::Trigger { name, by } = by else {
        panic!("{by:?}");
    };
    assert_eq!(name, "d1");
    assert!(matches!(by, Firing::TimelineEnd(t) if t.name == "a"));
}

#[test]
fn a_timeline_started_on_its_own_leaves_the_others_alone() {
    let mut engine = Engine::new();
    engine.load_show(CHAIN).unwrap();
    engine.drain_trace();
    assert!(engine.start_timeline(&box_layer(), 0), "a exists");
    assert!(!engine.start_timeline(&box_layer(), 7), "no such timeline");
    engine.advance_to(0.35);
    let trace = engine.drain_trace();
    assert!(
        matches!(&trace[0].what, Happened::Started { timeline, by: Cause::Host } if timeline.name == "a"),
        "{:?}",
        trace[0]
    );
    // `also` listens to the same trigger and did not start: nothing was
    // fired.
    assert!(!trace
        .iter()
        .any(|t| matches!(&t.what, Happened::Started { timeline, .. } if timeline.name == "also")));
    assert!(!trace
        .iter()
        .any(|t| matches!(t.what, Happened::Fired { .. })));
}

#[test]
fn conditions_are_traced_as_they_turn() {
    let show = r##"{ "name": "t", "size": [8, 8], "variables": { "lamp": 0 }, "layers": [
        { "name": "l", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "timelines": [
            { "name": "flash", "when": { "variable": "lamp" },
              "tracks": [{ "property": "x", "keys": [{ "t": 0, "v": 0 }, { "t": 1, "v": 1 }] }] },
            { "name": "blink", "while": { "variable": "lamp" }, "loop": true,
              "tracks": [{ "property": "opacity", "keys": [{ "t": 0, "v": 0 }, { "t": 1, "v": 1 }] }] }
          ] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    engine.advance_to(0.1);
    engine.drain_trace();
    engine.set_variable("lamp", 1.0);
    engine.advance_to(0.2);
    engine.set_variable("lamp", 0.0);
    engine.advance_to(0.3);
    let what: Vec<String> = engine
        .drain_trace()
        .iter()
        .map(|t| format!("{:.1} {}", t.at, t.what))
        .collect();
    assert_eq!(
        what,
        [
            "0.1 set \"lamp\" to 1",
            "0.1 the when of timeline \"flash\" of layer show/0 turned true",
            "0.1 the while of timeline \"blink\" of layer show/0 turned true",
            "0.1 started timeline \"flash\" of layer show/0 as its when turned true",
            "0.1 started timeline \"blink\" of layer show/0 as its while turned true",
            "0.2 set \"lamp\" to 0",
            "0.2 the when of timeline \"flash\" of layer show/0 turned false",
            "0.2 the while of timeline \"blink\" of layer show/0 turned false",
            "0.2 stopped timeline \"blink\" of layer show/0 as its while turned false",
        ]
    );
}

#[test]
fn a_scene_entered_is_traced_with_what_it_starts() {
    let show = r##"{ "name": "t", "size": [8, 8], "layers": [],
      "scenes": [
        { "name": "one", "trigger": "one", "layers": [] },
        { "name": "two", "trigger": "two", "layers": [
          { "name": "l", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
            "timelines": [{ "name": "in", "autoplay": true,
              "tracks": [{ "property": "x", "keys": [{ "t": 0, "v": 0 }, { "t": 1, "v": 1 }] }] }] } ] }
      ] }"##;
    let mut engine = Engine::new();
    engine.load_show(show).unwrap();
    let at_load: Vec<String> = engine
        .drain_trace()
        .iter()
        .map(|t| t.what.to_string())
        .collect();
    assert_eq!(at_load, ["entered scene \"one\" at load"]);
    engine.trigger("two");
    let what: Vec<String> = engine
        .drain_trace()
        .iter()
        .map(|t| t.what.to_string())
        .collect();
    assert_eq!(
        what,
        [
            "fired \"two\" by the host",
            "entered scene \"two\" on \"two\", fired by the host",
            "started timeline \"in\" of layer scene 1/0 on entering scene \"two\"",
        ]
    );
}

#[test]
fn explain_ranks_every_source_of_a_property() {
    let mut engine = Engine::new();
    engine.load_show(CHAIN).unwrap();
    // Nothing runs and the binding has no variable: the base value.
    let sources = engine.explain(&box_layer(), Property::X);
    assert_eq!(
        sources,
        [
            Influence::Binding {
                index: 0,
                variable: "nudge".into(),
                value: None
            },
            Influence::Base {
                value: Value::Number(3.0)
            },
        ]
    );
    // A running timeline over a binding that applies over the base.
    engine.set_variable("nudge", 5.0);
    engine.trigger("go");
    engine.advance_to(0.35);
    let sources = engine.explain(&box_layer(), Property::X);
    assert_eq!(sources.len(), 3);
    let Influence::Timeline {
        timeline,
        local,
        held,
        value,
    } = &sources[0]
    else {
        panic!("{:?}", sources[0]);
    };
    assert_eq!(timeline.name, "a");
    assert_eq!(*local, Some(0.35));
    assert!(!held);
    assert_eq!(*value, Some(5.0));
    assert_eq!(
        sources[1],
        Influence::Binding {
            index: 0,
            variable: "nudge".into(),
            value: Some(Value::Number(5.0))
        }
    );
    // The winner is what the layer resolves to.
    let x = engine
        .values()
        .unwrap()
        .into_iter()
        .find(|row| row.property == Property::X)
        .unwrap()
        .value;
    assert_eq!(x, Value::Number(5.0));
    // A property the layer does not have: nothing to say.
    assert!(engine.explain(&box_layer(), Property::Text).is_empty());
    assert_eq!(
        engine.explain(&box_layer(), Property::Y).len(),
        2,
        "the y timeline and the base"
    );
    let _ = Which::When;
}

/// A sound with a trigger, a stop and an `on_end`, and a looping bed
/// under a condition.
const SOUNDS: &str = r##"{ "name": "s", "size": [8, 8], "layers": [
    { "name": "thunder", "type": "audio", "sound": "thunder", "trigger": "strike",
      "stop": "hush", "on_end": "rumbled" },
    { "name": "bed", "type": "audio", "sound": "loop", "loop": true,
      "while": { "variable": "storm" } } ] }"##;

fn played(trace: &[cuelight_core::Traced]) -> Vec<String> {
    trace
        .iter()
        .filter(|t| matches!(t.what, Happened::Played { .. } | Happened::Over { .. }))
        .map(|t| format!("{:.2} {}", t.at, t.what))
        .collect()
}

#[test]
fn a_play_is_traced_from_its_start_to_its_end_with_why() {
    let mut engine = Engine::new();
    engine.set_sound("thunder", 2.0).unwrap();
    engine.set_sound("loop", 0.5).unwrap();
    engine.load_show(SOUNDS).unwrap();
    engine.drain_trace();
    engine.trigger("strike");
    engine.advance_to(3.0);
    let trace = engine.drain_trace();
    assert_eq!(
        played(&trace),
        [
            "0.00 played \"thunder\" on layer \"thunder\" (show/0) on \"strike\", fired by the host",
            "2.00 \"thunder\" on layer \"thunder\" (show/0) finished, firing \"rumbled\"",
        ]
    );
    // The start and the end carry the same id, the one the host hears
    // the play under.
    let ids: Vec<u64> = trace
        .iter()
        .filter_map(|t| match &t.what {
            Happened::Played { id, by, .. } => {
                assert!(matches!(by, Cause::Trigger { .. }));
                Some(*id)
            }
            Happened::Over { id, by, .. } => {
                assert_eq!(
                    *by,
                    cuelight_core::Ending::Finished {
                        on_end: Some("rumbled".into())
                    }
                );
                Some(*id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    // The end is traced before the trigger it fires.
    let order: Vec<&str> = trace
        .iter()
        .filter_map(|t| match &t.what {
            Happened::Over { .. } => Some("over"),
            Happened::Fired { name, .. } if name == "rumbled" => Some("fired"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["over", "fired"]);
}

#[test]
fn a_play_stopped_short_says_what_stopped_it() {
    let mut engine = Engine::new();
    engine.set_sound("thunder", 2.0).unwrap();
    engine.set_sound("loop", 0.5).unwrap();
    engine.load_show(SOUNDS).unwrap();
    engine.drain_trace();
    engine.trigger("strike");
    engine.advance_to(0.5);
    engine.trigger("hush");
    engine.set_variable("storm", 1.0);
    engine.advance_to(1.0);
    engine.set_variable("storm", 0.0);
    engine.advance_to(1.5);
    engine.trigger("strike");
    engine.advance_to(1.6);
    engine.trigger("strike");
    engine.advance_to(1.7);
    assert_eq!(
        played(&engine.drain_trace()),
        [
            "0.00 played \"thunder\" on layer \"thunder\" (show/0) on \"strike\", fired by the host",
            "0.50 \"thunder\" on layer \"thunder\" (show/0) stopped on \"hush\"",
            "0.50 played \"loop\" on layer \"bed\" (show/1) as its while turned true",
            "1.00 \"loop\" on layer \"bed\" (show/1) stopped as its while turned false",
            "1.50 played \"thunder\" on layer \"thunder\" (show/0) on \"strike\", fired by the host",
            "1.60 \"thunder\" on layer \"thunder\" (show/0) started over",
            "1.60 played \"thunder\" on layer \"thunder\" (show/0) on \"strike\", fired by the host",
        ]
    );
}

#[test]
fn a_queued_play_is_traced_after_the_end_it_waited_for_and_by_what_queued_it() {
    let show = r##"{ "name": "q", "size": [8, 8], "layers": [
        { "name": "tune", "type": "audio", "sound": "loop", "retrigger": "queue",
          "trigger": "play", "on_end": "done" } ] }"##;
    let mut engine = Engine::new();
    engine.set_sound("loop", 0.5).unwrap();
    engine.load_show(show).unwrap();
    engine.drain_trace();
    engine.trigger("play");
    engine.advance_to(0.1);
    engine.trigger("play");
    engine.advance_to(0.6);
    assert_eq!(
        played(&engine.drain_trace()),
        [
            "0.00 played \"loop\" on layer \"tune\" (show/0) on \"play\", fired by the host",
            "0.50 \"loop\" on layer \"tune\" (show/0) finished, firing \"done\"",
            "0.50 played \"loop\" on layer \"tune\" (show/0) on \"play\", fired by the host",
        ]
    );
}

#[test]
fn plays_that_end_together_are_traced_in_the_order_they_started() {
    let show = r##"{ "name": "v", "size": [8, 8], "layers": [
        { "name": "a", "type": "audio", "sound": "loop", "trigger": "go" },
        { "name": "b", "type": "audio", "sound": "loop", "trigger": "go" },
        { "name": "c", "type": "audio", "sound": "loop", "trigger": "go" } ] }"##;
    let mut engine = Engine::new();
    engine.set_sound("loop", 0.5).unwrap();
    engine.load_show(show).unwrap();
    engine.drain_trace();
    engine.trigger("go");
    engine.advance_to(1.0);
    let lines = played(&engine.drain_trace());
    let layers: Vec<&str> = lines
        .iter()
        .map(|l| {
            l.split("on layer ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(
        layers,
        ["\"a\"", "\"b\"", "\"c\"", "\"a\"", "\"b\"", "\"c\""]
    );
}

#[test]
fn a_play_past_the_voices_gives_way_and_says_so() {
    let show = r##"{ "name": "o", "size": [8, 8], "layers": [
        { "name": "coin", "type": "audio", "sound": "thunder", "trigger": "drop",
          "retrigger": "overlap", "voices": 1 } ] }"##;
    let mut engine = Engine::new();
    engine.set_sound("thunder", 2.0).unwrap();
    engine.load_show(show).unwrap();
    engine.drain_trace();
    engine.trigger("drop");
    engine.advance_to(0.3);
    engine.trigger("drop");
    engine.advance_to(0.4);
    assert_eq!(
        played(&engine.drain_trace()),
        [
            "0.00 played \"thunder\" on layer \"coin\" (show/0) on \"drop\", fired by the host",
            "0.30 \"thunder\" on layer \"coin\" (show/0) gave way to a newer play",
            "0.30 played \"thunder\" on layer \"coin\" (show/0) on \"drop\", fired by the host",
        ]
    );
}
