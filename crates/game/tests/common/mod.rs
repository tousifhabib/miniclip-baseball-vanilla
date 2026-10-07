//! Starts the real game with no window, for tests that drive it with
//! written steps.
//!
//! These tests need the extracted art. Where it is missing they pass
//! without checking anything, and say so.

#![allow(dead_code)]

use std::path::PathBuf;

use bb_engine::app::Runner;
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_game::baseball::{Baseball, Screen};
use bb_game::rules::Rules;
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
    game_ruled(screen, seed, None)
}

/// The same, played by `rules` instead of the ones built in.
pub fn game_ruled(screen: &str, seed: Option<u64>, rules: Option<Rules>) -> Option<Script> {
    game_set_up(screen, seed, rules, None)
}

/// The same, keeping its high scores in `scores`.
pub fn game_scored(screen: &str, scores: &std::path::Path) -> Option<Script> {
    game_set_up(screen, Some(1), None, Some(scores))
}

fn game_set_up(
    screen: &str,
    seed: Option<u64>,
    rules: Option<Rules>,
    scores: Option<&std::path::Path>,
) -> Option<Script> {
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
    if let Some(rules) = rules {
        logic.play_by(rules);
    }
    if let Some(scores) = scores {
        logic.keep_scores_in(scores.to_owned());
    }
    let runner = Runner::new(library, stage, logic, None);
    Some(Script::new(runner).expect("a renderer with no window"))
}

/// Where the middle of an object is on the stage, if it is there and showing.
pub fn middle_of(script: &Script, path: &[u16]) -> Option<(f32, f32)> {
    let (stage, library) = (&script.runner.stage, &script.runner.library);
    let child = stage.child(path)?;
    if !child.visible {
        return None;
    }
    let parent = stage.to_stage(&path[..path.len() - 1])?;
    // A button that draws nothing is still somewhere: where it can be hit.
    let hit_area = match &child.content {
        bb_engine::display::Content::Button(button) => {
            bb_engine::display::bounds_of(&button.hit, parent.then_inner(child.matrix), library)
        }
        _ => None,
    };
    let [left, top, right, bottom] =
        bb_engine::display::child_bounds(child, parent, library).or(hit_area)?;
    Some(((left + right) / 2.0, (top + bottom) / 2.0))
}

/// Clicks the middle of the first showing object with this instance name.
/// Returns whether there was one.
pub fn click_named(script: &mut Script, name: &str) -> bool {
    let found = bb_game::art::all_named(&script.runner.stage, &[], name)
        .into_iter()
        .find_map(|path| middle_of(script, &path));
    let Some((x, y)) = found else {
        return false;
    };
    script.run(&format!("click {x} {y}")).expect("the click");
    true
}

/// Clicks the middle of the first instance of this symbol.
pub fn click_symbol(script: &mut Script, symbol: u16) -> bool {
    let found = script
        .runner
        .stage
        .find_symbol(&[], symbol)
        .and_then(|path| middle_of(script, &path));
    let Some((x, y)) = found else {
        return false;
    };
    script.run(&format!("click {x} {y}")).expect("the click");
    true
}

/// Follows `steps` and returns what the last `state` in them gave.
pub fn state_after(script: &mut Script, steps: &str) -> String {
    let lines = script.run(steps).expect("the steps to run");
    lines.last().cloned().unwrap_or_default()
}
