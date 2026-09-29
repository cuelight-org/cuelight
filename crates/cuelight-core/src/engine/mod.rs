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

mod bindings;
mod conditions;
mod media;
mod reels;
mod tolerant;
mod trace;
mod validate;

use bindings::*;
use conditions::*;
pub use media::*;
pub use reels::*;
pub use tolerant::*;
pub use trace::*;
pub(crate) use validate::*;

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
    carry: bool,
    hold: bool,
    on_end: Option<&'a str>,
}

impl<'a> From<&'a Timeline> for Timing<'a> {
    fn from(tl: &'a Timeline) -> Self {
        Timing {
            duration: tl.duration(),
            play_time: tl.play_time(),
            looping: tl.looping,
            carry: tl.carry,
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
            carry: tl.carry,
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

    /// How many whole passes a carried loop has made at show time `now`:
    /// what each track's change over one pass is added that many times.
    /// Nought for anything else.
    fn passes(&self, now: f64, tl: Timing<'_>) -> f64 {
        let elapsed = now - self.starts;
        if self.held || !tl.looping || !tl.carry || tl.duration <= 0.0 || elapsed <= 0.0 {
            return 0.0;
        }
        (elapsed / tl.duration).floor()
    }

    /// The instant it finishes, for a timeline that does finish.
    fn ends(&self, tl: Timing<'_>) -> f64 {
        self.starts + tl.play_time
    }
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
                    let carried = p.passes(now, tl.into()) * crate::model::per_pass(&tl.keys);
                    out = Some(v + carried);
                }
            }
        }
        out
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

    /// Whether shapes are drawn with hard edges: on the canvas's own
    /// pixel grid ([`pixel_grid`](Engine::pixel_grid)), with `edges` set to
    /// `hard` in the active output.
    pub fn hard_edges(&self) -> bool {
        self.pixel_grid()
            && self.effective_output().edges.unwrap_or_default() == crate::model::Edges::Hard
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
            let Some(s) = self.sounding.get(i) else {
                continue;
            };
            let (root, path) = (s.root, s.layer_path.clone());
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
                    let Some(play) = self.sounding.get_mut(i) else {
                        continue;
                    };
                    let (was, old_id) = (play.playing.clone(), play.id);
                    self.next_voice += 1;
                    let id = self.next_voice;
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
                if let Some(playhead) = self.playing.get_mut(i) {
                    playhead.held = true;
                }
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
            let Some(s) = self.sounding.get(*i) else {
                continue;
            };
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
}

/// The layers of one tree of the show; `None` for a scene it does not
/// have.
pub fn root_layers(show: &Show, root: Root) -> Option<&[Layer]> {
    match root {
        Root::Show => Some(&show.layers),
        Root::Scene(i) => show.scenes.get(i).map(|s| s.layers.as_slice()),
    }
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
