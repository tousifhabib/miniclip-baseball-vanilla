//! A match in progress: the last innings, batting to overtake the other
//! side.
//!
//! The art builds the batting view afresh for every pitch, so everything
//! that lasts from one pitch to the next is kept here: the score, the count,
//! the outs, and where every runner stands.

mod arcade;
pub mod field;
mod fielding;
pub mod pitch;

use bb_engine::display::{ButtonEvent, Content, Event, Path, child_bounds};
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
use bb_format::SymbolId;

use crate::art;
use crate::menu::Game;
use crate::rng::Rng;
use crate::rules::PitchRules;
use field::{Ball, Contact};
use pitch::{Choice, Mound, Pitch, Point, Quality};

/// How a match ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
    Tied,
    /// The arcade game's pitches are used up.
    ArcadeOver,
}

/// Where a batter has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// At the plate, batting.
    AtBat,
    /// Standing on first, second or third.
    Base(u8),
    Out,
    /// Round all the bases: a run.
    Home,
}

#[derive(Clone, Debug)]
pub(crate) struct Runner {
    pub place: Place,
    /// The base he is running to now, home being 4.
    pub running_to: Option<u8>,
    pub sliding: bool,
    pub runs: u32,
    /// His clip on the field, for as long as this pitch's view lasts.
    pub path: Option<Path>,
}

/// What stage a pitch has reached.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Phase {
    /// The batting view is being put up.
    Arriving,
    /// The pitcher stands and waits.
    Settling {
        left: u32,
    },
    WindUp,
    /// The ball is on its way.
    Flight {
        step: usize,
    },
    /// A strike or a ball has been called, and is being shown.
    Called {
        left: u32,
    },
    /// The ball is seen leaving the bat.
    Watching {
        left: u32,
    },
    /// Four balls: a moment, then the batter walks.
    Walking {
        left: u32,
    },
    /// The overhead view: the ball, the fielders and the runners.
    Fielding,
    /// The play is over and the next-ball button is up.
    Ready,
    /// The button has been pressed and the view is about to be rebuilt.
    Leaving {
        left: u32,
    },
    Over,
}

/// Something to do to a clip when it reaches a frame, where the art has
/// nothing to do it.
#[derive(Clone, Debug)]
pub(crate) struct Cue {
    pub path: Path,
    pub frame: u16,
    /// Go back to the first frame and wait there. Otherwise just stop.
    pub rewind: bool,
}

/// Where the parts of the view are, found afresh for each pitch.
#[derive(Clone, Debug)]
pub(crate) struct Parts {
    pub main: Path,
    pub pitcher: Path,
    pub hitter: Path,
    pub aim: Path,
    pub aim_shadow: Path,
    pub marker: Path,
    pub ball: Path,
    pub shadow: Path,
    pub fly: Path,
    pub fly_ball: Path,
    pub fly_shadow: Option<Path>,
    pub aim_area: Path,
    pub field: Path,
    pub scoreboard: Option<Path>,
    pub strike_anim: Option<Path>,
    pub transitions: Path,
    pub next: Path,
    pub flare: Option<Path>,
    pub field_ball: Path,
    pub field_ball_inner: Path,
    pub holder: Option<Path>,
    pub fielders: Vec<Path>,
    pub umpires: Vec<Path>,
    pub field_scoreboard: Option<Path>,
    /// Fixed points of the batting view.
    pub centre_x: f32,
    pub fly_mark: Point,
    pub ground_y: f32,
    /// The box the aiming ring is kept inside: left, top, right, bottom.
    pub aim_box: [f32; 4],
    /// Fixed points of the field.
    pub home: Point,
    pub field_mark: Point,
    pub foul: (f32, f32),
    pub bases: [Point; 4],
}

/// The pitch being played.
pub(crate) struct AtBat {
    pub parts: Parts,
    pub table: PitchRules,
    pub pitch: Pitch,
    pub marker_shown: bool,
    pub aim: Point,
    /// Where the hit would go sideways, as the art's indicator shows it.
    pub aim_area_x: f32,
    /// Frames since the swing began.
    pub swing: Option<u32>,
    /// How far below the ball the ring was when the swing began.
    pub under: f32,
    pub contact: Option<Contact>,
    /// The ball leaving the bat, in the batting view: where it is, how high,
    /// and how fast it is rising.
    pub fly: (Point, f32, f32),
    /// The size the ball had grown to when the bat met it.
    pub fly_size: f32,
    pub fly_target: Point,
    /// Frames until the batter drops his bat and runs.
    pub run_in: Option<u32>,
    pub ball: Option<Ball>,
    pub fielding: Option<fielding::Fielding>,
}

