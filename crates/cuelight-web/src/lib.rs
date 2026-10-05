//! Play `cuelight` shows in a browser: a show folder fetched over HTTP,
//! rendered into a canvas with WebGPU, driven from JavaScript.
//!
//! ```js
//! import init, { CuelightPlayer } from "./pkg/cuelight_web.js";
//!
//! await init();
//! const player = await CuelightPlayer.create(canvas, "shows/beacon/");
//! player.actions();            // ["go", ...]
//! player.listeners();          // { go: { where: "anywhere" }, slide_2: { where: "opens", scene: "two" }, ... }
//! player.trigger("go");
//! player.set("score", 1200);
//! player.onEvent((event) => console.log(event));
//! player.onDriver((step) => console.log(step));   // { type: "trigger", name: "go", at: 0.3 }
//! player.onError((message) => alert(message));
//! ```
//!
//! What the folder's driver script does as it plays reaches the page
//! through `onDriver`, one call per step as it fires: a trigger as
//! `{ type: "trigger", name, at }` and a variable set as
//! `{ type: "set", name, value, at }`, with `at` the driver's own
//! instant on the show's clock. A page can then show why the show did
//! what it did, next to the show's own events from `onEvent` and its
//! own clicks and keys.
//!
//! The show folder needs a `manifest.json` (the `cuelight-manifest` tool of
//! `cuelight-loader` writes it): a browser cannot list a directory. A
//! packed show (`shows/beacon.cuelight`) needs nothing: it is one fetch,
//! unpacked here. Either way its `test-driver.json`, when there is one,
//! starts playing right away.
//!
//! The page sizes the canvas with CSS; the player keeps the canvas's pixel
//! size in step with it and fits the show inside. Frames follow
//! `requestAnimationFrame`, so a hidden tab costs nothing and the show
//! picks up where it was.
//!
//! `demo/build.sh` shows the build: `cargo build` for `wasm32`, then
//! `wasm-bindgen --target web`. It is a plain build; for production see the
//! note in that script on shrinking the download.
//!
//! Video plays through the browser: one `<video>` element per clip the
//! show ships, off the page, decoding what the engine says is playing,
//! its frames drawn into a canvas and copied from there on the GPU (every
//! browser copies a canvas; not every one copies a `<video>`) into a
//! texture the renderer draws where
//! the layer is, with rotation, tint, blend modes and the output passes
//! as any image. The element runs on its own clock and is put right
//! only when it is off by more than a moment, which is what a loop, a
//! seek or a stall looks like. A clip's soundtrack is the element's
//! own sound: its volume follows the layer's `gain`, the groups above
//! it and any ducking, through the engine's voice for the play, and it
//! stays muted until the first gesture lets sound through, as sounds
//! do. Buses do not apply to it. Every clip's length is read before the
//! clock starts, so `on_end` lands where the show says on any
//! connection. Each video layer that plays has an element of its own,
//! so two layers on one clip run at their own positions; a browser
//! decodes only a handful at once, so a show with many wants them few.
//! A browser lets the page have a clip's pixels only when it is from the
//! page's own origin (or served with CORS): a show fetched from elsewhere
//! plays its sound and draws no picture, with a warning in the console.
//!
//! Sound goes through WebAudio: the folder's `assets/sounds/` are decoded
//! by the browser and the engine's voice list drives buffer sources and
//! gain nodes (`cuelight_audio::WebAudio`). Browsers keep audio silent until the page has
//! been clicked or typed into; the player resumes its context on the first
//! such gesture, and `player.audioRunning` says whether it has.
//! `player.audioEnabled = false` silences a show and keeps it silent
//! through later gestures; `true` lets it through again.
//!
//! A press on the canvas goes through `player.press(x, y)`, which fires
//! what the layer under it fires and, for a layer that opens a web
//! address, opens it in a new tab right there, since a browser allows a
//! new tab only from the gesture itself; the page also hears of it as
//! `{ type: "open", url }` through `onEvent`, and need not open it
//! again. `player.pressedAt(x, y)` says what a press would do, as
//! `{ trigger?, open? }`, for a pointer cursor.
//!
//! The show is fitted inside the canvas, keeping its shape, with the
//! canvas's own background showing beside it. `player.fit = "cover"`
//! fills the canvas instead, cutting the canvas edges on the long axis,
//! and `"fill"` fills it losing the show's shape; `"contain"` is the
//! default. Whoever owns the page decides this, not the show.
//!
//! WebGPU only: without it `CuelightPlayer.create` rejects with a message
//! saying so. What goes wrong on the GPU afterwards (a shader that did
//! not compile, a validation error) stops the frames rather than drawing
//! black on: it is logged, listed by `player.warnings()` and handed to
//! `player.onError`, so a page can show it where a phone user can read
//! it. A frame that panics is handed to `onError` the same way, and the
//! player is dead after it: its frames stop and every call answers as if
//! there were no show. The crate is empty on targets other than `wasm32`.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

use cuelight::render::{Fit, Presenter};
use cuelight::vello;
use cuelight::Engine;
use cuelight_audio::WebAudio;
use cuelight_core::{Event, Value};
use cuelight_loader::{
    Applied, Driver, DriverPlayer, Manifest, Step, MANIFEST_FILE, VIDEO_EXTENSIONS,
};
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlCanvasElement;

/// A gap between frames past which the show is not caught up with, in
/// seconds: longer than any hitch, and shorter than a tab left in the
/// background. A tab coming back continues from where it stopped
/// instead of playing through everything it missed.
const A_STALL: f64 = 5.0;

type FrameCallback = Closure<dyn FnMut(f64)>;
/// An event name and the handler listening for it on the document.
type GestureListener = (String, Closure<dyn FnMut()>);

fn error(message: impl AsRef<str>) -> JsValue {
    js_sys::Error::new(message.as_ref()).into()
}

fn window() -> Result<web_sys::Window, JsValue> {
    web_sys::window().ok_or_else(|| error("no window: not running in a page"))
}

