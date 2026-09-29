//! The document audit: what nothing uses, what names nothing, and what
//! will surprise someone.

// Test code throughout, so clippy lets it panic as tests do.
#![cfg(test)]

use cuelight_core::{audit, FindingKind, Show};

fn findings(show: &str) -> Vec<(String, FindingKind, String)> {
    let show: Show = serde_json::from_str(show).unwrap();
    audit(&show)
        .into_iter()
        .map(|f| (f.path, f.kind, f.message))
        .collect()
}

#[test]
fn a_clean_show_has_nothing_to_say() {
    let show = r##"{ "name": "clean", "size": [8, 8],
      "variables": { "score": 0 }, "fonts": { "f": { "file": "f" } },
      "layers": [
        { "name": "score", "type": "text", "text": "0", "font": "f",
          "bindings": [{ "property": "text", "variable": "score" }],
          "timelines": [{ "name": "in", "autoplay": true, "tracks": [] }] } ],
      "scenes": [ { "name": "first", "layers": [] }, { "name": "next", "trigger": "go", "layers": [] } ] }"##;
    assert_eq!(findings(show), []);
}

#[test]
fn what_nothing_uses_is_unused() {
    let show = r##"{ "name": "u", "size": [8, 8],
      "variables": { "used": 0, "idle": 0 }, "values": { "spin": { "timelines": [] } },
      "fonts": { "f": { "file": "f" }, "spare": { "file": "f" } },
      "layers": [
        { "name": "dark", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF", "opacity": 0 },
        { "name": "lit", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF", "opacity": 0,
          "bindings": [{ "property": "opacity", "variable": "used" }] },
        { "name": "gone", "type": "text", "text": "x", "font": "f", "visible": false,
          "timelines": [{ "name": "still", "tracks": [] }] } ],
      "scenes": [ { "name": "a", "layers": [] }, { "name": "b", "layers": [] } ] }"##;
    let found = findings(show);
    let unused: Vec<&str> = found
        .iter()
        .filter(|(_, kind, _)| *kind == FindingKind::Unused)
        .map(|(path, ..)| path.as_str())
        .collect();
    assert_eq!(
        unused,
        [
            "layers[0]",
            "layers[2]",
            "layers[2].timelines[0]",
            "scenes[1]",
            "variables.idle",
            "values.spin",
            "fonts.spare",
        ],
        "{found:#?}"
    );
    assert_eq!(found.len(), unused.len(), "{found:#?}");
}

#[test]
fn a_reading_of_nothing_is_missing_and_a_habit_that_bites_is_unwise() {
    let show = r##"{ "name": "m", "size": [8, 8],
      "fonts": { "big": { "file": "f", "size": 96 } },
      "layers": [
        { "name": "twin", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "bindings": [{ "property": "x", "variable": "ghost" }],
          "timelines": [{ "name": "flash", "when": { "variable": "phantom" }, "tracks": [] }] },
        { "name": "twin", "type": "text", "text": "0", "font": "big",
          "bindings": [{ "property": "text", "variable": "ghost" }] },
        { "name": "bed", "type": "audio", "sound": "bed", "autoplay": true, "loop": true,
          "duck": { "under": "main", "to": 0.2 } } ],
      "scenes": [ { "name": "other", "trigger": "go", "layers": [
        { "name": "twin", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF" } ] } ] }"##;
    let found = findings(show);
    let by_kind = |kind: FindingKind| -> Vec<&str> {
        found
            .iter()
            .filter(|(_, k, _)| *k == kind)
            .map(|(path, ..)| path.as_str())
            .collect()
    };
    assert_eq!(
        by_kind(FindingKind::Missing),
        [
            "layers[0].bindings[0]",
            "layers[0].timelines[0].when",
            "layers[1].bindings[0]"
        ],
        "{found:#?}"
    );
    // The second twin beside the first, not the one in the scene; the
    // duck under everything.
    assert_eq!(
        by_kind(FindingKind::Unwise),
        ["layers[1]", "layers[2].duck"],
        "{found:#?}"
    );
    assert!(by_kind(FindingKind::Unused).is_empty(), "{found:#?}");
    let twin = found.iter().find(|(p, ..)| p == "layers[1]").unwrap();
    assert!(twin.2.contains("layers[0]"), "{}", twin.2);
}

#[test]
fn a_value_timeline_is_a_listener_and_one_nothing_starts_is_unused() {
    let show = r##"{ "name": "v", "size": [8, 8],
      "values": {
        "spin": { "timelines": [ { "name": "go", "trigger": "hop", "keys": [] },
                                 { "name": "idle", "keys": [] } ] } },
      "layers": [
        { "name": "wheel", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
          "bindings": [{ "property": "rotation", "variable": "spin" }] } ] }"##;
    let parsed: Show = serde_json::from_str(show).unwrap();
    assert!(parsed.triggers().contains("hop"));
    assert_eq!(
        parsed.listeners().get("hop"),
        Some(&cuelight_core::Listened::Anywhere)
    );
    let found = findings(show);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].0, "values.spin.timelines[1]");
    assert_eq!(found[0].1, FindingKind::Unused);
}

#[test]
fn a_document_is_audited_at_the_paths_it_was_written_with() {
    // The second layer is dropped; the fourth is never seen, and is
    // still called the fourth.
    let show = r##"{ "name": "d", "size": [8, 8], "layers": [
      { "name": "ok", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF" },
      { "name": "broken", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF", "rotation": "lots" },
      { "name": "fine", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF",
        "bindings": [{ "property": "x", "variable": "ghost" }], "colour": "#FF0000" },
      { "name": "unseen", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF", "opacity": 0 } ],
      "scenes": [ { "name": "a", "layers": [] }, { "trigger": "x" }, { "name": "c", "layers": [] } ] }"##;
    let audited = cuelight_core::audit_document(show).unwrap();
    let paths: Vec<(&str, FindingKind)> = audited
        .findings
        .iter()
        .map(|f| (f.path.as_str(), f.kind))
        .collect();
    assert_eq!(
        paths,
        [
            ("layers[1]", FindingKind::Error),
            ("scenes[1]", FindingKind::Error),
            ("layers[2].colour", FindingKind::Unwise),
            ("layers[2].bindings[0]", FindingKind::Missing),
            ("layers[3]", FindingKind::Unused),
            ("scenes[2]", FindingKind::Unused),
        ],
        "{:#?}",
        audited.findings
    );
    // The dropped scene, a blank now, is not reported as a scene nothing
    // enters; the document handed back keeps its place.
    assert_eq!(audited.show.scenes.len(), 3);
    assert_eq!(audited.show.layers[1].name, "");
    assert!(cuelight_core::audit_document("[]").is_err());
}

#[test]
fn a_part_hidden_for_good_is_leaving_an_element_out_not_dead_weight() {
    let show = r##"{ "name": "p", "size": [8, 8], "layers": [
      { "name": "girl", "type": "image", "image": "girl", "parts": [
        { "id": "basket", "visible": false },
        { "id": "hood", "opacity": 0 } ] },
      { "name": "gone", "type": "image", "image": "girl", "visible": false } ] }"##;
    let found = findings(show);
    let paths: Vec<&str> = found.iter().map(|(p, ..)| p.as_str()).collect();
    // The layer still is; the parts are not.
    assert_eq!(paths, ["layers[1]"], "{found:#?}");
}
