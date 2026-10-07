//! Plays the game's sounds through kira.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use bb_format::{SoundEvent, SoundStart, SymbolId, SymbolInfo};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::{PlaybackPosition, PlaybackState};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, StartTime, Tween};

use crate::library::Library;

/// Flash counts the points where a sound starts and stops in samples at this
/// rate, whatever the sound's own rate is.
const POINT_RATE: f64 = 44100.0;
/// Flash Player mixes at most this many sounds at once, and ignores requests
/// for more.
const MAX_PLAYING: usize = 32;
/// A sound asked to repeat at least this many times is taken to mean "for
/// ever", which is how Flash authors wrote looping music.
const ENDLESS: u16 = 1000;

pub struct Audio {
    manager: AudioManager<DefaultBackend>,
    /// Decoded sounds. `None` for one that could not be loaded.
    sounds: HashMap<SymbolId, Option<StaticSoundData>>,
    /// The copies of each sound that may still be playing.
    playing: HashMap<SymbolId, Vec<StaticSoundHandle>>,
    /// Sounds that could not be loaded or played, each reported once.
    pub problems: Vec<String>,
}

impl Audio {
    /// Opens the default sound device.
    pub fn new() -> Result<Audio> {
        let manager = AudioManager::new(AudioManagerSettings::default())
            .context("opening the sound device")?;
        Ok(Audio {
            manager,
            sounds: HashMap::new(),
            playing: HashMap::new(),
            problems: Vec::new(),
        })
    }

    /// Decodes every sound now, so that none has to be decoded mid-game.
    /// Returns how many loaded.
    pub fn load_all(&mut self, library: &Library) -> usize {
        let ids: Vec<SymbolId> = library
            .manifest
            .symbols
            .iter()
            .filter(|(_, symbol)| matches!(symbol.info, SymbolInfo::Sound { .. }))
            .map(|(&id, _)| id)
            .collect();
        ids.into_iter()
            .filter(|&id| self.data(library, id).is_some())
            .count()
    }

    /// Acts on a request from a timeline or a button.
    pub fn play(&mut self, library: &Library, start: &SoundStart) {
        let handles = self.playing.entry(start.sound).or_default();
        handles.retain(|handle| handle.state() != PlaybackState::Stopped);
        match start.event {
            SoundEvent::Stop => {
                for handle in handles.iter_mut() {
                    handle.stop(Tween::default());
                }
                handles.clear();
                return;
            }
            // "Start" means: not if a copy is already playing.
            SoundEvent::Start if !handles.is_empty() => return,
            SoundEvent::Start | SoundEvent::Event => {}
        }

        for handles in self.playing.values_mut() {
            handles.retain(|handle| handle.state() != PlaybackState::Stopped);
        }
        if self.playing.values().map(Vec::len).sum::<usize>() >= MAX_PLAYING {
            return;
        }

        let Some(data) = self.data(library, start.sound) else {
            return;
        };
        let seconds = |samples: u32| f64::from(samples) / POINT_RATE;
        let skipped = seconds(start.in_sample.unwrap_or(0));
        // One play through, from the start point to the end point.
        let length = match start.out_sample {
            Some(out) => (seconds(out) - skipped).max(0.0),
            None => (data.duration().as_secs_f64() - skipped).max(0.0),
        };
        let mut data = data;
        if skipped > 0.0 {
            data = data.start_position(PlaybackPosition::Seconds(skipped));
        }
        // An envelope's first point sets the level the sound starts at.
        if let Some(point) = start.envelope.first() {
            let level = (point.left + point.right) / 2.0;
            let volume = if level > 0.0 {
                Decibels((20.0 * level.log10()).max(Decibels::SILENCE.0))
            } else {
                Decibels::SILENCE
            };
            data = data.volume(volume);
        }
        let repeats = start.loops.max(1);
        if repeats > 1 {
            data = data.loop_region(..);
        }

        match self.manager.play(data) {
            Ok(mut handle) => {
                // A sound with an early end, or a fixed number of repeats,
                // is stopped when its time is up.
                let ends_early = start.out_sample.is_some() || (repeats > 1 && repeats < ENDLESS);
                if ends_early {
                    handle.stop(Tween {
                        start_time: StartTime::Delayed(Duration::from_secs_f64(
                            length * f64::from(repeats),
                        )),
                        duration: Duration::ZERO,
                        ..Tween::default()
                    });
                }
                self.playing.entry(start.sound).or_default().push(handle);
            }
            Err(error) => self.problem(start.sound, format!("could not be played: {error}")),
        }
    }

    /// Stops everything.
    pub fn stop_all(&mut self) {
        for handle in self.playing.values_mut().flatten() {
            handle.stop(Tween::default());
        }
        self.playing.clear();
    }

    /// The decoded sound, loading it on first use.
    fn data(&mut self, library: &Library, id: SymbolId) -> Option<StaticSoundData> {
        if let Some(known) = self.sounds.get(&id) {
            return known.clone();
        }
        let loaded = match library.manifest.symbols.get(&id) {
            Some(symbol) => match (
                &symbol.info,
                StaticSoundData::from_file(library.dir.join(&symbol.file)),
            ) {
                (SymbolInfo::Sound { skip_samples, .. }, Ok(data)) => {
                    // The encoder pads the front of the sound; the file says
                    // by how much.
                    let skip = usize::try_from(*skip_samples).unwrap_or(0);
                    Some(data.start_position(PlaybackPosition::Samples(skip)))
                }
                (SymbolInfo::Sound { .. }, Err(error)) => {
                    self.problem(id, format!("could not be loaded: {error}"));
                    None
                }
                _ => {
                    self.problem(id, "is not a sound".to_owned());
                    None
                }
            },
            None => {
                self.problem(id, "does not exist".to_owned());
                None
            }
        };
        self.sounds.insert(id, loaded.clone());
        loaded
    }

    fn problem(&mut self, id: SymbolId, what: String) {
        let message = format!("sound {id} {what}");
        if !self.problems.contains(&message) {
            self.problems.push(message);
        }
    }
}
