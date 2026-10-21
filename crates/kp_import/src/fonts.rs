//! System font discovery.
//!
//! Scanning reads only the font headers and the `name` / `OS/2` tables of each
//! file (a few kilobytes) instead of whole files, so listing a typical Windows
//! install (several hundred MB of fonts) takes well under a second.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rustybuzz::ttf_parser::{self, PlatformId, Tag};
use serde::{Deserialize, Serialize};

/// A font face installed on the system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    /// `"<absolute path>#<face index>"`, to pass back to [`load_font`] or in a
    /// [`TextRequest`](crate::TextRequest).
    pub id: String,
    pub family: String,
    /// Subfamily name as the font gives it ("Regular", "Bold Italic", "Light"…).
    pub style: String,
    /// OS/2 weight class, 100–900 (400 = regular, 700 = bold).
    pub weight: u16,
    pub italic: bool,
}

/// Directories scanned (recursively) by [`list_fonts`], in priority order.
pub fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(windir.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    } else {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = &home {
            dirs.push(home.join(".local/share/fonts"));
            dirs.push(home.join(".fonts"));
        }
        dirs.push(PathBuf::from("/Library/Fonts"));
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        if let Some(home) = &home {
            dirs.push(home.join("Library/Fonts"));
        }
    }
    dirs
}

/// Lists the installed fonts: every face of every `.ttf`/`.otf`/`.ttc`/`.otc`
/// in [`font_dirs`]. Unreadable fonts are skipped, as are duplicates (same
/// family and style: the first one found wins). Sorted by family
/// (case-insensitive), then weight, then upright before italic.
pub fn list_fonts() -> Vec<FontInfo> {
    let mut files = Vec::new();
    for dir in font_dirs() {
        collect_font_files(&dir, 0, &mut files);
    }
    let mut seen_files = HashSet::new();
    let mut seen_faces = HashSet::new();
    let mut fonts = Vec::new();
    for file in files {
        if !seen_files.insert(file.clone()) {
            continue;
        }
        let Some(path_str) = file.to_str() else {
            continue; // the id must round-trip through JSON
        };
        let mut faces = Vec::new();
        let _ = scan_file(&file, &mut faces);
        for (index, face) in faces {
            let key = (face.family.to_lowercase(), face.style.to_lowercase());
            if seen_faces.insert(key) {
                fonts.push(FontInfo {
                    id: format!("{path_str}#{index}"),
                    ..face
                });
            }
        }
    }
    fonts.sort_by(|a, b| {
        a.family
            .to_lowercase()
            .cmp(&b.family.to_lowercase())
            .then(a.weight.cmp(&b.weight))
            .then(a.italic.cmp(&b.italic))
            .then(a.style.cmp(&b.style))
    });
    fonts
}

/// Reads the font file behind `id` (`"<path>#<index>"`; a bare path means
/// face 0) and checks that the face parses. Returns the file bytes and the
/// face index.
pub fn load_font(id: &str) -> Result<(Vec<u8>, u32), String> {
    let (path, index) = split_id(id);
    if path.is_empty() {
        return Err("Aucune police choisie.".to_string());
    }
    let data = std::fs::read(path)
        .map_err(|e| format!("Impossible de lire la police « {path} » : {e}"))?;
    ttf_parser::Face::parse(&data, index)
        .map_err(|e| format!("Police illisible « {path} » : {e}"))?;
    Ok((data, index))
}

/// Splits a font id into its path and face index.
pub(crate) fn split_id(id: &str) -> (&str, u32) {
    match id.rsplit_once('#') {
        Some((path, index)) if !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()) => {
            (path, index.parse().unwrap_or(0))
        }
        _ => (id, 0),
    }
}

fn collect_font_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_font_files(&path, depth + 1, out);
        } else {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                out.push(path);
            }
        }
    }
}

