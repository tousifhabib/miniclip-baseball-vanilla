//! The arcade game: a fixed number of pitches, and a target on the outfield
//! to drop the ball on.
//!
//! Pitching and batting are the match's. What differs is here: there is no
//! count and nobody fields, and a ball that is hit scores by the ring of the
//! target it comes down in.

use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;

use super::field::{Happened, distance};
use super::pitch::Point;
use super::{AtBat, Match, Parts, at, put, show};
use crate::menu::Game;
use crate::rules::ArcadeRules;

pub(crate) struct Arcade {
    /// Pitches still to come.
    pub left: u32,
    pub points: u32,
    /// The middle of the target, in the overhead field's pixels.
    target: Point,
    /// Which rings have scored on this pitch, from the centre out.
    lit: Vec<bool>,
    /// How far from the target the ball was a frame ago, as the rings
    /// measure it.
    last_distance: f32,
    /// The ball has gone over the wall, and is taken away when it lands.
    cleared: bool,
    /// The overhead view is up and the ball is being followed.
    flying: bool,
}

impl Arcade {
    pub fn new(pitches: u32) -> Arcade {
        Arcade {
            left: pitches,
            points: 0,
            target: (0.0, 0.0),
            lit: Vec::new(),
            last_distance: f32::MAX,
            cleared: false,
            flying: false,
        }
    }

    /// How far a point on the grass is from the target as the rings count
    /// it. The target is drawn lying flat, so a miss up or down the field
    /// counts for more than one to the side.
    fn off_target(&self, ball: Point, rules: &ArcadeRules) -> f32 {
        distance(ball, self.target) + rules.depth_weight * (ball.1 - self.target.1).abs()
    }

    /// Scores a ball that has just touched the ground `off` from the target.
    /// Returns the ring it lit, counting from the centre, and the points.
    fn touch(&mut self, off: f32, rules: &ArcadeRules) -> Option<(usize, u32)> {
        let ring = rules.rings.iter().position(|ring| off <= ring.within)?;
        if self.lit.get(ring).copied().unwrap_or(true) {
            return None;
        }
        // The first ring scored on a pitch counts extra.
        let first = !self.lit.iter().any(|&lit| lit);
        self.lit[ring] = true;
        let points = rules.rings[ring].points * if first { rules.first_bonus } else { 1 };
        self.points += points;
        Some((ring, points))
    }
}

impl Match {
    /// Gets the arcade game's own parts of a new batting view ready.
    pub(crate) fn set_up_arcade(
        &mut self,
        parts: &Parts,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let rules = &game.rules.arcade;
        let Some(arcade) = &mut self.arcade else {
            return;
        };
        arcade.lit = vec![false; rules.rings.len()];
        arcade.last_distance = f32::MAX;
        arcade.cleared = false;
        arcade.flying = false;
        let area = &rules.target;
        arcade.target = (
            area.x + self.rng.below(area.width as u32) as f32,
            area.y + self.rng.below(area.height as u32) as f32,
        );
        let target = arcade.target;
        let left = arcade.left;

        if let Some(mark) = stage.find(&parts.field, &["landMarker"]) {
            let y = at(stage, &mark);
            let _ = y;
            if let Some(child) = stage.child_mut(&mark) {
                child.move_to(target.0, target.1);
            }
        }
        // The batting view has a copy of the target lying on the outfield,
        // drawn smaller and flatter the further up the field it is.
        let in_field = |name: &str| {
            stage
                .find(&parts.field, &[name])
                .map(|path| at(stage, &path))
        };
        let centre = in_field("centreMarker").map_or(301.45, |at| at.0);
        let back = in_field("bMarker").map_or(108.45, |at| at.1);
        let horizon = stage
            .find(&parts.main, &["hMarker"])
            .map_or(176.1, |path| at(stage, &path).1);
        let (across, depth) = (target.0 - centre, target.1 - back);
        if let Some(copy) = stage.find(&parts.main, &["landMarker"])
            && let Some(child) = stage.child_mut(&copy)
        {
            child.set_matrix(Matrix {
                a: (50.0 + depth) / 100.0,
                d: (10.0 + depth / 5.0) / 100.0,
                tx: parts.centre_x + 1.3 * across,
                ty: horizon + 0.3 * depth,
                ..Matrix::IDENTITY
            });
        }
        // One ball lit for every pitch still to come, this one included.
        if let Some(row) = stage.find(&parts.main, &["onScreen_ballsLeft"]) {
            stage.goto_clip(&row, left.max(1) as u16, library);
            if let Some(clip) = stage.clip_mut(&row) {
                clip.playing = false;
            }
        }
    }

