//! Drag-knife compensation.
//!
//! The knife tip trails the swivel axis by `offset` mm. To make the tip
//! follow a polyline, the tool centre (what the plotter positions) must lead
//! it by `offset` in the direction of travel:
//!
//! - along a segment `A -> B` of unit direction `d`, the centre goes to
//!   `B + offset·d`;
//! - at a corner `B` between `d1` and `d2` turning by more than
//!   `corner_threshold` degrees, the centre swivels around `B` on an arc of
//!   radius `offset`, from `B + offset·d1` to `B + offset·d2`, the short way,
//!   in steps of at most 10°; for smaller turns it goes straight to
//!   `B + offset·d2`;
//! - the knife goes down at `P0 + offset·d0`.
//!
//! Closed paths: the closing segment is a regular segment and the corner
//! back into the first segment gets its swivel too; then the cut continues
//! for `overcut` mm along the start of the path (compensated as well), so the
//! end of the cut overlaps its beginning.

use kurbo::{Point, Vec2};

use crate::flatten::Polyline;

/// Segments shorter than this (mm) are ignored.
const MIN_SEG: f64 = 1e-9;
/// Offsets below this (mm) mean "no compensation".
const MIN_OFFSET: f64 = 1e-6;
/// Maximum angular step on a swivel arc.
const ARC_STEP_DEG: f64 = 10.0;

fn push_distinct(out: &mut Vec<Point>, p: Point) {
    if out.last().is_none_or(|q| q.distance(p) > MIN_SEG) {
        out.push(p);
    }
}

/// The route the knife tip must follow, as an explicit point list: the
/// polyline's points, plus for a closed one the return to `P0` and the
/// overcut continuation (capped at one perimeter).
fn tip_route(poly: &Polyline, overcut: f64) -> Vec<Point> {
    let mut pts: Vec<Point> = Vec::with_capacity(poly.pts.len() + 4);
    for &p in &poly.pts {
        if p.x.is_finite() && p.y.is_finite() {
            push_distinct(&mut pts, p);
        }
    }
    if !poly.closed || pts.len() < 2 {
        return pts;
    }
    if pts.len() >= 2 && pts[0].distance(pts[pts.len() - 1]) <= MIN_SEG {
        pts.pop();
    }
    let ring = pts.clone();
    let p0 = ring[0];
    push_distinct(&mut pts, p0);

    let perimeter: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut left = if overcut.is_finite() {
        overcut.clamp(0.0, perimeter)
    } else {
        0.0
    };
    let n = ring.len();
    let mut i = 0;
    while left > MIN_SEG {
        let a = ring[i % n];
        let b = ring[(i + 1) % n];
        let len = a.distance(b);
        if len >= left {
            push_distinct(&mut pts, a.lerp(b, left / len));
            break;
        }
        push_distinct(&mut pts, b);
        left -= len;
        i += 1;
        if i > n {
            break;
        }
    }
    pts
}

/// Insert the swivel at corner `b` from direction `d1` to `d2`.
fn corner(out: &mut Vec<Point>, b: Point, d1: Vec2, d2: Vec2, offset: f64, threshold_deg: f64) {
    let turn = d1.cross(d2).atan2(d1.dot(d2)); // signed, short way, (-pi, pi]
    let end = b + d2 * offset;
    if turn.abs().to_degrees() > threshold_deg {
        let steps = (turn.abs().to_degrees() / ARC_STEP_DEG).ceil().max(1.0) as usize;
        let a0 = d1.atan2();
        for k in 1..steps {
            let a = a0 + turn * (k as f64 / steps as f64);
            push_distinct(out, b + Vec2::from_angle(a) * offset);
        }
    }
    push_distinct(out, end);
}

