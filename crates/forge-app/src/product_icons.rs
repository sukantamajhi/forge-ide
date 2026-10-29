//! Product icon themes in the app: load the font a theme ships, register it with the text
//! system, and tell `icons::glyph` which Codicons it replaces.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::sync::RwLock;

use forge_core::product_icons::ProductIcons;
use gpui::App;

/// Codicon character to the family and character of the active theme's replacement.
static ACTIVE: RwLock<Option<HashMap<char, (String, char)>>> = RwLock::new(None);

/// The replacement for a Codicon, when a product icon theme is active and defines one.
pub fn replacement(icon: char) -> Option<(String, char)> {
    ACTIVE.read().ok()?.as_ref()?.get(&icon).cloned()
}

/// The icon ids a theme may use for each Codicon Forge draws, most specific first.
fn ids(icon: char) -> &'static [&'static str] {
    use crate::codicons as c;
    match icon {
        c::FILES => &["files", "explorer-view-icon"],
        c::SEARCH => &["search", "search-view-icon"],
        c::SOURCE_CONTROL => &["source-control", "scm-view-icon"],
        c::SPARKLE => &["sparkle"],
        c::CHECK => &["check"],
        c::REFRESH => &["refresh"],
        c::ELLIPSIS => &["ellipsis"],
        c::SYNC => &["sync"],
        c::CHEVRON_DOWN => &["chevron-down"],
        c::CHEVRON_RIGHT => &["chevron-right"],
        c::CLOUD_UPLOAD => &["cloud-upload"],
        c::GIT_BRANCH => &["git-branch"],
        c::OPEN_PREVIEW => &["open-preview"],
        c::SPLIT_HORIZONTAL => &["split-horizontal"],
        c::CLOSE => &["close", "widget-close"],
        c::CIRCLE_FILLED => &["circle-filled"],
        c::CASE_SENSITIVE => &["case-sensitive"],
        c::WHOLE_WORD => &["whole-word"],
        c::REGEX => &["regex"],
        c::ARROW_UP => &["arrow-up"],
        c::ARROW_DOWN => &["arrow-down"],
        c::DEBUG_CONTINUE => &["debug-continue"],
        c::DEBUG_STEP_OVER => &["debug-step-over"],
        c::DEBUG_STEP_INTO => &["debug-step-into"],
        c::DEBUG_STEP_OUT => &["debug-step-out"],
        c::DEBUG_STOP => &["debug-stop"],
        c::ADD => &["add"],
        c::REMOVE => &["remove"],
        c::DISCARD => &["discard"],
        c::ERROR => &["error"],
        c::EXTENSIONS => &["extensions", "extensions-view-icon"],
        c::TRASH => &["trash"],
        c::CLOUD_DOWNLOAD => &["cloud-download"],
        _ => &[],
    }
}

const ALL: [char; 33] = [
    crate::codicons::FILES,
    crate::codicons::SEARCH,
    crate::codicons::SOURCE_CONTROL,
    crate::codicons::SPARKLE,
    crate::codicons::CHECK,
    crate::codicons::REFRESH,
    crate::codicons::ELLIPSIS,
    crate::codicons::SYNC,
    crate::codicons::CHEVRON_DOWN,
    crate::codicons::CHEVRON_RIGHT,
    crate::codicons::CLOUD_UPLOAD,
    crate::codicons::GIT_BRANCH,
    crate::codicons::OPEN_PREVIEW,
    crate::codicons::SPLIT_HORIZONTAL,
    crate::codicons::CLOSE,
    crate::codicons::CIRCLE_FILLED,
    crate::codicons::CASE_SENSITIVE,
    crate::codicons::WHOLE_WORD,
    crate::codicons::REGEX,
    crate::codicons::ARROW_UP,
    crate::codicons::ARROW_DOWN,
    crate::codicons::DEBUG_CONTINUE,
    crate::codicons::DEBUG_STEP_OVER,
    crate::codicons::DEBUG_STEP_INTO,
    crate::codicons::DEBUG_STEP_OUT,
    crate::codicons::DEBUG_STOP,
    crate::codicons::ADD,
    crate::codicons::REMOVE,
    crate::codicons::DISCARD,
    crate::codicons::ERROR,
    crate::codicons::EXTENSIONS,
    crate::codicons::TRASH,
    crate::codicons::CLOUD_DOWNLOAD,
];

