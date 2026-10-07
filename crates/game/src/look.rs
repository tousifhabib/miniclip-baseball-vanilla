//! What the batting side looks like: the colour of its shirts and helmets,
//! its batters' skin, and the logo on the bat.
//!
//! The art draws these parts as clips of their own, named so that they can
//! be found. Colouring one is a flat tint: every pixel of it becomes the
//! chosen colour, and the shading comes from other layers drawn over it.

use anyhow::{Context, Result};
use bb_engine::display::{Content, Path};
use bb_engine::library::Library;
use bb_engine::math::ColorTransform;
use bb_engine::stage::Stage;
use bb_format::{Op, PlaceAction, SymbolId, SymbolInfo};
use resvg::{tiny_skia, usvg};

/// A colour: red, green and blue.
pub type Rgb = [u8; 3];

/// Reads a colour written as `#rrggbb`.
pub fn rgb(text: &str) -> Option<Rgb> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let part = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}

/// How the side at bat is to be dressed. `None` leaves the art as drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    /// Shirts and helmets.
    pub clothes: Option<Rgb>,
    /// The batter at the plate.
    pub skin: Option<Rgb>,
    /// The frame of the art's logo clip to show on the bat.
    pub logo: Option<String>,
    /// The runner shown standing on second in the batting view.
    pub second_skin: Option<Rgb>,
}

/// The transform that paints a clip one flat colour.
pub fn tint(colour: Rgb) -> ColorTransform {
    let [r, g, b] = colour.map(|channel| f32::from(channel) / 255.0);
    ColorTransform {
        mult: [0.0, 0.0, 0.0, 1.0],
        add: [r, g, b, 0.0],
    }
}

/// Dresses everything at or below the clip at `from`. The art makes new
/// batters and runners all the time, each with these parts in it, so this is
/// done every frame.
pub fn dress(stage: &mut Stage, from: &[u16], look: &Look, library: &Library) {
    let mut tints: Vec<(Path, Rgb)> = Vec::new();
    let mut logos: Vec<Path> = Vec::new();
    let mut to_search = vec![from.to_vec()];
    while let Some(path) = to_search.pop() {
        let Some(clip) = stage.clip(&path) else {
            continue;
        };
        for (&depth, child) in &clip.children {
            let mut here = path.clone();
            here.push(depth);
            let colour = match child.name.as_deref() {
                Some("helmetMovie" | "tShirtMovie" | "helmet" | "box3" | "boxClothes") => {
                    look.clothes
                }
                Some("skinMovie" | "box2") => look.skin,
                Some("skinColor") => look.second_skin,
                Some("batLogo") => {
                    logos.push(here.clone());
                    None
                }
                _ => None,
            };
            if let Some(colour) = colour
                && child.color != tint(colour)
            {
                tints.push((here.clone(), colour));
            }
            if matches!(child.content, Content::Clip(_)) {
                to_search.push(here);
            }
        }
    }
    for (path, colour) in tints {
        if let Some(child) = stage.child_mut(&path) {
            child.set_color(tint(colour));
        }
    }
    let Some(logo) = &look.logo else {
        return;
    };
    for path in logos {
        // The logo is either this clip or one called `logo` inside it.
        let inner = stage.find(&path, &["logo"]);
        for clip in [Some(path), inner].into_iter().flatten() {
            if stage.goto_label(&clip, logo, false, library) {
                break;
            }
        }
    }
}

/// A strip of colours in the art that a click picks a colour from, drawn
/// once so that the colour under any point of it can be read back.
pub struct Swatch {
    picture: tiny_skia::Pixmap,
    /// The corner of the picture, in the strip clip's own coordinates.
    corner: (f32, f32),
}

impl Swatch {
    /// Draws the shapes on the first frame of `clip`, leaving out any mask.
    pub fn of_clip(library: &Library, clip: SymbolId) -> Result<Swatch> {
        let first = library
            .clips
            .get(&clip)
            .and_then(|timeline| timeline.frames.first())
            .with_context(|| format!("there is no clip {clip} to pick colours from"))?;
        let mut shapes = Vec::new();
        for op in &first.ops {
            if let Op::Place(place) = op
                && place.clip_depth.is_none()
                && let PlaceAction::Place(symbol) = place.action
                && let Some(entry) = library.manifest.symbols.get(&symbol)
                && let SymbolInfo::Shape { bounds } = entry.info
            {
                let matrix = place.matrix.unwrap_or(bb_format::IDENTITY);
                let at = (
                    (matrix[4] + bounds.x_min) as f32,
                    (matrix[5] + bounds.y_min) as f32,
                );
                let size = (
                    (bounds.x_max - bounds.x_min) as f32,
                    (bounds.y_max - bounds.y_min) as f32,
                );
                shapes.push((library.dir.join(&entry.file), at, size));
            }
        }
        let left = shapes
            .iter()
            .map(|(_, at, _)| at.0)
            .fold(f32::MAX, f32::min);
        let top = shapes
            .iter()
            .map(|(_, at, _)| at.1)
            .fold(f32::MAX, f32::min);
        let right = shapes
            .iter()
            .map(|(_, at, size)| at.0 + size.0)
            .fold(f32::MIN, f32::max);
        let bottom = shapes
            .iter()
            .map(|(_, at, size)| at.1 + size.1)
            .fold(f32::MIN, f32::max);
        let mut picture = tiny_skia::Pixmap::new(
            (right - left).ceil().max(1.0) as u32,
            (bottom - top).ceil().max(1.0) as u32,
        )
        .with_context(|| format!("clip {clip} has nothing in it to pick colours from"))?;
        for (file, at, _) in &shapes {
            let data =
                std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
            let options = usvg::Options {
                // A shape painted with a bitmap names it by a path from here.
                resources_dir: file.parent().map(std::path::Path::to_owned),
                ..usvg::Options::default()
            };
            let tree = usvg::Tree::from_data(&data, &options)
                .with_context(|| format!("parsing {}", file.display()))?;
            let place = tiny_skia::Transform::from_translate(at.0 - left, at.1 - top);
            resvg::render(&tree, place, &mut picture.as_mut());
        }
        Ok(Swatch {
            picture,
            corner: (left, top),
        })
    }

    /// The colour at a point of the strip, in the strip clip's own
    /// coordinates. `None` off the strip or where nothing is drawn.
    pub fn at(&self, x: f32, y: f32) -> Option<Rgb> {
        let (across, down) = (x - self.corner.0, y - self.corner.1);
        if across < 0.0 || down < 0.0 {
            return None;
        }
        let pixel = self.picture.pixel(across as u32, down as u32)?;
        if pixel.alpha() < 128 {
            return None;
        }
        let colour = pixel.demultiply();
        Some([colour.red(), colour.green(), colour.blue()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_read_from_its_usual_spelling() {
        assert_eq!(rgb("#3c311a"), Some([0x3c, 0x31, 0x1a]));
        assert_eq!(rgb("#FFFFFF"), Some([255, 255, 255]));
        assert_eq!(rgb("3c311a"), None);
        assert_eq!(rgb("#3c31"), None);
        assert_eq!(rgb("#zzzzzz"), None);
    }

    #[test]
    fn a_tint_replaces_every_colour_and_keeps_the_shape() {
        let painted = tint([255, 0, 51]);
        assert_eq!(painted.mult, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(painted.add, [1.0, 0.0, 0.2, 0.0]);
    }
}
