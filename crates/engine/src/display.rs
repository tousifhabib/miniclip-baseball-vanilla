//! The display tree: clips nested inside clips, each playing its own
//! timeline.
//!
//! This follows Flash's rules. A timeline stores changes per frame, so moving
//! the playhead forward applies each frame's changes in turn, and moving it
//! back replays from frame 1. An object that the replay puts back where it
//! already was is kept, not rebuilt, so a nested clip keeps its own position.

use std::collections::BTreeMap;

use bb_format::{Op, Place, PlaceAction, SymbolId, SymbolInfo};

use crate::library::Library;
use crate::math::{ColorTransform, Matrix};

/// Children by depth. Higher depths draw on top.
pub type Children = BTreeMap<u16, Child>;

/// One object on a display list.
#[derive(Clone, Debug)]
pub struct Child {
    pub symbol: SymbolId,
    pub matrix: Matrix,
    pub color: ColorTransform,
    /// Morph position, 0 to 65535.
    pub ratio: u16,
    /// When set, this object is a mask for the depths above it up to here.
    pub clip_depth: Option<u16>,
    pub name: Option<String>,
    pub visible: bool,
    /// The frame of the parent timeline that put this object here.
    pub placed_on: u16,
    pub content: Content,
}

#[derive(Clone, Debug)]
pub enum Content {
    /// Drawn as it is: a shape, a morph shape or text.
    Graphic,
    Clip(ClipState),
    /// A button, showing the objects of its "up" state.
    Button(Children),
}

/// A playing instance of a timeline.
#[derive(Clone, Debug)]
pub struct ClipState {
    /// `None` for the main timeline.
    pub symbol: Option<SymbolId>,
    /// The current frame, counting from 1.
    pub frame: u16,
    pub playing: bool,
    pub children: Children,
}

impl ClipState {
    /// Creates an instance showing its first frame.
    pub fn new(symbol: Option<SymbolId>, library: &Library) -> ClipState {
        let mut clip = ClipState {
            symbol,
            frame: 0,
            playing: true,
            children: Children::new(),
        };
        clip.goto(1, library);
        clip
    }

    pub fn frame_count(&self, library: &Library) -> u16 {
        library
            .timeline(self.symbol)
            .map_or(0, |timeline| timeline.frames.len() as u16)
    }

    /// Moves everything on by one frame.
    pub fn advance(&mut self, library: &Library) {
        // Objects already here move on before this timeline does, and objects
        // this timeline adds now stay on their first frame until the next
        // tick. That is the order Flash uses.
        advance_children(&mut self.children, library);
        if !self.playing {
            return;
        }
        let count = self.frame_count(library);
        if count <= 1 {
            return;
        }
        let next = if self.frame >= count {
            1
        } else {
            self.frame + 1
        };
        self.goto(next, library);
    }

    /// Moves the playhead to `frame`, clamped to the timeline's length.
    pub fn goto(&mut self, frame: u16, library: &Library) {
        let count = self.frame_count(library);
        if count == 0 {
            return;
        }
        let target = frame.clamp(1, count);
        if target == self.frame {
            return;
        }
        if target > self.frame {
            for frame in self.frame + 1..=target {
                self.run_frame(frame, library);
            }
        } else {
            self.rewind_to(target, library);
        }
        self.frame = target;
    }

    /// Applies one frame's changes to the display list.
    fn run_frame(&mut self, frame: u16, library: &Library) {
        let Some(timeline) = library.timeline(self.symbol) else {
            return;
        };
        let Some(data) = timeline.frames.get(usize::from(frame) - 1) else {
            return;
        };
        for op in &data.ops {
            apply(&mut self.children, op, frame, library);
        }
    }

    /// Rebuilds the display list as it stands on `target`, an earlier frame.
    fn rewind_to(&mut self, target: u16, library: &Library) {
        let old = std::mem::take(&mut self.children);
        for frame in 1..=target {
            self.run_frame(frame, library);
        }
        // Keep the old object wherever the replay made the same one, so that
        // it carries on from where it was instead of starting over.
        for (depth, child) in &mut self.children {
            if let Some(previous) = old.get(depth)
                && previous.symbol == child.symbol
                && previous.placed_on == child.placed_on
            {
                child.content = previous.content.clone();
            }
        }
    }
}

fn advance_children(children: &mut Children, library: &Library) {
    for child in children.values_mut() {
        match &mut child.content {
            Content::Clip(clip) => clip.advance(library),
            Content::Button(children) => advance_children(children, library),
            Content::Graphic => {}
        }
    }
}

