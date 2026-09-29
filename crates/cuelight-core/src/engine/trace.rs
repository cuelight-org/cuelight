//! The trace: what happened and why, for a host that explains a show.

use super::*;

/// One timeline of the show: whose it is, its index there, and its name.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRef {
    pub owner: TimelineOwner,
    pub index: usize,
    pub name: String,
}

/// What a timeline belongs to.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineOwner {
    Layer(LayerPath),
    /// A value the show animates, by name.
    Value(String),
}

/// Who fired a trigger.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Firing {
    /// The host, through `trigger`, a key or a press.
    Host,
    /// This timeline's `on_end`.
    TimelineEnd(TimelineRef),
    /// The `on_end` of a play on this audio or video layer.
    PlayEnd(LayerPath),
}

/// Why something happened.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Cause {
    /// The show loading, or restarting.
    Load,
    /// The scene of this name being entered.
    Entered(String),
    /// A trigger, and who fired it.
    Trigger { name: String, by: Firing },
    /// A timeline's `when` turning true.
    When,
    /// A timeline's `while` turning true, or false.
    While,
    /// The host asked for it by itself, with
    /// [`start_timeline`](Engine::start_timeline).
    Host,
    /// A binding pointed the layer at it: its `video` or `sound` took a
    /// new name.
    Pointed,
}

/// Why a play of a sound or a clip is over; see [`Happened::Over`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Ending {
    /// It ran to its end, its repeats included, and fired this `on_end`
    /// if it names one.
    Finished { on_end: Option<String> },
    /// This `stop` trigger fired.
    Stop(String),
    /// Its `while` turned false.
    While,
    /// Its trigger fired again and its `retrigger` is `restart`.
    Retriggered,
    /// A newer play took its place: more than the layer's `voices` were
    /// sounding at once.
    Voices,
    /// Its scene was left.
    SceneLeft,
    /// A binding pointed the layer at something else.
    Pointed,
}

/// Which of a timeline's two conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    When,
    While,
}

/// Something that happened inside the show; see [`Engine::drain_trace`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Happened {
    /// A trigger fired, by the host or by the show.
    Fired { name: String, by: Firing },
    /// The host set a variable.
    Set { name: String, value: Value },
    /// A scene became the active one.
    Entered { scene: String, by: Cause },
    /// A timeline started, or restarted.
    Started { timeline: TimelineRef, by: Cause },
    /// A timeline reached its end: gone, or holding its last values.
    Ended { timeline: TimelineRef, held: bool },
    /// A timeline was stopped short by its `while`.
    Stopped { timeline: TimelineRef },
    /// A timeline's condition turned true or false.
    Turned {
        timeline: TimelineRef,
        condition: Which,
        holds: bool,
    },
    /// A sound or a clip began a play: on this layer, of this asset,
    /// with the id [`Engine::voices`] and [`Engine::videos`] report it
    /// under, and why.
    Played {
        layer: LayerPath,
        name: String,
        media: String,
        id: u64,
        by: Cause,
    },
    /// A press asked for this web address to be opened.
    Opened { url: String },
    /// A play of a sound or a clip is over, and how: finished, or
    /// stopped short one way or another. The same `id` its start was
    /// traced with.
    Over {
        layer: LayerPath,
        name: String,
        media: String,
        id: u64,
        by: Ending,
    },
}

/// One record of the trace: what happened, and the instant it did.
#[derive(Debug, Clone, PartialEq)]
pub struct Traced {
    /// On the show's clock: the instant it happened, not the frame that
    /// noticed it.
    pub at: f64,
    pub what: Happened,
}

/// One thing a property's value comes from now; see
/// [`Engine::explain`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Influence {
    /// The value the document gives the property.
    Base { value: Value },
    /// A binding of the property: which, what it reads, and what it
    /// gives; `None` when it has nothing to say now, which leaves the
    /// property to the sources below it.
    Binding {
        index: usize,
        variable: String,
        value: Option<Value>,
    },
    /// A timeline with a track on the property: where it is in its own
    /// time (`None` while it waits out its delay), whether it is holding
    /// its end, and what its track gives.
    Timeline {
        timeline: TimelineRef,
        local: Option<f64>,
        held: bool,
        value: Option<f64>,
    },
}