pub struct Match {
    pub(crate) score: u32,
    pub(crate) target: u32,
    pub(crate) outs: u32,
    pub(crate) max_outs: u32,
    pub(crate) strikes: u32,
    pub(crate) balls: u32,
    pub(crate) pitched: u32,
    pub(crate) runners: Vec<Runner>,
    pub(crate) rng: Rng,
    pub(crate) phase: Phase,
    /// The last pitch ended a batter's turn, which the scoreboard marks at
    /// the start of the next.
    pub(crate) announce: bool,
    pub(crate) at: Option<AtBat>,
    pub(crate) cues: Vec<Cue>,
    /// Clips to send back to their first frame, where they show nothing,
    /// once this many more frames have gone by.
    put_away: Vec<(Path, u32)>,
    /// The arcade game's own state, when that is what is being played.
    pub(crate) arcade: Option<arcade::Arcade>,
    was_down: bool,
    runner_symbol: Option<SymbolId>,
}

/// The pitcher's frame label for his wind-up.
const PITCH: &str = "pitch";
/// The button on the next-ball panel.
const NEXT_BALL_BUTTON: SymbolId = 1618;

pub(crate) fn at(stage: &Stage, path: &[u16]) -> Point {
    stage
        .child(path)
        .map_or((0.0, 0.0), |child| (child.matrix.tx, child.matrix.ty))
}

/// Puts an object at a point, at a size, the art's own size being 1.
pub(crate) fn put(stage: &mut Stage, path: &[u16], at: Point, size: f32) {
    if let Some(child) = stage.child_mut(path) {
        child.set_matrix(Matrix {
            a: size,
            d: size,
            tx: at.0,
            ty: at.1,
            ..Matrix::IDENTITY
        });
    }
}

pub(crate) fn show(stage: &mut Stage, path: &[u16], visible: bool) {
    if let Some(child) = stage.child_mut(path) {
        child.set_visible(visible);
    }
}

pub(crate) fn frame_of(stage: &Stage, path: &[u16]) -> u16 {
    stage.clip(path).map_or(0, |clip| clip.frame)
}

impl Match {
    pub fn new(game: &Game, seed: u64, library: &Library) -> Match {
        let behind = game.rules.game.runs_down.at(game.settings.difficulty);
        Match {
            score: 0,
            // Drawing level is not enough: the target is one run more.
            target: behind + 1,
            outs: 0,
            max_outs: game.rules.game.outs,
            strikes: 0,
            balls: 0,
            pitched: 0,
            runners: Vec::new(),
            rng: Rng::new(seed),
            phase: Phase::Arriving,
            announce: false,
            at: None,
            cues: Vec::new(),
            put_away: Vec::new(),
            arcade: None,
            was_down: false,
            runner_symbol: library.manifest.exports.get("runner").copied(),
        }
    }

    /// The arcade game instead of a match.
    pub fn new_arcade(game: &Game, seed: u64, library: &Library) -> Match {
        let mut arcade = Match::new(game, seed, library);
        arcade.arcade = Some(arcade::Arcade::new(game.rules.arcade.pitches));
        arcade
    }

    /// How the match stands, if it is over.
    fn outcome(&self) -> Option<Outcome> {
        if let Some(arcade) = &self.arcade {
            return (arcade.left == 0).then_some(Outcome::ArcadeOver);
        }
        let level = self.target - 1;
        if self.score >= self.target {
            Some(Outcome::Won)
        } else if self.outs < self.max_outs {
            None
        } else if self.score == level {
            Some(Outcome::Tied)
        } else {
            Some(Outcome::Lost)
        }
    }

    /// The batter at the plate: his place in `runners`.
    pub(crate) fn batter(&self) -> Option<usize> {
        self.runners
            .iter()
            .position(|runner| runner.place == Place::AtBat)
    }

    /// The runner standing on a base, if there is one.
    pub(crate) fn on_base(&self, base: u8) -> Option<usize> {
        self.runners
            .iter()
            .position(|runner| runner.place == Place::Base(base) && runner.running_to.is_none())
    }

    pub(crate) fn anyone_running(&self) -> bool {
        self.runners
            .iter()
            .any(|runner| runner.running_to.is_some())
    }

    /// The batter's turn is over: the next one starts with a clean count.
    pub(crate) fn clear_count(&mut self) {
        self.strikes = 0;
        self.balls = 0;
    }

