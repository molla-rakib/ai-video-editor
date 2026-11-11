//! Reusable components built from the tokens and styles.

use iced::widget::tooltip::Position;
use iced::widget::{button, column, container, mouse_area, opaque, row, scrollable, space, stack, text, tooltip};
use iced::{Alignment, Background, Element, Fill, Length, Padding};

use super::icons::{self, icon};
use super::style;
use super::tokens::*;

pub fn label<'a, M: 'a>(s: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(s).size(T_SMALL).color(TEXT_3).into()
}

/// Small uppercase heading above a group of controls.
pub fn section<'a, M: 'a>(title: &str) -> Element<'a, M> {
    text(title.to_uppercase()).font(INTER_SEMIBOLD).size(10.5).color(TEXT_3).into()
}

pub fn kbd<'a, M: 'a>(keys: impl text::IntoFragment<'a>) -> Element<'a, M> {
    container(text(keys).font(INTER_MEDIUM).size(10.5)).padding([1, 5]).style(style::kbd).into()
}

pub fn divider<'a, M: 'a>() -> Element<'a, M> {
    container(space().width(Fill).height(1)).style(style::hairline).into()
}

pub fn vdivider<'a, M: 'a>(height: f32) -> Element<'a, M> {
    container(space().width(1).height(height)).style(style::hairline).into()
}

/// Wraps `content` in a styled tooltip with an optional shortcut hint.
pub fn tip<'a, M: 'a>(content: impl Into<Element<'a, M>>, label: &'a str, shortcut: Option<&'a str>, position: Position) -> Element<'a, M> {
    let mut body = row![text(label).size(T_SMALL)].spacing(S2).align_y(Alignment::Center);
    if let Some(keys) = shortcut {
        body = body.push(text(keys).size(T_CAPTION).color(TEXT_3));
    }
    tooltip(content, container(body).padding([5, 8]).style(style::tooltip), position).gap(6).snap_within_viewport(true).into()
}

/// A tooltip whose shortcut comes from the keymap (`keys` is empty when the action has none).
pub fn tip_keys<'a, M: 'a>(content: impl Into<Element<'a, M>>, label: &'a str, keys: String, position: Position) -> Element<'a, M> {
    let mut body = row![text(label).size(T_SMALL)].spacing(S2).align_y(Alignment::Center);
    if !keys.is_empty() {
        body = body.push(text(keys).size(T_CAPTION).color(TEXT_3));
    }
    tooltip(content, container(body).padding([5, 8]).style(style::tooltip), position).gap(6).snap_within_viewport(true).into()
}

pub fn icon_button<'a, M: Clone + 'a>(code: char, on_press: Option<M>) -> button::Button<'a, M> {
    button(container(icon(code, 15)).center(CONTROL_H - 2.0)).width(CONTROL_H - 2.0).height(CONTROL_H - 2.0).padding(0).style(style::ghost).on_press_maybe(on_press)
}

pub fn icon_toggle<'a, M: Clone + 'a>(code: char, active: bool, on_press: M) -> button::Button<'a, M> {
    button(container(icon(code, 15)).center(CONTROL_H - 2.0)).width(CONTROL_H - 2.0).height(CONTROL_H - 2.0).padding(0).style(style::toggle(active)).on_press(on_press)
}

