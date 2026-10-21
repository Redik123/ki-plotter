//! Importers for KI Plotter. All results are in millimetres, document space
//! (y down), using the path types from [`kp_geom`].
//!
//! Every importer follows the same convention: curves are cubic `C` segments
//! (quadratics are converted exactly), lines are `L`, closed subpaths end with
//! `Z`, and the result is translated so that the bounding box of all items
//! starts at `(0, 0)`; [`Imported::width`] / [`Imported::height`] give the size
//! of that box.
//!
//! Errors are `String`s in French, meant to be shown to the user as is.

pub mod dxf;
pub mod fonts;
pub mod image;
pub mod plt;
pub mod svg;
pub mod text;
pub mod trace;
mod util;

use std::path::Path;

use kp_geom::{FillRule, PathData};
use serde::{Deserialize, Serialize};

pub use crate::dxf::import_dxf;
pub use crate::fonts::{FontInfo, list_fonts, load_font};
pub use crate::image::{ImageInfo, load_image};
pub use crate::plt::import_plt;
pub use crate::svg::import_svg;
pub use crate::text::{Align, TextOutline, TextRequest, text_outline};
pub use crate::trace::{TraceRequest, trace_image};

/// One imported object: a (possibly compound) path with its colour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedItem {
    pub path: PathData,
    /// `"#rrggbb"` when the source gives one.
    pub color: Option<String>,
    pub fill_rule: FillRule,
}

/// The result of an import, normalised so its bounding box starts at `(0, 0)`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub items: Vec<ImportedItem>,
    /// Size of the bounding box of all items, mm.
    pub width: f64,
    pub height: f64,
}

/// Imports a vector file, choosing the importer from the extension:
/// `.svg`/`.svgz`, `.dxf`, `.plt`/`.hpgl`/`.hpg`/`.hgl`.
pub fn import_file(path: &Path) -> Result<Imported, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let read = || {
        std::fs::read(path).map_err(|e| {
            format!(
                "Impossible d'ouvrir le fichier « {} » : {e}",
                path.display()
            )
        })
    };
    match ext.as_str() {
        "svg" | "svgz" => import_svg(&read()?),
        "dxf" => import_dxf(&read()?),
        "plt" | "hpgl" | "hpg" | "hgl" => import_plt(&util::decode_text(&read()?)),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff" | "webp" => Err(
            "Ce fichier est une image : utilisez la vectorisation d'image pour l'importer."
                .to_string(),
        ),
        "" => Err(
            "Format de fichier inconnu (pas d'extension). Formats acceptés : SVG, DXF, PLT/HPGL."
                .to_string(),
        ),
        _ => Err(format!(
            "Format de fichier non pris en charge (.{ext}). Formats acceptés : SVG, DXF, PLT/HPGL."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_extension() {
        let err = import_file(Path::new("dessin.ai")).unwrap_err();
        assert!(err.contains("non pris en charge"), "{err}");
        let err = import_file(Path::new("photo.PNG")).unwrap_err();
        assert!(err.contains("image"), "{err}");
        let err = import_file(Path::new("sans_extension")).unwrap_err();
        assert!(err.contains("extension"), "{err}");
    }

    #[test]
    fn missing_file() {
        let err = import_file(Path::new("Z:/n/existe/pas.svg")).unwrap_err();
        assert!(err.starts_with("Impossible d'ouvrir"), "{err}");
    }

    #[test]
    fn json_shape() {
        let item = ImportedItem {
            path: vec![kp_geom::Seg::M { x: 0.0, y: 0.0 }, kp_geom::Seg::Z],
            color: None,
            fill_rule: FillRule::Evenodd,
        };
        let json = serde_json::to_value(Imported {
            items: vec![item],
            width: 1.0,
            height: 2.0,
        })
        .unwrap();
        assert_eq!(json["items"][0]["fillRule"], "evenodd");
        assert!(json["items"][0]["color"].is_null());
        assert_eq!(json["items"][0]["path"][0]["t"], "M");
        assert_eq!(json["height"], 2.0);
    }
}
