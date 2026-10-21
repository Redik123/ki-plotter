//! Tiling (`make_tiles`): split a design too large for the media into
//! overlapping panels.

use kurbo::Point;
use serde::{Deserialize, Serialize};

use crate::flatten::{DEFAULT_TOLERANCE, Polyline, flatten_shape};
use crate::num::{de_f64, non_negative};
use crate::path::{PathData, Seg, Shape};
use crate::rect::Rect;
use crate::transform::shapes_bounds;

/// Maximum number of tiles.
pub const MAX_TILES: usize = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TileRequest {
    pub shapes: Vec<Shape>,
    /// Column width along the roll (x), mm; 0 = no split.
    #[serde(deserialize_with = "de_f64")]
    pub tile_width: f64,
    /// Row height across the roll (y), mm; 0 = no split.
    #[serde(deserialize_with = "de_f64")]
    pub tile_height: f64,
    /// Each panel extends this far (mm) over its right and bottom neighbours.
    #[serde(deserialize_with = "de_f64")]
    pub overlap: f64,
    /// Add the panel rectangle to each panel's path.
    pub frame: bool,
}

impl Default for TileRequest {
    fn default() -> Self {
        Self {
            shapes: Vec::new(),
            tile_width: 0.0,
            tile_height: 0.0,
            overlap: 10.0,
            frame: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tile {
    pub col: u32,
    pub row: u32,
    /// Panel rectangle, document mm (overlap included).
    pub rect: Rect,
    /// Geometry of the panel (`M`/`L`/`Z` only), document mm.
    pub path: PathData,
}

/// Liang–Barsky: parameter range of segment `a -> b` inside `r`
/// (boundary included), or `None`.
fn clip_segment(a: Point, b: Point, r: &kurbo::Rect) -> Option<(f64, f64)> {
    let d = b - a;
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    for (p, q) in [
        (-d.x, a.x - r.x0),
        (d.x, r.x1 - a.x),
        (-d.y, a.y - r.y0),
        (d.y, r.y1 - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((t0, t1))
}

/// Pieces of `poly` inside `r`. A closed polyline entirely inside stays
/// closed; a cut one is reopened, and its pieces through the start vertex
/// are joined.
pub fn clip_polyline(poly: &Polyline, r: &kurbo::Rect) -> Vec<Polyline> {
    if poly.pts.len() < 2 {
        return Vec::new();
    }
    if let Some(b) = poly.bounds() {
        if b.x0 >= r.x0 && b.y0 >= r.y0 && b.x1 <= r.x1 && b.y1 <= r.y1 {
            return vec![poly.clone()];
        }
        if b.x1 < r.x0 || b.y1 < r.y0 || b.x0 > r.x1 || b.y0 > r.y1 {
            return Vec::new();
        }
    }
    let pts = poly.vertices_closed();
    let mut pieces: Vec<Vec<Point>> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let mut starts_at_p0 = false;
    let mut ends_at_p0 = false;
    let n_seg = pts.len() - 1;
    for (i, w) in pts.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let Some((t0, t1)) = clip_segment(a, b, r) else {
            if !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur));
            }
            continue;
        };
        let p0 = if t0 > 0.0 { a.lerp(b, t0) } else { a };
        let p1 = if t1 < 1.0 { a.lerp(b, t1) } else { b };
        if t0 > 0.0 || cur.is_empty() {
            if !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur));
            }
            cur.push(p0);
            if i == 0 && t0 == 0.0 {
                starts_at_p0 = true;
            }
        }
        if cur.last().is_none_or(|q| q.distance(p1) > 1e-9) {
            cur.push(p1);
        }
        if t1 < 1.0 {
            pieces.push(std::mem::take(&mut cur));
        } else if i + 1 == n_seg {
            ends_at_p0 = true;
        }
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    if poly.closed && starts_at_p0 && ends_at_p0 && pieces.len() >= 2 {
        // The last piece runs into P0 and the first leaves it: join them.
        let first = pieces.remove(0);
        if let Some(last) = pieces.last_mut() {
            last.extend(first.into_iter().skip(1));
        }
    }
    pieces
        .into_iter()
        .filter(|p| p.len() >= 2)
        .map(|p| Polyline::new(p, false))
        .filter(|p| p.length() > 1e-9)
        .collect()
}

