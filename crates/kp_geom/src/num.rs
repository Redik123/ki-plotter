//! Small numeric helpers: sanitising values coming from the UI, and lenient
//! serde deserialisers.
//!
//! `JSON.stringify` turns `NaN`/`Infinity` into `null`, and number inputs can
//! produce fractional or negative counts. The request types therefore accept
//! `null` for every number (read as `NaN`) and any number for counts; the
//! pipeline then replaces invalid values with defaults via `sanitized()`.

use serde::{Deserialize, Deserializer};

/// Largest coordinate magnitude accepted anywhere, in mm (1 km). Geometry
/// beyond this is treated as invalid and dropped, which also keeps curve
/// flattening from exploding.
pub const MAX_COORD: f64 = 1.0e6;

/// `v` if it is finite, else `default`.
#[inline]
pub fn finite_or(v: f64, default: f64) -> f64 {
    if v.is_finite() { v } else { default }
}

/// `v` if it is finite and `> 0`, else `default`.
#[inline]
pub fn positive_or(v: f64, default: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { default }
}

/// `v` if it is finite and `>= 0`, else `0`.
#[inline]
pub fn non_negative(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// Deserialise an `f64`, accepting `null` (read as `NaN`).
pub fn de_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
}

/// Deserialise a count from any JSON number (rounded; negative, `null` or
/// non-finite give `0`).
pub fn de_count<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = Option::<f64>::deserialize(d)?.unwrap_or(0.0);
    Ok(if v.is_finite() && v > 0.0 {
        v.round().min(u32::MAX as f64) as u32
    } else {
        0
    })
}

/// Format a number as an integer when whole, else with up to 2 decimals
/// (trailing zeros removed). Used for speed/force parameters.
pub fn fmt_param(v: f64) -> String {
    let r = (finite_or(v, 0.0) * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        let s = format!("{r:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_formatting() {
        assert_eq!(fmt_param(20.0), "20");
        assert_eq!(fmt_param(12.5), "12.5");
        assert_eq!(fmt_param(12.345), "12.35");
        assert_eq!(fmt_param(0.1), "0.1");
        assert_eq!(fmt_param(f64::NAN), "0");
    }

    #[test]
    fn sanitisers() {
        assert_eq!(finite_or(f64::NAN, 2.0), 2.0);
        assert_eq!(positive_or(-1.0, 3.0), 3.0);
        assert_eq!(positive_or(0.5, 3.0), 0.5);
        assert_eq!(non_negative(f64::INFINITY), 0.0);
        assert_eq!(non_negative(-4.0), 0.0);
    }
}
