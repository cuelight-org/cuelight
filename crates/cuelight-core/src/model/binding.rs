//! Readings and bindings: what a property or a condition reads, and the
//! transitions that ease a bound value to where it is going.

use super::*;

/// A value read from a variable: the one shape everything that reads a
/// variable uses, so a binding, a timeline's `when` and `while`, and
/// whatever reads a variable next take the same fields and read them in
/// the same order.
///
/// The order is `debounce`, then `map` (with `default`), then
/// `threshold` or `curve`. What comes out is a value: a binding goes on
/// to `scale`, `offset` and its `transition`; a condition takes it as
/// true when it is not 0.
///
/// The name is a host variable, or failing that a value the show
/// animates itself. Without a value of either kind, or with a `map` that
/// does not list the value and no `default`, the reading has nothing to
/// say: a binding then leaves its property as it was, a condition is
/// false.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Reading {
    pub variable: String,
    /// Replace the variable's value (as text: `1`, `2.5`, `true`,
    /// `attract`) by looking it up here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<BTreeMap<String, Value>>,
    /// Value for variable values `map` does not list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// A level: the value becomes 1 at or above it and 0 below. A lamp
    /// that lights when a brightness passes a half, a `visible` that
    /// follows a level, a condition that holds from a mark on.
    ///
    /// Shorthand for a `curve` of two keys with a `step` ease, and read
    /// as exactly that curve (see [`Reading::bend`]), which is the rule
    /// for any shorthand the format has: it is defined as the longer
    /// form it stands for, so the two cannot drift apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Seconds a new value has to hold before it is read; changes
    /// shorter than that (a strobing lamp, a bouncing switch) are never
    /// seen. When a show loads or a scene is entered the value applies
    /// at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce: Option<f64>,
    /// Bend the value against the input instead of taking it straight.
    ///
    /// Keys are a track's, with the input value where a track has time,
    /// and the same easings between them: below the first key it holds
    /// the first value, above the last it holds the last. A lamp whose
    /// glow wants a gamma curve, a tachometer compressed at the low end,
    /// a loudness in decibels rather than a linear gain.
    ///
    /// This shapes value against input; a [`Transition`]'s `ease` shapes
    /// a change over time. A binding can have both, and they do different
    /// things.
    ///
    /// It applies after `map`, so the curve is written in the variable's
    /// own units and a binding's `scale` stays the last change of unit.
    /// It cannot be combined with `threshold`, which is this curve
    /// written short.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<Key>,
}

impl Reading {
    /// What `map` and `default` make of `value`: the value itself
    /// without a map, else the mapped value, else the default. `None`
    /// when the map does not list it and there is no default.
    pub fn mapped(&self, value: Value) -> Option<Value> {
        match &self.map {
            None => Some(value),
            Some(map) => map.get(&value.to_text()).or(self.default.as_ref()).cloned(),
        }
    }

    /// The curve the reading bends its input through: `curve`, or the
    /// two keys `threshold` stands for. Empty without either.
    pub fn curve_in_effect(&self) -> std::borrow::Cow<'_, [Key]> {
        match self.threshold {
            // 0 up to the level, 1 from it on. Below its first key a
            // curve holds that key's value, so where the first key sits
            // does not matter as long as it is below the level.
            Some(level) => std::borrow::Cow::Owned(vec![
                Key {
                    t: level - 1.0,
                    v: 0.0,
                    ease: Easing::Linear,
                },
                Key {
                    t: level,
                    v: 1.0,
                    ease: Easing::Step,
                },
            ]),
            None => std::borrow::Cow::Borrowed(&self.curve),
        }
    }

    /// `n` bent against the input through
    /// [`curve_in_effect`](Reading::curve_in_effect); `n` itself without
    /// a curve.
    pub fn bend(&self, n: f64) -> f64 {
        sample_keys(&self.curve_in_effect(), n).unwrap_or(n)
    }
}