/// Reads `len` bytes at `offset`, checking against the file size first.
fn read_at(file: &mut File, size: u64, offset: u64, len: usize) -> Option<Vec<u8>> {
    if offset.checked_add(len as u64)? > size {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn be_u16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Appends `(face index, info)` for each readable face of the file.
fn scan_file(path: &Path, out: &mut Vec<(u32, FontInfo)>) -> Option<()> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let head = read_at(&mut file, size, 0, 12)?;
    let offsets: Vec<u32> = if &head[0..4] == b"ttcf" {
        let count = be_u32(&head[8..12]).min(1024) as usize;
        let raw = read_at(&mut file, size, 12, 4 * count)?;
        raw.as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_be_bytes(*c))
            .collect()
    } else {
        vec![0]
    };
    for (index, offset) in offsets.into_iter().enumerate() {
        if let Some(info) = scan_face(&mut file, size, offset as u64) {
            out.push((index as u32, info));
        }
    }
    Some(())
}

fn scan_face(file: &mut File, size: u64, offset: u64) -> Option<FontInfo> {
    let header = read_at(file, size, offset, 12)?;
    // TrueType, CFF ('OTTO') or old Apple TrueType ('true').
    if !matches!(
        be_u32(&header[0..4]),
        0x0001_0000 | 0x4F54_544F | 0x7472_7565
    ) {
        return None;
    }
    let num_tables = be_u16(&header[4..6]) as usize;
    if num_tables == 0 || num_tables > 1024 {
        return None;
    }
    let records = read_at(file, size, offset + 12, 16 * num_tables)?;
    let mut name_rec = None;
    let mut os2_rec = None;
    let mut has = HashSet::new();
    for r in records.as_chunks::<16>().0 {
        let tag = Tag::from_bytes(&[r[0], r[1], r[2], r[3]]);
        let rec = (be_u32(&r[8..12]) as u64, be_u32(&r[12..16]) as usize);
        has.insert(tag);
        if tag == Tag::from_bytes(b"name") {
            name_rec = Some(rec);
        } else if tag == Tag::from_bytes(b"OS/2") {
            os2_rec = Some(rec);
        }
    }
    // What rustybuzz/ttf-parser need to draw text.
    let required = [b"cmap", b"head", b"hhea", b"hmtx", b"maxp"];
    if !required.iter().all(|t| has.contains(&Tag::from_bytes(t))) {
        return None;
    }
    let outlines = (has.contains(&Tag::from_bytes(b"glyf"))
        && has.contains(&Tag::from_bytes(b"loca")))
        || has.contains(&Tag::from_bytes(b"CFF "))
        || has.contains(&Tag::from_bytes(b"CFF2"));
    if !outlines {
        return None;
    }

    let (name_off, name_len) = name_rec?;
    let name_data = read_at(file, size, name_off, name_len.min(4 << 20))?;
    let names = ttf_parser::name::Table::parse(&name_data)?.names;
    let typo = (best_name(names, 16), best_name(names, 17));
    let (family, style) = match typo {
        (Some(f), Some(s)) => (f, s),
        // ID 16 without 17 is ambiguous: keep the legacy pair together.
        _ => (
            best_name(names, 1)?,
            best_name(names, 2).unwrap_or_else(|| "Regular".into()),
        ),
    };

    let os2 = os2_rec
        .and_then(|(off, len)| read_at(file, size, off, len.min(4096)))
        .and_then(|data| {
            let table = ttf_parser::os2::Table::parse(&data)?;
            Some((table.weight().to_number(), table.style()))
        });
    let lower = style.to_lowercase();
    let (weight, italic) = match os2 {
        Some((w, s)) => (
            normalize_weight(w),
            s != ttf_parser::Style::Normal || lower.contains("italic") || lower.contains("oblique"),
        ),
        None => (
            if lower.contains("bold") { 700 } else { 400 },
            lower.contains("italic") || lower.contains("oblique"),
        ),
    };

    Some(FontInfo {
        id: String::new(),
        family,
        style,
        weight,
        italic,
    })
}