    /// Writes the numbers the scoreboards show.
    pub(crate) fn show_numbers(&self, stage: &mut Stage) {
        let batter = self.batter().map_or(self.runners.len(), |index| index + 1);
        for (name, value) in [
            ("score", self.score),
            ("out", self.outs),
            ("strikes", self.strikes),
            ("noBalls", self.balls),
            ("scoreTarget", self.target),
            ("oppositionScore", self.target - 1),
            ("maximumOuts", self.max_outs),
            ("runsToGet", self.target.saturating_sub(self.score)),
            ("ballsPitched", self.pitched),
            (
                "points_total",
                self.arcade.as_ref().map_or(0, |arcade| arcade.points),
            ),
            ("batsmanOnStrike", batter as u32),
        ] {
            stage.set_text(name, value.to_string());
        }
        for (index, runner) in self.runners.iter().enumerate() {
            stage.set_text(
                &format!("batsman{}_score", index + 1),
                runner.runs.to_string(),
            );
        }
    }

    /// What the result screens say about the match just played.
    pub fn show_result(&self, stage: &mut Stage) {
        self.show_numbers(stage);
        for index in self.runners.len()..9 {
            stage.set_text(&format!("batsman{}_score", index + 1), "0");
        }
    }

    pub(crate) fn sound(stage: &mut Stage, library: &Library, name: &str) {
        stage.play_sound(name, 1, library);
    }

    /// Sets a clip playing from a label, to be sent back to its first frame
    /// when it reaches `end`, where the art's own script did that.
    pub(crate) fn play_section(
        &mut self,
        path: &[u16],
        label: &str,
        end: u16,
        stage: &mut Stage,
        library: &Library,
    ) {
        self.cues.retain(|cue| cue.path != path);
        if stage.goto_label(path, label, true, library) {
            self.cues.push(Cue {
                path: path.to_vec(),
                frame: end,
                rewind: true,
            });
        }
    }

    fn run_cues(&mut self, stage: &mut Stage, library: &Library) {
        self.put_away.retain_mut(|(path, left)| {
            if *left > 0 {
                *left -= 1;
                return true;
            }
            stage.goto_clip(path, 1, library);
            if let Some(clip) = stage.clip_mut(path) {
                clip.playing = false;
            }
            false
        });
        let mut due = Vec::new();
        self.cues.retain(|cue| match stage.clip(&cue.path) {
            Some(clip) if clip.frame >= cue.frame => {
                due.push(cue.clone());
                false
            }
            Some(_) => true,
            None => false,
        });
        for cue in due {
            if cue.rewind {
                stage.goto_clip(&cue.path, 1, library);
            }
            if let Some(clip) = stage.clip_mut(&cue.path) {
                clip.playing = false;
            }
        }
    }

    /// Finds the parts of a batting view that has just been built.
    fn parts(stage: &Stage, library: &Library) -> Option<Parts> {
        let main = stage.find_named(&[], "gameMain")?;
        let part = |names: &[&str]| stage.find(&main, names);
        let field = part(&["field"])?;
        let in_field = |name: &str| stage.find(&field, &[name]);
        let point = |path: Option<Path>| path.map(|path| at(stage, &path));
        let aim_box = stage
            .child(&part(&["acl"])?)
            .and_then(|child| child_bounds(child, Matrix::IDENTITY, library))?;
        let fly = part(&["ballFly"])?;
        let field_ball = in_field("ballFly")?;
        Some(Parts {
            pitcher: part(&["pitcher"])?,
            hitter: part(&["hitter"])?,
            aim: part(&["aimCircle"])?,
            aim_shadow: part(&["aimCircleShadow"])?,
            marker: part(&["ballPassesBat_marker"])?,
            ball: part(&["ballAll"])?,
            shadow: part(&["ballShadow"])?,
            fly_ball: stage.find(&fly, &["ball"])?,
            fly_shadow: stage.find(&fly, &["ballShadow"]),
            fly,
            aim_area: part(&["aimArea"])?,
            scoreboard: part(&["scoreboard"]),
            strike_anim: part(&["strikeAnim_old"]).or_else(|| part(&["strikeAnim"])),
            transitions: part(&["transitions"])?,
            next: part(&["btn_nextBall"])?,
            flare: part(&["lightFlare"]),
            field_ball_inner: stage.find(&field_ball, &["ball"])?,
            field_ball,
            holder: in_field("runnerHolder"),
            fielders: (1..=9)
                .filter_map(|number| in_field(&format!("fielder{number}")))
                .collect(),
            umpires: (1..=3)
                .filter_map(|number| in_field(&format!("umpire{number}")))
                .collect(),
            field_scoreboard: in_field("scoreboard"),
            centre_x: point(part(&["centreMarker"]))?.0,
            fly_mark: point(part(&["shadowFlyMarker"]))?,
            ground_y: point(part(&["uMarker"]))?.1,
            aim_box,
            home: point(in_field("startPointMarker"))?,
            field_mark: point(in_field("shadowFlyMarker"))?,
            // The arcade game's field has no foul lines and no bases.
            foul: (
                point(in_field("foulMarkerLeft")).map_or(f32::MIN, |at| at.0),
                point(in_field("foulMarkerRight")).map_or(f32::MAX, |at| at.0),
            ),
            bases: [1, 2, 3, 4]
                .map(|base| point(in_field(&format!("base{base}"))).unwrap_or_default()),
            field,
            main,
        })
    }

