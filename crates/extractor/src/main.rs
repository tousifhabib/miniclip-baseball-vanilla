//! Converts a copy of the game's SWF into open files: SVG shapes, PNG
//! bitmaps, MP3 sounds, and JSON for timelines and everything else.

mod bitmap;
mod convert;
mod json;
mod paths;
mod shape;
mod timeline;

use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

use anyhow::{Context, Result};
use bb_format as f;
use clap::Parser;
use serde::Serialize;
use swf::{AudioCompression, ButtonActionCondition, ButtonState, FontFlag, Tag};

use crate::convert::{Strings, px};
use crate::shape::BitmapSizes;
use crate::timeline::Timeline;

#[derive(Parser)]
#[command(about = "Converts your copy of the game's SWF into open, editable files")]
struct Args {
    /// The SWF to read.
    swf: PathBuf,
    /// Folder to write to. Files already there are overwritten.
    #[arg(long, default_value = "extracted")]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let file =
        fs::File::open(&args.swf).with_context(|| format!("opening {}", args.swf.display()))?;
    let buf = swf::decompress_swf(BufReader::new(file)).context("decompressing the SWF")?;
    let movie = swf::parse_swf(&buf).context("parsing the SWF")?;

    let mut extractor = Extractor {
        out: args.out,
        strings: Strings {
            swf_version: movie.header.version(),
        },
        symbols: BTreeMap::new(),
        exports: BTreeMap::new(),
        jpeg_tables: None,
        bitmap_sizes: BitmapSizes::new(),
        glyph_codes: BTreeMap::new(),
        buttons: BTreeMap::new(),
        background: None,
        report: Report::default(),
    };
    extractor.run(&movie)?;
    extractor.print_report();
    Ok(())
}

struct Extractor {
    out: PathBuf,
    strings: Strings,
    symbols: BTreeMap<u16, f::Symbol>,
    exports: BTreeMap<String, u16>,
    /// The JPEG header shared by the oldest kind of bitmap tag.
    jpeg_tables: Option<Vec<u8>>,
    bitmap_sizes: BitmapSizes,
    /// Each font's character codes by glyph, for turning text back into
    /// strings.
    glyph_codes: BTreeMap<u16, Vec<u16>>,
    /// Held until the end, because a button's sounds arrive in a later tag.
    buttons: BTreeMap<u16, f::Button>,
    background: Option<f::Color>,
    report: Report,
}

#[derive(Default)]
struct Report {
    timelines: usize,
    frames: usize,
    placements: usize,
    /// Things that were dropped or could not be converted.
    problems: Vec<String>,
    /// Tags the extractor has no handling for, by name.
    skipped_tags: BTreeMap<String, usize>,
}

impl Extractor {
    fn run(&mut self, movie: &swf::Swf) -> Result<()> {
        for tag in &movie.tags {
            self.define(tag)?;
        }

        let buttons = std::mem::take(&mut self.buttons);
        for (id, button) in &buttons {
            let file = format!("buttons/{id}.json");
            self.write_json(&file, button)?;
            self.add(*id, file, f::SymbolInfo::Button);
        }

        let root = timeline::timeline(None, &movie.tags, self.strings);
        let frame_count = root.clip.frames.len() as u16;
        self.clip("clips/root.json", root)?;

        for (name, id) in &self.exports {
            match self.symbols.get_mut(id) {
                Some(symbol) => symbol.export_name = Some(name.clone()),
                None => self.report.problems.push(format!(
                    "export {name:?} names symbol {id}, which was not extracted"
                )),
            }
        }

        let stage = movie.header.stage_size();
        let manifest = f::Manifest {
            format_version: f::FORMAT_VERSION,
            swf_version: movie.header.version(),
            stage: f::Stage {
                width: px(stage.x_max) - px(stage.x_min),
                height: px(stage.y_max) - px(stage.y_min),
                frame_rate: movie.header.frame_rate().to_f64(),
                frame_count,
                background: self.background,
            },
            symbols: std::mem::take(&mut self.symbols),
            exports: std::mem::take(&mut self.exports),
        };
        self.write_json("manifest.json", &manifest)?;
        self.symbols = manifest.symbols;
        Ok(())
    }

