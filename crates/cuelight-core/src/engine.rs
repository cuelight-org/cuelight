//! The engine: the show's state and the clock that moves it.
//!
//! Everything here is a function of the show document, the inputs the
//! host gave and the instant asked for; nothing reads a pixel, a font or
//! a sample. What the layers should look like is answered as properties
//! ([`Engine::values`]); turning those into geometry is the `cuelight`
//! crate's job.

use crate::model::{
    parse_color, Binding, Choice, DigitDisplay, Justify, Layer, LayerKind, MediaKind, Output, Pass,
    Pick, Property, Reading, Retrigger, Scaling, Show, Timeline, Triggers, ValueTimeline, FORMAT,
};
use crate::value::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What the engine refuses: a show it cannot take, or a registration
/// that describes nothing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("no show loaded")]
    NoShow,
    #[error("invalid show: {0}")]
    InvalidShow(String),
    #[error("show format {found} is newer than this engine supports (up to {supported})")]
    UnsupportedFormat { found: u64, supported: u32 },
    #[error("invalid color literal {0:?}")]
    InvalidColor(String),
    #[error("invalid sound: {0}")]
    InvalidSound(String),
    #[error("invalid video: {0}")]
    InvalidVideo(String),
}

/// A binding with a transition: layer tree, layer path, binding index.
type TransitionSite = (Root, Vec<usize>, usize);

/// Where a variable is read: layer tree, layer path, and which of the
/// layer's readers.
type ReadSite = (Root, Vec<usize>, Reader);

/// One of the places on a layer that reads a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Reader {
    /// The binding at this index.
    Binding(usize),
    /// The `when` of the timeline at this index.
    When(usize),
    /// The `while` of the timeline at this index.
    While(usize),
    /// The `when` of the layer's playhead (a sound or a video).
    MediaWhen,
    /// The `while` of the layer's playhead.
    MediaWhile,
}

/// A debounced binding's input: the value that reached the property, and
/// the newer one waiting to have held long enough.
#[derive(Debug, Clone)]
struct Settling {
    settled: Value,
    candidate: Value,
    /// Engine time the candidate first appeared.
    since: f64,
}

/// A bound value on its way from `start` to `target` since engine time
/// `started`. The value in between is computed from this, never stepped,
/// so it does not depend on the frame rate.
#[derive(Debug, Clone, Copy)]
struct Change {
    start: f64,
    target: f64,
    started: f64,
    /// Whether this change runs between whole numbers, which is what
    /// decides if a counter shows whole numbers on the way.
    ///
    /// It has to be remembered rather than read off `start`, because a
    /// change that interrupts another starts from wherever the last one
    /// had got to, which is a fraction. What matters is the values the
    /// binding was given, not where the interruption happened to land.
    whole: bool,
}

/// A color on its way to another one. The same shape as a [`Change`], and
/// eased by the same transition: one progress from 0 to 1 carries all four
/// channels, so they arrive together however the ease is shaped.
#[derive(Debug, Clone, Copy)]
struct ColorChange {
    start: [u8; 4],
    target: [u8; 4],
    started: f64,
}

impl ColorChange {
    /// Where the color has reached, `progress` of the way along.
    fn value_at(&self, progress: f64) -> [u8; 4] {
        let mut out = [0u8; 4];
        for (i, channel) in out.iter_mut().enumerate() {
            let (from, to) = (f64::from(self.start[i]), f64::from(self.target[i]));
            *channel = (from + (to - from) * progress).round().clamp(0.0, 255.0) as u8;
        }
        out
    }
}

/// A color as a show writes one, so a transition's value is a value like
/// any other.
fn color_text([r, g, b, a]: [u8; 4]) -> String {
    match a {
        255 => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => format!("#{r:02X}{g:02X}{b:02X}{a:02X}"),
    }
}

/// Which layer tree a layer path is rooted in.
///
/// A layer is named by its tree and its path of child indices down it,
/// never by its name: names need not be unique, and a scene's layers
/// start over from the scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Root {
    /// The show's own, always present layers.
    Show,
    /// The layers of the scene at this index.
    Scene(usize),
}

/// Where a layer is in the document: which tree, and the index of each
/// step down it. Names need not be unique, so this is what tells two
/// layers apart, and what maps anything drawn or resolved back to its
/// place in the document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayerPath {
    pub root: Root,
    /// Each step an index into the children of the layer before it.
    pub indices: Vec<usize>,
}

impl LayerPath {
    pub fn new(root: Root, indices: impl Into<Vec<usize>>) -> Self {
        Self {
            root,
            indices: indices.into(),
        }
    }
}

/// One row of [`Engine::values`]: what one property of one layer
/// resolved to.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedValue {
    pub layer: LayerPath,
    /// The layer's name, for reading; two layers may share one.
    pub name: String,
    pub property: Property,
    pub value: Value,
}

/// What a playhead's timeline belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Owner {
    /// A layer's timeline, animating that layer's properties.
    Layer { root: Root, path: Vec<usize> },
    /// A show value's timeline, animating the value itself.
    Value(String),
}

impl Owner {
    /// Which tree it lives in. A value belongs to the show, so leaving a
    /// scene does not stop one.
    fn root(&self) -> Root {
        match self {
            Owner::Layer { root, .. } => *root,
            Owner::Value(_) => Root::Show,
        }
    }

    /// Whether it is the timeline of exactly this layer.
    fn is_layer(&self, root: Root, path: &[usize]) -> bool {
        matches!(self, Owner::Layer { root: r, path: p } if *r == root && p == path)
    }
}

/// A timeline's timing, whatever it animates: what the clock needs from
/// a layer's timeline and from a value's alike.
#[derive(Debug, Clone, Copy)]
struct Timing<'a> {
    duration: f64,
    play_time: f64,
    looping: bool,
    hold: bool,
    on_end: Option<&'a str>,
}

impl<'a> From<&'a Timeline> for Timing<'a> {
    fn from(tl: &'a Timeline) -> Self {
        Timing {
            duration: tl.duration(),
            play_time: tl.play_time(),
            looping: tl.looping,
            hold: tl.hold,
            on_end: tl.on_end.as_deref(),
        }
    }
}

impl<'a> From<&'a ValueTimeline> for Timing<'a> {
    fn from(tl: &'a ValueTimeline) -> Self {
        Timing {
            duration: tl.duration(),
            play_time: tl.play_time(),
            looping: tl.looping,
            hold: tl.hold,
            on_end: tl.on_end.as_deref(),
        }
    }
}

/// Which timelines a start is asking for.
#[derive(Debug, Clone, Copy)]
enum Want<'a> {
    /// Everything that starts when the show loads or a scene is entered.
    Autoplay,
    /// Everything this trigger starts.
    Trigger(&'a str),
}

impl Want<'_> {
    fn picks(self, autoplay: bool, trigger: &Triggers) -> bool {
        match self {
            Want::Autoplay => autoplay,
            Want::Trigger(name) => trigger.contains(name),
        }
    }
}

/// The timing of whatever `p` is playing, from the loaded show.
///
/// Free rather than a method so it can be called while the playheads are
/// borrowed.
fn timing_in<'a>(show: &'a Show, p: &Playhead) -> Option<Timing<'a>> {
    match &p.owner {
        Owner::Layer { root, path } => root_layers(show, *root)
            .and_then(|layers| layer_at(layers, path))
            .and_then(|layer| layer.timelines.get(p.timeline))
            .map(Timing::from),
        Owner::Value(name) => show
            .values
            .get(name)
            .and_then(|value| value.timelines.get(p.timeline))
            .map(Timing::from),
    }
}

/// A running timeline instance.
#[derive(Debug, Clone)]
struct Playhead {
    owner: Owner,
    /// Timeline index within its owner.
    timeline: usize,
    /// The instant on the show's clock the timeline's first key falls on:
    /// when it was started, plus its delay. A playhead keeps no clock of
    /// its own; where it is is worked out from this and the show's clock,
    /// so however long a chain of `on_end` runs for, the two can never
    /// drift apart.
    starts: f64,
    /// Finished, but holding its properties at their last values.
    held: bool,
}

impl Playhead {
    /// Seconds since the delay ended at show time `now`: negative while
    /// still delayed, and wrapped for a `loop`.
    fn at(&self, now: f64, tl: Timing<'_>) -> f64 {
        if self.held {
            return tl.play_time;
        }
        let mut elapsed = now - self.starts;
        // An anchor a hair ahead of the clock is one that has just
        // started, not one still waiting: the same instant, as in every
        // other comparison. Without this a link handed the property over
        // a rounding error early owns nothing for a frame, and the layer
        // falls back to its base value for it.
        if elapsed < 0.0 && elapsed > -SAME_INSTANT {
            elapsed = 0.0;
        }
        if elapsed > 0.0 && tl.looping && tl.duration > 0.0 {
            return elapsed % tl.duration;
        }
        elapsed
    }

    /// The instant it finishes, for a timeline that does finish.
    fn ends(&self, tl: Timing<'_>) -> f64 {
        self.starts + tl.play_time
    }
}

/// One play of an audio layer, from its trigger until it ends or is
/// stopped.
#[derive(Debug, Clone)]
struct Sounding {
    root: Root,
    layer_path: Vec<usize>,
    /// Engine-unique, so a backend can tell one play from the next.
    id: u64,
    /// Engine time it was triggered at; the delay counts from here.
    started: f64,
    /// What it is playing. A video layer's name can be bound, and
    /// pointing it at another clip starts that one from the top.
    playing: String,
}

/// Where a ducking layer's level is and when it started going there.
#[derive(Debug, Clone, Copy)]
struct Ducked {
    /// Whether the bus it listens to was sounding at the last step.
    down: bool,
    /// Engine time the level started moving toward where it is going.
    since: f64,
    /// The level it was at when it started moving, so a ramp interrupted
    /// halfway carries on from where it is rather than jumping.
    from: f64,
}

/// What the engine knows of a video: how long it runs and how big it is.
/// The frames are the host's business.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    /// Seconds, for looping, repeating and ending a play.
    pub duration: f64,
    /// The video's own size in pixels, what a layer without `size` draws
    /// at.
    pub width: f64,
    pub height: f64,
}

/// A video that should be showing now, as [`Engine::videos`] reports it:
/// the picture twin of a [`Voice`]. A host decodes to `position` and
/// hands the frame back to whatever draws the show, under `frame`.
#[derive(Debug, Clone, PartialEq)]
pub struct Playing {
    /// Identifies one play for as long as it lasts; never reused.
    pub id: u64,
    /// Name of the video layer.
    pub layer: String,
    /// The video, as registered with [`Engine::set_video`].
    pub video: String,
    /// The name to hand this play's picture over under (the `cuelight`
    /// crate's `set_image`), and the one the layer draws from.
    ///
    /// One per video layer, not per clip: two layers playing one clip at
    /// different positions each show their own frame, where a name they
    /// shared would leave both drawing whichever was written last. The
    /// engine makes it; a host only passes it back.
    pub frame: String,
    /// Seconds into the video, wrapped for loops and repeats.
    pub position: f64,
    /// Whether it plays on from its end.
    pub looping: bool,
}

/// A sound that should be heard now, as [`Engine::voices`] reports it: the
/// audio twin of a drawn layer. Plain data, so a backend can be fed and
/// tested without an engine.
#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    /// Identifies one play for as long as it lasts; never reused within an
    /// engine. A backend starts a sound when an id appears and stops it
    /// when the id is gone.
    pub id: u64,
    /// Name of the audio layer.
    pub layer: String,
    /// The sound, as registered with [`Engine::set_sound`].
    pub sound: String,
    /// Seconds into the sound, wrapped for loops and repeats. A backend
    /// starts a new voice here and resyncs one that has drifted away
    /// from it (a seek).
    pub position: f64,
    /// Effective loudness: the layer's gain times every group's above it.
    pub gain: f64,
    /// Whether the sound plays on from its end.
    pub looping: bool,
    /// The bus the layer names, if any.
    pub bus: Option<String>,
}

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

impl std::fmt::Display for LayerPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.root {
            Root::Show => write!(f, "show")?,
            Root::Scene(i) => write!(f, "scene {i}")?,
        }
        for step in &self.indices {
            write!(f, "/{step}")?;
        }
        Ok(())
    }
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

/// Something the show did that hosts may want to react to; collect them
/// with [`Engine::drain_events`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The show fired this trigger itself (a timeline's or a sound's
    /// `on_end`). Triggers the host fires are not echoed back.
    Trigger(String),
    /// A press landed on a layer that opens this web address. The host
    /// decides what to do with it: the players open it in the browser.
    Open { url: String },
}

/// Events kept while the host does not drain them; the oldest are dropped
/// beyond this so an uninterested host costs nothing.
const MAX_PENDING_EVENTS: usize = 256;

/// Trace records kept while the host does not drain them, the same way.
/// More than events: a show starts and ends a good many timelines.
const MAX_PENDING_TRACE: usize = 4096;

/// How many times one frame may be split at something ending.
///
/// A frame holds as many endings as the show puts in it, and each is
/// worth stopping at, so this has to be far above anything a show would
/// ask for: one long frame at a low frame rate can hold hundreds of
/// links of a chain, and capping it would make the frame rate part of
/// the answer, which is the one thing the clock must not depend on.
///
/// What it guards against is a show whose links are shorter than
/// [`SAME_INSTANT`], where a frame could be split until it ran out of
/// floats. Past this the rest of the frame is taken in one piece and the
/// chain carries on next frame, which is no longer exact; a show that
/// reaches it is asking for more than a hundred thousand endings inside
/// one frame.
const MAX_SUBSTEPS: usize = 100_000;

/// What is sounding on each bus, by the layer playing it and the instant
/// that play started sounding.
type BusyBuses = std::collections::BTreeMap<String, Vec<((Root, Vec<usize>), f64)>>;

/// Two instants closer together than this are the same instant.
///
/// Time is seconds in an `f64`, so the same instant reached two ways --
/// counting frames of a sixtieth, or adding up three clips of 0.7 s --
/// comes out a few parts in 10^15 apart. Without this a clip whose end
/// lands a femtosecond past a frame boundary waits a whole frame. A
/// microsecond is far under anything a show can ask for or a frame can
/// resolve, and far over that noise at any show length.
pub const SAME_INSTANT: f64 = 1e-6;

/// The engine: the loaded show and every piece of its runtime state.
///
/// Hosts drive it through four calls (`load_show`, `set_variable`,
/// `trigger`, `advance_frame`) and read back what every layer's
/// properties resolved to ([`values`](Engine::values)), what should be
/// heard ([`voices`](Engine::voices)) and shown
/// ([`videos`](Engine::videos)), and what the show fired
/// ([`drain_events`](Engine::drain_events)).
///
/// Sounds and videos are registered by what the clock needs of them,
/// their length and size; the samples and frames stay with the host.
/// Images and fonts are not registered here at all: they only matter
/// once a frame is drawn, which the `cuelight` crate does around this.
#[derive(Debug, Default)]
pub struct Engine {
    show: Option<Show>,
    variables: BTreeMap<String, Value>,
    /// Registered sounds and their durations in seconds.
    sounds: BTreeMap<String, f64>,
    /// Registered videos: what the engine knows without decoding them.
    videos: BTreeMap<String, VideoInfo>,
    playing: Vec<Playhead>,
    sounding: Vec<Sounding>,
    /// Id for the next play; ids are never reused.
    next_voice: u64,
    /// Every binding with a transition, found once at load.
    transition_sites: Vec<TransitionSite>,
    /// The change each of them is making; absent until its first frame,
    /// so a property starts at its value instead of easing in.
    transitions: HashMap<TransitionSite, Change>,
    /// The same, for the one property whose value is a color.
    color_transitions: HashMap<TransitionSite, ColorChange>,
    /// Values the show animates that a binding eases from, found once at
    /// load: a frame is cut where one of these changes, so a transition
    /// starts where its input moved rather than where the frame landed.
    eased_values: BTreeSet<String>,
    /// Every timeline condition that reads a value the show animates,
    /// with the tree it is in, found once at load: a frame is cut where
    /// one of these turns, for the same reason.
    value_conditions: Vec<(Root, Reading)>,
    /// Every reading with a debounce, binding or condition, found once
    /// at load, and what each has settled on.
    debounce_sites: Vec<ReadSite>,
    debounced: HashMap<ReadSite, Settling>,
    /// Where each ducking layer's level is: whether its bus was sounding
    /// at the last step and when that last changed, so a ramp knows where
    /// it started.
    ducking: HashMap<(Root, Vec<usize>), Ducked>,
    /// Which timeline conditions held last frame, so becoming true can be
    /// told from staying true.
    conditions: HashMap<(Root, Vec<usize>, usize), bool>,
    /// What each playhead's `when` or `while` last read as, by layer.
    media_conditions: HashMap<(Root, Vec<usize>), bool>,
    /// What each pointed layer last played, so pointing one somewhere new
    /// can be told from one that simply finished.
    shown: HashMap<(Root, Vec<usize>), String>,
    /// What each playhead has played so far: how many times, which is
    /// what decides which of several assets the next play takes, and when
    /// the last one started, for `rest`.
    plays: HashMap<(Root, Vec<usize>), Played>,
    /// Plays waiting their turn, oldest first; see
    /// [`Retrigger::Queue`](crate::model::Retrigger::Queue). Each holds
    /// the asset asked for, so a queue of different clips stays a queue
    /// of different clips.
    waiting: Vec<(Root, Vec<usize>, Option<String>, Cause)>,
    /// Scrambles the picks that are meant to vary; see
    /// [`Engine::set_seed`].
    seed: u64,
    /// Every reel row, found once at load, and where each of its cells is
    /// on its ring: one record per cell, in cell order.
    reel_sites: Vec<(Root, Vec<usize>)>,
    reels: HashMap<(Root, Vec<usize>), Vec<Change>>,
    /// Reel rows told to spin since the last frame; see
    /// [`Reel::spin`](crate::model::Reel::spin).
    spinning: Vec<(Root, Vec<usize>)>,
    active_scene: Option<usize>,
    events: std::collections::VecDeque<Event>,
    /// What happened inside the show, for a host that wants to see its
    /// working; see [`drain_trace`](Engine::drain_trace).
    trace: std::collections::VecDeque<Traced>,
    load_warnings: Vec<String>,
    time: f64,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a show from its JSON description, replacing any current show
    /// and resetting all runtime state. The first scene (if any) becomes
    /// active; autoplay timelines start at 0.
    ///
    /// A show that fails to load leaves the current one in place.
    pub fn load_show(&mut self, json: &str) -> Result<(), Error> {
        self.load_show_checked(json, |_| Ok(()))
    }

