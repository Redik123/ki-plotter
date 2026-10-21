//! SVG import through usvg.
//!
//! usvg resolves CSS, `<use>`, shapes, units (mm/cm/in → px at 96 dpi), the
//! `viewBox` and, with the system fonts, `<text>` into paths. We then take
//! every visible path node, apply its absolute transform and convert px to mm.
//! Clip paths, masks and embedded images are ignored.

use std::sync::{Arc, OnceLock};

use kp_geom::FillRule;
use usvg::tiny_skia_path::PathSegment;
use usvg::{Group, Node, Paint};

use crate::util::{self, PathBuilder, apply, hex_color};
use crate::{Imported, ImportedItem};

/// usvg works in CSS px (96 per inch).
const PX_TO_MM: f64 = 25.4 / 96.0;

/// The system fonts, loaded once per process (only for SVGs with text).
fn system_fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// Cheap check before paying for the font database: `<text` or `:text`
/// somewhere, or compressed (svgz) data we cannot peek into.
fn may_contain_text(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
        || bytes
            .windows(5)
            .any(|w| (w[0] == b'<' || w[0] == b':') && &w[1..] == b"text")
}

/// Imports an SVG (or gzip-compressed SVGZ) document.
pub fn import_svg(bytes: &[u8]) -> Result<Imported, String> {
    let items = svg_items(bytes)?;
    let imported = util::normalize(items);
    if imported.items.is_empty() {
        return Err("Le fichier SVG ne contient aucun tracé.".to_string());
    }
    Ok(imported)
}

/// The paths of an SVG document in mm (not normalised).
pub(crate) fn svg_items(bytes: &[u8]) -> Result<Vec<ImportedItem>, String> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err("Impossible de lire le fichier SVG : le fichier est vide.".to_string());
    }
    util::guard(
        "Impossible de lire le fichier SVG : fichier invalide ou corrompu.",
        || {
            let mut opt = usvg::Options::default();
            if may_contain_text(bytes) {
                opt.fontdb = system_fontdb();
            }
            let tree = usvg::Tree::from_data(bytes, &opt)
                .map_err(|e| format!("Impossible de lire le fichier SVG : {e}"))?;
            let mut items = Vec::new();
            collect(tree.root(), &mut items);
            Ok(items)
        },
    )
}

fn collect(group: &Group, items: &mut Vec<ImportedItem>) {
    for node in group.children() {
        match node {
            Node::Group(g) => collect(g, items),
            Node::Path(p) => {
                if let Some(item) = convert_path(p) {
                    items.push(item);
                }
            }
            Node::Text(t) => collect(t.flattened(), items),
            Node::Image(_) => {}
        }
    }
}

