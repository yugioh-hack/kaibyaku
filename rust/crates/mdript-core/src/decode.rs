use image::{DynamicImage, Rgb, RgbImage};
use std::{
    fs,
    io::{BufRead, BufReader, Cursor, Seek},
    path::Path,
};

/// Bounded decoder shared by the hash command and the identification pipeline.
/// JPEG pixel differences from Pillow are tracked in the compatibility report.
pub fn load_rgb(path: &Path) -> Result<RgbImage, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
        return Err("compressed image exceeds 512 MiB".into());
    }
    decode(image::ImageReader::new(BufReader::new(file)))
}

/// Decode an in-memory official image using exactly the local-file limits and conversion.
pub fn load_rgb_bytes(bytes: &[u8]) -> Result<RgbImage, String> {
    if bytes.len() as u64 > 512 * 1024 * 1024 {
        return Err("compressed image exceeds 512 MiB".into());
    }
    decode(image::ImageReader::new(Cursor::new(bytes)))
}

fn decode<R: BufRead + Seek>(reader: image::ImageReader<R>) -> Result<RgbImage, String> {
    let mut reader = reader.with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| e.to_string())?;
    if u64::from(decoded.width()) * u64::from(decoded.height()) > 64_000_000 {
        return Err("image exceeds 64 million pixels".into());
    }
    // Pillow opens 16-bit grayscale PNG as I;16 and clamps on convert("RGB").
    // It opens 16-bit LA/RGB/RGBA PNG as 8-bit RGBA/RGB, dropping the low byte.
    let rgb = match decoded {
        DynamicImage::ImageLuma16(im) => RgbImage::from_fn(im.width(), im.height(), |x, y| {
            let v = im.get_pixel(x, y)[0].min(255) as u8;
            Rgb([v, v, v])
        }),
        DynamicImage::ImageLumaA16(im) => RgbImage::from_fn(im.width(), im.height(), |x, y| {
            let v = (im.get_pixel(x, y)[0] >> 8) as u8;
            Rgb([v, v, v])
        }),
        DynamicImage::ImageRgb16(im) => RgbImage::from_fn(im.width(), im.height(), |x, y| {
            let p = im.get_pixel(x, y);
            Rgb([(p[0] >> 8) as u8, (p[1] >> 8) as u8, (p[2] >> 8) as u8])
        }),
        DynamicImage::ImageRgba16(im) => RgbImage::from_fn(im.width(), im.height(), |x, y| {
            let p = im.get_pixel(x, y);
            Rgb([(p[0] >> 8) as u8, (p[1] >> 8) as u8, (p[2] >> 8) as u8])
        }),
        other => other.to_rgb8(),
    };
    Ok(rgb)
}
