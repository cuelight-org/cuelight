//! Driver scripts: triggers and variable changes with delays, standing in
//! for a live host so a show can be demonstrated and tested repeatably.
//!
//! ```json
//! {
//!   "loop": true,
//!   "steps": [
//!     { "set": { "score": 0 } },
//!     { "wait": 0.5 },
//!     { "trigger": "go" }
//!   ]
//! }
//! ```

use crate::LoadError;
use cuelight_core::{Engine, Value, SAME_INSTANT};
use std::collections::BTreeMap;
use std::path::Path;

/// A scripted command sequence.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Driver {
    /// Start over after the last step.
    #[serde(default, rename = "loop")]
    pub looping: bool,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// One driver step: exactly one of `wait`, `trigger` or `set`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Step {
    /// Let this many seconds pass.
    Wait { wait: f64 },
    /// Fire a trigger.
    Trigger { trigger: String },
    /// Set variables.
    Set { set: BTreeMap<String, Value> },
}

// Described by hand: an untagged enum generates `anyOf` with nothing
// closed, so an editor would not flag a misspelt `trigge`, nor a step
// naming two of the three. A step is exactly one of them.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for Step {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Step".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::Step").into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let closed = |name: &str, shape: serde_json::Value| {
            serde_json::json!({
                "type": "object",
                "properties": { name: shape },
                "required": [name],
                "additionalProperties": false
            })
        };
        schemars::Schema::try_from(serde_json::json!({
            "description": "One driver step: exactly one of wait, trigger or set.",
            "oneOf": [
                closed("wait", serde_json::json!({
                    "description": "Let this many seconds pass.",
                    "type": "number",
                    "format": "double"
                })),
                closed("trigger", serde_json::json!({
                    "description": "Fire a trigger.",
                    "type": "string"
                })),
                closed("set", serde_json::json!({
                    "description": "Set variables.",
                    "type": "object",
                    "additionalProperties": true
                })),
            ]
        }))
        .expect("a schema built from an object literal")
    }
}

impl Driver {
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, LoadError> {
        let path = path.as_ref();
        let json = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_json(&json).map_err(|message| LoadError::Driver {
            path: path.to_owned(),
            message,
        })
    }

    /// Seconds one pass through the steps takes.
    pub fn duration(&self) -> f64 {
        self.steps
            .iter()
            .map(|s| match s {
                Step::Wait { wait } => wait.max(0.0),
                _ => 0.0,
            })
            .sum()
    }
}

/// Plays a [`Driver`] against an engine as time advances.
#[derive(Debug, Clone)]
pub struct DriverPlayer {
    driver: Driver,
    index: usize,
    wait_left: f64,
    done: bool,
}

impl DriverPlayer {
    pub fn new(driver: Driver) -> Self {
        Self {
            driver,
            index: 0,
            wait_left: 0.0,
            done: false,
        }
    }

    /// Whether the script ran to its end (never, while it loops).
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Advance the script by `dt` seconds from where the engine's clock
    /// is: every step whose wait ends inside that span is applied at its
    /// own instant, with the engine moved there first, so a step lands
    /// at the same instant whatever the frame rate. The engine is left
    /// at the last step's instant; call `Engine::advance_to` with the
    /// end of the frame afterwards, as for any frame. Returns the steps
    /// that were applied, each with its instant, for hosts that log
    /// them.
    pub fn advance(&mut self, engine: &mut Engine, dt: f64) -> Vec<Applied> {
        let start = engine.time();
        let end = start + dt.max(0.0);
        let mut applied = Vec::new();
        // A looping script without any wait would never yield.
        let mut wrapped = false;
        // Where the script has got to on the show's clock.
        let mut at = start;
        while !self.done {
            if self.wait_left > 0.0 {
                // A wait that runs past this frame keeps what is left
                // of it. The same instant as the frame's end counts as
                // inside it, as everywhere on the clock.
                if at + self.wait_left > end + SAME_INSTANT {
                    self.wait_left -= (end - at).max(0.0);
                    break;
                }
                at += self.wait_left;
                self.wait_left = 0.0;
            }
            if self.index >= self.driver.steps.len() {
                if !self.driver.looping || self.driver.steps.is_empty() || wrapped {
                    self.done = true;
                    break;
                }
                wrapped = true;
                self.index = 0;
            }
            let step = self.driver.steps[self.index].clone();
            self.index += 1;
            match &step {
                Step::Wait { wait } => {
                    self.wait_left = wait.max(0.0);
                    if self.wait_left > 0.0 {
                        wrapped = false;
                    }
                    continue;
                }
                Step::Trigger { trigger } => {
                    engine.advance_to(at.min(end));
                    engine.trigger(trigger);
                }
                Step::Set { set } => {
                    engine.advance_to(at.min(end));
                    for (name, value) in set {
                        engine.set_variable(name, value.clone());
                    }
                }
            }
            applied.push(Applied {
                at: at.min(end),
                step,
            });
        }
        applied
    }
}

