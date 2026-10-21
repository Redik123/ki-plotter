//! Affine transforms, `PathData` <-> `kurbo::BezPath`, and exact bounding boxes.

use kurbo::{Affine, BezPath, ParamCurveExtrema, PathEl, Point};

use crate::num::MAX_COORD;
use crate::path::{Mat, PathData, Seg, Shape};

/// The `kurbo` affine for a wire matrix (same coefficient order as SVG).
pub fn mat_to_affine(m: &Mat) -> Affine {
    Affine::new(*m)
}

/// True when all six coefficients are finite.
pub fn mat_is_finite(m: &Mat) -> bool {
    m.iter().all(|v| v.is_finite())
}

/// Apply `m` to every point of `path` (exact for lines and cubics).
pub fn transform_path(path: &PathData, m: &Mat) -> PathData {
    let a = mat_to_affine(m);
    let tp = |x: f64, y: f64| {
        let p = a * Point::new(x, y);
        (p.x, p.y)
    };
    path.iter()
        .map(|s| match *s {
            Seg::M { x, y } => {
                let (x, y) = tp(x, y);
                Seg::M { x, y }
            }
            Seg::L { x, y } => {
                let (x, y) = tp(x, y);
                Seg::L { x, y }
            }
            Seg::C {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (x1, y1) = tp(x1, y1);
                let (x2, y2) = tp(x2, y2);
                let (x, y) = tp(x, y);
                Seg::C {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                }
            }
            Seg::Z => Seg::Z,
        })
        .collect()
}

fn valid(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD
}

/// Convert to a `BezPath`, applying `affine` to every point.
///
/// The result is always well formed for `kurbo` (every subpath starts with a
/// `MoveTo`), whatever the input:
/// - `L`/`C` without a current point start a new subpath at their end point;
/// - after `Z`, a following `L`/`C` starts from the closed subpath's start
///   (SVG semantics), with an explicit `MoveTo`;
/// - a segment with a non-finite or out-of-range point (|v| > 1 km after the
///   transform) is dropped and breaks the subpath.
pub fn to_bezpath_with(path: &PathData, affine: Affine) -> BezPath {
    let mut bp = BezPath::new();
    let mut current: Option<Point> = None;
    let mut start: Option<Point> = None;
    // A MoveTo(current) must be emitted before the next drawing segment.
    let mut need_move = false;
    for seg in path {
        match *seg {
            Seg::M { x, y } => {
                let p = affine * Point::new(x, y);
                if valid(p) {
                    current = Some(p);
                    start = Some(p);
                    need_move = true;
                } else {
                    current = None;
                    start = None;
                }
            }
            Seg::L { x, y } => {
                let p = affine * Point::new(x, y);
                if !valid(p) {
                    current = None;
                    continue;
                }
                match current {
                    Some(c) => {
                        if need_move {
                            bp.move_to(c);
                            need_move = false;
                        }
                        bp.line_to(p);
                    }
                    None => {
                        start = Some(p);
                        need_move = true;
                    }
                }
                current = Some(p);
            }
            Seg::C {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let p1 = affine * Point::new(x1, y1);
                let p2 = affine * Point::new(x2, y2);
                let p = affine * Point::new(x, y);
                if !(valid(p1) && valid(p2) && valid(p)) {
                    current = None;
                    continue;
                }
                match current {
                    Some(c) => {
                        if need_move {
                            bp.move_to(c);
                            need_move = false;
                        }
                        bp.curve_to(p1, p2, p);
                    }
                    None => {
                        start = Some(p);
                        need_move = true;
                    }
                }
                current = Some(p);
            }
            Seg::Z => {
                // Only close a subpath that actually drew something.
                if current.is_some() && !need_move {
                    bp.close_path();
                    current = start;
                    need_move = true;
                }
            }
        }
    }
    bp
}

/// Convert to a `BezPath` without transform (see [`to_bezpath_with`]).
pub fn to_bezpath(path: &PathData) -> BezPath {
    to_bezpath_with(path, Affine::IDENTITY)
}

/// Convert a `BezPath` back to wire segments (quadratics are raised to cubics).
pub fn from_bezpath(bp: &BezPath) -> PathData {
    let mut out = Vec::with_capacity(bp.elements().len());
    let mut last = Point::ZERO;
    let mut start = Point::ZERO;
    for el in bp.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.push(Seg::M { x: p.x, y: p.y });
                last = p;
                start = p;
            }
            PathEl::LineTo(p) => {
                out.push(Seg::L { x: p.x, y: p.y });
                last = p;
            }
            PathEl::QuadTo(q, p) => {
                let c1 = last + (q - last) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                out.push(Seg::C {
                    x1: c1.x,
                    y1: c1.y,
                    x2: c2.x,
                    y2: c2.y,
                    x: p.x,
                    y: p.y,
                });
                last = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                out.push(Seg::C {
                    x1: c1.x,
                    y1: c1.y,
                    x2: c2.x,
                    y2: c2.y,
                    x: p.x,
                    y: p.y,
                });
                last = p;
            }
            PathEl::ClosePath => {
                out.push(Seg::Z);
                last = start;
            }
        }
    }
    out
}

