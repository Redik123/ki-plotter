//! A job held by the relay, and the relay's own test cut.

use std::sync::Arc;

use kp_geom::{FillRule, IDENTITY, JobRequest, Seg, Shape};
use kp_link::Flow;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub estimated_seconds: f64,
    /// Unix time it arrived.
    pub received: u64,
    /// Address of the computer that sent it ("" for jobs made on the relay).
    pub client: String,
    /// The relay's own test cut (never becomes the "last job").
    pub test: bool,
    /// Per-job line settings from the sender (empty/0/None = relay settings).
    pub device: String,
    pub baud: u32,
    pub flow: Option<Flow>,
    pub chunk_size: usize,
    pub chunk_delay_ms: u64,
    #[serde(skip)]
    pub bytes: Arc<Vec<u8>>,
}

/// The view the panel and the desktop get (no bytes).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: u64,
    pub name: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub estimated_seconds: f64,
    pub received: u64,
    pub client: String,
    pub test: bool,
    pub bytes: usize,
}

impl Job {
    pub fn view(&self) -> JobView {
        JobView {
            id: self.id,
            name: self.name.clone(),
            width_mm: self.width_mm,
            height_mm: self.height_mm,
            estimated_seconds: self.estimated_seconds,
            received: self.received,
            client: self.client.clone(),
            test: self.test,
            bytes: self.bytes.len(),
        }
    }
}

/// The same asymmetric test as the desktop's: a 25 mm square with a
/// triangle pointing along the feed, near its top.
fn test_shapes() -> Vec<Shape> {
    let poly = |pts: &[(f64, f64)]| {
        let mut path: Vec<Seg> = pts
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                if i == 0 {
                    Seg::M { x, y }
                } else {
                    Seg::L { x, y }
                }
            })
            .collect();
        path.push(Seg::Z);
        Shape {
            path,
            transform: IDENTITY,
            fill_rule: FillRule::Nonzero,
        }
    };
    vec![
        poly(&[(0.0, 0.0), (25.0, 0.0), (25.0, 25.0), (0.0, 25.0)]),
        poly(&[(6.0, 5.0), (19.0, 9.0), (6.0, 13.0)]),
    ]
}

/// Builds the test cut with the desktop's last known settings (protocol,
/// axes, blade offset), so it cuts like a test sent from the PC.
pub fn test_cut(settings: Option<&serde_json::Value>) -> Result<Job, String> {
    let mut req = JobRequest {
        shapes: test_shapes(),
        ..JobRequest::default()
    };
    if let Some(s) = settings {
        if let Some(c) = s
            .get("cut")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
        {
            req.cut = c;
        }
        if let Some(m) = s
            .get("machine")
            .and_then(|m| serde_json::from_value(m.clone()).ok())
        {
            req.machine = m;
        }
    }
    // A test always starts at the blade, never mirrored.
    req.cut.from_origin = true;
    req.cut.mirror = false;
    req.cut.passes = 1;
    let res = kp_geom::build_job(&req)?;
    Ok(Job {
        name: "Coupe test 25 mm".into(),
        width_mm: res.stats.bounds.w,
        height_mm: res.stats.bounds.h,
        estimated_seconds: res.stats.estimated_seconds,
        test: true,
        bytes: Arc::new(res.data.into_bytes()),
        ..Job::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cut_uses_the_desktop_protocol() {
        let hpgl = test_cut(None).unwrap();
        assert!(String::from_utf8_lossy(&hpgl.bytes).starts_with("IN;"));
        assert!((hpgl.width_mm - 25.0).abs() < 0.01);

        let dmpl = serde_json::json!({"machine": {"protocol": "dmpl"}, "cut": {"mirror": true}});
        let job = test_cut(Some(&dmpl)).unwrap();
        assert!(String::from_utf8_lossy(&job.bytes).starts_with(";:"));
        assert!(job.test);
    }
}
