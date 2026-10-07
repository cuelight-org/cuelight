use crate::sound::Sound;
use cuelight_core::Voice;
use std::collections::HashMap;
use std::sync::Arc;

/// Seconds a gain takes to move: keeps starts, stops and gain changes
/// from clicking.
const RAMP: f64 = 0.005;

/// How far a voice's distance from the position the engine reports may
/// change between two lists before it is moved there.
///
/// What is measured is the change, not the distance. A voice that simply
/// runs behind is normal and stays that way: the engine's position is a
/// frame and a device buffer old to begin with, and a device that was
/// asleep when the sound started can be most of a second behind for as
/// long as it plays. A seek is not that. A seek moves the position in one
/// step, so it shows up as the distance jumping, and only that is worth
/// throwing away the sound in hand for.
const RESYNC: f64 = 0.25;

/// One voice being played.
#[derive(Debug)]
struct Playing {
    id: u64,
    sound: Arc<Sound>,
    /// Position in the sound, in its own frames; fractional between two.
    cursor: f64,
    looping: bool,
    /// How far behind the engine's position this voice last was, so a
    /// seek can be told from a voice that is merely late.
    behind: f64,
    /// The gain applied now, on its way to `target`.
    gain: f32,
    target: f32,
    /// The pan applied now, on its way to `pan_target`, ramped as the
    /// gain is so that a moving sound does not click.
    pan: f32,
    pan_target: f32,
    /// Fading out, to be dropped once silent.
    ending: bool,
}

/// Mixes the engine's voice list into interleaved stereo samples at one
/// output rate: sounds are resampled linearly, gains ramp over a few
/// milliseconds, a voice that leaves the list fades out.
#[derive(Debug)]
pub struct Mixer {
    rate: u32,
    sounds: HashMap<String, Arc<Sound>>,
    playing: Vec<Playing>,
    /// Stereo sounds a show has panned, said once each.
    balanced: std::collections::HashSet<String>,
}

impl Mixer {
    /// A mixer producing samples at `rate` Hz.
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1),
            sounds: HashMap::new(),
            playing: Vec::new(),
            balanced: std::collections::HashSet::new(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Register (or replace) the samples of sound `name`, the one the
    /// engine knows by duration.
    pub fn set_sound(&mut self, name: &str, sound: Arc<Sound>) {
        self.sounds.insert(name.to_owned(), sound);
    }

    /// Make what plays match `voices`, the engine's list for this frame:
    /// a new id starts at its position, a missing id fades out, a gain
    /// change ramps, and a position that *jumped* away from the voice's is
    /// moved to. A voice that is merely running behind is left where it
    /// is: a device that was asleep when the sound started is late
    /// through no seek, and chasing the engine there would throw away the
    /// start of the sound. Voices of sounds without samples are left for
    /// a later call.
    pub fn apply(&mut self, voices: &[Voice]) {
        for voice in voices {
            let stereo = self
                .sounds
                .get(&voice.sound)
                .is_some_and(|s| s.channels >= 2);
            if stereo && voice.pan != 0.0 && self.balanced.insert(voice.sound.clone()) {
                log::warn!(
                    "sound {:?} is stereo, so its pan balances it rather than placing it; \
                     a sound that should move between the speakers is best mono",
                    voice.sound
                );
            }
            match self.playing.iter_mut().find(|p| p.id == voice.id) {
                Some(playing) => {
                    playing.target = voice.gain as f32;
                    playing.pan_target = voice.pan as f32;
                    playing.ending = false;
                    let rate = f64::from(playing.sound.rate);
                    let here = playing.cursor / rate;
                    let mut drift = voice.position - here;
                    if playing.looping {
                        // On a ring, the shorter way round.
                        let duration = playing.sound.duration();
                        drift = (drift + duration / 2.0).rem_euclid(duration) - duration / 2.0;
                    }
                    if (drift - playing.behind).abs() > RESYNC {
                        playing.cursor = voice.position * rate;
                        playing.behind = 0.0;
                    } else {
                        playing.behind = drift;
                    }
                }
                None => {
                    let Some(sound) = self.sounds.get(&voice.sound) else {
                        continue;
                    };
                    let cursor = voice.position * f64::from(sound.rate);
                    if !voice.looping && cursor >= sound.frames() as f64 {
                        continue;
                    }
                    self.playing.push(Playing {
                        id: voice.id,
                        sound: sound.clone(),
                        cursor,
                        looping: voice.looping,
                        behind: 0.0,
                        gain: 0.0,
                        target: voice.gain as f32,
                        pan: voice.pan as f32,
                        pan_target: voice.pan as f32,
                        ending: false,
                    });
                }
            }
        }
        for playing in &mut self.playing {
            if !voices.iter().any(|v| v.id == playing.id) {
                playing.ending = true;
                playing.target = 0.0;
            }
        }
    }

    /// Whether anything is sounding or fading out.
    pub fn is_active(&self) -> bool {
        !self.playing.is_empty()
    }

    /// Fill `out` (interleaved stereo, an even number of samples) with the
    /// next `out.len() / 2` frames, overwriting it.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let step_per_sample = (1.0 / (RAMP * f64::from(self.rate))) as f32;
        let mut done = Vec::new();
        for (index, playing) in self.playing.iter_mut().enumerate() {
            let sound = &playing.sound;
            let frames = sound.frames();
            let channels = usize::from(sound.channels);
            let advance = f64::from(sound.rate) / f64::from(self.rate);
            let last = (frames.max(1) - 1) as f64;
            for frame in out.as_chunks_mut::<2>().0 {
                // Ramp toward the wanted gain and pan.
                let delta = playing.target - playing.gain;
                playing.gain += delta.clamp(-step_per_sample, step_per_sample);
                let delta = playing.pan_target - playing.pan;
                playing.pan += delta.clamp(-step_per_sample, step_per_sample);
                if playing.ending && playing.gain <= 0.0 {
                    done.push(index);
                    break;
                }
                if playing.cursor >= frames as f64 {
                    if playing.looping && frames > 0 {
                        playing.cursor %= frames as f64;
                    } else {
                        done.push(index);
                        break;
                    }
                }
                // Linear interpolation between the two nearest frames.
                let a = playing.cursor.floor().min(last);
                let t = (playing.cursor - a) as f32;
                let b = if playing.looping {
                    (a + 1.0) % frames as f64
                } else {
                    (a + 1.0).min(last)
                };
                let (a, b) = (a as usize * channels, b as usize * channels);
                let sample = |offset: usize| {
                    let x = sound.samples.get(a + offset).copied().unwrap_or(0.0);
                    let y = sound.samples.get(b + offset).copied().unwrap_or(0.0);
                    (x + (y - x) * t) * playing.gain
                };
                let [left, right] = if channels >= 2 {
                    balance([sample(0), sample(1)], playing.pan)
                } else {
                    place(sample(0), playing.pan)
                };
                frame[0] += left;
                frame[1] += right;
                playing.cursor += advance;
            }
        }
        for index in done.into_iter().rev() {
            self.playing.remove(index);
        }
    }
}

