//! Draws clips with the engine and compares them, frame by frame, with
//! pictures of the same frames from another renderer.
//!
//! The reference pictures are laid out as JPEXS Free Flash Decompiler exports
//! sprites: `DefineSprite_<id>/<frame>.png`, every frame of a clip the same
//! size. JPEXS runs no scripts, so the engine plays every timeline straight
//! through too.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bb_engine::display::{Bounds, ClipState, commands};
use bb_engine::gpu::Renderer;
use bb_engine::library::Library;
use bb_engine::math::Matrix;
use bb_engine::stage::Stage;
use bb_format::{Filter, Op, PlaceAction, Rect, SymbolId, SymbolInfo};
use clap::Parser;

#[derive(Parser)]
#[command(about = "Compares the engine's frames with a reference render, pixel by pixel")]
struct Args {
    /// The folder `bb-extract` wrote.
    extracted: PathBuf,
    /// Folder of reference pictures: `DefineSprite_<id>/<frame>.png`.
    reference: PathBuf,
    /// Only compare these clips.
    #[arg(long, value_delimiter = ',')]
    clips: Vec<u16>,
    /// Leave pictures with fewer pixels than this out of the list of worst
    /// clips. In a picture a few pixels across, one edge pixel is a large
    /// share of the whole.
    #[arg(long, default_value_t = 0)]
    min_pixels: u32,
    /// Move every picture by this much before comparing, as `x,y` in pixels.
    /// For finding out whether a mismatch is only a small offset.
    #[arg(long, value_parser = parse_point, allow_hyphen_values = true)]
    nudge: Option<(f32, f32)>,
    /// Draw the engine's pictures this many times larger and average them
    /// down, for smoother edges than the engine's own four samples a pixel.
    /// The reference renderer smooths edges exactly, so this compares
    /// shapes, not smoothing.
    #[arg(long, default_value_t = 1)]
    supersample: u32,
    /// How many of the worst clips to list.
    #[arg(long, default_value_t = 20)]
    worst: usize,
    /// Save a picture of each listed clip's worst frame here, in three
    /// panels: the engine's, the reference, and a map of where they differ.
    #[arg(long)]
    save: Option<PathBuf>,
}

/// How far a channel may be from what the other picture has before the
/// pixel counts as wrong.
const WRONG: u8 = 64;

struct Diff {
    clip: u16,
    frame: u16,
    /// Share of pixels that are wrong: outside what the other picture has
    /// at that spot or right next to it.
    wrong: f64,
    /// Share of pixels that differ at the very same spot. This is high
    /// wherever one renderer smooths an edge and the other does not.
    strict: f64,
    /// Average difference over all pixels, from 0 to 1.
    mean: f64,
    /// The picture's size.
    width: u32,
    height: u32,
    /// How much each picture covers: the sum of its pixels' opacity, in
    /// whole pixels. A renderer that draws shapes fatter has more.
    ink: f64,
    reference_ink: f64,
}

/// The areas worked out so far, by clip. `None` for a clip that draws nothing.
type Areas = HashMap<SymbolId, Option<Bounds>>;

