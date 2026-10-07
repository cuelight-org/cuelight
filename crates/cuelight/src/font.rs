//! Bitmap fonts (AngelCode BMFont text format) and text rasterization.
//!
//! Text is rasterized on the CPU into a small RGBA bitmap, pixel for pixel
//! from the font's page images: glyphs are copied, never resampled, so
//! small pixel fonts stay exact.
//!
//! Scope: bitmap fonts only. Scalable outline fonts (TrueType and friends)
//! would need glyph rasterization or vector outlines fed to the renderer,
//! so they belong beside this module (a `font` directory with a module per
//! kind) rather than inside it, sharing only the layout rules.
//!
//! The parser below reads the text format, which is what our fonts use.
//! Should we need the binary or XML variants, or richer descriptions, it is
//! worth swapping in an existing crate (`bmfont_rs` reads all of them and
//! is MIT/Apache) instead of growing this one; only `BitmapFont::parse`
//! would change.

use cuelight_core::{Align, TextBox};
use std::collections::HashMap;

/// One glyph's placement data from the font description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Glyph {
    /// Where the glyph's pixels sit on its page.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Where those pixels go, from the pen and from the top of the line.
    pub xoffset: i32,
    pub yoffset: i32,
    pub xadvance: i32,
    pub page: usize,
}

/// A parsed BMFont description (`.fnt`, text format). Page images are
/// provided separately, see [`BitmapFont::pages`].
#[derive(Debug, Clone, PartialEq)]
pub struct BitmapFont {
    line_height: i32,
    /// Transparent room inside every glyph's rect, `[up, right, down,
    /// left]` pixels (the `.fnt`'s `info padding`), where a border is
    /// drawn into. Drawn with the glyph, but not ink: a block is
    /// measured, and characters are centred, on what is inside it.
    padding: [i32; 4],
    pages: Vec<String>,
    glyphs: HashMap<char, Glyph>,
    kerning: HashMap<(char, char), i32>,
}

impl BitmapFont {
    /// Parse the BMFont text format.
    pub fn parse(fnt: &str) -> Result<Self, String> {
        let mut line_height = None;
        let mut padding = [0; 4];
        let mut pages: Vec<(usize, String)> = Vec::new();
        let mut glyphs = HashMap::new();
        let mut kerning = HashMap::new();
        for (n, line) in fnt.lines().enumerate() {
            let mut tokens = tokenize(line).into_iter();
            let Some((tag, _)) = tokens.next() else {
                continue;
            };
            let fields: HashMap<String, String> = tokens.collect();
            let int = |key: &str| -> Result<i32, String> {
                fields
                    .get(key)
                    .ok_or_else(|| format!("line {}: missing {key}", n + 1))?
                    .parse()
                    .map_err(|e| format!("line {}: {key}: {e}", n + 1))
            };
            let char_of = |code: i32| {
                u32::try_from(code)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("line {}: invalid character code {code}", n + 1))
            };
            match tag.as_str() {
                "info" => {
                    if let Some(given) = fields.get("padding") {
                        let sides: Vec<i32> = given
                            .split(',')
                            .map(|n| n.trim().parse::<i32>())
                            .collect::<Result<_, _>>()
                            .map_err(|e| format!("line {}: padding: {e}", n + 1))?;
                        padding = <[i32; 4]>::try_from(sides).map_err(|sides| {
                            format!("line {}: padding has {} sides, not 4", n + 1, sides.len())
                        })?;
                    }
                }
                "common" => line_height = Some(int("lineHeight")?),
                "page" => {
                    let file = fields
                        .get("file")
                        .ok_or_else(|| format!("line {}: page without file", n + 1))?;
                    pages.push((int("id")? as usize, file.clone()));
                }
                "char" => {
                    let glyph = Glyph {
                        x: int("x")?,
                        y: int("y")?,
                        width: int("width")?,
                        height: int("height")?,
                        xoffset: int("xoffset")?,
                        yoffset: int("yoffset")?,
                        xadvance: int("xadvance")?,
                        page: int("page").unwrap_or(0) as usize,
                    };
                    glyphs.insert(char_of(int("id")?)?, glyph);
                }
                "kerning" => {
                    let pair = (char_of(int("first")?)?, char_of(int("second")?)?);
                    kerning.insert(pair, int("amount")?);
                }
                _ => {}
            }
        }
        pages.sort();
        if pages.iter().enumerate().any(|(i, (id, _))| *id != i) {
            return Err("page ids are not 0..n".into());
        }
        Ok(Self {
            line_height: line_height.ok_or("missing common lineHeight")?,
            padding,
            pages: pages.into_iter().map(|(_, file)| file).collect(),
            glyphs,
            kerning,
        })
    }

    /// A font made in memory rather than read from a description: one
    /// page, these glyphs with `pad` pixels of room round each, no
    /// kerning. What an outline font rasterized into pixels becomes.
    #[cfg(feature = "outline-fonts")]
    pub(crate) fn from_glyphs(line_height: i32, glyphs: HashMap<char, Glyph>, pad: i32) -> Self {
        Self {
            line_height,
            padding: [pad; 4],
            pages: vec![String::new()],
            glyphs,
            kerning: HashMap::new(),
        }
    }

    /// Page image file names, in page id order, as written in the
    /// description (relative to the `.fnt` file).
    pub fn pages(&self) -> &[String] {
        &self.pages
    }

    pub fn line_height(&self) -> i32 {
        self.line_height
    }

    /// The first and last rows of the line any glyph with ink among
    /// the characters `counted` picks reaches, down from the top of the
    /// line: `(0, line_height)` for a font whose line box is their ink.
    #[cfg(all(test, feature = "outline-fonts"))]
    pub(crate) fn glyph_rows(&self, counted: impl Fn(char) -> bool) -> (i32, i32) {
        let inked = self
            .glyphs
            .iter()
            .filter(|(c, g)| g.height > 0 && counted(**c));
        inked.fold((i32::MAX, i32::MIN), |(top, bottom), (_, g)| {
            (top.min(g.yoffset), bottom.max(g.yoffset + g.height))
        })
    }
}

