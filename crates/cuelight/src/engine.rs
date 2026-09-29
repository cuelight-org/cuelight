//! The engine with its assets: what the core decides, drawn.
//!
//! [`Engine`] wraps a [`cuelight_core::Engine`] and adds what a frame
//! needs that the clock does not: images, fonts and vector artwork, the
//! draw list built from them, and the press test against that list.
//! Everything about *when* and *what* stays in the core; this module only
//! answers *where* and *how it looks*.

use crate::font::{BitmapFont, Rgba, StyledFont};
use crate::lru::ByteLru;
use crate::output::OutputColor;
use crate::segments;
use cuelight_core::{
    frame_key, parse_color, revealed, row_cells, Align, Blend, DigitDisplay, Error, Event, Fill,
    Finding, Gradient, Influence, Justify, Layer, LayerKind, LayerPath, Pass, PathElement, Playing,
    Press, Property, Reel, ReelCells, ResolvedValue, Root, Scaling, Shape, Sheet, Show, Traced,
    Value, Voice,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// An asset a host handed over that is not one: the wrong number of
/// bytes for its size, a font file that does not parse, artwork without
/// a size. What the show itself gets wrong is a [`cuelight_core::Error`],
/// and comes back from the same calls it always did.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AssetError {
    #[error("invalid image: {0}")]
    InvalidImage(String),
    #[error("invalid font: {0}")]
    InvalidFont(String),
    #[error("invalid vector: {0}")]
    InvalidVector(String),
}

/// A host-provided raster image: tightly packed RGBA8 pixels (straight,
/// non-premultiplied alpha), kept in memory and shared with renderers.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major RGBA8.
    pub pixels: Arc<[u8]>,
    revision: u64,
}

/// The next revision for anything a renderer caches by it. One sequence
/// for the whole process, so a cache shared between engines (a host
/// playing several shows, tests sharing a renderer) cannot mistake one
/// engine's asset for another's.
fn next_revision() -> u64 {
    static REVISION: AtomicU64 = AtomicU64::new(0);
    REVISION.fetch_add(1, Ordering::Relaxed) + 1
}

impl ImageData {
    /// Unique to this upload for as long as the process runs. Lets
    /// renderers cache GPU resources per upload instead of comparing
    /// pixels.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Wrap engine-generated pixels with a fresh revision.
    fn generated(rgba: Rgba) -> Self {
        Self {
            width: rgba.width,
            height: rgba.height,
            pixels: rgba.pixels.into(),
            revision: next_revision(),
        }
    }
}

/// A map of the plane, `(x, y)` to `(a x + c y + e, b x + d y + f)`, its
/// coefficients in the order `[a, b, c, d, e, f]` that SVG and kurbo use.
/// The draw list places items with one where a rotation or an uneven
/// scale is involved; see [`ResolvedLayer::transform`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform(pub [f64; 6]);

