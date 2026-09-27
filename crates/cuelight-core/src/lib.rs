//! cuelight-core: the deterministic model behind a cuelight show.
//!
//! A show is a tree of typed layers whose properties are driven by three
//! kinds of input: **variables** (named values pushed by the host),
//! **triggers** (named events fired by the host) and **timelines**
//! (keyframed property animation described as data). This crate holds
//! the document those are written in ([`Show`] and the types under it),
//! the state a show is in, and the clock that moves it: the state at an
//! instant is a function of the show, the inputs so far and the instant,
//! never of how many frames it took to get there.
//!
//! The contract is four calls:
//!
//! - [`Engine::load_show`]: load a declarative show description (JSON)
//! - [`Engine::set_variable`]: push a named value from the host
//! - [`Engine::trigger`]: fire a named event
//! - [`Engine::advance_to`]: move the clock to an instant
//!
//! and what comes back is described, not drawn: [`Engine::values`] is
//! what every layer's properties resolved to, [`Engine::voices`] what
//! should be heard, [`Engine::videos`] what should be showing, and
//! [`Engine::drain_events`] what the show itself fired.
//!
//! # What belongs here
//!
//! Anything that decides *what the show is doing*: the document and its
//! validation, variables, triggers, scenes, timelines, bindings and their
//! transitions, media plays, ducking, reels, the frame cutting that keeps
//! all of that exact at any frame rate, and the input map (which key or
//! press means which trigger). Sounds and videos are registered by what
//! the clock needs of them, their length and size.
//!
//! # What does not
//!
//! Anything that needs pixels, glyphs or samples: images, fonts and vector
//! artwork, the draw list built from them, hit testing a press against
//! it, output colour handling and rendering. That is the `cuelight` crate,
//! which wraps this engine and adds the assets; decoding and mixing are
//! further out still (`cuelight-audio`, `cuelight-video`, the loader and
//! the players). A change that would make this crate read an asset is a
//! change on the wrong side of the line.
//!
//! No `std::time`, no I/O, no randomness: a show plays the same way
//! everywhere, and a test can drive it to any instant and look.

mod easing;
mod engine;
mod lamp;
mod model;
mod path;
mod value;

pub use easing::Easing;
pub use engine::{
    frame_key, layer_at, root_layers, row_cells, Engine, Error, Event, Playing, Root, VideoInfo,
    Voice,
};
pub use model::{
    parse_color, Align, Binding, Blend, Border, DigitDisplay, Direction, DotShape, Dots, Duck,
    Fill, FontStyle, Glow, Gradient, Justify, Key, Layer, LayerKind, Media, MediaKind, Model,
    NumberFormat, Output, OutputMode, Pass, Property, Reel, ReelCells, Retrigger, Scaling, Scene,
    SegmentStyle, Shadow, Shape, Sheet, Show, Stroke, Tile, Timeline, Track, Transition, Triggers,
    When, FORMAT, MAIN_BUS,
};
pub use path::{PathData, PathElement};
pub use value::Value;