/// Split a BMFont line into `(key, value)` pairs, the first being the tag
/// (with an empty value). Values may be quoted; `//` starts a comment.
fn tokenize(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = line.trim().chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '=' {
                break;
            }
            key.push(c);
            chars.next();
        }
        if key.starts_with("//") {
            break;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'=') {
            chars.next();
            if chars.peek() == Some(&'"') {
                chars.next();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    value.push(c);
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    value.push(c);
                    chars.next();
                }
            }
        }
        if !key.is_empty() {
            out.push((key, value));
        }
    }
    out
}

/// Straight-alpha RGBA8 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    pub(crate) fn transparent(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    fn get(&self, x: i32, y: i32) -> [u8; 4] {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return [0; 4];
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels
            .get(i..i + 4)
            .and_then(|px| px.try_into().ok())
            .unwrap_or([0; 4])
    }

    pub(crate) fn set(&mut self, x: i32, y: i32, px: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        if let Some(slot) = self.pixels.get_mut(i..i + 4) {
            slot.copy_from_slice(&px);
        }
    }

    /// A silhouette of one colour, its edge softened by a gaussian of blur
    /// radius `blur` as CSS `text-shadow` defines it, a standard deviation
    /// of half the radius: a soft shadow. The picture grows by three
    /// deviations on every side, where the gaussian has all but faded,
    /// which is what the returned padding says, so it is drawn that much
    /// up and to the left of where the hard one would be.
    pub(crate) fn blurred(&self, blur: f64) -> (Rgba, i32) {
        let sigma = blur.max(0.0) / 2.0;
        let pad = (sigma * 3.0).ceil() as i32;
        if pad == 0 {
            return (self.clone(), 0);
        }
        let sigma = sigma.max(0.1);
        let kernel: Vec<f64> = (-pad..=pad)
            .map(|i| (-(f64::from(i) * f64::from(i)) / (2.0 * sigma * sigma)).exp())
            .collect();
        let total: f64 = kernel.iter().sum();
        let kernel: Vec<f64> = kernel.iter().map(|k| k / total).collect();
        let (w, h) = (self.width as i32 + 2 * pad, self.height as i32 + 2 * pad);
        let alpha_at = |x: i32, y: i32| f64::from(self.get(x - pad, y - pad)[3]);
        // The colour of the silhouette: any pixel of it.
        let colour = self
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .find_map(|&[r, g, b, a]| (a > 0).then_some([r, g, b]))
            .unwrap_or([0, 0, 0]);
        // Row by row, so a pixel's place is `y * w + x`.
        let mut across = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                across.push(
                    kernel
                        .iter()
                        .enumerate()
                        .map(|(k, weight)| weight * alpha_at(x + k as i32 - pad, y))
                        .sum::<f64>(),
                );
            }
        }
        let mut out = Rgba::transparent(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let a: f64 = kernel
                    .iter()
                    .enumerate()
                    .map(|(k, weight)| {
                        let yy = y + k as i32 - pad;
                        match (0..h).contains(&yy) {
                            true => across
                                .get((yy * w + x) as usize)
                                .map_or(0.0, |a| weight * a),
                            false => 0.0,
                        }
                    })
                    .sum();
                let a = a.round().clamp(0.0, 255.0) as u8;
                if a > 0 {
                    out.set(x, y, [colour[0], colour[1], colour[2], a]);
                }
            }
        }
        (out, pad)
    }

    /// Composite `src` over the pixel at (x, y), straight alpha.
    fn blend(&mut self, x: i32, y: i32, src: [u8; 4]) {
        let sa = u32::from(src[3]);
        if sa == 0 {
            return;
        }
        if sa == 255 {
            self.set(x, y, src);
            return;
        }
        let dst = self.get(x, y);
        let da = u32::from(dst[3]) * (255 - sa) / 255;
        let out_a = sa + da;
        let mut out = [0u8; 4];
        for ((channel, s), d) in out.iter_mut().zip(src).zip(dst).take(3) {
            *channel = ((u32::from(s) * sa + u32::from(d) * da) / out_a) as u8;
        }
        out[3] = out_a as u8;
        self.set(x, y, out);
    }
}

