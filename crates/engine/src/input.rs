//! The pointer, and the buttons it rolls over and presses.

use bb_format::SymbolId;

use crate::display::{ButtonEvent, ButtonMode, Children, ClipState, Content, Event, Path};
use crate::library::Library;

/// Answers whether a point lies inside a drawn symbol. The renderer does this
/// with the triangles it draws.
pub trait Geometry {
    /// `x` and `y` are in the symbol's own coordinates.
    fn contains(&mut self, library: &Library, symbol: SymbolId, ratio: u16, x: f32, y: f32)
    -> bool;
}

/// A key the player has pressed, as far as a game needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Something typed: a letter, a digit, a space, punctuation.
    Char(char),
    Backspace,
    Enter,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
}

/// One button in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    path: Path,
    symbol: SymbolId,
}

/// Where the pointer is and which button it is dealing with.
#[derive(Debug)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub down: bool,
    /// The button the pointer is resting on.
    over: Option<Target>,
    /// The button a press began on. It keeps the press until release, even
    /// if the pointer wanders off it.
    pressed: Option<Target>,
    /// While pressing: whether the pointer is still on that button.
    inside: bool,
}

impl Default for Pointer {
    /// A pointer that has not been seen yet. It starts far from everything,
    /// so that nothing counts as hovered until the host says where it is.
    fn default() -> Pointer {
        Pointer {
            x: Pointer::NOWHERE,
            y: Pointer::NOWHERE,
            down: false,
            over: None,
            pressed: None,
            inside: false,
        }
    }
}

impl Pointer {
    /// A coordinate well outside any stage.
    pub const NOWHERE: f32 = -1.0e6;

    /// Whether the pointer is on a button, for showing a hand cursor.
    pub fn on_button(&self) -> bool {
        self.over.is_some() || (self.pressed.is_some() && self.inside)
    }

    /// Takes in the pointer's new position and button state, and works out
    /// what that does to the buttons in `root`. The position is in `root`'s
    /// coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        root: &mut ClipState,
        x: f32,
        y: f32,
        down: bool,
        library: &Library,
        geometry: &mut dyn Geometry,
        events: &mut Vec<Event>,
    ) {
        let was_down = self.down;
        (self.x, self.y, self.down) = (x, y, down);
        let hit = button_at(&root.children, x, y, library, geometry, &mut Path::new());
        let mut change = |target: &Target, event: ButtonEvent, mode: ButtonMode| {
            if let Some(child) = root.child_mut(&target.path)
                && let Content::Button(button) = &mut child.content
            {
                button.mode = mode;
            }
            events.push(Event::Button {
                symbol: target.symbol,
                path: target.path.clone(),
                event,
            });
            let sounds = library
                .buttons
                .get(&target.symbol)
                .and_then(|button| button.sounds.as_ref());
            let sound = sounds.and_then(|sounds| match event {
                ButtonEvent::RollOver => sounds.up_to_over.as_ref(),
                ButtonEvent::RollOut => sounds.over_to_up.as_ref(),
                ButtonEvent::Press => sounds.over_to_down.as_ref(),
                ButtonEvent::Release => sounds.down_to_over.as_ref(),
                _ => None,
            });
            events.extend(sound.cloned().map(Event::Sound));
        };

        if let Some(pressed) = self.pressed.clone() {
            let inside = hit.as_ref() == Some(&pressed);
            if down {
                if inside != self.inside {
                    self.inside = inside;
                    if inside {
                        change(&pressed, ButtonEvent::DragOver, ButtonMode::Down);
                    } else {
                        change(&pressed, ButtonEvent::DragOut, ButtonMode::Over);
                    }
                }
                return;
            }
            self.pressed = None;
            if inside {
                change(&pressed, ButtonEvent::Release, ButtonMode::Over);
                self.over = Some(pressed);
            } else {
                change(&pressed, ButtonEvent::ReleaseOutside, ButtonMode::Up);
                self.over = None;
            }
        }

        if self.over != hit {
            if let Some(old) = self.over.take() {
                change(&old, ButtonEvent::RollOut, ButtonMode::Up);
            }
            // A button does not light up for a pointer that arrives already
            // held down.
            if !down && let Some(new) = &hit {
                change(new, ButtonEvent::RollOver, ButtonMode::Over);
                self.over = hit;
            }
        }
        if down
            && !was_down
            && let Some(target) = self.over.clone()
        {
            change(&target, ButtonEvent::Press, ButtonMode::Down);
            self.pressed = Some(target);
            self.inside = true;
        }
    }
}

