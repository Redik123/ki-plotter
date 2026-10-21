//! DXF import through the `dxf` crate.
//!
//! Supported entities: LINE, LWPOLYLINE and 2D/3D POLYLINE (with bulges),
//! CIRCLE, ARC, ELLIPSE (full or partial), SPLINE, SOLID, and INSERT of blocks
//! (insertion point, scale, rotation, MINSERT arrays, nested inserts). Text,
//! dimensions, hatches and 3D meshes are ignored, as are entities on layers
//! that are switched off and paper-space entities (unless there is nothing in
//! model space).
//!
//! Arcs become cubic Béziers (at most 90° each). Non-rational splines of degree
//! ≤ 3 are converted exactly to Béziers; rational or higher-degree splines are
//! sampled into a polyline within 0.01 mm.
//!
//! Units come from `$INSUNITS` (unset or unitless = mm). DXF is y-up: the
//! result is flipped. Open entities whose ends meet (0.01 mm, either
//! direction) are chained into one subpath, closed when the chain returns to
//! its start, which matters for cut ordering and overcut. The result has one
//! item per layer (fill rule evenodd, no colour).

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, TAU};
use std::io::Cursor;

use ::dxf::entities::{Entity, EntityType, Spline};
use ::dxf::enums::Units;
use ::dxf::{Block, Drawing};
use kp_geom::{FillRule, IDENTITY, Mat};

use crate::util::{self, PathBuilder, apply, mul, rotate, scale, translate};
use crate::{Imported, ImportedItem};

/// Ends closer than this (mm) are joined.
const JOIN_TOL: f64 = 0.01;
/// Chord tolerance when sampling curves (mm).
const SAMPLE_TOL: f64 = 0.01;
/// Nested INSERT limit (also stops self-referencing blocks).
const MAX_DEPTH: usize = 16;
/// MINSERT arrays larger than this are truncated.
const MAX_ARRAY: usize = 10_000;

type P = (f64, f64);

#[derive(Clone, Copy, Debug)]
enum PSeg {
    Line(P),
    Cubic(P, P, P),
}

impl PSeg {
    fn end(&self) -> P {
        match *self {
            PSeg::Line(p) | PSeg::Cubic(_, _, p) => p,
        }
    }
}

/// A subpath in final document coordinates (mm, y down).
#[derive(Clone, Debug)]
struct Piece {
    start: P,
    segs: Vec<PSeg>,
    closed: bool,
}

impl Piece {
    fn new(start: P) -> Self {
        Piece {
            start,
            segs: Vec::new(),
            closed: false,
        }
    }

    fn end(&self) -> P {
        self.segs.last().map_or(self.start, PSeg::end)
    }

    fn reversed(&self) -> Piece {
        let mut starts = Vec::with_capacity(self.segs.len());
        let mut prev = self.start;
        for s in &self.segs {
            starts.push(prev);
            prev = s.end();
        }
        let segs = self
            .segs
            .iter()
            .zip(starts)
            .rev()
            .map(|(s, st)| match *s {
                PSeg::Line(_) => PSeg::Line(st),
                PSeg::Cubic(c1, c2, _) => PSeg::Cubic(c2, c1, st),
            })
            .collect();
        Piece {
            start: prev,
            segs,
            closed: self.closed,
        }
    }

    /// Marks the piece closed when it ends where it starts.
    fn close_if_looped(&mut self) {
        if self.segs.is_empty() || dist(self.end(), self.start) > JOIN_TOL {
            return;
        }
        let start = self.start;
        if let Some(last) = self.segs.last_mut() {
            *last = match *last {
                PSeg::Line(_) => PSeg::Line(start),
                PSeg::Cubic(c1, c2, _) => PSeg::Cubic(c1, c2, start),
            };
        }
        self.closed = true;
    }
}

fn dist(a: P, b: P) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Imports a DXF file (ASCII or binary).
pub fn import_dxf(bytes: &[u8]) -> Result<Imported, String> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err("Impossible de lire le fichier DXF : le fichier est vide.".to_string());
    }
    let drawing = util::guard(
        "Impossible de lire le fichier DXF : fichier invalide ou corrompu.",
        || {
            Drawing::load(&mut Cursor::new(bytes))
                .map_err(|e| format!("Impossible de lire le fichier DXF : {e}"))
        },
    )?;
    let items = util::guard(
        "Impossible de convertir le fichier DXF : géométrie invalide.",
        || Ok(convert(&drawing)),
    )?;
    let imported = util::normalize(items);
    if imported.items.is_empty() {
        return Err("Le fichier DXF ne contient aucun tracé utilisable.".to_string());
    }
    Ok(imported)
}

/// Millimetres per drawing unit.
fn unit_scale(units: Units) -> f64 {
    match units {
        Units::Inches => 25.4,
        Units::Feet => 304.8,
        Units::Miles => 1_609_344.0,
        Units::Millimeters => 1.0,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        Units::Kilometers => 1_000_000.0,
        Units::Microinches => 25.4e-6,
        Units::Mils => 0.0254,
        Units::Yards => 914.4,
        Units::Nanometers => 1e-6,
        Units::Microns => 1e-3,
        Units::Decimeters => 100.0,
        Units::USSurveyFeet => 1200.0 / 3937.0 * 1000.0,
        Units::USSurveyInch => 100.0 / 3937.0 * 1000.0,
        _ => 1.0,
    }
}