/// A registered font with its styled page images: glyph colors tinted,
/// optionally surrounded by a border.
#[derive(Debug)]
pub(crate) struct StyledFont {
    font: BitmapFont,
    pages: Vec<Rgba>,
    /// Extra advance per glyph: a border widens every glyph by two
    /// border widths.
    extra_advance: i32,
}

impl StyledFont {
    pub fn new(
        font: &BitmapFont,
        pages: &[Rgba],
        color: [u8; 3],
        border: Option<([u8; 3], u32)>,
    ) -> Self {
        let pages = pages
            .iter()
            .map(|page| {
                let mut out = Rgba::transparent(page.width, page.height);
                if let Some((border_color, width)) = border {
                    // Every pixel within `width` of a glyph pixel (square
                    // neighborhood) takes the border color.
                    let w = width as i32;
                    let [r, g, b] = border_color;
                    for y in 0..page.height as i32 {
                        for x in 0..page.width as i32 {
                            if page.get(x, y)[3] == 0 {
                                continue;
                            }
                            for dy in -w..=w {
                                for dx in -w..=w {
                                    out.set(x + dx, y + dy, [r, g, b, 255]);
                                }
                            }
                        }
                    }
                }
                for y in 0..page.height as i32 {
                    for x in 0..page.width as i32 {
                        let [r, g, b, a] = page.get(x, y);
                        if a == 0 {
                            continue;
                        }
                        let tint = |c: u8, t: u8| (u32::from(c) * u32::from(t) / 255) as u8;
                        out.set(
                            x,
                            y,
                            [tint(r, color[0]), tint(g, color[1]), tint(b, color[2]), a],
                        );
                    }
                }
                out
            })
            .collect();
        Self {
            font: font.clone(),
            pages,
            extra_advance: border.map_or(0, |(_, w)| 2 * w as i32),
        }
    }

    /// The glyph drawn for `c`: the character itself, else its uppercase
    /// form (pixel fonts often only have capitals), else a space.
    fn glyph(&self, c: char) -> Option<&Glyph> {
        self.font
            .glyphs
            .get(&c)
            .or_else(|| {
                c.is_lowercase()
                    .then(|| c.to_uppercase().next())
                    .flatten()
                    .and_then(|u| self.font.glyphs.get(&u))
            })
            .or_else(|| self.font.glyphs.get(&' '))
    }

    fn kerning(&self, previous: char, c: char) -> i32 {
        self.font.kerning.get(&(previous, c)).copied().unwrap_or(0)
    }