fn parse_point(text: &str) -> Result<(f32, f32), String> {
    let (x, y) = text
        .split_once(',')
        .ok_or("expected two numbers, like 0.5,-0.25")?;
    let number = |part: &str| {
        part.trim()
            .parse::<f32>()
            .map_err(|error| error.to_string())
    };
    Ok((number(x)?, number(y)?))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut library = Library::load(&args.extracted)?;
    library.obey_stops = false;
    let mut renderer = Renderer::headless()?;
    let mut areas = Areas::new();
    // How far each clip's picture had to be moved to line up with its
    // reference, for the clips that needed it.
    let mut shifts: HashMap<SymbolId, (f32, f32)> = HashMap::new();

    let mut diffs = Vec::new();
    // Clips whose area comes out differently here than in the reference.
    let mut size_differs = Vec::new();
    let mut clips_compared = 0;
    let mut ids: Vec<u16> = library.clips.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        if !args.clips.is_empty() && !args.clips.contains(&id) {
            continue;
        }
        let frames = reference_frames(&args.reference.join(format!("DefineSprite_{id}")));
        let (Some(&first), Some(&last)) = (frames.keys().next(), frames.keys().next_back()) else {
            continue;
        };
        clips_compared += 1;
        let [left, top, right, bottom] = clip_area(id, &library, &mut areas).unwrap_or([0.0; 4]);

        let shift = align(
            id,
            &frames,
            [left, top, right, bottom],
            &library,
            &mut renderer,
        )?;
        if shift != (0.0, 0.0) {
            shifts.insert(id, shift);
        }
        let nudge = args.nudge.unwrap_or_default();
        let shift = (shift.0 + nudge.0, shift.1 + nudge.1);
        let base = Matrix::translate(shift.0 - left, shift.1 - top);

        let mut stage = Stage::new(Some(id), &library);
        let count = stage.root.frame_count(&library);
        for frame in 1..=last.min(count) {
            if let Some(path) = frames.get(&frame) {
                let reference = image::open(path)
                    .with_context(|| format!("reading {}", path.display()))?
                    .to_rgba8();
                let size = reference.dimensions();
                if frame == first {
                    let ours = ((right - left).ceil() as u32, (bottom - top).ceil() as u32);
                    if ours.0.abs_diff(size.0) > 1 || ours.1.abs_diff(size.1) > 1 {
                        size_differs.push((id, ours, size));
                    }
                }
                let ours = draw(
                    &stage.root,
                    base,
                    size,
                    [0.0; 4],
                    args.supersample,
                    &library,
                    &mut renderer,
                )?;
                diffs.push(compare(id, frame, &ours, &reference));
            }
            stage.advance(&library, &mut renderer);
        }
    }
    diffs.sort_by(|a, b| b.wrong.total_cmp(&a.wrong).then(b.mean.total_cmp(&a.mean)));

    let count = diffs.len();
    let within = |limit: f64| diffs.iter().filter(|d| d.wrong <= limit).count();
    println!("{count} frames of {clips_compared} clips compared");
    println!("  {:>6}  with at most 0.1% wrong pixels", within(0.001));
    println!("  {:>6}  with at most 1% wrong pixels", within(0.01));
    println!("  {:>6}  with at most 5% wrong pixels", within(0.05));
    println!("  {:>6}  with more", count - within(0.05));
    let exact = diffs.iter().filter(|d| d.strict <= 0.01).count();
    println!(
        "  ({exact} agree to within 1% at the very same pixels; the rest differ only in how edges are smoothed or by under a pixel)"
    );
    println!(
        "{} clips cover a different area than their reference pictures",
        size_differs.len()
    );
    for (id, ours, theirs) in size_differs.iter().take(12) {
        let (x, y) = shifts.get(id).copied().unwrap_or_default();
        println!("  clip {id}: {ours:?} here, {theirs:?} in the reference, moved by ({x}, {y})");
    }

    // The worst frame of each clip, so that one bad clip does not fill the
    // whole list.
    let mut worst: Vec<&Diff> = Vec::new();
    for diff in &diffs {
        if worst.len() < args.worst
            && diff.width * diff.height >= args.min_pixels
            && !worst.iter().any(|seen| seen.clip == diff.clip)
        {
            worst.push(diff);
        }
    }
    println!("Worst frame of the worst clips:");
    for diff in &worst {
        let frames = diffs.iter().filter(|d| d.clip == diff.clip);
        let (bad, all) = frames.fold((0, 0), |(bad, all), d| {
            (bad + usize::from(d.wrong > 0.01), all + 1)
        });
        println!(
            "  clip {:<5} frame {:<5} {:>7.3}% wrong ({:>6.3}% at the same pixel)   {:>4}x{:<4}  ink {:>8.0} here, {:>8.0} in the reference   {bad} of {all} frames over 1%",
            diff.clip,
            diff.frame,
            diff.wrong * 100.0,
            diff.strict * 100.0,
            diff.width,
            diff.height,
            diff.ink,
            diff.reference_ink
        );
    }
    for problem in &renderer.problems {
        println!("problem: {problem}");
    }

    if let Some(dir) = &args.save {
        fs::create_dir_all(dir)?;
        for diff in worst {
            let shift = shifts.get(&diff.clip).copied().unwrap_or_default();
            save_pair(&args, &library, &mut renderer, &mut areas, shift, diff, dir)?;
        }
    }
    Ok(())
}

