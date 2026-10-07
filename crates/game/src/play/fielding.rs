//! The overhead view: the ball in play, the fielder going after it, the
//! throws to the bases, and the runners.

use bb_engine::display::Path;
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_format::SymbolId;

use super::field::{Facing, Happened, distance, reach};
use super::pitch::Point;
use super::{AtBat, Match, Parts, Phase, Place, at, frame_of, put, show};
use crate::menu::Game;

/// What the fielder with the ball, or going for it, is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Job {
    /// Running to where the ball will come down, or after it once it has.
    Chase,
    /// Standing under a ball still in the air.
    WaitCatch,
    PickUp {
        left: u32,
    },
    /// Drawing back to throw.
    WindUp {
        left: u32,
    },
    /// The ball is in the air between fielders.
    Throwing {
        step: Point,
    },
    Rest,
}

pub(crate) struct Fielding {
    /// Nobody hit it: the batter walks to first on four balls.
    walk: bool,
    foul: bool,
    home_run: bool,
    /// The ball is in play: runners may be put out, and may go on.
    live: bool,
    caught: bool,
    /// Which fielder has the job, counting from 0.
    fielder: usize,
    job: Job,
    /// Where the ball will first come down.
    land: Point,
    facing: Facing,
    /// The base the ball is being thrown to, home being 4.
    throw_to: u8,
    frames: u32,
    /// Frames since the play was settled by a foul or a home run.
    since_settled: u32,
}

/// The runner clip's frame labels for running and sliding to each base.
const RUN: [&str; 4] = ["runToFirst", "runToSecond", "runToThird", "runToFourth"];
const SLIDE: [&str; 4] = [
    "slideToFirst",
    "slideToSecond",
    "slideToThird",
    "slideToFourth",
];
/// The frame on which a runner gets to each base, the frame his slide ends
/// on, and the frame a slide rejoins the run at.
const ARRIVES: [u16; 4] = [211, 421, 630, 840];
const SLIDE_ENDS: [u16; 4] = [873, 890, 906, 922];
const SLIDE_JOINS: [u16; 4] = [210, 420, 629, 839];
/// Frames from here on are slides, not the run round the bases.
const FIRST_SLIDE_FRAME: u16 = 850;
/// The buttons that send a runner on from first, second and third.
const RUN_BUTTONS: [SymbolId; 3] = [1501, 1503, 1520];
const SLIDE_BUTTONS: [SymbolId; 5] = [1494, 1495, 1502, 1519, 1521];

impl Match {
    /// Sets a runner off for a base.
    fn send(&mut self, runner: usize, to: u8, stage: &mut Stage, library: &Library) {
        self.runners[runner].running_to = Some(to);
        self.runners[runner].sliding = false;
        if let Some(path) = &self.runners[runner].path {
            stage.goto_label(path, RUN[usize::from(to) - 1], true, library);
        }
    }

    /// The batter runs to first, and pushes on anyone in his way.
    fn start_runners(&mut self, stage: &mut Stage, library: &Library) {
        let mut going = Vec::new();
        if let Some(batter) = self.batter() {
            going.push((batter, 1));
        }
        for base in 1..=3 {
            // Only a runner with someone coming up behind him has to go.
            match self.on_base(base) {
                Some(runner) if going.len() == usize::from(base) => {
                    going.push((runner, base + 1));
                }
                _ => break,
            }
        }
        for (runner, to) in going {
            self.send(runner, to, stage, library);
        }
    }

    /// Puts a runner out.
    fn put_out(&mut self, runner: usize, stage: &mut Stage, library: &Library) {
        self.runners[runner].place = Place::Out;
        self.runners[runner].running_to = None;
        self.outs += 1;
        self.clear_count();
        self.announce = true;
        let Some(path) = self.runners[runner].path.clone() else {
            return;
        };
        if let Some(clip) = stage.clip_mut(&path) {
            clip.playing = false;
        }
        for name in ["runner", "btn_slide", "runBtn"] {
            if let Some(part) = stage.find(&path, &[name]) {
                show(stage, &part, false);
            }
        }
        // "OUT" comes up over him, and he walks off.
        for name in ["outText", "outWalk"] {
            if let Some(part) = stage.find(&path, &[name]) {
                stage.goto_clip(&part, 2, library);
                if let Some(clip) = stage.clip_mut(&part) {
                    clip.playing = true;
                }
            }
        }
    }

