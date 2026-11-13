//! The Assistant tab: chat with the built-in motion designer.

use iced::widget::{button, column, container, image, markdown, row, space, text};
use iced::{Alignment, Color, Element, Fill, Length};

use super::{Editor, Message};
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

pub const SCROLL_ID: &str = "assistant-transcript";
pub const INPUT_ID: &str = "assistant-input";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
    /// The reply ended (stopped or failed) before the tool reported back.
    Unfinished,
}

pub enum ChatItem {
    User { text: String, context: Option<String> },
    Reply(Reply),
    Tool { id: String, label: String, state: ToolState, detail: String, image: Option<image::Handle> },
    Notice(String),
    /// The user stopped the reply.
    Stopped,
}

/// How a reply ended.
pub enum TurnEnd {
    Finished,
    Failed(String),
    Stopped,
}

impl Editor {
    /// Closes the reply in the transcript: no tool is left spinning, and an error or
    /// "Stopped" line says why it ended. Every way a reply can end goes through here.
    pub(super) fn end_chat_turn(&mut self, end: TurnEnd) {
        let left = if matches!(end, TurnEnd::Finished) { ToolState::Done } else { ToolState::Unfinished };
        for item in &mut self.chat {
            if let ChatItem::Tool { state: s @ ToolState::Running, .. } = item {
                *s = left;
            }
        }
        match end {
            TurnEnd::Finished => {}
            TurnEnd::Failed(e) => self.chat.push(ChatItem::Notice(e)),
            TurnEnd::Stopped => self.chat.push(ChatItem::Stopped),
        }
    }
}

/// An assistant reply: the raw text, and its Markdown parsed as it streams in.
pub struct Reply {
    pub text: String,
    pub md: markdown::Content,
}

impl Reply {
    pub fn new(text: &str) -> Reply {
        Reply { text: text.to_string(), md: markdown::Content::parse(text) }
    }

    pub fn push_str(&mut self, more: &str) {
        self.text.push_str(more);
        self.md.push_str(more);
    }
}

/// Markdown sizes for the side panel: headings only a little larger than the text.
fn markdown_settings(text: &str) -> markdown::Settings {
    let style = markdown::Style {
        // Bangla replies read best in Hind Siliguri; everything else in Inter.
        font: if text.chars().any(|c| ('\u{0980}'..='\u{09FF}').contains(&c)) { HIND } else { INTER },
        inline_code_highlight: markdown::Highlight { background: SURFACE_3.into(), border: iced::Border { radius: 4.0.into(), ..Default::default() } },
        inline_code_padding: iced::Padding { left: 3.0, right: 3.0, top: 0.0, bottom: 0.0 },
        inline_code_color: ACCENT_HOVER,
        inline_code_font: iced::Font::MONOSPACE,
        code_block_font: iced::Font::MONOSPACE,
        link_color: ACCENT,
    };
    let mut s = markdown::Settings::with_text_size(T_BODY, style);
    (s.h1_size, s.h2_size, s.h3_size) = (T_TITLE.into(), (T_LARGE + 1.0).into(), T_LARGE.into());
    (s.h4_size, s.h5_size, s.h6_size) = (T_BODY.into(), T_BODY.into(), T_BODY.into());
    s.code_size = T_SMALL.into();
    s.spacing = 9.0.into();
    s
}

/// Draws replies with the app's look: code blocks get a copy button, links open in the browser.
struct ChatViewer;

impl<'a> markdown::Viewer<'a, Message> for ChatViewer {
    fn on_link_click(url: markdown::Uri) -> Message {
        Message::OpenLink(url)
    }

    fn heading(&self, settings: markdown::Settings, level: &'a markdown::HeadingLevel, content: &'a markdown::Text, index: usize) -> Element<'a, Message> {
        use markdown::HeadingLevel as H;
        let size = match level {
            H::H1 => settings.h1_size,
            H::H2 => settings.h2_size,
            H::H3 => settings.h3_size,
            _ => settings.h4_size,
        };
        let font = iced::Font { weight: iced::font::Weight::Semibold, ..settings.style.font };
        container(iced::widget::rich_text(content.spans(settings.style)).on_link_click(Self::on_link_click).font(font).size(size))
            .padding(iced::Padding { top: if index > 0 { 6.0 } else { 0.0 }, ..Default::default() })
            .into()
    }

    fn code_block(&self, settings: markdown::Settings, language: Option<&'a str>, code: &'a str, lines: &'a [markdown::Text]) -> Element<'a, Message> {
        let head = row![
            text(language.filter(|l| !l.is_empty()).unwrap_or("code")).size(T_CAPTION).color(TEXT_3),
            space().width(Fill),
            widgets::tip(button(icon(code::COPY, 12).color(TEXT_3)).padding(3).style(style::ghost).on_press(Message::CopyText(code.trim_end().to_string())), "Copy", None, iced::widget::tooltip::Position::Left),
        ]
        .align_y(Alignment::Center);
        let body = iced::widget::scrollable(
            column(lines.iter().map(|line| iced::widget::rich_text(line.spans(settings.style)).on_link_click(Self::on_link_click).font(settings.style.code_block_font).size(settings.code_size).into()))
                .padding(iced::Padding { bottom: 6.0, ..Default::default() }),
        )
        .direction(iced::widget::scrollable::Direction::Horizontal(iced::widget::scrollable::Scrollbar::new().width(4).scroller_width(4)))
        .style(style::scrollbar);
        container(column![head, body].spacing(4)).padding([6, 10]).width(Fill).style(style::inset).into()
    }
}

