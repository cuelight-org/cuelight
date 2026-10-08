//! Components: layers written once under the show's `components` and
//! used by name, expanded before anything else reads the document.
//!
//! A layer of type `component` becomes a group holding a copy of the
//! component's layers, with every `{input}` in them filled in from the
//! layer's `with`. Nothing after this sees a component, so the engine,
//! the draw list and every host work on the expanded show, and a layer's
//! path is the same before and after: the component's layers are the
//! group's children.

use super::{Finding, FindingKind};
use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;

/// A component as the show declares it.
struct Component {
    inputs: Vec<String>,
    layers: Vec<Json>,
}

/// Expand every component layer in `raw` and take `components` out.
///
/// What cannot be expanded is a finding at the path of the layer that
/// asked for it, and that layer is left an empty group; a component that
/// is broken itself is a finding at `components.<name>`.
pub(super) fn expand_components(raw: &mut Json) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(object) = raw.as_object_mut() else {
        return findings;
    };
    let components = match object.remove("components") {
        None => BTreeMap::new(),
        Some(Json::Object(map)) => map
            .into_iter()
            .map(|(name, value)| match component(&value) {
                Ok(component) => (name, Ok(component)),
                Err(message) => {
                    findings.push(error(format!("components.{name}"), message));
                    (name, Err(()))
                }
            })
            .collect(),
        Some(_) => {
            findings.push(error(
                "components".into(),
                "`components` is a map from a name to a component".into(),
            ));
            BTreeMap::new()
        }
    };
    let mut expand = Expand {
        components: &components,
        using: Vec::new(),
        findings: &mut findings,
    };
    if let Some(list) = object.get_mut("layers") {
        expand.list(list, "layers");
    }
    if let Some(scenes) = object.get_mut("scenes").and_then(Json::as_array_mut) {
        for (i, scene) in scenes.iter_mut().enumerate() {
            if let Some(list) = scene.get_mut("layers") {
                expand.list(list, &format!("scenes[{i}].layers"));
            }
        }
    }
    findings
}

fn error(path: String, message: String) -> Finding {
    Finding {
        path,
        message,
        kind: FindingKind::Error,
    }
}

/// Read one entry of `components`.
fn component(value: &Json) -> Result<Component, String> {
    let Some(object) = value.as_object() else {
        return Err("a component is an object with `inputs` and `layers`".into());
    };
    if let Some(key) = object.keys().find(|k| *k != "inputs" && *k != "layers") {
        return Err(format!(
            "a component has `inputs` and `layers`, not `{key}`"
        ));
    }
    let mut inputs: Vec<String> = Vec::new();
    match object.get("inputs") {
        None => {}
        Some(Json::Array(list)) => {
            for input in list {
                let Some(name) = input.as_str().filter(|n| is_identifier(n)) else {
                    return Err(format!(
                        "an input is a name of letters, digits and `_`, not {input}"
                    ));
                };
                if inputs.iter().any(|i| i == name) {
                    return Err(format!("input `{name}` is declared twice"));
                }
                inputs.push(name.to_owned());
            }
        }
        Some(_) => return Err("`inputs` is a list of names".into()),
    }
    let Some(layers) = object.get("layers").and_then(Json::as_array) else {
        return Err("a component needs `layers`, a list of layers".into());
    };
    // Every `{name}` in the layers must be one of the inputs, so a typo
    // is refused here rather than shown on screen.
    let mut unknown = None;
    for layer in layers {
        placeholders(layer, &mut |name| {
            if unknown.is_none() && !inputs.iter().any(|i| i == name) {
                unknown = Some(name.to_owned());
            }
        });
    }
    if let Some(name) = unknown {
        return Err(format!("`{{{name}}}` is not one of the component's inputs"));
    }
    Ok(Component {
        inputs,
        layers: layers.clone(),
    })
}

struct Expand<'a> {
    components: &'a BTreeMap<String, Result<Component, ()>>,
    /// The components being expanded around the current layer, to refuse
    /// one that uses itself.
    using: Vec<String>,
    findings: &'a mut Vec<Finding>,
}

