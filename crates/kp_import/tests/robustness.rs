//! Garbage and truncated inputs must give errors, never panics.

use kp_import::{import_dxf, import_plt, import_svg};

/// Deterministic pseudo-random bytes.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect()
}

const SVG: &str = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
  <g transform="rotate(10 40 20)"><path d="M10 10 C 20 0 30 20 40 10 S 60 0 70 10 Z" fill="#ff0000" fill-rule="evenodd"/></g>
  <ellipse cx="40" cy="30" rx="20" ry="5" stroke="#00ff00" fill="none"/>
</svg>"##;

const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
0\nLWPOLYLINE\n8\n0\n90\n3\n70\n1\n10\n0\n20\n0\n42\n0.5\n10\n10\n20\n0\n10\n10\n20\n10\n\
0\nARC\n8\n0\n10\n0\n20\n0\n40\n5\n50\n0\n51\n180\n\
0\nSPLINE\n8\n0\n70\n8\n71\n3\n72\n8\n73\n4\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n\
10\n0\n20\n0\n10\n1\n20\n2\n10\n3\n20\n2\n10\n4\n20\n0\n0\nENDSEC\n0\nEOF\n";

const PLT: &str = "IN;SP1;PU0,0;PD400,0,400,400,0,400,0,0;PU800,800;PR;PD40,0 0,40 -40,0;PU;";

#[test]
fn valid_samples_import() {
    assert_eq!(import_svg(SVG.as_bytes()).unwrap().items.len(), 2);
    assert!(!import_dxf(DXF.as_bytes()).unwrap().items.is_empty());
    assert_eq!(import_plt(PLT).unwrap().items.len(), 1);
}

#[test]
fn truncated_and_garbled_inputs_do_not_panic() {
    for sample in [SVG, DXF, PLT] {
        let bytes = sample.as_bytes();
        for cut in (0..bytes.len()).step_by(7) {
            let part = &bytes[..cut];
            let _ = import_svg(part);
            let _ = import_dxf(part);
            let _ = import_plt(&String::from_utf8_lossy(part));
        }
        // Flip some bytes.
        for seed in 0..20u64 {
            let mut b = bytes.to_vec();
            for (k, r) in noise(seed, 8).into_iter().enumerate() {
                let i = (r as usize * 31 + k * 97) % b.len();
                b[i] = r;
            }
            let _ = import_svg(&b);
            let _ = import_dxf(&b);
            let _ = import_plt(&String::from_utf8_lossy(&b));
        }
    }
    for seed in 0..20u64 {
        let b = noise(seed, 512);
        assert!(import_svg(&b).is_err());
        let _ = import_dxf(&b);
        let _ = import_plt(&String::from_utf8_lossy(&b));
    }
}
