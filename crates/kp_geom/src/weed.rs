//! Weeding lines (`make_weeding`): a frame around the shapes plus straight
//! lines across it, cut only outside the shapes.

use i_overlay::core::fill_rule::FillRule as OverlayFill;
use i_overlay::float::clip::FloatClip;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::string::clip::ClipRule;
use kurbo::Point;
use serde::{Deserialize, Serialize};

use crate::flatten::{DEFAULT_TOLERANCE, polygons_of};
use crate::num::{de_count, de_f64, non_negative};
use crate::path::{FillRule, PathData, Seg, Shape};
use crate::transform::shapes_bounds;

/// Maximum number of lines per direction.
pub const MAX_LINES: u32 = 200;
/// Pieces shorter than this (mm) are dropped.
const MIN_PIECE: f64 = 1.0;
/// A piece end closer than this (mm) to the line's end is on the frame.
const END_EPS: f64 = 1e-3;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WeedRequest {
    pub shapes: Vec<Shape>,
    /// Distance between the shapes' bounding box and the frame, mm.
    #[serde(deserialize_with = "de_f64")]
    pub margin: f64,
    /// Cut the frame rectangle.
    pub r#box: bool,
    /// Number of horizontal lines (along x), at most 200.
    #[serde(deserialize_with = "de_count")]
    pub horizontal: u32,
    /// Number of vertical lines (along y), at most 200.
    #[serde(deserialize_with = "de_count")]
    pub vertical: u32,
    /// Lines stop this far (mm) from the shapes.
    #[serde(deserialize_with = "de_f64")]
    pub gap: f64,
}

impl Default for WeedRequest {
    fn default() -> Self {
        Self {
            shapes: Vec::new(),
            margin: 5.0,
            r#box: true,
            horizontal: 0,
            vertical: 0,
            gap: 1.0,
        }
    }
}

type OvPoint = [f64; 2];
type OvShapes = Vec<Vec<Vec<OvPoint>>>;

/// Union of the shapes' fill areas, as i_overlay shapes. Each shape is
/// first normalised with its own fill rule (outer contours and holes with
/// consistent orientations); all normalised contours together, read with
/// the non-zero rule, then cover exactly the union.
fn union_of(shapes: &[Shape]) -> OvShapes {
    let mut all = OvShapes::new();
    for s in shapes {
        let rings: Vec<Vec<OvPoint>> = polygons_of(s, DEFAULT_TOLERANCE)
            .into_iter()
            .map(|r| r.into_iter().map(|p| [p.x, p.y]).collect())
            .collect();
        if rings.is_empty() {
            continue;
        }
        let rule = match s.fill_rule {
            FillRule::Nonzero => OverlayFill::NonZero,
            FillRule::Evenodd => OverlayFill::EvenOdd,
        };
        all.extend(rings.simplify_shape(rule));
    }
    all
}

fn ov_bounds(shape: &[Vec<OvPoint>]) -> Option<kurbo::Rect> {
    shape.iter().flatten().map(|p| Point::new(p[0], p[1])).fold(
        None,
        |acc: Option<kurbo::Rect>, p| {
            Some(acc.map_or(kurbo::Rect::from_points(p, p), |r| r.union_pt(p)))
        },
    )
}

