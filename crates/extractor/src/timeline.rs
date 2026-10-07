//! Timelines: the per-frame display list changes of the main movie and of
//! each movie clip.

use std::collections::BTreeMap;

use bb_format as f;
use swf::{BlendMode, ClipEventFlag, PlaceObject, PlaceObjectAction, Tag};

use crate::convert::{self, Strings};

pub struct Timeline {
    pub clip: f::Clip,
    /// Tags after the last frame was shown. Flash never displays them.
    pub trailing_ops: usize,
    /// The timeline holds streamed sound, which is not extracted.
    pub has_stream_sound: bool,
}

/// Builds a timeline from a tag list. Tags that define symbols are skipped;
/// the caller handles those.
pub fn timeline(id: Option<u16>, tags: &[Tag], strings: Strings) -> Timeline {
    let mut labels = BTreeMap::new();
    let mut frames = Vec::new();
    let mut frame = f::Frame::default();
    let mut pending_ops = 0;
    let mut has_stream_sound = false;

    for tag in tags {
        match tag {
            Tag::ShowFrame => {
                frames.push(std::mem::take(&mut frame));
                pending_ops = 0;
            }
            Tag::FrameLabel(label) => {
                let name = strings.get(label.label);
                // Frame numbers start at 1.
                labels.insert(name.clone(), frames.len() as u16 + 1);
                frame.label = Some(name);
            }
            Tag::PlaceObject(place) => {
                let place = Box::new(convert_place(place, strings));
                frame.ops.push(f::Op::Place(place));
                pending_ops += 1;
            }
            Tag::RemoveObject(remove) => {
                frame.ops.push(f::Op::Remove {
                    depth: remove.depth,
                });
                pending_ops += 1;
            }
            Tag::StartSound(start) => {
                frame
                    .sounds
                    .push(convert::sound_start(start.id, &start.sound_info));
            }
            Tag::DoAction(_) => frame.has_script = true,
            Tag::SoundStreamBlock(_) => has_stream_sound = true,
            _ => {}
        }
    }

    Timeline {
        clip: f::Clip { id, labels, frames },
        trailing_ops: pending_ops,
        has_stream_sound,
    }
}

fn convert_place(place: &PlaceObject, strings: Strings) -> f::Place {
    f::Place {
        depth: place.depth,
        action: match place.action {
            PlaceObjectAction::Place(id) => f::PlaceAction::Place(id),
            PlaceObjectAction::Modify => f::PlaceAction::Modify,
            PlaceObjectAction::Replace(id) => f::PlaceAction::Replace(id),
        },
        matrix: place.matrix.as_ref().map(convert::matrix),
        color: place.color_transform.as_ref().map(convert::color_transform),
        ratio: place.ratio,
        name: place.name.map(|name| strings.get(name)),
        clip_depth: place.clip_depth,
        filters: place
            .filters
            .as_ref()
            .map(|filters| filters.iter().map(convert::filter).collect()),
        blend_mode: place
            .blend_mode
            .filter(|mode| *mode != BlendMode::Normal)
            .map(|mode| mode.to_string()),
        visible: place.is_visible,
        clip_events: place
            .clip_actions
            .as_ref()
            .map(|actions| clip_event_names(actions.all_event_flags))
            .unwrap_or_default(),
    }
}

fn clip_event_names(flags: ClipEventFlag) -> Vec<String> {
    const NAMES: [(ClipEventFlag, &str); 19] = [
        (ClipEventFlag::LOAD, "load"),
        (ClipEventFlag::ENTER_FRAME, "enter_frame"),
        (ClipEventFlag::UNLOAD, "unload"),
        (ClipEventFlag::MOUSE_MOVE, "mouse_move"),
        (ClipEventFlag::MOUSE_DOWN, "mouse_down"),
        (ClipEventFlag::MOUSE_UP, "mouse_up"),
        (ClipEventFlag::KEY_DOWN, "key_down"),
        (ClipEventFlag::KEY_UP, "key_up"),
        (ClipEventFlag::DATA, "data"),
        (ClipEventFlag::INITIALIZE, "initialize"),
        (ClipEventFlag::PRESS, "press"),
        (ClipEventFlag::RELEASE, "release"),
        (ClipEventFlag::RELEASE_OUTSIDE, "release_outside"),
        (ClipEventFlag::ROLL_OVER, "roll_over"),
        (ClipEventFlag::ROLL_OUT, "roll_out"),
        (ClipEventFlag::DRAG_OVER, "drag_over"),
        (ClipEventFlag::DRAG_OUT, "drag_out"),
        (ClipEventFlag::KEY_PRESS, "key_press"),
        (ClipEventFlag::CONSTRUCT, "construct"),
    ];
    NAMES
        .iter()
        .filter(|(flag, _)| flags.contains(*flag))
        .map(|(_, name)| (*name).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use swf::{RemoveObject, SwfStr};

    use super::*;

    const STRINGS: Strings = Strings { swf_version: 8 };

    fn place(depth: u16, action: PlaceObjectAction) -> Tag<'static> {
        Tag::PlaceObject(Box::new(PlaceObject {
            version: 2,
            action,
            depth,
            matrix: None,
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            class_name: None,
            filters: None,
            background_color: None,
            blend_mode: None,
            clip_actions: None,
            has_image: false,
            is_bitmap_cached: None,
            is_visible: None,
            amf_data: None,
        }))
    }

    #[test]
    fn each_show_frame_ends_a_frame() {
        let tags = [
            place(1, PlaceObjectAction::Place(7)),
            Tag::ShowFrame,
            Tag::ShowFrame,
            Tag::RemoveObject(RemoveObject {
                depth: 1,
                character_id: None,
            }),
            Tag::ShowFrame,
        ];
        let timeline = timeline(Some(3), &tags, STRINGS);
        let ops: Vec<usize> = timeline.clip.frames.iter().map(|f| f.ops.len()).collect();
        assert_eq!(ops, [1, 0, 1]);
        assert_eq!(timeline.trailing_ops, 0);
    }

    #[test]
    fn labels_point_at_the_frame_being_built_counting_from_one() {
        let tags = [
            Tag::ShowFrame,
            Tag::FrameLabel(swf::FrameLabel {
                label: SwfStr::from_utf8_str("swing"),
                is_anchor: false,
            }),
            Tag::ShowFrame,
        ];
        let timeline = timeline(None, &tags, STRINGS);
        assert_eq!(timeline.clip.labels["swing"], 2);
        assert_eq!(timeline.clip.frames[1].label.as_deref(), Some("swing"));
    }

    #[test]
    fn changes_after_the_last_frame_are_counted_not_kept() {
        let tags = [Tag::ShowFrame, place(1, PlaceObjectAction::Modify)];
        let timeline = timeline(None, &tags, STRINGS);
        assert_eq!(timeline.clip.frames.len(), 1);
        assert_eq!(timeline.trailing_ops, 1);
    }

    #[test]
    fn a_script_tag_marks_its_frame() {
        let tags = [Tag::ShowFrame, Tag::DoAction(&[0]), Tag::ShowFrame];
        let timeline = timeline(None, &tags, STRINGS);
        assert!(!timeline.clip.frames[0].has_script);
        assert!(timeline.clip.frames[1].has_script);
    }
}
