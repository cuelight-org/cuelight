use crate::easing::Easing;
use crate::path::PathData;
use crate::value::Value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod binding;
mod layer;
mod media;
mod output;
mod reel;
mod segments;
mod shape;
mod text;
mod timeline;

pub use binding::*;
pub use layer::*;
pub use media::*;
pub use output::*;
pub use reel::*;
pub use segments::*;
pub use shape::*;
pub use text::*;
pub use timeline::*;

/// The show format version this engine reads and writes. It goes up when a
/// change could make an older engine misread a show; engines refuse shows
/// of a newer format instead of playing them wrongly.
pub const FORMAT: u32 = 1;

fn default_format() -> u32 {
    1
}

/// A declarative show description: what a `.json` show file deserializes into.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Show {
    /// Show format version, see [`FORMAT`]. A show without it is format 1.
    #[serde(default = "default_format")]
    pub format: u32,
    pub name: String,
    /// Logical canvas size in pixels `[width, height]`. Hosts scale the
    /// rendered texture; content is authored against this space.
    /// Coordinates have their origin at the top-left corner: x grows
    /// right, y grows down.
    pub size: [u32; 2],
    /// Background color, `#RRGGBB` or `#RRGGBBAA`.
    #[serde(default = "default_background")]
    pub background: String,
    /// How rendered colors reach the display: full color by default, or
    /// quantized luminance tinted in one color (DMD style). Scenes can
    /// override it.
    #[serde(default)]
    pub output: Output,
    /// Named text styles for text layers: a bitmap font plus colors.
    #[serde(default)]
    pub fonts: BTreeMap<String, FontStyle>,
    /// Declared variables and their initial values.
    #[serde(default)]
    pub variables: BTreeMap<String, Value>,
    /// Values the show animates itself, read by bindings the way a
    /// variable is. For motion that belongs to the content rather than
    /// to whatever is driving it.
    #[serde(default)]
    pub values: BTreeMap<String, ShowValue>,
    /// Layers that are always present, painted behind the active scene.
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Switchable views; exactly one is active at a time (the first one
    /// when the show loads), entered by firing its trigger.
    #[serde(default)]
    pub scenes: Vec<Scene>,
    /// What a key or a press on the show means, so a show can be played
    /// with rather than only driven.
    #[serde(default, skip_serializing_if = "Input::is_empty")]
    pub input: Input,
}

/// What the show makes of a host's keys and presses.
///
/// Declared by the show rather than wired into each host, so the player,
/// a browser and an embedder behave the same, and a host stays a source
/// of events: it says which key went down or where a press landed, and
/// the show says what that means.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Keys that fire a trigger, by the name a browser gives the key
    /// (`KeyboardEvent.key`): `ArrowRight`, `Enter`, `a`, `" "` for the
    /// space bar. Names are matched exactly, so `A` is shift and `a` is
    /// not.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    /// Trigger fired by a press that lands on nothing pressable: "click
    /// anywhere to go on". A press on a layer with its own `press` fires
    /// that instead, and never this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<String>,
    /// Variables the host keeps up to date with where the pointer is, a
    /// mouse or a finger: for eyes that follow the cursor, a spotlight,
    /// a hover highlight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<Pointer>,
}

impl Input {
    /// Whether the show says nothing about input.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.press.is_none() && self.pointer.is_none()
    }

    /// Whether the host sets the variable `name` for the show: one of the
    /// pointer's, which need no declaring.
    pub fn sets(&self, name: &str) -> bool {
        self.pointer
            .as_ref()
            .is_some_and(|pointer| pointer.names().any(|n| n == name))
    }
}

/// The variables a host sets as the pointer moves, each named by the
/// show; one left out is not set.
///
/// `x` and `y` are in canvas coordinates and stop at the canvas edge, so
/// a pointer beside the canvas still gives a place on it; `over` is
/// whether it is over the canvas. A pointer that leaves leaves `x` and
/// `y` where it was last. With several fingers down, the first one is
/// the pointer.
///
/// They are ordinary variables, so bindings, curves and transitions read
/// them as any other; a show value of the same name, with timelines of
/// its own, moves until a real pointer takes over.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Pointer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
    /// The name of the topmost pressable layer drawn under the pointer,
    /// the one a press there would hit; `""` over nothing. Only
    /// pressable layers count, as for a press: an overlay drawn on top
    /// is see-through. Set by a host that draws the show
    /// (`cuelight::Engine::point`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<String>,
}

