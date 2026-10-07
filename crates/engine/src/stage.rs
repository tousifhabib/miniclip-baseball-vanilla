//! A running movie: the display tree, the pointer, and a record of what has
//! happened for the caller to act on.

use bb_format::SymbolId;

use crate::display::{ClipState, Command, Content, Event, commands};
use crate::input::{Geometry, Pointer};
use crate::library::Library;
use crate::math::Matrix;

pub struct Stage {
    pub root: ClipState,
    pub pointer: Pointer,
    events: Vec<Event>,
}

impl Stage {
    /// Starts playing a clip, or the main timeline for `None`.
    pub fn new(clip: Option<SymbolId>, library: &Library) -> Stage {
        let mut events = Vec::new();
        Stage {
            root: ClipState::new(clip, library, &mut events),
            pointer: Pointer::default(),
            events,
        }
    }

    /// Plays one frame.
    pub fn advance(&mut self, library: &Library, geometry: &mut dyn Geometry) {
        self.root.advance(library, &mut self.events);
        // Things have moved, so what is under the pointer may have changed.
        let (x, y, down) = (self.pointer.x, self.pointer.y, self.pointer.down);
        self.pointer_changed(x, y, down, library, geometry);
    }

    /// Jumps the top timeline to `frame`.
    pub fn goto(&mut self, frame: u16, library: &Library) {
        self.root.goto(frame, library, &mut self.events);
    }

    /// The clip at `path`, or the top timeline for an empty path.
    pub fn clip_mut(&mut self, path: &[u16]) -> Option<&mut ClipState> {
        if path.is_empty() {
            return Some(&mut self.root);
        }
        match &mut self.root.child_mut(path)?.content {
            Content::Clip(clip) => Some(clip),
            _ => None,
        }
    }

    /// Jumps the clip at `path` to `frame`.
    pub fn goto_clip(&mut self, path: &[u16], frame: u16, library: &Library) {
        let mut events = std::mem::take(&mut self.events);
        if let Some(clip) = self.clip_mut(path) {
            clip.goto(frame, library, &mut events);
        }
        self.events = events;
    }

    /// Tells the stage where the pointer is, in the top timeline's
    /// coordinates, and whether its button is held.
    pub fn pointer_changed(
        &mut self,
        x: f32,
        y: f32,
        down: bool,
        library: &Library,
        geometry: &mut dyn Geometry,
    ) {
        self.pointer.update(
            &mut self.root,
            x,
            y,
            down,
            library,
            geometry,
            &mut self.events,
        );
    }

    /// Lists what to draw, back to front.
    pub fn commands(&self, base: Matrix, library: &Library) -> Vec<Command> {
        commands(&self.root, base, library)
    }

    /// What has happened since the last call: sounds to play and buttons the
    /// pointer has touched.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}
