//! What a font file says about itself (its family, style, version, designer, and how many
//! characters it draws), read with `ttf-parser`. TrueType and OpenType only: WOFF files are
//! compressed and shown by the browser without these details.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    pub family: Option<String>,
    pub style: Option<String>,
    pub full_name: Option<String>,
    pub version: Option<String>,
    pub designer: Option<String>,
    pub copyright: Option<String>,
    pub glyphs: u16,
    pub monospaced: bool,
    pub variable: bool,
}

pub fn read(path: &Path) -> Result<FontInfo, String> {
    let size = std::fs::metadata(path).map_err(|_| "Could not read the font".to_string())?.len();
    if size > 64 * 1024 * 1024 {
        return Err("This font file is too large".into());
    }
    let bytes = std::fs::read(path).map_err(|_| "Could not read the font".to_string())?;
    let face = ttf_parser::Face::parse(&bytes, 0).map_err(|_| "This font could not be read".to_string())?;
    let name = |id: u16| {
        let names = face.names();
        let found = names.into_iter().filter(|n| n.name_id == id);
        // English (or the Unicode platform) first, then any language.
        let mut best: Option<String> = None;
        for record in found {
            if let Some(text) = record.to_string() {
                let english = record.language_id == 0x0409 || record.platform_id == ttf_parser::PlatformId::Unicode;
                if english || best.is_none() {
                    best = Some(text);
                    if english {
                        break;
                    }
                }
            }
        }
        best.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    };
    use ttf_parser::name_id;
    Ok(FontInfo {
        family: name(name_id::TYPOGRAPHIC_FAMILY).or_else(|| name(name_id::FAMILY)),
        style: name(name_id::TYPOGRAPHIC_SUBFAMILY).or_else(|| name(name_id::SUBFAMILY)),
        full_name: name(name_id::FULL_NAME),
        version: name(name_id::VERSION),
        designer: name(name_id::DESIGNER).or_else(|| name(name_id::MANUFACTURER)),
        copyright: name(name_id::COPYRIGHT_NOTICE),
        glyphs: face.number_of_glyphs(),
        monospaced: face.is_monospaced(),
        variable: face.is_variable(),
    })
}
