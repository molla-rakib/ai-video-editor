//! Start screen: create a project or reopen a recent one.

use std::path::PathBuf;

use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Alignment, Element, Fill, Length};

use crate::model::{Aspect, Canvas, FRAME_RATES, Resolution};
use crate::settings::{RecentProject, relative_time};
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    AspectPicked(Aspect),
    ResolutionPicked(Resolution),
    FpsPicked(u32),
    Create,
    Open,
    OpenRecent(PathBuf),
    ForgetRecent(PathBuf),
    TryExample,
}

pub struct Start {
    pub name: String,
    pub canvas: Canvas,
}

impl Default for Start {
    fn default() -> Self {
        Start { name: "Untitled project".into(), canvas: Canvas::default() }
    }
}

impl Start {
    /// Handles the form messages; the app handles Create/Open itself.
    pub fn update(&mut self, message: &Message) {
        match message {
            Message::NameChanged(s) => self.name = s.clone(),
            Message::AspectPicked(a) => self.canvas.aspect = *a,
            Message::ResolutionPicked(r) => self.canvas.resolution = *r,
            Message::FpsPicked(f) => self.canvas.fps = *f,
            _ => {}
        }
    }

    pub fn view<'a>(&'a self, recent: &'a [RecentProject]) -> Element<'a, Message> {
        // A faint brand glow behind the content.
        let glow = container(space().width(Fill).height(Fill)).style(|_| iced::widget::container::Style {
            background: Some(style::vertical(iced::Color { a: 0.07, ..ACCENT }, iced::Color::TRANSPARENT)),
            ..Default::default()
        });
        let page = iced::widget::responsive(move |size| {
            let header = column![
                text("Create something remarkable").font(INTER_BOLD).size(T_DISPLAY),
                text("Motion graphics and product videos — edit by hand, or ask the assistant.").size(T_LARGE).color(TEXT_2),
            ]
            .spacing(S2);
            let content = column![header, row![self.new_project(), recent_list(recent)].spacing(S5).height(Length::Shrink)].spacing(S8).max_width(1080);
            let content = container(content).center_x(Fill).padding([40, 40]);
            widgets::scroll(container(content).center_y(Length::Fixed(size.height.max(720.0)))).height(Fill).into()
        });
        iced::widget::stack![glow, page].into()
    }

    fn new_project(&self) -> Element<'_, Message> {
        let aspects = row(Aspect::ALL.iter().map(|&a| aspect_card(a, a == self.canvas.aspect))).spacing(S2);
        let (w, h) = self.canvas.size();
        let create_label = column![
            row![icon(code::SPARKLES, 15), text("Create project").font(INTER_SEMIBOLD).size(T_LARGE)].spacing(S2).align_y(Alignment::Center),
        ];
        let body = column![
            row![text("New project").font(INTER_SEMIBOLD).size(T_TITLE), space().width(Fill), container(text(format!("{w} × {h}")).font(INTER_MEDIUM).size(T_CAPTION)).padding([3, 8]).style(style::badge(ACCENT_HOVER))]
                .align_y(Alignment::Center),
            widgets::field(
                "Name",
                text_input("Project name", &self.name).on_input(Message::NameChanged).on_submit(Message::Create).padding([8, 11]).size(T_BODY).style(style::input),
            ),
            widgets::field("Format", aspects),
            row![
                container(widgets::field("Resolution", widgets::segmented(&Resolution::ALL, self.canvas.resolution, |r| r.label().to_string(), Message::ResolutionPicked)))
                    .width(Length::FillPortion(3)),
                container(widgets::field("Frame rate", widgets::segmented(&FRAME_RATES, self.canvas.fps, |f| format!("{f}"), Message::FpsPicked)))
                    .width(Length::FillPortion(2)),
            ]
            .spacing(S4),
            space().height(2),
            row![
                button(container(create_label).center_x(Fill)).width(Fill).padding([11, 16]).style(style::primary).on_press(Message::Create),
                widgets::secondary_button(Some(code::FOLDER_OPEN), "Open…", Some(Message::Open)).padding([11, 16]).height(Length::Shrink),
            ]
            .spacing(S2),
            container(widgets::ghost_button(Some(code::SPARKLES), "Try an example project first", Some(Message::TryExample)).padding([6, 10])).center_x(Fill),
        ]
        .spacing(S5);
        container(body).padding(S6).width(Length::FillPortion(11)).style(style::card).into()
    }
}