/// A mono sample at `pan`, -1 to 1, as left and right: constant power,
/// so it is as loud wherever it is, and scaled so that in the middle it
/// is in both channels whole, as it is unpanned.
fn place(sample: f32, pan: f32) -> [f32; 2] {
    if pan == 0.0 {
        return [sample, sample];
    }
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let scale = std::f32::consts::SQRT_2 * sample;
    [scale * angle.cos(), scale * angle.sin()]
}

/// A stereo frame at `pan`, -1 to 1: the far side turned down and folded
/// into the near one, as a browser's stereo panner balances, so a show
/// sounds the same in both players. Unchanged at 0.
fn balance([left, right]: [f32; 2], pan: f32) -> [f32; 2] {
    if pan == 0.0 {
        return [left, right];
    }
    let pan = pan.clamp(-1.0, 1.0);
    if pan <= 0.0 {
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_2;
        [left + right * angle.cos(), right * angle.sin()]
    } else {
        let angle = pan * std::f32::consts::FRAC_PI_2;
        [left * angle.cos(), right + left * angle.sin()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, frames: usize) -> Arc<Sound> {
        Arc::new(Sound {
            rate,
            channels: 1,
            samples: (0..frames).map(|_| 0.5).collect(),
        })
    }

    fn voice(id: u64, position: f64, gain: f64, looping: bool) -> Voice {
        Voice {
            id,
            layer: "l".into(),
            sound: "tone".into(),
            position,
            gain,
            pan: 0.0,
            looping,
            bus: None,
        }
    }

    #[test]
    fn plays_ramps_and_stops() {
        let mut mixer = Mixer::new(1000);
        mixer.set_sound("tone", tone(1000, 100));
        mixer.apply(&[voice(1, 0.0, 1.0, false)]);
        let mut out = vec![0.0; 40];
        mixer.render(&mut out);
        // The ramp reaches full gain after 5 ms = 5 frames.
        assert!(out[0] < 0.5 && out[1] < 0.5);
        assert!((out[20] - 0.5).abs() < 1e-6 && (out[21] - 0.5).abs() < 1e-6);
        mixer.apply(&[]);
        mixer.render(&mut out);
        assert!(out[38].abs() < 1e-6);
        assert!(!mixer.is_active());
    }

    #[test]
    fn ends_with_the_sound() {
        let mut mixer = Mixer::new(1000);
        mixer.set_sound("tone", tone(1000, 10));
        mixer.apply(&[voice(1, 0.0, 1.0, false)]);
        let mut out = vec![0.0; 40];
        mixer.render(&mut out);
        assert!(!mixer.is_active());
        assert_eq!(out[30], 0.0);
    }

    #[test]
    fn loops_and_resamples() {
        let mut mixer = Mixer::new(2000);
        mixer.set_sound("tone", tone(1000, 10));
        mixer.apply(&[voice(1, 0.0, 1.0, true)]);
        let mut out = vec![0.0; 200];
        mixer.render(&mut out);
        assert!(mixer.is_active());
        assert!((out[198] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resyncs_only_when_far() {
        let mut mixer = Mixer::new(1000);
        mixer.set_sound("tone", tone(1000, 5000));
        mixer.apply(&[voice(1, 1.0, 1.0, false)]);
        assert_eq!(mixer.playing[0].cursor, 1000.0);
        mixer.apply(&[voice(1, 1.1, 1.0, false)]);
        assert_eq!(mixer.playing[0].cursor, 1000.0);
        mixer.apply(&[voice(1, 3.0, 1.0, false)]);
        assert_eq!(mixer.playing[0].cursor, 3000.0);
    }

    /// A device that was asleep when the sound started is most of a
    /// second behind by the time it produces anything. The engine's
    /// position runs on regardless, so the voice is far from where the
    /// engine says it is, through no seek: the sound must play from its
    /// start, not skip to the middle.
    #[test]
    fn a_device_that_woke_late_does_not_lose_the_start() {
        let mut mixer = Mixer::new(1000);
        mixer.set_sound("tone", tone(1000, 5000));
        mixer.apply(&[voice(1, 0.0, 1.0, false)]);
        assert_eq!(mixer.playing[0].cursor, 0.0);
        // Six hundred milliseconds of waking, a frame's worth at a time,
        // while nothing is rendered.
        let mut at = 0.0;
        while at < 0.6 {
            at += 1.0 / 60.0;
            mixer.apply(&[voice(1, at, 1.0, false)]);
        }
        assert_eq!(
            mixer.playing[0].cursor, 0.0,
            "the wake was mistaken for a seek and the start was thrown away"
        );

        // A real seek still moves it: the distance jumps in one step.
        mixer.apply(&[voice(1, at + 2.0, 1.0, false)]);
        assert!((mixer.playing[0].cursor - (at + 2.0) * 1000.0).abs() < 1e-6);
    }

    #[test]
    fn a_mono_sound_is_placed_with_constant_power() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        // In the middle, as it plays unpanned.
        assert_eq!(place(0.5, 0.0), [0.5, 0.5]);
        // At either side, all of it in one speaker.
        let [l, r] = place(0.5, -1.0);
        assert!(
            close(l, 0.5 * std::f32::consts::SQRT_2) && close(r, 0.0),
            "{l} {r}"
        );
        let [l, r] = place(0.5, 1.0);
        assert!(
            close(l, 0.0) && close(r, 0.5 * std::f32::consts::SQRT_2),
            "{l} {r}"
        );
        // As loud wherever it is.
        for pan in [-0.75, -0.3, 0.2, 0.6] {
            let [l, r] = place(0.5, pan);
            assert!(close(l * l + r * r, 0.5), "{pan}: {l} {r}");
        }
    }

    #[test]
    fn a_stereo_sound_is_balanced() {
        assert_eq!(balance([0.25, 0.5], 0.0), [0.25, 0.5]);
        // Hard left: the right channel folded into the left.
        let [l, r] = balance([0.25, 0.5], -1.0);
        assert!((l - 0.75).abs() < 1e-6 && r.abs() < 1e-6, "{l} {r}");
        let [l, r] = balance([0.25, 0.5], 1.0);
        assert!(l.abs() < 1e-6 && (r - 0.75).abs() < 1e-6, "{l} {r}");
    }

    #[test]
    fn a_voice_panned_right_sounds_on_the_right() {
        let mut mixer = Mixer::new(1000);
        mixer.set_sound("tone", tone(1000, 100));
        let mut right = voice(1, 0.0, 1.0, false);
        right.pan = 1.0;
        mixer.apply(&[right]);
        let mut out = vec![0.0; 40];
        mixer.render(&mut out);
        assert!(out[20].abs() < 1e-6, "left: {}", out[20]);
        assert!(
            (out[21] - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-6,
            "right: {}",
            out[21]
        );
    }

    #[test]
    fn waits_for_samples() {
        let mut mixer = Mixer::new(1000);
        mixer.apply(&[voice(1, 0.0, 1.0, false)]);
        assert!(!mixer.is_active());
        mixer.set_sound("tone", tone(1000, 100));
        mixer.apply(&[voice(1, 0.02, 1.0, false)]);
        assert_eq!(mixer.playing[0].cursor, 20.0);
    }
}
