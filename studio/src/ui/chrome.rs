//! Window chrome: custom title bar, window controls and resize edges.
//!
//! The window has no OS frame. On Windows the OS still draws its rounded corners
//! and shadow (see `main.rs`); on Linux we inset the content and draw them ourselves.

use iced::widget::{button, column, container, mouse_area, row, space, stack};
use iced::window::Direction;
use iced::{Alignment, Element, Fill, Length, mouse};

use super::icons::{code, icon};
use super::{style, widgets};
use super::tokens::*;

/// Messages the chrome emits; the app maps them to window tasks.
#[derive(Debug, Clone, Copy)]
pub enum WindowAction {
    Drag,
    ToggleMaximize,
    Minimize,
    Close,
    Resize(Direction),
}

/// Whether we draw our own rounded frame. The window is flush to its edges
/// everywhere; Windows still rounds the corners natively.
pub fn draws_frame(_maximized: bool) -> bool {
    false
}

/// Space around the frame for the drawn shadow.
pub const SHADOW_INSET: f32 = 10.0;

pub fn title_bar<'a, M: Clone + 'a>(
    left: Element<'a, M>,
    center: Element<'a, M>,
    right: Element<'a, M>,
    maximized: bool,
    on_window: impl Fn(WindowAction) -> M + Copy + 'a,
) -> Element<'a, M> {
    let rounded = draws_frame(maximized);
    let control = |code: char, action: WindowAction, close: bool| {
        button(container(icon(code, if close { 16 } else { 14 })).center(Length::Fill))
            .width(46)
            .height(TITLE_BAR_H)
            .padding(0)
            .style(style::window_control(close))
            .on_press(on_window(action))
    };
    let controls = row![
        control(code::MINUS, WindowAction::Minimize, false),
        control(if maximized { code::COPY } else { code::SQUARE }, WindowAction::ToggleMaximize, false),
        control(code::X, WindowAction::Close, true),
    ];
    // The right-hand buttons and the window controls get their room first, so a narrow window
    // never cuts off Minimize/Maximize/Close; the left side (name, then `center` just after it)
    // takes what is left and is clipped if even that is too little. The empty part of the left
    // side is where the window is dragged.
    let lead = row![left, center, space().width(Fill)].spacing(S4).align_y(Alignment::Center);
    let bar = row![
        widgets::clipped(container(lead).width(Fill).height(TITLE_BAR_H).align_y(Alignment::Center).clip(true)),
        space().width(S3),
        container(right).align_y(Alignment::Center),
        space().width(S3),
        controls,
    ]
    .height(TITLE_BAR_H)
    .align_y(Alignment::Center);
    let bar = column![container(bar).padding(iced::Padding { left: S3, ..Default::default() }), container(space().width(Fill).height(1)).style(style::hairline)];
    mouse_area(container(bar).width(Fill).style(style::title_bar(rounded)))
        .on_press(on_window(WindowAction::Drag))
        .on_double_click(on_window(WindowAction::ToggleMaximize))
        .into()
}

/// The app's mark: a play shape in motion (speed trails) on a lime-to-mint tile.
pub fn logo<'a, M: 'a>() -> Element<'a, M> {
    logo_sized(22.0)
}

pub fn logo_sized<'a, M: 'a>(size: f32) -> Element<'a, M> {
    iced::widget::canvas(Mark).width(size).height(size).into()
}

struct Mark;

impl<M> iced::widget::canvas::Program<M> for Mark {
    type State = ();

    fn draw(&self, _: &(), renderer: &iced::Renderer, _: &iced::Theme, bounds: iced::Rectangle, _: mouse::Cursor) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Gradient, Path, gradient};
        use iced::{Point, Size};
        let mut f = Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let tile = Path::rounded_rectangle(Point::ORIGIN, Size::new(s, s), (s * 0.3).into());
        let fill = Gradient::Linear(gradient::Linear::new(Point::ORIGIN, Point::new(s, s)).add_stop(0.0, ACCENT).add_stop(1.0, BRAND_2));
        f.fill(&tile, fill);
        // Speed trails behind the play shape.
        for (i, (y, len)) in [(0.36, 0.20), (0.5, 0.28), (0.64, 0.20)].into_iter().enumerate() {
            let a = if i == 1 { 0.85 } else { 0.5 };
            let bar = Path::rounded_rectangle(Point::new(s * (0.40 - len), s * y - s * 0.035), Size::new(s * len, s * 0.07), (s * 0.035).into());
            f.fill(&bar, iced::Color { a, ..ON_ACCENT });
        }
        let play = Path::new(|b| {
            b.move_to(Point::new(s * 0.42, s * 0.27));
            b.line_to(Point::new(s * 0.78, s * 0.5));
            b.line_to(Point::new(s * 0.42, s * 0.73));
            b.close();
        });
        f.fill(&play, ON_ACCENT);
        vec![f.into_geometry()]
    }
}

/// Wraps the window content in the frame, plus invisible resize edges when not maximized.
pub fn frame<'a, M: Clone + 'a>(content: Element<'a, M>, maximized: bool, on_window: impl Fn(WindowAction) -> M + Copy + 'a) -> Element<'a, M> {
    let rounded = draws_frame(maximized);
    let body = container(content).padding(if rounded { 1.0 } else { 0.0 }).width(Fill).height(Fill).style(style::window_frame(rounded));
    let framed: Element<'a, M> = if rounded { container(body).padding(SHADOW_INSET).width(Fill).height(Fill).into() } else { body.into() };
    if maximized {
        return framed;
    }
    stack![framed, resize_edges(on_window)].into()
}

fn resize_edges<'a, M: Clone + 'a>(on_window: impl Fn(WindowAction) -> M + Copy + 'a) -> Element<'a, M> {
    let e: f32 = if cfg!(windows) { 5.0 } else { SHADOW_INSET };
    let c = e + 6.0;
    let edge = |w: Length, h: Length, dir: Direction, cursor: mouse::Interaction| -> Element<'a, M> {
        mouse_area(space().width(w).height(h)).on_press(on_window(WindowAction::Resize(dir))).interaction(cursor).into()
    };
    use mouse::Interaction::{ResizingDiagonallyDown as Down, ResizingDiagonallyUp as Up, ResizingHorizontally as H, ResizingVertically as V};
    column![
        row![edge(c.into(), e.into(), Direction::NorthWest, Down), edge(Fill, e.into(), Direction::North, V), edge(c.into(), e.into(), Direction::NorthEast, Up)],
        row![edge(e.into(), Fill, Direction::West, H), space().width(Fill).height(Fill), edge(e.into(), Fill, Direction::East, H)],
        row![edge(c.into(), e.into(), Direction::SouthWest, Up), edge(Fill, e.into(), Direction::South, V), edge(c.into(), e.into(), Direction::SouthEast, Down)],
    ]
    .into()
}