impl Transform {
    pub const IDENTITY: Transform = Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f64, y: f64) -> Self {
        Transform([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Transform([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    /// Rotation by `degrees`, clockwise on a canvas whose y grows down.
    pub fn rotate(degrees: f64) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Transform([c, s, -s, c, 0.0, 0.0])
    }

    /// This map applied after `inner`.
    pub fn then(self, inner: Transform) -> Transform {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = inner.0;
        Transform([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }

    pub fn apply(self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d, e, f] = self.0;
        [a * x + c * y + e, b * x + d * y + f]
    }

    /// The map back, for asking where a canvas point is in a layer's own
    /// space. `None` for a map that flattens everything to a line, which
    /// has no back.
    pub fn invert(self) -> Option<Transform> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        Some(Transform([
            d / det,
            -b / det,
            -c / det,
            a / det,
            (c * f - d * e) / det,
            (b * e - a * f) / det,
        ]))
    }

    /// `(scale, x, y)` when this is only a positive uniform scale and a
    /// translation, which the draw list bakes into its coordinates.
    pub fn plain(self) -> Option<(f64, f64, f64)> {
        let [a, b, c, d, e, f] = self.0;
        (b == 0.0 && c == 0.0 && a == d && a > 0.0).then_some((a, e, f))
    }
}

/// A host-provided outline font file (TrueType / OpenType), shared with
/// renderers.
#[derive(Debug, Clone, PartialEq)]
pub struct FontData {
    /// The font file's bytes.
    pub data: Arc<[u8]>,
    revision: u64,
}

impl FontData {
    /// A font for a unit test, with no registration behind it.
    #[cfg(all(test, feature = "outline-fonts"))]
    pub(crate) fn for_test(bytes: &'static [u8]) -> Self {
        Self {
            data: Arc::from(bytes),
            revision: 0,
        }
    }

    /// Unique to this registration for as long as the process runs, so
    /// renderers can cache their font object per upload instead of
    /// comparing bytes.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Host-provided vector artwork (an SVG the loader converted): paths in
/// its own units, drawn by vector layers.
#[derive(Debug, Clone, PartialEq)]
pub struct Vector {
    /// The artwork's natural size, what a vector layer without `size`
    /// draws at.
    pub width: f64,
    pub height: f64,
    /// In paint order.
    pub paths: Vec<VectorPath>,
}

/// One path of a [`Vector`]: its geometry and how it is painted.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorPath {
    pub elements: Vec<PathElement>,
    /// Fill color as RGBA bytes; `None` for an outline only.
    pub fill: Option<[u8; 4]>,
    /// Stroke color and width, in the artwork's units.
    pub stroke: Option<([u8; 4], f64)>,
    /// The `id`s of the elements this path is inside, outermost first,
    /// its own last: what a show's `parts` name to move it.
    pub ids: Vec<String>,
}

/// One glyph of a [`ResolvedShape::GlyphRun`]: its id in the font and the
/// position of its origin on the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub id: u32,
    pub x: f64,
    pub y: f64,
}

/// A host-registered bitmap font: its description and page pixels.
#[derive(Debug)]
struct RegisteredFont {
    font: BitmapFont,
    pages: Vec<Rgba>,
}

/// What a press landed on, and so did: the trigger it fired, the web
/// address it asked to have opened, or both; see [`Engine::press`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pressed {
    pub trigger: Option<String>,
    pub open: Option<String>,
}

/// The bounds of every element id in `vector`, over the paths inside
/// it: `[x, y, width, height]` in the artwork's own units.
fn element_bounds(vector: &Vector) -> HashMap<String, [f64; 4]> {
    let mut out: HashMap<String, [f64; 4]> = HashMap::new();
    for path in &vector.paths {
        let Some([x, y, w, h]) = PathElement::bounds(&path.elements) else {
            continue;
        };
        for id in &path.ids {
            let joined = match out.get(id) {
                None => [x, y, w, h],
                Some([bx, by, bw, bh]) => {
                    let (left, top) = (bx.min(x), by.min(y));
                    let (right, bottom) = ((bx + bw).max(x + w), (by + bh).max(y + h));
                    [left, top, right - left, bottom - top]
                }
            };
            out.insert(id.clone(), joined);
        }
    }
    out
}

/// What a show's part does to the element it names, this frame.
#[derive(Debug, Clone, Copy)]
struct Moved {
    /// In the artwork's own coordinates.
    transform: Transform,
    opacity: f64,
    /// Not drawn at all.
    hidden: bool,
}

/// How a text layer gets drawn, see `Engine::text_draw`.
enum TextDraw {
    Bitmap(Arc<TextRaster>),
    #[cfg_attr(not(feature = "outline-fonts"), allow(dead_code))]
    Glyphs {
        font: FontData,
        size: f64,
        /// Relative to the layer's box.
        glyphs: Vec<PlacedGlyph>,
        container: [f64; 2],
    },
}

/// Rasterized text, placed relative to its layer's box.
#[derive(Debug)]
struct TextRaster {
    image: ImageData,
    offset: [i32; 2],
    /// The box the text was laid out in.
    container: [f64; 2],
}

/// Caches for text rendering, filled lazily while resolving layers.
#[derive(Debug)]
struct TextCache {
    /// Rasterizations asked for since the cache was made: served from
    /// the cache, and made afresh, with the bytes the fresh ones came to.
    hits: u64,
    misses: u64,
    rasterized_bytes: u64,
    /// Styled fonts by style name.
    styled: HashMap<String, Arc<StyledFont>>,
    /// Outline fonts rasterized into pixels, by font name, size and the
    /// padding round each glyph.
    #[cfg_attr(not(feature = "outline-fonts"), allow(dead_code))]
    pixels: HashMap<(String, u64, u32), Arc<RegisteredFont>>,
    /// Rasters by style, text, box and alignment; `None` for text that
    /// draws nothing.
    rasters: ByteLru<String, Option<Arc<TextRaster>>>,
}

impl Default for TextCache {
    fn default() -> Self {
        Self {
            hits: 0,
            misses: 0,
            rasterized_bytes: 0,
            styled: HashMap::new(),
            pixels: HashMap::new(),
            rasters: ByteLru::new(MAX_RASTER_BYTES),
        }
    }
}

/// Budget for cached text rasters. Every distinct string is its own
/// bitmap, so a changing number in a large font adds up quickly; the least
/// recently used rasters go first, which keeps static labels cached.
const MAX_RASTER_BYTES: usize = 32 * 1024 * 1024;

/// Passes a segment's halo is drawn in, each wider and fainter than the
/// last.
///
/// Few enough steps and a halo reads as stacked rings rather than a spill
/// of light: the banding is the steps showing, and every pass is a flat
/// step of the falloff. Sixteen is where the rings stop being visible at
/// the widest halo the format allows, magnified eight times; a glowing
/// row of eight digits is then 962 draw items and resolves in 0.08 ms,
/// against 514 and 0.04 ms at half that.
const GLOW_STEPS: u32 = 16;

/// The engine: a [`cuelight_core::Engine`] with the assets a frame needs.
///
/// Hosts drive it through the four core calls (`load_show`, `set_variable`,
/// `trigger`, `advance_frame`), hand it images, fonts and vector artwork,
/// and read the result back either as
/// [`resolved_layers`](Engine::resolved_layers) (a flattened draw list, no
/// GPU involved) or through the `render` feature's rasterizer.
///
/// The core calls are the core's, passed through: the clock, variables,
/// triggers, sounds, videos, events and `values` behave exactly as
/// [`cuelight_core::Engine`] documents them, and [`core`](Engine::core)
/// hands the core out for anything that only needs the model, such as a
/// driver script or a seek. Loading a show goes through this engine,
/// which checks it against the fonts it holds.
#[derive(Debug, Default)]
pub struct Engine {
    core: cuelight_core::Engine,
    images: BTreeMap<String, ImageData>,
    vectors: BTreeMap<String, Vector>,
    /// Per vector, the bounds of every element id its paths carry, worked
    /// out once when it is registered: what a part with no pivot of its
    /// own turns around, asked for every frame.
    part_bounds: BTreeMap<String, HashMap<String, [f64; 4]>>,
    fonts: BTreeMap<String, Arc<RegisteredFont>>,
    outline_fonts: BTreeMap<String, FontData>,
    text_cache: Mutex<TextCache>,
    /// The core's load warnings and this side's own, together.
    warnings: Vec<String>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a show from its JSON description, replacing any current show
    /// and resetting all runtime state. The first scene (if any) becomes
    /// active; autoplay timelines start at 0.
    ///
    /// Beyond what the core checks, a font style is held to the font it
    /// names: an outline font needs a size, a bitmap font has one of its
    /// own. Registered assets are left alone.
    pub fn load_show(&mut self, json: &str) -> Result<(), Error> {
        let (fonts, outline_fonts) = (&self.fonts, &self.outline_fonts);
        self.core.load_show_checked(json, |show| {
            match font_style_problems(show, fonts, outline_fonts).first() {
                Some((name, problem)) => {
                    Err(Error::InvalidShow(format!("font style {name:?} {problem}")))
                }
                None => Ok(()),
            }
        })?;
        self.loaded();
        Ok(())
    }

    /// Load as much of a show as can be loaded, and say what could not;
    /// see [`cuelight_core::Engine::load_show_tolerant`]. A font style
    /// that does not fit the font it names is a finding too, and its
    /// text is not drawn until it does.
    pub fn load_show_tolerant(&mut self, json: &str) -> Result<Vec<Finding>, Error> {
        let (fonts, outline_fonts) = (&self.fonts, &self.outline_fonts);
        let findings = self
            .core
            .load_show_tolerant_checked(json, |show, findings| {
                for (name, problem) in font_style_problems(show, fonts, outline_fonts) {
                    findings.push(Finding {
                        path: format!("fonts.{name}"),
                        message: format!("font style {name:?} {problem}"),
                        kind: cuelight_core::FindingKind::Error,
                    });
                }
            })?;
        self.loaded();
        Ok(findings)
    }

    /// What follows a show being loaded into the core.
    fn loaded(&mut self) {
        *self.text_cache.get_mut().unwrap_or_else(|e| e.into_inner()) = TextCache::default();
        self.warnings = self.core.load_warnings().to_vec();
        if let Some(show) = self.core.show() {
            quiet_artwork(show, &self.vectors, &mut self.warnings);
        }
    }

    /// The core this engine wraps: the show, its state and its clock,
    /// for whatever only needs those. A driver script or a seek takes
    /// this rather than the whole engine.
    ///
    /// Loading a show goes through [`load_show`](Engine::load_show) on
    /// this engine, not the core's: that is where the show is checked
    /// against the fonts and where the text cache is emptied.
    pub fn core(&self) -> &cuelight_core::Engine {
        &self.core
    }

    /// The core, to drive; see [`core`](Engine::core).
    pub fn core_mut(&mut self) -> &mut cuelight_core::Engine {
        &mut self.core
    }

    /// Put the loaded show back to its beginning; see
    /// [`cuelight_core::Engine::restart`].
    pub fn restart(&mut self) {
        self.core.restart();
    }

    /// Fields of the loaded show document that were ignored, and what
    /// will quietly do nothing: the core's warnings
    /// ([`cuelight_core::Engine::load_warnings`]) plus artwork asked for
    /// something its kind does not have.
    pub fn load_warnings(&self) -> &[String] {
        &self.warnings
    }

    /// See [`cuelight_core::Engine::set_variable`].
    pub fn set_variable(&mut self, name: &str, value: impl Into<Value>) {
        self.core.set_variable(name, value);
    }

    /// See [`cuelight_core::Engine::variable`].
    pub fn variable(&self, name: &str) -> Option<&Value> {
        self.core.variable(name)
    }

    /// See [`cuelight_core::Engine::value`].
    pub fn value(&self, name: &str) -> Option<Value> {
        self.core.value(name)
    }

    /// See [`cuelight_core::Engine::set_sound`].
    pub fn set_sound(&mut self, name: &str, duration: f64) -> Result<(), Error> {
        self.core.set_sound(name, duration)
    }

    /// See [`cuelight_core::Engine::sound_duration`].
    pub fn sound_duration(&self, name: &str) -> Option<f64> {
        self.core.sound_duration(name)
    }

    /// See [`cuelight_core::Engine::set_video`]. The frames come back
    /// through [`set_image`](Engine::set_image) under each play's
    /// [`frame`](Playing::frame) name.
    pub fn set_video(&mut self, name: &str, duration: f64, size: [f64; 2]) -> Result<(), Error> {
        self.core.set_video(name, duration, size)
    }

    /// See [`cuelight_core::Engine::video`].
    pub fn video(&self, name: &str) -> Option<cuelight_core::VideoInfo> {
        self.core.video(name)
    }

    /// See [`cuelight_core::Engine::set_seed`].
    pub fn set_seed(&mut self, seed: u64) {
        self.core.set_seed(seed);
    }

    /// See [`cuelight_core::Engine::videos`].
    pub fn videos(&self) -> Result<Vec<Playing>, Error> {
        self.core.videos()
    }

    /// See [`cuelight_core::Engine::voices`].
    pub fn voices(&self) -> Result<Vec<Voice>, Error> {
        self.core.voices()
    }

    /// See [`cuelight_core::Engine::values`].
    pub fn values(&self) -> Result<Vec<ResolvedValue>, Error> {
        self.core.values()
    }

    /// See [`cuelight_core::Engine::key`].
    pub fn key(&mut self, key: &str) -> Option<String> {
        self.core.key(key)
    }

    /// See [`cuelight_core::Engine::trigger`].
    pub fn trigger(&mut self, name: &str) {
        self.core.trigger(name);
    }

    /// See [`cuelight_core::Engine::advance_frame`].
    pub fn advance_frame(&mut self, dt: f64) {
        self.core.advance_frame(dt);
    }

    /// See [`cuelight_core::Engine::advance_to`].
    pub fn advance_to(&mut self, to: f64) {
        self.core.advance_to(to);
    }

    /// See [`cuelight_core::Engine::drain_events`].
    pub fn drain_events(&mut self) -> Vec<Event> {
        self.core.drain_events()
    }

    /// See [`cuelight_core::Engine::drain_trace`].
    pub fn drain_trace(&mut self) -> Vec<Traced> {
        self.core.drain_trace()
    }

    /// See [`cuelight_core::Engine::explain`].
    pub fn explain(&self, layer: &LayerPath, property: Property) -> Vec<Influence> {
        self.core.explain(layer, property)
    }

    /// See [`cuelight_core::Engine::start_timeline`].
    pub fn start_timeline(&mut self, layer: &LayerPath, timeline: usize) -> bool {
        self.core.start_timeline(layer, timeline)
    }

    /// See [`cuelight_core::Engine::time`].
    pub fn time(&self) -> f64 {
        self.core.time()
    }

    /// See [`cuelight_core::Engine::show`].
    pub fn show(&self) -> Option<&Show> {
        self.core.show()
    }

    /// See [`cuelight_core::Engine::active_scene`].
    pub fn active_scene(&self) -> Option<&str> {
        self.core.active_scene()
    }

    /// See [`cuelight_core::Engine::passes`].
    pub fn passes(&self) -> Vec<Pass> {
        self.core.passes()
    }

    /// See [`cuelight_core::Engine::scaling`].
    pub fn scaling(&self) -> Scaling {
        self.core.scaling()
    }

    /// See [`cuelight_core::Engine::pixel_grid`].
    pub fn pixel_grid(&self) -> bool {
        self.core.pixel_grid()
    }

    /// Register (or replace) a named RGBA8 image that image layers can
    /// reference. Images are host assets, not show content: they survive
    /// `load_show` and may be provided before or after the show that
    /// uses them (layers referencing a missing image are skipped).
    pub fn set_image(
        &mut self,
        name: &str,
        width: u32,
        height: u32,
        pixels: impl Into<Arc<[u8]>>,
    ) -> Result<(), AssetError> {
        let pixels = pixels.into();
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(AssetError::InvalidImage(format!(
                "{name:?}: {} bytes for {width}x{height}, expected {expected}",
                pixels.len()
            )));
        }
        self.images.insert(
            name.to_owned(),
            ImageData {
                width,
                height,
                pixels,
                revision: next_revision(),
            },
        );
        Ok(())
    }

    /// Register (or replace) a named bitmap font that font styles reference
    /// by `file`: a parsed description plus its page images in page order
    /// (see [`BitmapFont::pages`]), each `(width, height, RGBA8 pixels)`.
    /// Like images, fonts are host assets that survive `load_show`.
    pub fn set_font(
        &mut self,
        name: &str,
        font: BitmapFont,
        pages: Vec<(u32, u32, Vec<u8>)>,
    ) -> Result<(), AssetError> {
        if pages.len() != font.pages().len() {
            return Err(AssetError::InvalidFont(format!(
                "{name:?}: {} page images for {} pages",
                pages.len(),
                font.pages().len()
            )));
        }
        let pages = pages
            .into_iter()
            .map(|(width, height, pixels)| {
                if pixels.len() != width as usize * height as usize * 4 {
                    return Err(AssetError::InvalidFont(format!(
                        "{name:?}: page of {} bytes for {width}x{height}",
                        pixels.len()
                    )));
                }
                Ok(Rgba {
                    width,
                    height,
                    pixels,
                })
            })
            .collect::<Result<_, _>>()?;
        self.outline_fonts.remove(name);
        self.fonts
            .insert(name.to_owned(), Arc::new(RegisteredFont { font, pages }));
        *self.text_cache.get_mut().unwrap_or_else(|e| e.into_inner()) = TextCache::default();
        Ok(())
    }

    /// Register (or replace) a named outline font (TrueType / OpenType
    /// file bytes) that font styles reference by `file`, with the em size
    /// they set. Like images, fonts are host assets that survive
    /// `load_show`. Registering a name replaces a bitmap font of that name.
    #[cfg(feature = "outline-fonts")]
    pub fn set_outline_font(
        &mut self,
        name: &str,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<(), AssetError> {
        let data = bytes.into();
        crate::outline::validate(&data)
            .map_err(|e| AssetError::InvalidFont(format!("{name:?}: {e}")))?;
        self.fonts.remove(name);
        self.outline_fonts.insert(
            name.to_owned(),
            FontData {
                data,
                revision: next_revision(),
            },
        );
        *self.text_cache.get_mut().unwrap_or_else(|e| e.into_inner()) = TextCache::default();
        Ok(())
    }

    /// Whether a font, bitmap or outline, is registered under `name`.
    pub fn has_font(&self, name: &str) -> bool {
        self.fonts.contains_key(name) || self.outline_fonts.contains_key(name)
    }

    /// Every outline font registered, by name, in name order.
    ///
    /// The show's own fonts, for a host that has to hand them to
    /// something else: a loader turning an SVG's text into paths draws
    /// it with these rather than with whatever the machine has
    /// installed, so a show reads the same wherever it plays.
    pub fn outline_fonts(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.outline_fonts
            .iter()
            .map(|(name, font)| (name.as_str(), &*font.data))
    }

    /// The pixels registered under `name`, if any.
    pub fn image(&self, name: &str) -> Option<&ImageData> {
        self.images.get(name)
    }

    /// Register (or replace) named vector artwork that vector layers
    /// reference: paths with fills and strokes in the artwork's own units,
    /// what the loader makes of an SVG file. Like images, vectors are host
    /// assets that survive `load_show`; a layer whose vector is not (yet)
    /// registered is skipped.
    pub fn set_vector(&mut self, name: &str, vector: Vector) -> Result<(), AssetError> {
        if !(vector.width > 0.0 && vector.height > 0.0) {
            return Err(AssetError::InvalidVector(format!(
                "{name:?}: size {}x{} is not above 0",
                vector.width, vector.height
            )));
        }
        self.part_bounds
            .insert(name.to_owned(), element_bounds(&vector));
        self.vectors.insert(name.to_owned(), vector);
        Ok(())
    }

    /// The vector artwork registered under `name`, if any.
    pub fn vector(&self, name: &str) -> Option<&Vector> {
        self.vectors.get(name)
    }

    /// Press the canvas at `at`, in canvas coordinates.
    ///
    /// The topmost pressable layer drawn under that point fires its
    /// trigger; where nothing pressable is under it, the show's own
    /// `input.press` does, which is how "press anywhere to go on"
    /// is written. Hands back the trigger fired, or `None`.
    ///
    /// The point is tested against the frame as drawn: a layer hidden,
    /// clipped away or covered is not hit, and what counts as inside is
    /// the shape for a rect or a circle and the bounding box for
    /// anything else. A host turns a click into canvas coordinates
    /// first; `render::fit` says where the canvas landed on its surface.
    pub fn press(&mut self, at: [f64; 2]) -> Option<Pressed> {
        let pressed = self.pressed(at).or_else(|| {
            let trigger = self.core.show()?.input.press.clone()?;
            Some(Pressed {
                trigger: Some(trigger),
                open: None,
            })
        })?;
        if let Some(trigger) = &pressed.trigger {
            self.core.trigger(trigger);
        }
        if let Some(url) = &pressed.open {
            self.core.open_link(url);
        }
        Some(pressed)
    }

    /// What a press at `at` lands on, without firing it: what the
    /// topmost pressable layer there does. `None` when the point is
    /// over nothing pressable, which a host may show as a plain cursor.
    pub fn pressed(&self, at: [f64; 2]) -> Option<Pressed> {
        let drawn = self.drawn().ok()?;
        hit_items(&drawn, at).into_iter().rev().find_map(|i| {
            let (_, press) = drawn
                .pressable
                .iter()
                .find(|(range, _)| range.contains(&i))?;
            Some(Pressed {
                trigger: press.trigger.clone(),
                open: press.open.clone(),
            })
        })
    }

    /// The layers drawn under `at`, topmost first, by the same test a
    /// press uses: a rect and a circle exact, anything else by its box.
    /// Every layer with something under the point is listed, the ones
    /// drawn over it first and the ones it covers after; a hidden layer
    /// and one clipped away at that point draw nothing there and are
    /// not. Empty over nothing. An editor selects by clicking with this,
    /// and reaches what lies behind by walking down the list.
    pub fn layers_at(&self, at: [f64; 2]) -> Vec<LayerPath> {
        let Ok(drawn) = self.drawn() else {
            return Vec::new();
        };
        let mut out: Vec<LayerPath> = Vec::new();
        for i in hit_items(&drawn, at).into_iter().rev() {
            let layer = &drawn.items[i].layer;
            if !out.contains(layer) {
                out.push(layer.clone());
            }
        }
        out
    }

    /// Output color handling for the current frame (the active scene's
    /// settings over the show's). Full color when no show is loaded.
    /// Renderers apply it to the finished frame.
    pub fn output(&self) -> OutputColor {
        // Tints were validated at load.
        OutputColor::from_output(&self.core.effective_output()).unwrap_or_default()
    }

    /// Resolve the show into a flat draw list: visible layers in paint
    /// order with absolute position and effective opacity. Clipped groups
    /// bracket their children with [`ResolvedShape::ClipBegin`] and
    /// [`ResolvedShape::ClipEnd`].
    ///
    /// Property precedence, strongest first: running timeline, binding,
    /// base value from the show description; the core's
    /// [`values`](cuelight_core::Engine::values) is the same answer
    /// before any geometry is built.
    pub fn resolved_layers(&self) -> Result<Vec<ResolvedLayer>, Error> {
        Ok(self.drawn()?.items)
    }

    /// The draw list, and which of its items belong to a layer that can
    /// be pressed.
    fn drawn(&self) -> Result<Drawn, Error> {
        self.core.show().ok_or(Error::NoShow)?;
        let mut out = Drawn::default();
        for (root, layers) in self.core.trees() {
            self.walk(root, layers, &mut Vec::new(), Inherited::TOP, &mut out)?;
        }
        Ok(out)
    }

    /// Resolve the frame and say what it cost: the time, and per visible
    /// layer its own share of it, what it put in the draw list and the
    /// strings it had rasterized afresh. For finding what makes a show
    /// slow; a frame drawn from the profile's items is the frame that
    /// was measured. The timing is the resolve alone, on the CPU: what a
    /// renderer then makes of the items is the host's to time.
    pub fn profile(&self) -> Result<FrameProfile, Error> {
        let show = self.core.show().ok_or(Error::NoShow)?;
        let mut out = Drawn {
            profile: Some(Vec::new()),
            ..Drawn::default()
        };
        let started = std::time::Instant::now();
        for (root, layers) in self.core.trees() {
            self.walk(root, layers, &mut Vec::new(), Inherited::TOP, &mut out)?;
        }
        let resolve = started.elapsed();
        let layers = out.profile.unwrap_or_default();
        let bindings = layers
            .iter()
            .filter_map(|cost| {
                let tree = cuelight_core::root_layers(show, cost.layer.root)?;
                cuelight_core::layer_at(tree, &cost.layer.indices).map(|l| l.bindings.len())
            })
            .sum();
        Ok(FrameProfile {
            items: out.items,
            resolve,
            layers,
            bindings,
            timelines: self.core.timelines_running(),
        })
    }

    /// What the text rasterizer has done since its cache was last
    /// emptied, and what it holds.
    pub fn text_stats(&self) -> TextStats {
        let cache = self.text_cache.lock().unwrap_or_else(|e| e.into_inner());
        TextStats {
            hits: cache.hits,
            misses: cache.misses,
            rasterized_bytes: cache.rasterized_bytes,
            cached_bytes: cache.rasters.bytes(),
            budget_bytes: MAX_RASTER_BYTES,
        }
    }

    /// The bytes of every image registered: what the decoded pictures
    /// of a show weigh in memory.
    pub fn image_bytes(&self) -> usize {
        self.images.values().map(|i| i.pixels.len()).sum()
    }

    /// A layer's content box `[x, y, width, height]` in its local space,
    /// before any scale; `None` for groups, unregistered images and text
    /// whose font is not registered.
    fn content_box(&self, root: Root, layer: &Layer, path: &[usize]) -> Option<[f64; 4]> {
        let [x, y, w, h] = match &layer.kind {
            LayerKind::Group { .. } | LayerKind::Audio { .. } | LayerKind::Part { .. } => {
                return None
            }
            LayerKind::Shape { shape, .. } => match shape {
                Shape::Rect { rect, .. } => *rect,
                Shape::Circle {
                    circle: [cx, cy, r],
                } => [cx - r, cy - r, 2.0 * r, 2.0 * r],
                Shape::Path { path } => PathElement::bounds(path.elements())?,
            },
            LayerKind::Video { size, .. } => {
                // Its own size until a frame says otherwise, so a layer has
                // a box before the host has decoded anything.
                let video = self.core.showing(root, path);
                let info = self.core.video(&video);
                let natural = info.map(|info| [info.width, info.height]).or_else(|| {
                    let frame = self.images.get(&frame_key(root, path))?;
                    Some([f64::from(frame.width), f64::from(frame.height)])
                })?;
                let [w, h] = size.unwrap_or(natural);
                [0.0, 0.0, w, h]
            }
            LayerKind::Image {
                image, size, sheet, ..
            } => {
                // Pixels or vector artwork, whichever is registered
                // under the name.
                let natural = match self.images.get(image) {
                    Some(data) => match sheet {
                        Some(sheet) => sheet.cell.map(f64::from),
                        None => [f64::from(data.width), f64::from(data.height)],
                    },
                    None => {
                        let art = self.vectors.get(image)?;
                        [art.width, art.height]
                    }
                };
                let [w, h] = size.unwrap_or(natural);
                [0.0, 0.0, w, h]
            }
            LayerKind::Digits { size: [w, h], .. } => [0.0, 0.0, *w, *h],
            // The text's box: its size, or the measured text.
            LayerKind::Text { size, align, .. } => {
                let [w, h] = match size {
                    Some(size) => *size,
                    None => {
                        let text = self.core.text(root, layer, path, Property::Text);
                        let font = self.core.text(root, layer, path, Property::Font);
                        match self.text_draw(&font, &text, None, *align, usize::MAX)? {
                            TextDraw::Bitmap(raster) => raster.container,
                            TextDraw::Glyphs { container, .. } => container,
                        }
                    }
                };
                [0.0, 0.0, w, h]
            }
        };
        Some([x, y, w, h])
    }

    /// How `text` in font style `style_name` gets drawn: a glyph run for an
    /// outline font, a raster for a bitmap font. `None` when the style's
    /// font is not registered (or is an outline font without a `size`).
    /// Only the first `shown` characters are drawn; the rest keep their
    /// room.
    fn text_draw(
        &self,
        style_name: &str,
        text: &str,
        size: Option<[f64; 2]>,
        align: Align,
        shown: usize,
    ) -> Option<TextDraw> {
        #[cfg(feature = "outline-fonts")]
        if let Some((style, font)) = self
            .core
            .show()
            .and_then(|show| show.fonts.get(style_name))
            .and_then(|style| Some((style, self.outline_fonts.get(&style.file)?)))
            .filter(|(style, _)| !self.pixels(style))
        {
            let layout = crate::outline::layout(font, text, style.size?, size, align, shown)?;
            return Some(TextDraw::Glyphs {
                font: font.clone(),
                size: style.size?,
                glyphs: layout.glyphs,
                container: layout.container,
            });
        }
        self.text_raster(style_name, text, size, align, shown, false)
            .map(TextDraw::Bitmap)
    }

    /// Rasterize (or fetch from cache) `text` in font style `style`.
    /// `None` when the style's font is not registered or nothing draws.
    ///
    /// `as_shadow` draws the same text in the style's shadow color, border
    /// included, which is the silhouette that sits behind it.
    fn text_raster(
        &self,
        style_name: &str,
        text: &str,
        size: Option<[f64; 2]>,
        align: Align,
        shown: usize,
        as_shadow: bool,
    ) -> Option<Arc<TextRaster>> {
        let style = self.core.show()?.fonts.get(style_name)?;
        let mut cache = self.text_cache.lock().unwrap_or_else(|e| e.into_inner());
        let registered = self.registered_font(&mut cache, style)?;
        let ink = if as_shadow { "\u{1}shadow" } else { "" };
        // Every way of asking for the whole text is one entry.
        let shown = shown.min(text.chars().count());
        let key = format!("{style_name}{ink}\u{1}{text}\u{1}{size:?}\u{1}{align:?}\u{1}{shown}");
        if let Some(raster) = cache.rasters.get(&key).cloned() {
            cache.hits += 1;
            return raster;
        }
        cache.misses += 1;
        let styled = cache
            .styled
            .entry(format!("{style_name}{ink}"))
            .or_insert_with(|| {
                // Colors were validated at load.
                let rgb = |c: &str| {
                    let [r, g, b, _] = parse_color(c).unwrap_or([255; 4]);
                    [r, g, b]
                };
                // A shadow is one color throughout, so its border is the
                // shadow color too.
                let color = match (as_shadow, &style.shadow) {
                    (true, Some(shadow)) => rgb(&shadow.color),
                    _ => rgb(&style.color),
                };
                let border = style
                    .border
                    .as_ref()
                    .map(|b| (if as_shadow { color } else { rgb(&b.color) }, b.width));
                Arc::new(StyledFont::new(
                    &registered.font,
                    &registered.pages,
                    color,
                    border,
                ))
            })
            .clone();
        let raster = styled
            .rasterize(text, size, align, shown)
            .map(|(rgba, offset, container)| {
                Arc::new(TextRaster {
                    image: ImageData::generated(rgba),
                    offset,
                    container,
                })
            });
        let bytes = key.len() + raster.as_ref().map_or(0, |r| r.image.pixels.len());
        cache.rasterized_bytes += bytes as u64;
        cache.rasters.insert(key, raster.clone(), bytes);
        raster
    }

    /// Add `text` in font style `style_name` to the draw list, laid out in
    /// a box of `size` (the text's own size when `None`) whose top-left
    /// corner sits at the item's origin, drawing only its first `shown`
    /// characters. Text layers and the characters of a reel both come
    /// through here.
    #[allow(clippy::too_many_arguments)]
    fn push_text(
        &self,
        out: &mut Vec<ResolvedLayer>,
        placed: &Placed,
        style_name: &str,
        text: &str,
        size: Option<[f64; 2]>,
        align: Align,
        shown: usize,
    ) {
        let Placed {
            name,
            layer,
            origin: [x, y],
            scale,
            opacity,
            blend,
            overflow,
            transform,
        } = *placed;
        // Colors were validated at load.
        let style = self.core.show().and_then(|s| s.fonts.get(style_name));
        let rgba = |c: &str| parse_color(c).unwrap_or([255; 4]);
        // The shadow goes in first, so the text lands on top of it. It is
        // the same text moved over, in one color, border included.
        let shadow = style.and_then(|s| s.shadow.as_ref()).map(|s| {
            let [sx, sy] = s.offset;
            (rgba(&s.color), sx * scale, sy * scale)
        });
        match self.text_draw(style_name, text, size, align, shown) {
            Some(TextDraw::Bitmap(raster)) => {
                let mut bitmap = |raster: &Arc<TextRaster>, dx: f64, dy: f64, alpha: f64| {
                    let [ox, oy] = raster.offset;
                    out.push(ResolvedLayer {
                        gradient: None,
                        overflow,
                        name: name.to_owned(),
                        layer: layer.clone(),
                        shape: ResolvedShape::Bitmap {
                            x: x + f64::from(ox) * scale + dx,
                            y: y + f64::from(oy) * scale + dy,
                            width: f64::from(raster.image.width) * scale,
                            height: f64::from(raster.image.height) * scale,
                            image: raster.image.clone(),
                        },
                        color: [255, 255, 255, 255],
                        opacity: opacity * alpha,
                        blend,
                        transform,
                    });
                };
                // A raster carries no color of its own to tint, so the
                // shadow is a second rasterization; its alpha rides on the
                // layer's opacity.
                if let Some(([.., a], dx, dy)) = shadow {
                    if let Some(behind) =
                        self.text_raster(style_name, text, size, align, shown, true)
                    {
                        bitmap(&behind, dx, dy, f64::from(a) / 255.0);
                    }
                }
                bitmap(&raster, 0.0, 0.0, 1.0);
            }
            Some(TextDraw::Glyphs {
                font: data,
                size: em,
                glyphs,
                ..
            }) if !glyphs.is_empty() => {
                let width = style
                    .and_then(|s| s.border.as_ref())
                    .map(|b| f64::from(b.width) * scale);
                let border = style
                    .and_then(|s| s.border.as_ref())
                    .map(|b| (rgba(&b.color), f64::from(b.width) * scale));
                let mut run = |ink: [u8; 4], edge: Option<([u8; 4], f64)>, dx: f64, dy: f64| {
                    out.push(ResolvedLayer {
                        gradient: None,
                        overflow,
                        name: name.to_owned(),
                        layer: layer.clone(),
                        shape: ResolvedShape::GlyphRun {
                            font: data.clone(),
                            size: em * scale,
                            glyphs: glyphs
                                .iter()
                                .map(|g| PlacedGlyph {
                                    id: g.id,
                                    x: x + g.x * scale + dx,
                                    y: y + g.y * scale + dy,
                                })
                                .collect(),
                            border: edge,
                        },
                        color: ink,
                        opacity,
                        blend,
                        transform,
                    });
                };
                if let Some((ink, dx, dy)) = shadow {
                    run(ink, width.map(|w| (ink, w)), dx, dy);
                }
                run(style.map_or([255; 4], |s| rgba(&s.color)), border, 0.0, 0.0);
            }
            _ => {}
        }
    }

    /// Add the artwork of ring place `index` to the draw list, fitted into
    /// a box of `size` at the item's origin and centred in it, so a symbol
    /// keeps its shape whatever shape the cells are.
    fn push_artwork(
        &self,
        out: &mut Vec<ResolvedLayer>,
        placed: &Placed,
        cells: &ReelCells,
        index: usize,
        [box_width, box_height]: [f64; 2],
    ) {
        let Some(name) = cells.at(index) else { return };
        let [x, y] = placed.origin;
        // How the artwork's own size fits the box, and where that leaves it.
        let fitted = |width: f64, height: f64| {
            let fit = (box_width / width).min(box_height / height);
            (
                fit,
                (box_width - width * fit) / 2.0,
                (box_height - height * fit) / 2.0,
            )
        };
        match cells {
            ReelCells::Vectors(_) => {
                // Missing artwork is skipped, as a missing image is.
                let Some(data) = self.vectors.get(name) else {
                    return;
                };
                if data.width <= 0.0 || data.height <= 0.0 {
                    return;
                }
                let (fit, left, top) = fitted(data.width, data.height);
                let unit = fit * placed.scale;
                let origin = [x + left * placed.scale, y + top * placed.scale];
                for item in &data.paths {
                    out.push(ResolvedLayer {
                        gradient: None,
                        overflow: placed.overflow,
                        name: placed.name.to_owned(),
                        layer: placed.layer.clone(),
                        shape: ResolvedShape::Path {
                            elements: item
                                .elements
                                .iter()
                                .map(|e| {
                                    e.map(|[px, py]| [origin[0] + px * unit, origin[1] + py * unit])
                                })
                                .collect(),
                            stroke: item.stroke.map(|(color, width)| (color, width * unit)),
                        },
                        color: item.fill.unwrap_or([0; 4]),
                        opacity: placed.opacity,
                        blend: placed.blend,
                        transform: placed.transform,
                    });
                }
            }
            ReelCells::Images(_) => {
                let Some(data) = self.images.get(name) else {
                    return;
                };
                let (natural_width, natural_height) =
                    (f64::from(data.width), f64::from(data.height));
                if natural_width <= 0.0 || natural_height <= 0.0 {
                    return;
                }
                let (fit, left, top) = fitted(natural_width, natural_height);
                out.push(ResolvedLayer {
                    gradient: None,
                    overflow: placed.overflow,
                    name: placed.name.to_owned(),
                    layer: placed.layer.clone(),
                    shape: ResolvedShape::Image {
                        image: name.to_owned(),
                        tile: None,
                        source: None,
                        x: x + left * placed.scale,
                        y: y + top * placed.scale,
                        width: natural_width * fit * placed.scale,
                        height: natural_height * fit * placed.scale,
                    },
                    color: [255; 4],
                    opacity: placed.opacity,
                    blend: placed.blend,
                    transform: placed.transform,
                });
            }
        }
    }

    /// How far down a character has to move for the ink of `characters`
    /// to sit in the middle of the box it is laid out in, rather than the
    /// line they share. 0 when the font cannot say.
    ///
    /// A line reserves room for descenders whether the characters use any
    /// or not, so a row of digits drawn in it sits high and leaves a gap
    /// beneath. Measuring the whole ring at once, rather than each symbol
    /// as it comes, keeps the row still while it rolls: a ring that holds
    /// a descender reserves it, one of digits does not.
    fn ink_centring(&self, style_name: &str, characters: &[char]) -> f64 {
        let Some(style) = self.core.show().and_then(|show| show.fonts.get(style_name)) else {
            return 0.0;
        };
        #[cfg(feature = "outline-fonts")]
        if let (Some(font), Some(size), false) = (
            self.outline_fonts.get(&style.file),
            style.size,
            self.pixels(style),
        ) {
            if let Some((top, bottom, line)) = crate::outline::ink(font, size, characters) {
                return (line - (bottom - top)) / 2.0 - top;
            }
        }
        let mut cache = self.text_cache.lock().unwrap_or_else(|e| e.into_inner());
        let Some(registered) = self.registered_font(&mut cache, style) else {
            return 0.0;
        };
        let styled = cache.styled.get(style_name).cloned();
        drop(cache);
        let styled = styled.unwrap_or_else(|| {
            // Colors were validated at load.
            let rgb = |c: &str| {
                let [r, g, b, _] = parse_color(c).unwrap_or([255; 4]);
                [r, g, b]
            };
            let border = style.border.as_ref().map(|b| (rgb(&b.color), b.width));
            Arc::new(StyledFont::new(
                &registered.font,
                &registered.pages,
                rgb(&style.color),
                border,
            ))
        });
        match styled.ink(characters) {
            Some((top, bottom)) => (styled.line() - (bottom - top)) / 2.0 - top,
            None => 0.0,
        }
    }

    /// What each of `parts` does to its element of `art` now, by id: a
    /// transform in the artwork's own coordinates, around the part's
    /// pivot (the centre of the element's bounds when it names none),
    /// with its opacity; a part that is not visible hides its element.
    fn parts_of(
        &self,
        root: Root,
        parts: &[Layer],
        path: &mut Vec<usize>,
        image: &str,
    ) -> HashMap<String, Moved> {
        let bounds = self.part_bounds.get(image);
        let mut out = HashMap::new();
        for (i, part) in parts.iter().enumerate() {
            let LayerKind::Part { id, pivot } = &part.kind else {
                continue;
            };
            path.push(i);
            let number = |prop| self.core.number(root, part, path, prop);
            let moved = if self.core.is_visible(root, part, path) {
                let pivot = pivot
                    .or_else(|| {
                        let [x, y, w, h] = *bounds?.get(id)?;
                        Some([x + w / 2.0, y + h / 2.0])
                    })
                    .unwrap_or([0.0, 0.0]);
                let scale = number(Property::Scale);
                let transform = Transform::translate(number(Property::X), number(Property::Y))
                    .then(Transform::translate(pivot[0], pivot[1]))
                    .then(Transform::rotate(number(Property::Rotation)))
                    .then(Transform::scale(
                        scale * number(Property::ScaleX),
                        scale * number(Property::ScaleY),
                    ))
                    .then(Transform::translate(-pivot[0], -pivot[1]));
                Moved {
                    transform,
                    opacity: number(Property::Opacity).clamp(0.0, 1.0),
                    hidden: false,
                }
            } else {
                Moved {
                    transform: Transform::IDENTITY,
                    opacity: 0.0,
                    hidden: true,
                }
            };
            path.pop();
            out.insert(id.clone(), moved);
        }
        out
    }

    /// Whether `style` is drawn as exact pixels: an outline font the
    /// style asks it of, or any outline font on a show rendered on its
    /// own pixel grid, where nothing should sit between two pixels.
    #[cfg(feature = "outline-fonts")]
    fn pixels(&self, style: &cuelight_core::FontStyle) -> bool {
        self.outline_fonts.contains_key(&style.file)
            && style.pixels.unwrap_or_else(|| self.core.pixel_grid())
    }

    /// The bitmap font `style` draws with: the one registered under its
    /// name, or its outline font rasterized into pixels at its size,
    /// once, with room round each glyph for the style's border.
    fn registered_font(
        &self,
        cache: &mut TextCache,
        style: &cuelight_core::FontStyle,
    ) -> Option<Arc<RegisteredFont>> {
        if let Some(registered) = self.fonts.get(&style.file) {
            return Some(registered.clone());
        }
        #[cfg(feature = "outline-fonts")]
        if self.pixels(style) {
            let size = style.size.filter(|size| *size > 0.0)?;
            let pad = style.border.as_ref().map_or(0, |b| b.width);
            let key = (style.file.clone(), size.to_bits(), pad);
            if let Some(registered) = cache.pixels.get(&key) {
                return Some(registered.clone());
            }
            let font = self.outline_fonts.get(&style.file)?;
            let (font, page) = crate::pixels::rasterize(font, size, pad)?;
            let registered = Arc::new(RegisteredFont {
                font,
                pages: vec![page],
            });
            cache.pixels.insert(key, registered.clone());
            return Some(registered);
        }
        let _ = cache;
        None
    }

    /// Add a reel row to the draw list: each cell a window on its ring,
    /// showing the character it stands on and the one coming after it,
    /// slid by how far between the two it is. A cell whose character is
    /// not on the ring shows nothing.
    #[allow(clippy::too_many_arguments)]
    /// Only the cells of the first `shown` characters are drawn; the
    /// rest stay empty.
    #[allow(clippy::too_many_arguments)]
    fn push_reel(
        &self,
        out: &mut Vec<ResolvedLayer>,
        placed: &Placed,
        reel: &Reel,
        text: &str,
        shown: usize,
        [width, height]: [f64; 2],
        (count, justify): (usize, Justify),
        positions: Vec<f64>,
    ) {
        let ring = reel.characters();
        if ring.is_empty() || count == 0 {
            return;
        }
        let [x, y] = placed.origin;
        let scale = placed.scale;
        // The cell's box in the layer's own units, for the font to lay a
        // character out in, and on the canvas, for placing it.
        let window = reel.window.max(1);
        let cell = [width / count as f64, height / f64::from(window)];
        let (cell_w, character_h) = (cell[0] * scale, cell[1] * scale);
        let cell_h = character_h * f64::from(window);
        // Where the character a cell stands on sits in its window.
        let middle = (f64::from(window) - 1.0) / 2.0;
        // A cell is a window its symbol should sit in the middle of, so
        // the characters are centred on their ink, as artwork is on its
        // own box, rather than on the line they are laid out in.
        let centring = match (&reel.cells, &reel.font) {
            (None, Some(font)) => self.ink_centring(font, &ring) * scale,
            _ => 0.0,
        };
        // Which character of the text a cell holds: with `right`, the
        // text sits against the far end of the row.
        let characters = text.chars().count();
        let character_of = |cell: usize| match justify {
            Justify::Right if characters >= count => Some(cell + characters - count),
            Justify::Right => cell.checked_sub(count - characters),
            _ => Some(cell),
        };
        for (i, character) in row_cells(text, count, justify).into_iter().enumerate() {
            if character.is_none_or(|c| !ring.contains(&c)) {
                continue;
            }
            if character_of(i).is_none_or(|c| c >= shown) {
                continue;
            }
            let position = positions.get(i).copied().unwrap_or_default();
            let cell_x = x + i as f64 * cell_w;
            let marker = |shape| ResolvedLayer {
                gradient: None,
                overflow: placed.overflow,
                name: placed.name.to_owned(),
                layer: placed.layer.clone(),
                shape,
                color: [0; 4],
                opacity: placed.opacity,
                blend: Blend::Normal,
                transform: placed.transform,
            };
            // Only what stands in the window shows.
            out.push(marker(ResolvedShape::ClipBegin {
                shape: Box::new(ResolvedShape::Rect {
                    x: cell_x,
                    y,
                    width: cell_w,
                    height: cell_h,
                }),
            }));
            // Every character the window can see, from the one leaving at
            // its top to the one coming up at its bottom.
            let first = (position - middle).floor();
            for k in 0..=window {
                let at = first + f64::from(k);
                let slide = at - position + middle;
                let on = at.rem_euclid(ring.len() as f64) as usize;
                let placed = Placed {
                    origin: [cell_x, y + slide * character_h + centring],
                    ..*placed
                };
                match (&reel.cells, &reel.font) {
                    (Some(cells), _) => self.push_artwork(out, &placed, cells, on, cell),
                    (None, Some(font)) => {
                        let mut buffer = [0u8; 4];
                        let character = ring[on].encode_utf8(&mut buffer);
                        self.push_text(
                            out,
                            &placed,
                            font,
                            character,
                            Some(cell),
                            Align::Center,
                            usize::MAX,
                        );
                    }
                    (None, None) => {}
                }
            }
            out.push(marker(ResolvedShape::ClipEnd));
        }
    }

    /// Resolve `layers` under `from`, what the tree above them passes
    /// down.
    fn walk(
        &self,
        root: Root,
        layers: &[Layer],
        path: &mut Vec<usize>,
        from: Inherited,
        built: &mut Drawn,
    ) -> Result<(), Error> {
        let Inherited {
            transform: parent,
            opacity: oa,
            overflow: bleeding,
        } = from;
        for (i, layer) in layers.iter().enumerate() {
            path.push(i);
            if self.core.is_visible(root, layer, path) {
                let here = LayerPath::new(root, path.clone());
                // Where this layer's drawing starts, so a press can be
                // tested against what it drew rather than against a
                // second guess at where it went.
                let from = built.items.len();
                // When profiling: when this layer's work began, how
                // many costs were on record (its children's come after)
                // and the rasterizer's count of fresh strings.
                let measuring = built.profile.as_ref().map(|costs| {
                    let misses = self
                        .text_cache
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .misses;
                    (std::time::Instant::now(), costs.len(), misses)
                });
                // A group that may bleed lets its whole subtree bleed.
                let overflow = bleeding || layer.overflow;
                let number = |prop| self.core.number(root, layer, path, prop);
                let opacity = (oa * number(Property::Opacity)).clamp(0.0, 1.0);
                let scale = number(Property::Scale);
                let (sx, sy) = (
                    scale * number(Property::ScaleX),
                    scale * number(Property::ScaleY),
                );
                // The anchor point, in the layer's scaled space: it lands on
                // x/y and the layer turns and scales around it.
                let pivot = layer
                    .anchor
                    .and_then(|anchor| {
                        let [bx, by, bw, bh] = self.content_box(root, layer, path)?;
                        let (ax, ay) = anchor.offset(bw, bh, 0.0, 0.0);
                        Some([(bx - ax) * sx, (by - ay) * sy])
                    })
                    .unwrap_or([0.0, 0.0]);
                let local = Transform::translate(number(Property::X), number(Property::Y))
                    .then(Transform::rotate(number(Property::Rotation)))
                    .then(Transform::translate(-pivot[0], -pivot[1]))
                    .then(Transform::scale(sx, sy));
                let m = parent.then(local);
                // Baked into the coordinates where it can be; otherwise the
                // shape stays local and the transform carries it.
                let ((scale, x, y), transform) = match m.plain() {
                    Some(plain) => (plain, Transform::IDENTITY),
                    None => ((1.0, 0.0, 0.0), m),
                };
                match &layer.kind {
                    // Heard, not seen.
                    LayerKind::Audio { .. } => {}
                    // Drawn by its artwork layer, which reads it.
                    LayerKind::Part { .. } => {}
                    LayerKind::Group { children, clip, .. } => {
                        // A blended group is composited as one picture.
                        let mut marker = |shape: ResolvedShape| {
                            built.items.push(ResolvedLayer {
                                gradient: None,
                                overflow,
                                name: layer.name.clone(),
                                layer: here.clone(),
                                shape,
                                color: [0; 4],
                                opacity,
                                blend: Blend::Normal,
                                transform,
                            });
                        };
                        if layer.blend != Blend::Normal {
                            marker(ResolvedShape::BlendBegin { blend: layer.blend });
                        }
                        if let Some(clip) = clip {
                            marker(ResolvedShape::ClipBegin {
                                shape: Box::new(resolve_shape(clip, x, y, scale, None)),
                            });
                        }
                        self.walk(
                            root,
                            children,
                            path,
                            Inherited {
                                transform: m,
                                opacity,
                                overflow,
                            },
                            built,
                        )?;
                        if clip.is_some() {
                            built.items.push(ResolvedLayer {
                                gradient: None,
                                overflow,
                                name: layer.name.clone(),
                                layer: here.clone(),
                                shape: ResolvedShape::ClipEnd,
                                color: [0; 4],
                                opacity,
                                blend: Blend::Normal,
                                transform,
                            });
                        }
                        if layer.blend != Blend::Normal {
                            built.items.push(ResolvedLayer {
                                gradient: None,
                                overflow,
                                name: layer.name.clone(),
                                layer: here.clone(),
                                shape: ResolvedShape::BlendEnd,
                                color: [0; 4],
                                opacity,
                                blend: Blend::Normal,
                                transform,
                            });
                        }
                    }
                    LayerKind::Shape {
                        shape,
                        fill,
                        stroke,
                    } => {
                        // A gradient keeps the layer's color as what a
                        // host without gradients would draw: its first stop.
                        let (color, gradient) = match fill {
                            Fill::Color(color) => (
                                parse_color(color)
                                    .ok_or_else(|| Error::InvalidColor(color.clone()))?,
                                None,
                            ),
                            Fill::Gradient(gradient) => (
                                gradient
                                    .stops()
                                    .first()
                                    .and_then(|s| parse_color(&s.color))
                                    .unwrap_or([255; 4]),
                                Some(resolve_gradient(gradient, x, y, scale)?),
                            ),
                        };
                        let stroke = stroke
                            .as_ref()
                            .map(|s| {
                                let color = parse_color(&s.color)
                                    .ok_or_else(|| Error::InvalidColor(s.color.clone()))?;
                                Ok::<_, Error>((color, s.width * scale))
                            })
                            .transpose()?;
                        built.items.push(ResolvedLayer {
                            gradient,
                            overflow,
                            name: layer.name.clone(),
                            layer: here.clone(),
                            shape: resolve_shape(shape, x, y, scale, stroke),
                            color,
                            opacity,
                            blend: layer.blend,
                            transform,
                        });
                    }
                    LayerKind::Video { size, .. } => {
                        // The frame the host last handed over for whatever
                        // is playing. Nothing playing draws nothing, so
                        // what is behind shows through when a clip ends,
                        // and nothing is drawn before a frame arrives.
                        let playing = self.core.playing_on(root, path);
                        let video = playing.unwrap_or_default();
                        let key = playing.map(|_| frame_key(root, path)).unwrap_or_default();
                        if let Some(frame) = self.images.get(&key) {
                            let natural = self.core.video(video).map_or(
                                [f64::from(frame.width), f64::from(frame.height)],
                                |info| [info.width, info.height],
                            );
                            let [width, height] = size.unwrap_or(natural);
                            built.items.push(ResolvedLayer {
                                gradient: None,
                                overflow,
                                name: layer.name.clone(),
                                layer: here.clone(),
                                shape: ResolvedShape::Image {
                                    image: key,
                                    tile: None,
                                    source: None,
                                    x,
                                    y,
                                    width: width * scale,
                                    height: height * scale,
                                },
                                color: [255; 4],
                                opacity,
                                blend: layer.blend,
                                transform,
                            });
                        }
                    }
                    LayerKind::Image {
                        image,
                        size,
                        sheet,
                        repeat,
                        parts,
                        ..
                    } => {
                        // Vector artwork under the same name, drawn as
                        // the paths it is rather than as pixels; the
                        // layer is the same either way.
                        let art = match self.images.contains_key(image) {
                            true => None,
                            false => self.vectors.get(image),
                        };
                        if let Some(art) = art {
                            let moved = self.parts_of(root, parts, path, image);
                            let tint = self.core.text(root, layer, path, Property::Tint);
                            let tint = parse_color(&tint).unwrap_or([255; 4]);
                            let tile = repeat.map(|tile| {
                                let [tw, th] = tile.size.unwrap_or([art.width, art.height]);
                                Tiled {
                                    width: tw * scale,
                                    height: th * scale,
                                    offset: [
                                        self.core.number(root, layer, path, Property::TileX)
                                            * scale,
                                        self.core.number(root, layer, path, Property::TileY)
                                            * scale,
                                    ],
                                }
                            });
                            let box_size = size.unwrap_or([art.width, art.height]);
                            push_vector(
                                &mut built.items,
                                art,
                                &Placed {
                                    overflow,
                                    name: &layer.name,
                                    layer: &here,
                                    origin: [x, y],
                                    scale,
                                    opacity,
                                    blend: layer.blend,
                                    transform,
                                },
                                box_size,
                                tile,
                                tint,
                                &moved,
                            );
                        }
                        // Missing images are skipped, not an error: the
                        // host may provide them later.
                        else if let Some(data) = self.images.get(image) {
                            let source = sheet.map(|sheet| {
                                let frame = self.core.number(root, layer, path, Property::Frame);
                                sheet_cell(sheet, data.width, data.height, frame)
                            });
                            let natural = match source {
                                Some([_, _, w, h]) => [f64::from(w), f64::from(h)],
                                None => [f64::from(data.width), f64::from(data.height)],
                            };
                            let [width, height] = size.unwrap_or(natural);
                            // Tiling covers the layer's size with copies
                            // of one tile; without it the image is
                            // stretched to that size, as it always was.
                            let tile = repeat.map(|tile| {
                                let [tw, th] = tile.size.unwrap_or(natural);
                                Tiled {
                                    width: tw * scale,
                                    height: th * scale,
                                    offset: [
                                        self.core.number(root, layer, path, Property::TileX)
                                            * scale,
                                        self.core.number(root, layer, path, Property::TileY)
                                            * scale,
                                    ],
                                }
                            });
                            built.items.push(ResolvedLayer {
                                gradient: None,
                                overflow,
                                name: layer.name.clone(),
                                layer: here.clone(),
                                shape: ResolvedShape::Image {
                                    image: image.clone(),
                                    source,
                                    x,
                                    y,
                                    width: width * scale,
                                    height: height * scale,
                                    tile,
                                },
                                // Validated at load, and a binding only
                                // ever feeds it a color it could parse.
                                color: {
                                    let tint = self.core.text(root, layer, path, Property::Tint);
                                    parse_color(&tint).unwrap_or([255; 4])
                                },
                                opacity,
                                blend: layer.blend,
                                transform,
                            });
                        }
                    }
                    LayerKind::Digits {
                        digits,
                        size: [width, height],
                        justify,
                        display,
                        ..
                    } => {
                        let text = self.core.text(root, layer, path, Property::Text);
                        let shown =
                            revealed(&text, self.core.number(root, layer, path, Property::Reveal));
                        let placed = Placed {
                            overflow,
                            name: &layer.name,
                            layer: &here,
                            origin: [x, y],
                            scale,
                            opacity,
                            blend: layer.blend,
                            transform,
                        };
                        let cells = (*digits as usize, *justify);
                        match display {
                            DigitDisplay::Segments {
                                style,
                                fill,
                                unlit,
                                slant,
                                thickness,
                                glow,
                            } => {
                                let lit = parse_color(fill)
                                    .ok_or_else(|| Error::InvalidColor(fill.clone()))?;
                                let unlit = unlit
                                    .as_ref()
                                    .map(|c| {
                                        parse_color(c).ok_or_else(|| Error::InvalidColor(c.clone()))
                                    })
                                    .transpose()?;
                                let masks = segments::masks(*style, &text, cells.0, cells.1, shown);
                                // Where the frame is made on the canvas's own
                                // pixel grid, segments keep to it.
                                let snap =
                                    self.core.pixel_grid() && transform == Transform::IDENTITY;
                                let cell_w = width * scale / cells.0.max(1) as f64;
                                let look = segments::Look {
                                    thickness: thickness.unwrap_or(0.1),
                                    slant: *slant,
                                    grow: 0.0,
                                };
                                let cell_of =
                                    |i: usize| [x + i as f64 * cell_w, y, cell_w, height * scale];
                                let drawn = |shape: ResolvedShape, color: [u8; 4], blend: Blend| {
                                    ResolvedLayer {
                                        gradient: None,
                                        overflow,
                                        name: layer.name.clone(),
                                        layer: here.clone(),
                                        shape,
                                        color,
                                        opacity,
                                        blend,
                                        transform,
                                    }
                                };
                                let push =
                                    |out: &mut Vec<ResolvedLayer>,
                                     mask: u16,
                                     color: [u8; 4],
                                     look: segments::Look,
                                     blend: Blend,
                                     cell: [f64; 4]| {
                                        for points in
                                            segments::polygons(*style, mask, cell, snap, look)
                                        {
                                            out.push(drawn(
                                                ResolvedShape::Polygon { points },
                                                color,
                                                blend,
                                            ));
                                        }
                                    };
                                // The dark segments of the whole display
                                // first, then the halo over all of them,
                                // then the lit ones on top: a halo falls
                                // on its neighbours as much as on its own
                                // cell, and nothing dark should sit over
                                // light that reached it.
                                if let Some(unlit) = unlit {
                                    for (i, mask) in masks.iter().enumerate() {
                                        push(
                                            &mut built.items,
                                            !mask,
                                            unlit,
                                            look,
                                            layer.blend,
                                            cell_of(i),
                                        );
                                    }
                                }
                                if let Some(glow) = glow {
                                    let reach = (glow.size * cell_w).max(0.0);
                                    let strength = glow.strength.clamp(0.0, 1.0);
                                    // One picture of the segment's own
                                    // colour, at an opacity that falls
                                    // off outward. Where two halos meet
                                    // the brighter one shows, so the
                                    // light never climbs past the colour
                                    // of the segment casting it: added
                                    // together instead, two halos of a
                                    // warm colour saturate their red and
                                    // go on brightening the rest, which
                                    // turns the joins yellow-white.
                                    built.items.push(drawn(
                                        ResolvedShape::BlendBegin {
                                            blend: Blend::Screen,
                                        },
                                        [0; 4],
                                        Blend::Normal,
                                    ));
                                    for (i, mask) in masks.iter().enumerate() {
                                        // Widest and faintest outward, so
                                        // the segment itself lands on top.
                                        for step in (1..=GLOW_STEPS).rev() {
                                            let out_to = f64::from(step) / f64::from(GLOW_STEPS);
                                            let from = f64::from(step - 1) / f64::from(GLOW_STEPS);
                                            let [r, g, b, a] = lit;
                                            // The halo falls off as the
                                            // square of the distance &mut built.items,
                                            // which keeps most of the
                                            // light within a bar's width
                                            // of the segment and reaches
                                            // the segment's own colour at
                                            // full strength where it
                                            // leaves it. The passes are
                                            // drawn widest first and each
                                            // lands on the ones outside
                                            // it, so a pass carries what
                                            // is left to reach the
                                            // falloff there rather than
                                            // the whole of it, and the
                                            // halo reads the same however
                                            // many passes it is drawn in.
                                            let falloff =
                                                |u: f64| strength * (1.0 - u).max(0.0).powi(2);
                                            let (here, outside) = (falloff(from), falloff(out_to));
                                            let share = match outside < 1.0 {
                                                true => 1.0 - (1.0 - here) / (1.0 - outside),
                                                false => 0.0,
                                            };
                                            let alpha = f64::from(a) * share;
                                            push(
                                                &mut built.items,
                                                *mask,
                                                [r, g, b, (alpha.clamp(0.0, 255.0)) as u8],
                                                segments::Look {
                                                    grow: reach * out_to,
                                                    ..look
                                                },
                                                Blend::Normal,
                                                cell_of(i),
                                            );
                                        }
                                    }
                                    built.items.push(drawn(
                                        ResolvedShape::BlendEnd,
                                        [0; 4],
                                        Blend::Normal,
                                    ));
                                }
                                for (i, mask) in masks.iter().enumerate() {
                                    push(
                                        &mut built.items,
                                        *mask,
                                        lit,
                                        look,
                                        layer.blend,
                                        cell_of(i),
                                    );
                                }
                            }
                            DigitDisplay::Reel(reel) => self.push_reel(
                                &mut built.items,
                                &placed,
                                reel,
                                &text,
                                shown,
                                [*width, *height],
                                cells,
                                self.core.reel_positions(root, path, reel),
                            ),
                        }
                    }
                    LayerKind::Text { size, align, .. } => {
                        let text = self.core.text(root, layer, path, Property::Text);
                        let font = self.core.text(root, layer, path, Property::Font);
                        let shown =
                            revealed(&text, self.core.number(root, layer, path, Property::Reveal));
                        let placed = Placed {
                            overflow,
                            name: &layer.name,
                            layer: &here,
                            origin: [x, y],
                            scale,
                            opacity,
                            blend: layer.blend,
                            transform,
                        };
                        self.push_text(
                            &mut built.items,
                            &placed,
                            &font,
                            &text,
                            *size,
                            *align,
                            shown,
                        );
                    }
                }
                if let Some(press) = &layer.press {
                    built
                        .pressable
                        .push((from..built.items.len(), press.clone()));
                }
                if let Some((started, mark, misses_before)) = measuring {
                    let total = started.elapsed();
                    let misses = self
                        .text_cache
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .misses;
                    let costs = built.profile.as_mut().expect("measuring");
                    // The children's own costs were recorded while this
                    // layer ran; its own share is what is left.
                    let depth = path.len();
                    let children: std::time::Duration = costs[mark..]
                        .iter()
                        .filter(|c| c.layer.indices.len() == depth + 1)
                        .map(|c| c.total)
                        .sum();
                    let (mut path_elements, mut glyphs, mut pixels) = (0, 0, 0.0);
                    for item in &built.items[from..] {
                        match &item.shape {
                            ResolvedShape::Path { elements, .. } => path_elements += elements.len(),
                            ResolvedShape::Polygon { points } => path_elements += points.len(),
                            ResolvedShape::GlyphRun { glyphs: run, .. } => glyphs += run.len(),
                            ResolvedShape::Image { width, height, .. }
                            | ResolvedShape::Bitmap { width, height, .. } => {
                                pixels += width.abs() * height.abs();
                            }
                            _ => {}
                        }
                    }
                    costs.push(LayerCost {
                        layer: here.clone(),
                        name: layer.name.clone(),
                        kind: kind_name(&layer.kind),
                        total,
                        own: total.saturating_sub(children),
                        items: built.items.len() - from,
                        path_elements,
                        glyphs,
                        pixels,
                        text_misses: misses - misses_before,
                    });
                }
            }
            path.pop();
        }
        Ok(())
    }
}

/// A layer's kind as the document names it.
fn kind_name(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Group { .. } => "group",
        LayerKind::Shape { .. } => "shape",
        LayerKind::Image { .. } => "image",
        LayerKind::Text { .. } => "text",
        LayerKind::Digits { .. } => "digits",
        LayerKind::Video { .. } => "video",
        LayerKind::Audio { .. } => "audio",
        LayerKind::Part { .. } => "part",
    }
}
/// The items of a frame under `at`, in paint order, clips honoured.
/// Markers cover nothing.
fn hit_items(drawn: &Drawn, at: [f64; 2]) -> Vec<usize> {
    let mut clips: Vec<ResolvedShape> = Vec::new();
    let mut hits = Vec::new();
    for (i, item) in drawn.items.iter().enumerate() {
        match &item.shape {
            ResolvedShape::ClipBegin { shape } => {
                clips.push((**shape).clone());
                continue;
            }
            ResolvedShape::ClipEnd => {
                clips.pop();
                continue;
            }
            _ => {}
        }
        let inside = |shape: &ResolvedShape| match item.transform.invert() {
            // The shape is in its layer's own space when a rotation or
            // an uneven scale put it there; the point comes back the
            // same way.
            Some(back) => covers(shape, back.apply(at)),
            None => false,
        };
        if inside(&item.shape) && clips.iter().all(inside) {
            hits.push(i);
        }
    }
    hits
}

