//! Checking a show: the problems that refuse it, and the warnings that
//! point at what will quietly do nothing.

use super::*;

/// What is wrong with a set of `offset` keys, if anything: they carry
/// motion added on top of a move, so they run forward from 0 and have to
/// come back to where they started.
fn offset_problem(offset: &[crate::model::Key]) -> Option<&'static str> {
    if offset.windows(2).any(|pair| pair[1].t < pair[0].t)
        || offset.iter().any(|k| k.t.is_nan() || k.t < 0.0)
    {
        return Some("needs offset keys in time order, from 0 on");
    }
    let ends = [offset.first(), offset.last()];
    if ends.iter().flatten().any(|k| k.v != 0.0) {
        return Some("needs an offset that starts and ends at 0");
    }
    None
}

/// Something `load_show` refuses a show for, at the place that would
/// have to go for the show to load.
pub(super) struct Problem {
    pub(super) site: Site,
    pub(super) error: Error,
}

impl Problem {
    pub(super) fn finding(&self) -> Finding {
        Finding {
            path: self.site.path(),
            message: self.error.to_string(),
            kind: FindingKind::Error,
        }
    }
}

/// Everything `load_show` refuses a parsed show for, in the order it
/// checks: the show's own fields, its outputs, its font styles, then
/// every layer in document order, with at most one problem per site.
/// Strict loading fails on the first; tolerant loading drops every site
/// listed and asks again.
pub(super) fn problems(show: &Show) -> Vec<Problem> {
    let mut out = Vec::new();
    if parse_color(&show.background).is_none() {
        out.push(Problem {
            site: Site::Background,
            error: Error::InvalidColor(show.background.clone()),
        });
    }
    let outputs = std::iter::once((None, &show.output)).chain(
        show.scenes
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((Some(i), s.output.as_ref()?))),
    );
    for (scene, output) in outputs {
        if let Some(tint) = output.tint.as_ref().filter(|t| parse_color(t).is_none()) {
            out.push(Problem {
                site: Site::Tint(scene),
                error: Error::InvalidColor(tint.clone()),
            });
        }
        for (i, pass) in output.passes.iter().flatten().enumerate() {
            if let Err(error) = pass_problem(pass) {
                out.push(Problem {
                    site: Site::Pass(scene, i),
                    error,
                });
            }
        }
    }
    validate(show, &mut out);
    out
}

/// What is wrong with a pass, if anything.
fn pass_problem(pass: &Pass) -> Result<(), Error> {
    let Pass::Dots(dots) = pass;
    if !(dots.size > 0.0 && dots.size <= 1.0) {
        return Err(Error::InvalidShow(
            "a dots pass needs a size above 0, up to 1".into(),
        ));
    }
    if !(0.0..=1.0).contains(&dots.glow) {
        return Err(Error::InvalidShow(
            "a dots pass needs a glow from 0 to 1".into(),
        ));
    }
    if let Some(unlit) = &dots.unlit {
        parse_color(unlit).ok_or_else(|| Error::InvalidColor(unlit.clone()))?;
    }
    Ok(())
}

/// Checks `load_show` does beyond parsing: colors parse, text layers use
/// declared font styles, only numeric properties are keyframed. One
/// problem per font style and per layer, the first found.
fn validate(show: &Show, out: &mut Vec<Problem>) {
    for (name, style) in &show.fonts {
        if let Err(error) = font_style_problem(style) {
            out.push(Problem {
                site: Site::Font(name.clone()),
                error,
            });
        }
    }
    // A value's timelines follow the rule a layer's do.
    for (name, value) in &show.values {
        let carried = value.timelines.iter().find(|tl| tl.carry && !tl.looping);
        if let Some(timeline) = carried {
            out.push(Problem {
                site: Site::Value(name.clone()),
                error: Error::InvalidShow(format!(
                    "timeline {:?} of value {name:?} carries, which only a loop does",
                    timeline.name
                )),
            });
        }
    }
    fn layers(
        show: &Show,
        root: Root,
        path: &mut Vec<(usize, bool)>,
        list: &[Layer],
        parts: bool,
        out: &mut Vec<Problem>,
    ) {
        for (i, layer) in list.iter().enumerate() {
            path.push((i, parts));
            let problem = match (&layer.kind, parts) {
                // A part is made from an artwork layer's `parts`, and
                // is nothing anywhere else.
                (LayerKind::Part { .. }, false) => Err(Error::InvalidShow(format!(
                    "layer {:?} is a part, which only the parts of an artwork layer hold",
                    layer.name
                ))),
                _ => layer_problem(show, layer),
            };
            if let Err(error) = problem {
                out.push(Problem {
                    site: Site::Layer(root, path.clone()),
                    error,
                });
            }
            layers(show, root, path, layer.children(), layer.holds_parts(), out);
            path.pop();
        }
    }
    layers(show, Root::Show, &mut Vec::new(), &show.layers, false, out);
    for (i, scene) in show.scenes.iter().enumerate() {
        layers(
            show,
            Root::Scene(i),
            &mut Vec::new(),
            &scene.layers,
            false,
            out,
        );
    }
}