    /// [`load_show`](Engine::load_show) with one more check, run on the
    /// parsed show after the engine's own and before anything is
    /// replaced, so a show `check` refuses is not loaded.
    ///
    /// For what only a host can judge: this engine knows nothing of
    /// fonts, so whether a font style's size fits the font it names is
    /// checked by whoever registered the font.
    pub fn load_show_checked(
        &mut self,
        json: &str,
        check: impl FnOnce(&Show) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let raw = parse_document(json)?;
        let show = parse_show(&raw)?;
        if let Some(problem) = problems(&show).into_iter().next() {
            return Err(problem.error);
        }
        check(&show)?;
        self.install(&raw, show);
        Ok(())
    }

    /// Load as much of a show as can be loaded, and say what could not.
    ///
    /// [`load_show`](Engine::load_show) gives up at the first problem,
    /// which is right for anything shipping and useless while a show is
    /// being written: one mistake and there is nothing to look at. This
    /// keeps going. Whatever cannot be understood is dropped rather than
    /// guessed at, and each drop is a [`Finding`] that says where:
    ///
    /// - a layer that does not parse, or that [`load_show`](Engine::load_show)
    ///   would refuse, is left out of the show, with its children;
    /// - a font style or a show value that does not parse, or a font
    ///   style with a color that is not one, is left out, and so are the
    ///   layers that used it, each with a finding of its own;
    /// - a background or output tint that is not a color, or a pass with
    ///   nonsense in it, is dropped, and the default applies.
    ///
    /// What is loaded is exactly a show [`load_show`](Engine::load_show)
    /// accepts, so everything else the engine does holds for it.
    ///
    /// Fails only when there is no document at all: JSON that does not
    /// parse, a show without a name or a size, or a format newer than
    /// this engine reads. The findings come back in document order, and
    /// [`load_warnings`](Engine::load_warnings) are collected as always.
    /// A show that fails to load leaves the current one in place.
    pub fn load_show_tolerant(&mut self, json: &str) -> Result<Vec<Finding>, Error> {
        self.load_show_tolerant_checked(json, |_, _| {})
    }

    /// [`load_show_tolerant`](Engine::load_show_tolerant) with one more
    /// look at the loaded show, for what only a host can judge (see
    /// [`load_show_checked`](Engine::load_show_checked)). Whatever
    /// `check` finds is a finding beside the engine's; nothing is
    /// dropped for it, so a check that would have refused the show in
    /// strict loading has to be one the engine can live with.
    pub fn load_show_tolerant_checked(
        &mut self,
        json: &str,
        check: impl FnOnce(&Show, &mut Vec<Finding>),
    ) -> Result<Vec<Finding>, Error> {
        let Salvaged {
            mut raw,
            findings: mut found,
            blanked,
            ..
        } = salvaged(json)?;
        remove_blanks(&mut raw, blanked);
        let show = parse_show(&raw)?;
        debug_assert!(problems(&show).is_empty());
        check(&show, &mut found);
        self.install(&raw, show);
        Ok(found)
    }

    /// Take a show that passed every check, replacing the current one.
    fn install(&mut self, raw: &serde_json::Value, show: Show) {
        self.load_warnings.clear();
        if let Ok(understood) = serde_json::to_value(&show) {
            ignored_fields(raw, &understood, "", &mut self.load_warnings);
        }
        quiet_bindings(&show, true, &mut self.load_warnings);
        dark_segment_cells(&show, &mut self.load_warnings);
        self.eased_values = eased_values(&show);
        self.value_conditions = value_conditions(&show);
        (self.transition_sites, self.debounce_sites) = binding_sites(&show);
        self.reel_sites = reel_sites(&show);
        self.show = Some(show);
        self.restart();
    }

    /// Put the loaded show back to its beginning: time 0, the first
    /// scene, nothing playing, every variable at the value the document
    /// declares.
    ///
    /// Registered assets are host state and are left alone, so this is
    /// cheap: no file is read and nothing is decoded again. Reaching a
    /// moment is therefore restarting and advancing to it, which is what
    /// lets a host scrub without the engine holding any history.
    ///
    /// Does nothing without a show.
    pub fn restart(&mut self) {
        let Some(show) = &self.show else { return };
        self.variables = show.variables.clone();
        let scenes = !show.scenes.is_empty();
        self.playing.clear();
        self.sounding.clear();
        self.transitions.clear();
        self.color_transitions.clear();
        self.debounced.clear();
        self.reels.clear();
        self.spinning.clear();
        self.shown.clear();
        self.ducking.clear();
        self.conditions.clear();
        self.media_conditions.clear();
        self.plays.clear();
        self.waiting.clear();
        self.events.clear();
        self.trace.clear();
        self.time = 0.0;
        self.active_scene = scenes.then_some(0);
        self.start_matching(Some(Root::Show), self.time, Want::Autoplay, &Cause::Load);
        self.play_autoplay(Root::Show, &Cause::Load);
        if let Some(scene) = self.active_scene {
            let name = self.scene_name(scene);
            self.note(
                self.time,
                Happened::Entered {
                    scene: name,
                    by: Cause::Load,
                },
            );
            self.start_matching(
                Some(Root::Scene(scene)),
                self.time,
                Want::Autoplay,
                &Cause::Load,
            );
            self.play_autoplay(Root::Scene(scene), &Cause::Load);
        }
    }

    /// Fields of the loaded show document that the engine did not
    /// understand and ignored, as JSON paths (`layers[2].colour`): usually
    /// typos. Keys starting with `$` (like `$schema`) are never reported.
    pub fn load_warnings(&self) -> &[String] {
        &self.load_warnings
    }

    /// Say a press asked for `url` to be opened: reported through
    /// [`drain_events`](Engine::drain_events) as [`Event::Open`] and
    /// traced, for the host that owns the press to act on. Nothing in
    /// the show changes.
    pub fn open_link(&mut self, url: &str) {
        self.note(
            self.time,
            Happened::Opened {
                url: url.to_owned(),
            },
        );
        if self.events.len() == MAX_PENDING_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(Event::Open {
            url: url.to_owned(),
        });
    }

    /// Push a named value from the host. Unknown names are accepted:
    /// content may bind to them later.
    pub fn set_variable(&mut self, name: &str, value: impl Into<Value>) {
        let value = value.into();
        self.note(
            self.time,
            Happened::Set {
                name: name.to_owned(),
                value: value.clone(),
            },
        );
        self.variables.insert(name.to_owned(), value);
    }

    pub fn variable(&self, name: &str) -> Option<&Value> {
        self.variables.get(name)
    }

    /// What `name` reads as now: the host's variable of that name, or
    /// failing that the show's own value of it.
    ///
    /// A host that sets a variable takes the show's value over, so a show
    /// can ship with its own motion that a host is free to seize.
    pub fn value(&self, name: &str) -> Option<Value> {
        self.variables
            .get(name)
            .cloned()
            .or_else(|| self.show_value(name).map(Value::Number))
    }

    /// The number a show value stands at now, from whichever of its
    /// timelines is running: a held one only if nothing else is, as a
    /// layer's properties resolve.
    fn show_value(&self, name: &str) -> Option<f64> {
        self.show_value_at(name, self.time)
    }

    /// The number a show value stands at the instant `now`, from the
    /// timelines playing it as they are.
    fn show_value_at(&self, name: &str, now: f64) -> Option<f64> {
        let show = self.show.as_ref()?;
        let value = show.values.get(name)?;
        let mut out = None;
        for running in [false, true] {
            for p in self.playing.iter().filter(|p| p.held != running) {
                if !matches!(&p.owner, Owner::Value(v) if v == name) {
                    continue;
                }
                let Some(tl) = value.timelines.get(p.timeline) else {
                    continue;
                };
                if let Some(v) = tl.at(p.at(now, tl.into())) {
                    out = Some(v);
                }
            }
        }
        out
    }

    /// Register (or replace) a named sound by its `duration` in seconds,
    /// which is all the engine needs of it: to loop, repeat and end plays.
    /// The samples stay with the host's audio backend, which plays what
    /// [`voices`](Engine::voices) reports. Sounds are host assets that
    /// survive `load_show` and may arrive after it: an audio layer whose
    /// sound is not registered plays silently and does not end until it
    /// is.
    pub fn set_sound(&mut self, name: &str, duration: f64) -> Result<(), Error> {
        if !(duration.is_finite() && duration > 0.0) {
            return Err(Error::InvalidSound(format!(
                "{name:?}: duration {duration} is not above 0"
            )));
        }
        self.sounds.insert(name.to_owned(), duration);
        Ok(())
    }

    /// The duration registered for sound `name`, in seconds.
    pub fn sound_duration(&self, name: &str) -> Option<f64> {
        self.sounds.get(name).copied()
    }

    /// Register (or replace) a named video by its `duration` in seconds
    /// and its size in pixels, which is all the engine needs of it: to
    /// loop, repeat and end plays, and to lay its layer out. Decoding is
    /// the host's: it reads [`videos`](Engine::videos) each frame and
    /// hands the picture back to whatever draws the show under the
    /// play's [`frame`](Playing::frame) name. Like sounds, videos survive
    /// `load_show` and may arrive after it.
    pub fn set_video(
        &mut self,
        name: &str,
        duration: f64,
        [width, height]: [f64; 2],
    ) -> Result<(), Error> {
        let sane = |n: f64| n.is_finite() && n > 0.0;
        if !sane(duration) || !sane(width) || !sane(height) {
            return Err(Error::InvalidVideo(format!(
                "{name:?}: {duration}s at {width}x{height} is not above 0"
            )));
        }
        self.videos.insert(
            name.to_owned(),
            VideoInfo {
                duration,
                width,
                height,
            },
        );
        Ok(())
    }

