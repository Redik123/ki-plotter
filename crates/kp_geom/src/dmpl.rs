//! DMPL writer (experimental).
//!
//! Strings follow Inkcut's DMPL "mode 1": init `;:HAEC1 `, optional
//! `V{speed} BP{force} `, then one `U{x},{y} ` (pen up) or `D{x},{y} ` (pen
//! down) per point, each followed by a space. Units are the same as for HPGL
//! (`unitsPerMm`; Inkcut's ~1021 units/inch is ≈ 40/mm).

use std::fmt::Write as _;

use kurbo::Point;

use crate::hpgl::{path_units, to_units};
use crate::job::MachineSettings;
use crate::num::fmt_param;

/// Write the DMPL program for `paths` (machine mm), ending with a pen-up
/// move to `end`.
pub fn write_dmpl(paths: &[Vec<Point>], end: Point, m: &MachineSettings) -> String {
    let mut s = String::with_capacity(16 + paths.iter().map(|p| p.len() * 12).sum::<usize>());
    s.push_str(";:HAEC1 ");
    if m.send_speed_force {
        let _ = write!(s, "V{} BP{} ", fmt_param(m.speed), fmt_param(m.force));
    }
    for path in paths {
        let Some(pts) = path_units(path, m) else {
            continue;
        };
        let _ = write!(s, "U{},{} ", pts[0].0, pts[0].1);
        for (x, y) in &pts[1..] {
            let _ = write!(s, "D{x},{y} ");
        }
    }
    let (ex, ey) = to_units(end, m);
    let _ = write!(s, "U{ex},{ey} ");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_dmpl() {
        let m = MachineSettings {
            send_speed_force: true,
            speed: 12.5,
            ..Default::default()
        };
        let paths = vec![vec![Point::new(1.0, 2.0), Point::new(3.0, 2.0)]];
        let s = write_dmpl(&paths, Point::new(5.0, 0.0), &m);
        assert_eq!(s, ";:HAEC1 V12.5 BP80 U40,80 D120,80 U200,0 ");
    }
}
