//! Shapes to SVG, morph shapes to paired outlines, and glyphs to path data.

use std::collections::BTreeMap;
use std::fmt::Write;

use bb_format as f;
use swf::{FillStyle, LineCapStyle, LineJoinStyle, LineStyle, ShapeRecord, ShapeStyles};

use crate::convert;
use crate::paths::{Edge, Layer, Outliner, Pt, Seg, Which, path_data};

/// Half the width of the square Flash defines gradients in, in pixels.
const GRADIENT_EXTENT: f64 = 819.2;

/// Pixel sizes of the bitmaps seen so far, by symbol.
pub type BitmapSizes = BTreeMap<u16, (u32, u32)>;

fn pt(p: swf::Point<swf::Twips>) -> Pt {
    Pt {
        x: p.x.get(),
        y: p.y.get(),
    }
}

/// Reads a shape's records into layers. Also returns the style list each
/// layer after the first switched to.
fn outline(records: &[ShapeRecord]) -> (Vec<Layer>, Vec<&ShapeStyles>) {
    let mut outliner = Outliner::new();
    let mut later_styles = Vec::new();
    let mut at = Pt::ZERO;
    for record in records {
        match record {
            ShapeRecord::StyleChange(change) => {
                if let Some(to) = change.move_to {
                    at = pt(to);
                }
                if let Some(styles) = &change.new_styles {
                    outliner.new_layer();
                    later_styles.push(styles);
                }
                if let Some(style) = change.fill_style_0 {
                    outliner.fill0 = style;
                }
                if let Some(style) = change.fill_style_1 {
                    outliner.fill1 = style;
                }
                if let Some(style) = change.line_style {
                    outliner.line = style;
                }
            }
            edge => {
                let edge = edge_from(at, edge);
                at = edge.to;
                outliner.edge(Seg::plain(edge));
            }
        }
    }
    (outliner.finish(), later_styles)
}

/// The edge an edge record draws when the pen is at `from`.
fn edge_from(from: Pt, record: &ShapeRecord) -> Edge {
    match record {
        ShapeRecord::StraightEdge { delta } => Edge {
            from,
            ctrl: None,
            to: from.offset(delta.dx.get(), delta.dy.get()),
        },
        ShapeRecord::CurvedEdge {
            control_delta,
            anchor_delta,
        } => {
            let ctrl = from.offset(control_delta.dx.get(), control_delta.dy.get());
            Edge {
                from,
                ctrl: Some(ctrl),
                // The anchor is stored relative to the control point.
                to: ctrl.offset(anchor_delta.dx.get(), anchor_delta.dy.get()),
            }
        }
        ShapeRecord::StyleChange(_) => unreachable!("style changes are not edges"),
    }
}

/// Features of a shape that the SVG cannot express exactly.
#[derive(Default)]
pub struct ShapeNotes {
    pub missing_styles: usize,
    pub missing_bitmaps: usize,
    pub non_zero_winding: bool,
}

pub fn shape_svg(shape: &swf::Shape, bitmaps: &BitmapSizes) -> (String, ShapeNotes) {
    let (layers, later_styles) = outline(&shape.shape);
    let mut notes = ShapeNotes {
        non_zero_winding: shape.flags.contains(swf::ShapeFlag::NON_ZERO_WINDING_RULE),
        ..ShapeNotes::default()
    };
    let mut svg = SvgWriter {
        bitmaps,
        defs: String::new(),
        body: String::new(),
        next_id: 0,
        missing_bitmaps: 0,
    };

    let style_lists = std::iter::once(&shape.styles).chain(later_styles);
    for (layer, styles) in layers.iter().zip(style_lists) {
        for (number, contours) in &layer.fills {
            let Some(style) = styles.fill_styles.get(*number as usize - 1) else {
                notes.missing_styles += 1;
                continue;
            };
            let d = path_data(contours, Which::Start, 20, false);
            let paint = svg.paint("fill", style);
            writeln!(svg.body, r#"<path d="{d}"{paint} fill-rule="evenodd"/>"#).unwrap();
        }
        for (number, contours) in &layer.strokes {
            let Some(style) = styles.line_styles.get(*number as usize - 1) else {
                notes.missing_styles += 1;
                continue;
            };
            let d = path_data(contours, Which::Start, 20, false);
            let paint = svg.paint("stroke", style.fill_style());
            let stroke = stroke_attrs(style);
            writeln!(svg.body, r#"<path d="{d}" fill="none"{paint}{stroke}/>"#).unwrap();
        }
    }
    notes.missing_bitmaps = svg.missing_bitmaps;

    let bounds = convert::rect(&shape.shape_bounds);
    // An empty shape has no size, which SVG does not allow.
    let width = (bounds.x_max - bounds.x_min).max(0.05);
    let height = (bounds.y_max - bounds.y_min).max(0.05);
    let mut out = String::new();
    writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {width} {height}" width="{width}" height="{height}">"#,
        bounds.x_min, bounds.y_min
    )
    .unwrap();
    if !svg.defs.is_empty() {
        out.push_str("<defs>\n");
        out.push_str(&svg.defs);
        out.push_str("</defs>\n");
    }
    out.push_str(&svg.body);
    out.push_str("</svg>\n");
    (out, notes)
}

struct SvgWriter<'a> {
    bitmaps: &'a BitmapSizes,
    defs: String,
    body: String,
    next_id: usize,
    missing_bitmaps: usize,
}

