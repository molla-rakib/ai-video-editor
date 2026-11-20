//! A full color picker, like the browser's color input: a saturation/brightness square, a hue
//! bar and (where the color can be see-through) an opacity bar, plus a swatch that shows a
//! color over a checkerboard so transparency is visible.

use std::rc::Rc;

use iced::widget::canvas::{self, Action, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Fill, Point, Rectangle, Renderer, Size, Theme, mouse};

use crate::clip::Rgba;
use crate::ui::tokens::*;

const SQUARE_H: f32 = 132.0;
const BAR_H: f32 = 12.0;
const GAP: f32 = 10.0;

/// The picker for `value`. `alpha` adds the opacity bar. Every change calls `on_pick` with the
/// whole new color (dragging sends one per move; the editor merges them into one undo step).
pub fn picker<'a, M: 'a>(value: Rgba, alpha: bool, on_pick: impl Fn(Rgba) -> M + 'a) -> Element<'a, M> {
    let height = SQUARE_H + GAP + BAR_H + if alpha { GAP + BAR_H } else { 0.0 };
    canvas::Canvas::new(Picker { value, alpha, on_pick: Rc::new(on_pick) }).width(Fill).height(height).into()
}

/// A small square of `value`, over a checkerboard where it is see-through.
pub fn swatch<'a, M: 'a>(value: Rgba, size: f32) -> Element<'a, M> {
    canvas::Canvas::new(Swatch(value)).width(size).height(size).into()
}

fn checker(frame: &mut Frame, area: Rectangle, cell: f32) {
    frame.fill_rectangle(area.position(), area.size(), Color::from_rgb8(0xD8, 0xD8, 0xD8));
    let (cols, rows) = ((area.width / cell).ceil() as i32, (area.height / cell).ceil() as i32);
    for y in 0..rows {
        for x in (0..cols).filter(|x| (x + y) % 2 == 0) {
            let at = Point::new(area.x + x as f32 * cell, area.y + y as f32 * cell);
            let size = Size::new(cell.min(area.x + area.width - at.x), cell.min(area.y + area.height - at.y));
            frame.fill_rectangle(at, size, Color::from_rgb8(0x9A, 0x9A, 0x9A));
        }
    }
}

fn color(c: Rgba) -> Color {
    Color::from_rgba(c.0, c.1, c.2, c.3)
}

struct Swatch(Rgba);

impl<M> canvas::Program<M> for Swatch {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let area = Rectangle::new(Point::ORIGIN, bounds.size());
        let shape = Path::rounded_rectangle(Point::ORIGIN, bounds.size(), R_SM.into());
        if self.0.3 < 0.999 {
            // The checkerboard, clipped to the rounded square by drawing it in a clip.
            f.with_clip(area, |f| checker(f, area, (bounds.width / 4.0).max(3.0)));
        }
        f.fill(&shape, color(self.0));
        f.stroke(&shape, Stroke::default().with_width(1.0).with_color(LINE_STRONG));
        vec![f.into_geometry()]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Square,
    Hue,
    Alpha,
}

#[derive(Default)]
pub struct State {
    drag: Option<Part>,
    /// The hue/saturation/brightness last picked here. A color with no saturation (or no
    /// brightness) has no hue of its own; remembering it keeps the hue bar where the user left it.
    hsv: Option<[f32; 3]>,
    /// The square and bars, drawn again only when the hue, the color or the width changes.
    backdrop: canvas::Cache,
    drawn_for: std::cell::Cell<Option<[f32; 5]>>,
}

struct Picker<'a, M> {
    value: Rgba,
    alpha: bool,
    on_pick: Rc<dyn Fn(Rgba) -> M + 'a>,
}