fn apply(children: &mut Children, op: &Op, frame: u16, library: &Library) {
    match op {
        Op::Remove { depth } => {
            children.remove(depth);
        }
        Op::Place(place) => match place.action {
            PlaceAction::Place(symbol) => {
                // Flash ignores a placement at a depth that is already taken.
                if !children.contains_key(&place.depth)
                    && let Some(mut child) = new_child(symbol, frame, library)
                {
                    update(&mut child, place);
                    children.insert(place.depth, child);
                }
            }
            PlaceAction::Modify => {
                if let Some(child) = children.get_mut(&place.depth) {
                    update(child, place);
                }
            }
            PlaceAction::Replace(symbol) => {
                let Some(mut child) = new_child(symbol, frame, library) else {
                    return;
                };
                // The new object takes over the old one's settings, apart
                // from the ones this placement gives.
                if let Some(old) = children.remove(&place.depth) {
                    child.matrix = old.matrix;
                    child.color = old.color;
                    child.ratio = old.ratio;
                    child.clip_depth = old.clip_depth;
                    child.name = old.name;
                    child.visible = old.visible;
                }
                update(&mut child, place);
                children.insert(place.depth, child);
            }
        },
    }
}

/// Applies the settings a placement names, leaving the rest alone.
fn update(child: &mut Child, place: &Place) {
    if let Some(matrix) = place.matrix {
        child.matrix = matrix.into();
    }
    if let Some(color) = place.color {
        child.color = color.into();
    }
    if let Some(ratio) = place.ratio {
        child.ratio = ratio;
    }
    if let Some(name) = &place.name {
        child.name = Some(name.clone());
    }
    if let Some(clip_depth) = place.clip_depth {
        child.clip_depth = Some(clip_depth);
    }
    if let Some(visible) = place.visible {
        child.visible = visible;
    }
}

/// A fresh instance of `symbol`, or `None` if there is nothing to show for it.
fn new_child(symbol: SymbolId, placed_on: u16, library: &Library) -> Option<Child> {
    let content = match &library.manifest.symbols.get(&symbol)?.info {
        SymbolInfo::Clip { .. } => Content::Clip(ClipState::new(Some(symbol), library)),
        SymbolInfo::Button => {
            let button = library.buttons.get(&symbol)?;
            let mut children = Children::new();
            for record in &button.records {
                if !record.states.iter().any(|state| state == "up") {
                    continue;
                }
                if let Some(mut child) = new_child(record.symbol, 0, library) {
                    child.matrix = record.matrix.into();
                    if let Some(color) = record.color {
                        child.color = color.into();
                    }
                    children.insert(record.depth, child);
                }
            }
            Content::Button(children)
        }
        SymbolInfo::Shape { .. }
        | SymbolInfo::MorphShape
        | SymbolInfo::Text
        | SymbolInfo::EditText => Content::Graphic,
        SymbolInfo::Bitmap { .. } | SymbolInfo::Sound { .. } | SymbolInfo::Font { .. } => {
            return None;
        }
    };
    Some(Child {
        symbol,
        matrix: Matrix::IDENTITY,
        color: ColorTransform::IDENTITY,
        ratio: 0,
        clip_depth: None,
        name: None,
        visible: true,
        placed_on,
        content,
    })
}

/// One step of drawing a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Draw {
        symbol: SymbolId,
        ratio: u16,
        matrix: Matrix,
        color: ColorTransform,
    },
    /// The draws up to `ActivateMask` are the outline of a mask.
    PushMask,
    /// From here on, only pixels inside the mask are drawn.
    ActivateMask,
    /// The draws up to `PopMask` repeat the mask's outline, to remove it.
    DeactivateMask,
    PopMask,
}

/// Lists what to draw for a clip, back to front.
pub fn commands(clip: &ClipState, base: Matrix) -> Vec<Command> {
    let mut out = Vec::new();
    draw_children(
        &clip.children,
        base,
        ColorTransform::IDENTITY,
        false,
        &mut out,
    );
    out
}

fn draw_children(
    children: &Children,
    matrix: Matrix,
    color: ColorTransform,
    in_mask: bool,
    out: &mut Vec<Command>,
) {
    // Masks that are in force, innermost last, with the depth each ends at.
    let mut masks: Vec<(u16, &Child)> = Vec::new();
    let end_mask = |mask: &Child, out: &mut Vec<Command>| {
        out.push(Command::DeactivateMask);
        draw_child(mask, matrix, color, true, out);
        out.push(Command::PopMask);
    };

    for (&depth, child) in children {
        while let Some(&(last_depth, mask)) = masks.last()
            && last_depth < depth
        {
            masks.pop();
            end_mask(mask, out);
        }
        match child.clip_depth {
            // Inside a mask's outline everything is plain geometry.
            Some(clip_depth) if !in_mask => {
                out.push(Command::PushMask);
                draw_child(child, matrix, color, true, out);
                out.push(Command::ActivateMask);
                masks.push((clip_depth, child));
            }
            Some(_) => {}
            None if child.visible => draw_child(child, matrix, color, in_mask, out),
            None => {}
        }
    }
    while let Some((_, mask)) = masks.pop() {
        end_mask(mask, out);
    }
}