/// Whether `at` is inside a drawn shape, in the shape's own
/// coordinates.
///
/// Exact for a rect and a circle, which are most of what a show makes
/// pressable; anything else by the box it fills, which is predictable
/// and enough to start with. A marker covers nothing.
fn covers(shape: &ResolvedShape, at: [f64; 2]) -> bool {
    let [px, py] = at;
    let box_of = |[x, y, w, h]: [f64; 4]| px >= x && px <= x + w && py >= y && py <= y + h;
    match shape {
        ResolvedShape::Rect {
            x,
            y,
            width,
            height,
        } => box_of([*x, *y, *width, *height]),
        ResolvedShape::Circle { cx, cy, radius } => {
            let (dx, dy) = (px - cx, py - cy);
            dx * dx + dy * dy <= radius * radius
        }
        ResolvedShape::Polygon { points } => within(points, at),
        ResolvedShape::Path { elements, .. } => PathElement::bounds(elements).is_some_and(box_of),
        ResolvedShape::Image {
            x,
            y,
            width,
            height,
            ..
        }
        | ResolvedShape::Bitmap {
            x,
            y,
            width,
            height,
            ..
        } => box_of([*x, *y, *width, *height]),
        // Glyphs are outlines the host rasterizes, so the run is taken
        // as the line it sits on: its glyph boxes at the size it is
        // drawn.
        ResolvedShape::GlyphRun { size, glyphs, .. } => glyphs
            .iter()
            .any(|glyph| box_of([glyph.x, glyph.y - size, *size, *size])),
        ResolvedShape::ClipBegin { shape } => covers(shape, at),
        ResolvedShape::ClipEnd | ResolvedShape::BlendBegin { .. } | ResolvedShape::BlendEnd => {
            false
        }
    }
}