fn labeled<'a, M: 'a>(code: Option<char>, label: impl text::IntoFragment<'a>, font: iced::Font) -> Element<'a, M> {
    let mut r = row![].spacing(7).align_y(Alignment::Center);
    if let Some(c) = code {
        r = r.push(icon(c, 14));
    }
    r.push(text(label).font(font).size(T_BODY)).into()
}

pub fn primary_button<'a, M: Clone + 'a>(code: Option<char>, label: impl text::IntoFragment<'a>, on_press: Option<M>) -> button::Button<'a, M> {
    button(labeled(code, label, INTER_SEMIBOLD)).padding([7, 14]).height(CONTROL_H).style(style::primary).on_press_maybe(on_press)
}

pub fn secondary_button<'a, M: Clone + 'a>(code: Option<char>, label: impl text::IntoFragment<'a>, on_press: Option<M>) -> button::Button<'a, M> {
    button(labeled(code, label, INTER_MEDIUM)).padding([7, 13]).height(CONTROL_H).style(style::secondary).on_press_maybe(on_press)
}

pub fn ghost_button<'a, M: Clone + 'a>(code: Option<char>, label: impl text::IntoFragment<'a>, on_press: Option<M>) -> button::Button<'a, M> {
    button(labeled(code, label, INTER_MEDIUM)).padding([7, 10]).height(CONTROL_H).style(style::ghost).on_press_maybe(on_press)
}

/// A row of mutually exclusive options.
pub fn segmented<'a, T, M>(options: &[T], selected: T, label: impl Fn(&T) -> String, on_select: impl Fn(T) -> M) -> Element<'a, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'a,
{
    let items = options.iter().map(|&o| {
        button(container(text(label(&o)).font(INTER_MEDIUM).size(T_SMALL).wrapping(text::Wrapping::None)).center_x(Fill))
            .width(Fill)
            .padding([5, 4])
            .style(style::segment(o == selected))
            .on_press(on_select(o))
            .into()
    });
    container(row(items).spacing(2)).padding(2).style(style::segmented_track).into()
}

/// A labelled control stacked vertically.
/// A segmented control with an icon on each choice instead of a word (each with a tooltip).
pub fn segmented_icons<'a, T, M>(options: &[(T, char, &'a str)], selected: T, on_select: impl Fn(T) -> M) -> Element<'a, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'a,
{
    let items = options.iter().map(|&(o, glyph, name)| {
        let choice = button(container(icon(glyph, 14)).center_x(Fill)).width(Fill).padding([5, 4]).style(style::segment(o == selected)).on_press(on_select(o));
        tip(choice, name, None, Position::Top)
    });
    container(row(items).spacing(2)).padding(2).style(style::segmented_track).into()
}

pub fn field<'a, M: 'a>(name: &'a str, control: impl Into<Element<'a, M>>) -> Element<'a, M> {
    column![label(name), control.into()].spacing(6).into()
}

pub fn scroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> scrollable::Scrollable<'a, M> {
    scrollable(content)
        .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(6).scroller_width(6).margin(2)))
        .style(style::scrollbar)
}

pub enum MenuItem<'a, M> {
    Action { icon: Option<char>, label: &'a str, shortcut: Option<std::borrow::Cow<'a, str>>, on_press: Option<M>, danger: bool },
    /// A row of small icon buttons with their names below: the everyday actions (cut, copy,
    /// paste…) in one line instead of one row each.
    Icons(Vec<MenuIcon<'a, M>>),
    Separator,
}

/// One button of a menu's icon row.
pub struct MenuIcon<'a, M> {
    icon: char,
    label: &'a str,
    on_press: Option<M>,
    danger: bool,
}

impl<'a, M> MenuIcon<'a, M> {
    pub fn new(icon: char, label: &'a str, on_press: M) -> Self {
        MenuIcon { icon, label, on_press: Some(on_press), danger: false }
    }

    pub fn danger(self) -> Self {
        MenuIcon { danger: true, ..self }
    }

    pub fn disabled(self) -> Self {
        MenuIcon { on_press: None, ..self }
    }
}

impl<'a, M> MenuItem<'a, M> {
    pub fn new(icon: char, label: &'a str, on_press: M) -> Self {
        MenuItem::Action { icon: Some(icon), label, shortcut: None, on_press: Some(on_press), danger: false }
    }

    /// The keys shown next to the label (nothing when empty: the action has no shortcut).
    pub fn shortcut(self, keys: impl Into<std::borrow::Cow<'a, str>>) -> Self {
        let keys = keys.into();
        match self {
            MenuItem::Action { icon, label, on_press, danger, .. } => MenuItem::Action { icon, label, shortcut: (!keys.is_empty()).then_some(keys), on_press, danger },
            s => s,
        }
    }

    pub fn danger(self) -> Self {
        match self {
            MenuItem::Action { icon, label, shortcut, on_press, .. } => MenuItem::Action { icon, label, shortcut, on_press, danger: true },
            s => s,
        }
    }

    pub fn disabled(self) -> Self {
        match self {
            MenuItem::Action { icon, label, shortcut, danger, .. } => MenuItem::Action { icon, label, shortcut, on_press: None, danger },
            s => s,
        }
    }
}

