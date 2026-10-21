//! Bitmap tracing (black-and-white), with the vtracer engine.
//!
//! This is vtracer's binary mode (`binary_image_to_svg`) written directly
//! against `visioncortex`, the library vtracer is a thin layer over, so we get
//! the compound paths without an SVG round-trip and without vtracer's CLI
//! dependencies (clap 2, image 0.23 with every codec).
//!
//! One difference: `despeckle` is applied before clustering, to both colours
//! (specks are removed *and* pinholes are filled, both 4-connected), whereas
//! vtracer only drops small shapes after clustering. Tiny holes are a pain to
//! weed, and filtering first also keeps noisy scans under visioncortex's
//! 65 535-cluster limit.
//!
//! Output: one item, fill rule evenodd. Each connected shape gives its outer
//! contour plus one subpath per hole; islands inside holes are separate
//! shapes. (visioncortex also orients holes opposite to outlines, so nonzero
//! would render the same.)

use std::path::Path;

use ::image::DynamicImage;
use kp_geom::FillRule;
use serde::{Deserialize, Serialize};
use visioncortex::{BinaryImage, CompoundPathElement, PathSimplifyMode};

use crate::util::{self, PathBuilder};
use crate::{Imported, ImportedItem};

/// Images are traced at most this size (longest side, px); larger ones are
/// downscaled first, which keeps tracing time and memory reasonable.
const TRACE_MAX: u32 = 4096;
/// vtracer defaults for the remaining parameters.
const LENGTH_THRESHOLD: f64 = 4.0;
const MAX_ITERATIONS: usize = 10;
const SPLICE_THRESHOLD_DEG: f64 = 45.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    /// Image file.
    pub path: String,
    /// 0–255: pixels darker than this are foreground.
    pub threshold: f64,
    /// Swap foreground and background.
    #[serde(default)]
    pub invert: bool,
    /// Foreground = opaque pixels (alpha ≥ 128), whatever their colour.
    #[serde(default)]
    pub use_alpha: bool,
    /// Shapes and holes smaller than `despeckle × despeckle` px are removed
    /// (like vtracer's `filter_speckle`, see the module docs).
    #[serde(default)]
    pub despeckle: f64,
    /// Angle (degrees) above which a direction change is kept as a corner.
    pub corner_threshold: f64,
    /// Width given to the whole image (not to the traced shapes), mm.
    pub width_mm: f64,
}

/// Traces the image at `req.path`. The scale is `widthMm / image width`, so
/// shapes keep their place within the picture; the result is then normalised
/// to `(0, 0)` like every import. No shape found gives an empty result.
pub fn trace_image(req: &TraceRequest) -> Result<Imported, String> {
    if !(req.width_mm.is_finite() && req.width_mm > 0.0) {
        return Err("Largeur de vectorisation invalide.".to_string());
    }
    let (img, _) = crate::image::read_image(Path::new(&req.path))?;
    trace(img, req)
}

