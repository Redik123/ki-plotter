//! Cut-job preparation (`build_job`), see "Préparation du travail" in
//! `docs/ARCHITECTURE.md`.
//!
//! Pipeline: flatten → mirror → machine mapping → order → blade
//! compensation → passes → write. Geometry stays in millimetres until the
//! writer converts to plotter units.

use kurbo::Point;
use serde::{Deserialize, Serialize};

use crate::blade::compensate;
use crate::flatten::{Polyline, flatten_shape, sanitize_tolerance};
use crate::num::{de_count, de_f64, finite_or, non_negative, positive_or};
use crate::order::order_paths;
use crate::path::Shape;
use crate::rect::Rect;
use crate::{dmpl, hpgl};

/// Error when the job has nothing to cut.
pub const ERR_EMPTY: &str = "Rien à découper : aucun objet sélectionné.";
/// Error when the selected objects contain no drawable geometry.
pub const ERR_NO_GEOMETRY: &str =
    "Rien à découper : les objets sélectionnés ne contiennent aucun tracé.";

/// Maximum number of passes.
pub const MAX_PASSES: u32 = 10;

/// Cut parameters (per job).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CutSettings {
    /// Drag-knife offset, mm (0 = no compensation).
    #[serde(deserialize_with = "de_f64")]
    pub blade_offset: f64,
    /// Extra cut past the start of closed paths, mm.
    #[serde(deserialize_with = "de_f64")]
    pub overcut: f64,
    /// Number of times each path is cut (1–10).
    #[serde(deserialize_with = "de_count")]
    pub passes: u32,
    /// Reorder paths (inner first, nearest neighbour).
    pub optimize: bool,
    /// Mirror on `x` inside the job's bounding box (heat-transfer vinyl).
    pub mirror: bool,
    /// Machine origin at the job's bounding box (else at the media origin).
    pub from_origin: bool,
    /// Corners turning more than this (degrees) get a swivel arc.
    #[serde(deserialize_with = "de_f64")]
    pub corner_threshold: f64,
    /// Curve flattening tolerance, mm.
    #[serde(deserialize_with = "de_f64")]
    pub tolerance: f64,
}

impl Default for CutSettings {
    fn default() -> Self {
        Self {
            blade_offset: 0.25,
            overcut: 1.0,
            passes: 1,
            optimize: true,
            mirror: false,
            from_origin: true,
            corner_threshold: 15.0,
            tolerance: 0.02,
        }
    }
}

impl CutSettings {
    /// A copy with every value made usable: invalid numbers replaced by the
    /// defaults, ranges clamped (offset ≤ 5 mm, overcut ≤ 50 mm,
    /// passes 1–10, threshold 0–180°, tolerance 0.001–10 mm).
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        Self {
            blade_offset: non_negative(finite_or(self.blade_offset, d.blade_offset)).min(5.0),
            overcut: non_negative(finite_or(self.overcut, d.overcut)).min(50.0),
            passes: self.passes.clamp(1, MAX_PASSES),
            corner_threshold: finite_or(self.corner_threshold, d.corner_threshold)
                .clamp(0.0, 180.0),
            tolerance: sanitize_tolerance(self.tolerance),
            ..self.clone()
        }
    }
}

/// Output language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Hpgl,
    /// Experimental (strings from Inkcut).
    Dmpl,
}

/// Plotter description (per machine profile).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MachineSettings {
    pub protocol: Protocol,
    /// Plotter units per millimetre (HPGL: 40).
    #[serde(deserialize_with = "de_f64")]
    pub units_per_mm: f64,
    /// Calibration factor on the machine X axis (after `swapXY`).
    #[serde(deserialize_with = "de_f64")]
    pub scale_x: f64,
    /// Calibration factor on the machine Y axis (after `swapXY`).
    #[serde(deserialize_with = "de_f64")]
    pub scale_y: f64,
    #[serde(rename = "swapXY")]
    pub swap_xy: bool,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Send `speed`/`force` (else the panel settings apply).
    pub send_speed_force: bool,
    /// Cutting speed, cm/s (also used for the time estimate).
    #[serde(deserialize_with = "de_f64")]
    pub speed: f64,
    /// Knife force, grams (plotter-specific scale).
    #[serde(deserialize_with = "de_f64")]
    pub force: f64,
    /// Media feed after the job, mm (0 = return to the origin).
    #[serde(deserialize_with = "de_f64")]
    pub feed_after: f64,
    /// Maximum points per `PD` command (0 = unlimited).
    #[serde(deserialize_with = "de_count")]
    pub max_points_per_command: u32,
    /// Usable media width (across the roll), mm.
    #[serde(deserialize_with = "de_f64")]
    pub media_width: f64,
}