/// A dropdown or context menu panel.
pub fn menu<'a, M: Clone + 'a>(items: Vec<MenuItem<'a, M>>, width: f32) -> Element<'a, M> {
    let rows = items.into_iter().map(|item| match item {
        MenuItem::Separator => container(divider()).padding([4, 6]).into(),
        MenuItem::Icons(icons) => row(icons.into_iter().map(|i| {
            let face = column![icon(i.icon, 15), text(i.label).size(T_CAPTION).wrapping(text::Wrapping::None)].spacing(3).align_x(Alignment::Center).width(Fill);
            button(face).width(Fill).padding([7, 0]).style(style::menu_item(i.danger)).on_press_maybe(i.on_press).into()
        }))
        .spacing(1)
        .into(),
        MenuItem::Action { icon: code, label, shortcut, on_press, danger } => {
            let mut r = row![].spacing(9).align_y(Alignment::Center);
            r = r.push(container(code.map(|c| -> Element<'a, M> { icon(c, 14).into() }).unwrap_or_else(|| space().width(14).into())).width(14));
            r = r.push(text(label).size(T_BODY).width(Fill));
            if let Some(keys) = shortcut {
                r = r.push(text(keys).size(T_CAPTION).color(TEXT_3));
            }
            button(r).width(Fill).padding([6, 9]).style(style::menu_item(danger)).on_press_maybe(on_press).into()
        }
    });
    // Opaque: the whole panel takes the mouse, not just its rows, so nothing under an open menu
    // (toolbar buttons, clips) reacts to the pointer through its gaps, separators or greyed-out items.
    opaque(container(column(rows).spacing(1)).width(width).padding(5).style(style::popover))
}

/// A dialog over the whole window: a dimmed backdrop (a click on it sends `dismiss`) with
/// `card` in the middle. `appear` runs 0 to 1 as the dialog opens (see `motion::Appear`): the
/// backdrop fades in and the card rises the last few pixels into place.
pub fn modal<'a, M: Clone + 'a>(card: impl Into<Element<'a, M>>, dismiss: M, appear: f32) -> Element<'a, M> {
    let k = appear.clamp(0.0, 1.0);
    let backdrop = container(space().width(Fill).height(Fill)).style(move |theme| {
        let mut s = style::scrim(theme);
        if let Some(Background::Color(c)) = &mut s.background {
            c.a *= k;
        }
        s
    });
    // Extra space above the card moves its center down; it shrinks to nothing as the card settles.
    let lift = (1.0 - k) * 28.0;
    stack![mouse_area(backdrop).on_press(dismiss), container(opaque(card)).center(Fill).padding(Padding { top: lift, ..Padding::ZERO })].into()
}

pub fn empty_state<'a, M: Clone + 'a>(code: char, title: &'a str, body: &'a str, action: Option<Element<'a, M>>) -> Element<'a, M> {
    let mut col = column![
        container(icons::tinted(code, 20, TEXT_2)).center(44).style(style::card),
        text(title).font(INTER_SEMIBOLD).size(T_LARGE),
        text(body).size(T_SMALL).color(TEXT_3).align_x(Alignment::Center),
    ]
    .spacing(S2)
    .align_x(Alignment::Center)
    .max_width(260);
    if let Some(a) = action {
        col = col.push(space().height(4)).push(a);
    }
    container(col).center(Fill).padding(S4).into()
}

pub fn pad(top: f32, right: f32, bottom: f32, left: f32) -> Padding {
    Padding { top, right, bottom, left }
}

pub fn fixed(px: f32) -> Length {
    Length::Fixed(px)
}

/// Truly clips `content` to its bounds. (`container(..).clip(true)` only narrows the viewport
/// it passes down, so backgrounds and borders moved outside still draw over their neighbors.)
/// The mouse doesn't reach the parts outside either.
pub fn clipped<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(Clipped { content: content.into() })
}

struct Clipped<'a, M> {
    content: Element<'a, M>,
}

mod clipped_impl {
    use iced::advanced::widget::{Operation, Tree, tree};
    use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
    use iced::{Event, Length, Rectangle, Renderer, Size, Theme, Vector};

    use super::Clipped;

    /// The cursor as seen by the content: gone when it's outside the visible part.
    fn inside(cursor: mouse::Cursor, bounds: Rectangle) -> mouse::Cursor {
        if cursor.is_over(bounds) { cursor } else { mouse::Cursor::Unavailable }
    }

    impl<M> Widget<M, Theme, Renderer> for Clipped<'_, M> {
        fn tag(&self) -> tree::Tag {
            self.content.as_widget().tag()
        }

        fn state(&self) -> tree::State {
            self.content.as_widget().state()
        }

        fn children(&self) -> Vec<Tree> {
            self.content.as_widget().children()
        }

        fn diff(&self, tree: &mut Tree) {
            self.content.as_widget().diff(tree);
        }

        fn size(&self) -> Size<Length> {
            self.content.as_widget().size()
        }

        fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
            self.content.as_widget_mut().layout(tree, renderer, limits)
        }

        fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
            self.content.as_widget_mut().operate(tree, layout, renderer, operation);
        }

        fn update(&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &Renderer, clipboard: &mut dyn Clipboard, shell: &mut Shell<'_, M>, viewport: &Rectangle) {
            let bounds = layout.bounds();
            let viewport = viewport.intersection(&bounds).unwrap_or(Rectangle::new(bounds.position(), Size::ZERO));
            self.content.as_widget_mut().update(tree, event, layout, inside(cursor, bounds), renderer, clipboard, shell, &viewport);
        }

        fn mouse_interaction(&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle, renderer: &Renderer) -> mouse::Interaction {
            self.content.as_widget().mouse_interaction(tree, layout, inside(cursor, layout.bounds()), viewport, renderer)
        }

        fn draw(&self, tree: &Tree, renderer: &mut Renderer, theme: &Theme, style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle) {
            use iced::advanced::Renderer as _;
            let bounds = layout.bounds();
            let Some(visible) = viewport.intersection(&bounds) else { return };
            renderer.with_layer(visible, |r| self.content.as_widget().draw(tree, r, theme, style, layout, inside(cursor, bounds), &visible));
        }

        fn overlay<'b>(&'b mut self, tree: &'b mut Tree, layout: Layout<'b>, renderer: &Renderer, viewport: &Rectangle, translation: Vector) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
            self.content.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
        }
    }
}