fn draw_child(
    child: &Child,
    matrix: Matrix,
    color: ColorTransform,
    in_mask: bool,
    out: &mut Vec<Command>,
) {
    let matrix = matrix.then_inner(child.matrix);
    let color = color.then_inner(child.color);
    match &child.content {
        Content::Graphic => out.push(Command::Draw {
            symbol: child.symbol,
            ratio: child.ratio,
            matrix,
            color,
        }),
        Content::Clip(clip) => draw_children(&clip.children, matrix, color, in_mask, out),
        Content::Button(children) => draw_children(children, matrix, color, in_mask, out),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    use bb_format::{Clip, Frame, Manifest, Rect, Stage, Symbol};

    use super::*;

    const SHAPE: SymbolId = 1;
    const OTHER_SHAPE: SymbolId = 2;
    const INNER: SymbolId = 10;

    fn place(depth: u16, action: PlaceAction) -> Place {
        Place {
            depth,
            action,
            matrix: None,
            color: None,
            ratio: None,
            name: None,
            clip_depth: None,
            filters: None,
            blend_mode: None,
            visible: None,
            clip_events: Vec::new(),
        }
    }

    fn put(depth: u16, symbol: SymbolId) -> Op {
        Op::Place(Box::new(place(depth, PlaceAction::Place(symbol))))
    }

    fn frame(ops: Vec<Op>) -> Frame {
        Frame {
            ops,
            ..Frame::default()
        }
    }

    fn clip(id: Option<SymbolId>, frames: Vec<Frame>) -> Clip {
        Clip {
            id,
            labels: BTreeMap::new(),
            frames,
        }
    }

    /// A library with two shapes, the given main timeline, and one inner clip
    /// of `inner_frames` empty frames.
    fn library(root: Vec<Frame>, inner_frames: usize) -> Library {
        let shape = |file: &str| Symbol {
            file: file.to_owned(),
            export_name: None,
            info: SymbolInfo::Shape {
                bounds: Rect {
                    x_min: 0.0,
                    y_min: 0.0,
                    x_max: 1.0,
                    y_max: 1.0,
                },
            },
        };
        let mut symbols = BTreeMap::new();
        symbols.insert(SHAPE, shape("shapes/1.svg"));
        symbols.insert(OTHER_SHAPE, shape("shapes/2.svg"));
        symbols.insert(
            INNER,
            Symbol {
                file: "clips/10.json".to_owned(),
                export_name: None,
                info: SymbolInfo::Clip {
                    frame_count: inner_frames as u16,
                },
            },
        );
        let inner = clip(Some(INNER), vec![Frame::default(); inner_frames]);
        Library {
            dir: PathBuf::new(),
            manifest: Manifest {
                format_version: bb_format::FORMAT_VERSION,
                swf_version: 8,
                stage: Stage {
                    width: 100.0,
                    height: 100.0,
                    frame_rate: 60.0,
                    frame_count: root.len() as u16,
                    background: None,
                },
                symbols,
                exports: BTreeMap::new(),
            },
            root: clip(None, root),
            clips: HashMap::from([(INNER, inner)]),
            buttons: HashMap::new(),
            texts: HashMap::new(),
            edit_texts: HashMap::new(),
            fonts: HashMap::new(),
            morphs: HashMap::new(),
        }
    }

    fn inner_frame(clip: &ClipState, depth: u16) -> u16 {
        match &clip.children[&depth].content {
            Content::Clip(inner) => inner.frame,
            other => panic!("expected a clip, found {other:?}"),
        }
    }

    #[test]
    fn a_new_clip_shows_its_first_frame() {
        let library = library(vec![frame(vec![put(1, SHAPE)]), frame(vec![])], 1);
        let root = ClipState::new(None, &library);
        assert_eq!(root.frame, 1);
        assert_eq!(root.children[&1].symbol, SHAPE);
    }

    #[test]
    fn frames_apply_their_changes_in_turn() {
        let moved = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]),
            ..place(1, PlaceAction::Modify)
        };
        let library = library(
            vec![
                frame(vec![put(1, SHAPE)]),
                frame(vec![Op::Place(Box::new(moved))]),
                frame(vec![Op::Remove { depth: 1 }]),
            ],
            1,
        );
        let mut root = ClipState::new(None, &library);
        root.advance(&library);
        assert_eq!(root.children[&1].matrix, Matrix::translate(5.0, 7.0));
        root.advance(&library);
        assert!(root.children.is_empty());
    }

    #[test]
    fn replacing_keeps_the_old_position() {
        let moved = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]),
            ..place(1, PlaceAction::Place(SHAPE))
        };
        let swap = place(1, PlaceAction::Replace(OTHER_SHAPE));
        let library = library(
            vec![
                frame(vec![Op::Place(Box::new(moved))]),
                frame(vec![Op::Place(Box::new(swap))]),
            ],
            1,
        );
        let mut root = ClipState::new(None, &library);
        root.advance(&library);
        assert_eq!(root.children[&1].symbol, OTHER_SHAPE);
        assert_eq!(root.children[&1].matrix, Matrix::translate(5.0, 7.0));
    }

    #[test]
    fn a_nested_clip_plays_on_its_own_but_not_on_the_tick_it_appears() {
        let library = library(
            vec![frame(vec![]), frame(vec![put(1, INNER)]), frame(vec![])],
            5,
        );
        let mut root = ClipState::new(None, &library);
        root.advance(&library);
        assert_eq!(inner_frame(&root, 1), 1);
        root.advance(&library);
        assert_eq!(inner_frame(&root, 1), 2);
    }

    #[test]
    fn looping_keeps_objects_from_the_first_frame_and_drops_later_ones() {
        let library = library(
            vec![
                frame(vec![put(1, INNER)]),
                frame(vec![put(2, SHAPE)]),
                frame(vec![]),
            ],
            10,
        );
        let mut root = ClipState::new(None, &library);
        root.advance(&library);
        root.advance(&library);
        assert_eq!(root.frame, 3);
        assert!(root.children.contains_key(&2));

        root.advance(&library);
        assert_eq!(root.frame, 1);
        assert!(!root.children.contains_key(&2));
        // The inner clip was not rebuilt: it has kept counting.
        assert_eq!(inner_frame(&root, 1), 4);
    }

    #[test]
    fn a_stopped_clip_stays_put_while_its_children_play() {
        let library = library(vec![frame(vec![put(1, INNER)]), frame(vec![])], 5);
        let mut root = ClipState::new(None, &library);
        root.playing = false;
        root.advance(&library);
        assert_eq!(root.frame, 1);
        assert_eq!(inner_frame(&root, 1), 2);
    }

    #[test]
    fn going_back_rebuilds_the_list_for_that_frame() {
        let library = library(
            vec![
                frame(vec![put(1, SHAPE)]),
                frame(vec![Op::Remove { depth: 1 }, put(2, OTHER_SHAPE)]),
            ],
            1,
        );
        let mut root = ClipState::new(None, &library);
        root.goto(2, &library);
        assert_eq!(root.children.keys().copied().collect::<Vec<_>>(), [2]);
        root.goto(1, &library);
        assert_eq!(root.children.keys().copied().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_mask_wraps_the_depths_it_covers() {
        let mask = Place {
            clip_depth: Some(2),
            ..place(1, PlaceAction::Place(SHAPE))
        };
        let library = library(
            vec![frame(vec![
                Op::Place(Box::new(mask)),
                put(2, OTHER_SHAPE),
                put(3, OTHER_SHAPE),
            ])],
            1,
        );
        let root = ClipState::new(None, &library);
        let kinds: Vec<String> = commands(&root, Matrix::IDENTITY)
            .iter()
            .map(|command| match command {
                Command::Draw { symbol, .. } => format!("draw {symbol}"),
                other => format!("{other:?}"),
            })
            .collect();
        // Symbol 1 is the mask. Symbol 2 sits at depth 2, inside the mask, and
        // again at depth 3, past its end.
        assert_eq!(
            kinds,
            [
                "PushMask",
                "draw 1",
                "ActivateMask",
                "draw 2",
                "DeactivateMask",
                "draw 1",
                "PopMask",
                "draw 2",
            ]
        );
    }

    #[test]
    fn nested_transforms_multiply_outermost_first() {
        let moved = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 3.0, 4.0]),
            ..place(1, PlaceAction::Place(SHAPE))
        };
        let library = library(vec![frame(vec![Op::Place(Box::new(moved))])], 1);
        let root = ClipState::new(None, &library);
        let commands = commands(&root, Matrix::scale(2.0, 2.0));
        let Command::Draw { matrix, .. } = &commands[0] else {
            panic!("expected a draw");
        };
        assert_eq!(matrix.apply(0.0, 0.0), (6.0, 8.0));
    }
}
