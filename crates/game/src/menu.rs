//! The menu: its pages, and what the buttons on each one do.

use bb_engine::display::ClipState;
use bb_engine::library::Library;
use bb_engine::stage::Stage;

use crate::art;
use crate::rules::Rules;
use crate::settings::{Difficulty, Settings};

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

/// Something the menu wants that is not its own to do: another screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    Instructions,
    Match,
    Arcade,
}

/// What every screen works from: the numbers the game is played by, and
/// what the player has chosen.
#[derive(Clone, Debug, Default)]
pub struct Game {
    pub rules: Rules,
    pub settings: Settings,
}

pub struct Menu {
    page: MenuPage,
    /// A page is on its way in, and has things to be set once it is there.
    arriving: bool,
}

impl Default for Menu {
    fn default() -> Menu {
        Menu {
            page: MenuPage::Opening,
            arriving: false,
        }
    }
}

impl Menu {
    pub fn page(&self) -> MenuPage {
        self.page
    }

    fn clip(stage: &Stage) -> Option<&ClipState> {
        stage.clip(&art::in_shell(stage, art::MENU)?)
    }

    /// The menu has just been put on screen. Coming from the intro it plays
    /// its whole arrival. Coming back from anywhere else it goes straight to
    /// fading in.
    pub fn shown(&mut self, from_intro: bool, game: &Game, stage: &mut Stage, library: &Library) {
        self.page = MenuPage::Opening;
        if !from_intro {
            self.open(MenuPage::Main, game, stage, library);
        }
    }

    /// Plays in another page.
    pub fn open(&mut self, page: MenuPage, game: &Game, stage: &mut Stage, library: &Library) {
        let Some(menu) = art::in_shell(stage, art::MENU) else {
            return;
        };
        if stage.goto_label(&menu, page.label(), true, library) {
            self.page = page;
            // What the summary pages say about the game to come.
            let rules = &game.rules.game;
            let behind = rules.runs_down.at(game.settings.difficulty);
            stage.set_text("oppositionScore", behind.to_string());
            // Drawing level is not enough: the target is one run more.
            stage.set_text("scoreTarget", (behind + 1).to_string());
            stage.set_text("maximumOuts", rules.outs.to_string());
            // The page's own clips only exist once it has played in.
            self.arriving = true;
        }
    }

    /// Moves the marker on the setup pages to the chosen difficulty.
    fn show_difficulty(settings: &Settings, stage: &mut Stage, library: &Library) {
        let marker =
            art::in_shell(stage, art::MENU).and_then(|menu| stage.find_named(&menu, "skillSelect"));
        if let Some(marker) = marker {
            stage.goto_label(&marker, settings.difficulty.label(), false, library);
        }
    }

    /// Acts on a button the player has clicked, known by the words on it.
    pub fn clicked(
        &mut self,
        label: &str,
        game: &mut Game,
        stage: &mut Stage,
        library: &Library,
    ) -> Option<Leave> {
        use MenuPage::*;
        // A page that is still arriving cannot be used yet.
        if Menu::clip(stage).is_none_or(|menu| menu.playing) {
            return None;
        }
        let page = match (self.page, label) {
            (Main, "BOTTOM OF THE NINTH") => MatchSetup,
            (Main, "ARCADE") => ArcadeSetup,
            (Main, "INSTRUCTIONS") => return Some(Leave::Instructions),
            (Main, label) if label.starts_with("HIGH SCORES") => HighScores,
            (MatchSetup | ArcadeSetup | HighScores, "BACK") => Main,
            (MatchSummary, "BACK") => MatchSetup,
            (ArcadeSummary, "BACK") => ArcadeSetup,
            (MatchSetup, "NEXT") => MatchSummary,
            (ArcadeSetup, "NEXT") => ArcadeSummary,
            (MatchSummary, "PLAY BALL") => ToMatch,
            (ArcadeSummary, "PLAY BALL") => ToArcade,
            (MatchSetup | ArcadeSetup, "EASY" | "MEDIUM" | "HARD") => {
                game.settings.difficulty = match label {
                    "EASY" => Difficulty::Easy,
                    "MEDIUM" => Difficulty::Medium,
                    _ => Difficulty::Hard,
                };
                Menu::show_difficulty(&game.settings, stage, library);
                return None;
            }
            _ => return None,
        };
        self.open(page, game, stage, library);
        None
    }

    /// Called once a frame while the menu is showing.
    pub fn tick(&mut self, game: &Game, stage: &mut Stage, library: &Library) -> Option<Leave> {
        let menu = Menu::clip(stage)?;
        let (frame, playing, last) = (menu.frame, menu.playing, menu.frame_count(library));
        let labels = library
            .timeline(menu.symbol)
            .map(|timeline| &timeline.labels);
        if self.arriving && !playing {
            self.arriving = false;
            Menu::show_difficulty(&game.settings, stage, library);
        }
        match self.page {
            MenuPage::Opening if !playing => self.page = MenuPage::Main,
            MenuPage::ToMatch if frame >= last => return Some(Leave::Match),
            // The arcade's fade-out is followed directly by the match's, so
            // it has ended when that is about to begin.
            MenuPage::ToArcade => {
                let next = labels.and_then(|labels| labels.get(MenuPage::ToMatch.label()));
                if next.is_some_and(|&next| frame + 1 >= next) {
                    return Some(Leave::Arcade);
                }
            }
            _ => {}
        }
        None
    }
}