    /// The fixed points a pitch is drawn between.
    fn mound(parts: &Parts, stage: &Stage, library: &Library) -> Option<Mound> {
        let test = stage.find(&parts.main, &["test"])?;
        let point = |name: &str| stage.find(&test, &[name]).map(|path| at(stage, &path));
        // With no strike zone to miss, as in the arcade game, no pitch is
        // ever outside it.
        let zone = stage
            .find(&parts.main, &["strikeZone"])
            .and_then(|path| stage.child(&path))
            .and_then(|child| child_bounds(child, Matrix::IDENTITY, library))
            .unwrap_or([f32::MIN, f32::MIN, f32::MAX, f32::MAX]);
        Some(Mound {
            ball: point("ballAll")?,
            shadow: point("ballShadow")?,
            ball_from: point("startpointMarker")?,
            shadow_from: point("startpointShadowMarker")?,
            plate: point("shadowMarker")?.1,
            zone,
        })
    }

    /// Gets a freshly built batting view ready for a pitch. Returns how the
    /// match ended if it has.
    fn set_up(&mut self, game: &Game, stage: &mut Stage, library: &Library) -> Option<Outcome> {
        let parts = Match::parts(stage, library)?;
        self.cues.clear();
        self.put_away.clear();
        if let Some(outcome) = self.outcome() {
            self.phase = Phase::Over;
            return Some(outcome);
        }
        if self.batter().is_none() {
            self.runners.push(Runner {
                place: Place::AtBat,
                running_to: None,
                sliding: false,
                runs: 0,
                path: None,
            });
            self.clear_count();
        }

        let rules = &game.rules;
        let table = rules.pitch.at(game.settings.difficulty).clone();
        show(stage, &parts.ball, false);
        show(stage, &parts.shadow, false);
        if let Some(zone) = stage.find(&parts.main, &["strikeZone"]) {
            show(stage, &zone, table.show_zone);
        }
        // Every runner still in the game stands where the last pitch left
        // him.
        for index in 0..self.runners.len() {
            self.runners[index].path = None;
            self.runners[index].running_to = None;
            self.runners[index].sliding = false;
            let label = match self.runners[index].place {
                Place::AtBat => "waiting".to_owned(),
                Place::Base(base) => format!("base{base}"),
                Place::Out | Place::Home => continue,
            };
            let Some(symbol) = self.runner_symbol else {
                continue;
            };
            let Some(holder) = &parts.holder else {
                continue;
            };
            let depth = Stage::RULES_DEPTH + index as u16;
            let name = format!("runner{}", index + 1);
            if let Some(path) = stage.attach(holder, symbol, depth, &name, library) {
                stage.goto_label(&path, &label, false, library);
                self.runners[index].path = Some(path);
            }
        }
        if let Some(mark) = stage.find(&parts.main, &["runnerOnSecond"]) {
            let label = if self.on_base(2).is_some() {
                "full"
            } else {
                "none"
            };
            stage.goto_label(&mark, label, false, library);
        }
        // The fielders who mind the bases stand ready at them.
        for fielder in parts.fielders.iter().skip(5) {
            stage.goto_label(fielder, "baseWaiting", false, library);
        }
        if self.announce {
            self.announce = false;
            if let Some(board) = parts.scoreboard.clone() {
                self.play_section(&board, "runsToGet", 361, stage, library);
            }
        }
        self.set_up_arcade(&parts, game, stage, library);
        self.show_numbers(stage);

        let mound = Match::mound(&parts, stage, library)?;
        let choice = Choice::pick(&table, &rules.throw, &mut self.rng);
        let pitch = Pitch::throw(&choice, &mound, &rules.throw);
        self.phase = Phase::Settling {
            left: rules.throw.settle + pitch.samples.len() as u32,
        };
        self.at = Some(AtBat {
            aim: at(stage, &parts.aim),
            aim_area_x: at(stage, &parts.aim_area).0,
            parts,
            table,
            pitch,
            marker_shown: false,
            swing: None,
            under: 0.0,
            contact: None,
            fly: ((0.0, 0.0), 0.0, 0.0),
            fly_size: 1.0,
            fly_target: (0.0, 0.0),
            run_in: None,
            ball: None,
            fielding: None,
        });
        None
    }