/// A card with a proportional preview of the frame shape.
fn aspect_card(aspect: Aspect, selected: bool) -> Element<'static, Message> {
    let (aw, ah) = aspect.ratio();
    let max = 34.0;
    let (w, h) = if aw >= ah { (max, max * ah as f32 / aw as f32) } else { (max * aw as f32 / ah as f32, max) };
    let shape = container(space().width(w).height(h)).style(move |_| iced::widget::container::Style {
        background: Some(if selected { style::brand_gradient() } else { SURFACE_4.into() }),
        border: iced::Border { color: if selected { iced::Color { a: 0.3, ..iced::Color::WHITE } } else { LINE_STRONG }, width: 1.0, radius: 3.0.into() },
        ..Default::default()
    });
    let body = column![
        container(shape).center_x(Fill).height(40).align_y(Alignment::Center),
        text(aspect.label()).font(INTER_SEMIBOLD).size(T_BODY),
        text(aspect.use_case()).size(10.5).color(TEXT_3).align_x(Alignment::Center),
    ]
    .spacing(4)
    .align_x(Alignment::Center);
    button(body).width(Fill).height(112).padding([10, 6]).style(style::choice_card(selected)).on_press(Message::AspectPicked(aspect)).into()
}

fn recent_list(recent: &[RecentProject]) -> Element<'_, Message> {
    let title = row![text("Recent").font(INTER_SEMIBOLD).size(T_TITLE), space().width(Fill), text(format!("{}", recent.len())).size(T_SMALL).color(TEXT_3)].align_y(Alignment::Center);
    let body: Element<'_, Message> = if recent.is_empty() {
        container(widgets::empty_state::<Message>(code::CLAPPERBOARD, "No projects yet", "Projects you create or open will show up here.", None)).height(320).into()
    } else {
        let rows = recent.iter().map(|r| {
            let exists = r.path.is_file();
            let thumb = container(tinted(code::FILM, 16, if exists { ON_ACCENT } else { TEXT_3 }))
                .center(Length::Fixed(40.0))
                .style(move |t| if exists { style::logo(t) } else { style::card(t) });
            let meta = if exists { format!("{}  ·  {}", r.summary, relative_time(r.opened_at)) } else { "File missing".to_string() };
            // A long name ends in "…" instead of running under the remove button (and is clipped
            // there too, the cut being a guess about the font).
            let info = column![
                text(widgets::fit(&r.name, 300.0, T_BODY)).font(INTER_MEDIUM).size(T_BODY).wrapping(text::Wrapping::None),
                text(meta).size(T_CAPTION).color(TEXT_3).wrapping(text::Wrapping::None),
            ]
            .spacing(2)
            .width(Fill)
            .clip(true);
            let forget = widgets::tip(
                widgets::icon_button(code::X, Some(Message::ForgetRecent(r.path.clone()))).width(26).height(26),
                "Remove from list",
                None,
                iced::widget::tooltip::Position::Left,
            );
            button(row![thumb, info, forget].spacing(S3).align_y(Alignment::Center))
                .width(Fill)
                .padding([7, 8])
                .style(style::list_row(false))
                .on_press_maybe(exists.then(|| Message::OpenRecent(r.path.clone())))
                .into()
        });
        widgets::scroll(column(rows).spacing(2)).height(340).into()
    };
    container(column![title, body].spacing(S3)).padding(S6).width(Length::FillPortion(8)).height(Fill).style(style::card).into()
}