/// What is wrong with a font style, if anything.
fn font_style_problem(style: &crate::model::FontStyle) -> Result<(), Error> {
    for color in std::iter::once(&style.color)
        .chain(style.border.as_ref().map(|b| &b.color))
        .chain(style.shadow.as_ref().map(|s| &s.color))
    {
        parse_color(color).ok_or_else(|| Error::InvalidColor(color.clone()))?;
    }
    if let Some(shadow) = &style.shadow {
        if !shadow.offset.iter().all(|n| n.is_finite()) {
            return Err(Error::InvalidShow(format!(
                "font style {:?} needs a finite shadow offset",
                style.file
            )));
        }
        if !(shadow.blur.is_finite() && shadow.blur >= 0.0) {
            return Err(Error::InvalidShow(format!(
                "font style {:?} needs a shadow blur of 0 or more",
                style.file
            )));
        }
    }
    Ok(())
}

/// What is wrong with a layer itself, if anything: its children are
/// looked at on their own.
fn layer_problem(show: &Show, layer: &Layer) -> Result<(), Error> {
    if let LayerKind::Shape {
        fill: crate::model::Fill::Gradient(gradient),
        ..
    } = &layer.kind
    {
        let stops = gradient.stops();
        let problem = if stops.is_empty() {
            Some("needs a stop".to_owned())
        } else if !stops.iter().all(|s| s.at.is_finite()) {
            Some("needs finite stop positions".to_owned())
        } else if stops.windows(2).any(|w| w[1].at < w[0].at) {
            Some("needs its stops in order".to_owned())
        } else if matches!(
            gradient,
            crate::model::Gradient::Radial { radius, .. } if !(radius.is_finite() && *radius > 0.0)
        ) {
            Some("needs a radius above 0".to_owned())
        } else {
            stops
                .iter()
                .find(|s| parse_color(&s.color).is_none())
                .map(|s| format!("has a stop that is not a color: {:?}", s.color))
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "the gradient of layer {:?} {problem}",
                layer.name
            )));
        }
    }
    if let LayerKind::Shape {
        stroke: Some(stroke),
        ..
    } = &layer.kind
    {
        parse_color(&stroke.color).ok_or_else(|| Error::InvalidColor(stroke.color.clone()))?;
        if !(stroke.width.is_finite() && stroke.width > 0.0) {
            return Err(Error::InvalidShow(format!(
                "layer {:?} needs a stroke width above 0",
                layer.name
            )));
        }
    }
    let font = match &layer.kind {
        LayerKind::Text { font, .. } => Some(font),
        LayerKind::Digits {
            display: DigitDisplay::Reel(reel),
            ..
        } => reel.font.as_ref(),
        _ => None,
    };
    if let Some(font) = font.filter(|font| !show.fonts.contains_key(*font)) {
        return Err(Error::InvalidShow(format!(
            "layer {:?} uses undeclared font style {font:?}",
            layer.name
        )));
    }
    if let LayerKind::Digits {
        display: DigitDisplay::Reel(reel),
        ..
    } = &layer.kind
    {
        let problem = if reel.charset.is_empty() {
            Some("needs a charset with a character in it")
        } else if !(reel.duration.is_finite() && reel.duration > 0.0) {
            Some("needs a duration above 0")
        } else if !reel.stagger.is_finite() || reel.stagger < 0.0 {
            Some("needs a stagger of 0 or more")
        } else if reel.font.is_none() && reel.cells.is_none() {
            Some("needs a font for its characters, or cells to draw instead")
        } else if reel
            .cells
            .as_ref()
            .is_some_and(|cells| cells.len() != reel.charset.chars().count())
        {
            Some("needs one cell for every character of its charset")
        } else if reel.window == 0 {
            Some("needs a window of at least one character")
        } else if reel
            .step
            .is_some_and(|step| !step.is_finite() || step <= 0.0)
        {
            Some("needs a step above 0, or none at all to travel in one move")
        } else {
            offset_problem(&reel.offset)
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "reel of layer {:?} {problem}",
                layer.name
            )));
        }
    }
    // Properties must exist on this kind of layer; only numeric
    // ones can be keyframed.
    let tracks = layer.timelines.iter().flat_map(|tl| &tl.tracks);
    let used = tracks
        .map(|t| t.property)
        .chain(layer.bindings.iter().map(|b| b.property));
    for property in used {
        if layer.base_value(property).is_none() {
            return Err(Error::InvalidShow(format!(
                "layer {:?} has no {property:?} property",
                layer.name
            )));
        }
    }
    for timeline in &layer.timelines {
        if timeline.looping && timeline.repeat.is_some() {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} sets both loop and repeat",
                timeline.name, layer.name
            )));
        }
        if timeline.carry && !timeline.looping {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} carries, which only a loop does",
                timeline.name, layer.name
            )));
        }
        if let Some(track) = timeline.tracks.iter().find(|t| !t.property.is_numeric()) {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} animates {:?}, which can only be bound",
                timeline.name, layer.name, track.property
            )));
        }
        if timeline.when.is_some() && timeline.whilst.is_some() {
            return Err(Error::InvalidShow(format!(
                "timeline {:?} of layer {:?} sets both when and while, which \
                 want different things of the same condition",
                timeline.name, layer.name
            )));
        }
        for (which, condition) in [("when", &timeline.when), ("while", &timeline.whilst)] {
            if let Some(problem) = condition.as_ref().and_then(reading_problem) {
                return Err(Error::InvalidShow(format!(
                    "the {which} of timeline {:?} of layer {:?} {problem}",
                    timeline.name, layer.name
                )));
            }
        }
    }
    if let LayerKind::Audio {
        duck: Some(duck), ..
    }
    | LayerKind::Video {
        duck: Some(duck), ..
    } = &layer.kind
    {
        let finite = |n: f64| n.is_finite() && n >= 0.0;
        let problem = if duck.under.is_empty() {
            Some("needs a bus to listen to")
        } else if !finite(duck.to) {
            Some("needs a gain of 0 or more to duck to")
        } else if !finite(duck.attack) || !finite(duck.release) {
            Some("needs an attack and release of 0 or more")
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "the duck of layer {:?} {problem}",
                layer.name
            )));
        }
    }
    for binding in &layer.bindings {
        if let Some(problem) = reading_problem(&binding.reading) {
            return Err(Error::InvalidShow(format!(
                "the {:?} binding of layer {:?} {problem}",
                binding.property, layer.name
            )));
        }
        if binding.min_digits.is_some_and(|d| d > 20) {
            return Err(Error::InvalidShow(format!(
                "the {:?} binding of layer {:?} asks for more digits than a number has",
                binding.property, layer.name
            )));
        }
        if binding.decimals.is_some_and(|d| d > 15) {
            return Err(Error::InvalidShow(format!(
                "the {:?} binding of layer {:?} asks for more decimals than a number has",
                binding.property, layer.name
            )));
        }
        if let Some(transition) = &binding.transition {
            let positive = |n: f64| n.is_finite() && n > 0.0;
            let ring = transition.wrap.is_some() || transition.direction.is_some();
            let modelled = transition.model.is_some();
            let problem = if matches!(binding.property, Property::Font | Property::Visible) {
                Some("is on a binding that cannot be eased")
            } else if binding.property == Property::Tint && ring {
                Some("sets wrap or direction, which a color has no use for")
            } else if modelled
                && (positive(transition.duration)
                    || ring
                    || transition.step.is_some()
                    || !transition.offset.is_empty()
                    || transition.ease != crate::easing::Easing::default())
            {
                Some("follows a model, which decides its own timing, shape and way round")
            } else if !modelled
                && (transition.kelvin.is_some()
                    || transition.heating.is_some()
                    || transition.cooling.is_some())
            {
                Some("shapes a filament without naming a model to follow")
            } else if modelled
                && [transition.kelvin, transition.heating, transition.cooling]
                    .into_iter()
                    .flatten()
                    .any(|n| !positive(n))
            {
                Some("needs a kelvin, heating and cooling above 0")
            } else if !modelled && !positive(transition.duration) {
                Some("needs a duration above 0")
            } else if transition.wrap.is_some_and(|wrap| !positive(wrap)) {
                Some("needs a wrap above 0")
            } else if transition.direction.is_some() && transition.wrap.is_none() {
                Some("sets a direction, which needs wrap")
            } else if transition.step.is_some_and(|step| !positive(step)) {
                Some("needs a step above 0")
            } else {
                offset_problem(&transition.offset)
            };
            if let Some(problem) = problem {
                return Err(Error::InvalidShow(format!(
                    "transition of the {:?} binding of layer {:?} {problem}",
                    binding.property, layer.name
                )));
            }
        }
        // A modelled tint takes a power level, not a colour: the
        // filament decides what colour that is.
        let lamp = binding
            .transition
            .as_ref()
            .is_some_and(|t| t.model.is_some());
        if binding.property == Property::Tint && !lamp {
            let mapped = binding.reading.map.iter().flat_map(|m| m.values());
            for value in mapped.chain(&binding.reading.default) {
                let color = matches!(value, Value::Text(c) if c.is_empty()
                    || parse_color(c).is_some());
                if !color {
                    return Err(Error::InvalidShow(format!(
                        "tint binding of layer {:?} maps to {value:?}, not a color",
                        layer.name
                    )));
                }
            }
        }
        if binding.property != Property::Font {
            continue;
        }
        let mapped = binding.reading.map.iter().flat_map(|m| m.values());
        for value in mapped.chain(&binding.reading.default) {
            let known = matches!(value, Value::Text(style) if show.fonts.contains_key(style));
            if !known {
                return Err(Error::InvalidShow(format!(
                    "font binding of layer {:?} maps to {value:?}, not a declared font style",
                    layer.name
                )));
            }
        }
    }
    if let Some(press) = &layer.press {
        if press.trigger.is_none() && press.open.is_none() {
            return Err(Error::InvalidShow(format!(
                "the press of layer {:?} neither fires a trigger nor opens a link",
                layer.name
            )));
        }
        if let Some(url) = press.open.as_deref() {
            let web = url.starts_with("http://") || url.starts_with("https://");
            if !web {
                return Err(Error::InvalidShow(format!(
                    "the press of layer {:?} opens {url:?}, which is not an http or https address",
                    layer.name
                )));
            }
        }
    }
    if matches!(
        layer.kind,
        LayerKind::Group { .. } | LayerKind::Audio { .. }
    ) && layer.anchor.is_some()
    {
        return Err(Error::InvalidShow(format!(
            "layer {:?} has an anchor, but no content box",
            layer.name
        )));
    }
    if let LayerKind::Image {
        tint: Some(tint), ..
    } = &layer.kind
    {
        parse_color(tint).ok_or_else(|| Error::InvalidColor(tint.clone()))?;
    }
    if let Some(media) = layer.kind.media() {
        let gain = match &layer.kind {
            LayerKind::Audio { gain, .. } | LayerKind::Video { gain, .. } => *gain,
            _ => 1.0,
        };
        let problem = if media.looping && media.repeat.is_some() {
            Some("sets both loop and repeat")
        } else if !media.delay.is_finite() || media.delay < 0.0 {
            Some("needs a delay of 0 or more")
        } else if media.repeat.is_some_and(|r| !r.is_finite() || r < 0.0) {
            Some("needs a repeat of 0 or more")
        } else if !gain.is_finite() || gain < 0.0 {
            Some("needs a gain of 0 or more")
        } else if media.retrigger == Retrigger::Overlap && media.voices == 0 {
            Some("needs at least one voice to overlap")
        } else if media.retrigger == Retrigger::Overlap && media.kind == MediaKind::Video {
            Some("cannot overlap: a video layer shows one picture at a time")
        } else if media.names.is_empty() {
            Some("names nothing to play")
        } else if !media.rest.is_finite() || media.rest < 0.0 {
            Some("needs a rest of 0 or more")
        } else if media.when.is_some() && media.whilst.is_some() {
            Some("sets both when and while, which want different things of the same condition")
        } else {
            None
        };
        let kind = match media.kind {
            MediaKind::Sound => "audio",
            MediaKind::Video => "video",
        };
        if let Some(problem) = problem {
            return Err(Error::InvalidShow(format!(
                "{kind} layer {:?} {problem}",
                layer.name
            )));
        }
        for (which, condition) in [("when", media.when), ("while", media.whilst)] {
            if let Some(problem) = condition.and_then(reading_problem) {
                return Err(Error::InvalidShow(format!(
                    "the {which} of {kind} layer {:?} {problem}",
                    layer.name
                )));
            }
        }
    }
    Ok(())
}

