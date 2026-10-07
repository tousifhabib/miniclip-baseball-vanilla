//! Turns vector art into triangles: SVG shapes, text and morph shapes.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use bb_format as f;
use bytemuck::{Pod, Zeroable};
use lyon::math::point;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, LineCap, LineJoin,
    StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};

use crate::library::Library;
use crate::math::Matrix;

/// How far, in pixels at normal size, a flattened curve may stray from the
/// true curve.
const TOLERANCE: f32 = 0.02;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    /// For a stroke, the point on the centre line this vertex belongs to.
    pub position: [f32; 2],
    /// For a stroke, the direction to move out from the centre line; one
    /// unit of it is half the stroke's width. Zero for fills.
    pub normal: [f32; 2],
    /// Half the stroke's width. Zero for fills.
    pub half_width: f32,
    /// Used by solid paint. Not multiplied by alpha.
    pub color: [u8; 4],
}

/// What colours a run of triangles. Each matrix maps the shape's own
/// coordinates into the paint's.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Every vertex carries its own colour.
    Solid,
    /// The colour is ramp `ramp` at position x.
    Linear {
        ramp: usize,
        matrix: Matrix,
        spread: Spread,
    },
    /// The colour is ramp `ramp` at the distance from the origin.
    Radial {
        ramp: usize,
        matrix: Matrix,
        spread: Spread,
    },
    /// The colour is image `image` at (x, y), each from 0 to 1.
    Image {
        image: usize,
        matrix: Matrix,
        smooth: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    Pad,
    Reflect,
    Repeat,
}

#[derive(Clone, Debug)]
pub struct Draw {
    pub indices: Range<u32>,
    pub paint: Paint,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// In drawing order.
    pub draws: Vec<Draw>,
}

/// 256 colours along a gradient, not multiplied by alpha.
pub type Ramp = [[u8; 4]; 256];

/// Pixels in RGBA order, not multiplied by alpha.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Builds meshes, and collects the gradient ramps and images they refer to.
#[derive(Default)]
pub struct Tessellator {
    pub ramps: Vec<Ramp>,
    pub images: Vec<Image>,
    ramp_rows: HashMap<Ramp, usize>,
    image_slots: HashMap<u64, usize>,
}

/// A mesh under construction.
struct Builder {
    buffers: VertexBuffers<Vertex, u32>,
    draws: Vec<Draw>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            buffers: VertexBuffers::new(),
            draws: Vec::new(),
        }
    }

    fn fill(&mut self, path: &lyon::path::Path, color: [u8; 4], paint: Paint) -> Result<()> {
        let start = self.buffers.indices.len() as u32;
        let options = FillOptions::tolerance(TOLERANCE).with_fill_rule(FillRule::EvenOdd);
        FillTessellator::new()
            .tessellate_path(
                path,
                &options,
                &mut BuffersBuilder::new(&mut self.buffers, |vertex: FillVertex| Vertex {
                    position: vertex.position().to_array(),
                    normal: [0.0, 0.0],
                    half_width: 0.0,
                    color,
                }),
            )
            .map_err(|error| anyhow!("tessellating a fill: {error:?}"))?;
        self.finish_draw(start, paint);
        Ok(())
    }

    fn stroke(
        &mut self,
        path: &lyon::path::Path,
        options: &StrokeOptions,
        color: [u8; 4],
        paint: Paint,
    ) -> Result<()> {
        let start = self.buffers.indices.len() as u32;
        let half_width = options.line_width / 2.0;
        StrokeTessellator::new()
            .tessellate_path(
                path,
                options,
                &mut BuffersBuilder::new(&mut self.buffers, |vertex: StrokeVertex| Vertex {
                    position: vertex.position_on_path().to_array(),
                    normal: vertex.normal().to_array(),
                    half_width,
                    color,
                }),
            )
            .map_err(|error| anyhow!("tessellating a stroke: {error:?}"))?;
        self.finish_draw(start, paint);
        Ok(())
    }

    fn finish_draw(&mut self, start: u32, paint: Paint) {
        let end = self.buffers.indices.len() as u32;
        if end == start {
            return;
        }
        // Solid paint reads its colour from the vertices, so neighbouring
        // solid runs can go out in one draw.
        if paint == Paint::Solid
            && let Some(last) = self.draws.last_mut()
            && last.paint == Paint::Solid
            && last.indices.end == start
        {
            last.indices.end = end;
            return;
        }
        self.draws.push(Draw {
            indices: start..end,
            paint,
        });
    }

    fn build(self) -> Mesh {
        Mesh {
            vertices: self.buffers.vertices,
            indices: self.buffers.indices,
            draws: self.draws,
        }
    }
}

