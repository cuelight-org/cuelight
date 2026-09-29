//! How text looks: font styles, their shadow and border, alignment, and
//! the way a number is written.

use super::*;

/// A text style: a bitmap font the host registered, tinted and optionally
/// outlined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FontStyle {
    /// Name of the font as registered by the host, by convention the font
    /// file's stem: a bitmap font (the `cuelight` crate's `set_font`) or
    /// an outline font (its `set_outline_font`). Which kind it is
    /// decides how the text is drawn; the layers using the style do not
    /// change.
    pub file: String,
    /// Em size in canvas pixels. Required for outline fonts; not allowed
    /// for bitmap fonts, which have one fixed size.
    #[serde(default)]
    pub size: Option<f64>,
    /// Text color, `#RRGGBB`. For bitmap fonts it multiplies the glyph
    /// colors, so white keeps the font's own.
    #[serde(default = "default_font_color")]
    pub color: String,
    #[serde(default)]
    pub border: Option<Border>,
    /// A copy of the text drawn behind it, offset.
    #[serde(default)]
    pub shadow: Option<Shadow>,
    /// Draw an outline font as exact pixels: its glyphs are rasterized
    /// once, at `size`, with hard edges and no antialiasing, and drawn
    /// as a bitmap font's are, never between pixels and never
    /// resampled. For a pixel font shipped as the TTF it was drawn
    /// from, and for any text on a show that is rendered on its own
    /// pixel grid (`pixel_perfect` scaling, a gray output mode), where
    /// this is the default. Not for bitmap fonts, which are pixels
    /// already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixels: Option<bool>,
}

/// A copy of the text drawn behind it, offset by a few pixels: the
/// ordinary way to keep text legible over a moving picture.
///
/// What is drawn is the text's whole silhouette, `border` included, in one
/// color. A shadow of a bordered glyph is therefore the same shape as the
/// glyph, which is what makes it read as a shadow rather than a second
/// outline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Shadow {
    /// `#RRGGBB` or `#RRGGBBAA`
    pub color: String,
    /// How far behind the text it sits, `[x, y]` in canvas pixels. Down
    /// and to the right is positive; both may be negative.
    pub offset: [f64; 2],
    /// How soft the shadow's edge is, in canvas pixels, as CSS
    /// `text-shadow` defines its blur radius: a gaussian whose standard
    /// deviation is half of it, so a value copied from a stylesheet or a
    /// design tool looks the same. 0 (default) is a hard edge; 6 fades out
    /// over about nine pixels. Scales with the layer the way `offset`
    /// does. Drawn for text in a bitmap font, and in an outline font drawn
    /// as pixels.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blur: f64,
}

/// A border of `width` pixels drawn outside every glyph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Border {
    /// `#RRGGBB`
    pub color: String,
    /// Outline width in pixels.
    #[serde(default = "default_border_width")]
    pub width: u32,
}

fn default_font_color() -> String {
    "#FFFFFF".to_owned()
}

fn default_border_width() -> u32 {
    1
}

/// Where content sits in a box: one of nine positions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Align {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Align {
    /// Offset that places a `width` x `height` item in a container.
    pub fn offset(self, width: f64, height: f64, container_w: f64, container_h: f64) -> (f64, f64) {
        use Align::*;
        let x = match self {
            TopLeft | Left | BottomLeft => 0.0,
            Top | Center | Bottom => (container_w - width) / 2.0,
            TopRight | Right | BottomRight => container_w - width,
        };
        let y = match self {
            TopLeft | Top | TopRight => 0.0,
            Left | Center | Right => (container_h - height) / 2.0,
            BottomLeft | Bottom | BottomRight => container_h - height,
        };
        (x, y)
    }

    /// Whether text aligned this way starts at the left edge of its box.
    pub fn is_left(self) -> bool {
        matches!(self, Align::TopLeft | Align::Left | Align::BottomLeft)
    }
}

