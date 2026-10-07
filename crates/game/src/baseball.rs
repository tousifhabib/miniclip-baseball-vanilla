//! The game's own rules: which screen is showing and what each button does.
//!
//! The art only knows how to play its animations. Moving between screens is
//! decided here, by jumping the art's clips to their labelled frames.

use bb_engine::app::Logic;
use bb_engine::display::{ButtonEvent, Event, Path};
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_format::SymbolId;

use crate::art::{self, ButtonLabels};
use crate::menu::{Leave, Menu, MenuPage};
use crate::settings::Settings;

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
    settings: Settings,
    labels: ButtonLabels,
    /// The screen to open on, if not the intro.
    first: Option<Screen>,
    /// Clips that are playing an animation and must stop when they reach
    /// this frame, where the art has no stop of its own.
    holds: Vec<(Path, u16)>,
}

impl Baseball {
    pub fn new(library: &Library) -> Baseball {
        Baseball {
            screen: Screen::Loading,
            menu: Menu::default(),
            settings: Settings::default(),
            labels: ButtonLabels::read(library),
            first: None,
            holds: Vec::new(),
        }
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
        if screen == Screen::Menu {
            self.menu.shown(from_intro, &self.settings, stage, library);
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
                let leave = self.menu.clicked(label, &mut self.settings, stage, library);
                if let Some(leave) = leave {
                    self.leave_menu(leave, stage, library);
                }
            }
            Screen::Match | Screen::Arcade => match label {
                "QUIT" => self.quit_prompt(true, stage, library),
                "NO" => self.quit_prompt(false, stage, library),
                "YES" => self.show(Screen::Menu, stage, library),
                _ => {}
            },
            Screen::MatchLost | Screen::MatchWon | Screen::InningsTied | Screen::ArcadeFinish => {
                if label.contains("HIGH SCORES") {
                    self.show(Screen::Menu, stage, library);
                    self.menu
                        .open(MenuPage::HighScores, &self.settings, stage, library);
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
                if let Some(leave) = self.menu.tick(&self.settings, stage, library) {
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
                self.settings.difficulty
            ),
            screen => format!("{screen:?}, {:?}", self.settings.difficulty),
        }
    }
}