    /// Stills the batter once his swing is done.
    ///
    /// The swing is a clip that stops on its last frame, with his skin,
    /// shirt and helmet as clips of their own inside it, moving in step.
    /// The art stopped those from a script. Left alone they go round again
    /// over a body that has stopped, and he swings on for ever.
    fn still_batter(stage: &mut Stage, hitter: &[u16], library: &Library) {
        let Some(clip) = stage.clip(hitter) else {
            return;
        };
        let mut moving = Vec::new();
        for (&depth, child) in &clip.children {
            let Content::Clip(swing) = &child.content else {
                continue;
            };
            let last = swing.frame_count(library);
            if swing.playing || last <= 1 || swing.frame != last {
                continue;
            }
            for (&inner_depth, inner) in &swing.children {
                if let Content::Clip(part) = &inner.content
                    && part.playing
                    && part.frame_count(library) > 1
                {
                    let mut path = hitter.to_vec();
                    path.extend([depth, inner_depth]);
                    moving.push((path, part.frame_count(library)));
                }
            }
        }
        for (path, last) in moving {
            stage.goto_clip(&path, last, library);
            if let Some(part) = stage.clip_mut(&path) {
                part.playing = false;
            }
        }
    }

    /// Moves the aiming ring a step towards the pointer, and with it the
    /// art's pointer to where a hit would go.
    fn aim(at_bat: &mut AtBat, stage: &mut Stage) {
        let parts = &at_bat.parts;
        let pointer = (stage.pointer.x, stage.pointer.y);
        // A pointer nobody has moved yet is nowhere.
        if pointer.0 < -1.0e5 {
            return;
        }
        let Some(pointer) = stage.from_stage(&parts.main, pointer.0, pointer.1) else {
            return;
        };
        let [left, top, right, bottom] = parts.aim_box;
        let ease = at_bat.table.aim_ease.max(1.0);
        at_bat.aim.0 += (pointer.0 - at_bat.aim.0) / ease;
        at_bat.aim.1 += (pointer.1 - at_bat.aim.1) / ease;
        at_bat.aim.0 = at_bat.aim.0.clamp(left + 1.0, right - 1.0);
        at_bat.aim.1 = at_bat.aim.1.clamp(top + 1.0, bottom - 1.0);
        if let Some(ring) = stage.child_mut(&parts.aim) {
            ring.move_to(at_bat.aim.0, at_bat.aim.1);
        }
        let shadow_y = at(stage, &parts.aim_shadow).1;
        if let Some(shadow) = stage.child_mut(&parts.aim_shadow) {
            shadow.move_to(at_bat.aim.0, shadow_y);
        }
    }

    /// Works out where a hit made now would go sideways, and shows it.
    fn point_hit(at_bat: &mut AtBat, pull: f32, stage: &mut Stage, library: &Library) {
        if !at_bat.marker_shown || at_bat.contact.is_some() {
            return;
        }
        let parts = &at_bat.parts;
        let crosses = at_bat.pitch.crosses.0;
        // Aiming to one side sends the ball the other way, and a ball that
        // comes in off-centre goes off further still.
        let off = (crosses - at_bat.aim.0) + (crosses - parts.centre_x);
        at_bat.aim_area_x = (crosses + off * pull).ceil();
        let y = at(stage, &parts.aim_area).1;
        if let Some(area) = stage.child_mut(&parts.aim_area) {
            area.move_to(at_bat.aim_area_x, y);
        }
        // The pointer's look is drawn for every position, one a frame.
        let frame = at_bat.aim_area_x.clamp(1.0, 550.0) as u16;
        stage.goto_clip(&parts.aim_area, frame, library);
        if let Some(clip) = stage.clip_mut(&parts.aim_area) {
            clip.playing = false;
        }
    }

