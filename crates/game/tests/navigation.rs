//! Getting around: the menu, its pages, and the way into a game.

mod common;

use common::{game, state_after};

/// Where things are on the menu's pages, in stage pixels.
const BOTTOM_OF_THE_NINTH: &str = "click 200 192";
const ARCADE: &str = "click 200 237";
const NEXT: &str = "click 490 362";
const BACK: &str = "click 290 362";
const PLAY_BALL: &str = "click 480 362";

#[test]
fn the_menu_leads_through_setup_to_a_match() {
    let Some(mut script) = game("menu") else {
        return;
    };
    assert_eq!(
        state_after(&mut script, "wait 60; state"),
        "Menu, Main, Medium"
    );
    let setup = format!("{BOTTOM_OF_THE_NINTH}; wait 60; state");
    assert_eq!(state_after(&mut script, &setup), "Menu, MatchSetup, Medium");
    let summary = format!("{NEXT}; wait 60; state");
    assert_eq!(
        state_after(&mut script, &summary),
        "Menu, MatchSummary, Medium"
    );
    let play = format!("{PLAY_BALL}; wait 120; state");
    assert!(state_after(&mut script, &play).starts_with("Match"));
}

#[test]
fn the_menu_leads_through_setup_to_the_arcade_game() {
    let Some(mut script) = game("menu") else {
        return;
    };
    let steps = format!("wait 60; {ARCADE}; wait 60; {NEXT}; wait 60; state");
    assert_eq!(
        state_after(&mut script, &steps),
        "Menu, ArcadeSummary, Medium"
    );
    let play = format!("{PLAY_BALL}; wait 120; state");
    assert!(state_after(&mut script, &play).starts_with("Arcade"));
}

#[test]
fn back_returns_to_the_page_before() {
    let Some(mut script) = game("menu") else {
        return;
    };
    let steps =
        format!("wait 60; {BOTTOM_OF_THE_NINTH}; wait 60; {NEXT}; wait 60; {BACK}; wait 60; state");
    assert_eq!(state_after(&mut script, &steps), "Menu, MatchSetup, Medium");
    let steps = format!("{BACK}; wait 60; state");
    assert_eq!(state_after(&mut script, &steps), "Menu, Main, Medium");
}

#[test]
fn a_page_that_is_still_arriving_ignores_clicks() {
    let Some(mut script) = game("menu") else {
        return;
    };
    // No wait first: the menu has only begun to fade in.
    let steps = format!("{BOTTOM_OF_THE_NINTH}; wait 60; state");
    assert_eq!(state_after(&mut script, &steps), "Menu, Main, Medium");
}

#[test]
fn the_team_name_can_be_typed() {
    let Some(mut script) = game("menu") else {
        return;
    };
    let steps = format!(
        "wait 60; {BOTTOM_OF_THE_NINTH}; wait 60; click 450 134; type Red Sox 9; \
         key backspace; key backspace"
    );
    script.run(&steps).unwrap();
    assert_eq!(script.runner.stage.text("teamName"), Some("Red Sox"));
    // A click off the field ends the typing, so later keys change nothing.
    script.run("click 300 300; type xyz").unwrap();
    assert_eq!(script.runner.stage.text("teamName"), Some("Red Sox"));
}

#[test]
fn a_colour_picked_from_the_strip_dresses_the_batter_and_goes_into_the_match() {
    let Some(mut script) = game("menu") else {
        return;
    };
    // The green part of the Team Colours strip on the match setup page.
    let steps = format!("wait 60; {BOTTOM_OF_THE_NINTH}; wait 60; click 440 194; wait 5");
    script.run(&steps).unwrap();
    let colour_of = |script: &bb_game::script::Script, name: &str| {
        let stage = &script.runner.stage;
        let path = stage.find_named(&[], name).expect("the part to be there");
        stage.child(&path).unwrap().color
    };
    let helmet = colour_of(&script, "helmetMovie");
    // A flat tint: the art's own colours are thrown away for the one picked.
    assert_eq!(helmet.mult, [0.0, 0.0, 0.0, 1.0]);
    let [red, green, blue, _] = helmet.add;
    assert!(green > red && green > blue, "{:?}", helmet.add);
    assert_eq!(colour_of(&script, "tShirtMovie"), helmet);

    // In the match the batter wears it too, and has a skin of his own.
    let play = format!("{NEXT}; wait 60; {PLAY_BALL}; wait 150");
    script.run(&play).unwrap();
    assert!(state_after(&mut script, "state").starts_with("Match"));
    assert_eq!(colour_of(&script, "helmetMovie"), helmet);
    assert_eq!(colour_of(&script, "skinMovie").mult, [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn the_menu_has_music_and_a_game_has_a_crowd() {
    let Some(mut script) = game("menu") else {
        return;
    };
    let note = |script: &bb_game::script::Script, name: &str| {
        format!("sound {}", script.runner.library.manifest.exports[name])
    };
    let (music, crowd) = (
        note(&script, "introMusic_loop"),
        note(&script, "crowd_loop_1"),
    );
    let heard = script.run("wait 60; events").unwrap();
    assert!(heard.iter().any(|line| line.trim() == music), "{heard:?}");
    assert!(!heard.iter().any(|line| line.trim() == crowd), "{heard:?}");

    let steps =
        format!("{BOTTOM_OF_THE_NINTH}; wait 60; {NEXT}; wait 60; {PLAY_BALL}; wait 200; events");
    let heard = script.run(&steps).unwrap();
    assert!(heard.iter().any(|line| line.trim() == crowd), "{heard:?}");
}

#[test]
fn an_arcade_score_is_shown_on_the_high_score_page() {
    let Some(mut script) = game("arcade") else {
        return;
    };
    // Ten pitches let go by: a score of nothing, but a score.
    for _ in 0..10 {
        script.run("wait 320; click 545 355").unwrap();
    }
    assert!(state_after(&mut script, "wait 420; state").starts_with("ArcadeFinish"));
    // The finish screen's HIGH SCORES button.
    assert_eq!(
        state_after(&mut script, "click 300 373; wait 120; state"),
        "Menu, HighScores, Medium"
    );
    let stage = &script.runner.stage;
    let mut written = Vec::new();
    for line in bb_game::art::all_named(stage, &[], "scoreLine") {
        written.extend(stage.child(&line).unwrap().said.clone());
    }
    for wanted in ["HIGHSCORES", "1", "PLAYER", "0"] {
        assert!(written.iter().any(|text| text == wanted), "{written:?}");
    }
}

#[test]
fn the_pointer_is_hidden_only_while_the_ring_is_being_aimed() {
    let Some(mut script) = game("match") else {
        return;
    };
    // Over the batting area before the pitch: the ring stands for it.
    script.run("wait 30; move 300 250; wait 5").unwrap();
    assert!(script.runner.stage.hide_pointer);
    // Off to the side, over the quit button's corner: an ordinary pointer.
    script.run("move 20 390; wait 5").unwrap();
    assert!(!script.runner.stage.hide_pointer);
    // And once the pitch has gone by, wherever it is.
    script.run("move 300 250; wait 400").unwrap();
    assert!(state_after(&mut script, "state").contains("Ready"));
    assert!(!script.runner.stage.hide_pointer);
}
