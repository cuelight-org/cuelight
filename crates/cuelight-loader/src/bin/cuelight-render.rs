//! Render a show's frames at chosen times, and say what it fired and when.
//!
//! ```sh
//! cuelight-render eclipse/ --at 4.1,19.5,26 -o frames/
//! cuelight-render eclipse/ --every 0.5 --until 52 -o frames/
//! cuelight-render eclipse/ --until 52 --events
//! cuelight-render dmd/ --at 2 --scale 4 -o frames/
//! ```
//!
//! A frame is the canvas at the show's own size; `--scale` writes what a
//! host would show instead. Sound lengths come from the file's header, so
//! a sound's `on_end` fires; no device is opened and nothing is played.
//! A video layer draws the frame it is playing, decoded one frame at a
//! time for the frames that are written.
//!
//! Time is walked in fixed steps of `--fps`, from 0, so a run is
//! repeatable: the same command writes the same bytes. That is also what
//! makes it a test of whether a show is deterministic at all.

use cuelight::render::Renderer;
use cuelight::Engine;
use cuelight_loader::{DriverPlayer, Step};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(clap::Parser)]
#[command(
    name = "cuelight-render",
    about = "Render a show's frames at chosen times"
)]
struct Cli {
    /// Show folder, loose show file or packed show.
    show: PathBuf,
    /// Times to render, in seconds: `4.1,19.5,26`.
    #[arg(long, value_delimiter = ',')]
    at: Vec<f64>,
    /// Render every this many seconds instead, up to `--until`.
    #[arg(long)]
    every: Option<f64>,
    /// How far to run, in seconds. Required with `--every`, and the end
    /// of the run with `--events`.
    #[arg(long)]
    until: Option<f64>,
    /// Steps per second the show is advanced in. Part of what is being
    /// tested: a chain of timelines can land differently at another rate.
    #[arg(long, default_value_t = 60.0)]
    fps: f64,
    /// Where the frames go; created if it is not there.
    #[arg(short, long, default_value = "frames")]
    out: PathBuf,
    /// Print what the show fired, with the time, and render nothing.
    #[arg(long)]
    events: bool,
    /// Ignore the folder's driver script.
    #[arg(long)]
    no_driver: bool,
    /// Render what can be rendered of a show that does not load whole,
    /// and print what was left out.
    #[arg(long)]
    lenient: bool,
    /// Fire a trigger at a time: `--trigger 2.5:go`, repeatable.
    #[arg(long = "trigger", value_name = "TIME:NAME")]
    triggers: Vec<String>,
    /// Set a variable at a time: `--set 0:score=1500`, repeatable.
    #[arg(long = "set", value_name = "TIME:VAR=VALUE")]
    sets: Vec<String>,
    /// Render the frame as a host would show it, this many times the
    /// show's size: its `scaling`, output mode and passes applied. A dots
    /// pass needs about 3 to become dots at all.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "width")]
    scale: Option<u32>,
    /// Render the frame as a host would show it, at most this many pixels
    /// wide: the largest size that fits and keeps whole pixels. For a
    /// gallery of shows of different sizes.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    width: Option<u32>,
    /// Measure what the show costs instead of writing frames: every frame
    /// up to `--until` is resolved and drawn, and a report says where the
    /// time went, layer by layer.
    #[arg(long, conflicts_with_all = ["events", "scale", "width", "at", "every"])]
    profile: bool,
    /// How many layers each list of the profile names.
    #[arg(long, default_value_t = 10)]
    top: usize,
}

/// An input the command line asks for, at the time it asks for it.
#[derive(Debug)]
struct Input {
    at: f64,
    what: Step,
}

