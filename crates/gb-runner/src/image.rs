//! PNG loading (normalised to 0x00RRGGBB) and saving, plus framebuffer comparison.

use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::Path;

pub struct Image {
    pub width: usize,
    pub height: usize,
    /// Row-major, 0x00RRGGBB.
    pub pixels: Vec<u32>,
}

/// Decode any PNG (grayscale / palette / RGB / RGBA / gray+alpha, 1-16 bit) to RGB; alpha is dropped.
pub fn decode_png<R: Read>(reader: R) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(reader);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let data = &buf[..info.buffer_size()];
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    };
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("unexpected bit depth {:?} after expansion", info.bit_depth));
    }
    let pixels = data
        .chunks_exact(channels)
        .map(|p| {
            let (r, g, b) = if channels < 3 {
                (p[0], p[0], p[0])
            } else {
                (p[0], p[1], p[2])
            };
            (r as u32) << 16 | (g as u32) << 8 | b as u32
        })
        .collect();
    Ok(Image {
        width: info.width as usize,
        height: info.height as usize,
        pixels,
    })
}

pub fn load_png(path: &Path) -> Result<Image, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_png(std::io::BufReader::new(f)).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn encode_png_rgb(pixels: &[u32], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width as u32, height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let raw: Vec<u8> = pixels
        .iter()
        .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
        .collect();
    w.write_image_data(&raw).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn save_png(path: &Path, pixels: &[u32], width: usize, height: usize) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    let bytes = encode_png_rgb(pixels, width, height)?;
    let mut f = BufWriter::new(File::create(path).map_err(|e| format!("{}: {e}", path.display()))?);
    f.write_all(&bytes).map_err(|e| e.to_string())
}

/// Number of differing pixels (RGB only). A size mismatch counts every pixel as different.
pub fn diff_pixels(fb: &[u32], reference: &Image, width: usize, height: usize) -> usize {
    if reference.width != width || reference.height != height || fb.len() != width * height {
        return width * height;
    }
    fb.iter()
        .zip(&reference.pixels)
        .filter(|(a, b)| (**a & 0xFF_FFFF) != (**b & 0xFF_FFFF))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(
        color: png::ColorType,
        depth: png::BitDepth,
        w: u32,
        h: u32,
        data: &[u8],
        palette: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        if let Some(p) = palette {
            enc.set_palette(p.to_vec());
        }
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(data).unwrap();
        wr.finish().unwrap();
        out
    }

    #[test]
    fn rgb_roundtrip() {
        let px = [0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000, 0x123456, 0xFEDCBA];
        let bytes = encode_png_rgb(&px, 3, 2).unwrap();
        let img = decode_png(&bytes[..]).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.pixels, px);
    }

    #[test]
    fn grayscale_8bit_expands_to_rgb() {
        let bytes = encode(
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            2,
            1,
            &[0x55, 0xAA],
            None,
        );
        assert_eq!(decode_png(&bytes[..]).unwrap().pixels, [0x555555, 0xAAAAAA]);
    }

    #[test]
    fn grayscale_2bit_scales_to_full_range() {
        // 4 pixels packed in one byte: 0b00_01_10_11 -> 0, 85, 170, 255
        let bytes = encode(
            png::ColorType::Grayscale,
            png::BitDepth::Two,
            4,
            1,
            &[0b0001_1011],
            None,
        );
        assert_eq!(
            decode_png(&bytes[..]).unwrap().pixels,
            [0x000000, 0x555555, 0xAAAAAA, 0xFFFFFF]
        );
    }

    #[test]
    fn palette_expands() {
        let pal = [255, 255, 255, 0, 0, 0, 0x12, 0x34, 0x56];
        // 2-bit indices: 0,2,1
        let bytes = encode(
            png::ColorType::Indexed,
            png::BitDepth::Two,
            3,
            1,
            &[0b0010_0100],
            Some(&pal),
        );
        assert_eq!(decode_png(&bytes[..]).unwrap().pixels, [0xFFFFFF, 0x123456, 0x000000]);
    }

    #[test]
    fn rgba_drops_alpha() {
        let bytes = encode(png::ColorType::Rgba, png::BitDepth::Eight, 1, 1, &[1, 2, 3, 0], None);
        assert_eq!(decode_png(&bytes[..]).unwrap().pixels, [0x010203]);
    }

    #[test]
    fn gray_alpha_and_16bit() {
        let bytes = encode(
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            1,
            1,
            &[0xAA, 0x10],
            None,
        );
        assert_eq!(decode_png(&bytes[..]).unwrap().pixels, [0xAAAAAA]);
        let bytes = encode(
            png::ColorType::Rgb,
            png::BitDepth::Sixteen,
            1,
            1,
            &[0xFF, 0x00, 0x80, 0x00, 0x12, 0x00],
            None,
        );
        assert_eq!(decode_png(&bytes[..]).unwrap().pixels, [0xFF8012]);
    }

    #[test]
    fn diff_counts_pixels_and_ignores_high_byte() {
        let reference = Image {
            width: 2,
            height: 2,
            pixels: vec![0xFFFFFF, 0, 0x555555, 0xAAAAAA],
        };
        assert_eq!(diff_pixels(&[0xFFFFFF, 0, 0x555555, 0xAAAAAA], &reference, 2, 2), 0);
        assert_eq!(diff_pixels(&[0xFF_FFFFFF, 1, 0x555555, 0], &reference, 2, 2), 2);
        assert_eq!(diff_pixels(&[0; 4], &reference, 4, 1), 4);
    }
}