impl std::fmt::Display for TimelineRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.owner {
            TimelineOwner::Layer(layer) => write!(f, "timeline {:?} of layer {layer}", self.name),
            TimelineOwner::Value(value) => write!(f, "timeline {:?} of value {value:?}", self.name),
        }
    }
}

impl std::fmt::Display for Firing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Firing::Host => write!(f, "by the host"),
            Firing::TimelineEnd(timeline) => write!(f, "at the end of {timeline}"),
            Firing::PlayEnd(layer) => write!(f, "at the end of a play on layer {layer}"),
        }
    }
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cause::Load => write!(f, "at load"),
            Cause::Entered(scene) => write!(f, "on entering scene {scene:?}"),
            Cause::Trigger { name, by } => write!(f, "on {name:?}, fired {by}"),
            Cause::When => write!(f, "as its when turned true"),
            Cause::While => write!(f, "as its while turned true"),
            Cause::Host => write!(f, "asked for by the host"),
            Cause::Pointed => write!(f, "as a binding pointed the layer at it"),
        }
    }
}

impl std::fmt::Display for Ending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ending::Finished { on_end: Some(name) } => write!(f, "finished, firing {name:?}"),
            Ending::Finished { on_end: None } => write!(f, "finished"),
            Ending::Stop(name) => write!(f, "stopped on {name:?}"),
            Ending::While => write!(f, "stopped as its while turned false"),
            Ending::Retriggered => write!(f, "started over"),
            Ending::Voices => write!(f, "gave way to a newer play"),
            Ending::SceneLeft => write!(f, "stopped as its scene was left"),
            Ending::Pointed => write!(f, "stopped as its layer was pointed elsewhere"),
        }
    }
}

impl std::fmt::Display for Happened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Happened::Fired { name, by } => write!(f, "fired {name:?} {by}"),
            Happened::Opened { url } => write!(f, "opened {url:?} on a press"),
            Happened::Set { name, value } => write!(f, "set {name:?} to {}", value.to_text()),
            Happened::Entered { scene, by } => write!(f, "entered scene {scene:?} {by}"),
            Happened::Started { timeline, by } => write!(f, "started {timeline} {by}"),
            Happened::Ended { timeline, held } => match held {
                true => write!(f, "ended {timeline}, holding"),
                false => write!(f, "ended {timeline}"),
            },
            Happened::Stopped { timeline } => {
                write!(f, "stopped {timeline} as its while turned false")
            }
            Happened::Turned {
                timeline,
                condition,
                holds,
            } => {
                let which = match condition {
                    Which::When => "when",
                    Which::While => "while",
                };
                write!(f, "the {which} of {timeline} turned {holds}")
            }
            Happened::Played {
                layer,
                name,
                media,
                by,
                ..
            } => write!(f, "played {media:?} on layer {name:?} ({layer}) {by}"),
            Happened::Over {
                layer,
                name,
                media,
                by,
                ..
            } => write!(f, "{media:?} on layer {name:?} ({layer}) {by}"),
        }
    }
}

impl Engine {
    /// Take the trace since the last call, oldest first: what happened
    /// inside the show and why, each at its own instant. Triggers fired
    /// and by whom, variables set, scenes entered, timelines started (by
    /// which trigger, at load, on entering a scene, by a condition),
    /// ended, held or stopped, and conditions turning. A host that
    /// samples what is playing each frame misses a run that starts and
    /// ends inside one step; this does not, and with the host's own
    /// inputs in it, it is the one record of what happened to a show.
    ///
    /// Kept whether or not anyone reads it, capped at a few thousand
    /// records, so an uninterested host pays a little and never grows.
    pub fn drain_trace(&mut self) -> Vec<Traced> {
        self.trace.drain(..).collect()
    }

