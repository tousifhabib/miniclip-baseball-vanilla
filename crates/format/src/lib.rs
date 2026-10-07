//! Data types for the files `bb-extract` writes and the engine reads.
//!
//! Lengths and positions are in pixels (Flash stores twentieths of a pixel).
//! Frame numbers start at 1, as they do in ActionScript.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Bumped whenever a change would make older extracted files unreadable.
pub const FORMAT_VERSION: u32 = 1;

pub type SymbolId = u16;

/// `[a, b, c, d, tx, ty]`, the same order as SVG's `matrix()`.
pub type Matrix = [f64; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Written as `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl From<Color> for String {
    fn from(c: Color) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        let hex = s.strip_prefix('#').unwrap_or(&s);
        if hex.len() != 8 || !hex.is_ascii() {
            return Err(format!("expected a colour like #rrggbbaa, got {s:?}"));
        }
        let byte = |i: usize| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| format!("bad colour {s:?}: {e}"))
        };
        Ok(Color {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: byte(6)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x_min: f64,
    pub y_min: f64,
    pub x_max: f64,
    pub y_max: f64,
}

/// Each channel becomes `channel * mult + add`, with `add` on a 0-255 scale.
/// Channel order is red, green, blue, alpha.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorTransform {
    pub mult: [f64; 4],
    pub add: [i16; 4],
}

// ---------------------------------------------------------------------------
// manifest.json

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub swf_version: u8,
    pub stage: Stage,
    pub symbols: BTreeMap<SymbolId, Symbol>,
    /// Linkage names, as used by `attachMovie` and `attachSound`.
    pub exports: BTreeMap<String, SymbolId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage {
    pub width: f64,
    pub height: f64,
    pub frame_rate: f64,
    pub frame_count: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Color>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Symbol {
    /// Where the symbol's file is, relative to the manifest.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_name: Option<String>,
    #[serde(flatten)]
    pub info: SymbolInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SymbolInfo {
    Shape {
        bounds: Rect,
    },
    MorphShape,
    Bitmap {
        width: u32,
        height: u32,
    },
    Sound {
        sample_rate: u16,
        stereo: bool,
        sample_count: u32,
        /// Samples the player skips at the start, to hide encoder padding.
        skip_samples: i16,
    },
    Clip {
        frame_count: u16,
    },
    Button,
    Text,
    EditText,
    Font {
        name: String,
    },
}

// ---------------------------------------------------------------------------
// clips/*.json

/// A movie clip's timeline. `clips/root.json` is the main timeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    /// `None` for the main timeline.
    pub id: Option<SymbolId>,
    pub labels: BTreeMap<String, u16>,
    pub frames: Vec<Frame>,
}

/// What changes on the display list when the playhead enters this frame.
/// Frames hold changes, not full snapshots, exactly as Flash stores them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<Op>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounds: Vec<SoundStart>,
    /// The original had ActionScript on this frame.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_script: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Place(Box<Place>),
    Remove { depth: u16 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Place {
    pub depth: u16,
    pub action: PlaceAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<Matrix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorTransform>,
    /// Morph position, 0-65535.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Makes this object a mask for everything up to this depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_depth: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<Filter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Events the original had `onClipEvent` scripts for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clip_events: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceAction {
    /// Put a new object at an empty depth.
    Place(SymbolId),
    /// Change the object already at this depth.
    Modify,
    /// Swap the object at this depth for another symbol.
    Replace(SymbolId),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    Blur {
        blur_x: f64,
        blur_y: f64,
        passes: u8,
    },
    /// A filter the extractor does not convert yet.
    Unsupported { name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoundStart {
    pub sound: SymbolId,
    pub event: SoundEvent,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub loops: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_sample: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_sample: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelope: Vec<EnvelopePoint>,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundEvent {
    /// Play, even if this sound is already playing.
    Event,
    /// Play only if this sound is not already playing.
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EnvelopePoint {
    pub sample: u32,
    pub left: f32,
    pub right: f32,
}

// ---------------------------------------------------------------------------
// buttons/*.json

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Button {
    pub id: SymbolId,
    pub track_as_menu: bool,
    pub records: Vec<ButtonRecord>,
    /// Mouse transitions the original had scripts for.
    pub actions: Vec<ButtonAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sounds: Option<ButtonSounds>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ButtonRecord {
    /// Any of `up`, `over`, `down`, `hit`.
    pub states: Vec<String>,
    pub symbol: SymbolId,
    pub depth: u16,
    pub matrix: Matrix,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorTransform>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ButtonAction {
    pub conditions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ButtonSounds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_to_up: Option<SoundStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to_over: Option<SoundStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_to_down: Option<SoundStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_to_over: Option<SoundStart>,
}

// ---------------------------------------------------------------------------
// texts/*.json

/// Fixed text, laid out glyph by glyph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Text {
    pub id: SymbolId,
    pub bounds: Rect,
    pub matrix: Matrix,
    pub runs: Vec<TextRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextRun {
    pub font: SymbolId,
    pub color: Color,
    /// Pen position of the first glyph.
    pub x: f64,
    pub y: f64,
    /// Font size.
    pub height: f64,
    /// The run's characters, for reading and searching. Drawing uses `glyphs`.
    pub text: String,
    pub glyphs: Vec<GlyphPlacement>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GlyphPlacement {
    /// Index into the font's `glyphs`.
    pub glyph: u32,
    pub advance: f64,
}

/// A text field the game can write to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditText {
    pub id: SymbolId,
    pub bounds: Rect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<SymbolId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<TextLayout>,
    /// The ActionScript variable the field shows.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub variable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_text: Option<String>,
    /// Any of `word_wrap`, `multiline`, `password`, `read_only`, `auto_size`,
    /// `selectable`, `border`, `html`, `use_outlines`.
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextLayout {
    pub align: String,
    pub left_margin: f64,
    pub right_margin: f64,
    pub indent: f64,
    pub leading: f64,
}

// ---------------------------------------------------------------------------
// fonts/*.json

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Font {
    pub id: SymbolId,
    pub name: String,
    pub bold: bool,
    pub italic: bool,
    /// Glyph coordinates and advances are in units of `1 / em_size` of the
    /// font size.
    pub em_size: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<FontMetrics>,
    pub glyphs: Vec<Glyph>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FontMetrics {
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kerning: Vec<Kerning>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Kerning {
    pub left: u16,
    pub right: u16,
    pub adjustment: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Glyph {
    pub code: u16,
    /// The character for `code`, for reading and searching.
    pub char: String,
    pub advance: f64,
    /// SVG path data, filled with the even-odd rule.
    pub path: String,
}

// ---------------------------------------------------------------------------
// morphs/*.json

/// A shape that blends between two outlines. Every path has the same commands
/// in `start` and `end`, so a blend is a straight interpolation of the numbers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MorphShape {
    pub id: SymbolId,
    pub start_bounds: Rect,
    pub end_bounds: Rect,
    /// In drawing order.
    pub paths: Vec<MorphPath>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MorphPath {
    /// SVG path data: only `M`, `Q` and `Z`.
    pub start: String,
    pub end: String,
    #[serde(flatten)]
    pub style: MorphStyle,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "draw", rename_all = "snake_case")]
pub enum MorphStyle {
    Fill {
        start_paint: Paint,
        end_paint: Paint,
    },
    Stroke {
        start_width: f64,
        end_width: f64,
        start_paint: Paint,
        end_paint: Paint,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Paint {
    Solid {
        color: Color,
    },
    /// Runs along x from -819.2 to 819.2 before `matrix` is applied.
    LinearGradient {
        matrix: Matrix,
        stops: Vec<GradientStop>,
    },
    /// Centred on the origin with radius 819.2 before `matrix` is applied.
    RadialGradient {
        matrix: Matrix,
        stops: Vec<GradientStop>,
        /// Focal point along x, from -1 to 1.
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        focal: f64,
    },
    /// `matrix` maps bitmap pixels to shape coordinates.
    Bitmap {
        bitmap: SymbolId,
        matrix: Matrix,
        smoothed: bool,
        repeating: bool,
    },
}

fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// 0 to 1.
    pub offset: f64,
    pub color: Color,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_round_trips_through_its_hex_form() {
        let c = Color {
            r: 0x12,
            g: 0xab,
            b: 0x00,
            a: 0x4d,
        };
        assert_eq!(String::from(c), "#12ab004d");
        assert_eq!(Color::try_from("#12ab004d".to_owned()), Ok(c));
    }

    #[test]
    fn color_rejects_malformed_text() {
        assert!(Color::try_from("#12ab00".to_owned()).is_err());
        assert!(Color::try_from("#zzzzzzzz".to_owned()).is_err());
    }
}