async fn fetch(url: &str) -> Result<Vec<u8>, JsValue> {
    let response: web_sys::Response = JsFuture::from(window()?.fetch_with_str(url))
        .await
        .map_err(|_| error(format!("{url}: request failed")))?
        .dyn_into()?;
    if !response.ok() {
        return Err(error(format!("{url}: HTTP {}", response.status())));
    }
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// A show as fetched: its files, and where its clips are.
struct Fetched {
    files: BTreeMap<String, Vec<u8>>,
    /// Each clip by the name the show plays it under, and a URL a
    /// `<video>` element can play: the file where it lies for a folder,
    /// a blob of its bytes for a pack.
    clips: Vec<(String, String)>,
}

/// Whether `file`, a path in the show, is a clip: the browser plays
/// those from where they are rather than reading them into memory.
fn is_clip(file: &str) -> bool {
    file.rsplit_once('.')
        .is_some_and(|(_, e)| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The name a show plays the clip at `file` under: its stem for a file
/// in `assets/videos/`, the path itself for one named by path.
fn clip_name(file: &str) -> String {
    match file.strip_prefix("assets/videos/") {
        Some(rest) => rest
            .rsplit_once('.')
            .map_or(rest, |(stem, _)| stem)
            .to_owned(),
        None => file.to_owned(),
    }
}

/// Fetch a show: a packed `.cuelight` file, unpacked here, or the folder
/// at `base` (ending in `/`): its manifest, then the files it lists, all
/// at once. Clips are not read: a folder's stay where they are, and a
/// pack's become blobs, for the `<video>` elements to play.
async fn fetch_show(base: &str) -> Result<Fetched, JsValue> {
    if base.ends_with(".cuelight") {
        let bytes = fetch(base).await?;
        let mut files =
            cuelight_loader::unpack(&bytes).map_err(|e| error(format!("{base}: {e}")))?;
        let names: Vec<String> = files.keys().filter(|f| is_clip(f)).cloned().collect();
        let mut clips = Vec::new();
        for name in names {
            let bytes = files.remove(&name).unwrap_or_default();
            let array = js_sys::Uint8Array::from(bytes.as_slice());
            let parts = js_sys::Array::new();
            parts.push(&array.buffer());
            let blob = web_sys::Blob::new_with_buffer_source_sequence(&parts)?;
            let url = web_sys::Url::create_object_url_with_blob(&blob)?;
            clips.push((clip_name(&name), url));
        }
        return Ok(Fetched { files, clips });
    }
    let manifest_url = format!("{base}{MANIFEST_FILE}");
    let manifest = String::from_utf8(fetch(&manifest_url).await?)
        .map_err(|e| e.to_string())
        .and_then(|json| Manifest::from_json(&json))
        .map_err(|e| error(format!("{manifest_url}: {e}")))?;
    let clips = manifest
        .files
        .iter()
        .filter(|f| is_clip(f))
        .map(|f| (clip_name(f), format!("{base}{f}")))
        .collect();
    let fetches = manifest
        .files
        .iter()
        .filter(|f| !is_clip(f))
        .map(|file| async move {
            let bytes = fetch(&format!("{base}{file}")).await?;
            Ok::<_, JsValue>((file.clone(), bytes))
        });
    let files = futures_util::future::try_join_all(fetches)
        .await?
        .into_iter()
        .collect();
    Ok(Fetched { files, clips })
}

/// A `<video>` element for the clip at `url`, off the page, with its
/// length and size read: the engine wants both before the clock starts,
/// so that a clip's end lands where the show says on any connection.
async fn open_clip(url: &str) -> Result<(web_sys::HtmlVideoElement, f64, [u32; 2]), JsValue> {
    let document = window()?.document().ok_or_else(|| error("no document"))?;
    let element: web_sys::HtmlVideoElement = document.create_element("video")?.dyn_into()?;
    element.set_muted(true);
    element.set_preload("auto");
    // Inline on phones, where a video would otherwise take the screen.
    element.set_attribute("playsinline", "")?;
    element.set_src(url);
    let ready = js_sys::Promise::new(&mut |resolve, reject| {
        let _ = element.add_event_listener_with_callback("loadedmetadata", &resolve);
        let _ = element.add_event_listener_with_callback("error", &reject);
    });
    JsFuture::from(ready)
        .await
        .map_err(|_| error(format!("{url}: the browser could not open the clip")))?;
    let duration = element.duration();
    if !duration.is_finite() || duration <= 0.0 {
        return Err(error(format!("{url}: the clip has no length")));
    }
    let size = [element.video_width(), element.video_height()];
    Ok((element, duration, size))
}

/// A clip the show ships: where the browser plays it from, and the
/// element its length was read with, kept for the first layer to play it.
struct Clip {
    url: String,
    spare: Option<web_sys::HtmlVideoElement>,
}

/// A `<video>` element for the clip at `url`, off the page, muted until
/// a play says otherwise.
fn clip_element(url: &str) -> Result<web_sys::HtmlVideoElement, JsValue> {
    let document = window()?.document().ok_or_else(|| error("no document"))?;
    let element: web_sys::HtmlVideoElement = document.create_element("video")?.dyn_into()?;
    element.set_muted(true);
    element.set_preload("auto");
    // Inline on phones, where a video would otherwise take the screen.
    element.set_attribute("playsinline", "")?;
    element.set_src(url);
    Ok(element)
}

/// Let an element's decoder go at once: a browser keeps it until the
/// element is collected otherwise, and has only so many.
fn release(element: &web_sys::HtmlVideoElement) {
    element.pause().ok();
    let _ = element.remove_attribute("src");
    element.load();
}

/// What one video layer shows: an element of its own, so two layers on
/// one clip each run at their own position, and the texture its frames
/// are copied into, drawn under the layer's frame key.
struct Screen {
    /// The clip it plays, by the name the show plays it under.
    video: String,
    element: web_sys::HtmlVideoElement,
    texture: Option<wgpu::Texture>,
    size: [u32; 2],
    /// Whether the texture is registered with the renderer under the key.
    drawn: bool,
    /// The browser refused to play it (an autoplay policy, a decode
    /// error): not asked again until the next gesture, so a refusal is
    /// one line in the console and not one a frame.
    refused: Rc<std::cell::Cell<bool>>,
    /// What a refusal calls, made once for the element.
    on_refused: Closure<dyn FnMut(JsValue)>,
    /// The canvas each frame is drawn into before it is copied to the
    /// GPU: every browser copies a canvas, and not every one copies a
    /// `<video>`.
    canvas: Option<(
        web_sys::OffscreenCanvas,
        web_sys::OffscreenCanvasRenderingContext2d,
    )>,
    /// The clip is from another origin, without CORS: the browser keeps
    /// its pixels from the page, so it plays its sound and is not drawn.
    foreign: bool,
}

impl Screen {
    fn new(video: &str, element: web_sys::HtmlVideoElement) -> Screen {
        let refused = Rc::new(std::cell::Cell::new(false));
        let flag = refused.clone();
        let name = video.to_owned();
        let on_refused = Closure::new(move |why: JsValue| {
            if !flag.get() {
                web_sys::console::warn_1(
                    &format!(
                        "cuelight: clip {name:?} would not play: {}",
                        js_error_text(&why)
                    )
                    .into(),
                );
            }
            flag.set(true);
        });
        Screen {
            video: video.to_owned(),
            element,
            texture: None,
            size: [0, 0],
            drawn: false,
            refused,
            on_refused,
            canvas: None,
            foreign: false,
        }
    }

    /// The element's frame now, drawn into a canvas of `size` for the
    /// GPU to copy; `None` when there is nothing to copy. A clip the
    /// browser keeps from the page is found out on its first frame,
    /// since copying its canvas would fail inside the GPU call.
    fn frame(&mut self, size: [u32; 2]) -> Option<web_sys::OffscreenCanvas> {
        if self.foreign {
            return None;
        }
        let fresh = self
            .canvas
            .as_ref()
            .is_none_or(|(canvas, _)| [canvas.width(), canvas.height()] != size);
        if fresh {
            let canvas = web_sys::OffscreenCanvas::new(size[0], size[1]).ok()?;
            let context = canvas
                .get_context("2d")
                .ok()??
                .dyn_into::<web_sys::OffscreenCanvasRenderingContext2d>()
                .ok()?;
            self.canvas = Some((canvas, context));
        }
        let (canvas, context) = self.canvas.as_ref()?;
        context
            .draw_image_with_html_video_element(&self.element, 0.0, 0.0)
            .ok()?;
        if fresh && context.get_image_data(0.0, 0.0, 1.0, 1.0).is_err() {
            web_sys::console::warn_1(
                &format!(
                    "cuelight: clip {:?} is from another origin without CORS: \
                     it plays its sound and is not drawn",
                    self.video
                )
                .into(),
            );
            self.foreign = true;
            return None;
        }
        Some(canvas.clone())
    }

    /// Ask the element to play, unless it refused since the last gesture.
    fn play(&self) {
        if self.refused.get() {
            return;
        }
        if let Ok(promise) = self.element.play() {
            let _ = promise.catch(&self.on_refused);
        }
    }
}

/// How far a clip may run from where the engine says it is before it
/// is put right: a seek is not frame-accurate and stutters, so it is
/// saved for a loop, a scrub, a restart or a stall.
const CLIP_SLACK: f64 = 0.25;

fn to_js(value: &Value) -> JsValue {
    match value {
        Value::Bool(b) => JsValue::from_bool(*b),
        Value::Number(n) => JsValue::from_f64(*n),
        Value::Text(text) => JsValue::from_str(text),
        _ => JsValue::UNDEFINED,
    }
}

fn from_js(value: &JsValue) -> Result<Value, JsValue> {
    if let Some(b) = value.as_bool() {
        Ok(Value::Bool(b))
    } else if let Some(n) = value.as_f64() {
        Ok(Value::Number(n))
    } else if let Some(text) = value.as_string() {
        Ok(Value::Text(text))
    } else {
        Err(error("a variable takes a boolean, a number or a string"))
    }
}

/// What one frame brought: the driver's steps that fired in it, and
/// the events the show raised.
struct Happened {
    applied: Vec<Applied>,
    events: Vec<Event>,
}

/// A driver step that fired, as the page sees it: a trigger as one
/// object, a `set` of several variables as one object per variable, a
/// wait as nothing.
fn applied_to_js(applied: &Applied) -> Vec<JsValue> {
    let object = |kind: &str, name: &str, value: Option<&Value>| {
        let object = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&object, &"type".into(), &kind.into());
        let _ = js_sys::Reflect::set(&object, &"name".into(), &name.into());
        if let Some(value) = value {
            let _ = js_sys::Reflect::set(&object, &"value".into(), &to_js(value));
        }
        let _ = js_sys::Reflect::set(&object, &"at".into(), &JsValue::from_f64(applied.at));
        JsValue::from(object)
    };
    match &applied.step {
        Step::Trigger { trigger } => vec![object("trigger", trigger, None)],
        Step::Set { set } => set
            .iter()
            .map(|(name, value)| object("set", name, Some(value)))
            .collect(),
        _ => Vec::new(),
    }
}

fn event_to_js(event: &Event) -> JsValue {
    let object = js_sys::Object::new();
    let set = |key: &str, value: &str| {
        let _ = js_sys::Reflect::set(&object, &key.into(), &value.into());
    };
    match event {
        Event::Trigger(name) => {
            set("type", "trigger");
            set("name", name);
        }
        Event::Open { url } => {
            set("type", "open");
            set("url", url);
        }
        _ => set("type", "unknown"),
    }
    object.into()
}

/// What stopped the player, kept apart from its state: a panic leaves
/// that borrowed for good, and the page must still hear of it.
#[derive(Default)]
struct Reports {
    warnings: RefCell<Vec<String>>,
    on_error: RefCell<Option<js_sys::Function>>,
}

impl Reports {
    /// Keep `message` with the warnings and hand it to the page.
    fn fail(&self, message: &str) {
        self.warnings.borrow_mut().push(message.to_owned());
        let on_error = self.on_error.borrow().clone();
        if let Some(on_error) = on_error {
            let _ = on_error.call1(&JsValue::NULL, &JsValue::from_str(message));
        }
    }
}

thread_local! {
    /// The reports of the player whose frame is running, for the panic
    /// hook: a frame runs from `requestAnimationFrame`, where a panic
    /// reaches nothing on the page.
    static IN_FRAME: RefCell<Option<Rc<Reports>>> = const { RefCell::new(None) };
}

/// Log a panic, and report one in a frame through that player's
/// `onError`. The player is dead after it: its state stays borrowed,
/// its frames stop and its calls answer as if it had no show.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            console_error_panic_hook::hook(info);
            let reports = IN_FRAME.with(|cell| cell.try_borrow_mut().ok()?.take());
            if let Some(reports) = reports {
                reports.fail(&format!("the player stopped: {info}"));
            }
        }));
    });
}