impl<M> Picker<'_, M> {
    fn hsv(&self, state: &State) -> [f32; 3] {
        let rgb = [self.value.0, self.value.1, self.value.2];
        match state.hsv {
            Some(h) if hsv_to_rgb(h).iter().zip(rgb).all(|(a, b)| (a - b).abs() < 1.5 / 255.0) => h,
            _ => rgb_to_hsv(rgb),
        }
    }

    fn parts(&self, size: Size) -> [(Part, Rectangle); 3] {
        let square = Rectangle::new(Point::ORIGIN, Size::new(size.width, SQUARE_H));
        let hue = Rectangle::new(Point::new(0.0, SQUARE_H + GAP), Size::new(size.width, BAR_H));
        let alpha = Rectangle::new(Point::new(0.0, SQUARE_H + GAP * 2.0 + BAR_H), Size::new(size.width, BAR_H));
        [(Part::Square, square), (Part::Hue, hue), (Part::Alpha, alpha)]
    }

    /// The color with the pointer at `at` on `part`.
    fn pick(&self, state: &mut State, part: Part, at: Point, size: Size) -> Rgba {
        let [mut h, mut s, mut v] = self.hsv(state);
        let mut a = self.value.3;
        let rect = self.parts(size).into_iter().find(|p| p.0 == part).map(|p| p.1).unwrap_or_default();
        let fx = ((at.x - rect.x) / rect.width.max(1.0)).clamp(0.0, 1.0);
        let fy = ((at.y - rect.y) / rect.height.max(1.0)).clamp(0.0, 1.0);
        match part {
            Part::Square => (s, v) = (fx, 1.0 - fy),
            Part::Hue => h = fx * 360.0,
            Part::Alpha => a = fx,
        }
        // A color picked while invisible would show nothing: picking a shade makes it visible.
        if part != Part::Alpha && a < 0.01 {
            a = 1.0;
        }
        state.hsv = Some([h, s, v]);
        let [r, g, b] = hsv_to_rgb([h, s, v]);
        Rgba(r, g, b, a)
    }
}

