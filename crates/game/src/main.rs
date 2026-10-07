//! Plays the baseball game, in a window or from a script.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use bb_engine::app::Runner;
use bb_engine::audio::Audio;
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_engine::window::{self, Options};
use bb_game::baseball::{Baseball, Screen};
use bb_game::locate;
use bb_game::script::Script;
use clap::Parser;

#[derive(Parser)]
#[command(about = "Plays the baseball game")]
struct Args {
    /// The folder `bb-extract` wrote. Without this, the game looks inside
    /// its own app bundle, then in its folder under Application Support,
    /// then beside the program, then for `extracted` where it was started.
    dir: Option<PathBuf>,
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
    /// `wait N`, `click X Y`, `move X Y`, `press`, `release`, `type TEXT`,
    /// `key NAME`, `state`, `events`, `tree` and `shot FILE`.
    #[arg(long)]
    run: Option<String>,
    /// With `--run`: picture pixels per stage pixel.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = format!("{error:#}");
            eprintln!("Error: {message}");
            // An app has no terminal for that to be read in.
            if locate::in_app_bundle() {
                alert(&message);
            }
            ExitCode::FAILURE
        }
    }
}

/// Puts up a system alert saying `message`, and waits for it to be
/// dismissed.
fn alert(message: &str) {
    // AppleScript strings escape only the backslash and the double quote.
    let quoted = message.replace('\\', "\\\\").replace('"', "\\\"");
    let script =
        format!("display alert \"The game could not start\" message \"{quoted}\" as critical");
    // If this fails too there is nothing more to be done about it.
    let _ = Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .status();
}

fn run() -> Result<()> {
    let args = Args::parse();
    let library = Library::load(&locate::find(args.dir.as_deref())?)?;
    let stage = Stage::new(None, &library);
    let mut logic = Box::new(Baseball::new(&library));
    if let Some(label) = &args.screen {
        let screen = Screen::from_label(label)
            .with_context(|| format!("there is no screen called `{label}`"))?;
        logic.start_on(screen);
    }

    if let Some(steps) = &args.run {
        let mut script = Script::new(Runner::new(library, stage, logic, None))?;
        script.scale = args.scale;
        for line in script.run(steps)? {
            println!("{line}");
        }
        for problem in script.problems() {
            eprintln!("problem: {problem}");
        }
        return Ok(());
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
