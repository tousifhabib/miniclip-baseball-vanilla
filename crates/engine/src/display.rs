//! The display tree: clips nested inside clips, each playing its own
//! timeline.
//!
//! This follows Flash's rules. A timeline stores changes per frame, so moving
//! the playhead forward applies each frame's changes in turn, and moving it
//! back replays from frame 1. An object that the replay puts back where it
//! already was is kept, not rebuilt, so a nested clip keeps its own position.

use std::collections::BTreeMap;

use bb_format::{Filter, Op, Place, PlaceAction, SoundStart, SymbolId, SymbolInfo};

use crate::library::Library;
use crate::math::{ColorTransform, Matrix};

/// Children by depth. Higher depths draw on top.
pub type Children = BTreeMap<u16, Child>;

/// Where an object is in the tree: the depth to follow at each level, from
/// the top timeline down.
pub type Path = Vec<u16>;

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
    /// Applied to this object and everything inside it, drawn as one picture.
    pub filters: Vec<Filter>,
    /// The frame of the parent timeline that put this object here.
    pub placed_on: u16,
    pub content: Content,
}

#[derive(Clone, Debug)]
pub enum Content {
    /// Drawn as it is: a shape, a morph shape or text.
    Graphic,
    Clip(ClipState),
    Button(ButtonState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonMode {
    Up,
    Over,
    Down,
}

/// A button: one set of objects per look, and an area that reacts to the
/// pointer.
#[derive(Clone, Debug)]
pub struct ButtonState {
    pub mode: ButtonMode,
    pub up: Children,
    pub over: Children,
    pub down: Children,
    /// Never drawn.
    pub hit: Children,
}

impl ButtonState {
    /// The objects for the look the button has now.
    pub fn shown(&self) -> &Children {
        match self.mode {
            ButtonMode::Up => &self.up,
            ButtonMode::Over => &self.over,
            ButtonMode::Down => &self.down,
        }
    }

    fn shown_mut(&mut self) -> &mut Children {
        match self.mode {
            ButtonMode::Up => &mut self.up,
            ButtonMode::Over => &mut self.over,
            ButtonMode::Down => &mut self.down,
        }
    }
}

/// What the pointer did to a button, named after the ActionScript events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEvent {
    RollOver,
    RollOut,
    Press,
    Release,
    ReleaseOutside,
    DragOut,
    DragOver,
}

/// Something that happened while the tree played, for the caller to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A timeline or a button asked for a sound.
    Sound(SoundStart),
    Button {
        symbol: SymbolId,
        path: Path,
        event: ButtonEvent,
    },
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
    pub fn new(symbol: Option<SymbolId>, library: &Library, events: &mut Vec<Event>) -> ClipState {
        let mut clip = ClipState {
            symbol,
            frame: 0,
            playing: true,
            children: Children::new(),
        };
        clip.goto(1, library, events);
        clip
    }

    pub fn frame_count(&self, library: &Library) -> u16 {
        library
            .timeline(self.symbol)
            .map_or(0, |timeline| timeline.frames.len() as u16)
    }

    /// Moves everything on by one frame.
    pub fn advance(&mut self, library: &Library, events: &mut Vec<Event>) {
        // Objects already here move on before this timeline does, and objects
        // this timeline adds now stay on their first frame until the next
        // tick. That is the order Flash uses.
        advance_children(&mut self.children, library, events);
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
        self.goto(next, library, events);
    }

    /// Moves the playhead to `frame`, clamped to the timeline's length.
    pub fn goto(&mut self, frame: u16, library: &Library, events: &mut Vec<Event>) {
        let Some(timeline) = library.timeline(self.symbol) else {
            return;
        };
        let count = timeline.frames.len() as u16;
        if count == 0 {
            return;
        }
        let target = frame.clamp(1, count);
        if target == self.frame {
            return;
        }
        if target > self.frame {
            for frame in self.frame + 1..=target {
                for op in &timeline.frames[usize::from(frame) - 1].ops {
                    apply(&mut self.children, op, frame, library, events);
                }
            }
        } else {
            self.rewind_to(target, library, events);
        }
        self.frame = target;
        // Only the frame the playhead lands on is heard, not the ones it
        // passed over on the way.
        let landed = &timeline.frames[usize::from(target) - 1];
        events.extend(landed.sounds.iter().cloned().map(Event::Sound));
        if landed.stops {
            self.playing = false;
        }
    }