    /// Scramble the picks that are meant to vary.
    ///
    /// A layer that names several assets with `pick: random` or
    /// `pick: shuffle` picks by counting its plays, not by rolling dice,
    /// so a show plays the same way every run. That is what rendering a
    /// show to a file needs, and it is the wrong thing for a show that
    /// runs all day: seed the engine with something that differs per run
    /// (the clock will do) and the same show varies between runs while
    /// staying repeatable within one.
    ///
    /// Set it before the show loads; it changes nothing already played.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed;
    }

    /// What is registered for video `name`.
    pub fn video(&self, name: &str) -> Option<VideoInfo> {
        self.videos.get(name).copied()
    }

    /// How long the content of a playhead runs, whichever registry it
    /// comes from; `None` while the host has not registered it.
    fn media_duration(&self, media: &crate::model::Media<'_>, playing: &str) -> Option<f64> {
        // What it is playing, which a binding or a pick may have chosen.
        let name = if playing.is_empty() {
            media.names.first()
        } else {
            playing
        };
        match media.kind {
            MediaKind::Sound => self.sounds.get(name).copied(),
            MediaKind::Video => self.videos.get(name).map(|video| video.duration),
        }
    }

    /// The videos that should be showing now, in tree order: every play of
    /// a visible video layer whose video is registered and whose delay is
    /// over, at its position. The picture twin of
    /// [`voices`](Engine::voices); a host decodes to these positions.
    pub fn videos(&self) -> Result<Vec<Playing>, Error> {
        let show = self.show.as_ref().ok_or(Error::NoShow)?;
        let mut out = Vec::new();
        for (root, layers) in std::iter::once((Root::Show, show.layers.as_slice())).chain(
            self.active_scene
                .and_then(|i| Some((Root::Scene(i), root_layers(show, Root::Scene(i))?))),
        ) {
            self.watch(root, layers, &mut Vec::new(), &mut out);
        }
        Ok(out)
    }

    fn watch(&self, root: Root, layers: &[Layer], path: &mut Vec<usize>, out: &mut Vec<Playing>) {
        for (i, layer) in layers.iter().enumerate() {
            path.push(i);
            if self.is_visible(root, layer, path) {
                if let LayerKind::Video { .. } = &layer.kind {
                    let media = layer.kind.media();
                    let plays = self
                        .sounding
                        .iter()
                        .filter(|s| s.root == root && s.layer_path == *path);
                    for play in plays {
                        let video = &play.playing;
                        let (Some(media), Some(info)) = (media, self.videos.get(video)) else {
                            continue;
                        };
                        let elapsed = self.time - play.started - media.delay.max(0.0);
                        if elapsed < 0.0 {
                            continue;
                        }
                        let position = if media.looping || media.repeat.is_some() {
                            elapsed % info.duration
                        } else {
                            elapsed
                        };
                        out.push(Playing {
                            id: play.id,
                            layer: layer.name.clone(),
                            video: video.clone(),
                            frame: frame_key(root, path),
                            position,
                            looping: media.looping,
                        });
                    }
                }
                self.watch(root, layer.children(), path, out);
            }
            path.pop();
        }
    }

    /// Press a key, by the name a browser gives it
    /// (`KeyboardEvent.key`): `ArrowRight`, `Enter`, `a`, `" "`.
    ///
    /// Fires what the show's `input.keys` says the key means and hands
    /// back that trigger's name, or `None` when the show says nothing
    /// about it. A key is a host event like any other: the show decides
    /// what it means, so a player, a browser and an embedder agree
    /// without any of them knowing what the show is about.
    pub fn key(&mut self, key: &str) -> Option<String> {
        let show = self.show.as_ref()?;
        let trigger = show.input.keys.get(key)?.clone();
        self.trigger(&trigger);
        Some(trigger)
    }

    /// Fire a named event. A scene declaring it as its trigger becomes the
    /// active scene (restarting it when already active); then every
    /// timeline declaring it, in the show's layers or the active scene,
    /// (re)starts from 0, and every audio layer declaring it plays (or
    /// stops, when it is the layer's `stop`).
    pub fn trigger(&mut self, name: &str) {
        self.fire(name, self.time, Firing::Host);
    }

    /// Fire `name` at `at`, noting who fired it.
    fn fire(&mut self, name: &str, at: f64, by: Firing) {
        self.note(
            at,
            Happened::Fired {
                name: name.to_owned(),
                by: by.clone(),
            },
        );
        let cause = Cause::Trigger {
            name: name.to_owned(),
            by,
        };
        self.trigger_at(name, at, &cause);
    }

    /// Start one timeline of the layer at `layer`, on its own: nothing
    /// else listening to its trigger starts, and an `autoplay`, `when` or
    /// `while` timeline can be run alone, which is how a host previews
    /// one. `false` when there is no such timeline.
    pub fn start_timeline(&mut self, layer: &LayerPath, timeline: usize) -> bool {
        let exists = self
            .show
            .as_ref()
            .and_then(|show| root_layers(show, layer.root))
            .and_then(|layers| layer_at(layers, &layer.indices))
            .is_some_and(|l| l.timelines.get(timeline).is_some());
        if !exists {
            return false;
        }
        self.begin_timeline(
            layer.root,
            layer.indices.clone(),
            timeline,
            self.time,
            Cause::Host,
        );
        true
    }

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

    fn note(&mut self, at: f64, what: Happened) {
        if self.trace.len() == MAX_PENDING_TRACE {
            self.trace.pop_front();
        }
        self.trace.push_back(Traced { at, what });
    }

    /// The name of scene `scene`, empty when there is none.
    fn scene_name(&self, scene: usize) -> String {
        self.show
            .as_ref()
            .and_then(|show| show.scenes.get(scene))
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    /// `index` of `owner`'s timelines, named for the trace.
    fn timeline_ref(&self, owner: &Owner, index: usize) -> TimelineRef {
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
                    let value = local.and_then(|time| {
                        tl.tracks
                            .iter()
                            .filter(|t| t.property == property)
                            .filter_map(|t| t.sample(time))
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

    /// Fire `name` as if at the instant `at`, which is what whatever it
    /// starts is timed from.
    ///
    /// For a trigger from the host that instant is the clock. For an
    /// `on_end` it is the instant the thing that fired it finished, which
    /// is not quite the clock when the frame had to land a hair short of
    /// it; anchoring there is what stops a chain's slack adding up.
    fn trigger_at(&mut self, name: &str, at: f64, cause: &Cause) {
        let entered = self
            .show
            .as_ref()
            .and_then(|show| show.scenes.iter().position(|s| s.trigger.contains(name)));
        if let Some(scene) = entered {
            self.enter_scene(scene, cause.clone());
        }
        self.start_matching(None, at, Want::Trigger(name), cause);
        self.set_spinning(name);
        let roots: Vec<Root> = std::iter::once(Root::Show)
            .chain(self.active_scene.map(Root::Scene))
            .collect();
        for root in roots {
            for (path, (stops, plays)) in self.media_layers(root, |trigger, stop| {
                (stop.contains(name), trigger.contains(name))
            }) {
                if stops {
                    let ending = Ending::Stop(name.to_owned());
                    self.end_plays(at, ending, |s| s.root == root && s.layer_path == path);
                    // Whatever was waiting its turn is not owed a turn.
                    self.waiting
                        .retain(|(r, p, ..)| !(*r == root && *p == path));
                }
                if plays {
                    self.play(root, path, at, cause.clone());
                }
            }
        }
    }

    /// The layers pointed at media they are not playing: idle ones whose
    /// bound name has changed since they last played.
    fn repointed(&self) -> Vec<(Root, Vec<usize>)> {
        let Some(show) = &self.show else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let roots = std::iter::once(Root::Show).chain(self.active_scene.map(Root::Scene));
        for root in roots {
            let Some(layers) = root_layers(show, root) else {
                continue;
            };
            let mut paths = Vec::new();
            fn walk(layers: &[Layer], path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
                for (i, layer) in layers.iter().enumerate() {
                    path.push(i);
                    if matches!(
                        layer.kind,
                        LayerKind::Video { .. } | LayerKind::Audio { .. }
                    ) {
                        out.push(path.clone());
                    }
                    walk(layer.children(), path, out);
                    path.pop();
                }
            }
            walk(layers, &mut Vec::new(), &mut paths);
            for path in paths {
                let idle = !self
                    .sounding
                    .iter()
                    .any(|play| play.root == root && play.layer_path == path);
                if !idle {
                    continue;
                }
                let Some(layer) = layer_at(layers, &path) else {
                    continue;
                };
                // Only a layer that is pointed somewhere: one playing
                // through a list of its own waits to be told to play.
                let Some(now) = self.pointed_at(root, layer, &path) else {
                    continue;
                };
                // The clip it last finished: it stays as it is until it
                // is pointed somewhere new. Never having played counts as
                // somewhere new, so the first name a host gives a surface
                // starts it like every name after.
                let shown = self.shown.get(&(root, path.clone()));
                if !now.is_empty() && shown.is_none_or(|last| *last != now) {
                    out.push((root, path));
                }
            }
        }
        out
    }

    /// The asset a new play of the layer at `path` would take: what the
    /// layer is pointed at, or the next of the several it names.
    fn media_name(&self, root: Root, path: &[usize]) -> String {
        let layer = self
            .show
            .as_ref()
            .and_then(|show| root_layers(show, root))
            .and_then(|layers| layer_at(layers, path));
        let Some(layer) = layer else {
            return String::new();
        };
        if let Some(pointed) = self.pointed_at(root, layer, path) {
            return pointed;
        }
        let Some(media) = layer.kind.media() else {
            return String::new();
        };
        let ordinal = self
            .plays
            .get(&(root, path.to_vec()))
            .map_or(0, |played| played.count);
        pick_one(
            media.names,
            media.pick,
            ordinal,
            self.seed ^ seed_of(root, path),
        )
    }

    /// What the playhead at `path` does when asked to play while it is
    /// already playing.
    fn retrigger_of(&self, root: Root, path: &[usize]) -> Retrigger {
        self.media_at(root, path)
            .map_or(Retrigger::Restart, |media| media.retrigger)
    }

    /// How many plays the playhead at `path` may hold at once.
    fn voices_of(&self, root: Root, path: &[usize]) -> usize {
        self.media_at(root, path)
            .map_or(1, |media| media.voices.max(1) as usize)
    }

    /// The playhead of the layer at `path`, if it has one.
    fn media_at(&self, root: Root, path: &[usize]) -> Option<crate::model::Media<'_>> {
        self.show
            .as_ref()
            .and_then(|show| root_layers(show, root))
            .and_then(|layers| layer_at(layers, path))
            .and_then(|layer| layer.kind.media())
    }

    /// Where the layer at `path` is pointed, by path alone.
    fn pointed(&self, root: Root, path: &[usize]) -> Option<String> {
        let layer = self
            .show
            .as_ref()
            .and_then(|show| root_layers(show, root))
            .and_then(|layers| layer_at(layers, path))?;
        self.pointed_at(root, layer, path)
    }

    /// What a binding has pointed this layer at, if one has.
    ///
    /// `None` covers two cases that have to stay apart: a layer with no
    /// video binding at all, which plays a list of its own, and one whose
    /// binding has nothing to say yet, because its variable is unset or
    /// its map does not list the value. Neither has been told what to
    /// show, and a layer that has not been told does not play. Falling
    /// back to the layer's own `video` here would make those look like an
    /// instruction to show it.
    fn pointed_at(&self, root: Root, layer: &Layer, path: &[usize]) -> Option<String> {
        let mut pointed = None;
        for (index, b) in layer.bindings.iter().enumerate() {
            // Whichever of the two names a playhead's media; a layer can
            // only carry the one its kind has.
            if !matches!(b.property, Property::Video | Property::Sound) {
                continue;
            }
            let bound = self.in_transition(root, path, index, b).or_else(|| {
                self.binding_value(&(root, path.to_vec(), index), b)
                    .and_then(|value| b.convert(value, self.show.as_ref()?))
            });
            if let Some(bound) = bound {
                pointed = Some(bound.to_text());
            }
        }
        pointed
    }

    /// The asset the layer at `path` is showing: what its running play
    /// took, or what a new play would take while nothing runs.
    pub fn showing(&self, root: Root, path: &[usize]) -> String {
        self.sounding
            .iter()
            .find(|s| s.root == root && s.layer_path == *path)
            .map_or_else(|| self.media_name(root, path), |s| s.playing.clone())
    }

    /// The layers of `root` with a playhead for which `want` (given their
    /// `trigger` and `stop`) says something: their paths, with what it
    /// said.
    fn media_layers<T>(
        &self,
        root: Root,
        want: impl Fn(&crate::model::Triggers, &crate::model::Triggers) -> T,
    ) -> Vec<(Vec<usize>, T)> {
        fn walk<T>(
            layers: &[Layer],
            path: &mut Vec<usize>,
            want: &impl Fn(&crate::model::Triggers, &crate::model::Triggers) -> T,
            out: &mut Vec<(Vec<usize>, T)>,
        ) {
            for (i, layer) in layers.iter().enumerate() {
                path.push(i);
                if let Some(media) = layer.kind.media() {
                    out.push((path.clone(), want(media.trigger, media.stop)));
                }
                walk(layer.children(), path, want, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        if let Some(layers) = self.show.as_ref().and_then(|show| root_layers(show, root)) {
            walk(layers, &mut Vec::new(), &want, &mut out);
        }
        out
    }

    /// Start the autoplay sounds and videos of `root`, `by` the load or
    /// the scene entered.
    fn play_autoplay(&mut self, root: Root, by: &Cause) {
        let Some(layers) = self.show.as_ref().and_then(|show| root_layers(show, root)) else {
            return;
        };
        let mut starts = Vec::new();
        fn walk(layers: &[Layer], path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            for (i, layer) in layers.iter().enumerate() {
                path.push(i);
                if layer.kind.media().is_some_and(|media| media.autoplay) {
                    out.push(path.clone());
                }
                walk(layer.children(), path, out);
                path.pop();
            }
        }
        walk(layers, &mut Vec::new(), &mut starts);
        for path in starts {
            self.play(root, path, self.time, by.clone());
        }
    }

    /// Play the audio layer at `path`, as its `retrigger` says when it
    /// already plays.
    fn play(&mut self, root: Root, path: Vec<usize>, at: f64, by: Cause) {
        self.start(root, path, None, at, by);
    }

    /// Start a play of the layer at `path`, of `asked` when the caller
    /// has already settled which asset it wants, `by` whatever asked.
    fn start(&mut self, root: Root, path: Vec<usize>, asked: Option<String>, at: f64, by: Cause) {
        let layer = self
            .show
            .as_ref()
            .and_then(|show| root_layers(show, root))
            .and_then(|layers| layer_at(layers, &path));
        let Some(media) = layer.and_then(|layer| layer.kind.media()) else {
            return;
        };
        let (retrigger, voices) = (media.retrigger, media.voices as usize);
        // Too soon after the last play: dropped, whatever the layer would
        // otherwise do with it.
        if media.rest > 0.0 {
            let last = self.plays.get(&(root, path.clone())).map(|p| p.at);
            if last.is_some_and(|last| at - last < media.rest) {
                return;
            }
        }
        let mine = |s: &Sounding| s.root == root && s.layer_path == path;
        match retrigger {
            Retrigger::Restart => self.end_plays(at, Ending::Retriggered, mine),
            Retrigger::Ignore if self.sounding.iter().any(mine) => return,
            Retrigger::Ignore => {}
            Retrigger::Queue if self.sounding.iter().any(mine) => {
                // In line behind what is playing, and behind whatever is
                // already waiting. Beyond the layer's `voices` the
                // trigger is dropped rather than piling up.
                let waiting = self
                    .waiting
                    .iter()
                    .filter(|(r, p, ..)| *r == root && *p == path)
                    .count();
                if waiting < voices.max(1) {
                    let asked = self.media_name(root, &path);
                    self.waiting.push((root, path, Some(asked), by));
                }
                return;
            }
            Retrigger::Queue => {}
            Retrigger::Overlap => {
                // Plays are in start order: the oldest of this layer's
                // stands first.
                let mut over = (self.sounding.iter().filter(|s| mine(s)).count() + 1)
                    .saturating_sub(voices.max(1));
                self.end_plays(at, Ending::Voices, |s| {
                    if over > 0 && mine(s) {
                        over -= 1;
                        return true;
                    }
                    false
                });
            }
        }
        self.next_voice += 1;
        let playing = asked.unwrap_or_else(|| self.media_name(root, &path));
        self.shown.insert((root, path.clone()), playing.clone());
        let played = self.plays.entry((root, path.clone())).or_default();
        played.count += 1;
        played.at = at;
        let (layer, name) = self.play_ref(root, &path);
        self.note(
            at,
            Happened::Played {
                layer,
                name,
                media: playing.clone(),
                id: self.next_voice,
                by,
            },
        );
        self.sounding.push(Sounding {
            root,
            layer_path: path,
            id: self.next_voice,
            started: at,
            playing,
        });
    }

    /// Where a play is, for the trace: the layer's path and its name.
    fn play_ref(&self, root: Root, path: &[usize]) -> (LayerPath, String) {
        let name = self
            .show
            .as_ref()
            .and_then(|show| root_layers(show, root))
            .and_then(|layers| layer_at(layers, path))
            .map(|layer| layer.name.clone())
            .unwrap_or_default();
        (LayerPath::new(root, path.to_vec()), name)
    }

    /// End every play `gone` picks, at the instant `at`, `by` whatever
    /// ended them, and say so in the trace.
    fn end_plays(&mut self, at: f64, by: Ending, mut gone: impl FnMut(&Sounding) -> bool) {
        let (ended, kept): (Vec<Sounding>, Vec<Sounding>) = std::mem::take(&mut self.sounding)
            .into_iter()
            .partition(|s| gone(s));
        self.sounding = kept;
        for s in ended {
            let (layer, name) = self.play_ref(s.root, &s.layer_path);
            self.note(
                at,
                Happened::Over {
                    layer,
                    name,
                    media: s.playing,
                    id: s.id,
                    by: by.clone(),
                },
            );
        }
    }

    /// The clip running on the video layer at `path`, if one is.
    ///
    /// Only a running play: a layer between clips shows nothing, so what
    /// is behind it shows through.
    pub fn playing_on(&self, root: Root, path: &[usize]) -> Option<&str> {
        self.sounding
            .iter()
            .find(|play| play.root == root && play.layer_path == *path)
            .map(|play| play.playing.as_str())
    }

    /// The layer trees showing now, in draw order: the show's own, then
    /// the active scene's.
    pub fn trees(&self) -> impl Iterator<Item = (Root, &[Layer])> {
        let show = self.show.as_ref();
        show.map(|show| (Root::Show, show.layers.as_slice()))
            .into_iter()
            .chain(show.and_then(|show| {
                let scene = self.active_scene?;
                Some((Root::Scene(scene), root_layers(show, Root::Scene(scene))?))
            }))
    }

    /// Name of the active scene, if the show has scenes.
    pub fn active_scene(&self) -> Option<&str> {
        let show = self.show.as_ref()?;
        Some(show.scenes.get(self.active_scene?)?.name.as_str())
    }

    /// The output in effect: what the active scene sets, over the show's.
    /// Its tints were checked at load.
    pub fn effective_output(&self) -> Output {
        let Some(show) = &self.show else {
            return Output::default();
        };
        let scene_output = self
            .active_scene
            .and_then(|i| show.scenes.get(i))
            .and_then(|s| s.output.as_ref());
        match scene_output {
            Some(output) => output.over(&show.output),
            None => show.output.clone(),
        }
    }

    /// Effects hosts apply to the current frame as they show it (the active
    /// scene's list, else the show's), in order.
    pub fn passes(&self) -> Vec<Pass> {
        self.effective_output().passes.unwrap_or_default()
    }

    /// How hosts should scale the current frame up to their surface (the
    /// active scene's setting over the show's).
    pub fn scaling(&self) -> Scaling {
        self.effective_output().scaling.unwrap_or_default()
    }

    /// Whether the frame is made on the canvas's own pixel grid: a gray
    /// output mode, or pixel-perfect scaling.
    ///
    /// What such a show draws is dots, and their colours are its
    /// palette, so nothing should soften an edge or average two of them
    /// into a colour that is not in it.
    pub fn pixel_grid(&self) -> bool {
        let output = self.effective_output();
        output.mode.unwrap_or_default() != crate::model::OutputMode::Rgb
            || output.scaling.unwrap_or_default() == Scaling::PixelPerfect
    }

    fn enter_scene(&mut self, scene: usize, by: Cause) {
        let name = self.scene_name(scene);
        self.note(
            self.time,
            Happened::Entered {
                scene: name.clone(),
                by,
            },
        );
        self.playing.retain(|p| p.owner.root() == Root::Show);
        // Leaving a scene stops its sounds.
        self.end_plays(self.time, Ending::SceneLeft, |s| s.root != Root::Show);
        self.waiting.retain(|(root, ..)| *root == Root::Show);
        // A scene's properties start at their values, like at load.
        self.transitions.retain(|(root, ..), _| *root == Root::Show);
        self.color_transitions
            .retain(|(root, ..), _| *root == Root::Show);
        self.debounced.retain(|(root, ..), _| *root == Root::Show);
        self.reels.retain(|(root, ..), _| *root == Root::Show);
        self.active_scene = Some(scene);
        self.start_matching(
            Some(Root::Scene(scene)),
            self.time,
            Want::Autoplay,
            &Cause::Entered(name.clone()),
        );
        self.play_autoplay(Root::Scene(scene), &Cause::Entered(name));
    }

    /// Advance time by `dt` seconds: running timelines and sounds
    /// progress, looping ones wrap, finished ones stop (their properties
    /// fall back to bindings/base values) and fire their `on_end` trigger,
    /// which is also reported through [`drain_events`](Engine::drain_events).
    ///
    /// A frame is not one step. Anything that ends inside it ends at the
    /// instant it ends, not at the end of the frame, so a chain of
    /// timelines linked by `on_end` keeps the schedule its durations
    /// describe whatever the frame rate is. The host still hears about
    /// the event when the frame returns; the show's own clocks are
    /// already right.
    pub fn advance_frame(&mut self, dt: f64) {
        self.advance_to(self.time + dt);
    }

    /// Advance to the instant `to`, which is where the clock lands.
    ///
    /// The same as [`advance_frame`](Engine::advance_frame) except in
    /// what it is told. A host that knows what time it is -- replaying a
    /// script, rendering chosen moments, seeking -- should say so: a
    /// delta has to be worked out from where the clock already is, and
    /// `previous + delta` is not the instant that was meant, so the same
    /// moment reached at two frame rates lands a rounding error apart.
    /// Told the instant, the clock is exactly it.
    ///
    /// Going backwards does nothing; a show is walked forwards from its
    /// start. Landing where the clock already is runs a step without
    /// moving it, which is how a host settles a show it has just poked.
    pub fn advance_to(&mut self, to: f64) {
        if to < self.time {
            return;
        }
        let end = to;
        // Each pass runs to the first thing that ends, so the pass after
        // it starts exactly where that one finished.
        for _ in 0..MAX_SUBSTEPS {
            let to = self.next_change(end);
            self.step(to);
            if self.time >= end {
                return;
            }
        }
        // A show whose chain is all zero-length timelines would split a
        // frame for ever; it gets the rest of the frame in one piece.
        self.step(end);
    }

    /// The instant something next changes, or `end` if nothing does
    /// before then.
    ///
    /// Every end is an instant on the show's clock, never a countdown:
    /// the same expression picks the instant here and recognises it in
    /// [`Self::step`], so a step that lands on it cannot land a float's
    /// width short and put the ending off to the next frame.
    ///
    /// Only what is already running counts: whatever an `on_end` starts is
    /// looked at on the next pass, having begun at the right instant.
    fn next_change(&self, end: f64) -> f64 {
        let Some(show) = &self.show else {
            return end;
        };
        let mut first = end;
        for p in &self.playing {
            if p.held {
                continue;
            }
            let Some(tl) = timing_in(show, p) else {
                continue;
            };
            if tl.looping || tl.duration <= 0.0 {
                continue;
            }
            let ends = p.ends(tl);
            if ends > self.time + SAME_INSTANT && ends < first {
                first = ends;
            }
        }
        for play in &self.sounding {
            let layer = root_layers(show, play.root).and_then(|l| layer_at(l, &play.layer_path));
            let Some(media) = layer.and_then(|l| l.kind.media()) else {
                continue;
            };
            if media.looping {
                continue;
            }
            let Some(length) = self.media_duration(&media, &play.playing) else {
                continue;
            };
            let plays = media.repeat.unwrap_or(1.0).max(0.0);
            let starts = play.started + media.delay.max(0.0);
            // When it starts sounding, as well as when it stops. A play
            // waiting out a delay is not on its bus yet, so anything
            // ducking under that bus turns at this instant; without it a
            // frame could span a short delayed clip from before it was
            // audible to after it was over, and nothing would duck.
            if starts > self.time + SAME_INSTANT && starts < first {
                first = starts;
            }
            let ends = starts + length * plays;
            if ends > self.time + SAME_INSTANT && ends < first {
                first = ends;
            }
        }
        // And where a value an eased binding reads changes. A value is
        // an input to the property that follows it, so a frame that
        // carried the change across its middle would start the
        // transition where the frame landed instead of where the value
        // moved, and the same show at the same instant would look
        // different for having been reached in longer steps.
        for p in &self.playing {
            let Owner::Value(name) = &p.owner else {
                continue;
            };
            if p.held || !self.eased_values.contains(name) {
                continue;
            }
            for at in self.key_instants(show, p) {
                if at < first {
                    first = at;
                }
            }
        }
        // And where a value a condition reads turns it. A condition is
        // an edge, and a value crosses its mark between keys, so the
        // instant is searched for: the truth at each key up to the
        // earliest change found so far, then bisection over the interval
        // where it differs. Between two keys with an ease that goes
        // straight there that is the one crossing; an ease that overshoots
        // can cross and come back inside one interval, and a turn shorter
        // than the interval is not seen.
        let showing =
            |root: Root| root == Root::Show || Some(root) == self.active_scene.map(Root::Scene);
        for (root, condition) in &self.value_conditions {
            // A host variable of the name takes the value over and changes
            // between frames, where the clock already is; a debounced
            // reading settles at a step of its own.
            if !showing(*root)
                || self.variables.contains_key(&condition.variable)
                || condition.debounce.is_some()
            {
                continue;
            }
            let Some(p) = self.playing.iter().find(|p| {
                !p.held && matches!(&p.owner, Owner::Value(v) if *v == condition.variable)
            }) else {
                continue;
            };
            let was = self.condition_at(condition, self.time);
            let mut lo = self.time;
            let mut marks: Vec<f64> = self
                .key_instants(show, p)
                .filter(|at| *at < first)
                .collect();
            marks.push(first);
            for mark in marks {
                if self.condition_at(condition, mark) == was {
                    lo = mark;
                    continue;
                }
                // Down to adjacent floats, not to the tolerance: the
                // flip then lands on the same instant whichever frame
                // the search started from, which is what keeps the
                // state the same at every rate.
                let mut hi = mark;
                loop {
                    let mid = lo + (hi - lo) / 2.0;
                    if !(lo < mid && mid < hi) {
                        break;
                    }
                    if self.condition_at(condition, mid) == was {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                if hi > self.time + SAME_INSTANT && hi < first {
                    first = hi;
                }
                break;
            }
        }
        first
    }

    /// The instants of the keys of the value timeline `p` plays that lie
    /// ahead of the clock: every key of the play under way and of the
    /// round after it, since a loop's first key comes round again.
    fn key_instants<'a>(
        &'a self,
        show: &'a Show,
        p: &'a Playhead,
    ) -> impl Iterator<Item = f64> + 'a {
        let keys = match &p.owner {
            Owner::Value(name) => show
                .values
                .get(name)
                .and_then(|value| value.timelines.get(p.timeline))
                .map(|tl| tl.keys.as_slice()),
            Owner::Layer { .. } => None,
        };
        let tl = timing_in(show, p);
        keys.zip(tl).into_iter().flat_map(move |(keys, tl)| {
            let round = tl.duration.max(0.0);
            let played = (self.time - p.starts).max(0.0);
            let rounds = match round > 0.0 && (tl.looping || tl.play_time > round) {
                true => [(played / round).floor(), (played / round).floor() + 1.0],
                false => [0.0, 0.0],
            };
            rounds.into_iter().flat_map(move |turn| {
                keys.iter().filter_map(move |key| {
                    let at = p.starts + turn * round + key.t;
                    (at > self.time + SAME_INSTANT && at <= p.ends(tl)).then_some(at)
                })
            })
        })
    }

    /// Whether `condition`, reading a value the show animates, holds at
    /// the instant `at`.
    fn condition_at(&self, condition: &Reading, at: f64) -> bool {
        self.show_value_at(&condition.variable, at)
            .and_then(|n| condition.mapped(Value::Number(n)))
            .is_some_and(|value| condition.bend(value.as_number()) != 0.0)
    }

    /// One indivisible move of the clock, landing exactly on `to`; see
    /// [`Self::advance_frame`].
    ///
    /// It is given where to land, never how far to go: everything inside
    /// is worked out from instants on the clock, so a step cannot leave
    /// anything a fraction of a frame away from where the show says it
    /// should be.
    fn step(&mut self, to: f64) {
        // Also before the step, not only after it: a play triggered
        // between two frames starts the bus at the instant the step
        // begins, and a long frame would otherwise see it begin and end
        // without ever noticing it sounded.
        self.follow_ducks();
        self.settle_debounces(to);
        self.follow_conditions();
        self.follow_transitions();
        self.follow_reels();
        // A layer pointed at another clip shows that one, from the top,
        // whether or not it was showing anything before: that is what an
        // event asking for a clip means.
        let pointed: Vec<(usize, String)> = self
            .sounding
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let now = self.pointed(s.root, &s.layer_path)?;
                (now != s.playing).then_some((i, now))
            })
            .collect();
        for (i, now) in pointed {
            // Being pointed somewhere new is a play like any other, so
            // the layer's `retrigger` says what happens to the one that
            // is running.
            let (root, path) = (self.sounding[i].root, self.sounding[i].layer_path.clone());
            match self.retrigger_of(root, &path) {
                Retrigger::Ignore => continue,
                Retrigger::Queue => {
                    // Pointing a layer somewhere is one ask, however many
                    // frames it stays pointed there, so it takes one place
                    // in the queue: the name is already last in line.
                    let mine: Vec<_> = self
                        .waiting
                        .iter()
                        .filter(|(r, p, ..)| *r == root && *p == path)
                        .collect();
                    let asked = mine
                        .last()
                        .is_some_and(|(_, _, name, _)| name.as_deref() == Some(now.as_str()));
                    let waiting = mine.len();
                    if !asked && waiting < self.voices_of(root, &path) {
                        self.waiting.push((root, path, Some(now), Cause::Pointed));
                    }
                }
                _ => {
                    let (layer, name) = self.play_ref(root, &path);
                    let (was, old_id) = (self.sounding[i].playing.clone(), self.sounding[i].id);
                    self.next_voice += 1;
                    let id = self.next_voice;
                    let play = &mut self.sounding[i];
                    play.playing = now.clone();
                    play.started = self.time;
                    play.id = id;
                    // Being pointed somewhere new in place is still a
                    // play of that clip: without this the layer would be
                    // told to start it again the moment it ended.
                    self.shown.insert((root, path), now.clone());
                    self.note(
                        self.time,
                        Happened::Over {
                            layer: layer.clone(),
                            name: name.clone(),
                            media: was,
                            id: old_id,
                            by: Ending::Pointed,
                        },
                    );
                    self.note(
                        self.time,
                        Happened::Played {
                            layer,
                            name,
                            media: now,
                            id,
                            by: Cause::Pointed,
                        },
                    );
                }
            }
        }
        for (root, path) in self.repointed() {
            self.play(root, path, self.time, Cause::Pointed);
        }
        self.time = to;
        let now = self.time;
        let Some(show) = &self.show else { return };
        let mut finished: Vec<usize> = Vec::new();
        // Each with the instant the thing that fired it finished, not
        // the clock: that is what the next link in a chain is timed from.
        let mut on_end: Vec<(String, f64, Firing)> = Vec::new();
        let mut over: Vec<(usize, TimelineRef, f64, bool)> = Vec::new();
        let mut noted: Vec<(f64, Happened)> = Vec::new();
        for (i, p) in self.playing.iter().enumerate() {
            let Some(tl) = timing_in(show, p) else {
                finished.push(i);
                continue;
            };
            // A held one is done moving: it stays where it stopped.
            if p.held {
                continue;
            }
            if now + SAME_INSTANT < p.starts {
                continue;
            }
            if tl.looping && tl.duration > 0.0 {
                continue;
            }
            // Compared as instants on the one clock, the same way the
            // step that landed here was chosen.
            if tl.duration <= 0.0 || now + SAME_INSTANT >= p.ends(tl) {
                let ends = if tl.duration <= 0.0 { now } else { p.ends(tl) };
                let timeline = self.timeline_ref(&p.owner, p.timeline);
                on_end
                    .extend(tl.on_end.map(|name| {
                        (name.to_owned(), ends, Firing::TimelineEnd(timeline.clone()))
                    }));
                // Holding is not playing: it ends, fires its `on_end` once
                // like any other, and then keeps its last values. A
                // timeline of a single key has no duration and ends on
                // the instant it starts, which is the shortest way to
                // write "set this and keep it": it holds like the rest.
                over.push((i, timeline, ends, tl.hold && !tl.looping));
            }
        }
        // Marked once the playheads are free again; noted once `show` is.
        for (i, timeline, ends, held) in over {
            if held {
                self.playing[i].held = true;
            } else {
                finished.push(i);
            }
            noted.push((ends, Happened::Ended { timeline, held }));
        }
        finished.sort_unstable();
        for i in finished.into_iter().rev() {
            self.playing.remove(i);
        }
        // A play ends when its time is up; a play of an unregistered sound
        // has no end yet.
        let time = self.time;
        let mut ended: Vec<(usize, f64)> = Vec::new();
        // The instant each layer fell idle, for whatever is queued behind
        // it: its turn starts where the play before it stopped.
        let mut freed: Vec<(Root, Vec<usize>, f64)> = Vec::new();
        for (i, s) in self.sounding.iter().enumerate() {
            let layer = root_layers(show, s.root).and_then(|l| layer_at(l, &s.layer_path));
            let Some(media) = layer.and_then(|layer| layer.kind.media()) else {
                ended.push((i, time));
                continue;
            };
            if media.looping {
                continue;
            }
            // Content the host has not registered yet has no end.
            let Some(duration) = self.media_duration(&media, &s.playing) else {
                continue;
            };
            let plays = media.repeat.unwrap_or(1.0).max(0.0);
            let ends = s.started + media.delay.max(0.0) + duration * plays;
            if time + SAME_INSTANT >= ends {
                ended.push((i, ends));
                freed.push((s.root, s.layer_path.clone(), ends));
                on_end.extend(media.on_end.map(|name| {
                    let layer = LayerPath::new(s.root, s.layer_path.clone());
                    (name.to_owned(), ends, Firing::PlayEnd(layer))
                }));
            }
        }
        // Noted in the order they started, which is how a log reads;
        // taken out from the back, so the indices hold.
        for (i, ends) in &ended {
            let s = &self.sounding[*i];
            let (layer, name) = self.play_ref(s.root, &s.layer_path);
            let on_end = self
                .media_at(s.root, &s.layer_path)
                .and_then(|m| m.on_end.map(str::to_owned));
            noted.push((
                *ends,
                Happened::Over {
                    layer,
                    name,
                    media: s.playing.clone(),
                    id: s.id,
                    by: Ending::Finished { on_end },
                },
            ));
        }
        for (i, _) in ended.into_iter().rev() {
            self.sounding.remove(i);
        }
        // Written before anything that follows from them: a play whose
        // turn came because another ended is traced after that end.
        for (at, what) in noted {
            self.note(at, what);
        }
        // A layer that has just fallen idle takes the next play waiting
        // for it, in the order the triggers arrived.
        let mut turn: Vec<(Root, Vec<usize>, Option<String>, Cause)> = Vec::new();
        self.waiting.retain(|(root, path, asked, by)| {
            let busy = self
                .sounding
                .iter()
                .any(|s| s.root == *root && s.layer_path == *path);
            let taken = turn.iter().any(|(r, p, ..)| r == root && p == path);
            if busy || taken {
                return true;
            }
            turn.push((*root, path.clone(), asked.clone(), by.clone()));
            false
        });
        for (root, path, asked, by) in turn {
            let at = freed
                .iter()
                .find(|(r, p, _)| *r == root && *p == path)
                .map_or(self.time, |(.., ends)| *ends);
            self.start(root, path, asked, at, by);
        }
        for (name, at, by) in on_end {
            self.fire(&name, at, by);
            if self.events.len() == MAX_PENDING_EVENTS {
                self.events.pop_front();
            }
            self.events.push_back(Event::Trigger(name));
        }
        self.follow_ducks();
    }

    /// Note, for every layer that ducks, whether the bus it listens to is
    /// sounding now, and when that last changed.
    ///
    /// Only the change is remembered. What the level *is* at any moment is
    /// a function of that and of the time, which is what keeps it seekable:
    /// nothing here accumulates frame by frame.
    fn follow_ducks(&mut self) {
        type Found = Vec<((Root, Vec<usize>), Option<f64>)>;
        let Some(show) = &self.show else { return };
        let busy = self.busy_buses();
        let mut found: Found = Vec::new();
        let roots = std::iter::once(Root::Show).chain(self.active_scene.map(Root::Scene));
        for root in roots {
            let Some(layers) = root_layers(show, root) else {
                continue;
            };
            type Busy = BusyBuses;
            fn walk(
                root: Root,
                layers: &[Layer],
                path: &mut Vec<usize>,
                busy: &Busy,
                out: &mut Vec<(Vec<usize>, Option<f64>)>,
            ) {
                for (i, layer) in layers.iter().enumerate() {
                    path.push(i);
                    if let LayerKind::Audio {
                        duck: Some(duck), ..
                    }
                    | LayerKind::Video {
                        duck: Some(duck), ..
                    } = &layer.kind
                    {
                        // Its own plays do not duck it, so a layer on the
                        // bus it listens to is not forever out of its own
                        // way. The instant the first of them started is
                        // when the bus became busy.
                        let since = busy.get(&duck.under).and_then(|plays| {
                            plays
                                .iter()
                                .filter(|((r, p), _)| !(*r == root && p == path))
                                .map(|(_, at)| *at)
                                .min_by(f64::total_cmp)
                        });
                        out.push((path.clone(), since));
                    }
                    walk(root, layer.children(), path, busy, out);
                    path.pop();
                }
            }
            let mut here = Vec::new();
            walk(root, layers, &mut Vec::new(), &busy, &mut here);
            found.extend(here.into_iter().map(|(path, since)| ((root, path), since)));
        }
        let (time, mut ducking) = (self.time, std::mem::take(&mut self.ducking));
        for (key, busy_since) in found {
            let down = busy_since.is_some();
            let was = ducking.get(&key).map(|d| d.down);
            if was != Some(down) {
                // When the bus started, not when this step noticed it: a
                // play that began part way through a frame started the
                // ramp then, so where the level is now does not depend on
                // where the frame happened to end. Going quiet is already
                // exact, because a step always lands on a play's end.
                let since = busy_since.unwrap_or(time).min(time);
                let from = self.duck_level_at(&key, since, &ducking);
                // Where it had got to, so turning round halfway carries
                // on from there instead of jumping.
                ducking.insert(key, Ducked { down, since, from });
            }
        }
        self.ducking = ducking;
    }

    /// What is sounding on each bus right now, by the layer playing it,
    /// so a layer can be left out of its own bus.
    fn busy_buses(&self) -> BusyBuses {
        let Some(show) = &self.show else {
            return std::collections::BTreeMap::new();
        };
        let mut busy: BusyBuses = std::collections::BTreeMap::new();
        for play in &self.sounding {
            let layer = root_layers(show, play.root).and_then(|l| layer_at(l, &play.layer_path));
            let bus = match layer.map(|l| &l.kind) {
                Some(LayerKind::Audio { bus, delay, .. } | LayerKind::Video { bus, delay, .. }) => {
                    // Still waiting out its delay: not sounding yet.
                    if self.time < play.started + delay.max(0.0) {
                        continue;
                    }
                    (bus, play.started + delay.max(0.0))
                }
                _ => continue,
            };
            let (bus, since) = bus;
            busy.entry(effective_bus(bus).to_owned())
                .or_default()
                .push(((play.root, play.layer_path.clone()), since));
        }
        busy
    }

    /// Where a ducking layer's level is: 1 when its bus is quiet, the
    /// duck's `to` while it sounds, and on the ramp between.
    fn duck_level_at(
        &self,
        key: &(Root, Vec<usize>),
        time: f64,
        ducking: &HashMap<(Root, Vec<usize>), Ducked>,
    ) -> f64 {
        let Some(state) = ducking.get(key) else {
            return 1.0;
        };
        let duck = self
            .show
            .as_ref()
            .and_then(|show| root_layers(show, key.0))
            .and_then(|layers| layer_at(layers, &key.1))
            .and_then(|layer| match &layer.kind {
                LayerKind::Audio { duck, .. } | LayerKind::Video { duck, .. } => duck.as_ref(),
                _ => None,
            });
        let Some(duck) = duck else { return 1.0 };
        let (target, ramp) = if state.down {
            (duck.to, duck.attack)
        } else {
            (1.0, duck.release)
        };
        if ramp <= 0.0 || !ramp.is_finite() {
            return target;
        }
        let t = ((time - state.since) / ramp).clamp(0.0, 1.0);
        state.from + (target - state.from) * t
    }

    /// The gain multiplier a ducking layer is at now.
    fn duck_of(&self, root: Root, path: &[usize]) -> f64 {
        self.duck_level_at(&(root, path.to_vec()), self.time, &self.ducking)
    }

    /// Take the events raised since the last call, oldest first. This is
    /// how content talks back to the host: a show can end a sequence with
    /// `on_end` and the host reacts (award points, switch hardware, ...).
    pub fn drain_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    /// How many timelines are running now: started, not yet ended, and
    /// not holding their last values. What a frame's resolve scales
    /// with, beside the layers and bindings it evaluates.
    pub fn timelines_running(&self) -> usize {
        self.playing.iter().filter(|p| !p.held).count()
    }

    /// Seconds advanced since the show loaded.
    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn show(&self) -> Option<&Show> {
        self.show.as_ref()
    }

    /// Every property every layer resolves to now, with no geometry
    /// built: the state the show is in at [`time`](Engine::time).
    ///
    /// Property precedence, strongest first: running timeline, binding,
    /// base value from the show description.
    ///
    /// One level below a draw list, which turns this into shapes, paths
    /// and transforms for a renderer. That step costs around ninety times
    /// what the clock does and discards nothing the timing model
    /// produced, so this is what to compare two moments by, and what a
    /// test should assert on.
    ///
    /// Layers come in draw order, each with its path and name, and only
    /// the properties that layer actually has.
    pub fn values(&self) -> Result<Vec<ResolvedValue>, Error> {
        const EVERY: [Property; 16] = [
            Property::X,
            Property::Y,
            Property::Opacity,
            Property::Scale,
            Property::ScaleX,
            Property::ScaleY,
            Property::Rotation,
            Property::Text,
            Property::Font,
            Property::Reveal,
            Property::Video,
            Property::Sound,
            Property::Frame,
            Property::Gain,
            Property::Visible,
            Property::Tint,
        ];
        fn walk(
            engine: &Engine,
            root: Root,
            layers: &[Layer],
            path: &mut Vec<usize>,
            out: &mut Vec<ResolvedValue>,
        ) {
            for (i, layer) in layers.iter().enumerate() {
                path.push(i);
                for property in EVERY {
                    if let Some(value) = engine.resolve(root, layer, path, property) {
                        out.push(ResolvedValue {
                            layer: LayerPath::new(root, path.clone()),
                            name: layer.name.clone(),
                            property,
                            value,
                        });
                    }
                }
                walk(engine, root, layer.children(), path, out);
                path.pop();
            }
        }
        let show = self.show.as_ref().ok_or(Error::NoShow)?;
        let mut out = Vec::new();
        walk(self, Root::Show, &show.layers, &mut Vec::new(), &mut out);
        if let Some(scene) = self.active_scene {
            if let Some(layers) = root_layers(show, Root::Scene(scene)) {
                walk(self, Root::Scene(scene), layers, &mut Vec::new(), &mut out);
            }
        }
        Ok(out)
    }

    /// The sounds that should be heard now, in tree order: every play of a
    /// visible audio layer whose sound is registered and whose delay is
    /// over, at its position and effective gain. The audio twin of a draw
    /// list: a backend diffs it frame by frame (start what is new, stop
    /// what is gone, ramp gains, resync a position that jumped, but not
    /// one merely running behind) and hosts that mix themselves read the
    /// same list.
    pub fn voices(&self) -> Result<Vec<Voice>, Error> {
        let show = self.show.as_ref().ok_or(Error::NoShow)?;
        let mut out = Vec::new();
        self.hear(Root::Show, &show.layers, &mut Vec::new(), 1.0, &mut out);
        if let Some(scene) = self.active_scene {
            if let Some(layers) = root_layers(show, Root::Scene(scene)) {
                self.hear(Root::Scene(scene), layers, &mut Vec::new(), 1.0, &mut out);
            }
        }
        Ok(out)
    }

    fn hear(
        &self,
        root: Root,
        layers: &[Layer],
        path: &mut Vec<usize>,
        chain: f64,
        out: &mut Vec<Voice>,
    ) {
        for (i, layer) in layers.iter().enumerate() {
            path.push(i);
            if self.is_visible(root, layer, path) {
                match &layer.kind {
                    LayerKind::Group { children, .. } => {
                        let gain = chain * self.number(root, layer, path, Property::Gain).max(0.0);
                        self.hear(root, children, path, gain, out);
                    }
                    LayerKind::Audio {
                        looping,
                        delay,
                        repeat,
                        bus,
                        ..
                    } => {
                        // The duck multiplies like every other gain, so it
                        // composes with bindings and the tree above.
                        let gain = chain
                            * self.number(root, layer, path, Property::Gain).max(0.0)
                            * self.duck_of(root, path);
                        let plays = self
                            .sounding
                            .iter()
                            .filter(|s| s.root == root && s.layer_path == *path);
                        for play in plays {
                            let sound = &play.playing;
                            let Some(duration) = self.sounds.get(sound) else {
                                continue;
                            };
                            let elapsed = self.time - play.started - delay.max(0.0);
                            if elapsed < 0.0 {
                                continue;
                            }
                            let position = if *looping || repeat.is_some() {
                                elapsed % duration
                            } else {
                                elapsed
                            };
                            out.push(Voice {
                                id: play.id,
                                layer: layer.name.clone(),
                                sound: sound.clone(),
                                position,
                                gain,
                                looping: *looping,
                                bus: Some(effective_bus(bus).to_owned()),
                            });
                        }
                    }
                    // A clip is heard when the host has registered a sound
                    // under the video's name: that is how it says this clip
                    // has a soundtrack and hands over its samples. The
                    // picture's own duration governs the position, so the
                    // two stay together through loops and repeats, and the
                    // play's id is the one `videos` reports, so a host can
                    // see that the sound and the picture are one play.
                    LayerKind::Video { bus, .. } => {
                        let Some(media) = layer.kind.media() else {
                            path.pop();
                            continue;
                        };
                        let gain = chain * self.number(root, layer, path, Property::Gain).max(0.0);
                        let plays = self
                            .sounding
                            .iter()
                            .filter(|s| s.root == root && s.layer_path == *path);
                        for play in plays {
                            let clip = &play.playing;
                            let Some(info) = self
                                .sounds
                                .get(clip)
                                .and(self.videos.get(clip))
                                .filter(|info| info.duration > 0.0)
                            else {
                                continue;
                            };
                            let elapsed = self.time - play.started - media.delay.max(0.0);
                            if elapsed < 0.0 {
                                continue;
                            }
                            let position = if media.looping || media.repeat.is_some() {
                                elapsed % info.duration
                            } else {
                                elapsed
                            };
                            out.push(Voice {
                                id: play.id,
                                layer: layer.name.clone(),
                                sound: clip.clone(),
                                position,
                                gain,
                                looping: media.looping,
                                bus: Some(effective_bus(bus).to_owned()),
                            });
                        }
                    }
                    _ => {}
                }
            }
            path.pop();
        }
    }

    /// Start every timeline whose `when` has just become true.
    ///
    /// The edge is what starts it, not the condition holding: a lamp that
    /// stays on plays its animation once. A condition that is already
    /// true when the show loads or a scene is entered counts as an edge,
    /// the same way a host firing a trigger at that moment would.
    fn follow_conditions(&mut self) {
        let Some(show) = &self.show else { return };
        // Every tree, not only the showing one: a condition in a scene
        // that is away still has to notice its variable falling, or an
        // edge that happens while it is away is invisible on return.
        let showing =
            |root: Root| root == Root::Show || Some(root) == self.active_scene.map(Root::Scene);
        let roots: Vec<Root> = std::iter::once(Root::Show)
            .chain((0..show.scenes.len()).map(Root::Scene))
            .collect();
        let mut edges: Vec<(Root, Vec<usize>, usize, Cause)> = Vec::new();
        let mut stops: Vec<(Owner, usize)> = Vec::new();
        let mut now: HashMap<(Root, Vec<usize>, usize), bool> = HashMap::new();
        let mut turned: Vec<(Owner, usize, Which, bool)> = Vec::new();
        for root in roots {
            let Some(layers) = root_layers(show, root) else {
                continue;
            };
            /// A timeline with a condition: where it is, and what its
            /// `when` and `while` read as now.
            type Conditioned = (Vec<usize>, usize, Option<bool>, Option<bool>);
            let mut found: Vec<Conditioned> = Vec::new();
            collect_timelines(layers, &mut Vec::new(), &mut |path, idx, tl| {
                let holds = |reader, condition: &Option<Reading>| {
                    let condition = condition.as_ref()?;
                    Some(self.holds(&(root, path.to_vec(), reader), condition))
                };
                let when = holds(Reader::When(idx), &tl.when);
                let whilst = holds(Reader::While(idx), &tl.whilst);
                if when.is_some() || whilst.is_some() {
                    found.push((path.to_vec(), idx, when, whilst));
                }
            });
            for (path, idx, when, whilst) in found {
                let key = (root, path.clone(), idx);
                if let Some(holds) = when {
                    if !showing(root) {
                        // Away: only the fall is worth remembering. Not
                        // recording the rise is what makes it an edge on
                        // return, since the value it is compared against
                        // is then the false it fell to.
                        if !holds {
                            now.insert(key, false);
                        }
                        continue;
                    }
                    // The rising edge, and only that: a condition that
                    // was already true stays quiet.
                    let was = self.conditions.get(&key).copied();
                    if holds && was != Some(true) {
                        edges.push((root, path.clone(), idx, Cause::When));
                    }
                    // A first look that reads false is nothing turning.
                    if was != Some(holds) && (was.is_some() || holds) {
                        let owner = Owner::Layer { root, path };
                        turned.push((owner, idx, Which::When, holds));
                    }
                    now.insert(key, holds);
                } else if let Some(holds) = whilst {
                    if !showing(root) {
                        // A `while` is a state the scene is in, so it has
                        // nothing to remember: entering starts it again.
                        continue;
                    }
                    // No edge: it runs while it holds. Entering a scene
                    // empties the playheads, so this starts it again.
                    let running = self
                        .playing
                        .iter()
                        .any(|p| p.owner.is_layer(root, &path) && p.timeline == idx);
                    match (holds, running) {
                        (true, false) => {
                            turned.push((
                                Owner::Layer {
                                    root,
                                    path: path.clone(),
                                },
                                idx,
                                Which::While,
                                true,
                            ));
                            edges.push((root, path, idx, Cause::While));
                        }
                        (false, true) => {
                            turned.push((
                                Owner::Layer {
                                    root,
                                    path: path.clone(),
                                },
                                idx,
                                Which::While,
                                false,
                            ));
                            stops.push((Owner::Layer { root, path }, idx));
                        }
                        _ => {}
                    }
                }
            }
        }
        // Merged, not replaced: a scene that is not showing keeps what
        // its conditions last read, so coming back to it is not an edge
        // unless the variable turned true while it was away.
        self.conditions.extend(now);
        for (owner, timeline, condition, holds) in turned {
            let timeline = self.timeline_ref(&owner, timeline);
            self.note(
                self.time,
                Happened::Turned {
                    timeline,
                    condition,
                    holds,
                },
            );
        }
        for (owner, timeline) in stops {
            let stopped = self.timeline_ref(&owner, timeline);
            self.playing
                .retain(|p| !(p.owner == owner && p.timeline == timeline));
            self.note(self.time, Happened::Stopped { timeline: stopped });
        }
        for (root, path, timeline, cause) in edges {
            self.begin_timeline(root, path, timeline, self.time, cause);
        }
        self.follow_media_conditions();
    }

    /// Play every sound and video whose `when` has just become true, or
    /// whose `while` has, and stop those whose `while` has just become
    /// false; the same edges a timeline's conditions are, read the same
    /// way, with one playhead per layer where a layer has many
    /// timelines.
    ///
    /// A `while` starts a play on turning true and stops it on turning
    /// false: nothing in between. A one-shot that ends on its own while
    /// the condition still holds is not started again, so a state does
    /// not become a buzz; a loop plays for as long as the state does.
    /// Stopping is not finishing and fires no `on_end`.
    fn follow_media_conditions(&mut self) {
        let Some(show) = &self.show else { return };
        let showing =
            |root: Root| root == Root::Show || Some(root) == self.active_scene.map(Root::Scene);
        let roots: Vec<Root> = std::iter::once(Root::Show)
            .chain((0..show.scenes.len()).map(Root::Scene))
            .collect();
        let mut plays: Vec<(Root, Vec<usize>, Cause)> = Vec::new();
        let mut stops: Vec<(Root, Vec<usize>)> = Vec::new();
        let mut now: HashMap<(Root, Vec<usize>), bool> = HashMap::new();
        let mut forgotten: Vec<(Root, Vec<usize>)> = Vec::new();
        for root in roots {
            let Some(layers) = root_layers(show, root) else {
                continue;
            };
            /// A playhead with a condition: where it is, and what its
            /// `when` and `while` read as now.
            type Conditioned = (Vec<usize>, Option<bool>, Option<bool>);
            let mut found: Vec<Conditioned> = Vec::new();
            fn walk(
                engine: &Engine,
                root: Root,
                layers: &[Layer],
                path: &mut Vec<usize>,
                found: &mut Vec<Conditioned>,
            ) {
                for (i, layer) in layers.iter().enumerate() {
                    path.push(i);
                    if let Some(media) = layer.kind.media() {
                        let holds = |reader, condition: Option<&Reading>| {
                            let condition = condition?;
                            Some(engine.holds(&(root, path.clone(), reader), condition))
                        };
                        let when = holds(Reader::MediaWhen, media.when);
                        let whilst = holds(Reader::MediaWhile, media.whilst);
                        if when.is_some() || whilst.is_some() {
                            found.push((path.clone(), when, whilst));
                        }
                    }
                    walk(engine, root, layer.children(), path, found);
                    path.pop();
                }
            }
            walk(self, root, layers, &mut Vec::new(), &mut found);
            for (path, when, whilst) in found {
                let key = (root, path.clone());
                let was = self.media_conditions.get(&key).copied();
                if let Some(holds) = when {
                    // As for a timeline's `when`: away, only the fall is
                    // remembered, so a rise while away is an edge on
                    // return.
                    if !showing(root) {
                        if !holds {
                            now.insert(key, false);
                        }
                        continue;
                    }
                    if holds && was != Some(true) {
                        plays.push((root, path, Cause::When));
                    }
                    now.insert(key, holds);
                } else if let Some(holds) = whilst {
                    // A state the scene is in: away it is forgotten, so
                    // entering the scene starts it again if it holds.
                    if !showing(root) {
                        forgotten.push(key);
                        continue;
                    }
                    match (holds, was) {
                        (true, Some(true)) | (false, None) | (false, Some(false)) => {}
                        (true, _) => plays.push((root, path, Cause::While)),
                        (false, Some(true)) => stops.push((root, path)),
                    }
                    now.insert(key, holds);
                }
            }
        }
        for key in forgotten {
            self.media_conditions.remove(&key);
        }
        self.media_conditions.extend(now);
        for (root, path) in stops {
            self.end_plays(self.time, Ending::While, |s| {
                s.root == root && s.layer_path == path
            });
            self.waiting
                .retain(|(r, p, ..)| !(*r == root && *p == path));
        }
        for (root, path, by) in plays {
            self.play(root, path, self.time, by);
        }
    }

    /// Whether the condition at `site` reads as true right now: what it
    /// reads, bent through its threshold or curve, is not 0. A reading
    /// with nothing to say is false.
    fn holds(&self, site: &ReadSite, condition: &Reading) -> bool {
        match self.read(site, condition) {
            Some(value) => condition.bend(value.as_number()) != 0.0,
            None => false,
        }
    }

    /// (Re)start the timelines `want` selects, in `root` or, with `None`,
    /// in the show's layers and the active scene.
    ///
    /// A show value's timelines are selected the same way and by the same
    /// call, since a trigger means the same thing to both. Values belong
    /// to the show, so they are left alone when only a scene is asked for.
    fn start_matching(&mut self, root: Option<Root>, at: f64, want: Want<'_>, cause: &Cause) {
        let Some(show) = &self.show else { return };
        let roots = match root {
            Some(root) => vec![root],
            None => std::iter::once(Root::Show)
                .chain(self.active_scene.map(Root::Scene))
                .collect(),
        };
        let mut starts: Vec<(Owner, usize, f64)> = Vec::new();
        for root in roots {
            let Some(layers) = root_layers(show, root) else {
                continue;
            };
            collect_timelines(layers, &mut Vec::new(), &mut |path, idx, tl| {
                if want.picks(tl.autoplay, &tl.trigger) {
                    let owner = Owner::Layer {
                        root,
                        path: path.to_vec(),
                    };
                    starts.push((owner, idx, tl.delay.max(0.0)));
                }
            });
        }
        if root.is_none_or(|root| root == Root::Show) {
            for (name, value) in &show.values {
                for (idx, tl) in value.timelines.iter().enumerate() {
                    if want.picks(tl.autoplay, &tl.trigger) {
                        starts.push((Owner::Value(name.clone()), idx, tl.delay.max(0.0)));
                    }
                }
            }
        }
        for (owner, timeline, delay) in starts {
            let started = self.timeline_ref(&owner, timeline);
            self.playing
                .retain(|p| !(p.owner == owner && p.timeline == timeline));
            self.playing.push(Playhead {
                owner,
                timeline,
                starts: at + delay,
                held: false,
            });
            self.note(
                at,
                Happened::Started {
                    timeline: started,
                    by: cause.clone(),
                },
            );
        }
    }

    /// (Re)start one timeline from the top, minding its delay.
    /// Start the timeline at `path`, as if at the instant `at`.
    ///
    /// `at` is when it should have started, not when this was noticed, so
    /// a timeline a condition starts is timed from the condition turning
    /// true.
    fn begin_timeline(
        &mut self,
        root: Root,
        layer_path: Vec<usize>,
        timeline: usize,
        at: f64,
        cause: Cause,
    ) {
        let Some(show) = &self.show else { return };
        let delay = root_layers(show, root)
            .and_then(|layers| layer_at(layers, &layer_path))
            .and_then(|l| l.timelines.get(timeline))
            .map_or(0.0, |tl| tl.delay.max(0.0));
        let owner = Owner::Layer {
            root,
            path: layer_path,
        };
        let started = self.timeline_ref(&owner, timeline);
        self.playing
            .retain(|p| !(p.owner == owner && p.timeline == timeline));
        self.playing.push(Playhead {
            owner,
            timeline,
            starts: at + delay,
            held: false,
        });
        self.note(
            at,
            Happened::Started {
                timeline: started,
                by: cause,
            },
        );
    }

    /// Let every reading with a debounce take in its variable: a new value
    /// becomes the candidate, and a candidate that will have held for the
    /// debounce time by `to`, where this step lands, settles, so it shows
    /// in the frame the hold runs out. A first look settles at once.
    fn settle_debounces(&mut self, to: f64) {
        let Some(show) = &self.show else { return };
        let mut debounced = std::mem::take(&mut self.debounced);
        for site in &self.debounce_sites {
            let (root, ..) = site;
            if *root != Root::Show && Some(*root) != self.active_scene.map(Root::Scene) {
                continue;
            }
            let reading = reading_at(show, site);
            let Some((reading, hold)) = reading.and_then(|r| Some((r, r.debounce?))) else {
                continue;
            };
            let Some(value) = self.value(&reading.variable) else {
                debounced.remove(site);
                continue;
            };
            let settling = debounced.entry(site.clone()).or_insert_with(|| Settling {
                settled: value.clone(),
                candidate: value.clone(),
                since: self.time,
            });
            if settling.candidate != value {
                settling.candidate = value.clone();
                settling.since = self.time;
            }
            if settling.settled != settling.candidate && to + SAME_INSTANT - settling.since >= hold
            {
                settling.settled = settling.candidate.clone();
            }
        }
        self.debounced = debounced;
    }

    /// Note which character each reel cell is heading for, as of now. A
    /// cell that is already there is left alone, so a change moves only
    /// the cells it reaches, each from wherever it stands.
    fn follow_reels(&mut self) {
        let Some(show) = &self.show else { return };
        let mut reels = std::mem::take(&mut self.reels);
        let spinning = std::mem::take(&mut self.spinning);
        for site in &self.reel_sites {
            let (root, path) = site;
            if *root != Root::Show && Some(*root) != self.active_scene.map(Root::Scene) {
                continue;
            }
            let layer = root_layers(show, *root).and_then(|layers| layer_at(layers, path));
            let Some(layer) = layer else { continue };
            let LayerKind::Digits {
                digits,
                justify,
                display: DigitDisplay::Reel(reel),
                ..
            } = &layer.kind
            else {
                continue;
            };
            // Told to spin: every cell sets off, including one already
            // standing where the row is about to land.
            let spin = spinning.contains(site);
            let count = *digits as usize;
            let ring = reel.characters();
            let roll = reel.roll();
            let text = self.text(*root, layer, path, Property::Text);
            let wanted: Vec<Option<f64>> = row_cells(&text, count, *justify)
                .into_iter()
                .map(|c| {
                    let c = c?;
                    ring.iter().position(|on| *on == c).map(|i| i as f64)
                })
                .collect();
            let records = reels.entry(site.clone()).or_insert_with(|| {
                // At load a row stands at what it shows: nothing rolls in.
                wanted
                    .iter()
                    .map(|target| {
                        let target = target.unwrap_or(0.0);
                        Change {
                            start: target,
                            target,
                            started: self.time,
                            whole: false,
                        }
                    })
                    .collect()
            });
            records.resize(
                count,
                Change {
                    start: 0.0,
                    target: 0.0,
                    started: self.time,
                    whole: false,
                },
            );
            let ring_length = reel.ring();
            for (i, character) in wanted.into_iter().enumerate() {
                let Some(character) = character else { continue };
                let change = &mut records[i];
                // Where it is heading, as a place on the ring: a cell that
                // is already going there carries on, unless the row was
                // told to spin.
                if !spin && change.target.rem_euclid(ring_length) == character {
                    continue;
                }
                let reached =
                    roll.value_at(change.start, change.target, self.time - change.started);
                // The cell on the right moves first; the rest follow.
                let delay = (count - 1 - i) as f64 * reel.stagger.max(0.0);
                *change = Change {
                    start: reached,
                    target: reel.travel(reached, character),
                    started: self.time + delay,
                    whole: false,
                };
            }
        }
        self.reels = reels;
    }

    /// Note the reel rows whose `spin` trigger is `name`; the next frame
    /// sets their cells off.
    fn set_spinning(&mut self, name: &str) {
        let Some(show) = &self.show else { return };
        let mut spinning = Vec::new();
        for site in &self.reel_sites {
            let (root, path) = site;
            if *root != Root::Show && Some(*root) != self.active_scene.map(Root::Scene) {
                continue;
            }
            let reel = root_layers(show, *root)
                .and_then(|layers| layer_at(layers, path))
                .and_then(|layer| match &layer.kind {
                    LayerKind::Digits {
                        display: DigitDisplay::Reel(reel),
                        ..
                    } => Some(reel),
                    _ => None,
                });
            if reel.is_some_and(|reel| reel.spin.contains(name)) && !self.spinning.contains(site) {
                spinning.push(site.clone());
            }
        }
        self.spinning.extend(spinning);
    }

    /// Where the cells of the reel row at `path` stand on their ring now.
    pub fn reel_positions(
        &self,
        root: Root,
        path: &[usize],
        reel: &crate::model::Reel,
    ) -> Vec<f64> {
        let roll = reel.roll();
        self.reels
            .get(&(root, path.to_vec()))
            .map(|records| {
                records
                    .iter()
                    .map(|c| roll.value_at(c.start, c.target, self.time - c.started))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Note where every binding with a transition is heading, as of now
    /// (inputs arrive between frames): a first look starts the property at
    /// its value, a new target starts a change from the value reached.
    fn follow_transitions(&mut self) {
        let Some(show) = &self.show else { return };
        let mut transitions = std::mem::take(&mut self.transitions);
        let mut colors = std::mem::take(&mut self.color_transitions);
        for site in &self.transition_sites {
            let (root, path, index) = site;
            if *root != Root::Show && Some(*root) != self.active_scene.map(Root::Scene) {
                continue;
            }
            let layer = root_layers(show, *root).and_then(|layers| layer_at(layers, path));
            let binding = layer.and_then(|layer| layer.bindings.get(*index));
            let Some((binding, transition)) =
                binding.and_then(|b| Some((b, b.transition.as_ref()?)))
            else {
                continue;
            };
            // A modelled transition holds a filament temperature whatever
            // the property is, so it takes the numeric path.
            if binding.property == Property::Tint && transition.model.is_none() {
                let target = self
                    .binding_value(site, binding)
                    .and_then(|value| binding.convert(value, show))
                    .map(|value| value.to_text())
                    .and_then(|text| parse_color(&text));
                let Some(target) = target else {
                    colors.remove(site);
                    continue;
                };
                let change = colors.entry(site.clone()).or_insert(ColorChange {
                    start: target,
                    target,
                    started: self.time,
                });
                if change.target != target {
                    let progress = transition.value_at(0.0, 1.0, self.time - change.started);
                    *change = ColorChange {
                        start: change.value_at(progress),
                        target,
                        started: self.time,
                    };
                }
                continue;
            }
            let Some(target) = self.binding_number(site, binding) else {
                transitions.remove(site);
                continue;
            };
            let modelled = transition.model.is_some();
            let change = transitions.entry(site.clone()).or_insert(Change {
                // A show starts with its lamps cold, whatever they are
                // being told: a bulb takes its time even on the first
                // frame.
                start: if modelled {
                    crate::lamp::settled(crate::lamp::Filament::of(transition), 0.0)
                } else {
                    target
                },
                target,
                started: self.time,
                whole: target.fract() == 0.0,
            });
            if change.target != target {
                let reached = if modelled {
                    // Carry the heat over: a bulb re-lit while still warm
                    // comes up from where it is.
                    crate::lamp::temperature(
                        crate::lamp::Filament::of(transition),
                        change.start,
                        change.target,
                        self.time - change.started,
                    )
                } else {
                    transition.value_at(change.start, change.target, self.time - change.started)
                };
                *change = Change {
                    // Both values the binding was given: the one it was
                    // heading for, and the one it is heading for now.
                    whole: change.target.fract() == 0.0 && target.fract() == 0.0,
                    start: reached,
                    target,
                    started: self.time,
                };
            }
        }
        self.transitions = transitions;
        self.color_transitions = colors;
    }

    /// What binding `index` of the layer at `path` holds while its
    /// transition is under way; `None` without one.
    fn in_transition(
        &self,
        root: Root,
        path: &[usize],
        index: usize,
        b: &Binding,
    ) -> Option<Value> {
        let transition = b.transition.as_ref()?;
        if b.property == Property::Tint && transition.model.is_none() {
            let change = self.color_transitions.get(&(root, path.to_vec(), index))?;
            let progress = transition.value_at(0.0, 1.0, self.time - change.started);
            return Some(Value::Text(color_text(change.value_at(progress))));
        }
        let change = self.transitions.get(&(root, path.to_vec(), index))?;
        if let Some(crate::model::Model::Incandescent) = transition.model {
            let lamp = crate::lamp::Filament::of(transition);
            let hot = crate::lamp::temperature(
                lamp,
                change.start,
                change.target,
                self.time - change.started,
            );
            // The same filament, read two ways: how much light it gives,
            // or what colour that light is.
            return Some(match b.property {
                Property::Tint => {
                    let [r, g, bl] = crate::lamp::color(hot);
                    Value::Text(color_text([r, g, bl, 255]))
                }
                _ => Value::Number(crate::lamp::shown(lamp, hot)),
            });
        }
        let mut n = transition.value_at(change.start, change.target, self.time - change.started);
        if b.property != Property::Text {
            return Some(Value::Number(n));
        }
        // A counter between whole numbers shows whole numbers; with
        // decimals the formatting already quantises it to the last place
        // shown, so it does not flicker through digits that are rounded
        // away.
        if change.whole && b.decimals.is_none() {
            n = n.round();
        }
        Some(Value::Text(b.worded(b.format.format(n, b.decimals))))
    }

    /// Resolve a layer property: its base value, overridden by bindings
    /// (eased by their transitions), overridden by a running timeline
    /// (numeric properties only). `None` when this kind of layer does not
    /// have the property.
    fn resolve(&self, root: Root, layer: &Layer, path: &[usize], prop: Property) -> Option<Value> {
        let mut v = layer.base_value(prop)?;
        let bindings = layer.bindings.iter().enumerate();
        for (index, b) in bindings.filter(|(_, b)| b.property == prop) {
            let bound = self.in_transition(root, path, index, b).or_else(|| {
                self.binding_value(&(root, path.to_vec(), index), b)
                    .and_then(|value| b.convert(value, self.show.as_ref()?))
            });
            if let Some(bound) = bound {
                v = bound;
            }
        }
        if !prop.is_numeric() {
            return Some(v);
        }
        // A running timeline owns the property, over one that has
        // finished and is holding its last value.
        for running in [false, true] {
            for p in self.playing.iter().filter(|p| p.held != running) {
                if !p.owner.is_layer(root, path) {
                    continue;
                }
                let Some(tl) = layer.timelines.get(p.timeline) else {
                    continue;
                };
                // Still waiting out its delay: it owns nothing yet.
                let Some(time) = tl.local_time(p.at(self.time, tl.into())) else {
                    continue;
                };
                for track in tl.tracks.iter().filter(|t| t.property == prop) {
                    if let Some(sampled) = track.sample(time) {
                        v = Value::Number(sampled);
                    }
                }
            }
        }
        Some(v)
    }

    /// What `prop` of the layer at `path` resolves to, as a number.
    pub fn number(&self, root: Root, layer: &Layer, path: &[usize], prop: Property) -> f64 {
        self.resolve(root, layer, path, prop)
            .map_or(0.0, |v| v.as_number())
    }

    /// What `prop` of the layer at `path` resolves to, as text.
    pub fn text(&self, root: Root, layer: &Layer, path: &[usize], prop: Property) -> String {
        self.resolve(root, layer, path, prop)
            .map_or_else(String::new, |v| v.to_text())
    }

    /// What the reading at `site` gives now: the variable's value (as
    /// debounced), or the show's own value of that name when no host set
    /// one, then what `map` and `default` make of it. `None` when it has
    /// nothing to say.
    fn read(&self, site: &ReadSite, reading: &Reading) -> Option<Value> {
        let value = match (reading.debounce, self.debounced.get(site)) {
            (Some(_), Some(settling)) => settling.settled.clone(),
            // The show's own value when no host set one.
            _ => self.value(&reading.variable)?,
        };
        reading.mapped(value)
    }

    /// The value a binding at `site` feeds its property; see
    /// [`read`](Self::read).
    fn binding_value(&self, site: &TransitionSite, b: &Binding) -> Option<Value> {
        let (root, path, index) = site;
        self.read(&(*root, path.clone(), Reader::Binding(*index)), &b.reading)
    }

    /// The number a binding's transition eases toward: its value after
    /// `map`, `threshold`, `scale` and `offset`. `None` when that is not
    /// a number.
    fn binding_number(&self, site: &TransitionSite, b: &Binding) -> Option<f64> {
        let value = self.binding_value(site, b)?;
        // A modelled tint is fed a power level, not a colour: the model
        // decides what colour that is.
        let lamp = b.transition.as_ref().is_some_and(|t| t.model.is_some());
        let n = match (b.property, &value) {
            (Property::Tint, _) if lamp => value.as_number(),
            (Property::Font | Property::Tint, _) => return None,
            (Property::Text, Value::Number(n)) => *n,
            (Property::Text, _) => return None,
            _ => value.as_number(),
        };
        Some(b.scaled(n))
    }

    /// Whether the layer at `path` shows and sounds: its `visible`, as
    /// bound.
    pub fn is_visible(&self, root: Root, layer: &Layer, path: &[usize]) -> bool {
        match self.resolve(root, layer, path, Property::Visible) {
            Some(Value::Bool(on)) => on,
            Some(Value::Number(n)) => n != 0.0,
            Some(Value::Text(t)) => !t.is_empty(),
            None => layer.visible,
        }
    }
}
/// The characters of `text` laid into `count` cells, `None` where a cell
/// has nothing to show. Text longer than the row is cut at the far side
/// of `justify`, as a digit row's text is.
pub fn row_cells(text: &str, count: usize, justify: Justify) -> Vec<Option<char>> {
    let characters: Vec<char> = text.chars().collect();
    match justify {
        Justify::Right => {
            let skip = characters.len().saturating_sub(count);
            let mut out = vec![None; count.saturating_sub(characters.len())];
            out.extend(characters.into_iter().skip(skip).map(Some));
            out
        }
        _ => {
            let mut out: Vec<Option<char>> = characters.into_iter().map(Some).collect();
            out.resize(count, None);
            out
        }
    }
}

/// Where the show's reel rows are.
fn reel_sites(show: &Show) -> Vec<(Root, Vec<usize>)> {
    fn walk(
        root: Root,
        layers: &[Layer],
        path: &mut Vec<usize>,
        out: &mut Vec<(Root, Vec<usize>)>,
    ) {
        for (i, layer) in layers.iter().enumerate() {
            path.push(i);
            if let LayerKind::Digits {
                display: DigitDisplay::Reel(_),
                ..
            } = &layer.kind
            {
                out.push((root, path.clone()));
            }
            walk(root, layer.children(), path, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(Root::Show, &show.layers, &mut Vec::new(), &mut out);
    for (i, scene) in show.scenes.iter().enumerate() {
        walk(Root::Scene(i), &scene.layers, &mut Vec::new(), &mut out);
    }
    out
}

/// What a playhead has played so far.
#[derive(Debug, Clone, Copy, Default)]
struct Played {
    /// How many plays it has started, which picks from a list of assets.
    count: u64,
    /// When the last one started, which `rest` measures from.
    at: f64,
}

/// Which of `names` the play numbered `ordinal` on a layer takes.
///
/// Every mode is a function of the ordinal alone, so the same play always
/// takes the same asset: a show renders the same way twice, and a seek
/// back to a play would find what it found the first time.
fn pick_one(names: &Choice, how: Pick, ordinal: u64, seed: u64) -> String {
    let count = names.len() as u64;
    if count <= 1 {
        return names.first().to_owned();
    }
    let index = match how {
        Pick::InOrder => ordinal % count,
        Pick::Random => mix(seed ^ mix(ordinal)) % count,
        // A fresh scramble per round through the list.
        Pick::Shuffle => scramble(names.len(), seed, ordinal / count)[(ordinal % count) as usize],
    };
    names.get(index as usize).to_owned()
}

/// `0..count` in a scrambled order, the same order every time for a given
/// `seed` and `round`.
fn scramble(count: usize, seed: u64, round: u64) -> Vec<u64> {
    let mut order: Vec<u64> = (0..count as u64).collect();
    let mut state = mix(seed ^ mix(round));
    for i in (1..count).rev() {
        state = mix(state);
        order.swap(i, (state % (i as u64 + 1)) as usize);
    }
    order
}

/// Tells layers apart, so two of them picking from the same list do not
/// pick in step.
fn seed_of(root: Root, path: &[usize]) -> u64 {
    let start = match root {
        Root::Show => 0,
        Root::Scene(i) => i as u64 + 1,
    };
    path.iter()
        .fold(mix(start), |acc, i| mix(acc ^ (*i as u64 + 1)))
}

/// Scatters the bits of a counter (splitmix64's finalizer). Not random:
/// the same input always gives the same output.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The layers of one tree of the show; `None` for a scene it does not
/// have.
pub fn root_layers(show: &Show, root: Root) -> Option<&[Layer]> {
    match root {
        Root::Show => Some(&show.layers),
        Root::Scene(i) => show.scenes.get(i).map(|s| s.layers.as_slice()),
    }
}

/// What is wrong with a set of `offset` keys, if anything: they carry
/// motion added on top of a move, so they run forward from 0 and have to
/// come back to where they started.
fn offset_problem(offset: &[crate::model::Key]) -> Option<&'static str> {
    if offset.windows(2).any(|pair| pair[1].t < pair[0].t)
        || offset.iter().any(|k| k.t.is_nan() || k.t < 0.0)
    {
        return Some("needs offset keys in time order, from 0 on");
    }
    let ends = [offset.first(), offset.last()];
    if ends.iter().flatten().any(|k| k.v != 0.0) {
        return Some("needs an offset that starts and ends at 0");
    }
    None
}

/// The name one video layer's picture is registered under.
///
/// A layer, not a clip: a clip playing on two layers is at two positions
/// at once, and under one name the two would share a frame. Not a name
/// a host would give an asset, since nothing else is registered with a
/// space in it.
pub fn frame_key(root: Root, path: &[usize]) -> String {
    let mut key = match root {
        Root::Show => "video show".to_owned(),
        Root::Scene(i) => format!("video scene {i}"),
    };
    for step in path {
        key.push_str(&format!("/{step}"));
    }
    key
}

/// The values a show animates that a binding eases from.
///
/// A transition follows what its binding reads, so the instant that
/// input changes is an instant the clock has to stop at. Only values
/// the show animates need it: a variable is set by the host, which
/// happens between frames anyway.
fn eased_values(show: &Show) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    fn walk(show: &Show, layers: &[Layer], out: &mut BTreeSet<String>) {
        for layer in layers {
            let eased = layer
                .bindings
                .iter()
                .filter(|binding| binding.transition.is_some());
            for binding in eased {
                if show.values.contains_key(&binding.reading.variable) {
                    out.insert(binding.reading.variable.clone());
                }
            }
            walk(show, layer.children(), out);
        }
    }
    for layers in show.layer_trees() {
        walk(show, layers, &mut out);
    }
    out
}

/// The timeline conditions that read a value the show animates, each
/// with the tree it is in.
fn value_conditions(show: &Show) -> Vec<(Root, Reading)> {
    let mut out = Vec::new();
    for (root, layers) in std::iter::once((Root::Show, show.layers.as_slice())).chain(
        show.scenes
            .iter()
            .enumerate()
            .map(|(i, scene)| (Root::Scene(i), scene.layers.as_slice())),
    ) {
        collect_timelines(layers, &mut Vec::new(), &mut |_, _, tl| {
            for condition in [&tl.when, &tl.whilst].into_iter().flatten() {
                if show.values.contains_key(&condition.variable) {
                    out.push((root, condition.clone()));
                }
            }
        });
        fn media(layers: &[Layer], show: &Show, root: Root, out: &mut Vec<(Root, Reading)>) {
            for layer in layers {
                if let Some(media) = layer.kind.media() {
                    for condition in [media.when, media.whilst].into_iter().flatten() {
                        if show.values.contains_key(&condition.variable) {
                            out.push((root, condition.clone()));
                        }
                    }
                }
                media(layer.children(), show, root, out);
            }
        }
        media(layers, show, root, &mut out);
    }
    out
}

/// Where the show's bindings with a transition are, and every reading
/// with a debounce, binding or condition.
fn binding_sites(show: &Show) -> (Vec<TransitionSite>, Vec<ReadSite>) {
    type Sites = (Vec<TransitionSite>, Vec<ReadSite>);
    fn walk(root: Root, layers: &[Layer], path: &mut Vec<usize>, out: &mut Sites) {
        for (i, layer) in layers.iter().enumerate() {
            path.push(i);
            for (index, binding) in layer.bindings.iter().enumerate() {
                if binding.transition.is_some() {
                    out.0.push((root, path.clone(), index));
                }
                if binding.reading.debounce.is_some() {
                    out.1.push((root, path.clone(), Reader::Binding(index)));
                }
            }
            for (index, tl) in layer.timelines.iter().enumerate() {
                let conditions = [
                    (Reader::When(index), &tl.when),
                    (Reader::While(index), &tl.whilst),
                ];
                for (reader, condition) in conditions {
                    if condition.as_ref().is_some_and(|c| c.debounce.is_some()) {
                        out.1.push((root, path.clone(), reader));
                    }
                }
            }
            if let Some(media) = layer.kind.media() {
                let conditions = [
                    (Reader::MediaWhen, media.when),
                    (Reader::MediaWhile, media.whilst),
                ];
                for (reader, condition) in conditions {
                    if condition.is_some_and(|c| c.debounce.is_some()) {
                        out.1.push((root, path.clone(), reader));
                    }
                }
            }
            walk(root, layer.children(), path, out);
            path.pop();
        }
    }
    let mut out = (Vec::new(), Vec::new());
    walk(Root::Show, &show.layers, &mut Vec::new(), &mut out);
    for (i, scene) in show.scenes.iter().enumerate() {
        walk(Root::Scene(i), &scene.layers, &mut Vec::new(), &mut out);
    }
    out
}

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
fn parse_document(json: &str) -> Result<serde_json::Value, Error> {
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
fn parse_show(raw: &serde_json::Value) -> Result<Show, Error> {
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

/// Something `load_show` refuses a show for, at the place that would
/// have to go for the show to load.
struct Problem {
    site: Site,
    error: Error,
}

impl Problem {
    fn finding(&self) -> Finding {
        Finding {
            path: self.site.path(),
            message: self.error.to_string(),
            kind: FindingKind::Error,
        }
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
fn remove_blanks(raw: &mut serde_json::Value, mut blanked: Vec<Site>) {
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

/// Everything `load_show` refuses a parsed show for, in the order it
/// checks: the show's own fields, its outputs, its font styles, then
/// every layer in document order, with at most one problem per site.
/// Strict loading fails on the first; tolerant loading drops every site
/// listed and asks again.
fn problems(show: &Show) -> Vec<Problem> {
    let mut out = Vec::new();
    if parse_color(&show.background).is_none() {
        out.push(Problem {
            site: Site::Background,
            error: Error::InvalidColor(show.background.clone()),
        });
    }
    let outputs = std::iter::once((None, &show.output)).chain(
        show.scenes
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((Some(i), s.output.as_ref()?))),
    );
    for (scene, output) in outputs {
        if let Some(tint) = output.tint.as_ref().filter(|t| parse_color(t).is_none()) {
            out.push(Problem {
                site: Site::Tint(scene),
                error: Error::InvalidColor(tint.clone()),
            });
        }
        for (i, pass) in output.passes.iter().flatten().enumerate() {
            if let Err(error) = pass_problem(pass) {
                out.push(Problem {
                    site: Site::Pass(scene, i),
                    error,
                });
            }
        }
    }
    validate(show, &mut out);
    out
}

/// What is wrong with a pass, if anything.
fn pass_problem(pass: &Pass) -> Result<(), Error> {
    let Pass::Dots(dots) = pass;
    if !(dots.size > 0.0 && dots.size <= 1.0) {
        return Err(Error::InvalidShow(
            "a dots pass needs a size above 0, up to 1".into(),
        ));
    }
    if !(0.0..=1.0).contains(&dots.glow) {
        return Err(Error::InvalidShow(
            "a dots pass needs a glow from 0 to 1".into(),
        ));
    }
    if let Some(unlit) = &dots.unlit {
        parse_color(unlit).ok_or_else(|| Error::InvalidColor(unlit.clone()))?;
    }
    Ok(())
}

/// Checks `load_show` does beyond parsing: colors parse, text layers use
/// declared font styles, only numeric properties are keyframed. One
/// problem per font style and per layer, the first found.
fn validate(show: &Show, out: &mut Vec<Problem>) {
    for (name, style) in &show.fonts {
        if let Err(error) = font_style_problem(style) {
            out.push(Problem {
                site: Site::Font(name.clone()),
                error,
            });
        }
    }
    fn layers(
        show: &Show,
        root: Root,
        path: &mut Vec<(usize, bool)>,
        list: &[Layer],
        parts: bool,
        out: &mut Vec<Problem>,
    ) {
        for (i, layer) in list.iter().enumerate() {
            path.push((i, parts));
            let problem = match (&layer.kind, parts) {
                // A part is made from an artwork layer's `parts`, and
                // is nothing anywhere else.
                (LayerKind::Part { .. }, false) => Err(Error::InvalidShow(format!(
                    "layer {:?} is a part, which only the parts of an artwork layer hold",
                    layer.name
                ))),
                _ => layer_problem(show, layer),
            };
            if let Err(error) = problem {
                out.push(Problem {
                    site: Site::Layer(root, path.clone()),
                    error,
                });
            }
            layers(show, root, path, layer.children(), layer.holds_parts(), out);
            path.pop();
        }
    }
    layers(show, Root::Show, &mut Vec::new(), &show.layers, false, out);
    for (i, scene) in show.scenes.iter().enumerate() {
        layers(
            show,
            Root::Scene(i),
            &mut Vec::new(),
            &scene.layers,
            false,
            out,
        );
    }
}

/// What is wrong with a font style, if anything.
fn font_style_problem(style: &crate::model::FontStyle) -> Result<(), Error> {
    for color in std::iter::once(&style.color)
        .chain(style.border.as_ref().map(|b| &b.color))
        .chain(style.shadow.as_ref().map(|s| &s.color))
    {
        parse_color(color).ok_or_else(|| Error::InvalidColor(color.clone()))?;
    }
    if let Some(shadow) = &style.shadow {
        if !shadow.offset.iter().all(|n| n.is_finite()) {
            return Err(Error::InvalidShow(format!(
                "font style {:?} needs a finite shadow offset",
                style.file
            )));
        }
    }
    Ok(())
}

/// What is wrong with a layer itself, if anything: its children are
/// looked at on their own.
fn layer_problem(show: &Show, layer: &Layer) -> Result<(), Error> {
    if let LayerKind::Shape {
        fill: crate::model::Fill::Gradient(gradient),
        ..
    } = &layer.kind
    {
        let stops = gradient.stops();
        let problem = if stops.is_empty() {
            Some("needs a stop".to_owned())
        } else if !stops.iter().all(|s| s.at.is_finite()) {
            Some("needs finite stop positions".to_owned())
        } else if stops.windows(2).any(|w| w[1].at < w[0].at) {
            Some("needs its stops in order".to_owned())
        } else if matches!(
            gradient,
            crate::model::Gradient::Radial { radius, .. } if !(radius.is_finite() && *radius > 0.0)
        ) {
            Some("needs a radius above 0".to_owned())
        } else {
            stops
                .iter()
                .find(|s| parse_color(&s.color).is_none())
                .map(|s| format!("has a stop that is not a color: {:?}", s.color))
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "the gradient of layer {:?} {problem}",
                layer.name
            )));
        }
    }
    if let LayerKind::Shape {
        stroke: Some(stroke),
        ..
    } = &layer.kind
    {
        parse_color(&stroke.color).ok_or_else(|| Error::InvalidColor(stroke.color.clone()))?;
        if !(stroke.width.is_finite() && stroke.width > 0.0) {
            return Err(Error::InvalidShow(format!(
                "layer {:?} needs a stroke width above 0",
                layer.name
            )));
        }
    }
    let font = match &layer.kind {
        LayerKind::Text { font, .. } => Some(font),
        LayerKind::Digits {
            display: DigitDisplay::Reel(reel),
            ..
        } => reel.font.as_ref(),
        _ => None,
    };
    if let Some(font) = font.filter(|font| !show.fonts.contains_key(*font)) {
        return Err(Error::InvalidShow(format!(
            "layer {:?} uses undeclared font style {font:?}",
            layer.name
        )));
    }
    if let LayerKind::Digits {
        display: DigitDisplay::Reel(reel),
        ..
    } = &layer.kind
    {
        let problem = if reel.charset.is_empty() {
            Some("needs a charset with a character in it")
        } else if !(reel.duration.is_finite() && reel.duration > 0.0) {
            Some("needs a duration above 0")
        } else if !reel.stagger.is_finite() || reel.stagger < 0.0 {
            Some("needs a stagger of 0 or more")
        } else if reel.font.is_none() && reel.cells.is_none() {
            Some("needs a font for its characters, or cells to draw instead")
        } else if reel
            .cells
            .as_ref()
            .is_some_and(|cells| cells.len() != reel.charset.chars().count())
        {
            Some("needs one cell for every character of its charset")
        } else if reel.window == 0 {
            Some("needs a window of at least one character")
        } else if reel
            .step
            .is_some_and(|step| !step.is_finite() || step <= 0.0)
        {
            Some("needs a step above 0, or none at all to travel in one move")
        } else {
            offset_problem(&reel.offset)
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "reel of layer {:?} {problem}",
                layer.name
            )));
        }
    }
    // Properties must exist on this kind of layer; only numeric
    // ones can be keyframed.
    let tracks = layer.timelines.iter().flat_map(|tl| &tl.tracks);
    let used = tracks
        .map(|t| t.property)
        .chain(layer.bindings.iter().map(|b| b.property));
    for property in used {
        if layer.base_value(property).is_none() {
            return Err(Error::InvalidShow(format!(
                "layer {:?} has no {property:?} property",
                layer.name
            )));
        }
    }
    for timeline in &layer.timelines {
        if timeline.looping && timeline.repeat.is_some() {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} sets both loop and repeat",
                timeline.name, layer.name
            )));
        }
        if let Some(track) = timeline.tracks.iter().find(|t| !t.property.is_numeric()) {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} animates {:?}, which can only be bound",
                timeline.name, layer.name, track.property
            )));
        }
        if timeline.when.is_some() && timeline.whilst.is_some() {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} sets both when and while, which \
                 want different things of the same condition",
                timeline.name, layer.name
            )));
        }
        for (which, condition) in [("when", &timeline.when), ("while", &timeline.whilst)] {
            if let Some(problem) = condition.as_ref().and_then(reading_problem) {
                return Err(Error::InvalidShow(format!(
                    "the {which} of timeline {:?} of layer {:?} {problem}",
                    timeline.name, layer.name
                )));
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
        let finite = |n: f64| n.is_finite() && n >= 0.0;
        let problem = if duck.under.is_empty() {
            Some("needs a bus to listen to")
        } else if !finite(duck.to) {
            Some("needs a gain of 0 or more to duck to")
        } else if !finite(duck.attack) || !finite(duck.release) {
            Some("needs an attack and release of 0 or more")
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "the duck of layer {:?} {problem}",
                layer.name
            )));
        }
    }
    for binding in &layer.bindings {
        if let Some(problem) = reading_problem(&binding.reading) {
            return Err(Error::InvalidShow(format!(
                "the {:?} binding of layer {:?} {problem}",
                binding.property, layer.name
            )));
        }
        if binding.decimals.is_some_and(|d| d > 15) {
            return Err(Error::InvalidShow(format!(
                "the {:?} binding of layer {:?} asks for more decimals than a number has",
                binding.property, layer.name
            )));
        }
        if let Some(transition) = &binding.transition {
            let positive = |n: f64| n.is_finite() && n > 0.0;
            let ring = transition.wrap.is_some() || transition.direction.is_some();
            let modelled = transition.model.is_some();
            let problem = if matches!(binding.property, Property::Font | Property::Visible) {
                Some("is on a binding that cannot be eased")
            } else if binding.property == Property::Tint && ring {
                Some("sets wrap or direction, which a color has no use for")
            } else if modelled
                && (positive(transition.duration)
                    || ring
                    || transition.step.is_some()
                    || !transition.offset.is_empty()
                    || transition.ease != crate::easing::Easing::default())
            {
                Some("follows a model, which decides its own timing, shape and way round")
            } else if !modelled
                && (transition.kelvin.is_some()
                    || transition.heating.is_some()
                    || transition.cooling.is_some())
            {
                Some("shapes a filament without naming a model to follow")
            } else if modelled
                && [transition.kelvin, transition.heating, transition.cooling]
                    .into_iter()
                    .flatten()
                    .any(|n| !positive(n))
            {
                Some("needs a kelvin, heating and cooling above 0")
            } else if !modelled && !positive(transition.duration) {
                Some("needs a duration above 0")
            } else if transition.wrap.is_some_and(|wrap| !positive(wrap)) {
                Some("needs a wrap above 0")
            } else if transition.direction.is_some() && transition.wrap.is_none() {
                Some("sets a direction, which needs wrap")
            } else if transition.step.is_some_and(|step| !positive(step)) {
                Some("needs a step above 0")
            } else {
                offset_problem(&transition.offset)
            };
            if let Some(problem) = problem {
                return Err(Error::InvalidShow(format!(
                    "transition of the {:?} binding of layer {:?} {problem}",
                    binding.property, layer.name
                )));
            }
        }
        // A modelled tint takes a power level, not a colour: the
        // filament decides what colour that is.
        let lamp = binding
            .transition
            .as_ref()
            .is_some_and(|t| t.model.is_some());
        if binding.property == Property::Tint && !lamp {
            let mapped = binding.reading.map.iter().flat_map(|m| m.values());
            for value in mapped.chain(&binding.reading.default) {
                let color = matches!(value, Value::Text(c) if c.is_empty()
                    || parse_color(c).is_some());
                if !color {
                    return Err(Error::InvalidShow(format!(
                        "tint binding of layer {:?} maps to {value:?}, not a color",
                        layer.name
                    )));
                }
            }
        }
        if binding.property != Property::Font {
            continue;
        }
        let mapped = binding.reading.map.iter().flat_map(|m| m.values());
        for value in mapped.chain(&binding.reading.default) {
            let known = matches!(value, Value::Text(style) if show.fonts.contains_key(style));
            if !known {
                return Err(Error::InvalidShow(format!(
                    "font binding of layer {:?} maps to {value:?}, not a declared font style",
                    layer.name
                )));
            }
        }
    }
    if let Some(press) = &layer.press {
        if press.trigger.is_none() && press.open.is_none() {
            return Err(Error::InvalidShow(format!(
                "the press of layer {:?} neither fires a trigger nor opens a link",
                layer.name
            )));
        }
        if let Some(url) = press.open.as_deref() {
            let web = url.starts_with("http://") || url.starts_with("https://");
            if !web {
                return Err(Error::InvalidShow(format!(
                    "the press of layer {:?} opens {url:?}, which is not an http or https address",
                    layer.name
                )));
            }
        }
    }
    if matches!(
        layer.kind,
        LayerKind::Group { .. } | LayerKind::Audio { .. }
    ) && layer.anchor.is_some()
    {
        return Err(Error::InvalidShow(format!(
            "layer {:?} has an anchor, but no content box",
            layer.name
        )));
    }
    if let LayerKind::Image {
        tint: Some(tint), ..
    } = &layer.kind
    {
        parse_color(tint).ok_or_else(|| Error::InvalidColor(tint.clone()))?;
    }
    if let Some(media) = layer.kind.media() {
        let gain = match &layer.kind {
            LayerKind::Audio { gain, .. } | LayerKind::Video { gain, .. } => *gain,
            _ => 1.0,
        };
        let problem = if media.looping && media.repeat.is_some() {
            Some("sets both loop and repeat")
        } else if !media.delay.is_finite() || media.delay < 0.0 {
            Some("needs a delay of 0 or more")
        } else if media.repeat.is_some_and(|r| !r.is_finite() || r < 0.0) {
            Some("needs a repeat of 0 or more")
        } else if !gain.is_finite() || gain < 0.0 {
            Some("needs a gain of 0 or more")
        } else if media.retrigger == Retrigger::Overlap && media.voices == 0 {
            Some("needs at least one voice to overlap")
        } else if media.retrigger == Retrigger::Overlap && media.kind == MediaKind::Video {
            Some("cannot overlap: a video layer shows one picture at a time")
        } else if media.names.is_empty() {
            Some("names nothing to play")
        } else if !media.rest.is_finite() || media.rest < 0.0 {
            Some("needs a rest of 0 or more")
        } else if media.when.is_some() && media.whilst.is_some() {
            Some("sets both when and while, which want different things of the same condition")
        } else {
            None
        };
        let kind = match media.kind {
            MediaKind::Sound => "audio",
            MediaKind::Video => "video",
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "{kind} layer {:?} {problem}",
                layer.name
            )));
        }
        for (which, condition) in [("when", media.when), ("while", media.whilst)] {
            if let Some(problem) = condition.and_then(reading_problem) {
                return Err(Error::InvalidShow(format!(
                    "the {which} of {kind} layer {:?} {problem}",
                    layer.name
                )));
            }
        }
    }
    Ok(())
}

