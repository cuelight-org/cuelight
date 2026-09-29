//! Loading a show tolerantly: what cannot be used is blanked, and each
//! blank is a finding that says where it was.

use super::*;

/// A document with everything a tolerant load drops blanked in place,
/// so that what is left parses and every path still means what it does
/// in the document the author sees.
pub(crate) struct Salvaged {
    /// The document, the blanks still in.
    pub raw: serde_json::Value,
    /// What it parses to: a show `load_show` would take, blanks and all.
    pub show: Show,
    /// What was dropped, and why.
    pub findings: Vec<Finding>,
    /// The places blanked, to be taken out before the show is played.
    pub blanked: Vec<Site>,
}

impl Salvaged {
    /// The paths of the blanks, as the document names them.
    pub fn blank_paths(&self) -> Vec<String> {
        self.blanked.iter().map(Site::path).collect()
    }
}

/// Salvage `json`: drop whatever does not parse or a strict load would
/// refuse, each drop a finding, until what is left is a show
/// `load_show` takes. Layers and scenes are blanked rather than taken
/// out, so every finding names the place the author sees, not one
/// shifted by the drops before it; see [`remove_blanks`].
pub(crate) fn salvaged(json: &str) -> Result<Salvaged, Error> {
    let mut raw = parse_document(json)?;
    let mut findings = Vec::new();
    let mut blanked: Vec<Site> = Vec::new();
    salvage(&mut raw, &mut findings, &mut blanked);
    let mut show = parse_show(&raw)?;
    loop {
        let problems = problems(&show);
        if problems.is_empty() {
            break;
        }
        // Dropping one thing can leave another wanting it, so the
        // checks run again until nothing is left to drop. Later sites
        // first, so the index of an earlier one still holds.
        for problem in problems.iter().rev() {
            problem.site.drop_from(&mut raw);
        }
        for problem in problems {
            findings.push(problem.finding());
            if problem.site.is_blanked() {
                blanked.push(problem.site);
            }
        }
        show = parse_show(&raw)?;
    }
    Ok(Salvaged {
        raw,
        show,
        findings,
        blanked,
    })
}

/// Parse a show document as JSON, refusing a format this engine does
/// not read before interpreting anything else: its fields may mean
/// something this engine would get wrong.
pub(super) fn parse_document(json: &str) -> Result<serde_json::Value, Error> {
    let raw: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::InvalidShow(e.to_string()))?;
    if let Some(found) = raw.get("format").and_then(|f| f.as_u64()) {
        if found > u64::from(FORMAT) {
            return Err(Error::UnsupportedFormat {
                found,
                supported: FORMAT,
            });
        }
    }
    Ok(raw)
}

/// The show a parsed document describes, when it describes one at all.
pub(super) fn parse_show(raw: &serde_json::Value) -> Result<Show, Error> {
    let show: Show =
        serde_json::from_value(raw.clone()).map_err(|e| Error::InvalidShow(e.to_string()))?;
    if show.format == 0 {
        return Err(Error::InvalidShow("format 0 does not exist".into()));
    }
    Ok(show)
}

/// One thing wrong with a show, and where in the document it is.
///
/// What a strict load refuses a show for, one at a time, and what a
/// tolerant load drops and reports, all at once. The path is the
/// document's: `layers[2]`, `scenes[1].layers[0].children[3]`,
/// `fonts.score`, `output.tint`, `background`. A host loading the show's
/// files reports its own findings in the same shape, with the file's
/// path in the show folder for the path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub path: String,
    pub message: String,
    /// What sort of thing it is, so a host can show or keep the sorts it
    /// cares about.
    #[serde(default)]
    pub kind: FindingKind,
}

/// What sort of thing a [`Finding`] is. Sorts, not severities: which of
/// them matter is the reader's call, and a project may care about one
/// and not another.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Wrong: what a strict load refuses, and a tolerant load drops.
    #[default]
    Error,
    /// A name with nothing behind it: a file the show names that is
    /// not there, a variable nothing declares, a driver step into
    /// nothing. It runs, and that part of it does nothing.
    Missing,
    /// Dead weight: a file, layer, timeline, scene, variable or font
    /// style nothing uses.
    Unused,
    /// Works today and will surprise someone: a field the engine does
    /// not know, two layers of one name, a habit that costs.
    Unwise,
}

impl FindingKind {
    /// The four, in the order they are worth reading.
    pub const ALL: [FindingKind; 4] = [
        FindingKind::Error,
        FindingKind::Missing,
        FindingKind::Unused,
        FindingKind::Unwise,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FindingKind::Error => "error",
            FindingKind::Missing => "missing",
            FindingKind::Unused => "unused",
            FindingKind::Unwise => "unwise",
        }
    }

    /// The kind `name` names.
    pub fn parse(name: &str) -> Option<FindingKind> {
        FindingKind::ALL
            .into_iter()
            .find(|kind| kind.name() == name)
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// A place in the document a problem is about, and that a tolerant load
/// drops to be rid of it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Site {
    Background,
    /// The tint of the show's output, or of the scene's at this index.
    Tint(Option<usize>),
    /// A pass of the show's output, or of the scene's at this index.
    Pass(Option<usize>, usize),
    /// The font style of this name.
    Font(String),
    /// The value the show animates of this name.
    Value(String),
    /// The layer at this path down this tree: each step an index and
    /// whether it is into the `parts` of an artwork layer rather than
    /// the `children` of a group.
    Layer(Root, Vec<(usize, bool)>),
    /// The scene at this index.
    Scene(usize),
}