    /// Changes the view to the overhead field, where the ball is followed
    /// to the target. The next pitch is on offer from this moment.
    pub(crate) fn show_arcade_field(
        &mut self,
        at_bat: &mut AtBat,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let parts = at_bat.parts.clone();
        let y = at(stage, &parts.field).1;
        if let Some(field) = stage.child_mut(&parts.field) {
            field.move_to(game.rules.field.x, y);
        }
        if let Some(arcade) = &mut self.arcade {
            arcade.flying = true;
        }
        self.ready(&parts, stage, library);
    }

    /// One frame of the ball over the arcade game's field.
    pub(crate) fn arcade_ball(
        &mut self,
        at_bat: &mut AtBat,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let rules = &game.rules;
        let (Some(arcade), Some(ball), Some(contact)) =
            (&mut self.arcade, &mut at_bat.ball, at_bat.contact)
        else {
            return;
        };
        if !arcade.flying {
            return;
        }
        let parts = &at_bat.parts;
        let happened = ball.step(parts.home, contact.miss(), &rules.field);
        let size = (0.6 + (ball.at.1 - parts.home.1) / 1000.0).max(0.1);
        put(stage, &parts.field_ball, ball.at, size);
        if let Some(inner) = stage.child_mut(&parts.field_ball_inner) {
            inner.move_to(inner.matrix.tx, -ball.height);
        }
        let mut scored = None;
        match happened {
            Happened::Cleared => arcade.cleared = true,
            Happened::Landed if arcade.cleared => show(stage, &parts.field_ball, false),
            Happened::Landed => scored = arcade.touch(arcade.last_distance, &rules.arcade),
            _ => {}
        }
        arcade.last_distance = arcade.off_target(ball.at, &rules.arcade);

        if let Some((ring, points)) = scored {
            // The art numbers its rings from the outside in.
            let name = format!("ring{}", rules.arcade.rings.len() - ring);
            if let Some(path) = stage.find(&parts.field, &["landMarker", &name]) {
                stage.goto_clip(&path, 2, library);
            }
            stage.set_text("thisScore", points.to_string());
            if let Some(pulse) = stage.find(&parts.main, &["onScreenScore", "scoreAnim"]) {
                stage.goto_clip(&pulse, 2, library);
                if let Some(clip) = stage.clip_mut(&pulse) {
                    clip.playing = true;
                }
            }
            self.show_numbers(stage);
        }
    }

    /// The arcade game's points and what they come to with the skill level
    /// counted in, for the finish screen.
    pub fn show_arcade_result(&self, game: &Game, stage: &mut Stage) {
        let points = self.arcade.as_ref().map_or(0, |arcade| arcade.points);
        let times = game.rules.arcade.multiplier.at(game.settings.difficulty);
        stage.set_text("points_total", points.to_string());
        stage.set_text("points_final", (points * times).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;

    fn arcade() -> (Arcade, Rules) {
        let rules = Rules::default();
        let mut arcade = Arcade::new(rules.arcade.pitches);
        arcade.target = (200.0, 180.0);
        arcade.lit = vec![false; rules.arcade.rings.len()];
        (arcade, rules)
    }

    #[test]
    fn the_first_ring_scored_on_a_pitch_counts_double() {
        let (mut arcade, rules) = arcade();
        // Dead centre first: the middle ring, doubled.
        assert_eq!(arcade.touch(5.0, &rules.arcade), Some((0, 200)));
        // A later hop into the next ring out scores once.
        assert_eq!(arcade.touch(30.0, &rules.arcade), Some((1, 75)));
        assert_eq!(arcade.points, 275);
    }

    #[test]
    fn a_ring_scores_only_once_a_pitch() {
        let (mut arcade, rules) = arcade();
        assert!(arcade.touch(50.0, &rules.arcade).is_some());
        assert_eq!(arcade.touch(50.0, &rules.arcade), None);
    }

    #[test]
    fn a_ball_that_comes_down_wide_scores_nothing() {
        let (mut arcade, rules) = arcade();
        assert_eq!(arcade.touch(86.0, &rules.arcade), None);
        assert_eq!(arcade.points, 0);
    }

    #[test]
    fn a_miss_up_the_field_counts_for_more_than_one_to_the_side() {
        let (arcade, rules) = arcade();
        let aside = arcade.off_target((230.0, 180.0), &rules.arcade);
        let beyond = arcade.off_target((200.0, 150.0), &rules.arcade);
        assert_eq!(aside, 30.0);
        assert!(beyond > aside * 2.0);
    }

    #[test]
    fn the_best_a_pitch_can_score_is_every_ring_with_the_centre_first() {
        let (mut arcade, rules) = arcade();
        for off in [5.0, 30.0, 50.0, 80.0] {
            arcade.touch(off, &rules.arcade);
        }
        assert_eq!(arcade.points, 350);
    }
}
