// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai_connect;
mod app;
mod assistant;
mod clip;
mod doc;
mod edit;
mod editor;
mod ffmpeg;
mod fonts;
mod icon;
mod mcp;
mod media;
mod model;
mod platform;
mod playback;
mod render;
mod reverse;
mod sample;
mod settings;
mod settings_dialog;
mod sfx;
mod shortcuts;
mod shortcuts_dialog;
mod start;
mod ui;

use iced::{Size, window};

use crate::ui::tokens;

/// winit can't receive files dragged in from the file manager on Wayland, so on
/// Linux the app runs through XWayland, where drag and drop works. Set
/// `STUDIO_WAYLAND=1` to stay on native Wayland anyway.
#[cfg(target_os = "linux")]
fn prefer_xwayland() {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let xwayland = std::env::var_os("DISPLAY").is_some();
    if wayland && xwayland && std::env::var_os("STUDIO_WAYLAND").is_none() {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
}

/// `ai-video-studio --write-icons DIR` writes the app icon as PNGs (every size) and a
/// Windows `.ico` into DIR, for packaging, then exits.
fn write_icons(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for size in icon::SIZES {
        let path = dir.join(format!("ai-video-studio-{size}.png"));
        image::save_buffer(&path, &icon::rgba(size), size, size, image::ExtendedColorType::Rgba8).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    std::fs::write(dir.join("ai-video-studio.ico"), icon::ico()).map_err(|e| e.to_string())
}

fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--write-icons") {
        let dir = std::path::PathBuf::from(args.get(2).map(String::as_str).unwrap_or("icons"));
        if let Err(e) = write_icons(&dir) {
            eprintln!("Couldn't write the icons: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    prefer_xwayland();
    let mut application = iced::application(app::App::new, app::App::update, app::App::view)
        .title(app::App::title)
        .subscription(app::App::subscription)
        .scale_factor(app::App::scale_factor)
        .theme(theme())
        .default_font(tokens::INTER)
        .antialiasing(true)
        .window(window_settings());
    for font in fonts::BUNDLED {
        application = application.font(font);
    }
    application.run()
}

fn theme() -> iced::Theme {
    iced::Theme::custom(
        "Studio",
        iced::theme::Palette {
            background: tokens::WINDOW,
            text: tokens::TEXT_1,
            primary: tokens::ACCENT,
            success: tokens::SUCCESS,
            warning: tokens::WARNING,
            danger: tokens::DANGER,
        },
    )
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(1480.0, 920.0),
        min_size: Some(Size::new(1100.0, 680.0)),
        decorations: false,
        transparent: false,
        icon: window::icon::from_rgba(icon::rgba(128), 128, 128).ok(),
        platform_specific: platform_specific(),
        ..Default::default()
    }
}

#[cfg(windows)]
fn platform_specific() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific {
        undecorated_shadow: true,
        corner_preference: window::settings::platform::CornerPreference::Round,
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
fn platform_specific() -> window::settings::PlatformSpecific {
    window::settings::PlatformSpecific { application_id: "ai-video-editor".into(), ..Default::default() }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_specific() -> window::settings::PlatformSpecific {
    Default::default()
}
