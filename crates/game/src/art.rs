//! How the original's art is put together: which symbol is which screen,
//! and what each button says.
//!
//! Everything here describes the extracted files. None of it is game logic.

use std::collections::HashMap;

use bb_engine::display::Path;
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_format::{Op, PlaceAction, SymbolId, SymbolInfo};

/// The clip that holds every screen, one per labelled frame.
pub const SHELL: SymbolId = 2027;
/// The opening animation, on the shell's `intro` frame.
pub const INTRO: SymbolId = 165;
/// The menu, on the shell's `menu` frame. Its labelled sections are the
/// pages of the menu.
pub const MENU: SymbolId = 489;
/// The "are you sure?" panel shown over a game.
pub const QUIT_PROMPT: SymbolId = 1675;
/// The three versions of the instructions, each a clip that stops at the end
/// of every page.
pub const INSTRUCTIONS: [SymbolId; 3] = [2026, 1988, 2000];

/// Buttons whose words are drawn as artwork, not text, so they have to be
/// known by number: the ones on the result screens that lead back to the
/// menu, such as the "restart game" badge.
pub const CONTINUE_BUTTONS: [SymbolId; 4] = [493, 1725, 1767, 1847];

/// Where the shell is in the tree, once the art has reached it.
pub fn shell(stage: &Stage) -> Option<Path> {
    stage.find_symbol(&[], SHELL)
}

/// A clip that sits directly inside the shell: the screen that is showing.
pub fn in_shell(stage: &Stage, symbol: SymbolId) -> Option<Path> {
    stage.find_symbol(&shell(stage)?, symbol)
}

/// The words on each button, in capitals, for telling buttons apart.
pub struct ButtonLabels(HashMap<SymbolId, String>);

impl ButtonLabels {
    pub fn read(library: &Library) -> ButtonLabels {
        let labels = library
            .buttons
            .iter()
            .map(|(&id, button)| {
                let mut words = Vec::new();
                for record in &button.records {
                    if record.states.iter().any(|state| state == "up") {
                        collect_words(record.symbol, library, 0, &mut words);
                    }
                }
                (id, words.join(" ").to_uppercase())
            })
            .filter(|(_, label)| !label.is_empty())
            .collect();
        ButtonLabels(labels)
    }

    pub fn get(&self, button: SymbolId) -> Option<&str> {
        self.0.get(&button).map(String::as_str)
    }
}

/// Gathers the fixed text inside a symbol: its own, and that of anything on
/// the first frame of a clip inside it.
fn collect_words(symbol: SymbolId, library: &Library, depth: usize, words: &mut Vec<String>) {
    if depth > 5 {
        return;
    }
    match library.manifest.symbols.get(&symbol).map(|s| &s.info) {
        Some(SymbolInfo::Text) => {
            let Some(text) = library.texts.get(&symbol) else {
                return;
            };
            words.extend(
                text.runs
                    .iter()
                    .map(|run| run.text.trim().to_owned())
                    .filter(|run| !run.is_empty()),
            );
        }
        Some(SymbolInfo::Clip { .. }) => {
            let first = library
                .clips
                .get(&symbol)
                .and_then(|clip| clip.frames.first());
            for op in first.map(|frame| frame.ops.as_slice()).unwrap_or_default() {
                if let Op::Place(place) = op
                    && let PlaceAction::Place(child) = place.action
                {
                    collect_words(child, library, depth + 1, words);
                }
            }
        }
        _ => {}
    }
}
