//! Text to outlines, shaped with rustybuzz (kerning, ligatures, marks).
//!
//! Layout, in mm, y down, origin at the top-left of the first line box:
//! - line `i` has its baseline at `y = ascent + i × lineHeight`, where
//!   `lineHeight = (ascender + |descender| + lineGap) × scale × lineSpacing`
//!   and `scale = size / unitsPerEm`; ascender/descender/lineGap are the
//!   font's horizontal metrics as ttf-parser reports them (`hhea`, or the OS/2
//!   typographic values when the font sets `USE_TYPO_METRICS`);
//! - `width` is the advance of the widest line (letter spacing included);
//! - `height = ascent + descent + (lines − 1) × lineHeight`: from the top of
//!   the first line's ascent to the bottom of the last line's descent;
//! - lines are aligned within `width` (left, centre, right).
//!
//! The outline is deliberately *not* moved to its own bounding box, so the text
//! stays put while the user types.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use kp_geom::PathData;
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use rustybuzz::{Face, UnicodeBuffer};
use serde::{Deserialize, Serialize};

use crate::fonts;
use crate::util::PathBuilder;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRequest {
    /// A [`FontInfo::id`](crate::FontInfo::id).
    pub font_id: String,
    /// Lines separated by `\n` (`\r` is ignored).
    pub text: String,
    /// Em size in mm.
    pub size: f64,
    /// Extra space after each glyph cluster except the last of a line, mm.
    #[serde(default)]
    pub letter_spacing: f64,
    /// Multiplier of the font's natural line height.
    #[serde(default = "one")]
    pub line_spacing: f64,
    #[serde(default)]
    pub align: Align,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextOutline {
    /// Glyph outlines (fill rule nonzero), see the module docs for the layout.
    pub path: PathData,
    /// Advance of the widest line, mm.
    pub width: f64,
    /// `ascent + descent + (lines − 1) × lineHeight`, mm.
    pub height: f64,
    /// Font ascender at this size, mm (the first baseline is at `y = ascent`).
    pub ascent: f64,
    /// Font descender at this size, positive, mm.
    pub descent: f64,
}

/// Font files already read, by font id, so that re-shaping on every keystroke
/// does not hit the disk.
static FONT_CACHE: LazyLock<Mutex<HashMap<String, Arc<Vec<u8>>>>> = LazyLock::new(Default::default);

/// Fonts kept in [`FONT_CACHE`] before it is cleared.
const FONT_CACHE_MAX: usize = 32;

fn cached_font(id: &str) -> Result<(Arc<Vec<u8>>, u32), String> {
    let (_, index) = fonts::split_id(id);
    let lock = || FONT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(data) = lock().get(id) {
        return Ok((data.clone(), index));
    }
    let (data, index) = fonts::load_font(id)?;
    let data = Arc::new(data);
    let mut cache = lock();
    if cache.len() >= FONT_CACHE_MAX {
        cache.clear();
    }
    cache.insert(id.to_string(), data.clone());
    Ok((data, index))
}

/// A glyph placed on its line: id and pen position (x right, y up), mm.
struct Placed {
    id: GlyphId,
    x: f64,
    y: f64,
}

/// Converts `req.text` to outlines; see the module docs for the layout.
pub fn text_outline(req: &TextRequest) -> Result<TextOutline, String> {
    if !(req.size.is_finite() && req.size > 0.0) {
        return Err("Taille de texte invalide.".to_string());
    }
    let letter_spacing = if req.letter_spacing.is_finite() {
        req.letter_spacing
    } else {
        0.0
    };
    let line_spacing = if req.line_spacing.is_finite() {
        req.line_spacing
    } else {
        1.0
    };

    let (data, index) = cached_font(&req.font_id)?;
    let face = Face::from_slice(&data, index)
        .ok_or_else(|| format!("Police illisible « {} ».", req.font_id))?;
    let upem = face.units_per_em() as f64;
    if upem <= 0.0 {
        return Err(format!("Police illisible « {} ».", req.font_id));
    }
    let scale = req.size / upem;
    let asc = face.ascender() as f64;
    let desc = (face.descender() as f64).abs();
    let gap = (face.line_gap() as f64).max(0.0);
    let ascent = asc * scale;
    let descent = desc * scale;
    let line_height = (asc + desc + gap) * scale * line_spacing;

    // Shape every line first: alignment needs the widest one.
    let mut lines: Vec<(Vec<Placed>, f64)> = Vec::new();
    for line in req.text.split('\n') {
        let line: String = line.chars().filter(|&c| c != '\r').collect();
        if line.is_empty() {
            lines.push((Vec::new(), 0.0));
            continue;
        }
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(&line);
        buffer.guess_segment_properties();
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let infos = shaped.glyph_infos();
        let positions = shaped.glyph_positions();
        let mut glyphs = Vec::with_capacity(infos.len());
        let mut pen = 0.0;
        for (k, (info, pos)) in infos.iter().zip(positions).enumerate() {
            glyphs.push(Placed {
                id: GlyphId(info.glyph_id as u16),
                x: pen + pos.x_offset as f64 * scale,
                y: pos.y_offset as f64 * scale,
            });
            pen += pos.x_advance as f64 * scale;
            // Spacing between clusters, so marks stay on their base letter.
            if infos
                .get(k + 1)
                .is_some_and(|next| next.cluster != info.cluster)
            {
                pen += letter_spacing;
            }
        }
        lines.push((glyphs, pen));
    }

    let width = lines.iter().map(|(_, w)| *w).fold(0.0, f64::max);
    let mut builder = PathBuilder::new();
    for (i, (glyphs, advance)) in lines.iter().enumerate() {
        let dx = match req.align {
            Align::Left => 0.0,
            Align::Center => (width - advance) / 2.0,
            Align::Right => width - advance,
        };
        let baseline = ascent + i as f64 * line_height;
        for g in glyphs {
            // .notdef (missing character): keep the advance, cut no box.
            if g.id.0 == 0 {
                continue;
            }
            let mut sink = GlyphSink {
                b: &mut builder,
                scale,
                ox: dx + g.x,
                oy: baseline - g.y,
            };
            face.outline_glyph(g.id, &mut sink);
        }
    }

    Ok(TextOutline {
        path: builder.finish(),
        width,
        height: ascent + descent + (lines.len().max(1) - 1) as f64 * line_height,
        ascent,
        descent,
    })
}

/// Receives a glyph outline in font units (y up) and writes it in mm (y down).
struct GlyphSink<'a> {
    b: &'a mut PathBuilder,
    scale: f64,
    ox: f64,
    oy: f64,
}

