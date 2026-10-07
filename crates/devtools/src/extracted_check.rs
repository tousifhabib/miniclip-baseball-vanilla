//! Reads an extracted folder back with the format's own types and checks
//! that everything one file says about another is true.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use bb_format::{
    Button, Clip, EditText, FORMAT_VERSION, Font, Manifest, MorphShape, Op, PlaceAction,
    SoundStart, SymbolId, SymbolInfo, Text,
};
use clap::Parser;
use serde::de::DeserializeOwned;

#[derive(Parser)]
#[command(about = "Checks that an extracted folder is complete and consistent")]
struct Args {
    /// The folder `bb-extract` wrote.
    dir: PathBuf,
}

struct Checker {
    dir: PathBuf,
    manifest: Manifest,
    glyph_counts: BTreeMap<SymbolId, usize>,
    files: usize,
    references: usize,
    problems: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let manifest: Manifest = read_json(&args.dir.join("manifest.json"))?;
    ensure!(
        manifest.format_version == FORMAT_VERSION,
        "the folder is format version {}, this tool reads version {FORMAT_VERSION}",
        manifest.format_version
    );

    let mut checker = Checker {
        dir: args.dir,
        manifest,
        glyph_counts: BTreeMap::new(),
        files: 1,
        references: 0,
        problems: Vec::new(),
    };
    checker.run()?;

    println!(
        "Read {} files and followed {} references.",
        checker.files, checker.references
    );
    if checker.problems.is_empty() {
        println!("No problems.");
        return Ok(());
    }
    println!("Problems ({}):", checker.problems.len());
    for problem in &checker.problems {
        println!("  {problem}");
    }
    std::process::exit(1);
}

impl Checker {
    fn run(&mut self) -> Result<()> {
        let symbols = self.manifest.symbols.clone();

        // Fonts first, because texts are checked against their glyph counts.
        for (id, symbol) in &symbols {
            if let SymbolInfo::Font { .. } = symbol.info {
                let font: Font = self.read(&symbol.file)?;
                self.glyph_counts.insert(*id, font.glyphs.len());
            }
        }

        for (id, symbol) in &symbols {
            match &symbol.info {
                SymbolInfo::Font { .. } => {}
                SymbolInfo::Shape { .. } | SymbolInfo::Bitmap { .. } | SymbolInfo::Sound { .. } => {
                    self.files += 1;
                    let path = self.dir.join(&symbol.file);
                    let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                    if size == 0 {
                        self.problem(format!("{} is missing or empty", symbol.file));
                    }
                }
                SymbolInfo::MorphShape => {
                    let morph: MorphShape = self.read(&symbol.file)?;
                    for (index, path) in morph.paths.iter().enumerate() {
                        if commands(&path.start) != commands(&path.end) {
                            self.problem(format!(
                                "morph {id}: path {index} has different commands at its two ends"
                            ));
                        }
                    }
                }
                SymbolInfo::Clip { frame_count } => {
                    let clip: Clip = self.read(&symbol.file)?;
                    if clip.frames.len() != usize::from(*frame_count) {
                        self.problem(format!(
                            "clip {id}: the manifest says {frame_count} frames, the file has {}",
                            clip.frames.len()
                        ));
                    }
                    self.check_clip(&format!("clip {id}"), &clip);
                }
                SymbolInfo::Button => {
                    let button: Button = self.read(&symbol.file)?;
                    for record in &button.records {
                        self.check_drawable(&format!("button {id}"), record.symbol);
                    }
                    if let Some(sounds) = &button.sounds {
                        let starts = [
                            &sounds.over_to_up,
                            &sounds.up_to_over,
                            &sounds.over_to_down,
                            &sounds.down_to_over,
                        ];
                        for start in starts.into_iter().flatten() {
                            self.check_sound(&format!("button {id}"), start);
                        }
                    }
                }
                SymbolInfo::Text => {
                    let text: Text = self.read(&symbol.file)?;
                    for run in &text.runs {
                        self.references += 1;
                        let Some(&glyphs) = self.glyph_counts.get(&run.font) else {
                            self.problem(format!("text {id}: font {} is not a font", run.font));
                            continue;
                        };
                        if run.glyphs.iter().any(|g| g.glyph as usize >= glyphs) {
                            self.problem(format!(
                                "text {id}: uses a glyph font {} does not have",
                                run.font
                            ));
                        }
                    }
                }
                SymbolInfo::EditText => {
                    let text: EditText = self.read(&symbol.file)?;
                    if let Some(font) = text.font {
                        self.references += 1;
                        if !self.glyph_counts.contains_key(&font) {
                            self.problem(format!("text field {id}: font {font} is not a font"));
                        }
                    }
                }
            }
        }

        let root: Clip = self.read("clips/root.json")?;
        if root.frames.len() != usize::from(self.manifest.stage.frame_count) {
            self.problem("the main timeline's length differs from the manifest".to_owned());
        }
        self.check_clip("the main timeline", &root);

        for (name, id) in &self.manifest.exports.clone() {
            self.references += 1;
            match self.manifest.symbols.get(id) {
                Some(symbol) if symbol.export_name.as_deref() == Some(name) => {}
                Some(_) => self.problem(format!("export {name:?}: symbol {id} is not named so")),
                None => self.problem(format!("export {name:?}: no symbol {id}")),
            }
        }
        Ok(())
    }

    fn check_clip(&mut self, name: &str, clip: &Clip) {
        for (label, &frame) in &clip.labels {
            if frame == 0 || usize::from(frame) > clip.frames.len() {
                self.problem(format!("{name}: label {label:?} points at frame {frame}"));
            }
        }
        for frame in &clip.frames {
            for op in &frame.ops {
                if let Op::Place(place) = op
                    && let PlaceAction::Place(symbol) | PlaceAction::Replace(symbol) = place.action
                {
                    self.check_drawable(name, symbol);
                }
            }
            for start in &frame.sounds {
                self.check_sound(name, start);
            }
        }
    }

    /// Checks that `symbol` exists and is something a timeline can show.
    fn check_drawable(&mut self, name: &str, symbol: SymbolId) {
        self.references += 1;
        match self
            .manifest
            .symbols
            .get(&symbol)
            .map(|symbol| &symbol.info)
        {
            None => self.problem(format!(
                "{name}: places symbol {symbol}, which does not exist"
            )),
            Some(
                SymbolInfo::Sound { .. } | SymbolInfo::Font { .. } | SymbolInfo::Bitmap { .. },
            ) => self.problem(format!(
                "{name}: places symbol {symbol}, which cannot be shown"
            )),
            Some(_) => {}
        }
    }

    fn check_sound(&mut self, name: &str, start: &SoundStart) {
        self.references += 1;
        let info = self.manifest.symbols.get(&start.sound).map(|s| &s.info);
        if !matches!(info, Some(SymbolInfo::Sound { .. })) {
            self.problem(format!(
                "{name}: plays symbol {}, which is not a sound",
                start.sound
            ));
        }
    }

    fn read<T: DeserializeOwned>(&mut self, file: &str) -> Result<T> {
        self.files += 1;
        read_json(&self.dir.join(file))
    }

    fn problem(&mut self, message: String) {
        self.problems.push(message);
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// The command letters of some SVG path data, without their numbers.
fn commands(path: &str) -> String {
    path.chars().filter(char::is_ascii_alphabetic).collect()
}
