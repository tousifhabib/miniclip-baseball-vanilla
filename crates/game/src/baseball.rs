//! The game's own rules: which screen is showing and what each button does.
//!
//! The art only knows how to play its animations. Moving between screens is
//! decided here, by jumping the art's clips to their labelled frames.

use bb_engine::app::Logic;
use bb_engine::display::{ButtonEvent, Event, Path};
use bb_engine::input::Key;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
use bb_format::SymbolId;

use crate::art::{self, ButtonLabels};
use crate::look::{self, Look, Rgb, Swatch};
use crate::menu::{Game, Leave, Menu, MenuPage};
use crate::play::{Match, Outcome};
use crate::rng::Rng;
use crate::rules::Rules;
use crate::scores::Scores;
use crate::settings::Difficulty;

/// What the player is looking at. Each is a labelled frame of the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The art has not reached the shell yet.
    Loading,
    Intro,
    Menu,
    Match,
    Arcade,
    MatchLost,
    MatchWon,
    InningsTied,
    ArcadeFinish,
    Instructions,
}

impl Screen {
    const ALL: [Screen; 9] = [
        Screen::Intro,
        Screen::Menu,
        Screen::Match,
        Screen::Arcade,
        Screen::MatchLost,
        Screen::MatchWon,
        Screen::InningsTied,
        Screen::ArcadeFinish,
        Screen::Instructions,
    ];

    /// The screen the shell shows on the frame with this label.
    pub fn from_label(label: &str) -> Option<Screen> {
        Screen::ALL
            .into_iter()
            .find(|screen| screen.label() == Some(label))
    }

    /// The shell's label for this screen.
    fn label(self) -> Option<&'static str> {
        Some(match self {
            Screen::Loading => return None,
            Screen::Intro => "intro",
            Screen::Menu => "menu",
            Screen::Match => "match",
            Screen::Arcade => "arcade",
            Screen::MatchLost => "matchLost",
            Screen::MatchWon => "matchWon",
            Screen::InningsTied => "inningsTied",
            Screen::ArcadeFinish => "arcadeFinish",
            Screen::Instructions => "instructionsAll",
        })
    }
}

pub struct Baseball {
    screen: Screen,
    menu: Menu,
    game: Game,
    labels: ButtonLabels,
    /// The screen to open on, if not the intro.
    first: Option<Screen>,
    /// The match being played, while the match screen is showing.
    play: Option<Match>,
    /// What the game's chances are worked out from, if not the clock.
    seed: Option<u64>,
    /// Frames until a game that has been quit gives way to the menu.
    leaving: Option<u32>,
    /// The pointer is over one of the setup pages' colour strips.
    over_strip: bool,
    scores: Scores,
    /// Where the scores are kept. `None` keeps them only for this run.
    scores_file: Option<std::path::PathBuf>,
    /// The lines of the score table on the stage, while its page is up.
    table: Vec<Path>,
    /// Whether the menu's music and the game's crowd are being heard.
    music_on: bool,
    crowd_on: bool,
    /// The strips on the setup pages that colours are picked from.
    clothes_strip: Option<Swatch>,
    skin_strip: Option<Swatch>,
    /// Clips that are playing an animation and must stop when they reach
    /// this frame, where the art has no stop of its own.
    holds: Vec<(Path, u16)>,
}

impl Baseball {
    pub fn new(library: &Library) -> Baseball {
        Baseball {
            screen: Screen::Loading,
            menu: Menu::default(),
            game: Game::default(),
            labels: ButtonLabels::read(library),
            first: None,
            play: None,
            seed: None,
            leaving: None,
            over_strip: false,
            scores: Scores::default(),
            scores_file: None,
            table: Vec::new(),
            music_on: false,
            crowd_on: false,
            // A game whose art has no such strip is played in the art's own
            // colours.
            clothes_strip: Swatch::of_clip(library, art::CLOTHES_STRIP).ok(),
            skin_strip: Swatch::of_clip(library, art::SKIN_STRIP).ok(),
            holds: Vec::new(),
        }
    }

    /// Keeps the high scores in this file, starting from what it holds.
    pub fn keep_scores_in(&mut self, file: std::path::PathBuf) {
        self.scores = Scores::load(&file);
        self.scores_file = Some(file);
    }