    /// A runner has got to the base he was running to.
    fn arrive(&mut self, runner: usize, parts: &Parts, stage: &mut Stage, library: &Library) {
        let Some(base) = self.runners[runner].running_to.take() else {
            return;
        };
        let was_batting = self.runners[runner].place == Place::AtBat;
        let path = self.runners[runner].path.clone();
        self.runners[runner].sliding = false;
        if base == 4 {
            self.runners[runner].place = Place::Home;
            self.runners[runner].runs += 1;
            self.score += 1;
            if let Some(path) = &path {
                stage.goto_label(path, "addRun", false, library);
                if let Some(walk) = stage.find(path, &["outWalk"]) {
                    stage.goto_clip(&walk, 2, library);
                    if let Some(clip) = stage.clip_mut(&walk) {
                        clip.playing = true;
                    }
                }
            }
        } else {
            self.runners[runner].place = Place::Base(base);
            if let Some(path) = &path {
                stage.goto_label(path, &format!("base{base}"), false, library);
            }
            let umpire = parts.umpires[usize::from(base) - 1].clone();
            self.play_section(&umpire, "safe", 49, stage, library);
        }
        if was_batting {
            self.clear_count();
        }
        self.show_numbers(stage);
    }

    /// Changes the view to the field, for a ball that was hit or a walk.
    pub(crate) fn show_field(
        &mut self,
        at_bat: &mut AtBat,
        walk: bool,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let rules = &game.rules;
        let parts = at_bat.parts.clone();
        let y = at(stage, &parts.field).1;
        if let Some(field) = stage.child_mut(&parts.field) {
            field.move_to(rules.field.x, y);
        }
        let mut fielding = Fielding {
            walk,
            foul: false,
            home_run: false,
            live: !walk,
            caught: false,
            fielder: 0,
            job: Job::Rest,
            land: parts.home,
            facing: Facing::Down,
            throw_to: 1,
            frames: 0,
            since_settled: 0,
        };
        self.phase = Phase::Fielding;

        if walk {
            show(stage, &parts.field_ball, false);
            self.start_runners(stage, library);
        } else if let (Some(ball), Some(contact)) = (at_bat.ball, at_bat.contact) {
            let mark_x = parts.field_mark.0 + contact.aside / rules.field.aim_share;
            if mark_x < parts.foul.0 || mark_x > parts.foul.1 {
                // A foul is a strike, but never the last one.
                fielding.foul = true;
                fielding.live = false;
                if self.strikes + 1 < rules.count.strikes {
                    self.strikes += 1;
                }
                stage.goto_label(&parts.transitions, "foulHit", true, library);
            } else {
                fielding.land = if ball.bounced {
                    ball.at
                } else {
                    ball.landing(parts.home, contact.miss(), &rules.field)
                };
                // Whoever of the five in the field is nearest goes for it.
                fielding.fielder = (0..5)
                    .min_by(|&a, &b| {
                        let far = |index: usize| {
                            distance(at(stage, &parts.fielders[index]), fielding.land)
                        };
                        far(a).total_cmp(&far(b))
                    })
                    .unwrap_or(0);
                fielding.job = Job::Chase;
                self.start_runners(stage, library);
            }
        }
        self.show_numbers(stage);
        at_bat.fielding = Some(fielding);
    }

    /// The base to throw to: the nearest one that a runner is making for.
    /// With nobody running, the nearest base at all.
    fn pick_base(&self, from: Point, parts: &Parts) -> u8 {
        let wanted = |base: u8| {
            self.runners
                .iter()
                .any(|runner| runner.running_to == Some(base))
        };
        let nearest = |bases: &mut dyn Iterator<Item = u8>| {
            bases.min_by(|&a, &b| {
                let far = |base: u8| distance(from, parts.bases[usize::from(base) - 1]);
                far(a).total_cmp(&far(b))
            })
        };
        nearest(&mut (1..=4).filter(|&base| wanted(base)))
            .or_else(|| nearest(&mut (1..=4)))
            .unwrap_or(1)
    }

