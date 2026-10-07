//! Layers: what each kind draws, and what every layer has in common.

use super::*;

/// How an image's pixels are read when it is drawn at another size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Sampling {
    /// Filtered: smooth at any size.
    #[default]
    Smooth,
    /// Each source pixel a block with hard edges, at any scale and
    /// rotation: pixel art, a pattern from a tiny tile, a mosaic. Its
    /// edges are still smoothed where they are not upright or level.
    Nearest,
}

/// An element of vector artwork the show moves on its own, as it is
/// written in a document: the `parts` of an artwork layer.
///
/// A character drawn as one SVG keeps its moving pieces inside it; a
/// part names one by the `id` the SVG gives it and takes the transform
/// properties, `opacity` and `visible` a layer has, with timelines and
/// bindings, applied in the artwork's own coordinates on top of what
/// the SVG says. Parts nest the way the SVG's groups do: turning the
/// element `head` carries `jaw` and `eye` inside it. Loaded, a part is
/// a [`Layer`] of the [`Part`](LayerKind::Part) kind, a child of its
/// artwork layer, named after its id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Part {
    /// The element's `id` in the SVG.
    pub id: String,
    /// The point it turns and scales around, in the artwork's
    /// coordinates; the centre of its bounds when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<[f64; 2]>,
    /// Moved by this much, in the artwork's units.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default = "default_scale")]
    pub scale_x: f64,
    #[serde(default = "default_scale")]
    pub scale_y: f64,
    /// Degrees, clockwise, around the pivot.
    #[serde(default)]
    pub rotation: f64,
    #[serde(default = "default_visible")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timelines: Vec<Timeline>,
}

impl From<Part> for Layer {
    fn from(part: Part) -> Self {
        Layer {
            name: part.id.clone(),
            kind: LayerKind::Part {
                id: part.id,
                pivot: part.pivot,
            },
            x: part.x,
            y: part.y,
            opacity: part.opacity,
            scale: part.scale,
            scale_x: part.scale_x,
            scale_y: part.scale_y,
            rotation: part.rotation,
            anchor: None,
            visible: part.visible,
            overflow: false,
            blend: Blend::default(),
            press: None,
            bindings: part.bindings,
            timelines: part.timelines,
        }
    }
}

impl Layer {
    /// This layer as a [`Part`], when it is one.
    pub fn as_part(&self) -> Option<Part> {
        let LayerKind::Part { id, pivot } = &self.kind else {
            return None;
        };
        Some(Part {
            id: id.clone(),
            pivot: *pivot,
            x: self.x,
            y: self.y,
            opacity: self.opacity,
            scale: self.scale,
            scale_x: self.scale_x,
            scale_y: self.scale_y,
            rotation: self.rotation,
            visible: self.visible,
            bindings: self.bindings.clone(),
            timelines: self.timelines.clone(),
        })
    }
}

/// The `parts` of an artwork layer, read as [`Part`]s and held as layers.
fn parts_in<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Layer>, D::Error> {
    let parts: Vec<Part> = Vec::deserialize(deserializer)?;
    Ok(parts.into_iter().map(Layer::from).collect())
}

/// The `parts` of an artwork layer, written back as [`Part`]s.
fn parts_out<S: serde::Serializer>(parts: &[Layer], serializer: S) -> Result<S::Ok, S::Error> {
    let parts: Vec<Part> = parts.iter().filter_map(Layer::as_part).collect();
    parts.serialize(serializer)
}

