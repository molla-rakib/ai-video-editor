//! The Connect AI dialog: choose which AI service the assistant uses.

use iced::widget::{button, column, container, row, space, text};
use iced::{Alignment, Element, Fill};

use crate::assistant::{CODEX_INSTALL, CODEX_SIGNED_OUT};
use crate::settings::AiProvider;
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

#[derive(Debug, Clone)]
pub enum Message {
    Close,
    Connect(AiProvider),
    Disconnect,
    /// ChatGPT: open the sign-in page (through Codex) in the browser.
    SignIn,
    CopyUrl,
    /// The MCP setup snippet for other AI apps.
    CopySetup,
    /// The command that installs Codex.
    CopyInstall,
}

/// What the dialog knows about the connection.
pub struct State<'a> {
    pub connected: Option<AiProvider>,
    /// The provider being checked right now.
    pub checking: Option<AiProvider>,
    /// Waiting for the user to finish the ChatGPT sign-in in the browser.
    pub signing_in: bool,
    /// The last check: the tool's version, or why connecting failed.
    pub detail: Option<(AiProvider, &'a Result<String, String>)>,
    pub mcp_url: String,
}

/// The setup other MCP apps (Claude Desktop, Cursor, VS Code…) need to reach the editor.
pub fn setup_snippet(url: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": { crate::mcp::SERVER_KEY: { "type": "http", "url": url } } })).unwrap_or_default()
}

/// A small square mark with the service's initial, for the provider rows.
fn mark<'a, M: 'a>(letter: &'a str, active: bool) -> Element<'a, M> {
    container(text(letter).font(INTER_BOLD).size(T_LARGE).color(if active { ON_ACCENT } else { TEXT_2 }))
        .center(38)
        .style(move |t| if active { style::logo(t) } else { style::card(t) })
        .into()
}

fn status_chip<'a, M: 'a>(label: impl text::IntoFragment<'a>, color: iced::Color) -> Element<'a, M> {
    container(row![container(space().width(6).height(6)).style(style::swatch(color, 3.0)), text(label).size(T_CAPTION).color(TEXT_2)].spacing(6).align_y(Alignment::Center))
        .padding([3, 8])
        .style(style::segmented_track)
        .into()
}

fn note<'a>(body: impl text::IntoFragment<'a>, color: iced::Color) -> Element<'a, Message> {
    container(text(body).size(T_SMALL).color(TEXT_2)).padding([8, 10]).width(Fill).style(style::badge(color)).into()
}