/// Whether `at` is inside a closed polygon, by crossings.
fn within(points: &[[f64; 2]], [px, py]: [f64; 2]) -> bool {
    let mut inside = false;
    for (i, &[x1, y1]) in points.iter().enumerate() {
        let [x2, y2] = points[(i + 1) % points.len()];
        if (y1 > py) != (y2 > py) && px < (x2 - x1) * (py - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    inside
}

/// What the text rasterizer has done since its cache was last emptied
/// (a font registered, a show loaded); see [`Engine::text_stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextStats {
    /// Rasterizations served from the cache: a string drawn again.
    pub hits: u64,
    /// Rasterizations made afresh: a string not seen before, or one the
    /// cache had let go of.
    pub misses: u64,
    /// The bytes those fresh rasterizations came to, all told.
    pub rasterized_bytes: u64,
    /// What the cache holds now, and the most it keeps.
    pub cached_bytes: usize,
    pub budget_bytes: usize,
}

/// What one layer cost to resolve in one frame; see [`Engine::profile`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LayerCost {
    pub layer: LayerPath,
    pub name: String,
    /// The layer's kind, as the document names it: `text`, `image`, ...
    pub kind: &'static str,
    /// Resolving this layer and everything under it.
    pub total: std::time::Duration,
    /// Resolving this layer alone: `total` less its children's.
    pub own: std::time::Duration,
    /// Draw list items it and its subtree put in the frame, and what
    /// they ask of a renderer: path elements to fill, glyphs to draw,
    /// pixels of images and rasters to paint (drawn size, not source).
    pub items: usize,
    pub path_elements: usize,
    pub glyphs: usize,
    pub pixels: f64,
    /// Strings rasterized afresh for it this frame.
    pub text_misses: u64,
}

