use crate::listen::Analyser;
use crate::mixer::Mixer;
use crate::sound::Sound;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use cuelight_core::{Heard, Voice};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What the game thread tells the mixer on the device thread.
enum Command {
    Sound(String, Arc<Sound>),
    Voices(Vec<Voice>),
}

/// The default sound device playing a [`Mixer`].
///
/// The device asks for blocks on its own thread and the mixer runs there;
/// this side only queues commands: sounds to know and, once per frame, the
/// engine's voice list. Dropping it stops the sound and closes the device.
///
/// Opening one opens the device and holds it for as long as it lives. Both
/// halves of that are deliberate.
///
/// Whether to open it at all is the host's call, not this type's: a host
/// asks [`Show::has_sound`](cuelight_core::Show::has_sound) and does not build
/// an `Output` for a show that cannot make a sound, so such a show is
/// never listed by a desktop as an application making one.
///
/// For a show that can, the device is opened at once rather than at its
/// first cue. An idle sound card is free to suspend and waking one is slow
/// enough to hear: an HDMI output here takes about six hundred
/// milliseconds, which is a cue arriving visibly after the picture it
/// belongs to. Opening early spends that while the show is loading, where
/// nobody is listening, instead of inserting it in front of the first
/// sound. Finding the device and starting the stream takes about ten
/// milliseconds, so it does not hold up the show.
pub struct Output {
    commands: Sender<Command>,
    rate: u32,
    channels: u16,
    /// Set by the device's own thread the first time it asks for audio.
    ready: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

impl Output {
    /// Open the default output device in its default configuration.
    pub fn open() -> Result<Output, String> {
        let opening = Instant::now();
        let host = playback_host();
        let device = host
            .default_output_device()
            .ok_or("no default output device")?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("no default output config: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let (rate, channels) = (config.sample_rate, config.channels);
        let (tx, rx) = channel();
        let ready = Arc::new(AtomicBool::new(false));
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, rx, &ready),
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, rx, &ready),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, rx, &ready),
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, rx, &ready),
            cpal::SampleFormat::F64 => build::<f64>(&device, &config, rx, &ready),
            other => return Err(format!("unsupported sample format {other}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("starting the stream: {e}"))?;
        log::info!(
            "audio: {} at {rate} Hz, {channels} channel(s), {format}, open in {:.0} ms",
            device
                .description()
                .map_or_else(|_| "output device".to_owned(), |d| d.to_string()),
            opening.elapsed().as_secs_f64() * 1000.0
        );
        Ok(Output {
            commands: tx,
            rate,
            channels,
            ready,
            _stream: stream,
        })
    }

    /// Whether the device has started asking for audio.
    ///
    /// Opening a stream is quick; a sound card that was suspended waking
    /// up behind it is not, and until it does nothing that is played will
    /// be heard. A host that wants to know whether a cue will be heard
    /// when it is fired, rather than a moment later, can ask this. It goes
    /// true once and stays true.
    ///
    /// Nothing is lost by playing before then: the mixer keeps the start
    /// of a sound whose device woke late rather than skipping into it.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// The device's sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Hand the mixer the samples of sound `name`.
    pub fn set_sound(&self, name: &str, sound: Arc<Sound>) {
        let _ = self.commands.send(Command::Sound(name.to_owned(), sound));
    }

    /// Hand the mixer this frame's voice list; see [`Mixer::apply`].
    pub fn apply(&self, voices: &[Voice]) {
        let _ = self.commands.send(Command::Voices(voices.to_vec()));
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    commands: Receiver<Command>,
    ready: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let channels = usize::from(config.channels.max(1));
    let ready = ready.clone();
    let opened = Instant::now();
    let mut mixer = Mixer::new(config.sample_rate);
    let mut stereo: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _| {
                if !ready.swap(true, Ordering::Relaxed) {
                    log::debug!(
                        "audio: device asking for blocks {:.0} ms after it opened",
                        opened.elapsed().as_secs_f64() * 1000.0
                    );
                }
                while let Ok(command) = commands.try_recv() {
                    match command {
                        Command::Sound(name, sound) => mixer.set_sound(&name, sound),
                        Command::Voices(voices) => mixer.apply(&voices),
                    }
                }
                let frames = data.len() / channels;
                stereo.resize(frames * 2, 0.0);
                mixer.render(&mut stereo);
                // Stereo onto whatever the device has: both channels of a
                // mono device, the first two of a wider one.
                for (frame, out) in stereo
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .zip(data.chunks_exact_mut(channels))
                {
                    for (i, sample) in out.iter_mut().enumerate() {
                        *sample = T::from_sample(match (channels, i) {
                            (1, _) => (frame[0] + frame[1]) * 0.5,
                            (_, 0) => frame[0],
                            (_, 1) => frame[1],
                            _ => 0.0,
                        });
                    }
                }
            },
            |e| log::error!("audio stream: {e}"),
            None,
        )
        .map_err(|e| format!("opening the stream: {e}"))
}

