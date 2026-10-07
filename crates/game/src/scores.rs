//! The high-score table.
//!
//! The original's table was kept on its publisher's servers and shown by a
//! file loaded from there, which only happened on their site. This one is a
//! file on the player's own machine.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::locate::APP_ID;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub points: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scores {
    /// The best first.
    #[serde(default)]
    pub entries: Vec<Entry>,
}

impl Scores {
    /// How many scores are kept.
    pub const KEPT: usize = 10;

    /// Where the table is kept: in the game's folder under Application
    /// Support.
    pub fn usual_file() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support")
                .join(APP_ID)
                .join("scores.toml"),
        )
    }

    /// Reads the table. A file that is missing or cannot be read is an empty
    /// table: a damaged file should not stop anyone playing.
    pub fn load(file: &Path) -> Scores {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Puts a score in its place. Among equal scores the earlier stays
    /// ahead. Returns whether it made the table.
    pub fn add(&mut self, name: &str, points: u32) -> bool {
        let place = self
            .entries
            .iter()
            .position(|entry| entry.points < points)
            .unwrap_or(self.entries.len());
        if place >= Scores::KEPT {
            return false;
        }
        self.entries.insert(
            place,
            Entry {
                name: name.to_owned(),
                points,
            },
        );
        self.entries.truncate(Scores::KEPT);
        true
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)
                .with_context(|| format!("making {}", folder.display()))?;
        }
        let text = toml::to_string(self).context("writing out the scores")?;
        std::fs::write(file, text).with_context(|| format!("writing {}", file.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_are_kept_best_first_and_no_more_than_ten() {
        let mut scores = Scores::default();
        for points in [50, 300, 100, 300, 0] {
            assert!(scores.add(&format!("p{points}"), points));
        }
        let order: Vec<u32> = scores.entries.iter().map(|entry| entry.points).collect();
        assert_eq!(order, [300, 300, 100, 50, 0]);
        for points in 1..=20 {
            scores.add("more", 1000 + points);
        }
        assert_eq!(scores.entries.len(), Scores::KEPT);
        assert_eq!(scores.entries[0].points, 1020);
        // Too low for a full table.
        assert!(!scores.add("late", 5));
    }

    #[test]
    fn of_two_equal_scores_the_earlier_stays_ahead() {
        let mut scores = Scores::default();
        scores.add("first", 100);
        scores.add("second", 100);
        assert_eq!(scores.entries[0].name, "first");
    }

    #[test]
    fn the_table_comes_back_as_it_was_saved() {
        let file = std::env::temp_dir().join(format!("bb-scores-{}.toml", std::process::id()));
        let mut scores = Scores::default();
        scores.add("Red Sox", 450);
        scores.add("A \"quoted\" name", 75);
        scores.save(&file).unwrap();
        assert_eq!(Scores::load(&file), scores);
        std::fs::remove_file(&file).unwrap();
        // No file, or nonsense in it, is an empty table.
        assert_eq!(Scores::load(&file), Scores::default());
        std::fs::write(&file, "not a table at all [").unwrap();
        assert_eq!(Scores::load(&file), Scores::default());
        std::fs::remove_file(&file).unwrap();
    }
}
