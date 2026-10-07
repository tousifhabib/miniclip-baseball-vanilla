//! Starts the real game with no window, for tests that drive it with
//! written steps.
//!
//! These tests need the extracted art, which is not part of the repository.
//! Where it is missing they pass without checking anything, and say so.

#![allow(dead_code)]

use std::path::PathBuf;

use bb_engine::app::Runner;
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_game::baseball::{Baseball, Screen};
use bb_game::script::Script;

/// The folder `bb-extract` wrote, if it is there.
fn extracted() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extracted");
    dir.join("manifest.json").exists().then_some(dir)
}

/// The game, opened on the screen with this label. `None` when there is no
/// extracted art to play.
pub fn game(screen: &str) -> Option<Script> {
    game_with(screen, Some(1))
}

/// The game opened on a screen, with its chances worked out from `seed`.
pub fn game_with(screen: &str, seed: Option<u64>) -> Option<Script> {
    let Some(dir) = extracted() else {
        eprintln!("skipped: there is no extracted art to play");
        return None;
    };
    let library = Library::load(&dir).expect("loading the extracted art");
    let stage = Stage::new(None, &library);
    let mut logic = Box::new(Baseball::new(&library));
    logic.start_on(Screen::from_label(screen).expect("a screen with that label"));
    if let Some(seed) = seed {
        logic.seed(seed);
    }
    let runner = Runner::new(library, stage, logic, None);
    Some(Script::new(runner).expect("a renderer with no window"))
}

/// Follows `steps` and returns what the last `state` in them gave.
pub fn state_after(script: &mut Script, steps: &str) -> String {
    let lines = script.run(steps).expect("the steps to run");
    lines.last().cloned().unwrap_or_default()
}