/// The host sound plays through: the system's default, except on Linux,
/// where it stays ALSA (on PipeWire and PulseAudio systems, through their
/// ALSA plugin) even though the PulseAudio client is built in for
/// listening; see `crate::listen`.
pub(crate) fn playback_host() -> cpal::Host {
    #[cfg(target_os = "linux")]
    if let Ok(host) = cpal::host_from_id(cpal::HostId::Alsa) {
        return host;
    }
    cpal::default_host()
}

/// Frames a listened device is asked to hand over at a time: about 10 ms
/// at 48 kHz, so what is heard reaches the show within a frame or two.
const LISTEN_FRAMES: u32 = 480;

/// What a [`Listener`] listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    /// What the computer plays: the default output, heard as it goes out.
    Output,
    /// The default input device, a microphone.
    Mic,
}

/// A sound device listened to for a show's `input.audio`: its samples go
/// through an [`Analyser`] on the device's own thread, and
/// [`heard`](Listener::heard) says what it made of them so far.
///
/// A device that goes away, a sound server restarting or the machine
/// waking from sleep, is opened again by itself, every few seconds until
/// it is back; until then it hears silence.
pub struct Listener {
    from: Listen,
    bands: usize,
    /// What is being listened to, as the system names it.
    pub name: String,
    live: Option<Live>,
    /// When to try opening the device again, once it went away.
    retry_at: Instant,
}

/// An open device being listened to.
struct Live {
    analyser: Arc<Mutex<Analyser>>,
    /// Set by the device's thread when the stream fails.
    lost: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

/// How long to wait before opening a device that went away again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(2);

impl Listener {
    /// Listen to `from`, measuring `bands` frequency ranges.
    pub fn open(from: Listen, bands: usize) -> Result<Listener, String> {
        let (live, name) = connect(from, bands)?;
        log::info!("listening to {name}");
        Ok(Listener {
            from,
            bands,
            name,
            live: Some(live),
            retry_at: Instant::now(),
        })
    }

    /// What was heard: the numbers as they stand, and whether the sound
    /// jumped since the last time this was asked. Silence while the device
    /// is away, and each call past the retry time tries to open it again.
    pub fn heard(&mut self) -> Heard {
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.lost.load(Ordering::Relaxed))
        {
            log::warn!("lost {}; listening again when it is back", self.name);
            self.live = None;
            self.retry_at = Instant::now() + RETRY;
        }
        if self.live.is_none() && Instant::now() >= self.retry_at {
            match connect(self.from, self.bands) {
                Ok((live, name)) => {
                    log::info!("listening to {name} again");
                    self.name = name;
                    self.live = Some(live);
                }
                Err(_) => self.retry_at = Instant::now() + RETRY,
            }
        }
        match &self.live {
            Some(live) => live
                .analyser
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .heard(),
            None => Heard::default(),
        }
    }
}