/// One node in the show tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Layer {
    pub name: String,
    #[serde(flatten)]
    pub kind: LayerKind,
    /// Translation applied to this layer (and its subtree, for groups),
    /// in canvas coordinates (origin top-left, y down).
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    /// Opacity in [0, 1], multiplied down the tree.
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    /// Uniform scale of the layer around its x/y origin (shape coordinates
    /// and sizes, image destination size), or around its anchor point.
    /// A group's scale applies to its whole subtree.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Scale along x and y on top of `scale`, for stretching and flips
    /// (negative values mirror). Inherited like `scale`.
    #[serde(default = "default_scale")]
    pub scale_x: f64,
    #[serde(default = "default_scale")]
    pub scale_y: f64,
    /// Rotation in degrees, clockwise on the canvas, around the layer's
    /// x/y origin or its anchor point. A group turns its whole subtree.
    #[serde(default)]
    pub rotation: f64,
    /// Which point of the layer's content box sits at its x/y. Without an
    /// anchor, images put their top-left corner there and shapes their
    /// local origin. Not supported on groups.
    #[serde(default)]
    pub anchor: Option<Align>,
    #[serde(default = "default_visible")]
    pub visible: bool,
    /// Let this layer (and a group's subtree) draw past the canvas.
    ///
    /// A show is authored against a fixed canvas and a host fits that
    /// canvas into whatever surface it has, painting the area around it
    /// with the show's `background`. That is the right default: it keeps
    /// the canvas meaning exactly what it means for layout. It is the
    /// wrong answer for a backdrop whose job is to reach the edges, which
    /// on a wider display sits in two bars of flat colour with nothing
    /// the show can say about them.
    ///
    /// Marked here, only the clip changes: coordinates are still authored
    /// against the canvas, which stays a safe area for everything placed
    /// exactly. A show cannot know how far it will be asked to stretch,
    /// so anything that bleeds has to be drawn generously.
    ///
    /// It does nothing where the frame *is* the canvas: an offscreen
    /// render, `pixel_perfect` scaling, a `dots` pass, or an output mode
    /// other than `rgb`, all of which draw the canvas at its own size
    /// first. There is no area outside a dot matrix to reach into.
    #[serde(default)]
    pub overflow: bool,
    /// How the layer combines with what is painted beneath it; a group
    /// blends its children as one picture.
    #[serde(default)]
    pub blend: Blend,
    /// What a press on this layer fires, when it lands on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<Press>,
    /// Live property bindings: `property = variable * scale + offset`.
    #[serde(default)]
    pub bindings: Vec<Binding>,
    /// Keyframed animations on this layer's properties.
    #[serde(default)]
    pub timelines: Vec<Timeline>,
}

fn default_opacity() -> f64 {
    1.0
}

/// What a press on a layer does.
///
/// What pressing a layer does: fire a trigger, open a web address, or
/// both. The engine's inputs stay one-way: a press is the same thing a
/// host firing that trigger would be, and a show is still a function
/// of its triggers and its clock; opening an address is reported to the
/// host as an event, and the host decides what to do with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Press {
    /// Trigger fired when the layer is pressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// A web address opened when the layer is pressed: `http` or
    /// `https` only, so a show cannot point a kiosk at a local file or a
    /// custom scheme. Reported as [`Event::Open`](crate::Event); the
    /// players open it in the browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
}

/// How a layer's colors combine with the colors already beneath it.
/// Opacity applies on top of the blend, as usual.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Blend {
    /// Paint over: the layer covers what is beneath.
    #[default]
    Normal,
    /// Sum the colors: light that adds to the picture, as a lamp behind
    /// art does. Overlapping glows brighten each other; white saturates.
    Add,
    /// `1 - (1 - a)(1 - b)`: light that adds but never saturates, softer
    /// than `add`.
    Screen,
    /// Product of the colors: a coloured shape darkens and tints what is
    /// beneath, as a gel over a lamp does; white leaves it alone.
    Multiply,
}

pub(super) fn is_zero(n: &f64) -> bool {
    *n == 0.0
}

pub(super) fn is_smooth(sampling: &Sampling) -> bool {
    *sampling == Sampling::Smooth
}

