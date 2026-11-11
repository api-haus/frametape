//! Annotations authored over a take's frames: containers of marks over a frame range.
//!
//! This is the reader and writer an agent (or the review UI) uses. It is serde only and depends on
//! nothing that draws: the marks are normalized to the frame, so they land the same on any video
//! resolution and a tool can raster them without a window.
//!
//! A container is the unit a person authors: it lives over an inclusive `start_frame..=end_frame`
//! and holds every mark drawn while it was selected. Outside that range it is not shown. A take's
//! sidecar is a flat list of containers.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A point on the frame: `x` and `y` in 0..1, `(0, 0)` the top-left and `(1, 1)` the bottom-right,
/// whichever way up the frame's own pixels are.
pub type Point = [f32; 2];

/// The colour a mark is drawn in, an sRGB triple.
pub type Colour = [u8; 3];

/// The colour every mark falls back to when none is authored.
pub const DEFAULT_COLOUR: Colour = [255, 72, 72];

/// The stroke a shape falls back to when none is authored, as a fraction of the frame's short side.
pub const DEFAULT_WIDTH: f32 = 0.0035;

/// The cap height a label falls back to when none is authored, as a fraction of the frame's short
/// side.
pub const DEFAULT_TEXT_SIZE: f32 = 0.035;

fn default_colour() -> Colour {
    DEFAULT_COLOUR
}

fn default_width() -> f32 {
    DEFAULT_WIDTH
}

fn default_text_size() -> f32 {
    DEFAULT_TEXT_SIZE
}

/// One mark drawn over the frame. A rectangle and an ellipse are the two corners of their box; an
/// arrow runs one way; a stroke is the freehand polyline of a brush drag; text is words at a point.
///
/// Geometry is normalized 0..1 across and down the frame and `width` and `size` are fractions of
/// the frame's short side, so a mark lands the same on any resolution. The ink travels with the
/// mark, so one container can hold two colours.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "mark", rename_all = "snake_case")]
pub enum Mark {
    Rectangle {
        a: Point,
        b: Point,
        #[serde(default = "default_colour")]
        colour: Colour,
        #[serde(default = "default_width")]
        width: f32,
    },
    Ellipse {
        a: Point,
        b: Point,
        #[serde(default = "default_colour")]
        colour: Colour,
        #[serde(default = "default_width")]
        width: f32,
    },
    Arrow {
        from: Point,
        to: Point,
        #[serde(default = "default_colour")]
        colour: Colour,
        #[serde(default = "default_width")]
        width: f32,
    },
    Stroke {
        points: Vec<Point>,
        #[serde(default = "default_colour")]
        colour: Colour,
        #[serde(default = "default_width")]
        width: f32,
    },
    Text {
        at: Point,
        text: String,
        #[serde(default = "default_text_size")]
        size: f32,
        #[serde(default = "default_colour")]
        colour: Colour,
    },
}

impl Mark {
    /// The ink this mark is drawn in.
    pub fn colour(&self) -> Colour {
        match self {
            Mark::Rectangle { colour, .. }
            | Mark::Ellipse { colour, .. }
            | Mark::Arrow { colour, .. }
            | Mark::Stroke { colour, .. }
            | Mark::Text { colour, .. } => *colour,
        }
    }

    /// The stroke this mark is drawn at, zero for text which carries a `size` instead.
    pub fn width(&self) -> f32 {
        match self {
            Mark::Rectangle { width, .. }
            | Mark::Ellipse { width, .. }
            | Mark::Arrow { width, .. }
            | Mark::Stroke { width, .. } => *width,
            Mark::Text { .. } => 0.0,
        }
    }

    /// The points that define the mark, whether as a box's two corners, an arrow's ends, a stroke's
    /// polyline or a label's anchor. A rasterizer walks this and the mark's own kind.
    pub fn points(&self) -> Vec<Point> {
        match self {
            Mark::Rectangle { a, b, .. } | Mark::Ellipse { a, b, .. } => vec![*a, *b],
            Mark::Arrow { from, to, .. } => vec![*from, *to],
            Mark::Stroke { points, .. } => points.clone(),
            Mark::Text { at, .. } => vec![*at],
        }
    }

    /// The point a label is anchored at, or the first point of a shape.
    pub fn anchor(&self) -> Point {
        self.points().first().copied().unwrap_or([0.02, 0.02])
    }

