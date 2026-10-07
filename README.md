# miniclip-baseball-mod

An unofficial, fan-made project to rebuild the Flash game *Miniclip Baseball*
as a native Rust application that is easy to mod.

**This repository contains no game files.** The game and its art, sound and
code belong to Miniclip. You need your own copy of the game's SWF. The tools
here convert it on your machine, and nothing taken from it is committed. This
project is not affiliated with or endorsed by Miniclip.

## Status

| Step | What | State |
|---|---|---|
| 1 | Project setup | Done |
| 2 | Extractor: turn the SWF into open, editable files | Done |
| 3 | Engine: a Flash-style tree of clips with timelines, drawn with `wgpu` | In progress |
| 4 | Check the engine's output against the original | |
| 5 | Port the game logic to Rust, kept parallel to the original | |
| 6 | Restructure for modding: data files, mod folders, hot reload | |
| 7 | Package as a Mac app | |

## Layout

| Crate | Purpose |
|---|---|
| `crates/format` | Data types for the extracted files, shared by the extractor and the engine |
| `crates/extractor` | `bb-extract`, which converts the SWF |
| `crates/engine` | Plays the extracted timelines and draws them; `bb-player` and `bb-shot` |
| `crates/devtools` | Checks used while developing |

## Extracting the game

Needs a current stable Rust toolchain.

```bash
cargo run --release -p bb-extractor -- path/to/the-game.swf --out extracted
```

This writes:

| Path | Contents |
|---|---|
| `manifest.json` | Stage size, frame rate, and every symbol with its kind and file |
| `shapes/*.svg` | Vector shapes, editable in any SVG editor |
| `bitmaps/*.png` | Bitmaps |
| `sounds/*.mp3` | Sounds |
| `clips/*.json` | Timelines: what each frame places, moves and removes |
| `buttons/*.json` | Button states and the mouse events they respond to |
| `texts/*.json` | Fixed text and text fields |
| `fonts/*.json` | Glyph outlines and metrics |
| `morphs/*.json` | Shapes that blend between two outlines |

Lengths are in pixels and frame numbers start at 1. The types in
`crates/format` document every field.

The extractor prints anything it could not convert instead of dropping it
silently.

## Checking the output

```bash
# Read everything back and follow every reference between files.
cargo run --release -p bb-devtools --bin extracted-check -- extracted

# Compare shapes or bitmaps with another tool's export of the same SWF,
# for example one made with JPEXS Free Flash Decompiler.
cargo run --release -p bb-devtools --bin shape-diff -- extracted/shapes path/to/reference/shapes
cargo run --release -p bb-devtools --bin bitmap-diff -- extracted/bitmaps path/to/reference/images
```

## Running the engine

The engine plays the extracted timelines. The game logic is not ported yet,
so nothing reacts to input and no script stops a timeline: clips simply play
through their frames.

```bash
# Play in a window. Space pauses, the right arrow steps a frame while paused,
# Escape quits. --hold keeps the main timeline on its frame, as the game's own
# stop() would, while the clips inside it play.
cargo run --release -p bb-engine --bin bb-player -- extracted --frame 3 --hold

# Draw one frame to a PNG, with no window.
cargo run --release -p bb-engine --bin bb-shot -- extracted --frame 3 --hold --ticks 88 --out frame.png
```

What the engine does so far:

- Timelines with Flash's rules: per-frame changes, looping, rewinding, and
  nested clips that keep their own place.
- Vector shapes tessellated with `lyon` and drawn with `wgpu`: solid,
  gradient and bitmap fills, strokes, and colour transforms.
- Masks, fixed text, text fields showing their starting text, morph shapes,
  and buttons in their resting state.

Not there yet: sound, blur filters, button and mouse handling, and an
inspector for looking inside a running scene.

## What the extractor leaves out

- **Scripts.** Frames, buttons and clip events only record that a script was
  there. The game logic is ported by hand in step 5.
- **Filters other than blur.** They are recorded by name. This game only uses
  blur.
- **Formats this game does not use:** 15-bit bitmaps, sound that is not MP3,
  streamed sound, and the oldest and newest font tags. They are reported as
  problems, not converted.
- **Flash's minimum stroke width.** Flash Player draws every stroke at least
  one screen pixel wide. Stroke widths are written as stored, and the rule is
  left to the renderer because it depends on the zoom.

## Licence

No licence has been chosen yet.