struct Inner {
    engine: Engine,
    script: Option<Driver>,
    /// What keys and presses fired while the show played, so a seek can
    /// put it back: they are inputs like the script's.
    live: cuelight_loader::Live,
    driver: Option<DriverPlayer>,
    driver_playing: bool,
    /// The clock is stopped: frames still paint, nothing advances.
    paused: bool,
    /// What the GPU reported since the last frame, from wgpu's
    /// uncaptured-error handler: it runs outside the frame, so the
    /// frame loop picks these up and stops.
    gpu_errors: Arc<Mutex<Vec<String>>>,
    canvas: HtmlCanvasElement,
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: vello::Renderer,
    presenter: Presenter,
    /// The page timestamp the show's time 0 was at, so the clock is read
    /// from it rather than added up frame by frame. Moved deliberately:
    /// paused, seeked, or after a gap long enough to be a background
    /// tab.
    anchor_ms: Option<f64>,
    on_event: Option<js_sys::Function>,
    on_driver: Option<js_sys::Function>,
    pending_frame: Option<i32>,
    /// The page's sound, when the browser gave us an audio context.
    audio: Option<WebAudio>,
    /// The show's clips, by the name the show plays them under.
    clips: HashMap<String, Clip>,
    /// What each video layer shows, by its frame key.
    screens: HashMap<String, Screen>,
}