/// A condition on a variable that starts something when it becomes true.
///
/// A host that only sends states - lamps going on and off, a score
/// crossing a mark, a mode taking a value - has no trigger to fire, and
/// without this every such host has to watch its own variables and invent
/// trigger names for them, which is show logic living outside the show.
///
/// A [`Reading`], true when what it reads is not 0: `{ "variable":
/// "mode", "map": { "multiball": 1 } }` is true exactly in that mode, and
/// a `debounce` keeps a flickering lamp from starting the timeline over.
/// What starts the timeline is *becoming* true, so a lamp that stays on
/// plays it once rather than every frame.
pub type When = Reading;

/// A condition a timeline runs under, rather than starts on.
///
/// Read exactly as a [`When`] is; the difference is what it does with the
/// answer. See [`Timeline::whilst`].
pub type While = Reading;

/// A permanent wiring of a property to a variable, evaluated every frame.
///
/// The variable is read as a [`Reading`] (its fields sit directly on the
/// binding), then numeric properties take `value * scale + offset`. The
/// `text` property takes the value as text: numbers get `scale`/`offset`
/// applied, then `format`. The `font` property takes the value as a
/// font style name. A reading with nothing to say leaves the property at
/// its base value. The order is: `debounce`, `map`, `threshold` or
/// `curve`, `scale` and `offset`, `transition`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Binding {
    pub property: Property,
    /// How the variable is read; see [`Reading`].
    #[serde(flatten)]
    pub reading: Reading,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default)]
    pub offset: f64,
    /// How many decimal places a number shows, when it becomes text.
    ///
    /// Without it a number prints as short as it can, so a value a
    /// timeline is moving reads `1.4833333333333334` between its keys.
    /// With it the number is rounded to that many places and always
    /// shows them: `1.5` with two is `1.50`.
    ///
    /// Applies after `scale` and `offset`, as the rest of formatting
    /// does, and alongside `format`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u32>,
    /// At least this many digits before the point, zero-filled on the
    /// left (text bindings only): a page `03`, a clock's `09`. A minimum,
    /// never a cut; the sign comes before the zeros, and with `thousands`
    /// the zeros are grouped like any digits (`0,005`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_digits: Option<u32>,
    /// How a number becomes text (text bindings only).
    #[serde(default)]
    pub format: NumberFormat,
    /// Words in front of the value, and behind it (text bindings only).
    ///
    /// A readout is rarely a bare number: `40%`, `BALL 2`, `2.5 X`,
    /// `LEVEL 12`. Putting them in a layer of their own beside the number
    /// only holds while nothing is centred or right-aligned, since the
    /// number's width changes and the words do not move with it.
    ///
    /// They apply last, to whatever text the binding produces, so a
    /// counting `transition` counts the number and leaves the words
    /// still, and a `map`'s text gets them as much as a number does. A
    /// binding that does not apply (an unset variable, a `map` with
    /// nothing to say and no `default`) leaves the property as it was,
    /// words included.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    /// Words behind the value; see `prefix`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    /// Ease toward a new value instead of jumping to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
}

impl Binding {
    /// A number after the reading's `threshold` or `curve`, then `scale`
    /// and `offset`: what a numeric property takes from a value.
    pub fn scaled(&self, n: f64) -> f64 {
        // Bent against the input before any change of unit, so the curve
        // is written in whatever the variable counts in.
        self.reading.bend(n) * self.scale + self.offset
    }

    /// `text` with the binding's `prefix` and `suffix` round it.
    pub fn worded(&self, text: String) -> String {
        match (self.prefix.is_empty(), self.suffix.is_empty()) {
            (true, true) => text,
            _ => format!("{}{text}{}", self.prefix, self.suffix),
        }
    }

    /// What the property gets for a value the reading produced: a number
    /// through [`scaled`](Binding::scaled), text through `format` and
    /// the words, a name as it is. `None` when the property cannot use
    /// the value, which leaves it as it was: a tint that is not a color,
    /// a font style `show` does not declare.
    /// A number as this binding shows it as text: `format`, `decimals`
    /// and `min_digits`, before the words.
    pub fn number_text(&self, n: f64) -> String {
        self.format.format_padded(n, self.decimals, self.min_digits)
    }