    /// Writes the score table over the panel on the high-score page, for as
    /// long as that page is up.
    fn show_scores(&mut self, stage: &mut Stage, library: &Library) {
        // The lines go when the panel does, as the page is left.
        if self
            .table
            .first()
            .is_some_and(|line| stage.child(line).is_none())
        {
            self.table.clear();
        }
        let on_page = self.screen == Screen::Menu && self.menu.page() == MenuPage::HighScores;
        if !on_page || !self.table.is_empty() {
            return;
        }
        let Some(panel) =
            art::shell(stage).and_then(|shell| stage.find_symbol(&shell, art::SCORE_PANEL))
        else {
            return;
        };
        // The panel says the scores are kept on a web site. Here they are
        // not, so that goes and the table takes its place.
        let notice: Vec<Path> = stage.clip(&panel).map_or(Vec::new(), |clip| {
            clip.children
                .iter()
                .filter(|(_, child)| art::SCORE_PANEL_NOTICE.contains(&child.symbol))
                .map(|(&depth, _)| {
                    let mut path = panel.clone();
                    path.push(depth);
                    path
                })
                .collect()
        });
        if notice.is_empty() {
            // The panel has not finished arriving.
            return;
        }
        for path in notice {
            if let Some(child) = stage.child_mut(&path) {
                child.set_visible(false);
            }
        }
        let Some(field) = library.edit_texts.get(&art::TABLE_FIELD) else {
            return;
        };
        // The field centres what it says, so a line is placed by its middle.
        let middle = ((field.bounds.x_min + field.bounds.x_max) / 2.0) as f32;
        // What to write, where its middle goes, and in what colour: dark
        // on the panel's white, and white for the heading on its bar. The
        // heading was drawn in one piece with the notice, so it is written
        // back in.
        const DARK: [u8; 3] = [0x0b, 0x3a, 0x5e];
        const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
        let mut lines = vec![("HIGHSCORES".to_owned(), -104.0, -123.0, WHITE)];
        if self.scores.entries.is_empty() {
            lines.push(("NO SCORES YET".to_owned(), 0.0, -10.0, DARK));
        }
        for (place, entry) in self.scores.entries.iter().enumerate() {
            let down = -88.0 + place as f32 * 19.0;
            lines.push((format!("{}", place + 1), -150.0, down, DARK));
            lines.push((entry.name.to_uppercase(), -35.0, down, DARK));
            lines.push((entry.points.to_string(), 125.0, down, DARK));
        }
        const SIZE: f32 = 0.8;
        for (index, (text, across, down, colour)) in lines.into_iter().enumerate() {
            let depth = Stage::RULES_DEPTH + 200 + index as u16;
            let Some(path) = stage.attach(&panel, art::TABLE_FIELD, depth, "scoreLine", library)
            else {
                continue;
            };
            if let Some(child) = stage.child_mut(&path) {
                child.said = Some(text);
                child.set_matrix(Matrix {
                    a: SIZE,
                    d: SIZE,
                    tx: across - middle * SIZE,
                    ty: down,
                    ..Matrix::IDENTITY
                });
                child.set_color(look::tint(colour));
            }
            self.table.push(path);
        }
    }

    /// Starts and stops the music and the crowd for the screen being shown.
    /// The music belongs to the menu and the screens a game ends on. The
    /// crowd is heard under a game.
    fn sound_for(&mut self, screen: Screen, stage: &mut Stage, library: &Library) {
        let sound = &self.game.rules.sound;
        for (name, level) in &sound.levels {
            stage.set_sound_level(name, *level, library);
        }
        let in_game = matches!(screen, Screen::Match | Screen::Arcade);
        let wants_music = screen == Screen::Menu;
        let stops_music = in_game || screen == Screen::Instructions;
        if wants_music && !self.music_on {
            self.music_on = stage.play_sound(&sound.music, 999, library);
        } else if stops_music && self.music_on {
            stage.stop_sound(&sound.music, library);
            self.music_on = false;
        }
        if in_game && !self.crowd_on {
            self.crowd_on = stage.play_sound(&sound.crowd, 999, library);
        } else if screen == Screen::Menu && self.crowd_on {
            stage.stop_sound(&sound.crowd, library);
            self.crowd_on = false;
        }
    }