/// The reference pictures of one clip, by frame number.
fn reference_frames(dir: &Path) -> BTreeMap<u16, PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let frame = path.file_stem()?.to_str()?.parse().ok()?;
            Some((frame, path))
        })
        .collect()
}

/// The area the reference renderer crops a clip's pictures to: everything
/// the clip places on any frame, where a clip placed inside it counts for
/// everything on any frame of its own timeline, whether or not the outer
/// clip runs long enough to show it. A blurred object counts for the area
/// its blur spreads over.
fn clip_area(clip: SymbolId, library: &Library, areas: &mut Areas) -> Option<Bounds> {
    if let Some(&area) = areas.get(&clip) {
        return area;
    }
    // Stops a clip that contains itself from going round for ever.
    areas.insert(clip, None);

    let timeline = library.clips.get(&clip)?;
    // What is at each depth: the symbol, where it is, and how far its blur
    // spreads on each side.
    let mut placed: BTreeMap<u16, (SymbolId, Matrix, (f32, f32))> = BTreeMap::new();
    let mut area = None;
    for frame in &timeline.frames {
        for op in &frame.ops {
            let place = match op {
                Op::Remove { depth } => {
                    placed.remove(depth);
                    continue;
                }
                Op::Place(place) => place,
            };
            let matrix = place.matrix.map(Matrix::from);
            let spread = place.filters.as_ref().map(|filters| {
                filters
                    .iter()
                    .fold((0.0, 0.0), |(x, y), filter| match filter {
                        Filter::Blur { blur_x, blur_y, .. } => {
                            (x + *blur_x as f32, y + *blur_y as f32)
                        }
                        Filter::Unsupported { .. } => (x, y),
                    })
            });
            match place.action {
                PlaceAction::Place(symbol) => {
                    placed.entry(place.depth).or_insert((
                        symbol,
                        matrix.unwrap_or(Matrix::IDENTITY),
                        spread.unwrap_or_default(),
                    ));
                }
                PlaceAction::Modify => {
                    if let Some(entry) = placed.get_mut(&place.depth) {
                        entry.1 = matrix.unwrap_or(entry.1);
                        entry.2 = spread.unwrap_or(entry.2);
                    }
                }
                PlaceAction::Replace(symbol) => {
                    let kept = placed.get(&place.depth);
                    let matrix = matrix
                        .or(kept.map(|entry| entry.1))
                        .unwrap_or(Matrix::IDENTITY);
                    let spread = spread.or(kept.map(|entry| entry.2)).unwrap_or_default();
                    placed.insert(place.depth, (symbol, matrix, spread));
                }
            }
        }
        for &(symbol, matrix, (spread_x, spread_y)) in placed.values() {
            let own = symbol_area(symbol, library, areas).map(|bounds| {
                let [left, top, right, bottom] = moved(bounds, matrix);
                [
                    left - spread_x,
                    top - spread_y,
                    right + spread_x,
                    bottom + spread_y,
                ]
            });
            area = join(area, own);
        }
    }
    areas.insert(clip, area);
    area
}

/// The area a symbol covers in its own coordinates.
fn symbol_area(symbol: SymbolId, library: &Library, areas: &mut Areas) -> Option<Bounds> {
    let rect = |r: &Rect| {
        [
            r.x_min as f32,
            r.y_min as f32,
            r.x_max as f32,
            r.y_max as f32,
        ]
    };
    match &library.manifest.symbols.get(&symbol)?.info {
        SymbolInfo::Shape { bounds } => Some(rect(bounds)),
        SymbolInfo::Text => Some(rect(&library.texts.get(&symbol)?.bounds)),
        SymbolInfo::EditText => Some(rect(&library.edit_texts.get(&symbol)?.bounds)),
        SymbolInfo::MorphShape => {
            let morph = library.morphs.get(&symbol)?;
            join(
                Some(rect(&morph.start_bounds)),
                Some(rect(&morph.end_bounds)),
            )
        }
        SymbolInfo::Clip { .. } => clip_area(symbol, library, areas),
        // Every look of a button counts, and so does its hit area.
        SymbolInfo::Button => {
            let button = library.buttons.get(&symbol)?;
            button.records.iter().fold(None, |all, record| {
                let own = symbol_area(record.symbol, library, areas);
                join(all, own.map(|bounds| moved(bounds, record.matrix.into())))
            })
        }
        SymbolInfo::Bitmap { .. } | SymbolInfo::Sound { .. } | SymbolInfo::Font { .. } => None,
    }
}

