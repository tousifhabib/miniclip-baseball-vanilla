//! The numbers the game is played by, read from data files so that they can
//! be changed without touching the code.
//!
//! `data/rules.toml` holds every number and is built into the program. A mod
//! supplies a file of the same shape holding only what it changes, and the
//! files are laid over one another in order: the last to name a number wins.

use anyhow::{Context, Result};
use serde::Deserialize;
use toml::{Table, Value};

use crate::play::pitch::Quality;
use crate::settings::Difficulty;

/// The file that holds every number, as built into the program.
const BUILT_IN: &str = include_str!("../../../data/rules.toml");

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// The match: batting in the last innings to overtake the other side.
    #[serde(rename = "match")]
    pub game: MatchRules,
    pub count: CountRules,
    /// The pitch, for each skill level.
    pub pitch: BySkillRef<PitchRules>,
    /// What every pitch shares.
    pub throw: ThrowRules,
    /// The ball off the bat.
    pub hit: HitRules,
    /// The ball over the field.
    pub field: FieldRules,
    pub arcade: ArcadeRules,
    pub team: TeamRules,
    pub sound: SoundRules,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundRules {
    /// How loud each sound is, by its name in the art, from 0 to 1.
    pub levels: std::collections::BTreeMap<String, f32>,
    pub field_catch: f32,
    pub music: String,
    pub crowd: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamRules {
    /// Colours written as `#rrggbb`.
    pub skins: Vec<String>,
    /// Frame labels of the art's logo clip.
    pub logos: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArcadeRules {
    pub pitches: u32,
    pub watch: u32,
    pub target: Area,
    pub depth_weight: f32,
    /// From the centre out.
    pub rings: Vec<Ring>,
    pub first_bonus: u32,
    pub multiplier: BySkill<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ring {
    pub over: f32,
    pub within: f32,
    pub points: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountRules {
    pub strikes: u32,
    pub balls: u32,
}

/// A whole number picked from `low` to `high`, both included.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub low: u32,
    pub high: u32,
}

/// A number worked out as `base + over / n`, with n picked from 1 to
/// `parts`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curve {
    pub base: f32,
    pub over: f32,
    pub parts: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Band {
    pub top: f32,
    pub bottom: f32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PitchRules {
    pub speed: Span,
    pub swing: Curve,
    pub dip: Curve,
    pub target: Area,
    pub marker_frame: u16,
    pub aim_ease: f32,
    pub show_zone: bool,
    pub band: Band,
    /// Frames after the swing, how well the ball is met, and the power.
    pub window: Vec<(u32, Quality, f32)>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThrowRules {
    pub settle: u32,
    pub release_frame: u16,
    pub swing_lead: f32,
    pub dip_lead: f32,
    pub approach: f32,
    pub size: f32,
    pub growth: f32,
    pub fade: f32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HitRules {
    pub pull: f32,
    pub pointer_before_pitch: bool,
    pub watch: u32,
    pub walk_wait: u32,
    pub lift: f32,
    pub lift_aim: f32,
    pub power_drag: f32,
    pub gravity: f32,
    pub bounce: f32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRules {
    pub x: f32,
    pub pace: f32,
    pub aim_share: f32,
    pub lift_share: f32,
    pub gravity: f32,
    pub drag: f32,
    pub drag_aim: f32,
    pub drag_reach: f32,
    pub bounce_run: f32,
    pub bounce_lift: f32,
    pub bounce_cap: f32,
    pub bounce_loss: f32,
    pub wall: f32,
    pub clear: f32,
    pub wall_bounce: f32,
    pub fielder_reach: f32,
    pub catch_height: f32,
    pub throw_speed: f32,
    pub throw_near: f32,
    pub pick_time: u32,
    pub throw_time: u32,
    pub longest: u32,
    pub fielder_speed: BySkill<f32>,
}

/// A table that differs with the skill level chosen.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BySkillRef<T> {
    pub easy: T,
    pub medium: T,
    pub hard: T,
}

impl<T> BySkillRef<T> {
    pub fn at(&self, difficulty: Difficulty) -> &T {
        match difficulty {
            Difficulty::Easy => &self.easy,
            Difficulty::Medium => &self.medium,
            Difficulty::Hard => &self.hard,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRules {
    /// Outs a side gets in its innings.
    pub outs: u32,
    /// How many runs behind the player starts.
    pub runs_down: BySkill<u32>,
}

/// A number that differs with the skill level chosen.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BySkill<T> {
    pub easy: T,
    pub medium: T,
    pub hard: T,
}

impl<T: Copy> BySkill<T> {
    pub fn at(&self, difficulty: Difficulty) -> T {
        match difficulty {
            Difficulty::Easy => self.easy,
            Difficulty::Medium => self.medium,
            Difficulty::Hard => self.hard,
        }
    }
}

impl Default for Rules {
    /// The rules as built in, with nothing laid over them.
    fn default() -> Rules {
        Rules::layered(&[]).expect("the built-in rules are tested to be sound")
    }
}

impl Rules {
    /// The built-in rules with each of `layers` laid over them in turn.
    /// Each layer is the name of where it came from, for reporting a
    /// mistake in it, and its text.
    pub fn layered(layers: &[(&str, &str)]) -> Result<Rules> {
        let mut all: Table = BUILT_IN.parse().context("reading the built-in rules")?;
        for (name, text) in layers {
            let layer: Table = text.parse().with_context(|| format!("reading {name}"))?;
            lay_over(&mut all, layer);
            // Checked after every layer, so that a mistake is laid at the
            // door of the file that made it.
            Rules::from_table(all.clone()).with_context(|| format!("in {name}"))?;
        }
        Rules::from_table(all).context("in the built-in rules")
    }

    fn from_table(table: Table) -> Result<Rules> {
        Ok(table.try_into()?)
    }
}

/// Puts everything in `layer` into `base`. A table is merged with the table
/// already there, so that naming one number in it leaves its other numbers
/// alone. Anything else replaces what was there.
fn lay_over(base: &mut Table, layer: Table) {
    for (key, value) in layer {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(under)), Value::Table(over)) => lay_over(under, over),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_rules_are_sound() {
        let rules = Rules::layered(&[]).unwrap();
        assert_eq!(rules.game.outs, 3);
        assert_eq!(rules.game.runs_down.at(Difficulty::Hard), 3);
    }

    #[test]
    fn a_layer_changes_only_what_it_names() {
        let rules = Rules::layered(&[("a mod", "[match.runs_down]\nhard = 5\n")]).unwrap();
        assert_eq!(rules.game.runs_down.at(Difficulty::Hard), 5);
        assert_eq!(rules.game.runs_down.at(Difficulty::Easy), 1);
        assert_eq!(rules.game.outs, 3);
    }

    #[test]
    fn the_last_layer_to_name_a_number_wins() {
        let rules = Rules::layered(&[
            ("first", "[match]\nouts = 1\n"),
            ("second", "[match]\nouts = 2\n"),
        ])
        .unwrap();
        assert_eq!(rules.game.outs, 2);
    }

    #[test]
    fn a_number_the_game_does_not_have_is_refused_by_name() {
        let error = Rules::layered(&[("typo.toml", "[match]\nouts_allowed = 4\n")]).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("typo.toml"), "{message}");
        assert!(message.contains("outs_allowed"), "{message}");
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused() {
        let error = Rules::layered(&[("wrong.toml", "[match]\nouts = \"three\"\n")]).unwrap_err();
        assert!(format!("{error:#}").contains("wrong.toml"));
    }

    #[test]
    fn a_file_that_is_not_toml_is_refused_with_its_name() {
        let error = Rules::layered(&[("broken.toml", "[match\nouts = 3")]).unwrap_err();
        assert!(format!("{error:#}").contains("broken.toml"));
    }

    #[test]
    fn a_mistake_is_blamed_on_the_layer_that_made_it() {
        let error = Rules::layered(&[
            ("good.toml", "[match]\nouts = 4\n"),
            ("bad.toml", "[match]\nnonsense = 1\n"),
        ])
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("bad.toml"), "{message}");
        assert!(!message.contains("good.toml"), "{message}");
    }
}