impl Tessellator {
    /// Tessellates an SVG shape. `origin` is where the SVG's top-left corner
    /// sits in the shape's own coordinates.
    pub fn svg(&mut self, path: &Path, origin: (f32, f32)) -> Result<Mesh> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let options = usvg::Options {
            // So that bitmap fills can load the images they point at.
            resources_dir: path.parent().map(Path::to_path_buf),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(&data, &options)
            .with_context(|| format!("parsing {}", path.display()))?;
        let mut builder = Builder::new();
        self.svg_group(tree.root(), origin, &mut builder)
            .with_context(|| format!("tessellating {}", path.display()))?;
        Ok(builder.build())
    }

    fn svg_group(
        &mut self,
        group: &usvg::Group,
        origin: (f32, f32),
        builder: &mut Builder,
    ) -> Result<()> {
        for node in group.children() {
            match node {
                usvg::Node::Group(group) => self.svg_group(group, origin, builder)?,
                usvg::Node::Path(path) if path.is_visible() => {
                    self.svg_path(path, origin, builder)?;
                }
                // Images and text outside a fill are not something the
                // extractor writes.
                _ => {}
            }
        }
        Ok(())
    }

    fn svg_path(
        &mut self,
        path: &usvg::Path,
        origin: (f32, f32),
        builder: &mut Builder,
    ) -> Result<()> {
        let t = path.abs_transform();
        // From the path's own coordinates to the shape's.
        let to_shape = Matrix::translate(origin.0, origin.1).then_inner(Matrix {
            a: t.sx,
            b: t.ky,
            c: t.kx,
            d: t.sy,
            tx: t.tx,
            ty: t.ty,
        });
        let Some(from_shape) = to_shape.inverse() else {
            return Ok(());
        };
        let outline = lyon_path(
            path.data().segments().map(|segment| {
                use usvg::tiny_skia_path::PathSegment as S;
                match segment {
                    S::MoveTo(p) => Segment::Move(p.x, p.y),
                    S::LineTo(p) => Segment::Line(p.x, p.y),
                    S::QuadTo(c, p) => Segment::Quad(c.x, c.y, p.x, p.y),
                    S::CubicTo(c1, c2, p) => Segment::Cubic(c1.x, c1.y, c2.x, c2.y, p.x, p.y),
                    S::Close => Segment::Close,
                }
            }),
            to_shape,
        );

        if let Some(fill) = path.fill() {
            let (color, paint) = self.svg_paint(fill.paint(), fill.opacity().get(), from_shape)?;
            builder.fill(&outline, color, paint)?;
        }
        if let Some(stroke) = path.stroke() {
            let (color, paint) =
                self.svg_paint(stroke.paint(), stroke.opacity().get(), from_shape)?;
            let options = StrokeOptions::tolerance(TOLERANCE)
                .with_line_width(stroke.width().get())
                .with_line_cap(match stroke.linecap() {
                    usvg::LineCap::Butt => LineCap::Butt,
                    usvg::LineCap::Round => LineCap::Round,
                    usvg::LineCap::Square => LineCap::Square,
                })
                .with_line_join(match stroke.linejoin() {
                    usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter,
                    usvg::LineJoin::Round => LineJoin::Round,
                    usvg::LineJoin::Bevel => LineJoin::Bevel,
                })
                .with_miter_limit(stroke.miterlimit().get().max(1.0));
            builder.stroke(&outline, &options, color, paint)?;
        }
        Ok(())
    }