impl Site {
    /// Where this is, as a path in the document.
    pub(crate) fn path(&self) -> String {
        fn output(scene: Option<usize>) -> String {
            match scene {
                None => "output".to_owned(),
                Some(i) => format!("scenes[{i}].output"),
            }
        }
        match self {
            Site::Background => "background".to_owned(),
            Site::Tint(scene) => format!("{}.tint", output(*scene)),
            Site::Pass(scene, i) => format!("{}.passes[{i}]", output(*scene)),
            Site::Font(name) => format!("fonts.{name}"),
            Site::Value(name) => format!("values.{name}"),
            Site::Scene(i) => format!("scenes[{i}]"),
            Site::Layer(root, path) => {
                let mut out = match root {
                    Root::Show => "layers".to_owned(),
                    Root::Scene(i) => format!("scenes[{i}].layers"),
                };
                for (depth, (i, parts)) in path.iter().enumerate() {
                    if depth > 0 {
                        out.push_str(if *parts { ".parts" } else { ".children" });
                    }
                    out.push_str(&format!("[{i}]"));
                }
                out
            }
        }
    }

    /// Whether dropping this leaves a blank in its place until the end,
    /// so that the sites after it keep their indices: a layer or a
    /// scene. Everything else is taken out on the spot.
    fn is_blanked(&self) -> bool {
        matches!(self, Site::Layer(..) | Site::Scene(_))
    }

    /// The list this sits in, when it is an entry of one.
    fn list_of<'a>(
        &self,
        raw: &'a mut serde_json::Value,
    ) -> Option<(&'a mut Vec<serde_json::Value>, usize)> {
        fn output(
            raw: &mut serde_json::Value,
            scene: Option<usize>,
        ) -> Option<&mut serde_json::Value> {
            match scene {
                None => raw.get_mut("output"),
                Some(i) => raw.get_mut("scenes")?.get_mut(i)?.get_mut("output"),
            }
        }
        let (list, index) = match self {
            Site::Pass(scene, i) => (output(raw, *scene)?.get_mut("passes")?, *i),
            Site::Scene(i) => (raw.get_mut("scenes")?, *i),
            Site::Layer(root, path) => {
                let (last, above) = path.split_last()?;
                let mut list = match root {
                    Root::Show => raw.get_mut("layers")?,
                    Root::Scene(i) => raw.get_mut("scenes")?.get_mut(*i)?.get_mut("layers")?,
                };
                // Which list a step descends into is what the step
                // after it says it is in.
                for (k, (i, _)) in above.iter().enumerate() {
                    let key = if path[k + 1].1 { "parts" } else { "children" };
                    list = list.get_mut(*i)?.get_mut(key)?;
                }
                (list, last.0)
            }
            _ => return None,
        };
        let list = list.as_array_mut()?;
        (index < list.len()).then_some((list, index))
    }

    /// Take this out of the document, so that what is left loads. A
    /// layer or a scene is blanked instead (see [`Site::is_blanked`]),
    /// and [`remove_blanks`] takes it out.
    fn drop_from(&self, raw: &mut serde_json::Value) {
        match self {
            Site::Background => {
                raw.as_object_mut().map(|show| show.remove("background"));
            }
            Site::Tint(scene) => {
                let output = match scene {
                    None => raw.get_mut("output"),
                    Some(i) => raw
                        .get_mut("scenes")
                        .and_then(|s| s.get_mut(*i))
                        .and_then(|s| s.get_mut("output")),
                };
                output
                    .and_then(serde_json::Value::as_object_mut)
                    .map(|output| output.remove("tint"));
            }
            Site::Font(name) => {
                raw.get_mut("fonts")
                    .and_then(serde_json::Value::as_object_mut)
                    .map(|fonts| fonts.remove(name));
            }
            Site::Value(name) => {
                raw.get_mut("values")
                    .and_then(serde_json::Value::as_object_mut)
                    .map(|values| values.remove(name));
            }
            Site::Pass(..) => {
                if let Some((list, i)) = self.list_of(raw) {
                    list.remove(i);
                }
            }
            Site::Layer(_, path) => {
                let part = path.last().is_some_and(|(_, parts)| *parts);
                if let Some((list, i)) = self.list_of(raw) {
                    list[i] = match part {
                        true => serde_json::json!({ "id": "" }),
                        false => serde_json::json!({ "name": "", "type": "group", "children": [] }),
                    };
                }
            }
            Site::Scene(_) => {
                if let Some((list, i)) = self.list_of(raw) {
                    list[i] = serde_json::json!({ "name": "", "layers": [] });
                }
            }
        }
    }
}

