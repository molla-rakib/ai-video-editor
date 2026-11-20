//! Fonts bundled with the app, for the UI and for text in videos.

use std::borrow::Cow;

/// Which writing a text font is made for (its place in the font lists).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    English,
    Bangla,
}

/// The font families text in videos can use, English first, then Bangla: the inspector's font
/// lists and the AI tools' schema read this. Each name must be a family in `BUNDLED`
/// (`every_text_font_is_bundled` checks). All are from Google Fonts, under the SIL Open Font License.
pub const TEXT_FONTS: [(&str, Script); 15] = [
    ("Inter", Script::English),
    ("Poppins", Script::English),
    ("Montserrat", Script::English),
    ("Roboto", Script::English),
    ("Lato", Script::English),
    ("Space Grotesk", Script::English),
    ("Playfair Display", Script::English),
    ("Bebas Neue", Script::English),
    ("Hind Siliguri", Script::Bangla),
    ("Noto Sans Bengali", Script::Bangla),
    ("Noto Serif Bengali", Script::Bangla),
    ("Anek Bangla", Script::Bangla),
    ("Baloo Da 2", Script::Bangla),
    ("Tiro Bangla", Script::Bangla),
    ("Galada", Script::Bangla),
];

/// Just the names of `TEXT_FONTS`, in the same order.
pub const TEXT_FONT_NAMES: [&str; TEXT_FONTS.len()] = {
    let mut names = [""; TEXT_FONTS.len()];
    let mut i = 0;
    while i < names.len() {
        names[i] = TEXT_FONTS[i].0;
        i += 1;
    }
    names
};

macro_rules! font {
    ($file:literal) => {
        include_bytes!(concat!("../assets/fonts/", $file, ".ttf"))
    };
}

/// Every font file the app carries. A family missing a weight falls back to its nearest one
/// (most families here carry Regular and Bold; Bebas Neue, Tiro Bangla and Galada come in one weight).
pub const BUNDLED: [&[u8]; 33] = [
    font!("Inter-Regular"),
    font!("Inter-Medium"),
    font!("Inter-SemiBold"),
    font!("Inter-Bold"),
    font!("Poppins-Regular"),
    font!("Poppins-Medium"),
    font!("Poppins-SemiBold"),
    font!("Montserrat-Regular"),
    font!("Montserrat-Bold"),
    font!("Roboto-Regular"),
    font!("Roboto-Bold"),
    font!("Lato-Regular"),
    font!("Lato-Bold"),
    font!("SpaceGrotesk-Regular"),
    font!("SpaceGrotesk-Bold"),
    font!("PlayfairDisplay-Regular"),
    font!("PlayfairDisplay-Bold"),
    font!("BebasNeue-Regular"),
    font!("HindSiliguri-Regular"),
    font!("HindSiliguri-SemiBold"),
    font!("NotoSansBengali-Regular"),
    font!("NotoSansBengali-Medium"),
    font!("NotoSansBengali-SemiBold"),
    font!("NotoSansBengali-Bold"),
    font!("NotoSerifBengali-Regular"),
    font!("NotoSerifBengali-Bold"),
    font!("AnekBangla-Regular"),
    font!("AnekBangla-Bold"),
    font!("BalooDa2-Regular"),
    font!("BalooDa2-Bold"),
    font!("TiroBangla-Regular"),
    font!("Galada-Regular"),
    include_bytes!("../assets/icons/lucide.ttf"),
];

/// Registers the bundled fonts with the shared text engine. Safe to call repeatedly;
/// export calls it so rendering works the same outside the UI.
pub fn ensure_loaded() {
    static LOADED: std::sync::Once = std::sync::Once::new();
    LOADED.call_once(|| {
        let mut fs = iced::advanced::graphics::text::font_system().write().expect("font system");
        for font in BUNDLED {
            fs.load_font(Cow::Borrowed(font));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_font_is_bundled() {
        ensure_loaded();
        let mut guard = iced::advanced::graphics::text::font_system().write().unwrap();
        let fs = guard.raw();
        for (name, script) in TEXT_FONTS {
            let faces: Vec<_> = fs.db().faces().filter(|f| f.families.iter().any(|(n, _)| n == name)).map(|f| (f.id, f.weight)).collect();
            assert!(!faces.is_empty(), "{name} is listed but no bundled face has that family name");
            let letter = if script == Script::Bangla { 'ক' } else { 'A' };
            for (id, weight) in faces {
                let font = fs.get_font(id, weight).expect("bundled face loads");
                assert_ne!(font.as_swash().charmap().map(letter), 0, "{name} {weight:?} has no '{letter}'");
            }
        }
    }

    /// Each font really draws its words: a family the text engine couldn't find would fall back
    /// to another font and draw the same pixels as it.
    #[test]
    fn every_text_font_draws_in_its_own_letters() {
        use crate::clip::TextLayer;
        use crate::render::text::rasterize;
        ensure_loaded();
        let mut drawn: Vec<(&str, Vec<u8>)> = Vec::new();
        for script in [Script::English, Script::Bangla] {
            let words = if script == Script::English { "Quick brown fox" } else { "আমার সোনার বাংলা" };
            for (name, _) in TEXT_FONTS.iter().filter(|f| f.1 == script) {
                let f = rasterize(&TextLayer { text: words.into(), font: (*name).into(), weight: 400, size: 48.0, ..Default::default() }, 1.0, 0.0);
                assert!(f.rgba.chunks_exact(4).filter(|p| p[3] > 128).count() > 500, "{name} draws nothing");
                for (other, pixels) in &drawn {
                    assert_ne!(&f.rgba, pixels, "{name} draws exactly like {other}: it isn't found");
                }
                drawn.push((name, f.rgba.clone()));
            }
        }
        // A family with a bold face draws it for bold text.
        let at = |weight| rasterize(&TextLayer { text: "আমার সোনার বাংলা".into(), font: "Noto Sans Bengali".into(), weight, size: 48.0, ..Default::default() }, 1.0, 0.0);
        let ink = |w| at(w).rgba.chunks_exact(4).filter(|p| p[3] > 128).count();
        assert!(ink(700) > ink(400) + 200, "bold Noto Sans Bengali is heavier than regular");
    }
}
