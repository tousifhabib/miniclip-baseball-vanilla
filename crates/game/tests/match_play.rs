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
    state.starts_with("Match,") || state.starts_with("Loading")
}

/// Plays a match to its end, swinging at every pitch in the zone, and
/// returns the screen it ended on and how many pitches it took. The ring is
/// held `off` away from where the ball will cross, and the swing comes
/// `swing_early_by` frames before the ball is gone.
fn play_out(seed: u64, swing_early_by: u32, off: (f32, f32)) -> (String, u32) {
    let Some(mut script) = game_seeded("match", seed) else {
        return (String::new(), 0);
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
            return (now, pitches);
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
