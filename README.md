# miniclip-baseball-vanilla

A project to rebuild the Flash game *Miniclip Baseball* as a native Rust
application, playing as closely to the original as it can be made to.
Changing the game is for a separate project built on this one. Here, where
the rebuild and the original differ, the original is right.

The game's art and sound are published here by the project's owner, who
holds the rights to them for this game under an agreement with Miniclip.
The original SWF is in `original/`, and `extracted/` is that file converted
to open formats, which is what the game loads. Those rights are for this
game: they do not make the art free to use anywhere else. Miniclip does not
run or support this project.

## Status

| Step | What | State |
|---|---|---|
| 1 | Project setup | Done |
| 2 | Extractor: turn the SWF into open, editable files | Done |
| 3 | Engine: a Flash-style tree of clips with timelines, drawn with `wgpu` | Done |
| 4 | Check the engine's output against an independent renderer | Done |
| 5 | Game logic: the rules, written fresh on top of the original art | In progress: both games can be played from the first pitch to the result |
| 6 | Restructure for modding: data files, mod folders, hot reload | The numbers live in a data file here. The rest is for [miniclip-baseball-mod](https://github.com/tousifhabib/miniclip-baseball-mod), the project for changing the game |
| 7 | Package as a Mac app | In progress: the app builds and runs |

## Layout

| Crate | Purpose |
|---|---|
| `crates/format` | Data types for the extracted files, shared by the extractor and the engine |
| `crates/extractor` | `bb-extract`, which converts the SWF |
| `crates/engine` | Plays the extracted timelines and draws them; `bb-player` and `bb-shot` |
| `crates/game` | The baseball game itself: `bb-game` |
| `crates/devtools` | Checks used while developing |
| `data/rules.toml` | The numbers the game is played by |
| `scripts/bundle-mac.sh` | Builds the Mac app |
| `assets/icon.svg` | The app's icon, drawn for this project |

## Extracting the game

`extracted/` is already here, so this only needs doing again after a change
to the extractor. Needs a current stable Rust toolchain.

```bash
cargo run --release -p bb-extractor -- original/miniclip-baseball.swf --out extracted
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

F1 opens the inspector, F2 pauses, F3 steps one frame while paused, and
Escape quits. `--mute` turns the sound off, `--screen menu` (or `match`,
`arcade`, `matchWon` and so on) starts on a screen of your choice, and
`--scores FILE` keeps the high scores in a file of your choosing.

The game's rules are new code, not a translation of the original's
scripts. They use the original only for how its art is wired: the names of
its clips, the labels on their frames, and the variable each text field
shows. So the screens and the look are the original's, and the rules are
free to change.

What works so far:

- Getting around: the intro and its SKIP, the main menu, team setup with
  the skill level and a team name that can be typed, the summary pages,
  PLAY BALL, high scores, the instructions, the quit prompt, and the result
  screens.
- A match, from the first pitch to winning, losing or tying. The pitcher
  winds up and throws; the game shows where the ball will cross; the ring
  follows the pointer and a click swings. Timing and aim decide whether the
  bat meets the ball and how well. Strikes, balls, walks and strike-outs
  are counted and called. A ball that is hit is followed over the field:
  the nearest fielder runs it down or catches it and throws to the bases,
  runners are forced on, can be sent on or made to slide with their own
  buttons, and are safe or out. A ball over the wall is a home run.

- The arcade game: ten pitches at a target on the outfield. A ball scores
  by the ring it comes down in, the first ring on a pitch counts double,
  and the total is multiplied by the skill level at the end.

- The team's look. A click on the Team Colours strip takes the colour
  under the pointer for the shirts and helmets, on the setup page and in
  the game. In a match each batter has a skin of his own and they carry
  the bat logos in turn; the arcade game's batter is as chosen on its
  setup page.

- Sound: music on the menu, a crowd under each game, and the levels the
  original set for its quieter sounds.

- A high-score table for the arcade game, kept in a file on the player's
  own machine. The original's was kept on its publisher's servers and only
  shown on their site.
- The pointer is hidden while the ring is being aimed, as it was.

The original also has an instruction page for each kind of game, apart
from the one the menu opens. Nothing in the original ever shows them, so
nothing here does either.

- The small things: the space bar takes the next pitch as its button
  does, a flare covers quitting a game, and over a colour strip the art's
  own eyedropper stands in for the pointer.

Where the original does something odd, so does this. A third strike is
counted as the next pitch is got ready, after the look at whether the match
is over, so a side that strikes out for its last out sees one more pitch.
A fielder waiting under the ball has it as soon as it is low enough,
whichever way it is going. The arcade target's rings do not quite meet.
And nothing calls a play dead if it will not end: QUIT is the way out.

One thing is kept that the original did not have, switched off: a limit on
how long a play may last, `longest` under `[field]` in the rules.

### The numbers the game is played by

Numbers such as how many outs an innings has are not in the code. They are
in `data/rules.toml`, which is built into the program. A change is made by
laying another file of the same shape over it, holding only the numbers to
change; a number the game does not have, or one of the wrong kind, is
refused with the name of the file that had it. Here that file holds the
original's numbers and is meant to stay that way. Loading other files over
it from mod folders is the business of the project for changing the game.

### How the rules meet the engine

The engine's timelines only know how to play. A game supplies a `Logic`
(`crates/engine/src/app.rs`), which is told about every button the pointer
touches and every scripted frame a clip lands on, and is called once a
frame. In return it steers the stage: jump a clip to a labelled frame, play
or stop it, find a clip by its instance name, set what a text field says,
or ask for a sound. In `crates/game/src`, `baseball.rs` decides which screen
is showing, `menu.rs` is the menu, `play/` is a match (`pitch.rs` and
`field.rs` are the ball's flight to the bat and over the field, with no art
in them), `rules.rs` reads the numbers, and `art.rs` describes the art.
`--seed N` makes every game go the same way.

### Driving it from a script

For checking the rules without a mouse, `--run` plays with no window and
follows a list of steps:

```bash
cargo run --release -p bb-game -- extracted --screen menu \
  --run "wait 60; click 200 192; wait 60; state; shot setup.png"
```

`wait N` plays N frames, `click X Y` clicks at a stage position, `move`,
`press` and `release` work the pointer by hand, `type TEXT` types, `key NAME`
presses a key such as `backspace` or `enter`, `state` prints where the game
is, `events` prints the buttons touched and sounds asked for, `tree` prints
every object on the stage, and `shot FILE` saves a picture.

The same steps drive the tests in `crates/game/tests`, which play the real
game with no window.

## Making the Mac app

```bash
scripts/bundle-mac.sh
open target/app/Baseball.app
```

This builds the game with its slowest, smallest settings, draws the icon in
every size, and puts the art from `extracted/` inside the app, so the app
runs on its own. It is signed for the Mac it was built on, which is enough
to run it there. Giving it to anyone else would need a Developer ID
signature and notarising.

`--no-art` leaves the art out. The app then looks for it in
`~/Library/Application Support/io.github.tousifhabib.baseball/extracted`.
`--universal` builds for both Apple and Intel Macs, once both targets have
been added with `rustup target add`.

Started with no folder named, the game looks for the art inside its own
app, then in that Application Support folder, then beside the program, then
for `extracted` in the folder it was started from. If it cannot start, the
app says why in an alert.

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

To look at an animation without stepping through it, `clip-sheet` draws
many frames of one clip side by side, each marked with its number:

```bash
cargo run --release -p bb-devtools --bin clip-sheet -- extracted --clip 688 --every 6 --play --out pitcher.png
```

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

In the window: F1 opens the inspector, F2 pauses, and F3 steps one frame
while paused. Every other key is offered to the game first; where the game
has no use for it, Space also pauses, the right arrow also steps, and Escape
quits. Both tools take `--clip ID` to show one clip on its own, `--frame N`
to start on a frame, and `--hold` to keep the top timeline from playing.

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
- Text fields the player can type in: a click gives one the typing and shows
  a caret, and it keeps to the field's length and to the letters its font
  has.
- Objects the rules take charge of. Once the rules have moved, tinted or
  hidden an object, its timeline leaves that setting alone, as Flash did
  for an object a script had touched. The rules can also add objects of
  their own, which stay until the rules remove them.
- Keys, handed to the game's rules when no text field wants them.

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

No licence has been chosen yet for the code. The art and sound are not
covered by whatever licence the code is given: see the top of this page.
