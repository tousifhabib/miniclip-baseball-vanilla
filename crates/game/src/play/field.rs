//! The ball over the field: how it flies, bounces and meets the wall.
//!
//! Nothing here touches the art. Positions are in the overhead field's own
//! pixels, and height is in the same pixels, up from the grass.

use crate::play::pitch::Point;
use crate::rules::{FieldRules, HitRules};

/// What the bat did to the ball.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// From the swing's timing. A lower power sends the ball further.
    pub power: f32,
    /// How far below the ball the ring was, in pixels. Above is negative.
    pub under: f32,
    /// How far to the side of straight the ball is sent, in pixels of the
    /// batting view.
    pub aside: f32,
}

impl Contact {
    /// How far off the ball's height the ring was, either way.
    pub fn miss(&self) -> f32 {
        self.under.abs()
    }

    /// How hard the ball leaves the bat upwards, in the batting view.
    pub fn lift(&self, rules: &HitRules) -> f32 {
        rules.lift + self.under / rules.lift_aim - self.power / rules.power_drag
    }
}

/// What happened to the ball on one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Happened {
    Nothing,
    /// It came down and bounced.
    Landed,
    /// It reached the wall too low, and came back off it.
    HitWall,
    /// It cleared the wall: a home run, if the hit was fair.
    Cleared,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    pub at: Point,
    pub speed: Point,
    /// How far above the grass.
    pub height: f32,
    /// How fast it is going up. Negative coming down.
    pub lift: f32,
    pub bounced: bool,
    /// It has been to the wall, one way or the other.
    pub walled: bool,
}

/// How far a point is from home plate as the game measures a hit: the plain
/// distance, allowing for the field being drawn at a slant.
pub fn reach(home: Point, at: Point) -> f32 {
    let plain = ((at.0 - home.0).powi(2) + (at.1 - home.1).powi(2)).sqrt();
    plain - ((at.1 - home.1) * 3.0 + at.0 / 5.6)
}

pub fn distance(a: Point, b: Point) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

impl Ball {
    /// The ball as it leaves the bat, heading for `mark`.
    pub fn hit(
        home: Point,
        mark: Point,
        contact: &Contact,
        hit: &HitRules,
        rules: &FieldRules,
    ) -> Ball {
        let frames = contact.power * rules.pace;
        Ball {
            at: home,
            speed: ((mark.0 - home.0) / frames, (mark.1 - home.1) / frames),
            height: 0.0,
            lift: contact.lift(hit) * rules.lift_share,
            bounced: false,
            walled: false,
        }
    }

    /// Moves the ball on by one frame. `miss` is how far off the ball's
    /// height the ring was: a ball hit off-centre is slowed by the air more.
    pub fn step(&mut self, home: Point, miss: f32, rules: &FieldRules) -> Happened {
        self.at.0 += self.speed.0;
        self.at.1 += self.speed.1;
        let far = distance(home, self.at);
        // Never more than all of it, or a mishit far from home would turn
        // round in the air.
        let lost = (rules.drag * (miss / rules.drag_aim) * (far * far / rules.drag_reach)).min(1.0);
        self.speed.0 -= self.speed.0 * lost;
        self.speed.1 -= self.speed.1 * lost;
        self.height += self.lift;
        self.lift -= rules.gravity;

        let mut happened = Happened::Nothing;
        if self.height < 0.0 {
            self.height = -self.height;
            self.lift = (-self.lift * rules.bounce_lift).min(rules.bounce_cap) - rules.bounce_loss;
            self.speed.0 *= rules.bounce_run;
            self.speed.1 *= rules.bounce_run;
            self.bounced = true;
            happened = Happened::Landed;
        }
        if !self.walled && reach(home, self.at) >= rules.wall {
            self.walled = true;
            if self.height > rules.clear {
                return Happened::Cleared;
            }
            self.speed.0 *= -rules.wall_bounce;
            self.speed.1 *= -rules.wall_bounce;
            return Happened::HitWall;
        }
        happened
    }

    /// Where the ball will first come down, if it is left alone.
    pub fn landing(&self, home: Point, miss: f32, rules: &FieldRules) -> Point {
        let mut ball = *self;
        for _ in 0..600 {
            match ball.step(home, miss, rules) {
                Happened::Nothing => {}
                // Wherever it stops being in the air over the field.
                _ => break,
            }
        }
        ball.at
    }
}

