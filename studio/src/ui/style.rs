//! Widget style functions. Widgets never pick colors themselves; they use these.

use std::f32::consts::PI;

use iced::border::Radius;
use iced::gradient::Linear;
use iced::overlay::menu;
use iced::widget::{self as w, button, container, pick_list, scrollable, text_input};
use iced::{Background, Border, Color, Gradient, Shadow, Theme, Vector};

use super::tokens::*;

fn border(color: Color, width: f32, radius: f32) -> Border {
    Border { color, width, radius: radius.into() }
}

fn soft_shadow(blur: f32, y: f32, a: f32) -> Shadow {
    Shadow { color: Color { a, ..Color::BLACK }, offset: Vector::new(0.0, y), blur_radius: blur }
}

/// Vertical gradient, `top` to `bottom`.
pub fn vertical(top: Color, bottom: Color) -> Background {
    Gradient::Linear(Linear::new(PI).add_stop(0.0, top).add_stop(1.0, bottom)).into()
}

pub fn brand_gradient() -> Background {
    Gradient::Linear(Linear::new(PI * 0.75).add_stop(0.0, ACCENT).add_stop(1.0, BRAND_2)).into()
}

// ---------- containers ----------

/// The whole window. On Linux it is inset with a drawn shadow and rounded corners.
pub fn window_frame(rounded: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(WINDOW.into()),
        text_color: Some(TEXT_1),
        border: if rounded { border(LINE_STRONG, 1.0, R_XL) } else { Border::default() },
        shadow: if rounded { soft_shadow(22.0, 8.0, 0.6) } else { Shadow::default() },
        ..Default::default()
    }
}

pub fn title_bar(rounded: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(WINDOW.into()),
        border: Border {
            radius: if rounded { Radius { top_left: R_XL, top_right: R_XL, bottom_right: 0.0, bottom_left: 0.0 } } else { 0.0.into() },
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn status_bar(rounded: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(SURFACE_0.into()),
        text_color: Some(TEXT_3),
        border: Border {
            radius: if rounded { Radius { top_left: 0.0, top_right: 0.0, bottom_right: R_XL, bottom_left: R_XL } } else { 0.0.into() },
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn hairline(_: &Theme) -> container::Style {
    container::Style { background: Some(LINE.into()), ..Default::default() }
}

/// A block: matte dark gray with a faint top sheen, a quiet lighter border and soft corners.
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(vertical(hex_mix(SURFACE_1, Color::WHITE, 0.018), SURFACE_1)),
        border: border(BLOCK_BORDER, 1.0, R_LG),
        ..Default::default()
    }
}

/// The preview block: darker inside, so the canvas stands out.
pub fn workspace(_: &Theme) -> container::Style {
    container::Style { background: Some(SURFACE_0.into()), border: border(BLOCK_BORDER, 1.0, R_LG), ..Default::default() }
}

/// The plain background the blocks sit on.
pub fn backdrop(_: &Theme) -> container::Style {
    container::Style { background: Some(WINDOW.into()), ..Default::default() }
}

pub fn panel_header(_: &Theme) -> container::Style {
    container::Style {
        background: None,
        text_color: Some(TEXT_2),
        ..Default::default()
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style { background: Some(SURFACE_2.into()), border: border(LINE, 1.0, R_LG), ..Default::default() }
}

pub fn inset(_: &Theme) -> container::Style {
    container::Style { background: Some(SURFACE_0.into()), border: border(LINE, 1.0, R_MD), ..Default::default() }
}

/// Menus, popovers, dropdowns.
pub fn popover(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SURFACE_2.into()),
        text_color: Some(TEXT_1),
        border: border(LINE_STRONG, 1.0, R_LG),
        shadow: soft_shadow(28.0, 12.0, 0.55),
        ..Default::default()
    }
}

pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SURFACE_4.into()),
        text_color: Some(TEXT_1),
        border: border(LINE_STRONG, 1.0, R_SM),
        shadow: soft_shadow(12.0, 4.0, 0.45),
        ..Default::default()
    }
}

pub fn kbd(_: &Theme) -> container::Style {
    container::Style { background: Some(Color { a: 0.06, ..Color::WHITE }.into()), text_color: Some(TEXT_3), border: border(LINE, 1.0, 4.0), ..Default::default() }
}

pub fn badge(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Color { a: 0.14, ..color }.into()),
        text_color: Some(color),
        border: border(Color { a: 0.22, ..color }, 1.0, 6.0),
        ..Default::default()
    }
}

pub fn swatch(color: Color, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style { background: Some(color.into()), border: border(Color::TRANSPARENT, 0.0, radius), ..Default::default() }
}