/// Which end of a digit row its text sits against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Justify {
    #[default]
    Left,
    /// As scores are shown: the last character in the last cell.
    Right,
}

/// How many of `text`'s characters a `reveal` of `share` shows: the
/// share of them, rounded down, so the next character snaps in the
/// instant the share reaches it, the way a typewriter does. Shares
/// outside 0 to 1 show none or all.
pub fn revealed(text: &str, share: f64) -> usize {
    let count = text.chars().count();
    if share.is_nan() || share >= 1.0 {
        return count;
    }
    // A little slack, so a share written as a fraction of the count
    // (0.3 of 10) reaches the character it means.
    ((share.max(0.0) * count as f64 + 1e-9).floor() as usize).min(count)
}

/// Number to text conversion for text bindings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum NumberFormat {
    /// Shortest form: `1500`, `2.5`.
    #[default]
    Plain,
    /// Rounded to an integer with comma thousands separators: `1,500`.
    Thousands,
}

impl NumberFormat {
    /// `n` as text, to `decimals` places when asked for, with at least
    /// `min_digits` digits before the point.
    pub fn format_padded(self, n: f64, decimals: Option<u32>, min_digits: Option<u32>) -> String {
        let text = self.format(n, decimals);
        let Some(min) = min_digits.map(|m| m as usize) else {
            return text;
        };
        // Whatever this is not a plain number of, it stays as it is.
        if text.contains(['e', 'E', 'i', 'N']) {
            return text;
        }
        let (sign, rest) = match text.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", text.as_str()),
        };
        let (whole, fraction) = match rest.find('.') {
            Some(at) => rest.split_at(at),
            None => (rest, ""),
        };
        let digits: String = whole.chars().filter(char::is_ascii_digit).collect();
        if digits.len() >= min {
            return text;
        }
        let digits = format!("{digits:0>min$}");
        let whole = match self {
            NumberFormat::Thousands => Self::group_digits(&digits),
            NumberFormat::Plain => digits,
        };
        format!("{sign}{whole}{fraction}")
    }

    /// `digits` with a comma before every group of three from the right.
    fn group_digits(digits: &str) -> String {
        let mut out = String::new();
        for (i, d) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(d);
        }
        out
    }

    /// `n` as text, to `decimals` places when asked for.
    pub fn format(self, n: f64, decimals: Option<u32>) -> String {
        let Some(places) = decimals else {
            return self.format_short(n);
        };
        let places = places as usize;
        // Rounded to the last shown place first, so the grouping below
        // sees the number that will be printed, and a value that rounds
        // to zero does not keep a minus sign from the way it came.
        let scale = 10f64.powi(places as i32);
        let n = (n * scale).round() / scale;
        let n = if n == 0.0 { 0.0 } else { n };
        match self {
            NumberFormat::Plain => format!("{n:.places$}"),
            NumberFormat::Thousands => {
                let whole = n.abs().trunc();
                let grouped = Self::group(whole as u64);
                let fraction = format!("{:.places$}", n.abs().fract());
                let sign = if n < 0.0 { "-" } else { "" };
                match places {
                    0 => format!("{sign}{grouped}"),
                    // `fraction` is "0.25": everything after its point.
                    _ => format!("{sign}{grouped}{}", &fraction[1..]),
                }
            }
        }
    }

    /// The comma-grouped digits of a whole number.
    fn group(whole: u64) -> String {
        let digits = whole.to_string();
        let mut out = String::new();
        for (i, d) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(d);
        }
        out
    }

    fn format_short(self, n: f64) -> String {
        match self {
            NumberFormat::Plain => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            }
            NumberFormat::Thousands => {
                let digits = (n.round().abs() as u64).to_string();
                let mut out = String::new();
                for (i, d) in digits.chars().enumerate() {
                    if i > 0 && (digits.len() - i).is_multiple_of(3) {
                        out.push(',');
                    }
                    out.push(d);
                }
                if n.round() < 0.0 {
                    out.insert(0, '-');
                }
                out
            }
        }
    }
}