    fn line_width(&self, line: &str) -> i32 {
        let mut previous = ' ';
        let mut width = 0;
        for c in line.chars() {
            if let Some(glyph) = self.glyph(c) {
                width += glyph.xadvance + self.extra_advance + self.kerning(previous, c);
            }
            previous = c;
        }
        width
    }

    /// Size of a text block: the widest line; every line one line height
    /// except the last, which is as tall as its tallest glyph when that
    /// exceeds the line height.
    pub fn measure(&self, text: &str) -> (i32, i32) {
        if text.is_empty() {
            return (0, 0);
        }
        let lines: Vec<&str> = split_lines(text);
        let width = lines.iter().map(|l| self.line_width(l)).max().unwrap_or(0);
        // As far as the ink goes, not the room round it.
        let last = lines.last().copied().unwrap_or_default();
        let last_height = last
            .chars()
            .filter_map(|c| self.glyph(c))
            .map(|g| g.yoffset + g.height - self.font.padding[2])
            .fold(self.font.line_height, i32::max);
        let height = (lines.len() as i32 - 1) * self.font.line_height + last_height;
        (width, height)
    }

    /// Lay out and rasterize `text` in a container of `container` size
    /// (the measured block size when `None`), aligned per `align`: the
    /// block within the container and, for multi-line text, each line
    /// within the container's width. Returns the bitmap and its top-left
    /// offset from the container origin (glyphs may overhang the
    /// container).
    /// How high the ink of `characters` reaches and how far it falls,
    /// measured down from the top of the line they are laid out in, with
    /// the border that is drawn around them. `None` when the font has
    /// none of them.
    ///
    /// The line reserves room for descenders whether or not the
    /// characters use any, so digits and capitals sit above the middle of
    /// it. What is drawn is the ink, so a display that wants its
    /// characters centred wants this rather than the line.
    pub fn ink(&self, characters: &[char]) -> Option<(f64, f64)> {
        let border = f64::from(self.extra_advance / 2);
        let [up, _, down, _] = self.font.padding.map(f64::from);
        let (mut top, mut bottom) = (f64::MAX, f64::MIN);
        for glyph in characters.iter().filter_map(|c| self.glyph(*c)) {
            if glyph.width == 0 || glyph.height == 0 {
                continue;
            }
            top = top.min(f64::from(glyph.yoffset) + up - border);
            bottom = bottom.max(f64::from(glyph.yoffset + glyph.height) - down + border);
        }
        (top <= bottom).then_some((top, bottom))
    }

    /// The height of one line, what the block of a single character is.
    pub fn line(&self) -> f64 {
        f64::from(self.font.line_height)
    }

    /// Only the first `shown` characters are drawn (line breaks count);
    /// the rest keep their room in the layout.
    pub fn rasterize(
        &self,
        text: &str,
        container: Option<[f64; 2]>,
        align: Align,
        text_box: TextBox,
        shown: usize,
    ) -> Option<(Rgba, [i32; 2], [f64; 2])> {
        let (block_w, mut block_h) = self.measure(text);
        // Boxed by the cap: from the top of the H on the first line to
        // the bottom of it on the last, and the lines moved up to match.
        let mut start = 0;
        if let (TextBox::Cap, Some((top, bottom))) = (text_box, self.ink(&['H'])) {
            let lines = split_lines(text).len() as i32;
            if !text.is_empty() {
                (start, block_h) = (
                    top as i32,
                    (lines - 1) * self.font.line_height + (bottom - top) as i32,
                );
            }
        }
        let [cw, ch] = container.unwrap_or([f64::from(block_w), f64::from(block_h)]);
        let (bx, by) = align.offset(f64::from(block_w), f64::from(block_h), cw, ch);
        let lines = split_lines(text);
        let per_line = lines.len() > 1 && !align.is_left();

        // Glyph placements relative to the container origin.
        let mut placed: Vec<(i32, i32, &Glyph)> = Vec::new();
        let mut index = 0;
        for (i, line) in lines.iter().enumerate() {
            let x0 = if per_line {
                let (lx, _) = align.offset(f64::from(self.line_width(line)), 0.0, cw, 0.0);
                lx.floor() as i32
            } else {
                bx.floor() as i32
            };
            let y0 = (by + f64::from(i as i32 * self.font.line_height - start)).floor() as i32;
            let mut pen = 0;
            let mut previous = ' ';
            for c in line.chars() {
                if let Some(glyph) = self.glyph(c) {
                    let kern = self.kerning(previous, c);
                    if index < shown {
                        placed.push((x0 + pen + glyph.xoffset + kern, y0 + glyph.yoffset, glyph));
                    }
                    pen += glyph.xadvance + self.extra_advance + kern;
                }
                previous = c;
                index += 1;
            }
            // The break between lines is a character too.
            index += 1;
        }
        let placed: Vec<_> = placed
            .into_iter()
            .filter(|(_, _, g)| g.width > 0 && g.height > 0)
            .collect();
        let min_x = placed.iter().map(|(x, _, _)| *x).min()?;
        let min_y = placed.iter().map(|(_, y, _)| *y).min()?;
        let max_x = placed.iter().map(|(x, _, g)| x + g.width).max()?;
        let max_y = placed.iter().map(|(_, y, g)| y + g.height).max()?;
        let mut out = Rgba::transparent((max_x - min_x) as u32, (max_y - min_y) as u32);
        for (x, y, glyph) in placed {
            let Some(page) = self.pages.get(glyph.page) else {
                continue;
            };
            for gy in 0..glyph.height {
                for gx in 0..glyph.width {
                    let px = page.get(glyph.x + gx, glyph.y + gy);
                    out.blend(x - min_x + gx, y - min_y + gy, px);
                }
            }
        }
        Some((out, [min_x, min_y], [cw, ch]))
    }
}

fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two 2x3 glyphs on a 4x3 page: 'A' solid, 'B' only its left column.
    const FNT: &str = r#"info face="Test Font" size=3
common lineHeight=4 base=3 scaleW=4 scaleH=3 pages=1
page id=0 file="test_0.png"
chars count=3
char id=32 x=0 y=0 width=0 height=0 xoffset=0 yoffset=0 xadvance=2 page=0 chnl=15 //
char id=65 x=0 y=0 width=2 height=3 xoffset=0 yoffset=0 xadvance=3 page=0 chnl=15 // A
char id=66 x=2 y=0 width=2 height=3 xoffset=0 yoffset=1 xadvance=3 page=0 chnl=15 // B
kernings count=1
kerning first=65 second=66 amount=-1
"#;

    fn page() -> Rgba {
        let mut page = Rgba::transparent(4, 3);
        for y in 0..3 {
            page.set(0, y, [255, 255, 255, 255]);
            page.set(1, y, [255, 255, 255, 255]);
            page.set(2, y, [255, 255, 255, 255]);
        }
        page
    }

    fn styled(border: Option<([u8; 3], u32)>) -> StyledFont {
        let font = BitmapFont::parse(FNT).unwrap();
        StyledFont::new(&font, &[page()], [255, 0, 0], border)
    }

    #[test]
    fn parses_bmfont_text_format() {
        let font = BitmapFont::parse(FNT).unwrap();
        assert_eq!(font.line_height(), 4);
        assert_eq!(font.pages(), ["test_0.png"]);
        assert_eq!(font.glyphs.len(), 3);
        assert_eq!(font.glyphs[&'B'].yoffset, 1);
        assert_eq!(font.kerning[&('A', 'B')], -1);
    }

    #[test]
    fn tokenizer_handles_quotes_and_comments() {
        assert_eq!(
            tokenize(r#"info face="BM army" size=-12 // = comment "x"#),
            [
                ("info".into(), "".into()),
                ("face".into(), "BM army".into()),
                ("size".into(), "-12".into())
            ]
        );
    }

    #[test]
    fn measure_uses_advance_kerning_and_line_height() {
        let font = styled(None);
        // A (3) + B (3 - 1 kerning)
        assert_eq!(font.measure("AB"), (5, 4));
        // last line: B is 3 tall at yoffset 1 -> 4, the line height
        assert_eq!(font.measure("A\nB"), (3, 8));
        assert_eq!(font.measure(""), (0, 0));
    }

    /// The same font with a pixel of padding declared inside every
    /// glyph rect: what a border is drawn into, and not ink.
    /// The deviation is half the blur, as CSS defines it: a lone dot
    /// blurred by 2 falls to the gaussian's value at one deviation one
    /// pixel out, about 0.61 of its centre.
    #[test]
    fn a_blur_is_a_deviation_of_half_its_value() {
        let mut dot = Rgba::transparent(1, 1);
        dot.set(0, 0, [0, 0, 0, 255]);
        let (soft, pad) = dot.blurred(2.0);
        assert_eq!(pad, 3);
        let alpha = |x: i32, y: i32| f64::from(soft.get(x, y)[3]);
        let ratio = alpha(pad + 1, pad) / alpha(pad, pad);
        assert!((ratio - (-0.5f64).exp()).abs() < 0.02, "{ratio}");
    }

    #[test]
    fn padding_inside_a_glyph_rect_is_not_measured() {
        let fnt = FNT.replace(
            "info face=\"Test Font\" size=3",
            "info face=\"Test Font\" size=3 padding=1,1,1,1",
        );
        let font = BitmapFont::parse(&fnt).unwrap();
        assert_eq!(font.padding, [1, 1, 1, 1]);
        let page = Rgba::transparent(4, 3);
        let padded = StyledFont::new(&font, &[page], [255; 3], None);
        // B's rect ends at row 4, but its ink at row 3: the line holds it.
        assert_eq!(padded.measure("B"), (3, 4));
        // The ink of A runs from row 1 to row 2 inside its rect.
        assert_eq!(padded.ink(&['A']), Some((1.0, 2.0)));
        let unpadded = styled(None);
        assert_eq!(unpadded.measure("B"), (3, 4));
        assert_eq!(unpadded.ink(&['A']), Some((0.0, 3.0)));
        assert!(BitmapFont::parse(&FNT.replace("size=3", "size=3 padding=1,2")).is_err());
    }

    #[test]
    fn missing_lowercase_falls_back_to_uppercase() {
        let font = styled(None);
        assert_eq!(font.measure("a"), font.measure("A"));
        // Kerning looks at the characters as written.
        assert_eq!(font.measure("ab"), (6, 4));
    }

    #[test]
    fn rasterizes_tinted_glyphs() {
        let font = styled(None);
        let (bitmap, offset, container) = font
            .rasterize("A", None, Align::TopLeft, TextBox::Line, usize::MAX)
            .unwrap();
        assert_eq!((bitmap.width, bitmap.height), (2, 3));
        assert_eq!(offset, [0, 0]);
        assert_eq!(container, [3.0, 4.0]);
        assert_eq!(bitmap.get(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn border_surrounds_glyphs_and_widens_advance() {
        let font = styled(Some(([0, 0, 255], 1)));
        assert_eq!(font.measure("A"), (5, 4));
        // Only the glyph rectangle is copied, so the border shows where
        // the page leaves room inside it (BMFont padding); here 'B''s
        // rectangle includes the border pixels around 'A''s column 1.
        let (bitmap, _, _) = font
            .rasterize("B", None, Align::TopLeft, TextBox::Line, usize::MAX)
            .unwrap();
        assert_eq!(bitmap.get(0, 0), [255, 0, 0, 255]);
        assert_eq!(bitmap.get(1, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn aligns_block_and_lines_in_container() {
        let font = styled(None);
        let (_, offset, container) = font
            .rasterize(
                "A",
                Some([9.0, 8.0]),
                Align::Center,
                TextBox::Line,
                usize::MAX,
            )
            .unwrap();
        assert_eq!(container, [9.0, 8.0]);
        // block 3x4 centered in 9x8
        assert_eq!(offset, [3, 2]);
        let (_, offset, _) = font
            .rasterize(
                "A",
                Some([9.0, 8.0]),
                Align::BottomRight,
                TextBox::Line,
                usize::MAX,
            )
            .unwrap();
        assert_eq!(offset, [6, 4]);
        // Multi-line, centered: the narrow second line centers on its own.
        let (bitmap, offset, _) = font
            .rasterize("AA\nA", None, Align::Center, TextBox::Line, usize::MAX)
            .unwrap();
        assert_eq!(offset, [0, 0]);
        // line widths 6 and 3: the second line starts at floor(1.5)
        assert_eq!(bitmap.get(0, 4), [0, 0, 0, 0]);
        assert_eq!(bitmap.get(1, 4), [255, 0, 0, 255]);
    }
}