/// Warn about a segment display whose own text, given as masks or
/// levels, has a cell that is not hexadecimal: it draws dark, which
/// looks like a segment that does not work rather than a typo. Text a
/// binding gives is not the document's, and is not looked at here.
pub(super) fn dark_segment_cells(show: &Show, out: &mut Vec<String>) {
    use crate::model::SegmentInput;
    fn walk(layers: &[Layer], out: &mut Vec<String>) {
        for layer in layers {
            if let LayerKind::Digits {
                text,
                display: DigitDisplay::Segments { input, .. },
                ..
            } = &layer.kind
            {
                let cells = text
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|t| !t.is_empty());
                for cell in cells {
                    let fine = match input {
                        SegmentInput::Masks => {
                            let digits = cell.trim_start_matches("0x").trim_start_matches("0X");
                            u32::from_str_radix(digits, 16).is_ok()
                        }
                        SegmentInput::Levels => cell.chars().all(|c| c.is_ascii_hexdigit()),
                        SegmentInput::Text => true,
                    };
                    if !fine {
                        out.push(format!(
                            "the text of layer {:?} has a cell {cell:?} that is not hexadecimal; \
                             it draws dark",
                            layer.name
                        ));
                    }
                }
            }
            walk(layer.children(), out);
        }
    }
    for layers in show.layer_trees() {
        walk(layers, out);
    }
}