/// One frame resolved with its costs; see [`Engine::profile`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FrameProfile {
    /// The draw list, the same [`resolved_layers`](Engine::resolved_layers)
    /// gives, so a host can draw the frame it measured.
    pub items: Vec<ResolvedLayer>,
    /// Resolving the whole frame.
    pub resolve: std::time::Duration,
    /// Every visible layer, in resolve order.
    pub layers: Vec<LayerCost>,
    /// Bindings on the visible layers, evaluated this frame.
    pub bindings: usize,
    /// Timelines running this frame.
    pub timelines: usize,
}

/// A resolved frame: what to draw, and which of it can be pressed.
#[derive(Debug, Default)]
struct Drawn {
    items: Vec<ResolvedLayer>,
    /// Per visible layer, what it cost, when the frame is being
    /// profiled.
    profile: Option<Vec<LayerCost>>,
    /// Per pressable layer, the items it drew and what a press on them
    /// does, in paint order. Item ranges rather than shapes, so a press
    /// is tested against the very geometry the frame drew, clips and
    /// transforms included.
    pressable: Vec<(std::ops::Range<usize>, Press)>,
}

/// What a layer takes from the tree above it.
#[derive(Debug, Clone, Copy)]
struct Inherited {
    transform: Transform,
    opacity: f64,
    /// Whether the tree above may draw past the canvas.
    overflow: bool,
}