    /// Called once a frame while the match screen is showing. Returns how
    /// the match ended, once it has.
    pub fn tick(&mut self, game: &Game, stage: &mut Stage, library: &Library) -> Option<Outcome> {
        let down = stage.pointer.down;
        let pressed = down && !self.was_down && !stage.pointer.on_button();
        self.was_down = down;
        self.run_cues(stage, library);

        if self.phase == Phase::Arriving {
            return self.set_up(game, stage, library);
        }
        let mut at_bat = self.at.take()?;
        // The view has gone: the screen was left.
        stage.clip(&at_bat.parts.main)?;
        let rules = &game.rules;
        Match::still_batter(stage, &at_bat.parts.hitter, library);
        if at_bat.contact.is_none() {
            Match::aim(&mut at_bat, stage);
            Match::point_hit(&mut at_bat, rules.hit.pull, stage, library);
        }

        // In the arcade game the ball goes on over the field while the next
        // pitch is already on offer.
        if matches!(self.phase, Phase::Ready | Phase::Leaving { .. }) {
            self.arcade_ball(&mut at_bat, game, stage, library);
        }
        match self.phase {
            Phase::Settling { left } => {
                if left == 0 {
                    stage.goto_label(&at_bat.parts.pitcher, PITCH, true, library);
                    self.phase = Phase::WindUp;
                } else {
                    self.phase = Phase::Settling { left: left - 1 };
                }
            }
            Phase::WindUp => {
                let frame = frame_of(stage, &at_bat.parts.pitcher);
                if !at_bat.marker_shown && frame >= at_bat.table.marker_frame {
                    at_bat.marker_shown = true;
                    put(stage, &at_bat.parts.marker, at_bat.pitch.crosses, 1.0);
                }
                if frame >= rules.throw.release_frame {
                    show(stage, &at_bat.parts.ball, true);
                    show(stage, &at_bat.parts.shadow, true);
                    self.pitched += 1;
                    if let Some(arcade) = &mut self.arcade {
                        arcade.left = arcade.left.saturating_sub(1);
                    }
                    self.show_numbers(stage);
                    self.phase = Phase::Flight { step: 0 };
                }
            }
            Phase::Flight { step } => {
                self.flight(&mut at_bat, step, pressed, game, stage, library);
            }
            Phase::Called { left } => {
                if left == 0 {
                    self.ready(&at_bat.parts, stage, library);
                } else {
                    self.phase = Phase::Called { left: left - 1 };
                }
            }
            Phase::Watching { left } => {
                self.watch(&mut at_bat, game, stage, library);
                if left == 0 && self.arcade.is_some() {
                    self.show_arcade_field(&mut at_bat, game, stage, library);
                } else if left == 0 {
                    self.show_field(&mut at_bat, false, game, stage, library);
                } else {
                    self.phase = Phase::Watching { left: left - 1 };
                }
            }
            Phase::Walking { left } => {
                if left == 0 {
                    self.show_field(&mut at_bat, true, game, stage, library);
                } else {
                    self.phase = Phase::Walking { left: left - 1 };
                }
            }
            Phase::Fielding => self.field(&mut at_bat, game, stage, library),
            Phase::Leaving { left } => {
                if left == 0 {
                    // Building the view again starts the next pitch.
                    let mut holder = at_bat.parts.main.clone();
                    holder.pop();
                    stage.goto_clip(&holder, 1, library);
                    if let Some(clip) = stage.clip_mut(&holder) {
                        clip.playing = true;
                    }
                    self.phase = Phase::Arriving;
                    return None;
                }
                self.phase = Phase::Leaving { left: left - 1 };
            }
            Phase::Ready | Phase::Arriving | Phase::Over => {}
        }
        self.at = Some(at_bat);
        None
    }

