//! Reel rows: cells that roll through a ring of characters, and spin.

use super::*;

/// The characters of `text` laid into `count` cells, `None` where a cell
/// has nothing to show. Text longer than the row is cut at the far side
/// of `justify`, as a digit row's text is.
pub fn row_cells(text: &str, count: usize, justify: Justify) -> Vec<Option<char>> {
    let characters: Vec<char> = text.chars().collect();
    match justify {
        Justify::Right => {
            let skip = characters.len().saturating_sub(count);
            let mut out = vec![None; count.saturating_sub(characters.len())];
            out.extend(characters.into_iter().skip(skip).map(Some));
            out
        }
        _ => {
            let mut out: Vec<Option<char>> = characters.into_iter().map(Some).collect();
            out.resize(count, None);
            out
        }
    }
}

/// Where the show's reel rows are.
pub(super) fn reel_sites(show: &Show) -> Vec<(Root, Vec<usize>)> {
    fn walk(
        root: Root,
        layers: &[Layer],
        path: &mut Vec<usize>,
        out: &mut Vec<(Root, Vec<usize>)>,
    ) {
        for (i, layer) in layers.iter().enumerate() {
            path.push(i);
            if let LayerKind::Digits {
                display: DigitDisplay::Reel(_),
                ..
            } = &layer.kind
            {
                out.push((root, path.clone()));
            }
            walk(root, layer.children(), path, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(Root::Show, &show.layers, &mut Vec::new(), &mut out);
    for (i, scene) in show.scenes.iter().enumerate() {
        walk(Root::Scene(i), &scene.layers, &mut Vec::new(), &mut out);
    }
    out
}

impl Engine {
    /// Note which character each reel cell is heading for, as of now. A
    /// cell that is already there is left alone, so a change moves only
    /// the cells it reaches, each from wherever it stands.
    pub(super) fn follow_reels(&mut self) {
        let Some(show) = &self.show else { return };
        let mut reels = std::mem::take(&mut self.reels);
        let spinning = std::mem::take(&mut self.spinning);
        for site in &self.reel_sites {
            let (root, path) = site;
            if *root != Root::Show && Some(*root) != self.active_scene.map(Root::Scene) {
                continue;
            }
            let layer = root_layers(show, *root).and_then(|layers| layer_at(layers, path));
            let Some(layer) = layer else { continue };
            let LayerKind::Digits {
                digits,
                justify,
                display: DigitDisplay::Reel(reel),
                ..
            } = &layer.kind
            else {
                continue;
            };
            // Told to spin: every cell sets off, including one already
            // standing where the row is about to land.
            let spin = spinning.contains(site);
            let count = *digits as usize;
            let ring = reel.characters();
            let roll = reel.roll();
            let text = self.text(*root, layer, path, Property::Text);
            let wanted: Vec<Option<f64>> = row_cells(&text, count, *justify)
                .into_iter()
                .map(|c| {
                    let c = c?;
                    ring.iter().position(|on| *on == c).map(|i| i as f64)
                })
                .collect();
            let records = reels.entry(site.clone()).or_insert_with(|| {
                // At load a row stands at what it shows: nothing rolls in.
                wanted
                    .iter()
                    .map(|target| {
                        let target = target.unwrap_or(0.0);
                        Change {
                            start: target,
                            target,
                            started: self.time,
                            whole: false,
                        }
                    })
                    .collect()
            });
            records.resize(
                count,
                Change {
                    start: 0.0,
                    target: 0.0,
                    started: self.time,
                    whole: false,
                },
            );
            let ring_length = reel.ring();
            for (i, character) in wanted.into_iter().enumerate() {
                let Some(character) = character else { continue };
                let Some(change) = records.get_mut(i) else {
                    continue;
                };
                // Where it is heading, as a place on the ring: a cell that
                // is already going there carries on, unless the row was
                // told to spin.
                if !spin && change.target.rem_euclid(ring_length) == character {
                    continue;
                }
                let reached =
                    roll.value_at(change.start, change.target, self.time - change.started);
                // The cell on the right moves first; the rest follow.
                let delay = (count - 1 - i) as f64 * reel.stagger.max(0.0);
                *change = Change {
                    start: reached,
                    target: reel.travel(reached, character),
                    started: self.time + delay,
                    whole: false,
                };
            }
        }
        self.reels = reels;
    }

    /// Note the reel rows whose `spin` trigger is `name`; the next frame
    /// sets their cells off.
    pub(super) fn set_spinning(&mut self, name: &str) {
        let Some(show) = &self.show else { return };
        let mut spinning = Vec::new();
        for site in &self.reel_sites {
            let (root, path) = site;
            if *root != Root::Show && Some(*root) != self.active_scene.map(Root::Scene) {
                continue;
            }
            let reel = root_layers(show, *root)
                .and_then(|layers| layer_at(layers, path))
                .and_then(|layer| match &layer.kind {
                    LayerKind::Digits {
                        display: DigitDisplay::Reel(reel),
                        ..
                    } => Some(reel),
                    _ => None,
                });
            if reel.is_some_and(|reel| reel.spin.contains(name)) && !self.spinning.contains(site) {
                spinning.push(site.clone());
            }
        }
        self.spinning.extend(spinning);
    }

    /// Where the cells of the reel row at `path` stand on their ring now.
    pub fn reel_positions(
        &self,
        root: Root,
        path: &[usize],
        reel: &crate::model::Reel,
    ) -> Vec<f64> {
        let roll = reel.roll();
        self.reels
            .get(&(root, path.to_vec()))
            .map(|records| {
                records
                    .iter()
                    .map(|c| roll.value_at(c.start, c.target, self.time - c.started))
                    .collect()
            })
            .unwrap_or_default()
    }
}
