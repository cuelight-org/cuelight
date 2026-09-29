//! Conditions: timelines that start when a variable reads a certain way,
//! and debounced inputs that must hold before they count.

use super::*;

/// The timeline conditions that read a value the show animates, each
/// with the tree it is in.
pub(super) fn value_conditions(show: &Show) -> Vec<(Root, Reading)> {
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

impl Engine {
    /// Whether `condition`, reading a value the show animates, holds at
    /// the instant `at`.
    pub(super) fn condition_at(&self, condition: &Reading, at: f64) -> bool {
        self.show_value_at(&condition.variable, at)
            .and_then(|n| condition.mapped(Value::Number(n)))
            .is_some_and(|value| condition.bend(value.as_number()) != 0.0)
    }

    /// Start every timeline whose `when` has just become true.
    ///
    /// The edge is what starts it, not the condition holding: a lamp that
    /// stays on plays its animation once. A condition that is already
    /// true when the show loads or a scene is entered counts as an edge,
    /// the same way a host firing a trigger at that moment would.
    pub(super) fn follow_conditions(&mut self) {
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

    /// Whether the condition at `site` reads as true right now: what it
    /// reads, bent through its threshold or curve, is not 0. A reading
    /// with nothing to say is false.
    pub(super) fn holds(&self, site: &ReadSite, condition: &Reading) -> bool {
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
    pub(super) fn start_matching(
        &mut self,
        root: Option<Root>,
        at: f64,
        want: Want<'_>,
        cause: &Cause,
    ) {
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

    /// Let every reading with a debounce take in its variable: a new value
    /// becomes the candidate, and a candidate that will have held for the
    /// debounce time by `to`, where this step lands, settles, so it shows
    /// in the frame the hold runs out. A first look settles at once.
    pub(super) fn settle_debounces(&mut self, to: f64) {
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
}