/// The upright rectangle around `bounds` once it has gone through `matrix`.
fn moved(bounds: Bounds, matrix: Matrix) -> Bounds {
    let [left, top, right, bottom] = bounds;
    let corners = [(left, top), (right, top), (right, bottom), (left, bottom)]
        .map(|(x, y)| matrix.apply(x, y));
    let xs = corners.map(|corner| corner.0);
    let ys = corners.map(|corner| corner.1);
    [
        xs.iter().copied().fold(f32::INFINITY, f32::min),
        ys.iter().copied().fold(f32::INFINITY, f32::min),
        xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    ]
}

fn join(a: Option<Bounds>, b: Option<Bounds>) -> Option<Bounds> {
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

/// Draws `clip` as it stands, with `base` taking its coordinates to pixels,
/// at `supersample` times the size, and averages the result down to `size`.
fn draw(
    clip: &ClipState,
    base: Matrix,
    size: (u32, u32),
    background: [f64; 4],
    supersample: u32,
    library: &Library,
    renderer: &mut Renderer,
) -> Result<image::RgbaImage> {
    let n = supersample.max(1);
    let scaled = Matrix::scale(n as f32, n as f32).then_inner(base);
    // A stroke is at least one pixel of the final picture wide.
    renderer.min_stroke = n as f32;
    let list = commands(clip, scaled, library);
    let large = renderer.capture(library, &list, (size.0 * n, size.1 * n), background)?;
    if n == 1 {
        return Ok(large);
    }
    let mut out = image::RgbaImage::new(size.0, size.1);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let mut sum = [0u32; 4];
        for dy in 0..n {
            for dx in 0..n {
                let sample = large.get_pixel(x * n + dx, y * n + dy);
                for channel in 0..4 {
                    sum[channel] += u32::from(sample[channel]);
                }
            }
        }
        *pixel = image::Rgba(sum.map(|total| ((total + n * n / 2) / (n * n)) as u8));
    }
    Ok(out)
}

/// How far to move a clip's picture so that it lines up with its reference.
///
/// The reference renderer sometimes crops to a larger area than `area`, by
/// rules of its own. When the sizes differ, the extra room is either all on
/// one side or shared between the two, so each of those is tried on a few
/// frames and the best fit kept.
fn align(
    clip: u16,
    frames: &BTreeMap<u16, PathBuf>,
    area: Bounds,
    library: &Library,
    renderer: &mut Renderer,
) -> Result<(f32, f32)> {
    let [left, top, right, bottom] = area;
    let numbers: Vec<u16> = frames.keys().copied().collect();
    let Some(sample) = numbers.first().and_then(|first| frames.get(first)) else {
        return Ok((0.0, 0.0));
    };
    let (width, height) = image::image_dimensions(sample)?;
    let extra = (
        width as f32 - (right - left).ceil(),
        height as f32 - (bottom - top).ceil(),
    );
    if extra.0.abs() <= 1.0 && extra.1.abs() <= 1.0 {
        return Ok((0.0, 0.0));
    }

    // The first, middle and last of the frames there are pictures for.
    let count = Stage::new(Some(clip), library).root.frame_count(library);
    let mut tried: Vec<(ClipState, image::RgbaImage)> = Vec::new();
    for index in [0, numbers.len() / 2, numbers.len() - 1] {
        let frame = numbers[index];
        if frame <= count && !tried.iter().any(|(state, _)| state.frame == frame) {
            let reference = image::open(&frames[&frame])?.to_rgba8();
            tried.push((played_to(clip, frame, library, renderer), reference));
        }
    }

    let steps = |extra: f32| [0.0, (extra / 2.0).round(), extra];
    let mut best = ((0.0, 0.0), f64::INFINITY);
    for x in steps(extra.0) {
        for y in steps(extra.1) {
            let mut score = 0.0;
            for (state, reference) in &tried {
                let list = commands(state, Matrix::translate(x - left, y - top), library);
                let ours = renderer.capture(library, &list, (width, height), [0.0; 4])?;
                let diff = compare(clip, state.frame, &ours, reference);
                score += diff.wrong + diff.mean;
            }
            if score < best.1 {
                best = ((x, y), score);
            }
        }
    }
    Ok(best.0)
}

