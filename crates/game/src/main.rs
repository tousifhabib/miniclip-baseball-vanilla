//! The baseball game: the engine playing the extracted art, with the rules
//! in `baseball`.

mod art;
mod baseball;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use bb_engine::app::Runner;
use bb_engine::audio::Audio;
use bb_engine::display::describe_tree;
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
use bb_engine::window::{self, Options};
use clap::Parser;

use crate::baseball::{Baseball, Screen};

#[derive(Parser)]
#[command(about = "Plays the baseball game")]
struct Args {
    /// The folder `bb-extract` wrote.
    dir: PathBuf,
    /// Play no sound.
    #[arg(long)]
    mute: bool,
    /// Open with the inspector showing.
    #[arg(long)]
    inspect: bool,
    /// Quit after drawing this many frames. For testing.
    #[arg(long)]
    exit_after: Option<u32>,
    /// Start on this screen instead of the intro, by its label in the art:
    /// `menu`, `match`, `arcade`, `matchWon`, `matchLost`, `inningsTied`,
    /// `arcadeFinish` or `instructionsAll`.
    #[arg(long)]
    screen: Option<String>,
    /// Play with no window, following these steps, separated by semicolons:
    /// `wait N` plays N frames, `click X Y` clicks at a stage position,
    /// `move X Y` moves the pointer, `press` and `release` work its button
    /// where it is, `state` prints where the game is,
    /// `events` prints the buttons touched and sounds asked for since it
    /// was last used, `tree` prints every object on the stage, and
    /// `shot FILE` saves a picture.
    #[arg(long)]
    run: Option<String>,
    /// With `--run`: picture pixels per stage pixel.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let library = Library::load(&args.dir)?;
    let stage = Stage::new(None, &library);
    let mut logic = Box::new(Baseball::new(&library));
    if let Some(label) = &args.screen {
        let screen = Screen::from_label(label)
            .with_context(|| format!("there is no screen called `{label}`"))?;
        logic.start_on(screen);
    }

    if let Some(steps) = &args.run {
        let runner = Runner::new(library, stage, logic, None);
        return run_steps(runner, steps, args.scale);
    }

    let audio = if args.mute {
        None
    } else {
        // A machine with no sound device can still play, silently.
        Audio::new()
            .inspect_err(|error| eprintln!("Playing without sound: {error:#}"))
            .ok()
    };
    let runner = Runner::new(library, stage, logic, audio);
    let summary = window::run(
        runner,
        Options {
            title: "Miniclip Baseball".to_owned(),
            inspect: args.inspect,
            centre_origin: false,
            exit_after: args.exit_after,
        },
    )?;
    for problem in &summary.problems {
        eprintln!("problem: {problem}");
    }
    Ok(())
}

/// Plays with no window, doing each step in turn.
fn run_steps(mut runner: Runner, steps: &str, scale: f32) -> Result<()> {
    let mut renderer = Renderer::headless()?;
    renderer.min_stroke = scale.max(1.0);

    for step in steps
        .split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
    {
        let words: Vec<&str> = step.split_whitespace().collect();
        let number = |index: usize| -> Result<f32> {
            let word = words
                .get(index)
                .with_context(|| format!("`{step}` needs more after it"))?;
            word.parse()
                .with_context(|| format!("`{word}` in `{step}` is not a number"))
        };
        match words[0] {
            "wait" => {
                for _ in 0..number(1)? as u32 {
                    runner.tick(&mut renderer);
                }
            }
            "move" => {
                let (x, y) = (number(1)?, number(2)?);
                runner.pointer(x, y, false, &mut renderer);
            }
            "click" => {
                let (x, y) = (number(1)?, number(2)?);
                // Arrive, press, let a frame pass, let go: what a hand does.
                runner.pointer(x, y, false, &mut renderer);
                runner.pointer(x, y, true, &mut renderer);
                runner.tick(&mut renderer);
                runner.pointer(x, y, false, &mut renderer);
            }
            "press" | "release" => {
                let (x, y) = (runner.stage.pointer.x, runner.stage.pointer.y);
                runner.pointer(x, y, words[0] == "press", &mut renderer);
            }
            "state" => println!("{}", runner.describe()),
            "events" => {
                // Frames are reported in their hundreds; buttons and sounds
                // are what a script wants to see.
                for note in runner.take_notes() {
                    if !note.contains(": frame ") {
                        println!("  {note}");
                    }
                }
            }
            "tree" => print!(
                "{}",
                describe_tree(&runner.stage.root.children, &runner.library)
            ),
            "shot" => {
                let file = words
                    .get(1)
                    .with_context(|| format!("`{step}` needs a file name"))?;
                let stage = &runner.library.manifest.stage;
                let size = (
                    (stage.width as f32 * scale).round().max(1.0) as u32,
                    (stage.height as f32 * scale).round().max(1.0) as u32,
                );
                let background = stage.background.map_or([0.0, 0.0, 0.0, 1.0], |c| {
                    [c.r, c.g, c.b, c.a].map(|channel| f64::from(channel) / 255.0)
                });
                let commands = runner
                    .stage
                    .commands(Matrix::scale(scale, scale), &runner.library);
                renderer
                    .capture(&runner.library, &commands, size, background)?
                    .save(file)
                    .with_context(|| format!("writing {file}"))?;
            }
            other => bail!("unknown step `{other}`"),
        }
    }
    for problem in &renderer.problems {
        eprintln!("problem: {problem}");
    }
    Ok(())
}