/// Warn about bindings that will quietly do nothing.
///
/// A bound value the engine cannot use leaves the property as it was,
/// silently, the way an unregistered image simply does not draw: the host
/// may send something usable later, and a frame is no place to complain.
/// That is right at runtime and useless while writing a show, where a
/// mistyped variable or a color that is not one looks exactly like a
/// feature that does not work.
///
/// What a show does state up front is which variables it declares and
/// what they start at, so that is what is checked. Values written in the
/// show itself, like the colors and styles a `map` lists, are errors at
/// load instead.
/// With `undeclared`, a binding reading a variable the show does not
/// declare is one of them; the audit says that itself, with the
/// binding's place, and asks for the rest.
pub(crate) fn quiet_bindings(show: &Show, undeclared: bool, out: &mut Vec<String>) {
    fn walk(show: &Show, layers: &[Layer], undeclared: bool, out: &mut Vec<String>) {
        for layer in layers {
            for binding in &layer.bindings {
                // A curve bends a number, and these properties never hold
                // one, so it would quietly do nothing.
                if !binding.reading.curve.is_empty()
                    && matches!(
                        binding.property,
                        Property::Tint | Property::Font | Property::Video | Property::Sound
                    )
                {
                    out.push(format!(
                        "the {:?} binding of layer {:?} has a curve, which only bends a number; \
                         this property never holds one",
                        binding.property, layer.name
                    ));
                }
                // Words only go round text; every other property holds
                // a number or a name of its own.
                if (!binding.prefix.is_empty() || !binding.suffix.is_empty())
                    && binding.property != Property::Text
                {
                    out.push(format!(
                        "the {:?} binding of layer {:?} has words round its value, which only a \
                         text binding shows",
                        binding.property, layer.name
                    ));
                }
                let name = &binding.reading.variable;
                if !show.variables.contains_key(name) && show.values.contains_key(name) {
                    // A value the show animates is declared as much as a
                    // variable is. It is always a number, though, so a
                    // property that cannot take one without a map still
                    // gets nothing.
                    let problem = match binding.property {
                        _ if binding.reading.map.is_some() => None,
                        Property::Tint => Some("a color like \"#RRGGBB\""),
                        Property::Font => Some("one of the show's font styles"),
                        _ => None,
                    };
                    if let Some(wanted) = problem {
                        out.push(format!(
                            "the {:?} binding of layer {:?} reads value {name:?}, which is a \
                             number, not {wanted}; values it cannot use leave the property alone",
                            binding.property, layer.name
                        ));
                    }
                    continue;
                }
                let Some(value) = show.variables.get(name) else {
                    if undeclared {
                        out.push(format!(
                            "the {:?} binding of layer {:?} reads variable {name:?}, which the \
                             show does not declare; it does nothing until a host sets that \
                             variable",
                            binding.property, layer.name
                        ));
                    }
                    continue;
                };
                // With a map it is the mapped values that reach the
                // property, and those are checked at load.
                if binding.reading.map.is_some() {
                    continue;
                }
                // A modelled tint reads a power level, not a colour.
                if binding
                    .transition
                    .as_ref()
                    .is_some_and(|t| t.model.is_some())
                {
                    continue;
                }
                let text = value.to_text();
                let problem = match binding.property {
                    Property::Tint if !text.is_empty() && parse_color(&text).is_none() => {
                        Some("a color like \"#RRGGBB\"")
                    }
                    Property::Font if !show.fonts.contains_key(&text) => {
                        Some("one of the show's font styles")
                    }
                    _ => None,
                };
                if let Some(wanted) = problem {
                    out.push(format!(
                        "the {:?} binding of layer {:?} reads variable {name:?}, which starts at \
                         {text:?}, not {wanted}; values it cannot use leave the property alone",
                        binding.property, layer.name
                    ));
                }
            }
            walk(show, layer.children(), undeclared, out);
        }
    }
    for layers in show.layer_trees() {
        walk(show, layers, undeclared, out);
    }
}