impl Pointer {
    /// The variable names it sets.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        [&self.x, &self.y, &self.over, &self.under]
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

/// The trigger names something listens to: in a show document one name
/// (`"go"`), a list (`["turn_left", "hazard"]`), or nothing. Firing any of
/// them has the same effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Triggers(pub Vec<String>);

impl Triggers {
    pub fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|t| t == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Triggers {
    /// Written back the way it is usually authored: nothing, one name, or
    /// a list.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.as_slice() {
            [] => serializer.serialize_none(),
            [one] => serializer.serialize_str(one),
            many => many.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Triggers {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            One(String),
            Many(Vec<String>),
        }
        Ok(Triggers(match Option::<Form>::deserialize(deserializer)? {
            None => Vec::new(),
            Some(Form::One(name)) => vec![name],
            Some(Form::Many(names)) => names,
        }))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Triggers {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Triggers".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A trigger name, or a list of names; firing any of them has the same effect.",
            "anyOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "string" } },
                { "type": "null" }
            ]
        })
    }
}

/// A switchable view of the show: its layers render only while it is the
/// active scene. Entering a scene (again) restarts it: timelines of the
/// previous scene stop, the entered scene's autoplay timelines start at 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Scene {
    pub name: String,
    /// Trigger name, or list of names, that enters this scene.
    #[serde(default)]
    pub trigger: Triggers,
    /// Output color handling while this scene is active; the show's when
    /// omitted.
    #[serde(default)]
    pub output: Option<Output>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

fn default_background() -> String {
    "#000000".to_owned()
}

/// Where a trigger is listened to; see [`Show::listeners`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listened {
    /// Firing it enters this scene.
    Opens(String),
    /// The show's own layers hear it, or more than one scene does, or
    /// nothing does and a key or a press fires it.
    Anywhere,
    /// Only this scene hears it.
    Scene(String),
}

impl Show {
    /// Every trigger name the show listens to: what enters its scenes,
    /// what starts its timelines and what plays or stops its sounds, in
    /// any layer tree. What a host can offer as the show's actions.
    pub fn triggers(&self) -> std::collections::BTreeSet<String> {
        fn timelines(layers: &[Layer], out: &mut std::collections::BTreeSet<String>) {
            for layer in layers {
                for timeline in &layer.timelines {
                    out.extend(timeline.trigger.iter().map(str::to_owned));
                }
                if let Some(media) = layer.kind.media() {
                    out.extend(
                        media
                            .trigger
                            .iter()
                            .chain(media.stop.iter())
                            .map(str::to_owned),
                    );
                }
                if let LayerKind::Digits {
                    display: DigitDisplay::Reel(reel),
                    ..
                } = &layer.kind
                {
                    out.extend(reel.spin.iter().map(str::to_owned));
                }
                timelines(layer.children(), out);
            }
        }
        let mut out = std::collections::BTreeSet::new();
        for scene in &self.scenes {
            out.extend(scene.trigger.iter().map(str::to_owned));
        }
        for layers in self.layer_trees() {
            timelines(layers, &mut out);
        }
        // A value the show animates is started by a trigger like a
        // layer's timeline is.
        for value in self.values.values() {
            for timeline in &value.timelines {
                out.extend(timeline.trigger.iter().map(str::to_owned));
            }
        }
        out
    }

