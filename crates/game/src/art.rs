//! How the original's art is put together: which symbol is which screen,
//! and what each button says.
//!
//! Everything here describes the extracted files. None of it is game logic.

use std::collections::HashMap;

use bb_engine::display::{Content, Path};
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

/// The strips of colour on the setup pages that a click picks from.
pub const CLOTHES_STRIP: SymbolId = 247;
pub const SKIN_STRIP: SymbolId = 439;
/// The unseen buttons lying over those strips.
pub const CLOTHES_STRIP_BUTTONS: [SymbolId; 2] = [248, 449];
pub const SKIN_STRIP_BUTTONS: [SymbolId; 1] = [450];
/// Buttons that choose one set colour: the button, and the colour.
pub const CLOTHES_BUTTON: (SymbolId, [u8; 3]) = (445, [0x00, 0x33, 0x66]);
pub const SKIN_BUTTON: (SymbolId, [u8; 3]) = (446, [0xbb, 0xa6, 0x74]);
/// Buttons that choose the logo on the bat: the button, and the frame of the
/// art's logo clip it shows.
pub const LOGO_BUTTONS: [(SymbolId, &str); 6] = [
    (447, "fire"),
    (448, "eagle"),
    (451, "miniclip"),
    (452, "sweetspot"),
    (453, "bigbat"),
    (454, "warclub"),
];

/// The panel on the high-score page, and the parts of it that tell the
/// player the scores are kept elsewhere: one drawing that holds both the
/// panel's heading and that notice, and a link to the web site.
pub const SCORE_PANEL: SymbolId = 422;
pub const SCORE_PANEL_NOTICE: [SymbolId; 2] = [421, 420];
/// A text field in the game's display lettering, with letters and figures,
/// which the table is written in.
pub const TABLE_FIELD: SymbolId = 1762;

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

/// Every object with this instance name at or below the clip at `from`.
pub fn all_named(stage: &Stage, from: &[u16], name: &str) -> Vec<Path> {
    let mut found = Vec::new();
    let mut to_search = vec![from.to_vec()];
    while let Some(path) = to_search.pop() {
        let Some(clip) = stage.clip(&path) else {
            continue;
        };
        for (&depth, child) in &clip.children {
            let mut here = path.clone();
            here.push(depth);
            if child.name.as_deref() == Some(name) {
                found.push(here.clone());
            }
            if matches!(child.content, Content::Clip(_)) {
                to_search.push(here);
            }
        }
    }
    found
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
