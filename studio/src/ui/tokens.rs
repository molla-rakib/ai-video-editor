//! Design tokens: the only place colors, sizes and fonts are defined.

use iced::font::Weight;
use iced::{Color, Font};

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

// Neutral grays, darkest to lightest. `WINDOW` is the near-black background the
// panels (blocks) float on; `SURFACE_1` is a block.
pub const WINDOW: Color = hex(0x050505);
pub const SURFACE_0: Color = hex(0x0A0A0A);
pub const SURFACE_1: Color = hex(0x121212);
pub const SURFACE_2: Color = hex(0x1A1A1A);
pub const SURFACE_3: Color = hex(0x242424);
pub const SURFACE_4: Color = hex(0x2E2E2E);

/// The border around every block: one step lighter than the block, clearly visible but quiet.
pub const BLOCK_BORDER: Color = hex(0x262626);
/// The border around the preview canvas.
pub const CANVAS_BORDER: Color = hex(0x3A3A3A);

// Hairlines inside blocks are translucent so they read the same on every surface.
pub const LINE: Color = alpha(Color::WHITE, 0.07);
pub const LINE_STRONG: Color = alpha(Color::WHITE, 0.12);
pub const LINE_FOCUS: Color = alpha(ACCENT, 0.85);

/// The small grip centered in each gap between blocks that can be resized.
pub const SPLIT_GRIP: Color = hex(0x3A3A3A);
/// A gap being hovered or dragged to resize: barely there, never a bright line.
pub const SPLIT_HOVER: Color = alpha(Color::WHITE, 0.06);
pub const SPLIT_DRAG: Color = alpha(Color::WHITE, 0.12);

pub const TEXT_1: Color = hex(0xF5F5F5);
pub const TEXT_2: Color = hex(0xA6A6A6);
pub const TEXT_3: Color = hex(0x737373);
pub const TEXT_4: Color = hex(0x4D4D4D);

/// Lime: primary buttons, selection, active tools.
pub const ACCENT: Color = hex(0xD6FF00);
pub const ACCENT_HOVER: Color = hex(0xE3FF4F);
pub const ACCENT_PRESS: Color = hex(0xB9DC00);
pub const ACCENT_SOFT: Color = alpha(ACCENT, 0.13);
pub const ACCENT_GLOW: Color = alpha(ACCENT, 0.28);
/// Text and icons on lime.
pub const ON_ACCENT: Color = hex(0x0A0B07);
/// Second stop of the brand gradient (lime → mint).
pub const BRAND_2: Color = hex(0x2EF2A2);

pub const SUCCESS: Color = hex(0x3DDC97);
pub const WARNING: Color = hex(0xFFB547);
pub const DANGER: Color = hex(0xFF6363);
pub const DANGER_SOFT: Color = alpha(DANGER, 0.14);

pub const SHADOW: Color = alpha(Color::BLACK, 0.55);

/// Clip colors on the timeline, by media kind.
pub mod clip {
    use super::hex;
    use iced::Color;
    pub const VIDEO: Color = hex(0x4C8DFF);
    pub const IMAGE: Color = hex(0xE5A84B);
    pub const TEXT: Color = hex(0xA672FF);
    pub const AUDIO: Color = hex(0x37C98B);
    pub const EFFECT: Color = hex(0xFF8A4C);
    /// Blur bands on clips.
    pub const BLUR: Color = hex(0x7CC4FF);
    /// Mask bands on clips.
    pub const MASK: Color = hex(0xFF6FA8);
}

// Spacing scale (px).
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const S4: f32 = 16.0;
pub const S5: f32 = 20.0;
pub const S6: f32 = 24.0;
pub const S8: f32 = 32.0;

// Corner radii.
pub const R_SM: f32 = 5.0;
pub const R_MD: f32 = 7.0;
pub const R_LG: f32 = 10.0;
pub const R_XL: f32 = 14.0;

// Type scale.
pub const T_CAPTION: f32 = 11.0;
pub const T_SMALL: f32 = 12.0;
pub const T_BODY: f32 = 13.0;
pub const T_LARGE: f32 = 14.0;
pub const T_TITLE: f32 = 17.0;
pub const T_HEADLINE: f32 = 24.0;
pub const T_DISPLAY: f32 = 34.0;

pub const TITLE_BAR_H: f32 = 44.0;
pub const CONTROL_H: f32 = 32.0;

pub const INTER: Font = Font::with_name("Inter");
pub const INTER_MEDIUM: Font = Font { weight: Weight::Medium, ..INTER };
pub const INTER_SEMIBOLD: Font = Font { weight: Weight::Semibold, ..INTER };
pub const INTER_BOLD: Font = Font { weight: Weight::Bold, ..INTER };
pub const HIND: Font = Font::with_name("Hind Siliguri");
pub const HIND_SEMIBOLD: Font = Font { weight: Weight::Semibold, ..HIND };
pub const ICONS: Font = Font::with_name("lucide");
