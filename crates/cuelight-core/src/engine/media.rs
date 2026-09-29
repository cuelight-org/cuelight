//! Sound and video: what a layer plays, who is heard, and the buses
//! that duck under one another.

use super::*;

/// One play of an audio layer, from its trigger until it ends or is
/// stopped.
#[derive(Debug, Clone)]
pub(super) struct Sounding {
    pub(super) root: Root,
    pub(super) layer_path: Vec<usize>,
    /// Engine-unique, so a backend can tell one play from the next.
    pub(super) id: u64,
    /// Engine time it was triggered at; the delay counts from here.
    pub(super) started: f64,
    /// What it is playing. A video layer's name can be bound, and
    /// pointing it at another clip starts that one from the top.
    pub(super) playing: String,
}

/// Where a ducking layer's level is and when it started going there.
#[derive(Debug, Clone, Copy)]
pub(super) struct Ducked {
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

/// What is sounding on each bus, by the layer playing it and the instant
/// that play started sounding.
type BusyBuses = std::collections::BTreeMap<String, Vec<((Root, Vec<usize>), f64)>>;

/// What a playhead has played so far.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Played {
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
        Pick::Shuffle => scramble(names.len(), seed, ordinal / count)
            .get((ordinal % count) as usize)
            .copied()
            .unwrap_or(0),
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

/// The bus a layer's sound is on: the one it names, or [`MAIN_BUS`].
fn effective_bus(bus: &Option<String>) -> &str {
    bus.as_deref().unwrap_or(crate::model::MAIN_BUS)
}

impl Engine {
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

    /// What is registered for video `name`.
    pub fn video(&self, name: &str) -> Option<VideoInfo> {
        self.videos.get(name).copied()
    }

    /// How long the content of a playhead runs, whichever registry it
    /// comes from; `None` while the host has not registered it.
    pub(super) fn media_duration(
        &self,
        media: &crate::model::Media<'_>,
        playing: &str,
    ) -> Option<f64> {
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

    /// The layers pointed at media they are not playing: idle ones whose
    /// bound name has changed since they last played.
    pub(super) fn repointed(&self) -> Vec<(Root, Vec<usize>)> {
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
    pub(super) fn retrigger_of(&self, root: Root, path: &[usize]) -> Retrigger {
        self.media_at(root, path)
            .map_or(Retrigger::Restart, |media| media.retrigger)
    }

    /// How many plays the playhead at `path` may hold at once.
    pub(super) fn voices_of(&self, root: Root, path: &[usize]) -> usize {
        self.media_at(root, path)
            .map_or(1, |media| media.voices.max(1) as usize)
    }

    /// The playhead of the layer at `path`, if it has one.
    pub(super) fn media_at(&self, root: Root, path: &[usize]) -> Option<crate::model::Media<'_>> {
        self.show
            .as_ref()
            .and_then(|show| root_layers(show, root))
            .and_then(|layers| layer_at(layers, path))
            .and_then(|layer| layer.kind.media())
    }

    /// Where the layer at `path` is pointed, by path alone.
    pub(super) fn pointed(&self, root: Root, path: &[usize]) -> Option<String> {
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
    pub(super) fn media_layers<T>(
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
    pub(super) fn play_autoplay(&mut self, root: Root, by: &Cause) {
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
    pub(super) fn play(&mut self, root: Root, path: Vec<usize>, at: f64, by: Cause) {
        self.start(root, path, None, at, by);
    }

    /// Start a play of the layer at `path`, of `asked` when the caller
    /// has already settled which asset it wants, `by` whatever asked.
    pub(super) fn start(
        &mut self,
        root: Root,
        path: Vec<usize>,
        asked: Option<String>,
        at: f64,
        by: Cause,
    ) {
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
    pub(super) fn play_ref(&self, root: Root, path: &[usize]) -> (LayerPath, String) {
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
    pub(super) fn end_plays(
        &mut self,
        at: f64,
        by: Ending,
        mut gone: impl FnMut(&Sounding) -> bool,
    ) {
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

    /// Note, for every layer that ducks, whether the bus it listens to is
    /// sounding now, and when that last changed.
    ///
    /// Only the change is remembered. What the level *is* at any moment is
    /// a function of that and of the time, which is what keeps it seekable:
    /// nothing here accumulates frame by frame.
    pub(super) fn follow_ducks(&mut self) {
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
    pub(super) fn follow_media_conditions(&mut self) {
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
}