impl Inner {
    /// Make the video layers match what the engine says is playing. Each
    /// layer that plays has an element of its own, so two layers on one
    /// clip run at their own positions: it runs at the play's position
    /// give or take [`CLIP_SLACK`], paused or not, its volume the play's
    /// voice's gain, muted until sound may be heard; its frame is copied
    /// on the GPU into the texture the renderer draws under the layer's
    /// frame key. A layer that stops playing lets its element go.
    fn sync_clips(&mut self, voices: &[cuelight_core::Voice]) {
        if self.clips.is_empty() {
            return;
        }
        let plays = self.engine.videos().unwrap_or_default();
        let audible = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.enabled() && audio.running());
        let Some(handle) = self.context.devices.get(self.surface.dev_id) else {
            return;
        };
        for play in &plays {
            let Some(clip) = self.clips.get_mut(&play.video) else {
                continue;
            };
            // A layer pointed at another clip starts that one afresh.
            let stale = self
                .screens
                .get(&play.frame)
                .is_some_and(|screen| screen.video != play.video);
            if stale {
                if let Some(old) = self.screens.remove(&play.frame) {
                    release(&old.element);
                    if old.drawn {
                        self.presenter
                            .set_external_image(&mut self.renderer, &play.frame, None);
                    }
                }
            }
            if !self.screens.contains_key(&play.frame) {
                let element = match clip.spare.take() {
                    Some(element) => element,
                    None => match clip_element(&clip.url) {
                        Ok(element) => element,
                        Err(_) => continue,
                    },
                };
                self.screens
                    .insert(play.frame.clone(), Screen::new(&play.video, element));
            }
            let Some(screen) = self.screens.get_mut(&play.frame) else {
                continue;
            };
            let element = &screen.element;
            // The clock: the element's own, put right only when it has
            // strayed, since a seek lands where the browser can and not
            // on the frame asked for. Paused, it is still put right, so a
            // seek while paused shows the frame it landed on.
            element.set_loop(play.looping);
            if (element.current_time() - play.position).abs() > CLIP_SLACK {
                element.set_current_time(play.position);
            }
            if self.paused {
                if !element.paused() {
                    element.pause().ok();
                }
            } else if element.paused() {
                screen.play();
            }
            // The sound: the layer's gain and everything above it, as
            // the engine worked it out for this play's voice.
            let gain = voices
                .iter()
                .find(|v| v.id == play.id)
                .map_or(0.0, |v| v.gain.clamp(0.0, 1.0));
            element.set_muted(!audible || gain <= 0.0);
            element.set_volume(gain);
            // The picture: the frame the element shows now, drawn into a
            // canvas and copied from there on the GPU into a texture the
            // renderer draws under this layer's key.
            if element.ready_state() < 2 {
                continue;
            }
            let size = [element.video_width(), element.video_height()];
            if size[0] == 0 || size[1] == 0 {
                continue;
            }
            if screen.texture.is_none() || screen.size != size {
                screen.texture = Some(handle.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("cuelight-clip"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    // Written by the browser's copy, which wants a render
                    // attachment; read by the renderer's copy into its atlas.
                    usage: wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                }));
                screen.size = size;
                screen.drawn = false;
            }
            let Some(frame) = screen.frame(size) else {
                continue;
            };
            let Some(texture) = &screen.texture else {
                continue;
            };
            handle.queue.copy_external_image_to_texture(
                &wgpu::CopyExternalImageSourceInfo {
                    source: wgpu::ExternalImageSource::OffscreenCanvas(frame),
                    origin: wgpu::Origin2d::ZERO,
                    flip_y: false,
                },
                wgpu::CopyExternalImageDestInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                    color_space: wgpu::PredefinedColorSpace::Srgb,
                    premultiplied_alpha: true,
                },
                wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
            );
            if !screen.drawn {
                self.presenter.set_external_image(
                    &mut self.renderer,
                    &play.frame,
                    Some(texture.clone()),
                );
                screen.drawn = true;
            }
            self.presenter
                .touch_external_image(&mut self.renderer, &play.frame);
            // The engine draws the layer only once an image is registered
            // under the key; the texture stands in for the pixels.
            if self.engine.image(&play.frame).is_none() {
                let _ = self.engine.set_image(&play.frame, 1, 1, vec![0; 4]);
            }
        }
        // A layer that stopped playing lets its element go, back to its
        // clip for the next layer when the clip has none spare.
        let gone: Vec<String> = self
            .screens
            .keys()
            .filter(|key| !plays.iter().any(|p| p.frame == **key))
            .cloned()
            .collect();
        for key in gone {
            let Some(screen) = self.screens.remove(&key) else {
                continue;
            };
            screen.element.pause().ok();
            if screen.drawn {
                self.presenter
                    .set_external_image(&mut self.renderer, &key, None);
            }
            match self.clips.get_mut(&screen.video) {
                Some(clip) if clip.spare.is_none() => clip.spare = Some(screen.element),
                // Not needed: let its decoder go now rather than when the
                // element is collected, since a browser has only so many.
                _ => release(&screen.element),
            }
        }
    }

    /// Pixels per CSS pixel on this screen, for turning a pointer's
    /// place on the element into a place on the surface.
    fn pixel_ratio(&self) -> f64 {
        web_sys::window().map_or(1.0, |w| w.device_pixel_ratio())
    }

    /// Pixel size the canvas should have for its CSS size on this screen.
    fn wanted_size(&self) -> (u32, u32) {
        let ratio = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
        let max = self
            .context
            .devices
            .get(self.surface.dev_id)
            .map_or(1, |handle| handle.device.limits().max_texture_dimension_2d);
        let pixels = |css: i32| ((f64::from(css) * ratio).round() as u32).clamp(1, max);
        (
            pixels(self.canvas.client_width()),
            pixels(self.canvas.client_height()),
        )
    }

    /// Keep the canvas's pixel size in step with its CSS size.
    fn sync_size(&mut self) {
        let (width, height) = self.wanted_size();
        if (width, height) == (self.surface.config.width, self.surface.config.height)
            && (width, height) == (self.canvas.width(), self.canvas.height())
        {
            return;
        }
        let css = (self.canvas.client_width(), self.canvas.client_height());
        self.canvas.set_width(width);
        self.canvas.set_height(height);
        // A canvas without a CSS size takes its layout size from the pixel
        // size just set, which would grow it every frame on a high-density
        // screen: pin it to the size it had.
        if (self.canvas.client_width(), self.canvas.client_height()) != css {
            let style = self.canvas.style();
            let _ = style.set_property("width", &format!("{}px", css.0));
            let _ = style.set_property("height", &format!("{}px", css.1));
        }
        self.context
            .resize_surface(&mut self.surface, width, height);
    }

    /// Advance the show to `now_ms` and draw it. Returns the driver's
    /// steps that fired and the show's events, for the caller to deliver
    /// once the player is no longer borrowed.
    fn frame(&mut self, now_ms: f64) -> Result<Happened, String> {
        // Paused stops the clock, not the painting: a resize, a seek or a
        // variable set from the page still shows. The anchor rides along
        // under the time the show is stopped at, so playing carries on
        // from there.
        let time = self.engine.time();
        if self.paused || self.anchor_ms.is_none() {
            self.anchor_ms = Some(now_ms - time * 1000.0);
        }
        let anchor = self.anchor_ms.unwrap_or(now_ms);
        let mut target = (now_ms - anchor) / 1000.0;
        let mut dt = (target - time).max(0.0);
        // A frame that took a moment is caught up with, since sound
        // plays on whatever the page is doing and a show that dropped
        // that time would be out against its own soundtrack for good. A
        // gap long enough to be a background tab is not a frame to catch
        // up with: the show goes on from where it stopped.
        if dt > A_STALL {
            self.anchor_ms = Some(now_ms - time * 1000.0);
            (target, dt) = (time, 0.0);
        }
        let mut applied = Vec::new();
        if self.driver_playing && !self.paused {
            if let Some(driver) = &mut self.driver {
                applied = driver.advance(self.engine.core_mut(), dt);
            }
        }
        self.engine.advance_to(target);
        let events = self.engine.drain_events();
        let voices = self.engine.voices().unwrap_or_default();
        if let Some(audio) = &mut self.audio {
            audio.apply(&voices);
        }
        self.sync_clips(&voices);

        self.sync_size();
        let surface = &self.surface;
        let (width, height) = (surface.config.width, surface.config.height);
        let handle = self
            .context
            .devices
            .get(surface.dev_id)
            .ok_or("no device for the canvas")?;
        let presented = self
            .presenter
            .present(
                &self.engine,
                &handle.device,
                &handle.queue,
                &mut self.renderer,
                [width, height],
            )
            .map_err(|e| e.to_string())?;
        self.renderer
            .render_to_texture(
                &handle.device,
                &handle.queue,
                &presented.scene,
                &surface.target_view,
                &vello::RenderParams {
                    base_color: presented.base_color,
                    width,
                    height,
                    antialiasing_method: vello::AaConfig::Area,
                },
            )
            .map_err(|e| e.to_string())?;
        let texture = match surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Skip this frame; the next one retries.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.context.configure_surface(surface);
                return Ok(Happened { applied, events });
            }
            _ => return Ok(Happened { applied, events }),
        };
        let mut encoder = handle
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cuelight-surface-blit"),
            });
        surface.blitter.copy(
            &handle.device,
            &mut encoder,
            &surface.target_view,
            &texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        handle.queue.submit([encoder.finish()]);
        texture.present();
        Ok(Happened { applied, events })
    }
}