    /// Whether a person would read this mark as still having something to draw.
    pub fn is_empty(&self) -> bool {
        match self {
            Mark::Stroke { points, .. } => points.len() < 2,
            Mark::Text { text, .. } => text.trim().is_empty(),
            Mark::Rectangle { a, b, .. } | Mark::Ellipse { a, b, .. } => {
                (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
            }
            Mark::Arrow { from, to, .. } => {
                (from[0] - to[0]).abs() < 1e-4 && (from[1] - to[1]).abs() < 1e-4
            }
        }
    }

    /// Moves every point by `delta`, for a select-and-drag.
    pub fn translate(&mut self, delta: Point) {
        let shift = |point: &mut Point| {
            point[0] = (point[0] + delta[0]).clamp(0.0, 1.0);
            point[1] = (point[1] + delta[1]).clamp(0.0, 1.0);
        };
        match self {
            Mark::Rectangle { a, b, .. } | Mark::Ellipse { a, b, .. } => {
                shift(a);
                shift(b);
            }
            Mark::Arrow { from, to, .. } => {
                shift(from);
                shift(to);
            }
            Mark::Stroke { points, .. } => points.iter_mut().for_each(shift),
            Mark::Text { at, .. } => shift(at),
        }
    }

    /// The mark the review UI's eraser keeps: the same mark without a `text` that is only spaces.
    pub fn trimmed(mut self) -> Option<Mark> {
        if let Mark::Text { text, .. } = &mut self {
            *text = text.trim().to_owned();
        }
        (!self.is_empty()).then_some(self)
    }
}

/// One annotation container: every mark a person drew while it was selected, over an inclusive
/// range of take frames. Outside the range it is not shown.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Container {
    pub start_frame: u64,
    pub end_frame: u64,
    #[serde(default)]
    pub marks: Vec<Mark>,
}

impl Container {
    /// A container over one frame, ready for the first mark.
    pub fn at(frame: u64) -> Self {
        Self {
            start_frame: frame,
            end_frame: frame,
            marks: Vec::new(),
        }
    }

    /// Whether this container is shown on `frame`.
    pub fn covers(&self, frame: u64) -> bool {
        self.start_frame <= frame && frame <= self.end_frame
    }

    /// The frames the container spans, shortest first.
    pub fn span(&self) -> u64 {
        self.end_frame
            .saturating_sub(self.start_frame)
            .saturating_add(1)
    }

    /// Puts the two ends in order, which a drag handle can invert.
    pub fn normalise(&mut self) {
        if self.end_frame < self.start_frame {
            std::mem::swap(&mut self.start_frame, &mut self.end_frame);
        }
    }

    /// The text of every label mark, in authored order.
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.marks.iter().filter_map(|mark| match mark {
            Mark::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }
}

/// Every annotation container a take carries, in the order they were authored.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Annotations {
    #[serde(default = "schema_version")]
    pub version: u32,
    #[serde(default)]
    pub containers: Vec<Container>,
}

impl Default for Annotations {
    fn default() -> Self {
        Self {
            version: schema_version(),
            containers: Vec::new(),
        }
    }
}

/// The schema this build writes. Version 1 held a flat list of annotations, each its own box; the
/// loader lifts one into one container.
pub fn schema_version() -> u32 {
    2
}