    /// One frame of the ball on its way to the batter.
    fn flight(
        &mut self,
        at_bat: &mut AtBat,
        step: usize,
        pressed: bool,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let rules = &game.rules;
        let Some(&sample) = at_bat.pitch.samples.get(step) else {
            return self.call(at_bat, game, stage, library);
        };
        for (path, point) in [
            (&at_bat.parts.ball, sample.ball),
            (&at_bat.parts.shadow, sample.shadow),
        ] {
            put(stage, path, point, sample.size);
            if let Some(child) = stage.child_mut(path) {
                child.set_alpha(sample.alpha);
            }
        }

        if pressed && at_bat.swing.is_none() {
            // The swing is high, level or low by where the pointer is.
            let pointer = stage
                .from_stage(&at_bat.parts.main, stage.pointer.x, stage.pointer.y)
                .unwrap_or(at_bat.aim);
            let label = match pointer.1 {
                y if y <= 200.0 => "hitHigh",
                y if y <= 280.0 => "hitMed",
                _ => "hitLow",
            };
            stage.goto_label(&at_bat.parts.hitter, label, true, library);
            if self.arcade.is_none() {
                Match::sound(stage, library, "batSwing_fast");
            }
            at_bat.swing = Some(0);
            at_bat.under = at_bat.aim.1 - at_bat.pitch.crosses.1;
        } else if let Some(frames) = &mut at_bat.swing {
            *frames += 1;
        }

        let band = at_bat.table.band;
        let in_band = sample.shadow.1 > band.top && sample.shadow.1 < band.bottom;
        let met = at_bat
            .swing
            .and_then(|frames| pitch::meets(&at_bat.table, frames));
        if let (true, Some((quality, power))) = (in_band, met) {
            let (hit, cheer): (&str, &[&str]) = match quality {
                Quality::Poor => ("batHit_poorly", &["crowd_smallClap"]),
                Quality::MediumPoor => ("batHit_mediumPoor", &["crowd_smallCheer"]),
                Quality::Medium => ("batHit_medium", &["crowd_smallCheer", "crowd_smallClap"]),
                Quality::Good => ("batHit_good", &["crowd_bigClap"]),
            };
            Match::sound(stage, library, hit);
            for name in cheer {
                Match::sound(stage, library, name);
            }
            let contact = Contact {
                power,
                under: at_bat.under,
                aside: at_bat.aim_area_x - at_bat.parts.centre_x,
            };
            // In the batting view the ball flies off towards where the
            // art's pointer showed, dropping further the weaker the hit.
            let parts = &at_bat.parts;
            at_bat.fly = (
                sample.shadow,
                sample.shadow.1 - sample.ball.1,
                contact.lift(&rules.hit),
            );
            // It leaves the bat the size it had come to, and shrinks from
            // there as it goes away.
            at_bat.fly_size = sample.size;
            if let Some(shadow) = &parts.fly_shadow {
                let place = at(stage, shadow);
                put(stage, shadow, place, sample.size);
            }
            at_bat.fly_target = (
                at_bat.aim_area_x,
                parts.fly_mark.1 + power + contact.miss() / 2.0,
            );
            // And over the field it heads for the mark, pushed aside by
            // the same amount.
            let mark = (
                parts.field_mark.0 + contact.aside / rules.field.aim_share,
                parts.field_mark.1,
            );
            at_bat.ball = Some(Ball::hit(
                parts.home,
                mark,
                &contact,
                &rules.hit,
                &rules.field,
            ));
            show(stage, &parts.ball, false);
            show(stage, &parts.shadow, false);
            at_bat.contact = Some(contact);
            // He is 34 frames into his swing when he drops the bat.
            at_bat.run_in = Some(34u32.saturating_sub(at_bat.swing.unwrap_or(0)));
            self.phase = Phase::Watching {
                left: if self.arcade.is_some() {
                    at_bat.run_in = None;
                    rules.arcade.watch
                } else {
                    rules.hit.watch
                },
            };
            return;
        }
        self.phase = Phase::Flight { step: step + 1 };
    }

    /// The ball has gone by: a strike, or a ball.
    fn call(&mut self, at_bat: &mut AtBat, game: &Game, stage: &mut Stage, library: &Library) {
        let rules = &game.rules;
        let parts = at_bat.parts.clone();
        show(stage, &parts.ball, false);
        show(stage, &parts.shadow, false);
        if self.arcade.is_some() {
            // No count in the arcade game: a miss is just a pitch gone.
            return self.ready(&parts, stage, library);
        }
        Match::sound(stage, library, "ballCatch_1");
        if !at_bat.pitch.in_zone && at_bat.swing.is_none() {
            self.balls += 1;
            if let Some(board) = &parts.scoreboard {
                self.play_section(board, "noBall", 261, stage, library);
            }
            self.show_numbers(stage);
            if self.balls >= rules.count.balls {
                self.phase = Phase::Walking {
                    left: rules.hit.walk_wait,
                };
            } else {
                self.ready(&parts, stage, library);
            }
            return;
        }
        self.strikes += 1;
        if let Some(anim) = &parts.strike_anim {
            let label = format!("strike{}", self.strikes.min(3));
            stage.goto_label(anim, &label, false, library);
            // The badge plays for 69 frames and is then taken down. Left
            // up, it would play again and again over the scoreboard.
            self.put_away.push((anim.clone(), 68));
        }
        if let Some(board) = &parts.scoreboard {
            self.play_section(board, "strike", 136, stage, library);
        }
        if self.strikes >= rules.count.strikes {
            let call = ["1", "2", "3"][self.rng.below(3) as usize];
            Match::sound(stage, library, &format!("umpire_yourOuttaHere_{call}"));
            Match::sound(stage, library, "crowd_unhappy");
            if let Some(batter) = self.batter() {
                self.runners[batter].place = Place::Out;
            }
            self.outs += 1;
            self.clear_count();
            self.announce = true;
        } else {
            Match::sound(stage, library, "umpire_Strike_grunt");
            if self.strikes + 1 == rules.count.strikes {
                let organ = ["baseball_organ_FX", "baseball_organ_tense_FX"];
                Match::sound(stage, library, organ[self.rng.below(2) as usize]);
            }
        }
        self.show_numbers(stage);
        // The call is left up for a moment before the next pitch is offered.
        self.phase = Phase::Called { left: 58 };
    }

