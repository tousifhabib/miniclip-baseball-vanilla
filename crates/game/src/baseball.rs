//! The game's own rules: which screen is showing and what each button does.
//!
//! The art only knows how to play its animations. Moving between screens is
//! decided here, by jumping the art's clips to their labelled frames.

use bb_engine::app::Logic;
use bb_engine::display::{ButtonEvent, ClipState, Event, Path};
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_format::SymbolId;

use crate::art::{self, ButtonLabels};

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

/// Where the player is in the menu. Each is a section of the menu clip that
/// plays in and then waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPage {
    /// The menu arriving for the first time, before it can be used.
    Opening,
    Main,
    MatchSetup,
    ArcadeSetup,
    HighScores,
    MatchSummary,
    ArcadeSummary,
    /// Fading out on the way to a match.
    ToMatch,
    /// Fading out on the way to the arcade game.
    ToArcade,
}

impl MenuPage {
    /// The label of the section that shows this page.
    fn label(self) -> &'static str {
        match self {
            MenuPage::Opening | MenuPage::Main => "menuFadeIn",
            MenuPage::MatchSetup => "matchIn",
            MenuPage::ArcadeSetup => "arcadeIn",
            MenuPage::HighScores => "highScoresIn",
            MenuPage::MatchSummary => "matchSummary",
            MenuPage::ArcadeSummary => "arcadeSummary",
            MenuPage::ToMatch => "fadeOutMatch",
            MenuPage::ToArcade => "fadeOutArcade",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

/// How many outs a side gets in its innings.
const OUTS: u32 = 3;

impl Difficulty {
    /// In a match you come to bat in the last innings already behind. This
    /// is by how many runs.
    fn runs_down(self) -> u32 {
        match self {
            Difficulty::Easy => 1,
            Difficulty::Medium => 2,
            Difficulty::Hard => 3,
        }
    }
}

pub struct Baseball {
    screen: Screen,
    page: MenuPage,
    difficulty: Difficulty,
    labels: ButtonLabels,
    /// The screen to open on, if not the intro.
    first: Option<Screen>,
    /// A menu page is on its way in, and has things to be set once it is
    /// there.
    arriving: bool,
    /// Clips that are playing an animation and must stop when they reach
    /// this frame, where the art has no stop of its own.
    holds: Vec<(Path, u16)>,
}

impl Baseball {
    pub fn new(library: &Library) -> Baseball {
        Baseball {
            screen: Screen::Loading,
            page: MenuPage::Opening,
            difficulty: Difficulty::Medium,
            labels: ButtonLabels::read(library),
            first: None,
            arriving: false,
            holds: Vec::new(),
        }
    }

    /// Opens on `screen` instead of the intro.
    pub fn start_on(&mut self, screen: Screen) {
        self.first = Some(screen);
    }

    /// Where the shell is in the tree, once the art has reached it.
    fn shell(stage: &Stage) -> Option<Path> {
        stage.find_symbol(&[], art::SHELL)
    }

    /// A clip that sits directly inside the shell.
    fn in_shell(stage: &Stage, symbol: SymbolId) -> Option<Path> {
        stage.find_symbol(&Baseball::shell(stage)?, symbol)
    }

    fn menu(stage: &Stage) -> Option<&ClipState> {
        stage.clip(&Baseball::in_shell(stage, art::MENU)?)
    }

    /// Switches to another screen.
    fn show(&mut self, screen: Screen, stage: &mut Stage, library: &Library) {
        let (Some(shell), Some(label)) = (Baseball::shell(stage), screen.label()) else {
            return;
        };
        let first_menu = self.screen == Screen::Intro;
        self.holds.clear();
        stage.goto_label(&shell, label, false, library);
        self.screen = screen;
        if screen == Screen::Menu {
            // Coming from the intro, the menu plays its whole arrival.
            // Coming back from anywhere else, it goes straight to fading in.
            self.page = MenuPage::Opening;
            if !first_menu {
                self.open(MenuPage::Main, stage, library);
            }
        }
    }

    /// Plays in another page of the menu.
    fn open(&mut self, page: MenuPage, stage: &mut Stage, library: &Library) {
        let Some(menu) = Baseball::in_shell(stage, art::MENU) else {
            return;
        };
        if stage.goto_label(&menu, page.label(), true, library) {
            self.page = page;
            // What the summary pages say about the game to come.
            let behind = self.difficulty.runs_down();
            stage.set_text("oppositionScore", behind.to_string());
            // Drawing level is not enough: the target is one run more.
            stage.set_text("scoreTarget", (behind + 1).to_string());
            stage.set_text("maximumOuts", OUTS.to_string());
            // The page's own clips only exist once it has played in.
            self.arriving = true;
        }
    }

    fn set_difficulty(&mut self, difficulty: Difficulty, stage: &mut Stage, library: &Library) {
        self.difficulty = difficulty;
        self.show_difficulty(stage, library);
    }

    /// Moves the marker on the setup pages to the chosen difficulty.
    fn show_difficulty(&self, stage: &mut Stage, library: &Library) {
        let label = match self.difficulty {
            Difficulty::Easy => "easy",
            Difficulty::Medium => "medium",
            Difficulty::Hard => "hard",
        };
        let marker = Baseball::in_shell(stage, art::MENU)
            .and_then(|menu| stage.find_named(&menu, "skillSelect"));
        if let Some(marker) = marker {
            stage.goto_label(&marker, label, false, library);
        }
    }

    /// Acts on a button the player has clicked.
    fn clicked(&mut self, button: SymbolId, stage: &mut Stage, library: &Library) {
        let label = self.labels.get(button).unwrap_or_default().to_owned();
        let label = label.as_str();
        match self.screen {
            Screen::Intro if label == "SKIP" => self.show(Screen::Menu, stage, library),
            Screen::Menu => self.menu_clicked(label, stage, library),
            Screen::Match | Screen::Arcade => match label {
                "QUIT" => self.quit_prompt(true, stage, library),
                "NO" => self.quit_prompt(false, stage, library),
                "YES" => self.show(Screen::Menu, stage, library),
                _ => {}
            },
            Screen::MatchLost | Screen::MatchWon | Screen::InningsTied | Screen::ArcadeFinish => {
                if label.contains("HIGH SCORES") {
                    self.show(Screen::Menu, stage, library);
                    self.open(MenuPage::HighScores, stage, library);
                } else if label == "MAIN MENU" || art::CONTINUE_BUTTONS.contains(&button) {
                    self.show(Screen::Menu, stage, library);
                }
            }
            Screen::Instructions => self.instructions_clicked(label, stage, library),
            Screen::Loading | Screen::Intro => {}
        }
    }

    fn menu_clicked(&mut self, label: &str, stage: &mut Stage, library: &Library) {
        use MenuPage::*;
        // A page that is still arriving cannot be used yet.
        if Baseball::menu(stage).is_none_or(|menu| menu.playing) {
            return;
        }
        match (self.page, label) {
            (Main, "BOTTOM OF THE NINTH") => self.open(MatchSetup, stage, library),
            (Main, "ARCADE") => self.open(ArcadeSetup, stage, library),
            (Main, "INSTRUCTIONS") => self.show(Screen::Instructions, stage, library),
            (Main, label) if label.starts_with("HIGH SCORES") => {
                self.open(HighScores, stage, library);
            }
            (MatchSetup | ArcadeSetup | HighScores, "BACK") => self.open(Main, stage, library),
            (MatchSummary, "BACK") => self.open(MatchSetup, stage, library),
            (ArcadeSummary, "BACK") => self.open(ArcadeSetup, stage, library),
            (MatchSetup, "NEXT") => self.open(MatchSummary, stage, library),
            (ArcadeSetup, "NEXT") => self.open(ArcadeSummary, stage, library),
            (MatchSummary, "PLAY BALL") => self.open(ToMatch, stage, library),
            (ArcadeSummary, "PLAY BALL") => self.open(ToArcade, stage, library),
            (MatchSetup | ArcadeSetup, "EASY") => {
                self.set_difficulty(Difficulty::Easy, stage, library);
            }
            (MatchSetup | ArcadeSetup, "MEDIUM") => {
                self.set_difficulty(Difficulty::Medium, stage, library);
            }
            (MatchSetup | ArcadeSetup, "HARD") => {
                self.set_difficulty(Difficulty::Hard, stage, library);
            }
            _ => {}
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
            .find_map(|&clip| Baseball::in_shell(stage, clip))
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
        if let Event::Button {
            symbol,
            event: ButtonEvent::Release,
            ..
        } = event
        {
            self.clicked(*symbol, stage, library);
        }
    }

    fn tick(&mut self, stage: &mut Stage, library: &Library) {
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
        match self.screen {
            Screen::Loading => {
                if Baseball::shell(stage).is_some() {
                    match self.first.take() {
                        Some(first) => self.show(first, stage, library),
                        None => self.screen = Screen::Intro,
                    }
                }
            }
            // The intro leads into the menu when it has played out.
            Screen::Intro => {
                let finished = Baseball::in_shell(stage, art::INTRO)
                    .and_then(|path| stage.clip(&path))
                    .is_some_and(|intro| intro.frame >= intro.frame_count(library));
                if finished {
                    self.show(Screen::Menu, stage, library);
                }
            }
            Screen::Menu => {
                let Some(menu) = Baseball::menu(stage) else {
                    return;
                };
                let (frame, playing, last) = (menu.frame, menu.playing, menu.frame_count(library));
                let labels = library
                    .timeline(menu.symbol)
                    .map(|timeline| &timeline.labels);
                if self.arriving && !playing {
                    self.arriving = false;
                    self.show_difficulty(stage, library);
                }
                match self.page {
                    MenuPage::Opening if !playing => self.page = MenuPage::Main,
                    MenuPage::ToMatch if frame >= last => self.show(Screen::Match, stage, library),
                    // The arcade's fade-out is followed directly by the
                    // match's, so it has ended when that is about to begin.
                    MenuPage::ToArcade => {
                        let next = labels.and_then(|labels| labels.get(MenuPage::ToMatch.label()));
                        if next.is_some_and(|&next| frame + 1 >= next) {
                            self.show(Screen::Arcade, stage, library);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn describe(&self) -> String {
        match self.screen {
            Screen::Menu => format!("Menu, {:?}, {:?}", self.page, self.difficulty),
            screen => format!("{screen:?}, {:?}", self.difficulty),
        }
    }
}
