//! Renders two folders of SVGs and reports how far apart the pictures are.
//!
//! Used to check the extractor's shapes against another tool's export of the
//! same file.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use resvg::{tiny_skia, usvg};

#[derive(Parser)]
#[command(about = "Compares SVG shapes with a reference export, pixel by pixel")]
struct Args {
    /// Folder of SVGs to check.
    ours: PathBuf,
    /// Folder of reference SVGs with the same file names.
    reference: PathBuf,
    /// Pixels per SVG unit.
    #[arg(long, default_value_t = 2.0)]
    scale: f32,
    /// How many of the worst matches to list.
    #[arg(long, default_value_t = 15)]
    worst: usize,
    /// Save a side-by-side picture of each listed shape here: ours on the
    /// left, the reference on the right.
    #[arg(long)]
    save: Option<PathBuf>,
}

/// A channel difference above this counts a pixel as wrong, not as a
/// difference in edge smoothing.
const WRONG: u8 = 64;

struct Diff {
    name: String,
    /// Share of pixels that are wrong.
    wrong: f64,
    /// Average difference over all pixels, from 0 to 1.
    mean: f64,
    size_differs: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut names: Vec<String> = fs::read_dir(&args.ours)
        .with_context(|| format!("reading {}", args.ours.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".svg"))
        .collect();
    names.sort();

    let mut diffs = Vec::new();
    let mut missing = 0;
    for name in names {
        let reference = args.reference.join(&name);
        if !reference.exists() {
            missing += 1;
            continue;
        }
        let ours = render(&args.ours.join(&name), args.scale)?;
        let reference = render(&reference, args.scale)?;
        diffs.push(compare(name, &ours, &reference));
    }
    diffs.sort_by(|a, b| b.wrong.total_cmp(&a.wrong).then(b.mean.total_cmp(&a.mean)));

    let count = diffs.len();
    let within = |limit: f64| diffs.iter().filter(|d| d.wrong <= limit).count();
    println!("{count} shapes compared, {missing} with no reference");
    println!("  {:>5}  with no wrong pixels", within(0.0));
    println!("  {:>5}  with at most 0.1% wrong pixels", within(0.001));
    println!("  {:>5}  with at most 1% wrong pixels", within(0.01));
    println!(
        "  {:>5}  rendered at a different size",
        diffs.iter().filter(|d| d.size_differs).count()
    );

    println!("Worst {}:", args.worst.min(count));
    for diff in diffs.iter().take(args.worst) {
        println!(
            "  {:<12} {:>7.3}% wrong   mean difference {:.4}{}",
            diff.name,
            diff.wrong * 100.0,
            diff.mean,
            if diff.size_differs {
                "   (size differs)"
            } else {
                ""
            }
        );
        if let Some(dir) = &args.save {
            fs::create_dir_all(dir)?;
            let ours = render(&args.ours.join(&diff.name), args.scale)?;
            let reference = render(&args.reference.join(&diff.name), args.scale)?;
            let path = dir.join(Path::new(&diff.name).with_extension("png"));
            side_by_side(&ours, &reference)?
                .save_png(&path)
                .with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(())
}

fn render(path: &Path, scale: f32) -> Result<tiny_skia::Pixmap> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let options = usvg::Options {
        // So that bitmap fills can load the images they point at.
        resources_dir: path.parent().map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(&data, &options)
        .with_context(|| format!("parsing {}", path.display()))?;
    let size = tree.size();
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .with_context(|| format!("{} is too large to render", path.display()))?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

fn compare(name: String, ours: &tiny_skia::Pixmap, reference: &tiny_skia::Pixmap) -> Diff {
    let width = ours.width().min(reference.width());
    let height = ours.height().min(reference.height());
    let mut wrong = 0u64;
    let mut total = 0u64;
    for y in 0..height {
        for x in 0..width {
            let a = ours.pixel(x, y).expect("inside the image");
            let b = reference.pixel(x, y).expect("inside the image");
            let difference = [
                a.red().abs_diff(b.red()),
                a.green().abs_diff(b.green()),
                a.blue().abs_diff(b.blue()),
                a.alpha().abs_diff(b.alpha()),
            ]
            .into_iter()
            .max()
            .expect("four channels");
            total += u64::from(difference);
            wrong += u64::from(difference > WRONG);
        }
    }
    let pixels = (u64::from(width) * u64::from(height)).max(1) as f64;
    Diff {
        name,
        wrong: wrong as f64 / pixels,
        mean: total as f64 / pixels / 255.0,
        size_differs: ours.width() != reference.width() || ours.height() != reference.height(),
    }
}

fn side_by_side(left: &tiny_skia::Pixmap, right: &tiny_skia::Pixmap) -> Result<tiny_skia::Pixmap> {
    const GAP: u32 = 12;
    let width = left.width() + GAP + right.width();
    let height = left.height().max(right.height());
    let mut out = tiny_skia::Pixmap::new(width, height).context("picture is too large")?;
    // Mid grey, so that both light and dark shapes show up.
    out.fill(tiny_skia::Color::from_rgba8(128, 128, 128, 255));
    let paint = tiny_skia::PixmapPaint::default();
    let identity = tiny_skia::Transform::identity();
    out.draw_pixmap(0, 0, left.as_ref(), &paint, identity, None);
    let right_x = (left.width() + GAP) as i32;
    out.draw_pixmap(right_x, 0, right.as_ref(), &paint, identity, None);
    Ok(out)
}
