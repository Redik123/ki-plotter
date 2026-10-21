//! Shared helpers: path building, affine matrices, bounding boxes and the
//! final normalisation to `(0, 0)`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use kp_geom::{Mat, PathData, Seg};
use kurbo::{CubicBez, ParamCurveExtrema, Point};

use crate::{Imported, ImportedItem};

/// Two points closer than this (in the builder's units) are the same point.
const SAME_POINT: f64 = 1e-7;

/// Coordinates beyond this (1000 km in mm) are garbage and dropped, so that
/// sizes never overflow.
const MAX_COORD: f64 = 1e12;

fn ok(v: f64) -> bool {
    v.is_finite() && v.abs() < MAX_COORD
}

/// Builds a [`PathData`] the way a canvas path is built, dropping empty
/// subpaths and non-finite coordinates.
#[derive(Default)]
pub(crate) struct PathBuilder {
    segs: Vec<Seg>,
    start: (f64, f64),
    cur: (f64, f64),
    /// Index of the current subpath's `M` in `segs`, if a subpath is open.
    sub: Option<usize>,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        if !(ok(x) && ok(y)) {
            return;
        }
        self.drop_empty_subpath();
        self.sub = Some(self.segs.len());
        self.segs.push(Seg::M { x, y });
        self.start = (x, y);
        self.cur = (x, y);
    }

    fn ensure_subpath(&mut self) {
        if self.sub.is_none() {
            let (x, y) = self.cur;
            self.move_to(x, y);
        }
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        if !(ok(x) && ok(y)) {
            return;
        }
        self.ensure_subpath();
        self.segs.push(Seg::L { x, y });
        self.cur = (x, y);
    }

    /// Quadratic Bézier, converted exactly to a cubic.
    pub fn quad_to(&mut self, x1: f64, y1: f64, x: f64, y: f64) {
        let (x0, y0) = self.cur;
        self.cubic_to(
            x0 + 2.0 / 3.0 * (x1 - x0),
            y0 + 2.0 / 3.0 * (y1 - y0),
            x + 2.0 / 3.0 * (x1 - x),
            y + 2.0 / 3.0 * (y1 - y),
            x,
            y,
        );
    }

    pub fn cubic_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        if ![x1, y1, x2, y2, x, y].into_iter().all(ok) {
            return;
        }
        self.ensure_subpath();
        self.segs.push(Seg::C {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        });
        self.cur = (x, y);
    }

    /// Closes the current subpath. A final line back onto the start point is
    /// dropped since `Z` draws it.
    pub fn close(&mut self) {
        let Some(i) = self.sub else { return };
        if self.segs.len() == i + 1 {
            // Lone M: nothing to close.
            self.segs.pop();
        } else {
            if self.segs.len() > i + 2
                && let Some(Seg::L { x, y }) = self.segs.last()
                && same_point((*x, *y), self.start)
            {
                self.segs.pop();
            }
            self.segs.push(Seg::Z);
        }
        self.sub = None;
        self.cur = self.start;
    }

    fn drop_empty_subpath(&mut self) {
        if let Some(i) = self.sub
            && self.segs.len() == i + 1
        {
            self.segs.pop();
        }
        self.sub = None;
    }

    pub fn finish(mut self) -> PathData {
        self.drop_empty_subpath();
        self.segs
    }
}

pub(crate) fn same_point(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() <= SAME_POINT && (a.1 - b.1).abs() <= SAME_POINT
}

// ---- Affine matrices ([a, b, c, d, e, f], as in kp_geom) ----