/// A show playing in a canvas. Call `free()` to stop it.
#[wasm_bindgen]
pub struct CuelightPlayer {
    inner: Rc<RefCell<Inner>>,
    reports: Rc<Reports>,
    // Owns the frame callback; the callback itself only holds weak
    // references, so dropping the player ends the loop.
    _frame: Rc<RefCell<Option<FrameCallback>>>,
    // The gesture listeners that wake the audio context, removed on drop.
    _gestures: Vec<GestureListener>,
}

#[wasm_bindgen]
impl CuelightPlayer {
    /// Fetch the show folder at `url` and start playing it in `canvas`.
    pub async fn create(canvas: HtmlCanvasElement, url: String) -> Result<CuelightPlayer, JsValue> {
        install_panic_hook();
        // wgpu and vello say what went wrong through `log`; without a
        // logger that went nowhere.
        let _ = console_log::init_with_level(log::Level::Warn);
        let gpu = js_sys::Reflect::get(&window()?.navigator(), &"gpu".into())?;
        if gpu.is_undefined() {
            return Err(error(
                "this browser has no WebGPU, which the cuelight player needs",
            ));
        }

        let base = if url.is_empty() || url.ends_with('/') || url.ends_with(".cuelight") {
            url
        } else {
            format!("{url}/")
        };
        let Fetched { files, clips } = fetch_show(&base).await?;
        let mut engine = Engine::new();
        let loaded = cuelight_loader::load_from_memory(&mut engine, &files)
            .map_err(|e| error(format!("{base}: {e}")))?;
        // Clips: opened by the browser, their lengths and sizes read
        // before the clock starts. A clip's soundtrack is registered as
        // a sound too, so the engine reports the play's voice with the
        // layer's gain, which is what the element's volume follows.
        let mut opened: HashMap<String, Clip> = HashMap::new();
        let mut clip_warnings = Vec::new();
        for (name, url) in clips {
            match open_clip(&url).await {
                Ok((element, duration, size)) => {
                    let registered = engine
                        .set_video(&name, duration, [f64::from(size[0]), f64::from(size[1])])
                        .and_then(|()| engine.set_sound(&name, duration));
                    if let Err(e) = registered {
                        clip_warnings.push(format!("clip {name:?}: {e}"));
                        continue;
                    }
                    opened.insert(
                        name,
                        Clip {
                            url,
                            spare: Some(element),
                        },
                    );
                }
                Err(e) => clip_warnings.push(format!("clip {name:?}: {}", js_error_text(&e))),
            }
        }
        let mut warnings: Vec<String> = engine
            .load_warnings()
            .iter()
            .map(|field| format!("show field {field:?} is not understood and was ignored"))
            .collect();
        warnings.extend(
            loaded
                .skipped
                .iter()
                .map(|file| format!("asset {file:?} was skipped: no decoder for this format")),
        );
        warnings.extend(clip_warnings);
        // Sounds: decoded by the browser, registered by duration.
        let mut audio = match WebAudio::new() {
            Ok(audio) => Some(audio),
            Err(e) => {
                warnings.push(format!("no sound: {}", js_error_text(&e)));
                None
            }
        };
        if let Some(audio) = &mut audio {
            for file in &loaded.sounds {
                match audio.decode(&file.name, &file.bytes).await {
                    Ok(duration) => {
                        if let Err(e) = engine.set_sound(&file.name, duration) {
                            warnings.push(format!("sound {:?}: {e}", file.name));
                        }
                    }
                    Err(e) => warnings.push(format!(
                        "sound {:?} could not be decoded: {}",
                        file.name,
                        js_error_text(&e)
                    )),
                }
            }
        }
        for warning in &warnings {
            web_sys::console::warn_1(&format!("cuelight: {warning}").into());
        }

        let mut context = RenderContext::new();
        let surface = context
            .create_surface(
                wgpu::SurfaceTarget::Canvas(canvas.clone()),
                canvas.width().max(1),
                canvas.height().max(1),
                wgpu::PresentMode::AutoVsync,
            )
            .await
            .map_err(|e| error(format!("no WebGPU surface for the canvas: {e}")))?;
        let device = &context
            .devices
            .get(surface.dev_id)
            .ok_or_else(|| error("no device for the WebGPU surface"))?
            .device;
        // A GPU error is reported between frames, not from a call that
        // could return it; kept here for the frame loop, and logged at
        // once in case the loop is already gone. Registered as soon as
        // the device exists: the renderer makes its pipelines right
        // away, and a pipeline that fails to build is reported here,
        // before every dispatch that then fails for lack of it.
        let gpu_errors: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = gpu_errors.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            let message = format!("GPU error: {e}");
            web_sys::console::error_1(&format!("cuelight: {message}").into());
            sink.lock().unwrap_or_else(|p| p.into_inner()).push(message);
        }));
        let renderer = vello::Renderer::new(device, vello::RendererOptions::default())
            .map_err(|e| error(format!("renderer: {e}")))?;

        let inner = Rc::new(RefCell::new(Inner {
            live: cuelight_loader::Live::default(),
            paused: false,
            engine,
            driver: loaded.driver.clone().map(DriverPlayer::new),
            driver_playing: loaded.driver.is_some(),
            script: loaded.driver,
            gpu_errors,
            canvas,
            context,
            surface,
            renderer,
            presenter: Presenter::new(),
            anchor_ms: None,
            on_event: None,
            on_driver: None,
            pending_frame: None,
            audio,
            clips: opened,
            screens: HashMap::new(),
        }));
        let reports = Rc::new(Reports {
            warnings: RefCell::new(warnings),
            on_error: RefCell::default(),
        });
        let frame = start_frames(&inner, &reports)?;
        let gestures = listen_for_gestures(&inner)?;
        Ok(CuelightPlayer {
            inner,
            reports,
            _frame: frame,
            _gestures: gestures,
        })
    }

    /// Whether sound is playing, or the browser still waits for a click or
    /// a key press on the page before it lets audio through.
    #[wasm_bindgen(getter, js_name = audioRunning)]
    pub fn audio_running(&self) -> bool {
        self.state()
            .is_some_and(|inner| inner.audio.as_ref().is_some_and(WebAudio::running))
    }

    /// Ask the browser to let sound through; only works from within a
    /// click or key handler, which the player already installs on the
    /// page, so pages rarely need this.
    #[wasm_bindgen(js_name = resumeAudio)]
    pub fn resume_audio(&self) {
        if let Some(audio) = self.state().as_ref().and_then(|inner| inner.audio.as_ref()) {
            audio.resume();
        }
    }

    /// Whether the page wants sound: on by default. Off silences the show
    /// (every voice stops and its audio context is suspended) and stays off
    /// through clicks and key presses; on lets sound through again, with
    /// plays picking up at their current positions. On a page the browser
    /// has not seen a gesture on yet, sound still waits for one:
    /// `audioRunning` is the truth. `false` when the browser gave no audio
    /// context.
    #[wasm_bindgen(getter, js_name = audioEnabled)]
    pub fn audio_enabled(&self) -> bool {
        self.state()
            .is_some_and(|inner| inner.audio.as_ref().is_some_and(WebAudio::enabled))
    }

    #[wasm_bindgen(setter, js_name = audioEnabled)]
    pub fn set_audio_enabled(&self, enabled: bool) {
        if let Some(audio) = self
            .state_mut()
            .as_mut()
            .and_then(|inner| inner.audio.as_mut())
        {
            audio.set_enabled(enabled);
        }
    }

    /// Fire a trigger.
    pub fn trigger(&self, name: &str) {
        if let Some(mut inner) = self.state_mut() {
            inner.engine.trigger(name);
        }
    }

    /// Press a key, by the name the browser gives it
    /// (`KeyboardEvent.key`). Fires what the show says the key means and
    /// returns that trigger, or `null` when the show says nothing about
    /// it, so a page can leave the key to the browser.
    pub fn key(&self, key: &str) -> Option<String> {
        let mut inner = self.state_mut()?;
        let fired = inner.engine.key(key)?;
        let at = inner.engine.time();
        inner.live.record(at, fired.clone());
        Some(fired)
    }

    /// Press the canvas at a point on the element, in CSS pixels from
    /// its top-left corner: `press(event.offsetX, event.offsetY)`.
    ///
    /// Fires the topmost pressable layer there, or the show's own
    /// `input.press` when there is none, and returns the trigger fired.
    /// `null` when the press landed in the letterbox beside the canvas
    /// or on nothing that answers.
    pub fn press(&self, x: f64, y: f64) -> Option<String> {
        let mut inner = self.state_mut()?;
        let size = inner.engine.show()?.size;
        // CSS pixels to the surface the frame was presented on.
        let ratio = inner.pixel_ratio();
        let surface = [inner.surface.config.width, inner.surface.config.height];
        let at = cuelight::render::canvas_at(
            size,
            surface,
            inner.engine.scaling(),
            inner.presenter.fit(),
            [x * ratio, y * ratio],
        )?;
        let pressed = inner.engine.press(at)?;
        let at = inner.engine.time();
        if let Some(trigger) = &pressed.trigger {
            inner.live.record(at, trigger.clone());
        }
        // Opened here, inside the pointer event that was the press:
        // browsers allow a new tab only from a user's gesture, and the
        // frame loop that reports events runs outside it. Without an
        // opener or a referrer: a show's links are someone else's pages,
        // and one with an opener could steer the kiosk's page away.
        if let Some(url) = &pressed.open {
            if let Some(window) = web_sys::window() {
                let _ = window.open_with_url_and_target_and_features(
                    url,
                    "_blank",
                    "noopener,noreferrer",
                );
            }
        }
        pressed.trigger
    }

    /// What a press at that point would do, without doing it, as
    /// `{ trigger?, open? }`; `undefined` over nothing pressable. For a
    /// page that wants a pointer cursor over what can be pressed.
    #[wasm_bindgen(js_name = pressedAt)]
    pub fn pressed_at(&self, x: f64, y: f64) -> JsValue {
        let Some(inner) = self.state() else {
            return JsValue::UNDEFINED;
        };
        let Some(size) = inner.engine.show().map(|show| show.size) else {
            return JsValue::UNDEFINED;
        };
        let ratio = inner.pixel_ratio();
        let surface = [inner.surface.config.width, inner.surface.config.height];
        let Some(at) = cuelight::render::canvas_at(
            size,
            surface,
            inner.engine.scaling(),
            inner.presenter.fit(),
            [x * ratio, y * ratio],
        ) else {
            return JsValue::UNDEFINED;
        };
        let Some(pressed) = inner.engine.pressed(at) else {
            return JsValue::UNDEFINED;
        };
        let object = js_sys::Object::new();
        if let Some(trigger) = &pressed.trigger {
            let _ = js_sys::Reflect::set(&object, &"trigger".into(), &trigger.as_str().into());
        }
        if let Some(url) = &pressed.open {
            let _ = js_sys::Reflect::set(&object, &"open".into(), &url.as_str().into());
        }
        object.into()
    }

    /// How the show is brought to the canvas: `contain`, `cover` or
    /// `fill`, as CSS's `object-fit` names them.
    #[wasm_bindgen(getter)]
    pub fn fit(&self) -> String {
        self.state()
            .map(|inner| inner.presenter.fit().name().to_owned())
            .unwrap_or_default()
    }

    /// Bring the show to the canvas as `fit` says from the next frame
    /// on; a word that is none of the three is refused.
    #[wasm_bindgen(setter)]
    pub fn set_fit(&self, fit: &str) -> Result<(), JsValue> {
        let fit = Fit::parse(fit)
            .ok_or_else(|| error(format!("{fit:?} is not a fit: contain, cover or fill")))?;
        if let Some(mut inner) = self.state_mut() {
            inner.presenter.set_fit(fit);
        }
        Ok(())
    }

    /// Set a variable to a boolean, a number or a string.
    pub fn set(&self, name: &str, value: JsValue) -> Result<(), JsValue> {
        let value = from_js(&value)?;
        if let Some(mut inner) = self.state_mut() {
            inner.engine.set_variable(name, value);
        }
        Ok(())
    }

    /// A variable's current value, `undefined` when the show has none by
    /// that name.
    pub fn get(&self, name: &str) -> JsValue {
        self.state().map_or(JsValue::UNDEFINED, |inner| {
            inner
                .engine
                .variable(name)
                .map_or(JsValue::UNDEFINED, to_js)
        })
    }

    /// The triggers the show listens to, sorted.
    pub fn actions(&self) -> Vec<String> {
        let Some(inner) = self.state() else {
            return Vec::new();
        };
        inner
            .engine
            .show()
            .map(|show| show.triggers().into_iter().collect())
            .unwrap_or_default()
    }

    /// Where each trigger is listened to, as an object by name: `{ where:
    /// "opens", scene }` for one that enters a scene, `{ where: "scene",
    /// scene }` for one only that scene hears, and `{ where: "anywhere" }`
    /// for the rest, so a page can group its buttons and dim the ones the
    /// active scene is not listening to. Keys and presses that fire a
    /// trigger nobody hears are listed too, as heard anywhere.
    pub fn listeners(&self) -> JsValue {
        use cuelight_core::Listened;
        let object = js_sys::Object::new();
        let Some(inner) = self.state() else {
            return object.into();
        };
        let listeners = inner.engine.show().map(|show| show.listeners());
        for (name, listened) in listeners.unwrap_or_default() {
            let place = js_sys::Object::new();
            let (at, scene) = match &listened {
                Listened::Opens(scene) => ("opens", Some(scene)),
                Listened::Scene(scene) => ("scene", Some(scene)),
                Listened::Anywhere => ("anywhere", None),
            };
            let _ = js_sys::Reflect::set(&place, &"where".into(), &at.into());
            if let Some(scene) = scene {
                let _ = js_sys::Reflect::set(&place, &"scene".into(), &scene.as_str().into());
            }
            let _ = js_sys::Reflect::set(&object, &name.into(), &place);
        }
        object.into()
    }

    /// The show's variables with their current values, as an object.
    pub fn variables(&self) -> JsValue {
        let object = js_sys::Object::new();
        let Some(inner) = self.state() else {
            return object.into();
        };
        for name in inner
            .engine
            .show()
            .into_iter()
            .flat_map(|s| s.variables.keys())
        {
            let value = inner
                .engine
                .variable(name)
                .map_or(JsValue::UNDEFINED, to_js);
            let _ = js_sys::Reflect::set(&object, &name.into(), &value);
        }
        object.into()
    }

    /// The show's scene names, in document order.
    pub fn scenes(&self) -> Vec<String> {
        let Some(inner) = self.state() else {
            return Vec::new();
        };
        inner.engine.show().map_or_else(Vec::new, |show| {
            show.scenes.iter().map(|s| s.name.clone()).collect()
        })
    }

    #[wasm_bindgen(getter, js_name = activeScene)]
    pub fn active_scene(&self) -> Option<String> {
        self.state()?.engine.active_scene().map(str::to_owned)
    }

    /// The show's canvas size, e.g. for the page to set an aspect ratio.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.state()
            .and_then(|inner| inner.engine.show().map(|show| show.size[0]))
            .unwrap_or(0)
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.state()
            .and_then(|inner| inner.engine.show().map(|show| show.size[1]))
            .unwrap_or(0)
    }

    /// What loading complained about, and what stopped the frames since
    /// (a GPU error, a frame that failed); also logged to the console.
    pub fn warnings(&self) -> Vec<String> {
        self.reports.warnings.borrow().clone()
    }

    /// Call `callback` with a message when the GPU reports an error or a
    /// frame fails; the frames stop then, since the next would fail the
    /// same way, while the page keeps the player to read `warnings()`
    /// from. A frame that panics stops the player for good: every call
    /// after it answers as if there were no show. `null` stops it. The
    /// message is logged to the console either way.
    #[wasm_bindgen(js_name = onError)]
    pub fn on_error(&self, callback: Option<js_sys::Function>) {
        *self.reports.on_error.borrow_mut() = callback;
    }

    /// Whether the show folder came with a driver script.
    #[wasm_bindgen(getter, js_name = hasDriver)]
    pub fn has_driver(&self) -> bool {
        self.state().is_some_and(|inner| inner.script.is_some())
    }

    /// Whether the driver script is playing: false once paused or finished.
    #[wasm_bindgen(getter, js_name = driverPlaying)]
    pub fn driver_playing(&self) -> bool {
        self.state().is_some_and(|inner| {
            inner.driver_playing && inner.driver.as_ref().is_some_and(|d| !d.is_done())
        })
    }

    /// Continue the driver script, from the top when it had finished.
    #[wasm_bindgen(js_name = driverPlay)]
    pub fn driver_play(&self) {
        let Some(mut inner) = self.state_mut() else {
            return;
        };
        if inner.driver.as_ref().is_some_and(DriverPlayer::is_done) {
            inner.driver = inner.script.clone().map(DriverPlayer::new);
        }
        inner.driver_playing = inner.driver.is_some();
    }

    #[wasm_bindgen(js_name = driverPause)]
    pub fn driver_pause(&self) {
        if let Some(mut inner) = self.state_mut() {
            inner.driver_playing = false;
        }
    }

    /// Stop the clock. Frames keep painting, so a resize or a seek still
    /// shows, but nothing advances and the driver stops with it.
    pub fn pause(&self) {
        if let Some(mut inner) = self.state_mut() {
            inner.paused = true;
        }
    }

    /// Start the clock again from where it stopped.
    pub fn resume(&self) {
        if let Some(mut inner) = self.state_mut() {
            inner.paused = false;
        }
    }

    #[wasm_bindgen(getter)]
    pub fn paused(&self) -> bool {
        self.state().is_none_or(|inner| inner.paused)
    }

    /// Seconds of show time that have passed.
    #[wasm_bindgen(getter)]
    pub fn time(&self) -> f64 {
        self.state().map_or(0.0, |inner| inner.engine.time())
    }

    /// Put the show at `seconds`, forwards or backwards.
    ///
    /// The show is restarted and advanced to that moment, replaying the
    /// driver script and everything the page did on the way, because a
    /// show's state is a function of its inputs and the clock rather than
    /// anything the engine remembers. It lands exactly where playing
    /// there would have, and costs well under a millisecond, so a page
    /// may call this as a scrub bar moves.
    pub fn seek(&self, seconds: f64) {
        let Some(mut inner) = self.state_mut() else {
            return;
        };
        let script = inner.script.clone();
        let live = inner.live.clone();
        let Inner { engine, .. } = &mut *inner;
        let played = cuelight_loader::seek(engine.core_mut(), script, &live, seconds.max(0.0));
        inner.driver = played;
        // The clock is read from the anchor, and the show is somewhere
        // else now: the next frame works out where it starts from.
        inner.anchor_ms = None;
    }

    /// Call `callback` with every event the show raises, as
    /// `{ type: "trigger", name }`; `null` stops it.
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, callback: Option<js_sys::Function>) {
        if let Some(mut inner) = self.state_mut() {
            inner.on_event = callback;
        }
    }

    /// Call `callback` with every step of the driver script as it fires,
    /// as `{ type: "trigger", name, at }` or `{ type: "set", name, value,
    /// at }`, `at` being the driver's instant on the show's clock; `null`
    /// stops it. A wait is not a step that does anything, and is not
    /// reported.
    #[wasm_bindgen(js_name = onDriver)]
    pub fn on_driver(&self, callback: Option<js_sys::Function>) {
        if let Some(mut inner) = self.state_mut() {
            inner.on_driver = callback;
        }
    }
}

