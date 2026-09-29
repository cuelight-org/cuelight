//! What a show document could do without, and what in it will surprise
//! someone: advisory findings, never a reason not to load.
//!
//! Loading answers whether a show can run. Whether it is well made is a
//! different question, asked here over the document alone: what nothing
//! uses, what names nothing, what works and will bite later. A host with
//! the show's files beside it (the loader) asks the rest.

use crate::engine::Error;
use crate::engine::{ignored_fields, quiet_bindings, salvaged};
use crate::engine::{Finding, FindingKind};
use crate::model::{DigitDisplay, Layer, LayerKind, Property, Reading, Show, MAIN_BUS};
use crate::value::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Every advisory finding the document alone gives: layers nothing can
/// see, timelines nothing starts, scenes nothing enters, variables,
/// values and font styles nothing uses ([`FindingKind::Unused`]); readings of
/// names nothing declares ([`FindingKind::Missing`]); and what works today and
/// will surprise someone: two layers of one name side by side, a duck
/// under the main bus ([`FindingKind::Unwise`]). Each names its place in the
/// document, in document order.
///
/// Nothing here needs the show's files or a running engine; the loader's
/// audit adds what those tell.
pub fn audit(show: &Show) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut read: BTreeSet<String> = BTreeSet::new();
    let mut styled: BTreeSet<String> = BTreeSet::new();
    let mut seen_names: BTreeMap<(String, String), String> = BTreeMap::new();
    let trees = std::iter::once(("layers".to_owned(), show.layers.as_slice())).chain(
        show.scenes
            .iter()
            .enumerate()
            .map(|(i, s)| (format!("scenes[{i}].layers"), s.layers.as_slice())),
    );
    for (prefix, layers) in trees {
        walk(
            show,
            layers,
            &prefix,
            &mut read,
            &mut styled,
            &mut seen_names,
            &mut out,
        );
    }
    for (i, scene) in show.scenes.iter().enumerate() {
        // The first scene is entered at load; the rest want a trigger.
        if i > 0 && scene.trigger.iter().next().is_none() {
            out.push(finding(
                format!("scenes[{i}]"),
                FindingKind::Unused,
                format!(
                    "scene {:?} has no trigger, so nothing enters it",
                    scene.name
                ),
            ));
        }
    }
    for name in show.variables.keys() {
        if !read.contains(name) {
            out.push(finding(
                format!("variables.{name}"),
                FindingKind::Unused,
                format!("variable {name:?} is declared and nothing reads it"),
            ));
        }
    }
    for (name, value) in &show.values {
        if !read.contains(name) {
            out.push(finding(
                format!("values.{name}"),
                FindingKind::Unused,
                format!("value {name:?} is animated and nothing reads it"),
            ));
        }
        for (t, timeline) in value.timelines.iter().enumerate() {
            if !timeline.autoplay && timeline.trigger.iter().next().is_none() {
                out.push(finding(
                    format!("values.{name}.timelines[{t}]"),
                    FindingKind::Unused,
                    format!(
                        "timeline {:?} of value {name:?} has no trigger or autoplay, so nothing \
                         starts it",
                        timeline.name
                    ),
                ));
            }
        }
    }
    for name in show.fonts.keys() {
        if !styled.contains(name) {
            out.push(finding(
                format!("fonts.{name}"),
                FindingKind::Unused,
                format!("font style {name:?} is declared and no layer uses it"),
            ));
        }
    }
    out
}

/// The field the layers nested in `layer` are written in: a group's
/// `children`, an artwork layer's `parts`.
fn nested_in(layer: &Layer) -> &'static str {
    match layer.kind {
        LayerKind::Image { .. } => "parts",
        _ => "children",
    }
}

/// A document audited as it was written: what a strict load would
/// refuse, the fields the engine does not know, and everything
/// [`audit`] finds, each at the path the document gives it.
///
/// The document is salvaged first, the way a tolerant load does it, with
/// what was dropped blanked in place rather than taken out, so a finding
/// after a dropped layer still names the layer the author sees.
/// [`Audited::show`] is that salvaged document, blanks and all: what the
/// audit looked at, and what a host with the show's files beside it can
/// look at further, with the same paths.
///
/// Fails only when there is no document at all, as a tolerant load does.
pub fn audit_document(json: &str) -> Result<Audited, Error> {
    let salvaged = salvaged(json)?;
    let blanks = salvaged.blank_paths();
    let mut findings = salvaged.findings;
    let show = salvaged.show;
    if let Ok(understood) = serde_json::to_value(&show) {
        let mut ignored = Vec::new();
        ignored_fields(&salvaged.raw, &understood, "", &mut ignored);
        findings.extend(ignored.into_iter().map(|path| Finding {
            path,
            message: "is not a field the engine knows, and was ignored".to_owned(),
            kind: FindingKind::Unwise,
        }));
    }
    let mut quiet = Vec::new();
    // Not the undeclared variables: the audit's own check below says
    // that with the binding's place.
    quiet_bindings(&show, false, &mut quiet);
    findings.extend(quiet.into_iter().map(|message| Finding {
        path: "show.json".to_owned(),
        message,
        kind: FindingKind::Unwise,
    }));
    // A blank is an empty group, a part of nothing or an empty scene:
    // whatever the audit says of one belongs to what was dropped, and
    // that has been said.
    let at_blank = |path: &str| {
        blanks
            .iter()
            .any(|blank| path == blank || path.starts_with(&format!("{blank}.")))
    };
    findings.extend(audit(&show).into_iter().filter(|f| !at_blank(&f.path)));
    Ok(Audited { findings, show })
}