    /// Converts an SVG paint. Returns the vertex colour and the paint.
    fn svg_paint(
        &mut self,
        paint: &usvg::Paint,
        opacity: f32,
        from_shape: Matrix,
    ) -> Result<([u8; 4], Paint)> {
        const WHITE: [u8; 4] = [255; 4];
        let alpha = |opacity: f32| (opacity * 255.0).round().clamp(0.0, 255.0) as u8;
        let spread = |method: usvg::SpreadMethod| match method {
            usvg::SpreadMethod::Pad => Spread::Pad,
            usvg::SpreadMethod::Reflect => Spread::Reflect,
            usvg::SpreadMethod::Repeat => Spread::Repeat,
        };
        let stops = |stops: &[usvg::Stop]| -> Vec<f::GradientStop> {
            stops
                .iter()
                .map(|stop| f::GradientStop {
                    offset: f64::from(stop.offset().get()),
                    color: f::Color {
                        r: stop.color().red,
                        g: stop.color().green,
                        b: stop.color().blue,
                        a: alpha(stop.opacity().get() * opacity),
                    },
                })
                .collect()
        };
        // From the shape's coordinates into a gradient's or pattern's own.
        let into = |transform: usvg::Transform| {
            Matrix {
                a: transform.sx,
                b: transform.ky,
                c: transform.kx,
                d: transform.sy,
                tx: transform.tx,
                ty: transform.ty,
            }
            .inverse()
            .map(|inverse| inverse.then_inner(from_shape))
        };

        Ok(match paint {
            usvg::Paint::Color(color) => (
                [color.red, color.green, color.blue, alpha(opacity)],
                Paint::Solid,
            ),
            usvg::Paint::LinearGradient(gradient) => {
                let Some(matrix) = into(gradient.transform()) else {
                    return Ok(([0; 4], Paint::Solid));
                };
                let (dx, dy) = (gradient.x2() - gradient.x1(), gradient.y2() - gradient.y1());
                let length_squared = (dx * dx + dy * dy).max(1e-12);
                // Position along the line from the first point to the second.
                let along = Matrix {
                    a: dx / length_squared,
                    b: 0.0,
                    c: dy / length_squared,
                    d: 0.0,
                    tx: -(dx * gradient.x1() + dy * gradient.y1()) / length_squared,
                    ty: 0.0,
                };
                let paint = Paint::Linear {
                    ramp: self.ramp(&stops(gradient.stops())),
                    matrix: along.then_inner(matrix),
                    spread: spread(gradient.spread_method()),
                };
                (WHITE, paint)
            }
            usvg::Paint::RadialGradient(gradient) => {
                let Some(matrix) = into(gradient.transform()) else {
                    return Ok(([0; 4], Paint::Solid));
                };
                let r = gradient.r().get().max(1e-6);
                // Distance from the centre, in radii.
                let from_centre = Matrix::scale(1.0 / r, 1.0 / r)
                    .then_inner(Matrix::translate(-gradient.cx(), -gradient.cy()));
                let paint = Paint::Radial {
                    ramp: self.ramp(&stops(gradient.stops())),
                    matrix: from_centre.then_inner(matrix),
                    spread: spread(gradient.spread_method()),
                };
                (WHITE, paint)
            }
            usvg::Paint::Pattern(pattern) => {
                let Some(image) = first_image(pattern.root()) else {
                    bail!("a pattern fill has no image in it");
                };
                let Some(matrix) = into(pattern.transform()) else {
                    return Ok(([0; 4], Paint::Solid));
                };
                let size = image.size();
                let to_unit = Matrix::scale(1.0 / size.width(), 1.0 / size.height());
                let smooth = !matches!(
                    image.rendering_mode(),
                    usvg::ImageRendering::OptimizeSpeed
                        | usvg::ImageRendering::CrispEdges
                        | usvg::ImageRendering::Pixelated
                );
                let paint = Paint::Image {
                    image: self.image(image.kind())?,
                    matrix: to_unit.then_inner(matrix),
                    smooth,
                };
                ([255, 255, 255, alpha(opacity)], paint)
            }
        })
    }

    /// Tessellates fixed text.
    pub fn text(&mut self, text: &f::Text, library: &Library) -> Result<Mesh> {
        let mut builder = Builder::new();
        let text_matrix = Matrix::from(text.matrix);
        for run in &text.runs {
            let Some(font) = library.fonts.get(&run.font) else {
                continue;
            };
            let scale = (run.height / font.em_size) as f32;
            let color = [run.color.r, run.color.g, run.color.b, run.color.a];
            let mut pen = run.x as f32;
            for placement in &run.glyphs {
                if let Some(glyph) = font.glyphs.get(placement.glyph as usize) {
                    let place = text_matrix
                        .then_inner(Matrix::translate(pen, run.y as f32))
                        .then_inner(Matrix::scale(scale, scale));
                    let outline = lyon_path(parse_path(&glyph.path)?.into_iter(), place);
                    builder.fill(&outline, color, Paint::Solid)?;
                }
                pen += placement.advance as f32;
            }
        }
        Ok(builder.build())
    }