fn convert_path(p: &usvg::Path) -> Option<ImportedItem> {
    if !p.is_visible() || (p.fill().is_none() && p.stroke().is_none()) {
        return None;
    }
    let t = p.abs_transform();
    let m = util::mul(
        &util::scale(PX_TO_MM, PX_TO_MM),
        &[
            t.sx as f64,
            t.ky as f64,
            t.kx as f64,
            t.sy as f64,
            t.tx as f64,
            t.ty as f64,
        ],
    );
    let pt = |q: usvg::tiny_skia_path::Point| apply(&m, q.x as f64, q.y as f64);
    let mut b = PathBuilder::new();
    for seg in p.data().segments() {
        match seg {
            PathSegment::MoveTo(a) => {
                let (x, y) = pt(a);
                b.move_to(x, y);
            }
            PathSegment::LineTo(a) => {
                let (x, y) = pt(a);
                b.line_to(x, y);
            }
            PathSegment::QuadTo(c, a) => {
                let (x1, y1) = pt(c);
                let (x, y) = pt(a);
                b.quad_to(x1, y1, x, y);
            }
            PathSegment::CubicTo(c1, c2, a) => {
                let (x1, y1) = pt(c1);
                let (x2, y2) = pt(c2);
                let (x, y) = pt(a);
                b.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathSegment::Close => b.close(),
        }
    }
    let path = b.finish();
    if path.is_empty() {
        return None;
    }
    let color = p
        .fill()
        .and_then(|f| paint_color(f.paint()))
        .or_else(|| p.stroke().and_then(|s| paint_color(s.paint())));
    let fill_rule = match p.fill().map(|f| f.rule()) {
        Some(usvg::FillRule::EvenOdd) => FillRule::Evenodd,
        _ => FillRule::Nonzero,
    };
    Some(ImportedItem {
        path,
        color,
        fill_rule,
    })
}

/// Flat colour, or the first stop of a gradient.
fn paint_color(paint: &Paint) -> Option<String> {
    let c = match paint {
        Paint::Color(c) => *c,
        Paint::LinearGradient(g) => g.stops().first()?.color(),
        Paint::RadialGradient(g) => g.stops().first()?.color(),
        Paint::Pattern(_) => return None,
    };
    Some(hex_color(c.red, c.green, c.blue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{count_closed, count_subpaths, path_bbox};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn rect_in_mm() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50"><rect x="10" y="10" width="20" height="10"/></svg>"#;
        let imp = import_svg(svg).unwrap();
        assert_eq!(imp.items.len(), 1);
        let item = &imp.items[0];
        assert_eq!(count_subpaths(&item.path), 1);
        assert_eq!(count_closed(&item.path), 1);
        assert!(close(imp.width, 20.0) && close(imp.height, 10.0), "{imp:?}");
        let b = path_bbox(&item.path).unwrap();
        assert!(close(b[0], 0.0) && close(b[1], 0.0));
        assert_eq!(item.color.as_deref(), Some("#000000"));
        assert_eq!(item.fill_rule, FillRule::Nonzero);
    }

    #[test]
    fn groups_and_transforms() {
        // Rect (5×5 scaled ×2, at 10,5) → 10..20 × 5..15; circle → 75..85 × 35..45.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
            <g transform="translate(10,5)"><g transform="scale(2)">
              <rect width="5" height="5" fill="#ff8000" fill-rule="evenodd"/>
            </g></g>
            <circle cx="80" cy="40" r="5" fill="none" stroke="#0000ff"/>
            <rect x="0" y="0" width="1" height="1" fill="none" stroke="none"/>
            <rect x="0" y="0" width="1" height="1" visibility="hidden"/>
        </svg>"##;
        let imp = import_svg(svg).unwrap();
        assert_eq!(imp.items.len(), 2);
        assert!(close(imp.width, 75.0) && close(imp.height, 40.0), "{imp:?}");
        let rect = path_bbox(&imp.items[0].path).unwrap();
        assert!(
            close(rect[2] - rect[0], 10.0) && close(rect[0], 0.0),
            "{rect:?}"
        );
        assert_eq!(imp.items[0].color.as_deref(), Some("#ff8000"));
        assert_eq!(imp.items[0].fill_rule, FillRule::Evenodd);
        let circle = &imp.items[1];
        assert_eq!(circle.color.as_deref(), Some("#0000ff"));
        assert!(
            circle
                .path
                .iter()
                .any(|s| matches!(s, kp_geom::Seg::C { .. }))
        );
        let cb = path_bbox(&circle.path).unwrap();
        assert!(
            close(cb[0], 65.0) && close(cb[2], 75.0) && close(cb[3], 40.0),
            "{cb:?}"
        );
    }

    #[test]
    fn px_and_inches() {
        // No unit = px at 96 dpi: 96 px = 1 in = 25.4 mm.
        let imp = import_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 H96 V48 Z"/></svg>"#,
        )
        .unwrap();
        assert!(close(imp.width, 25.4) && close(imp.height, 12.7), "{imp:?}");
        let imp = import_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="2in" height="1in" viewBox="0 0 2 1"><path d="M0 0 Q1 1 2 0"/></svg>"#,
        )
        .unwrap();
        assert!(close(imp.width, 50.8), "{imp:?}");
    }

    #[test]
    fn text_becomes_paths() {
        if !std::path::Path::new(r"C:\Windows\Fonts\arial.ttf").exists() {
            return;
        }
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200mm" height="50mm" viewBox="0 0 200 50">
            <text x="10" y="40" font-family="Arial" font-size="40" fill="#123456">HO</text></svg>"##;
        let imp = import_svg(svg).unwrap();
        let subpaths: usize = imp.items.iter().map(|i| count_subpaths(&i.path)).sum();
        assert_eq!(subpaths, 3, "{imp:?}"); // H + O (outer, inner)
        assert!((imp.height - 40.0 * 0.716).abs() < 1.5, "{}", imp.height);
        assert_eq!(imp.items[0].color.as_deref(), Some("#123456"));
    }

    #[test]
    fn bad_input() {
        assert!(import_svg(b"").unwrap_err().contains("vide"));
        assert!(import_svg(b"<svg").is_err());
        assert!(import_svg(b"\x1f\x8b\x08garbage").is_err());
        assert!(import_svg(b"not xml at all").is_err());
        let empty =
            import_svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#);
        assert!(empty.unwrap_err().contains("aucun tracé"));
    }
}