    /// One frame of the ball leaving the bat, seen from behind the batter.
    fn watch(&mut self, at_bat: &mut AtBat, game: &Game, stage: &mut Stage, library: &Library) {
        let rules = &game.rules;
        let Some(contact) = at_bat.contact else {
            return;
        };
        let parts = &at_bat.parts;
        let (at_point, height, lift) = &mut at_bat.fly;
        at_point.0 -= (at_point.0 - at_bat.fly_target.0) / contact.power;
        at_point.1 -= (at_point.1 - at_bat.fly_target.1) / contact.power;
        *height += *lift;
        if *lift >= -5.0 {
            *lift -= rules.hit.gravity;
        }
        if *height < 0.0 {
            *height = 0.0;
            *lift = -*lift * rules.hit.bounce;
        }
        let size = (1.0 + (at_point.1 - parts.ground_y) / 190.0).max(0.05);
        put(stage, &parts.fly, *at_point, size);
        let across = at(stage, &parts.fly_ball).0;
        put(stage, &parts.fly_ball, (across, -*height), at_bat.fly_size);
        if let Some(left) = at_bat.run_in {
            if left == 0 {
                at_bat.run_in = None;
                stage.goto_label(&parts.hitter, "run", true, library);
                let last = stage
                    .clip(&parts.hitter)
                    .map_or(1, |clip| clip.frame_count(library));
                self.cues.push(Cue {
                    path: parts.hitter.clone(),
                    frame: last,
                    rewind: false,
                });
            } else {
                at_bat.run_in = Some(left - 1);
            }
        }
        // The ball is already on its way over the field, out of sight.
        if let Some(ball) = &mut at_bat.ball {
            ball.step(parts.home, contact.miss(), &rules.field);
        }
    }

    /// The play is over: offers the next pitch.
    pub(crate) fn ready(&mut self, parts: &Parts, stage: &mut Stage, library: &Library) {
        if self.phase == Phase::Ready {
            return;
        }
        self.phase = Phase::Ready;
        stage.goto_clip(&parts.next, 2, library);
        self.show_numbers(stage);
    }

    /// Takes in something the stage has reported.
    pub fn event(&mut self, event: &Event, game: &Game, stage: &mut Stage, library: &Library) {
        let Event::Button {
            symbol,
            path,
            event,
        } = event
        else {
            return;
        };
        match (*symbol, *event) {
            (NEXT_BALL_BUTTON, ButtonEvent::Release) if self.phase == Phase::Ready => {
                let Some(at_bat) = &self.at else {
                    return;
                };
                // The panel plays itself out, and a flare covers the change.
                let mut panel = path.clone();
                panel.pop();
                stage.goto_label(&panel, "nextBall", true, library);
                if let Some(flare) = &at_bat.parts.flare {
                    stage.goto_clip(flare, 2, library);
                    if let Some(clip) = stage.clip_mut(flare) {
                        clip.playing = true;
                    }
                }
                self.phase = Phase::Leaving { left: 12 };
            }
            (_, ButtonEvent::Press) => self.runner_button(*symbol, path, game, stage, library),
            _ => {}
        }
    }

    pub fn describe(&self) -> String {
        let bases: String = (1..=3)
            .map(|base| {
                if self.on_base(base).is_some() {
                    'x'
                } else {
                    '-'
                }
            })
            .collect();
        // Where this pitch crosses and how many frames it takes, which a
        // script needs to know to time a swing.
        let pitch = self.at.as_ref().map_or(String::new(), |at_bat| {
            format!(
                ", crossing {:.0},{:.0} after {} frames{}",
                at_bat.pitch.crosses.0,
                at_bat.pitch.crosses.1,
                at_bat.pitch.samples.len(),
                if at_bat.pitch.in_zone {
                    ""
                } else {
                    " outside the zone"
                },
            )
        });
        if let Some(arcade) = &self.arcade {
            return format!(
                "{:?}, {} points, {} pitches left{pitch}",
                self.phase, arcade.points, arcade.left
            );
        }
        format!(
            "{:?}, score {} of {}, outs {}, count {}-{}, bases {bases}, pitched {}{pitch}",
            self.phase, self.score, self.target, self.outs, self.balls, self.strikes, self.pitched
        )
    }
}

/// The screen a finished match leads to.
pub fn result_screen(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Won => "matchWon",
        Outcome::Lost => "matchLost",
        Outcome::Tied => "inningsTied",
        Outcome::ArcadeOver => "arcadeFinish",
    }
}

/// Used by the match screen's own art to find the shell, kept here so that
/// the wiring is in one place.
pub fn shell(stage: &Stage) -> Option<Path> {
    art::shell(stage)
}
