//! Everything the extractor wrote, loaded into memory.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use bb_format::{
    Button, Clip, EditText, FORMAT_VERSION, Font, Manifest, MorphShape, SymbolId, SymbolInfo, Text,
};
use serde::de::DeserializeOwned;

/// The game's symbols. Shapes and bitmaps are not held here: they go straight
/// to the renderer, which reads them from `dir`.
pub struct Library {
    /// The extracted folder this was loaded from.
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// The main timeline.
    pub root: Clip,
    pub clips: HashMap<SymbolId, Clip>,
    pub buttons: HashMap<SymbolId, Button>,
    pub texts: HashMap<SymbolId, Text>,
    pub edit_texts: HashMap<SymbolId, EditText>,
    pub fonts: HashMap<SymbolId, Font>,
    pub morphs: HashMap<SymbolId, MorphShape>,
}

impl Library {
    pub fn load(dir: &Path) -> Result<Library> {
        let manifest: Manifest = read_json(&dir.join("manifest.json"))?;
        ensure!(
            manifest.format_version == FORMAT_VERSION,
            "{} is format version {}, but this engine reads version {FORMAT_VERSION}; \
             run the extractor again",
            dir.display(),
            manifest.format_version
        );

        let mut library = Library {
            dir: dir.to_owned(),
            root: read_json(&dir.join("clips/root.json"))?,
            clips: HashMap::new(),
            buttons: HashMap::new(),
            texts: HashMap::new(),
            edit_texts: HashMap::new(),
            fonts: HashMap::new(),
            morphs: HashMap::new(),
            manifest,
        };
        for (&id, symbol) in &library.manifest.symbols {
            let path = dir.join(&symbol.file);
            match symbol.info {
                SymbolInfo::Clip { .. } => {
                    library.clips.insert(id, read_json(&path)?);
                }
                SymbolInfo::Button => {
                    library.buttons.insert(id, read_json(&path)?);
                }
                SymbolInfo::Text => {
                    library.texts.insert(id, read_json(&path)?);
                }
                SymbolInfo::EditText => {
                    library.edit_texts.insert(id, read_json(&path)?);
                }
                SymbolInfo::Font { .. } => {
                    library.fonts.insert(id, read_json(&path)?);
                }
                SymbolInfo::MorphShape => {
                    library.morphs.insert(id, read_json(&path)?);
                }
                SymbolInfo::Shape { .. } | SymbolInfo::Bitmap { .. } | SymbolInfo::Sound { .. } => {
                }
            }
        }
        Ok(library)
    }

    /// The timeline of a clip, or the main timeline for `None`.
    pub fn timeline(&self, clip: Option<SymbolId>) -> Option<&Clip> {
        match clip {
            Some(id) => self.clips.get(&id),
            None => Some(&self.root),
        }
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}