pub fn logo(_: &Theme) -> container::Style {
    container::Style {
        background: Some(brand_gradient()),
        text_color: Some(ON_ACCENT),
        border: border(Color { a: 0.3, ..Color::WHITE }, 1.0, 7.0),
        shadow: Shadow { color: ACCENT_GLOW, offset: Vector::new(0.0, 2.0), blur_radius: 10.0 },
        ..Default::default()
    }
}

pub fn scrim(_: &Theme) -> container::Style {
    container::Style { background: Some(Color { a: 0.45, ..Color::BLACK }.into()), ..Default::default() }
}

// ---------- buttons ----------

fn button_base(bg: Option<Background>, text: Color, border: Border, shadow: Shadow) -> button::Style {
    button::Style { background: bg, text_color: text, border, shadow, snap: true }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let (top, bottom) = match status {
        button::Status::Hovered => (ACCENT_HOVER, ACCENT),
        button::Status::Pressed => (ACCENT_PRESS, ACCENT_PRESS),
        button::Status::Disabled => (SURFACE_3, SURFACE_3),
        button::Status::Active => (hex_mix(ACCENT, Color::WHITE, 0.10), ACCENT),
    };
    let disabled = matches!(status, button::Status::Disabled);
    button_base(
        Some(vertical(top, bottom)),
        if disabled { TEXT_4 } else { ON_ACCENT },
        border(Color { a: if disabled { 0.0 } else { 0.28 }, ..Color::WHITE }, 1.0, R_MD),
        if disabled || matches!(status, button::Status::Pressed) { Shadow::default() } else { Shadow { color: ACCENT_GLOW, offset: Vector::new(0.0, 3.0), blur_radius: 12.0 } },
    )
}

/// The main action when it stops or ends something (Stop recording): the primary button in red.
pub fn stop(_: &Theme, status: button::Status) -> button::Style {
    let (top, bottom) = match status {
        button::Status::Hovered => (hex_mix(DANGER, Color::WHITE, 0.18), hex_mix(DANGER, Color::WHITE, 0.06)),
        button::Status::Pressed => (hex_mix(DANGER, Color::BLACK, 0.14), hex_mix(DANGER, Color::BLACK, 0.14)),
        button::Status::Disabled => (SURFACE_3, SURFACE_3),
        button::Status::Active => (hex_mix(DANGER, Color::WHITE, 0.10), DANGER),
    };
    let disabled = matches!(status, button::Status::Disabled);
    button_base(
        Some(vertical(top, bottom)),
        if disabled { TEXT_4 } else { ON_ACCENT },
        border(Color { a: if disabled { 0.0 } else { 0.28 }, ..Color::WHITE }, 1.0, R_MD),
        if disabled || matches!(status, button::Status::Pressed) { Shadow::default() } else { Shadow { color: Color { a: 0.28, ..DANGER }, offset: Vector::new(0.0, 3.0), blur_radius: 12.0 } },
    )
}

pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => SURFACE_4,
        button::Status::Pressed => SURFACE_2,
        _ => SURFACE_3,
    };
    let text = if matches!(status, button::Status::Disabled) { TEXT_4 } else { TEXT_1 };
    button_base(Some(bg.into()), text, border(LINE_STRONG, 1.0, R_MD), soft_shadow(4.0, 1.0, 0.25))
}

pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    let (bg, text) = match status {
        button::Status::Hovered => (Some(SURFACE_3.into()), TEXT_1),
        button::Status::Pressed => (Some(SURFACE_4.into()), TEXT_1),
        button::Status::Disabled => (None, TEXT_4),
        button::Status::Active => (None, TEXT_2),
    };
    button_base(bg, text, border(Color::TRANSPARENT, 0.0, R_MD), Shadow::default())
}

/// A ghost button that stays highlighted while its feature is on.
pub fn toggle(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        if !active {
            return ghost(theme, status);
        }
        let bg = if matches!(status, button::Status::Hovered | button::Status::Pressed) { Color { a: 0.24, ..ACCENT } } else { ACCENT_SOFT };
        button_base(Some(bg.into()), ACCENT_HOVER, border(Color { a: 0.3, ..ACCENT }, 1.0, R_MD), Shadow::default())
    }
}

pub fn danger(_: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => Color { a: 0.24, ..DANGER },
        _ => DANGER_SOFT,
    };
    button_base(Some(bg.into()), DANGER, border(Color { a: 0.3, ..DANGER }, 1.0, R_MD), Shadow::default())
}

