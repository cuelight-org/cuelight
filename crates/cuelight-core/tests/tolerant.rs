//! A tolerant load keeps what it can and says what it dropped.

use cuelight_core::{Engine, Error, Finding};

fn finding(path: &str, message_has: &str, findings: &[Finding]) {
    let found = findings.iter().find(|f| f.path == path);
    let Some(found) = found else {
        panic!("no finding at {path:?} in {findings:#?}");
    };
    assert!(
        found.message.contains(message_has),
        "{path}: {:?} does not mention {message_has:?}",
        found.message
    );
}

#[test]
fn a_layer_that_does_not_parse_is_dropped_and_the_rest_loads() {
    let show = r##"{ "name": "t", "size": [8, 8], "layers": [
      { "name": "ok", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#FFFFFF" },
      { "name": "broken", "type": "shape", "shape": "not a shape" },
      { "name": "no kind" },
      { "name": "g", "type": "group", "children": [
        { "name": "bad child", "type": "image" },
        { "name": "good child", "type": "image", "image": "orb" } ] } ] }"##;
    let mut engine = Engine::new();
    assert!(matches!(engine.load_show(show), Err(Error::InvalidShow(_))));
    let findings = engine.load_show_tolerant(show).unwrap();
    let paths: Vec<&str> = findings.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["layers[1]", "layers[2]", "layers[3].children[0]"]);
    let loaded = engine.show().unwrap();
    let names: Vec<&str> = loaded.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["ok", "g"]);
    assert_eq!(loaded.layers[1].children().len(), 1);
    assert_eq!(loaded.layers[1].children()[0].name, "good child");
    // Nothing about the dropped layers is reported as an ignored field.
    assert!(
        engine.load_warnings().is_empty(),
        "{:?}",
        engine.load_warnings()
    );
}

#[test]
fn what_strict_loading_refuses_is_dropped_with_a_finding_that_says_where() {
    let show = r##"{ "name": "t", "size": [8, 8], "background": "blue",
      "output": { "mode": "gray4", "tint": "orange",
                  "passes": [{ "dots": { "size": 2 } }, { "dots": { "size": 0.8 } }] },
      "fonts": { "good": { "file": "f" }, "bad": { "file": "f", "color": "red" } },
      "layers": [
        { "name": "stroke", "type": "shape", "shape": { "rect": [0, 0, 1, 1] },
          "fill": "#FFFFFF", "stroke": { "color": "#FFFFFF", "width": 0 } },
        { "name": "fine", "type": "text", "text": "hi", "font": "good" },
        { "name": "orphan", "type": "text", "text": "hi", "font": "bad" } ],
      "scenes": [{ "name": "s", "output": { "tint": "#GGGGGG" }, "layers": [
        { "name": "lost", "type": "text", "text": "x", "font": "nope" },
        { "name": "kept", "type": "shape", "shape": { "rect": [0, 0, 1, 1] }, "fill": "#000000" } ] }] }"##;
    let mut engine = Engine::new();
    assert!(engine.load_show(show).is_err());
    let findings = engine.load_show_tolerant(show).unwrap();
    finding("background", "blue", &findings);
    finding("output.tint", "orange", &findings);
    finding("output.passes[0]", "size", &findings);
    finding("scenes[0].output.tint", "#GGGGGG", &findings);
    finding("fonts.bad", "red", &findings);
    finding("layers[0]", "stroke width", &findings);
    // The style went, so the layer that used it goes in the next round.
    finding("layers[2]", "undeclared font style \"bad\"", &findings);
    finding(
        "scenes[0].layers[0]",
        "undeclared font style \"nope\"",
        &findings,
    );
    assert_eq!(findings.len(), 8, "{findings:#?}");
    // First round in document order, then what its drops left wanting.
    assert_eq!(findings[0].path, "background");
    assert_eq!(findings.last().unwrap().path, "layers[2]");

    let loaded = engine.show().unwrap();
    assert_eq!(loaded.background, "#000000");
    assert_eq!(loaded.output.tint, None);
    assert_eq!(loaded.output.passes.as_ref().map(Vec::len), Some(1));
    assert_eq!(loaded.fonts.keys().collect::<Vec<_>>(), ["good"]);
    let names: Vec<&str> = loaded.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["fine"]);
    let names: Vec<&str> = loaded.scenes[0]
        .layers
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(names, ["kept"]);
    // And what was kept is a show strict loading takes.
    let kept = serde_json::to_string(loaded).unwrap();
    Engine::new().load_show(&kept).unwrap();
}

#[test]
fn a_font_style_or_value_or_scene_that_does_not_parse_is_dropped() {
    let show = r##"{ "name": "t", "size": [8, 8],
      "output": { "mode": "sepia" },
      "fonts": { "f": { "file": "f" }, "no file": { "color": "#FFFFFF" } },
      "variables": { "score": 0, "odd": { "not": "a value" } },
      "values": { "spin": { "timelines": [] }, "odd": 3 },
      "scenes": [ { "layers": [] }, { "name": "real", "output": { "scaling": "blurry" } } ] }"##;
    let mut engine = Engine::new();
    let findings = engine.load_show_tolerant(show).unwrap();
    let paths: Vec<&str> = findings.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "output",
            "fonts.no file",
            "variables.odd",
            "values.odd",
            "scenes[0]",
            "scenes[1].output"
        ]
    );
    let loaded = engine.show().unwrap();
    assert_eq!(loaded.output.mode, None);
    assert_eq!(loaded.fonts.len(), 1);
    assert_eq!(loaded.variables.len(), 1);
    assert_eq!(loaded.values.len(), 1);
    assert_eq!(loaded.scenes.len(), 1);
    assert_eq!(loaded.scenes[0].name, "real");
    assert_eq!(loaded.scenes[0].output, None);
}

#[test]
fn without_a_document_there_is_nothing_to_keep() {
    let mut engine = Engine::new();
    assert!(matches!(
        engine.load_show_tolerant("{ not json"),
        Err(Error::InvalidShow(_))
    ));
    // No canvas: nothing can be drawn on nothing.
    assert!(matches!(
        engine.load_show_tolerant(r#"{ "name": "t", "size": "big" }"#),
        Err(Error::InvalidShow(_))
    ));
    assert!(matches!(
        engine.load_show_tolerant(r#"{ "name": "t", "size": [8, 8], "format": 99 }"#),
        Err(Error::UnsupportedFormat { .. })
    ));
    // A show that fails to load leaves the current one in place.
    engine
        .load_show(r#"{ "name": "kept", "size": [8, 8] }"#)
        .unwrap();
    assert!(engine.load_show_tolerant("[]").is_err());
    assert_eq!(engine.show().unwrap().name, "kept");
}

#[test]
fn a_clean_show_has_no_findings_and_a_finding_reads_as_one_line() {
    let mut engine = Engine::new();
    let findings = engine
        .load_show_tolerant(r#"{ "name": "t", "size": [8, 8] }"#)
        .unwrap();
    assert!(findings.is_empty());
    let finding = Finding {
        path: "layers[2]".into(),
        message: "invalid color literal \"blue\"".into(),
    };
    assert_eq!(
        finding.to_string(),
        "layers[2]: invalid color literal \"blue\""
    );
}