    /// Where each trigger is listened to, by name: what [`triggers`](Show::triggers)
    /// lists, and the triggers the show's keys and presses fire, so a
    /// host can group its actions and dim the ones the active scene is
    /// not listening to.
    ///
    /// A trigger that enters a scene is [`Listened::Opens`] whatever else
    /// hears it. One heard by the show's own layers, or by more than one
    /// scene, is [`Listened::Anywhere`], and so is one nothing hears but
    /// a key or a press fires. One only a scene's layers hear is
    /// [`Listened::Scene`]. Heard means a timeline's `trigger`, a sound's
    /// or video's `trigger` and `stop`, and a reel's `spin`.
    pub fn listeners(&self) -> BTreeMap<String, Listened> {
        fn heard(layers: &[Layer], out: &mut Vec<String>, fired: &mut Vec<String>) {
            for layer in layers {
                for timeline in &layer.timelines {
                    out.extend(timeline.trigger.iter().map(str::to_owned));
                }
                if let Some(media) = layer.kind.media() {
                    out.extend(
                        media
                            .trigger
                            .iter()
                            .chain(media.stop.iter())
                            .map(str::to_owned),
                    );
                }
                if let LayerKind::Digits {
                    display: DigitDisplay::Reel(reel),
                    ..
                } = &layer.kind
                {
                    out.extend(reel.spin.iter().map(str::to_owned));
                }
                if let Some(press) = &layer.press {
                    fired.extend(press.trigger.clone());
                }
                heard(layer.children(), out, fired);
            }
        }
        let mut out: BTreeMap<String, Listened> = BTreeMap::new();
        // Fired by an input rather than heard: an action all the same.
        let mut fired: Vec<String> = self.input.keys.values().cloned().collect();
        fired.extend(self.input.press.clone());
        let mut everywhere: Vec<String> = Vec::new();
        heard(&self.layers, &mut everywhere, &mut fired);
        // The show's values are the show's own, wherever it is.
        for value in self.values.values() {
            for timeline in &value.timelines {
                everywhere.extend(timeline.trigger.iter().map(str::to_owned));
            }
        }
        // By the scenes, one at a time, so a name two of them share is
        // told from one only one has.
        let mut by_scene: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for scene in &self.scenes {
            let mut names = Vec::new();
            heard(&scene.layers, &mut names, &mut fired);
            for name in names {
                let scenes = by_scene.entry(name).or_default();
                if !scenes.contains(&scene.name.as_str()) {
                    scenes.push(&scene.name);
                }
            }
        }
        for name in fired {
            out.insert(name, Listened::Anywhere);
        }
        for (name, scenes) in by_scene {
            let listened = match scenes.as_slice() {
                [only] => Listened::Scene((*only).to_owned()),
                _ => Listened::Anywhere,
            };
            out.insert(name, listened);
        }
        for name in everywhere {
            out.insert(name, Listened::Anywhere);
        }
        // Entering a scene is what firing it does, whoever else hears it;
        // the first scene that answers is the one entered.
        for scene in self.scenes.iter().rev() {
            for name in scene.trigger.iter() {
                out.insert(name.to_owned(), Listened::Opens(scene.name.clone()));
            }
        }
        out
    }

    /// Whether anything in the show can make a sound: an audio layer, in
    /// its own layers or any scene's. A host that opens a sound device
    /// can skip doing so entirely for a show that is silent by
    /// construction.
    ///
    /// A video layer is not counted, even though a clip can be heard: it
    /// is heard only if the host registers a sound under the video's name,
    /// and the show's own document cannot say whether it will. A host that
    /// registers a clip's soundtrack knows it did, and should open a sound
    /// device on that ground rather than on this answer.
    pub fn has_sound(&self) -> bool {
        fn any(layers: &[Layer]) -> bool {
            layers
                .iter()
                .any(|layer| matches!(layer.kind, LayerKind::Audio { .. }) || any(layer.children()))
        }
        self.layer_trees().any(any)
    }

    /// Every layer tree of the show: its own layers, then each scene's.
    pub fn layer_trees(&self) -> impl Iterator<Item = &[Layer]> {
        std::iter::once(self.layers.as_slice())
            .chain(self.scenes.iter().map(|s| s.layers.as_slice()))
    }
}

/// Parse a `#RRGGBB` / `#RRGGBBAA` color into RGBA bytes.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.strip_prefix('#')?;
    let parse = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some([parse(0)?, parse(2)?, parse(4)?, 0xFF]),
        8 => Some([parse(0)?, parse(2)?, parse(4)?, parse(6)?]),
        _ => None,
    }
}