impl Default for MachineSettings {
    fn default() -> Self {
        Self {
            protocol: Protocol::Hpgl,
            units_per_mm: 40.0,
            scale_x: 1.0,
            scale_y: 1.0,
            swap_xy: false,
            flip_x: false,
            flip_y: true,
            send_speed_force: false,
            speed: 20.0,
            force: 80.0,
            feed_after: 0.0,
            max_points_per_command: 0,
            media_width: 720.0,
        }
    }
}

impl MachineSettings {
    /// A copy with every value made usable (invalid or non-positive
    /// units/scales/media width → defaults, negative speed/force/feed → 0).
    pub fn sanitized(&self) -> Self {
        let d = Self::default();
        Self {
            units_per_mm: positive_or(self.units_per_mm, d.units_per_mm).min(1.0e4),
            scale_x: positive_or(self.scale_x, 1.0).min(100.0),
            scale_y: positive_or(self.scale_y, 1.0).min(100.0),
            speed: if self.speed.is_nan() {
                d.speed
            } else {
                non_negative(self.speed)
            },
            force: if self.force.is_nan() {
                d.force
            } else {
                non_negative(self.force)
            },
            feed_after: non_negative(self.feed_after).min(1.0e5),
            media_width: positive_or(self.media_width, d.media_width),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobRequest {
    pub shapes: Vec<Shape>,
    pub cut: CutSettings,
    pub machine: MachineSettings,
}

/// One stretch of the knife's real trajectory, in document mm.
/// Pen-up travel moves have exactly two points (from → to).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Move {
    pub down: bool,
    pub pts: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobStats {
    /// Number of paths in the job (not multiplied by the passes).
    pub path_count: usize,
    /// Knife-down distance, mm (compensated path, all passes).
    pub cut_length: f64,
    /// Knife-up distance, mm (including the final return/feed move).
    pub travel_length: f64,
    pub estimated_seconds: f64,
    /// Job bounding box, document mm.
    pub bounds: Rect,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobResult {
    /// Plotter program (HPGL or DMPL text).
    pub data: String,
    pub moves: Vec<Move>,
    pub stats: JobStats,
}

/// Document ↔ machine mapping (both in mm), step 3 of the pipeline.
#[derive(Clone, Copy, Debug)]
pub struct MachineMap {
    min_x: f64,
    min_y: f64,
    w: f64,
    h: f64,
    flip_x: bool,
    flip_y: bool,
    swap: bool,
}

impl MachineMap {
    /// Reference box `R`: the job's bounding box when `from_origin`, else
    /// `x ∈ [0, job maxX]`, `y ∈ [0, mediaWidth]`.
    pub fn new(job_bbox: &kurbo::Rect, cut: &CutSettings, m: &MachineSettings) -> Self {
        let (min_x, min_y, w, h) = if cut.from_origin {
            (
                job_bbox.x0,
                job_bbox.y0,
                job_bbox.width(),
                job_bbox.height(),
            )
        } else {
            (0.0, 0.0, job_bbox.x1, m.media_width)
        };
        Self {
            min_x,
            min_y,
            w,
            h,
            flip_x: m.flip_x,
            flip_y: m.flip_y,
            swap: m.swap_xy,
        }
    }

    pub fn to_machine(&self, p: Point) -> Point {
        let u = p.x - self.min_x;
        let v = p.y - self.min_y;
        let x = if self.flip_x { self.w - u } else { u };
        let y = if self.flip_y { self.h - v } else { v };
        if self.swap {
            Point::new(y, x)
        } else {
            Point::new(x, y)
        }
    }

    pub fn to_document(&self, q: Point) -> Point {
        let (x, y) = if self.swap { (q.y, q.x) } else { (q.x, q.y) };
        let u = if self.flip_x { self.w - x } else { x };
        let v = if self.flip_y { self.h - y } else { y };
        Point::new(u + self.min_x, v + self.min_y)
    }
}

fn bounds_of(polys: &[Polyline]) -> Option<kurbo::Rect> {
    polys
        .iter()
        .filter_map(Polyline::bounds)
        .reduce(|a, b| a.union(b))
}

fn path_length(pts: &[Point]) -> f64 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Prepare a cut job: plotter program, real knife trajectory (for the
/// preview) and statistics.
///
/// `moves` are in document mm: the compensated tool-centre path mapped back
/// through the inverse machine mapping (the mirror, if any, is *not*
/// undone: the preview shows where the knife really goes). They start and
/// end at the machine origin (or the feed position).
pub fn build_job(req: &JobRequest) -> Result<JobResult, String> {
    if req.shapes.is_empty() {
        return Err(ERR_EMPTY.into());
    }
    let cut = req.cut.sanitized();
    let mach = req.machine.sanitized();

    // 1. Flatten.
    let mut polys: Vec<Polyline> = req
        .shapes
        .iter()
        .flat_map(|s| flatten_shape(s, cut.tolerance))
        .collect();
    let Some(bbox) = bounds_of(&polys) else {
        return Err(ERR_NO_GEOMETRY.into());
    };

    // 2. Mirror on x inside the job box (the box itself is unchanged).
    if cut.mirror {
        let axis = bbox.x0 + bbox.x1;
        for p in &mut polys {
            p.map(|q| Point::new(axis - q.x, q.y));
        }
    }

    // 3. Machine space (mm).
    let map = MachineMap::new(&bbox, &cut, &mach);
    for p in &mut polys {
        p.map(|q| map.to_machine(q));
    }

    // 4. Order.
    if cut.optimize {
        polys = order_paths(polys, Point::ZERO);
    }

    // 5. Blade compensation.
    let cuts: Vec<Vec<Point>> = polys
        .iter()
        .map(|p| compensate(p, cut.blade_offset, cut.overcut, cut.corner_threshold))
        .filter(|c| c.len() >= 2)
        .collect();
    if cuts.is_empty() {
        return Err(ERR_NO_GEOMETRY.into());
    }

    // 6. Passes.
    let mut program: Vec<Vec<Point>> = Vec::with_capacity(cuts.len() * cut.passes as usize);
    for c in &cuts {
        for _ in 0..cut.passes {
            program.push(c.clone());
        }
    }

    // The blade offset and overcut push the knife slightly past the job box.
    // When starting from the origin, keep every coordinate non-negative: cheap
    // cutter firmware may clamp or misbehave below zero.
    let shift = if cut.from_origin {
        let (mx, my) = program
            .iter()
            .flatten()
            .fold((0.0_f64, 0.0_f64), |(a, b), p| (a.min(p.x), b.min(p.y)));
        kurbo::Vec2::new(-mx, -my)
    } else {
        kurbo::Vec2::ZERO
    };
    if shift != kurbo::Vec2::ZERO {
        for p in program.iter_mut().flatten() {
            *p += shift;
        }
    }

    // End position: origin, or feed along the roll. The roll runs along
    // document x, which is machine X unless swapXY, then machine Y.
    let end = if mach.feed_after > 0.0 {
        let all = program.iter().flatten();
        if mach.swap_xy {
            let max_y = all.map(|p| p.y).fold(0.0, f64::max);
            Point::new(0.0, max_y + mach.feed_after)
        } else {
            let max_x = all.map(|p| p.x).fold(0.0, f64::max);
            Point::new(max_x + mach.feed_after, 0.0)
        }
    } else {
        Point::ZERO
    };

    // 7. Write.
    let data = match mach.protocol {
        Protocol::Hpgl => hpgl::write_hpgl(&program, end, &mach),
        Protocol::Dmpl => dmpl::write_dmpl(&program, end, &mach),
    };

    // 8. Trajectory and stats.
    let doc = |p: Point| {
        let d = map.to_document(p - shift);
        [d.x, d.y]
    };
    let mut moves = Vec::with_capacity(program.len() * 2 + 1);
    let mut pos = Point::ZERO;
    let mut travel = 0.0;
    let mut cut_length = 0.0;
    for path in &program {
        let start = path[0];
        let d = pos.distance(start);
        if d > 1e-9 {
            moves.push(Move {
                down: false,
                pts: vec![doc(pos), doc(start)],
            });
            travel += d;
        }
        cut_length += path_length(path);
        moves.push(Move {
            down: true,
            pts: path.iter().map(|&p| doc(p)).collect(),
        });
        pos = path[path.len() - 1];
    }
    let d = pos.distance(end);
    if d > 1e-9 {
        moves.push(Move {
            down: false,
            pts: vec![doc(pos), doc(end)],
        });
        travel += d;
    }

    let speed_mm_s = if mach.speed > 0.0 { mach.speed } else { 20.0 } * 10.0;
    let estimated_seconds =
        cut_length / speed_mm_s + travel / (2.0 * speed_mm_s) + 0.1 * program.len() as f64;

    Ok(JobResult {
        data,
        moves,
        stats: JobStats {
            path_count: cuts.len(),
            cut_length,
            travel_length: travel,
            estimated_seconds,
            bounds: Rect::from(bbox),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hpgl::parse_hpgl;
    use crate::path::{FillRule, IDENTITY, Seg};

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

    fn req(shapes: Vec<Shape>) -> JobRequest {
        JobRequest {
            shapes,
            ..Default::default()
        }
    }

    #[test]
    fn empty_job_is_an_error() {
        assert_eq!(build_job(&req(vec![])).unwrap_err(), ERR_EMPTY);
        let empty = Shape {
            path: vec![Seg::M { x: 1.0, y: 1.0 }],
            transform: IDENTITY,
            fill_rule: FillRule::Nonzero,
        };
        assert_eq!(build_job(&req(vec![empty])).unwrap_err(), ERR_NO_GEOMETRY);
    }

    #[test]
    fn hpgl_round_trip_rectangle() {
        let mut r = req(vec![rect_shape(10.0, 20.0, 100.0, 50.0)]);
        r.cut.blade_offset = 0.0;
        r.cut.overcut = 0.0;
        let res = build_job(&r).unwrap();
        assert!(res.data.starts_with("IN;PU"));
        assert!(res.data.ends_with("PU0,0;"));
        assert!(!res.data.contains(' ') && !res.data.contains('\n'));

        let cmds = parse_hpgl(&res.data);
        let downs: Vec<&Vec<(i64, i64)>> =
            cmds.iter().filter(|(d, _)| *d).map(|(_, c)| c).collect();
        assert_eq!(downs.len(), 1);
        // Box R = [10,110]×[20,70]; X = x-10, Y = 50-(y-20) (flipY), ×40.
        // The path starts at the corner nearest to the machine origin.
        assert_eq!(cmds[0], (false, vec![(0, 0)]));
        let mut got = downs[0].clone();
        assert_eq!(got.last(), Some(&(0, 0)), "closed back to start");
        got.sort();
        assert_eq!(got, vec![(0, 0), (0, 2000), (4000, 0), (4000, 2000)]);

        assert_eq!(res.stats.path_count, 1);
        assert!((res.stats.cut_length - 300.0).abs() < 1e-9);
        assert_eq!(res.stats.bounds, Rect::new(10.0, 20.0, 100.0, 50.0));
        // Cut moves are back in document coordinates.
        let cut = res.moves.iter().find(|m| m.down).unwrap();
        assert_eq!(cut.pts[0], [10.0, 70.0]);
        for p in &cut.pts {
            assert!(p[0] == 10.0 || p[0] == 110.0);
            assert!(p[1] == 20.0 || p[1] == 70.0);
        }
        // 300 mm at 200 mm/s + one pen down.
        assert!((res.stats.estimated_seconds - 1.6).abs() < 1e-9);
    }

    #[test]
    fn compensated_job_never_goes_below_origin() {
        let mut r = req(vec![rect_shape(10.0, 20.0, 30.0, 30.0)]);
        r.cut.blade_offset = 0.5;
        r.cut.overcut = 2.0;
        let job = build_job(&r).unwrap();
        let pts: Vec<(i64, i64)> = parse_hpgl(&job.data).into_iter().flat_map(|(_, p)| p).collect();
        assert!(pts.iter().all(|&(x, y)| x >= 0 && y >= 0), "{}", job.data);
        // The swivel arcs stick out by the offset: the job really reaches zero.
        assert!(pts.iter().any(|&(x, _)| x == 0) && pts.iter().any(|&(_, y)| y == 0));
        // The preview still lands on the design (within the blade offset).
        let b = job.stats.bounds;
        for m in job.moves.iter().filter(|m| m.down) {
            for p in &m.pts {
                assert!(p[0] >= b.x - 0.51 && p[0] <= b.x + b.w + 0.51, "{p:?}");
                assert!(p[1] >= b.y - 0.51 && p[1] <= b.y + b.h + 0.51, "{p:?}");
            }
        }
    }

    #[test]
    fn speed_force_feed_and_chunking() {
        let mut r = req(vec![rect_shape(0.0, 0.0, 10.0, 10.0)]);
        r.cut.blade_offset = 0.0;
        r.cut.overcut = 0.0;
        r.machine.send_speed_force = true;
        r.machine.speed = 30.0;
        r.machine.force = 92.5;
        r.machine.feed_after = 15.0;
        r.machine.max_points_per_command = 2;
        let res = build_job(&r).unwrap();
        assert!(res.data.starts_with("IN;VS30;FS92.5;PU"));
        // 4 PD points (after the PU start) in chunks of 2.
        assert_eq!(res.data.matches("PD").count(), 2);
        // Feed: max X 10 mm + 15 mm = 25 mm = 1000 units.
        assert!(res.data.ends_with("PU1000,0;"), "{}", res.data);
        let last = res.moves.last().unwrap();
        assert!(!last.down);
        // Machine (25, 0) → document (25, 10) with flipY and H = 10.
        assert_eq!(last.pts[1], [25.0, 10.0]);

        r.machine.swap_xy = true;
        let res = build_job(&r).unwrap();
        assert!(res.data.ends_with("PU0,1000;"), "{}", res.data);
    }

    #[test]
    fn passes_mirror_and_media_origin() {
        let mut r = req(vec![rect_shape(5.0, 5.0, 10.0, 10.0)]);
        r.cut.passes = 3;
        let res = build_job(&r).unwrap();
        assert_eq!(res.moves.iter().filter(|m| m.down).count(), 3);
        assert_eq!(res.stats.path_count, 1);
        assert_eq!(parse_hpgl(&res.data).iter().filter(|c| c.0).count(), 3);

        // fromOrigin = false: R = [0, 15] × [0, 720]; flipY → Y = 720 - y.
        let mut r = req(vec![rect_shape(5.0, 5.0, 10.0, 10.0)]);
        r.cut.blade_offset = 0.0;
        r.cut.overcut = 0.0;
        r.cut.from_origin = false;
        let res = build_job(&r).unwrap();
        let cmds = parse_hpgl(&res.data);
        let mut pts = cmds.iter().find(|c| c.0).unwrap().1.clone();
        pts.sort();
        pts.dedup();
        assert_eq!(
            pts,
            vec![(200, 28200), (200, 28600), (600, 28200), (600, 28600)]
        );

        // Mirror: an asymmetric shape flips inside its box.
        let tri = Shape {
            path: vec![
                Seg::M { x: 0.0, y: 0.0 },
                Seg::L { x: 10.0, y: 0.0 },
                Seg::L { x: 0.0, y: 10.0 },
                Seg::Z,
            ],
            transform: IDENTITY,
            fill_rule: FillRule::Nonzero,
        };
        let mut r = req(vec![tri]);
        r.cut.blade_offset = 0.0;
        r.cut.overcut = 0.0;
        r.cut.mirror = true;
        let res = build_job(&r).unwrap();
        let cut = res.moves.iter().find(|m| m.down).unwrap();
        assert!(cut.pts.contains(&[10.0, 10.0]));
        assert!(!cut.pts.contains(&[0.0, 10.0]));
        assert_eq!(res.stats.bounds, Rect::new(0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn mapping_round_trips() {
        let bbox = kurbo::Rect::new(3.0, 4.0, 50.0, 30.0);
        for bits in 0..16u8 {
            let cut = CutSettings {
                from_origin: bits & 8 == 0,
                ..Default::default()
            };
            let m = MachineSettings {
                flip_x: bits & 1 != 0,
                flip_y: bits & 2 != 0,
                swap_xy: bits & 4 != 0,
                ..Default::default()
            };
            let map = MachineMap::new(&bbox, &cut, &m);
            let p = Point::new(12.5, 7.25);
            assert!(map.to_document(map.to_machine(p)).distance(p) < 1e-12);
        }
    }

    #[test]
    fn dmpl_output() {
        let mut r = req(vec![rect_shape(0.0, 0.0, 10.0, 10.0)]);
        r.machine.protocol = Protocol::Dmpl;
        r.machine.send_speed_force = true;
        let res = build_job(&r).unwrap();
        assert!(res.data.starts_with(";:HAEC1 V20 BP80 U"), "{}", res.data);
        assert!(res.data.ends_with(" U0,0 "));
        assert!(res.data.contains(" D"));
    }

    #[test]
    fn hostile_settings_do_not_panic() {
        let mut r = req(vec![rect_shape(0.0, 0.0, 10.0, 10.0)]);
        r.cut = CutSettings {
            blade_offset: f64::NAN,
            overcut: f64::INFINITY,
            passes: 1_000_000,
            optimize: true,
            mirror: true,
            from_origin: false,
            corner_threshold: f64::NAN,
            tolerance: 0.0,
        };
        r.machine.units_per_mm = f64::NAN;
        r.machine.scale_x = -1.0;
        r.machine.speed = 0.0;
        r.machine.media_width = f64::NAN;
        let res = build_job(&r).unwrap();
        assert_eq!(
            res.moves.iter().filter(|m| m.down).count(),
            MAX_PASSES as usize
        );
        assert!(res.stats.estimated_seconds.is_finite());
    }

    #[test]
    fn serde_matches_typescript() {
        let json = r#"{
            "shapes": [{
                "path": [{"t":"M","x":0,"y":0},{"t":"L","x":10,"y":0},{"t":"Z"}],
                "transform": [1,0,0,1,0,0],
                "fillRule": "evenodd"
            }],
            "cut": {
                "bladeOffset": 0.3, "overcut": 0.5, "passes": 2, "optimize": false,
                "mirror": true, "fromOrigin": false, "cornerThreshold": 20,
                "tolerance": 0.05
            },
            "machine": {
                "protocol": "dmpl", "unitsPerMm": 39.37, "scaleX": 1.01,
                "scaleY": 0.99, "swapXY": true, "flipX": true, "flipY": false,
                "sendSpeedForce": true, "speed": 25, "force": 100,
                "feedAfter": 10, "maxPointsPerCommand": 50, "mediaWidth": 600
            }
        }"#;
        let r: JobRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.shapes[0].fill_rule, FillRule::Evenodd);
        assert_eq!(
            r.shapes[0].path,
            vec![
                Seg::M { x: 0.0, y: 0.0 },
                Seg::L { x: 10.0, y: 0.0 },
                Seg::Z
            ]
        );
        assert_eq!(r.cut.passes, 2);
        assert!(r.cut.mirror && !r.cut.optimize && !r.cut.from_origin);
        assert_eq!(r.machine.protocol, Protocol::Dmpl);
        assert!(r.machine.swap_xy && r.machine.flip_x && !r.machine.flip_y);
        assert_eq!(r.machine.max_points_per_command, 50);
        assert_eq!(r.machine.media_width, 600.0);

        // Seg list round trip.
        let segs = r#"[{"t":"M","x":0,"y":0},{"t":"L","x":10,"y":0},{"t":"Z"}]"#;
        let p: Vec<Seg> = serde_json::from_str(segs).unwrap();
        assert_eq!(p, r.shapes[0].path);
        let back: Vec<Seg> = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);

        // Settings serialise with the TS field names.
        let v = serde_json::to_value(MachineSettings::default()).unwrap();
        for k in [
            "protocol",
            "unitsPerMm",
            "scaleX",
            "scaleY",
            "swapXY",
            "flipX",
            "flipY",
            "sendSpeedForce",
            "speed",
            "force",
            "feedAfter",
            "maxPointsPerCommand",
            "mediaWidth",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert_eq!(v["protocol"], "hpgl");
        let v = serde_json::to_value(CutSettings::default()).unwrap();
        for k in [
            "bladeOffset",
            "overcut",
            "passes",
            "optimize",
            "mirror",
            "fromOrigin",
            "cornerThreshold",
            "tolerance",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        let res = build_job(&r).unwrap();
        let v = serde_json::to_value(&res).unwrap();
        for k in ["pathCount", "cutLength", "travelLength", "estimatedSeconds"] {
            assert!(v["stats"].get(k).is_some(), "missing {k}");
        }
        assert!(v["stats"]["bounds"].get("w").is_some());
        assert!(v["moves"][0]["pts"][0].is_array());

        // Missing fields take defaults; null and fractional numbers are
        // tolerated.
        let r: JobRequest = serde_json::from_str(
            r#"{"shapes":[],"cut":{"passes":2.6,"bladeOffset":null},"machine":{}}"#,
        )
        .unwrap();
        assert_eq!(r.cut.passes, 3);
        assert!(r.cut.blade_offset.is_nan());
        assert_eq!(r.cut.sanitized().blade_offset, 0.25);
        assert_eq!(r.machine, MachineSettings::default());
        let r: JobRequest = serde_json::from_str("{}").unwrap();
        assert!(r.shapes.is_empty());
    }
}
