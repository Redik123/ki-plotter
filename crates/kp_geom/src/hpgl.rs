//! HPGL writer (and a small parser, used by the tests).
//!
//! Output is compact, without spaces or newlines:
//! `IN;` [`VS{speed};FS{force};`] then per path `PU{x},{y};PD{x},{y},…;`,
//! and finally `PU{x},{y};` at the end position (origin or feed).

use std::fmt::Write as _;

use kurbo::Point;

use crate::job::MachineSettings;
use crate::num::fmt_param;

/// Convert a machine-space point (mm) to integer plotter units:
/// `round(mm × unitsPerMm × scaleX|scaleY)`, rounding half away from zero.
/// The scales apply to the machine axes (after `swapXY`).
pub fn to_units(p: Point, m: &MachineSettings) -> (i64, i64) {
    // `as` saturates (and maps NaN to 0), so this never panics.
    let x = (p.x * m.units_per_mm * m.scale_x).round() as i64;
    let y = (p.y * m.units_per_mm * m.scale_y).round() as i64;
    (x, y)
}

/// A path in plotter units, consecutive duplicates removed. Paths that
/// collapse to a single unit point are dropped (`None`).
pub(crate) fn path_units(path: &[Point], m: &MachineSettings) -> Option<Vec<(i64, i64)>> {
    let mut v: Vec<(i64, i64)> = Vec::with_capacity(path.len());
    for &p in path {
        let u = to_units(p, m);
        if v.last() != Some(&u) {
            v.push(u);
        }
    }
    (v.len() >= 2).then_some(v)
}

/// Write the HPGL program for `paths` (machine mm, knife-centre points,
/// first point = knife down), ending with a pen-up move to `end`.
/// `PD` commands carry at most `maxPointsPerCommand` points (0 = no limit).
pub fn write_hpgl(paths: &[Vec<Point>], end: Point, m: &MachineSettings) -> String {
    let mut s = String::with_capacity(16 + paths.iter().map(|p| p.len() * 12).sum::<usize>());
    s.push_str("IN;");
    if m.send_speed_force {
        let _ = write!(s, "VS{};FS{};", fmt_param(m.speed), fmt_param(m.force));
    }
    let chunk = match m.max_points_per_command {
        0 => usize::MAX,
        n => n as usize,
    };
    for path in paths {
        let Some(pts) = path_units(path, m) else {
            continue;
        };
        let _ = write!(s, "PU{},{};", pts[0].0, pts[0].1);
        for c in pts[1..].chunks(chunk) {
            s.push_str("PD");
            for (i, (x, y)) in c.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                let _ = write!(s, "{x},{y}");
            }
            s.push(';');
        }
    }
    let (ex, ey) = to_units(end, m);
    let _ = write!(s, "PU{ex},{ey};");
    s
}

/// Parse HPGL into `(pen_down, points)` per command that carries points.
///
/// Understands `IN` (pen up), `PU`/`PD` (with or without coordinates) and
/// `PA` (absolute move with the current pen state). Other commands (`VS`,
/// `FS`, `SP`, …) are ignored. Commands may be separated by `;`, spaces or
/// newlines; coordinates by commas or spaces; decimals are rounded.
pub fn parse_hpgl(s: &str) -> Vec<(bool, Vec<(i64, i64)>)> {
    let mut out = Vec::new();
    let mut down = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let mut name = String::new();
        name.push(c.to_ascii_uppercase());
        if let Some(c2) = chars.get(i + 1).filter(|c| c.is_ascii_alphabetic()) {
            name.push(c2.to_ascii_uppercase());
            i += 2;
        } else {
            i += 1;
        }
        let start = i;
        while i < chars.len() && chars[i] != ';' && !chars[i].is_ascii_alphabetic() {
            i += 1;
        }
        let params: String = chars[start..i].iter().collect();
        let nums: Vec<f64> = params
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .collect();
        let coords: Vec<(i64, i64)> = nums
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[x, y]| (x.round() as i64, y.round() as i64))
            .collect();
        match name.as_str() {
            "IN" => down = false,
            "PU" => down = false,
            "PD" => down = true,
            "PA" => {}
            _ => continue,
        }
        if !coords.is_empty() && name != "IN" {
            out.push((down, coords));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_compact_hpgl() {
        let m = MachineSettings::default();
        let paths = vec![vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 0.0001),
            Point::new(1.0, 1.0),
        ]];
        let s = write_hpgl(&paths, Point::ZERO, &m);
        assert_eq!(s, "IN;PU0,0;PD40,0,40,40;PU0,0;");
    }

    #[test]
    fn rounding_half_away_from_zero() {
        let m = MachineSettings {
            units_per_mm: 1.0,
            ..Default::default()
        };
        assert_eq!(to_units(Point::new(2.5, -2.5), &m), (3, -3));
        assert_eq!(to_units(Point::new(f64::NAN, 1e300), &m), (0, i64::MAX));
    }

    #[test]
    fn parses_common_forms() {
        let cmds = parse_hpgl("IN;SP1;PU10,20;PD30,40,50,60;PU;PA70,80;PD;PA 1.6 2.4;\nVS10;");
        assert_eq!(
            cmds,
            vec![
                (false, vec![(10, 20)]),
                (true, vec![(30, 40), (50, 60)]),
                (false, vec![(70, 80)]),
                (true, vec![(2, 2)]),
            ]
        );
        assert!(parse_hpgl("").is_empty());
        assert!(parse_hpgl("garbage;;;1,2,3").is_empty());
    }
}