impl Expand<'_> {
    fn list(&mut self, list: &mut Json, path: &str) {
        let Some(list) = list.as_array_mut() else {
            return;
        };
        for (i, layer) in list.iter_mut().enumerate() {
            let here = format!("{path}[{i}]");
            if layer.get("type").and_then(Json::as_str) == Some("component") {
                if let Err((inside, message)) = self.instance(layer) {
                    self.findings
                        .push(error(format!("{here}{inside}"), message));
                    let name = layer.get("name").cloned().unwrap_or_default();
                    *layer = serde_json::json!({ "name": name, "type": "group", "children": [] });
                    continue;
                }
            }
            if let Some(children) = layer.get_mut("children") {
                self.list(children, &format!("{here}.children"));
            }
        }
    }

    /// Turn one component layer into the group it stands for, then
    /// expand the components inside it. What is wrong comes back with
    /// where under the layer it is: nothing for the layer itself.
    fn instance(&mut self, layer: &mut Json) -> Result<(), (String, String)> {
        let name = self
            .expand_instance(layer)
            .map_err(|message| (String::new(), message))?;
        self.inside(layer, name)
    }

    /// Expand the layer itself, and say which component it was.
    fn expand_instance(&mut self, layer: &mut Json) -> Result<String, String> {
        let object = layer.as_object_mut().ok_or("a layer is an object")?;
        let Some(name) = object.get("component").and_then(Json::as_str) else {
            return Err("a component layer names its component in `component`".into());
        };
        let name = name.to_owned();
        let component = match self.components.get(&name) {
            None => return Err(format!("there is no component named `{name}`")),
            Some(Err(())) => {
                return Err(format!(
                    "component `{name}` cannot be used: see components.{name}"
                ))
            }
            Some(Ok(component)) => component,
        };
        if self.using.contains(&name) {
            return Err(format!("component `{name}` uses itself"));
        }
        if object.contains_key("children") {
            return Err("a component layer's children are the component's layers".into());
        }
        let with = match object.remove("with") {
            None => Map::new(),
            Some(Json::Object(with)) => with,
            Some(_) => return Err("`with` is a map from an input to its value".into()),
        };
        if let Some(input) = with.keys().find(|k| !component.inputs.contains(k)) {
            return Err(format!("component `{name}` has no input `{input}`"));
        }
        if let Some(input) = component.inputs.iter().find(|i| !with.contains_key(*i)) {
            return Err(format!("`with` gives no value for input `{input}`"));
        }
        let mut children = Json::Array(component.layers.clone());
        fill(&mut children, &with)?;
        object.remove("component");
        object.insert("type".into(), "group".into());
        object.insert("children".into(), children);
        Ok(name)
    }

    /// Expand the components inside a layer just expanded.
    fn inside(&mut self, layer: &mut Json, name: String) -> Result<(), (String, String)> {
        self.using.push(name);
        let mut inner = Vec::new();
        let mut nested = Expand {
            components: self.components,
            using: std::mem::take(&mut self.using),
            findings: &mut inner,
        };
        if let Some(children) = layer.get_mut("children") {
            nested.list(children, ".children");
        }
        self.using = nested.using;
        self.using.pop();
        // A component inside this one that cannot be used makes this one
        // unusable too: a half expanded copy would draw something else.
        match inner.into_iter().next() {
            Some(finding) => Err((finding.path, finding.message)),
            None => Ok(()),
        }
    }
}