/// Both pictures have colour multiplied by alpha after this, so that clear
/// pixels compare equal whatever colour is stored in them.
fn premultiplied(pixel: &image::Rgba<u8>, already: bool) -> [u8; 4] {
    let [r, g, b, a] = pixel.0;
    if already {
        return [r, g, b, a];
    }
    let scale = |channel: u8| ((u32::from(channel) * u32::from(a) + 127) / 255) as u8;
    [scale(r), scale(g), scale(b), a]
}

/// For every pixel, the lowest and the highest value of each channel among
/// it and the eight pixels around it.
fn neighbourhood(pixels: &[[u8; 4]], width: usize, height: usize) -> (Vec<[u8; 4]>, Vec<[u8; 4]>) {
    // Rows first, then columns: the range over a square is the range over
    // the three row-ranges that make it up.
    let across = |pick: fn(u8, u8) -> u8| {
        let mut out = pixels.to_vec();
        for y in 0..height {
            for x in 0..width {
                let at = y * width + x;
                for neighbour in [x.checked_sub(1), (x + 1 < width).then_some(x + 1)]
                    .into_iter()
                    .flatten()
                {
                    for channel in 0..4 {
                        out[at][channel] =
                            pick(out[at][channel], pixels[y * width + neighbour][channel]);
                    }
                }
            }
        }
        out
    };
    let down = |rows: &[[u8; 4]], pick: fn(u8, u8) -> u8| {
        let mut out = rows.to_vec();
        for y in 0..height {
            for neighbour in [y.checked_sub(1), (y + 1 < height).then_some(y + 1)]
                .into_iter()
                .flatten()
            {
                for x in 0..width {
                    for channel in 0..4 {
                        out[y * width + x][channel] = pick(
                            out[y * width + x][channel],
                            rows[neighbour * width + x][channel],
                        );
                    }
                }
            }
        }
        out
    };
    (
        down(&across(u8::min), u8::min),
        down(&across(u8::max), u8::max),
    )
}

/// Whether `pixel` is further than the limit from anything between `low` and
/// `high`.
fn outside(pixel: [u8; 4], low: [u8; 4], high: [u8; 4]) -> bool {
    (0..4).any(|channel| {
        pixel[channel].saturating_add(WRONG) < low[channel]
            || pixel[channel] > high[channel].saturating_add(WRONG)
    })
}

/// Marks each pixel: `None` if the pictures agree there, `Some(false)` if
/// they differ at that exact spot but each has a matching value next door,
/// and `Some(true)` if one has something the other has nothing like nearby.
///
/// The second kind is what a smoothed edge beside a hard one, or a shift of
/// under a pixel, looks like. Only the third kind is a real disagreement.
fn marks(ours: &image::RgbaImage, reference: &image::RgbaImage) -> Vec<Option<bool>> {
    let (width, height) = (ours.width() as usize, ours.height() as usize);
    let a: Vec<[u8; 4]> = ours.pixels().map(|p| premultiplied(p, true)).collect();
    let b: Vec<[u8; 4]> = reference
        .pixels()
        .map(|p| premultiplied(p, false))
        .collect();
    let (a_low, a_high) = neighbourhood(&a, width, height);
    let (b_low, b_high) = neighbourhood(&b, width, height);
    (0..a.len())
        .map(|i| {
            let differs = (0..4).any(|channel| a[i][channel].abs_diff(b[i][channel]) > WRONG);
            let wrong = outside(a[i], b_low[i], b_high[i]) || outside(b[i], a_low[i], a_high[i]);
            (differs || wrong).then_some(wrong)
        })
        .collect()
}

