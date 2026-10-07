//! Draws many frames of one clip side by side in a single picture, each
//! marked with its frame number. For seeing what an animation does without
//! stepping through it in a window.

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use bb_engine::display::{Bounds, bounds_of};
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
use clap::Parser;
use image::{Rgba, RgbaImage};

#[derive(Parser)]
#[command(about = "Draws many frames of one clip side by side")]
struct Args {
    /// The folder `bb-extract` wrote.
    extracted: PathBuf,
    /// The clip to draw.
    #[arg(long)]
    clip: u16,
    /// The frames to draw, such as `1,5,9`. Without this, every `--every`th
    /// frame from `--from` to `--to`.
    #[arg(long, value_delimiter = ',')]
    frames: Vec<u16>,
    #[arg(long, default_value_t = 1)]
    from: u16,
    /// Defaults to the clip's last frame.
    #[arg(long)]
    to: Option<u16>,
    #[arg(long, default_value_t = 1)]
    every: u16,
    /// Play the clip from its first frame, with the clips inside it playing
    /// along, instead of jumping to each frame with the clips inside on
    /// their first.
    #[arg(long)]
    play: bool,
    /// The part of the clip to show, in its own coordinates, as
    /// `left,top,right,bottom`. Without this, whatever the frames cover.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    area: Vec<f32>,
    /// The width and height of each frame's picture, in pixels.
    #[arg(long, default_value_t = 180)]
    cell: u32,
    #[arg(long, default_value_t = 8)]
    columns: u32,
    #[arg(long, default_value = "sheet.png")]
    out: PathBuf,
}

/// The digits 0 to 9, three pixels wide and five tall, a row to a number
/// with the leftmost pixel in the highest bit.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// How many picture pixels each pixel of a digit covers.
const DIGIT_SCALE: u32 = 2;

/// Writes `number` on a dark plate with its corner at (`left`, `top`).
fn stamp(image: &mut RgbaImage, number: u16, left: u32, top: u32) {
    let text = number.to_string();
    let width = (text.len() as u32 * 4 + 1) * DIGIT_SCALE;
    let height = 7 * DIGIT_SCALE;
    let mut put = |x: u32, y: u32, colour: Rgba<u8>| {
        if x < image.width() && y < image.height() {
            image.put_pixel(x, y, colour);
        }
    };
    for y in 0..height {
        for x in 0..width {
            put(left + x, top + y, Rgba([0, 0, 0, 255]));
        }
    }
    for (place, digit) in text.bytes().enumerate() {
        let rows = DIGITS[usize::from(digit - b'0')];
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..3 {
                if bits & (0b100 >> column) == 0 {
                    continue;
                }
                for dy in 0..DIGIT_SCALE {
                    for dx in 0..DIGIT_SCALE {
                        put(
                            left + (place as u32 * 4 + 1 + column) * DIGIT_SCALE + dx,
                            top + (row as u32 + 1) * DIGIT_SCALE + dy,
                            Rgba([255, 255, 255, 255]),
                        );
                    }
                }
            }
        }
    }
}

fn union(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (one, other) => one.or(other),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut library = Library::load(&args.extracted)?;
    // The point is to see every frame, so nothing may halt on the way.
    library.obey_stops = false;
    ensure!(
        library.clips.contains_key(&args.clip),
        "there is no clip {}",
        args.clip
    );
    let count = Stage::new(Some(args.clip), &library)
        .root
        .frame_count(&library);
    let frames: Vec<u16> = if args.frames.is_empty() {
        (args.from.max(1)..=args.to.unwrap_or(count).min(count))
            .step_by(usize::from(args.every.max(1)))
            .collect()
    } else {
        args.frames.clone()
    };
    ensure!(!frames.is_empty(), "there are no frames to draw");
    if let Some(&bad) = frames.iter().find(|&&frame| frame < 1 || frame > count) {
        bail!("clip {} has frames 1 to {count}, not {bad}", args.clip);
    }
    ensure!(
        !args.play || frames.is_sorted(),
        "with --play the frames must be in order"
    );

    let mut renderer = Renderer::headless()?;
    // The stage as it stands on each frame wanted.
    let mut stages = Vec::new();
    if args.play {
        let mut stage = Stage::new(Some(args.clip), &library);
        // The clips inside play on while the one being looked at is moved
        // by hand, so that a frame can be asked for twice.
        stage.root.playing = false;
        for &frame in &frames {
            while stage.root.frame < frame {
                stage.advance(&library, &mut renderer);
                let next = stage.root.frame + 1;
                stage.goto(next, &library);
            }
            stages.push((frame, stage.root.clone()));
        }
    } else {
        for &frame in &frames {
            let mut stage = Stage::new(Some(args.clip), &library);
            stage.goto(frame, &library);
            stages.push((frame, stage.root.clone()));
        }
    }

    let area = match args.area[..] {
        [left, top, right, bottom] => [left, top, right, bottom],
        [] => stages
            .iter()
            .fold(None, |all, (_, root)| {
                union(all, bounds_of(&root.children, Matrix::IDENTITY, &library))
            })
            .with_context(|| format!("clip {} draws nothing on these frames", args.clip))?,
        _ => bail!("--area takes four numbers: left,top,right,bottom"),
    };
    let (width, height) = (area[2] - area[0], area[3] - area[1]);
    ensure!(width > 0.0 && height > 0.0, "the area to show is empty");
    let cell = args.cell.max(16);
    let scale = cell as f32 / width.max(height);
    // The area sits in the middle of its square.
    let base = Matrix::translate(
        (cell as f32 - width * scale) / 2.0,
        (cell as f32 - height * scale) / 2.0,
    )
    .then_inner(Matrix::scale(scale, scale))
    .then_inner(Matrix::translate(-area[0], -area[1]));
    renderer.min_stroke = scale.max(1.0);

    let columns = args.columns.clamp(1, frames.len() as u32);
    let rows = (frames.len() as u32).div_ceil(columns);
    // A gap between the pictures, so that it is clear where each one ends.
    let step = cell + 2;
    let mut sheet = RgbaImage::from_pixel(columns * step, rows * step, Rgba([255, 0, 255, 255]));
    let empty = bb_engine::display::Texts::new();
    for (index, (frame, root)) in stages.iter().enumerate() {
        let commands = bb_engine::display::commands(root, base, &library, &empty);
        // A flat grey shows up both light artwork and dark.
        let picture =
            renderer.capture(&library, &commands, (cell, cell), [0.45, 0.45, 0.5, 1.0])?;
        let (left, top) = (
            (index as u32 % columns) * step,
            (index as u32 / columns) * step,
        );
        image::imageops::replace(&mut sheet, &picture, i64::from(left), i64::from(top));
        stamp(&mut sheet, *frame, left, top);
    }
    sheet
        .save(&args.out)
        .with_context(|| format!("writing {}", args.out.display()))?;

    println!(
        "Drew {} frames of clip {} ({count} in all) to {}, showing {:.1},{:.1} to {:.1},{:.1} at {scale:.2}x",
        frames.len(),
        args.clip,
        args.out.display(),
        area[0],
        area[1],
        area[2],
        area[3],
    );
    for problem in &renderer.problems {
        println!("  problem: {problem}");
    }
    Ok(())
}