/// Warn about a segment display whose own text, given as masks or
/// levels, has a cell that is not hexadecimal: it draws dark, which
/// looks like a segment that does not work rather than a typo. Text a
/// binding gives is not the document's, and is not looked at here.
fn dark_segment_cells(show: &Show, out: &mut Vec<String>) {
    use crate::model::SegmentInput;
    fn walk(layers: &[Layer], out: &mut Vec<String>) {
        for layer in layers {
            if let LayerKind::Digits {
                text,
                display: DigitDisplay::Segments { input, .. },
                ..
            } = &layer.kind
            {
                let cells = text
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|t| !t.is_empty());
                for cell in cells {
                    let fine = match input {
                        SegmentInput::Masks => {
                            let digits = cell.trim_start_matches("0x").trim_start_matches("0X");
                            u32::from_str_radix(digits, 16).is_ok()
                        }
                        SegmentInput::Levels => cell.chars().all(|c| c.is_ascii_hexdigit()),
                        SegmentInput::Text => true,
                    };
                    if !fine {
                        out.push(format!(
                            "the text of layer {:?} has a cell {cell:?} that is not hexadecimal; \
                             it draws dark",
                            layer.name
                        ));
                    }
                }
            }
            walk(layer.children(), out);
        }
    }
    for layers in show.layer_trees() {
        walk(layers, out);
    }
}

