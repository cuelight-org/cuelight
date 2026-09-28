//! Where each trigger of a show is listened to.

use cuelight_core::{Listened, Show};

#[test]
fn a_show_says_where_each_trigger_is_heard() {
    let show = r##"{ "name": "deck", "size": [8, 8],
      "input": { "keys": { "ArrowRight": "next", "c": "coin" }, "press": "tap" },
      "layers": [
        { "name": "frame", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "press": { "trigger": "poke" },
          "timelines": [{ "name": "pulse", "trigger": "next", "tracks": [] }] } ],
      "scenes": [
        { "name": "one", "trigger": "slide_1", "layers": [
          { "name": "coin", "type": "audio", "sound": "coin", "trigger": "coin", "stop": "quiet" },
          { "name": "reel", "type": "digits", "digits": 3, "size": [3, 1], "text": "777",
            "display": { "reel": { "charset": "0123456789", "font": "f", "duration": 0.5, "spin": "roll" } } } ] },
        { "name": "two", "trigger": ["slide_2", "jackpot"], "layers": [
          { "name": "coin again", "type": "audio", "sound": "coin", "trigger": "coin" },
          { "name": "g", "type": "group", "children": [
            { "name": "boom", "type": "audio", "sound": "boom", "trigger": "jackpot" } ] } ] } ],
      "fonts": { "f": { "file": "f" } } }"##;
    let show: Show = serde_json::from_str(show).unwrap();
    let listeners = show.listeners();
    let at = |name: &str| {
        listeners
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing"))
    };
    // Entering a scene, whoever else hears it.
    assert_eq!(*at("slide_1"), Listened::Opens("one".into()));
    assert_eq!(*at("slide_2"), Listened::Opens("two".into()));
    assert_eq!(*at("jackpot"), Listened::Opens("two".into()));
    // The show's own layers hear it.
    assert_eq!(*at("next"), Listened::Anywhere);
    // Two scenes hear it; one scene hears these.
    assert_eq!(*at("coin"), Listened::Anywhere);
    assert_eq!(*at("quiet"), Listened::Scene("one".into()));
    assert_eq!(*at("roll"), Listened::Scene("one".into()));
    // Fired by a press or a key, heard by nobody: an action all the same.
    assert_eq!(*at("tap"), Listened::Anywhere);
    assert_eq!(*at("poke"), Listened::Anywhere);
    // Everything triggers() lists is placed, and nothing else but those.
    for name in show.triggers() {
        assert!(listeners.contains_key(&name), "{name}");
    }
    let mut names: Vec<&str> = listeners.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["coin", "jackpot", "next", "poke", "quiet", "roll", "slide_1", "slide_2", "tap"]
    );
}