impl Inherited {
    /// At the top of a tree: nothing placed, nothing faded, nothing
    /// allowed past the canvas yet.
    const TOP: Inherited = Inherited {
        transform: Transform::IDENTITY,
        opacity: 1.0,
        overflow: false,
    };
}

/// The most copies of a tiled piece of artwork drawn in one layer.
///
/// A tile far smaller than the box it fills is a mistake rather than a
/// picture, and each copy is every path of the artwork; this draws what
/// fits in the cap and leaves the rest.
const MAX_TILES: usize = 4096;

/// Draw vector artwork into `box_size`, stretched to it or tiled across
/// it, every colour multiplied by `tint`.
fn push_vector(
    out: &mut Vec<ResolvedLayer>,
    art: &Vector,
    placed: &Placed,
    box_size: [f64; 2],
    tile: Option<Tiled>,
    tint: [u8; 4],
    moved: &HashMap<String, Moved>,
) {
    let [x, y] = placed.origin;
    let scale = placed.scale;
    let stain = |color: [u8; 4]| {
        let mix = |c: u8, t: u8| ((u32::from(c) * u32::from(t)) / 255) as u8;
        [
            mix(color[0], tint[0]),
            mix(color[1], tint[1]),
            mix(color[2], tint[2]),
            mix(color[3], tint[3]),
        ]
    };
    // What the parts a path is inside do to it, outermost first, so a
    // jaw turns with the head it is in: none of them, the identity.
    let part_of = |item: &VectorPath| -> Option<(Transform, f64)> {
        let mut transform = Transform::IDENTITY;
        let mut opacity = 1.0;
        for id in &item.ids {
            let Some(part) = moved.get(id) else {
                continue;
            };
            if part.hidden {
                return None;
            }
            transform = transform.then(part.transform);
            opacity *= part.opacity;
        }
        Some((transform, opacity))
    };
    let paths = |at: [f64; 2], size: [f64; 2], out: &mut Vec<ResolvedLayer>| {
        let (sx, sy) = (size[0] / art.width, size[1] / art.height);
        for item in &art.paths {
            let Some((by, opacity)) = part_of(item) else {
                continue;
            };
            // How much the parts above it scale it: its stroke grows and
            // shrinks with it, the average of the two axes, as the
            // artwork's own size scales strokes.
            let [a, b, c, d, ..] = by.0;
            let part_scale = (a.hypot(b) + c.hypot(d)) / 2.0;
            out.push(ResolvedLayer {
                gradient: None,
                overflow: placed.overflow,
                name: placed.name.to_owned(),
                layer: placed.layer.clone(),
                shape: ResolvedShape::Path {
                    elements: item
                        .elements
                        .iter()
                        .map(|e| {
                            e.map(|p| {
                                let [px, py] = by.apply(p);
                                [at[0] + px * sx, at[1] + py * sy]
                            })
                        })
                        .collect(),
                    stroke: item
                        .stroke
                        .map(|(c, w)| (stain(c), w * part_scale * (sx + sy) / 2.0)),
                },
                color: stain(item.fill.unwrap_or([0; 4])),
                opacity: placed.opacity * opacity,
                blend: placed.blend,
                transform: placed.transform,
            });
        }
    };
    let [width, height] = [box_size[0] * scale, box_size[1] * scale];
    let Some(tile) = tile else {
        paths([x, y], [width, height], out);
        return;
    };
    if !(tile.width > 0.0 && tile.height > 0.0) {
        return;
    }
    // Copies of one tile across the box, from wherever the pattern
    // starts, and nothing outside the box: the same picture a tiled
    // image gives, drawn as paths.
    let first = |offset: f64, span: f64| (-offset / span).floor();
    let count = |offset: f64, span: f64, across: f64| {
        (((across - offset) / span).ceil() - first(offset, span)).max(0.0)
    };
    let (cols, rows) = (
        count(tile.offset[0], tile.width, width),
        count(tile.offset[1], tile.height, height),
    );
    let clip = ResolvedShape::Rect {
        x,
        y,
        width,
        height,
    };
    let marker = |shape: ResolvedShape| ResolvedLayer {
        gradient: None,
        overflow: placed.overflow,
        name: placed.name.to_owned(),
        layer: placed.layer.clone(),
        shape,
        color: [0; 4],
        opacity: placed.opacity,
        blend: Blend::Normal,
        transform: placed.transform,
    };
    out.push(marker(ResolvedShape::ClipBegin {
        shape: Box::new(clip),
    }));
    let mut drawn = 0;
    for row in 0..rows as i64 {
        for col in 0..cols as i64 {
            drawn += 1;
            if drawn > MAX_TILES {
                break;
            }
            let at = [
                x + tile.offset[0] + (first(tile.offset[0], tile.width) + col as f64) * tile.width,
                y + tile.offset[1]
                    + (first(tile.offset[1], tile.height) + row as f64) * tile.height,
            ];
            paths(at, [tile.width, tile.height], out);
        }
    }
    out.push(marker(ResolvedShape::ClipEnd));
}