    /// Extracts the symbol a tag defines, if it defines one.
    fn define(&mut self, tag: &Tag) -> Result<()> {
        match tag {
            Tag::SetBackgroundColor(color) => self.background = Some(convert::color(color)),
            Tag::JpegTables(tables) => self.jpeg_tables = Some(tables.to_vec()),
            Tag::DefineBits { id, jpeg_data } => {
                let decoded = bitmap::decode_jpeg(jpeg_data, self.jpeg_tables.as_deref(), None);
                self.bitmap(*id, decoded)?;
            }
            Tag::DefineBitsJpeg2 { id, jpeg_data } => {
                self.bitmap(*id, bitmap::decode_jpeg(jpeg_data, None, None))?;
            }
            Tag::DefineBitsJpeg3(bits) => {
                let decoded = bitmap::decode_jpeg(bits.data, None, Some(bits.alpha_data));
                self.bitmap(bits.id, decoded)?;
            }
            Tag::DefineBitsLossless(bits) => {
                self.bitmap(bits.id, bitmap::decode_lossless(bits))?;
            }
            Tag::DefineShape(shape) => {
                let (svg, notes) = shape::shape_svg(shape, &self.bitmap_sizes);
                let id = shape.id;
                if notes.missing_styles > 0 {
                    self.problem(format!(
                        "shape {id}: {} paths name a style that does not exist",
                        notes.missing_styles
                    ));
                }
                if notes.missing_bitmaps > 0 {
                    self.problem(format!(
                        "shape {id}: {} fills use a bitmap that was not extracted",
                        notes.missing_bitmaps
                    ));
                }
                if notes.non_zero_winding {
                    self.problem(format!(
                        "shape {id}: uses the non-zero fill rule, written as even-odd"
                    ));
                }
                let file = format!("shapes/{id}.svg");
                self.write(&file, svg)?;
                let bounds = convert::rect(&shape.shape_bounds);
                self.add(id, file, f::SymbolInfo::Shape { bounds });
            }
            Tag::DefineMorphShape(morph) => {
                let (shape, notes) = shape::morph_shape(morph);
                if notes.missing_styles > 0 {
                    self.problem(format!(
                        "morph shape {}: {} paths name a style that does not exist",
                        morph.id, notes.missing_styles
                    ));
                }
                let file = format!("morphs/{}.json", morph.id);
                self.write_json(&file, &shape)?;
                self.add(morph.id, file, f::SymbolInfo::MorphShape);
            }
            Tag::DefineSound(sound) => self.sound(sound)?,
            Tag::DefineSprite(sprite) => {
                let timeline = timeline::timeline(Some(sprite.id), &sprite.tags, self.strings);
                let frame_count = timeline.clip.frames.len() as u16;
                if frame_count != sprite.num_frames {
                    self.problem(format!(
                        "clip {}: declares {} frames but has {frame_count}",
                        sprite.id, sprite.num_frames
                    ));
                }
                let file = format!("clips/{}.json", sprite.id);
                self.clip(&file, timeline)?;
                self.add(sprite.id, file, f::SymbolInfo::Clip { frame_count });
            }
            Tag::DefineButton(button) | Tag::DefineButton2(button) => {
                self.buttons.insert(button.id, convert_button(button));
            }
            Tag::DefineButtonSound(sounds) => {
                let start = |sound: &Option<swf::ButtonSound>| {
                    sound
                        .as_ref()
                        .map(|(id, info)| convert::sound_start(*id, info))
                };
                let converted = f::ButtonSounds {
                    over_to_up: start(&sounds.over_to_up_sound),
                    up_to_over: start(&sounds.up_to_over_sound),
                    over_to_down: start(&sounds.over_to_down_sound),
                    down_to_over: start(&sounds.down_to_over_sound),
                };
                match self.buttons.get_mut(&sounds.id) {
                    Some(button) => button.sounds = Some(converted),
                    None => self.problem(format!("sounds for unknown button {}", sounds.id)),
                }
            }
            Tag::DefineText(text) | Tag::DefineText2(text) => self.text(text)?,
            Tag::DefineEditText(text) => {
                let file = format!("texts/{}.json", text.id());
                self.write_json(&file, &self.convert_edit_text(text))?;
                self.add(text.id(), file, f::SymbolInfo::EditText);
            }
            Tag::DefineFont2(font) => self.font(font)?,
            Tag::ExportAssets(assets) => {
                for asset in assets {
                    self.exports.insert(self.strings.get(asset.name), asset.id);
                }
            }
            // Timeline tags, read by `timeline::timeline`.
            Tag::ShowFrame
            | Tag::PlaceObject(_)
            | Tag::RemoveObject(_)
            | Tag::FrameLabel(_)
            | Tag::StartSound(_)
            | Tag::DoAction(_)
            | Tag::SoundStreamHead(_)
            | Tag::SoundStreamHead2(_)
            | Tag::SoundStreamBlock(_) => {}
            // Tags with nothing the extracted files need: file flags, and
            // hints for Flash's own text renderer.
            Tag::FileAttributes(_)
            | Tag::CsmTextSettings(_)
            | Tag::DefineFontAlignZones { .. }
            | Tag::DefineFontName { .. }
            | Tag::End => {}
            Tag::Unknown { tag_code, .. } => {
                *self
                    .report
                    .skipped_tags
                    .entry(format!("unknown tag {tag_code}"))
                    .or_default() += 1;
            }
            other => {
                let debug = format!("{other:?}");
                let name: String = debug
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                *self.report.skipped_tags.entry(name).or_default() += 1;
            }
        }
        Ok(())
    }

