//! What a bound property reads, and the transitions that ease it there.

use super::*;

/// A binding with a transition: layer tree, layer path, binding index.
pub(super) type TransitionSite = (Root, Vec<usize>, usize);

/// Where a variable is read: layer tree, layer path, and which of the
/// layer's readers.
pub(super) type ReadSite = (Root, Vec<usize>, Reader);

/// One of the places on a layer that reads a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Reader {
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
pub(super) struct Settling {
    pub(super) settled: Value,
    pub(super) candidate: Value,
    /// Engine time the candidate first appeared.
    pub(super) since: f64,
}

/// A bound value on its way from `start` to `target` since engine time
/// `started`. The value in between is computed from this, never stepped,
/// so it does not depend on the frame rate.
#[derive(Debug, Clone, Copy)]
pub(super) struct Change {
    pub(super) start: f64,
    pub(super) target: f64,
    pub(super) started: f64,
    /// Whether this change runs between whole numbers, which is what
    /// decides if a counter shows whole numbers on the way.
    ///
    /// It has to be remembered rather than read off `start`, because a
    /// change that interrupts another starts from wherever the last one
    /// had got to, which is a fraction. What matters is the values the
    /// binding was given, not where the interruption happened to land.
    pub(super) whole: bool,
}

/// A color on its way to another one. The same shape as a [`Change`], and
/// eased by the same transition: one progress from 0 to 1 carries all four
/// channels, so they arrive together however the ease is shaped.
#[derive(Debug, Clone, Copy)]
pub(super) struct ColorChange {
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

/// The values a show animates that a binding eases from.
///
/// A transition follows what its binding reads, so the instant that
/// input changes is an instant the clock has to stop at. Only values
/// the show animates need it: a variable is set by the host, which
/// happens between frames anyway.
pub(super) fn eased_values(show: &Show) -> BTreeSet<String> {
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

/// Where the show's bindings with a transition are, and every reading
/// with a debounce, binding or condition.
pub(super) fn binding_sites(show: &Show) -> (Vec<TransitionSite>, Vec<ReadSite>) {
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

/// The reading at `site`, from the loaded show.
pub(super) fn reading_at<'a>(
    show: &'a Show,
    (root, path, reader): &ReadSite,
) -> Option<&'a Reading> {
    let layer = layer_at(root_layers(show, *root)?, path)?;
    match reader {
        Reader::Binding(i) => layer.bindings.get(*i).map(|b| &b.reading),
        Reader::When(i) => layer.timelines.get(*i)?.when.as_ref(),
        Reader::While(i) => layer.timelines.get(*i)?.whilst.as_ref(),
        Reader::MediaWhen => layer.kind.media()?.when,
        Reader::MediaWhile => layer.kind.media()?.whilst,
    }
}

impl Engine {
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

    /// Note where every binding with a transition is heading, as of now
    /// (inputs arrive between frames): a first look starts the property at
    /// its value, a new target starts a change from the value reached.
    pub(super) fn follow_transitions(&mut self) {
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
    pub(super) fn in_transition(
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
        Some(Value::Text(b.worded(b.number_text(n))))
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
                let passes = p.passes(self.time, tl.into());
                for track in tl.tracks.iter().filter(|t| t.property == prop) {
                    if let Some(sampled) = track.sample(time) {
                        v = Value::Number(sampled + passes * track.per_pass());
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
    pub(super) fn read(&self, site: &ReadSite, reading: &Reading) -> Option<Value> {
        let value = match (reading.debounce, self.debounced.get(site)) {
            (Some(_), Some(settling)) => settling.settled.clone(),
            // The show's own value when no host set one.
            _ => self.value(&reading.variable)?,
        };
        reading.mapped(value)
    }

    /// The value a binding at `site` feeds its property; see
    /// [`read`](Self::read).
    pub(super) fn binding_value(&self, site: &TransitionSite, b: &Binding) -> Option<Value> {
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
