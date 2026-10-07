//! The pitch: where it goes, and what a swing does to it.
//!
//! Nothing here touches the art. A pitch is worked out in full before it is
//! thrown, as a list of where the ball and its shadow are on each frame, so
//! that the player can be shown where it will cross before it leaves the
//! pitcher's hand.

use serde::Deserialize;

use crate::rng::Rng;
use crate::rules::{PitchRules, ThrowRules};

/// How well the bat met the ball.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Poor,
    MediumPoor,
    Medium,
    Good,
}

/// A point of the batting view, in pixels.
pub type Point = (f32, f32);

/// The fixed points a pitch is drawn between, taken from the art.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mound {
    /// Where the ball and its shadow are as they leave the hand.
    pub ball: Point,
    pub shadow: Point,
    /// The points the art measures the ball's journey from.
    pub ball_from: Point,
    pub shadow_from: Point,
    /// The height on screen at which the shadow passes the batter.
    pub plate: f32,
    /// The strike zone: left, top, right, bottom.
    pub zone: [f32; 4],
}

/// Where the ball and its shadow are on one frame of a pitch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub ball: Point,
    pub shadow: Point,
    /// The size to draw both at, 1 being the art's own.
    pub size: f32,
    /// How solid both are, from 1 down to 0 as they fade past the batter.
    pub alpha: f32,
}

/// One pitch, worked out from the hand to past the batter.
#[derive(Clone, Debug, PartialEq)]
pub struct Pitch {
    pub samples: Vec<Sample>,
    /// Where the ball is as its shadow passes the batter.
    pub crosses: Point,
    /// Whether that is inside the strike zone.
    pub in_zone: bool,
}

/// What the pitcher has decided to throw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choice {
    pub speed: f32,
    pub swing: f32,
    pub dip: f32,
    pub aim: Point,
}

impl Choice {
    /// Picks a pitch for this skill level.
    pub fn pick(rules: &PitchRules, throw: &ThrowRules, rng: &mut Rng) -> Choice {
        let speed = rules.speed.low + rng.below(rules.speed.high - rules.speed.low + 1);
        let mut curve = |curve: &crate::rules::Curve| {
            if curve.over == 0.0 {
                curve.base
            } else {
                curve.base + curve.over / (rng.below(curve.parts.max(1)) + 1) as f32
            }
        };
        let (swing, dip) = (curve(&rules.swing), curve(&rules.dip));
        let target = &rules.target;
        // He aims off to allow for the curve.
        let aim = (
            target.x + rng.below(target.width as u32) as f32 - swing * throw.swing_lead,
            target.y + rng.below(target.height as u32) as f32 + dip * throw.dip_lead,
        );
        Choice {
            speed: speed as f32,
            swing,
            dip,
            aim,
        }
    }
}

impl Pitch {
    /// Works out the whole of a pitch.
    pub fn throw(choice: &Choice, mound: &Mound, rules: &ThrowRules) -> Pitch {
        let (mut ball, mut shadow) = (mound.ball, mound.shadow);
        // The whole way each has to go, to the aim and to the ground under
        // it at the batter.
        let way = (
            choice.aim.0 - mound.ball_from.0,
            choice.aim.1 - mound.ball_from.1,
        );
        let shadow_way = (
            choice.aim.0 - mound.shadow_from.0,
            mound.plate - mound.shadow_from.1,
        );
        let mut pitch = Pitch {
            samples: Vec::new(),
            crosses: choice.aim,
            in_zone: false,
        };
        let mut alpha = 1.0f32;
        let mut crossed = false;
        // No pitch takes anything like this long: it is a guard against
        // numbers in a data file that would never bring the ball in.
        for _ in 0..3000 {
            // The nearer the ball, the faster it comes on.
            let come = (shadow.1 - mound.shadow_from.1).max(0.0);
            let step = come / rules.approach / choice.speed;
            ball.0 += way.0 * step + choice.swing;
            ball.1 += way.1 * step - choice.dip;
            shadow.0 += shadow_way.0 * step + choice.swing;
            shadow.1 += shadow_way.1 * step;
            if shadow.1 >= mound.plate {
                alpha -= rules.fade;
                if !crossed {
                    crossed = true;
                    pitch.crosses = ball;
                    let [left, top, right, bottom] = mound.zone;
                    pitch.in_zone =
                        (left..=right).contains(&ball.0) && (top..=bottom).contains(&ball.1);
                }
            }
            pitch.samples.push(Sample {
                ball,
                shadow,
                size: rules.size + come * rules.growth,
                alpha: alpha.max(0.0),
            });
            if alpha <= 0.0 {
                break;
            }
        }
        pitch
    }
}