/// The eight ways a fielder can face, by where he is headed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    Left,
    Right,
    Up,
    Down,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl Facing {
    /// The way from `from` to `to`. Up the screen is away from home plate.
    pub fn towards(from: Point, to: Point) -> Facing {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        // Eighths of a turn, starting from due right and going clockwise on
        // screen.
        let eighth = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() as i32;
        match eighth.rem_euclid(8) {
            0 => Facing::Right,
            1 => Facing::DownRight,
            2 => Facing::Down,
            3 => Facing::DownLeft,
            4 => Facing::Left,
            5 => Facing::UpLeft,
            6 => Facing::Up,
            _ => Facing::UpRight,
        }
    }

    /// The fielder's frame label for running this way.
    pub fn run_label(self) -> &'static str {
        match self {
            Facing::Left => "left",
            Facing::Right => "right",
            Facing::Up => "up",
            Facing::Down => "down",
            Facing::UpLeft => "upLeft",
            Facing::UpRight => "upRight",
            Facing::DownLeft => "downLeft",
            Facing::DownRight => "downRight",
        }
    }

    /// The label for picking the ball up after running this way. The art
    /// has four of these.
    pub fn pick_label(self) -> &'static str {
        match self {
            Facing::Left | Facing::UpLeft | Facing::DownLeft => "pickLeft",
            Facing::Right | Facing::UpRight | Facing::DownRight => "pickRight",
            Facing::Up => "pickUp",
            Facing::Down => "pickDown",
        }
    }

    /// The label for throwing this way. The art throws in four directions.
    pub fn throw_label(self) -> &'static str {
        match self {
            Facing::Left | Facing::UpLeft | Facing::DownLeft => "throwLeft",
            Facing::Right | Facing::UpRight | Facing::DownRight => "throwRight",
            Facing::Up => "throwUp",
            Facing::Down => "throwDown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;

    const HOME: Point = (240.8, 336.85);
    const STRAIGHT: Point = (303.8, 168.7);

    fn well_hit() -> Contact {
        Contact {
            power: 14.0,
            under: 0.0,
            aside: 0.0,
        }
    }

    fn fly(contact: &Contact, mark: Point) -> (Vec<Happened>, Ball) {
        let rules = Rules::default();
        let mut ball = Ball::hit(HOME, mark, contact, &rules.hit, &rules.field);
        let mut seen = Vec::new();
        for _ in 0..900 {
            let happened = ball.step(HOME, contact.miss(), &rules.field);
            if happened != Happened::Nothing {
                seen.push(happened);
            }
        }
        (seen, ball)
    }

    #[test]
    fn a_ball_met_squarely_clears_the_wall() {
        let (seen, _) = fly(&well_hit(), STRAIGHT);
        assert_eq!(seen.first(), Some(&Happened::Cleared), "{seen:?}");
    }

    #[test]
    fn a_weak_hit_lands_in_the_field_and_stays_there() {
        let weak = Contact {
            power: 30.0,
            under: -30.0,
            aside: 0.0,
        };
        let (seen, ball) = fly(&weak, STRAIGHT);
        assert_eq!(seen.first(), Some(&Happened::Landed), "{seen:?}");
        assert!(!seen.contains(&Happened::Cleared));
        assert!(ball.bounced);
        let rules = Rules::default();
        assert!(reach(HOME, ball.at) < rules.field.wall);
    }

    #[test]
    fn the_landing_is_where_the_ball_first_comes_down() {
        let rules = Rules::default();
        let weak = Contact {
            power: 25.0,
            under: -20.0,
            aside: 0.0,
        };
        let start = Ball::hit(HOME, STRAIGHT, &weak, &rules.hit, &rules.field);
        let landing = start.landing(HOME, weak.miss(), &rules.field);
        let mut ball = start;
        while ball.step(HOME, weak.miss(), &rules.field) == Happened::Nothing {}
        assert_eq!(ball.at, landing);
        // It went up the field, away from home.
        assert!(landing.1 < HOME.1);
    }

    #[test]
    fn swinging_under_the_ball_lifts_it_more() {
        let rules = Rules::default();
        let level = well_hit();
        let under = Contact {
            under: 40.0,
            ..level
        };
        assert!(under.lift(&rules.hit) > level.lift(&rules.hit));
    }

    #[test]
    fn a_ball_never_turns_round_in_the_air() {
        // Far off-centre, the air takes all the ball's speed but no more.
        let mishit = Contact {
            power: 14.0,
            under: 200.0,
            aside: 0.0,
        };
        let rules = Rules::default();
        let mut ball = Ball::hit(HOME, STRAIGHT, &mishit, &rules.hit, &rules.field);
        for _ in 0..300 {
            ball.step(HOME, mishit.miss(), &rules.field);
            if !ball.walled {
                assert!(ball.speed.1 <= 0.0, "{ball:?}");
            }
        }
    }

    #[test]
    fn a_fielder_faces_the_way_he_is_going() {
        let from = (100.0, 100.0);
        assert_eq!(Facing::towards(from, (200.0, 100.0)), Facing::Right);
        assert_eq!(Facing::towards(from, (0.0, 100.0)), Facing::Left);
        assert_eq!(Facing::towards(from, (100.0, 0.0)), Facing::Up);
        assert_eq!(Facing::towards(from, (100.0, 200.0)), Facing::Down);
        assert_eq!(Facing::towards(from, (200.0, 0.0)), Facing::UpRight);
        assert_eq!(Facing::towards(from, (0.0, 200.0)), Facing::DownLeft);
        assert_eq!(Facing::UpLeft.pick_label(), "pickLeft");
        assert_eq!(Facing::DownRight.throw_label(), "throwRight");
    }
}
