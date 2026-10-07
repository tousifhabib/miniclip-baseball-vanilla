//! What the player chose before the game began.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

/// How many outs a side gets in its innings.
pub const OUTS: u32 = 3;

impl Difficulty {
    /// In a match you come to bat in the last innings already behind. This
    /// is by how many runs.
    pub fn runs_down(self) -> u32 {
        match self {
            Difficulty::Easy => 1,
            Difficulty::Medium => 2,
            Difficulty::Hard => 3,
        }
    }

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
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            difficulty: Difficulty::Medium,
        }
    }
}