    /// Rebuilds the display list as it stands on `target`, an earlier frame.
    fn rewind_to(&mut self, target: u16, library: &Library, events: &mut Vec<Event>) {
        let Some(timeline) = library.timeline(self.symbol) else {
            return;
        };
        let old = std::mem::take(&mut self.children);
        // What the objects built by the replay asked for, by depth. Held back
        // until it is known which of them are really new.
        let mut pending: BTreeMap<u16, Vec<Event>> = BTreeMap::new();
        for frame in 1..=target {
            for op in &timeline.frames[usize::from(frame) - 1].ops {
                let mut made = Vec::new();
                apply(&mut self.children, op, frame, library, &mut made);
                match op {
                    Op::Remove { depth } => {
                        pending.remove(depth);
                    }
                    Op::Place(place) if place.action != PlaceAction::Modify => {
                        pending.insert(place.depth, made);
                    }
                    Op::Place(_) => {}
                }
            }
        }
        // Keep the old object wherever the replay made the same one, so that
        // it carries on from where it was instead of starting over.
        for (depth, child) in &mut self.children {
            if let Some(previous) = old.get(depth)
                && previous.symbol == child.symbol
                && previous.placed_on == child.placed_on
            {
                child.content = previous.content.clone();
                pending.remove(depth);
            }
        }
        events.extend(pending.into_values().flatten());
    }

    /// The object at `path`, counted from this clip.
    pub fn child_mut(&mut self, path: &[u16]) -> Option<&mut Child> {
        let (first, rest) = path.split_first()?;
        let child = self.children.get_mut(first)?;
        if rest.is_empty() {
            return Some(child);
        }
        match &mut child.content {
            Content::Clip(clip) => clip.child_mut(rest),
            _ => None,
        }
    }
}

fn advance_children(children: &mut Children, library: &Library, events: &mut Vec<Event>) {
    for child in children.values_mut() {
        match &mut child.content {
            Content::Clip(clip) => clip.advance(library, events),
            Content::Button(button) => advance_children(button.shown_mut(), library, events),
            Content::Graphic => {}
        }
    }
}