/// Warn about bindings that will quietly do nothing.
///
/// A bound value the engine cannot use leaves the property as it was,
/// silently, the way an unregistered image simply does not draw: the host
/// may send something usable later, and a frame is no place to complain.
/// That is right at runtime and useless while writing a show, where a
/// mistyped variable or a color that is not one looks exactly like a
/// feature that does not work.
///
/// What a show does state up front is which variables it declares and
/// what they start at, so that is what is checked. Values written in the
/// show itself, like the colors and styles a `map` lists, are errors at
/// load instead.
/// With `undeclared`, a binding reading a variable the show does not
/// declare is one of them; the audit says that itself, with the
/// binding's place, and asks for the rest.
pub(crate) fn quiet_bindings(show: &Show, undeclared: bool, out: &mut Vec<String>) {
    fn walk(show: &Show, layers: &[Layer], undeclared: bool, out: &mut Vec<String>) {
        for layer in layers {
            for binding in &layer.bindings {
                // A curve bends a number, and these properties never hold
                // one, so it would quietly do nothing.
                if !binding.reading.curve.is_empty()
                    && matches!(
                        binding.property,
                        Property::Tint | Property::Font | Property::Video | Property::Sound
                    )
                {
                    out.push(format!(
                        "the {:?} binding of layer {:?} has a curve, which only bends a number; \
                         this property never holds one",
                        binding.property, layer.name
                    ));
                }
                // Words only go round text; every other property holds
                // a number or a name of its own.
                if (!binding.prefix.is_empty() || !binding.suffix.is_empty())
                    && binding.property != Property::Text
                {
                    out.push(format!(
                        "the {:?} binding of layer {:?} has words round its value, which only a \
                         text binding shows",
                        binding.property, layer.name
                    ));
                }
                let name = &binding.reading.variable;
                if !show.variables.contains_key(name) && show.values.contains_key(name) {
                    // A value the show animates is declared as much as a
                    // variable is. It is always a number, though, so a
                    // property that cannot take one without a map still
                    // gets nothing.
                    let problem = match binding.property {
                        _ if binding.reading.map.is_some() => None,
                        Property::Tint => Some("a color like \"#RRGGBB\""),
                        Property::Font => Some("one of the show's font styles"),
                        _ => None,
                    };
                    if let Some(wanted) = problem {
                        out.push(format!(
                            "the {:?} binding of layer {:?} reads value {name:?}, which is a \
                             number, not {wanted}; values it cannot use leave the property alone",
                            binding.property, layer.name
                        ));
                    }
                    continue;
                }
                let Some(value) = show.variables.get(name) else {
                    if undeclared {
                        out.push(format!(
                            "the {:?} binding of layer {:?} reads variable {name:?}, which the \
                             show does not declare; it does nothing until a host sets that \
                             variable",
                            binding.property, layer.name
                        ));
                    }
                    continue;
                };
                // With a map it is the mapped values that reach the
                // property, and those are checked at load.
                if binding.reading.map.is_some() {
                    continue;
                }
                // A modelled tint reads a power level, not a colour.
                if binding
                    .transition
                    .as_ref()
                    .is_some_and(|t| t.model.is_some())
                {
                    continue;
                }
                let text = value.to_text();
                let problem = match binding.property {
                    Property::Tint if !text.is_empty() && parse_color(&text).is_none() => {
                        Some("a color like \"#RRGGBB\"")
                    }
                    Property::Font if !show.fonts.contains_key(&text) => {
                        Some("one of the show's font styles")
                    }
                    _ => None,
                };
                if let Some(wanted) = problem {
                    out.push(format!(
                        "the {:?} binding of layer {:?} reads variable {name:?}, which starts at \
                         {text:?}, not {wanted}; values it cannot use leave the property alone",
                        binding.property, layer.name
                    ));
                }
            }
            walk(show, layer.children(), undeclared, out);
        }
    }
    for layers in show.layer_trees() {
        walk(show, layers, undeclared, out);
    }
}