fn push_polyline(out: &mut PathData, p: &Polyline) {
    let Some(first) = p.pts.first() else { return };
    out.push(Seg::M {
        x: first.x,
        y: first.y,
    });
    for q in &p.pts[1..] {
        out.push(Seg::L { x: q.x, y: q.y });
    }
    if p.closed {
        out.push(Seg::Z);
    }
}

/// Grid positions along one axis: `(start, end)` of each cell, overlap
/// included, clamped to the design's extent. `size <= 0` means one cell.
fn cells(lo: f64, hi: f64, size: f64, overlap: f64) -> Vec<(f64, f64)> {
    let extent = hi - lo;
    if !size.is_finite() || size <= 0.0 || extent <= size {
        return vec![(lo, hi)];
    }
    // Small tolerance so that 300 / 100 gives 3 cells, not 4.
    let n = ((extent / size) - 1e-9).ceil().max(1.0);
    let n = n.min((MAX_TILES + 1) as f64) as usize;
    (0..n)
        .map(|i| {
            let a = lo + i as f64 * size;
            let b = if i + 1 == n {
                hi
            } else {
                (lo + (i + 1) as f64 * size + overlap).min(hi)
            };
            (a, b)
        })
        .collect()
}

/// Split the shapes into panels over their bounding box.
///
/// Column `i` covers `[x0 + i·w, x0 + (i+1)·w]` plus `overlap` on the
/// right (rows likewise, downwards), except the last column/row, which
/// stops at the design's edge (as does any overlap reaching past it).
/// Curves are flattened (0.02 mm), so panel paths only contain `M`/`L`/`Z`.
/// Panels left without geometry are skipped. Order: column by column,
/// rows top to bottom.
pub fn make_tiles(req: &TileRequest) -> Result<Vec<Tile>, String> {
    if req.shapes.is_empty() {
        return Err("Mosaïque : aucun objet sélectionné.".into());
    }
    let Some(bb) = shapes_bounds(&req.shapes) else {
        return Err("Mosaïque : les objets sélectionnés ne contiennent aucun tracé.".into());
    };
    let overlap = non_negative(req.overlap);
    let cols = cells(bb.x0, bb.x1, req.tile_width, overlap);
    let rows = cells(bb.y0, bb.y1, req.tile_height, overlap);
    if cols.len() * rows.len() > MAX_TILES {
        return Err(format!(
            "Mosaïque : trop de panneaux ({} × {}) ; augmentez la taille des panneaux.",
            cols.len(),
            rows.len()
        ));
    }

    let polys: Vec<Polyline> = req
        .shapes
        .iter()
        .flat_map(|s| flatten_shape(s, DEFAULT_TOLERANCE))
        .collect();

    let mut tiles = Vec::new();
    for (ci, &(x0, x1)) in cols.iter().enumerate() {
        for (ri, &(y0, y1)) in rows.iter().enumerate() {
            let r = kurbo::Rect::new(x0, y0, x1, y1);
            let mut path = PathData::new();
            for p in &polys {
                for piece in clip_polyline(p, &r) {
                    push_polyline(&mut path, &piece);
                }
            }
            if path.is_empty() {
                continue;
            }
            if req.frame {
                let frame = Polyline::new(
                    vec![
                        Point::new(x0, y0),
                        Point::new(x1, y0),
                        Point::new(x1, y1),
                        Point::new(x0, y1),
                    ],
                    true,
                );
                push_polyline(&mut path, &frame);
            }
            tiles.push(Tile {
                col: ci as u32,
                row: ri as u32,
                rect: Rect::from(r),
                path,
            });
        }
    }
    Ok(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{FillRule, IDENTITY};

    fn rect_shape(x: f64, y: f64, w: f64, h: f64) -> Shape {
        Shape {
            path: vec![
                Seg::M { x, y },
                Seg::L { x: x + w, y },
                Seg::L { x: x + w, y: y + h },
                Seg::L { x, y: y + h },
                Seg::Z,
            ],
            transform: IDENTITY,
            fill_rule: FillRule::Nonzero,
        }
    }

    #[test]
    fn three_columns_with_overlap() {
        let req = TileRequest {
            shapes: vec![rect_shape(0.0, 0.0, 300.0, 100.0)],
            tile_width: 120.0,
            tile_height: 0.0,
            overlap: 10.0,
            frame: false,
        };
        let tiles = make_tiles(&req).unwrap();
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].rect, Rect::new(0.0, 0.0, 130.0, 100.0));
        assert_eq!(tiles[1].rect, Rect::new(120.0, 0.0, 130.0, 100.0));
        assert_eq!(tiles[2].rect, Rect::new(240.0, 0.0, 60.0, 100.0));
        assert_eq!((tiles[2].col, tiles[2].row), (2, 0));
        // The first panel holds the left part of the outline: one open
        // piece from (130,0) round the left side to (130,100).
        let t0 = &tiles[0].path;
        assert_eq!(t0.iter().filter(|s| matches!(s, Seg::M { .. })).count(), 1);
        assert!(!t0.contains(&Seg::Z));
        let pts: Vec<Point> = t0
            .iter()
            .filter_map(|s| match *s {
                Seg::M { x, y } | Seg::L { x, y } => Some(Point::new(x, y)),
                _ => None,
            })
            .collect();
        let expect = [(130.0, 100.0), (0.0, 100.0), (0.0, 0.0), (130.0, 0.0)];
        assert_eq!(pts.len(), expect.len());
        for (p, (x, y)) in pts.iter().zip(expect) {
            assert!(p.distance(Point::new(x, y)) < 1e-9, "{pts:?}");
        }
        // The middle panel only has the top and bottom edges.
        let t1 = &tiles[1].path;
        assert_eq!(t1.iter().filter(|s| matches!(s, Seg::M { .. })).count(), 2);
    }

    #[test]
    fn rows_frame_and_skipping_empty_tiles() {
        // Two small squares at opposite corners of a 100×100 area.
        let req = TileRequest {
            shapes: vec![
                rect_shape(0.0, 0.0, 10.0, 10.0),
                rect_shape(90.0, 90.0, 10.0, 10.0),
            ],
            tile_width: 50.0,
            tile_height: 50.0,
            overlap: 0.0,
            frame: true,
        };
        let tiles = make_tiles(&req).unwrap();
        assert_eq!(tiles.len(), 2);
        assert_eq!((tiles[0].col, tiles[0].row), (0, 0));
        assert_eq!((tiles[1].col, tiles[1].row), (1, 1));
        // Square kept closed + frame.
        assert_eq!(tiles[0].path.iter().filter(|s| **s == Seg::Z).count(), 2);
        assert_eq!(tiles[1].rect, Rect::new(50.0, 50.0, 50.0, 50.0));
    }

    #[test]
    fn no_split_and_limits() {
        let req = TileRequest {
            shapes: vec![rect_shape(5.0, 5.0, 10.0, 10.0)],
            ..Default::default()
        };
        let tiles = make_tiles(&req).unwrap();
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].rect, Rect::new(5.0, 5.0, 10.0, 10.0));
        let big = TileRequest {
            tile_width: 0.001,
            tile_height: 0.001,
            ..req.clone()
        };
        assert!(make_tiles(&big).is_err());
        let nan = TileRequest {
            tile_width: f64::NAN,
            overlap: f64::NAN,
            ..req
        };
        assert_eq!(make_tiles(&nan).unwrap().len(), 1);
        assert!(make_tiles(&TileRequest::default()).is_err());
    }

    #[test]
    fn clip_closed_polyline_joins_at_start() {
        // Square starting at (0,0) cut by x <= 5: one piece through P0.
        let sq = Polyline::new(
            vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 10.0),
                Point::new(0.0, 10.0),
            ],
            true,
        );
        let r = kurbo::Rect::new(-1.0, -1.0, 5.0, 11.0);
        let v = clip_polyline(&sq, &r);
        assert_eq!(v.len(), 1);
        assert_eq!(
            v[0].pts,
            vec![
                Point::new(5.0, 10.0),
                Point::new(0.0, 10.0),
                Point::new(0.0, 0.0),
                Point::new(5.0, 0.0)
            ]
        );
    }
}