    pub fn convert(&self, value: Value, show: &Show) -> Option<Value> {
        match self.property {
            Property::Text => Some(Value::Text(self.worded(match value {
                Value::Number(n) => self.number_text(self.scaled(n)),
                other => other.to_text(),
            }))),
            Property::Visible => Some(Value::Bool(self.scaled(value.as_number()) != 0.0)),
            // Only real colors apply, as only declared styles do.
            Property::Tint => match value {
                Value::Text(color) if color.is_empty() || parse_color(&color).is_some() => {
                    Some(Value::Text(color))
                }
                _ => None,
            },
            // Any name will do: a video nobody registered simply has no
            // frames, as an unregistered image has no pixels.
            Property::Video | Property::Sound | Property::Image => {
                Some(Value::Text(value.to_text()))
            }
            // Only declared font styles apply.
            Property::Font => match value {
                Value::Text(style) if show.fonts.contains_key(&style) => Some(Value::Text(style)),
                _ => None,
            },
            _ => Some(Value::Number(self.scaled(value.as_number()))),
        }
    }

    /// The binding's reading of `value` one stage at a time, with what
    /// each stage made of it: what an editor shows beside the pipeline.
    /// The same functions the engine applies, in the same order, so the
    /// two cannot disagree. `debounce` and `transition` happen over time
    /// and are not stages of a value; see
    /// [`Transition::step_response`] for the latter.
    pub fn stages(&self, value: Value, show: &Show) -> Stages {
        let mapped = self.reading.mapped(value.clone());
        // Where the property takes a number, the stages between.
        let takes_number = match self.property {
            Property::Text => matches!(mapped, Some(Value::Number(_))),
            Property::Tint
            | Property::Video
            | Property::Sound
            | Property::Image
            | Property::Font => false,
            _ => true,
        };
        let bent = mapped
            .as_ref()
            .filter(|_| takes_number)
            .map(|value| self.reading.bend(value.as_number()));
        let scaled = bent.map(|n| n * self.scale + self.offset);
        let output = mapped.clone().and_then(|value| self.convert(value, show));
        Stages {
            read: value,
            mapped,
            bent,
            scaled,
            output,
        }
    }
}

/// A binding's reading of one value, stage by stage; see
/// [`Binding::stages`].
#[derive(Debug, Clone, PartialEq)]
pub struct Stages {
    /// The value read, before anything.
    pub read: Value,
    /// After `map` and `default`. `None` when the map does not list the
    /// value and there is no default, where the binding stops.
    pub mapped: Option<Value>,
    /// After `threshold` or `curve`, where the property takes a number.
    pub bent: Option<f64>,
    /// After `scale` and `offset`.
    pub scaled: Option<f64>,
    /// What the property gets; `None` when it cannot use the value.
    pub output: Option<Value>,
}

/// How a bound property moves when its binding's value changes: from the
/// value it has now to the new one over `duration` seconds.
///
/// What is eased is the binding's output (after `map`, `scale` and
/// `offset`), so it has to be a number: on a `text` binding numbers count
/// up or down before they are formatted (whole numbers stay whole on the
/// way), any other text jumps. A change while a transition runs starts a
/// new one from the value reached so far. When a show loads or a scene is
/// entered, properties start at their value; nothing eases in.
///
/// With `wrap` the value lives on a ring of that size (360 for an angle,
/// 10 for a sheet with a frame per digit) and `direction` picks the way
/// round.
///
/// A transition is a small timeline played on every change. `duration`
/// and `ease` say how a move progresses; `offset` adds keyed motion on top,
/// in the value's own units, so it is the same size however far the move
/// goes (a reel settling against its stop); `step` plays a large change as
/// several moves in a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Transition {
    /// Seconds a change takes; above 0. Not used, and not needed, with a
    /// `model`, which decides its own timing.
    #[serde(default)]
    pub duration: f64,
    /// Follow a physical model instead of easing over a duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// Temperature of the filament at full power, in kelvin, which is
    /// also the colour it glows there. 2700 by default, a warm white; a
    /// bigger lamp runs hotter and whiter, a small one cooler and redder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kelvin: Option<f64>,
    /// How quickly the filament heats, in seconds: the time it takes to
    /// close about two thirds of the gap to where it is heading. Full
    /// brightness takes roughly five times this. 0.007 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating: Option<f64>,
    /// The same going the other way, and always the slower of the two: a
    /// filament loses heat more slowly than the power puts it in. 0.06 by
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling: Option<f64>,
    #[serde(default)]
    pub ease: Easing,
    /// Size of the ring the value lives on; above 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<f64>,
    /// Which way round a wrapped value goes; needs `wrap`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// Size of one move, above 0: a larger change plays as several moves in
    /// a row, each with its own `duration`, `ease` and `offset`, the last
    /// one shorter when the change is no whole number of steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Motion added on top of a move, along its direction of travel: keys
    /// like a timeline track's, `t` in seconds from the start of the move,
    /// `v` in the value's units. Has to start and end at 0. A move lasts as
    /// long as the longer of `duration` and these keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offset: Vec<Key>,
}

