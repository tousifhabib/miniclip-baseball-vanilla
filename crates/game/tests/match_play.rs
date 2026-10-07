//! Whole matches, played by a script that reads where each pitch will cross
//! and swings at it.

mod common;

use common::game;

/// What the state line says about the pitch in hand.
struct Seen {
    phase: String,
    crossing: Option<(f32, f32)>,
    frames: u32,
    in_zone: bool,
}

fn seen(state: &str) -> Seen {
    let phase = state
        .split(": ")
        .nth(1)
        .and_then(|rest| rest.split([' ', ',']).next())
        .unwrap_or_default()
        .to_owned();
    let crossing = state.split("crossing ").nth(1).and_then(|rest| {
        let (x, rest) = rest.split_once(',')?;
        let y = rest.split(' ').next()?;
        Some((x.parse().ok()?, y.parse().ok()?))
    });
    let frames = state
        .split(" after ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|frames| frames.parse().ok())
        .unwrap_or(0);
    Seen {
        phase,
        crossing,
        frames,
        in_zone: !state.contains("outside the zone"),
    }
}

/// Whether the match is still going, or has not yet begun.
fn in_match(state: &str) -> bool {
    state.starts_with("Match,") || state.starts_with("Arcade,") || state.starts_with("Loading")
}

/// Plays a match to its end, swinging at every pitch in the zone, and
/// returns the screen it ended on and how many pitches it took. The ring is
/// held `off` away from where the ball will cross, and the swing comes
/// `swing_early_by` frames before the ball is gone.
fn play_out(seed: u64, swing_early_by: u32, off: (f32, f32)) -> (String, u32) {
    let (end, pitches, _) = play_screen("match", seed, swing_early_by, off);
    (end, pitches)
}

/// The same for any game screen. Also returns the arcade game's points.
fn play_screen(
    screen: &str,
    seed: u64,
    swing_early_by: u32,
    off: (f32, f32),
) -> (String, u32, u32) {
    let Some(mut script) = game_seeded(screen, seed) else {
        return (String::new(), 0, 0);
    };
    let state = |script: &mut bb_game::script::Script| {
        script.run("state").unwrap().pop().unwrap_or_default()
    };
    let mut pitches = 0;
    // Far more frames than any match needs: a play that never ends fails
    // here instead of hanging the test.
    for _ in 0..200_000 {
        let now = state(&mut script);
        if !in_match(&now) {
            let points = script
                .runner
                .stage
                .text("points_total")
                .and_then(|points| points.parse().ok())
                .unwrap_or(0);
            return (now, pitches, points);
        }
        let look = seen(&now);
        match look.phase.as_str() {
            "Settling" => {
                // Put the ring where the ball will cross, and wait for it.
                if let Some((x, y)) = look.crossing {
                    let (x, y) = (x + off.0, y + off.1);
                    script.run(&format!("move {x} {y}")).unwrap();
                }
                script.run("wait 1").unwrap();
            }
            "Flight" => {
                pitches += 1;
                if look.in_zone {
                    let step: u32 = now
                        .split("step: ")
                        .nth(1)
                        .and_then(|rest| rest.split(' ').next())
                        .and_then(|step| step.parse().ok())
                        .unwrap_or(0);
                    let swing_at = look.frames.saturating_sub(swing_early_by);
                    let wait = swing_at.saturating_sub(step);
                    let (x, y) = look.crossing.unwrap();
                    let (x, y) = (x + off.0, y + off.1);
                    script.run(&format!("wait {wait}; click {x} {y}")).unwrap();
                }
                // Let the pitch finish, one way or the other.
                while seen(&state(&mut script)).phase == "Flight" {
                    script.run("wait 1").unwrap();
                }
            }
            "Ready" => {
                // In the arcade game the next pitch is offered while the
                // ball is still in the air, and taking it loses the points.
                if now.starts_with("Arcade") {
                    script.run("wait 400").unwrap();
                }
                script.run("click 545 355; wait 2").unwrap();
            }
            _ => {
                script.run("wait 1").unwrap();
            }
        }
    }
    panic!("the match never ended: {}", state(&mut script));
}

fn game_seeded(screen: &str, seed: u64) -> Option<bb_game::script::Script> {
    common::game_with(screen, Some(seed))
}

#[test]
fn a_match_left_alone_is_lost_on_strikes() {
    let Some(mut script) = game("match") else {
        return;
    };
    // Nobody swings. Every pitch is offered again as soon as it is over.
    let mut last = String::new();
    for _ in 0..4000 {
        last = script
            .run("click 545 355; wait 30; state")
            .unwrap()
            .pop()
            .unwrap();
        if !in_match(&last) {
            break;
        }
    }
    assert!(
        last.starts_with("MatchLost")
            || last.starts_with("InningsTied")
            || last.starts_with("MatchWon"),
        "{last}"
    );
}

#[test]
fn matches_played_with_a_bat_come_to_an_end() {
    let mut ends = Vec::new();
    for seed in 1..=6 {
        // A swing 24 frames before the ball is gone meets it well.
        let (end, pitches) = play_out(seed, 24, (0.0, 0.0));
        if end.is_empty() {
            return;
        }
        assert!(pitches > 0, "seed {seed} threw nothing");
        eprintln!("seed {seed}: {pitches} pitches, then {end}");
        ends.push(end.split(',').next().unwrap_or_default().to_owned());
    }
    for end in &ends {
        assert!(
            ["MatchWon", "MatchLost", "InningsTied"].contains(&end.as_str()),
            "{ends:?}"
        );
    }
}

#[test]
fn matches_played_badly_come_to_an_end_too() {
    // Off-centre rings and mistimed swings keep the ball in the field,
    // where it has to be chased, caught and thrown in.
    let batters = [
        (22, (0.0, -25.0)),
        (26, (0.0, 30.0)),
        (24, (12.0, -40.0)),
        (21, (-10.0, 15.0)),
        (27, (6.0, -15.0)),
        (24, (-25.0, 0.0)),
        (24, (30.0, 10.0)),
    ];
    let mut seen_ends = std::collections::BTreeSet::new();
    for (index, (early, off)) in batters.into_iter().enumerate() {
        for seed in [11, 12, 13] {
            let (end, pitches) = play_out(seed + index as u64 * 10, early, off);
            if end.is_empty() {
                return;
            }
            let end = end.split(',').next().unwrap_or_default().to_owned();
            eprintln!("early {early} off {off:?} seed {seed}: {pitches} pitches, then {end}");
            assert!(
                ["MatchWon", "MatchLost", "InningsTied"].contains(&end.as_str()),
                "{end}"
            );
            seen_ends.insert(end);
        }
    }
    // Between them they should not all go the same way.
    assert!(seen_ends.len() > 1, "{seen_ends:?}");
}

#[test]
fn an_arcade_game_is_ten_pitches_and_the_target_can_be_hit() {
    // Swings aimed under the ball by different amounts drop it at different
    // depths, so between them some come down on the target.
    let mut best = 0;
    for (index, under) in [0.0, 10.0, 18.0, 26.0, 34.0, 42.0, -15.0, -30.0]
        .into_iter()
        .enumerate()
    {
        let (end, pitches, points) = play_screen("arcade", 40 + index as u64, 24, (0.0, under));
        if end.is_empty() {
            return;
        }
        eprintln!("ring {under} under: {pitches} pitches, {points} points, then {end}");
        assert!(end.starts_with("ArcadeFinish"), "{end}");
        assert_eq!(pitches, 10);
        best = best.max(points);
    }
    assert!(best > 0, "nobody hit the target");
}

#[test]
fn after_a_swing_the_batter_is_still_and_the_strike_badge_is_taken_down() {
    let Some(mut script) = game("match") else {
        return;
    };
    let state = |script: &mut bb_game::script::Script| {
        script.run("state").unwrap().pop().unwrap_or_default()
    };
    // Swing as the ball leaves the pitcher's hand, which is far too early.
    for _ in 0..2000 {
        if seen(&state(&mut script)).phase == "Flight" {
            break;
        }
        script.run("wait 1").unwrap();
    }
    script.run("click 300 250; wait 220").unwrap();
    assert!(state(&mut script).contains("count 0-1"));

    let tree = script.run("tree").unwrap();
    // The parts of him that move with the swing have stopped with it. Left
    // running they swing on for ever over a body that has stopped.
    for part in ["skinMovie", "helmetMovie", "tShirtMovie"] {
        let name = format!("\"{part}\"");
        let lines: Vec<&String> = tree.iter().filter(|line| line.contains(&name)).collect();
        assert!(!lines.is_empty(), "no {part} on the stage");
        for line in lines {
            assert!(line.contains("stopped"), "{line}");
        }
    }
    // The badge has played once and gone back to showing nothing.
    let badge = tree
        .iter()
        .find(|line| line.contains("\"strikeAnim"))
        .expect("the strike badge");
    assert!(badge.contains("on frame 1 of"), "{badge}");
}

#[test]
fn a_ball_that_is_hit_leaves_the_bat_at_the_size_it_had_grown_to() {
    let Some(mut script) = game("match") else {
        return;
    };
    let state = |script: &mut bb_game::script::Script| {
        script.run("state").unwrap().pop().unwrap_or_default()
    };
    // Seed 1's first pitch is met by a swing 16 frames before it is gone.
    loop {
        let now = state(&mut script);
        let look = seen(&now);
        if look.phase == "Flight" {
            let (x, y) = look.crossing.unwrap();
            let wait = look.frames - 16;
            script
                .run(&format!(
                    "move {x} {y}; wait {wait}; click {x} {y}; wait 14"
                ))
                .unwrap();
            break;
        }
        if let Some((x, y)) = look.crossing {
            script.run(&format!("move {x} {y}")).unwrap();
        }
        script.run("wait 1").unwrap();
    }
    assert!(
        state(&mut script).contains("Watching"),
        "{}",
        state(&mut script)
    );
    let stage = &script.runner.stage;
    let main = stage.find_named(&[], "gameMain").unwrap();
    let ball = stage.find(&main, &["ballFly", "ball"]).unwrap();
    let size = stage.child(&ball).unwrap().matrix.a;
    // The art's ball is three pixels across. By the plate the pitch has
    // grown to several times that, and the hit starts from there.
    assert!(
        size > 2.5,
        "the ball is drawn at {size} times the art's size"
    );
}

#[test]
fn the_landing_pointer_follows_the_ring_before_the_pitch_is_shown() {
    let Some(mut script) = game("match") else {
        return;
    };
    // The pitcher has not begun his wind-up, so nothing has been shown yet.
    script.run("wait 20; move 150 250; wait 30").unwrap();
    let pointer_x = |script: &bb_game::script::Script| {
        let stage = &script.runner.stage;
        let main = stage.find_named(&[], "gameMain").unwrap();
        let area = stage.find(&main, &["aimArea"]).unwrap();
        stage.child(&area).unwrap().matrix.tx
    };
    let ring_left = pointer_x(&script);
    script.run("move 420 250; wait 30").unwrap();
    let ring_right = pointer_x(&script);
    assert!(
        script.run("state").unwrap()[0].contains("Settling"),
        "the pitch should not have been shown yet"
    );
    // Aiming to one side sends the ball the other way.
    assert!(
        ring_left > ring_right + 100.0,
        "ring left gave {ring_left}, ring right gave {ring_right}"
    );
}

#[test]
fn a_fielder_throws_once_and_then_stands() {
    use bb_engine::display::Content;

    let Some(mut script) = game("match") else {
        return;
    };
    let state = |script: &mut bb_game::script::Script| {
        script.run("state").unwrap().pop().unwrap_or_default()
    };
    // Seed 1's first pitch, met poorly, stays in the field and is thrown in.
    loop {
        let look = seen(&state(&mut script));
        if look.phase == "Flight" {
            let (x, y) = look.crossing.unwrap();
            let wait = look.frames - 16;
            script
                .run(&format!("move {x} {y}; wait {wait}; click {x} {y}"))
                .unwrap();
            break;
        }
        if let Some((x, y)) = look.crossing {
            script.run(&format!("move {x} {y}")).unwrap();
        }
        script.run("wait 1").unwrap();
    }
    for _ in 0..3000 {
        if seen(&state(&mut script)).phase == "Ready" {
            break;
        }
        script.run("wait 1").unwrap();
    }
    assert_eq!(seen(&state(&mut script)).phase, "Ready");
    // Long enough for any throw to have played out, and to have begun
    // again if nothing stopped it.
    script.run("wait 240").unwrap();

    let stage = &script.runner.stage;
    let main = stage.find_named(&[], "gameMain").unwrap();
    let mut threw = 0;
    for number in 1..=9 {
        let path = stage
            .find(&main, &["field", &format!("fielder{number}")])
            .unwrap();
        let fielder = stage.clip(&path).unwrap();
        if !(46..=145).contains(&fielder.frame) {
            continue;
        }
        threw += 1;
        for child in fielder.children.values() {
            if let Content::Clip(part) = &child.content {
                assert!(
                    !part.playing || part.frame_count(&script.runner.library) <= 1,
                    "fielder {number} is still going through frame {}",
                    part.frame
                );
            }
        }
    }
    assert!(threw > 0, "nobody was left in a throwing pose to check");
}