    /// Tessellates a text field showing the text it starts with.
    pub fn edit_text(&mut self, text: &f::EditText, library: &Library) -> Result<Mesh> {
        let mut builder = Builder::new();
        let (Some(font), Some(height), Some(content)) = (
            text.font.and_then(|font| library.fonts.get(&font)),
            text.height,
            text.initial_text.as_deref(),
        ) else {
            return Ok(builder.build());
        };
        // Fields that hold markup need a parser the engine does not have yet.
        if content.is_empty() || text.flags.iter().any(|flag| flag == "html") {
            return Ok(builder.build());
        }

        let scale = (height / font.em_size) as f32;
        let color = text.color.map_or([0, 0, 0, 255], |c| [c.r, c.g, c.b, c.a]);
        let glyph_for = |c: char| font.glyphs.iter().find(|glyph| glyph.char.starts_with(c));
        let (left, right, indent, leading, align) = match &text.layout {
            Some(layout) => (
                layout.left_margin as f32,
                layout.right_margin as f32,
                layout.indent as f32,
                layout.leading as f32,
                layout.align.as_str(),
            ),
            None => (0.0, 0.0, 0.0, 0.0, "left"),
        };
        // Flash keeps a two pixel gutter inside a field's edges.
        const GUTTER: f32 = 2.0;
        let inner_left = text.bounds.x_min as f32 + GUTTER + left;
        let inner_right = text.bounds.x_max as f32 - GUTTER - right;
        let (ascent, descent) = match &font.metrics {
            Some(metrics) => (
                metrics.ascent as f32 * scale,
                metrics.descent as f32 * scale,
            ),
            // A typical split for a font that does not say.
            None => (height as f32 * 0.8, height as f32 * 0.2),
        };

        let mut baseline = text.bounds.y_min as f32 + GUTTER + ascent;
        for line in content.split(['\n', '\r']) {
            let width: f32 = line
                .chars()
                .filter_map(glyph_for)
                .map(|glyph| glyph.advance as f32 * scale)
                .sum();
            let mut pen = match align {
                "right" => inner_right - width,
                "center" => (inner_left + inner_right - width) / 2.0,
                _ => inner_left + indent,
            };
            for glyph in line.chars().filter_map(glyph_for) {
                let place =
                    Matrix::translate(pen, baseline).then_inner(Matrix::scale(scale, scale));
                let outline = lyon_path(parse_path(&glyph.path)?.into_iter(), place);
                builder.fill(&outline, color, Paint::Solid)?;
                pen += glyph.advance as f32 * scale;
            }
            baseline += ascent + descent + leading;
        }
        Ok(builder.build())
    }

    /// Tessellates a morph shape part of the way through its blend, where
    /// `ratio` runs from 0 (the start) to 65535 (the end).
    pub fn morph(&mut self, morph: &f::MorphShape, ratio: u16) -> Result<Mesh> {
        let t = f32::from(ratio) / 65535.0;
        let mut builder = Builder::new();
        for path in &morph.paths {
            let start = parse_path(&path.start)?;
            let end = parse_path(&path.end)?;
            if start.len() != end.len() {
                bail!(
                    "morph shape {}: a path's two ends differ in length",
                    morph.id
                );
            }
            let blended = start.iter().zip(&end).map(|(a, b)| a.blend(b, t));
            let outline = lyon_path(blended, Matrix::IDENTITY);
            match &path.style {
                f::MorphStyle::Fill {
                    start_paint,
                    end_paint,
                } => {
                    let (color, paint) = self.blended_paint(start_paint, end_paint, t)?;
                    builder.fill(&outline, color, paint)?;
                }
                f::MorphStyle::Stroke {
                    start_width,
                    end_width,
                    start_paint,
                    end_paint,
                } => {
                    let width = lerp(*start_width as f32, *end_width as f32, t);
                    let options = StrokeOptions::tolerance(TOLERANCE)
                        .with_line_width(width)
                        .with_line_cap(LineCap::Round)
                        .with_line_join(LineJoin::Round);
                    let (color, paint) = self.blended_paint(start_paint, end_paint, t)?;
                    builder.stroke(&outline, &options, color, paint)?;
                }
            }
        }
        Ok(builder.build())
    }