/// The field a name the model still accepts is written back as, so an
/// older spelling is not reported as a field nothing read.
///
/// A document is checked against itself after a round trip through the
/// model, and an alias comes back as the name the model keeps: `vector`
/// returns as `image`, since one artwork layer draws both.
fn also(key: &str) -> Option<&'static str> {
    match key {
        "vector" => Some("image"),
        _ => None,
    }
}

/// Collect the paths of object keys present in `given` but absent from
/// `understood` (the same document after a round trip through the model),
/// which are the fields deserialization silently dropped.
pub(crate) fn ignored_fields(
    given: &serde_json::Value,
    understood: &serde_json::Value,
    path: &str,
    out: &mut Vec<String>,
) {
    use serde_json::Value as Json;
    match (given, understood) {
        (Json::Object(given), Json::Object(understood)) => {
            for (key, value) in given {
                let here = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match understood.get(key).or_else(|| understood.get(also(key)?)) {
                    Some(kept) => ignored_fields(value, kept, &here, out),
                    None if key.starts_with('$') => {}
                    // An explicit null carries no value to lose, and a
                    // field whose value is nothing is written back as
                    // nothing.
                    None if value.is_null() => {}
                    None => out.push(here),
                }
            }
        }
        (Json::Array(given), Json::Array(understood)) => {
            for (i, (value, kept)) in given.iter().zip(understood).enumerate() {
                ignored_fields(value, kept, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// What is wrong with how a variable is read, if anything.
fn reading_problem(reading: &Reading) -> Option<&'static str> {
    if reading.variable.is_empty() {
        return Some("needs a variable");
    }
    if reading.threshold.is_some_and(|t| !t.is_finite()) {
        return Some("needs a finite threshold");
    }
    if reading.debounce.is_some_and(|d| !d.is_finite() || d < 0.0) {
        return Some("needs a debounce of 0 or more");
    }
    if reading.curve.is_empty() {
        return None;
    }
    if reading.threshold.is_some() {
        // A threshold is a curve of two keys written short, so doing
        // both says nothing clear about which happens first.
        return Some("sets both curve and threshold, which are the same job");
    }
    if !reading
        .curve
        .iter()
        .all(|k| k.t.is_finite() && k.v.is_finite())
    {
        return Some("needs finite curve keys");
    }
    if !reading.curve.windows(2).all(|w| w[0].t <= w[1].t) {
        return Some("needs its curve keys in order of input");
    }
    None
}

/// The reading at `site`, from the loaded show.
fn reading_at<'a>(show: &'a Show, (root, path, reader): &ReadSite) -> Option<&'a Reading> {
    let layer = layer_at(root_layers(show, *root)?, path)?;
    match reader {
        Reader::Binding(i) => layer.bindings.get(*i).map(|b| &b.reading),
        Reader::When(i) => layer.timelines.get(*i)?.when.as_ref(),
        Reader::While(i) => layer.timelines.get(*i)?.whilst.as_ref(),
        Reader::MediaWhen => layer.kind.media()?.when,
        Reader::MediaWhile => layer.kind.media()?.whilst,
    }
}

/// The layer at `path` under `layers`: each step an index into the
/// children of the one before.
pub fn layer_at<'a>(layers: &'a [Layer], path: &[usize]) -> Option<&'a Layer> {
    let (&first, rest) = path.split_first()?;
    let layer = layers.get(first)?;
    if rest.is_empty() {
        return Some(layer);
    }
    layer_at(layer.children(), rest)
}

fn collect_timelines(
    layers: &[Layer],
    path: &mut Vec<usize>,
    f: &mut impl FnMut(&[usize], usize, &Timeline),
) {
    for (i, layer) in layers.iter().enumerate() {
        path.push(i);
        for (idx, tl) in layer.timelines.iter().enumerate() {
            f(path, idx, tl);
        }
        collect_timelines(layer.children(), path, f);
        path.pop();
    }
}

/// The bus a layer's sound is on: the one it names, or [`MAIN_BUS`].
fn effective_bus(bus: &Option<String>) -> &str {
    bus.as_deref().unwrap_or(crate::model::MAIN_BUS)
}
