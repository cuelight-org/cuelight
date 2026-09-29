//! cuelight: an embeddable multimedia engine.
//!
//! Shows are trees of typed layers whose properties are driven by three
//! kinds of input: **variables** (named values pushed by the host),
//! **triggers** (named events fired by the host) and **timelines**
//! (keyframed property animation described as data). The show document
//! and the clock that plays it are the [`cuelight_core`] crate; this
//! crate is that engine with the assets a frame needs, and the frame.
//!
//! The core contract is four calls:
//!
//! - [`Engine::load_show`]: load a declarative show description (JSON)
//! - [`Engine::set_variable`]: push a named value from the host
//! - [`Engine::trigger`]: fire a named event
//! - [`Engine::advance_frame`]: advance time by `dt` seconds
//!
//! plus obtaining the output. [`Engine::resolved_layers`] is a flat draw
//! list for hosts that draw themselves: the engine only describes what to
//! draw and never touches the GPU. With the `render`
//! feature, [`render::Renderer`] renders offscreen to RGBA pixels and
//! [`render::Presenter`] puts the show on a host surface, fitted and with
//! its output mode applied. [`Engine::voices`] is the same for sound: what
//! should be heard, for an audio backend to play (the engine never touches
//! samples). [`Engine::drain_events`] hands back what the show itself
//! fired.
//!
//! # What belongs here
//!
//! Anything that needs an asset to answer: images, bitmap and outline
//! fonts and vector artwork ([`Engine::set_image`], [`Engine::set_font`],
//! [`Engine::set_vector`]), text layout and rasterization, the geometry
//! of the draw list (shapes, paths, segment displays, reels, transforms),
//! hit testing a press against that list ([`Engine::press`]), output
//! colour handling ([`OutputColor`]) and the renderer.
//!
//! # What does not
//!
//! Anything that decides what the show is doing: that is
//! [`cuelight_core`], and this crate only reads its answers. Everything
//! further out (files, decoding, sound devices, windows) belongs to hosts
//! and adapters: the loader, `cuelight-audio`, `cuelight-video` and the
//! players.
//!
//! Types the show is written in (`Show`, `Layer`, `Value`, `Property`,
//! ...) and what the core reports (`Event`, `Voice`, `Playing`) are
//! [`cuelight_core`]'s and are used from there; nothing is re-exported.

mod engine;
mod font;
mod lru;
#[cfg(feature = "outline-fonts")]
mod outline;
mod output;
#[cfg(feature = "outline-fonts")]
mod pixels;
mod segments;

pub use engine::{
    AssetError, Engine, FontData, FrameProfile, ImageData, LayerCost, PlacedGlyph,
    ResolvedGradient, ResolvedGradientKind, ResolvedLayer, ResolvedShape, TextStats, Tiled,
    Transform, Vector, VectorPath,
};
pub use font::BitmapFont;
pub use output::{OutputColor, LUMA_WEIGHTS};

#[cfg(feature = "render")]
pub mod render;

/// Re-export of the vello crate (with the `render` feature) so hosts that
/// composite the show themselves can use matching vello/wgpu types.
#[cfg(feature = "render")]
pub use vello;