pub(crate) fn apply(m: &Mat, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// `mul(a, b)` applies `b` first, then `a`.
pub(crate) fn mul(a: &Mat, b: &Mat) -> Mat {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

pub(crate) fn translate(tx: f64, ty: f64) -> Mat {
    [1.0, 0.0, 0.0, 1.0, tx, ty]
}

pub(crate) fn scale(sx: f64, sy: f64) -> Mat {
    [sx, 0.0, 0.0, sy, 0.0, 0.0]
}

/// Counter-clockwise rotation in a y-up frame.
pub(crate) fn rotate(rad: f64) -> Mat {
    let (s, c) = rad.sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}

/// Translates every point of `path` by `(dx, dy)`.
pub(crate) fn translate_path(path: &mut PathData, dx: f64, dy: f64) {
    for seg in path.iter_mut() {
        match seg {
            Seg::M { x, y } | Seg::L { x, y } => {
                *x += dx;
                *y += dy;
            }
            Seg::C {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                *x1 += dx;
                *y1 += dy;
                *x2 += dx;
                *y2 += dy;
                *x += dx;
                *y += dy;
            }
            Seg::Z => {}
        }
    }
}

/// Exact bounding box `[min_x, min_y, max_x, max_y]` (curve extrema, not
/// control points), or `None` for an empty path.
pub(crate) fn path_bbox(path: &PathData) -> Option<[f64; 4]> {
    let mut bb: Option<[f64; 4]> = None;
    let mut add = |x: f64, y: f64| {
        let b = bb.get_or_insert([x, y, x, y]);
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x);
        b[3] = b[3].max(y);
    };
    let mut cur = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for seg in path {
        match *seg {
            Seg::M { x, y } => {
                add(x, y);
                cur = (x, y);
                start = cur;
            }
            Seg::L { x, y } => {
                add(x, y);
                cur = (x, y);
            }
            Seg::C {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let r = CubicBez::new(
                    Point::new(cur.0, cur.1),
                    Point::new(x1, y1),
                    Point::new(x2, y2),
                    Point::new(x, y),
                )
                .bounding_box();
                add(r.x0, r.y0);
                add(r.x1, r.y1);
                cur = (x, y);
            }
            Seg::Z => cur = start,
        }
    }
    bb
}

/// Drops empty items and translates everything so the overall bounding box
/// starts at `(0, 0)`.
pub(crate) fn normalize(items: Vec<ImportedItem>) -> Imported {
    let mut items: Vec<ImportedItem> = items.into_iter().filter(|i| !i.path.is_empty()).collect();
    let mut total: Option<[f64; 4]> = None;
    for item in &items {
        if let Some(b) = path_bbox(&item.path) {
            let t = total.get_or_insert(b);
            t[0] = t[0].min(b[0]);
            t[1] = t[1].min(b[1]);
            t[2] = t[2].max(b[2]);
            t[3] = t[3].max(b[3]);
        }
    }
    let Some([x0, y0, x1, y1]) = total else {
        return Imported::default();
    };
    for item in &mut items {
        translate_path(&mut item.path, -x0, -y0);
    }
    Imported {
        items,
        width: x1 - x0,
        height: y1 - y0,
    }
}

/// Runs `f`, turning a panic inside a third-party parser into an error.
pub(crate) fn guard<T>(what: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(what.to_string()))
}

/// Decodes a text file: UTF-8 when valid, else Latin-1.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// `"#rrggbb"`.
pub(crate) fn hex_color(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
pub(crate) fn count_subpaths(path: &PathData) -> usize {
    path.iter().filter(|s| matches!(s, Seg::M { .. })).count()
}

#[cfg(test)]
pub(crate) fn count_closed(path: &PathData) -> usize {
    path.iter().filter(|s| matches!(s, Seg::Z)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_drops_empty_and_closing_line() {
        let mut b = PathBuilder::new();
        b.move_to(5.0, 5.0); // empty subpath, dropped
        b.move_to(0.0, 0.0);
        b.line_to(1.0, 0.0);
        b.line_to(1.0, 1.0);
        b.line_to(0.0, 0.0);
        b.close();
        b.move_to(9.0, 9.0); // trailing M, dropped
        let p = b.finish();
        assert_eq!(
            p,
            vec![
                Seg::M { x: 0.0, y: 0.0 },
                Seg::L { x: 1.0, y: 0.0 },
                Seg::L { x: 1.0, y: 1.0 },
                Seg::Z
            ]
        );
    }

    #[test]
    fn quad_is_exact() {
        let mut b = PathBuilder::new();
        b.move_to(0.0, 0.0);
        b.quad_to(3.0, 3.0, 6.0, 0.0);
        let p = b.finish();
        assert_eq!(
            p[1],
            Seg::C {
                x1: 2.0,
                y1: 2.0,
                x2: 4.0,
                y2: 2.0,
                x: 6.0,
                y: 0.0
            }
        );
    }

    #[test]
    fn bbox_uses_curve_extrema() {
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::C {
                x1: 0.0,
                y1: 10.0,
                x2: 10.0,
                y2: 10.0,
                x: 10.0,
                y: 0.0,
            },
        ];
        let b = path_bbox(&p).unwrap();
        assert!((b[3] - 7.5).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn matrices() {
        let m = mul(&translate(10.0, 0.0), &rotate(std::f64::consts::FRAC_PI_2));
        let (x, y) = apply(&m, 1.0, 0.0);
        assert!((x - 10.0).abs() < 1e-12 && (y - 1.0).abs() < 1e-12);
        let (x, y) = apply(&scale(2.0, 3.0), 1.0, 1.0);
        assert_eq!((x, y), (2.0, 3.0));
    }
}