const SUGGESTIONS: [&str; 5] = [
    "Add an Apple-style title for this product",
    "Zoom into the part of the screen I'm talking about",
    "Add a whoosh at every transition",
    "Make the face-cam a rounded card in the corner",
    "যে অংশটা সিলেক্ট করেছি সেখানে বাংলা শিরোনাম দাও",
];

impl Editor {
    pub(super) fn assistant_view(&self) -> Element<'_, Message> {
        if !self.agent.available() && !self.dev_chat {
            return widgets::empty_state(
                code::BOT,
                "Connect an AI",
                "Choose the AI that edits with you. It works through the same tools you use, and everything it does can be undone.",
                Some(widgets::primary_button(Some(code::LINK), "Connect AI", Some(Message::OpenAiConnect)).into()),
            );
        }
        let transcript: Element<'_, Message> = if self.chat.is_empty() {
            let hero = column![
                container(icon(code::SPARKLES, 18).color(ON_ACCENT)).center(40).style(style::logo),
                text("What should we make?").font(INTER_SEMIBOLD).size(T_TITLE),
                text("Click Range and drag across the timeline to point at a part, then describe the motion you want there.").size(T_SMALL).color(TEXT_3).align_x(Alignment::Center),
            ]
            .spacing(S2)
            .align_x(Alignment::Center);
            let chips = column(SUGGESTIONS.iter().map(|s| {
                let font = if s.is_ascii() { INTER } else { HIND };
                button(row![tinted(code::SPARKLE, 12, ACCENT_HOVER), text(*s).font(font).size(T_SMALL)].spacing(8).align_y(Alignment::Center))
                    .width(Fill)
                    .padding([8, 10])
                    .style(style::choice_card(false))
                    .on_press(Message::ChatSuggestion(s.to_string()))
                    .into()
            }))
            .spacing(6);
            widgets::scroll(column![space().height(12), hero, space().height(8), chips].spacing(S3).padding([8, 12])).height(Fill).into()
        } else {
            let last = self.chat.len().saturating_sub(1);
            let items = self.chat.iter().enumerate().map(|(i, item)| -> Element<'_, Message> {
                match item {
                    ChatItem::User { text: t, context } => {
                        let mut col = column![].spacing(4).align_x(Alignment::End);
                        if let Some(c) = context {
                            col = col.push(text(c).size(10.5).color(TEXT_4));
                        }
                        let font = if t.is_ascii() { INTER } else { HIND };
                        col = col.push(container(text(t).font(font).size(T_BODY)).padding([8, 11]).max_width(260).style(|_| container::Style {
                            background: Some(ACCENT_SOFT.into()),
                            text_color: Some(TEXT_1),
                            border: iced::Border { color: Color { a: 0.3, ..ACCENT }, width: 1.0, radius: iced::border::Radius { top_left: 12.0, top_right: 12.0, bottom_right: 4.0, bottom_left: 12.0 } },
                            ..Default::default()
                        }));
                        container(col).width(Fill).align_right(Fill).into()
                    }
                    ChatItem::Reply(r) => {
                        let mut body = column![markdown::view_with(r.md.items(), markdown_settings(&r.text), &ChatViewer)].spacing(4).width(Fill);
                        // Copy the whole reply as Markdown, once it has finished streaming.
                        if !(self.agent.busy && i == last) && !r.text.trim().is_empty() {
                            let copy = button(row![icon(code::COPY, 11).color(TEXT_3), text("Copy").size(T_CAPTION).color(TEXT_3)].spacing(5).align_y(Alignment::Center))
                                .padding([2, 6])
                                .style(style::ghost)
                                .on_press(Message::CopyText(r.text.trim().to_string()));
                            body = body.push(copy);
                        }
                        row![container(icon(code::SPARKLES, 11).color(ON_ACCENT)).center(20).style(style::logo), body].spacing(8).into()
                    }
                    ChatItem::Tool { label, state, detail, image: img, .. } => {
                        let (glyph, color) = match state {
                            ToolState::Running => (code::LOADER_CIRCLE, ACCENT_HOVER),
                            ToolState::Done => (code::CIRCLE_CHECK, SUCCESS),
                            ToolState::Failed => (code::CIRCLE_X, DANGER),
                            ToolState::Unfinished => (code::CIRCLE_DASHED, TEXT_4),
                        };
                        let head: Element<'_, Message> = if *state == ToolState::Running {
                            iced_aw::Spinner::new().width(Length::Fixed(12.0)).height(Length::Fixed(12.0)).circle_radius(1.5).into()
                        } else {
                            tinted(glyph, 12, color).into()
                        };
                        let mut col = column![row![head, text(label).size(T_SMALL).color(TEXT_2)].spacing(8).align_y(Alignment::Center)].spacing(6);
                        if *state == ToolState::Failed && !detail.is_empty() {
                            col = col.push(text(detail).size(T_CAPTION).color(DANGER));
                        }
                        if let Some(h) = img {
                            col = col.push(image(h.clone()).width(Fill).border_radius(6.0));
                        }
                        container(col).padding([6, 10]).width(Fill).style(style::inset).into()
                    }
                    ChatItem::Notice(t) => {
                        container(row![tinted(code::TRIANGLE_ALERT, 12, WARNING), text(t).size(T_SMALL).color(TEXT_2)].spacing(8)).padding([6, 10]).width(Fill).style(style::badge(WARNING)).into()
                    }
                    ChatItem::Stopped => row![tinted(code::SQUARE, 10, TEXT_4), text("Stopped").size(T_CAPTION).color(TEXT_3)].spacing(6).align_y(Alignment::Center).into(),
                }
            });
            let mut col = column(items).spacing(10);
            if self.agent.busy && !matches!(self.chat.last(), Some(ChatItem::Tool { state: ToolState::Running, .. })) {
                col = col.push(row![iced_aw::Spinner::new().width(Length::Fixed(14.0)).height(Length::Fixed(14.0)).circle_radius(1.5), text("Thinking…").size(T_SMALL).color(TEXT_3)].spacing(8).align_y(Alignment::Center));
            }
            // Follows new content only while the user is at the bottom.
            widgets::scroll(col.padding([8, 12]))
                .id(SCROLL_ID)
                .on_scroll(|v| {
                    let y = v.relative_offset().y;
                    Message::ChatScrolled(y.is_nan() || y >= 0.98 || v.content_bounds().height <= v.bounds().height)
                })
                .height(Fill)
                .into()
        };

        // What the assistant will see along with the message.
        let mut chips = row![].spacing(6);
        if let Some((a, b)) = self.range {
            chips = chips.push(
                container(
                    row![
                        tinted(code::SQUARE_DASHED, 12, ACCENT_HOVER),
                        text(format!("{} – {}", super::fmt_ruler(a), super::fmt_ruler(b))).font(INTER_MEDIUM).size(T_CAPTION),
                        button(icon(code::X, 11)).padding(0).style(style::ghost).on_press(Message::RangeDone),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([3, 8])
                .style(style::badge(ACCENT_HOVER)),
            );
        }
        if let [id] = self.selection.as_slice() {
            if let Some(c) = self.doc.project.clip(*id) {
                let label = super::timeline::clip_label(&self.doc.project, c);
                let font = if label.is_ascii() { INTER_MEDIUM } else { HIND };
                chips = chips.push(
                    container(row![tinted(code::MOUSE_POINTER_2, 12, TEXT_3), text(label).font(font).size(T_CAPTION).wrapping(text::Wrapping::None)].spacing(6).align_y(Alignment::Center))
                        .padding([3, 8])
                        .max_width(180)
                        .clip(true)
                        .style(style::segmented_track),
                );
            }
        }
        let send: Element<'_, Message> = if self.agent.busy {
            widgets::tip(button(container(icon(code::SQUARE, 13)).center(32)).width(34).height(34).padding(0).style(style::secondary).on_press(Message::ChatStop), "Stop", None, iced::widget::tooltip::Position::Top)
        } else {
            let ready = !self.chat_input.text().trim().is_empty();
            button(container(icon(code::SEND, 14)).center(32)).width(34).height(34).padding(0).style(style::primary).on_press_maybe(ready.then_some(Message::ChatSend)).into()
        };
        let font = if self.chat_input.text().is_ascii() { INTER } else { HIND };
        let composer = container(
            column![
                chips,
                row![
                    // Enter starts a new line; only the Send button sends. Grows up to about six lines.
                    iced::widget::text_editor(&self.chat_input)
                        .id(INPUT_ID)
                        .placeholder("Describe what you want…")
                        .on_action(Message::ChatEdit)
                        .key_binding(widgets::editor_keys)
                        .font(font)
                        .size(T_BODY)
                        .padding([9, 11])
                        .min_height(40)
                        .max_height(140)
                        .style(style::text_area),
                    send,
                ]
                .spacing(S2)
                .align_y(Alignment::End),
            ]
            .spacing(S2),
        )
        .padding([10, 10]);
        column![transcript, widgets::divider(), composer].into()
    }
}