pub fn menu_item(danger: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        let bg = hovered.then(|| Background::Color(if danger { DANGER_SOFT } else { ACCENT }));
        let text = match () {
            _ if disabled => TEXT_4,
            _ if danger => DANGER,
            _ if hovered => ON_ACCENT,
            _ => TEXT_1,
        };
        button_base(bg, text, border(Color::TRANSPARENT, 0.0, R_SM), Shadow::default())
    }
}

/// Selectable card (e.g. aspect-ratio presets).
pub fn choice_card(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, line) = match (selected, hovered) {
            (true, _) => (Color { a: 0.10, ..ACCENT }, ACCENT),
            (false, true) => (SURFACE_3, LINE_STRONG),
            (false, false) => (SURFACE_2, LINE),
        };
        let shadow = if selected { Shadow { color: Color { a: 0.25, ..ACCENT }, offset: Vector::ZERO, blur_radius: 14.0 } } else { Shadow::default() };
        button_base(Some(bg.into()), TEXT_1, border(line, if selected { 1.5 } else { 1.0 }, R_LG), shadow)
    }
}

/// A row in a list (recent projects, media).
pub fn list_row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(ACCENT_SOFT.into()),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(SURFACE_3.into()),
            _ => None,
        };
        button_base(bg, TEXT_1, border(if selected { Color { a: 0.3, ..ACCENT } } else { Color::TRANSPARENT }, 1.0, R_MD), Shadow::default())
    }
}

pub fn segment(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, text) = match (active, hovered) {
            (true, _) => (Some(SURFACE_4.into()), TEXT_1),
            (false, true) => (None, TEXT_1),
            (false, false) => (None, TEXT_3),
        };
        let shadow = if active { soft_shadow(4.0, 1.0, 0.35) } else { Shadow::default() };
        button_base(bg, text, border(if active { LINE_STRONG } else { Color::TRANSPARENT }, 1.0, R_SM), shadow)
    }
}

pub fn segmented_track(_: &Theme) -> container::Style {
    container::Style { background: Some(SURFACE_0.into()), border: border(LINE, 1.0, R_MD), ..Default::default() }
}

/// Window controls; close turns red on hover, like native Windows.
pub fn window_control(close: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            button::Status::Hovered if close => Some(hex_mix(DANGER, Color::BLACK, 0.12)),
            button::Status::Pressed if close => Some(hex_mix(DANGER, Color::BLACK, 0.3)),
            button::Status::Hovered => Some(SURFACE_3),
            button::Status::Pressed => Some(SURFACE_4),
            _ => None,
        };
        let text = if close && matches!(status, button::Status::Hovered | button::Status::Pressed) { Color::WHITE } else { TEXT_2 };
        button_base(bg.map(Background::Color), text, Border::default(), Shadow::default())
    }
}