/// A step a [`DriverPlayer`] applied, and the instant on the show's
/// clock it applied it at.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub at: f64,
    pub step: Step,
}

/// One thing a host told a show while it played.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveInput {
    /// A trigger fired: what a key or a press meant.
    Trigger(String),
    /// A variable set to a value.
    Set(String, Value),
}

impl LiveInput {
    /// Tell it to `engine` again.
    fn apply(&self, engine: &mut Engine) {
        match self {
            LiveInput::Trigger(name) => engine.trigger(name),
            LiveInput::Set(name, value) => engine.set_variable(name, value.clone()),
        }
    }
}

/// What a host told a show while it played, kept so seeking can put it
/// back: the trigger each key and press fired, and each variable set by
/// hand, on the show's own clock.
///
/// A show is a function of its inputs and the clock, which is what makes
/// scrubbing a matter of replaying rather than rewinding. Live input is
/// an input like a script's, so it has to be replayed too, or a show
/// scrubbed back would lose everything anyone did to it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    inputs: Vec<(f64, LiveInput)>,
}

impl Live {
    /// Remember that `trigger` was fired at `at` seconds.
    pub fn record(&mut self, at: f64, trigger: impl Into<String>) {
        self.insert(at, LiveInput::Trigger(trigger.into()));
    }

    /// Remember that variable `name` was set to `value` at `at` seconds.
    pub fn record_set(&mut self, at: f64, name: impl Into<String>, value: impl Into<Value>) {
        self.insert(at, LiveInput::Set(name.into(), value.into()));
    }

    /// Kept in time order: a host records as it plays, and a seek does
    /// not disturb what it recorded, so scrubbing back and forward again
    /// finds the same show both times.
    fn insert(&mut self, at: f64, input: LiveInput) {
        let at = at.max(0.0);
        let after = self.inputs.iter().rposition(|(when, _)| *when <= at);
        let index = after.map_or(0, |i| i + 1);
        self.inputs.insert(index, (at, input));
    }

    /// Whether the show has been told anything live.
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// Everything the host did, in time order.
    pub fn inputs(&self) -> impl Iterator<Item = (f64, &LiveInput)> {
        self.inputs.iter().map(|(at, input)| (*at, input))
    }

    /// Forget everything from `at` on: what a host calls when a take
    /// starts again from there and the rest should not come back.
    pub fn forget_from(&mut self, at: f64) {
        self.inputs.retain(|(when, _)| *when < at);
    }
}

/// Put the show back to its beginning and walk it to `to` seconds,
/// replaying `driver` and `live` on the way, each input at the instant
/// it belongs to. Hands back the driver where it ended up, so playing on
/// from there continues.
///
/// A show's state is a function of its inputs and the clock, so reaching
/// a moment is restarting and advancing to it. Nothing is stored, nothing
/// is rewound, and a host can scrub by calling this as the pointer moves.
/// The clock jumps from one input to the next rather than walking in
/// frames, since a frame is cut wherever something happens anyway: a
/// show of minutes takes well under a millisecond, and lands exactly
/// where playing it at any frame rate would.
pub fn seek(
    engine: &mut Engine,
    driver: Option<Driver>,
    live: &Live,
    to: f64,
) -> Option<DriverPlayer> {
    engine.restart();
    let to = to.max(0.0);
    let mut player = driver.map(DriverPlayer::new);
    for (at, input) in live.inputs().take_while(|(at, _)| *at <= to) {
        // The script's steps due by then, each at its own instant, then
        // the clock to the instant the host acted, then what it did.
        if let Some(player) = &mut player {
            player.advance(engine, at - engine.time());
        }
        engine.advance_to(at);
        input.apply(engine);
    }
    if let Some(player) = &mut player {
        player.advance(engine, to - engine.time());
    }
    engine.advance_to(to);
    player
}