    fn bitmap(&mut self, id: u16, decoded: Result<bitmap::Bitmap>) -> Result<()> {
        let bitmap = match decoded {
            Ok(bitmap) => bitmap,
            Err(error) => {
                self.problem(format!("bitmap {id}: {error:#}"));
                return Ok(());
            }
        };
        let file = format!("bitmaps/{id}.png");
        let path = self.path_for(&file)?;
        image::save_buffer(
            &path,
            &bitmap.rgba,
            bitmap.width,
            bitmap.height,
            image::ExtendedColorType::Rgba8,
        )
        .with_context(|| format!("writing {}", path.display()))?;
        self.bitmap_sizes.insert(id, (bitmap.width, bitmap.height));
        let info = f::SymbolInfo::Bitmap {
            width: bitmap.width,
            height: bitmap.height,
        };
        self.add(id, file, info);
        Ok(())
    }

    fn sound(&mut self, sound: &swf::Sound) -> Result<()> {
        let id = sound.id;
        if sound.format.compression != AudioCompression::Mp3 || sound.data.len() < 2 {
            self.problem(format!(
                "sound {id}: {:?} audio is not supported",
                sound.format.compression
            ));
            return Ok(());
        }
        // MP3 data starts with the number of samples to skip.
        let (skip, mp3) = sound.data.split_at(2);
        let file = format!("sounds/{id}.mp3");
        self.write(&file, mp3)?;
        let info = f::SymbolInfo::Sound {
            sample_rate: sound.format.sample_rate,
            stereo: sound.format.is_stereo,
            sample_count: sound.num_samples,
            skip_samples: i16::from_le_bytes([skip[0], skip[1]]),
        };
        self.add(id, file, info);
        Ok(())
    }

    fn clip(&mut self, file: &str, timeline: Timeline) -> Result<()> {
        let Timeline {
            clip,
            trailing_ops,
            has_stream_sound,
        } = timeline;
        let name = match clip.id {
            Some(id) => format!("clip {id}"),
            None => "the main timeline".to_owned(),
        };
        if trailing_ops > 0 {
            self.problem(format!(
                "{name}: dropped {trailing_ops} changes after its last frame"
            ));
        }
        if has_stream_sound {
            self.problem(format!(
                "{name}: has streamed sound, which is not extracted"
            ));
        }
        self.report.timelines += 1;
        self.report.frames += clip.frames.len();
        self.report.placements += clip
            .frames
            .iter()
            .flat_map(|frame| &frame.ops)
            .filter(|op| matches!(op, f::Op::Place(_)))
            .count();
        self.write_json(file, &clip)
    }

