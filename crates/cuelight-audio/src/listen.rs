//! Listening to sound for a show's `input.audio`: its loudness, the
//! loudness of a few frequency ranges, and whether it jumped, worked out
//! from samples as they arrive ([`Analyser`]), and with the `live` feature
//! taken from a sound device ([`Listener`]): what the computer plays, or
//! the microphone.

use cuelight_core::Heard;

/// The audible range the bands divide, in hertz: nine octaves, so that
/// three bands split at 250 Hz and 2 kHz.
const LOWEST: f64 = 31.25;
const HIGHEST: f64 = 16_000.0;

/// Samples measured together, at 48 kHz about 5 ms.
const CHUNK: usize = 256;

/// How fast a number rises to a louder sound, and falls from it, in
/// seconds: quick enough to see a beat, slow enough not to flicker.
const ATTACK: f64 = 0.01;
const RELEASE: f64 = 0.2;

/// How long the loudest the sound has been is remembered, in seconds:
/// what a number is measured against.
const MEMORY: f64 = 8.0;

/// Loudness below which a sound counts as silence, as an RMS level
/// (about -50 dB): a quiet room reads near 0, not as loud as a track.
const FLOOR: f64 = 0.003;

/// The least time between two onsets, in seconds: at most about six a
/// second, so a busy track does not fire one on every drum.
const REFRACTORY: f64 = 0.16;

/// How far the quick loudness has to rise over the slow one to count as
/// a jump.
const JUMP: f64 = 1.6;

/// Decibels below full scale an absolute level spans: -60 dB reads 0, as
/// a meter's scale does.
const RANGE: f64 = 60.0;

/// The least a band is measured against, as a share of how loud the whole
/// sound has been lately: a band that only hears what leaks in from its
/// neighbours reads near 0, not as full as one the music is in.
const BAND_SHARE: f64 = 0.25;