/// Where a piece of a layer lands, shared by everything the walk adds.
#[derive(Debug, Clone, Copy)]
struct Placed<'a> {
    name: &'a str,
    layer: &'a LayerPath,
    /// Top-left corner on the canvas.
    origin: [f64; 2],
    scale: f64,
    opacity: f64,
    blend: Blend,
    /// Whether the tree this sits in may draw past the canvas.
    overflow: bool,
    transform: Transform,
}

/// The pixel rectangle `[x, y, width, height]` of sheet cell `frame`
/// (rounded down, clamped to the cells that fit the image).
fn sheet_cell(sheet: Sheet, image_width: u32, image_height: u32, frame: f64) -> [u32; 4] {
    let [cw, ch] = sheet.cell.map(|c| c.max(1));
    let columns = sheet.columns.clamp(1, (image_width / cw).max(1));
    let rows = (image_height / ch).max(1);
    let last = columns * rows - 1;
    let index = if frame.is_finite() && frame > 0.0 {
        (frame.floor() as u32).min(last)
    } else {
        0
    };
    [index % columns * cw, index / columns * ch, cw, ch]
}

/// Font styles held to the fonts they name: an outline font needs a
/// size, a bitmap font has one of its own. Each style's problem, if it
/// has one, in name order.
fn font_style_problems<'a>(
    show: &'a Show,
    fonts: &BTreeMap<String, Arc<RegisteredFont>>,
    outline_fonts: &BTreeMap<String, FontData>,
) -> Vec<(&'a str, &'static str)> {
    let mut out = Vec::new();
    for (name, style) in &show.fonts {
        let problem = if outline_fonts.contains_key(&style.file) {
            match style.size {
                Some(size) if size > 0.0 => None,
                Some(_) => Some("needs a size above 0"),
                None => Some("uses an outline font and needs a size"),
            }
        } else if fonts.contains_key(&style.file) && style.size.is_some() {
            Some("uses a bitmap font, which has one fixed size: remove size")
        } else if fonts.contains_key(&style.file) && style.pixels == Some(true) {
            Some("uses a bitmap font, which is pixels already: remove pixels")
        } else {
            None
        };
        if let Some(problem) = problem {
            out.push((name.as_str(), problem));
        }
    }
    out
}