    /// One frame of the play in the field.
    pub(crate) fn field(
        &mut self,
        at_bat: &mut AtBat,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let rules = &game.rules.field;
        let parts = at_bat.parts.clone();
        let Some(mut state) = at_bat.fielding.take() else {
            return;
        };
        state.frames += 1;
        let miss = at_bat.contact.map_or(0.0, |contact| contact.miss());

        // The ball, for as long as nobody has hold of it.
        let loose = matches!(state.job, Job::Chase | Job::WaitCatch | Job::Rest);
        if let (Some(ball), true, false) = (&mut at_bat.ball, loose, state.walk || state.foul) {
            let happened = if state.home_run {
                Happened::Nothing
            } else {
                ball.step(parts.home, miss, rules)
            };
            let size = (0.6 + (ball.at.1 - parts.home.1) / 1000.0).max(0.1);
            put(stage, &parts.field_ball, ball.at, size);
            if let Some(inner) = stage.child_mut(&parts.field_ball_inner) {
                inner.move_to(inner.matrix.tx, -ball.height);
            }
            match happened {
                Happened::Cleared if state.live => {
                    self.home_run(&mut state, &parts, stage, library);
                }
                // Back off the wall: somebody has to go and get it.
                Happened::HitWall if state.live => state.job = Job::Chase,
                _ => {}
            }
        }

        let fielder = parts.fielders[state.fielder].clone();
        let here = at(stage, &fielder);
        match state.job {
            Job::Chase => {
                let ball = at_bat.ball.unwrap_or_else(|| unreachable_ball(state.land));
                let target = if ball.bounced { ball.at } else { state.land };
                let gap = distance(here, target);
                let speed = rules.fielder_speed.at(game.settings.difficulty);
                let next = if gap <= speed {
                    target
                } else {
                    (
                        here.0 + (target.0 - here.0) / gap * speed,
                        here.1 + (target.1 - here.1) / gap * speed,
                    )
                };
                if gap > 0.01 {
                    state.facing = Facing::towards(here, target);
                }
                stage.goto_label(&fielder, state.facing.run_label(), false, library);
                // He is drawn smaller the further up the field he is.
                let out = reach(parts.home, next);
                put(stage, &fielder, next, (0.6 - out / 5000.0).max(0.2));
                if distance(next, target) <= 2.0 {
                    if ball.bounced {
                        show(stage, &parts.field_ball, false);
                        stage.goto_label(&fielder, state.facing.pick_label(), false, library);
                        state.throw_to = self.pick_base(next, &parts);
                        state.job = Job::PickUp {
                            left: rules.pick_time,
                        };
                    } else {
                        stage.goto_label(&fielder, "waitingToCatch", false, library);
                        state.job = Job::WaitCatch;
                    }
                } else if out >= rules.fielder_reach {
                    stage.goto_label(&fielder, "waiting", false, library);
                    state.job = Job::Rest;
                }
            }
            Job::WaitCatch => {
                if let Some(ball) = at_bat.ball {
                    if ball.bounced {
                        // It got down before he could take it.
                        state.job = Job::Chase;
                    } else if ball.lift < 0.0 && ball.height <= rules.catch_height {
                        state.caught = true;
                        show(stage, &parts.field_ball, false);
                        Match::sound(stage, library, "ballCatch_3");
                        Match::sound(stage, library, "umpire_out_1");
                        Match::sound(stage, library, "crowd_unhappy");
                        // Caught: the batter is out wherever he has got to.
                        let batter = self
                            .runners
                            .iter()
                            .position(|runner| runner.place == Place::AtBat);
                        if let Some(batter) = batter {
                            self.put_out(batter, stage, library);
                        }
                        state.throw_to = self.pick_base(here, &parts);
                        state.job =
                            self.wind_up(&state, here, &fielder, &parts, game, stage, library);
                    }
                }
            }
            Job::PickUp { left } => {
                state.job = if left == 0 {
                    self.wind_up(&state, here, &fielder, &parts, game, stage, library)
                } else {
                    Job::PickUp { left: left - 1 }
                };
            }
            Job::WindUp { left } => {
                if left == 0 {
                    let to = parts.bases[usize::from(state.throw_to) - 1];
                    let gap = distance(here, to).max(0.001);
                    let step = (
                        (to.0 - here.0) / gap * rules.throw_speed,
                        (to.1 - here.1) / gap * rules.throw_speed,
                    );
                    if let Some(ball) = &mut at_bat.ball {
                        ball.at = here;
                        ball.height = rules.catch_height;
                    }
                    show(stage, &parts.field_ball, true);
                    state.job = Job::Throwing { step };
                } else {
                    state.job = Job::WindUp { left: left - 1 };
                }
            }
            Job::Throwing { step } => {
                let to = parts.bases[usize::from(state.throw_to) - 1];
                if let Some(ball) = &mut at_bat.ball {
                    ball.at = (ball.at.0 + step.0, ball.at.1 + step.1);
                    let size = (0.6 + (ball.at.1 - parts.home.1) / 1000.0).max(0.1);
                    put(stage, &parts.field_ball, ball.at, size);
                    if let Some(inner) = stage.child_mut(&parts.field_ball_inner) {
                        inner.move_to(inner.matrix.tx, -ball.height);
                    }
                    if distance(ball.at, to) < rules.throw_near {
                        self.ball_at_base(&mut state, &parts, game, stage, library);
                    }
                }
            }
            Job::Rest => {}
        }

        let down = at_bat.ball.is_some_and(|ball| ball.bounced);
        self.move_runners(&state, down, &parts, stage, library);

        // How the play ends.
        let over = if state.foul || state.home_run {
            state.since_settled += 1;
            // The foul and home-run pictures play themselves out first.
            if state.home_run
                && state.since_settled == 60
                && let Some(board) = &parts.field_scoreboard
            {
                stage.goto_label(board, "homeRun", true, library);
            }
            state.since_settled >= if state.foul { 91 } else { 116 }
        } else if state.walk {
            !self.anyone_running()
        } else {
            !state.live || state.frames > rules.longest
        };
        if over {
            // Anyone still between bases when a play is called dead is given
            // the base he was making for.
            for runner in 0..self.runners.len() {
                if self.runners[runner].running_to.is_some() {
                    self.arrive(runner, &parts, stage, library);
                }
            }
            self.ready(&parts, stage, library);
        }
        at_bat.fielding = Some(state);
    }

