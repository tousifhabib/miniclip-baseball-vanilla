//! A running movie: the display tree, the pointer, what the text fields say,
//! and a record of what has happened for the caller to act on.

use bb_format::SymbolId;

use crate::display::{Child, ClipState, Command, Content, Event, Path, Texts, commands, text_key};
use crate::input::{Geometry, Key, Pointer, field_at};
use crate::library::Library;
use crate::math::Matrix;

pub struct Stage {
    pub root: ClipState,
    pub pointer: Pointer,
    /// What the text fields say, by the variable each one shows.
    pub texts: Texts,
    /// The text field that typing goes to, if the player has clicked on one.
    pub focus: Option<Focus>,
    /// The game wants the system's pointer out of sight, because it is
    /// drawing something of its own where the pointer is.
    pub hide_pointer: bool,
    /// Frames played, for blinking the caret.
    ticks: u32,
    /// How loud each sound is to be played, where the game has said.
    pub(crate) levels: std::collections::HashMap<SymbolId, f32>,
    events: Vec<Event>,
}

/// A text field the player is typing in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub path: Path,
    pub symbol: SymbolId,
}

/// Stands for the caret in the text handed to the renderer. It is in the
/// range set aside for private use, so no font has a letter for it.
pub const CARET: char = '\u{e000}';

/// How many frames the caret shows for, and then hides for.
const BLINK: u32 = 30;

impl Stage {
    /// Starts playing a clip, or the main timeline for `None`.
    pub fn new(clip: Option<SymbolId>, library: &Library) -> Stage {
        let mut events = Vec::new();
        Stage {
            root: ClipState::new(clip, library, &mut events, &mut Path::new()),
            pointer: Pointer::default(),
            texts: Texts::new(),
            focus: None,
            hide_pointer: false,
            ticks: 0,
            levels: std::collections::HashMap::new(),
            events,
        }
    }

    /// The lowest depth for objects the rules add. It is above every depth
    /// a timeline uses, as the depths `attachMovie` gave were in Flash.
    pub const RULES_DEPTH: u16 = 16384;

