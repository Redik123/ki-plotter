//! Bitmap loading for the trace dialog preview.

use std::io::Cursor;
use std::path::Path;

use ::image::{DynamicImage, ImageFormat, ImageReader};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Longest side of the preview sent to the frontend, px.
const PREVIEW_MAX: u32 = 1600;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    /// `data:image/png;base64,…`, downscaled to fit 1600 px.
    pub data_url: String,
    /// Size of the original image, px.
    pub width: u32,
    pub height: u32,
    /// Resolution stored in the file (PNG pHYs, JPEG JFIF, BMP header), if any.
    pub dpi: Option<f64>,
}

/// Reads and decodes an image file (PNG, JPEG, BMP, GIF).
pub(crate) fn read_image(path: &Path) -> Result<(DynamicImage, Vec<u8>), String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Impossible d'ouvrir l'image « {} » : {e}", path.display()))?;
    let img = crate::util::guard(
        "Impossible de lire l'image : fichier invalide ou corrompu.",
        || {
            ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|e| format!("Impossible de lire l'image : {e}"))?
                .decode()
                .map_err(|e| format!("Impossible de lire l'image : {e}"))
        },
    )?;
    if img.width() == 0 || img.height() == 0 {
        return Err("Impossible de lire l'image : elle est vide.".to_string());
    }
    Ok((img, bytes))
}

/// Loads an image for preview: a PNG data URL (at most 1600 px on its longest
/// side), the original size and the stored resolution.
pub fn load_image(path: &Path) -> Result<ImageInfo, String> {
    let (img, bytes) = read_image(path)?;
    let (width, height) = (img.width(), img.height());
    let preview = if width > PREVIEW_MAX || height > PREVIEW_MAX {
        img.thumbnail(PREVIEW_MAX, PREVIEW_MAX)
    } else {
        img
    };
    // 8-bit keeps the data URL small; alpha only when there is some.
    let preview = if preview.color().has_alpha() {
        DynamicImage::ImageRgba8(preview.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(preview.to_rgb8())
    };
    let mut png = Vec::new();
    preview
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("Impossible de préparer l'aperçu de l'image : {e}"))?;
    Ok(ImageInfo {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        ),
        width,
        height,
        dpi: file_dpi(&bytes),
    })
}

/// Horizontal resolution from the file headers, rounded to 0.1 dpi.
fn file_dpi(bytes: &[u8]) -> Option<f64> {
    let dpi = png_dpi(bytes)
        .or_else(|| jpeg_dpi(bytes))
        .or_else(|| bmp_dpi(bytes))?;
    (dpi.is_finite() && dpi >= 1.0).then(|| (dpi * 10.0).round() / 10.0)
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// PNG `pHYs` chunk, when its unit is the metre.
fn png_dpi(bytes: &[u8]) -> Option<f64> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut i = 8;
    while i + 8 <= bytes.len() {
        let len = be_u32(&bytes[i..]) as usize;
        let kind = &bytes[i + 4..i + 8];
        let data = bytes.get(i + 8..i + 8 + len)?;
        match kind {
            b"pHYs" if len >= 9 => {
                return (data[8] == 1).then(|| be_u32(data) as f64 * 0.0254);
            }
            b"IDAT" | b"IEND" => return None,
            _ => {}
        }
        i += 12 + len;
    }
    None
}

/// JPEG JFIF `APP0` density, when given in dots per inch or per cm.
fn jpeg_dpi(bytes: &[u8]) -> Option<f64> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() && bytes[i] == 0xff {
        let marker = bytes[i + 1];
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if marker == 0xe0 {
            let seg = bytes.get(i + 4..i + 2 + len)?;
            if seg.len() >= 12 && seg.starts_with(b"JFIF\0") {
                let x = u16::from_be_bytes([seg[8], seg[9]]) as f64;
                return match seg[7] {
                    1 => Some(x),
                    2 => Some(x * 2.54),
                    _ => None,
                };
            }
        }
        // Stop at start of scan; skip other segments.
        if marker == 0xda || len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

/// BMP `BITMAPINFOHEADER` horizontal pixels per metre.
fn bmp_dpi(bytes: &[u8]) -> Option<f64> {
    if !bytes.starts_with(b"BM") || bytes.len() < 42 {
        return None;
    }
    let ppm = i32::from_le_bytes([bytes[38], bytes[39], bytes[40], bytes[41]]);
    (ppm > 0).then_some(ppm as f64 * 0.0254)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::{Rgb, RgbImage};

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kp_import_{}_{name}", std::process::id()))
    }

    #[test]
    fn preview_and_dpi() {
        let path = temp("big.png");
        let img = RgbImage::from_pixel(2000, 1000, Rgb([255, 0, 0]));
        img.save(&path).unwrap();
        // Add a pHYs chunk (300 dpi = 11811 px/m) right after IHDR.
        let mut bytes = std::fs::read(&path).unwrap();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&9u32.to_be_bytes());
        chunk.extend_from_slice(b"pHYs");
        chunk.extend_from_slice(&11811u32.to_be_bytes());
        chunk.extend_from_slice(&11811u32.to_be_bytes());
        chunk.push(1);
        chunk.extend_from_slice(&[0, 0, 0, 0]); // CRC (not checked by us)
        bytes.splice(33..33, chunk);
        assert_eq!(file_dpi(&bytes), Some(300.0));

        let info = load_image(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!((info.width, info.height), (2000, 1000));
        assert!(info.data_url.starts_with("data:image/png;base64,"));
        let png = base64::engine::general_purpose::STANDARD
            .decode(&info.data_url["data:image/png;base64,".len()..])
            .unwrap();
        let preview = ::image::load_from_memory(&png).unwrap();
        assert_eq!((preview.width(), preview.height()), (1600, 800));
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("dataUrl").is_some() && json.get("dpi").is_some());
    }

    #[test]
    fn jpeg_and_bmp_dpi() {
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16];
        jpeg.extend_from_slice(b"JFIF\0\x01\x02\x01\x00\x48\x00\x48\x00\x00");
        assert_eq!(file_dpi(&jpeg), Some(72.0));
        let mut bmp = vec![0u8; 54];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[38..42].copy_from_slice(&3780i32.to_le_bytes());
        assert_eq!(file_dpi(&bmp), Some(96.0));
        assert_eq!(file_dpi(b"GIF89a"), None);
    }

    #[test]
    fn bad_files() {
        assert!(
            load_image(Path::new("Z:/nope.png"))
                .unwrap_err()
                .starts_with("Impossible d'ouvrir")
        );
        let path = temp("broken.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        let err = load_image(&path).unwrap_err();
        std::fs::write(&path, b"").unwrap();
        let err2 = load_image(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(err.starts_with("Impossible de lire l'image"), "{err}");
        assert!(err2.starts_with("Impossible de lire l'image"), "{err2}");
    }
}
