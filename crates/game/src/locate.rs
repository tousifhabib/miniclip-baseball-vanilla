//! Finds the extracted art.
//!
//! Run from a terminal, the game is told where the art is. Run as an app, it
//! is told nothing, and has to look.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// The app's identifier, which names its folder under Application Support.
pub const APP_ID: &str = "io.github.tousifhabib.baseball";

/// The name of the folder `bb-extract` writes, wherever it is kept.
const FOLDER: &str = "extracted";

/// The places the art may be, most particular first:
///
/// 1. inside the app bundle the program is running from, in its resources;
/// 2. the player's own copy, under Application Support;
/// 3. beside the program;
/// 4. in the folder the program was started from.
///
/// `program` is the path of the running program, `home` the player's home
/// folder and `current` the folder the program was started from. Any of them
/// may be unknown.
pub fn candidates(
    program: Option<&Path>,
    home: Option<&Path>,
    current: Option<&Path>,
) -> Vec<PathBuf> {
    let mut places = Vec::new();
    let beside = program.and_then(Path::parent);
    // In a bundle the program is at `Name.app/Contents/MacOS/program`.
    if let Some(contents) = beside
        .filter(|folder| folder.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
    {
        places.push(contents.join("Resources").join(FOLDER));
    }
    if let Some(home) = home {
        places.push(
            home.join("Library/Application Support")
                .join(APP_ID)
                .join(FOLDER),
        );
    }
    if let Some(beside) = beside {
        places.push(beside.join(FOLDER));
    }
    if let Some(current) = current {
        places.push(current.join(FOLDER));
    }
    places.dedup();
    places
}

/// Whether `dir` holds extracted art.
fn holds_art(dir: &Path) -> bool {
    dir.join("manifest.json").is_file()
}

/// The folder to load the art from: `asked` if the player named one, or
/// else the first of the usual places that has it.
pub fn find(asked: Option<&Path>) -> Result<PathBuf> {
    if let Some(asked) = asked {
        // A folder that was asked for by name is never swapped for another.
        if !holds_art(asked) {
            bail!(
                "{} has no extracted art in it (no manifest.json)",
                asked.display()
            );
        }
        return Ok(asked.to_owned());
    }
    let program = std::env::current_exe().ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let current = std::env::current_dir().ok();
    let places = candidates(program.as_deref(), home.as_deref(), current.as_deref());
    if let Some(found) = places.iter().find(|place| holds_art(place)) {
        return Ok(found.clone());
    }
    let looked: Vec<String> = places
        .iter()
        .map(|place| format!("  {}", place.display()))
        .collect();
    bail!(
        "The game's art was not found. Looked in:\n{}\n\
         Make it from your copy of the game with `bb-extract the-game.swf --out extracted`, \
         and put the folder in one of those places or name it when starting the game.",
        looked.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(program: &str, home: &str, current: &str) -> Vec<String> {
        candidates(
            Some(Path::new(program)),
            Some(Path::new(home)),
            Some(Path::new(current)),
        )
        .iter()
        .map(|place| place.display().to_string())
        .collect()
    }

    #[test]
    fn an_app_looks_in_its_own_resources_first() {
        assert_eq!(
            places(
                "/Applications/Baseball.app/Contents/MacOS/bb-game",
                "/Users/pat",
                "/"
            ),
            [
                "/Applications/Baseball.app/Contents/Resources/extracted",
                "/Users/pat/Library/Application Support/io.github.tousifhabib.baseball/extracted",
                "/Applications/Baseball.app/Contents/MacOS/extracted",
                "/extracted",
            ]
        );
    }

    #[test]
    fn a_plain_program_has_no_bundle_to_look_in() {
        assert_eq!(
            places("/work/target/release/bb-game", "/Users/pat", "/work"),
            [
                "/Users/pat/Library/Application Support/io.github.tousifhabib.baseball/extracted",
                "/work/target/release/extracted",
                "/work/extracted",
            ]
        );
    }

    #[test]
    fn the_same_place_is_not_looked_in_twice() {
        assert_eq!(
            places("/work/bb-game", "/Users/pat", "/work"),
            [
                "/Users/pat/Library/Application Support/io.github.tousifhabib.baseball/extracted",
                "/work/extracted",
            ]
        );
    }

    #[test]
    fn nothing_known_means_nowhere_to_look() {
        assert!(candidates(None, None, None).is_empty());
    }

    #[test]
    fn a_folder_asked_for_by_name_must_hold_the_art() {
        let missing = Path::new("/nowhere/at/all");
        let error = find(Some(missing)).unwrap_err().to_string();
        assert!(error.contains("/nowhere/at/all"), "{error}");
    }
}
