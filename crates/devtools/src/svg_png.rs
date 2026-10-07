//! Draws an SVG to a PNG of a given width. Used to make the app's icon in
//! each size the system wants.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use resvg::{tiny_skia, usvg};

#[derive(Parser)]
#[command(about = "Draws an SVG to a PNG")]
struct Args {
    svg: PathBuf,
    /// The picture's width in pixels. Its height follows the drawing's shape.
    #[arg(long)]
    width: u32,
    #[arg(long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let data = fs::read(&args.svg).with_context(|| format!("reading {}", args.svg.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .with_context(|| format!("parsing {}", args.svg.display()))?;
    let size = tree.size();
    let scale = args.width as f32 / size.width();
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap =
        tiny_skia::Pixmap::new(args.width.max(1), height).context("the picture is too large")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .save_png(&args.out)
        .map_err(|error| anyhow!("writing {}: {error}", args.out.display()))?;
    Ok(())
}
