use image::RgbImage;
use std::{fs, io::BufReader, path::Path};

/// Bounded decoder shared by the hash command and the identification pipeline.
/// JPEG pixel differences from Pillow are tracked in the compatibility report.
pub fn load_rgb(path: &Path) -> Result<RgbImage, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
        return Err("compressed image exceeds 512 MiB".into());
    }
    let mut reader = image::ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| e.to_string())?;
    if u64::from(decoded.width()) * u64::from(decoded.height()) > 64_000_000 {
        return Err("image exceeds 64 million pixels".into());
    }
    Ok(decoded.to_rgb8())
}