/// Which way round a wrapped [`Transition`] goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Direction {
    /// Whichever way is shorter; forward on a tie.
    #[default]
    Shortest,
    /// Always toward higher values: 9 to 0 rolls on through the wrap.
    Forward,
    /// Always toward lower values.
    Backward,
}

impl Transition {
    /// Seconds one move takes: `duration`, or the `offset` when it runs
    /// longer.
    pub fn move_time(&self) -> f64 {
        let offset_end = self.offset.last().map_or(0.0, |k| k.t);
        self.duration.max(offset_end)
    }

    /// The value `elapsed` seconds after the binding's input stepped from
    /// `from` to `to`, having rested at `from` before: eased there over
    /// the transition's timing, or for a modelled transition the light
    /// of a filament driven from one power to the other. What an editor
    /// draws as the transition's response.
    pub fn step_response(&self, from: f64, to: f64, elapsed: f64) -> f64 {
        match self.model {
            Some(Model::Incandescent) => {
                let lamp = crate::lamp::Filament::of(self);
                let start = crate::lamp::settled(lamp, from);
                crate::lamp::shown(lamp, crate::lamp::temperature(lamp, start, to, elapsed))
            }
            None => self.value_at(from, to, elapsed),
        }
    }

    /// The value `elapsed` seconds into a change from `start` to `target`.
    pub fn value_at(&self, start: f64, target: f64, elapsed: f64) -> f64 {
        let on_ring = |v: f64| self.wrap.map_or(v, |wrap| v.rem_euclid(wrap));
        let delta = match self.wrap {
            None => target - start,
            Some(wrap) => {
                let forward = (target - start).rem_euclid(wrap);
                match self.direction.unwrap_or_default() {
                    Direction::Shortest if forward > wrap / 2.0 => forward - wrap,
                    Direction::Shortest | Direction::Forward => forward,
                    Direction::Backward if forward == 0.0 => 0.0,
                    Direction::Backward => forward - wrap,
                }
            }
        };
        let distance = delta.abs();
        // One move for the whole change, unless `step` divides it.
        let unit = self
            .step
            .filter(|step| *step < distance)
            .unwrap_or(distance);
        // Tolerant of 3.0000000001 steps being three.
        let moves = if unit > 0.0 {
            (distance / unit - 1e-9).ceil().max(1.0)
        } else {
            1.0
        };
        let period = self.move_time();
        // Decided by time, not by progress: an ease that overshoots passes
        // 1 on the way. Exactly the target, not a rounding step from it.
        if distance == 0.0 || elapsed >= moves * period {
            return on_ring(target);
        }
        let index = (elapsed.max(0.0) / period).floor().min(moves - 1.0);
        let local = elapsed.max(0.0) - index * period;
        let covered = index * unit;
        let length = (distance - covered).min(unit);
        let along = covered + length * self.ease.apply(local / self.duration);
        let offset = sample_keys(&self.offset, local).unwrap_or(0.0);
        on_ring(start + delta.signum() * (along + offset))
    }
}