/// A WOFF 1 file rewritten as the plain TrueType/OpenType file the text system loads.
fn woff_to_sfnt(woff: &[u8]) -> Result<Vec<u8>, String> {
    let u32_at = |at: usize| woff.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or("truncated WOFF file");
    let count = woff.get(12..14).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or("truncated WOFF file")? as usize;
    let flavor = u32_at(4)?;
    let mut tables = Vec::new();
    for i in 0..count {
        let at = 44 + i * 20;
        let (tag, offset, comp, orig, checksum) = (u32_at(at)?, u32_at(at + 4)? as usize, u32_at(at + 8)? as usize, u32_at(at + 12)? as usize, u32_at(at + 16)?);
        let raw = woff.get(offset..offset + comp).ok_or("truncated WOFF table")?;
        let data = if comp < orig {
            let mut out = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(raw).read_to_end(&mut out).map_err(|e| format!("bad WOFF table: {e}"))?;
            out
        } else {
            raw.to_vec()
        };
        tables.push((tag, checksum, data));
    }
    let entry_selector = (usize::BITS - 1 - count.max(1).leading_zeros()) as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let mut out = Vec::new();
    out.extend(flavor.to_be_bytes());
    out.extend((count as u16).to_be_bytes());
    out.extend(search_range.to_be_bytes());
    out.extend(entry_selector.to_be_bytes());
    out.extend((count as u16 * 16 - search_range).to_be_bytes());
    let mut offset = 12 + count * 16;
    for (tag, checksum, data) in &tables {
        out.extend(tag.to_be_bytes());
        out.extend(checksum.to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, _, data) in &tables {
        out.extend(data);
        out.resize(out.len() + (4 - data.len() % 4) % 4, 0);
    }
    Ok(out)
}

/// The font file's bytes in a form the text system accepts, and the family name it will have.
fn load_font(path: &std::path::Path) -> Result<(Vec<u8>, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = if bytes.starts_with(b"wOFF") { woff_to_sfnt(&bytes)? } else { bytes };
    let face = ttf_parser::Face::parse(&bytes, 0).map_err(|e| format!("{}: not a usable font ({e})", path.display()))?;
    let names = face.names();
    let family = [16u16, 1]
        .iter()
        .find_map(|id| names.into_iter().find(|n| n.name_id == *id && n.is_unicode()).and_then(|n| n.to_string()))
        .ok_or_else(|| format!("{}: the font has no family name", path.display()))?;
    Ok((bytes, family))
}

/// Make `theme` the active product icon theme (`None`: the built-in Codicons). On failure
/// the Codicons stay in use and the error says why.
pub fn apply(theme: Option<&ProductIcons>, cx: &mut App) -> Result<(), String> {
    let set = |map: Option<HashMap<char, (String, char)>>| {
        if let Ok(mut active) = ACTIVE.write() {
            *active = map;
        }
    };
    let Some(theme) = theme else {
        set(None);
        return Ok(());
    };
    let mut families = HashMap::new();
    let mut registered = Vec::new();
    for font in &theme.fonts {
        let (bytes, family) = load_font(&font.path).inspect_err(|_| set(None))?;
        families.insert(font.id.clone(), family);
        registered.push(Cow::Owned(bytes));
    }
    cx.text_system().add_fonts(registered).map_err(|e| {
        set(None);
        format!("could not load the theme's font: {e}")
    })?;
    let map: HashMap<char, (String, char)> = ALL
        .iter()
        .filter_map(|icon| {
            let (font, c) = theme.icon(ids(*icon))?;
            Some((*icon, (families.get(font)?.clone(), c)))
        })
        .collect();
    set(Some(map));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codicon_forge_draws_has_an_id() {
        for icon in ALL {
            assert!(!ids(icon).is_empty(), "{icon:?}");
        }
    }

    #[test]
    fn a_woff_file_becomes_a_font_the_parser_reads() {
        // Wrap the bundled TrueType font's tables as an uncompressed WOFF and read it back.
        let ttf = include_bytes!("../../../assets/fonts/codicon.ttf");
        let face = ttf_parser::RawFace::parse(ttf, 0).unwrap();
        let tables: Vec<_> = face.table_records.into_iter().collect();
        let mut woff = Vec::new();
        woff.extend(b"wOFF");
        woff.extend(0x0001_0000u32.to_be_bytes());
        woff.extend(0u32.to_be_bytes());
        woff.extend((tables.len() as u16).to_be_bytes());
        woff.extend([0u8; 2 + 4 + 12 + 4 * 4]);
        woff.truncate(44);
        let mut offset = 44 + tables.len() * 20;
        let mut data = Vec::new();
        for t in &tables {
            let bytes = face.table(t.tag).unwrap();
            woff.extend(t.tag.to_bytes());
            woff.extend((offset as u32).to_be_bytes());
            woff.extend((bytes.len() as u32).to_be_bytes());
            woff.extend((bytes.len() as u32).to_be_bytes());
            woff.extend(t.check_sum.to_be_bytes());
            data.extend(bytes);
            let pad = (4 - bytes.len() % 4) % 4;
            data.resize(data.len() + pad, 0);
            offset += bytes.len() + pad;
        }
        woff.extend(data);
        let sfnt = woff_to_sfnt(&woff).unwrap();
        let face = ttf_parser::Face::parse(&sfnt, 0).unwrap();
        assert!(face.glyph_index('\u{eaf0}').is_some(), "the files glyph survives the conversion");
    }
}