/// The topmost button whose hit area holds the point. `x` and `y` are in the
/// coordinates of whatever owns `children`.
fn button_at(
    children: &Children,
    x: f32,
    y: f32,
    library: &Library,
    geometry: &mut dyn Geometry,
    path: &mut Path,
) -> Option<Target> {
    for (&depth, child) in children.iter().rev() {
        // Masks and hidden objects take no part.
        if !child.visible || child.clip_depth.is_some() {
            continue;
        }
        let Some(inverse) = child.matrix.inverse() else {
            continue;
        };
        let (x, y) = inverse.apply(x, y);
        match &child.content {
            Content::Button(button) => {
                if area_contains(&button.hit, x, y, library, geometry) {
                    path.push(depth);
                    return Some(Target {
                        path: path.clone(),
                        symbol: child.symbol,
                    });
                }
            }
            Content::Clip(clip) => {
                path.push(depth);
                if let Some(found) = button_at(&clip.children, x, y, library, geometry, path) {
                    return Some(found);
                }
                path.pop();
            }
            // Plain artwork does not stop the pointer reaching what is under
            // it.
            Content::Graphic => {}
        }
    }
    None
}

/// The topmost text field that can be typed in and has the point inside its
/// box: where it is in the tree, and its symbol. `x` and `y` are in the
/// coordinates of whatever owns `children`.
pub fn field_at(
    children: &Children,
    x: f32,
    y: f32,
    library: &Library,
    path: &mut Path,
) -> Option<(Path, SymbolId)> {
    for (&depth, child) in children.iter().rev() {
        if !child.visible || child.clip_depth.is_some() {
            continue;
        }
        let Some(inverse) = child.matrix.inverse() else {
            continue;
        };
        let (x, y) = inverse.apply(x, y);
        path.push(depth);
        let found = match &child.content {
            Content::Graphic => library
                .edit_texts
                .get(&child.symbol)
                .filter(|field| {
                    let bounds = &field.bounds;
                    !field.flags.iter().any(|flag| flag == "read_only")
                        && (bounds.x_min as f32..=bounds.x_max as f32).contains(&x)
                        && (bounds.y_min as f32..=bounds.y_max as f32).contains(&y)
                })
                .map(|_| (path.clone(), child.symbol)),
            Content::Clip(clip) => field_at(&clip.children, x, y, library, path),
            // Nothing inside a button takes typing.
            Content::Button(_) => None,
        };
        path.pop();
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Whether any of `children` draws something over the point.
fn area_contains(
    children: &Children,
    x: f32,
    y: f32,
    library: &Library,
    geometry: &mut dyn Geometry,
) -> bool {
    children.values().any(|child| {
        let Some(inverse) = child.matrix.inverse() else {
            return false;
        };
        let (x, y) = inverse.apply(x, y);
        match &child.content {
            Content::Graphic => geometry.contains(library, child.symbol, child.ratio, x, y),
            Content::Clip(clip) => area_contains(&clip.children, x, y, library, geometry),
            Content::Button(button) => area_contains(&button.hit, x, y, library, geometry),
        }
    })
}

#[cfg(test)]
mod tests {
    use bb_format::{
        Button, ButtonRecord, ButtonSounds, Op, PlaceAction, SoundEvent, SoundStart, SymbolInfo,
    };

    use super::*;
    use crate::display::tests::{OTHER_SHAPE, SHAPE, frame, library_with, place, symbol};

    const BUTTON: SymbolId = 20;
    const CLICK: SymbolId = 30;

    /// Treats every symbol as the square from (0, 0) to (10, 10).
    struct Squares;

    impl Geometry for Squares {
        fn contains(&mut self, _: &Library, _: SymbolId, _: u16, x: f32, y: f32) -> bool {
            (0.0..=10.0).contains(&x) && (0.0..=10.0).contains(&y)
        }
    }

    fn click_sound() -> SoundStart {
        SoundStart {
            sound: CLICK,
            event: SoundEvent::Event,
            loops: 1,
            in_sample: None,
            out_sample: None,
            envelope: Vec::new(),
        }
    }

    /// A main timeline with one button at (20, 20), 10 by 10.
    fn scene() -> (Library, ClipState) {
        let at = bb_format::Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 20.0, 20.0]),
            ..place(1, PlaceAction::Place(BUTTON))
        };
        let mut library = library_with(vec![frame(vec![Op::Place(Box::new(at))])], vec![]);
        let record = |states: &[&str], symbol| ButtonRecord {
            states: states.iter().map(|s| (*s).to_owned()).collect(),
            symbol,
            depth: 1,
            matrix: bb_format::IDENTITY,
            color: None,
            filters: Vec::new(),
        };
        library.buttons.insert(
            BUTTON,
            Button {
                id: BUTTON,
                track_as_menu: false,
                records: vec![
                    record(&["up", "hit"], SHAPE),
                    record(&["over", "down"], OTHER_SHAPE),
                ],
                actions: Vec::new(),
                sounds: Some(ButtonSounds {
                    over_to_down: Some(click_sound()),
                    ..ButtonSounds::default()
                }),
            },
        );
        library
            .manifest
            .symbols
            .insert(BUTTON, symbol("buttons/20.json", SymbolInfo::Button));
        let root = ClipState::new(None, &library, &mut Vec::new(), &mut Path::new());
        (library, root)
    }

    /// Moves the pointer and returns what happened to buttons, leaving out
    /// sounds.
    fn act(
        pointer: &mut Pointer,
        root: &mut ClipState,
        library: &Library,
        x: f32,
        down: bool,
    ) -> Vec<ButtonEvent> {
        let mut events = Vec::new();
        pointer.update(root, x, 25.0, down, library, &mut Squares, &mut events);
        events
            .into_iter()
            .filter_map(|event| match event {
                Event::Button { event, .. } => Some(event),
                Event::Sound(_) | Event::Frame { .. } => None,
            })
            .collect()
    }

    fn mode(root: &ClipState) -> ButtonMode {
        match &root.children[&1].content {
            Content::Button(button) => button.mode,
            other => panic!("expected a button, found {other:?}"),
        }
    }

    #[test]
    fn a_pointer_nobody_has_moved_hovers_over_nothing() {
        // The button below is moved to the corner of the stage, where a
        // pointer resting at (0, 0) would be on top of it.
        let (library, mut root) = scene();
        root.children.get_mut(&1).unwrap().matrix = crate::math::Matrix::IDENTITY;
        let mut pointer = Pointer::default();
        let mut events = Vec::new();
        let (x, y, down) = (pointer.x, pointer.y, pointer.down);
        pointer.update(&mut root, x, y, down, &library, &mut Squares, &mut events);
        assert!(events.is_empty());
        assert_eq!(mode(&root), ButtonMode::Up);
    }

    #[test]
    fn rolling_over_pressing_and_releasing_a_button() {
        let (library, mut root) = scene();
        let mut pointer = Pointer::default();

        assert!(act(&mut pointer, &mut root, &library, 5.0, false).is_empty());
        assert_eq!(mode(&root), ButtonMode::Up);

        assert_eq!(
            act(&mut pointer, &mut root, &library, 25.0, false),
            [ButtonEvent::RollOver]
        );
        assert_eq!(mode(&root), ButtonMode::Over);
        assert!(pointer.on_button());

        assert_eq!(
            act(&mut pointer, &mut root, &library, 25.0, true),
            [ButtonEvent::Press]
        );
        assert_eq!(mode(&root), ButtonMode::Down);

        assert_eq!(
            act(&mut pointer, &mut root, &library, 25.0, false),
            [ButtonEvent::Release]
        );
        assert_eq!(mode(&root), ButtonMode::Over);

        assert_eq!(
            act(&mut pointer, &mut root, &library, 5.0, false),
            [ButtonEvent::RollOut]
        );
        assert_eq!(mode(&root), ButtonMode::Up);
        assert!(!pointer.on_button());
    }

    #[test]
    fn dragging_off_a_pressed_button_and_letting_go_outside() {
        let (library, mut root) = scene();
        let mut pointer = Pointer::default();
        act(&mut pointer, &mut root, &library, 25.0, false);
        act(&mut pointer, &mut root, &library, 25.0, true);

        assert_eq!(
            act(&mut pointer, &mut root, &library, 5.0, true),
            [ButtonEvent::DragOut]
        );
        assert_eq!(
            act(&mut pointer, &mut root, &library, 25.0, true),
            [ButtonEvent::DragOver]
        );
        assert_eq!(mode(&root), ButtonMode::Down);

        act(&mut pointer, &mut root, &library, 5.0, true);
        assert_eq!(
            act(&mut pointer, &mut root, &library, 5.0, false),
            [ButtonEvent::ReleaseOutside]
        );
        assert_eq!(mode(&root), ButtonMode::Up);
    }

    #[test]
    fn a_pointer_that_arrives_held_down_does_not_press() {
        let (library, mut root) = scene();
        let mut pointer = Pointer::default();
        act(&mut pointer, &mut root, &library, 5.0, true);
        assert!(act(&mut pointer, &mut root, &library, 25.0, true).is_empty());
        assert_eq!(mode(&root), ButtonMode::Up);
        // Letting go over the button is a roll over, not a release.
        assert_eq!(
            act(&mut pointer, &mut root, &library, 25.0, false),
            [ButtonEvent::RollOver]
        );
    }

    #[test]
    fn a_press_plays_the_buttons_sound_and_names_the_button() {
        let (library, mut root) = scene();
        let mut pointer = Pointer::default();
        act(&mut pointer, &mut root, &library, 25.0, false);

        let mut events = Vec::new();
        pointer.update(
            &mut root,
            25.0,
            25.0,
            true,
            &library,
            &mut Squares,
            &mut events,
        );
        assert_eq!(
            events,
            [
                Event::Button {
                    symbol: BUTTON,
                    path: vec![1],
                    event: ButtonEvent::Press,
                },
                Event::Sound(click_sound()),
            ]
        );
    }
}