fn default_visible() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum LayerKind {
    /// A container: children are positioned relative to the group and
    /// painted in order. With `clip`, children only show inside that
    /// shape, given in the group's local space like a shape layer's.
    Group {
        children: Vec<Layer>,
        #[serde(default)]
        clip: Option<Shape>,
        /// Loudness of the sounds in the subtree, multiplied down the tree
        /// like opacity.
        #[serde(default = "default_scale")]
        gain: f64,
    },
    /// A vector shape filled with `fill`, and outlined by `stroke` when
    /// given. The fill is a color, or a gradient that stays smooth at any
    /// size where a show would otherwise carry a small image of one.
    Shape {
        shape: Shape,
        fill: Fill,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<Stroke>,
    },
    /// Artwork the host registered under `image` with the `cuelight`
    /// crate: pixels through its `set_image`, or vector artwork through
    /// its `set_vector`, which the loader does for every `assets/*.svg`.
    /// Drawn with its top-left corner at the layer's x/y unless the layer
    /// has an `anchor`; what is not registered yet is skipped.
    ///
    /// One layer kind for both, since the asset says how to draw itself
    /// and everything else about a layer of artwork is the same. `type`
    /// and the name may still be written as `vector`, which older shows
    /// do; `sheet` and `frame` are for pixels, and say so at load on
    /// vector artwork.
    #[serde(alias = "vector")]
    Image {
        #[serde(alias = "vector")]
        image: String,
        /// Destination size `[width, height]`; the image's natural size
        /// (one cell's size with a `sheet`) when omitted.
        #[serde(default)]
        size: Option<[f64; 2]>,
        /// Treat the image as a grid of equally sized cells and draw one:
        /// the one the `frame` property selects. Pixels only.
        #[serde(default)]
        sheet: Option<Sheet>,
        /// Base cell index for sheets (row-major, 0 is the top-left cell).
        #[serde(default)]
        frame: f64,
        /// Color the artwork is multiplied by, `#RRGGBB` or `#RRGGBBAA`:
        /// white leaves it alone, a color stains it (a lamp behind white
        /// art, a worn look, one sprite or icon in several colors).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tint: Option<String>,
        /// Tile the artwork across `size` instead of stretching to it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat: Option<Tile>,
        /// How the pixels are read when drawn at another size: `smooth`
        /// (default) or `nearest`. The layer's own choice, whatever the
        /// show's `scaling`. Nothing to vector artwork.
        #[serde(default, skip_serializing_if = "is_smooth")]
        sampling: Sampling,
        /// Elements of vector artwork the show moves on their own, each
        /// named by the `id` the SVG gives it; see [`Part`]. Held as
        /// layers of the [`Part`](LayerKind::Part) kind, so a part is
        /// animated, bound, traced and addressed like any layer, as the
        /// children of this one.
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "parts_in",
            serialize_with = "parts_out"
        )]
        #[cfg_attr(feature = "schema", schemars(with = "Vec<Part>"))]
        parts: Vec<Layer>,
    },
    /// One element of the vector artwork of the layer above it, moved on
    /// its own. Only ever a child of an artwork layer, made from its
    /// `parts`; never written as a layer of its own. Its transform
    /// properties, `opacity` and `visible` apply in the artwork's own
    /// coordinates, on top of what the SVG says, around `pivot`.
    Part {
        /// The element's `id` in the SVG. Every path under that element
        /// moves with it, and a part inside another part moves with
        /// both, as the SVG's groups nest.
        id: String,
        /// The point the part turns and scales around, in the artwork's
        /// coordinates; the centre of its bounds when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pivot: Option<[f64; 2]>,
    },
    /// Text in a bitmap font style from the show's `fonts`. With `size` the
    /// text is aligned inside that box (its top-left corner at the layer's
    /// x/y); without, the box is the text's own size. Multi-line text
    /// (`\n`) aligns each line on its own. Skipped while the style's font
    /// is not registered.
    Text {
        text: String,
        font: String,
        #[serde(default)]
        size: Option<[f64; 2]>,
        #[serde(default)]
        align: Align,
        /// How much of the text shows, as a share of its characters from
        /// 0 to 1: the first `reveal` of them are drawn, the rest keep
        /// their room and are not, so a line neither reflows nor
        /// re-centres as it types. A numeric property like `opacity`,
        /// keyframed for a typewriter, bound for a dialogue box a host
        /// advances. Every character counts, spaces and line breaks
        /// too, so a pause can be written into the text.
        #[serde(default = "default_scale")]
        reveal: f64,
    },
    /// A row of `digits` equal cells across `size` `[width, height]`
    /// (top-left at the layer's x/y) showing `text`, one character per
    /// cell; how a cell is drawn is up to `display`. Text longer than the
    /// row is cut at the far side of `justify`.
    Digits {
        digits: u32,
        size: [f64; 2],
        #[serde(default)]
        text: String,
        #[serde(default)]
        justify: Justify,
        display: DigitDisplay,
        /// How much of the text shows, as on a text layer: the cells of
        /// the characters past it stay dark or empty.
        #[serde(default = "default_scale")]
        reveal: f64,
    },
    /// A video the host registered under `video` with its duration and
    /// size ([`Engine::set_video`](crate::Engine::set_video)), played the
    /// way a sound is: on `trigger` or at load with `autoplay`, after
    /// `delay`, looping or repeating, firing `on_end`. The engine decodes
    /// nothing: it reports what should be showing and at which position
    /// ([`Engine::videos`](crate::Engine::videos)), and draws whatever
    /// frame the host last registered as an image under the video's name.
    Video {
        video: Choice,
        /// Which of several videos a play shows; ignored for one.
        #[serde(default)]
        pick: Pick,
        /// Drawn size `[width, height]`; the video's own when omitted.
        #[serde(default)]
        size: Option<[f64; 2]>,
        /// Trigger name, or list of names, that plays it.
        #[serde(default)]
        trigger: Triggers,
        /// Play when the show loads or the scene is entered.
        #[serde(default)]
        autoplay: bool,
        /// Repeat forever. Cannot be combined with `repeat`.
        #[serde(default, rename = "loop")]
        looping: bool,
        /// Seconds between the trigger and the first frame.
        #[serde(default)]
        delay: f64,
        /// Number of plays (fractions allowed); once when omitted.
        #[serde(default)]
        repeat: Option<f64>,
        /// Trigger fired when a play finishes (never for loops).
        #[serde(default)]
        on_end: Option<String>,
        /// Trigger name, or list of names, that stops it.
        #[serde(default)]
        stop: Triggers,
        /// A variable condition that plays it on the rising edge, as a
        /// timeline's `when` does; see [`Timeline::when`]. Beside
        /// `trigger`: either starts a play.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<When>,
        /// A variable condition it plays under, as a timeline's `while`:
        /// it starts when the condition turns true and stops, without
        /// `on_end`, when it turns false. A play that ends on its own
        /// is not started again until the condition turns true again,
        /// so a one-shot sounds once per turn and a loop sounds
        /// throughout. See [`Timeline::whilst`].
        #[serde(default, rename = "while", skip_serializing_if = "Option::is_none")]
        whilst: Option<While>,
        /// Least seconds between one play starting and the next; a
        /// trigger that comes sooner is dropped. 0 (default) never drops.
        #[serde(default)]
        rest: f64,
        /// What the trigger does while a clip is already showing:
        /// `restart` (default), `ignore` or `queue`. Not `overlap`: a
        /// layer shows one picture at a time.
        #[serde(default)]
        retrigger: Retrigger,
        /// How many plays may wait their turn with `queue`. Default 4.
        #[serde(default = "default_voices")]
        voices: u32,
        /// Loudness of the clip's own soundtrack, if the host registered
        /// one: 0 to 1 and above, times the gains of the groups above it.
        /// 0 plays the picture silently. A normal numeric property:
        /// bindable and animatable.
        #[serde(default = "default_scale")]
        gain: f64,
        /// Name of the bus the clip's sound plays through; hosts route
        /// buses to outputs. [`MAIN_BUS`] when omitted, so every sound is
        /// on one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<String>,
        /// Step back while something else is sounding; see [`Duck`]. A
        /// clip with a soundtrack wants this as much as a sound does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duck: Option<Duck>,
    },
    /// A sound the host registered under `sound` with its duration
    /// ([`Engine::set_sound`](crate::Engine::set_sound)), played the way a
    /// timeline is: on `trigger` or at load with `autoplay`, after `delay`,
    /// looping or repeating, firing `on_end`. Draws nothing. What plays is
    /// reported by [`Engine::voices`](crate::Engine::voices); the engine
    /// never touches samples.
    Audio {
        sound: Choice,
        /// Which of several sounds a play uses; ignored for one.
        #[serde(default)]
        pick: Pick,
        /// Trigger name, or list of names, that plays it.
        #[serde(default)]
        trigger: Triggers,
        /// Play when the show loads or the scene is entered.
        #[serde(default)]
        autoplay: bool,
        /// Repeat forever. Cannot be combined with `repeat`.
        #[serde(default, rename = "loop")]
        looping: bool,
        /// Seconds between the trigger and the first sample.
        #[serde(default)]
        delay: f64,
        /// Number of plays (fractions allowed); once when omitted.
        #[serde(default)]
        repeat: Option<f64>,
        /// Trigger fired when a play finishes (never for loops).
        #[serde(default)]
        on_end: Option<String>,
        /// Trigger name, or list of names, that stops it.
        #[serde(default)]
        stop: Triggers,
        /// A variable condition that plays it on the rising edge, as a
        /// timeline's `when` does; see [`Timeline::when`]. Beside
        /// `trigger`: either starts a play.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<When>,
        /// A variable condition it plays under, as a timeline's `while`:
        /// it starts when the condition turns true and stops, without
        /// `on_end`, when it turns false. A play that ends on its own
        /// is not started again until the condition turns true again,
        /// so a one-shot sounds once per turn and a loop sounds
        /// throughout. See [`Timeline::whilst`].
        #[serde(default, rename = "while", skip_serializing_if = "Option::is_none")]
        whilst: Option<While>,
        /// Least seconds between one play starting and the next; a
        /// trigger that comes sooner is dropped. 0 (default) never drops.
        #[serde(default)]
        rest: f64,
        /// What the trigger does while the sound is already playing.
        #[serde(default)]
        retrigger: Retrigger,
        /// With `overlap`, how many plays may sound at once; the oldest
        /// stops beyond it. Default 4.
        #[serde(default = "default_voices")]
        voices: u32,
        /// Loudness, 0 to 1 and above, times the gains of the groups above
        /// it. A normal numeric property: bindable and animatable.
        #[serde(default = "default_scale")]
        gain: f64,
        /// Name of the bus the sound plays through; hosts route buses to
        /// outputs. [`MAIN_BUS`] when omitted, so every sound is on one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bus: Option<String>,
        /// Step back while something else is sounding; see [`Duck`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duck: Option<Duck>,
    },
}