    fn text(&mut self, text: &swf::Text) -> Result<()> {
        let mut runs = Vec::new();
        // A record changes only the settings it names; the rest carry over.
        let mut font = None;
        let mut height = 0.0;
        let mut color = f::Color {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let (mut x, mut y) = (0.0, 0.0);
        for record in &text.records {
            font = record.font_id.or(font);
            if let Some(h) = record.height {
                height = px(h);
            }
            if let Some(c) = &record.color {
                color = convert::color(c);
            }
            if let Some(offset) = record.x_offset {
                x = px(offset);
            }
            if let Some(offset) = record.y_offset {
                y = px(offset);
            }
            let Some(font) = font else {
                self.problem(format!("text {}: a run has no font", text.id));
                continue;
            };
            let codes = self.glyph_codes.get(&font);
            let glyphs: Vec<_> = record
                .glyphs
                .iter()
                .map(|glyph| f::GlyphPlacement {
                    glyph: glyph.index,
                    advance: f64::from(glyph.advance) / 20.0,
                })
                .collect();
            let string = record
                .glyphs
                .iter()
                .map(|glyph| {
                    codes
                        .and_then(|codes| codes.get(glyph.index as usize))
                        .and_then(|&code| char::from_u32(u32::from(code)))
                        .unwrap_or(char::REPLACEMENT_CHARACTER)
                })
                .collect();
            let width: f64 = glyphs.iter().map(|glyph| glyph.advance).sum();
            runs.push(f::TextRun {
                font,
                color,
                x,
                y,
                height,
                text: string,
                glyphs,
            });
            x += width;
        }

        let converted = f::Text {
            id: text.id,
            bounds: convert::rect(&text.bounds),
            matrix: convert::matrix(&text.matrix),
            runs,
        };
        let file = format!("texts/{}.json", text.id);
        self.write_json(&file, &converted)?;
        self.add(text.id, file, f::SymbolInfo::Text);
        Ok(())
    }

    fn convert_edit_text(&self, text: &swf::EditText) -> f::EditText {
        let flags = [
            (text.is_word_wrap(), "word_wrap"),
            (text.is_multiline(), "multiline"),
            (text.is_password(), "password"),
            (text.is_read_only(), "read_only"),
            (text.is_auto_size(), "auto_size"),
            (text.is_selectable(), "selectable"),
            (text.has_border(), "border"),
            (text.is_html(), "html"),
            (text.use_outlines(), "use_outlines"),
        ];
        f::EditText {
            id: text.id(),
            bounds: convert::rect(text.bounds()),
            font: text.font_id(),
            height: text.height().map(px),
            color: text.color().map(|color| convert::color(&color)),
            max_length: text.max_length(),
            layout: text.layout().map(|layout| f::TextLayout {
                align: format!("{:?}", layout.align).to_lowercase(),
                left_margin: px(layout.left_margin),
                right_margin: px(layout.right_margin),
                indent: px(layout.indent),
                leading: px(layout.leading),
            }),
            variable: self.strings.get(text.variable_name()),
            initial_text: text.initial_text().map(|text| self.strings.get(text)),
            flags: flags
                .iter()
                .filter(|(set, _)| *set)
                .map(|(_, name)| (*name).to_owned())
                .collect(),
        }
    }

    fn font(&mut self, font: &swf::Font) -> Result<()> {
        // From version 3, fonts store every measurement at twenty times the
        // size, for finer outlines.
        let units = if font.version >= 3 { 20 } else { 1 };
        let scale = f64::from(units);
        let name = self.strings.get(font.name);
        let name = name.trim_end_matches('\0').to_owned();
        let converted = f::Font {
            id: font.id,
            name: name.clone(),
            bold: font.flags.contains(FontFlag::IS_BOLD),
            italic: font.flags.contains(FontFlag::IS_ITALIC),
            em_size: 1024.0,
            metrics: font.layout.as_ref().map(|layout| f::FontMetrics {
                ascent: f64::from(layout.ascent) / scale,
                descent: f64::from(layout.descent) / scale,
                leading: f64::from(layout.leading) / scale,
                kerning: layout
                    .kerning
                    .iter()
                    .map(|kerning| f::Kerning {
                        left: kerning.left_code,
                        right: kerning.right_code,
                        adjustment: f64::from(kerning.adjustment.get()) / scale,
                    })
                    .collect(),
            }),
            glyphs: font
                .glyphs
                .iter()
                .map(|glyph| f::Glyph {
                    code: glyph.code,
                    char: char::from_u32(u32::from(glyph.code))
                        .map(String::from)
                        .unwrap_or_default(),
                    advance: f64::from(glyph.advance) / scale,
                    path: shape::glyph_path(&glyph.shape_records, units),
                })
                .collect(),
        };
        self.glyph_codes.insert(
            font.id,
            font.glyphs.iter().map(|glyph| glyph.code).collect(),
        );
        let file = format!("fonts/{}.json", font.id);
        self.write_json(&file, &converted)?;
        self.add(font.id, file, f::SymbolInfo::Font { name });
        Ok(())
    }

    fn add(&mut self, id: u16, file: String, info: f::SymbolInfo) {
        let symbol = f::Symbol {
            file,
            export_name: None,
            info,
        };
        if self.symbols.insert(id, symbol).is_some() {
            self.problem(format!("symbol {id} is defined more than once"));
        }
    }

    fn problem(&mut self, message: String) {
        self.report.problems.push(message);
    }

    /// The full path for a file in the output folder, with its folder made.
    fn path_for(&self, file: &str) -> Result<PathBuf> {
        let path = self.out.join(file);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(path)
    }

    fn write(&self, file: &str, contents: impl AsRef<[u8]>) -> Result<()> {
        let path = self.path_for(file)?;
        fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
    }

    fn write_json<T: Serialize>(&self, file: &str, value: &T) -> Result<()> {
        self.write(file, json::to_pretty(value)?)
    }

    fn print_report(&self) {
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for symbol in self.symbols.values() {
            let kind = match symbol.info {
                f::SymbolInfo::Shape { .. } => "shapes",
                f::SymbolInfo::MorphShape => "morph shapes",
                f::SymbolInfo::Bitmap { .. } => "bitmaps",
                f::SymbolInfo::Sound { .. } => "sounds",
                f::SymbolInfo::Clip { .. } => "clips",
                f::SymbolInfo::Button => "buttons",
                f::SymbolInfo::Text => "fixed texts",
                f::SymbolInfo::EditText => "text fields",
                f::SymbolInfo::Font { .. } => "fonts",
            };
            *kinds.entry(kind).or_default() += 1;
        }

        println!("Extracted to {}", self.out.display());
        for (kind, count) in &kinds {
            println!("  {count:>6}  {kind}");
        }
        println!(
            "  {:>6}  frames across {} timelines",
            self.report.frames, self.report.timelines
        );
        println!("  {:>6}  placements", self.report.placements);

        if !self.report.skipped_tags.is_empty() {
            println!("Tags with no handling:");
            for (name, count) in &self.report.skipped_tags {
                println!("  {count:>6}  {name}");
            }
        }
        if self.report.problems.is_empty() {
            println!("No problems.");
        } else {
            println!("Problems ({}):", self.report.problems.len());
            for problem in &self.report.problems {
                println!("  {problem}");
            }
        }
    }
}

fn convert_button(button: &swf::Button) -> f::Button {
    const STATES: [(ButtonState, &str); 4] = [
        (ButtonState::UP, "up"),
        (ButtonState::OVER, "over"),
        (ButtonState::DOWN, "down"),
        (ButtonState::HIT_TEST, "hit"),
    ];
    // Named after the ActionScript event each transition fires. The two
    // `menu_` ones only happen on buttons that track as menus.
    const CONDITIONS: [(ButtonActionCondition, &str); 9] = [
        (ButtonActionCondition::IDLE_TO_OVER_UP, "roll_over"),
        (ButtonActionCondition::OVER_UP_TO_IDLE, "roll_out"),
        (ButtonActionCondition::OVER_UP_TO_OVER_DOWN, "press"),
        (ButtonActionCondition::OVER_DOWN_TO_OVER_UP, "release"),
        (ButtonActionCondition::OVER_DOWN_TO_OUT_DOWN, "drag_out"),
        (ButtonActionCondition::OUT_DOWN_TO_OVER_DOWN, "drag_over"),
        (ButtonActionCondition::OUT_DOWN_TO_IDLE, "release_outside"),
        (ButtonActionCondition::IDLE_TO_OVER_DOWN, "menu_drag_over"),
        (ButtonActionCondition::OVER_DOWN_TO_IDLE, "menu_drag_out"),
    ];
    f::Button {
        id: button.id,
        track_as_menu: button.is_track_as_menu,
        records: button
            .records
            .iter()
            .map(|record| f::ButtonRecord {
                states: STATES
                    .iter()
                    .filter(|(state, _)| record.states.contains(*state))
                    .map(|(_, name)| (*name).to_owned())
                    .collect(),
                symbol: record.id,
                depth: record.depth,
                matrix: convert::matrix(&record.matrix),
                color: (record.color_transform != swf::ColorTransform::IDENTITY)
                    .then(|| convert::color_transform(&record.color_transform)),
                filters: record.filters.iter().map(convert::filter).collect(),
            })
            .collect(),
        actions: button
            .actions
            .iter()
            .map(|action| f::ButtonAction {
                conditions: CONDITIONS
                    .iter()
                    .filter(|(condition, _)| action.conditions.contains(*condition))
                    .map(|(_, name)| (*name).to_owned())
                    .collect(),
                key: action.key_press().map(|key| key.get()),
            })
            .collect(),
        sounds: None,
    }
}
