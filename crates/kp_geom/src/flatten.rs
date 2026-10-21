//! Flattening shapes into polylines.

use kurbo::{PathEl, Point};

use crate::path::{Mat, PathData, Shape};
use crate::transform::{mat_is_finite, mat_to_affine, to_bezpath_with};

/// Default flattening tolerance, mm.
pub const DEFAULT_TOLERANCE: f64 = 0.02;

/// Points closer than this (mm) are considered identical.
pub const SAME_POINT: f64 = 1e-6;

/// A flattened subpath. For a closed polyline the closing segment from the
/// last point back to the first is implicit (the first point is not
/// repeated at the end).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polyline {
    pub pts: Vec<Point>,
    pub closed: bool,
}

impl Polyline {
    pub fn new(pts: Vec<Point>, closed: bool) -> Self {
        Self { pts, closed }
    }

    /// Length including the implicit closing segment.
    pub fn length(&self) -> f64 {
        let open: f64 = self.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        match (self.closed, self.pts.first(), self.pts.last()) {
            (true, Some(a), Some(b)) => open + a.distance(*b),
            _ => open,
        }
    }

    /// Bounding box of the points, `None` if empty.
    pub fn bounds(&self) -> Option<kurbo::Rect> {
        let first = *self.pts.first()?;
        Some(
            self.pts
                .iter()
                .fold(kurbo::Rect::from_points(first, first), |r, p| {
                    r.union_pt(*p)
                }),
        )
    }

    /// The points in drawing order, with the first point repeated at the end
    /// for a closed polyline.
    pub fn vertices_closed(&self) -> Vec<Point> {
        let mut v = self.pts.clone();
        if self.closed
            && let Some(&f) = self.pts.first()
        {
            v.push(f);
        }
        v
    }

    /// Apply `f` to every point.
    pub fn map(&mut self, f: impl Fn(Point) -> Point) {
        for p in &mut self.pts {
            *p = f(*p);
        }
    }
}

/// Clamp a tolerance to a sane range (0.001–10 mm); invalid values give the
/// default. Too small a tolerance would make the point count explode.
pub fn sanitize_tolerance(tol: f64) -> f64 {
    if tol.is_finite() && tol > 0.0 {
        tol.clamp(0.001, 10.0)
    } else {
        DEFAULT_TOLERANCE
    }
}

/// Finish a subpath: detect implicit closure, drop the duplicate end point,
/// drop degenerate subpaths (< 2 distinct points).
fn finish(out: &mut Vec<Polyline>, pts: &mut Vec<Point>, mut closed: bool) {
    let mut v = std::mem::take(pts);
    if v.len() >= 2 && v[0].distance(v[v.len() - 1]) <= SAME_POINT {
        closed = true;
        v.pop();
    }
    if v.len() >= 2 {
        out.push(Polyline::new(v, closed));
    }
}

/// Flatten `path` transformed by `m`, one polyline per subpath.
///
/// A subpath is closed if it ends with `Z`, or if its first and last points
/// coincide (within 1e-6 mm); the duplicate last point is then dropped.
/// Subpaths with fewer than 2 distinct points are dropped. The tolerance
/// is the maximum distance between the curve and the polyline, in output mm.
pub fn flatten_path(path: &PathData, m: &Mat, tolerance: f64) -> Vec<Polyline> {
    if !mat_is_finite(m) {
        return Vec::new();
    }
    let tol = sanitize_tolerance(tolerance);
    let bp = to_bezpath_with(path, mat_to_affine(m));
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    kurbo::flatten(bp.elements().iter().copied(), tol, |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut out, &mut cur, false);
            cur.push(p);
        }
        PathEl::LineTo(p) => {
            if cur.last().is_none_or(|q| q.distance(p) > SAME_POINT) {
                cur.push(p);
            }
        }
        PathEl::ClosePath => finish(&mut out, &mut cur, true),
        // `flatten` only emits MoveTo/LineTo/ClosePath.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    finish(&mut out, &mut cur, false);
    out
}

/// Flatten a shape (its transform applied), one polyline per subpath.
pub fn flatten_shape(shape: &Shape, tolerance: f64) -> Vec<Polyline> {
    flatten_path(&shape.path, &shape.transform, tolerance)
}

