//! A running movie: the display tree, the pointer, what the text fields say,
//! and a record of what has happened for the caller to act on.

use bb_format::SymbolId;

use crate::display::{Child, ClipState, Command, Content, Event, Path, Texts, commands};
use crate::input::{Geometry, Pointer};
use crate::library::Library;
use crate::math::Matrix;

pub struct Stage {
    pub root: ClipState,
    pub pointer: Pointer,
    /// What the text fields say, by the variable each one shows.
    pub texts: Texts,
    events: Vec<Event>,
}

impl Stage {
    /// Starts playing a clip, or the main timeline for `None`.
    pub fn new(clip: Option<SymbolId>, library: &Library) -> Stage {
        let mut events = Vec::new();
        Stage {
            root: ClipState::new(clip, library, &mut events, &mut Path::new()),
            pointer: Pointer::default(),
            texts: Texts::new(),
            events,
        }
    }

    /// Plays one frame.
    pub fn advance(&mut self, library: &Library, geometry: &mut dyn Geometry) {
        self.root
            .advance(library, &mut self.events, &mut Path::new());
        // Things have moved, so what is under the pointer may have changed.
        let (x, y, down) = (self.pointer.x, self.pointer.y, self.pointer.down);
        self.pointer_changed(x, y, down, library, geometry);
    }

    /// Jumps the top timeline to `frame`.
    pub fn goto(&mut self, frame: u16, library: &Library) {
        self.goto_clip(&[], frame, library);
    }

    /// The object at `path`.
    pub fn child(&self, path: &[u16]) -> Option<&Child> {
        let (last, parents) = path.split_last()?;
        let mut clip = &self.root;
        for depth in parents {
            match &clip.children.get(depth)?.content {
                Content::Clip(inner) => clip = inner,
                _ => return None,
            }
        }
        clip.children.get(last)
    }

    /// The clip at `path`, or the top timeline for an empty path.
    pub fn clip(&self, path: &[u16]) -> Option<&ClipState> {
        if path.is_empty() {
            return Some(&self.root);
        }
        match &self.child(path)?.content {
            Content::Clip(clip) => Some(clip),
            _ => None,
        }
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

    /// Finds an object by the instance names leading to it, as an
    /// ActionScript path like `game.hitter` would: `["game", "hitter"]`.
    /// The search starts inside the clip at `from`.
    pub fn find(&self, from: &[u16], names: &[&str]) -> Option<Path> {
        let mut path = from.to_vec();
        for name in names {
            let clip = self.clip(&path)?;
            let (&depth, _) = clip
                .children
                .iter()
                .find(|(_, child)| child.name.as_deref() == Some(*name))?;
            path.push(depth);
        }
        Some(path)
    }

    /// Finds the first instance of `symbol` at or below the clip at `from`,
    /// looking through each level before going deeper.
    pub fn find_symbol(&self, from: &[u16], symbol: SymbolId) -> Option<Path> {
        self.search(from, &|child| child.symbol == symbol)
    }

    /// Finds the first object with this instance name at or below the clip
    /// at `from`, looking through each level before going deeper.
    pub fn find_named(&self, from: &[u16], name: &str) -> Option<Path> {
        self.search(from, &|child| child.name.as_deref() == Some(name))
    }

    fn search(&self, from: &[u16], wanted: &dyn Fn(&Child) -> bool) -> Option<Path> {
        let mut level = vec![from.to_vec()];
        while !level.is_empty() {
            let mut next = Vec::new();
            for path in &level {
                let Some(clip) = self.clip(path) else {
                    continue;
                };
                for (&depth, child) in &clip.children {
                    let mut here = path.clone();
                    here.push(depth);
                    if wanted(child) {
                        return Some(here);
                    }
                    if matches!(child.content, Content::Clip(_)) {
                        next.push(here);
                    }
                }
            }
            level = next;
        }
        None
    }

    /// Jumps the clip at `path` to `frame`.
    pub fn goto_clip(&mut self, path: &[u16], frame: u16, library: &Library) {
        let mut events = std::mem::take(&mut self.events);
        if let Some(clip) = self.clip_mut(path) {
            clip.goto(frame, library, &mut events, &mut path.to_vec());
        }
        self.events = events;
    }

    /// Jumps the clip at `path` to the frame with this label, and sets it
    /// playing or stopped. Returns whether there is such a clip and label.
    pub fn goto_label(&mut self, path: &[u16], label: &str, play: bool, library: &Library) -> bool {
        let mut events = std::mem::take(&mut self.events);
        let found = self.clip_mut(path).is_some_and(|clip| {
            let found = clip.goto_label(label, library, &mut events, &mut path.to_vec());
            if found {
                clip.playing = play;
            }
            found
        });
        self.events = events;
        found
    }

    /// Makes every text field that shows `variable` say `value`.
    pub fn set_text(&mut self, variable: &str, value: impl Into<String>) {
        self.texts.insert(variable.to_owned(), value.into());
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

    /// Adds something for the caller to act on, as if the timelines had
    /// reported it.
    pub(crate) fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Lists what to draw, back to front.
    pub fn commands(&self, base: Matrix, library: &Library) -> Vec<Command> {
        commands(&self.root, base, library, &self.texts)
    }

    /// What has happened since the last call: sounds to play, buttons the
    /// pointer has touched, and scripted frames that clips have landed on.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}
