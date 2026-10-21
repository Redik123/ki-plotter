//! Cut ordering: inner paths first, then greedy nearest neighbour.
//!
//! A path may be cut only once every path nested inside it has been cut
//! (holes and small pieces before the outline that frees them, so the vinyl
//! does not move while it is still being cut). Among the paths available,
//! the one nearest to the current knife position is taken; a closed path is
//! rotated to start at its vertex nearest to the knife, an open path is
//! reversed when its far end is nearer.

use kurbo::Point;

use crate::flatten::{Polyline, SAME_POINT};

/// Absolute area of a closed polygon (shoelace).
fn area(pts: &[Point]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    (s * 0.5).abs()
}

/// Squared distance from `p` to segment `ab`.
fn dist2_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t - p).hypot2()
}

/// Even-odd point-in-polygon test. `None` when `p` lies on the boundary
/// (within [`SAME_POINT`]).
pub(crate) fn point_in_ring(p: Point, ring: &[Point]) -> Option<bool> {
    let n = ring.len();
    let mut inside = false;
    let eps2 = SAME_POINT * SAME_POINT;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        if dist2_to_segment(p, a, b) <= eps2 {
            return None;
        }
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    Some(inside)
}

fn within(inner: &kurbo::Rect, outer: &kurbo::Rect) -> bool {
    let e = SAME_POINT;
    inner.x0 >= outer.x0 - e
        && inner.y0 >= outer.y0 - e
        && inner.x1 <= outer.x1 + e
        && inner.y1 <= outer.y1 + e
}

/// Distance from `p` to the rectangle (0 inside): a lower bound of the
/// distance to any point of a path with that bounding box.
fn rect_dist(p: Point, r: &kurbo::Rect) -> f64 {
    let dx = (r.x0 - p.x).max(0.0).max(p.x - r.x1);
    let dy = (r.y0 - p.y).max(0.0).max(p.y - r.y1);
    dx.hypot(dy)
}

/// How to enter a path.
#[derive(Clone, Copy, Debug)]
enum Entry {
    /// Closed: start at vertex `k`.
    Rotate(usize),
    Forward,
    Reverse,
}

fn nearest_entry(path: &Polyline, pos: Point) -> (f64, Entry) {
    if path.closed {
        let mut best = (f64::INFINITY, Entry::Rotate(0));
        for (k, p) in path.pts.iter().enumerate() {
            let d = p.distance(pos);
            if d < best.0 {
                best = (d, Entry::Rotate(k));
            }
        }
        best
    } else {
        let first = path.pts.first().map_or(f64::INFINITY, |p| p.distance(pos));
        let last = path.pts.last().map_or(f64::INFINITY, |p| p.distance(pos));
        if last < first {
            (last, Entry::Reverse)
        } else {
            (first, Entry::Forward)
        }
    }
}

/// Paths containing each path: `containers[a]` lists every closed `b`
/// such that `a` lies inside `b`.
///
/// `a` is inside `b` if `a`'s bounding box is within `b`'s, `a` is smaller
/// than `b` (when `a` is closed; this also rules out containment cycles),
/// and the first vertex of `a` not on `b`'s boundary is inside `b`.
/// The bounding-box test comes first, so the vertex-level test only runs
/// for plausible pairs.
fn containers(paths: &[Polyline], boxes: &[Option<kurbo::Rect>]) -> Vec<Vec<usize>> {
    let n = paths.len();
    let areas: Vec<f64> = paths
        .iter()
        .map(|p| if p.closed { area(&p.pts) } else { 0.0 })
        .collect();
    let mut out = vec![Vec::new(); n];
    for b in 0..n {
        let pb = &paths[b];
        if !pb.closed || pb.pts.len() < 3 || areas[b] <= 0.0 {
            continue;
        }
        let Some(bb) = boxes[b] else { continue };
        for a in 0..n {
            if a == b {
                continue;
            }
            let Some(ba) = boxes[a] else { continue };
            if !within(&ba, &bb) {
                continue;
            }
            if paths[a].closed && areas[a] >= areas[b] {
                continue;
            }
            let inside = paths[a]
                .pts
                .iter()
                .find_map(|&p| point_in_ring(p, &pb.pts))
                .unwrap_or(false);
            if inside {
                out[a].push(b);
            }
        }
    }
    out
}