struct Ctx<'a> {
    blocks: HashMap<String, &'a Block>,
    hidden_layers: Vec<String>,
    layer_names: Vec<String>,
    /// Pieces per layer index (into `layer_names`).
    pieces: Vec<Vec<Piece>>,
}

fn convert(drawing: &Drawing) -> Vec<ImportedItem> {
    let unit = unit_scale(drawing.header.default_drawing_units);
    // Drawing units, y up → mm, y down.
    let base: Mat = [unit, 0.0, 0.0, -unit, 0.0, 0.0];
    let mut ctx = Ctx {
        blocks: drawing
            .blocks()
            .map(|b| (b.name.to_uppercase(), b))
            .collect(),
        hidden_layers: drawing
            .layers()
            .filter(|l| !l.is_layer_on)
            .map(|l| l.name.to_uppercase())
            .collect(),
        layer_names: Vec::new(),
        pieces: Vec::new(),
    };
    let all: Vec<&Entity> = drawing.entities().collect();
    let model: Vec<&Entity> = all
        .iter()
        .copied()
        .filter(|e| !e.common.is_in_paper_space)
        .collect();
    let list = if model.is_empty() { all } else { model };
    for e in list {
        ctx.entity(e, &base, "0", 0);
    }

    let mut items = Vec::new();
    for pieces in ctx.pieces {
        let mut b = PathBuilder::new();
        for piece in chain(pieces) {
            b.move_to(piece.start.0, piece.start.1);
            for s in &piece.segs {
                match *s {
                    PSeg::Line(p) => b.line_to(p.0, p.1),
                    PSeg::Cubic(c1, c2, p) => b.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1),
                }
            }
            if piece.closed {
                b.close();
            }
        }
        items.push(ImportedItem {
            path: b.finish(),
            color: None,
            fill_rule: FillRule::Evenodd,
        });
    }
    items
}

/// Object coordinate system → world, for 2D entities. Only the common
/// mirrored case (extrusion −Z, which maps x to −x) is handled; other 3D
/// extrusions are projected as if they were +Z.
fn ocs(normal_z: f64) -> Mat {
    if normal_z < 0.0 {
        scale(-1.0, 1.0)
    } else {
        IDENTITY
    }
}

impl<'a> Ctx<'a> {
    fn push(&mut self, layer: &str, piece: Piece) {
        if piece.segs.is_empty() {
            return;
        }
        let idx = match self.layer_names.iter().position(|n| n == layer) {
            Some(i) => i,
            None => {
                self.layer_names.push(layer.to_string());
                self.pieces.push(Vec::new());
                self.layer_names.len() - 1
            }
        };
        self.pieces[idx].push(piece);
    }