/// A sprite sheet layout: cells of `cell` `[width, height]` pixels,
/// `columns` per row, numbered row by row from the top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Sheet {
    pub cell: [u32; 2],
    pub columns: u32,
}

/// Repeat a layer's content across its `size` instead of stretching it
/// to fit.
///
/// A pattern otherwise has to be written out: a checkerboard is a hundred
/// and twenty eight rectangles, and a reader cannot tell that from a
/// hundred and twenty eight unrelated ones. The same want turns up for a
/// grid, a scanline overlay, a floor, a wall of dots, any texture meant to
/// cover whatever it is put behind.
///
/// Tiling happens in the layer's own space, so a rotating or scaled group
/// carries the pattern with it rather than sliding underneath it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Tile {
    /// Size of one tile in the layer's units. The content's own size when
    /// omitted, which is what "repeat this at its natural size" means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    /// Where the pattern starts, in the layer's units. Animate it and the
    /// texture scrolls under a fixed window.
    #[serde(default)]
    pub offset: [f64; 2],
}

/// An animatable layer property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Property {
    X,
    Y,
    Opacity,
    Scale,
    ScaleX,
    ScaleY,
    Rotation,
    /// The text of a text layer; bindable, not animatable.
    Text,
    /// The font style of a text layer; bindable, not animatable.
    Font,
    /// The share of a text or digits layer's characters that show, 0 to
    /// 1; see [`revealed`].
    Reveal,
    /// The video a video layer plays; bindable, not animatable. Binding
    /// it lets one layer show whatever it is pointed at, rather than
    /// needing a layer per clip.
    Video,
    /// The sound an audio layer plays; bindable, not animatable, and the
    /// mirror of `video`. Binding it lets one layer sound whatever it is
    /// pointed at: a bed that follows the state a show is in, rather than
    /// a layer per track each having to stop the others.
    Sound,
    /// The artwork an image layer draws, by the name it is registered
    /// under; bindable, not animatable. Binding it lets one layer show
    /// whichever picture it is pointed at: the avatar of whoever is
    /// playing, rather than a layer per player.
    Image,
    /// Sprite sheet cell of an image layer: rounded down and clamped to
    /// the sheet, so a linear key from 0 to n steps through n cells.
    Frame,
    /// Where a tiled image's pattern starts, along x and y, in the
    /// layer's units. Animating one scrolls the texture under the layer.
    TileX,
    TileY,
    /// Loudness of an audio layer or a group's subtree.
    Gain,
    /// Whether the layer (and its subtree) shows and sounds; bindable,
    /// not animatable. Bound, it is on when the binding's number is not 0.
    Visible,
    /// The color an image layer is multiplied by, as `#RRGGBB` or
    /// `#RRGGBBAA`; bindable, not animatable. An empty value leaves the
    /// image alone.
    Tint,
}