fn apply(children: &mut Children, op: &Op, frame: u16, library: &Library, events: &mut Vec<Event>) {
    match op {
        Op::Remove { depth } => {
            children.remove(depth);
        }
        Op::Place(place) => match place.action {
            PlaceAction::Place(symbol) => {
                // Flash ignores a placement at a depth that is already taken.
                if !children.contains_key(&place.depth)
                    && let Some(mut child) = new_child(symbol, frame, library, events)
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
                let Some(mut child) = new_child(symbol, frame, library, events) else {
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
                    child.filters = old.filters;
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
    if let Some(filters) = &place.filters {
        child.filters = filters.clone();
    }
}

/// A fresh instance of `symbol`, or `None` if there is nothing to show for it.
fn new_child(
    symbol: SymbolId,
    placed_on: u16,
    library: &Library,
    events: &mut Vec<Event>,
) -> Option<Child> {
    let content = match &library.manifest.symbols.get(&symbol)?.info {
        SymbolInfo::Clip { .. } => Content::Clip(ClipState::new(Some(symbol), library, events)),
        SymbolInfo::Button => {
            let button = library.buttons.get(&symbol)?;
            let mut state = ButtonState {
                mode: ButtonMode::Up,
                up: Children::new(),
                over: Children::new(),
                down: Children::new(),
                hit: Children::new(),
            };
            for record in &button.records {
                for name in &record.states {
                    let children = match name.as_str() {
                        "up" => &mut state.up,
                        "over" => &mut state.over,
                        "down" => &mut state.down,
                        "hit" => &mut state.hit,
                        _ => continue,
                    };
                    if let Some(mut child) = new_child(record.symbol, 0, library, events) {
                        child.matrix = record.matrix.into();
                        if let Some(color) = record.color {
                            child.color = color.into();
                        }
                        child.filters = record.filters.clone();
                        children.insert(record.depth, child);
                    }
                }
            }
            Content::Button(state)
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
        filters: Vec::new(),
        placed_on,
        content,
    })
}

/// A rectangle: left, top, right, bottom.
pub type Bounds = [f32; 4];

fn union(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (one, other) => one.or(other),
    }
}

/// The area `children` cover once moved through `matrix`, or `None` if they
/// draw nothing.
pub fn bounds_of(children: &Children, matrix: Matrix, library: &Library) -> Option<Bounds> {
    children
        .values()
        .filter(|child| child.visible && child.clip_depth.is_none())
        .fold(None, |all, child| {
            union(all, child_bounds(child, matrix, library))
        })
}

/// The area one object covers once moved through `matrix`, the transform of
/// its parent.
pub fn child_bounds(child: &Child, matrix: Matrix, library: &Library) -> Option<Bounds> {
    let matrix = matrix.then_inner(child.matrix);
    let own = |r: &bb_format::Rect| {
        let corners = [
            (r.x_min, r.y_min),
            (r.x_max, r.y_min),
            (r.x_max, r.y_max),
            (r.x_min, r.y_max),
        ]
        .map(|(x, y)| matrix.apply(x as f32, y as f32));
        corners
            .iter()
            .fold(None, |all, &(x, y)| union(all, Some([x, y, x, y])))
    };
    match &child.content {
        Content::Clip(clip) => bounds_of(&clip.children, matrix, library),
        Content::Button(button) => bounds_of(button.shown(), matrix, library),
        Content::Graphic => match &library.manifest.symbols.get(&child.symbol)?.info {
            SymbolInfo::Shape { bounds } => own(bounds),
            SymbolInfo::Text => own(&library.texts.get(&child.symbol)?.bounds),
            SymbolInfo::EditText => own(&library.edit_texts.get(&child.symbol)?.bounds),
            SymbolInfo::MorphShape => {
                let morph = library.morphs.get(&child.symbol)?;
                union(own(&morph.start_bounds), own(&morph.end_bounds))
            }
            _ => None,
        },
    }
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
    /// The draws up to `EndBlur` are drawn together as one picture, which is
    /// then blurred. `bounds` is the area they cover and the blur sizes are
    /// box widths, all in pixels of the target.
    BeginBlur {
        blur_x: f32,
        blur_y: f32,
        passes: u8,
        bounds: Bounds,
    },
    EndBlur,
}

/// Lists what to draw for a clip, back to front.
pub fn commands(clip: &ClipState, base: Matrix, library: &Library) -> Vec<Command> {
    let mut out = Vec::new();
    let context = Context {
        library,
        // Filter sizes are in stage pixels, so they grow with the view.
        view_scale: (base.a * base.d - base.b * base.c).abs().sqrt(),
    };
    draw_children(
        &clip.children,
        base,
        ColorTransform::IDENTITY,
        false,
        &context,
        &mut out,
    );
    out
}

struct Context<'a> {
    library: &'a Library,
    view_scale: f32,
}

fn draw_children(
    children: &Children,
    matrix: Matrix,
    color: ColorTransform,
    in_mask: bool,
    context: &Context,
    out: &mut Vec<Command>,
) {
    // Masks that are in force, innermost last, with the depth each ends at.
    let mut masks: Vec<(u16, &Child)> = Vec::new();
    let end_mask = |mask: &Child, out: &mut Vec<Command>| {
        out.push(Command::DeactivateMask);
        draw_child(mask, matrix, color, true, context, out);
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
                draw_child(child, matrix, color, true, context, out);
                out.push(Command::ActivateMask);
                masks.push((clip_depth, child));
            }
            Some(_) => {}
            None if child.visible => draw_child(child, matrix, color, in_mask, context, out),
            None => {}
        }
    }
    while let Some((_, mask)) = masks.pop() {
        end_mask(mask, out);
    }
}

