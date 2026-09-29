//! Reel rows: cells that roll through a ring of characters or artwork.

use super::*;

/// Characters a reel carries when its show does not say.
fn default_charset() -> String {
    "0123456789".to_owned()
}

/// One character at a time, the way a wheel that lands on every character
/// travels.
fn default_reel_step() -> Option<f64> {
    Some(1.0)
}

fn default_window() -> u32 {
    1
}

/// A row of cells carrying a ring of symbols, each rolling to the symbol
/// the layer's `text` asks of it, as the wheels of an odometer, a counter
/// or a departure board do. Each cell stands somewhere on the ring of its
/// own, so a change moves only the cells it reaches, and `stagger` keeps
/// them from moving in lockstep.
///
/// A symbol is named by a character of `charset` and drawn either as that
/// character in a font style of the show, which needs no artwork and
/// stays sharp at any size, or as the artwork `cells` gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Reel {
    /// The characters naming the ring's symbols, in the order they pass
    /// by. A cell shows nothing for anything the text asks of it that the
    /// ring does not carry.
    #[serde(default = "default_charset")]
    pub charset: String,
    /// Font style from the show's `fonts` the symbols are drawn in as
    /// their own characters, when `cells` does not put artwork on the ring
    /// instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// What each symbol looks like, one entry per character of `charset`.
    /// Without it a symbol is drawn as its own character, in `font`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<ReelCells>,
    /// How cells given as images are read when drawn at another size;
    /// see [`Sampling`].
    #[serde(default, skip_serializing_if = "is_smooth")]
    pub sampling: Sampling,
    /// Seconds one move takes; above 0.
    pub duration: f64,
    /// How a step progresses; the same easings timelines know.
    #[serde(default)]
    pub ease: Easing,
    /// Which way round the ring a cell travels: `shortest` by default,
    /// `forward` for a wheel that only turns one way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// How far a cell travels in one move, in symbols: one by default, so
    /// it lands on every symbol on the way, as a counter does. `null`
    /// makes the whole journey one move instead, which is how a wheel
    /// spins: `duration` then covers all of it and `ease` shapes the spin
    /// rather than each symbol.
    /// Always written back, `null` included: absent means one symbol at a
    /// time, which is not what `null` means, so dropping it would change
    /// a spinning wheel into a stepping one.
    #[serde(default = "default_reel_step")]
    pub step: Option<f64>,
    /// Extra whole turns of the ring a cell makes before it lands, on top
    /// of the distance to its symbol. 0 by default; a spinning wheel takes
    /// a few.
    #[serde(default)]
    pub turns: u32,
    /// How many symbols of the ring the cell shows at once, stacked with
    /// the one it stands on in the middle. 1 by default; a wheel behind a
    /// tall window shows its neighbours as well.
    #[serde(default = "default_window")]
    pub window: u32,
    /// Motion added on top of a move, in symbols: keys like a binding
    /// transition's `offset`, so a cell can settle against its stop.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offset: Vec<Key>,
    /// Seconds each cell waits behind the one to its right, so a row does
    /// not move as one piece. 0 by default.
    #[serde(default)]
    pub stagger: f64,
    /// Trigger name, or list of names, that sets the row spinning: every
    /// cell travels its `turns` and lands on the symbol its text names at
    /// that moment, whether or not that is the one it already shows.
    #[serde(default)]
    pub spin: Triggers,
}

/// What a reel's symbols are drawn as. The charset stays the ring's
/// identity, so a show still says which symbol a cell lands on by its
/// character; this only says what that symbol looks like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ReelCells {
    /// Vector artwork the host registered, by name, one per symbol.
    /// Artwork scales with the row, so a reel of pictures is as sharp as
    /// one of letters.
    Vectors(Vec<String>),
    /// Images the host registered, by name, one per symbol.
    Images(Vec<String>),
}

impl ReelCells {
    /// How many symbols the artwork covers.
    pub fn len(&self) -> usize {
        match self {
            ReelCells::Vectors(names) | ReelCells::Images(names) => names.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The asset drawn for symbol `index` of the ring.
    pub fn at(&self, index: usize) -> Option<&str> {
        match self {
            ReelCells::Vectors(names) | ReelCells::Images(names) => {
                names.get(index).map(String::as_str)
            }
        }
    }
}

impl Reel {
    /// The characters naming the ring's symbols, in order.
    pub fn characters(&self) -> Vec<char> {
        self.charset.chars().collect()
    }

    /// How many symbols the ring holds.
    pub fn ring(&self) -> f64 {
        self.charset.chars().count().max(1) as f64
    }

    /// How a cell travels from one symbol to another. Symbols are counted
    /// straight, not around the ring, so a journey can be longer than one
    /// turn; where a cell stands is that count folded back onto the ring.
    pub fn roll(&self) -> Transition {
        Transition {
            duration: self.duration,
            model: None,
            kelvin: None,
            heating: None,
            cooling: None,
            ease: self.ease,
            wrap: None,
            direction: None,
            step: self.step,
            offset: self.offset.clone(),
        }
    }

    /// Where a cell standing at `from` travels to show the symbol that
    /// `character` names: the way round `direction` asks for, plus the
    /// turns it takes before landing.
    pub fn travel(&self, from: f64, character: f64) -> f64 {
        let ring = self.ring();
        let forward = (character - from).rem_euclid(ring);
        let step = match self.direction.unwrap_or_default() {
            Direction::Backward => forward - ring,
            Direction::Shortest if forward > ring / 2.0 => forward - ring,
            _ => forward,
        };
        // Turns go the way the cell is already travelling.
        let turns = f64::from(self.turns) * ring;
        from + step + if step < 0.0 { -turns } else { turns }
    }
}