fn union(acc: Option<kurbo::Rect>, r: kurbo::Rect) -> Option<kurbo::Rect> {
    Some(match acc {
        Some(a) => a.union(r),
        None => r,
    })
}

fn bezpath_bounds(bp: &BezPath) -> Option<kurbo::Rect> {
    let mut acc = None;
    for seg in bp.segments() {
        acc = union(acc, ParamCurveExtrema::bounding_box(&seg));
    }
    acc
}

/// Exact bounding box of the drawn segments of `path` (curve extrema
/// included; lone `M` points are ignored). `None` if nothing is drawn.
pub fn path_bounds(path: &PathData) -> Option<kurbo::Rect> {
    bezpath_bounds(&to_bezpath(path))
}

/// Exact bounding box of a shape, its transform applied. Shapes with a
/// non-finite transform have no geometry.
pub fn shape_bounds(shape: &Shape) -> Option<kurbo::Rect> {
    if !mat_is_finite(&shape.transform) {
        return None;
    }
    bezpath_bounds(&to_bezpath_with(
        &shape.path,
        mat_to_affine(&shape.transform),
    ))
}

/// Union of [`shape_bounds`] over `shapes`.
pub fn shapes_bounds(shapes: &[Shape]) -> Option<kurbo::Rect> {
    shapes.iter().filter_map(shape_bounds).fold(None, union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{FillRule, IDENTITY};

    fn shape(path: PathData, transform: Mat) -> Shape {
        Shape {
            path,
            transform,
            fill_rule: FillRule::Nonzero,
        }
    }

    #[test]
    fn transform_translates_and_scales() {
        let p = vec![Seg::M { x: 1.0, y: 2.0 }, Seg::L { x: 3.0, y: 4.0 }, Seg::Z];
        let t = transform_path(&p, &[2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
        assert_eq!(
            t,
            vec![
                Seg::M { x: 12.0, y: 26.0 },
                Seg::L { x: 16.0, y: 32.0 },
                Seg::Z
            ]
        );
    }

    #[test]
    fn bbox_includes_curve_extrema() {
        // Cubic bulging up to y = -7.5 between two points at y = 0.
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::C {
                x1: 0.0,
                y1: -10.0,
                x2: 10.0,
                y2: -10.0,
                x: 10.0,
                y: 0.0,
            },
        ];
        let r = path_bounds(&p).unwrap();
        assert!((r.y0 + 7.5).abs() < 1e-9, "{r:?}");
        assert!((r.x1 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn bezpath_round_trip_and_robustness() {
        let p = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::L { x: 10.0, y: 0.0 },
            Seg::Z,
            Seg::L { x: 0.0, y: 5.0 },
        ];
        let back = from_bezpath(&to_bezpath(&p));
        assert_eq!(
            back,
            vec![
                Seg::M { x: 0.0, y: 0.0 },
                Seg::L { x: 10.0, y: 0.0 },
                Seg::Z,
                Seg::M { x: 0.0, y: 0.0 },
                Seg::L { x: 0.0, y: 5.0 },
            ]
        );
        // No leading M, NaN in the middle: no panic, well-formed output.
        let bad = vec![
            Seg::L { x: 1.0, y: 1.0 },
            Seg::L { x: 2.0, y: 1.0 },
            Seg::L {
                x: f64::NAN,
                y: 0.0,
            },
            Seg::L { x: 3.0, y: 3.0 },
            Seg::Z,
            Seg::Z,
        ];
        let bp = to_bezpath(&bad);
        assert!(matches!(bp.elements()[0], PathEl::MoveTo(_)));
        assert_eq!(
            path_bounds(&bad).unwrap(),
            kurbo::Rect::new(1.0, 1.0, 2.0, 1.0)
        );
    }

    #[test]
    fn shapes_bbox() {
        let sq = vec![
            Seg::M { x: 0.0, y: 0.0 },
            Seg::L { x: 1.0, y: 0.0 },
            Seg::L { x: 1.0, y: 1.0 },
            Seg::Z,
        ];
        let a = shape(sq.clone(), IDENTITY);
        let b = shape(sq, [1.0, 0.0, 0.0, 1.0, 5.0, 5.0]);
        let r = shapes_bounds(&[a, b]).unwrap();
        assert_eq!(r, kurbo::Rect::new(0.0, 0.0, 6.0, 6.0));
        assert!(shapes_bounds(&[]).is_none());
        let nan = shape(vec![Seg::M { x: 0.0, y: 0.0 }], [f64::NAN; 6]);
        assert!(shape_bounds(&nan).is_none());
    }
}