/// The field a name the model still accepts is written back as, so an
/// older spelling is not reported as a field nothing read.
///
/// A document is checked against itself after a round trip through the
/// model, and an alias comes back as the name the model keeps: `vector`
/// returns as `image`, since one artwork layer draws both.
fn also(key: &str) -> Option<&'static str> {
    match key {
        "vector" => Some("image"),
        _ => None,
    }
}

/// Collect the paths of object keys present in `given` but absent from
/// `understood` (the same document after a round trip through the model),
/// which are the fields deserialization silently dropped.
pub(crate) fn ignored_fields(
    given: &serde_json::Value,
    understood: &serde_json::Value,
    path: &str,
    out: &mut Vec<String>,
) {
    use serde_json::Value as Json;
    match (given, understood) {
        (Json::Object(given), Json::Object(understood)) => {
            for (key, value) in given {
                let here = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match understood.get(key).or_else(|| understood.get(also(key)?)) {
                    Some(kept) => ignored_fields(value, kept, &here, out),
                    None if key.starts_with('$') => {}
                    // An explicit null carries no value to lose, and a
                    // field whose value is nothing is written back as
                    // nothing.
                    None if value.is_null() => {}
                    None => out.push(here),
                }
            }
        }
        (Json::Array(given), Json::Array(understood)) => {
            for (i, (value, kept)) in given.iter().zip(understood).enumerate() {
                ignored_fields(value, kept, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// What is wrong with how a variable is read, if anything.
fn reading_problem(reading: &Reading) -> Option<&'static str> {
    if reading.variable.is_empty() {
        return Some("needs a variable");
    }
    if reading.threshold.is_some_and(|t| !t.is_finite()) {
        return Some("needs a finite threshold");
    }
    if reading.debounce.is_some_and(|d| !d.is_finite() || d < 0.0) {
        return Some("needs a debounce of 0 or more");
    }
    if reading.curve.is_empty() {
        return None;
    }
    if reading.threshold.is_some() {
        // A threshold is a curve of two keys written short, so doing
        // both says nothing clear about which happens first.
        return Some("sets both curve and threshold, which are the same job");
    }
    if !reading
        .curve
        .iter()
        .all(|k| k.t.is_finite() && k.v.is_finite())
    {
        return Some("needs finite curve keys");
    }
    if !reading.curve.windows(2).all(|w| w[0].t <= w[1].t) {
        return Some("needs its curve keys in order of input");
    }
    None
}
