//! Timelines: keys over time, and the tracks and show values they drive.

use super::*;

pub(super) fn default_scale() -> f64 {
    1.0
}

/// A keyframed animation over one or more properties of its layer.
///
/// A timeline runs when it is `autoplay` and the show loads, or when the
/// host fires its `trigger`. While running it owns the properties it
/// animates: timeline values override bindings, which override base values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Timeline {
    pub name: String,
    /// Trigger name, or list of names, that (re)starts this timeline.
    #[serde(default)]
    pub trigger: Triggers,
    /// A variable condition that (re)starts it on the rising edge, for
    /// hosts that send states rather than events. The edge belongs to the
    /// variable, not to the scene: leaving a scene and coming back does
    /// not replay it unless the condition turned true while away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
    /// A variable condition it runs under: it plays while the condition
    /// holds and stops when it stops holding.
    ///
    /// What `when` cannot say. A blink that means "this is lit" should
    /// run for as long as it is lit, and a looping timeline started on an
    /// edge would never stop. Unlike `when`, entering a scene starts it
    /// again, since it describes a state the scene is in rather than
    /// something that happened.
    ///
    /// Stopping is not finishing: it fires no `on_end`.
    #[serde(default, rename = "while", skip_serializing_if = "Option::is_none")]
    pub whilst: Option<While>,
    #[serde(default)]
    pub autoplay: bool,
    /// Repeat forever. Cannot be combined with `repeat`.
    #[serde(default, rename = "loop")]
    pub looping: bool,
    /// With `loop`, each pass carries on from where the last one ended
    /// rather than snapping back to the first key: pass `n` adds `n`
    /// times the change from its first key to its last. A wheel that
    /// turns 0 to 360 in a second, carried, keeps turning; so does a
    /// scroll. Still a function of the clock, so seeking holds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub carry: bool,
    /// Seconds to wait after starting before the first key plays; the
    /// timeline does not own its properties meanwhile. Loops and repeats
    /// do not wait again.
    #[serde(default)]
    pub delay: f64,
    /// Number of plays (fractions allowed: 2.5 stops halfway through the
    /// third); once when omitted.
    #[serde(default)]
    pub repeat: Option<f64>,
    /// Trigger fired when the timeline finishes (never for loops).
    #[serde(default)]
    pub on_end: Option<String>,
    /// Keep the last value instead of giving the properties back.
    ///
    /// A timeline that ends hands its properties back to their binding or
    /// base value, which is what a pulse or a flash wants. A fade *into* a
    /// state wants to stay there, and writing the end value into the base
    /// as well only works while nothing else ever animates that property.
    ///
    /// Held, a finished timeline keeps its properties at their last values
    /// until it is started again or its scene is left, and it ranks below
    /// any timeline still running, so a later flash on the same property
    /// wins while it plays and hands back to the held value afterwards. It
    /// means nothing on a loop, which never finishes.
    #[serde(default)]
    pub hold: bool,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Track {
    pub property: Property,
    pub keys: Vec<Key>,
}

/// A physical model a transition follows instead of an ease.
///
/// A lamp is not a fade. Much of how a panel of lamps looks is how they
/// switch, and a lamp switching is its filament's temperature chasing the
/// power put into it: full brightness in a few tens of milliseconds, most
/// of the light gone as fast when the power goes, then a dim red glow for
/// much longer. A filament re-lit while still warm comes up quicker than
/// a cold one, which no duration and ease can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Model {
    /// A glowing filament. The binding's value is the power put into it,
    /// 0 to 1; a numeric property gets the light that comes out and a
    /// `tint` gets the filament's colour, which reddens as it cools.
    ///
    /// Shaped by `kelvin`, `heating` and `cooling`, so it is any lamp
    /// with a filament rather than one make of bulb.
    Incandescent,
}

/// A keyframe: at time `t` (seconds) the property reaches value `v`,
/// approached with `ease` from the previous key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Key {
    pub t: f64,
    pub v: f64,
    #[serde(default)]
    pub ease: Easing,
}

/// Sample `keys` at `time`: the first value before the first key, the
/// last after the last, eased in between. `None` without keys. A track's
/// keys at a time, or a `curve`'s at an input: the one function under
/// both.
pub fn sample_keys(keys: &[Key], time: f64) -> Option<f64> {
    let first = keys.first()?;
    if time <= first.t {
        return Some(first.v);
    }
    let last = keys.last()?;
    if time >= last.t {
        return Some(last.v);
    }
    let next_idx = keys.iter().position(|k| k.t > time)?;
    let a = &keys[next_idx - 1];
    let b = &keys[next_idx];
    let span = b.t - a.t;
    let t = if span <= 0.0 {
        1.0
    } else {
        (time - a.t) / span
    };
    Some(a.v + (b.v - a.v) * b.ease.apply(t))
}

impl Track {
    /// Sample the track at `time` seconds from timeline start.
    pub fn sample(&self, time: f64) -> Option<f64> {
        sample_keys(&self.keys, time)
    }

