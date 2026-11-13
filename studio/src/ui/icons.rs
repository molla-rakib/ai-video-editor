//! Lucide icons drawn from the bundled icon font.

#[path = "icon_codes.rs"]
pub mod code;

use iced::widget::{Text, text};
use iced::{Color, Pixels, alignment};

use super::tokens;

pub fn icon<'a>(code: char, size: impl Into<Pixels>) -> Text<'a> {
    let size = size.into();
    text(code.to_string())
        .font(tokens::ICONS)
        .size(size)
        .line_height(1.0)
        .width(size.0)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
}

pub fn tinted<'a>(code: char, size: impl Into<Pixels>, color: Color) -> Text<'a> {
    icon(code, size).color(color)
}