impl Annotations {
    pub fn parse(text: &str) -> Result<Self, String> {
        let annotations = parse(text)?;
        annotations.validate()?;
        Ok(annotations)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != schema_version() {
            return Err(format!("unsupported annotation version: {}", self.version));
        }
        for container in &self.containers {
            if container.end_frame < container.start_frame {
                return Err("annotation frame range is reversed".into());
            }
            for mark in &container.marks {
                if mark
                    .points()
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite())
                {
                    return Err("annotation coordinates must be finite".into());
                }
                let size = match mark {
                    Mark::Text { size, .. } => *size,
                    _ => mark.width(),
                };
                if !size.is_finite() || size <= 0.0 {
                    return Err("annotation stroke/text size must be finite and positive".into());
                }
            }
        }
        Ok(())
    }

    /// Where a take's annotations live: beside it, named after it.
    ///
    /// `recordings/<stem>.jsonl` → `recordings/<stem>.annotations.json`.
    pub fn sidecar(take: &Path) -> PathBuf {
        take.with_extension("annotations.json")
    }

    /// Reads a take's annotations. A take with no sidecar has none, which is not an error, and a
    /// sidecar still in the version 1 shape is lifted into containers.
    pub fn load(take: &Path) -> io::Result<Annotations> {
        let path = Self::sidecar(take);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text)
                .map_err(|error| io::Error::other(format!("{}: {error}", path.display()))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Annotations::default()),
            Err(error) => Err(io::Error::other(format!("{}: {error}", path.display()))),
        }
    }

    /// Writes the annotations beside the take, creating the directory if it is gone.
    pub fn save(&self, take: &Path) -> io::Result<PathBuf> {
        self.validate().map_err(io::Error::other)?;
        let path = Self::sidecar(take);
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|error| io::Error::other(format!("{}: {error}", directory.display())))?;
        }
        crate::atomic_json(&path, self)
            .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
        Ok(path)
    }

    /// The containers shown on `frame`, in authored order.
    pub fn covering(&self, frame: u64) -> impl Iterator<Item = &Container> {
        self.containers
            .iter()
            .filter(move |container| container.covers(frame))
    }

    /// Every mark shown on `frame`, in container then authored order.
    pub fn marks(&self, frame: u64) -> impl Iterator<Item = &Mark> {
        self.covering(frame)
            .flat_map(|container| container.marks.iter())
    }

    /// The frames any container covers, in order and without repeats — what a tool has to render.
    pub fn frames(&self) -> Vec<u64> {
        let mut frames: Vec<u64> = self
            .containers
            .iter()
            .flat_map(|container| container.start_frame..=container.end_frame)
            .collect();
        frames.sort_unstable();
        frames.dedup();
        frames
    }

    pub fn is_empty(&self) -> bool {
        self.containers.is_empty()
    }
}

/// Reads a sidecar of either schema: containers as this build writes them, or the version 1 flat
/// list of annotations, which each become a container holding the shapes and words they carried.
fn parse(text: &str) -> Result<Annotations, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if value.get("containers").is_some() {
        return serde_json::from_value(value).map_err(|error| error.to_string());
    }
    if value.get("annotations").is_some() {
        let legacy: LegacyAnnotations =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        return Ok(legacy.into());
    }
    Err("annotation document has neither containers nor legacy annotations".into())
}

#[derive(Deserialize)]
struct LegacyAnnotations {
    #[serde(default)]
    annotations: Vec<LegacyAnnotation>,
}

#[derive(Deserialize)]
struct LegacyAnnotation {
    start_frame: u64,
    end_frame: u64,
    #[serde(default)]
    text: String,
    #[serde(default = "default_colour")]
    colour: Colour,
    #[serde(default)]
    shapes: Vec<LegacyShape>,
}

#[derive(Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
enum LegacyShape {
    Rectangle { a: Point, b: Point },
    Ellipse { a: Point, b: Point },
    Arrow { from: Point, to: Point },
    Stroke { points: Vec<Point> },
}

impl From<LegacyAnnotations> for Annotations {
    fn from(legacy: LegacyAnnotations) -> Self {
        let containers = legacy
            .annotations
            .into_iter()
            .map(|annotation| {
                let LegacyAnnotation {
                    start_frame,
                    end_frame,
                    text,
                    colour,
                    shapes,
                } = annotation;
                let mut marks: Vec<Mark> = shapes
                    .into_iter()
                    .map(|shape| match shape {
                        LegacyShape::Rectangle { a, b } => Mark::Rectangle {
                            a,
                            b,
                            colour,
                            width: DEFAULT_WIDTH,
                        },
                        LegacyShape::Ellipse { a, b } => Mark::Ellipse {
                            a,
                            b,
                            colour,
                            width: DEFAULT_WIDTH,
                        },
                        LegacyShape::Arrow { from, to } => Mark::Arrow {
                            from,
                            to,
                            colour,
                            width: DEFAULT_WIDTH,
                        },
                        LegacyShape::Stroke { points } => Mark::Stroke {
                            points,
                            colour,
                            width: DEFAULT_WIDTH,
                        },
                    })
                    .collect();
                if !text.is_empty() {
                    let at = marks.first().map_or([0.02, 0.02], |mark| mark.anchor());
                    marks.push(Mark::Text {
                        at,
                        text,
                        size: DEFAULT_TEXT_SIZE,
                        colour,
                    });
                }
                Container {
                    start_frame,
                    end_frame,
                    marks,
                }
            })
            .collect();
        Self {
            version: schema_version(),
            containers,
        }
    }
}