/// Tool-centre path for `poly` (first point = knife-down position).
///
/// `offset <= 0` means no compensation: the result is the polyline itself
/// (closed: back to `P0`, then the overcut). `corner_threshold` is in
/// degrees. Zero-length segments are skipped; a polyline with a single
/// distinct point returns that point.
pub fn compensate(poly: &Polyline, offset: f64, overcut: f64, corner_threshold: f64) -> Vec<Point> {
    let route = tip_route(poly, overcut);
    let offset = if offset.is_finite() { offset } else { 0.0 };
    if route.len() < 2 || offset < MIN_OFFSET {
        return route;
    }
    let threshold = if corner_threshold.is_finite() {
        corner_threshold.clamp(0.0, 180.0)
    } else {
        15.0
    };
    let dirs: Vec<Vec2> = route
        .windows(2)
        .map(|w| {
            let d = w[1] - w[0];
            d / d.hypot()
        })
        .collect();

    let mut out = Vec::with_capacity(route.len() * 2);
    out.push(route[0] + dirs[0] * offset);
    for i in 0..dirs.len() {
        let b = route[i + 1];
        push_distinct(&mut out, b + dirs[i] * offset);
        if i + 1 < dirs.len() {
            corner(&mut out, b, dirs[i], dirs[i + 1], offset, threshold);
        }
    }
    // Closed path with no overcut: the route ends at P0, still swivel into
    // the first segment so the blade is aligned for a following pass.
    if poly.closed && dirs.len() >= 2 && route[route.len() - 1].distance(route[0]) <= MIN_SEG {
        corner(
            &mut out,
            route[0],
            dirs[dirs.len() - 1],
            dirs[0],
            offset,
            threshold,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-9
    }

    fn square() -> Polyline {
        Polyline::new(
            vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 10.0),
                Point::new(0.0, 10.0),
            ],
            true,
        )
    }

    #[test]
    fn straight_line_is_shifted_by_offset() {
        let l = Polyline::new(vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)], false);
        let c = compensate(&l, 0.25, 1.0, 15.0);
        assert_eq!(c.len(), 2);
        assert!(close(c[0], Point::new(0.25, 0.0)));
        assert!(close(c[1], Point::new(10.25, 0.0)));
    }

    #[test]
    fn zero_offset_returns_input() {
        let l = Polyline::new(
            vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 5.0),
            ],
            false,
        );
        assert_eq!(compensate(&l, 0.0, 1.0, 15.0), l.pts);
        // Closed: back to start, plus overcut along the first segment.
        let c = compensate(&square(), 0.0, 1.0, 15.0);
        assert_eq!(c.len(), 6);
        assert!(close(c[4], Point::new(0.0, 0.0)));
        assert!(close(c[5], Point::new(1.0, 0.0)));
    }

    #[test]
    fn square_gets_four_swivels() {
        let off = 0.25;
        let c = compensate(&square(), off, 0.0, 15.0);
        assert!(close(c[0], Point::new(0.25, 0.0)));
        // Every 90° corner: the centre on the segment's extension, 8
        // intermediate arc points, the centre on the next segment.
        for (pre, corner, post) in [
            (
                Point::new(10.25, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 0.25),
            ),
            (
                Point::new(10.0, 10.25),
                Point::new(10.0, 10.0),
                Point::new(9.75, 10.0),
            ),
            (
                Point::new(-0.25, 10.0),
                Point::new(0.0, 10.0),
                Point::new(0.0, 9.75),
            ),
            (
                Point::new(0.0, -0.25),
                Point::new(0.0, 0.0),
                Point::new(0.25, 0.0),
            ),
        ] {
            let i = c.iter().position(|p| close(*p, pre)).expect("pre point");
            assert!(close(c[i + 9], post), "{:?} vs {post:?}", c[i + 9]);
            for p in &c[i..=i + 9] {
                assert!((p.distance(corner) - off).abs() < 1e-9);
            }
        }
        assert_eq!(c.len(), 1 + 4 * 10);
        // Ends where it started (no overcut).
        assert!(close(*c.last().unwrap(), c[0]));
    }

    #[test]
    fn overcut_continues_past_start() {
        let c = compensate(&square(), 0.25, 1.0, 15.0);
        // Overcut of 1 mm along the first segment, compensated.
        assert!(close(*c.last().unwrap(), Point::new(1.25, 0.0)));
        // Overcut longer than a side wraps onto the next one.
        let c = compensate(&square(), 0.0, 12.0, 15.0);
        assert!(close(*c.last().unwrap(), Point::new(10.0, 2.0)));
    }

    #[test]
    fn shallow_corner_goes_straight() {
        let bend = |deg: f64| {
            let a = deg.to_radians();
            Polyline::new(
                vec![
                    Point::new(0.0, 0.0),
                    Point::new(10.0, 0.0),
                    Point::new(10.0 + 10.0 * a.cos(), -10.0 * a.sin()),
                ],
                false,
            )
        };
        // 10° turn, below the 15° threshold: straight to B + offset·d2.
        let c = compensate(&bend(10.0), 0.5, 0.0, 15.0);
        assert_eq!(c.len(), 4);
        let a = (-10f64).to_radians();
        assert!(close(c[2], Point::new(10.0 + 0.5 * a.cos(), 0.5 * a.sin())));
        // 30° turn (clockwise here): 3 steps of 10°, i.e. 2 points between.
        let c = compensate(&bend(30.0), 0.5, 0.0, 15.0);
        assert_eq!(c.len(), 6);
        for p in &c[1..5] {
            assert!((p.distance(Point::new(10.0, 0.0)) - 0.5).abs() < 1e-9);
            assert!(p.y <= 1e-12, "short way round: {p:?}");
        }
    }

    #[test]
    fn degenerate_inputs() {
        let one = Polyline::new(vec![Point::new(1.0, 1.0), Point::new(1.0, 1.0)], false);
        assert_eq!(compensate(&one, 0.25, 1.0, 15.0).len(), 1);
        let empty = Polyline::default();
        assert!(compensate(&empty, 0.25, 1.0, 15.0).is_empty());
        let nan = Polyline::new(vec![Point::new(0.0, 0.0), Point::new(5.0, 0.0)], true);
        let c = compensate(&nan, f64::NAN, f64::NAN, f64::NAN);
        assert_eq!(c.len(), 3);
    }
}