    pub(super) fn note(&mut self, at: f64, what: Happened) {
        if self.trace.len() == MAX_PENDING_TRACE {
            self.trace.pop_front();
        }
        self.trace.push_back(Traced { at, what });
    }

    /// The name of scene `scene`, empty when there is none.
    pub(super) fn scene_name(&self, scene: usize) -> String {
        self.show
            .as_ref()
            .and_then(|show| show.scenes.get(scene))
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    /// `index` of `owner`'s timelines, named for the trace.
    pub(super) fn timeline_ref(&self, owner: &Owner, index: usize) -> TimelineRef {
        let show = self.show.as_ref();
        let (owner, name) = match owner {
            Owner::Layer { root, path } => (
                TimelineOwner::Layer(LayerPath::new(*root, path.clone())),
                show.and_then(|show| root_layers(show, *root))
                    .and_then(|layers| layer_at(layers, path))
                    .and_then(|layer| layer.timelines.get(index))
                    .map(|tl| tl.name.clone()),
            ),
            Owner::Value(value) => (
                TimelineOwner::Value(value.clone()),
                show.and_then(|show| show.values.get(value))
                    .and_then(|value| value.timelines.get(index))
                    .map(|tl| tl.name.clone()),
            ),
        };
        TimelineRef {
            owner,
            index,
            name: name.unwrap_or_default(),
        }
    }

    /// Every source the property `property` of the layer at `layer`
    /// takes its value from now, strongest first: the running timelines
    /// with a track on it, then the held ones, then its bindings, then
    /// the document's base value. What the docs call precedence, answered
    /// for one property at one instant; the first with a value is the
    /// one that wins. Empty when the layer does not have the property.
    ///
    /// Timelines are listed for numeric properties only, since only
    /// those can be keyframed; a text, font, tint, video or sound
    /// property comes from its bindings and its base value.
    pub fn explain(&self, layer: &LayerPath, property: Property) -> Vec<Influence> {
        let Some(show) = &self.show else {
            return Vec::new();
        };
        let (root, path) = (layer.root, layer.indices.as_slice());
        let Some(layer) = root_layers(show, root).and_then(|layers| layer_at(layers, path)) else {
            return Vec::new();
        };
        let Some(base) = layer.base_value(property) else {
            return Vec::new();
        };
        // Built weakest first, in the order the value is resolved, and
        // turned round: whatever applies last wins.
        let mut sources = vec![Influence::Base { value: base }];
        for (index, b) in layer.bindings.iter().enumerate() {
            if b.property != property {
                continue;
            }
            let value = self.in_transition(root, path, index, b).or_else(|| {
                self.binding_value(&(root, path.to_vec(), index), b)
                    .and_then(|value| b.convert(value, show))
            });
            sources.push(Influence::Binding {
                index,
                variable: b.reading.variable.clone(),
                value,
            });
        }
        if property.is_numeric() {
            for running in [false, true] {
                for p in self.playing.iter().filter(|p| p.held != running) {
                    if !p.owner.is_layer(root, path) {
                        continue;
                    }
                    let Some(tl) = layer.timelines.get(p.timeline) else {
                        continue;
                    };
                    if !tl.tracks.iter().any(|t| t.property == property) {
                        continue;
                    }
                    let local = tl.local_time(p.at(self.time, tl.into()));
                    let passes = p.passes(self.time, tl.into());
                    let value = local.and_then(|time| {
                        tl.tracks
                            .iter()
                            .filter(|t| t.property == property)
                            .filter_map(|t| Some(t.sample(time)? + passes * t.per_pass()))
                            .next_back()
                    });
                    sources.push(Influence::Timeline {
                        timeline: self.timeline_ref(&p.owner, p.timeline),
                        local,
                        held: p.held,
                        value,
                    });
                }
            }
        }
        sources.reverse();
        sources
    }
}