impl Property {
    /// Whether the property holds a number; only those can be keyframed.
    pub fn is_numeric(self) -> bool {
        !matches!(
            self,
            Property::Text
                | Property::Font
                | Property::Visible
                | Property::Tint
                | Property::Video
                | Property::Sound
                | Property::Image
        )
    }
}

impl LayerKind {
    /// The playhead of this layer, when its content has a length: a sound
    /// or a video.
    pub fn media(&self) -> Option<Media<'_>> {
        match self {
            LayerKind::Audio {
                sound,
                pick,
                trigger,
                autoplay,
                looping,
                delay,
                repeat,
                on_end,
                stop,
                when,
                whilst,
                retrigger,
                voices,
                rest,
                ..
            } => Some(Media {
                kind: MediaKind::Sound,
                names: sound,
                pick: *pick,
                trigger,
                stop,
                when: when.as_ref(),
                whilst: whilst.as_ref(),
                autoplay: *autoplay,
                looping: *looping,
                delay: *delay,
                repeat: *repeat,
                on_end: on_end.as_deref(),
                retrigger: *retrigger,
                voices: *voices,
                rest: *rest,
            }),
            LayerKind::Video {
                video,
                pick,
                trigger,
                autoplay,
                looping,
                delay,
                repeat,
                on_end,
                stop,
                when,
                whilst,
                retrigger,
                voices,
                rest,
                ..
            } => Some(Media {
                kind: MediaKind::Video,
                names: video,
                pick: *pick,
                trigger,
                stop,
                when: when.as_ref(),
                whilst: whilst.as_ref(),
                autoplay: *autoplay,
                looping: *looping,
                delay: *delay,
                repeat: *repeat,
                on_end: on_end.as_deref(),
                retrigger: *retrigger,
                voices: *voices,
                rest: *rest,
            }),
            _ => None,
        }
    }
}