    fn entity(&mut self, e: &'a Entity, m: &Mat, parent_layer: &str, depth: usize) {
        if !e.common.is_visible {
            return;
        }
        // Entities on layer "0" inside a block take the INSERT's layer.
        let layer = if depth > 0 && e.common.layer == "0" {
            parent_layer.to_string()
        } else {
            e.common.layer.clone()
        };
        if self.hidden_layers.contains(&layer.to_uppercase()) {
            return;
        }
        match &e.specific {
            EntityType::Line(l) => {
                let a = apply(m, l.p1.x, l.p1.y);
                let b = apply(m, l.p2.x, l.p2.y);
                if dist(a, b) > 1e-9 {
                    let mut p = Piece::new(a);
                    p.segs.push(PSeg::Line(b));
                    self.push(&layer, p);
                }
            }
            EntityType::LwPolyline(pl) => {
                let mm = mul(m, &ocs(pl.extrusion_direction.z));
                let verts: Vec<(P, f64)> =
                    pl.vertices.iter().map(|v| ((v.x, v.y), v.bulge)).collect();
                if let Some(p) = polyline(&mm, &verts, pl.is_closed()) {
                    self.push(&layer, p);
                }
            }
            EntityType::Polyline(pl) => {
                // 3D meshes (16) and polyface meshes (64) are not outlines.
                if pl.flags & (16 | 64) != 0 {
                    return;
                }
                let is_3d = pl.flags & 8 != 0;
                let mm = if is_3d { *m } else { mul(m, &ocs(pl.normal.z)) };
                // Skip spline frame control points (vertex flag 16).
                let verts: Vec<(P, f64)> = pl
                    .vertices()
                    .filter(|v| v.flags & 16 == 0)
                    .map(|v| {
                        (
                            (v.location.x, v.location.y),
                            if is_3d { 0.0 } else { v.bulge },
                        )
                    })
                    .collect();
                if let Some(p) = polyline(&mm, &verts, pl.is_closed()) {
                    self.push(&layer, p);
                }
            }
            EntityType::Circle(c) => {
                if c.radius > 0.0 && c.radius.is_finite() {
                    let mm = mul(m, &ocs(c.normal.z));
                    let r = c.radius;
                    let mut p =
                        ellipse_arc(&mm, (c.center.x, c.center.y), (r, 0.0), (0.0, r), 0.0, TAU);
                    p.closed = true;
                    self.push(&layer, p);
                }
            }
            EntityType::Arc(a) => {
                if a.radius > 0.0 && a.radius.is_finite() {
                    let mm = mul(m, &ocs(a.normal.z));
                    let mut sweep = (a.end_angle - a.start_angle).rem_euclid(360.0);
                    if sweep < 1e-9 {
                        sweep = 360.0;
                    }
                    let t0 = a.start_angle.to_radians();
                    let r = a.radius;
                    let p = ellipse_arc(
                        &mm,
                        (a.center.x, a.center.y),
                        (r, 0.0),
                        (0.0, r),
                        t0,
                        t0 + sweep.to_radians(),
                    );
                    self.push(&layer, p);
                }
            }
            EntityType::Ellipse(el) => {
                let u = (el.major_axis.x, el.major_axis.y);
                let ratio = el.minor_axis_ratio;
                if u.0.hypot(u.1) <= 0.0 || !(ratio.is_finite() && ratio > 0.0) {
                    return;
                }
                // Minor axis = normal × major axis × ratio (world coordinates).
                let nz = if el.normal.z < 0.0 { -1.0 } else { 1.0 };
                let v = (-nz * u.1 * ratio, nz * u.0 * ratio);
                let mut sweep = (el.end_parameter - el.start_parameter).rem_euclid(TAU);
                if sweep < 1e-9 {
                    sweep = TAU;
                }
                let full = (sweep - TAU).abs() < 1e-6;
                let t0 = el.start_parameter;
                let mut p = ellipse_arc(m, (el.center.x, el.center.y), u, v, t0, t0 + sweep);
                p.closed = full;
                self.push(&layer, p);
            }
            EntityType::Spline(s) => {
                if let Some(p) = spline(m, s) {
                    self.push(&layer, p);
                }
            }
            EntityType::Solid(s) => {
                let mm = mul(m, &ocs(s.extrusion_direction.z));
                // Corner order is 1, 2, 4, 3; 3 == 4 for a triangle.
                let mut corners = vec![
                    (s.first_corner.x, s.first_corner.y),
                    (s.second_corner.x, s.second_corner.y),
                    (s.fourth_corner.x, s.fourth_corner.y),
                    (s.third_corner.x, s.third_corner.y),
                ];
                corners.dedup_by(|a, b| dist(*a, *b) < 1e-12);
                let verts: Vec<(P, f64)> = corners.into_iter().map(|c| (c, 0.0)).collect();
                if let Some(p) = polyline(&mm, &verts, true) {
                    self.push(&layer, p);
                }
            }
            EntityType::Insert(ins) => {
                if depth >= MAX_DEPTH {
                    return;
                }
                let Some(block) = self.blocks.get(&ins.name.to_uppercase()).copied() else {
                    return;
                };
                let sx = if ins.x_scale_factor == 0.0 {
                    1.0
                } else {
                    ins.x_scale_factor
                };
                let sy = if ins.y_scale_factor == 0.0 {
                    1.0
                } else {
                    ins.y_scale_factor
                };
                let cols = ins.column_count.max(1) as usize;
                let rows = ins.row_count.max(1) as usize;
                let place = mul(
                    &mul(m, &ocs(ins.extrusion_direction.z)),
                    &mul(
                        &translate(ins.location.x, ins.location.y),
                        &rotate(ins.rotation.to_radians()),
                    ),
                );
                let inner = mul(
                    &scale(sx, sy),
                    &translate(-block.base_point.x, -block.base_point.y),
                );
                let mut count = 0;
                'array: for r in 0..rows {
                    for c in 0..cols {
                        count += 1;
                        if count > MAX_ARRAY {
                            break 'array;
                        }
                        let cell =
                            translate(c as f64 * ins.column_spacing, r as f64 * ins.row_spacing);
                        let mm = mul(&place, &mul(&cell, &inner));
                        if !mm.iter().all(|v| v.is_finite()) {
                            continue;
                        }
                        for child in &block.entities {
                            self.entity(child, &mm, &layer, depth + 1);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Vertices with bulges (`tan(θ/4)` of the arc to the next vertex, positive =
/// counter-clockwise), in entity coordinates, mapped through `m`.
fn polyline(m: &Mat, verts: &[(P, f64)], closed: bool) -> Option<Piece> {
    if verts.len() < 2
        || verts
            .iter()
            .any(|((x, y), b)| !(x.is_finite() && y.is_finite() && b.is_finite()))
    {
        return None;
    }
    let mut piece = Piece::new(apply(m, verts[0].0.0, verts[0].0.1));
    let n = verts.len();
    let count = if closed { n } else { n - 1 };
    for i in 0..count {
        let (p0, bulge) = verts[i];
        let p1 = verts[(i + 1) % n].0;
        let d = dist(p0, p1);
        if d < 1e-12 {
            continue;
        }
        if bulge.abs() < 1e-12 {
            piece.segs.push(PSeg::Line(apply(m, p1.0, p1.1)));
            continue;
        }
        let theta = 4.0 * bulge.atan();
        let mid = ((p0.0 + p1.0) / 2.0, (p0.1 + p1.1) / 2.0);
        // Left normal of the chord; the centre is on that side for 0 < θ < π.
        let nrm = (-(p1.1 - p0.1) / d, (p1.0 - p0.0) / d);
        let h = d / 2.0 / (theta / 2.0).tan();
        let c = (mid.0 + nrm.0 * h, mid.1 + nrm.1 * h);
        let r = dist(p0, c);
        let a0 = (p0.1 - c.1).atan2(p0.0 - c.0);
        let mut arc = ellipse_arc(m, c, (r, 0.0), (0.0, r), a0, a0 + theta).segs;
        // Land exactly on the next vertex.
        let end = apply(m, p1.0, p1.1);
        if let Some(PSeg::Cubic(c1, c2, _)) = arc.last().copied() {
            *arc.last_mut().unwrap() = PSeg::Cubic(c1, c2, end);
        }
        piece.segs.extend(arc);
    }
    piece.closed = closed;
    Some(piece)
}

/// `center + u·cos t + v·sin t` for `t` from `t0` to `t1` (either direction),
/// mapped through `m`, as cubics of at most 90° of parameter each.
fn ellipse_arc(m: &Mat, center: P, u: P, v: P, t0: f64, t1: f64) -> Piece {
    let at = |t: f64| {
        (
            center.0 + u.0 * t.cos() + v.0 * t.sin(),
            center.1 + u.1 * t.cos() + v.1 * t.sin(),
        )
    };
    let tangent = |t: f64| {
        (
            -u.0 * t.sin() + v.0 * t.cos(),
            -u.1 * t.sin() + v.1 * t.cos(),
        )
    };
    let sweep = t1 - t0;
    let n = ((sweep.abs() / FRAC_PI_2) - 1e-9).ceil().max(1.0) as usize;
    let dt = sweep / n as f64;
    let k = 4.0 / 3.0 * (dt / 4.0).tan();
    let start = at(t0);
    let mut piece = Piece::new(apply(m, start.0, start.1));
    for i in 0..n {
        let a = t0 + dt * i as f64;
        let b = if i + 1 == n { t1 } else { a + dt };
        let (pa, pb) = (at(a), at(b));
        let (da, db) = (tangent(a), tangent(b));
        let c1 = (pa.0 + k * da.0, pa.1 + k * da.1);
        let c2 = (pb.0 - k * db.0, pb.1 - k * db.1);
        piece.segs.push(PSeg::Cubic(
            apply(m, c1.0, c1.1),
            apply(m, c2.0, c2.1),
            apply(m, pb.0, pb.1),
        ));
    }
    piece
}

// ---- Splines ----

fn spline(m: &Mat, s: &Spline) -> Option<Piece> {
    let ctrl: Vec<P> = s.control_points.iter().map(|q| (q.x, q.y)).collect();
    let fit: Vec<P> = s.fit_points.iter().map(|q| (q.x, q.y)).collect();
    let finite = |pts: &[P]| pts.iter().all(|p| p.0.is_finite() && p.1.is_finite());
    let p = s.degree_of_curve.clamp(1, 25) as usize;
    if ctrl.len() < 2 || !finite(&ctrl) {
        return (fit.len() >= 2 && finite(&fit)).then(|| through_points(m, &fit, s.is_closed()));
    }
    let p = p.min(ctrl.len() - 1);
    let mut knots = s.knot_values.clone();
    let n = ctrl.len();
    let valid = knots.len() == n + p + 1
        && knots.iter().all(|k| k.is_finite())
        && knots.windows(2).all(|w| w[1] >= w[0])
        && knots[n] > knots[p];
    if !valid {
        knots = clamped_uniform_knots(n, p);
    }
    let weights = &s.weight_values;
    let rational = weights.len() == n
        && weights.iter().all(|w| w.is_finite() && *w > 0.0)
        && weights.iter().any(|w| (w - weights[0]).abs() > 1e-12);

    let mut piece = if !rational && p <= 3 {
        bezier_spline(m, p, knots, ctrl)
    } else {
        let w = if rational {
            weights.clone()
        } else {
            vec![1.0; n]
        };
        sampled_spline(m, p, &knots, &ctrl, &w)
    };
    if s.is_closed() {
        piece.close_if_looped();
    }
    Some(piece)
}

fn clamped_uniform_knots(n: usize, p: usize) -> Vec<f64> {
    let inner = n - p; // number of spans
    let mut k = vec![0.0; p + 1];
    k.extend((1..inner).map(|i| i as f64));
    k.extend(std::iter::repeat_n(inner as f64, p + 1));
    k
}

fn lerp(a: P, b: P, t: f64) -> P {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Exact conversion of a non-rational B-spline of degree 1–3 to Béziers, by
/// inserting knots until every knot of the domain has multiplicity `p`.
fn bezier_spline(m: &Mat, p: usize, mut knots: Vec<f64>, mut ctrl: Vec<P>) -> Piece {
    let (lo, hi) = (knots[p], knots[ctrl.len()]);
    let tol = (hi - lo).abs() * 1e-10 + 1e-14;
    let mut values: Vec<f64> = knots
        .iter()
        .copied()
        .filter(|&u| u >= lo - tol && u <= hi + tol)
        .collect();
    values.dedup_by(|a, b| (*a - *b).abs() <= tol);
    for u in values {
        let u = u.clamp(lo, hi);
        let mult = knots.iter().filter(|&&k| (k - u).abs() <= tol).count();
        for _ in mult..p {
            insert_knot(&mut knots, &mut ctrl, p, u);
        }
    }
    let pt = |q: P| apply(m, q.0, q.1);
    let mut piece: Option<Piece> = None;
    for k in p..ctrl.len() {
        if knots[k + 1] - knots[k] <= tol {
            continue;
        }
        let b = &ctrl[k - p..=k];
        let piece = piece.get_or_insert_with(|| Piece::new(pt(b[0])));
        piece.segs.push(match p {
            1 => PSeg::Line(pt(b[1])),
            2 => PSeg::Cubic(
                pt(lerp(b[0], b[1], 2.0 / 3.0)),
                pt(lerp(b[2], b[1], 2.0 / 3.0)),
                pt(b[2]),
            ),
            _ => PSeg::Cubic(pt(b[1]), pt(b[2]), pt(b[3])),
        });
    }
    piece.unwrap_or_else(|| Piece::new(pt(ctrl[0])))
}

/// Boehm knot insertion of `u` (inside the domain) into a B-spline of degree `p`.
fn insert_knot(knots: &mut Vec<f64>, ctrl: &mut Vec<P>, p: usize, u: f64) {
    let n = ctrl.len();
    // Span k with knots[k] <= u <= knots[k + 1], within [p, n - 1].
    let mut k = p;
    while k + 1 < n && knots[k + 1] <= u {
        k += 1;
    }
    let mut new = Vec::with_capacity(n + 1);
    for i in 0..=n {
        if i + p <= k {
            new.push(ctrl[i]);
        } else if i <= k {
            let denom = knots[i + p] - knots[i];
            let a = if denom.abs() < 1e-300 {
                0.0
            } else {
                (u - knots[i]) / denom
            };
            new.push(lerp(ctrl[i - 1], ctrl[i], a));
        } else {
            new.push(ctrl[i - 1]);
        }
    }
    knots.insert(k + 1, u);
    *ctrl = new;
}

/// de Boor evaluation (homogeneous coordinates for rational splines).
fn de_boor(p: usize, knots: &[f64], ctrl: &[P], w: &[f64], u: f64) -> P {
    let n = ctrl.len();
    let mut k = p;
    while k + 1 < n && knots[k + 1] <= u {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> = (0..=p)
        .map(|j| {
            let i = j + k - p;
            (ctrl[i].0 * w[i], ctrl[i].1 * w[i], w[i])
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let denom = knots[i + p + 1 - r] - knots[i];
            let a = if denom.abs() < 1e-300 {
                0.0
            } else {
                (u - knots[i]) / denom
            };
            d[j] = (
                d[j - 1].0 + (d[j].0 - d[j - 1].0) * a,
                d[j - 1].1 + (d[j].1 - d[j - 1].1) * a,
                d[j - 1].2 + (d[j].2 - d[j - 1].2) * a,
            );
        }
    }
    let (x, y, h) = d[p];
    if h.abs() < 1e-300 {
        (x, y)
    } else {
        (x / h, y / h)
    }
}

/// Polyline through the spline, subdividing each knot span until the chord
/// is within [`SAMPLE_TOL`] (in mm, after `m`).
fn sampled_spline(m: &Mat, p: usize, knots: &[f64], ctrl: &[P], w: &[f64]) -> Piece {
    let eval = |u: f64| {
        let q = de_boor(p, knots, ctrl, w, u);
        apply(m, q.0, q.1)
    };
    let (lo, hi) = (knots[p], knots[ctrl.len()]);
    let mut piece = Piece::new(eval(lo));
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for k in p..ctrl.len() {
        if knots[k + 1] > knots[k] {
            spans.push((knots[k].max(lo), knots[k + 1].min(hi)));
        }
    }
    for (a, b) in spans {
        let pa = eval(a);
        let pb = eval(b);
        subdivide(&eval, a, b, pa, pb, 0, &mut piece.segs);
    }
    piece
}

fn subdivide(
    eval: &dyn Fn(f64) -> P,
    a: f64,
    b: f64,
    pa: P,
    pb: P,
    depth: usize,
    out: &mut Vec<PSeg>,
) {
    let mid = (a + b) / 2.0;
    let pm = eval(mid);
    // Distance of the midpoint from the chord (or from pa for a null chord).
    let chord = dist(pa, pb);
    let dev = if chord < 1e-12 {
        dist(pa, pm)
    } else {
        ((pb.0 - pa.0) * (pa.1 - pm.1) - (pa.0 - pm.0) * (pb.1 - pa.1)).abs() / chord
    };
    if depth < 14 && (depth < 3 || dev > SAMPLE_TOL) {
        subdivide(eval, a, mid, pa, pm, depth + 1, out);
        subdivide(eval, mid, b, pm, pb, depth + 1, out);
    } else {
        out.push(PSeg::Line(pb));
    }
}

/// Smooth curve through fit points (Catmull-Rom), for splines given only
/// by their fit points.
fn through_points(m: &Mat, pts: &[P], closed: bool) -> Piece {
    let mut pts = pts.to_vec();
    pts.dedup_by(|a, b| dist(*a, *b) < 1e-12);
    let looped = closed || (pts.len() > 2 && dist(pts[0], pts[pts.len() - 1]) < 1e-9);
    if looped && pts.len() > 2 && dist(pts[0], pts[pts.len() - 1]) < 1e-9 {
        pts.pop();
    }
    let n = pts.len();
    let get = |i: isize| -> P {
        if looped {
            pts[i.rem_euclid(n as isize) as usize]
        } else {
            pts[i.clamp(0, n as isize - 1) as usize]
        }
    };
    let pt = |q: P| apply(m, q.0, q.1);
    let mut piece = Piece::new(pt(pts[0]));
    let count = if looped { n } else { n - 1 };
    for i in 0..count as isize {
        let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        piece.segs.push(PSeg::Cubic(pt(c1), pt(c2), pt(p2)));
    }
    piece.closed = looped;
    piece
}

// ---- Chaining ----

type Cell = (i64, i64);

fn cell_of(p: P) -> Cell {
    (
        (p.0 / JOIN_TOL).floor() as i64,
        (p.1 / JOIN_TOL).floor() as i64,
    )
}

/// Joins open pieces end to end (either direction) and closes the chains that
/// loop back. Closed pieces pass through unchanged.
fn chain(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut open = Vec::new();
    for mut p in pieces {
        if !p.closed {
            p.close_if_looped();
        }
        if p.closed { out.push(p) } else { open.push(p) }
    }
    // Endpoint grid: cells of JOIN_TOL, searched with their 8 neighbours.
    let mut grid: HashMap<Cell, Vec<(usize, bool)>> = HashMap::new();
    for (i, p) in open.iter().enumerate() {
        grid.entry(cell_of(p.start)).or_default().push((i, false));
        grid.entry(cell_of(p.end())).or_default().push((i, true));
    }
    let mut used = vec![false; open.len()];
    let find = |at: P, used: &[bool]| -> Option<(usize, bool)> {
        let (cx, cy) = cell_of(at);
        let mut best: Option<(f64, usize, bool)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = grid.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &(i, is_end) in list {
                    if used[i] {
                        continue;
                    }
                    let q = if is_end { open[i].end() } else { open[i].start };
                    let d = dist(at, q);
                    if d <= JOIN_TOL && best.is_none_or(|(bd, _, _)| d < bd) {
                        best = Some((d, i, is_end));
                    }
                }
            }
        }
        best.map(|(_, i, e)| (i, e))
    };
    for i in 0..open.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut current = open[i].clone();
        // Extend forward, then flip the chain and extend from its other end.
        for pass in 0..2 {
            if pass == 1 {
                if dist(current.end(), current.start) <= JOIN_TOL {
                    break;
                }
                current = current.reversed();
            }
            while dist(current.end(), current.start) > JOIN_TOL {
                let Some((j, at_end)) = find(current.end(), &used) else {
                    break;
                };
                used[j] = true;
                let next = if at_end {
                    open[j].reversed()
                } else {
                    open[j].clone()
                };
                current.segs.extend(next.segs);
            }
        }
        current.close_if_looped();
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{count_closed, count_subpaths, path_bbox};
    use kp_geom::Seg;

    #[test]
    fn partial_ellipse_and_nested_inserts() {
        // Half of an ellipse 20×10 (upper half, parameters 0..π).
        let e = [
            (0, "ELLIPSE"),
            (8, "0"),
            (10, "0"),
            (20, "0"),
            (30, "0"),
            (11, "10"),
            (21, "0"),
            (31, "0"),
            (40, "0.5"),
            (41, "0"),
            (42, "3.141592653589793"),
        ];
        let imp = import_dxf(&dxf(&[], &[], &e)).unwrap();
        assert!(close(imp.width, 20.0) && close(imp.height, 5.0), "{imp:?}");
        assert_eq!(count_closed(&imp.items[0].path), 0);

        // OUTER inserts INNER (a 1 mm line) twice, scaled ×2; OUTER inserted at (10, 0).
        let blocks = [
            (0, "BLOCK"),
            (8, "0"),
            (2, "INNER"),
            (10, "0"),
            (20, "0"),
            (0, "LINE"),
            (8, "0"),
            (10, "0"),
            (20, "0"),
            (11, "1"),
            (21, "0"),
            (0, "ENDBLK"),
            (0, "BLOCK"),
            (8, "0"),
            (2, "OUTER"),
            (10, "0"),
            (20, "0"),
            (0, "INSERT"),
            (8, "0"),
            (2, "INNER"),
            (10, "0"),
            (20, "0"),
            (41, "2"),
            (42, "2"),
            (0, "INSERT"),
            (8, "0"),
            (2, "inner"),
            (10, "0"),
            (20, "5"),
            (41, "2"),
            (42, "2"),
            (0, "ENDBLK"),
        ];
        let e = [
            (0, "INSERT"),
            (8, "CUT"),
            (2, "OUTER"),
            (10, "10"),
            (20, "0"),
        ];
        let imp = import_dxf(&dxf(&[], &blocks, &e)).unwrap();
        assert_eq!(imp.items.len(), 1);
        assert_eq!(count_subpaths(&imp.items[0].path), 2);
        assert!(close(imp.width, 2.0) && close(imp.height, 5.0), "{imp:?}");
        let b = path_bbox(&imp.items[0].path).unwrap();
        assert!(close(b[0], 0.0) && close(b[1], 0.0));
    }

    /// Builds a DXF from `(code, value)` pairs, one section per argument.
    fn dxf(header: &[(i32, &str)], blocks: &[(i32, &str)], entities: &[(i32, &str)]) -> Vec<u8> {
        let mut s = String::new();
        let mut put = |pairs: &[(i32, &str)]| {
            for (c, v) in pairs {
                s.push_str(&format!("{c}\n{v}\n"));
            }
        };
        if !header.is_empty() {
            put(&[(0, "SECTION"), (2, "HEADER")]);
            put(header);
            put(&[(0, "ENDSEC")]);
        }
        if !blocks.is_empty() {
            put(&[(0, "SECTION"), (2, "BLOCKS")]);
            put(blocks);
            put(&[(0, "ENDSEC")]);
        }
        put(&[(0, "SECTION"), (2, "ENTITIES")]);
        put(entities);
        put(&[(0, "ENDSEC"), (0, "EOF")]);
        s.into_bytes()
    }

    fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Vec<(i32, String)> {
        vec![
            (0, "LINE".into()),
            (8, "0".into()),
            (10, x1.to_string()),
            (20, y1.to_string()),
            (30, "0".into()),
            (11, x2.to_string()),
            (21, y2.to_string()),
            (31, "0".into()),
        ]
    }

    fn refs(v: &[(i32, String)]) -> Vec<(i32, &str)> {
        v.iter().map(|(c, s)| (*c, s.as_str())).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn four_lines_make_one_closed_square() {
        // Mixed directions and order.
        let mut e = line(0.0, 0.0, 10.0, 0.0);
        e.extend(line(0.0, 10.0, 0.0, 0.0));
        e.extend(line(10.0, 10.0, 10.0, 0.0));
        e.extend(line(0.0, 10.0, 10.0, 10.005)); // within tolerance of (10, 10)
        let imp = import_dxf(&dxf(&[], &[], &refs(&e))).unwrap();
        assert_eq!(imp.items.len(), 1);
        let path = &imp.items[0].path;
        assert_eq!(count_subpaths(path), 1, "{path:?}");
        assert_eq!(count_closed(path), 1, "{path:?}");
        assert!(
            close(imp.width, 10.0) && (imp.height - 10.0).abs() < 0.01,
            "{imp:?}"
        );
        assert_eq!(imp.items[0].fill_rule, FillRule::Evenodd);
    }

    #[test]
    fn open_chain_stays_open_and_y_flips() {
        let mut e = line(0.0, 0.0, 10.0, 0.0);
        e.extend(line(10.0, 20.0, 10.0, 0.0));
        let imp = import_dxf(&dxf(&[], &[], &refs(&e))).unwrap();
        let path = &imp.items[0].path;
        assert_eq!(count_subpaths(path), 1);
        assert_eq!(count_closed(path), 0);
        // DXF (10, 20) is the top: y = 0 in document space.
        let ends: Vec<(f64, f64)> = path
            .iter()
            .filter_map(|s| match *s {
                Seg::M { x, y } | Seg::L { x, y } => Some((x, y)),
                _ => None,
            })
            .collect();
        assert!(
            ends.iter().any(|&(x, y)| close(x, 10.0) && close(y, 0.0)),
            "{ends:?}"
        );
        assert!(
            ends.iter().any(|&(x, y)| close(x, 0.0) && close(y, 20.0)),
            "{ends:?}"
        );
    }

    #[test]
    fn circle_in_inches() {
        let header = [(9, "$INSUNITS"), (70, "1")];
        let e = [
            (0, "CIRCLE"),
            (8, "0"),
            (10, "5"),
            (20, "5"),
            (30, "0"),
            (40, "1"),
        ];
        let imp = import_dxf(&dxf(&header, &[], &e)).unwrap();
        let path = &imp.items[0].path;
        assert_eq!(count_subpaths(path), 1);
        assert_eq!(count_closed(path), 1);
        assert_eq!(
            path.iter().filter(|s| matches!(s, Seg::C { .. })).count(),
            4
        );
        assert!(close(imp.width, 50.8) && close(imp.height, 50.8), "{imp:?}");
        // On-curve points at radius 25.4 from the centre.
        for s in path {
            if let Seg::C { x, y, .. } = *s {
                assert!(close((x - 25.4).hypot(y - 25.4), 25.4));
            }
        }
    }

    #[test]
    fn arcs_bulges_and_blocks() {
        // Block: unit square as a closed LWPOLYLINE, base point (0, 0).
        let blocks = [
            (0, "BLOCK"),
            (8, "0"),
            (2, "SQ"),
            (70, "0"),
            (10, "0"),
            (20, "0"),
            (30, "0"),
            (0, "LWPOLYLINE"),
            (8, "0"),
            (90, "4"),
            (70, "1"),
            (10, "0"),
            (20, "0"),
            (10, "1"),
            (20, "0"),
            (10, "1"),
            (20, "1"),
            (10, "0"),
            (20, "1"),
            (0, "ENDBLK"),
        ];
        let e = [
            // Scaled ×10 in x, ×5 in y, rotated 90°, at (100, 0): x ∈ [95, 100], y ∈ [0, 10].
            (0, "INSERT"),
            (8, "0"),
            (2, "SQ"),
            (10, "100"),
            (20, "0"),
            (30, "0"),
            (41, "10"),
            (42, "5"),
            (50, "90"),
            // Half circle via bulge 1 from (0, 0) to (20, 0): CCW, bulges below to y = -10.
            (0, "LWPOLYLINE"),
            (8, "0"),
            (90, "2"),
            (70, "0"),
            (10, "0"),
            (20, "0"),
            (42, "1"),
            (10, "20"),
            (20, "0"),
            // 90° arc, radius 10, centre (50, 0), from 0° to 90°: x ∈ [50, 60], y ∈ [0, 10].
            (0, "ARC"),
            (8, "0"),
            (10, "50"),
            (20, "0"),
            (30, "0"),
            (40, "10"),
            (50, "0"),
            (51, "90"),
        ];
        let imp = import_dxf(&dxf(&[], &blocks, &e)).unwrap();
        // x from 0 to 100, y (DXF) from -10 to 10.
        assert!(
            close(imp.width, 100.0) && close(imp.height, 20.0),
            "{imp:?}"
        );
        let path = &imp.items[0].path;
        assert_eq!(count_subpaths(path), 3, "{path:?}");
        assert_eq!(count_closed(path), 1);
    }

    #[test]
    fn spline_is_exact() {
        // Clamped cubic with a single span = one Bézier (0,0) (0,10) (10,10) (10,0).
        let e = [
            (0, "SPLINE"),
            (8, "0"),
            (70, "8"),
            (71, "3"),
            (72, "8"),
            (73, "4"),
            (40, "0"),
            (40, "0"),
            (40, "0"),
            (40, "0"),
            (40, "1"),
            (40, "1"),
            (40, "1"),
            (40, "1"),
            (10, "0"),
            (20, "0"),
            (30, "0"),
            (10, "0"),
            (20, "10"),
            (30, "0"),
            (10, "10"),
            (20, "10"),
            (30, "0"),
            (10, "10"),
            (20, "0"),
            (30, "0"),
        ];
        let imp = import_dxf(&dxf(&[], &[], &e)).unwrap();
        let path = &imp.items[0].path;
        assert_eq!(path.len(), 2, "{path:?}");
        assert!(close(imp.width, 10.0) && close(imp.height, 7.5), "{imp:?}");
    }

    #[test]
    fn knot_insertion_matches_de_boor() {
        // Uniform (unclamped) cubic with 6 control points.
        let ctrl = vec![
            (0.0, 0.0),
            (1.0, 3.0),
            (3.0, 4.0),
            (5.0, 1.0),
            (7.0, 2.0),
            (8.0, 5.0),
        ];
        let knots: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let piece = bezier_spline(&IDENTITY, 3, knots.clone(), ctrl.clone());
        let w = vec![1.0; ctrl.len()];
        // Spans [3,4), [4,5), [5,6]: compare Bézier midpoints with de Boor.
        assert_eq!(piece.segs.len(), 3);
        let mut start = piece.start;
        assert!(dist(start, de_boor(3, &knots, &ctrl, &w, 3.0)) < 1e-9);
        for (i, s) in piece.segs.iter().enumerate() {
            let PSeg::Cubic(c1, c2, e) = *s else { panic!() };
            let mid = (
                (start.0 + 3.0 * c1.0 + 3.0 * c2.0 + e.0) / 8.0,
                (start.1 + 3.0 * c1.1 + 3.0 * c2.1 + e.1) / 8.0,
            );
            let expect = de_boor(3, &knots, &ctrl, &w, 3.5 + i as f64);
            assert!(dist(mid, expect) < 1e-9, "{mid:?} vs {expect:?}");
            start = e;
        }
        assert!(dist(start, de_boor(3, &knots, &ctrl, &w, 6.0)) < 1e-9);
    }

    #[test]
    fn rational_spline_quarter_circle() {
        // Quadratic rational quarter circle, radius 10.
        let s = (2.0f64).sqrt() / 2.0;
        let piece = sampled_spline(
            &IDENTITY,
            2,
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            &[(10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            &[1.0, s, 1.0],
        );
        for seg in &piece.segs {
            assert!((dist(seg.end(), (0.0, 0.0)) - 10.0).abs() < 1e-9);
        }
        assert!(dist(piece.end(), (0.0, 10.0)) < 1e-9);
    }

    #[test]
    fn bad_input() {
        assert!(import_dxf(b"").unwrap_err().contains("vide"));
        assert!(import_dxf(b"0\nSECTION\n2\nENTITIES\n0\nLINE\n10\nabc\n").is_err());
        assert!(import_dxf(b"garbage garbage").is_err());
        let full = dxf(&[], &[], &refs(&line(0.0, 0.0, 1.0, 1.0)));
        for cut in [5, 20, full.len() / 2, full.len() - 4] {
            let _ = import_dxf(&full[..cut]);
        }
        let empty = dxf(&[], &[], &[]);
        assert!(import_dxf(&empty).unwrap_err().contains("aucun tracé"));
        // Self-referencing block: no infinite recursion.
        let blocks = [
            (0, "BLOCK"),
            (8, "0"),
            (2, "LOOP"),
            (10, "0"),
            (20, "0"),
            (0, "INSERT"),
            (8, "0"),
            (2, "LOOP"),
            (10, "1"),
            (20, "0"),
            (0, "ENDBLK"),
        ];
        let e = [(0, "INSERT"), (8, "0"), (2, "LOOP"), (10, "0"), (20, "0")];
        assert!(import_dxf(&dxf(&[], &blocks, &e)).is_err());
    }
}