/// Parts of the segment `a -> b` outside `union`, as `(from, to)` pairs,
/// shortened by `gap` at ends touching a shape (not at `a` or `b`, which
/// are on the frame), pieces under 1 mm dropped.
fn outside_pieces(a: Point, b: Point, union: &OvShapes, gap: f64) -> Vec<(Point, Point)> {
    let len = a.distance(b);
    if len.is_nan() || len < MIN_PIECE {
        return Vec::new();
    }
    let dir = (b - a) / len;
    let mut spans: Vec<(f64, f64)> = if union.is_empty() {
        vec![(0.0, len)]
    } else {
        // One line per overlay: several lines in one overlay would share
        // nodes where they cross, and pieces could turn corners.
        let line: Vec<OvPoint> = vec![[a.x, a.y], [b.x, b.y]];
        // With `invert`, `boundary_included: true` counts the shapes'
        // boundaries as inside: a line running along a shape's edge is
        // dropped there instead of re-cutting the edge. (A line merely
        // touching a vertex is not split by the clipper.)
        let rule = ClipRule {
            invert: true,
            boundary_included: true,
        };
        line.clip_by(union, OverlayFill::NonZero, rule)
            .iter()
            .filter_map(|piece| {
                let ts = piece
                    .iter()
                    .map(|p| (Point::new(p[0], p[1]) - a).dot(dir).clamp(0.0, len));
                let lo = ts.clone().fold(f64::INFINITY, f64::min);
                let hi = ts.fold(f64::NEG_INFINITY, f64::max);
                (hi > lo).then_some((lo, hi))
            })
            .collect()
    };
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    // Merge pieces that touch, should the clipper ever split a piece
    // somewhere that is not a shape contact.
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
    for (lo, hi) in spans {
        match merged.last_mut() {
            Some(last) if lo <= last.1 + 1e-6 => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    merged
        .into_iter()
        .filter_map(|(lo, hi)| {
            let lo = if lo <= END_EPS { 0.0 } else { lo + gap };
            let hi = if hi >= len - END_EPS { len } else { hi - gap };
            (hi - lo >= MIN_PIECE).then(|| (a + dir * lo, a + dir * hi))
        })
        .collect()
}

/// Build the weeding lines, in document mm: the frame (if `box`) as a
/// closed rectangle, then each line piece as an open `M`/`L` subpath.
///
/// The frame is the shapes' exact bounding box grown by `margin`. The
/// `horizontal` lines sit at `y = minY + k·H/(n+1)` (k = 1..n) inside the
/// frame, the `vertical` ones likewise; they are cut only outside the union
/// of the shapes (each with its own fill rule) and stop `gap` mm before
/// touching a shape.
pub fn make_weeding(req: &WeedRequest) -> Result<PathData, String> {
    if req.shapes.is_empty() {
        return Err("Échenillage : aucun objet sélectionné.".into());
    }
    let Some(bb) = shapes_bounds(&req.shapes) else {
        return Err("Échenillage : les objets sélectionnés ne contiennent aucun tracé.".into());
    };
    let margin = non_negative(req.margin).min(1.0e4);
    let gap = non_negative(req.gap).min(1.0e4);
    let frame = bb.inflate(margin, margin);
    let mut out = PathData::new();
    if req.r#box {
        out.extend([
            Seg::M {
                x: frame.x0,
                y: frame.y0,
            },
            Seg::L {
                x: frame.x1,
                y: frame.y0,
            },
            Seg::L {
                x: frame.x1,
                y: frame.y1,
            },
            Seg::L {
                x: frame.x0,
                y: frame.y1,
            },
            Seg::Z,
        ]);
    }

    let nh = req.horizontal.min(MAX_LINES);
    let nv = req.vertical.min(MAX_LINES);
    if nh == 0 && nv == 0 {
        return Ok(out);
    }
    let union = union_of(&req.shapes);
    let mut lines = Vec::with_capacity((nh + nv) as usize);
    for k in 1..=nh {
        let y = frame.y0 + k as f64 * frame.height() / (nh + 1) as f64;
        lines.push((Point::new(frame.x0, y), Point::new(frame.x1, y)));
    }
    for k in 1..=nv {
        let x = frame.x0 + k as f64 * frame.width() / (nv + 1) as f64;
        lines.push((Point::new(x, frame.y0), Point::new(x, frame.y1)));
    }
    let boxes: Vec<Option<kurbo::Rect>> = union.iter().map(|s| ov_bounds(s)).collect();
    for (a, b) in lines {
        // Only the shapes the line can meet take part in its clip.
        let lb = kurbo::Rect::from_points(a, b);
        let near: OvShapes = union
            .iter()
            .zip(&boxes)
            .filter(|(_, r)| {
                r.is_some_and(|r| r.x0 <= lb.x1 && r.x1 >= lb.x0 && r.y0 <= lb.y1 && r.y1 >= lb.y0)
            })
            .map(|(s, _)| s.clone())
            .collect();
        for (p, q) in outside_pieces(a, b, &near, gap) {
            out.push(Seg::M { x: p.x, y: p.y });
            out.push(Seg::L { x: q.x, y: q.y });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::IDENTITY;

    fn square(x: f64, y: f64, s: f64, rule: FillRule) -> Shape {
        Shape {
            path: vec![
                Seg::M { x, y },
                Seg::L { x: x + s, y },
                Seg::L { x: x + s, y: y + s },
                Seg::L { x, y: y + s },
                Seg::Z,
            ],
            transform: IDENTITY,
            fill_rule: rule,
        }
    }

    /// The open line pieces of a weeding result, as (x0, y0, x1, y1).
    fn pieces(p: &PathData) -> Vec<(f64, f64, f64, f64)> {
        let mut v = Vec::new();
        for i in 0..p.len() {
            if let (Some(Seg::M { x: x0, y: y0 }), Some(Seg::L { x: x1, y: y1 })) =
                (p.get(i), p.get(i + 1))
                && !matches!(p.get(i + 2), Some(Seg::L { .. } | Seg::Z))
            {
                v.push((*x0, *y0, *x1, *y1));
            }
        }
        v
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn two_squares_three_pieces() {
        let req = WeedRequest {
            shapes: vec![
                square(0.0, 0.0, 10.0, FillRule::Nonzero),
                square(20.0, 0.0, 10.0, FillRule::Nonzero),
            ],
            margin: 5.0,
            r#box: false,
            horizontal: 1,
            vertical: 0,
            gap: 0.0,
        };
        let out = make_weeding(&req).unwrap();
        let p = pieces(&out);
        assert_eq!(p.len(), 3, "{out:?}");
        let expect = [(-5.0, 0.0), (10.0, 20.0), (30.0, 35.0)];
        for ((x0, y0, x1, y1), (ex0, ex1)) in p.iter().zip(expect) {
            assert!(approx(*y0, 5.0) && approx(*y1, 5.0));
            assert!(
                approx(x0.min(*x1), ex0) && approx(x0.max(*x1), ex1),
                "{p:?}"
            );
        }

        // With a gap: shortened at shape ends only, not at the frame.
        let out = make_weeding(&WeedRequest { gap: 1.0, ..req }).unwrap();
        let p = pieces(&out);
        let expect = [(-5.0, -1.0), (11.0, 19.0), (31.0, 35.0)];
        for ((x0, _, x1, _), (ex0, ex1)) in p.iter().zip(expect) {
            assert!(
                approx(x0.min(*x1), ex0) && approx(x0.max(*x1), ex1),
                "{p:?}"
            );
        }
    }

    #[test]
    fn frame_and_vertical_lines() {
        let req = WeedRequest {
            shapes: vec![square(0.0, 0.0, 10.0, FillRule::Nonzero)],
            margin: 2.0,
            r#box: true,
            horizontal: 0,
            vertical: 3,
            gap: 0.5,
        };
        let out = make_weeding(&req).unwrap();
        assert_eq!(out[0], Seg::M { x: -2.0, y: -2.0 });
        assert_eq!(out[2], Seg::L { x: 12.0, y: 12.0 });
        assert_eq!(out[4], Seg::Z);
        let p = pieces(&out);
        // x = 1.5, 5, 8.5: all three cross the square, so 2 pieces each
        // (-2..-0.5 and 10.5..12).
        assert_eq!(p.len(), 6, "{p:?}");
        for (x0, y0, x1, y1) in &p {
            assert!(approx(*x0, *x1));
            let (lo, hi) = (y0.min(*y1), y0.max(*y1));
            assert!(
                (approx(lo, -2.0) && approx(hi, -0.5)) || (approx(lo, 10.5) && approx(hi, 12.0)),
                "{p:?}"
            );
        }
    }

    #[test]
    fn evenodd_hole_is_outside() {
        // A square ring (evenodd): the line is cut inside the hole too.
        let mut ring = square(0.0, 0.0, 30.0, FillRule::Evenodd);
        ring.path
            .extend(square(10.0, 10.0, 10.0, FillRule::Evenodd).path);
        let req = WeedRequest {
            shapes: vec![ring.clone()],
            margin: 5.0,
            r#box: false,
            horizontal: 1,
            vertical: 0,
            gap: 0.0,
        };
        let p = pieces(&make_weeding(&req).unwrap());
        assert_eq!(p.len(), 3, "{p:?}");
        // Same ring under nonzero with both contours in the same direction:
        // no hole, only the two outer pieces.
        ring.fill_rule = FillRule::Nonzero;
        let p = pieces(
            &make_weeding(&WeedRequest {
                shapes: vec![ring],
                ..req
            })
            .unwrap(),
        );
        assert_eq!(p.len(), 2, "{p:?}");
    }

    #[test]
    fn errors_and_limits() {
        assert!(make_weeding(&WeedRequest::default()).is_err());
        let req = WeedRequest {
            shapes: vec![square(0.0, 0.0, 10.0, FillRule::Nonzero)],
            margin: f64::NAN,
            r#box: false,
            horizontal: 100_000,
            vertical: 0,
            gap: f64::NAN,
        };
        // margin 0: lines touch the square's sides, nothing remains outside.
        let out = make_weeding(&req).unwrap();
        assert!(out.is_empty());
        let req = WeedRequest { margin: 3.0, ..req };
        let out = make_weeding(&req).unwrap();
        // 200 lines max, each crossing the square except the outermost
        // ones in the margin band, which stay whole.
        assert!(pieces(&out).len() <= 2 * MAX_LINES as usize);
        assert!(!out.is_empty());
    }

    #[test]
    fn line_along_an_edge_is_not_cut_on_it() {
        // Two stacked squares: the single horizontal line (y = 10) runs
        // along the bottom edge of the top square and the top edge of the
        // bottom one; it must stop at x = 0 and x = 10 (minus the gap).
        let req = WeedRequest {
            shapes: vec![
                square(0.0, 0.0, 10.0, FillRule::Nonzero),
                square(0.0, 10.0, 10.0, FillRule::Nonzero),
            ],
            margin: 5.0,
            r#box: false,
            horizontal: 1,
            vertical: 0,
            gap: 1.0,
        };
        let p = pieces(&make_weeding(&req).unwrap());
        assert_eq!(p.len(), 2, "{p:?}");
        let expect = [(-5.0, -1.0), (11.0, 15.0)];
        for ((x0, y0, x1, _), (ex0, ex1)) in p.iter().zip(expect) {
            assert!(approx(*y0, 10.0));
            assert!(
                approx(x0.min(*x1), ex0) && approx(x0.max(*x1), ex1),
                "{p:?}"
            );
        }
    }

    #[test]
    fn serde_box_field() {
        let r: WeedRequest = serde_json::from_str(
            r#"{"shapes":[],"margin":2,"box":false,"horizontal":3,"vertical":1.0,"gap":0.5}"#,
        )
        .unwrap();
        assert!(!r.r#box);
        assert_eq!(r.horizontal, 3);
        assert_eq!(r.vertical, 1);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["box"], false);
    }

    #[test]
    fn many_shapes_and_lines() {
        // 20×20 grid of 5 mm squares on a 10 mm pitch, 200 + 200 lines.
        let mut shapes = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                shapes.push(square(
                    i as f64 * 10.0,
                    j as f64 * 10.0,
                    5.0,
                    FillRule::Evenodd,
                ));
            }
        }
        let req = WeedRequest {
            shapes,
            margin: 5.0,
            r#box: true,
            horizontal: 500,
            vertical: 500,
            gap: 0.5,
        };
        let out = make_weeding(&req).unwrap();
        // Every piece stays outside all squares (checked at its midpoint).
        for (x0, y0, x1, y1) in pieces(&out) {
            let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let inside = |v: f64| v.rem_euclid(10.0) < 5.0 && (0.0..195.0).contains(&v);
            assert!(!(inside(mx) && inside(my)), "({mx}, {my})");
        }
    }
}
