//! Minimal HPGL reader, to reopen `.plt` files written by SignMaster and other
//! cutter software.
//!
//! Understood: `IN`, `PU`, `PD`, `PA`, `PR` with comma- or space-separated
//! coordinates, terminated by `;`, a newline or the next command. Everything
//! else (`SP`, `VS`, `FS`, …) is skipped with its parameters; `LB` labels are
//! skipped up to their terminator, and device-control escapes (`ESC . …`) are
//! ignored. 40 plotter units per mm, y up (flipped). Each pen-down run is a
//! subpath, closed when it ends on its start point. One item, evenodd.

use kp_geom::FillRule;

use crate::util::{self, PathBuilder};
use crate::{Imported, ImportedItem};

/// HPGL plotter units per millimetre.
const UNITS_PER_MM: f64 = 40.0;
/// A run ending this close to its start (plotter units) is closed.
const CLOSE_TOL: f64 = 0.5;

struct Plotter {
    builder: PathBuilder,
    pos: (f64, f64),
    pen_down: bool,
    absolute: bool,
    /// Start of the current pen-down run (plotter units).
    run: Option<(f64, f64)>,
}

impl Plotter {
    fn new() -> Self {
        Plotter {
            builder: PathBuilder::new(),
            pos: (0.0, 0.0),
            pen_down: false,
            absolute: true,
            run: None,
        }
    }

    fn end_run(&mut self) {
        if let Some(start) = self.run.take() {
            let (dx, dy) = (self.pos.0 - start.0, self.pos.1 - start.1);
            if dx.abs() <= CLOSE_TOL && dy.abs() <= CLOSE_TOL {
                self.builder.close();
            }
        }
    }

    fn pen(&mut self, down: bool) {
        if !down {
            self.end_run();
        }
        self.pen_down = down;
    }

    fn go(&mut self, x: f64, y: f64) {
        let target = if self.absolute {
            (x, y)
        } else {
            (self.pos.0 + x, self.pos.1 + y)
        };
        if !(target.0.is_finite() && target.1.is_finite()) {
            return;
        }
        if self.pen_down {
            if target != self.pos {
                if self.run.is_none() {
                    self.builder
                        .move_to(self.pos.0 / UNITS_PER_MM, -self.pos.1 / UNITS_PER_MM);
                    self.run = Some(self.pos);
                }
                self.builder
                    .line_to(target.0 / UNITS_PER_MM, -target.1 / UNITS_PER_MM);
            }
        } else {
            self.end_run();
        }
        self.pos = target;
    }

    fn finish(mut self) -> kp_geom::PathData {
        self.end_run();
        self.builder.finish()
    }
}

/// Parses HPGL text into one item (mm, y down, normalised to `(0, 0)`).
pub fn import_plt(text: &str) -> Result<Imported, String> {
    let mut plotter = Plotter::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == 0x1b {
            // Device control: ESC . <char> [params] [: or terminator].
            i += 1;
            if bytes.get(i) == Some(&b'.') {
                i += 2;
            }
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() && bytes[i] != 0x1b {
                let done = matches!(bytes[i], b':' | b';');
                i += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        if !c.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let Some(&c2) = bytes.get(i + 1).filter(|b| b.is_ascii_alphabetic()) else {
            i += 1;
            continue;
        };
        let cmd = [c.to_ascii_uppercase(), c2.to_ascii_uppercase()];
        i += 2;
        if &cmd == b"LB" {
            // Label text runs to ETX (the default terminator).
            while i < bytes.len() && bytes[i] != 0x03 {
                i += 1;
            }
            i += 1;
            continue;
        }
        // Parameters run to ';', a newline, or the next letter.
        let start = i;
        while i < bytes.len()
            && !matches!(bytes[i], b';' | b'\n' | b'\r' | 0x1b)
            && !bytes[i].is_ascii_alphabetic()
        {
            i += 1;
        }
        let params: Vec<f64> = text[start..i]
            .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        match &cmd {
            b"IN" | b"DF" => {
                plotter.pen(false);
                plotter.absolute = true;
                plotter.pos = (0.0, 0.0);
            }
            b"PU" | b"PD" | b"PA" | b"PR" => {
                match &cmd {
                    b"PU" => plotter.pen(false),
                    b"PD" => plotter.pen(true),
                    b"PA" => plotter.absolute = true,
                    _ => plotter.absolute = false,
                }
                for &[x, y] in params.as_chunks::<2>().0 {
                    plotter.go(x, y);
                }
            }
            _ => {}
        }
    }
    let path = plotter.finish();
    let imported = util::normalize(vec![ImportedItem {
        path,
        color: None,
        fill_rule: FillRule::Evenodd,
    }]);
    if imported.items.is_empty() {
        return Err("Le fichier PLT ne contient aucun tracé.".to_string());
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{count_closed, count_subpaths};
    use kp_geom::Seg;

    #[test]
    fn closed_square() {
        let imp = import_plt("IN;VS10;FS50;SP1;PU0,0;PD400,0,400,400,0,400,0,0;PU0,0;").unwrap();
        let path = &imp.items[0].path;
        assert_eq!(count_subpaths(path), 1);
        assert_eq!(count_closed(path), 1);
        assert_eq!((imp.width, imp.height), (10.0, 10.0));
    }

    #[test]
    fn relative_spaces_and_flip() {
        // Vertical line going up 10 mm, then a separate relative run.
        let imp = import_plt("IN\nPU 0 0\nPD 0 400\nPU\nPR 800 0\nPD 400 0\nPU\n").unwrap();
        let path = &imp.items[0].path;
        assert_eq!(count_subpaths(path), 2);
        assert_eq!(count_closed(path), 0);
        // y up → down: the line starts at the bottom (y = 10) and goes to y = 0.
        assert_eq!(path[0], Seg::M { x: 0.0, y: 10.0 });
        assert_eq!(path[1], Seg::L { x: 0.0, y: 0.0 });
        assert_eq!(path[2], Seg::M { x: 20.0, y: 0.0 });
        assert_eq!(path[3], Seg::L { x: 30.0, y: 0.0 });
        assert_eq!((imp.width, imp.height), (30.0, 10.0));
    }

    #[test]
    fn pa_after_pd_and_labels() {
        let imp = import_plt("IN;PU0,0;PD;PA400,0;PA400,400;LBPD9999,9999\x03;PU;").unwrap();
        let path = &imp.items[0].path;
        assert_eq!(path.len(), 3);
        assert_eq!((imp.width, imp.height), (10.0, 10.0));
    }

    #[test]
    fn junk() {
        assert!(import_plt("").is_err());
        assert!(import_plt("IN;SP1;PU100,100;").is_err());
        assert!(import_plt("\x1b.(;\x1b.I81;;17:IN;PD1,x,;;;PU,,,;ZZ").is_err());
        let _ = import_plt("PD1e308,1e308,-1e308,-1e308;PD1,1;");
    }
}