/// A second order filter section (RBJ cookbook), Butterworth.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(rate: f64, cutoff: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * cutoff / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let b = match high {
            false => [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            true => [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
        };
        Self {
            b: b.map(|b| b / a0),
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    fn step(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// One measured loudness: smoothed, and measured against how loud it has
/// been lately.
#[derive(Debug, Clone, Copy)]
struct Meter {
    envelope: f64,
    peak: f64,
}

impl Meter {
    fn new() -> Self {
        Self {
            envelope: 0.0,
            peak: FLOOR,
        }
    }

    /// Take a chunk's RMS level, `seconds` long.
    fn take(&mut self, rms: f64, seconds: f64) {
        let speed = if rms > self.envelope { ATTACK } else { RELEASE };
        self.envelope += (rms - self.envelope) * (1.0 - (-seconds / speed).exp());
        self.peak = (self.peak * (-seconds / MEMORY).exp())
            .max(self.envelope)
            .max(FLOOR);
    }

    /// From 0 to 1, measured against at least `least`.
    fn value(&self, least: f64) -> f64 {
        (self.envelope / self.peak.max(least)).clamp(0.0, 1.0)
    }

    /// From 0 to 1 on a decibel scale, whatever has been heard before:
    /// [`RANGE`] below full scale reads 0, full scale 1.
    fn absolute(&self) -> f64 {
        let db = 20.0 * self.envelope.max(1e-9).log10();
        (1.0 + db / RANGE).clamp(0.0, 1.0)
    }
}

/// One frequency range: the samples filtered to it, and its meter.
#[derive(Debug, Clone)]
struct Band {
    filters: Vec<Biquad>,
    sum: f64,
    meter: Meter,
}

/// Works out what a show's `input.audio` asks for from samples as they
/// arrive, any number of them at a time: the sound's loudness, that of
/// `bands` frequency ranges dividing the audible spectrum evenly in
/// octaves, and whether it jumped.
#[derive(Debug, Clone)]
pub struct Analyser {
    rate: f64,
    bands: Vec<Band>,
    level: Meter,
    level_sum: f64,
    filled: usize,
    /// The quick and the slow loudness onsets are found between.
    quick: f64,
    slow: f64,
    since_onset: f64,
    onset: bool,
}

impl Analyser {
    /// An analyser for sound at `rate` Hz, measuring `bands` ranges.
    pub fn new(rate: u32, bands: usize) -> Self {
        let rate = f64::from(rate.max(1));
        let nyquist = rate / 2.0 * 0.95;
        let octaves = (HIGHEST / LOWEST).log2();
        let edge = |i: usize| LOWEST * 2f64.powf(octaves * i as f64 / bands.max(1) as f64);
        let bands = (0..bands)
            .map(|i| {
                let (low, high) = (edge(i), edge(i + 1));
                // Two sections an edge, 24 dB an octave, so little of a
                // neighbour's sound leaks in.
                let low_cut = Biquad::new(rate, low.min(nyquist), true);
                let mut filters = vec![low_cut, low_cut];
                if high < nyquist {
                    let high_cut = Biquad::new(rate, high, false);
                    filters.extend([high_cut, high_cut]);
                }
                Band {
                    filters,
                    sum: 0.0,
                    meter: Meter::new(),
                }
            })
            .collect();
        Self {
            rate,
            bands,
            level: Meter::new(),
            level_sum: 0.0,
            filled: 0,
            quick: 0.0,
            slow: 0.0,
            since_onset: REFRACTORY,
            onset: false,
        }
    }

    /// Take interleaved samples with `channels` channels, mixed to one.
    pub fn take(&mut self, samples: &[f32], channels: usize) {
        let channels = channels.max(1);
        for frame in samples.chunks(channels) {
            let x = frame.iter().map(|s| f64::from(*s)).sum::<f64>() / frame.len() as f64;
            self.level_sum += x * x;
            for band in &mut self.bands {
                let y = band.filters.iter_mut().fold(x, |y, f| f.step(y));
                band.sum += y * y;
            }
            self.filled += 1;
            if self.filled == CHUNK {
                self.measure();
            }
        }
    }

    /// Measure the chunk just filled.
    fn measure(&mut self) {
        let n = self.filled as f64;
        let seconds = n / self.rate;
        let rms = (self.level_sum / n).sqrt();
        self.level.take(rms, seconds);
        for band in &mut self.bands {
            band.meter.take((band.sum / n).sqrt(), seconds);
            band.sum = 0.0;
        }
        // A jump: the quick loudness well over the slow one, above
        // silence, and not too soon after the last.
        self.quick += (rms - self.quick) * (1.0 - (-seconds / 0.005).exp());
        self.slow += (rms - self.slow) * (1.0 - (-seconds / 0.3).exp());
        self.since_onset += seconds;
        if self.quick > self.slow * JUMP
            && self.quick > FLOOR * 2.0
            && self.since_onset >= REFRACTORY
        {
            self.onset = true;
            self.since_onset = 0.0;
        }
        self.level_sum = 0.0;
        self.filled = 0;
    }

    /// What was heard: the numbers as they stand, and whether the sound
    /// jumped since the last time this was asked.
    pub fn heard(&mut self) -> Heard {
        Heard {
            level: self.level.value(FLOOR),
            bands: self
                .bands
                .iter()
                .map(|b| b.meter.value(self.level.peak * BAND_SHARE))
                .collect(),
            absolute_level: self.level.absolute(),
            absolute_bands: self.bands.iter().map(|b| b.meter.absolute()).collect(),
            onset: std::mem::take(&mut self.onset),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(hz: f64, seconds: f64, amplitude: f32) -> Vec<f32> {
        let n = (f64::from(RATE) * seconds) as usize;
        (0..n)
            .map(|i| {
                amplitude * (std::f64::consts::TAU * hz * i as f64 / f64::from(RATE)).sin() as f32
            })
            .collect()
    }

    #[test]
    fn three_bands_split_at_250_hz_and_2_khz() {
        let octaves = (HIGHEST / LOWEST).log2();
        let edge = |i: f64| LOWEST * 2f64.powf(octaves * i / 3.0);
        assert!((edge(1.0) - 250.0).abs() < 1e-9 && (edge(2.0) - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn a_low_tone_moves_the_bass_and_a_high_one_the_treble() {
        let mut low = Analyser::new(RATE, 3);
        low.take(&tone(80.0, 1.0, 0.5), 1);
        let heard = low.heard();
        assert!(heard.bands[0] > 0.9 && heard.bands[2] < 0.1, "{heard:?}");
        // What leaks into the middle from the bass stays small.
        assert!(heard.bands[1] < 0.2, "{heard:?}");
        assert!(heard.level > 0.9, "{heard:?}");

        let mut high = Analyser::new(RATE, 3);
        high.take(&tone(6_000.0, 1.0, 0.5), 1);
        let heard = high.heard();
        assert!(heard.bands[2] > 0.9 && heard.bands[0] < 0.1, "{heard:?}");
    }

    #[test]
    fn a_quiet_track_moves_a_show_as_a_loud_one_does_and_silence_reads_nothing() {
        let mut quiet = Analyser::new(RATE, 3);
        quiet.take(&tone(80.0, 1.0, 0.05), 1);
        assert!(quiet.heard().level > 0.9);
        let mut silent = Analyser::new(RATE, 3);
        silent.take(&vec![0.0; RATE as usize], 1);
        let heard = silent.heard();
        assert_eq!(heard.level, 0.0);
        assert!(!heard.onset);
    }

    #[test]
    fn the_absolute_level_follows_the_volume_and_the_other_does_not() {
        // A sine of amplitude 0.5 is -9 dB of full scale, one of 0.05
        // twenty decibels below it.
        let mut loud = Analyser::new(RATE, 1);
        loud.take(&tone(440.0, 1.0, 0.5), 1);
        let mut quiet = Analyser::new(RATE, 1);
        quiet.take(&tone(440.0, 1.0, 0.05), 1);
        let (loud, quiet) = (loud.heard(), quiet.heard());
        assert!(
            (loud.absolute_level - (1.0 - 9.03 / 60.0)).abs() < 0.01,
            "{loud:?}"
        );
        assert!(
            (quiet.absolute_level - (1.0 - 29.03 / 60.0)).abs() < 0.01,
            "{quiet:?}"
        );
        assert!(loud.level > 0.9 && quiet.level > 0.9);
    }

    #[test]
    fn the_level_falls_after_the_sound_stops() {
        let mut analyser = Analyser::new(RATE, 1);
        analyser.take(&tone(440.0, 0.5, 0.5), 1);
        let loud = analyser.heard().level;
        analyser.take(&vec![0.0; RATE as usize / 2], 1);
        let after = analyser.heard().level;
        assert!(loud > 0.9 && after < 0.2, "{loud} then {after}");
    }

    #[test]
    fn a_hit_after_quiet_is_an_onset_and_onsets_come_at_most_a_few_a_second() {
        let mut analyser = Analyser::new(RATE, 3);
        // A sound starting out of silence is a jump; one going on is not.
        analyser.take(&tone(100.0, 0.5, 0.02), 1);
        let _ = analyser.heard();
        analyser.take(&tone(100.0, 0.5, 0.02), 1);
        assert!(!analyser.heard().onset, "a steady sound is no jump");
        analyser.take(&tone(100.0, 0.05, 0.5), 1);
        assert!(analyser.heard().onset, "a hit is");
        // Hits every 50 ms for a second: no more than one per refractory
        // stretch gets through.
        let mut onsets = 0;
        for _ in 0..20 {
            analyser.take(&tone(100.0, 0.025, 0.02), 1);
            analyser.take(&tone(100.0, 0.025, 0.5), 1);
            onsets += usize::from(analyser.heard().onset);
        }
        assert!((1..=7).contains(&onsets), "{onsets} onsets in a second");
    }

    #[test]
    fn stereo_is_mixed_to_one() {
        let mono = tone(80.0, 0.5, 0.5);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        let (mut a, mut b) = (Analyser::new(RATE, 3), Analyser::new(RATE, 3));
        a.take(&mono, 1);
        b.take(&stereo, 2);
        assert_eq!(a.heard(), b.heard());
    }
}