/// What [`audit_document`] found, and the document it looked at.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Audited {
    /// Every finding, each at the path the document gives it.
    pub findings: Vec<Finding>,
    /// The document as the audit saw it: what a strict load takes, with
    /// what could not be taken blanked in place so its paths hold. Not
    /// a show to play; a show to look at further.
    pub show: Show,
}

fn finding(path: String, kind: FindingKind, message: String) -> Finding {
    Finding {
        path,
        message,
        kind,
    }
}

fn walk(
    show: &Show,
    layers: &[Layer],
    prefix: &str,
    read: &mut BTreeSet<String>,
    styled: &mut BTreeSet<String>,
    seen_names: &mut BTreeMap<(String, String), String>,
    out: &mut Vec<Finding>,
) {
    for (i, layer) in layers.iter().enumerate() {
        let here = format!("{prefix}[{i}]");
        // Two of one name side by side: legal, and the second is the one
        // nothing can tell from the first. The same name in another
        // group is how rows of a board are written, and is left alone.
        if !layer.name.is_empty() {
            let key = (prefix.to_owned(), layer.name.clone());
            match seen_names.get(&key) {
                Some(first) => out.push(finding(
                    here.clone(),
                    FindingKind::Unwise,
                    format!(
                        "layer {:?} shares its name with {first}; a name should tell them apart",
                        layer.name
                    ),
                )),
                None => {
                    seen_names.insert(key, here.clone());
                }
            }
        }
        // Never seen: shut, and nothing that opens it.
        let bound = |property: Property| layer.bindings.iter().any(|b| b.property == property);
        let tracked = |property: Property| {
            layer
                .timelines
                .iter()
                .any(|tl| tl.tracks.iter().any(|t| t.property == property))
        };
        // A part hidden for good is how a use of shared artwork leaves an
        // element out: the point, not dead weight.
        let part = matches!(layer.kind, LayerKind::Part { .. });
        let shut = if part {
            None
        } else if !layer.visible && !bound(Property::Visible) {
            Some("visible is false")
        } else if layer.opacity <= 0.0 && !bound(Property::Opacity) && !tracked(Property::Opacity) {
            Some("opacity is 0")
        } else {
            None
        };
        if let Some(why) = shut {
            out.push(finding(
                here.clone(),
                FindingKind::Unused,
                format!(
                    "layer {:?} is never seen: its {why}, and nothing changes that",
                    layer.name
                ),
            ));
        }
        for (b, binding) in layer.bindings.iter().enumerate() {
            let at = format!("{here}.bindings[{b}]");
            reading(show, &binding.reading, &at, read, out);
            if binding.property == Property::Font {
                for value in binding.reading.map.iter().flat_map(|m| m.values()) {
                    if let Value::Text(style) = value {
                        styled.insert(style.clone());
                    }
                }
                if let Some(Value::Text(style)) = &binding.reading.default {
                    styled.insert(style.clone());
                }
            }
        }
        for (t, timeline) in layer.timelines.iter().enumerate() {
            let at = format!("{here}.timelines[{t}]");
            let started = timeline.autoplay
                || timeline.trigger.iter().next().is_some()
                || timeline.when.is_some()
                || timeline.whilst.is_some();
            if !started {
                out.push(finding(
                    at.clone(),
                    FindingKind::Unused,
                    format!(
                        "timeline {:?} of layer {:?} has no trigger, when, while or autoplay, \
                         so nothing starts it",
                        timeline.name, layer.name
                    ),
                ));
            }
            for (which, condition) in [("when", &timeline.when), ("while", &timeline.whilst)] {
                if let Some(condition) = condition {
                    reading(show, condition, &format!("{at}.{which}"), read, out);
                }
            }
        }
        match &layer.kind {
            LayerKind::Text { font, .. } => {
                styled.insert(font.clone());
            }
            LayerKind::Digits {
                display: DigitDisplay::Reel(reel),
                ..
            } => {
                if let Some(font) = &reel.font {
                    styled.insert(font.clone());
                }
            }
            _ => {}
        }
        if let Some(media) = layer.kind.media() {
            for (which, condition) in [("when", media.when), ("while", media.whilst)] {
                if let Some(condition) = condition {
                    reading(show, condition, &format!("{here}.{which}"), read, out);
                }
            }
        }
        if let LayerKind::Audio {
            duck: Some(duck), ..
        }
        | LayerKind::Video {
            duck: Some(duck), ..
        } = &layer.kind
        {
            if duck.under == MAIN_BUS {
                out.push(finding(
                    format!("{here}.duck"),
                    FindingKind::Unwise,
                    format!(
                        "layer {:?} ducks under {MAIN_BUS:?}, which every sound naming no bus is \
                         on, including ones added later; a narrower bus named for what should \
                         duck it keeps the coupling visible from both ends",
                        layer.name
                    ),
                ));
            }
        }
        walk(
            show,
            layer.children(),
            &format!("{here}.{}", nested_in(layer)),
            read,
            styled,
            seen_names,
            out,
        );
    }
}

/// Note what a reading reads, and say so when nothing declares it.
fn reading(
    show: &Show,
    reading: &Reading,
    at: &str,
    read: &mut BTreeSet<String>,
    out: &mut Vec<Finding>,
) {
    let name = &reading.variable;
    read.insert(name.clone());
    if !show.variables.contains_key(name) && !show.values.contains_key(name) {
        out.push(finding(
            at.to_owned(),
            FindingKind::Missing,
            format!(
                "reads {name:?}, which the show declares as neither a variable nor a value; it \
                 does nothing until a host sets it"
            ),
        ));
    }
}