    /// The colour under the pointer on one of the setup pages' strips.
    fn picked(stage: &Stage, strip: &Option<Swatch>, name: &str) -> Option<Rgb> {
        let shell = art::shell(stage)?;
        let path = stage.find_named(&shell, name)?;
        let (x, y) = stage.from_stage(&path, stage.pointer.x, stage.pointer.y)?;
        strip.as_ref()?.at(x, y)
    }

    /// Acts on a click on one of the setup pages' colour and logo choices.
    fn choose_look(&mut self, button: SymbolId, stage: &Stage) {
        let settings = &mut self.game.settings;
        if art::CLOTHES_STRIP_BUTTONS.contains(&button) {
            if let Some(colour) = Baseball::picked(stage, &self.clothes_strip, "clothesPicker") {
                settings.clothes = Some(colour);
            }
        } else if art::SKIN_STRIP_BUTTONS.contains(&button) {
            if let Some(colour) = Baseball::picked(stage, &self.skin_strip, "skinPicker") {
                settings.skin = Some(colour);
            }
        } else if button == art::CLOTHES_BUTTON.0 {
            settings.clothes = Some(art::CLOTHES_BUTTON.1);
        } else if button == art::SKIN_BUTTON.0 {
            settings.skin = Some(art::SKIN_BUTTON.1);
        } else if let Some((_, logo)) = art::LOGO_BUTTONS.iter().find(|(id, _)| *id == button) {
            settings.logo = Some((*logo).to_owned());
        }
    }

    /// How the batting side should look on the screen that is showing.
    fn look(&self) -> Look {
        let settings = &self.game.settings;
        match (&self.play, self.screen) {
            (Some(play), Screen::Match) => play.look(settings.clothes),
            // The arcade game and the setup pages show what was chosen.
            _ => Look {
                clothes: settings.clothes,
                skin: settings.skin,
                logo: settings.logo.clone(),
                second_skin: None,
            },
        }
    }

    /// Makes every game go the same way, for a test or for chasing a fault.
    pub fn seed(&mut self, seed: u64) {
        self.seed = Some(seed);
    }

    /// Plays by `rules` instead of the ones built in.
    pub fn play_by(&mut self, rules: Rules) {
        self.game.rules = rules;
    }

    /// Opens on `screen` instead of the intro.
    pub fn start_on(&mut self, screen: Screen) {
        self.first = Some(screen);
    }

    /// Switches to another screen.
    fn show(&mut self, screen: Screen, stage: &mut Stage, library: &Library) {
        let (Some(shell), Some(label)) = (art::shell(stage), screen.label()) else {
            return;
        };
        let from_intro = self.screen == Screen::Intro;
        self.holds.clear();
        stage.goto_label(&shell, label, false, library);
        self.screen = screen;
        self.leaving = None;
        self.over_strip = false;
        self.sound_for(screen, stage, library);
        let seed = self.seed.unwrap_or_else(Rng::seed_from_clock);
        self.play = match screen {
            Screen::Match => Some(Match::new(&self.game, seed, library)),
            Screen::Arcade => Some(Match::new_arcade(&self.game, seed, library)),
            _ => None,
        };
        if screen == Screen::Menu {
            self.menu.shown(from_intro, &self.game, stage, library);
        }
    }

    /// Goes where the menu has asked to go.
    fn leave_menu(&mut self, leave: Leave, stage: &mut Stage, library: &Library) {
        let screen = match leave {
            Leave::Instructions => Screen::Instructions,
            Leave::Match => Screen::Match,
            Leave::Arcade => Screen::Arcade,
        };
        self.show(screen, stage, library);
    }

    /// Acts on a button the player has clicked.
    fn clicked(&mut self, button: SymbolId, stage: &mut Stage, library: &Library) {
        let label = self.labels.get(button).unwrap_or_default().to_owned();
        let label = label.as_str();
        match self.screen {
            Screen::Intro if label == "SKIP" => self.show(Screen::Menu, stage, library),
            Screen::Menu => {
                let leave = self.menu.clicked(label, &mut self.game, stage, library);
                if let Some(leave) = leave {
                    self.leave_menu(leave, stage, library);
                }
            }
            Screen::Match | Screen::Arcade => match label {
                "QUIT" => self.quit_prompt(true, stage, library),
                "NO" => self.quit_prompt(false, stage, library),
                "YES" => {
                    // A flare covers the way out, as it covers the way to
                    // the next pitch.
                    if let Some(flare) = stage.find_named(&[], "lightFlareQuit") {
                        stage.goto_clip(&flare, 2, library);
                        if let Some(clip) = stage.clip_mut(&flare) {
                            clip.playing = true;
                        }
                    }
                    self.leaving = Some(11);
                }
                _ => {}
            },
            Screen::MatchLost | Screen::MatchWon | Screen::InningsTied | Screen::ArcadeFinish => {
                if label.contains("HIGH SCORES") {
                    self.show(Screen::Menu, stage, library);
                    self.menu
                        .open(MenuPage::HighScores, &self.game, stage, library);
                } else if label == "MAIN MENU" || art::CONTINUE_BUTTONS.contains(&button) {
                    self.show(Screen::Menu, stage, library);
                }
            }
            Screen::Instructions => self.instructions_clicked(label, stage, library),
            Screen::Loading | Screen::Intro => {}
        }
    }