fn trace(img: DynamicImage, req: &TraceRequest) -> Result<Imported, String> {
    let img = if img.width() > TRACE_MAX || img.height() > TRACE_MAX {
        img.resize(
            TRACE_MAX,
            TRACE_MAX,
            ::image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    // The whole picture spans width_mm (whatever the downscaling).
    let mm_per_px = req.width_mm / w as f64;

    let threshold = if req.threshold.is_finite() {
        req.threshold.clamp(0.0, 256.0)
    } else {
        128.0
    };
    let mut bits: Vec<bool> = rgba
        .pixels()
        .map(|px| {
            let [r, g, b, a] = px.0;
            let fg = if req.use_alpha {
                a >= 128
            } else {
                // Over white, then Rec. 601 luma.
                let a = a as f64 / 255.0;
                let over = |c: u8| c as f64 * a + 255.0 * (1.0 - a);
                0.299 * over(r) + 0.587 * over(g) + 0.114 * over(b) < threshold
            };
            fg != req.invert
        })
        .collect();
    drop(rgba);

    let speckle = if req.despeckle.is_finite() {
        req.despeckle.clamp(0.0, 1000.0).round() as usize
    } else {
        0
    };
    let min_area = speckle * speckle;
    let corner = if req.corner_threshold.is_finite() {
        req.corner_threshold.clamp(0.0, 180.0)
    } else {
        60.0
    };
    remove_small_regions(&mut bits, w, h, true, min_area);
    remove_small_regions(&mut bits, w, h, false, min_area);
    let mut bin = BinaryImage::new_w_h(w, h);
    for (i, _) in bits.iter().enumerate().filter(|(_, b)| **b) {
        bin.set_pixel_index(i, true);
    }
    drop(bits);

    let path = util::guard(
        "Impossible de vectoriser l'image : elle contient trop de taches. Augmentez le nettoyage ou changez le seuil.",
        || {
            let clusters = bin.to_clusters(false);
            let mut b = PathBuilder::new();
            for cluster in clusters.iter() {
                if cluster.size() < min_area.max(1) {
                    continue;
                }
                let compound = cluster.to_compound_path(
                    PathSimplifyMode::Spline,
                    corner.to_radians(),
                    LENGTH_THRESHOLD,
                    MAX_ITERATIONS,
                    SPLICE_THRESHOLD_DEG.to_radians(),
                );
                for element in compound.iter() {
                    add_element(&mut b, element, mm_per_px);
                }
            }
            Ok(b.finish())
        },
    )?;
    Ok(util::normalize(vec![ImportedItem {
        path,
        color: None,
        fill_rule: FillRule::Evenodd,
    }]))
}

/// Flips every 4-connected region of `value` pixels smaller than `min_area`.
fn remove_small_regions(bits: &mut [bool], w: usize, h: usize, value: bool, min_area: usize) {
    if min_area <= 1 {
        return;
    }
    let mut seen = vec![false; bits.len()];
    let mut stack = Vec::new();
    let mut region = Vec::new();
    for start in 0..bits.len() {
        if seen[start] || bits[start] != value {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        region.clear();
        let mut small = true;
        while let Some(i) = stack.pop() {
            if small {
                region.push(i);
                if region.len() >= min_area {
                    small = false;
                    region.clear();
                }
            }
            let (x, y) = (i % w, i / w);
            let mut visit = |n: usize| {
                if !seen[n] && bits[n] == value {
                    seen[n] = true;
                    stack.push(n);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if small {
            for &i in &region {
                bits[i] = !value;
            }
        }
    }
}

fn add_element(b: &mut PathBuilder, element: &CompoundPathElement, k: f64) {
    match element {
        CompoundPathElement::Spline(s) => {
            // 1 + 3n points: start, then (c1, c2, end) per cubic.
            let pts = &s.points;
            if pts.len() < 4 {
                return;
            }
            b.move_to(pts[0].x * k, pts[0].y * k);
            for [c1, c2, end] in pts[1..].as_chunks::<3>().0 {
                b.cubic_to(c1.x * k, c1.y * k, c2.x * k, c2.y * k, end.x * k, end.y * k);
            }
            b.close();
        }
        CompoundPathElement::PathI32(p) => {
            polygon(b, p.path.iter().map(|q| (q.x as f64 * k, q.y as f64 * k)));
        }
        CompoundPathElement::PathF64(p) => {
            polygon(b, p.path.iter().map(|q| (q.x * k, q.y * k)));
        }
    }
}

fn polygon(b: &mut PathBuilder, mut pts: impl Iterator<Item = (f64, f64)>) {
    let Some((x, y)) = pts.next() else { return };
    b.move_to(x, y);
    for (x, y) in pts {
        b.line_to(x, y);
    }
    b.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{count_closed, count_subpaths, path_bbox};
    use ::image::{Rgba, RgbaImage};

    fn ring() -> RgbaImage {
        RgbaImage::from_fn(200, 200, |x, y| {
            let d = ((x as f64 + 0.5 - 100.0).powi(2) + (y as f64 + 0.5 - 100.0).powi(2)).sqrt();
            if (40.0..=80.0).contains(&d) {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        })
    }

    fn req(path: &str) -> TraceRequest {
        TraceRequest {
            path: path.to_string(),
            threshold: 128.0,
            invert: false,
            use_alpha: false,
            despeckle: 2.0,
            corner_threshold: 60.0,
            width_mm: 100.0,
        }
    }

    #[test]
    fn ring_keeps_its_hole() {
        let path = std::env::temp_dir().join(format!("kp_import_{}_ring.png", std::process::id()));
        ring().save(&path).unwrap();
        let result = trace_image(&req(path.to_str().unwrap()));
        std::fs::remove_file(&path).ok();
        let imp = result.unwrap();
        assert_eq!(imp.items.len(), 1);
        let item = &imp.items[0];
        assert_eq!(item.fill_rule, FillRule::Evenodd);
        assert_eq!(count_subpaths(&item.path), 2, "{:?}", item.path);
        assert_eq!(count_closed(&item.path), 2);
        // 160 px of a 200 px image traced at 100 mm → 80 mm.
        assert!(
            (imp.width - 80.0).abs() < 2.0 && (imp.height - 80.0).abs() < 2.0,
            "{imp:?}"
        );
        // The hole: second subpath ≈ 40 mm wide.
        let i = item
            .path
            .iter()
            .rposition(|s| matches!(s, kp_geom::Seg::M { .. }))
            .unwrap();
        let hole = path_bbox(&item.path[i..].to_vec()).unwrap();
        assert!((hole[2] - hole[0] - 40.0).abs() < 2.0, "{hole:?}");
    }

    #[test]
    fn invert_alpha_and_speckles() {
        let mut img = ring();
        // A 2×2 speck and a 2×2 pinhole in the ring, removed with despeckle = 3.
        for (x, y) in [(5, 5), (5, 6), (6, 5), (6, 6)] {
            img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            img.put_pixel(x + 35, y + 95, Rgba([255, 255, 255, 255]));
        }
        let mut r = req("");
        r.despeckle = 3.0;
        let imp = trace(DynamicImage::ImageRgba8(img.clone()), &r).unwrap();
        assert_eq!(count_subpaths(&imp.items[0].path), 2);
        r.despeckle = 0.0;
        let imp = trace(DynamicImage::ImageRgba8(img.clone()), &r).unwrap();
        assert_eq!(count_subpaths(&imp.items[0].path), 4);

        // Inverted: the white outside and the white disc become the shapes.
        r.despeckle = 3.0;
        r.invert = true;
        let imp = trace(DynamicImage::ImageRgba8(img.clone()), &r).unwrap();
        assert!((imp.width - 100.0).abs() < 1.0, "{}", imp.width);

        // Alpha: transparent background, colour ignored.
        let alpha = RgbaImage::from_fn(100, 100, |x, y| {
            if (20..60).contains(&x) && (30..50).contains(&y) {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let mut r = req("");
        r.use_alpha = true;
        let imp = trace(DynamicImage::ImageRgba8(alpha), &r).unwrap();
        assert_eq!(count_subpaths(&imp.items[0].path), 1);
        assert!(
            (imp.width - 40.0).abs() < 1.0 && (imp.height - 20.0).abs() < 1.0,
            "{imp:?}"
        );

        // Nothing dark: empty result, not an error.
        let blank = RgbaImage::from_pixel(50, 50, Rgba([255, 255, 255, 255]));
        let imp = trace(DynamicImage::ImageRgba8(blank), &req("")).unwrap();
        assert!(imp.items.is_empty() && imp.width == 0.0);
    }

    #[test]
    fn bad_requests() {
        let mut r = req("Z:/nope.png");
        assert!(
            trace_image(&r)
                .unwrap_err()
                .starts_with("Impossible d'ouvrir")
        );
        r.width_mm = 0.0;
        assert!(trace_image(&r).unwrap_err().contains("Largeur"));
        let json: TraceRequest = serde_json::from_str(
            r#"{"path":"a.png","threshold":128,"invert":false,"useAlpha":true,"despeckle":4,"cornerThreshold":60,"widthMm":120}"#,
        )
        .unwrap();
        assert!(json.use_alpha && json.width_mm == 120.0);
    }
}
