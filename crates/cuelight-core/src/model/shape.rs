//! Vector shapes and how they are filled and stroked.

use super::*;

/// Vector shapes, in the layer's local coordinate space.
///
/// Untagged, so a rect can carry a corner radius beside it
/// (`{ "rect": [0, 0, 8, 4], "radius": 2 }`) rather than nesting it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Shape {
    Rect {
        /// `[x, y, width, height]`
        rect: [f64; 4],
        /// Corner radius, clamped to half the shorter side, so a large
        /// one gives a pill. All four corners; absent is square.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
    },
    Circle {
        /// `[cx, cy, radius]`
        circle: [f64; 3],
    },
    Path {
        /// SVG path data (`"M 0 0 L 10 0 L 5 8 Z"`): lines, curves and arcs.
        path: PathData,
    },
}

// Described by hand for the same reason it is read by hand: an untagged
// enum generates `anyOf` with no `additionalProperties`, so an editor
// would not flag a misspelt `raduis`, a `radius` on a circle, or a rect
// and a circle in one object. `oneOf` with each form closed says what
// the loader accepts.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for Shape {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Shape".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        concat!(module_path!(), "::Shape").into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let numbers = |n: usize, description: &str| {
            serde_json::json!({
                "description": description,
                "type": "array",
                "items": { "type": "number", "format": "double" },
                "minItems": n,
                "maxItems": n
            })
        };
        let closed = |name: &str, shape: serde_json::Value, extra: serde_json::Value| {
            let mut properties = serde_json::Map::new();
            properties.insert(name.to_owned(), shape);
            if let serde_json::Value::Object(more) = extra {
                properties.extend(more);
            }
            serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": [name],
                "additionalProperties": false
            })
        };
        let radius = serde_json::json!({
            "radius": {
                "description": "Corner radius, clamped to half the shorter side, so a \
                                large one gives a pill. All four corners; absent is square.",
                "type": ["number", "null"],
                "format": "double"
            }
        });
        let none = serde_json::Value::Null;
        let serde_json::Value::Object(schema) = serde_json::json!({
            "description": "Vector shapes, in the layer's local coordinate space.\n\n\
                            One of rect, circle or path; a rect may carry a corner \
                            radius beside it.",
            "oneOf": [
                closed("rect", numbers(4, "`[x, y, width, height]`"), radius),
                closed("circle", numbers(3, "`[cx, cy, radius]`"), none.clone()),
                closed(
                    "path",
                    serde_json::to_value(generator.subschema_for::<PathData>())
                        .unwrap_or(serde_json::Value::Bool(true)),
                    none,
                ),
            ]
        }) else {
            return schemars::Schema::default();
        };
        schemars::Schema::from(schema)
    }
}

// Read by hand rather than as an untagged enum: serde answers a bad
// shape with "data did not match any variant", which says nothing about
// what is wrong with it. Naming the key first means a malformed path
// still reports what the path parser found.
impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            rect: Option<[f64; 4]>,
            #[serde(default)]
            radius: Option<f64>,
            #[serde(default)]
            circle: Option<[f64; 3]>,
            #[serde(default)]
            path: Option<PathData>,
        }
        let raw = Raw::deserialize(deserializer)?;
        match (raw.rect, raw.circle, raw.path) {
            (Some(rect), None, None) => Ok(Shape::Rect {
                rect,
                radius: raw.radius,
            }),
            (None, Some(circle), None) => Ok(Shape::Circle { circle }),
            (None, None, Some(path)) => Ok(Shape::Path { path }),
            (None, None, None) => Err(serde::de::Error::custom(
                "a shape needs one of rect, circle or path",
            )),
            _ => Err(serde::de::Error::custom(
                "a shape takes one of rect, circle or path, not several",
            )),
        }
    }
}

impl Shape {
    /// A rect's corner radius, clamped to what its size allows.
    pub fn corner_radius(rect: [f64; 4], radius: Option<f64>) -> f64 {
        let [_, _, w, h] = rect;
        radius
            .unwrap_or(0.0)
            .max(0.0)
            .min(w.abs() / 2.0)
            .min(h.abs() / 2.0)
    }
}

/// What a shape is filled with: one color, or a gradient.
///
/// Untagged, so `"fill": "#FF0000"` keeps working and
/// `"fill": { "radial": { ... } }` is the other one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum Fill {
    /// `#RRGGBB` or `#RRGGBBAA`.
    Color(String),
    Gradient(Gradient),
}

impl Default for Fill {
    fn default() -> Self {
        Fill::Color("#FFFFFF".to_owned())
    }
}

/// A smooth run of colors across a shape, in the shape's own space, so it
/// travels with whatever moves the shape.
///
/// Glows, vignettes, the shading that makes a drum look round and the
/// sheen on glass are all gradients, and carrying them as small raster
/// images means the same workaround in every show and blurring whenever
/// one is scaled up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Gradient {
    /// Runs along the line from `from` to `to`, and holds its end colors
    /// beyond either end.
    Linear {
        from: [f64; 2],
        to: [f64; 2],
        stops: Vec<Stop>,
    },
    /// Runs out from `center` to `radius`, and holds its last color
    /// beyond it.
    Radial {
        center: [f64; 2],
        radius: f64,
        stops: Vec<Stop>,
    },
}

impl Gradient {
    pub fn stops(&self) -> &[Stop] {
        match self {
            Gradient::Linear { stops, .. } | Gradient::Radial { stops, .. } => stops,
        }
    }
}

/// One color of a gradient, `at` a fraction of the way along it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Stop {
    /// 0 at the start of the gradient, 1 at its end.
    pub at: f64,
    /// `#RRGGBB` or `#RRGGBBAA`.
    pub color: String,
}

/// An outline drawn along a shape's edge, centered on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Stroke {
    /// `#RRGGBB` or `#RRGGBBAA`
    pub color: String,
    /// Line width in the layer's units, above 0; 1 by default.
    #[serde(default = "default_scale")]
    pub width: f64,
}
