//! Sound and video on a layer: which asset a play takes, what a second
//! play does, and the buses that duck.

use super::*;

/// One asset name, or several for a layer to pick between: written as a
/// name or a list of names.
///
/// A layer that names several plays one of them per play, chosen by its
/// [`Pick`]. It is the same idea whatever the asset: a sound with three
/// recordings of the same knock, a layer with a folder of clips to show
/// between rounds, an idle animation that should not look like a loop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice(pub Vec<String>);

impl Choice {
    /// A choice of exactly one name.
    pub fn one(name: impl Into<String>) -> Choice {
        Choice(vec![name.into()])
    }

    /// The name at `index`, or nothing when there are none.
    pub fn get(&self, index: usize) -> &str {
        self.0.get(index).map_or("", String::as_str)
    }

    /// The first name, or `""`.
    pub fn first(&self) -> &str {
        self.get(0)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl Serialize for Choice {
    /// Written back the way it is usually authored: one name, or a list.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.as_slice() {
            [one] => serializer.serialize_str(one),
            many => many.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Choice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            One(String),
            Many(Vec<String>),
        }
        Ok(Choice(match Option::<Form>::deserialize(deserializer)? {
            None => Vec::new(),
            Some(Form::One(name)) => vec![name],
            Some(Form::Many(names)) => names,
        }))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Choice {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Choice".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "An asset name, or a list of names to pick between; see `pick`.",
            "anyOf": [
                { "type": "string" },
                { "type": "array", "items": { "type": "string" } }
            ]
        })
    }
}

/// Which of a layer's several assets a play uses.
///
/// Every one of these is a function of how many times the layer has
/// played, never of a running dice roll, so a show plays the same way
/// twice and a host that seeds the engine
/// ([`Engine::set_seed`](crate::Engine::set_seed)) decides how much it
/// varies between runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Pick {
    /// The next one each play, wrapping round at the end.
    #[default]
    InOrder,
    /// Any of them, which may be the one that just played.
    Random,
    /// All of them in a scrambled order, then scrambled again: varied,
    /// but nothing is skipped and nothing repeats until the rest have had
    /// their turn.
    Shuffle,
}

pub(super) fn default_voices() -> u32 {
    4
}

/// Which registry the content of a playhead comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Sound,
    Video,
}

/// The playhead of a layer whose content has a length: what starts it,
/// how long it goes on and what it says when it ends. A sound and a video
/// carry the same one, so the engine runs both through the same code.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Media<'a> {
    pub kind: MediaKind,
    /// The assets the host registered, by name: one, or several to pick
    /// between.
    pub names: &'a Choice,
    /// Which of `names` a play uses.
    pub pick: Pick,
    pub trigger: &'a Triggers,
    pub stop: &'a Triggers,
    /// A condition that plays it on its rising edge.
    pub when: Option<&'a When>,
    /// A condition it plays under.
    pub whilst: Option<&'a While>,
    pub autoplay: bool,
    pub looping: bool,
    pub delay: f64,
    pub repeat: Option<f64>,
    pub on_end: Option<&'a str>,
    /// What a trigger does while it already plays.
    pub retrigger: Retrigger,
    /// Least seconds between plays; 0 never drops a trigger.
    pub rest: f64,
    /// How many plays may run at once; one for a video.
    pub voices: u32,
}

/// What an audio layer's trigger does while the layer is already playing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Retrigger {
    /// Start over: the play so far stops.
    #[default]
    Restart,
    /// Start another play on top, up to the layer's `voices`.
    Overlap,
    /// Let the play finish; the trigger does nothing.
    Ignore,
    /// Let the play finish, then play: the trigger waits its turn, up to
    /// `voices` of them waiting at once.
    Queue,
}

/// The bus a sound is on when it names none.
///
/// Every sound is on a bus, so a show that never mentions one can still
/// be ducked under: give the bed a bus of its own and point its `duck` at
/// this.
pub const MAIN_BUS: &str = "main";

/// Step this layer back while something on another bus is sounding.
///
/// A bed under clips that speak over it has to get out of the way and
/// come back, which is the ordinary arrangement whenever there is music
/// under anything that talks. `gain` is bindable and a transition can
/// ease it, but nothing in a show can see that something else is
/// sounding, so without this a host has to watch the engine's voices and
/// feed a variable back, putting show structure outside the show.
///
/// It multiplies like every other gain, so it composes with bindings and
/// with the gain of the groups above rather than fighting them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Duck {
    /// The bus to listen to. Anything sounding on it ducks this layer;
    /// the layer's own plays never do.
    pub under: String,
    /// Gain multiplier while that bus sounds. 0.1 is a tenth.
    pub to: f64,
    /// Seconds to go down. 0 (the default) drops at once, which is what
    /// makes room in time for the first word.
    #[serde(default)]
    pub attack: f64,
    /// Seconds to come back up once the bus falls silent. The part that
    /// has to be smooth.
    #[serde(default)]
    pub release: f64,
}