    /// Plays one frame.
    pub fn advance(&mut self, library: &Library, geometry: &mut dyn Geometry) {
        self.ticks = self.ticks.wrapping_add(1);
        self.root
            .advance(library, &mut self.events, &mut Path::new());
        // A field that has gone takes the typing with it.
        if let Some(focus) = &self.focus
            && self
                .child(&focus.path)
                .is_none_or(|child| child.symbol != focus.symbol)
        {
            self.focus = None;
        }
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

    /// The object at `path`, to move, tint, show or hide.
    pub fn child_mut(&mut self, path: &[u16]) -> Option<&mut Child> {
        self.root.child_mut(path)
    }

    /// The transform from the coordinates inside the object at `path` to the
    /// stage's, or the identity for an empty path.
    pub fn to_stage(&self, path: &[u16]) -> Option<Matrix> {
        let mut matrix = Matrix::IDENTITY;
        for end in 1..=path.len() {
            matrix = matrix.then_inner(self.child(&path[..end])?.matrix);
        }
        Some(matrix)
    }

    /// Where a point of the stage is in the coordinates inside the object at
    /// `path`: what `_xmouse` and `_ymouse` gave for the pointer.
    pub fn from_stage(&self, path: &[u16], x: f32, y: f32) -> Option<(f32, f32)> {
        Some(self.to_stage(path)?.inverse()?.apply(x, y))
    }

    /// Puts a new instance of `symbol` inside the clip at `parent`, at
    /// `depth`, as `attachMovie` did. Use depths from
    /// [`Stage::RULES_DEPTH`] up. Returns where the new object is.
    pub fn attach(
        &mut self,
        parent: &[u16],
        symbol: SymbolId,
        depth: u16,
        name: &str,
        library: &Library,
    ) -> Option<Path> {
        let mut events = std::mem::take(&mut self.events);
        let made = self.clip_mut(parent).is_some_and(|clip| {
            clip.attach(
                symbol,
                depth,
                Some(name),
                library,
                &mut events,
                &mut parent.to_vec(),
            )
            .is_some()
        });
        self.events = events;
        made.then(|| {
            let mut path = parent.to_vec();
            path.push(depth);
            path
        })
    }

    /// Takes the object at `path` off the stage. Returns whether it was
    /// there.
    pub fn remove(&mut self, path: &[u16]) -> bool {
        let Some((depth, parent)) = path.split_last() else {
            return false;
        };
        self.clip_mut(parent)
            .is_some_and(|clip| clip.children.remove(depth).is_some())
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
        let pressed = down && !self.pointer.down;
        self.pointer.update(
            &mut self.root,
            x,
            y,
            down,
            library,
            geometry,
            &mut self.events,
        );
        // A press on a field that can be typed in gives it the typing, and a
        // press anywhere else takes the typing away.
        if pressed {
            self.focus = if self.pointer.on_button() {
                None
            } else {
                field_at(&self.root.children, x, y, library, &mut Path::new())
                    .map(|(path, symbol)| Focus { path, symbol })
            };
            self.ticks = 0;
        }
    }

    /// What a text field showing `variable` says now.
    pub fn text(&self, variable: &str) -> Option<&str> {
        self.texts.get(text_key(variable)).map(String::as_str)
    }

    /// Takes in a key the player has pressed. Returns whether a text field
    /// used it, so that the caller knows not to act on it too.
    pub fn key(&mut self, key: &Key, library: &Library) -> bool {
        let Some(focus) = &self.focus else {
            return false;
        };
        let Some(field) = library.edit_texts.get(&focus.symbol) else {
            return false;
        };
        let variable = text_key(&field.variable).to_owned();
        let mut said = self
            .texts
            .get(&variable)
            .cloned()
            .or_else(|| field.initial_text.clone())
            .unwrap_or_default();
        match key {
            Key::Char(c) => {
                let room = field
                    .max_length
                    .is_none_or(|most| said.chars().count() < usize::from(most));
                // A field can only show the letters its font has.
                let drawable = field
                    .font
                    .and_then(|font| library.fonts.get(&font))
                    .is_none_or(|font| font.glyphs.iter().any(|glyph| glyph.char.starts_with(*c)));
                if room && drawable && !c.is_control() {
                    said.push(*c);
                }
            }
            Key::Backspace => {
                said.pop();
            }
            Key::Enter | Key::Escape | Key::Tab => {
                self.focus = None;
                return true;
            }
            // Other keys are not for the field, but while it has the typing
            // they are not for anything else either.
            _ => return true,
        }
        self.texts.insert(variable, said);
        self.ticks = 0;
        true
    }

    /// Adds something for the caller to act on, as if the timelines had
    /// reported it.
    pub(crate) fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Lists what to draw, back to front.
    pub fn commands(&self, base: Matrix, library: &Library) -> Vec<Command> {
        let mut list = commands(&self.root, base, library, &self.texts);
        // The field being typed in shows a caret after its text, on and off.
        if let Some(focus) = &self.focus
            && (self.ticks / BLINK).is_multiple_of(2)
            && let Some(field) = library.edit_texts.get(&focus.symbol)
        {
            for command in &mut list {
                if let Command::Draw { symbol, text, .. } = command
                    && *symbol == focus.symbol
                {
                    let mut said = text
                        .take()
                        .or_else(|| field.initial_text.clone())
                        .unwrap_or_default();
                    said.push(CARET);
                    *text = Some(said);
                }
            }
        }
        list
    }

    /// What has happened since the last call: sounds to play, buttons the
    /// pointer has touched, and scripted frames that clips have landed on.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use bb_format::{Op, Place, PlaceAction};

    use super::*;
    use crate::display::tests::{FIELD, INNER, SHAPE, add_field, frame, library_with, place, put};

    /// Nothing is ever under the pointer.
    struct Empty;

    impl Geometry for Empty {
        fn contains(&mut self, _: &Library, _: SymbolId, _: u16, _: f32, _: f32) -> bool {
            false
        }
    }

    fn click(stage: &mut Stage, library: &Library, x: f32, y: f32) {
        stage.pointer_changed(x, y, false, library, &mut Empty);
        stage.pointer_changed(x, y, true, library, &mut Empty);
        stage.pointer_changed(x, y, false, library, &mut Empty);
    }

    /// A main timeline with a text field at (100, 100) that can be typed in.
    fn with_field() -> Library {
        let at = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 100.0, 100.0]),
            ..place(1, PlaceAction::Place(FIELD))
        };
        let mut library = library_with(vec![frame(vec![Op::Place(Box::new(at))])], vec![]);
        add_field(&mut library, "_root.teamName", &[]);
        library.edit_texts.get_mut(&FIELD).unwrap().initial_text = None;
        library.edit_texts.get_mut(&FIELD).unwrap().max_length = Some(5);
        library
    }

    #[test]
    fn typing_goes_to_the_field_that_was_clicked() {
        let library = with_field();
        let mut stage = Stage::new(None, &library);
        // Nothing has the typing yet, so the key is free for the rules.
        assert!(!stage.key(&Key::Char('a'), &library));
        assert_eq!(stage.text("teamName"), None);

        click(&mut stage, &library, 110.0, 110.0);
        assert_eq!(stage.focus.as_ref().map(|focus| focus.symbol), Some(FIELD));
        for c in "abcdefg".chars() {
            assert!(stage.key(&Key::Char(c), &library));
        }
        // The field holds five letters at most.
        assert_eq!(stage.text("teamName"), Some("abcde"));
        stage.key(&Key::Backspace, &library);
        assert_eq!(stage.text("teamName"), Some("abcd"));

        // A click anywhere else ends the typing.
        click(&mut stage, &library, 10.0, 10.0);
        assert_eq!(stage.focus, None);
        assert!(!stage.key(&Key::Char('z'), &library));
        assert_eq!(stage.text("teamName"), Some("abcd"));
    }

    #[test]
    fn a_field_that_only_shows_text_cannot_be_typed_in() {
        let mut library = with_field();
        library.edit_texts.get_mut(&FIELD).unwrap().flags = vec!["read_only".to_owned()];
        let mut stage = Stage::new(None, &library);
        click(&mut stage, &library, 110.0, 110.0);
        assert_eq!(stage.focus, None);
    }

    #[test]
    fn the_field_being_typed_in_shows_a_caret_that_blinks() {
        let library = with_field();
        let mut stage = Stage::new(None, &library);
        click(&mut stage, &library, 110.0, 110.0);
        stage.key(&Key::Char('a'), &library);
        let said = |stage: &Stage| match &stage.commands(Matrix::IDENTITY, &library)[0] {
            Command::Draw { text, .. } => text.clone(),
            other => panic!("expected a draw, found {other:?}"),
        };
        assert_eq!(said(&stage), Some(format!("a{CARET}")));
        for _ in 0..BLINK {
            stage.advance(&library, &mut Empty);
        }
        assert_eq!(said(&stage), Some("a".to_owned()));
        // The caret is only drawn: it is no part of what the field says.
        assert_eq!(stage.text("teamName"), Some("a"));
    }

    #[test]
    fn enter_ends_the_typing() {
        let library = with_field();
        let mut stage = Stage::new(None, &library);
        click(&mut stage, &library, 110.0, 110.0);
        assert!(stage.key(&Key::Enter, &library));
        assert_eq!(stage.focus, None);
    }

    #[test]
    fn a_point_is_carried_into_and_out_of_a_nested_clip() {
        // A clip at (100, 50), twice the size, holding a shape at (10, 10).
        let outer = Place {
            matrix: Some([2.0, 0.0, 0.0, 2.0, 100.0, 50.0]),
            ..place(3, PlaceAction::Place(INNER))
        };
        let inner = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 10.0, 10.0]),
            ..place(1, PlaceAction::Place(SHAPE))
        };
        let library = library_with(
            vec![frame(vec![Op::Place(Box::new(outer))])],
            vec![frame(vec![Op::Place(Box::new(inner))])],
        );
        let stage = Stage::new(None, &library);
        assert_eq!(stage.to_stage(&[]), Some(Matrix::IDENTITY));
        assert_eq!(stage.to_stage(&[3]).unwrap().apply(0.0, 0.0), (100.0, 50.0));
        assert_eq!(
            stage.to_stage(&[3, 1]).unwrap().apply(0.0, 0.0),
            (120.0, 70.0)
        );
        assert_eq!(stage.from_stage(&[3], 120.0, 70.0), Some((10.0, 10.0)));
        assert_eq!(stage.to_stage(&[9]), None);
    }

    #[test]
    fn the_rules_can_add_an_object_and_take_it_away() {
        let library = library_with(vec![frame(vec![put(1, SHAPE)])], vec![frame(vec![])]);
        let mut stage = Stage::new(None, &library);
        let path = stage
            .attach(&[], INNER, Stage::RULES_DEPTH, "extra", &library)
            .unwrap();
        assert_eq!(path, [Stage::RULES_DEPTH]);
        assert_eq!(stage.find_named(&[], "extra"), Some(path.clone()));
        assert!(stage.remove(&path));
        assert!(!stage.remove(&path));
        assert_eq!(stage.find_named(&[], "extra"), None);
        // There is no such clip to add to.
        assert_eq!(stage.attach(&[7], INNER, 1, "lost", &library), None);
    }
}