impl CuelightPlayer {
    /// The player's state, or `None` once a panic has left it borrowed:
    /// a dead player answers every call as if it had no show, rather
    /// than panicking again for each.
    fn state(&self) -> Option<std::cell::Ref<'_, Inner>> {
        self.inner.try_borrow().ok()
    }

    fn state_mut(&self) -> Option<std::cell::RefMut<'_, Inner>> {
        self.inner.try_borrow_mut().ok()
    }
}

impl Drop for CuelightPlayer {
    fn drop(&mut self) {
        // Every clip's decoder goes now, not when the elements are
        // collected: a page that swaps shows makes a player per show.
        if let Ok(inner) = self.inner.try_borrow() {
            for screen in inner.screens.values() {
                release(&screen.element);
            }
            for clip in inner.clips.values() {
                if let Some(spare) = &clip.spare {
                    release(spare);
                }
            }
        }
        let pending = self
            .inner
            .try_borrow_mut()
            .ok()
            .and_then(|mut inner| inner.pending_frame.take());
        if let (Some(id), Some(window)) = (pending, web_sys::window()) {
            let _ = window.cancel_animation_frame(id);
        }
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            for (event, closure) in &self._gestures {
                let _ = document
                    .remove_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
            }
        }
    }
}

fn js_error_text(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            e.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("{e:?}"))
}