impl SvgWriter<'_> {
    /// Returns the attributes that paint `target` ("fill" or "stroke") with
    /// `style`, adding a gradient or pattern definition when one is needed.
    fn paint(&mut self, target: &str, style: &FillStyle) -> String {
        match convert::paint(style) {
            f::Paint::Solid { color } => {
                let mut attrs = format!(
                    r##" {target}="#{:02x}{:02x}{:02x}""##,
                    color.r, color.g, color.b
                );
                if color.a != 255 {
                    write!(attrs, r#" {target}-opacity="{}""#, unit(color.a)).unwrap();
                }
                attrs
            }
            f::Paint::LinearGradient { matrix, stops } => {
                let id = self.new_id("g");
                writeln!(
                    self.defs,
                    r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="-{GRADIENT_EXTENT}" x2="{GRADIENT_EXTENT}" gradientTransform="{}">"#,
                    matrix_attr(&matrix)
                )
                .unwrap();
                self.stops(&stops);
                self.defs.push_str("</linearGradient>\n");
                format!(r#" {target}="url(#{id})""#)
            }
            f::Paint::RadialGradient {
                matrix,
                stops,
                focal,
            } => {
                let id = self.new_id("g");
                writeln!(
                    self.defs,
                    r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="0" cy="0" r="{GRADIENT_EXTENT}" fx="{}" fy="0" gradientTransform="{}">"#,
                    focal * GRADIENT_EXTENT,
                    matrix_attr(&matrix)
                )
                .unwrap();
                self.stops(&stops);
                self.defs.push_str("</radialGradient>\n");
                format!(r#" {target}="url(#{id})""#)
            }
            f::Paint::Bitmap {
                bitmap,
                matrix,
                smoothed,
                ..
            } => {
                let Some(&(width, height)) = self.bitmaps.get(&bitmap) else {
                    self.missing_bitmaps += 1;
                    return format!(r#" {target}="none""#);
                };
                let id = self.new_id("p");
                // SVG only accepts `pixelated` inside a style, not as an
                // attribute of its own.
                let rendering = if smoothed {
                    ""
                } else {
                    r#" style="image-rendering:pixelated""#
                };
                writeln!(
                    self.defs,
                    r#"<pattern id="{id}" patternUnits="userSpaceOnUse" width="{width}" height="{height}" patternTransform="{}"><image href="../bitmaps/{bitmap}.png" width="{width}" height="{height}"{rendering}/></pattern>"#,
                    matrix_attr(&matrix)
                )
                .unwrap();
                format!(r#" {target}="url(#{id})""#)
            }
        }
    }

    fn stops(&mut self, stops: &[f::GradientStop]) {
        for stop in stops {
            let c = stop.color;
            write!(
                self.defs,
                r##"<stop offset="{}" stop-color="#{:02x}{:02x}{:02x}""##,
                round4(stop.offset),
                c.r,
                c.g,
                c.b
            )
            .unwrap();
            if c.a != 255 {
                write!(self.defs, r#" stop-opacity="{}""#, unit(c.a)).unwrap();
            }
            self.defs.push_str("/>\n");
        }
    }

    fn new_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }
}

fn stroke_attrs(style: &LineStyle) -> String {
    // Widths are written as stored, apart from zero, which Flash draws as a
    // hairline and not as nothing. Flash Player also draws every stroke at
    // least one screen pixel wide; that depends on the zoom, so it is left to
    // the renderer.
    let width = convert::px(style.width()).max(0.05);
    let cap = match style.start_cap() {
        LineCapStyle::Round => "round",
        LineCapStyle::None => "butt",
        LineCapStyle::Square => "square",
    };
    let mut attrs = format!(r#" stroke-width="{width}" stroke-linecap="{cap}""#);
    match style.join_style() {
        LineJoinStyle::Round => attrs.push_str(r#" stroke-linejoin="round""#),
        LineJoinStyle::Bevel => attrs.push_str(r#" stroke-linejoin="bevel""#),
        LineJoinStyle::Miter(limit) => write!(
            attrs,
            r#" stroke-linejoin="miter" stroke-miterlimit="{}""#,
            limit.to_f64()
        )
        .unwrap(),
    }
    attrs
}

fn matrix_attr(m: &f::Matrix) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

/// A 0-255 channel as a 0-1 number, short enough to read.
fn unit(channel: u8) -> f64 {
    round4(f64::from(channel) / 255.0)
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Features of a morph shape that were dropped.
#[derive(Default)]
pub struct MorphNotes {
    pub missing_styles: usize,
}

pub fn morph_shape(morph: &swf::DefineMorphShape) -> (f::MorphShape, MorphNotes) {
    let mut outliner = Outliner::new();
    let mut start_at = Pt::ZERO;
    let mut end_at = Pt::ZERO;
    // The two record lists hold the same edges in the same order. Only the
    // start list carries styles; the end list just moves its own pen.
    let mut end_records = morph.end.shape.iter().peekable();
    for record in &morph.start.shape {
        match record {
            ShapeRecord::StyleChange(change) => {
                if let Some(to) = change.move_to {
                    start_at = pt(to);
                }
                if let Some(style) = change.fill_style_0 {
                    outliner.fill0 = style;
                }
                if let Some(style) = change.fill_style_1 {
                    outliner.fill1 = style;
                }
                if let Some(style) = change.line_style {
                    outliner.line = style;
                }
            }
            edge => {
                while let Some(ShapeRecord::StyleChange(change)) = end_records.peek() {
                    if let Some(to) = change.move_to {
                        end_at = pt(to);
                    }
                    end_records.next();
                }
                let start = edge_from(start_at, edge);
                let end = match end_records.next() {
                    Some(end_edge) => edge_from(end_at, end_edge),
                    // The end list ran out: the edge collapses to a point.
                    None => Edge {
                        from: end_at,
                        ctrl: None,
                        to: end_at,
                    },
                };
                start_at = start.to;
                end_at = end.to;
                outliner.edge(Seg { start, end });
            }
        }
    }

    let mut notes = MorphNotes::default();
    let mut paths = Vec::new();
    for layer in outliner.finish() {
        for (number, contours) in &layer.fills {
            let index = *number as usize - 1;
            let (Some(start), Some(end)) = (
                morph.start.fill_styles.get(index),
                morph.end.fill_styles.get(index),
            ) else {
                notes.missing_styles += 1;
                continue;
            };
            paths.push(f::MorphPath {
                start: path_data(contours, Which::Start, 20, true),
                end: path_data(contours, Which::End, 20, true),
                style: f::MorphStyle::Fill {
                    start_paint: convert::paint(start),
                    end_paint: convert::paint(end),
                },
            });
        }
        for (number, contours) in &layer.strokes {
            let index = *number as usize - 1;
            let (Some(start), Some(end)) = (
                morph.start.line_styles.get(index),
                morph.end.line_styles.get(index),
            ) else {
                notes.missing_styles += 1;
                continue;
            };
            paths.push(f::MorphPath {
                start: path_data(contours, Which::Start, 20, true),
                end: path_data(contours, Which::End, 20, true),
                style: f::MorphStyle::Stroke {
                    start_width: convert::px(start.width()),
                    end_width: convert::px(end.width()),
                    start_paint: convert::paint(start.fill_style()),
                    end_paint: convert::paint(end.fill_style()),
                },
            });
        }
    }

    let shape = f::MorphShape {
        id: morph.id,
        start_bounds: convert::rect(&morph.start.shape_bounds),
        end_bounds: convert::rect(&morph.end.shape_bounds),
        paths,
    };
    (shape, notes)
}

/// A glyph's outline as SVG path data, with coordinates divided by
/// `units_per_px`.
pub fn glyph_path(records: &[ShapeRecord], units_per_px: i32) -> String {
    let (layers, _) = outline(records);
    let contours: Vec<_> = layers
        .into_iter()
        .flat_map(|layer| layer.fills)
        .flat_map(|(_, contours)| contours)
        .collect();
    path_data(&contours, Which::Start, units_per_px, false)
}