    /// Turns the fielder to face the base and starts his throw.
    #[allow(clippy::too_many_arguments)]
    fn wind_up(
        &self,
        state: &Fielding,
        here: Point,
        fielder: &Path,
        parts: &Parts,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) -> Job {
        let to = parts.bases[usize::from(state.throw_to) - 1];
        let label = Facing::towards(here, to).throw_label();
        stage.goto_label(fielder, label, false, library);
        Job::WindUp {
            left: game.rules.field.throw_time,
        }
    }

    /// A throw has reached its base.
    fn ball_at_base(
        &mut self,
        state: &mut Fielding,
        parts: &Parts,
        game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        let base = state.throw_to;
        Match::sound(stage, library, "ballCatch_1");
        show(stage, &parts.field_ball, false);
        // The fielder minding that base has the ball now.
        state.fielder = 4 + usize::from(base);
        let late: Vec<usize> = (0..self.runners.len())
            .filter(|&runner| self.runners[runner].running_to == Some(base))
            .collect();
        for runner in late {
            self.put_out(runner, stage, library);
            Match::sound(stage, library, "umpire_out_2");
            Match::sound(stage, library, "crowd_unhappy");
            if base <= 3 {
                let umpire = parts.umpires[usize::from(base) - 1].clone();
                self.play_section(&umpire, "out", 99, stage, library);
            }
        }
        self.show_numbers(stage);
        if self.anyone_running() {
            // Somebody is still between bases: on it goes.
            let fielder = parts.fielders[state.fielder].clone();
            let here = at(stage, &fielder);
            state.throw_to = self.pick_base(here, parts);
            state.job = self.wind_up(state, here, &fielder, parts, game, stage, library);
        } else {
            state.live = false;
            state.job = Job::Rest;
        }
    }