/// Resume the audio context on the first click or key press anywhere on
/// the page: browsers only let sound through after a gesture, and only
/// when asked from within its handler.
fn listen_for_gestures(inner: &Rc<RefCell<Inner>>) -> Result<Vec<GestureListener>, JsValue> {
    let document = window()?.document().ok_or_else(|| error("no document"))?;
    let mut listeners = Vec::new();
    for event in ["pointerdown", "keydown"] {
        let weak = Rc::downgrade(inner);
        let closure = Closure::new(move || {
            if let Some(inner) = weak.upgrade() {
                let Ok(inner) = inner.try_borrow() else {
                    return;
                };
                if let Some(audio) = &inner.audio {
                    audio.resume();
                }
                // A gesture is what a refused clip was waiting for.
                for screen in inner.screens.values() {
                    screen.refused.set(false);
                }
            }
        });
        document.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())?;
        listeners.push((event.to_owned(), closure));
    }
    Ok(listeners)
}

/// Start the `requestAnimationFrame` loop. The returned cell owns the
/// callback; the loop ends when it is dropped.
fn start_frames(
    inner: &Rc<RefCell<Inner>>,
    reports: &Rc<Reports>,
) -> Result<Rc<RefCell<Option<FrameCallback>>>, JsValue> {
    fn request(window: &web_sys::Window, inner: &RefCell<Inner>, callback: &FrameCallback) {
        let id = window.request_animation_frame(callback.as_ref().unchecked_ref());
        if let Ok(mut inner) = inner.try_borrow_mut() {
            inner.pending_frame = id.ok();
        }
    }

    let cell: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let (weak_cell, weak_inner) = (Rc::downgrade(&cell), Rc::downgrade(inner));
    let reports = reports.clone();
    let callback = Closure::new(move |now_ms: f64| {
        let (Some(cell), Some(inner)): (Option<Rc<_>>, Option<Rc<RefCell<Inner>>>) =
            (Weak::upgrade(&weak_cell), Weak::upgrade(&weak_inner))
        else {
            return;
        };
        // The borrow ends before any callback runs: a callback may well
        // call back into the player.
        let (result, failed, on_event, on_driver) = {
            let Ok(mut inner) = inner.try_borrow_mut() else {
                return;
            };
            inner.pending_frame = None;
            // What the GPU reported since the last frame comes first: a
            // frame that failed after it failed because of it.
            let mut failed: Vec<String> =
                std::mem::take(&mut *inner.gpu_errors.lock().unwrap_or_else(|p| p.into_inner()));
            // A panic in the frame is reported through these.
            IN_FRAME.with(|cell| *cell.borrow_mut() = Some(reports.clone()));
            let result = inner.frame(now_ms);
            IN_FRAME.with(|cell| cell.borrow_mut().take());
            if let Err(e) = &result {
                web_sys::console::error_1(&format!("cuelight: {e}").into());
                failed.push(e.clone());
            }
            (
                result,
                failed,
                inner.on_event.clone(),
                inner.on_driver.clone(),
            )
        };
        // The driver's steps first: what the show did this frame follows
        // from them.
        if let (Ok(happened), Some(on_driver)) = (&result, on_driver) {
            for applied in &happened.applied {
                for step in applied_to_js(applied) {
                    let _ = on_driver.call1(&JsValue::NULL, &step);
                }
            }
        }
        if let (Ok(happened), Some(on_event)) = (&result, on_event) {
            for event in &happened.events {
                let _ = on_event.call1(&JsValue::NULL, &event_to_js(event));
            }
        }
        // The loop stops: a frame that failed will fail again, and a
        // canvas drawing black on is what hid the error in the first
        // place.
        if !failed.is_empty() {
            for message in &failed {
                reports.fail(message);
            }
            return;
        }
        let callback = cell.borrow();
        if let (Some(window), Some(callback)) = (web_sys::window(), callback.as_ref()) {
            request(&window, &inner, callback);
        }
    });
    request(&window()?, inner, &callback);
    *cell.borrow_mut() = Some(callback);
    Ok(cell)
}