// ---------- inputs ----------

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let (line, bg) = match status {
        text_input::Status::Focused { .. } => (LINE_FOCUS, SURFACE_0),
        text_input::Status::Hovered => (LINE_STRONG, SURFACE_0),
        text_input::Status::Disabled => (LINE, SURFACE_1),
        text_input::Status::Active => (LINE, SURFACE_0),
    };
    text_input::Style {
        background: bg.into(),
        border: border(line, 1.0, R_MD),
        icon: TEXT_3,
        placeholder: TEXT_4,
        value: TEXT_1,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

pub fn text_area(_: &Theme, status: w::text_editor::Status) -> w::text_editor::Style {
    let (line, bg) = match status {
        w::text_editor::Status::Focused { .. } => (LINE_FOCUS, SURFACE_0),
        w::text_editor::Status::Hovered => (LINE_STRONG, SURFACE_0),
        w::text_editor::Status::Disabled => (LINE, SURFACE_1),
        w::text_editor::Status::Active => (LINE, SURFACE_0),
    };
    w::text_editor::Style { background: bg.into(), border: border(line, 1.0, R_MD), placeholder: TEXT_4, value: TEXT_1, selection: Color { a: 0.35, ..ACCENT } }
}

pub fn select(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let line = match status {
        pick_list::Status::Opened { .. } => LINE_FOCUS,
        pick_list::Status::Hovered => LINE_STRONG,
        _ => LINE,
    };
    pick_list::Style { text_color: TEXT_1, placeholder_color: TEXT_4, handle_color: TEXT_3, background: SURFACE_0.into(), border: border(line, 1.0, R_MD) }
}

pub fn select_menu(_: &Theme) -> menu::Style {
    menu::Style {
        background: SURFACE_2.into(),
        border: border(LINE_STRONG, 1.0, R_MD),
        text_color: TEXT_1,
        selected_text_color: ON_ACCENT,
        selected_background: ACCENT.into(),
        shadow: soft_shadow(24.0, 10.0, 0.5),
    }
}

pub fn slider(_: &Theme, status: w::slider::Status) -> w::slider::Style {
    let ring = match status {
        w::slider::Status::Dragged => ACCENT_HOVER,
        w::slider::Status::Hovered => ACCENT,
        w::slider::Status::Active => Color { a: 0.0, ..ACCENT },
    };
    w::slider::Style {
        rail: w::slider::Rail { backgrounds: (ACCENT.into(), SURFACE_4.into()), width: 3.0, border: border(Color::TRANSPARENT, 0.0, 2.0) },
        handle: w::slider::Handle { shape: w::slider::HandleShape::Circle { radius: 6.5 }, background: Color::WHITE.into(), border_width: 2.0, border_color: ring },
    }
}

/// A slider whose zero is in the middle (−100…+100): the colored part runs from the middle
/// to the handle instead of from the left end. `at` is where the handle sits, 0 (left) to 1.
pub fn slider_centered(at: f32) -> impl Fn(&Theme, w::slider::Status) -> w::slider::Style {
    move |theme, status| {
        // The rail is drawn as two pieces, left and right of the handle; the middle of the
        // whole rail falls inside one of them, where the color changes.
        let split = |from: Color, to: Color, stop: f32| -> Background {
            let stop = stop.clamp(0.0, 0.999);
            Gradient::Linear(Linear::new(PI / 2.0).add_stop(0.0, from).add_stop(stop, from).add_stop(stop + 0.001, to).add_stop(1.0, to)).into()
        };
        let at = at.clamp(0.0, 1.0);
        let backgrounds = if at > 0.505 {
            (split(SURFACE_4, ACCENT, 0.5 / at), SURFACE_4.into())
        } else if at < 0.495 {
            (SURFACE_4.into(), split(ACCENT, SURFACE_4, (0.5 - at) / (1.0 - at)))
        } else {
            (SURFACE_4.into(), SURFACE_4.into())
        };
        let mut s = slider(theme, status);
        s.rail.backgrounds = backgrounds;
        s
    }
}

pub fn toggler(_: &Theme, status: w::toggler::Status) -> w::toggler::Style {
    let (on, hovered) = match status {
        w::toggler::Status::Active { is_toggled } => (is_toggled, false),
        w::toggler::Status::Hovered { is_toggled } => (is_toggled, true),
        w::toggler::Status::Disabled { is_toggled } => (is_toggled, false),
    };
    w::toggler::Style {
        background: if on { ACCENT.into() } else if hovered { SURFACE_4.into() } else { SURFACE_3.into() },
        background_border_width: 1.0,
        background_border_color: if on { Color { a: 0.2, ..Color::WHITE } } else { LINE_STRONG },
        foreground: if on { ON_ACCENT.into() } else { TEXT_1.into() },
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(TEXT_2),
        border_radius: None,
        padding_ratio: 0.18,
    }
}

pub fn checkbox(_: &Theme, status: w::checkbox::Status) -> w::checkbox::Style {
    let (on, hovered) = match status {
        w::checkbox::Status::Active { is_checked } => (is_checked, false),
        w::checkbox::Status::Hovered { is_checked } => (is_checked, true),
        w::checkbox::Status::Disabled { is_checked } => (is_checked, false),
    };
    w::checkbox::Style {
        background: if on { ACCENT.into() } else { SURFACE_0.into() },
        icon_color: ON_ACCENT,
        border: border(if on { ACCENT } else if hovered { LINE_STRONG } else { LINE_STRONG }, 1.0, 4.0),
        text_color: Some(TEXT_2),
    }
}

pub fn scrollbar(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let (hovered, dragged) = match status {
        scrollable::Status::Hovered { is_vertical_scrollbar_hovered, is_horizontal_scrollbar_hovered, .. } => {
            (is_vertical_scrollbar_hovered || is_horizontal_scrollbar_hovered, false)
        }
        scrollable::Status::Dragged { is_vertical_scrollbar_dragged, is_horizontal_scrollbar_dragged, .. } => {
            (false, is_vertical_scrollbar_dragged || is_horizontal_scrollbar_dragged)
        }
        scrollable::Status::Active { .. } => (false, false),
    };
    let thumb = match (hovered, dragged) {
        (_, true) => Color { a: 0.32, ..Color::WHITE },
        (true, _) => Color { a: 0.22, ..Color::WHITE },
        _ => Color { a: 0.10, ..Color::WHITE },
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller { background: thumb.into(), border: border(Color::TRANSPARENT, 0.0, 4.0) },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: SURFACE_3.into(),
            border: border(LINE_STRONG, 1.0, 12.0),
            shadow: soft_shadow(8.0, 2.0, 0.4),
            icon: TEXT_2,
        },
    }
}

fn hex_mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a }
}
