//! Path types shared with the frontend over JSON.
//!
//! These are the wire format: `{"t":"M","x":0,"y":0}`, `{"t":"C",...}`,
//! `{"t":"Z"}`. Keep them in sync with `src/model/types.ts`.

use serde::{Deserialize, Serialize};

/// A path segment, in millimetres, document space (y down).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Seg {
    M {
        x: f64,
        y: f64,
    },
    L {
        x: f64,
        y: f64,
    },
    C {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    },
    Z,
}

/// One or more subpaths, each starting with `M`.
pub type PathData = Vec<Seg>;

/// Affine matrix `[a, b, c, d, e, f]`, as in SVG and canvas:
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
pub type Mat = [f64; 6];

pub const IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FillRule {
    #[default]
    Nonzero,
    Evenodd,
}

/// A document object as the backend sees it: a path and the transform that
/// places it on the media.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub path: PathData,
    #[serde(default = "identity")]
    pub transform: Mat,
    #[serde(default)]
    pub fill_rule: FillRule,
}

const fn identity() -> Mat {
    IDENTITY
}
