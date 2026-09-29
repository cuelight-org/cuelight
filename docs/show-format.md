# Show format

A cuelight show is a single JSON document describing everything the engine
can play: a layer tree, the variables the host may drive, and keyframed
timelines fired by triggers. The normative, machine-readable definition is
the generated JSON Schema at
[`crates/cuelight-core/schemas/show.schema.json`](../crates/cuelight-core/schemas/show.schema.json);
this page explains the concepts the schema cannot.

Point your editor at the schema for autocomplete and validation:

```json
{ "$schema": "../../../cuelight-core/schemas/show.schema.json", "name": "my_show", ... }
```

## Format version

```json
{ "format": 1, "name": "my_show", ... }
```

`format` is the version of the show format the document was written for;
a show without it is format 1. It goes up whenever a change could make an
older engine misread a show, and an engine refuses a show of a newer
format (`Error::UnsupportedFormat`) rather than play it wrongly. The
version this engine reads is the `FORMAT` constant.

Within a format, fields the engine does not know are ignored, so additions
stay compatible, but they are not silent: `Engine::load_warnings()` lists
their paths (`layers[2].colour`), which is how a typo shows up. Keys
starting with `$`, like `$schema`, are never reported. The player logs the
warnings at load.

The same list reports bindings that will quietly do nothing: one reading a
name the show declares as neither a variable nor a
[value](#values-the-show-animates), and one whose variable starts at a
value the property cannot use, such as a `tint` variable starting at
`"green"`. A value counts as declared, and since a value is always a
number, a `tint` or `font` binding on one is reported the same way. None
of those is an error, since a host may set something usable later, but
each looks exactly like a feature that does not work.

Loading otherwise stops at the first problem: a color that is not one, a
layer using a font style the show does not declare, a file the show names
that is not there. That is right for anything shipping and useless while a
show is being written, so there is a tolerant load beside the strict one.
`Engine::load_show_tolerant()` keeps what it can and returns a list of
findings, each with a path in the document (`scenes[1].layers[0]`) and what
was wrong there. Whatever cannot be understood is dropped rather than
guessed at: a layer that does not parse or that the strict load would
refuse, a font style with a color that is not one (and then the layers
that used it, each with a finding of its own), a background that is not a
color. What is loaded is exactly a show the strict load accepts. It fails
only when there is no document at all: JSON that does not parse, no
`size`, a newer format. The loader's `Options::lenient()` does the same
for a show folder, adding the files it could not use to the same list,
and `--lenient` asks for it on the player and on `cuelight-render`.

Naming note: a **show** is the whole loaded document; **scenes** are the
switchable views inside it (see [Scenes](#scenes)).

## On disk

A show exists in three forms:

- A **loose file**: one `.json` document, self-sufficient as long as the
  show needs no images. A driver script for it is conventionally named
  `<show>.test-driver.json` next to it.
- A **show folder**: the self-contained unit for anything with assets.
  The folder is the show; its contents follow fixed names:

  ```text
  myshow/
    show.json           the show document (required)
    test-driver.json    optional driver script, hosts may pick it up
    assets/             images (PNG) and vector artwork (SVG), registered
                        by filename stem
      fonts/            fonts, registered by filename stem: bitmap (.fnt
                        plus its page PNGs) or outline (.ttf, .otf)
      sounds/           sounds, registered by filename stem (.wav, .flac,
                        .ogg, .mp3)
  ```

- A **show beside media it does not own**: a `show.json` dropped into a
  collection that is already arranged the way whoever made it wanted.
  Instead of a stem, an asset is named by a path relative to the
  document:

  ```text
  mycollection/
    show.json
    Intro/ Amazonia/ Fonts/ overlays/      whatever was already there
  ```

  ```json
  { "type": "video", "video": "AmazoniaProgress/Amazonia14.mp4" }
  ```

  Nothing is copied and nothing is linked, which is the point: a few
  hundred clips are hundreds of megabytes to copy, and a symbolic link is
  not something an ordinary Windows user can make. Folders that reuse a
  filename stay apart without renaming, and the names in the document are
  the names on disk.

  A name counts as a path when it holds a `/` or ends in an asset
  extension; anything else is still a stem from `assets/`, so existing
  shows are untouched and the two can be mixed. Paths are relative,
  `/`-separated, and may not climb out of the show's folder: `..`, an
  absolute path and a drive letter are refused rather than sanitized,
  because a document that asks for something outside its folder is wrong
  about where it is. A named file that is not there is reported in
  `skipped` rather than failing the load.

  Everything under `assets/` is walked, at any depth. A clip in a
  subfolder is opened like any other, which is the natural layout for a
  large collection. A file in a folder the loader has no use for is
  reported in `skipped`, unless the document names it by path: the
  decision to ignore something belongs in the report, not in silence,
  or the layer using it fails later with a missing-asset error pointing
  at the name rather than at the file sitting right there.

  The document decides what is loaded, which is the other half of this:
  the conventional folders are read whether anything uses them or not,
  while a collection of hundreds of clips loads only what a layer names.

- A **packed show**: the folder as one file, `myshow.cuelight`, a plain
  zip with the folder's contents at its root (`show.json`, the driver,
  `assets/...`, and every file the document names by a path of its own,
  wherever beside the document that was). The `cuelight-pack` tool of `cuelight-loader` writes it;
  any zip tool opens it, and a zip that wraps the folder in a directory is
  accepted too. Media that is already compressed (PNG, Ogg, MP3, FLAC) is
  stored as is, the rest deflated. The player and the loader open it like
  the folder.

Any of these forms is loaded the same way: the engine only ever receives the single JSON document through
`load_show` plus `set_image`, `set_vector`, `set_font` and `set_sound`
calls; it does no file I/O itself. Resolving a folder (reading the manifest, decoding and
registering `assets/`, picking up the driver) is host-side convention,
implemented by the `cuelight-loader` crate (sounds are decoded by
`cuelight-audio`). A zipped folder is the natural future single-file
distribution form.

## Top level

```json
{
  "format": 1,
  "name": "minigolf",
  "size": [512, 128],
  "background": "#101018",
  "variables": { "score": 0 },
  "layers": []
}
```

- `size` is the logical canvas in pixels. All coordinates in the show are
  authored against this space; hosts scale the rendered output to whatever
  surface they have (the bundled examples letterbox it into the window).
- The origin is the **top-left** corner: x grows right, y grows down.
  Shapes place their geometry relative to the layer's x/y (a circle at
  `[0, 0, r]` is centered on the layer origin), while an image's
  **top-left corner** sits at the layer's x/y, unless the layer sets an
  `anchor` (see [Layers](#layers)).
- `background` and every `fill` are `#RRGGBB` or `#RRGGBBAA`.
- `variables` declares the host-drivable inputs and their initial values
  (numbers, booleans, or text; bindings read them as numbers, booleans as
  0/1).
- `layers` are always present; `scenes` (optional) are switchable views
  painted on top of them.
- `input` (optional) says what a key or a press on the show means; see
  [Input](#input).

## Input

A show can be played with, not only driven. What a key or a press means
is the show's to say, so a player, a browser and an embedder behave the
same and none of them needs to know what the show is about:

```json
{ "input": { "keys": { "ArrowRight": "next", "ArrowLeft": "prev", " ": "next" },
             "press": "next" } }
```

- `keys` maps a key to a trigger. Keys are named as a browser names them
  (`KeyboardEvent.key`): `ArrowRight`, `Enter`, `Escape`, `a`, and `" "`
  for the space bar. Names match exactly, so `A` is the shifted one.
- `press` is the trigger a press fires when it lands on nothing
  pressable, which is how "press anywhere to go on" is written.

A layer says it can be pressed, and what that fires:

```json
{ "name": "lever", "type": "image", "image": "lever", "press": { "trigger": "pull" } }
```

A press finds the topmost pressable layer drawn under the point. The
frame as drawn is what answers: a layer that is hidden, clipped away or
covered is not hit, and one that has moved is hit where it is now. What
counts as inside is the shape for a rect or a circle and the box it
fills for anything else, which is enough for a lever, a word or a
button, and predictable.

A press fires a trigger and nothing else. That keeps the show's inputs
one-way: a press is the same thing a host firing that trigger would be,
so a show stays a function of its triggers and its clock, and pressing
the same things at the same times gives the same show. It is also what
lets a host record presses and keys and replay them, which is how
scrubbing keeps working while a show is being played with
(`cuelight_loader::Live`, see [Host contract](#host-contract)).

Hosts: `Engine::key(name)` and `Engine::press([x, y])` fire, and hand
back the trigger they fired so a host can log or record it;
`Engine::pressed([x, y])` asks without firing, for a pointer cursor.
Presses are in canvas coordinates, which `render::canvas_at` works out
from a point on the surface.

## Scenes

```json
{
  "scenes": [
    { "name": "attract", "trigger": "attract", "layers": [] },
    { "name": "game", "trigger": "start", "layers": [] }
  ]
}
```

Exactly one scene is active at a time: the first one when the show loads,
afterwards the one whose `trigger` fired last. Only the show's own
`layers` and the active scene's layers render, and only their timelines
respond to triggers.

Entering a scene restarts it: the previous scene's running timelines stop,
the entered scene's `autoplay` timelines start at 0. Firing the trigger of
the scene that is already active restarts it the same way. The show's own
layers are not affected by scene changes. A trigger that enters a scene
also starts the timelines that declare the same trigger, in the show's
layers and in the newly entered scene.

## Output

```json
{ "output": { "mode": "gray4", "tint": "#FF5820" } }
```

`output` says how the finished frame's colors reach the display:

- `rgb` (default): full color, unchanged.
- `gray4` / `gray2`: each pixel's brightness (luma, see below) is
  quantized to 16 / 4 levels and multiplied by `tint` (`#RRGGBB`, white by
  default). This is how a monochrome DMD shows content: author in any
  color, it lands as shades of the display's color.

Luma is how bright a color looks, as one number. The eye is far more
sensitive to green than to red, and least to blue, so a plain average of
r, g and b would make pure blue look as bright as pure green. The weights
come from Rec. 709, the HDTV standard that also defines the sRGB primaries:

```text
luma = 0.2126 r + 0.7152 g + 0.0722 b
```

They are applied directly to the 8-bit channel values (no linearization),
so white is full brightness, pure green lands at about 72 %, pure red at
21 % and pure blue at 7 %: in `gray4`, levels 11, 3 and 1 of 15.

`scaling` tells hosts how to bring the frame up to their surface:
`smooth` (default: any factor, filtered) or `pixel_perfect` (whole-number
factors with nearest-neighbor sampling, so every canvas pixel becomes a
crisp square block, which is what DMD-resolution content wants).
`render::Presenter` honors it; hosts doing their own presentation read it
with `Engine::scaling()` and place the frame with `render::fit`.

How the canvas meets a surface of another shape is the host's to say, not
the show's: the canvas is the show's safe area, and a layer that should
reach the edges uses `overflow`. A host fits it inside by default, keeping
its shape, with bars where the shapes differ (`contain`); whoever owns the
screen may prefer to fill it, cutting the canvas edges on the long axis
(`cover`), or to stretch it, losing the show's shape (`fill`). The names
are the ones CSS's `object-fit` uses. Under `pixel_perfect`, `contain`
rounds the factor down and `cover` up, and `fill` keeps a whole factor per
axis. The player takes `--fit` and cycles the three on Tab, logging how
many canvas pixels a `cover` cuts off; the web player takes `player.fit`.
Offline renders are canvas-sized
and have no fit.

`passes` lists effects applied to the finished frame as it is shown, in
order. One exists so far, `dots`, the dot matrix look: every canvas pixel
becomes a separate dot on black.

```json
"output": { "mode": "gray4", "tint": "#FF5820",
            "passes": [ { "dots": { "size": 0.8, "unlit": "#1A0904", "glow": 0.3 } } ] }
```

- `size` (default 0.8): dot diameter as a share of the pixel pitch, above
  0 up to 1.
- `shape`: `round` (default) or `square` (an LED matrix).
- `unlit`: color of a dot that is off, as on a real panel; dots never get
  darker than this. None by default. It is not run through the output
  `mode`, so give it in the tint's hue.
- `glow` (default 0): how much lit dots bleed into the dark around them,
  0 to 1.

Passes work at the surface's resolution, so `render::Presenter` applies
them (the player, the web player) and the offscreen renderer, whose frames
are canvas size, does not. A dot needs room: below three surface pixels
per canvas pixel the frame is shown plain. Hosts doing their own
presentation read the list with `Engine::passes()`.

A scene may declare its own `output`; while it is active, the fields it
sets override the show's and the rest still come from the show (a scene's
`passes` replace the show's list; an empty list turns them off). The conversion happens on the whole frame after
compositing, so antialiased edges and opacity quantize like everything
else. Hosts read the active mode with `Engine::output()`; the offscreen
renderer applies it after readback. Hosts rendering on their own device
use `render::Presenter`, which fits the show into their surface, applies
the output mode on the GPU and honors `scaling` (`render::OutputPass` is
the conversion alone, for hosts that want to do the rest themselves).

## Layers

`layers` is a tree painted in order: earlier layers are behind later ones,
group children behind whatever follows the group. Every layer has:

- `name`: identifier, also surfaced in the resolved draw list.
- `type`: `group`, `shape`, `image`, `text`, `digits`, `audio`
  or `video` (see below).
- `x`, `y` (default 0): translation. Groups pass it down to their subtree.
- `opacity` (default 1): multiplied down the tree.
- `scale` (default 1): uniform scale of the layer around its x/y origin
  (or its anchor point). `scale_x`, `scale_y` (default 1) scale along one
  axis on top of it, for stretching and flips (negative mirrors).
- `rotation` (default 0): degrees, clockwise on the canvas, around the
  layer's x/y origin or its anchor point (so an image anchored at
  `center` spins in place). A needle is a shape with `rotation` bound to a
  value; a reel window is a group turned by a timeline.

  A group's translation, scale and rotation apply to its whole subtree,
  in that order (children turn and scale with the group). In the draw list
  anything only translated and uniformly scaled arrives in canvas
  coordinates; a rotation or an uneven scale anywhere above leaves the
  shape in its own space and places it with the item's `transform`, an
  affine hosts drawing the list themselves apply (see
  [Host contract](#host-contract)).
- `anchor` (optional): which point of the layer's content box sits at its
  x/y: `top_left`, `top`, `top_right`, `left`, `center`, `right`,
  `bottom_left`, `bottom`, `bottom_right`. The content box is an image's
  drawn rectangle, a shape's bounding box or a text layer's box (its
  `size`, or the measured text) or a digit row's `size`, after `scale`, so an image
  anchored at `center` stays centered while it scales (see the beacon
  show). Without an anchor, images put their top-left corner and shapes
  their local origin at x/y. Groups cannot be anchored.
- `visible` (default true): invisible layers (and their subtrees) resolve
  to nothing and are not heard. Bindable (on when the binding's number is
  not 0), not keyframed.
- `overflow` (default false): let the layer, and a group's whole subtree,
  draw past the canvas into the letterbox. A host fits the canvas into
  whatever surface it has and paints the area around it with the show's
  `background`, which is right for everything placed exactly and wrong for
  a backdrop whose job is to reach the edges: on a wider display it sits in
  two bars of flat colour with nothing the show can say about them. Only
  the clip changes; coordinates are still authored against the canvas,
  which stays a safe area. A show cannot know how far it will be asked to
  stretch, so anything that bleeds has to be drawn generously. It does
  nothing where the frame *is* the canvas: an offscreen render,
  `pixel_perfect` scaling, a `dots` pass, or an output mode other than
  `rgb` all draw the canvas at its own size first, and there is no area
  outside a dot matrix to reach into.
- `blend` (default `normal`): how the layer combines with what is painted
  beneath it. `add` sums the colors (light that adds to the art: a lamp
  behind a backglass, overlapping glows; white saturates), `screen` adds
  softly and never saturates, `multiply` darkens and tints (a coloured
  gel over white art). A group blends its children as one picture, so two
  overlapping glows inside an `add` group add once, not twice. Opacity
  applies on top.
- `bindings`, `timelines`: see below.

Layer kinds:

- `group`: `children` is a nested layer list. Optional `clip` is a shape
  (a `rect`, `circle` or `path` as a shape layer writes it, in the group's
  local space) outside which the children do not show: a window onto its
  content. `gain` (default 1)
  scales the loudness of the audio layers in the subtree, see
  [Sound](#sound).
- `shape`: `shape` is `{ "rect": [x, y, width, height] }`,
  `{ "circle": [cx, cy, radius] }` or `{ "path": "M 0 0 L 10 0 L 5 8 Z" }`
  in the layer's local space, plus a `fill` (`#00000000` for an outline
  only). `path` takes SVG path data: `M L H V C S Q T A Z`, absolute or
  relative; arcs become curves. Optional `stroke`
  `{ "color": "#RRGGBB", "width": 1 }` outlines the shape, centered on its
  edge, in the layer's units (so it scales with the layer).

  A rect takes an optional `radius`, rounding all four corners:

  ```json
  { "rect": [0, 0, 600, 568], "radius": 14 }
  ```

  It is clamped to half the shorter side, so a large radius gives a pill.
  A rounded rect strokes and clips like any other shape.

  `fill` is a color, or a gradient:

  ```json
  { "fill": { "linear": { "from": [0, 0], "to": [0, 52],
                          "stops": [{ "at": 0, "color": "#1B3A6B" },
                                    { "at": 1, "color": "#F2B25C" }] } } }
  { "fill": { "radial": { "center": [0, 0], "radius": 60,
                          "stops": [{ "at": 0, "color": "#FFFFFFFF" },
                                    { "at": 1, "color": "#FFFFFF00" }] } } }
  ```

  Any number of stops, alpha included, `at` a fraction from 0 to 1 and in
  order. The geometry is in the same local space as the shape, so a
  gradient travels with whatever moves or scales the layer. A linear one
  holds its end colors beyond either end of its line, a radial one holds
  its last color beyond `radius`. Glows, vignettes, the shading that makes
  a drum look round and the sheen on glass are all gradients, and carrying
  them as small raster images means the same workaround in every show and
  blurring whenever one is scaled up. A host drawing the resolved list
  itself gets the gradient beside the layer's `color`, which is its first
  stop, so one that draws no gradients still draws something.
- `image`: artwork the host registered under that name, pixels or vector.
  Pixels come from `Engine::set_image` (RGBA8, kept in memory); vector
  artwork from `Engine::set_vector`, which the loader does for every
  `assets/*.svg`, by stem. One layer kind draws both: the asset says how
  to draw itself, and where it goes, how big it is, what stains it and
  whether it tiles are the same question either way. Optional `size`
  `[width, height]` scales the artwork into the canvas; omitted, it draws
  at its natural size (an SVG's viewBox). Artwork is a host asset, not
  show content: a layer whose artwork is not (yet) registered is skipped,
  so hosts can stream assets in after `load_show`. Optional `tint`
  (`#RRGGBB` or `#RRGGBBAA`) multiplies its colors, leaving transparency
  alone: white changes nothing, a color stains the art (a coloured bulb
  behind white artwork, one sprite or icon reused in several colors, a
  worn look over a clean texture).

  An image drawn at less than half its pixels is resampled: the renderer
  keeps reduced copies of it, halved and averaged, and samples the one
  nearest the size it is drawn at, so a high-contrast edge lands between
  pixels instead of stepping from one to the next. The copies are built
  the first time a show shrinks that image that far, cost a third of its
  memory in total, and an image drawn at its own size, near it, or at an
  exact fraction of it is untouched. A show drawn on its own pixel grid
  keeps its pixels: there the colours are the palette, and averaging two
  neighbouring dots invents one that is not in it.

  `sheet` and `frame` are for pixels, since a sheet is a grid of them;
  on vector artwork they are reported at load and do nothing. Older
  shows write `"type": "vector"` with the name in a `vector` field, and
  still load: both spellings mean this layer.

  **Parts.** A character drawn as one SVG keeps its moving pieces inside
  it. `parts` names elements of the artwork by the `id` the SVG gives
  them and moves each on its own:

  ```json
  { "type": "image", "image": "wolf", "parts": [
      { "id": "jaw", "pivot": [118, 70],
        "timelines": [{ "name": "teeth", "trigger": "tap_teeth", "tracks": [
          { "property": "rotation", "keys": [{ "t": 0, "v": 0 }, { "t": 0.25, "v": -20, "ease": "back_out" }] }] }] },
      { "id": "tail", "pivot": [214, 44], "bindings": [{ "property": "rotation", "variable": "wag" }] },
      { "id": "eye", "timelines": [ ... ] } ] }
  ```

  A part takes the transform properties (`x`, `y`, `rotation`, `scale`,
  `scale_x`, `scale_y`), `opacity` and `visible`, with timelines and
  bindings as a layer has them, applied in the artwork's own coordinates
  on top of what the SVG already says. `pivot` is the point it turns and
  scales around, in artwork coordinates, since a part has no box an
  `anchor` could name; left out, it is the centre of the element's bounds.
  Parts nest the way the SVG's groups do: turning `head` carries `jaw` and
  `eye` inside it, whether or not they are parts themselves. Loaded, a
  part is a layer of its own under the artwork layer, named after its id,
  so it is animated, bound, traced and addressed like any layer. A part
  naming an id the artwork has no element of is reported at load and
  moves nothing, since artwork is a host asset that may still arrive. The
  SVG's own animation is not read: the show is the one source of motion.

  What an SVG keeps: paths, basic shapes and text (as outlines), with
  solid fills and strokes, group transforms and opacities. A gradient
  paints as its first stop's color; patterns, raster images, clip paths,
  masks, filters, dashes and line joins are dropped. Animation is not
  read: address moving parts as separate layers and animate those.

  Text is drawn with the show's own fonts, the ones in `assets/fonts`,
  and never with the machine's: a show that took whatever happened to be
  installed would draw its text on one machine and drop it on another,
  and a browser has none to take. `font-family` names the family inside
  the font file, not the name the file is registered under:

  ```xml
  <text font-family="DM Mono" font-size="18"><tspan fill="#9DB8FF">"fuel"</tspan>: 0.01</text>
  ```

  draws in `assets/fonts/DMMono-Regular.ttf` wherever the show plays, and
  gives artwork several colours and weights in one line of text, which a
  text layer cannot do. A family the show does not ship is reported at
  load and its text is not drawn. Text that names no family at all gets
  the first font the show registered.

  A family's weights and styles are picked with `font-weight` and
  `font-style` under the one family name: the medium cut is
  `font-family="DM Mono" font-weight="500"`. The name some font menus
  show for it, `DM Mono Medium`, is not a family, and is reported as one
  the show does not ship.

  `size` stretches the artwork to fill the box. `repeat` tiles it across
  the box instead, which is what a pattern wants: a checkerboard, a grid, a
  scanline overlay, a floor, any texture meant to cover whatever it is put
  behind. Written out, those are hundreds of rectangles that a reader
  cannot tell from hundreds of unrelated ones.

  ```json
  { "type": "image", "image": "checker", "size": [800, 600],
    "repeat": { "size": [64, 64], "offset": [0, 0] } }
  ```

  A tile's `size` defaults to the artwork's own, which is what "repeat
  this at its natural size" means. Tiling happens in the layer's own
  space, so a rotating or scaled group carries the pattern with it rather
  than sliding underneath it. `offset` says where the pattern starts, and
  it is bindable and keyframable as `tile_x` and `tile_y`, so a scrolling
  texture is a tiled image with an animated offset.

  Vector artwork tiles the same way, and stays sharp at any size where a
  raster tile has to choose one: a board of squares is one small SVG
  repeated rather than a hundred rectangles written out. The copies are
  drawn as paths and cut off at the box, and a tile so small that it
  would fill the box more than 4096 times draws the first 4096.
- `text`: `text` (use `\n` for line breaks) drawn in `font`, a style
  declared in the show's `fonts` (see [Fonts](#fonts)). Optional `size`
  `[width, height]` is a box whose top-left corner sits at the layer's
  x/y; `align` places the text in it (`top_left`, `top`, `top_right`,
  `left`, `center` (default), `right`, `bottom_left`, `bottom`,
  `bottom_right`). Without `size` the box is exactly the text's size.
  Multi-line text aligns each line on its own within the box width.
  The `text` property can be bound (see [Bindings](#bindings)) but not
  keyframed.

  `reveal` (default 1) is how much of the text shows, as a share of its
  characters from 0 to 1: the first `reveal` of them are drawn and the
  rest keep their room, so a line neither reflows nor re-centres as it
  types. The next character snaps in the instant the share reaches it,
  the way a typewriter does. It is a numeric property like `opacity`,
  keyframed for text that types itself out, bound for a dialogue box the
  host advances. Every character counts, spaces and line breaks too, so a
  pause can be written into the text as spaces:

  ```json
  { "type": "text", "text": "$ cuelight-render deck/ --events", "font": "code", "reveal": 0,
    "timelines": [{ "name": "type", "trigger": "go", "hold": true,
      "tracks": [{ "property": "reveal", "keys": [{ "t": 0, "v": 0 }, { "t": 1.2, "v": 1 }] }] }] }
  ```
- `digits`: a row of `digits` equal cells across `size` `[width, height]`
  (top-left at the layer's x/y) showing `text`, one character per cell.
  `justify` is `left` (default) or `right`, as scores are shown; text that
  does not fit is cut at the other side. `text` can be bound like a text
  layer's, and `reveal` (default 1) means what it means there: the cells
  of the characters past it stay dark or empty. How a cell is drawn is up
  to `display`:

  ```json
  { "type": "digits", "digits": 16, "size": [128, 16], "text": "HELLO",
    "display": { "segments": { "style": "alpha14", "fill": "#FF5820", "unlit": "#2A0E05" } } }
  ```

  `segments` is a segment display, the kind that spells a number out of
  bars: lit segments in `fill`, the dark ones in `unlit` when given.
  `style` is `alpha14` (14 segments plus dot: letters, digits,
  `- + * / \ = _ '`) or `numeric7` (7 segments plus dot: digits and `-`).
  A `.` or `,` lights the dot of the cell before it instead of taking a
  cell, so `1,250` needs four cells. Characters the style cannot show stay dark.

  In shows that are rendered on their own pixel grid (a gray
  `output.mode`, or `pixel_perfect` scaling) segments are drawn in whole
  dots: the straight bars are a whole number of pixels thick, lie on
  pixel boundaries and end flat, and the diagonal strokes of `alpha14`
  are a staircase of whole dots rather than a shaded line. A cell then
  holds the three colours a dot display has, background, unlit and lit,
  whatever it spells.

  Three more say how it looks rather than what it says, all fixed per
  display and none of them bindable. Real displays differ a lot here, and
  one recreated without them looks wrong however right the digits are.

  - `slant`: degrees the cells lean, as a shear rather than a rotation, so
    the baseline stays level and the bars stay square to the row. Most
    real displays lean about ten, and 45 is as far as it goes; positive
    leans the tops to the right. On a pixel grid the bar is cut into one
    strip per pixel row, each shifted by whole pixels, so a leaning
    display is a staircase of crisp blocks and stays as sharp as an
    upright one.
  - `thickness`: bar width as a share of the cell's shorter side, 0.1 by
    default and 0.2 at most. The gaps follow it, so a fat display stays
    legible instead of running together, and past that cap the bars would
    meet.
  - `glow`: a halo round the lit segments in their own colour, `size` how
    far it reaches as a share of the cell's width and `strength` from 0
    to 1, where 1 is as bright as the segment itself where it leaves it.
    Unlit segments never glow. It is light on the panel, and it falls off
    as the square of the distance out, which keeps most of it within a
    bar's width of the segment. Where two segments' halos meet, inside
    the loop of an 8 for instance, the brighter of them shows: a halo is
    never brighter than the segment casting it, however many of them
    overlap, so a row of digits keeps its colour instead of burning to
    white at the joins.

    Useful sizes run from about 0.05, a rim, to about 0.25, a lit panel;
    past a third of a cell it is a wash rather than a display. It is
    drawn as the segment again a number of times, each grown and fainter,
    as one picture: a halo rather than a true blur, which nothing on the
    GPU can give us yet, and at the size a display is drawn it reads the
    same. Being one picture is what keeps two halos that meet from
    climbing past the colour of the segments casting them. On a small
    pixel grid there is no room for it, and it is better left off there.

  ```json
  { "segments": { "style": "numeric7", "fill": "#FF5820", "unlit": "#2A0E05",
                  "slant": 10, "thickness": 0.16,
                  "glow": { "size": 0.12, "strength": 0.8 } } }
  ```

  `reel` is a row of wheels: every cell carries a ring of symbols and
  rolls to the one the text asks of it, as an odometer, a counter or a
  departure board does. A symbol is named by a character of the ring's
  `charset` and drawn either as that character in a font style of the
  show, which needs no artwork and stays sharp at any size, or as the
  artwork `cells` gives it.

  ```json
  { "type": "digits", "digits": 6, "size": [180, 48], "justify": "right",
    "display": { "reel": { "font": "score", "charset": "0123456789",
                           "duration": 0.12, "ease": "quad_in",
                           "direction": "forward", "stagger": 0.03,
                           "offset": [ { "t": 0.12, "v": 0 }, { "t": 0.17, "v": 0.12 },
                                       { "t": 0.22, "v": 0 } ] } } }
  ```

  - `charset` (default `0123456789`): the characters naming the ring's
    symbols, in the order they pass by. A cell shows nothing for anything
    the text asks of it that the ring does not carry, so a ring can hold
    letters, punctuation or a blank as well as digits.
  - `font`: a style from the show's `fonts`, bitmap or outline, that a
    symbol is drawn in as its own character. Characters are centred on
    their ink rather than on the line they are laid out in, so a ring of
    digits sits in the middle of its cell instead of high, and the whole
    ring is measured at once so the row keeps still while it rolls: a ring
    holding a descender reserves room for it, one of digits does not.
  - `cells`: artwork for the symbols instead, one entry per character of
    the charset: `{ "vectors": ["cherry", "bar", "seven"] }` or
    `{ "images": [...] }`, named as the host registered them. The charset
    stays the ring's identity, so the text still says which symbol a cell
    lands on; this only says what that symbol looks like. Artwork is
    fitted into the cell and centred, keeping its shape, and vector
    artwork stays sharp at any size. One of `font` or `cells` is needed.
  - `duration`, `ease`, `direction` and `offset` say how a cell moves, and
    mean what they do on a binding's [transition](#transitions):
    `forward` is a wheel that only turns one way, an `offset` settles a
    cell against its stop.
  - `step` (default 1): how far a cell travels in one move, in symbols.
    One means it lands on every symbol on the way, each taking `duration`,
    which is how a counter or a split-flap board reads. `null` makes the
    whole journey a single move instead, so `duration` covers all of it
    and `ease` shapes the journey rather than each symbol: that is how a
    wheel spins, and how a meter whose lowest digit never stops rolling
    behaves.
  - `turns` (default 0): whole turns of the ring a cell adds before it
    lands. A spinning wheel takes a few; with `step` set to `null` the
    symbols fly past and the ease brings it to rest.
  - `window` (default 1): how many symbols of the ring the cell shows at
    once, stacked, with the one it stands on in the middle. A wheel behind
    a tall window shows its neighbours, as a machine with several rows
    does; the row's height is shared between them.
  - `stagger` (default 0): seconds each cell waits behind the one to its
    right, so a row does not move as one piece.
  - `spin` (a trigger name or a list): fires the row off. Every cell
    travels its `turns` and lands on the symbol its text names at that
    moment, whether or not that is the one it already shows. Without it a
    cell only moves when the symbol it is asked for changes, which is
    what a binding means everywhere else, and a wheel asked for what it
    already carries would stand still while its neighbours spin. A host
    that already fires a trigger for the lever and the sound lets the
    reels hear it too, and the value still says where they land: set it
    and fire in the same breath and the wheel makes one journey to the
    new symbol, not two.

  A row is one value across several cells, which is what a counter or a
  board line is. Things that move independently are independent rows: a
  machine with three wheels is three one-cell reel layers side by side,
  each bound to its own variable, so a host can start and stop them when
  it likes and give each its own charset, turns and duration. `stagger` is
  for the other case, where one value ripples across a row as a carry
  does.

  Each cell keeps its own place on the ring, so a change moves only the
  cells it reaches: going from `109` to `119` rolls the tens wheel and
  leaves the others standing. Where a cell stands is counted straight
  rather than around the ring, so a journey can be longer than one turn,
  and a cell stopped mid-travel carries on from exactly where it stands. A
  cell shows the symbol it stands on and the one coming up behind it,
  clipped to the cell, which is what makes a roll look like a wheel rather
  than a fade.
- `audio`: a sound, played like a timeline is; draws nothing. See
  [Sound](#sound).
- `video`: a moving picture, played like a sound is; the host decodes it.
  See [Video](#video).

## Sound

```json
{ "name": "thunder", "type": "audio", "sound": "thunder",
  "trigger": "strike", "stop": "hush", "gain": 0.8, "bus": "sfx",
  "on_end": "thunder_done" }
```

An audio layer plays `sound`, a sound the host registered by name (by
convention the file's stem, from `assets/sounds/`). It sits in the layer
tree like anything else: a scene's music starts with the scene and stops
when the scene is left, a group's `gain` scales every sound below it, and
`gain` is a normal numeric property, so bindings, transitions and
timelines give volume control, fades and warm-ups for free. It is
controlled the way a timeline is, with the same names and meanings:

- `trigger` (a name or a list) plays it; `autoplay` plays it when the show
  loads or its scene is entered.
- `duck`: step this layer back while something on another bus is
  sounding.

  ```json
  { "name": "bed", "type": "audio", "sound": "theme", "loop": true, "bus": "music",
    "duck": { "under": "voice", "to": 0.1, "attack": 0, "release": 0.1 } }
  ```

  A bed under clips that speak over it has to get out of the way and come
  back, which is the ordinary arrangement whenever there is music under
  anything that talks. `under` names the bus to listen to, `to` is the
  gain multiplier while it sounds, and `attack` and `release` are how long
  the level takes to get down and back, in seconds.

  Every sound is on a bus: the one its layer names, or `main`. So a bed
  with a bus of its own, ducking `under: "main"`, needs one annotation and
  works for every sound and every clip's soundtrack in the show. The cost
  of that default is that a sound added later, naming no bus, joins
  `main` and ducks the bed too; give it a bus of its own, or point the
  duck at a narrower one, if that is not wanted. A video layer takes
  `duck` as well, since a clip with a soundtrack wants it as much as a
  sound does. 0 is instant, which is
  usually right for the attack: the drop makes room for the first word,
  and it is the return that has to be smooth.

  It multiplies like every other gain, so it composes with bindings and
  with the gain of the groups above. A layer's own plays never duck it, so
  naming the bus it plays on does not hold it down for ever. Only the
  moment the bus changed is remembered; where the level *is* is a function
  of that and the time, so seeking stays possible.
- `delay`, `loop`, `repeat`, `on_end`: as on timelines. `on_end` fires when
  a play finishes (after its repeats, never for loops) and is reported to
  the host like a timeline's.
- `stop` (a name or a list): a trigger that ends the play at once, without
  `on_end`.
- `when` and `while`: a variable condition instead of a trigger, for
  hosts that report states rather than events, meaning what they mean on
  a [timeline](#timelines). `when` plays it on the rising edge, so a
  warning count reaching 2 plays its sound once, and beside `trigger`
  either starts a play. `while` plays it while the condition holds and
  stops it, without `on_end`, when it stops holding, which is what a
  looping bed under a mode wants. A one-shot under `while` sounds once
  when the condition turns true and is not started again until it turns
  true again, so a state never becomes a buzz. Video layers take both.

  ```json
  { "type": "audio", "sound": "warning_two",
    "when": { "variable": "warnings", "map": { "2": 1 } } }
  { "type": "audio", "sound": "alarm", "loop": true,
    "while": { "variable": "mode", "map": { "tilt": 1 } } }
  ```
- `retrigger` says what the trigger does while the sound already plays:
  `restart` (default: the play so far stops and a new one begins),
  `overlap` (another play sounds on top, up to `voices` at once, default
  4; the oldest stops beyond that: footsteps, flaps, coins), `ignore`
  (the play finishes undisturbed) or `queue` (the play finishes, then the
  one that waited begins; up to `voices` wait at once and the rest are
  dropped).
- `rest` (default 0): the least seconds between one play starting and the
  next. A trigger that arrives sooner is dropped, whatever `retrigger`
  would otherwise do with it and whether or not anything is playing: a
  switch that chatters, or an event a game fires every frame while a
  condition holds, then plays a sound at a sane rate instead of a buzz.
  Video layers take it too.
- `gain` (default 1): loudness, 0 to 1 and above, multiplied by the gains
  of the groups above it. Only groups and audio layers have it.
- `bus` (optional): a name the host may route to an output; nothing in
  the engine depends on it yet.

An invisible audio layer, or one in an invisible subtree, is not heard,
like everything else in that subtree resolves to nothing.

### One of several

`sound` takes a list, and each play uses one of them. Three recordings of
the same knock stop a bumper sounding like a tape loop, and it is the
same idea on a video layer: a handful of clips to fill a gap, a set of
idle animations that should not cycle visibly.

```json
{ "name": "knock", "type": "audio", "sound": ["knock1", "knock2", "knock3"],
  "pick": "random", "trigger": "hit" }
```

`pick` says which one a play takes:

- `in_order` (default): the next one each play, wrapping round. A single
  name is this, trivially.
- `random`: any of them, which may be the one that just played.
- `shuffle`: all of them in a scrambled order, then scrambled again, so
  nothing is skipped and nothing repeats until the rest have had a turn.

A list is not a playlist: the layer plays once per trigger like any
other, it does not run on to the next by itself.

None of this rolls dice. A pick is a function of how many times the layer
has played, so a show plays the same way every run, which is what
rendering it to a file needs, and two layers holding the same list still
pick apart. A host that wants a show to differ between runs seeds the
engine (`Engine::set_seed`, the clock will do): the same seed always
plays the same way.

The engine never opens an audio device or touches a sample. A host
registers each sound with only its duration (`Engine::set_sound`), which
is all the engine needs to loop, repeat and end plays, and reads back what
should be heard each frame with `Engine::voices()`: one voice per play,
with a stable id, the sound's name, the position in seconds, the effective
gain, whether it loops, and the bus. An audio backend diffs that list
frame by frame (a new id starts at its position, a vanished id stops, a
changed gain ramps, a position that jumped is resynced); a host with a
mixer of its own consumes the same list. What a backend must not do is
resync a voice that is merely *behind*: a sound device that was asleep
when the sound started can be most of a second late, and the position the
engine reports runs on regardless. Chasing it there throws away the start
of the sound. Only a position that moves in one step is a seek. Positions are a function of
engine time, so an offline render mixes sound sample-exact against the
frames, and a seek only needs the backend to resync. A sound registered
after its layer started playing is heard from where it would be by then;
until then the play is silent and does not end. The `cuelight-audio` crate
is that backend: decoding, a mixer, a sound device for the player and a
WAV file for offline rendering.

## Video

```json
{ "name": "intro", "type": "video", "video": "intro", "trigger": "start",
  "size": [640, 360], "loop": false, "on_end": "intro_done" }
```

A video layer plays `video`, a video the host registered, and draws its
current frame. It takes the playhead a sound takes and means it the same
way: `trigger` or `autoplay` starts it, `delay`, `loop`, `repeat` and
`on_end` behave as on a sound or a timeline, and `stop` ends it without
firing `on_end`. What it does not take is what only makes sense for
sound: a picture shows one thing at a time, so it cannot `overlap`. It does take the other three `retrigger` modes,
and they are how a layer fed by a driver arbitrates: `restart` (default)
cuts to whatever it is asked for last, `ignore` protects the clip that is
running, `queue` plays each in turn. Pointing a layer somewhere new
counts as asking it to play, so the same rule applies, and it counts
once: a layer left pointed at a clip while another finishes takes one
place in the queue, not one per frame. `size` scales
the picture as it does on an image; without it the video's own size is
used. `video` takes a list and a `pick` like a sound does.

`video` is a bindable property, so one layer can show whatever it is
pointed at rather than needing a layer per clip: bind it to a
variable, and setting that variable plays another clip from the top, at
its own size and for its own length. This holds whether or not the
layer is playing: naming a clip is the whole of what a host has to say,
so a layer that has run out starts the next one it is pointed at, and so
does one that has never played, which is what makes a surface with no
`trigger` and no `autoplay` work at all.

What does not start it is a binding with nothing to say: a variable that
is unset, or a value its `map` does not list, leaves the layer as it is.
It does not fall back to the layer's own `video` and treat that as an
instruction, because then a show could never say "nothing yet". A name
nothing registered simply shows nothing, as an unregistered image does.

An audio layer's `sound` is bindable in exactly the same way, and follows
the same rules: pointing a layer at a track plays it from the top, a
looping one keeps looping, a binding with nothing to say leaves the layer
alone, and what happens to a play already running is its `retrigger`. One
layer can therefore be a bed that follows whatever state a show is in,
instead of a layer per track each having to stop the others.

A video layer draws only while a play is running. Once a clip ends it
shows nothing, rather than holding its last frame, so whatever is behind
it comes through; a layer that should stay visible loops.

```json
{ "name": "backdrop", "type": "video", "video": "attract", "autoplay": true,
  "bindings": [ { "property": "video", "variable": "clip" } ] }
```

**The engine decodes nothing.** It knows a video only by what
`Engine::set_video(name, duration, size)` tells it: the length, so it can
loop, repeat and end a play, and the size, so the layer has a box before
anything has been decoded. Each frame the host reads `Engine::videos()`,
the picture twin of `voices()`: per playing video an id, the layer, the
video's name, the position in seconds, whether it loops, and the `frame`
name to hand the picture back under. The host decodes to that position
and hands it back with `Engine::set_image` under that name, which the
layer draws. A frame is then an image like any other, with the same
upload path and caches, and a host that cannot decode video still loads
the show: the layer shows nothing until a frame arrives, exactly as an
image layer does.

The `frame` name belongs to the layer, not to the clip, and the engine
makes it: a host passes it back and never builds one. That is what lets
the same clip play on two layers at different positions, each drawing
its own picture; under one name for the clip both would draw whichever
frame was written last. A layer shows one picture at a time, so one name
per layer is all it takes, and it does not change when the layer is
pointed at another clip.

Decoding lives outside the engine, in the `cuelight-video` crate, behind
cargo features, so a host pays for a decoder only if it wants one.

### A clip's own soundtrack

A clip cut as a self-contained sequence usually carries sound, and that
sound travels the path a sound already travels. The host registers it
with `Engine::set_sound` under **the video's name**, which is how it says
this clip has a soundtrack and hands over its samples; the layer is then
reported by `Engine::voices()` for as long as it plays, alongside its
picture in `Engine::videos()`. Both carry the same play id, so a host can
see they are one play.

The picture's own duration governs the position, so the two cannot drift
apart over a loop however long. A clip the host registered no sound for
is silent, which is also how a host says a clip should not be heard: it
simply does not hand over a soundtrack.

```json
{ "name": "backdrop", "type": "video", "video": "attract",
  "autoplay": true, "loop": true, "gain": 0 }
```

Video layers take `gain` (default 1) and `bus`, meaning exactly what they
mean on a sound: `gain` multiplies with the gains of the groups above, so
a group turns down everything below it, and it is a normal numeric
property, so a binding or a timeline fades a clip in. `gain: 0` keeps the
pictures and loses the sound, which is what a looping backdrop wants.

`Show::has_sound()` does not count video layers: whether a clip is heard
is the host's to know, not the document's.

## Fonts

```json
{
  "fonts": {
    "score": { "file": "teeny_tiny_pixls-5", "color": "#808080" },
    "title": { "file": "bm_army-12", "border": { "color": "#102C80", "width": 1 } },
    "over_video": {
      "file": "futura", "size": 24, "color": "#FFFFFF",
      "border": { "color": "#000000", "width": 1 },
      "shadow": { "color": "#000000A0", "offset": [3, 2] }
    }
  }
}
```

A style names a font the host registered (`file`, by convention the font
file's stem), a `color`, and an optional `border` of `width` pixels in
`color` outside every glyph.

A style may also carry a `shadow`: the same text drawn behind itself,
moved by `offset` pixels and painted in one `color`. It is drawn first, so
the text lands on top of it, and it is the text's whole silhouette with
the border included rather than the fill alone, which is what makes it
read as a shadow instead of a second outline. The offset is in canvas
pixels and scales with the layer; either number may be negative. Both
kinds of font take one, and so does any text drawn from a style: a text
layer, a reel's characters, a digit row. Fonts are host assets like images: a text
layer whose font is not registered is skipped. There are two kinds, and
which one a style uses is decided by what was registered under that name,
so the layers using it do not change.

**Bitmap fonts**, in the [BMFont](https://www.angelcode.com/products/bmfont/doc/file_format.html)
text format common for pixel fonts: the host parses the `.fnt` with
`BitmapFont::parse` and registers it with its page images through
`Engine::set_font`. The `padding` its `info` line declares, the room
inside every glyph's rect that a border is drawn into, is drawn with the
glyph but is not ink: a block is measured and characters are centred on
what is inside it, so a padded atlas and the same font rasterized from its
TrueType file lay out the same. They have one fixed size (setting `size` is an error),
`color` multiplies the glyph colors (white keeps the font's own), and a
border also widens each character's advance by two widths. Text is
rasterized on the CPU pixel for pixel from the font pages, never
resampled, so pixel fonts stay exact on DMD-sized canvases. A line is as
wide as its characters' advances plus kerning; a block is as tall as its
line heights, except that the last line grows to fit its tallest glyph.
Characters missing from the font fall back to their uppercase form, then
to a space.

**Outline fonts** (TrueType / OpenType), for text that stays sharp at any
size: the host registers the file's bytes with `Engine::set_outline_font`
(cargo feature `outline-fonts`), and the style sets `size`, the em size in
canvas pixels (required). Text resolves to a glyph run the renderer draws
from the font's outlines, so nothing is rasterized or cached per string.
Line height and the baseline come from the font's metrics. Glyphs are
placed by their advance width alone: there is no kerning, ligatures or
complex shaping yet, so text sets slightly looser than in a browser. A
character the font lacks shows as the font's "missing" box.

```json
{ "fonts": { "speed": { "file": "inter_bold", "size": 180, "color": "#FFFFFF" } } }
```

**Outline fonts as pixels.** A pixel font is usually shipped as the TTF it
was drawn from, and drawn from its outlines it is only exact when it
happens to land on the grid: at a half-pixel position, or a size that is
no whole multiple of its design size, its edges smear. `pixels: true`
rasterizes the font's glyphs once, at `size`, with hard edges and no
antialiasing, and draws them the way a bitmap font is drawn: never between
pixels, never resampled, with `border` and `shadow` as a bitmap font has
them. Its line box is the rows its glyphs occupy, from the highest
ascender to the lowest descender of the printable ASCII characters plus
the font's leading, with the baseline where the glyphs put it, the way a
bitmap font's `lineHeight` is; the roomier box a pixel font's metrics
declare would set every line a row or two low, so a pixel TTF and its
bitmap conversion lay out the same. Characters outside that range keep
their own rows and hang above or below the line, as an accented capital
does in any bitmap font. On a show rendered on its own pixel grid (`pixel_perfect` scaling or
a gray output mode) this is the default for every outline font, since
antialiased text is never what such a show wants; `pixels: false` asks for
the outlines anyway. The cost is once, at first use: the characters the
font has, at `size` pixels each. The same TTF then serves an HD show and a
dot display, and nothing generated needs to be kept.

```json
{ "fonts": { "score": { "file": "tiny5", "size": 8, "pixels": true } } }
```

## Bindings

A binding wires a layer property to a variable, evaluated every frame:

```json
{ "property": "opacity", "variable": "score", "scale": 0.001, "offset": 0.2 }
```

means `opacity = score * 0.001 + 0.2`. Animatable/bindable properties:
`x`, `y`, `opacity`, `scale`, `scale_x`, `scale_y`, `rotation`, `frame`
for sprite sheet images, `reveal` for text and digits layers, and `gain`
for groups and audio layers. `visible`, a video layer's `video` and an audio layer's `sound` can be
bound but not keyframed.

A text or digits layer's `text` property can be bound too: the variable's text as
is, a boolean as `true`/`false`, a number after `scale`/`offset` formatted
per `format`: `plain` (default: `1500`, `2.5`) or `thousands` (rounded, with
comma separators: `1,500`).

```json
{ "property": "text", "variable": "score", "format": "thousands" }
```

`decimals` rounds a number to that many places and always shows them, so
a value a timeline is moving reads `1.5` rather than
`1.4833333333333334`:

```json
{ "property": "text", "variable": "speed", "decimals": 1 }
{ "property": "text", "variable": "total",
  "format": "thousands", "decimals": 2 }
```

It applies after `scale` and `offset`, like the rest of formatting. A
value that rounds to nothing prints without a sign, and a counting
`transition` steps in the last place shown rather than flickering
through digits that are rounded away.

`prefix` and `suffix` put words round the value, which most readouts
have: `40%`, `2.5 X`, `BALL 2`, `LEVEL 12`, `$4.99`.

```json
{ "property": "text", "variable": "progress", "suffix": "%" }
{ "property": "text", "variable": "multiplier", "decimals": 1, "suffix": " X" }
{ "property": "text", "variable": "ball", "prefix": "BALL " }
```

They apply last, to whatever text the binding produces, so a counting
`transition` counts the number and leaves the words still, and a `map`'s
text gets them as much as a number does. A binding that does not apply
(an unset variable, a `map` with nothing to say and no `default`) leaves
the property at its base text, words included: the words belong to the
value, not to the layer. Words on a property other than `text` are
reported as a load warning and do nothing.


A text layer's `font` property can be bound to a variable naming a font
style, typically through a map. An image layer's `tint` works the same
way, bound to a variable naming a color: one sprite becomes a status
light, a lamp or a team color without a layer per state.

```json
{ "property": "tint", "variable": "state",
  "map": { "ok": "#00FF00", "warn": "#FFAA00", "bad": "#FF0000" } }
```

`map` looks the variable's value up (as text: `1`, `2.5`, `true`,
`attract`) and uses the mapped value in its place; `default` covers values
the map does not list, and without a default an unlisted value leaves the
property at its base. The mapped value then goes through the same
conversion as a plain variable (`scale`/`offset`, `format`). `map` and
`default` are part of how every reader reads a variable, not only a
binding; see [Reading a variable](#reading-a-variable). Highlighting the
active player's score:

```json
{ "property": "font", "variable": "player", "map": { "2": "score_active" }, "default": "score_inactive" }
```

Font bindings may only map to declared font styles and tint bindings only
to colors (`#RRGGBB`, `#RRGGBBAA`, or empty for no tint); `text`, `font`
and `visible` can be bound but not keyframed.

A `tint` cannot be keyframed either, but it can take a `transition`, so a
status light fades from green to red rather than snapping:

```json
{ "property": "tint", "variable": "state",
  "map": { "ok": "#00FF00", "bad": "#FF0000" },
  "transition": { "duration": 0.25 } }
```

One eased progress from 0 to 1 carries all four channels, so `duration`
and `ease` mean what they mean everywhere else and the channels arrive
together; alpha travels with them. Turned round halfway, it eases from
the color it had reached. `step` bands the fade into that many moves,
which is a way to get a DMD-like ramp. `wrap` and `direction` are
rejected: they describe a value on a ring, which a color is not.

Channels are mixed as the show writes them, the same 8-bit values the
`gray4` luma weights are applied to, so a fade is a straight line between
two colors rather than a walk through a perceptual space.

A value the property cannot use, a font style the show does not declare
or a tint that is not a color, leaves the property as it was. That is the
same silence as an image nobody has registered: the host may send
something usable on the next frame, and a frame is no place to complain.
What the show can be checked for at load is checked at load, and lands in
`Engine::load_warnings()`. Binding or keyframing a property
the layer's kind does not have (`font` on a shape) is rejected at load.

### Reading a variable

Everything that reads a variable reads it the same way: a binding, a
timeline's `when` and `while`, and whatever reads a variable next. The
fields are the same, they sit directly on the reader, and they apply in
the same order: `debounce`, then `map` (with `default`), then `threshold`
or `curve`.

- `variable`: the name read. A host variable of that name, or failing
  that a [value the show animates](#values-the-show-animates). Without
  either the reading has nothing to say: a binding leaves its property
  as it was, a condition is false.
- `map` and `default`: look the value up (as text) and use what is found,
  or the default, in its place; with a map that lists nothing and no
  default the reading has nothing to say.
- `threshold`: the value becomes 1 at or above this level and 0 below. A
  lamp whose brightness arrives as a float lights at a half; with a
  `transition` it warms up from there. On a `visible` binding it is the
  level the layer shows from; on a condition it is where the condition
  starts to hold. It is a `curve` written short, and is read as exactly
  that curve (see below).
- `debounce`: seconds a new value has to hold before it is read. Shorter
  changes (a strobing lamp, a switch that bounces) are never seen: a
  bound property does not move, a `when` timeline does not restart. At
  load and on entering a scene the current value applies at once.
- `curve`: bend the value against the input instead of taking it
  straight. Keys are a track's, with the input value where a track has
  time, and the same easings between them; below the first key it holds
  the first value, above the last it holds the last.

  ```json
  { "property": "opacity", "variable": "brightness",
    "curve": [ { "t": 0, "v": 0 }, { "t": 0.5, "v": 0.2, "ease": "quad_in" },
               { "t": 1, "v": 1 } ] }
  ```

  A lamp whose glow wants a gamma curve, a tachometer compressed at the
  low end, a loudness in decibels rather than a linear gain. One variable
  can feed several properties that each bend it their own way, which is
  what a host bending it before sending cannot do.

  A curve shapes value against **input**; a `transition`'s `ease` shapes a
  change over **time**. A binding can have both. It cannot have both
  `curve` and `threshold`, which are the same job: `"threshold": 0.5` is
  `[{ "t": 0, "v": 0 }, { "t": 0.5, "v": 1, "ease": "step" }]` written
  short, and the engine reads it as that curve. That is the rule for any
  shorthand the format has: it is defined as the longer form it stands
  for, so the two cannot drift apart. A curve only bends numbers, so one
  on `tint`, `font`, `video` or `sound` is reported as a binding that
  does nothing.

What comes out is a value. A binding goes on to `scale` and `offset`,
then its `transition`, so a curve is written in the variable's own units
and `scale` stays the last change of unit; a condition takes the value as
true when it is not 0.

```json
{ "property": "visible", "variable": "lamp_41", "threshold": 0.5, "debounce": 0.05 }
{ "name": "flash", "when": { "variable": "lamp_41", "threshold": 0.5, "debounce": 0.05 }, "tracks": [] }
```

### Transitions

By default a bound property jumps when its variable changes. With a
`transition` it moves there instead:

```json
{ "property": "x", "variable": "speed", "scale": 2.4,
  "transition": { "duration": 0.4, "ease": "cubic_out" } }
```

- `duration`: seconds a change takes, from the value the property has now.
- `ease` (default `linear`): any easing timelines know.
- `model`: follow a physical model instead, which decides its own timing;
  a `duration`, a `wrap` or a `direction` beside one is an error. The only
  model is `incandescent`, a glowing filament, shaped by three optional
  numbers: `kelvin`, how hot it runs at full power and so what colour it
  glows there (2700 by default, a warm white); `heating`, the seconds it
  takes to close about two thirds of the gap when the power goes up
  (0.007, so full brightness in a few tens of milliseconds); and
  `cooling`, the same going down, which on a real filament is the slower
  of the two (0.06).
  A bigger lamp is hotter and slower, a small one cooler and quicker.

  ```json
  { "property": "opacity", "variable": "lamp_12",
    "transition": { "model": "incandescent" } }
  ```

  A lamp is not a fade, and much of how a panel of lamps looks is how
  they switch. The binding's value is the **drive**, 0 to 1: what a dimmer
  or a duty cycle sets, not a brightness already worked out. The
  filament's temperature chases it: full brightness in a few tens of
  milliseconds, most of the light gone as fast when the power goes, then a
  dim glow for much longer, and a filament re-lit while still warm coming
  up quicker than a cold one. No duration and ease can say that.

  Light does not follow drive in a straight line, on a real lamp or here.
  At rest the default lamp shows:

  | drive | 0.25 | 0.5 | 0.75 | 1 |
  | --- | --- | --- | --- | --- |
  | shown | 9% | 33% | 64% | 100% |

  So half drive is a lamp turned well down, not a lamp at half. A host
  that already has the brightness it wants should bend it on the way in
  with a [`curve`](#bindings), rather than expect this to be linear.

  A numeric property gets the light the filament gives. A `tint` gets the
  colour it gives, which reddens as it cools, and there its variable is a
  power level rather than a colour. Put both bindings on the same variable
  and they agree, because they are the same filament run twice:

  ```json
  "bindings": [
    { "property": "opacity", "variable": "lamp_12",
      "transition": { "model": "incandescent" } },
    { "property": "tint", "variable": "lamp_12",
      "transition": { "model": "incandescent" } }
  ]
  ```

  Entering a scene starts its properties at their values, as at load, so
  a lamp in a scene is cold again however warm it was when the scene was
  left. One that should keep its heat belongs in the show's own layers,
  which are never left.

  A `model` and a `curve` both bend a number, and they are not the same
  thing: a curve maps a value against its **input**, and stays where it
  is put; a model is a thing with **state**, whose output depends on how
  warm it already was. A lamp whose brightness is simply the wrong shape
  wants a curve; one that should come up fast and fade slowly, and come
  up faster when it is already warm, wants this. A binding can have both,
  the curve shaping what is fed in.

  The heating and cooling times come from D. C. Agrawal's *Heating-times
  of tungsten filament incandescent lamps*, and the colour from Tanner
  Helland's fit to Mitchell Charity's blackbody table; both are credited
  in the README.

  A show starts with its lamps cold, whatever they are being told, so a
  filament takes its time even on the first frame. Like every transition it
  stays a function of time, so a render is independent of the frame rate
  and seeking works.
- `wrap`: the value lives on a ring of this size: 360 for an angle, 10 for
  a sheet with one frame per digit. `direction` picks the way round:
  `shortest` (default), `forward` (a reel: 9 to 0 rolls on) or `backward`.

- `step`: the size of one move. A larger change plays as several moves in
  a row, each taking `duration` with the `ease` applied per move, the last
  one shorter when the change is no whole number of steps: a reel kicked
  digit by digit.
- `offset`: motion added on top of a move, along its direction of travel.
  Keys like a timeline track's: `t` in seconds from the start of the move,
  `v` in the units of the binding's result, optional `ease`. It has to
  start and end at 0. Unlike an overshooting ease, which swings by a share
  of the distance, an offset is the same size however far the move goes.
  With `step` every move gets it, and a move lasts as long as the longer of
  `duration` and the offset.

A score reel from a sheet with one frame per digit: kicked one digit at a
time, there in 0.12 s, settling against its stop by an eighth of a digit:

```json
{ "property": "frame", "variable": "credits",
  "transition": { "duration": 0.12, "ease": "quad_in", "step": 1,
                  "wrap": 10, "direction": "forward",
                  "offset": [ { "t": 0.12, "v": 0 }, { "t": 0.17, "v": 0.12 },
                              { "t": 0.22, "v": 0 } ] } }
```

What is eased is the binding's result, after `map`, `scale` and `offset`,
so a map from states to opacities fades between them. On a `text` binding
a number counts up or down before it is formatted, in whole numbers when
it goes from one whole number to another; text that is not a number
jumps. `font` bindings cannot have a transition.

A change while a transition runs starts a new one from the value reached
so far, with the full duration. A counter keeps counting in whole numbers
across such a change: what decides that is the values the binding was
given, not where the interruption landed. When a show loads or a scene is entered,
properties start at their value: nothing eases in from the base. The value
is a function of time only, so it does not depend on the frame rate.

A transition also smooths a variable the host updates less often than the
display refreshes, at the price of showing the value late by about the
duration. For a regular feed use a `linear` ease with the feed's interval
as `duration`: the value then moves continuously, one interval behind. An
ease that slows down at the end pauses before every update.

## Timelines

A timeline is a keyframed animation owned by its layer:

```json
{
  "name": "slide",
  "trigger": "go",
  "autoplay": false,
  "loop": false,
  "tracks": [
    {
      "property": "x",
      "keys": [
        { "t": 0.0, "v": 64 },
        { "t": 1.0, "v": 440, "ease": "cubic_in_out" }
      ]
    }
  ]
}
```

- It starts when the host fires its `trigger`, or at load when
  `autoplay` is true. Re-firing the trigger restarts it from 0. `trigger`
  is one name or a list (`["turn_left", "hazard"]`); firing any of them
  has the same effect. A scene's `trigger` takes the same two forms.
- `when` starts it from a variable instead, for hosts that send states
  rather than events: a lamp going on, a score crossing a mark, a mode
  taking a value. Without it such a host has to watch its own variables
  and invent trigger names for them, which is show logic living outside
  the show.

  ```json
  { "name": "flash", "when": { "variable": "lamp_12", "threshold": 0.5 }, "tracks": [] }
  ```

  The condition is a [reading](#reading-a-variable): the same `variable`,
  `map`, `default`, `threshold`, `curve` and `debounce` a binding has,
  read in the same order, and it is true when what comes out is not 0.
  So `{ "variable": "mode", "map": { "multiball": 1 } }` is true exactly
  in that mode, and a `debounce` keeps a flickering lamp from starting
  the timeline over. What starts the timeline is *becoming* true, not
  being true, so a lamp that stays on plays its animation once; going
  false and true again starts it again.
  The edge belongs to the variable, not to the scene: leaving a scene and
  coming back does not replay it, unless the condition turned true while
  the scene was away. A condition already true when the show loads counts
  as becoming true. `trigger` and `when` can both be set: either starts
  it, which is what a test trigger on top of a state wants.

  `while` runs a timeline for as long as its condition holds, and stops
  it when it stops holding:

  ```json
  { "name": "blink", "loop": true, "while": { "variable": "lamp_12" },
    "tracks": [] }
  ```

  A blink that means "this is lit" should last as long as it is lit, and
  a looping timeline started on an edge would never stop. Unlike `when`,
  entering a scene starts it again, since it describes a state the scene
  is in rather than something that happened. Stopping is not finishing,
  so it fires no `on_end`. A timeline takes one or the other, not both.
- Keys are `(t seconds, value)`; between two keys the value interpolates
  using the **later** key's `ease` (before the first key it holds the
  first value, after the last it holds the last). Easings: `linear`
  (default), `quad_in`, `quad_out`, `quad_in_out`, `cubic_in`,
  `cubic_out`, `cubic_in_out`, `step`, and three kinds that do not go
  straight there: `back_in`, `back_out`, `back_in_out` swing about 10%
  past an end (`back_out` overshoots and comes back: a reel snapping
  against its stop), `elastic_out` arrives fast and rings around the end,
  `bounce_out` hits the end and bounces off it without passing it.
- The timeline's duration is its longest track's last key. When it ends
  it stops and its properties fall back (see precedence); with `loop`
  the playhead wraps instead. For a seamless loop, author each track's
  value at the end equal to its value at 0.
- `delay` (seconds) postpones the first key after the timeline starts;
  meanwhile it does not own its properties. A loop or repeat does not
  wait again, so `delay` plus `loop` is "wait, then repeat forever".
- `repeat` plays it that many times (fractions stop partway: `2.5` ends
  halfway through the third play); `loop` repeats forever. The two cannot
  be combined.
- `on_end` names a trigger fired when the timeline finishes (after its
  last repeat; loops never finish). It behaves exactly like a host firing
  the trigger, so it can start other timelines or restart a sequence, and
  it is reported to the host as an event (see [Host contract](#host-contract)).
- `hold` keeps the last values instead of handing the properties back.
  Ending and falling back is what a pulse or a flash wants; a fade *into*
  a state wants to stay there. A held timeline still ends once, and fires
  its `on_end` once, and then keeps its properties until it is started
  again or its scene is left. It means nothing on a `loop`, which never
  finishes.

  A held timeline of a single key at `t: 0` is the shortest way to write
  "set this and keep it": it ends on the instant it starts and holds that
  value, so a trigger can show a caption or hide a page until another
  trigger takes it back.

  ```json
  { "name": "show", "trigger": "game_over", "hold": true,
    "tracks": [{ "property": "opacity", "keys": [{ "t": 0, "v": 1 }] }] }
  ```

## Values the show animates

A timeline animates a property of its own layer. Two layers that must move
together therefore have to duplicate its keys, and nothing then says they
are meant to agree: a scene restarting one, or an edit to one set of keys,
parts them silently. A group shares a transform instead, but it scales
positions along with everything else, so it only works when every reader
sits at the group's origin.

`values` names a value the show animates itself, which bindings read the
way they read a variable:

```json
{
  "values": {
    "distance": { "timelines": [
      { "name": "approach", "autoplay": true, "on_end": "c1",
        "keys": [{ "t": 0, "v": 270 }, { "t": 4, "v": 206 }] },
      { "name": "partial_in", "trigger": "c1", "on_end": "beads",
        "keys": [{ "t": 0, "v": 206 }, { "t": 14, "v": 22 }] }
    ] }
  }
}
```

```json
{ "property": "x", "variable": "distance", "scale": 0.5 }
```

- A value is played by timelines, exactly as a layer is. `name`,
  `trigger`, `autoplay`, `delay`, `loop`, `repeat`, `on_end` and `hold`
  all mean what they mean on a layer's timeline, and `keys` are a track's
  keys, eases and all. What a value has instead of `tracks` is one set of
  keys, because the value is the one thing being animated.
- Several timelines cover one value in stretches, each started by its own
  trigger, the way a layer holds several timelines for one property. A
  running one wins over one that has finished and is holding.
- A value that just cycles is one timeline with `loop`.
- A host that sets a variable of the same name takes it over, so a show
  can ship with motion of its own that a host is free to seize.
- Nothing in a show writes one. Variables are the host's inputs; content
  writing them would make ownership ambiguous and allow a variable that
  drives a timeline that writes that variable.
- `Engine::value(name)` reads either: the host's variable when it set one,
  else the show's value at that moment.

Timelines are the one kind of value so far. Whatever a show comes to
animate or decide itself next (a timer counting at a rate, a counter
stepped by triggers, a queue of announcements whose current item is read)
is another kind of value under `values`, read by the same bindings and
taken over by a host variable of the same name the same way. The format
grows by kinds of value, never by a new top-level section per mechanism:
one place to look for what the show owns, one way to read it.

## Property precedence

Each frame a property resolves to, strongest first:

1. a running timeline that animates it
2. a finished timeline with `hold` that animated it
3. a binding
4. the base value on the layer

So a timeline temporarily owns whatever it animates; when it finishes the
property falls back to its binding or base value instantly, unless it
holds. To avoid a visible jump, make base values match the timeline's
endpoints, or hold. A binding's transition keeps following its variable
underneath a timeline, so the property falls back to where the transition
is by then.

Because a held timeline ranks below a running one, a flash over a faded-in
panel plays on top and hands the property back to the held value rather
than to the base when it ends.

## Host contract

The engine comes in two crates. `cuelight-core` is the model: the show
document, the state a show is in, and the clock that moves it, with
nothing drawn; `cuelight` wraps that engine with the assets a frame needs
(images, fonts, vector artwork), builds the draw list and, with the
`render` feature, rasterizes it. Hosts hold a `cuelight::Engine`; the
core calls below are the same on both, and `core()` / `core_mut()` hand
the core out to anything that only needs the model, such as a driver
script or a seek. Loading a show goes through the `cuelight` engine,
which is where a font style is checked against the font it names.

The engine is driven exclusively through four calls: `load_show` (JSON in),
`set_variable`, `trigger`, and moving the clock. `key` and `press` are
the same door: they look up what the show says the key or the point
means and fire that trigger (see [Input](#input)). What the show itself
fires (`on_end` triggers) comes back through `drain_events()`, so content
can tell the host that something finished. Output is either
`resolved_layers()` (a flat, GPU-free draw list; each item carries a
`transform` that is the identity unless a rotation or uneven scale placed
it, in which case the shape is in its layer's space and the transform
puts it on the canvas) or the `render` feature's vello rasterizer, plus
`voices()` for what should be heard (see [Sound](#sound)), and
`values()` for what every layer's properties resolved to, with no
geometry built. Both name each layer by its path (the tree it is in and
the index of each step down it), since names need not be unique, and
`layers_at([x, y])` lists the layers drawn under a canvas point, topmost
first, by the test a press uses.

Three more calls show the show's working rather than its result, for an
editor or a debugger: `drain_trace()` hands back what happened since the
last call and why, each at its own instant (a trigger fired and by whom,
a scene entered, a timeline started by which trigger, at load, on
entering a scene or by its condition, ended, held or stopped, a
condition turning, a sound or a clip played and why, and its play over,
finished or stopped short and how); `explain(layer, property)` lists every source the
property takes its value from now, strongest first, which is the
[precedence](#property-precedence) answered for one property at one
instant; and `start_timeline(layer, index)` starts one timeline on its
own, without firing its trigger, so an `autoplay`, `when` or `while`
timeline can be previewed alone. Everything else, including where variable values and
trigger events come from (game state, audio, MIDI, a console), is the
host's business: see the `cuelight-player` crate and the `mic_pop` example.

What a host can offer as the show's actions is `Show::triggers()`, every
trigger name the show listens to, and `Show::listeners()` says where each
is heard, so the buttons can be grouped and the ones the active scene is
not listening to dimmed: a trigger that enters a scene *opens* it, one the
show's own layers hear, or more than one scene does, is heard *anywhere*,
and one only a scene's layers hear belongs to that *scene*. Keys and
presses that fire a trigger nobody hears are listed as heard anywhere.
The player marks its menu with these, and the web player hands them out
as `player.listeners()`.

The clock moves with `advance_to(instant)` or `advance_frame(dt)`. A host
that can say what time it is should say so: `advance_frame` adds the
delta to where the clock already is, and `previous + delta` is not the
instant that was meant, so the same moment reached at two frame rates
lands a rounding error apart. Worse, a host that calls it every frame
keeps the show's time as a running total of its frame times, and whatever
a frame does not account for, a hitch or a delta it clamped, is gone from
the show for good while the sound plays on at the sound card's rate.

Anchor once and derive:

```rust
let start = Instant::now();                        // once
engine.advance_to(start.elapsed().as_secs_f64());  // every frame
```

A monotonic clock, never a wall clock: a show that jumps back an hour on
a clock correction is worse than one that drifts. Anchored, a long stall
makes the show catch up rather than run late, which is what keeps it
against its own soundtrack. Pausing and seeking move the anchor
deliberately (`start = Instant::now() - show_time`), and so does a gap
too long to be a frame, a machine that slept or a tab in the background:
a decision the host makes and can log, rather than time quietly
vanishing. `cuelight-player` keeps five seconds as that mark.

`advance_frame` is still the one for a host that genuinely has only a
delta, and for a script or a render that has the instant, `advance_to`
takes it directly.

A host that lets a show be played with records what it did and when,
the triggers it fired and the variables it set by hand, in a
`cuelight_loader::Live`, and hands that to `seek` along with the driver
script and the engine's core (`engine.core_mut()`). Seeking is
replaying, so an input that was not recorded is an input the show loses
the moment anyone scrubs; recorded, a scrub puts the show back exactly
where playing left it, and scrubbing past a press and back again finds
it still there. A driver's steps and the recorded inputs are applied at
their own instants, with the clock moved there first, so a seek jumps
from one input to the next and lands exactly where playing at any frame
rate would have.

`dt` is how much time passed, not how much of the show to play in one
piece. A frame is cut at every instant something inside it ends, at
every instant a value an eased binding reads changes, and at the instant
a value the show animates turns a condition, so a timeline or a clip
lasts exactly as long as it says whatever the frame rate is, a
transition starts where its input moved rather than where the frame
landed, a `when` fires where its value crossed the mark, and a chain of
timelines linked by `on_end` lasts what its parts
add up to: three ten-second clips end at thirty seconds at 60 fps and at
0.1 fps alike. Frame rate only decides when the host is *told*, since
`drain_events()` is read once a frame; the show's own clock is already
right.

## Rendering frames from the command line

`cuelight-render`, in `cuelight-loader` behind the `render-cli` feature,
loads a show the way the player does and writes frames without a window:

```sh
cuelight-render eclipse/ --at 4.1,19.5,26 -o frames/
cuelight-render eclipse/ --every 0.5 --until 52 -o frames/
cuelight-render eclipse/ --until 52 --events
cuelight-render dmd/ --at 2 --scale 4 -o frames/
```

Time is walked in fixed steps of `--fps` (60 by default) from 0, so a run
is repeatable and a frame at a given time is reached the same way however
many were asked for. `--events` prints the show's trace, what fired,
started, played, ended and turned, when and why, and needs no GPU. `--trigger 2.5:go` and `--set 0:score=1500` add inputs at a
time, and `--no-driver` ignores the folder's driver script. `--lenient`
renders what can be rendered of a show that does not load whole, printing
what was left out (see [Format version](#format-version)).

By default a frame is the canvas at the show's own size. `--scale N`
writes what a host would show instead, N times that size: fitted the way
the show's `scaling` asks, with its output mode and passes applied. A
`dots` pass needs about `--scale 3` before there are enough pixels to
make dots out of.

`--width W` picks the factor instead, giving a frame at most `W` wide,
for a gallery of shows that are not all the same size:

```sh
# 128x32 becomes 640x160, 192x64 becomes 576x192,
# and 1920x1080 comes down to 640x360.
cuelight-render dmd/ --at 2 --width 640 -o thumbs/
```

A show narrower than `W` goes up by the largest whole factor that fits,
so a pixel-perfect show gets no letterbox and no uneven pixels. A wider
one is presented at a whole fraction of its size, drawn sharp at that
size rather than shrunk afterwards. Whole factors only, so `W` is a bound
rather than a promise: a 960 wide show at `--width 640` comes out 480
wide, the next whole step down.

Sound lengths are read from the file's header, so a sound ends and its
`on_end` fires and a show chained through one runs to the end. Nothing is
decoded unless the header does not say (a constant-bitrate MP3 without a
Xing header), no sound device is opened and nothing is played.

Video lengths come from the file's header, through `ffprobe`, so a show
chained through a video's `on_end` runs to the end as well, and a clip is
measured bounded by the canvas exactly as the player measures it.

A video layer draws the frame it is playing: for each frame written, the
engine says which clip is showing and how far in, and that one frame is
decoded and handed back as an image. Only frames that are written cost
anything, so a still of a show at twenty seconds decodes one frame of
each clip rather than twenty seconds of video, and `--events` decodes
nothing at all. A long strip is a decode per frame, though, so it is
slower than the same strip of a show without video.

A show with no videos never looks for ffmpeg; one that has them says so
per file and carries on if ffmpeg is not installed, drawing the layer as
nothing.

Determinism: within one run the engine and the renderer are exact, so a
strip is internally consistent. Across runs the GPU can differ by one
level on a pixel, so comparing frames from separate runs wants a
tolerance.

## Regenerating the schema

The schema is generated from the Rust model types (`schemars`) and checked
by CI, so it cannot drift. After changing the model:

```sh
UPDATE_SCHEMA=1 cargo test --features schema --test schema
```

Driver files have one too, generated the same way from
`cuelight-loader`'s model and checked the same way:

```sh
UPDATE_SCHEMA=1 cargo test -p cuelight-loader --features schema --test schema
```

Point a driver file at it as a show points at its own:

```json
{ "$schema": "../../schemas/driver.schema.json", "loop": true, "steps": [] }
```
