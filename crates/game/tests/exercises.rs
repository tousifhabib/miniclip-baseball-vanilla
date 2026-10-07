//! Parts of the game that only a player's own clicks reach: the runners'
//! buttons, the arcade game's setup page, and the score table from one run
//! of the game to the next.

mod common;

use common::{click_named, click_symbol, game, game_scored, game_with, state_after};

fn state(script: &mut bb_game::script::Script) -> String {
    state_after(script, "state")
}

fn phase(state: &str) -> String {
    state
        .split(": ")
        .nth(1)
        .and_then(|rest| rest.split([' ', ',']).next())
        .unwrap_or_default()
        .to_owned()
}

/// Puts the ring on the ball and swings `early` frames before the pitch is
/// gone. Returns once the pitch has been dealt with one way or the other.
fn bat(script: &mut bb_game::script::Script, early: u32, under: f32) {
    for _ in 0..5000 {
        let now = state(script);
        let crossing = now.split("crossing ").nth(1).and_then(|rest| {
            let (x, rest) = rest.split_once(',')?;
            let y = rest.split(' ').next()?;
            Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()? + under))
        });
        let frames: u32 = now
            .split(" after ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|frames| frames.parse().ok())
            .unwrap_or(0);
        match (phase(&now).as_str(), crossing) {
            ("Flight", Some((x, y))) => {
                let wait = frames.saturating_sub(early);
                script.run(&format!("wait {wait}; click {x} {y}")).unwrap();
                while phase(&state(script)) == "Flight" {
                    script.run("wait 1").unwrap();
                }
                return;
            }
            (_, Some((x, y))) => {
                script.run(&format!("move {x} {y}; wait 1")).unwrap();
            }
            _ => {
                script.run("wait 1").unwrap();
            }
        }
    }
    panic!("no pitch came: {}", state(script));
}

/// The frame every runner's clip is on.
fn runner_frames(script: &bb_game::script::Script) -> Vec<u16> {
    let stage = &script.runner.stage;
    (1..=18)
        .flat_map(|number| bb_game::art::all_named(stage, &[], &format!("runner{number}")))
        .filter_map(|path| stage.clip(&path).map(|clip| clip.frame))
        .collect()
}

#[test]
fn a_runner_slides_when_his_slide_button_is_pressed() {
    let Some(mut script) = game("match") else {
        return;
    };
    // A poor hit that stays in the field: the batter runs for first.
    bat(&mut script, 16, 0.0);
    let mut slid = false;
    for _ in 0..3000 {
        if phase(&state(&mut script)) == "Ready" {
            break;
        }
        // The button comes up as he nears the base.
        if !slid && click_named(&mut script, "btn_slide") {
            slid = true;
            script.run("wait 2").unwrap();
            // The slides are the last stretch of the runner's clip.
            let frames = runner_frames(&script);
            assert!(frames.iter().any(|&frame| frame >= 850), "{frames:?}");
        }
        script.run("wait 1").unwrap();
    }
    assert!(slid, "the slide button never came up");
    let end = state(&mut script);
    assert_eq!(phase(&end), "Ready", "{end}");
    // He got there, one way or the other: on first, or out trying.
    assert!(end.contains("bases x--") || end.contains("outs 1"), "{end}");
}

#[test]
fn a_runner_on_base_goes_on_when_his_run_button_is_pressed() {
    // Not every hit leaves time for it, so batters of different timing and
    // luck are tried until one does.
    let mut pressed = false;
    'games: for seed in 1..=10 {
        for (early, under) in [(16, 0.0), (15, 20.0), (17, -20.0), (22, -30.0)] {
            let Some(mut script) = game_with("match", Some(seed)) else {
                return;
            };
            bat(&mut script, early, under);
            let before = state(&mut script);
            for _ in 0..3000 {
                if phase(&state(&mut script)) == "Ready" {
                    break;
                }
                if !pressed && click_named(&mut script, "runBtn") {
                    pressed = true;
                    script.run("wait 3").unwrap();
                    // He has left his base for the next: second is frames
                    // 221 to 420 of his clip, third 431 to 629.
                    let frames = runner_frames(&script);
                    assert!(
                        frames.iter().any(|&frame| (221..=420).contains(&frame)
                            || (431..=629).contains(&frame)
                            || frame >= 850),
                        "{frames:?} after {before}"
                    );
                }
                script.run("wait 1").unwrap();
            }
            if pressed {
                let end = state(&mut script);
                assert_eq!(phase(&end), "Ready", "{end}");
                // He made the next base, or was put out going for it.
                assert!(end.contains("bases -x-") || end.contains("outs 1"), "{end}");
                break 'games;
            }
        }
    }
    assert!(pressed, "no play left a runner the chance to go on");
}