    /// A paint part of the way between two paints of the same kind.
    fn blended_paint(&mut self, a: &f::Paint, b: &f::Paint, t: f32) -> Result<([u8; 4], Paint)> {
        const WHITE: [u8; 4] = [255; 4];
        let blend_matrix = |a: &f::Matrix, b: &f::Matrix| {
            let mut out = [0.0; 6];
            for i in 0..6 {
                out[i] = a[i] + (b[i] - a[i]) * f64::from(t);
            }
            Matrix::from(out)
        };
        let blend_stops = |a: &[f::GradientStop], b: &[f::GradientStop]| -> Vec<f::GradientStop> {
            a.iter()
                .zip(b)
                .map(|(a, b)| f::GradientStop {
                    offset: a.offset + (b.offset - a.offset) * f64::from(t),
                    color: blend_color(a.color, b.color, t),
                })
                .collect()
        };
        // A gradient is defined on a square 1638.4 pixels wide, centred on
        // the origin, before its matrix is applied.
        const EXTENT: f32 = 819.2;
        Ok(match (a, b) {
            (f::Paint::Solid { color: a }, f::Paint::Solid { color: b }) => {
                let c = blend_color(*a, *b, t);
                ([c.r, c.g, c.b, c.a], Paint::Solid)
            }
            (
                f::Paint::LinearGradient {
                    matrix: ma,
                    stops: sa,
                },
                f::Paint::LinearGradient {
                    matrix: mb,
                    stops: sb,
                },
            ) => {
                let Some(inverse) = blend_matrix(ma, mb).inverse() else {
                    return Ok(([0; 4], Paint::Solid));
                };
                // x from -EXTENT to EXTENT becomes 0 to 1.
                let along = Matrix {
                    a: 0.5 / EXTENT,
                    b: 0.0,
                    c: 0.0,
                    d: 0.0,
                    tx: 0.5,
                    ty: 0.0,
                };
                let paint = Paint::Linear {
                    ramp: self.ramp(&blend_stops(sa, sb)),
                    matrix: along.then_inner(inverse),
                    spread: Spread::Pad,
                };
                (WHITE, paint)
            }
            (
                f::Paint::RadialGradient {
                    matrix: ma,
                    stops: sa,
                    ..
                },
                f::Paint::RadialGradient {
                    matrix: mb,
                    stops: sb,
                    ..
                },
            ) => {
                let Some(inverse) = blend_matrix(ma, mb).inverse() else {
                    return Ok(([0; 4], Paint::Solid));
                };
                let paint = Paint::Radial {
                    ramp: self.ramp(&blend_stops(sa, sb)),
                    matrix: Matrix::scale(1.0 / EXTENT, 1.0 / EXTENT).then_inner(inverse),
                    spread: Spread::Pad,
                };
                (WHITE, paint)
            }
            _ => bail!("a morph shape blends between paints the engine cannot mix"),
        })
    }

    /// The row of the ramp for these stops, adding it if it is new.
    fn ramp(&mut self, stops: &[f::GradientStop]) -> usize {
        let mut ramp: Ramp = [[0; 4]; 256];
        for (i, entry) in ramp.iter_mut().enumerate() {
            let position = i as f64 / 255.0;
            let after = stops
                .iter()
                .position(|stop| stop.offset >= position)
                .unwrap_or(stops.len());
            let color = match (
                after.checked_sub(1).and_then(|i| stops.get(i)),
                stops.get(after),
            ) {
                (Some(a), Some(b)) => {
                    let span = (b.offset - a.offset).max(1e-9);
                    blend_color(a.color, b.color, ((position - a.offset) / span) as f32)
                }
                (Some(only), None) | (None, Some(only)) => only.color,
                (None, None) => f::Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                },
            };
            *entry = [color.r, color.g, color.b, color.a];
        }
        if let Some(&row) = self.ramp_rows.get(&ramp) {
            return row;
        }
        self.ramps.push(ramp);
        self.ramp_rows.insert(ramp, self.ramps.len() - 1);
        self.ramps.len() - 1
    }

    /// The slot of this image, decoding and adding it if it is new.
    fn image(&mut self, kind: &usvg::ImageKind) -> Result<usize> {
        let encoded: &Arc<Vec<u8>> = match kind {
            usvg::ImageKind::PNG(data)
            | usvg::ImageKind::JPEG(data)
            | usvg::ImageKind::GIF(data)
            | usvg::ImageKind::WEBP(data) => data,
            usvg::ImageKind::SVG(_) => bail!("an SVG inside a pattern fill is not supported"),
        };
        let mut hasher = DefaultHasher::new();
        encoded.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(&slot) = self.image_slots.get(&key) {
            return Ok(slot);
        }
        let decoded = image::load_from_memory(encoded)
            .context("decoding an image used as a fill")?
            .to_rgba8();
        self.images.push(Image {
            width: decoded.width(),
            height: decoded.height(),
            rgba: decoded.into_raw(),
        });
        self.image_slots.insert(key, self.images.len() - 1);
        Ok(self.images.len() - 1)
    }
}