impl Layer {
    /// The layers nested in this one: a group's children, an artwork
    /// layer's parts, else none.
    pub fn children(&self) -> &[Layer] {
        match &self.kind {
            LayerKind::Group { children, .. } => children,
            LayerKind::Image { parts, .. } => parts,
            _ => &[],
        }
    }

    /// Whether the layers nested in this one are its artwork's parts
    /// rather than a group's children: what `parts` in the document
    /// holds, where `children` would hold layers.
    pub fn holds_parts(&self) -> bool {
        matches!(self.kind, LayerKind::Image { .. })
    }

    /// The property's value as authored on this layer, or `None` when
    /// this kind of layer does not have the property.
    pub fn base_value(&self, property: Property) -> Option<Value> {
        Some(match (property, &self.kind) {
            (Property::X, _) => Value::Number(self.x),
            (Property::Y, _) => Value::Number(self.y),
            (Property::Opacity, _) => Value::Number(self.opacity),
            (Property::Visible, _) => Value::Bool(self.visible),
            (Property::Scale, _) => Value::Number(self.scale),
            (Property::ScaleX, _) => Value::Number(self.scale_x),
            (Property::ScaleY, _) => Value::Number(self.scale_y),
            (Property::Rotation, _) => Value::Number(self.rotation),
            (Property::Text, LayerKind::Text { text, .. } | LayerKind::Digits { text, .. }) => {
                Value::Text(text.clone())
            }
            (Property::Font, LayerKind::Text { font, .. }) => Value::Text(font.clone()),
            (
                Property::Reveal,
                LayerKind::Text { reveal, .. } | LayerKind::Digits { reveal, .. },
            ) => Value::Number(*reveal),
            (Property::Video, LayerKind::Video { video, .. }) => {
                Value::Text(video.first().to_owned())
            }
            (Property::Sound, LayerKind::Audio { sound, .. }) => {
                Value::Text(sound.first().to_owned())
            }
            (Property::Image, LayerKind::Image { image, .. }) => Value::Text(image.clone()),
            (Property::Frame, LayerKind::Image { frame, .. }) => Value::Number(*frame),
            (
                Property::TileX,
                LayerKind::Image {
                    repeat: Some(tile), ..
                },
            ) => Value::Number(tile.offset[0]),
            (
                Property::TileY,
                LayerKind::Image {
                    repeat: Some(tile), ..
                },
            ) => Value::Number(tile.offset[1]),
            (Property::Tint, LayerKind::Image { tint, .. }) => {
                Value::Text(tint.clone().unwrap_or_default())
            }
            (
                Property::Gain,
                LayerKind::Group { gain, .. }
                | LayerKind::Audio { gain, .. }
                | LayerKind::Video { gain, .. },
            ) => Value::Number(*gain),
            (
                Property::Text
                | Property::Font
                | Property::Reveal
                | Property::Frame
                | Property::Gain
                | Property::Tint
                | Property::Video
                | Property::Sound
                | Property::Image
                | Property::TileX
                | Property::TileY,
                _,
            ) => return None,
        })
    }
}
