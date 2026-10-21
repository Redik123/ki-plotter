//! The wire rectangle `{x, y, w, h}` (mm, document space).

use serde::{Deserialize, Serialize};

use crate::num::de_f64;

/// Axis-aligned rectangle: top-left corner `(x, y)` and size `w × h`, in mm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rect {
    #[serde(deserialize_with = "de_f64")]
    pub x: f64,
    #[serde(deserialize_with = "de_f64")]
    pub y: f64,
    #[serde(deserialize_with = "de_f64")]
    pub w: f64,
    #[serde(deserialize_with = "de_f64")]
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }

    pub fn to_kurbo(self) -> kurbo::Rect {
        kurbo::Rect::new(self.x, self.y, self.x + self.w, self.y + self.h)
    }
}

impl From<kurbo::Rect> for Rect {
    fn from(r: kurbo::Rect) -> Self {
        let r = r.abs();
        Self::new(r.x0, r.y0, r.width(), r.height())
    }
}

impl From<Rect> for kurbo::Rect {
    fn from(r: Rect) -> Self {
        r.to_kurbo()
    }
}
