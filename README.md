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
| 3 | Engine: a Flash-style tree of clips with timelines, drawn with `wgpu` | Done |
| 4 | Check the engine's output against an independent renderer | Done |
| 5 | Game logic: the rules, written fresh on top of the original art | In progress |
| 6 | Restructure for modding: data files, mod folders, hot reload | |
| 7 | Package as a Mac app | |

## Layout

| Crate | Purpose |
|---|---|
| `crates/format` | Data types for the extracted files, shared by the extractor and the engine |
| `crates/extractor` | `bb-extract`, which converts the SWF |
| `crates/engine` | Plays the extracted timelines and draws them; `bb-player` and `bb-shot` |
| `crates/game` | The baseball game itself: `bb-game` |
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

## Playing the game

```bash
cargo run --release -p bb-game -- extracted
```

Space pauses, F1 opens the inspector, Escape quits, `--mute` turns the sound
off, and `--screen menu` (or `match`, `arcade`, `matchWon` and so on) starts
on a screen of your choice.

The game's rules are new code, not a translation of the original's
scripts. They use the original only for how its art is wired: the names of
its clips, the labels on their frames, and the variable each text field
shows. So the screens and the look are the original's, and the rules are
free to change.

What works so far is getting around: the intro and its SKIP, the main
menu, team setup with the skill level, the summary pages, PLAY BALL into a
match or the arcade game, high scores, the instructions with NEXT and
BACK, the quit prompt, and the result screens. What is not written yet is
the play itself: pitching, batting, fielding and scoring. The match and
arcade screens show the field, and nothing happens on it.

### How the rules meet the engine

The engine's timelines only know how to play. A game supplies a `Logic`
(`crates/engine/src/app.rs`), which is told about every button the pointer
touches and every scripted frame a clip lands on, and is called once a
frame. In return it steers the stage: jump a clip to a labelled frame, play
or stop it, find a clip by its instance name, set what a text field says,
or ask for a sound. `crates/game/src/baseball.rs` is the whole of the
baseball rules, and `crates/game/src/art.rs` is its description of the art.

### Driving it from a script

For checking the rules without a mouse, `--run` plays with no window and
follows a list of steps:

```bash
cargo run --release -p bb-game -- extracted --screen menu \
  --run "wait 60; click 200 192; wait 60; state; shot setup.png"
```

`wait N` plays N frames, `click X Y` clicks at a stage position, `move`,
`press` and `release` work the pointer by hand, `state` prints where the
game is, `events` prints the buttons touched and sounds asked for, `tree`
prints every object on the stage, and `shot FILE` saves a picture.

## Checking the engine

`frame-diff` draws clips with the engine and compares them, frame by frame,
with pictures of the same frames from another renderer. The reference is
JPEXS Free Flash Decompiler's sprite export, which draws any clip at any
frame on its own:

```bash
# With JPEXS: export frames of every clip as PNG (see its -select option to
# pick frames), into a folder laid out as DefineSprite_<id>/<frame>.png.
java -jar ffdec.jar -format sprite:png -export sprite reference path/to/the-game.swf

cargo run --release -p bb-devtools --bin frame-diff -- extracted reference --save worst
```

JPEXS runs no scripts, so the engine plays every timeline straight through
for this. A pixel counts as wrong only if its value is outside what the
other picture has at that spot or right beside it. That forgives a
smoothed edge next to a hard one and shifts of under a pixel, and still
catches anything missing, misplaced or the wrong colour. `--save` writes
three panels for each of the worst clips: the engine's picture, the
reference, and a map of where they disagree.

On 1,056 frames sampled across all 268 clips, 1,031 are within 1% and
1,054 within 5%. Looking into the rest found two faults in
the engine, both fixed:

- A stroke was stretched and skewed along with its shape. Flash draws
  strokes with a round pen on the screen, so the width stays even.
- The pointer started at the corner of the stage, where it could rest on a
  button before the mouse had moved.

The differences that remain were each traced to the reference, or could
not be settled with it:

- JPEXS draws shapes with hard edges and leaves hairline gaps between
  fills that share an edge. The engine smooths edges and leaves no gap.
- JPEXS times a clip nested two levels deep by adding its parent's elapsed
  time. The engine advances every clip one frame per tick from when it
  appears, as Flash does.
- Text fields: JPEXS sets digits about two pixels from where the engine
  does, and closer together. The engine uses the font's own advance
  widths. Which is right needs a comparison with the real player.

## Running the engine

The engine plays the extracted timelines, with sound and a working pointer.
The game logic is not ported yet, so buttons light up and click but nothing
happens when they are pressed. Each timeline halts where the original's
script always calls `stop()`, which is why the game sits on its intro screen.

```bash
# Play in a window. --mute turns the sound off.
cargo run --release -p bb-engine --bin bb-player -- extracted

# Draw one frame to a PNG, with no window: here after 240 frames of play,
# with the pointer pressing at stage position (545, 380).
cargo run --release -p bb-engine --bin bb-shot -- extracted --ticks 240 --pointer 545,380 --press --out frame.png
```

In the window: Space pauses, the right arrow steps one frame while paused,
F1 opens the inspector, Escape quits. Both tools take `--clip ID` to show one
clip on its own, `--frame N` to start on a frame, and `--hold` to keep the
top timeline from playing.

The inspector lists every object on the stage as a tree. For each clip it
shows the frame it is on, with a switch to play or stop it and a slider to
move it. Any object can be hidden, or selected to outline it on the stage.
It also shows the pointer's position, how much each frame takes to draw, and
the latest sounds and button events.

What the engine does:

- Timelines with Flash's rules: per-frame changes, looping, rewinding, and
  nested clips that keep their own place.
- Vector shapes tessellated with `lyon` and drawn with `wgpu`: solid,
  gradient and bitmap fills, strokes, and colour transforms.
- Masks, blur filters, fixed text, text fields showing their starting text,
  and morph shapes.
- Sound through `kira`: timeline and button sounds, repeats, start and end
  points, and starting volume, with Flash's limit of 32 sounds at once.
- Buttons with their up, over and down looks, hit areas, and all seven
  pointer events, reported for the game logic to act on.
- Text fields that say whatever the game sets for the variable they show.
  A field holding markup is drawn as its plain words in the field's own
  font, size and colour; the markup's own styling is not applied yet.

## What the extractor leaves out

- **Scripts.** Frames, buttons and clip events only record that a script was
  there, and frames record whether it always stops the timeline. The game
  logic is ported by hand in step 5.
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