    /// Slides the "are you sure?" panel on, or puts it away.
    fn quit_prompt(&mut self, on: bool, stage: &mut Stage, library: &Library) {
        let Some(prompt) = stage.find_symbol(&[], art::QUIT_PROMPT) else {
            return;
        };
        if on {
            // The slide runs to the end of the clip, which would loop back
            // to its hidden first frame if left to play on.
            let last = stage
                .clip(&prompt)
                .map_or(1, |clip| clip.frame_count(library));
            stage.goto_label(&prompt, "onScreen", true, library);
            self.holds.push((prompt, last));
        } else {
            self.holds.retain(|(path, _)| *path != prompt);
            stage.goto_label(&prompt, "offScreen", false, library);
        }
    }

    fn instructions_clicked(&mut self, label: &str, stage: &mut Stage, library: &Library) {
        let Some(path) = art::INSTRUCTIONS
            .iter()
            .find_map(|&clip| art::in_shell(stage, clip))
        else {
            return;
        };
        match label {
            "MENU" | "SKIP" => self.show(Screen::Menu, stage, library),
            // Each page stops at its end, so going on is just playing again.
            "NEXT" => {
                if let Some(clip) = stage.clip_mut(&path) {
                    clip.playing = true;
                }
            }
            "BACK" => {
                let Some(clip) = stage.clip(&path) else {
                    return;
                };
                let Some(timeline) = library.timeline(clip.symbol) else {
                    return;
                };
                // The pages start at the labelled frames. The one being
                // shown is the last to start at or before this frame, and
                // the one to go back to is the one before that.
                let mut starts: Vec<u16> = timeline.labels.values().copied().collect();
                starts.sort_unstable();
                let before: Vec<u16> = starts
                    .into_iter()
                    .filter(|&start| start <= clip.frame)
                    .collect();
                if let [.., previous, _] = before[..] {
                    stage.goto_clip(&path, previous, library);
                    if let Some(clip) = stage.clip_mut(&path) {
                        clip.playing = true;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Logic for Baseball {
    fn event(&mut self, event: &Event, stage: &mut Stage, library: &Library) {
        if let Some(play) = &mut self.play {
            play.event(event, &self.game, stage, library);
        }
        if let Event::Button { symbol, event, .. } = event
            && (art::CLOTHES_STRIP_BUTTONS.contains(symbol)
                || art::SKIN_STRIP_BUTTONS.contains(symbol))
        {
            match event {
                ButtonEvent::RollOver | ButtonEvent::DragOver => self.over_strip = true,
                ButtonEvent::RollOut | ButtonEvent::DragOut | ButtonEvent::ReleaseOutside => {
                    self.over_strip = false;
                }
                _ => {}
            }
        }
        if let Event::Button {
            symbol,
            event: ButtonEvent::Release,
            ..
        } = event
        {
            self.choose_look(*symbol, stage);
            self.clicked(*symbol, stage, library);
        }
    }

    fn key(&mut self, key: &Key, stage: &mut Stage, library: &Library) -> bool {
        // The space bar takes the next pitch, as its button does.
        match (key, &mut self.play) {
            (Key::Char(' '), Some(play)) => play.take_next_pitch(stage, library),
            _ => false,
        }
    }

    fn tick(&mut self, stage: &mut Stage, library: &Library) {
        if let Some(left) = self.leaving {
            if left == 0 {
                self.show(Screen::Menu, stage, library);
            } else {
                self.leaving = Some(left - 1);
            }
        }
        self.holds
            .retain(|(path, frame)| match stage.clip_mut(path) {
                Some(clip) if clip.frame >= *frame => {
                    clip.playing = false;
                    false
                }
                Some(_) => true,
                // The clip has gone, and its hold with it.
                None => false,
            });
        if let Some(play) = &mut self.play
            && let Some(outcome) = play.tick(&self.game, stage, library)
        {
            play.show_result(stage);
            play.show_arcade_result(&self.game, stage);
            if let Some(points) = play.arcade_score(&self.game) {
                // Under the name typed on the setup page, if one was.
                let name = stage
                    .text("playerName")
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("PLAYER")
                    .to_owned();
                if self.scores.add(&name, points)
                    && let Some(file) = &self.scores_file
                    && let Err(error) = self.scores.save(file)
                {
                    eprintln!("The score could not be saved: {error:#}");
                }
            }
            let screen = match outcome {
                Outcome::Won => Screen::MatchWon,
                Outcome::Lost => Screen::MatchLost,
                Outcome::Tied => Screen::InningsTied,
                Outcome::ArcadeOver => Screen::ArcadeFinish,
            };
            self.show(screen, stage, library);
        }
        // The arcade game's finish screen names the skill level played, on
        // a clip with a frame for each. It appears part of the way through
        // the screen's arrival, so it is set whenever it is there.
        if self.screen == Screen::ArcadeFinish
            && let Some(shell) = art::shell(stage)
        {
            let frame = match self.game.settings.difficulty {
                Difficulty::Easy => 1,
                Difficulty::Medium => 2,
                Difficulty::Hard => 3,
            };
            for path in art::all_named(stage, &shell, "skillLevelText") {
                stage.goto_clip(&path, frame, library);
                if let Some(clip) = stage.clip_mut(&path) {
                    clip.playing = false;
                }
            }
        }
        if let Some(shell) = art::shell(stage) {
            look::dress(stage, &shell, &self.look(), library);
        }
        self.show_scores(stage, library);
        // A pointer hidden for aiming comes back for the quit prompt, and
        // whenever no game is being played.
        let prompt_up = stage
            .find_symbol(&[], art::QUIT_PROMPT)
            .and_then(|path| stage.clip(&path))
            .is_some_and(|prompt| prompt.frame > 1);
        if self.play.is_none() || prompt_up {
            stage.hide_pointer = false;
        }
        // Over a colour strip the art has a pointer of its own, a little
        // ring, which takes the system pointer's place. Away from the strips
        // it is kept well off the stage.
        if self.screen == Screen::Menu
            && let Some(shell) = art::shell(stage)
        {
            let pointer = (stage.pointer.x, stage.pointer.y);
            for path in art::all_named(stage, &shell, "picker_mc") {
                let place = self
                    .over_strip
                    .then(|| stage.from_stage(&path[..path.len() - 1], pointer.0, pointer.1))
                    .flatten()
                    .unwrap_or((1000.0, 1000.0));
                if let Some(ring) = stage.child_mut(&path) {
                    ring.move_to(place.0, place.1);
                }
            }
            stage.hide_pointer = self.over_strip;
        }
        match self.screen {
            Screen::Loading => {
                if art::shell(stage).is_some() {
                    match self.first.take() {
                        Some(first) => self.show(first, stage, library),
                        None => self.screen = Screen::Intro,
                    }
                }
            }
            // The intro leads into the menu when it has played out.
            Screen::Intro => {
                let finished = art::in_shell(stage, art::INTRO)
                    .and_then(|path| stage.clip(&path))
                    .is_some_and(|intro| intro.frame >= intro.frame_count(library));
                if finished {
                    self.show(Screen::Menu, stage, library);
                }
            }
            Screen::Menu => {
                if let Some(leave) = self.menu.tick(&self.game, stage, library) {
                    self.leave_menu(leave, stage, library);
                }
            }
            _ => {}
        }
    }

    fn describe(&self) -> String {
        match self.screen {
            Screen::Menu => format!(
                "Menu, {:?}, {:?}",
                self.menu.page(),
                self.game.settings.difficulty
            ),
            screen => {
                let play = self
                    .play
                    .as_ref()
                    .map(|play| format!(": {}", play.describe()))
                    .unwrap_or_default();
                format!("{screen:?}, {:?}{play}", self.game.settings.difficulty)
            }
        }
    }
}