fn first_image(group: &usvg::Group) -> Option<&usvg::Image> {
    group.children().iter().find_map(|node| match node {
        usvg::Node::Image(image) => Some(&**image),
        usvg::Node::Group(group) => first_image(group),
        _ => None,
    })
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn blend_color(a: f::Color, b: f::Color, t: f32) -> f::Color {
    let channel = |a: u8, b: u8| lerp(f32::from(a), f32::from(b), t).round() as u8;
    f::Color {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: channel(a.a, b.a),
    }
}

/// One command of an outline, with absolute coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

impl Segment {
    /// The segment part of the way to `other`, which must be the same kind.
    fn blend(&self, other: &Segment, t: f32) -> Segment {
        match (*self, *other) {
            (Segment::Move(x, y), Segment::Move(x2, y2)) => {
                Segment::Move(lerp(x, x2, t), lerp(y, y2, t))
            }
            (Segment::Line(x, y), Segment::Line(x2, y2)) => {
                Segment::Line(lerp(x, x2, t), lerp(y, y2, t))
            }
            (Segment::Quad(a, b, c, d), Segment::Quad(a2, b2, c2, d2)) => Segment::Quad(
                lerp(a, a2, t),
                lerp(b, b2, t),
                lerp(c, c2, t),
                lerp(d, d2, t),
            ),
            (same, _) => same,
        }
    }
}

/// Parses the path data the extractor writes: `M`, `L`, `Q` and `Z` with
/// absolute coordinates.
fn parse_path(data: &str) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut rest = data.trim_start();
    while let Some(command) = rest.chars().next() {
        rest = &rest[command.len_utf8()..];
        let mut number = || -> Result<f32> {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let (text, tail) = rest.split_at(end);
            rest = tail;
            text.parse()
                .with_context(|| format!("bad number {text:?} in path data"))
        };
        segments.push(match command {
            'M' => Segment::Move(number()?, number()?),
            'L' => Segment::Line(number()?, number()?),
            'Q' => Segment::Quad(number()?, number()?, number()?, number()?),
            'Z' => Segment::Close,
            other => bail!("unexpected {other:?} in path data"),
        });
        rest = rest.trim_start();
    }
    Ok(segments)
}