/// Warn about artwork asking for something its kind does not have.
///
/// A sheet is a grid of pixels, and vector artwork has none: a `sheet`
/// or a `frame` on a layer drawn from paths would quietly do nothing.
/// Only artwork already registered can be told apart, which is the
/// common case at load; one registered later simply ignores them.
fn quiet_artwork(show: &Show, vectors: &BTreeMap<String, Vector>, out: &mut Vec<String>) {
    fn walk(layers: &[Layer], vectors: &BTreeMap<String, Vector>, out: &mut Vec<String>) {
        for layer in layers {
            if let LayerKind::Image {
                image,
                sheet,
                frame,
                parts,
                ..
            } = &layer.kind
            {
                if (sheet.is_some() || *frame != 0.0) && vectors.contains_key(image) {
                    out.push(format!(
                        "layer {:?} draws vector artwork {image:?} as a sheet of cells, \
                         which only pixels have",
                        layer.name
                    ));
                }
                // Only artwork already registered can say which ids it
                // has; one registered later is taken at its word.
                if let Some(art) = vectors.get(image) {
                    for part in parts {
                        let LayerKind::Part { id, .. } = &part.kind else {
                            continue;
                        };
                        if !art.paths.iter().any(|p| p.ids.iter().any(|n| n == id)) {
                            out.push(format!(
                                "layer {:?} names a part {id:?}, which artwork {image:?} \
                                 has no element of; the part moves nothing",
                                layer.name
                            ));
                        }
                    }
                }
            }
            walk(layer.children(), vectors, out);
        }
    }
    walk(&show.layers, vectors, out);
    for scene in &show.scenes {
        walk(&scene.layers, vectors, out);
    }
}

/// The outline of a rect, with rounded corners when it has a radius.
///
/// Quarter circles as cubics, the same approximation SVG arcs get, so a
/// rounded rect strokes and clips like any other path.
fn rect_path(rect: [f64; 4], radius: Option<f64>) -> Vec<PathElement> {
    let [x, y, w, h] = rect;
    let r = Shape::corner_radius(rect, radius);
    if r <= 0.0 {
        return vec![
            PathElement::MoveTo([x, y]),
            PathElement::LineTo([x + w, y]),
            PathElement::LineTo([x + w, y + h]),
            PathElement::LineTo([x, y + h]),
            PathElement::Close,
        ];
    }
    // How far a cubic's control point sits along the tangent to meet a
    // quarter circle: the usual 4/3 * (sqrt(2) - 1).
    let k = r * 0.552_284_749_830_793_4;
    let (r1, b) = (x + w, y + h);
    vec![
        PathElement::MoveTo([x + r, y]),
        PathElement::LineTo([r1 - r, y]),
        PathElement::CubicTo([r1 - r + k, y], [r1, y + r - k], [r1, y + r]),
        PathElement::LineTo([r1, b - r]),
        PathElement::CubicTo([r1, b - r + k], [r1 - r + k, b], [r1 - r, b]),
        PathElement::LineTo([x + r, b]),
        PathElement::CubicTo([x + r - k, b], [x, b - r + k], [x, b - r]),
        PathElement::LineTo([x, y + r]),
        PathElement::CubicTo([x, y + r - k], [x + r - k, y], [x + r, y]),
        PathElement::Close,
    ]
}

/// Place a shape's local geometry: scaled uniformly around the layer's
/// x/y origin, then translated to it. A stroked rect or circle resolves
/// as a path, the one shape that carries a stroke.
fn resolve_shape(
    shape: &Shape,
    x: f64,
    y: f64,
    scale: f64,
    stroke: Option<([u8; 4], f64)>,
) -> ResolvedShape {
    let place = |[px, py]: [f64; 2]| [px * scale + x, py * scale + y];
    match (shape, stroke) {
        (
            Shape::Rect {
                rect: [rx, ry, w, h],
                radius,
            },
            None,
        ) if Shape::corner_radius([*rx, *ry, *w, *h], *radius) <= 0.0 => ResolvedShape::Rect {
            x: rx * scale + x,
            y: ry * scale + y,
            width: w * scale,
            height: h * scale,
        },
        (
            Shape::Circle {
                circle: [cx, cy, r],
            },
            None,
        ) => ResolvedShape::Circle {
            cx: cx * scale + x,
            cy: cy * scale + y,
            radius: r * scale,
        },
        (
            Shape::Rect {
                rect: [rx, ry, w, h],
                radius,
            },
            stroke,
        ) => ResolvedShape::Path {
            elements: rect_path([*rx, *ry, *w, *h], *radius)
                .into_iter()
                .map(|e| e.map(place))
                .collect(),
            stroke,
        },
        (
            Shape::Circle {
                circle: [cx, cy, r],
            },
            stroke,
        ) => {
            // Four cubic quarter arcs, the usual approximation.
            const K: f64 = 0.552_284_749_8;
            let (cx, cy, r) = (*cx, *cy, *r);
            let k = K * r;
            let elements = [
                PathElement::MoveTo([cx + r, cy]),
                PathElement::CubicTo([cx + r, cy + k], [cx + k, cy + r], [cx, cy + r]),
                PathElement::CubicTo([cx - k, cy + r], [cx - r, cy + k], [cx - r, cy]),
                PathElement::CubicTo([cx - r, cy - k], [cx - k, cy - r], [cx, cy - r]),
                PathElement::CubicTo([cx + k, cy - r], [cx + r, cy - k], [cx + r, cy]),
                PathElement::Close,
            ];
            ResolvedShape::Path {
                elements: elements.into_iter().map(|e| e.map(place)).collect(),
                stroke,
            }
        }
        (Shape::Path { path: data }, stroke) => ResolvedShape::Path {
            elements: data.elements().iter().map(|e| e.map(place)).collect(),
            stroke,
        },
    }
}

/// One paintable item of the flattened show, in canvas coordinates.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ResolvedLayer {
    /// Where the layer is in the document, which tells two layers of one
    /// name apart and maps the item back to its place.
    pub layer: LayerPath,
    pub name: String,
    pub shape: ResolvedShape,
    /// Fill color as RGBA bytes; opaque white for images. With a
    /// `gradient` this is its first stop, so a host that draws no
    /// gradients still draws something sensible.
    pub color: [u8; 4],
    /// A gradient to fill the shape with instead of `color`, already in
    /// the same space as the shape's coordinates.
    pub gradient: Option<ResolvedGradient>,
    /// Effective opacity in [0, 1] (tree-multiplied).
    pub opacity: f64,
    /// How the item combines with what was painted before it. Markers
    /// (clips, blend groups) carry `Normal`.
    pub blend: Blend,
    /// Whether this item may draw past the canvas into the letterbox; see
    /// [`Layer::overflow`](cuelight_core::Layer::overflow). A host that fits the
    /// canvas into a larger surface leaves these unclipped.
    pub overflow: bool,
    /// Applied to the shape's coordinates to place it on the canvas. The
    /// identity for anything only translated and uniformly scaled, which is
    /// then already in canvas coordinates; a rotation or an uneven scale
    /// anywhere up the tree leaves the shape in the layer's own space and
    /// puts the whole placement here. Hosts drawing the list themselves
    /// apply it (a clip's shape included).
    pub transform: Transform,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ResolvedShape {
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    Circle {
        cx: f64,
        cy: f64,
        radius: f64,
    },
    /// Start clipping: until the matching [`ResolvedShape::ClipEnd`],
    /// items only show inside `shape` (a rect or circle). Clips nest.
    ClipBegin {
        shape: Box<ResolvedShape>,
    },
    /// End the innermost clip.
    ClipEnd,
    /// Start a group that is composited as one picture with `blend`, until
    /// the matching [`ResolvedShape::BlendEnd`]. Nests with clips: a
    /// clipped blended group opens the blend first.
    BlendBegin {
        blend: Blend,
    },
    /// End the innermost blend group.
    BlendEnd,
    /// A filled polygon (a segment of a segment display), closed
    /// implicitly.
    Polygon {
        points: Vec<[f64; 2]>,
    },
    /// A path of lines and curves in canvas coordinates, filled with the
    /// layer's color (a fully transparent color means no fill) and then
    /// outlined with `stroke` (color, width in canvas pixels) when given.
    /// As a clip, the stroke is ignored.
    Path {
        elements: Vec<PathElement>,
        stroke: Option<([u8; 4], f64)>,
    },
    /// A host image (look the pixels up via [`Engine::image`]) drawn into
    /// the destination rectangle: the whole image, or with `source` only
    /// that pixel rectangle `[x, y, width, height]` of it (a sheet cell).
    Image {
        image: String,
        source: Option<[u32; 4]>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        /// Repeat one tile across the box instead of stretching the image
        /// to fill it.
        tile: Option<Tiled>,
    },
    /// Text in an outline font: fill the outlines of `glyphs` from `font`
    /// at `size` pixels per em with the layer's color, after drawing
    /// `border` (color, width in pixels) around them when given. Hosts
    /// drawing the list themselves need a font rasterizer for this; they
    /// may skip it, bitmap fonts being the portable choice.
    GlyphRun {
        font: FontData,
        size: f64,
        glyphs: Vec<PlacedGlyph>,
        border: Option<([u8; 4], f64)>,
    },
    /// Pixels the engine generated (rasterized text), drawn into the
    /// destination rectangle. `image.revision()` identifies the content.
    Bitmap {
        image: ImageData,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
}

/// A gradient with its colors parsed and its geometry scaled, ready to
/// draw: the shape's own space, like the shape's coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGradient {
    pub kind: ResolvedGradientKind,
    /// `(position, RGBA)`, in order, at least one.
    pub stops: Vec<(f32, [u8; 4])>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolvedGradientKind {
    Linear { from: [f64; 2], to: [f64; 2] },
    Radial { center: [f64; 2], radius: f64 },
}

/// Parse a gradient's colors and scale its geometry the way a shape's
/// coordinates are scaled.
fn resolve_gradient(
    gradient: &Gradient,
    x: f64,
    y: f64,
    scale: f64,
) -> Result<ResolvedGradient, Error> {
    let stops = gradient
        .stops()
        .iter()
        .map(|stop| {
            parse_color(&stop.color)
                .map(|rgba| (stop.at as f32, rgba))
                .ok_or_else(|| Error::InvalidColor(stop.color.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let place = |[px, py]: [f64; 2]| [px * scale + x, py * scale + y];
    let kind = match gradient {
        Gradient::Linear { from, to, .. } => ResolvedGradientKind::Linear {
            from: place(*from),
            to: place(*to),
        },
        Gradient::Radial { center, radius, .. } => ResolvedGradientKind::Radial {
            center: place(*center),
            radius: radius * scale,
        },
    };
    Ok(ResolvedGradient { kind, stops })
}

/// How a tiled image covers its box: one tile's size and where the
/// pattern starts, both in the same units as the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tiled {
    pub width: f64,
    pub height: f64,
    pub offset: [f64; 2],
}