/// What a swing made this many frames ago does to a ball in the band now:
/// how well it is met and with what power, or `None` for a miss.
pub fn meets(rules: &PitchRules, frames_since_swing: u32) -> Option<(Quality, f32)> {
    rules
        .window
        .iter()
        .find(|(frames, _, _)| *frames == frames_since_swing)
        .map(|&(_, quality, power)| (quality, power))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::settings::Difficulty;

    /// The fixed points as they are in the game's art.
    pub(crate) fn mound() -> Mound {
        Mound {
            ball: (286.1, 137.85),
            shadow: (286.1, 233.35),
            ball_from: (285.85, 137.8),
            shadow_from: (285.85, 230.35),
            plate: 351.0,
            zone: [246.2, 190.6, 341.1, 305.7],
        }
    }

    fn straight(aim: Point, speed: f32) -> Choice {
        Choice {
            speed,
            swing: 0.0,
            dip: 0.0,
            aim,
        }
    }

    #[test]
    fn a_straight_pitch_crosses_where_it_was_aimed() {
        let rules = Rules::default();
        let pitch = Pitch::throw(&straight((300.0, 250.0), 70.0), &mound(), &rules.throw);
        assert!((pitch.crosses.0 - 300.0).abs() < 4.0, "{:?}", pitch.crosses);
        assert!((pitch.crosses.1 - 250.0).abs() < 8.0, "{:?}", pitch.crosses);
        assert!(pitch.in_zone);
    }

    #[test]
    fn a_pitch_aimed_wide_is_outside_the_zone() {
        let rules = Rules::default();
        let pitch = Pitch::throw(&straight((400.0, 250.0), 70.0), &mound(), &rules.throw);
        assert!(!pitch.in_zone);
    }

    #[test]
    fn the_ball_comes_on_faster_and_larger_and_then_fades() {
        let rules = Rules::default();
        let pitch = Pitch::throw(&straight((300.0, 250.0), 70.0), &mound(), &rules.throw);
        let samples = &pitch.samples;
        let early = samples[10].shadow.1 - samples[9].shadow.1;
        let late = samples[samples.len() - 12].shadow.1 - samples[samples.len() - 13].shadow.1;
        assert!(late > early * 5.0, "{early} then {late}");
        assert!(samples.last().unwrap().size > samples[0].size * 2.0);
        assert_eq!(samples[0].alpha, 1.0);
        assert_eq!(samples.last().unwrap().alpha, 0.0);
    }

    #[test]
    fn a_slower_setting_takes_more_frames() {
        let rules = Rules::default();
        let fast = Pitch::throw(&straight((300.0, 250.0), 35.0), &mound(), &rules.throw);
        let slow = Pitch::throw(&straight((300.0, 250.0), 70.0), &mound(), &rules.throw);
        assert!(slow.samples.len() > fast.samples.len());
        // Sanity: a pitch is a second or three, not an age.
        assert!(
            (40..400).contains(&slow.samples.len()),
            "{}",
            slow.samples.len()
        );
    }

    #[test]
    fn swing_carries_the_ball_sideways() {
        let rules = Rules::default();
        let mut curving = straight((300.0, 250.0), 60.0);
        curving.swing = 0.5;
        let straight = Pitch::throw(&straight((300.0, 250.0), 60.0), &mound(), &rules.throw);
        let curved = Pitch::throw(&curving, &mound(), &rules.throw);
        assert!(curved.crosses.0 > straight.crosses.0 + 10.0);
    }

    #[test]
    fn the_pitcher_keeps_to_what_the_rules_allow() {
        let rules = Rules::default();
        let mut rng = Rng::new(9);
        for difficulty in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
            let table = rules.pitch.at(difficulty);
            for _ in 0..200 {
                let choice = Choice::pick(table, &rules.throw, &mut rng);
                assert!((table.speed.low as f32..=table.speed.high as f32).contains(&choice.speed));
                let pitch = Pitch::throw(&choice, &mound(), &rules.throw);
                assert!(pitch.samples.len() < 1000, "{choice:?}");
            }
        }
        // On easy the ball is thrown straight.
        let easy = Choice::pick(rules.pitch.at(Difficulty::Easy), &rules.throw, &mut rng);
        assert_eq!((easy.swing, easy.dip), (0.0, 0.0));
    }

    #[test]
    fn a_swing_only_meets_the_ball_inside_its_window() {
        let rules = Rules::default();
        let easy = rules.pitch.at(Difficulty::Easy);
        assert_eq!(meets(easy, 6), None);
        assert_eq!(meets(easy, 7), Some((Quality::MediumPoor, 18.0)));
        assert_eq!(meets(easy, 11), Some((Quality::Good, 14.0)));
        assert_eq!(meets(easy, 14), None);
        // The harder the level, the fewer frames there are to hit in.
        let frames = |d| rules.pitch.at(d).window.len();
        assert!(frames(Difficulty::Easy) > frames(Difficulty::Medium));
        assert!(frames(Difficulty::Medium) > frames(Difficulty::Hard));
    }
}
