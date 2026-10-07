//! What the player chose before the game began.

use crate::look::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    /// The art's name for this difficulty, as it labels the frames of the
    /// marker on the setup pages.
    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Medium => "medium",
            Difficulty::Hard => "hard",
        }
    }
}

/// The choices made on the setup pages. They last from one game to the next.
#[derive(Clone, Debug)]
pub struct Settings {
    pub difficulty: Difficulty,
    /// The colour of the team's shirts and helmets, if one has been picked.
    pub clothes: Option<Rgb>,
    /// The arcade batter's skin, if one has been picked.
    pub skin: Option<Rgb>,
    /// The logo on the arcade batter's bat, if one has been picked.
    pub logo: Option<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            difficulty: Difficulty::Medium,
            clothes: None,
            skin: None,
            logo: None,
        }
    }
}