/// Order paths for cutting, starting from `start` (machine space).
/// See the module documentation.
pub fn order_paths(paths: Vec<Polyline>, start: Point) -> Vec<Polyline> {
    let n = paths.len();
    if n == 0 {
        return paths;
    }
    let boxes: Vec<Option<kurbo::Rect>> = paths.iter().map(Polyline::bounds).collect();
    let containers = containers(&paths, &boxes);
    // Number of not-yet-cut paths inside each path.
    let mut pending = vec![0usize; n];
    for cs in &containers {
        for &b in cs {
            pending[b] += 1;
        }
    }

    let mut remaining: Vec<usize> = (0..n).collect();
    let mut slots: Vec<Option<Polyline>> = paths.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(n);
    let mut pos = start;
    let lower = |i: usize, pos: Point| boxes[i].map_or(f64::INFINITY, |r| rect_dist(pos, &r));

    while !remaining.is_empty() {
        let any_free = remaining.iter().any(|&i| pending[i] == 0);
        let eligible = |i: usize| !any_free || pending[i] == 0;

        // Seed with the eligible path of smallest lower bound, then only
        // examine paths whose lower bound beats the best exact distance.
        let mut seed: Option<(f64, usize)> = None;
        for (ri, &i) in remaining.iter().enumerate() {
            if eligible(i) {
                let lb = lower(i, pos);
                if seed.is_none_or(|(s, _)| lb < s) {
                    seed = Some((lb, ri));
                }
            }
        }
        let Some((_, seed_ri)) = seed else { break };
        let seed_i = remaining[seed_ri];
        let Some(seed_path) = slots[seed_i].as_ref() else {
            break;
        };
        let (d0, e0) = nearest_entry(seed_path, pos);
        let mut best = (d0, seed_ri, e0);
        for (ri, &i) in remaining.iter().enumerate() {
            if ri == seed_ri || !eligible(i) || lower(i, pos) >= best.0 {
                continue;
            }
            if let Some(p) = slots[i].as_ref() {
                let (d, e) = nearest_entry(p, pos);
                if d < best.0 {
                    best = (d, ri, e);
                }
            }
        }

        let (_, ri, entry) = best;
        let i = remaining.swap_remove(ri);
        for &b in &containers[i] {
            pending[b] = pending[b].saturating_sub(1);
        }
        let Some(mut p) = slots[i].take() else {
            continue;
        };
        match entry {
            Entry::Rotate(k) => p.pts.rotate_left(k),
            Entry::Reverse => p.pts.reverse(),
            Entry::Forward => {}
        }
        // A closed path ends where it started.
        let end = if p.closed {
            p.pts.first()
        } else {
            p.pts.last()
        };
        pos = end.copied().unwrap_or(pos);
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Polyline {
        Polyline::new(
            vec![
                Point::new(x, y),
                Point::new(x + s, y),
                Point::new(x + s, y + s),
                Point::new(x, y + s),
            ],
            true,
        )
    }

    #[test]
    fn holes_before_outline() {
        // Big outline first in the input, nearest to the origin; the hole
        // and the small piece inside must still come first.
        let outer = square(0.0, 0.0, 100.0);
        let hole = square(40.0, 40.0, 20.0);
        let island = square(45.0, 45.0, 5.0);
        let far = square(200.0, 0.0, 10.0);
        let out = order_paths(
            vec![outer.clone(), hole.clone(), island.clone(), far.clone()],
            Point::ZERO,
        );
        assert_eq!(out.len(), 4);
        let idx = |q: &Polyline| out.iter().position(|p| p.bounds() == q.bounds()).unwrap();
        assert!(idx(&island) < idx(&hole));
        assert!(idx(&hole) < idx(&outer));
    }

    #[test]
    fn nearest_neighbour_rotation_and_reversal() {
        let a = square(10.0, 10.0, 5.0);
        let line = Polyline::new(vec![Point::new(50.0, 0.0), Point::new(20.0, 0.0)], false);
        let out = order_paths(vec![line, a], Point::ZERO);
        // The square is nearer to the origin; starts at its (10,10) corner.
        assert!(out[0].closed);
        assert_eq!(out[0].pts[0], Point::new(10.0, 10.0));
        // Then the line, entered from its (20,0) end.
        assert_eq!(out[1].pts[0], Point::new(20.0, 0.0));

        // Rotation to the nearest vertex.
        let out = order_paths(vec![square(0.0, 0.0, 10.0)], Point::new(11.0, 11.0));
        assert_eq!(out[0].pts[0], Point::new(10.0, 10.0));
        assert_eq!(out[0].pts.len(), 4);
    }

    #[test]
    fn open_path_inside_closed_goes_first() {
        let outer = square(0.0, 0.0, 10.0);
        let inner = Polyline::new(vec![Point::new(2.0, 5.0), Point::new(8.0, 5.0)], false);
        let out = order_paths(vec![outer, inner], Point::ZERO);
        assert!(!out[0].closed);
    }

    #[test]
    fn identical_paths_do_not_deadlock() {
        let out = order_paths(
            vec![square(0.0, 0.0, 10.0), square(0.0, 0.0, 10.0)],
            Point::ZERO,
        );
        assert_eq!(out.len(), 2);
        assert!(order_paths(vec![], Point::ZERO).is_empty());
        let out = order_paths(
            vec![Polyline::default(), square(0.0, 0.0, 1.0)],
            Point::ZERO,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn many_paths_quickly() {
        let mut v = Vec::new();
        for i in 0..60 {
            for j in 0..50 {
                v.push(square(i as f64 * 3.0, j as f64 * 3.0, 2.0));
            }
        }
        let out = order_paths(v, Point::ZERO);
        assert_eq!(out.len(), 3000);
    }

    #[test]
    fn point_in_ring_boundary() {
        let r = square(0.0, 0.0, 10.0).pts;
        assert_eq!(point_in_ring(Point::new(5.0, 5.0), &r), Some(true));
        assert_eq!(point_in_ring(Point::new(15.0, 5.0), &r), Some(false));
        assert_eq!(point_in_ring(Point::new(10.0, 5.0), &r), None);
    }
}
