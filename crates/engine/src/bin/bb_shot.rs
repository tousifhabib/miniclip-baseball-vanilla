//! Draws one frame of the extracted game to a PNG, with no window.

use std::path::PathBuf;

use anyhow::{Context, Result};
use bb_engine::display::{ClipState, commands};
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use clap::Parser;

#[derive(Parser)]
#[command(about = "Draws one frame of the extracted game to a PNG")]
struct Args {
    /// The folder `bb-extract` wrote.
    dir: PathBuf,
    /// Where to write the picture.
    #[arg(long, default_value = "frame.png")]
    out: PathBuf,
    /// Show this clip on its own, with its origin at the centre of the
    /// picture, instead of the main timeline.
    #[arg(long)]
    clip: Option<u16>,
    /// Go to this frame before drawing.
    #[arg(long, default_value_t = 1)]
    frame: u16,
    /// Then play this many frames.
    #[arg(long, default_value_t = 0)]
    ticks: u32,
    /// Keep the top timeline on its frame while the clips inside it play, as
    /// a `stop()` in the original would.
    #[arg(long)]
    hold: bool,
    /// Picture pixels per stage pixel.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let library = Library::load(&args.dir)?;
    let stage = &library.manifest.stage;
    let size = (
        (stage.width as f32 * args.scale).round().max(1.0) as u32,
        (stage.height as f32 * args.scale).round().max(1.0) as u32,
    );

    let mut clip = ClipState::new(args.clip, &library);
    clip.goto(args.frame, &library);
    clip.playing = !args.hold;
    for _ in 0..args.ticks {
        clip.advance(&library);
    }

    let scale = Matrix::scale(args.scale, args.scale);
    let base = match args.clip {
        Some(_) => Matrix::translate(size.0 as f32 / 2.0, size.1 as f32 / 2.0).then_inner(scale),
        None => scale,
    };
    let background = stage.background.map_or([0.0, 0.0, 0.0, 1.0], |c| {
        [c.r, c.g, c.b, c.a].map(|channel| f64::from(channel) / 255.0)
    });

    let mut renderer = Renderer::headless()?;
    renderer.min_stroke = args.scale.max(1.0);
    let list = commands(&clip, base);
    let image = renderer.capture(&library, &list, size, background)?;
    image
        .save(&args.out)
        .with_context(|| format!("writing {}", args.out.display()))?;

    println!(
        "Drew frame {} of {} with {} commands to {}",
        clip.frame,
        clip.frame_count(&library),
        list.len(),
        args.out.display()
    );
    for problem in &renderer.problems {
        println!("  problem: {problem}");
    }
    Ok(())
}