/// Take the blanked layers and scenes out of the document, later ones
/// first so the index of an earlier one still holds, and layers before
/// the scenes that hold them.
pub(super) fn remove_blanks(raw: &mut serde_json::Value, mut blanked: Vec<Site>) {
    blanked.sort();
    blanked.dedup();
    let (scenes, layers): (Vec<Site>, Vec<Site>) = blanked
        .into_iter()
        .partition(|site| matches!(site, Site::Scene(_)));
    for site in layers.iter().rev().chain(scenes.iter().rev()) {
        if let Some((list, i)) = site.list_of(raw) {
            list.remove(i);
        }
    }
}

/// Blank whatever does not parse and can be done without: a layer, a
/// font style, a variable, a show value, an output, a scene. Each is
/// tried on its own, so one that is broken does not take the document
/// with it, and each is a finding. What is left is a document
/// [`parse_show`] takes, or the document was never a show. Layers and
/// scenes are blanked and listed in `blanked`, the rest taken out.
fn salvage(raw: &mut serde_json::Value, findings: &mut Vec<Finding>, blanked: &mut Vec<Site>) {
    fn field<T: serde::de::DeserializeOwned>(
        object: &mut serde_json::Value,
        key: &str,
        path: &str,
        findings: &mut Vec<Finding>,
    ) {
        let Some(object) = object.as_object_mut() else {
            return;
        };
        if let Some(value) = object.get(key) {
            if let Err(e) = serde_json::from_value::<T>(value.clone()) {
                findings.push(Finding {
                    path: path.to_owned(),
                    message: e.to_string(),
                    kind: FindingKind::Error,
                });
                object.remove(key);
            }
        }
    }
    fn entries<T: serde::de::DeserializeOwned>(
        map: Option<&mut serde_json::Value>,
        path: &str,
        findings: &mut Vec<Finding>,
    ) {
        let Some(map) = map.and_then(serde_json::Value::as_object_mut) else {
            return;
        };
        map.retain(
            |name, value| match serde_json::from_value::<T>(value.clone()) {
                Ok(_) => true,
                Err(e) => {
                    findings.push(Finding {
                        path: format!("{path}.{name}"),
                        message: e.to_string(),
                        kind: FindingKind::Error,
                    });
                    false
                }
            },
        );
    }
    if let Some(list) = raw.get_mut("layers") {
        salvage_layers(list, Root::Show, &mut Vec::new(), false, findings, blanked);
    }
    field::<Output>(raw, "output", "output", findings);
    entries::<crate::model::FontStyle>(raw.get_mut("fonts"), "fonts", findings);
    entries::<Value>(raw.get_mut("variables"), "variables", findings);
    entries::<crate::model::ShowValue>(raw.get_mut("values"), "values", findings);
    if let Some(scenes) = raw
        .get_mut("scenes")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (i, scene) in scenes.iter_mut().enumerate() {
            let here = format!("scenes[{i}]");
            field::<Output>(scene, "output", &format!("{here}.output"), findings);
            if let Some(list) = scene.get_mut("layers") {
                salvage_layers(
                    list,
                    Root::Scene(i),
                    &mut Vec::new(),
                    false,
                    findings,
                    blanked,
                );
            }
            if let Err(e) = serde_json::from_value::<crate::model::Scene>(scene.clone()) {
                findings.push(Finding {
                    path: here,
                    message: e.to_string(),
                    kind: FindingKind::Error,
                });
                let site = Site::Scene(i);
                *scene = serde_json::json!({ "name": "", "layers": [] });
                blanked.push(site);
            }
        }
    }
}

/// [`salvage`] for one list of layers: children first, and on their
/// own, so a broken child is one blanked layer, not a blanked group.
fn salvage_layers(
    list: &mut serde_json::Value,
    root: Root,
    path: &mut Vec<(usize, bool)>,
    parts: bool,
    findings: &mut Vec<Finding>,
    blanked: &mut Vec<Site>,
) {
    let Some(list) = list.as_array_mut() else {
        return;
    };
    for (i, layer) in list.iter_mut().enumerate() {
        path.push((i, parts));
        if let Some(children) = layer.get_mut("children") {
            salvage_layers(children, root, path, false, findings, blanked);
        }
        if let Some(inner) = layer.get_mut("parts") {
            salvage_layers(inner, root, path, true, findings, blanked);
        }
        let parses = match parts {
            true => serde_json::from_value::<crate::model::Part>(layer.clone()).map(|_| ()),
            false => serde_json::from_value::<Layer>(layer.clone()).map(|_| ()),
        };
        if let Err(e) = parses {
            let site = Site::Layer(root, path.clone());
            findings.push(Finding {
                path: site.path(),
                message: e.to_string(),
                kind: FindingKind::Error,
            });
            // A blank of the kind the list holds: an empty group, or a
            // part of nothing.
            *layer = match parts {
                true => serde_json::json!({ "id": "" }),
                false => serde_json::json!({ "name": "", "type": "group", "children": [] }),
            };
            blanked.push(site);
        }
        path.pop();
    }
}