    /// End time of the track's last key, 0.0 when empty.
    pub fn duration(&self) -> f64 {
        self.keys.last().map(|k| k.t).unwrap_or(0.0)
    }

    /// How far one pass moves the value: its last key's less its first.
    /// What a carried loop adds each time round.
    pub fn per_pass(&self) -> f64 {
        per_pass(&self.keys)
    }
}

/// How far `keys` move a value from the first to the last.
pub(crate) fn per_pass(keys: &[Key]) -> f64 {
    match (keys.first(), keys.last()) {
        (Some(first), Some(last)) => last.v - first.v,
        _ => 0.0,
    }
}

/// A value the show owns and animates, named and read like a variable.
///
/// A timeline animates a property of its own layer, so two layers that
/// must move together have to duplicate its keys, and nothing then says
/// they are meant to agree: a scene restarting one, or an edit to one set
/// of keys, parts them silently. A group shares a transform instead, but
/// it scales positions along with everything else, so it only works when
/// every reader sits at the group's origin.
///
/// A show value is the source both of them read. A host that sets a
/// variable of the same name takes it over, so a show can ship with its
/// own motion that a host is free to seize.
///
/// Nothing in a show writes one. Variables are the host's inputs, and
/// content writing them would make ownership ambiguous and allow a
/// variable that drives a timeline that writes that variable.
///
/// Timelines are the one kind of value so far. Whatever a show comes to
/// animate or decide itself next (a timer, a counter stepped by
/// triggers, a queue whose current item is read) is another kind of
/// value here, read and taken over the same way: the format grows by
/// kinds of value, never by a new top-level section per mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ShowValue {
    /// What moves it, played exactly as a layer's timelines are. Several
    /// of them cover one value in stretches, each started by its own
    /// trigger, the way a layer holds several timelines for one property.
    #[serde(default)]
    pub timelines: Vec<ValueTimeline>,
}

/// One stretch of a show value's motion: a timeline whose keys are the
/// value itself, so it has no tracks and names no property.
///
/// Everything above `keys` means what it means on a
/// [`Timeline`](crate::Timeline), and the two are kept in step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ValueTimeline {
    pub name: String,
    /// What starts it; firing any of several has the same effect.
    #[serde(default, skip_serializing_if = "Triggers::is_empty")]
    pub trigger: Triggers,
    /// Start it when the show loads.
    #[serde(default)]
    pub autoplay: bool,
    /// Repeat for ever; cannot be combined with `repeat`.
    #[serde(default, rename = "loop")]
    pub looping: bool,
    /// With `loop`, each pass carries on from where the last one ended
    /// rather than snapping back to the first key: pass `n` adds `n`
    /// times the change from its first key to its last. A wheel that
    /// turns 0 to 360 in a second, carried, keeps turning; so does a
    /// scroll. Still a function of the clock, so seeking holds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub carry: bool,
    /// Seconds to wait after starting before the first key.
    #[serde(default)]
    pub delay: f64,
    /// Play it this many times; fractions stop part way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<f64>,
    /// Trigger fired when it finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_end: Option<String>,
    /// Keep the last value instead of handing the value back.
    #[serde(default)]
    pub hold: bool,
    /// Keyframes of the value itself.
    pub keys: Vec<Key>,
}

impl ValueTimeline {
    /// One play, in seconds.
    pub fn duration(&self) -> f64 {
        self.keys.last().map(|k| k.t).unwrap_or(0.0)
    }

    /// Time after its delay at which a non-looping one finishes.
    pub fn play_time(&self) -> f64 {
        self.duration() * self.repeat.unwrap_or(1.0).max(0.0)
    }

    /// Where within one play the keys are sampled, `elapsed` seconds
    /// after the delay; `None` while still delayed.
    pub fn local_time(&self, elapsed: f64) -> Option<f64> {
        if elapsed < 0.0 {
            return None;
        }
        let duration = self.duration();
        if self.repeat.is_some() && duration > 0.0 && elapsed < self.play_time() {
            return Some(elapsed % duration);
        }
        Some(elapsed)
    }

    /// The value `elapsed` seconds after its delay.
    pub fn at(&self, elapsed: f64) -> Option<f64> {
        sample_keys(&self.keys, self.local_time(elapsed)?)
    }
}

impl Timeline {
    /// Duration of the longest track: one play.
    pub fn duration(&self) -> f64 {
        self.tracks
            .iter()
            .map(Track::duration)
            .fold(0.0_f64, f64::max)
    }

    /// Time after its delay at which a non-looping timeline finishes.
    pub fn play_time(&self) -> f64 {
        self.duration() * self.repeat.unwrap_or(1.0).max(0.0)
    }

    /// Where within one play the tracks are sampled, `elapsed` seconds
    /// after the delay; `None` while still delayed.
    pub fn local_time(&self, elapsed: f64) -> Option<f64> {
        if elapsed < 0.0 {
            return None;
        }
        let duration = self.duration();
        if self.repeat.is_some() && duration > 0.0 && elapsed < self.play_time() {
            return Some(elapsed % duration);
        }
        Some(elapsed)
    }
}