fn draw_child(
    child: &Child,
    parent_matrix: Matrix,
    parent_color: ColorTransform,
    in_mask: bool,
    context: &Context,
    out: &mut Vec<Command>,
) {
    // A mask is only an outline, so filters do nothing to it.
    let blur = (!in_mask)
        .then(|| {
            child.filters.iter().find_map(|filter| match *filter {
                Filter::Blur {
                    blur_x,
                    blur_y,
                    passes,
                } if passes > 0 && (blur_x > 1.0 || blur_y > 1.0) => Some((blur_x, blur_y, passes)),
                _ => None,
            })
        })
        .flatten()
        .zip(child_bounds(child, parent_matrix, context.library));
    if let Some(((blur_x, blur_y, passes), bounds)) = blur {
        out.push(Command::BeginBlur {
            blur_x: blur_x as f32 * context.view_scale,
            blur_y: blur_y as f32 * context.view_scale,
            passes,
            bounds,
        });
    }

    let matrix = parent_matrix.then_inner(child.matrix);
    let color = parent_color.then_inner(child.color);
    match &child.content {
        Content::Graphic => out.push(Command::Draw {
            symbol: child.symbol,
            ratio: child.ratio,
            matrix,
            color,
        }),
        Content::Clip(clip) => {
            draw_children(&clip.children, matrix, color, in_mask, context, out);
        }
        Content::Button(button) => {
            draw_children(button.shown(), matrix, color, in_mask, context, out);
        }
    }

    if blur.is_some() {
        out.push(Command::EndBlur);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    use bb_format::{Clip, Frame, Manifest, Rect, SoundEvent, Stage, Symbol};

    use super::*;

    pub(crate) const SHAPE: SymbolId = 1;
    pub(crate) const OTHER_SHAPE: SymbolId = 2;
    pub(crate) const INNER: SymbolId = 10;

    pub(crate) fn place(depth: u16, action: PlaceAction) -> Place {
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

    pub(crate) fn put(depth: u16, symbol: SymbolId) -> Op {
        Op::Place(Box::new(place(depth, PlaceAction::Place(symbol))))
    }

    pub(crate) fn frame(ops: Vec<Op>) -> Frame {
        Frame {
            ops,
            ..Frame::default()
        }
    }

    fn sound(id: SymbolId) -> SoundStart {
        SoundStart {
            sound: id,
            event: SoundEvent::Event,
            loops: 1,
            in_sample: None,
            out_sample: None,
            envelope: Vec::new(),
        }
    }

    fn clip(id: Option<SymbolId>, frames: Vec<Frame>) -> Clip {
        Clip {
            id,
            labels: BTreeMap::new(),
            frames,
        }
    }

    pub(crate) fn symbol(file: &str, info: SymbolInfo) -> Symbol {
        Symbol {
            file: file.to_owned(),
            export_name: None,
            info,
        }
    }

    /// A library with two 10 by 10 shapes, the given main timeline, and one
    /// inner clip with the given frames.
    pub(crate) fn library_with(root: Vec<Frame>, inner: Vec<Frame>) -> Library {
        let shape = SymbolInfo::Shape {
            bounds: Rect {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 10.0,
                y_max: 10.0,
            },
        };
        let mut symbols = BTreeMap::new();
        symbols.insert(SHAPE, symbol("shapes/1.svg", shape.clone()));
        symbols.insert(OTHER_SHAPE, symbol("shapes/2.svg", shape));
        let inner_info = SymbolInfo::Clip {
            frame_count: inner.len() as u16,
        };
        symbols.insert(INNER, symbol("clips/10.json", inner_info));
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
            clips: HashMap::from([(INNER, clip(Some(INNER), inner))]),
            buttons: HashMap::new(),
            texts: HashMap::new(),
            edit_texts: HashMap::new(),
            fonts: HashMap::new(),
            morphs: HashMap::new(),
        }
    }

    fn library(root: Vec<Frame>, inner_frames: usize) -> Library {
        library_with(root, vec![Frame::default(); inner_frames])
    }

    fn start(library: &Library) -> ClipState {
        ClipState::new(None, library, &mut Vec::new())
    }

    fn tick(clip: &mut ClipState, library: &Library) -> Vec<Event> {
        let mut events = Vec::new();
        clip.advance(library, &mut events);
        events
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
        let root = start(&library);
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
        let mut root = start(&library);
        tick(&mut root, &library);
        assert_eq!(root.children[&1].matrix, Matrix::translate(5.0, 7.0));
        tick(&mut root, &library);
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
        let mut root = start(&library);
        tick(&mut root, &library);
        assert_eq!(root.children[&1].symbol, OTHER_SHAPE);
        assert_eq!(root.children[&1].matrix, Matrix::translate(5.0, 7.0));
    }

    #[test]
    fn a_nested_clip_plays_on_its_own_but_not_on_the_tick_it_appears() {
        let library = library(
            vec![frame(vec![]), frame(vec![put(1, INNER)]), frame(vec![])],
            5,
        );
        let mut root = start(&library);
        tick(&mut root, &library);
        assert_eq!(inner_frame(&root, 1), 1);
        tick(&mut root, &library);
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
        let mut root = start(&library);
        tick(&mut root, &library);
        tick(&mut root, &library);
        assert_eq!(root.frame, 3);
        assert!(root.children.contains_key(&2));

        tick(&mut root, &library);
        assert_eq!(root.frame, 1);
        assert!(!root.children.contains_key(&2));
        // The inner clip was not rebuilt: it has kept counting.
        assert_eq!(inner_frame(&root, 1), 4);
    }

    #[test]
    fn a_stopped_clip_stays_put_while_its_children_play() {
        let library = library(vec![frame(vec![put(1, INNER)]), frame(vec![])], 5);
        let mut root = start(&library);
        root.playing = false;
        tick(&mut root, &library);
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
        let mut root = start(&library);
        root.goto(2, &library, &mut Vec::new());
        assert_eq!(root.children.keys().copied().collect::<Vec<_>>(), [2]);
        root.goto(1, &library, &mut Vec::new());
        assert_eq!(root.children.keys().copied().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_frame_marked_as_stopping_halts_the_timeline() {
        let stopping = Frame {
            stops: true,
            ..Frame::default()
        };
        let library = library(vec![frame(vec![]), stopping, frame(vec![])], 1);
        let mut root = start(&library);
        assert!(root.playing);
        tick(&mut root, &library);
        assert_eq!(root.frame, 2);
        assert!(!root.playing);
        tick(&mut root, &library);
        assert_eq!(root.frame, 2);
    }

    #[test]
    fn a_sound_is_heard_when_the_playhead_lands_on_its_frame() {
        let with_sound = |id| Frame {
            sounds: vec![sound(id)],
            ..Frame::default()
        };
        let library = library(vec![with_sound(7), with_sound(8), with_sound(9)], 1);

        let mut events = Vec::new();
        let mut root = ClipState::new(None, &library, &mut events);
        assert_eq!(events, [Event::Sound(sound(7))]);

        assert_eq!(tick(&mut root, &library), [Event::Sound(sound(8))]);

        // Jumping from frame 2 back to 1 and on to 3 passes over nothing
        // that should be heard except where it lands.
        let mut events = Vec::new();
        root.goto(1, &library, &mut events);
        root.goto(3, &library, &mut events);
        assert_eq!(events, [Event::Sound(sound(7)), Event::Sound(sound(9))]);
    }

    #[test]
    fn a_clip_kept_through_a_loop_does_not_start_its_sound_again() {
        let inner = vec![
            Frame {
                sounds: vec![sound(7)],
                ..Frame::default()
            },
            Frame::default(),
            Frame::default(),
        ];
        let library = library_with(vec![frame(vec![put(1, INNER)]), frame(vec![])], inner);
        let mut events = Vec::new();
        let mut root = ClipState::new(None, &library, &mut events);
        assert_eq!(events, [Event::Sound(sound(7))]);
        assert!(tick(&mut root, &library).is_empty());
        // Back to frame 1: the inner clip is the same one, part way through.
        assert!(tick(&mut root, &library).is_empty());
        assert_eq!(root.frame, 1);
    }

    #[test]
    fn a_clip_rebuilt_by_a_rewind_starts_its_sound() {
        let inner = vec![Frame {
            sounds: vec![sound(7)],
            ..Frame::default()
        }];
        let library = library_with(
            vec![
                frame(vec![put(1, INNER)]),
                frame(vec![Op::Remove { depth: 1 }]),
            ],
            inner,
        );
        let mut root = start(&library);
        assert!(tick(&mut root, &library).is_empty());
        assert_eq!(tick(&mut root, &library), [Event::Sound(sound(7))]);
    }

    fn kinds(commands: &[Command]) -> Vec<String> {
        commands
            .iter()
            .map(|command| match command {
                Command::Draw { symbol, .. } => format!("draw {symbol}"),
                Command::BeginBlur { .. } => "BeginBlur".to_owned(),
                other => format!("{other:?}"),
            })
            .collect()
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
        let root = start(&library);
        // Symbol 1 is the mask. Symbol 2 sits at depth 2, inside the mask, and
        // again at depth 3, past its end.
        assert_eq!(
            kinds(&commands(&root, Matrix::IDENTITY, &library)),
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
        let root = start(&library);
        let commands = commands(&root, Matrix::scale(2.0, 2.0), &library);
        let Command::Draw { matrix, .. } = &commands[0] else {
            panic!("expected a draw");
        };
        assert_eq!(matrix.apply(0.0, 0.0), (6.0, 8.0));
    }

    #[test]
    fn a_blurred_object_is_wrapped_with_its_area_and_scaled_blur() {
        let blurred = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 20.0, 30.0]),
            filters: Some(vec![Filter::Blur {
                blur_x: 5.0,
                blur_y: 4.0,
                passes: 1,
            }]),
            ..place(1, PlaceAction::Place(SHAPE))
        };
        let library = library(vec![frame(vec![Op::Place(Box::new(blurred))])], 1);
        let root = start(&library);
        let commands = commands(&root, Matrix::scale(2.0, 2.0), &library);
        assert_eq!(kinds(&commands), ["BeginBlur", "draw 1", "EndBlur"]);
        // The 10 by 10 shape at (20, 30), drawn at twice the size.
        assert_eq!(
            commands[0],
            Command::BeginBlur {
                blur_x: 10.0,
                blur_y: 8.0,
                passes: 1,
                bounds: [40.0, 60.0, 60.0, 80.0],
            }
        );
    }

    #[test]
    fn bounds_cover_everything_inside_a_clip() {
        let far = Place {
            matrix: Some([1.0, 0.0, 0.0, 1.0, 50.0, 0.0]),
            ..place(2, PlaceAction::Place(SHAPE))
        };
        let inner = vec![frame(vec![put(1, SHAPE), Op::Place(Box::new(far))])];
        let library = library_with(vec![frame(vec![put(1, INNER)])], inner);
        let root = start(&library);
        assert_eq!(
            bounds_of(&root.children, Matrix::IDENTITY, &library),
            Some([0.0, 0.0, 60.0, 10.0])
        );
    }
}
