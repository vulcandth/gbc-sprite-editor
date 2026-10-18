use std::path::Path;

/// Bound decoded PNG memory before allocating. The editor is for pixel art.
pub fn load_png(path: &Path) -> Result<image::RgbaImage, String> {
    let mut reader =
        image::ImageReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    reader.set_format(image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Cannot decode {}: {e}", path.display()))?;
    if u64::from(decoded.width()) * u64::from(decoded.height()) > 16_777_216 {
        return Err("PNG exceeds the 16 megapixel editor limit".into());
    }
    Ok(decoded.to_rgba8())
}
