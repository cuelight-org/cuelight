//! How a show asks to be shown: scaling, edges and the passes over the
//! finished frame.

use super::*;

/// How the finished frame reaches the display. Every field is optional: a
/// scene's output overrides only the fields it sets, the rest come from
/// the show's, then from the defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Output {
    /// Color conversion; `rgb` by default.
    #[serde(default)]
    pub mode: Option<OutputMode>,
    /// Color that full luminance maps to in the gray modes, `#RRGGBB`
    /// (white by default). Ignored in `rgb` mode.
    #[serde(default)]
    pub tint: Option<String>,
    /// How hosts scale the frame up to their surface; `smooth` by default.
    #[serde(default)]
    pub scaling: Option<Scaling>,
    /// Effects applied to the finished frame as it is shown, in order. A
    /// scene's list replaces the show's; an empty list turns them off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<Vec<Pass>>,
    /// How the edges of shapes fall on a show drawn on its own pixel
    /// grid: `soft` (default) smooths them, `hard` lights a pixel only
    /// where its centre is inside the shape, so a circle is a disc of
    /// whole dots and a moving shape steps a dot at a time. Nothing to a
    /// show not on its pixel grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edges: Option<Edges>,
}

/// How the edges of shapes fall on a pixel grid; see [`Output::edges`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Edges {
    /// Smoothed, as everywhere else.
    #[default]
    Soft,
    /// Whole pixels: lit where the pixel's centre is inside.
    Hard,
}

/// An effect on the finished frame, applied where it is shown (windows, the
/// web player), not by the offscreen renderer: it works at the surface's
/// resolution, which a frame at canvas size does not have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Pass {
    /// Every canvas pixel becomes a dot, as on a dot matrix display.
    Dots(Dots),
}

/// The dot matrix look: canvas pixels shown as separate dots on black.
/// Below three surface pixels per dot there is no room for it and the frame
/// is shown plain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Dots {
    /// Dot diameter as a share of the pixel pitch, above 0 up to 1.
    #[serde(default = "default_dot_size")]
    pub size: f64,
    #[serde(default)]
    pub shape: DotShape,
    /// Color of a dot that is off, `#RRGGBB`: the faint dots of a real
    /// panel. Dots never get darker than this. None by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlit: Option<String>,
    /// How much lit dots bleed into the dark around them, 0 (not at all,
    /// the default) to 1.
    #[serde(default)]
    pub glow: f64,
}

fn default_dot_size() -> f64 {
    0.8
}

impl Default for Dots {
    fn default() -> Self {
        Self {
            size: default_dot_size(),
            shape: DotShape::default(),
            unlit: None,
            glow: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DotShape {
    #[default]
    Round,
    /// Square dots with gaps: an LED matrix.
    Square,
}

impl Output {
    /// This output with its unset fields taken from `base`.
    pub fn over(&self, base: &Output) -> Output {
        Output {
            mode: self.mode.or(base.mode),
            tint: self.tint.clone().or_else(|| base.tint.clone()),
            scaling: self.scaling.or(base.scaling),
            passes: self.passes.clone().or_else(|| base.passes.clone()),
            edges: self.edges.or(base.edges),
        }
    }
}

/// How hosts should scale the rendered frame up to their surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Scaling {
    /// Any factor, smoothly filtered.
    #[default]
    Smooth,
    /// Whole-number factors with nearest-neighbor sampling, so every
    /// canvas pixel becomes a crisp square block (DMD-resolution content).
    PixelPerfect,
}

/// How the finished frame's colors are converted for the display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum OutputMode {
    /// Full color, unchanged.
    #[default]
    Rgb,
    /// Luminance quantized to 4 levels (2 bits), times the tint.
    Gray2,
    /// Luminance quantized to 16 levels (4 bits), times the tint.
    Gray4,
}