fn compare(clip: u16, frame: u16, ours: &image::RgbaImage, reference: &image::RgbaImage) -> Diff {
    let marks = marks(ours, reference);
    let mut total = 0u64;
    let (mut ink, mut reference_ink) = (0u64, 0u64);
    for (a, b) in ours.pixels().zip(reference.pixels()) {
        let (a, b) = (premultiplied(a, true), premultiplied(b, false));
        ink += u64::from(a[3]);
        reference_ink += u64::from(b[3]);
        total += (0..4)
            .map(|channel| u64::from(a[channel].abs_diff(b[channel])))
            .max()
            .expect("four channels");
    }
    let pixels = marks.len().max(1) as f64;
    Diff {
        clip,
        frame,
        wrong: marks.iter().filter(|mark| **mark == Some(true)).count() as f64 / pixels,
        strict: marks.iter().flatten().count() as f64 / pixels,
        mean: total as f64 / pixels / 255.0,
        width: ours.width(),
        height: ours.height(),
        ink: ink as f64 / 255.0,
        reference_ink: reference_ink as f64 / 255.0,
    }
}

/// The state of a clip on `frame`, having played there from the start.
fn played_to(clip: u16, frame: u16, library: &Library, renderer: &mut Renderer) -> ClipState {
    let mut stage = Stage::new(Some(clip), library);
    for _ in 1..frame {
        stage.advance(library, renderer);
    }
    stage.root
}

/// Draws one frame again and saves three panels side by side: the engine's
/// picture, the reference, and a map of where they differ. The first two are
/// on grey so that light and dark art shows up.
#[allow(clippy::too_many_arguments)]
fn save_pair(
    args: &Args,
    library: &Library,
    renderer: &mut Renderer,
    areas: &mut Areas,
    shift: (f32, f32),
    diff: &Diff,
    dir: &Path,
) -> Result<()> {
    let folder = args.reference.join(format!("DefineSprite_{}", diff.clip));
    let reference = image::open(folder.join(format!("{}.png", diff.frame)))?.to_rgba8();
    let (width, height) = reference.dimensions();

    let [left, top, ..] = clip_area(diff.clip, library, areas).unwrap_or([0.0; 4]);
    let root = played_to(diff.clip, diff.frame, library, renderer);
    let base = Matrix::translate(shift.0 - left, shift.1 - top);
    let list = commands(&root, base, library);
    let grey = [0.5, 0.5, 0.5, 1.0];
    let ours = renderer.capture(library, &list, (width, height), grey)?;

    // The same frame on a clear background, to find the pixels that differ.
    let clear = renderer.capture(library, &list, (width, height), [0.0; 4])?;
    let marked = marks(&clear, &reference);

    const GAP: u32 = 12;
    let mut out = image::RgbaImage::from_pixel(width * 3 + GAP * 2, height, image::Rgba([128; 4]));
    image::imageops::replace(&mut out, &ours, 0, 0);
    for (x, y, pixel) in reference.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let over = |channel: u8| ((u32::from(channel) * alpha + 128 * (255 - alpha)) / 255) as u8;
        let blended = image::Rgba([over(pixel[0]), over(pixel[1]), over(pixel[2]), 255]);
        out.put_pixel(x + width + GAP, y, blended);

        // Third panel: white where the pictures really disagree, grey
        // where they differ only in smoothing or by under a pixel.
        let shade = match marked[(y * width + x) as usize] {
            Some(true) => 255,
            Some(false) => 110,
            None => 0,
        };
        let mark = image::Rgba([shade, shade, shade, 255]);
        out.put_pixel(x + (width + GAP) * 2, y, mark);
    }
    let path = dir.join(format!("clip{}-frame{}.png", diff.clip, diff.frame));
    out.save(&path)
        .with_context(|| format!("writing {}", path.display()))
}