impl<M> canvas::Program<M> for Picker<'_, M> {
    type State = State;

    fn update(&self, state: &mut State, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<M>> {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let at = cursor.position_in(bounds)?;
                let part = self.parts(bounds.size()).into_iter().filter(|p| self.alpha || p.0 != Part::Alpha).find(|p| p.1.expand(4.0).contains(at))?.0;
                state.drag = Some(part);
                let c = self.pick(state, part, at, bounds.size());
                Some(Action::publish((self.on_pick)(c)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let part = state.drag?;
                let at = cursor.position_from(bounds.position())?;
                let c = self.pick(state, part, at, bounds.size());
                Some(Action::publish((self.on_pick)(c)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.drag.is_some() => {
                state.drag = None;
                Some(Action::capture())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &State, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let [h, s, v] = self.hsv(state);
        let [sq, hue, al] = self.parts(bounds.size()).map(|p| p.1);
        let pure = {
            let [r, g, b] = hsv_to_rgb([h, 1.0, 1.0]);
            Color::from_rgb(r, g, b)
        };
        let [r, g, b] = hsv_to_rgb([h, s, v]);
        // Drawn from small solid cells rather than gradient fills: iced draws a gradient that a
        // scroll area cuts off as stray triangles, and the inspector scrolls. Cached, so only a
        // new hue, color or width costs anything.
        let key = [h, r, g, b, bounds.width];
        if state.drawn_for.get() != Some(key) {
            state.backdrop.clear();
            state.drawn_for.set(Some(key));
        }
        let alpha = self.alpha;
        let backdrop = state.backdrop.draw(renderer, bounds.size(), |f| {
            let outline = |f: &mut Frame, r: Rectangle| f.stroke(&Path::rectangle(r.position(), r.size()), Stroke::default().with_width(1.0).with_color(LINE));
            // The square: white to the pure hue across, bright to black down.
            let (cw, ch) = (4.0_f32, 3.0_f32);
            let (cols, rows) = ((sq.width / cw).ceil() as i32, (sq.height / ch).ceil() as i32);
            for y in 0..rows {
                let vv = 1.0 - (y as f32 + 0.5) / rows as f32;
                for x in 0..cols {
                    let ss = (x as f32 + 0.5) / cols as f32;
                    let [cr, cg, cb] = hsv_to_rgb([h, ss, vv]);
                    let at = Point::new(sq.x + x as f32 * cw, sq.y + y as f32 * ch);
                    f.fill_rectangle(at, Size::new(cw.min(sq.x + sq.width - at.x), ch.min(sq.y + sq.height - at.y)), Color::from_rgb(cr, cg, cb));
                }
            }
            outline(f, sq);
            // The hue bar: the rainbow, red to red.
            let steps = (hue.width / 2.0).ceil() as i32;
            for i in 0..steps {
                let [cr, cg, cb] = hsv_to_rgb([(i as f32 + 0.5) / steps as f32 * 360.0, 1.0, 1.0]);
                let x = hue.x + i as f32 * 2.0;
                f.fill_rectangle(Point::new(x, hue.y), Size::new(2.0_f32.min(hue.x + hue.width - x), hue.height), Color::from_rgb(cr, cg, cb));
            }
            outline(f, hue);
            // The opacity bar: the color from clear to solid, over a checkerboard.
            if alpha {
                checker(f, al, BAR_H / 2.0);
                let steps = (al.width / 2.0).ceil() as i32;
                for i in 0..steps {
                    let x = al.x + i as f32 * 2.0;
                    f.fill_rectangle(Point::new(x, al.y), Size::new(2.0_f32.min(al.x + al.width - x), al.height), Color::from_rgba(r, g, b, (i as f32 + 0.5) / steps as f32));
                }
                outline(f, al);
            }
        });

        // Handles: a ring on the square, a knob on each bar, filled with what they pick.
        let ring = |f: &mut Frame, at: Point, radius: f32, fill: Color| {
            let c = Path::circle(at, radius);
            f.fill(&c, fill);
            f.stroke(&c, Stroke::default().with_width(2.0).with_color(Color::WHITE));
            f.stroke(&Path::circle(at, radius + 1.5), Stroke::default().with_width(1.0).with_color(Color { a: 0.5, ..Color::BLACK }));
        };
        let clamp_x = |r: Rectangle, fx: f32| (r.x + fx * r.width).clamp(r.x + 6.0, r.x + r.width - 6.0);
        ring(&mut f, Point::new(sq.x + s * sq.width, sq.y + (1.0 - v) * sq.height), 6.0, Color::from_rgb(r, g, b));
        ring(&mut f, Point::new(clamp_x(hue, h / 360.0), hue.center_y()), 6.0, pure);
        if self.alpha {
            ring(&mut f, Point::new(clamp_x(al, self.value.3), al.center_y()), 6.0, Color::from_rgba(r, g, b, self.value.3.max(0.0)));
        }
        vec![backdrop, f.into_geometry()]
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if state.drag.is_some() {
            return mouse::Interaction::Grabbing;
        }
        match cursor.position_in(bounds) {
            Some(at) if self.parts(bounds.size()).into_iter().filter(|p| self.alpha || p.0 != Part::Alpha).any(|p| p.1.contains(at)) => mouse::Interaction::Crosshair,
            _ => mouse::Interaction::default(),
        }
    }
}

/// Hue in degrees (0–360), saturation and value 0–1.
pub fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h, if max < 1e-6 { 0.0 } else { d / max }, max]
}

pub fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips() {
        for rgb in [[1.0, 0.0, 0.0], [0.2, 0.6, 0.4], [0.78, 0.94, 0.19], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.1, 0.1, 0.9]] {
            let back = hsv_to_rgb(rgb_to_hsv(rgb));
            assert!(back.iter().zip(rgb).all(|(a, b)| (a - b).abs() < 1e-5), "{rgb:?} came back as {back:?}");
        }
        assert_eq!(rgb_to_hsv([0.0, 1.0, 0.0])[0], 120.0);
    }
}
