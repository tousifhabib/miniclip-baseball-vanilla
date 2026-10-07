//! Draws one frame of the extracted game to a PNG, with no window.

use std::path::PathBuf;

use anyhow::{Context, Result};
use bb_engine::display::{Children, Content, Event};
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
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
    /// Keep the top timeline on its frame while the clips inside it play, even
    /// where the original has no `stop()`.
    #[arg(long)]
    hold: bool,
    /// Put the pointer here, in stage coordinates, before drawing: `x,y`.
    #[arg(long, value_parser = parse_point)]
    pointer: Option<(f32, f32)>,
    /// With `--pointer`: hold the pointer's button down there.
    #[arg(long, requires = "pointer")]
    press: bool,
    /// Play every timeline straight through, ignoring the frames where the
    /// original stops.
    #[arg(long)]
    ignore_stops: bool,
    /// Print the tree of objects on the stage, with the frame each clip is
    /// on.
    #[arg(long)]
    tree: bool,
    /// Picture pixels per stage pixel.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

/// Prints one line per object, indented by how deep it is nested.
fn print_tree(children: &Children, library: &Library, indent: usize) {
    for (depth, child) in children {
        let pad = "  ".repeat(indent);
        let name = child
            .name
            .as_deref()
            .map(|name| format!(" \"{name}\""))
            .unwrap_or_default();
        let mask = if child.clip_depth.is_some() {
            " (mask)"
        } else {
            ""
        };
        let placed = child.placed_on;
        match &child.content {
            Content::Graphic => {
                println!("{pad}{depth}: symbol {}{name}{mask}", child.symbol);
            }
            Content::Clip(clip) => {
                println!(
                    "{pad}{depth}: clip {}{name}{mask}, placed on frame {placed}, now on frame {} of {}",
                    child.symbol,
                    clip.frame,
                    clip.frame_count(library)
                );
                print_tree(&clip.children, library, indent + 1);
            }
            Content::Button(button) => {
                println!(
                    "{pad}{depth}: button {}{name}{mask}, {:?}",
                    child.symbol, button.mode
                );
                print_tree(button.shown(), library, indent + 1);
            }
        }
    }
}

fn parse_point(text: &str) -> Result<(f32, f32), String> {
    let (x, y) = text
        .split_once(',')
        .ok_or("expected two numbers, like 120,80")?;
    let number = |part: &str| {
        part.trim()
            .parse::<f32>()
            .map_err(|error| error.to_string())
    };
    Ok((number(x)?, number(y)?))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut library = Library::load(&args.dir)?;
    library.obey_stops = !args.ignore_stops;
    let stage_size = &library.manifest.stage;
    let size = (
        (stage_size.width as f32 * args.scale).round().max(1.0) as u32,
        (stage_size.height as f32 * args.scale).round().max(1.0) as u32,
    );

    let mut renderer = Renderer::headless()?;
    renderer.min_stroke = args.scale.max(1.0);
    let mut stage = Stage::new(args.clip, &library);
    stage.goto(args.frame, &library);
    if args.hold {
        stage.root.playing = false;
    }
    for _ in 0..args.ticks {
        stage.advance(&library, &mut renderer);
    }

    if let Some((x, y)) = args.pointer {
        // Arrive first, then press, as a real pointer would.
        stage.pointer_changed(x, y, false, &library, &mut renderer);
        if args.press {
            stage.pointer_changed(x, y, true, &library, &mut renderer);
        }
    }
    for event in stage.take_events() {
        match event {
            Event::Button { symbol, event, .. } => println!("button {symbol}: {event:?}"),
            Event::Sound(start) => println!("sound {}", start.sound),
        }
    }

    if args.tree {
        print_tree(&stage.root.children, &library, 1);
    }

    let scale = Matrix::scale(args.scale, args.scale);
    let base = match args.clip {
        Some(_) => Matrix::translate(size.0 as f32 / 2.0, size.1 as f32 / 2.0).then_inner(scale),
        None => scale,
    };
    let background = stage_size.background.map_or([0.0, 0.0, 0.0, 1.0], |c| {
        [c.r, c.g, c.b, c.a].map(|channel| f64::from(channel) / 255.0)
    });

    let list = stage.commands(base, &library);
    let image = renderer.capture(&library, &list, size, background)?;
    image
        .save(&args.out)
        .with_context(|| format!("writing {}", args.out.display()))?;

    println!(
        "Drew frame {} of {} to {}: {} draws in {} blurred layers",
        stage.root.frame,
        stage.root.frame_count(&library),
        args.out.display(),
        renderer.stats.draws,
        renderer.stats.layers,
    );
    for problem in &renderer.problems {
        println!("  problem: {problem}");
    }
    Ok(())
}
