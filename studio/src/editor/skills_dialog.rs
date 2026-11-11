//! The Skills manager: reusable instruction blocks for the AI assistant, written once and
//! switched on or off per conversation instead of retyping them every time.

use iced::widget::{button, column, container, row, space, text, text_editor, text_input, toggler};
use iced::{Alignment, Element, Fill};

use super::Message;
use crate::settings::Skill;
use crate::ui::icons::{code, icon};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

pub const NAME_ID: &str = "skill-name";

pub struct SkillsDialog {
    /// The add/edit form is open: `index` is `Some` while editing an existing skill (by its
    /// position in `Editor::skills`), `None` while composing a new one.
    pub editing: Option<Editing>,
}

pub struct Editing {
    pub index: Option<usize>,
    pub name: String,
    pub body: text_editor::Content,
}

impl SkillsDialog {
    pub fn view<'a>(&'a self, skills: &'a [Skill], appear: f32) -> Element<'a, Message> {
        let body: Element<'a, Message> = match &self.editing {
            Some(editing) => self.form(editing),
            None => self.list(skills),
        };
        let card = container(body).width(440).padding(S6).style(style::popover);
        widgets::modal(card, Message::CloseSkills, appear)
    }

    fn list<'a>(&'a self, skills: &'a [Skill]) -> Element<'a, Message> {
        let head = row![
            container(icon(code::WAND_SPARKLES, 16).color(ON_ACCENT)).center(34).style(style::logo),
            column![text("Skills").font(INTER_SEMIBOLD).size(T_TITLE), text("Extra instructions the assistant follows when they're on").size(T_SMALL).color(TEXT_3)].spacing(2),
        ]
        .spacing(S3)
        .align_y(Alignment::Center);
        let rows: Element<'a, Message> = if skills.is_empty() {
            container(text("No skills yet. Add one below.").size(T_SMALL).color(TEXT_3)).padding([S4, 0.0]).into()
        } else {
            let items = skills.iter().enumerate().map(|(i, s)| {
                let font = if s.name.is_ascii() { INTER_MEDIUM } else { HIND_SEMIBOLD };
                container(
                    row![
                        toggler(s.enabled).on_toggle(move |on| Message::SkillToggle(i, on)).size(18).style(style::toggler),
                        text(&s.name).font(font).size(T_BODY).width(Fill),
                        widgets::tip(widgets::icon_button(code::PENCIL, Some(Message::SkillEditStart(Some(i)))), "Edit", None, iced::widget::tooltip::Position::Top),
                        widgets::tip(widgets::icon_button(code::TRASH_2, Some(Message::SkillDelete(i))), "Delete", None, iced::widget::tooltip::Position::Top),
                    ]
                    .spacing(S2)
                    .align_y(Alignment::Center),
                )
                .padding([8, 10])
                .style(style::inset)
                .into()
            });
            container(widgets::scroll(column(items).spacing(6))).max_height(280).into()
        };
        column![
            head,
            rows,
            row![
                widgets::primary_button(Some(code::PLUS), "New skill", Some(Message::SkillEditStart(None))),
                space().width(Fill),
                button(text("Close").size(T_BODY)).padding([8, 14]).style(style::secondary).on_press(Message::CloseSkills),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(S4)
        .into()
    }

    fn form<'a>(&'a self, editing: &'a Editing) -> Element<'a, Message> {
        let title = if editing.index.is_some() { "Edit skill" } else { "New skill" };
        let valid = !editing.name.trim().is_empty() && !editing.body.text().trim().is_empty();
        column![
            row![
                container(icon(code::WAND_SPARKLES, 16).color(ON_ACCENT)).center(34).style(style::logo),
                text(title).font(INTER_SEMIBOLD).size(T_TITLE),
            ]
            .spacing(S3)
            .align_y(Alignment::Center),
            text_input("Name (e.g. \"Brand colors\")", &editing.name).id(NAME_ID).on_input(Message::SkillNameInput).padding([9, 11]).size(T_BODY).style(style::input),
            text_editor(&editing.body)
                .placeholder("What should the assistant do or know? This is appended to its instructions whenever this skill is on.")
                .on_action(Message::SkillBodyEdit)
                .size(T_BODY)
                .padding([9, 11])
                .height(160)
                .style(style::text_area),
            row![
                space().width(Fill),
                button(text("Cancel").size(T_BODY)).padding([8, 14]).style(style::secondary).on_press(Message::SkillEditCancel),
                widgets::primary_button(None, "Save", valid.then_some(Message::SkillSave)),
            ]
            .spacing(S2),
        ]
        .spacing(S3)
        .into()
    }
}
