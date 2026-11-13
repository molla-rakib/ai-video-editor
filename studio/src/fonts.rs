//! Fonts bundled with the app, for the UI and for text in videos.

use std::borrow::Cow;

pub const BUNDLED: [&[u8]; 10] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/HindSiliguri-Regular.ttf"),
    include_bytes!("../assets/fonts/HindSiliguri-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Poppins-Regular.ttf"),
    include_bytes!("../assets/fonts/Poppins-Medium.ttf"),
    include_bytes!("../assets/fonts/Poppins-SemiBold.ttf"),
    include_bytes!("../assets/icons/lucide.ttf"),
];

/// Registers the bundled fonts with the shared text engine. Safe to call repeatedly;
/// export calls it so rendering works the same outside the UI.
pub fn ensure_loaded() {
    let mut fs = iced::advanced::graphics::text::font_system().write().expect("font system");
    for font in BUNDLED {
        fs.load_font(Cow::Borrowed(font));
    }
}
