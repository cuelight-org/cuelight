//! The audit over a show folder: files nothing names, names with no file,
//! a driver firing into nothing, and the document's own findings.

use cuelight_core::FindingKind;
use cuelight_loader::{audit, Driver, Manifest};
use std::path::PathBuf;

fn shows() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cuelight/examples/shows")
}

/// A show folder's document, files and driver, audited as the check
/// tool does it.
fn audit_folder(dir: &std::path::Path) -> Vec<(String, FindingKind, String)> {
    let manifest = Manifest::for_dir(dir).unwrap();
    let json = std::fs::read_to_string(dir.join("show.json")).unwrap();
    let driver = dir
        .join("test-driver.json")
        .is_file()
        .then(|| Driver::from_file(dir.join("test-driver.json")).unwrap());
    audit(&json, Some(&manifest.files), driver.as_ref())
        .into_iter()
        .map(|f| (f.path, f.kind, f.message))
        .collect()
}

#[test]
fn the_bundled_shows_have_nothing_wrong_or_missing() {
    for entry in std::fs::read_dir(shows()).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        let found = audit_folder(&path);
        let bad: Vec<_> = found
            .iter()
            .filter(|(_, kind, _)| matches!(kind, FindingKind::Error | FindingKind::Missing))
            .collect();
        assert!(bad.is_empty(), "{}: {bad:#?}", path.display());
    }
}

#[test]
fn files_nothing_names_and_names_with_no_file_are_found() {
    let dir = std::env::temp_dir().join(format!("cuelight-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets/fonts")).unwrap();
    std::fs::create_dir_all(dir.join("assets/sounds")).unwrap();
    std::fs::copy(
        shows().join("beacon/assets/orb.png"),
        dir.join("assets/orb.png"),
    )
    .unwrap();
    std::fs::copy(
        shows().join("beacon/assets/orb.png"),
        dir.join("assets/spare.png"),
    )
    .unwrap();
    std::fs::write(dir.join("assets/fonts/score.fnt"), b"info face=\"s\"\n").unwrap();
    std::fs::write(dir.join("assets/fonts/score_0.png"), b"page").unwrap();
    std::fs::write(dir.join("assets/sounds/hum.wav"), b"not really").unwrap();
    std::fs::write(
        dir.join("show.json"),
        r##"{ "name": "folder", "size": [8, 8], "variables": { "score": 0 },
          "fonts": { "digits": { "file": "score" }, "title": { "file": "missing" } },
          "layers": [
            { "name": "orb", "type": "image", "image": "orb" },
            { "name": "ghost", "type": "image", "image": "art/ghost.png" },
            { "name": "score", "type": "text", "text": "0", "font": "digits",
              "bindings": [{ "property": "text", "variable": "score" }] },
            { "name": "big", "type": "text", "text": "!", "font": "title", "colour": "#FF0000" } ] }"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("test-driver.json"),
        r##"{ "steps": [ { "trigger": "nothing" }, { "set": { "score": 5, "lives": 3 } } ] }"##,
    )
    .unwrap();
    let found = audit_folder(&dir);
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
            "fonts.title",
            "layers[1]",
            "test-driver.json:steps[0]",
            "test-driver.json:steps[1]",
        ],
        "{found:#?}"
    );
    // The spare image and the sound nothing plays; not the font's page.
    assert_eq!(
        by_kind(FindingKind::Unused),
        ["assets/sounds/hum.wav", "assets/spare.png"],
        "{found:#?}"
    );
    // A field the engine does not know.
    assert_eq!(
        by_kind(FindingKind::Unwise),
        ["layers[3].colour"],
        "{found:#?}"
    );
    assert!(by_kind(FindingKind::Error).is_empty(), "{found:#?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_document_that_is_not_a_show_is_one_finding() {
    let found = audit("{ \"name\": \"x\" }", None, None);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, FindingKind::Error);
    assert_eq!(found[0].path, "show.json");
    // Nothing given beyond the document: only the document is judged.
    let found = audit(
        r#"{ "name": "x", "size": [8, 8], "layers": [{ "name": "a", "type": "image", "image": "art/none.png" }] }"#,
        None,
        None,
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn file_findings_keep_the_paths_of_the_document_after_a_dropped_layer() {
    let dir = std::env::temp_dir().join(format!("cuelight-audit-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("show.json"),
        r##"{ "name": "paths", "size": [8, 8], "layers": [
            { "name": "broken", "type": "shape", "shape": 7 },
            { "name": "ghost", "type": "image", "image": "art/ghost.png" } ] }"##,
    )
    .unwrap();
    let found = audit_folder(&dir);
    let paths: Vec<(&str, FindingKind)> = found.iter().map(|(p, k, _)| (p.as_str(), *k)).collect();
    assert_eq!(
        paths,
        [
            ("layers[0]", FindingKind::Error),
            ("layers[1]", FindingKind::Missing)
        ],
        "{found:#?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
