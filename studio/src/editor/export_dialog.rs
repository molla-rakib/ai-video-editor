//! The export dialog: pick a file and quality, watch progress, open the result.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use iced::widget::{column, container, mouse_area, opaque, row, space, text, text_input};
use iced::{Alignment, Element, Fill, Length};

use super::Message;
use crate::model::Project;
use crate::render::export::{self, ExportSettings, Preset, Quality};
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

pub enum Phase {
    Setup,
    Running { done: u64, total: u64, eta: Option<f64>, job: usize, job_total: usize, cancel: Arc<AtomicBool> },
    Done { items: Vec<(PathBuf, Preset)>, seconds: f64 },
    Failed(String),
}

/// A delivery preset queued to run after the current setup's job finishes.
#[derive(Clone)]
pub struct QueuedJob {
    pub preset: Preset,
    pub path: PathBuf,
}

pub struct ExportDialog {
    pub path: String,
    pub preset: Preset,
    pub quality: Quality,
    pub phase: Phase,
    /// Extra jobs to run, in order, after the current setup's export finishes.
    pub queue: Vec<QueuedJob>,
    /// Finished jobs from this batch, shown together once the whole queue is done.
    pub completed: Vec<(PathBuf, Preset)>,
    pub total_seconds: f64,
    /// The range marked on the timeline when the dialog opened, and whether only it is exported.
    pub range: Option<(f64, f64)>,
    pub only_range: bool,
}