/// Open `from` and start listening to it.
fn connect(from: Listen, bands: usize) -> Result<(Live, String), String> {
    let (device, name) = listening_device(from)?;
    // An output listened to is opened as an input: Windows and macOS turn
    // that into a loopback, and on Linux the device is the output's
    // monitor, an input already.
    let supported = device
        .default_input_config()
        .or_else(|_| device.default_output_config())
        .map_err(|e| format!("{name}: no configuration to listen with: {e}"))?;
    let format = supported.sample_format();
    // Asked for in small pieces: left to itself, a PulseAudio server hands
    // a recording over in chunks of up to two seconds, and a show would
    // pulse to the music two seconds late.
    let small = match *supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            Some(cpal::BufferSize::Fixed(LISTEN_FRAMES.clamp(min, max)))
        }
        cpal::SupportedBufferSize::Unknown => None,
    };
    let mut config: cpal::StreamConfig = supported.into();
    if let Some(small) = small {
        config.buffer_size = small;
    }
    let analyser = Arc::new(Mutex::new(Analyser::new(config.sample_rate, bands)));
    let lost = Arc::new(AtomicBool::new(false));
    let stream = match format {
        cpal::SampleFormat::F32 => listen::<f32>(&device, &config, &analyser, &lost),
        cpal::SampleFormat::I16 => listen::<i16>(&device, &config, &analyser, &lost),
        cpal::SampleFormat::U16 => listen::<u16>(&device, &config, &analyser, &lost),
        cpal::SampleFormat::I32 => listen::<i32>(&device, &config, &analyser, &lost),
        cpal::SampleFormat::F64 => listen::<f64>(&device, &config, &analyser, &lost),
        other => return Err(format!("{name}: unsupported sample format {other}")),
    }?;
    stream
        .play()
        .map_err(|e| format!("{name}: starting to listen: {e}"))?;
    log::debug!(
        "{name} at {} Hz, {} channel(s)",
        config.sample_rate,
        config.channels
    );
    Ok((
        Live {
            analyser,
            lost,
            _stream: stream,
        },
        name,
    ))
}

/// The device `from` names, and what the system calls it.
fn listening_device(from: Listen) -> Result<(cpal::Device, String), String> {
    let describe = |device: &cpal::Device| {
        device
            .description()
            .map_or_else(|_| "a sound device".to_owned(), |d| d.to_string())
    };
    // On Linux the output is heard through its monitor, which only the
    // PulseAudio protocol offers (PipeWire speaks it too).
    #[cfg(target_os = "linux")]
    if from == Listen::Output {
        let host = cpal::host_from_id(cpal::HostId::PulseAudio)
            .map_err(|e| format!("hearing the output needs PulseAudio or PipeWire: {e}"))?;
        let output = host
            .default_output_device()
            .ok_or("no default output to listen to")?;
        let monitor = format!(
            "{}.monitor",
            output
                .id()
                .map_err(|e| format!("the output has no name: {e}"))?
                .id()
        );
        let device = host
            .devices()
            .map_err(|e| format!("listing sound devices: {e}"))?
            .find(|d| d.id().is_ok_and(|id| id.id() == monitor))
            .ok_or_else(|| format!("no monitor of {} to listen to", describe(&output)))?;
        let name = format!("what {} plays", describe(&output));
        return Ok((device, name));
    }
    match from {
        Listen::Output => {
            let device = playback_host()
                .default_output_device()
                .ok_or("no default output to listen to")?;
            let name = format!("what {} plays", describe(&device));
            Ok((device, name))
        }
        Listen::Mic => {
            let device = listening_host()
                .default_input_device()
                .ok_or("no microphone to listen to")?;
            let name = describe(&device);
            Ok((device, name))
        }
    }
}

/// The host a microphone is listened to through: PulseAudio on Linux when
/// it is there, as for the output, otherwise the default.
fn listening_host() -> cpal::Host {
    #[cfg(target_os = "linux")]
    if let Ok(host) = cpal::host_from_id(cpal::HostId::PulseAudio) {
        return host;
    }
    cpal::default_host()
}

fn listen<T: SizedSample + cpal::Sample>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    analyser: &Arc<Mutex<Analyser>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let analyser = analyser.clone();
    let mut samples: Vec<f32> = Vec::new();
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                samples.clear();
                samples.extend(
                    data.iter()
                        .map(|s| <f32 as FromSample<T>>::from_sample_(*s)),
                );
                if let Ok(mut analyser) = analyser.lock() {
                    analyser.take(&samples, channels);
                }
            },
            {
                let lost = lost.clone();
                move |e| {
                    log::warn!("listening: {e}");
                    lost.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|e| format!("listening: {e}"))
}