/// Keys for multi-line text boxes: the defaults, plus deleting a word at a time like
/// single-line inputs do (Ctrl+Backspace / Ctrl+Delete; Option on macOS).
pub fn editor_keys<M>(key: iced::widget::text_editor::KeyPress) -> Option<iced::widget::text_editor::Binding<M>> {
    use iced::keyboard::{Key, key::Named};
    use iced::widget::text_editor::{Binding, Motion};
    if key.modifiers.jump() {
        match key.key {
            Key::Named(Named::Backspace) => return Some(Binding::Sequence(vec![Binding::Select(Motion::WordLeft), Binding::Backspace])),
            Key::Named(Named::Delete) => return Some(Binding::Sequence(vec![Binding::Select(Motion::WordRight), Binding::Delete])),
            _ => {}
        }
    }
    Binding::from_key_press(key)
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{Key, Modifiers, key};
    use iced::widget::text_editor::{Action, Binding, Content, Edit, KeyPress, Motion};

    fn press(named: key::Named, modifiers: Modifiers) -> Option<Binding<()>> {
        super::editor_keys(KeyPress { key: Key::Named(named), modified_key: Key::Named(named), physical_key: iced::keyboard::key::Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified), modifiers, text: None, status: iced::widget::text_editor::Status::Focused { is_hovered: false } })
    }

    /// Runs a binding the way the text box does, action by action.
    fn apply(content: &mut Content, b: Binding<()>) {
        match b {
            Binding::Sequence(list) => list.into_iter().for_each(|b| apply(content, b)),
            Binding::Select(m) => content.perform(Action::Select(m)),
            Binding::Backspace => content.perform(Action::Edit(Edit::Backspace)),
            Binding::Delete => content.perform(Action::Edit(Edit::Delete)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ctrl_backspace_and_ctrl_delete_remove_a_word() {
        let jump = if cfg!(target_os = "macos") { Modifiers::ALT } else { Modifiers::CTRL };
        let mut c = Content::with_text("Launch your store today");
        c.perform(Action::Move(Motion::DocumentEnd));
        apply(&mut c, press(key::Named::Backspace, jump).unwrap());
        assert_eq!(c.text(), "Launch your store ");
        c.perform(Action::Move(Motion::DocumentStart));
        apply(&mut c, press(key::Named::Delete, jump).unwrap());
        assert_eq!(c.text().trim_start(), "your store ");
        // A plain Backspace still removes one character.
        assert!(matches!(press(key::Named::Backspace, Modifiers::empty()), Some(Binding::Backspace)));
    }
}