impl ExportDialog {
    pub fn new(project: &Project, project_path: Option<&std::path::Path>, range: Option<(f64, f64)>) -> ExportDialog {
        let dir = project_path.and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_else(crate::settings::projects_dir);
        let safe: String = project.name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
        let mut path = dir.join(format!("{safe}.mp4"));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("{safe} ({n}).mp4"));
            n += 1;
        }
        ExportDialog { path: path.display().to_string(), preset: Preset::Custom, quality: Quality::High, phase: Phase::Setup, queue: Vec::new(), completed: Vec::new(), total_seconds: 0.0, range: range.filter(|(a, b)| b - a >= 0.05), only_range: false }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    pub fn settings(&self) -> ExportSettings {
        ExportSettings { range: self.range.filter(|_| self.only_range), ..ExportSettings::from_preset(self.preset, self.quality) }
    }

    /// A filename suggestion for `preset`, derived from the current path with the
    /// preset's name appended (so batched jobs don't collide on disk).
    pub fn suggest_path(&self, preset: Preset) -> PathBuf {
        let base = PathBuf::from(self.path.trim());
        let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into());
        base.with_file_name(format!("{stem} ({}).mp4", preset.label()))
    }

    pub fn view<'a>(&'a self, project: &'a Project) -> Element<'a, Message> {
        let body: Element<'a, Message> = match &self.phase {
            Phase::Setup => self.setup(project),
            Phase::Running { done, total, eta, job, job_total, .. } => running(*done, *total, *eta, *job, *job_total),
            Phase::Done { items, seconds } => done(items, *seconds),
            Phase::Failed(err) => failed(err),
        };
        let card = container(body).width(480).padding(S6).style(style::popover);
        let scrim = mouse_area(container(space().width(Fill).height(Fill)).style(style::scrim)).on_press(if self.is_running() { Message::Noop } else { Message::CloseExport });
        iced::widget::stack![scrim, container(opaque(card)).center(Fill)].into()
    }

    fn setup<'a>(&'a self, project: &'a Project) -> Element<'a, Message> {
        let (w, h) = project.canvas.size();
        let settings = self.settings();
        let duration = match settings.range {
            Some((a, b)) => (b.min(project.duration()) - a).max(0.0),
            None => project.duration(),
        };
        let effective = export::scaled_size((w, h), settings.max_width);
        let mb = export::estimate_mb(effective, project.canvas.fps, settings.quality, duration);
        let chip = |glyph: char, label: String| container(row![tinted(glyph, 12, TEXT_3), text(label).size(T_SMALL).color(TEXT_2)].spacing(6).align_y(Alignment::Center)).padding([5, 9]).style(style::segmented_track);
        let valid = !self.path.trim().is_empty() && PathBuf::from(self.path.trim()).parent().is_some_and(|d| d.as_os_str().is_empty() || d.is_dir());
        let mut body = column![
            row![
                container(icon(code::DOWNLOAD, 16).color(ON_ACCENT)).center(34).style(style::logo),
                column![text("Export video").font(INTER_SEMIBOLD).size(T_TITLE), text("MP4 · H.264 video with AAC audio").size(T_SMALL).color(TEXT_3)].spacing(2),
            ]
            .spacing(S3)
            .align_y(Alignment::Center),
            row![
                chip(code::MONITOR, if effective == (w, h) { format!("{w} × {h}") } else { format!("{w} × {h} → {}×{}", effective.0, effective.1) }),
                chip(code::GAUGE, format!("{} fps", project.canvas.fps)),
                chip(code::CLOCK, super::fmt_timecode(duration)),
                chip(code::HARD_DRIVE, if mb < 1000.0 { format!("~{mb:.0} MB") } else { format!("~{:.1} GB", mb / 1024.0) }),
            ]
            .spacing(6),
            widgets::field(
                "Save to",
                row![
                    text_input("File path", &self.path).on_input(Message::ExportPath).padding([8, 10]).size(T_BODY).style(style::input),
                    widgets::secondary_button(Some(code::FOLDER_OPEN), "Browse", Some(Message::ExportBrowse)),
                ]
                .spacing(S2)
                .align_y(Alignment::Center)
            ),
            widgets::field("Delivery preset", widgets::segmented(&Preset::ALL, self.preset, |p| p.label().to_string(), Message::ExportPreset)),
            text(self.preset.description()).size(T_CAPTION).color(TEXT_3),
        ]
        .spacing(S5);
        // With a range marked on the timeline, just that part can be exported.
        if let Some((a, b)) = self.range {
            let label = move |only: &bool| if *only { format!("Marked range ({} – {})", super::fmt_timecode_short(a), super::fmt_timecode_short(b)) } else { "Whole video".to_string() };
            body = body.push(widgets::field("What to export", widgets::segmented(&[false, true], self.only_range, label, Message::ExportOnlyRange)));
        }
        if self.preset == Preset::Custom {
            body = body.push(widgets::field("Quality", widgets::segmented(&Quality::ALL, self.quality, |q| q.label().to_string(), Message::ExportQuality)));
        }
        if !self.queue.is_empty() {
            let rows = self.queue.iter().enumerate().map(|(i, j)| {
                row![
                    tinted(code::LAYERS, 13, TEXT_3),
                    text(format!("{}  ·  {}", j.preset.label(), PathBuf::from(&j.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).size(T_SMALL).color(TEXT_2),
                    space().width(Fill),
                    widgets::icon_button(code::X, Some(Message::ExportQueueRemove(i))).width(22).height(22),
                ]
                .spacing(S2)
                .align_y(Alignment::Center)
                .into()
            });
            body = body.push(widgets::field("Queued for this export", column(rows).spacing(4)));
        }
        let available: Vec<Preset> = Preset::ALL.into_iter().filter(|p| *p != Preset::Custom && *p != self.preset && !self.queue.iter().any(|q| q.preset == *p)).collect();
        if !available.is_empty() {
            let adds = row(available.iter().map(|p| widgets::ghost_button(Some(code::PLUS), format!("Also as {}", p.label()), Some(Message::ExportQueueAddPreset(*p))).padding([6, 10]).into())).spacing(S2);
            body = body.push(widgets::field("Batch export", adds));
        }
        let export_label = if self.queue.is_empty() { "Export".to_string() } else { format!("Export all ({})", self.queue.len() + 1) };
        body = body.push(
            row![
                space().width(Fill),
                widgets::ghost_button(None, "Cancel", Some(Message::CloseExport)),
                widgets::primary_button(Some(code::SPARKLES), export_label, (valid && duration > 0.0).then_some(Message::StartExport)),
            ]
            .spacing(S2)
            .align_y(Alignment::Center),
        );
        body.into()
    }
}

fn progress_bar<'a>(fraction: f32) -> Element<'a, Message> {
    let fill = container(space().height(6)).width(Length::FillPortion(((fraction.clamp(0.0, 1.0)) * 1000.0) as u16 + 1)).style(|_| container::Style {
        background: Some(style::brand_gradient()),
        border: iced::Border { radius: 3.0.into(), ..Default::default() },
        ..Default::default()
    });
    let rest = space().width(Length::FillPortion((((1.0 - fraction.clamp(0.0, 1.0)) * 1000.0) as u16).max(1)));
    container(row![fill, rest]).width(Fill).style(|_| container::Style {
        background: Some(SURFACE_0.into()),
        border: iced::Border { radius: 3.0.into(), color: LINE, width: 1.0 },
        ..Default::default()
    })
    .into()
}

fn running<'a>(done: u64, total: u64, eta: Option<f64>, job: usize, job_total: usize) -> Element<'a, Message> {
    let f = done as f32 / total.max(1) as f32;
    let eta = match eta {
        Some(s) if s >= 60.0 => format!("about {} min {} s left", (s / 60.0) as u64, s as u64 % 60),
        Some(s) => format!("about {} s left", s.ceil() as u64),
        None => "estimating time…".into(),
    };
    let title = if job_total > 1 { format!("Rendering video {job} of {job_total}") } else { "Rendering your video".to_string() };
    column![
        row![iced_aw::Spinner::new().width(Length::Fixed(18.0)).height(Length::Fixed(18.0)).circle_radius(2.0), text(title).font(INTER_SEMIBOLD).size(T_TITLE)].spacing(S3).align_y(Alignment::Center),
        progress_bar(f),
        row![text(format!("{:.0}%", f * 100.0)).font(INTER_SEMIBOLD).size(T_BODY), text(format!("·  frame {done} of {total}  ·  {eta}")).size(T_SMALL).color(TEXT_3)].spacing(6).align_y(Alignment::Center),
        row![space().width(Fill), widgets::secondary_button(Some(code::X), "Cancel export", Some(Message::CancelExport))],
    ]
    .spacing(S4)
    .into()
}

fn done<'a>(items: &'a [(PathBuf, Preset)], seconds: f64) -> Element<'a, Message> {
    let title = if items.len() > 1 { format!("{} videos are ready", items.len()) } else { "Your video is ready".to_string() };
    let rows = items.iter().enumerate().map(|(i, (path, preset))| {
        let label: Element<'a, Message> = if items.len() > 1 { text(preset.label()).size(T_CAPTION).color(TEXT_3).into() } else { space().height(0).into() };
        column![
            label,
            container(text(path.display().to_string()).size(T_SMALL).color(TEXT_2)).padding([8, 10]).width(Fill).style(style::inset),
            row![
                widgets::secondary_button(Some(code::FOLDER_OPEN), "Show in folder", Some(Message::RevealExport(i))),
                widgets::secondary_button(Some(code::PLAY), "Play", Some(Message::PlayExport(i))),
            ]
            .spacing(S2),
        ]
        .spacing(4)
        .into()
    });
    column![
        row![
            container(icon(code::CIRCLE_CHECK, 18).color(SUCCESS)).center(34).style(style::badge(SUCCESS)),
            column![
                text(title).font(INTER_SEMIBOLD).size(T_TITLE),
                text(format!("Rendered in {}", if seconds >= 60.0 { format!("{} min {} s", (seconds / 60.0) as u64, seconds as u64 % 60) } else { format!("{seconds:.0} s") })).size(T_SMALL).color(TEXT_3),
            ]
            .spacing(2),
        ]
        .spacing(S3)
        .align_y(Alignment::Center),
        column(rows).spacing(S3),
        row![space().width(Fill), widgets::primary_button(None, "Done", Some(Message::CloseExport))].spacing(S2),
    ]
    .spacing(S4)
    .into()
}

fn failed<'a>(err: &'a str) -> Element<'a, Message> {
    column![
        row![
            container(icon(code::TRIANGLE_ALERT, 18).color(DANGER)).center(34).style(style::badge(DANGER)),
            text("Export didn't finish").font(INTER_SEMIBOLD).size(T_TITLE),
        ]
        .spacing(S3)
        .align_y(Alignment::Center),
        container(text(err).size(T_SMALL).color(TEXT_2)).padding([8, 10]).width(Fill).style(style::inset),
        row![space().width(Fill), widgets::secondary_button(None, "Back", Some(Message::ExportRetry)), widgets::primary_button(None, "Close", Some(Message::CloseExport))].spacing(S2),
    ]
    .spacing(S4)
    .into()
}