/// Builds a lyon path from segments, moving every point through `matrix`.
fn lyon_path(segments: impl Iterator<Item = Segment>, matrix: Matrix) -> lyon::path::Path {
    let at = |x: f32, y: f32| {
        let (x, y) = matrix.apply(x, y);
        point(x, y)
    };
    let mut builder = lyon::path::Path::builder();
    let mut open = false;
    // Where the current outline began, for a line that follows a close.
    let mut start = point(0.0, 0.0);
    let ensure_open = |builder: &mut lyon::path::Builder, open: &mut bool, start| {
        if !*open {
            builder.begin(start);
            *open = true;
        }
    };
    for segment in segments {
        match segment {
            Segment::Move(x, y) => {
                if open {
                    builder.end(false);
                }
                start = at(x, y);
                builder.begin(start);
                open = true;
            }
            Segment::Line(x, y) => {
                ensure_open(&mut builder, &mut open, start);
                builder.line_to(at(x, y));
            }
            Segment::Quad(cx, cy, x, y) => {
                ensure_open(&mut builder, &mut open, start);
                builder.quadratic_bezier_to(at(cx, cy), at(x, y));
            }
            Segment::Cubic(c1x, c1y, c2x, c2y, x, y) => {
                ensure_open(&mut builder, &mut open, start);
                builder.cubic_bezier_to(at(c1x, c1y), at(c2x, c2y), at(x, y));
            }
            Segment::Close => {
                if open {
                    builder.end(true);
                    open = false;
                }
            }
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The total area of a mesh's triangles.
    fn area(mesh: &Mesh) -> f32 {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|triangle| {
                let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize].position);
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
            })
            .sum()
    }

    fn filled(data: &str) -> Mesh {
        let mut builder = Builder::new();
        let outline = lyon_path(parse_path(data).unwrap().into_iter(), Matrix::IDENTITY);
        builder.fill(&outline, [255; 4], Paint::Solid).unwrap();
        builder.build()
    }

    #[test]
    fn path_data_parses_into_segments() {
        assert_eq!(
            parse_path("M0 0 L1.5 0 Q2 -1 3 0 Z").unwrap(),
            [
                Segment::Move(0.0, 0.0),
                Segment::Line(1.5, 0.0),
                Segment::Quad(2.0, -1.0, 3.0, 0.0),
                Segment::Close,
            ]
        );
        assert!(parse_path("M0 0 C1 1 2 2 3 3").is_err());
    }

    #[test]
    fn a_square_fills_its_whole_area() {
        let mesh = filled("M0 0 L10 0 L10 10 L0 10 Z");
        assert!((area(&mesh) - 100.0).abs() < 0.01);
    }

    #[test]
    fn an_inner_outline_cuts_a_hole() {
        let mesh = filled("M0 0 L10 0 L10 10 L0 10 Z M2 2 L8 2 L8 8 L2 8 Z");
        assert!((area(&mesh) - 64.0).abs() < 0.01);
    }

    #[test]
    fn neighbouring_solid_fills_share_one_draw() {
        let mut builder = Builder::new();
        for data in ["M0 0 L1 0 L1 1 Z", "M5 5 L6 5 L6 6 Z"] {
            let outline = lyon_path(parse_path(data).unwrap().into_iter(), Matrix::IDENTITY);
            builder.fill(&outline, [255; 4], Paint::Solid).unwrap();
        }
        let mesh = builder.build();
        assert_eq!(mesh.draws.len(), 1);
        assert_eq!(mesh.draws[0].indices, 0..6);
    }

    #[test]
    fn a_stroke_keeps_its_centre_line_and_points_outwards() {
        let mut builder = Builder::new();
        let outline = lyon_path(
            parse_path("M0 0 L10 0").unwrap().into_iter(),
            Matrix::IDENTITY,
        );
        let options = StrokeOptions::tolerance(TOLERANCE).with_line_width(4.0);
        builder
            .stroke(&outline, &options, [255; 4], Paint::Solid)
            .unwrap();
        let mesh = builder.build();
        assert!(!mesh.indices.is_empty());
        for vertex in &mesh.vertices {
            // Every vertex sits on the line, and knows which way is out.
            assert!(vertex.position[1].abs() < 1e-4);
            assert!((vertex.normal[1].abs() - 1.0).abs() < 1e-4);
            assert_eq!(vertex.half_width, 2.0);
        }
    }

    #[test]
    fn a_ramp_blends_between_its_stops_and_is_reused() {
        let stop = |offset: f64, v: u8| f::GradientStop {
            offset,
            color: f::Color {
                r: v,
                g: v,
                b: v,
                a: 255,
            },
        };
        let mut tessellator = Tessellator::default();
        let row = tessellator.ramp(&[stop(0.0, 0), stop(1.0, 255)]);
        assert_eq!(tessellator.ramps[row][0], [0, 0, 0, 255]);
        assert_eq!(tessellator.ramps[row][128], [128, 128, 128, 255]);
        assert_eq!(tessellator.ramps[row][255], [255, 255, 255, 255]);
        assert_eq!(tessellator.ramp(&[stop(0.0, 0), stop(1.0, 255)]), row);
        assert_eq!(tessellator.ramps.len(), 1);
    }

    #[test]
    fn a_morph_segment_blends_part_way() {
        let a = Segment::Quad(0.0, 0.0, 10.0, 0.0);
        let b = Segment::Quad(0.0, 10.0, 10.0, 20.0);
        assert_eq!(a.blend(&b, 0.5), Segment::Quad(0.0, 5.0, 10.0, 10.0));
    }
}
