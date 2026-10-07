//! Runs a stage together with the game's own rules.
//!
//! The timelines only know how to play. Everything a script did in the
//! original, such as moving between screens, keeping score or reacting to a
//! click, is the job of a [`Logic`]. A [`Runner`] plays the stage frame by
//! frame and hands the logic everything that happens.

use bb_format::{EnvelopePoint, SoundEvent, SoundStart};

use crate::audio::Audio;
use crate::display::Event;
use crate::input::{Geometry, Key};
use crate::library::Library;
use crate::stage::Stage;

/// The most rounds of "the logic reacts, which causes more events" to follow
/// in one go, in case two handlers keep setting each other off.
const MAX_ROUNDS: usize = 32;

/// The game's rules. Every method has a default that does nothing.
pub trait Logic {
    /// Called once, before the first frame is played.
    fn start(&mut self, _stage: &mut Stage, _library: &Library) {}

    /// Called for each thing the stage reports: a button the pointer has
    /// touched, a scripted frame a clip has landed on, or a sound.
    fn event(&mut self, _event: &Event, _stage: &mut Stage, _library: &Library) {}

    /// Called once a frame, after the timelines have moved on.
    fn tick(&mut self, _stage: &mut Stage, _library: &Library) {}

    /// Called for each key the player presses that no text field has taken.
    /// Returns whether the rules had a use for it.
    fn key(&mut self, _key: &Key, _stage: &mut Stage, _library: &Library) -> bool {
        false
    }

    /// A short account of where the game is, for an inspector or a test.
    fn describe(&self) -> String {
        String::new()
    }
}

/// Rules that do nothing: the timelines just play.
pub struct NoLogic;

impl Logic for NoLogic {}

pub struct Runner {
    pub library: Library,
    pub stage: Stage,
    logic: Box<dyn Logic>,
    /// `None` to play in silence.
    pub audio: Option<Audio>,
    /// How many sounds have been asked for, heard or not.
    pub sounds_asked: u32,
    /// A line for each event since the last call to [`Runner::take_notes`],
    /// for an inspector to show.
    notes: Vec<String>,
    started: bool,
}

impl Runner {
    pub fn new(
        library: Library,
        stage: Stage,
        logic: Box<dyn Logic>,
        audio: Option<Audio>,
    ) -> Runner {
        Runner {
            library,
            stage,
            logic,
            audio,
            sounds_asked: 0,
            notes: Vec::new(),
            started: false,
        }
    }

    /// Plays one frame.
    pub fn tick(&mut self, geometry: &mut dyn Geometry) {
        self.ensure_started();
        self.stage.advance(&self.library, geometry);
        self.react();
        self.logic.tick(&mut self.stage, &self.library);
        self.react();
    }

    /// Tells the stage where the pointer is, in stage coordinates, and
    /// whether its button is held.
    pub fn pointer(&mut self, x: f32, y: f32, down: bool, geometry: &mut dyn Geometry) {
        self.ensure_started();
        self.stage
            .pointer_changed(x, y, down, &self.library, geometry);
        self.react();
    }

    /// Takes in a key the player has pressed. A text field being typed in
    /// gets it first, and the logic gets it otherwise. Returns whether either
    /// had a use for it.
    pub fn key(&mut self, key: Key) -> bool {
        self.ensure_started();
        let used = self.stage.key(&key, &self.library)
            || self.logic.key(&key, &mut self.stage, &self.library);
        self.notes.push(format!("key {key:?}"));
        self.react();
        used
    }

    /// Lets the logic act on anything that has happened without playing a
    /// frame, for instance after an inspector has moved a clip.
    pub fn settle(&mut self) {
        self.ensure_started();
        self.react();
    }

    /// The logic's own account of where the game is.
    pub fn describe(&self) -> String {
        self.logic.describe()
    }

    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    fn ensure_started(&mut self) {
        if !self.started {
            self.started = true;
            self.logic.start(&mut self.stage, &self.library);
            self.react();
        }
    }

    /// Hands the logic every waiting event, and then the events its
    /// reactions cause, until things are quiet.
    fn react(&mut self) {
        for _ in 0..MAX_ROUNDS {
            let events = self.stage.take_events();
            if events.is_empty() {
                return;
            }
            for event in &events {
                match event {
                    Event::Sound(start) => {
                        self.sounds_asked += 1;
                        if let Some(audio) = &mut self.audio {
                            audio.play(&self.library, start);
                        }
                        self.notes.push(format!("sound {}", start.sound));
                    }
                    Event::Button {
                        symbol,
                        path,
                        event,
                    } => self
                        .notes
                        .push(format!("button {symbol} at {path:?}: {event:?}")),
                    Event::Frame {
                        symbol,
                        path,
                        frame,
                    } => {
                        let clip =
                            symbol.map_or("main timeline".to_owned(), |id| format!("clip {id}"));
                        self.notes
                            .push(format!("{clip} at {path:?}: frame {frame}"));
                    }
                }
                self.logic.event(event, &mut self.stage, &self.library);
            }
        }
    }
}

impl Stage {
    /// Asks for a sound by its export name, as `attachSound` used: played
    /// `loops` times. Returns whether there is a sound with that name.
    pub fn play_sound(&mut self, name: &str, loops: u16, library: &Library) -> bool {
        self.sound(name, SoundEvent::Event, loops, library)
    }

    /// Sets how loud the sound with this export name is whenever the game
    /// asks for it, from 0 to 1, as `setVolume` did. Returns whether there is
    /// a sound with that name.
    pub fn set_sound_level(&mut self, name: &str, level: f32, library: &Library) -> bool {
        let Some(&sound) = library.manifest.exports.get(name) else {
            return false;
        };
        self.levels.insert(sound, level.clamp(0.0, 1.0));
        true
    }

    /// Stops every copy of the sound with this export name.
    pub fn stop_sound(&mut self, name: &str, library: &Library) -> bool {
        self.sound(name, SoundEvent::Stop, 0, library)
    }

    fn sound(&mut self, name: &str, event: SoundEvent, loops: u16, library: &Library) -> bool {
        let Some(&sound) = library.manifest.exports.get(name) else {
            return false;
        };
        // A sound the game has turned down starts at that level.
        let envelope = self
            .levels
            .get(&sound)
            .map(|&level| EnvelopePoint {
                sample: 0,
                left: level,
                right: level,
            })
            .into_iter()
            .collect();
        self.push_event(Event::Sound(SoundStart {
            sound,
            event,
            loops,
            in_sample: None,
            out_sample: None,
            envelope,
        }));
        true
    }
}