impl GlyphSink<'_> {
    fn p(&self, x: f32, y: f32) -> (f64, f64) {
        (
            self.ox + x as f64 * self.scale,
            self.oy - y as f64 * self.scale,
        )
    }
}

impl OutlineBuilder for GlyphSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.b.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.b.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x, y) = self.p(x, y);
        self.b.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x2, y2) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.b.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.b.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{count_closed, count_subpaths, path_bbox};

    const ARIAL: &str = r"C:\Windows\Fonts\arial.ttf";

    fn arial() -> Option<String> {
        std::path::Path::new(ARIAL)
            .exists()
            .then(|| format!("{ARIAL}#0"))
    }

    fn req(font_id: &str, text: &str) -> TextRequest {
        TextRequest {
            font_id: font_id.to_string(),
            text: text.to_string(),
            size: 100.0,
            letter_spacing: 0.0,
            line_spacing: 1.0,
            align: Align::Left,
        }
    }

    #[test]
    fn kerning_applies() {
        let Some(id) = arial() else { return };
        let av = text_outline(&req(&id, "AV")).unwrap();
        let a = text_outline(&req(&id, "A")).unwrap();
        let v = text_outline(&req(&id, "V")).unwrap();
        assert!(
            av.width < a.width + v.width - 1.0,
            "{} vs {}",
            av.width,
            a.width + v.width
        );
    }

    #[test]
    fn metrics_and_contours() {
        let Some(id) = arial() else { return };
        let o = text_outline(&req(&id, "O")).unwrap();
        assert_eq!(count_subpaths(&o.path), 2);
        assert_eq!(count_closed(&o.path), 2);
        assert!((o.ascent - 90.5).abs() < 1.0, "ascent {}", o.ascent);
        assert!((o.descent - 21.2).abs() < 1.0, "descent {}", o.descent);
        assert!((o.height - (o.ascent + o.descent)).abs() < 1e-9);
        // Y down: the letter sits above the baseline (y = ascent), cap height ≈ 72 mm.
        let b = path_bbox(&o.path).unwrap();
        assert!(b[3] <= o.ascent + 2.0 && b[3] > o.ascent - 2.0, "{b:?}");
        assert!((o.ascent - b[1] - 72.0).abs() < 3.0, "{b:?}");
    }

    #[test]
    fn lines_alignment_and_spacing() {
        let Some(id) = arial() else { return };
        let mut r = req(&id, "MMMM\r\nI");
        let left = text_outline(&r).unwrap();
        let line_height = 100.0 * (1854.0 + 434.0 + 67.0) / 2048.0;
        assert!((left.height - (left.ascent + left.descent + line_height)).abs() < 1e-6);
        // The "I" is the last subpath.
        let last = |p: &PathData| {
            let i = p
                .iter()
                .rposition(|s| matches!(s, kp_geom::Seg::M { .. }))
                .unwrap();
            path_bbox(&p[i..].to_vec()).unwrap()
        };
        let i_left = last(&left.path);
        r.align = Align::Right;
        let right = text_outline(&r).unwrap();
        assert!((right.width - left.width).abs() < 1e-9);
        let i_right = last(&right.path);
        assert!(i_right[0] > i_left[0] + 100.0);
        // Second line baseline one line height lower.
        assert!(i_left[3] > left.ascent + line_height - 1.0);

        let mut spaced = req(&id, "II");
        let plain = text_outline(&spaced).unwrap();
        spaced.letter_spacing = 5.0;
        let wide = text_outline(&spaced).unwrap();
        assert!((wide.width - plain.width - 5.0).abs() < 1e-9);
    }

    #[test]
    fn empty_and_errors() {
        let Some(id) = arial() else { return };
        let e = text_outline(&req(&id, "")).unwrap();
        assert!(e.path.is_empty());
        assert_eq!(e.width, 0.0);
        let mut bad = req(&id, "x");
        bad.size = 0.0;
        assert!(text_outline(&bad).is_err());
        assert!(text_outline(&req("Z:/nope.ttf#0", "x")).is_err());
    }

    #[test]
    fn json_names() {
        let r: TextRequest = serde_json::from_str(
            r#"{"fontId":"a#0","text":"x","size":10,"letterSpacing":1,"lineSpacing":1.2,"align":"center"}"#,
        )
        .unwrap();
        assert_eq!(r.align, Align::Center);
        assert_eq!(r.line_spacing, 1.2);
        let json = serde_json::to_value(TextOutline::default()).unwrap();
        for key in ["path", "width", "height", "ascent", "descent"] {
            assert!(json.get(key).is_some(), "{key}");
        }
    }
}
