//! The Settings dialog: preview quality, interface size, editing defaults, export and files.

use std::path::PathBuf;

use iced::widget::{column, container, row, space, text, toggler};
use iced::{Alignment, Element, Fill};

use crate::render::export::Quality;
use crate::settings::{AUTOSAVE_SECONDS, PreviewQuality, Prefs, STILL_SECONDS, UI_SCALES};
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

#[derive(Debug, Clone)]
pub enum Message {
    Close,
    PreviewQuality(PreviewQuality),
    UiScale(f32),
    Snapping(bool),
    Still(f64),
    Autosave(u64),
    StartMuted(bool),
    ExportQuality(Quality),
    BrowseFolder,
    FolderPicked(Option<PathBuf>),
    DefaultFolder,
    ResetAll,
}

/// Applies a change to `prefs`. Returns false for messages that aren't changes.
pub fn apply(prefs: &mut Prefs, m: &Message) -> bool {
    match m {
        Message::PreviewQuality(q) => prefs.preview_quality = *q,
        Message::UiScale(s) => prefs.ui_scale = *s,
        Message::Snapping(on) => prefs.snapping = *on,
        Message::Still(s) => prefs.still_seconds = *s,
        Message::Autosave(s) => prefs.autosave_seconds = *s,
        Message::StartMuted(on) => prefs.start_muted = *on,
        Message::ExportQuality(q) => prefs.export_quality = *q,
        Message::FolderPicked(Some(dir)) => prefs.projects_dir = Some(dir.clone()),
        Message::DefaultFolder => prefs.projects_dir = None,
        Message::ResetAll => *prefs = Prefs::default(),
        _ => return false,
    }
    true
}

/// A titled group of settings.
fn group<'a>(glyph: char, title: &'a str, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let head = row![tinted(glyph, 13, TEXT_3), text(title).font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_2)].spacing(7).align_y(Alignment::Center);
    container(column![head, column(rows).spacing(S3)].spacing(S3)).padding(S4).width(Fill).style(style::card).into()
}

/// A setting: its name and explanation on the left, the control on the right or below.
fn setting<'a>(name: &'a str, hint: impl text::IntoFragment<'a>, control: Element<'a, Message>, below: bool) -> Element<'a, Message> {
    let label = column![text(name).font(INTER_MEDIUM).size(T_BODY), text(hint).size(T_CAPTION).color(TEXT_3)].spacing(2);
    if below {
        column![label, control].spacing(S2).into()
    } else {
        row![container(label).width(Fill), control].spacing(S3).align_y(Alignment::Center).into()
    }
}

fn switch<'a>(on: bool, f: fn(bool) -> Message) -> Element<'a, Message> {
    toggler(on).on_toggle(f).size(18).style(style::toggler).into()
}

pub fn view<'a>(prefs: &'a Prefs, default_dir: PathBuf, appear: f32) -> Element<'a, Message> {
    let preview = group(
        code::MONITOR,
        "Preview",
        vec![setting(
            "Preview quality",
            prefs.preview_quality.hint(),
            widgets::segmented(&PreviewQuality::ALL, prefs.preview_quality, |q| q.label().to_string(), Message::PreviewQuality),
            true,
        )],
    );
    let interface = group(
        code::SLIDERS_HORIZONTAL,
        "Interface",
        vec![setting(
            "Interface size",
            "Makes text, buttons and panels bigger or smaller.",
            widgets::segmented(&UI_SCALES, prefs.ui_scale, |s| format!("{:.0}%", s * 100.0), Message::UiScale),
            true,
        )],
    );
    let editing = group(
        code::SCISSORS,
        "Editing",
        vec![
            setting("Snapping", "Clips, trims and the playhead snap to edges and markers when a project opens (N toggles it).", switch(prefs.snapping, Message::Snapping), false),
            setting(
                "Length of new text, shapes and images",
                "How long they last when you add them.",
                widgets::segmented(&STILL_SECONDS, prefs.still_seconds, |s| format!("{s:.0} s"), Message::Still),
                true,
            ),
            setting(
                "Save automatically",
                "How soon after a change the project saves itself.",
                widgets::segmented(&AUTOSAVE_SECONDS, prefs.autosave_seconds, |s| format!("after {s} s"), Message::Autosave),
                true,
            ),
        ],
    );
    let audio = group(code::VOLUME_2, "Audio", vec![setting("Start with the preview muted", "Exports always include the sound.", switch(prefs.start_muted, Message::StartMuted), false)]);
    let export = group(
        code::DOWNLOAD,
        "Export",
        vec![setting("Default quality", "Used when the export dialog opens; you can still change it there.", widgets::segmented(&Quality::ALL, prefs.export_quality, |q| q.label().to_string(), Message::ExportQuality), true)],
    );
    let folder = prefs.projects_dir.clone().unwrap_or(default_dir);
    let mut folder_actions = row![widgets::secondary_button(Some(code::FOLDER_OPEN), "Change…", Some(Message::BrowseFolder))].spacing(S2);
    if prefs.projects_dir.is_some() {
        folder_actions = folder_actions.push(widgets::ghost_button(None, "Use default", Some(Message::DefaultFolder)));
    }
    let files = group(
        code::FOLDER_OPEN,
        "Files",
        vec![setting(
            "New projects folder",
            "Where new projects are saved.",
            column![container(text(folder.display().to_string()).size(T_SMALL)).padding([7, 10]).width(Fill).style(style::inset), folder_actions].spacing(S2).into(),
            true,
        )],
    );

    let header = row![
        container(icon(code::SLIDERS_HORIZONTAL, 16).color(ON_ACCENT)).center(34).style(style::logo),
        column![text("Settings").font(INTER_SEMIBOLD).size(T_TITLE), text("Changes apply right away and are remembered.").size(T_SMALL).color(TEXT_3)].spacing(2).width(Fill),
        widgets::icon_button(code::X, Some(Message::Close)),
    ]
    .spacing(S3)
    .align_y(Alignment::Center);
    let footer = row![widgets::ghost_button(Some(code::REFRESH_CW), "Reset to defaults", Some(Message::ResetAll)), space().width(Fill), widgets::primary_button(None, "Done", Some(Message::Close))].align_y(Alignment::Center);
    let body = widgets::scroll(column![preview, interface, editing, audio, export, files].spacing(S3).padding(iced::Padding { right: 10.0, ..Default::default() })).height(iced::Length::Shrink);
    let card = container(column![header, container(body).max_height(560), widgets::divider(), footer].spacing(S4)).width(600).padding(S6).style(style::popover);
    widgets::modal(card, Message::Close, appear)
}