/// Fill in every `{input}` under `value` from `with`. A string that is
/// exactly `{input}` takes the input's value as it is, a number, a list
/// or anything else; `{input}` inside a longer string takes it as text.
fn fill(value: &mut Json, with: &Map<String, Json>) -> Result<(), String> {
    match value {
        Json::String(text) => {
            if let Some(name) = whole_placeholder(text) {
                *value = with[name].clone();
                return Ok(());
            }
            let mut filled = String::new();
            let mut rest = text.as_str();
            while let Some((before, name, after)) = next_placeholder(rest) {
                filled.push_str(before);
                match &with[name] {
                    Json::String(s) => filled.push_str(s),
                    Json::Number(n) => filled.push_str(&n.to_string()),
                    other => {
                        return Err(format!(
                            "input `{name}` is {other}, which cannot go inside a text"
                        ))
                    }
                }
                rest = after;
            }
            filled.push_str(rest);
            *text = filled;
        }
        Json::Array(list) => {
            for item in list {
                fill(item, with)?;
            }
        }
        Json::Object(map) => {
            for item in map.values_mut() {
                fill(item, with)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Call `found` with the name of every `{name}` under `value`.
fn placeholders(value: &Json, found: &mut impl FnMut(&str)) {
    match value {
        Json::String(text) => {
            let mut rest = text.as_str();
            while let Some((_, name, after)) = next_placeholder(rest) {
                found(name);
                rest = after;
            }
        }
        Json::Array(list) => list.iter().for_each(|item| placeholders(item, found)),
        Json::Object(map) => map.values().for_each(|item| placeholders(item, found)),
        _ => {}
    }
}

fn whole_placeholder(text: &str) -> Option<&str> {
    match next_placeholder(text) {
        Some(("", name, "")) => Some(name),
        _ => None,
    }
}

/// The first `{name}` in `text`: what comes before it, the name, and
/// what comes after. Braces around anything but a name are left alone.
fn next_placeholder(text: &str) -> Option<(&str, &str, &str)> {
    let mut from = 0;
    while let Some(open) = text[from..].find('{').map(|i| from + i) {
        if let Some(close) = text[open..].find('}').map(|i| open + i) {
            let name = &text[open + 1..close];
            if is_identifier(name) {
                return Some((&text[..open], name, &text[close + 1..]));
            }
        }
        from = open + 1;
    }
    None
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The JSON Schema of a show document: the model's, and the components
/// a document may declare and use, which never reach the model.
#[cfg(feature = "schema")]
pub fn show_schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(crate::model::Show);
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    let components = serde_json::json!({
        "description": "Layers written once and used by name from a layer of type \
                        `component`, by the show's layers and its scenes'.",
        "type": "object",
        "additionalProperties": { "$ref": "#/$defs/Component" },
        "default": {}
    });
    let component = serde_json::json!({
        "description": "Layers written once, with what differs between their uses \
                        as named inputs.",
        "type": "object",
        "properties": {
            "inputs": {
                "description": "What a use fills in: an `{input}` anywhere in the layers \
                                takes the value its `with` gives. A string that is only \
                                `{input}` takes the value as it is, a number or a list; \
                                inside a longer string it goes in as text.",
                "type": "array",
                "items": { "type": "string", "pattern": "^[A-Za-z_][A-Za-z0-9_]*$" },
                "default": []
            },
            "layers": {
                "description": "The layers each use draws, as the children of the layer \
                                that uses them.",
                "type": "array",
                "items": { "$ref": "#/$defs/Layer" }
            }
        },
        "required": ["layers"],
        "additionalProperties": false
    });
    if let Some(properties) = object.get_mut("properties").and_then(Json::as_object_mut) {
        properties.insert("components".into(), components);
    }
    let Some(defs) = object.get_mut("$defs").and_then(Json::as_object_mut) else {
        return schema;
    };
    defs.insert("Component".into(), component);
    if let Some(kinds) = defs
        .get_mut("Layer")
        .and_then(|layer| layer.get_mut("oneOf"))
        .and_then(Json::as_array_mut)
    {
        kinds.push(serde_json::json!({
            "description": "A use of a component: a group whose children are the \
                            component's layers, with its inputs filled in.",
            "type": "object",
            "properties": {
                "type": { "type": "string", "const": "component" },
                "component": {
                    "description": "The name of the component under `components`.",
                    "type": "string"
                },
                "with": {
                    "description": "A value for each of the component's inputs, and \
                                    nothing else.",
                    "type": "object",
                    "default": {}
                }
            },
            "required": ["type", "component"]
        }));
    }
    schema
}