/// The fill polygons of a shape: every subpath with at least 3 points,
/// implicitly closed (as for SVG fill), transform applied. Use with the
/// shape's own fill rule.
pub fn polygons_of(shape: &Shape, tolerance: f64) -> Vec<Vec<Point>> {
    flatten_shape(shape, tolerance)
        .into_iter()
        .filter(|p| p.pts.len() >= 3)
        .map(|p| p.pts)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{FillRule, IDENTITY, Seg};

    fn sh(path: PathData) -> Shape {
        Shape {
            path,
            transform: IDENTITY,
            fill_rule: FillRule::Nonzero,
        }
    }

    #[test]
    fn closed_by_z_and_by_coincidence() {
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::L { x: 10.0, y: 0.0 },
            Seg::L { x: 10.0, y: 10.0 },
            Seg::Z,
            Seg::M { x: 20.0, y: 0.0 },
            Seg::L { x: 30.0, y: 0.0 },
            Seg::L { x: 30.0, y: 10.0 },
            Seg::L { x: 20.0, y: 0.0 },
            Seg::M { x: 40.0, y: 0.0 },
            Seg::L { x: 50.0, y: 0.0 },
        ];
        let v = flatten_shape(&sh(p), 0.02);
        assert_eq!(v.len(), 3);
        assert!(v[0].closed && v[0].pts.len() == 3);
        assert!(v[1].closed && v[1].pts.len() == 3);
        assert!(!v[2].closed && v[2].pts.len() == 2);
        assert!((v[0].length() - (20.0 + 200f64.sqrt())).abs() < 1e-9);
    }

    #[test]
    fn degenerate_dropped() {
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::M { x: 1.0, y: 1.0 },
            Seg::L { x: 1.0, y: 1.0 },
            Seg::Z,
            Seg::Z,
        ];
        assert!(flatten_shape(&sh(p), 0.02).is_empty());
        assert!(flatten_shape(&sh(vec![]), 0.02).is_empty());
    }

    #[test]
    fn curve_within_tolerance_and_transform_applied() {
        // Quarter circle of radius 10 (cubic approximation), scaled ×2.
        let k = 0.552_284_749_8 * 10.0;
        let p = vec![
            Seg::M { x: 10.0, y: 0.0 },
            Seg::C {
                x1: 10.0,
                y1: k,
                x2: k,
                y2: 10.0,
                x: 0.0,
                y: 10.0,
            },
        ];
        let s = Shape {
            path: p,
            transform: [2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            fill_rule: FillRule::Nonzero,
        };
        let v = flatten_shape(&s, 0.01);
        assert_eq!(v.len(), 1);
        let pts = &v[0].pts;
        assert!(pts.len() > 10);
        assert!(pts[0].distance(Point::new(20.0, 0.0)) < 1e-9);
        assert!(pts.last().unwrap().distance(Point::new(0.0, 20.0)) < 1e-9);
        for w in pts.windows(2) {
            let mid = w[0].midpoint(w[1]);
            // Chord midpoints stay within tolerance (plus the cubic's own
            // tiny deviation from a true circle) of radius 20.
            assert!((mid.to_vec2().hypot() - 20.0).abs() < 0.02);
        }
    }

    #[test]
    fn tiny_tolerance_is_clamped() {
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::C {
                x1: 0.0,
                y1: 100.0,
                x2: 100.0,
                y2: 100.0,
                x: 100.0,
                y: 0.0,
            },
        ];
        let v = flatten_shape(&sh(p.clone()), 0.0);
        let w = flatten_shape(&sh(p.clone()), f64::NAN);
        let x = flatten_shape(&sh(p), 1e-12);
        assert_eq!(v[0].pts.len(), w[0].pts.len());
        assert!(x[0].pts.len() < 10_000);
    }

    #[test]
    fn polygons_need_three_points() {
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::L { x: 10.0, y: 0.0 },
            Seg::M { x: 0.0, y: 0.0 },
            Seg::L { x: 10.0, y: 0.0 },
            Seg::L { x: 10.0, y: 10.0 },
        ];
        let polys = polygons_of(&sh(p), 0.02);
        assert_eq!(polys.len(), 1);
        assert_eq!(polys[0].len(), 3);
    }
}