    /// The ball has cleared the wall: everybody scores.
    fn home_run(
        &mut self,
        state: &mut Fielding,
        parts: &Parts,
        stage: &mut Stage,
        library: &Library,
    ) {
        state.home_run = true;
        state.live = false;
        state.job = Job::Rest;
        for runner in &mut self.runners {
            if matches!(runner.place, Place::AtBat | Place::Base(_)) {
                runner.place = Place::Home;
                runner.running_to = None;
                runner.runs += 1;
                self.score += 1;
                if let Some(path) = &runner.path {
                    stage.goto_label(path, "empty", false, library);
                }
            }
        }
        self.clear_count();
        self.announce = true;
        let fielder = parts.fielders[state.fielder].clone();
        stage.goto_label(&fielder, "waiting", false, library);
        show(stage, &parts.field_ball, false);
        let transitions = parts.transitions.clone();
        self.play_section(&transitions, "homeRun", 117, stage, library);
        self.show_numbers(stage);
    }

    /// Brings in the runners who have got to their bases, and keeps the
    /// buttons that send them on for when they can be used.
    fn move_runners(
        &mut self,
        state: &Fielding,
        ball_down: bool,
        parts: &Parts,
        stage: &mut Stage,
        library: &Library,
    ) {
        // Once the ball has been caught or has come down, a runner on a
        // base may try for the next.
        let may_go_on = state.live && (state.caught || ball_down);
        for runner in 0..self.runners.len() {
            let Some(path) = self.runners[runner].path.clone() else {
                continue;
            };
            let frame = frame_of(stage, &path);
            if let Some(base) = self.runners[runner].running_to {
                let index = usize::from(base) - 1;
                if self.runners[runner].sliding && frame >= SLIDE_ENDS[index] {
                    self.runners[runner].sliding = false;
                    stage.goto_clip(&path, SLIDE_JOINS[index], library);
                    if let Some(clip) = stage.clip_mut(&path) {
                        clip.playing = true;
                    }
                } else if frame >= ARRIVES[index] && frame < FIRST_SLIDE_FRAME {
                    self.arrive(runner, parts, stage, library);
                }
            } else if let Place::Base(base) = self.runners[runner].place {
                // The way on must be clear: nobody on the next base and
                // nobody making for it.
                let next = base + 1;
                let clear = next == 4
                    || (self.on_base(next).is_none()
                        && !self
                            .runners
                            .iter()
                            .any(|other| other.running_to == Some(next)));
                if let Some(button) = stage.find(&path, &["runBtn"]) {
                    show(stage, &button, may_go_on && clear);
                }
            }
        }
    }

    /// A press on one of a runner's own buttons: slide, or go on.
    pub(crate) fn runner_button(
        &mut self,
        symbol: SymbolId,
        path: &[u16],
        _game: &Game,
        stage: &mut Stage,
        library: &Library,
    ) {
        if self.phase != Phase::Fielding {
            return;
        }
        let Some(runner) = self.runners.iter().position(|runner| {
            runner
                .path
                .as_ref()
                .is_some_and(|own| path.starts_with(own))
        }) else {
            return;
        };
        let Some(own) = self.runners[runner].path.clone() else {
            return;
        };
        if SLIDE_BUTTONS.contains(&symbol) {
            if let (Some(base), false) = (
                self.runners[runner].running_to,
                self.runners[runner].sliding,
            ) {
                self.runners[runner].sliding = true;
                stage.goto_label(&own, SLIDE[usize::from(base) - 1], true, library);
            }
        } else if RUN_BUTTONS.contains(&symbol)
            && self.runners[runner].running_to.is_none()
            && let Place::Base(base) = self.runners[runner].place
        {
            // The button is only showing when he may go, so going is all
            // there is to do.
            self.send(runner, base + 1, stage, library);
        }
    }
}

/// A ball that is not there: nothing was hit. Only reached if a fielder is
/// somehow sent after one, and then he runs to where he was told.
fn unreachable_ball(at: Point) -> super::field::Ball {
    super::field::Ball {
        at,
        speed: (0.0, 0.0),
        height: 0.0,
        lift: 0.0,
        bounced: true,
        walled: true,
    }
}