/// Some old fonts use 1–9 instead of 100–900; 0 means unset.
fn normalize_weight(w: u16) -> u16 {
    match w {
        0 => 400,
        1..=9 => w * 100,
        _ => w.min(1000),
    }
}

/// Picks the best name record with `name_id`: English (Windows, en-US)
/// first, then other English, then any Unicode name.
fn best_name(names: ttf_parser::name::Names, name_id: u16) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for name in names {
        if name.name_id != name_id {
            continue;
        }
        let score = match name.platform_id {
            PlatformId::Windows if name.language_id == 0x0409 => 0,
            PlatformId::Windows if name.language_id & 0x3FF == 0x09 => 1,
            PlatformId::Unicode => 2,
            PlatformId::Macintosh if name.language_id == 0 => 3,
            PlatformId::Windows => 4,
            _ => 5,
        };
        if best.as_ref().is_some_and(|(s, _)| *s <= score) {
            continue;
        }
        let text = if name.platform_id == PlatformId::Macintosh && name.encoding_id == 0 {
            Some(mac_roman(name.name))
        } else {
            name.to_string()
        };
        if let Some(text) = text {
            let text = text.trim().trim_matches('\0').trim();
            if !text.is_empty() {
                best = Some((score, text.to_string()));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// Mac Roman, the encoding of old Macintosh name records.
fn mac_roman(bytes: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
                        ¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{f8ff}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                HIGH.chars().nth(b as usize - 0x80).unwrap_or('?')
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(
            split_id(r"C:\Windows\Fonts\arial.ttf#0"),
            (r"C:\Windows\Fonts\arial.ttf", 0)
        );
        assert_eq!(split_id("/x/cambria.ttc#1"), ("/x/cambria.ttc", 1));
        assert_eq!(split_id("/x/a#b.ttf"), ("/x/a#b.ttf", 0));
        assert_eq!(split_id("/x/a#b.ttf#2"), ("/x/a#b.ttf", 2));
    }

    #[test]
    fn mac_roman_table_is_complete() {
        assert_eq!(mac_roman(&[0x80, 0x8e, 0xff, b'A']), "ÄéˇA");
    }

    #[test]
    fn bad_font_ids() {
        assert!(load_font("").is_err());
        assert!(
            load_font("Z:/nope/nope.ttf#0")
                .unwrap_err()
                .starts_with("Impossible de lire")
        );
        // A file that exists but is not a font.
        let path =
            std::env::temp_dir().join(format!("kp_import_not_a_font_{}.ttf", std::process::id()));
        std::fs::write(&path, b"definitely not a font").unwrap();
        let err = load_font(path.to_str().unwrap()).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(err.starts_with("Police illisible"), "{err}");
    }

    #[test]
    fn lists_system_fonts() {
        let t = std::time::Instant::now();
        let fonts = list_fonts();
        let elapsed = t.elapsed();
        eprintln!("{} fonts in {elapsed:?}", fonts.len());
        if !Path::new(r"C:\Windows\Fonts\arial.ttf").exists() {
            return;
        }
        assert!(fonts.len() > 10);
        let arial = fonts
            .iter()
            .find(|f| f.family == "Arial" && f.style == "Regular")
            .expect("Arial Regular");
        assert_eq!(arial.weight, 400);
        assert!(!arial.italic);
        assert!(
            arial.id.to_lowercase().ends_with("arial.ttf#0"),
            "{}",
            arial.id
        );
        let bold_italic = fonts
            .iter()
            .find(|f| f.family == "Arial" && f.style == "Bold Italic")
            .expect("Arial Bold Italic");
        assert_eq!(bold_italic.weight, 700);
        assert!(bold_italic.italic);
        // Sorted, no duplicates.
        for w in fonts.windows(2) {
            assert!(w[0].family.to_lowercase() <= w[1].family.to_lowercase());
        }
        let keys: HashSet<_> = fonts.iter().map(|f| (&f.family, &f.style)).collect();
        assert_eq!(keys.len(), fonts.len());
        // Every listed face loads.
        for f in fonts.iter().take(50) {
            load_font(&f.id).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}
