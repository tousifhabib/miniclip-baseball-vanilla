//! Compares two folders of bitmaps and reports how far apart the pixels are.
//!
//! Files are paired by name without the extension, so `12.png` is compared
//! with `12.jpg`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
#[command(about = "Compares decoded bitmaps with a reference export, channel by channel")]
struct Args {
    /// Folder of bitmaps to check.
    ours: PathBuf,
    /// Folder of reference bitmaps.
    reference: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let ours = by_stem(&args.ours)?;
    let reference = by_stem(&args.reference)?;

    println!(
        "{:<8} {:<10} {:>10} {:>11} {:>10} {:>11} {:>12}",
        "name", "size", "alpha max", "alpha mean", "colour max", "colour mean", "see-through"
    );
    for (stem, path) in &ours {
        let Some(reference_path) = reference.get(stem) else {
            println!("{stem:<8} no reference");
            continue;
        };
        let a = load(path)?;
        let b = load(reference_path)?;
        if a.dimensions() != b.dimensions() {
            println!(
                "{stem:<8} size differs: {:?} against {:?}",
                a.dimensions(),
                b.dimensions()
            );
            continue;
        }

        let (mut alpha_max, mut alpha_sum) = (0u8, 0u64);
        let (mut colour_max, mut colour_sum, mut colour_count) = (0u8, 0u64, 0u64);
        // Pixels of ours that are neither clear nor solid.
        let mut see_through = 0u64;
        for (p, q) in a.pixels().zip(b.pixels()) {
            let difference = p[3].abs_diff(q[3]);
            alpha_max = alpha_max.max(difference);
            alpha_sum += u64::from(difference);
            see_through += u64::from(p[3] != 0 && p[3] != 255);
            // Colour means nothing where a pixel is fully clear.
            if p[3] > 0 && q[3] > 0 {
                for channel in 0..3 {
                    let difference = p[channel].abs_diff(q[channel]);
                    colour_max = colour_max.max(difference);
                    colour_sum += u64::from(difference);
                    colour_count += 1;
                }
            }
        }
        let pixels = u64::from(a.width()) * u64::from(a.height());
        println!(
            "{stem:<8} {:<10} {alpha_max:>10} {:>11.3} {colour_max:>10} {:>11.3} {see_through:>12}",
            format!("{}x{}", a.width(), a.height()),
            alpha_sum as f64 / pixels.max(1) as f64,
            colour_sum as f64 / colour_count.max(1) as f64,
        );
    }
    Ok(())
}

fn by_stem(dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            files.insert(stem.to_owned(), path);
        }
    }
    Ok(files)
}

fn load(path: &Path) -> Result<image::RgbaImage> {
    Ok(image::open(path)
        .with_context(|| format!("reading {}", path.display()))?
        .to_rgba8())
}