#[test]
fn the_arcade_setup_pages_choices_dress_the_batter_there_and_in_the_game() {
    let Some(mut script) = game("menu") else {
        return;
    };
    assert_eq!(
        state_after(&mut script, "wait 60; click 200 237; wait 60; state"),
        "Menu, ArcadeSetup, Medium"
    );
    let colour_of = |script: &bb_game::script::Script, name: &str| {
        let stage = &script.runner.stage;
        let path = stage.find_named(&[], name).expect("the part to be there");
        stage.child(&path).unwrap().color
    };
    let logo_frame = |script: &bb_game::script::Script| {
        let stage = &script.runner.stage;
        let logos = bb_game::art::all_named(stage, &[], "logo");
        logos
            .iter()
            .filter_map(|path| stage.clip(path).map(|clip| clip.frame))
            .collect::<Vec<u16>>()
    };

    // A skin from the strip of skin tones.
    assert!(click_named(&mut script, "skinPicker"), "no skin strip");
    script.run("wait 3").unwrap();
    let skin = colour_of(&script, "skinMovie");
    assert_eq!(skin.mult, [0.0, 0.0, 0.0, 1.0], "the skin was not taken");
    let [red, green, blue, _] = skin.add;
    assert!(
        red > green && green > blue,
        "{:?} is no skin tone",
        skin.add
    );

    // The "fire" logo for the bat: the button, and the frame it shows.
    assert!(click_symbol(&mut script, 447), "no fire button");
    script.run("wait 3").unwrap();
    let fire = script.runner.library.clips[&341].labels["fire"];
    assert!(
        logo_frame(&script).contains(&fire),
        "{:?}",
        logo_frame(&script)
    );

    // On into the game: the batter there is the same.
    let play = state_after(
        &mut script,
        "click 490 362; wait 60; click 480 362; wait 200; state",
    );
    assert!(play.starts_with("Arcade"), "{play}");
    assert_eq!(colour_of(&script, "skinMovie"), skin);
    assert!(
        logo_frame(&script).contains(&fire),
        "{:?}",
        logo_frame(&script)
    );
}

#[test]
fn a_score_saved_by_one_run_of_the_game_is_shown_by_the_next() {
    let file = std::env::temp_dir().join(format!("bb-exercise-{}.toml", std::process::id()));
    let _ = std::fs::remove_file(&file);
    {
        let Some(mut script) = game_scored("menu", &file) else {
            return;
        };
        // The arcade game's setup page, where the player's name is typed.
        script.run("wait 60; click 200 237; wait 60").unwrap();
        assert!(click_symbol(&mut script, 442), "no field for the name");
        script.run("type Ace 9").unwrap();
        let play = state_after(
            &mut script,
            "click 490 362; wait 60; click 480 362; wait 200; state",
        );
        assert!(play.starts_with("Arcade"), "{play}");
        // Ten pitches let go by.
        for _ in 0..10 {
            script.run("wait 320; click 545 355").unwrap();
        }
        assert!(state_after(&mut script, "wait 420; state").starts_with("ArcadeFinish"));
    }
    let saved = std::fs::read_to_string(&file).expect("the scores to have been written");
    assert!(saved.contains("Ace 9"), "{saved}");

    // A new run of the game, reading the same file.
    let Some(mut script) = game_scored("menu", &file) else {
        return;
    };
    assert_eq!(
        state_after(&mut script, "wait 60; click 258 277; wait 100; state"),
        "Menu, HighScores, Medium"
    );
    let stage = &script.runner.stage;
    let written: Vec<String> = bb_game::art::all_named(stage, &[], "scoreLine")
        .iter()
        .filter_map(|path| stage.child(path).and_then(|child| child.said.clone()))
        .collect();
    assert!(written.iter().any(|text| text == "ACE 9"), "{written:?}");
    std::fs::remove_file(&file).unwrap();
}

#[test]
fn nothing_is_showing_on_the_score_page_that_was_not_scored() {
    let Some(mut script) = game("menu") else {
        return;
    };
    script.run("wait 60; click 258 277; wait 100").unwrap();
    let stage = &script.runner.stage;
    let written: Vec<String> = bb_game::art::all_named(stage, &[], "scoreLine")
        .iter()
        .filter_map(|path| stage.child(path).and_then(|child| child.said.clone()))
        .collect();
    assert_eq!(written, ["HIGHSCORES", "NO SCORES YET"]);
}
