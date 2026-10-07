//! Decodes the file's bitmaps to plain RGBA.

use std::io::Read;

use anyhow::{Context, Result, bail, ensure};
use flate2::read::ZlibDecoder;
use swf::{BitmapFormat, DefineBitsLossless};

/// Pixels in RGBA order, with colour not multiplied by alpha.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes a JPEG (or, in later tags, PNG or GIF) image.
///
/// `tables` is the file's shared JPEG header, used by the oldest tag only.
/// `alpha` is a zlib-compressed plane of one alpha byte per pixel.
pub fn decode_jpeg(data: &[u8], tables: Option<&[u8]>, alpha: Option<&[u8]>) -> Result<Bitmap> {
    let mut data = strip_stray_markers(data);
    if let Some(tables) = tables.filter(|tables| !tables.is_empty()) {
        // Join the two streams: drop the header's end marker and the image's
        // start marker.
        let tables = strip_stray_markers(tables);
        let head = tables.strip_suffix(&[0xFF, 0xD9]).unwrap_or(&tables);
        let body = data.strip_prefix(&[0xFF, 0xD8]).unwrap_or(&data);
        data = [head, body].concat();
    }

    let image = image::load_from_memory(&data)
        .context("decoding image data")?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut rgba = image.into_raw();

    if let Some(alpha) = alpha.filter(|alpha| !alpha.is_empty()) {
        let alpha = inflate(alpha).context("inflating the alpha plane")?;
        ensure!(
            alpha.len() == (width * height) as usize,
            "alpha plane has {} bytes for a {width}x{height} image",
            alpha.len()
        );
        for (pixel, &a) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&alpha) {
            // The colour is stored already multiplied by alpha.
            unmultiply(pixel, a);
        }
    }
    Ok(Bitmap {
        width,
        height,
        rgba,
    })
}

/// Old authoring tools wrote an end marker followed by a start marker at the
/// front of the data, or between the tables and the image. Flash Player
/// skips the pair, and decoders need it gone.
fn strip_stray_markers(data: &[u8]) -> Vec<u8> {
    const STRAY: [u8; 4] = [0xFF, 0xD9, 0xFF, 0xD8];
    let data = data.strip_prefix(&STRAY).unwrap_or(data);
    match data.windows(4).position(|window| window == STRAY) {
        Some(at) => [&data[..at], &data[at + 4..]].concat(),
        None => data.to_vec(),
    }
}

pub fn decode_lossless(bitmap: &DefineBitsLossless) -> Result<Bitmap> {
    let width = usize::from(bitmap.width);
    let height = usize::from(bitmap.height);
    let has_alpha = bitmap.version == 2;
    let data = inflate(&bitmap.data).context("inflating pixel data")?;
    let mut rgba = Vec::with_capacity(width * height * 4);

    match bitmap.format {
        BitmapFormat::Rgb32 => {
            ensure!(data.len() >= width * height * 4, "pixel data is too short");
            for pixel in data.as_chunks::<4>().0.iter().take(width * height) {
                // Stored as alpha (or padding), red, green, blue.
                let mut out = [pixel[1], pixel[2], pixel[3], 255];
                if has_alpha {
                    unmultiply(&mut out, pixel[0]);
                }
                rgba.extend_from_slice(&out);
            }
        }
        BitmapFormat::ColorMap8 { num_colors } => {
            // The file stores the palette size minus one.
            let colors = usize::from(num_colors) + 1;
            let entry = if has_alpha { 4 } else { 3 };
            // Each row is padded to a multiple of four bytes.
            let stride = width.div_ceil(4) * 4;
            ensure!(
                data.len() >= colors * entry + stride * height,
                "pixel data is too short"
            );
            let (palette, pixels) = data.split_at(colors * entry);
            for row in pixels.chunks_exact(stride).take(height) {
                for &index in &row[..width] {
                    let mut out = [0, 0, 0, 0];
                    if let Some(color) = palette.chunks_exact(entry).nth(usize::from(index)) {
                        out = [color[0], color[1], color[2], 255];
                        if has_alpha {
                            unmultiply(&mut out, color[3]);
                        }
                    }
                    rgba.extend_from_slice(&out);
                }
            }
        }
        BitmapFormat::Rgb15 => bail!("15-bit bitmaps are not supported"),
    }

    Ok(Bitmap {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

/// Sets a pixel's alpha, undoing the multiplication of its colour by alpha.
fn unmultiply(pixel: &mut [u8], alpha: u8) {
    pixel[3] = alpha;
    if alpha == 0 || alpha == 255 {
        return;
    }
    for channel in &mut pixel[..3] {
        let value = (u32::from(*channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha);
        *channel = value.min(255) as u8;
    }
}

fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ZlibDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn lossless(version: u8, format: BitmapFormat, width: u16, height: u16, raw: &[u8]) -> Bitmap {
        decode_lossless(&DefineBitsLossless {
            version,
            id: 1,
            format,
            width,
            height,
            data: Cow::Owned(deflate(raw)),
        })
        .unwrap()
    }

    #[test]
    fn unmultiply_restores_the_original_colour() {
        // Half-transparent pure red is stored as (128, 0, 0) with alpha 128.
        let mut pixel = [128, 0, 0, 0];
        unmultiply(&mut pixel, 128);
        assert_eq!(pixel, [255, 0, 0, 128]);
    }

    #[test]
    fn rgb32_without_alpha_ignores_the_padding_byte() {
        let bitmap = lossless(1, BitmapFormat::Rgb32, 1, 1, &[0, 10, 20, 30]);
        assert_eq!(bitmap.rgba, [10, 20, 30, 255]);
    }

    #[test]
    fn rgb32_with_alpha_reads_alpha_first() {
        let bitmap = lossless(2, BitmapFormat::Rgb32, 1, 1, &[128, 128, 0, 0]);
        assert_eq!(bitmap.rgba, [255, 0, 0, 128]);
    }

    #[test]
    fn palette_rows_are_padded_to_four_bytes() {
        // Two colours, then two rows of three pixels, each padded to four.
        let raw = [
            255, 0, 0, // colour 0: red
            0, 0, 255, // colour 1: blue
            0, 1, 0, 99, // row 0
            1, 0, 1, 99, // row 1
        ];
        let bitmap = lossless(1, BitmapFormat::ColorMap8 { num_colors: 1 }, 3, 2, &raw);
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        assert_eq!(bitmap.rgba, [red, blue, red, blue, red, blue].concat());
    }

    #[test]
    fn stray_markers_are_removed_from_the_front_and_the_middle() {
        assert_eq!(strip_stray_markers(&[0xFF, 0xD9, 0xFF, 0xD8, 1, 2]), [1, 2]);
        assert_eq!(strip_stray_markers(&[1, 0xFF, 0xD9, 0xFF, 0xD8, 2]), [1, 2]);
        assert_eq!(strip_stray_markers(&[1, 2]), [1, 2]);
    }
}