fn inputs(cli: &Cli) -> Result<Vec<Input>, String> {
    let split = |s: &str| -> Result<(f64, String), String> {
        let (time, rest) = s
            .split_once(':')
            .ok_or_else(|| format!("{s:?} needs a time, as in 2.5:name"))?;
        let at: f64 = time
            .parse()
            .map_err(|_| format!("{time:?} in {s:?} is not a time"))?;
        Ok((at, rest.to_owned()))
    };
    let mut out = Vec::new();
    for arg in &cli.triggers {
        let (at, trigger) = split(arg)?;
        out.push(Input {
            at,
            what: Step::Trigger { trigger },
        });
    }
    for arg in &cli.sets {
        let (at, rest) = split(arg)?;
        let (name, value) = rest
            .split_once('=')
            .ok_or_else(|| format!("{arg:?} needs a value, as in 0:score=1500"))?;
        let value = match value.parse::<f64>() {
            Ok(number) => cuelight_core::Value::Number(number),
            Err(_) => cuelight_core::Value::Text(value.to_owned()),
        };
        out.push(Input {
            at,
            what: Step::Set {
                set: std::collections::BTreeMap::from([(name.to_owned(), value)]),
            },
        });
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    Ok(out)
}

/// The largest frame no wider than `width` that keeps whole pixels.
///
/// A show narrower than `width` goes up by the largest whole factor that
/// fits, so a pixel-perfect show gets no letterbox and no uneven pixels.
/// A wider one comes down to `1/n` of its size, `n` the smallest that
/// fits: it is presented at that size rather than shrunk afterwards, so a
/// smooth show is drawn sharp there instead of being filtered.
fn sized_to(show: [u32; 2], width: u32) -> Result<[u32; 2], Stop> {
    let [w, h] = show;
    if w <= width {
        let factor = (width / w).max(1);
        let both = w.checked_mul(factor).zip(h.checked_mul(factor));
        let (w, h) = both.ok_or_else(|| {
            Stop::Failed(format!(
                "--width {width} is past what {w}x{h} can be scaled to"
            ))
        })?;
        Ok([w, h])
    } else {
        let n = w.div_ceil(width);
        Ok([(w / n).max(1), (h / n).max(1)])
    }
}

/// Why a run stopped.
#[derive(Debug)]
enum Stop {
    /// Nobody is reading any more: `--events | head` has what it wants.
    /// Not a failure, and nothing more to say.
    PipeClosed,
    Failed(String),
}

impl From<String> for Stop {
    fn from(why: String) -> Self {
        Stop::Failed(why)
    }
}

impl From<&str> for Stop {
    fn from(why: &str) -> Self {
        Stop::Failed(why.to_owned())
    }
}

/// Print a line, saying so if the other end of the pipe has gone.
fn say(line: &str) -> Result<(), Stop> {
    use std::io::Write;
    match writeln!(std::io::stdout(), "{line}") {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Err(Stop::PipeClosed),
        Err(e) => Err(Stop::Failed(e.to_string())),
    }
}

/// The times to render, in order.
fn wanted(cli: &Cli) -> Result<Vec<f64>, String> {
    if let Some(every) = cli.every {
        if !(every.is_finite() && every > 0.0) {
            return Err("--every wants a number above 0".into());
        }
        let until = cli.until.ok_or("--every needs --until")?;
        let mut times = Vec::new();
        let mut t = 0.0;
        while t <= until + f64::EPSILON {
            times.push(t);
            t += every;
        }
        return Ok(times);
    }
    let mut times = cli.at.clone();
    times.sort_by(f64::total_cmp);
    Ok(times)
}

/// Hand the engine the frame of every video that should be showing, so
/// the picture is there when the frame is drawn.
///
/// Only for frames that are written: the engine says which clip is
/// playing where and how far in, and one frame of it is decoded on the
/// spot. A run that writes one frame at twenty seconds decodes one frame
/// of each clip, not twenty seconds of video. A clip that cannot be
/// decoded at all (no ffmpeg on the machine) is reported once and drawn
/// as nothing, which is what a show with no decoder has always done.
fn show_video_frames(
    engine: &mut Engine,
    clips: &BTreeMap<String, cuelight_video::Clip>,
    quiet: &mut BTreeSet<String>,
) {
    for playing in engine.videos().unwrap_or_default() {
        let Some(clip) = clips.get(&playing.video) else {
            continue;
        };
        let details = clip.details();
        let frame = clip.still(playing.position).and_then(|frame| {
            // Under the key the play reports, not the clip's name: two
            // layers playing one clip are at two positions, and each
            // draws its own picture.
            engine
                .set_image(&playing.frame, details.width, details.height, frame)
                .map_err(|e| e.to_string())
        });
        if let Err(e) = frame {
            if quiet.insert(playing.video.clone()) {
                eprintln!("warning: video {:?}: {e}", playing.video);
            }
        }
    }
}

fn run(cli: &Cli) -> Result<(), Stop> {
    let mut engine = Engine::new();
    let mut options = cuelight_loader::Options::default();
    options.lenient = cli.lenient;
    let loaded =
        cuelight_loader::load_with(&mut engine, &cli.show, &options).map_err(|e| e.to_string())?;
    for finding in &loaded.findings {
        eprintln!("warning: {finding}");
    }
    for name in &loaded.skipped {
        eprintln!("skipped {name}");
    }
    for family in &loaded.missing_fonts {
        eprintln!("warning: the artwork asks for font {family:?}, which the show does not ship");
    }
    for warning in engine.load_warnings() {
        eprintln!("warning: {warning}");
    }
    // Lengths, so a sound ends and its `on_end` fires: a show chained
    // through one stops at the first without this. Decoding opens no
    // device.
    for sound in &loaded.sounds {
        // The header usually says, and reading it beats turning the whole
        // file into samples for one number. A constant-bitrate MP3 with no
        // Xing header does not say, and is decoded.
        let length = match cuelight_audio::length(&sound.extension, &sound.bytes) {
            Some(length) => Some(length),
            None => match cuelight_audio::Sound::decode(&sound.extension, &sound.bytes) {
                Ok(decoded) => Some(decoded.duration()),
                Err(e) => {
                    eprintln!("warning: sound {:?}: {e}", sound.name);
                    None
                }
            },
        };
        if let Some(length) = length {
            engine
                .set_sound(&sound.name, length)
                .map_err(|e| e.to_string())?;
        }
    }
    // The same for videos, measured the way the player measures them:
    // the file's header through ffprobe, no frames decoded. A machine
    // without ffmpeg says so once per file and carries on, since a show
    // that never ends a clip still renders.
    //
    // Bounded by the canvas, as the player bounds it, so a layer showing
    // a clip far bigger than the show lays out the same in both.
    let how = cuelight_video::Decode {
        size: engine.show().map(|show| show.size),
        ..cuelight_video::Decode::default()
    };
    let mut clips: BTreeMap<String, cuelight_video::Clip> = BTreeMap::new();
    for path in &loaded.videos {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        match cuelight_video::Clip::open(path, Some(how)) {
            Ok(clip) => {
                let details = clip.details();
                engine
                    .set_video(&name, details.duration, details.size())
                    .map_err(|e| e.to_string())?;
                clips.insert(name, clip);
            }
            Err(e) => eprintln!("warning: video {name:?}: {e}"),
        }
    }
    // A clip that would not give a frame is said so once rather than per
    // frame written: a strip of a hundred frames of a show whose decoder
    // is missing should not print a hundred lines.
    let mut quiet: BTreeSet<String> = BTreeSet::new();
    let mut driver = loaded
        .driver
        .filter(|_| !cli.no_driver)
        .map(DriverPlayer::new);
    let mut inputs = inputs(cli)?.into_iter().peekable();
    let mut times = wanted(cli)?.into_iter().peekable();

    if !cli.events && !cli.profile && times.peek().is_none() {
        return Err(
            "nothing to render: give --at or --every, or ask for --events or --profile".into(),
        );
    }
    if cli.profile && cli.until.is_none() {
        return Err("--profile runs to --until: say how far".into());
    }
    if (cli.scale.is_some() || cli.width.is_some()) && cli.events {
        let what = if cli.scale.is_some() {
            "--scale"
        } else {
            "--width"
        };
        return Err(
            format!("{what} renders frames, which --events does not: drop one of them").into(),
        );
    }
    let last = cli
        .until
        .or_else(|| wanted(cli).ok().and_then(|t| t.last().copied()))
        .unwrap_or(0.0);
    let step = 1.0 / cli.fps.max(1.0);

    let mut renderer = match cli.events {
        true => None,
        false => Some(Renderer::new().map_err(|e| format!("no renderer: {e}"))?),
    };
    if renderer.is_some() && !cli.profile {
        std::fs::create_dir_all(&cli.out).map_err(|e| format!("{}: {e}", cli.out.display()))?;
    }
    let mut profiled = cli.profile.then(Profiled::default);

    // Walked in fixed steps from 0, so the run is repeatable and a frame
    // at a time is reached the same way however many were asked for.
    let mut time = 0.0;
    let mut steps = 0u64;
    let mut frames = 0;
    // An input is applied at its own instant, with the driver's steps
    // due before it applied first, the way a seek replays: the same
    // path a driver step takes, so a trigger asked for at 1.0 s starts
    // what it starts at 1.0 s whatever the frame rate.
    let apply = |engine: &mut Engine, input: Input| {
        engine.advance_to(input.at);
        match input.what {
            Step::Trigger { trigger } => engine.trigger(&trigger),
            Step::Set { set } => {
                for (name, value) in set {
                    engine.set_variable(&name, value);
                }
            }
            _ => {}
        }
    };
    loop {
        // What is due on the frame itself, before it is drawn.
        while inputs.peek().is_some_and(|i| i.at <= time) {
            apply(&mut engine, inputs.next().expect("peeked"));
        }
        // Profiling: every frame, resolved and drawn, and its costs kept.
        if let (Some(profiled), Some(renderer)) = (&mut profiled, &mut renderer) {
            show_video_frames(&mut engine, &clips, &mut quiet);
            let mut frame = engine.profile().map_err(|e| e.to_string())?;
            let items = std::mem::take(&mut frame.items);
            let count = items.len();
            let started = std::time::Instant::now();
            renderer
                .render_items_to_rgba(&engine, items)
                .map_err(|e| e.to_string())?;
            let draw = started.elapsed();
            profiled.add(time, frame, count, draw);
        }
        // A frame is due once the clock has reached it.
        while times.peek().is_some_and(|t| *t <= time + step / 2.0) {
            let at = times.next().expect("peeked");
            if let Some(renderer) = &mut renderer {
                show_video_frames(&mut engine, &clips, &mut quiet);
                let file = cli.out.join(format!("t{at:08.3}.png"));
                let size = engine.show().ok_or("no show")?.size;
                let target = match (cli.scale, cli.width) {
                    (None, None) => None,
                    (Some(scale), _) => {
                        let [w, h] = size;
                        // Checked: wrapping here would land on a size the
                        // renderer is happy with and quietly draw the
                        // wrong one.
                        let both = w.checked_mul(scale).zip(h.checked_mul(scale));
                        let (w, h) = both.ok_or_else(|| {
                            format!("--scale {scale} is past what {w}x{h} can be scaled to")
                        })?;
                        Some([w, h])
                    }
                    (None, Some(width)) => Some(sized_to(size, width)?),
                };
                let frame = match target {
                    None => renderer.render_to_rgba(&engine),
                    Some(target) => renderer.present_to_rgba(&engine, target),
                };
                frame
                    .map_err(|e| e.to_string())?
                    .write_png(&file)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                frames += 1;
            }
        }
        if cli.events {
            // The show's trace: each record at the instant it happened
            // on the show's own clock, not the frame that noticed it,
            // and with its cause.
            for traced in engine.drain_trace() {
                say(&format!("{:8.3}  {}", traced.at, traced.what))?;
            }
        }
        if time >= last {
            break;
        }
        steps += 1;
        // Multiplied, not accumulated, and handed to the engine as the
        // instant to land on rather than as a delta, so a run at one
        // frame rate reaches a given time in exactly the state a run at
        // another does.
        let next = steps as f64 * step;
        // Inputs inside the frame, each at its instant, the driver's
        // steps due before each one first; then the driver to the
        // frame's end.
        let mut drive = |engine: &mut Engine, to: f64| -> Result<(), Stop> {
            if let Some(driver) = &mut driver {
                let dt = to - engine.time();
                for played in driver.advance(engine.core_mut(), dt) {
                    if cli.events {
                        say(&format!("{:8.3}  driver {:?}", played.at, played.step))?;
                    }
                }
            }
            Ok(())
        };
        while inputs.peek().is_some_and(|i| i.at < next) {
            let input = inputs.next().expect("peeked");
            drive(&mut engine, input.at)?;
            apply(&mut engine, input);
        }
        drive(&mut engine, next)?;
        time = next;
        engine.advance_to(time);
    }
    if frames > 0 {
        say(&format!("{frames} frame(s) in {}", cli.out.display()))?;
    }
    if let Some(profiled) = profiled {
        let show = engine.show().ok_or("no show")?;
        let name = show.name.clone();
        let size = show.size;
        for line in profiled.report(
            &name,
            size,
            cli.fps,
            engine.text_stats(),
            engine.image_bytes(),
            cli.top,
        ) {
            say(&line)?;
        }
    }
    Ok(())
}

/// What a layer cost over the run, added up frame by frame.
#[derive(Debug, Default)]
struct LayerTotals {
    name: String,
    kind: &'static str,
    frames: u64,
    own: std::time::Duration,
    total: std::time::Duration,
    items: usize,
    path_elements: usize,
    glyphs: usize,
    pixels: f64,
    text_misses: u64,
}

/// A measure of one thing over the run, frame by frame.
#[derive(Debug, Default)]
struct Measure {
    samples: Vec<(f64, std::time::Duration)>,
}

/// What a measure came to over the frames that count.
struct Summary {
    counted: usize,
    mean: std::time::Duration,
    worst: std::time::Duration,
    worst_at: f64,
}

impl Measure {
    fn add(&mut self, at: f64, took: std::time::Duration) {
        self.samples.push((at, took));
    }

    /// Over the frames from `warm_up` on: the first frames build
    /// pipelines and upload what the show needs, which outlasts the
    /// first frame, and says nothing about the frames after.
    fn summary(&self, warm_up: f64) -> Option<Summary> {
        let counted: Vec<_> = self
            .samples
            .iter()
            .filter(|(at, _)| *at >= warm_up)
            .collect();
        let (mut worst, mut worst_at) = (std::time::Duration::ZERO, 0.0);
        let mut total = std::time::Duration::ZERO;
        for (at, took) in &counted {
            total += *took;
            if *took > worst {
                (worst, worst_at) = (*took, *at);
            }
        }
        (!counted.is_empty()).then(|| Summary {
            counted: counted.len(),
            mean: total / counted.len() as u32,
            worst,
            worst_at,
        })
    }
}

/// What a run cost, frame by frame, for the report at its end.
#[derive(Debug, Default)]
struct Profiled {
    frames: u64,
    resolve: Measure,
    draw: Measure,
    seen: usize,
    bindings: usize,
    timelines: usize,
    items: usize,
    layers: BTreeMap<String, LayerTotals>,
}

impl Profiled {
    /// Keep what `frame` cost: its resolve, the `draw` it took, and
    /// `items`, how many draw items it had (the list itself has gone to
    /// the renderer).
    fn add(
        &mut self,
        at: f64,
        frame: cuelight::FrameProfile,
        items: usize,
        draw: std::time::Duration,
    ) {
        self.frames += 1;
        self.resolve.add(at, frame.resolve);
        self.draw.add(at, draw);
        self.seen += frame.layers.len();
        self.bindings += frame.bindings;
        self.timelines += frame.timelines;
        self.items += items;
        for cost in frame.layers {
            let totals = self.layers.entry(cost.layer.to_string()).or_default();
            totals.name = cost.name;
            totals.kind = cost.kind;
            totals.frames += 1;
            totals.own += cost.own;
            totals.total += cost.total;
            totals.items += cost.items;
            totals.path_elements += cost.path_elements;
            totals.glyphs += cost.glyphs;
            totals.pixels += cost.pixels;
            totals.text_misses += cost.text_misses;
        }
    }

    /// The report: the frame as a whole, then the layers that cost the
    /// most to resolve, the ones that ask the most of the renderer, and
    /// the ones that keep the text rasterizer busy.
    fn report(
        &self,
        name: &str,
        [w, h]: [u32; 2],
        fps: f64,
        text: cuelight::TextStats,
        image_bytes: usize,
        top: usize,
    ) -> Vec<String> {
        let frames = self.frames.max(1) as f64;
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        // Below a megabyte, kilobytes read; a dot matrix's rasters are tiny.
        let size = |bytes: u64| match bytes < 1024 * 1024 {
            true => format!("{:.0} KB", bytes as f64 / 1024.0),
            false => format!("{:.1} MB", mb(bytes)),
        };
        // The first second builds pipelines and uploads what the show
        // needs, which outlasts the first frame; a run too short to
        // spare a second counts everything after its first frame.
        let last = self.resolve.samples.last().map_or(0.0, |(at, _)| *at);
        let warm_up = if last >= 2.0 { 1.0 } else { f64::MIN_POSITIVE };
        let mut out = vec![format!(
            "profiled {name:?} ({w}x{h}): {} frames at {fps} fps, {:.2} s",
            self.frames,
            self.frames as f64 / fps.max(1.0)
        )];
        for (what, measure) in [("resolve", &self.resolve), ("draw", &self.draw)] {
            let Some(summary) = measure.summary(warm_up) else {
                continue;
            };
            out.push(format!(
                "  {what:8} {:8.3} ms mean  {:8.3} ms worst, at {:.3} s  (over {} frames, the first {} of warm-up aside)",
                ms(summary.mean),
                ms(summary.worst),
                summary.worst_at,
                summary.counted,
                if warm_up >= 1.0 { "second".to_owned() } else { "frame".to_owned() }
            ));
        }
        out.push(
            "  draw is the renderer's frame at canvas size, read back to the CPU as well; a player draws the same and reads nothing back"
                .to_owned(),
        );
        out.push(format!(
            "  per frame {:.1} layers seen, {:.1} bindings, {:.1} timelines running, {:.1} draw items",
            self.seen as f64 / frames,
            self.bindings as f64 / frames,
            self.timelines as f64 / frames,
            self.items as f64 / frames
        ));
        let asked = text.hits + text.misses;
        if asked > 0 {
            out.push(format!(
                "  text     {asked} rasterizations: {} fresh ({:.1}% served from the cache), {} \
                 rasterized; the cache holds {} of {:.0} MB",
                text.misses,
                text.hits as f64 / asked as f64 * 100.0,
                size(text.rasterized_bytes),
                size(text.cached_bytes as u64),
                mb(text.budget_bytes as u64)
            ));
        }
        let resident = peak_resident_bytes().map_or("unknown here".to_owned(), |b| {
            format!("{:.0} MB resident at its peak", mb(b))
        });
        out.push(format!(
            "  memory   {resident}; images {} decoded",
            size(image_bytes as u64)
        ));
        let mut by_time: Vec<_> = self
            .layers
            .iter()
            .filter(|(_, t)| t.own > std::time::Duration::ZERO)
            .collect();
        by_time.sort_by_key(|(_, t)| std::cmp::Reverse(t.own));
        if !by_time.is_empty() {
            out.push(
                "layers by resolve time, their own work, mean per frame they were seen:".to_owned(),
            );
            for (path, t) in by_time.iter().take(top) {
                let per = t.frames.max(1) as f64;
                let text = match t.text_misses {
                    0 => String::new(),
                    n => format!(", {:.2} fresh strings", n as f64 / per),
                };
                out.push(format!(
                    "  {:8.3} ms  {path} {:?} ({}), {:.1} items{text}",
                    ms(t.own) / per,
                    t.name,
                    t.kind,
                    t.items as f64 / per
                ));
            }
        }
        // Leaves only: a group's items are its children's, and would be
        // counted once for each level above them.
        let weight = |t: &LayerTotals| t.path_elements + t.glyphs;
        let mut by_weight: Vec<_> = self
            .layers
            .iter()
            .filter(|(_, t)| t.kind != "group" && (weight(t) > 0 || t.pixels > 0.0))
            .collect();
        by_weight.sort_by(|a, b| {
            let (wa, wb) = (
                weight(a.1) as f64 + a.1.pixels / 1000.0,
                weight(b.1) as f64 + b.1.pixels / 1000.0,
            );
            wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
        });
        if !by_weight.is_empty() {
            out.push("layers by what they ask of the renderer, mean per frame they were seen (leaves only):".to_owned());
            for (path, t) in by_weight.iter().take(top) {
                let per = t.frames.max(1) as f64;
                out.push(format!(
                    "  {:8.0} path elements, {:6.0} glyphs, {:8.0} kpx  {path} {:?} ({})",
                    t.path_elements as f64 / per,
                    t.glyphs as f64 / per,
                    t.pixels / per / 1000.0,
                    t.name,
                    t.kind
                ));
            }
        }
        // Leaves only, as for the renderer: a group's misses are its
        // children's.
        let mut by_text: Vec<_> = self
            .layers
            .iter()
            .filter(|(_, t)| t.kind != "group" && t.text_misses > 0)
            .collect();
        by_text.sort_by_key(|(_, t)| std::cmp::Reverse(t.text_misses));
        if !by_text.is_empty() {
            out.push("layers by strings rasterized afresh, over the run:".to_owned());
            for (path, t) in by_text.iter().take(top) {
                out.push(format!(
                    "  {:8} strings  {path} {:?} ({})",
                    t.text_misses, t.name, t.kind
                ));
            }
        }
        out
    }
}

/// The most memory this process has had resident, where the system
/// says: Linux does, through `/proc`.
fn peak_resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn main() -> std::process::ExitCode {
    let cli = <Cli as clap::Parser>::parse();
    match run(&cli) {
        Ok(()) | Err(Stop::PipeClosed) => std::process::ExitCode::SUCCESS,
        Err(Stop::Failed(e)) => {
            eprintln!("cuelight-render: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> Cli {
        Cli {
            show: PathBuf::from("show"),
            at: Vec::new(),
            every: None,
            until: None,
            fps: 60.0,
            out: PathBuf::from("frames"),
            events: false,
            no_driver: false,
            lenient: false,
            profile: false,
            top: 10,
            triggers: Vec::new(),
            sets: Vec::new(),
            scale: None,
            width: None,
        }
    }

    #[test]
    fn a_width_picks_a_whole_factor_either_way() {
        // Up by the largest whole factor that fits.
        assert_eq!(sized_to([128, 32], 640).unwrap(), [640, 160]);
        assert_eq!(sized_to([192, 64], 640).unwrap(), [576, 192]);
        // Exactly the width already.
        assert_eq!(sized_to([640, 360], 640).unwrap(), [640, 360]);
        // Down to a whole fraction: 1920 wants three, 1280 two.
        assert_eq!(sized_to([1920, 1080], 640).unwrap(), [640, 360]);
        assert_eq!(sized_to([1280, 720], 640).unwrap(), [640, 360]);
        // Wider than the target by a hair still comes down a whole step,
        // so a width is a bound rather than a promise.
        assert_eq!(sized_to([960, 540], 640).unwrap(), [480, 270]);
    }

    #[test]
    fn a_width_never_gives_nothing_to_draw() {
        // A show far wider than the target keeps at least one pixel.
        assert_eq!(sized_to([4000, 3], 1).unwrap(), [1, 1]);
    }

    #[test]
    fn times_come_back_in_order() {
        let mut c = cli();
        c.at = vec![26.0, 4.1, 19.5];
        assert_eq!(wanted(&c).unwrap(), [4.1, 19.5, 26.0]);
    }

    #[test]
    fn a_strip_covers_its_end() {
        let mut c = cli();
        (c.every, c.until) = (Some(0.5), Some(2.0));
        assert_eq!(wanted(&c).unwrap(), [0.0, 0.5, 1.0, 1.5, 2.0]);
    }

    #[test]
    fn a_strip_needs_an_end_and_a_step_above_zero() {
        let mut c = cli();
        c.every = Some(0.5);
        assert!(wanted(&c).is_err(), "no --until");
        (c.every, c.until) = (Some(0.0), Some(2.0));
        assert!(wanted(&c).is_err(), "a step of nothing never arrives");
    }

    #[test]
    fn inputs_are_read_and_ordered_by_their_time() {
        let mut c = cli();
        c.triggers = vec!["2.5:go".into()];
        c.sets = vec!["0:score=1500".into(), "1:mode=multiball".into()];
        let inputs = inputs(&c).unwrap();
        let times: Vec<f64> = inputs.iter().map(|i| i.at).collect();
        assert_eq!(times, [0.0, 1.0, 2.5]);
        // A number stays a number and anything else is text.
        match &inputs[0].what {
            Step::Set { set } => assert_eq!(set["score"], cuelight_core::Value::Number(1500.0)),
            other => panic!("{other:?}"),
        }
        match &inputs[1].what {
            Step::Set { set } => {
                assert_eq!(set["mode"], cuelight_core::Value::Text("multiball".into()));
            }
            other => panic!("{other:?}"),
        }
        match &inputs[2].what {
            Step::Trigger { trigger } => assert_eq!(trigger, "go"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_malformed_input_says_what_it_wanted() {
        let mut c = cli();
        c.triggers = vec!["go".into()];
        assert!(inputs(&c).unwrap_err().contains("needs a time"));
        c.triggers.clear();
        c.sets = vec!["0:score".into()];
        assert!(inputs(&c).unwrap_err().contains("needs a value"));
    }
}
