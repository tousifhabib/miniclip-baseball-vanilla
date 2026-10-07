//! Plays the extracted timelines in a window, with no game rules.

use std::path::PathBuf;

use anyhow::Result;
use bb_engine::app::{NoLogic, Runner};
use bb_engine::audio::Audio;
use bb_engine::library::Library;
use bb_engine::stage::Stage;
use bb_engine::window::{self, Options};
use clap::Parser;

#[derive(Parser)]
#[command(about = "Plays the extracted timelines in a window, with no game rules")]
struct Args {
    /// The folder `bb-extract` wrote.
    dir: PathBuf,
    /// Show this clip on its own, with its origin at the centre of the
    /// window, instead of the main timeline.
    #[arg(long)]
    clip: Option<u16>,
    /// Start on this frame.
    #[arg(long, default_value_t = 1)]
    frame: u16,
    /// Keep the top timeline on its frame while the clips inside it play, even
    /// where the original has no `stop()`.
    #[arg(long)]
    hold: bool,
    /// Play no sound.
    #[arg(long)]
    mute: bool,
    /// Open with the inspector showing.
    #[arg(long)]
    inspect: bool,
    /// Load every sound, report any that fail, and quit without a window.
    #[arg(long)]
    check_sounds: bool,
    /// Quit after drawing this many frames. For testing.
    #[arg(long)]
    exit_after: Option<u32>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let library = Library::load(&args.dir)?;

    if args.check_sounds {
        let mut audio = Audio::new()?;
        let loaded = audio.load_all(&library);
        println!("Loaded {loaded} sounds.");
        for problem in &audio.problems {
            println!("  problem: {problem}");
        }
        return Ok(());
    }

    let mut stage = Stage::new(args.clip, &library);
    stage.goto(args.frame, &library);
    if args.hold {
        stage.root.playing = false;
    }
    let audio = if args.mute {
        None
    } else {
        // A machine with no sound device can still play, silently.
        Audio::new()
            .inspect_err(|error| eprintln!("Playing without sound: {error:#}"))
            .ok()
    };

    let runner = Runner::new(library, stage, Box::new(NoLogic), audio);
    let summary = window::run(
        runner,
        Options {
            title: "Miniclip Baseball (engine preview)".to_owned(),
            inspect: args.inspect,
            centre_origin: args.clip.is_some(),
            exit_after: args.exit_after,
        },
    )?;
    println!(
        "Drew {} frames; the game asked for {} sounds.",
        summary.frames_drawn, summary.sounds_asked
    );
    for problem in &summary.problems {
        println!("  problem: {problem}");
    }
    Ok(())
}
