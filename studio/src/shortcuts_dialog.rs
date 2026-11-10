//! The Keyboard Shortcuts dialog, reachable from the app menu (or Ctrl+/): every shortcut in
//! one place, and where they are changed. Click a shortcut, press the new keys.

use iced::widget::{button, column, container, mouse_area, opaque, row, space, text};
use iced::{Alignment, Element, Fill};

use crate::shortcuts::{ACTIONS, Action, GROUPS, Keymap};
use crate::ui::icons::{code, icon};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

#[derive(Debug, Clone, Copy)]
pub enum Message {
    Close,
    /// Start (or, on the same row again, stop) waiting for the new keys of an action.
    Rebind(Action),
    /// Back to the default keys of one action, or of all of them.
    Reset(Action),
    ResetAll,
}

/// The keys of one action: a button that starts listening for new ones.
fn keys_button<'a>(action: Action, keymap: &Keymap, capturing: bool) -> Element<'a, Message> {
    let content: Element<'a, Message> = if capturing {
        text("Press the new keys…").font(INTER_MEDIUM).size(T_CAPTION).color(ACCENT).into()
    } else {
        let keys = keymap.keys(action);
        if keys.is_empty() {
            text("None").size(T_CAPTION).color(TEXT_4).into()
        } else {
            let mut r = row![].spacing(5).align_y(Alignment::Center);
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    r = r.push(text("or").size(10.5).color(TEXT_4));
                }
                r = r.push(widgets::kbd(k));
            }
            r.into()
        }
    };
    button(content).padding([3, 6]).style(style::ghost).on_press(Message::Rebind(action)).into()
}

fn group<'a>(title: &'static str, keymap: &Keymap, capturing: Option<Action>) -> Element<'a, Message> {
    let items = ACTIONS.iter().filter(|i| i.group == title).map(|i| {
        // Changed shortcuts get a small button that puts the default back.
        let reset: Element<'a, Message> = if keymap.is_custom(i.action) {
            widgets::tip(button(icon(code::UNDO_2, 12).color(TEXT_3)).padding(4).style(style::ghost).on_press(Message::Reset(i.action)), "Back to the default", None, iced::widget::tooltip::Position::Left)
        } else {
            space().width(20).into()
        };
        row![text(i.label).size(T_SMALL).color(TEXT_2).width(Fill), keys_button(i.action, keymap, capturing == Some(i.action)), reset].spacing(4).align_y(Alignment::Center).into()
    });
    container(column![widgets::section(title), column(items).spacing(3)].spacing(S3)).padding(S4).width(Fill).style(style::card).into()
}

pub fn view<'a>(keymap: &Keymap, capturing: Option<Action>) -> Element<'a, Message> {
    let groups = column(GROUPS.iter().map(|title| group(title, keymap, capturing))).spacing(S3);
    let header = row![
        container(icon(code::KEYBOARD, 16).color(ON_ACCENT)).center(34).style(style::logo),
        column![text("Keyboard shortcuts").font(INTER_SEMIBOLD).size(T_TITLE), text("Click a shortcut and press the keys you want instead. Esc cancels.").size(T_SMALL).color(TEXT_3)].spacing(2).width(Fill),
        widgets::icon_button(code::X, Some(Message::Close)),
    ]
    .spacing(S3)
    .align_y(Alignment::Center);
    let reset_all: Element<'a, Message> = if keymap.any_custom() { widgets::ghost_button(Some(code::UNDO_2), "Reset all to defaults", Some(Message::ResetAll)).into() } else { space().into() };
    let footer = row![reset_all, space().width(Fill), widgets::primary_button(None, "Done", Some(Message::Close))].align_y(Alignment::Center);
    let body = widgets::scroll(groups.padding(iced::Padding { right: 10.0, ..Default::default() })).height(iced::Length::Shrink);
    let card = container(column![header, container(body).max_height(520), widgets::divider(), footer].spacing(S4)).width(600).padding(S6).style(style::popover);
    let scrim = mouse_area(container(space().width(Fill).height(Fill)).style(style::scrim)).on_press(Message::Close);
    iced::widget::stack![scrim, container(opaque(card)).center(Fill)].into()
}