/// One provider: its mark, name and status, what it uses, and the action button.
fn provider<'a>(s: &State<'a>, p: AiProvider) -> Element<'a, Message> {
    let connected = s.connected == Some(p);
    let checking = s.checking == Some(p);
    let signing_in = p == AiProvider::Codex && s.signing_in;
    let detail = s.detail.filter(|(dp, _)| *dp == p).map(|(_, r)| r);
    let signed_out = matches!(detail, Some(Err(e)) if e == CODEX_SIGNED_OUT);
    let missing = matches!(detail, Some(Err(e)) if e.contains("isn't installed"));
    let status = match (connected, detail) {
        _ if checking => status_chip("Checking…", ACCENT_HOVER),
        _ if signing_in => status_chip("Signing in…", ACCENT_HOVER),
        (true, Some(Ok(v))) => status_chip(v.as_str(), SUCCESS),
        (true, _) => status_chip("Connected", SUCCESS),
        _ if signed_out => status_chip("Not signed in", WARNING),
        _ if missing => status_chip("Not installed", DANGER),
        (false, Some(Err(_))) => status_chip("Problem", DANGER),
        _ => status_chip("Not connected", TEXT_4),
    };
    let busy = checking || signing_in;
    let action: Element<'a, Message> = if connected {
        widgets::secondary_button(None, "Disconnect", Some(Message::Disconnect)).into()
    } else if signed_out || signing_in {
        widgets::primary_button(Some(code::LINK), "Sign in with ChatGPT", (!busy).then_some(Message::SignIn)).into()
    } else {
        widgets::primary_button(Some(code::LINK), "Connect", (!busy).then_some(Message::Connect(p))).into()
    };
    let (letter, name, about) = match p {
        AiProvider::ClaudeCode => ("C", "Claude Code", "Uses your Claude subscription through the Claude Code app. No API key."),
        AiProvider::Codex => ("G", "ChatGPT", "Uses your ChatGPT plan through OpenAI's Codex app. You sign in once in the browser."),
    };
    let mut body = column![
        row![
            mark(letter, connected),
            column![row![text(name).font(INTER_SEMIBOLD).size(T_LARGE), status].spacing(S2).align_y(Alignment::Center), text(about).size(T_SMALL).color(TEXT_3)].spacing(4).width(Fill),
            action,
        ]
        .spacing(S3)
        .align_y(Alignment::Center),
    ]
    .spacing(S2);
    if signing_in {
        body = body.push(note("Finish signing in to ChatGPT in your browser. This updates by itself when you're done.", ACCENT_HOVER));
    } else if signed_out {
        body = body.push(text("Codex is installed. Sign in with your ChatGPT account to let it edit here.").size(T_SMALL).color(TEXT_3));
    } else if missing && p == AiProvider::Codex {
        body = body.push(note("Codex isn't installed yet. It needs Node.js; install it from a terminal, then click Connect again:", DANGER));
        body = body.push(
            row![
                container(text(CODEX_INSTALL).font(iced::Font::MONOSPACE).size(T_SMALL)).padding([7, 10]).width(Fill).style(style::inset),
                widgets::secondary_button(Some(code::COPY), "Copy", Some(Message::CopyInstall)),
            ]
            .spacing(S2)
            .align_y(Alignment::Center),
        );
    } else if let (false, Some(Err(e))) = (connected, detail) {
        body = body.push(note(e.as_str(), DANGER));
    }
    container(body)
        .padding(S4)
        .width(Fill)
        .style(move |t| {
            let mut st = style::card(t);
            if !connected {
                st.background = Some(SURFACE_1.into());
            }
            st
        })
        .into()
}

pub fn view<'a>(s: State<'a>, appear: f32) -> Element<'a, Message> {
    let other = column![
        row![tinted(code::LINK, 13, TEXT_3), text("Other AI apps").font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_2)].spacing(7).align_y(Alignment::Center),
        text("Claude Desktop, Cursor, VS Code and other MCP apps on this computer can edit this project too, while the editor is open. Add this server to the app's MCP settings:").size(T_SMALL).color(TEXT_3),
        row![
            container(text(s.mcp_url.clone()).font(INTER_MEDIUM).size(T_SMALL)).padding([7, 10]).width(Fill).style(style::inset),
            widgets::secondary_button(Some(code::COPY), "Copy", Some(Message::CopyUrl)),
            widgets::tip(widgets::secondary_button(Some(code::COPY), "Copy setup", Some(Message::CopySetup)), "The JSON most MCP apps accept", None, iced::widget::tooltip::Position::Top),
        ]
        .spacing(S2)
        .align_y(Alignment::Center),
    ]
    .spacing(S2);

    let card = container(
        column![
            row![
                container(icon(code::SPARKLES, 16).color(ON_ACCENT)).center(34).style(style::logo),
                column![text("Connect AI").font(INTER_SEMIBOLD).size(T_TITLE), text("Pick the AI that edits with you. Everything it does can be undone.").size(T_SMALL).color(TEXT_3)].spacing(2).width(Fill),
                widgets::icon_button(code::X, Some(Message::Close)),
            ]
            .spacing(S3)
            .align_y(Alignment::Center),
            provider(&s, AiProvider::ClaudeCode),
            provider(&s, AiProvider::Codex),
            widgets::divider(),
            other,
        ]
        .spacing(S4),
    )
    .width(560)
    .padding(S6)
    .style(style::popover);
    widgets::modal(card, Message::Close, appear)
}

/// The title-bar chip that shows which AI is connected.
pub fn chip<'a, M: Clone + 'a>(connected: Option<AiProvider>, on_press: M) -> Element<'a, M> {
    let (label, color) = match connected {
        Some(p) => (p.name(), ACCENT),
        None => ("Connect AI", TEXT_4),
    };
    let content = row![container(space().width(7).height(7)).style(style::swatch(color, 3.5)), text(label).font(INTER_MEDIUM).size(T_SMALL)].spacing(7).align_y(Alignment::Center);
    button(content).padding([5, 10]).style(style::secondary).on_press(on_press).into()
}
