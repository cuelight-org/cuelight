//! Digit displays: segments that light, lean, thicken and glow.

use super::*;

/// How the cells of a digit row are drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DigitDisplay {
    /// A segment display: lit segments in `fill`, and the dark ones in
    /// `unlit` when given. A `.` or `,` lights the dot of the cell before
    /// it instead of taking a cell. Characters the style cannot show stay
    /// dark.
    Segments {
        style: SegmentStyle,
        /// What the text says: characters (default), or the segments
        /// themselves as masks or brightness levels.
        #[serde(default)]
        input: SegmentInput,
        fill: String,
        #[serde(default)]
        unlit: Option<String>,
        /// Degrees the cells lean, as a shear rather than a rotation, so
        /// the baseline stays level. Most real displays lean about ten,
        /// and 45 is as far as it goes. Positive leans the tops to the
        /// right.
        #[serde(default)]
        slant: f64,
        /// Segment width as a share of the cell's shorter side, 0.1 by
        /// default and 0.2 at most. The gaps between segments follow it,
        /// so a fat display stays legible instead of running together,
        /// and past that cap the bars would meet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thickness: Option<f64>,
        /// A halo around lit segments, in their own colour. Unlit ones
        /// never glow.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glow: Option<Glow>,
    },
    /// Cells that roll through a ring of characters, drawn in a font.
    Reel(Reel),
}

/// A halo around the lit segments of a display.
///
/// Gas-discharge and fluorescent displays spill light around every lit
/// segment, and a panel recreated without it looks wrong however right
/// the digits are.
///
/// Drawn as the segment again, a few times, each grown and fainter than
/// the last: a halo rather than a true blur, which nothing on the GPU can
/// give us yet (see the note on text shadows). At the sizes a display is
/// drawn it reads the same.
///
/// Drawn as one picture, so that where two segments' halos meet the
/// brighter of them shows: a halo is never brighter than the segment
/// casting it, however many of them overlap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Glow {
    /// How far it reaches beyond the segment, as a share of the cell's
    /// width. About 0.05 is a rim and about 0.25 a lit panel; past a
    /// third of a cell it is a wash rather than a display.
    pub size: f64,
    /// How bright it is where it leaves the segment, 0 to 1, where 1 is
    /// as bright as the segment itself.
    #[serde(default = "default_glow_strength")]
    pub strength: f64,
}

fn default_glow_strength() -> f64 {
    0.5
}

/// Segment layout of a segment display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SegmentStyle {
    /// 14 segments plus dot: letters and digits.
    Alpha14,
    /// 16 segments plus dot: the 14, with the top and bottom bars split
    /// in two halves.
    Alpha16,
    /// 7 segments plus dot: digits and `-`.
    Numeric7,
    /// 9 segments plus dot: the 7, with two upright bars down the middle
    /// for a narrow `1`.
    Numeric9,
}

/// What the text of a segment display says: characters, which the
/// display spells out, or the segments themselves, for what is no
/// character (a test pattern, a sweep, a lone bar).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SegmentInput {
    /// Characters, one per cell.
    #[default]
    Text,
    /// One hexadecimal number per cell, separated by spaces or commas,
    /// each bit a segment lit.
    Masks,
    /// One group of hexadecimal digits per cell, separated by spaces or
    /// commas, each digit the brightness of one segment from 0 (dark) to
    /// `f` (full), the first digit for the segment of bit 0.
    Levels,
}
