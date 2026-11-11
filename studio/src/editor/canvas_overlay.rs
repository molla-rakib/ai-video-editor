//! Direct manipulation on the preview: click to select a layer, drag to move
//! (snapping to the canvas center), corner handles to scale, the knob to rotate,
//! side handles to stretch shapes, the inner dot to round corners.
//! Edit modes show the zoom's focus rectangle or the crop window
//! over the frame, each with handles. Effects dragged from the library drop here.

use std::sync::Arc;

use iced::mouse;
use iced::widget::canvas::{self, Action, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, Vector, alignment};

use super::Message;
use super::inspector::Edit;
use super::library::Effect;
use crate::clip::{Content, Crop, Zoom};
use crate::model::{Aspect, Id, Project};
use crate::render::scene::{LayerSpec, Scene, px_per_unit};
use crate::ui::tokens::*;

const HANDLE: f32 = 9.0;
const KNOB_GAP: f32 = 26.0;
const SNAP_PX: f32 = 7.0;
/// Where the corner-radius dot sits when the corners are square (screen px from the corner).
const RADIUS_DOT_MIN: f32 = 14.0;

pub struct Overlay<'a> {
    pub project: &'a Project,
    pub scene: Arc<Scene>,
    pub selection: &'a [Id],
    /// The zoom being edited, if any: (clip, zoom).
    pub zoom_edit: Option<(Id, Zoom)>,
    /// The clip being cropped (shown uncropped) and its crop.
    pub crop_edit: Option<(Id, Crop)>,
    /// An effect being dragged in from the library.
    pub dragging: Option<Effect>,
    /// Where the canvas sits inside the overlay, and its on-screen size.
    pub origin: Vector,
    pub display: Size,
    /// The preview is zoomed in or out (not fitted), so the wheel pans it.
    pub zoomed: bool,
    /// Live 9:16/1:1 crop guides, shown while the canvas itself is 16:9.
    pub show_safe_zones: bool,
}

/// The centered rectangle `target`'s aspect ratio would occupy inside a `(canvas_w, canvas_h)`
/// canvas — how a 16:9 master re-crops to a narrower export without re-animating. In canvas
/// pixels: `(x, y, w, h)`.
fn safe_zone_rect(canvas_w: f32, canvas_h: f32, target: Aspect) -> (f32, f32, f32, f32) {
    let (tw, th) = target.ratio();
    let (tw, th) = (tw as f32, th as f32);
    let scale = (canvas_w / tw).min(canvas_h / th);
    let (w, h) = (tw * scale, th * scale);
    ((canvas_w - w) / 2.0, (canvas_h - h) / 2.0, w, h)
}

/// A grabbed part of an editable rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Body,
    /// Moves these edges: (left, top, right, bottom).
    Edges(bool, bool, bool, bool),
}

/// What a rectangle drag edits.
#[derive(Debug, Clone, Copy)]
enum RectTarget {
    Crop(Id),
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Move { clip: Id, grab: Vector, moved: bool },
    Scale { clip: Id, start_dist: f32, start_scale: f64 },
    Rotate { clip: Id, start_angle: f32, start_rotation: f64 },
    /// Resizing a shape (markers too) by a corner or side, the opposite side staying put.
    /// `start` is where it was grabbed (fractions of `from`, the layer at the press).
    ShapeBox { clip: Id, grip: Grip, start: (f32, f32), from: BoxStart },
    Radius { clip: Id },
    ZoomMove { clip: Id, zoom: Id, grab: Vector },
    ZoomScale { clip: Id, zoom: Id },
    /// Crop window; `rect` and `start` are layer fractions at the press.
    Rect { target: RectTarget, grip: Grip, rect: FRect, start: (f32, f32) },
    /// Middle-button drag moving a zoomed preview.
    Pan { last: Point },
}

/// A shape's box when a resize started.
#[derive(Debug, Clone, Copy)]
struct BoxStart {
    center: [f32; 2],
    size: [f32; 2],
    rotation: f32,
    /// The clip's base position and shape size (canvas fractions, canvas units).
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
    guides: (bool, bool),
    modifiers: iced::keyboard::Modifiers,
}

/// Canvas pixels per overlay pixel, and the conversions both ways.
struct Map {
    k: f32,
    origin: Vector,
}

impl Map {
    fn to_canvas(&self, p: Point) -> Point {
        Point::new((p.x - self.origin.x) * self.k, (p.y - self.origin.y) * self.k)
    }
    fn to_screen(&self, p: Point) -> Point {
        Point::new(p.x / self.k + self.origin.x, p.y / self.k + self.origin.y)
    }
}

/// A rectangle in fractions of a layer (0..1 across its unrotated box).
#[derive(Debug, Clone, Copy, PartialEq)]
struct FRect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl FRect {
    fn corners(&self) -> [(f32, f32); 4] {
        [(self.x0, self.y0), (self.x1, self.y0), (self.x1, self.y1), (self.x0, self.y1)]
    }

    /// Corners then edge midpoints, with the edges each one moves.
    fn grips(&self) -> [((f32, f32), Grip); 8] {
        let (mx, my) = ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0);
        [
            ((self.x0, self.y0), Grip::Edges(true, true, false, false)),
            ((self.x1, self.y0), Grip::Edges(false, true, true, false)),
            ((self.x1, self.y1), Grip::Edges(false, false, true, true)),
            ((self.x0, self.y1), Grip::Edges(true, false, false, true)),
            ((mx, self.y0), Grip::Edges(false, true, false, false)),
            ((self.x1, my), Grip::Edges(false, false, true, false)),
            ((mx, self.y1), Grip::Edges(false, false, false, true)),
            ((self.x0, my), Grip::Edges(true, false, false, false)),
        ]
    }

    fn contains(&self, (x, y): (f32, f32)) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Like `dragged`, but free to grow past 0..1 (a shape can get bigger than it was).
    fn stretched(self, grip: Grip, dx: f32, dy: f32, min: f32) -> FRect {
        let mut r = self;
        if let Grip::Edges(l, t, rt, b) = grip {
            if l {
                r.x0 = (r.x0 + dx).min(r.x1 - min);
            }
            if rt {
                r.x1 = (r.x1 + dx).max(r.x0 + min);
            }
            if t {
                r.y0 = (r.y0 + dy).min(r.y1 - min);
            }
            if b {
                r.y1 = (r.y1 + dy).max(r.y0 + min);
            }
        }
        r
    }

    /// The rectangle after dragging `grip` by (dx, dy), kept inside 0..1 and at least `min` across.
    fn dragged(self, grip: Grip, dx: f32, dy: f32, min: f32) -> FRect {
        let mut r = self;
        match grip {
            Grip::Body => {
                let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
                r.x0 = (r.x0 + dx).clamp(0.0, 1.0 - w);
                r.y0 = (r.y0 + dy).clamp(0.0, 1.0 - h);
                (r.x1, r.y1) = (r.x0 + w, r.y0 + h);
            }
            Grip::Edges(l, t, rt, b) => {
                if l {
                    r.x0 = (r.x0 + dx).clamp(0.0, r.x1 - min);
                }
                if rt {
                    r.x1 = (r.x1 + dx).clamp(r.x0 + min, 1.0);
                }
                if t {
                    r.y0 = (r.y0 + dy).clamp(0.0, r.y1 - min);
                }
                if b {
                    r.y1 = (r.y1 + dy).clamp(r.y0 + min, 1.0);
                }
            }
        }
        r
    }
}

/// A whole layer, as a rectangle of its own fractions.
const FULL: FRect = FRect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };

fn rotate(v: Vector, angle: f32) -> Vector {
    let (s, c) = angle.sin_cos();
    Vector::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Canvas point → fractions of the layer's unrotated box.
fn to_frac(l: &LayerSpec, p: Point) -> (f32, f32) {
    let v = rotate(Vector::new(p.x - l.center[0], p.y - l.center[1]), -l.rotation);
    (v.x / l.size[0].max(1e-3) + 0.5, v.y / l.size[1].max(1e-3) + 0.5)
}

/// Fractions of the layer's box → canvas point.
fn from_frac(l: &LayerSpec, (fx, fy): (f32, f32)) -> Point {
    let v = rotate(Vector::new((fx - 0.5) * l.size[0], (fy - 0.5) * l.size[1]), l.rotation);
    Point::new(l.center[0] + v.x, l.center[1] + v.y)
}

/// Whether canvas point `p` is inside the layer's rotated rectangle.
fn contains(l: &LayerSpec, p: Point) -> bool {
    let (fx, fy) = to_frac(l, p);
    (0.0..=1.0).contains(&fx) && (0.0..=1.0).contains(&fy)
}

/// Corners of a layer in canvas pixels: top-left, top-right, bottom-right, bottom-left.
fn corners(l: &LayerSpec) -> [Point; 4] {
    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|f| from_frac(l, f))
}

/// The zoom's focus rectangle in canvas pixels, inside the layer's (unrotated) bounds.
fn zoom_rect(l: &LayerSpec, z: &Zoom) -> Rectangle {
    let span = 1.0 / z.scale.max(1.0) as f32;
    let half = span / 2.0;
    let cx = (z.focus_x as f32).clamp(half, 1.0 - half);
    let cy = (z.focus_y as f32).clamp(half, 1.0 - half);
    let (x0, y0) = (l.center[0] - l.size[0] / 2.0, l.center[1] - l.size[1] / 2.0);
    Rectangle::new(Point::new(x0 + (cx - half) * l.size[0], y0 + (cy - half) * l.size[1]), Size::new(span * l.size[0], span * l.size[1]))
}

fn crop_frect(c: &Crop) -> FRect {
    FRect { x0: c.left as f32, y0: c.top as f32, x1: 1.0 - c.right as f32, y1: 1.0 - c.bottom as f32 }
}

impl Overlay<'_> {
    fn layer(&self, clip: Id) -> Option<&LayerSpec> {
        self.scene.layers.iter().find(|l| l.clip == clip)
    }

    fn selected_layer(&self) -> Option<&LayerSpec> {
        match self.selection {
            [id] => self.layer(*id),
            _ => None,
        }
    }

    fn map(&self, _bounds: Rectangle) -> Map {
        Map { k: self.scene.width as f32 / self.display.width.max(1.0), origin: self.origin }
    }

    fn knob(&self, l: &LayerSpec, map: &Map) -> Point {
        let top_mid = rotate(Vector::new(0.0, -l.size[1] / 2.0 - KNOB_GAP * map.k), l.rotation);
        Point::new(l.center[0] + top_mid.x, l.center[1] + top_mid.y)
    }

    fn locked(&self, clip: Id) -> bool {
        self.project.clip(clip).and_then(|c| self.project.track(c.track)).is_some_and(|t| t.locked)
    }

    /// Topmost unlocked layer under canvas point `p`.
    fn layer_at(&self, p: Point) -> Option<&LayerSpec> {
        self.scene.layers.iter().rev().find(|l| contains(l, p) && !self.locked(l.clip))
    }

    /// Shapes, markers, blur and mask boxes resize from every corner and side.
    fn stretchable(&self, clip: Id) -> bool {
        self.project.clip(clip).is_some_and(|c| matches!(c.content, Content::Shape(_) | Content::Blur(_) | Content::Mask(_)))
    }

    /// Media and shapes have rounded corners; text, blur and mask boxes don't.
    fn roundable(&self, clip: Id) -> bool {
        self.project.clip(clip).is_some_and(|c| !matches!(c.content, Content::Text(_) | Content::Blur(_) | Content::Mask(_)))
    }

    /// Blur and mask boxes stay upright, so they have no rotation knob.
    fn rotatable(&self, clip: Id) -> bool {
        self.project.clip(clip).is_some_and(|c| !matches!(c.content, Content::Blur(_) | Content::Mask(_)))
    }

    /// The corner-radius dot, inside the top-left corner.
    fn radius_dot(l: &LayerSpec, map: &Map) -> Point {
        let r = l.radius.max(RADIUS_DOT_MIN * map.k).min(l.size[0].min(l.size[1]) / 2.0);
        let inset = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
        from_frac(l, (inset / l.size[0].max(1.0), inset / l.size[1].max(1.0)))
    }

    /// The rectangle being edited, its target, and the layer it belongs to.
    fn edit_rect(&self) -> Option<(RectTarget, FRect, &LayerSpec)> {
        if let Some((clip, crop)) = self.crop_edit {
            return Some((RectTarget::Crop(clip), crop_frect(&crop), self.layer(clip)?));
        }
        None
    }

    /// The edit a rectangle drag produces.
    fn rect_message(&self, target: RectTarget, r: FRect) -> Option<Message> {
        Some(match target {
            RectTarget::Crop(clip) => Message::Edit(clip, Edit::Crop(Crop { left: r.x0 as f64, top: r.y0 as f64, right: (1.0 - r.x1) as f64, bottom: (1.0 - r.y1) as f64 })),
        })
    }

    /// The grip of the edited rectangle under canvas point `p`.
    fn rect_grip(&self, p: Point, map: &Map) -> Option<(RectTarget, FRect, Grip)> {
        let (target, r, l) = self.edit_rect()?;
        let near = |a: Point| a.distance(p) <= HANDLE * map.k;
        if let Some((_, grip)) = r.grips().into_iter().find(|(f, _)| near(from_frac(l, *f))) {
            return Some((target, r, grip));
        }
        r.contains(to_frac(l, p)).then_some((target, r, Grip::Body))
    }
}

impl canvas::Program<Message> for Overlay<'_> {
    type State = State;

    fn update(&self, state: &mut State, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let map = self.map(bounds);
        let (cw, ch) = (self.scene.width as f32, self.scene.height as f32);
        match event {
            canvas::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = *m;
                None
            }
            // Ctrl + wheel zooms around the cursor; the wheel pans a zoomed preview (Shift: sideways).
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let at = cursor.position_in(bounds)?;
                let (dx, dy) = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => (x * 48.0, y * 48.0),
                    mouse::ScrollDelta::Pixels { x, y } => (x, y),
                };
                if state.modifiers.command() {
                    return Some(Action::publish(Message::PreviewZoomAt(1.0025f32.powf(dy), at)).and_capture());
                }
                if !self.zoomed {
                    return None;
                }
                let d = if state.modifiers.shift() { Vector::new(dy, 0.0) } else { Vector::new(dx, dy) };
                Some(Action::publish(Message::PreviewPan(d)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) if self.zoomed => {
                state.drag = Some(Drag::Pan { last: cursor.position_in(bounds)? });
                Some(Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Middle)) if matches!(state.drag, Some(Drag::Pan { .. })) => {
                state.drag = None;
                Some(Action::capture())
            }
            // Right-click selects what's under the cursor, so the context menu acts on it.
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let p = map.to_canvas(cursor.position_in(bounds)?);
                Some(Action::publish(Message::Select(self.layer_at(p).map(|l| l.clip), false)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = map.to_canvas(cursor.position_in(bounds)?);
                let near = |a: Point| a.distance(p) <= HANDLE * map.k;
                // Zoom editing takes the clicks first.
                if let Some((clip, z)) = self.zoom_edit {
                    let l = self.layer(clip)?;
                    let r = zoom_rect(l, &z);
                    let corners = [r.position(), Point::new(r.x + r.width, r.y), Point::new(r.x + r.width, r.y + r.height), Point::new(r.x, r.y + r.height)];
                    if corners.iter().any(|&c| near(c)) {
                        state.drag = Some(Drag::ZoomScale { clip, zoom: z.id });
                        return Some(Action::capture());
                    }
                    if r.contains(p) {
                        let center = Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0);
                        state.drag = Some(Drag::ZoomMove { clip, zoom: z.id, grab: center - p });
                        return Some(Action::capture());
                    }
                    return Some(Action::capture());
                }
                if let Some((target, rect, l)) = self.edit_rect() {
                    if let Some((_, _, grip)) = self.rect_grip(p, &map) {
                        state.drag = Some(Drag::Rect { target, grip, rect, start: to_frac(l, p) });
                        return Some(Action::capture());
                    }
                    // Cropping keeps the clicks.
                    let _ = target;
                    return Some(Action::capture());
                }
                if let Some(l) = self.selected_layer() {
                    let clip = self.project.clip(l.clip)?;
                    if self.rotatable(l.clip) && near(self.knob(l, &map)) {
                        let angle = (p.y - l.center[1]).atan2(p.x - l.center[0]);
                        state.drag = Some(Drag::Rotate { clip: l.clip, start_angle: angle, start_rotation: clip.transform.rotation });
                        return Some(Action::capture());
                    }
                    if self.roundable(l.clip) && near(Self::radius_dot(l, &map)) {
                        state.drag = Some(Drag::Radius { clip: l.clip });
                        return Some(Action::capture());
                    }
                    // Shapes, markers and blur boxes resize by any corner or side.
                    let boxed = match &clip.content {
                        Content::Shape(s) => Some((s.width, s.height)),
                        Content::Blur(b) => Some((b.width, b.height)),
                        Content::Mask(m) => Some((m.width, m.height)),
                        _ => None,
                    };
                    if let Some((width, height)) = boxed {
                        if let Some((_, grip)) = FULL.grips().into_iter().find(|(f, _)| near(from_frac(l, *f))) {
                            let from = BoxStart { center: l.center, size: l.size, rotation: l.rotation, x: clip.transform.x, y: clip.transform.y, width, height };
                            state.drag = Some(Drag::ShapeBox { clip: l.clip, grip, start: to_frac(l, p), from });
                            return Some(Action::capture());
                        }
                    } else if corners(l).iter().any(|&c| near(c)) {
                        let d = Point::new(l.center[0], l.center[1]).distance(p).max(1.0);
                        state.drag = Some(Drag::Scale { clip: l.clip, start_dist: d, start_scale: clip.transform.scale });
                        return Some(Action::capture());
                    }
                }
                // The selected element keeps the drag wherever it's under the cursor, even
                // where another element (or an invisible box) lies on top of it.
                let selected = self.selected_layer().filter(|l| contains(l, p) && !self.locked(l.clip));
                match selected.or_else(|| self.layer_at(p)) {
                    Some(l) => {
                        let clip = self.project.clip(l.clip)?;
                        let base = Point::new(clip.transform.x as f32 * cw, clip.transform.y as f32 * ch);
                        state.drag = Some(Drag::Move { clip: l.clip, grab: base - p, moved: false });
                        Some(Action::publish(Message::Select(Some(l.clip), false)).and_capture())
                    }
                    None => Some(Action::publish(Message::Select(None, false)).and_capture()),
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let local = Point::new(position.x - bounds.x, position.y - bounds.y);
                // Keep the drop highlight following the cursor while an effect is dragged in.
                let Some(drag) = state.drag.as_mut() else { return self.dragging.map(|_| Action::request_redraw()) };
                let p = map.to_canvas(local);
                let msg = match drag {
                    Drag::Pan { last } => {
                        let d = local - *last;
                        *last = local;
                        Message::PreviewPan(d)
                    }
                    Drag::Move { clip, grab, moved } => {
                        *moved = true;
                        let mut target = p + *grab;
                        let snap = SNAP_PX * map.k;
                        state.guides = ((target.x - cw / 2.0).abs() < snap, (target.y - ch / 2.0).abs() < snap);
                        if state.guides.0 {
                            target.x = cw / 2.0;
                        }
                        if state.guides.1 {
                            target.y = ch / 2.0;
                        }
                        Message::Edit(*clip, Edit::Position((target.x / cw) as f64, (target.y / ch) as f64))
                    }
                    Drag::Scale { clip, start_dist, start_scale } => {
                        let l = self.layer(*clip)?;
                        let d = Point::new(l.center[0], l.center[1]).distance(p);
                        Message::Edit(*clip, Edit::Scale((*start_scale * (d / *start_dist) as f64).clamp(0.02, 10.0)))
                    }
                    Drag::Rotate { clip, start_angle, start_rotation } => {
                        let l = self.layer(*clip)?;
                        let angle = (p.y - l.center[1]).atan2(p.x - l.center[0]);
                        let mut deg = *start_rotation + (angle - *start_angle).to_degrees() as f64;
                        deg = ((deg + 180.0).rem_euclid(360.0)) - 180.0;
                        // Snap to 15° steps when close.
                        let snapped = (deg / 15.0).round() * 15.0;
                        if (snapped - deg).abs() < 2.0 {
                            deg = snapped;
                        }
                        Message::Edit(*clip, Edit::Rotation(deg))
                    }
                    Drag::ShapeBox { clip, grip, start, from } => {
                        let (grip, start, from) = (*grip, *start, *from);
                        // Work in the box as it was at the press, so the math doesn't chase itself.
                        let l0 = LayerSpec { center: from.center, size: from.size, rotation: from.rotation, ..self.layer(*clip)?.clone() };
                        let (fx, fy) = to_frac(&l0, p);
                        let min = (12.0 / from.size[0].min(from.size[1]).max(1.0)).min(0.5);
                        let mut r = FULL.stretched(grip, fx - start.0, fy - start.1, min);
                        // Shift on a corner keeps the proportions.
                        if state.modifiers.shift() {
                            if let Grip::Edges(l, t, rt, b) = grip {
                                if (l || rt) && (t || b) {
                                    let k = (r.x1 - r.x0).max(r.y1 - r.y0);
                                    if l { r.x0 = r.x1 - k } else { r.x1 = r.x0 + k }
                                    if t { r.y0 = r.y1 - k } else { r.y1 = r.y0 + k }
                                }
                            }
                        }
                        let center = from_frac(&l0, ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0));
                        let (dx, dy) = ((center.x - from.center[0]) / cw, (center.y - from.center[1]) / ch);
                        let (w, h) = ((from.width * (r.x1 - r.x0) as f64).round(), (from.height * (r.y1 - r.y0) as f64).round());
                        Message::Edit(*clip, Edit::ShapeBox(w, h, from.x + dx as f64, from.y + dy as f64))
                    }
                    Drag::Radius { clip } => {
                        let l = self.layer(*clip)?;
                        let c = self.project.clip(*clip)?;
                        let (fx, fy) = to_frac(l, p);
                        // Distance in from the corner along the diagonal, as a radius.
                        let inset = (fx * l.size[0]).min(fy * l.size[1]).max(0.0);
                        let r = inset / (1.0 - std::f32::consts::FRAC_1_SQRT_2);
                        let r = if r < RADIUS_DOT_MIN * map.k * 1.05 { 0.0 } else { r.min(l.size[0].min(l.size[1]) / 2.0) };
                        let units = px_per_unit(self.scene.width, self.scene.height) * c.transform.scale.max(0.01) as f32;
                        Message::Edit(*clip, Edit::Radius((r / units).clamp(0.0, 1000.0).round() as f64))
                    }
                    Drag::ZoomMove { clip, zoom, grab } => {
                        let l = self.layer(*clip)?;
                        let c = p + *grab;
                        let (x0, y0) = (l.center[0] - l.size[0] / 2.0, l.center[1] - l.size[1] / 2.0);
                        let fx = ((c.x - x0) / l.size[0]).clamp(0.0, 1.0) as f64;
                        let fy = ((c.y - y0) / l.size[1]).clamp(0.0, 1.0) as f64;
                        Message::Edit(*clip, Edit::ZoomFocus(*zoom, fx, fy))
                    }
                    Drag::ZoomScale { clip, zoom } => {
                        let l = self.layer(*clip)?;
                        let z = self.project.clip(*clip)?.zooms.iter().find(|z| z.id == *zoom)?;
                        let r = zoom_rect(l, z);
                        let center = Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0);
                        // Half-size from the cursor, keeping the frame's aspect ratio.
                        let half_span = ((p.x - center.x).abs() / l.size[0]).max((p.y - center.y).abs() / l.size[1]);
                        let scale = (1.0 / (half_span * 2.0).max(0.05)).clamp(1.0, 6.0) as f64;
                        Message::Edit(*clip, Edit::ZoomScale(*zoom, scale))
                    }
                    Drag::Rect { target, grip, rect, start } => {
                        let (target, grip, rect, start) = (*target, *grip, *rect, *start);
                        let clip = match target {
                            RectTarget::Crop(c) => c,
                        };
                        let (fx, fy) = to_frac(self.layer(clip)?, p);
                        let min = 1.0 - Crop::MAX as f32;
                        let moved = rect.dragged(grip, fx - start.0, fy - start.1, min);
                        self.rect_message(target, moved)?
                    }
                };
                Some(Action::publish(msg).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.guides = (false, false);
                // An effect dragged from the library lands on the layer under the cursor.
                if let Some(effect) = self.dragging {
                    let local = cursor.position_in(bounds)?;
                    let p = map.to_canvas(local);
                    let clip = self.layer_at(p).map(|l| l.clip);
                    let at = ((p.x / cw).clamp(0.0, 1.0) as f64, (p.y / ch).clamp(0.0, 1.0) as f64);
                    return Some(Action::publish(Message::DropEffect { effect, target: super::DropTarget::Canvas { clip, at } }).and_capture());
                }
                state.drag.take().map(|_| Action::request_redraw().and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &State, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, cursor: mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let map = self.map(bounds);
        let s = |p: Point| map.to_screen(p);
        // Hairline around the canvas.
        f.stroke(&Path::rectangle(Point::new(self.origin.x - 0.5, self.origin.y - 0.5), Size::new(self.display.width + 1.0, self.display.height + 1.0)), Stroke::default().with_color(CANVAS_BORDER).with_width(1.0));

        if self.show_safe_zones && self.project.canvas.aspect == Aspect::Landscape {
            let (cw, ch) = (self.scene.width as f32, self.scene.height as f32);
            let dashed = |color: Color| Stroke { line_dash: canvas::LineDash { segments: &[6.0, 5.0], offset: 0 }, ..Stroke::default().with_color(color).with_width(1.5) };
            for (target, color, label) in [(Aspect::Portrait, Color { a: 0.8, ..Color::WHITE }, "9:16"), (Aspect::Square, Color { a: 0.8, ..BRAND_2 }, "1:1")] {
                let (x, y, w, h) = safe_zone_rect(cw, ch, target);
                let (a, b) = (s(Point::new(x, y)), s(Point::new(x + w, y + h)));
                let rect = Path::rectangle(a, Size::new(b.x - a.x, b.y - a.y));
                f.stroke(&rect, dashed(color));
                chip(&mut f, a, label, color);
            }
        }

        if let Some((clip, z)) = self.zoom_edit {
            if let Some(l) = self.layer(clip) {
                let r = zoom_rect(l, &z);
                let (a, b) = (s(r.position()), s(Point::new(r.x + r.width, r.y + r.height)));
                // Dim the canvas outside the focus area.
                let dim = Color { a: 0.55, ..Color::BLACK };
                let (ox, oy, w, h) = (self.origin.x, self.origin.y, self.display.width, self.display.height);
                let (ax, ay, bx, by) = (a.x.clamp(ox, ox + w), a.y.clamp(oy, oy + h), b.x.clamp(ox, ox + w), b.y.clamp(oy, oy + h));
                f.fill_rectangle(Point::new(ox, oy), Size::new(w, ay - oy), dim);
                f.fill_rectangle(Point::new(ox, by), Size::new(w, oy + h - by), dim);
                f.fill_rectangle(Point::new(ox, ay), Size::new(ax - ox, by - ay), dim);
                f.fill_rectangle(Point::new(bx, ay), Size::new(ox + w - bx, by - ay), dim);
                let rect = Path::rectangle(a, Size::new(b.x - a.x, b.y - a.y));
                f.stroke(&rect, Stroke::default().with_color(BRAND_2).with_width(2.0));
                for c in [a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)] {
                    handle(&mut f, c, BRAND_2);
                }
                chip(&mut f, a, &format!("Zoom {:.0}%", z.scale * 100.0), BRAND_2);
            }
            return vec![f.into_geometry()];
        }

        if let Some((target, r, l)) = self.edit_rect() {
            let quad = r.corners().map(|c| s(from_frac(l, c)));
            let color = ACCENT;
            {
                // Dim the part that's cut away: the whole frame minus the window (even-odd).
                let frame = corners(l).map(s);
                let cut = Path::new(|b| {
                    for poly in [&frame, &quad] {
                        b.move_to(poly[0]);
                        for p in &poly[1..] {
                            b.line_to(*p);
                        }
                        b.close();
                    }
                });
                f.fill(&cut, canvas::Fill { style: canvas::Style::Solid(Color { a: 0.6, ..Color::BLACK }), rule: canvas::fill::Rule::EvenOdd });
                // Rule-of-thirds guides inside the window.
                let guide = Stroke::default().with_color(Color { a: 0.35, ..Color::WHITE }).with_width(1.0);
                for k in [1.0 / 3.0, 2.0 / 3.0] {
                    let x = r.x0 + (r.x1 - r.x0) * k;
                    let y = r.y0 + (r.y1 - r.y0) * k;
                    f.stroke(&Path::line(s(from_frac(l, (x, r.y0))), s(from_frac(l, (x, r.y1)))), guide);
                    f.stroke(&Path::line(s(from_frac(l, (r.x0, y))), s(from_frac(l, (r.x1, y)))), guide);
                }
            }
            f.stroke(&poly(&quad), Stroke::default().with_color(color).with_width(2.0));
            for (frac, _) in r.grips() {
                handle(&mut f, s(from_frac(l, frac)), color);
            }
            let label = match target {
                RectTarget::Crop(_) => "Crop".to_string(),
            };
            let top_left = quad.iter().copied().fold(Point::new(f32::MAX, f32::MAX), |m, p| Point::new(m.x.min(p.x), m.y.min(p.y)));
            chip(&mut f, top_left, &label, color);
            return vec![f.into_geometry()];
        }

        // Where a dragged effect would land.
        if self.dragging.is_some() {
            if let Some(l) = cursor.position_in(bounds).map(|p| map.to_canvas(p)).and_then(|p| self.layer_at(p)) {
                let c = corners(l).map(s);
                f.fill(&poly(&c), Color { a: 0.12, ..ACCENT });
                f.stroke(&poly(&c), Stroke::default().with_color(ACCENT).with_width(2.0));
            }
            return vec![f.into_geometry()];
        }

        // Blur boxes are invisible layers: show where each one is, dashed.
        for l in self.scene.layers.iter().filter(|l| l.blur_box.is_some() && !self.selection.contains(&l.clip)) {
            let dashed = Stroke { line_dash: canvas::LineDash { segments: &[5.0, 4.0], offset: 0 }, ..Stroke::default().with_color(Color { a: 0.8, ..clip::BLUR }).with_width(1.2) };
            let oval = l.blur_box.is_some_and(|b| b[1] > 0.5);
            f.stroke(&if oval { ellipse(l, FULL, &s) } else { poly(&corners(l).map(s)) }, dashed);
        }

        // Hover outline for layers that aren't selected.
        if state.drag.is_none() {
            if let Some(p) = cursor.position_in(bounds).map(|p| map.to_canvas(p)) {
                if let Some(l) = self.scene.layers.iter().rev().find(|l| contains(l, p)) {
                    if !self.selection.contains(&l.clip) {
                        let c = corners(l).map(s);
                        f.stroke(&poly(&c), Stroke::default().with_color(Color { a: 0.6, ..ACCENT }).with_width(1.0));
                    }
                }
            }
        }

        if let Some(l) = self.selected_layer() {
            let c = corners(l).map(s);
            f.stroke(&poly(&c), Stroke::default().with_color(ACCENT).with_width(1.5));
            // Shapes (and markers) resize from every corner and side; other layers scale from the corners.
            if self.stretchable(l.clip) {
                for (frac, _) in FULL.grips() {
                    handle(&mut f, s(from_frac(l, frac)), ACCENT);
                }
            } else {
                for p in c {
                    handle(&mut f, p, ACCENT);
                }
            }
            if self.roundable(l.clip) && l.size[0].min(l.size[1]) / map.k > 60.0 {
                let dot = s(Self::radius_dot(l, &map));
                f.fill(&Path::circle(dot, 4.0), Color { a: 0.95, ..ACCENT });
                f.stroke(&Path::circle(dot, 4.0), Stroke::default().with_color(ON_ACCENT).with_width(1.0));
            }
            if self.rotatable(l.clip) {
                let knob = s(self.knob(l, &map));
                let top_mid = Point::new((c[0].x + c[1].x) / 2.0, (c[0].y + c[1].y) / 2.0);
                f.stroke(&Path::line(top_mid, knob), Stroke::default().with_color(ACCENT).with_width(1.0));
                f.fill(&Path::circle(knob, 5.0), Color::WHITE);
                f.stroke(&Path::circle(knob, 5.0), Stroke::default().with_color(ACCENT).with_width(1.5));
            }
            let size = self.project.clip(l.clip).and_then(|c| match &c.content {
                Content::Shape(sh) => Some((sh.width, sh.height)),
                Content::Blur(b) => Some((b.width, b.height)),
                Content::Mask(m) => Some((m.width, m.height)),
                _ => None,
            });
            if let (Some(Drag::ShapeBox { .. }), Some((w, h))) = (state.drag, size) {
                chip(&mut f, Point::new(c[0].x.min(c[3].x), c[0].y.min(c[1].y)), &format!("{w:.0} × {h:.0}"), ACCENT);
            }
            if let Some(Drag::Radius { .. }) = state.drag {
                chip(&mut f, Point::new(c[0].x, c[0].y), &format!("Corners {:.0}", self.project.clip(l.clip).map(|c| c.look.corner_radius).unwrap_or(0.0)), ACCENT);
            }
        }

        let (gx, gy) = state.guides;
        let (ox, oy, w, h) = (self.origin.x, self.origin.y, self.display.width, self.display.height);
        if gx {
            f.fill_rectangle(Point::new(ox + w / 2.0 - 0.5, oy), Size::new(1.0, h), BRAND_2);
        }
        if gy {
            f.fill_rectangle(Point::new(ox, oy + h / 2.0 - 0.5), Size::new(w, 1.0), BRAND_2);
        }
        vec![f.into_geometry()]
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.drag {
            Some(Drag::Move { moved: true, .. } | Drag::ZoomMove { .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Scale { .. } | Drag::ZoomScale { .. }) => return mouse::Interaction::ResizingDiagonallyDown,
            Some(Drag::Rotate { .. } | Drag::Pan { .. } | Drag::Radius { .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Rect { grip, .. } | Drag::ShapeBox { grip, .. }) => return grip_cursor(grip),
            _ => {}
        }
        if self.dragging.is_some() {
            return mouse::Interaction::Copy;
        }
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        let map = self.map(bounds);
        let p = map.to_canvas(p);
        let near = |a: Point| a.distance(p) <= HANDLE * map.k;
        if let Some((clip, z)) = self.zoom_edit {
            if let Some(l) = self.layer(clip) {
                let r = zoom_rect(l, &z);
                let cs = [r.position(), Point::new(r.x + r.width, r.y), Point::new(r.x + r.width, r.y + r.height), Point::new(r.x, r.y + r.height)];
                if cs.iter().any(|&c| near(c)) {
                    return mouse::Interaction::ResizingDiagonallyDown;
                }
                if r.contains(p) {
                    return mouse::Interaction::Grab;
                }
            }
            return mouse::Interaction::default();
        }
        if self.edit_rect().is_some() {
            return self.rect_grip(p, &map).map(|(_, _, g)| grip_cursor(g)).unwrap_or_default();
        }
        if let Some(l) = self.selected_layer() {
            if self.rotatable(l.clip) && near(self.knob(l, &map)) {
                return mouse::Interaction::Grab;
            }
            if self.roundable(l.clip) && near(Self::radius_dot(l, &map)) {
                return mouse::Interaction::Grab;
            }
            if self.stretchable(l.clip) {
                if let Some((_, grip)) = FULL.grips().into_iter().find(|(f, _)| near(from_frac(l, *f))) {
                    return grip_cursor(grip);
                }
            } else if corners(l).iter().any(|&c| near(c)) {
                return mouse::Interaction::ResizingDiagonallyDown;
            }
        }
        if self.scene.layers.iter().any(|l| contains(l, p)) { mouse::Interaction::Move } else { mouse::Interaction::default() }
    }
}

fn grip_cursor(grip: Grip) -> mouse::Interaction {
    match grip {
        Grip::Body => mouse::Interaction::Grab,
        Grip::Edges(l, t, r, b) if (l || r) && (t || b) => mouse::Interaction::ResizingDiagonallyDown,
        Grip::Edges(l, _, r, _) if l || r => mouse::Interaction::ResizingHorizontally,
        Grip::Edges(..) => mouse::Interaction::ResizingVertically,
    }
}

fn poly(points: &[Point; 4]) -> Path {
    Path::new(|b| {
        b.move_to(points[0]);
        for p in &points[1..] {
            b.line_to(*p);
        }
        b.close();
    })
}

/// The ellipse inscribed in a layer-fraction rectangle, in screen space.
fn ellipse(l: &LayerSpec, r: FRect, s: &dyn Fn(Point) -> Point) -> Path {
    let (cx, cy, rx, ry) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0, (r.x1 - r.x0) / 2.0, (r.y1 - r.y0) / 2.0);
    Path::new(|b| {
        for i in 0..=48 {
            let a = i as f32 / 48.0 * std::f32::consts::TAU;
            let p = s(from_frac(l, (cx + rx * a.cos(), cy + ry * a.sin())));
            if i == 0 { b.move_to(p) } else { b.line_to(p) }
        }
    })
}

fn handle(f: &mut Frame, at: Point, color: Color) {
    let r = Path::rounded_rectangle(Point::new(at.x - 4.5, at.y - 4.5), Size::new(9.0, 9.0), 2.0.into());
    f.fill(&r, Color::WHITE);
    f.stroke(&r, Stroke::default().with_color(color).with_width(1.5));
}

/// A small label above a point.
fn chip(f: &mut Frame, at: Point, label: &str, color: Color) {
    let w = 14.0 + label.len() as f32 * 6.4;
    let chip = Path::rounded_rectangle(Point::new(at.x, at.y - 24.0), Size::new(w, 19.0), 5.0.into());
    f.fill(&chip, color);
    f.fill_text(Text { content: label.to_string(), position: Point::new(at.x + w / 2.0, at.y - 14.5), color: ON_ACCENT, size: 11.0.into(), font: INTER_SEMIBOLD, align_x: alignment::Horizontal::Center.into(), align_y: alignment::Vertical::Center, ..Text::default() });
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: FRect = FRect { x0: 0.2, y0: 0.2, x1: 0.6, y1: 0.5 };

    #[test]
    fn moving_a_rectangle_keeps_its_size_inside_the_frame() {
        let r = R.dragged(Grip::Body, 0.9, -0.9, 0.05);
        assert!((r.x1 - r.x0 - 0.4).abs() < 1e-6 && (r.y1 - r.y0 - 0.3).abs() < 1e-6);
        assert_eq!((r.x1, r.y0), (1.0, 0.0), "stopped at the edges");
    }

    #[test]
    fn edge_handles_move_only_their_edges_and_keep_a_minimum() {
        let r = R.dragged(Grip::Edges(true, false, false, false), -0.5, 0.3, 0.05);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (0.0, 0.2, 0.6, 0.5), "the left edge stops at 0; the others don't move");
        let r = R.dragged(Grip::Edges(false, false, true, true), -1.0, -1.0, 0.05);
        assert!((r.x1 - (R.x0 + 0.05)).abs() < 1e-6 && (r.y1 - (R.y0 + 0.05)).abs() < 1e-6, "can't turn inside out");
    }

    #[test]
    fn safe_zone_is_centered_and_sized_to_the_target_aspect() {
        let (x, y, w, h) = safe_zone_rect(1920.0, 1080.0, Aspect::Portrait);
        assert!((w / h - 9.0 / 16.0).abs() < 1e-3, "keeps the 9:16 ratio, got {w}x{h}");
        assert!((h - 1080.0).abs() < 1e-3, "the full height fits inside a 16:9 canvas");
        assert!((x - (1920.0 - w) / 2.0).abs() < 1e-3 && (y - 0.0).abs() < 1e-3, "centered horizontally, flush top/bottom");

        let (x, y, w, h) = safe_zone_rect(1920.0, 1080.0, Aspect::Square);
        assert!((w - h).abs() < 1e-3, "1:1 is actually square");
        assert!((w - 1080.0).abs() < 1e-3, "limited by the shorter side");
        assert!((x - (1920.0 - w) / 2.0).abs() < 1e-3 && (y - (1080.0 - h) / 2.0).abs() < 1e-3, "centered both ways");
    }

    #[test]
    fn shapes_stretch_past_their_old_box_with_the_far_side_fixed() {
        let r = FULL.stretched(Grip::Edges(false, false, true, false), 0.5, 0.3, 0.05);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (0.0, 0.0, 1.5, 1.0), "only the right side moves, past 1");
        let r = FULL.stretched(Grip::Edges(true, true, false, false), 2.0, 2.0, 0.05);
        assert_eq!((r.x0, r.y0), (0.95, 0.95), "can't flip past the far corner");
    }

    fn layer(clip: Id, center: [f32; 2], size: [f32; 2]) -> LayerSpec {
        LayerSpec {
            clip, slot: 1, frame: None, center, size, rotation: 0.0, opacity: 1.0, radius: 0.0, border: 0.0, border_color: [0.0; 4],
            shadow_blur: 0.0, shadow_y: 0.0, shadow_color: [0.0; 4], uv: [0.0, 0.0, 1.0, 1.0], tint_top: [1.0; 4], tint_bottom: [1.0; 4], grade: [0.0; 4], grade2: [0.0; 4], grade3: [0.0; 4], grade4: [0.0; 4], shape: [0.0; 4], blurs: [[0.0; 8]; crate::clip::MAX_BLURS], masks: [[0.0; 8]; crate::clip::MAX_MASKS], blurrable: false, blur_box: None, mask_box: None,
        }
    }

    /// Presses the left button at canvas point (x, y) and says which clip the drag moves.
    fn press(project: &Project, layers: Vec<LayerSpec>, selection: &[Id], x: f32, y: f32) -> Option<Id> {
        let scene = Arc::new(Scene { width: 1920, height: 1080, background: [0.0; 4], layers });
        let overlay = Overlay { project, scene, selection, zoom_edit: None, crop_edit: None, dragging: None, origin: Vector::ZERO, display: Size::new(1920.0, 1080.0), zoomed: false, show_safe_zones: false };
        let mut state = State::default();
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(1920.0, 1080.0));
        let _ = canvas::Program::update(&overlay, &mut state, &canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)), bounds, mouse::Cursor::Available(Point::new(x, y)));
        match state.drag {
            Some(Drag::Move { clip, .. }) => Some(clip),
            _ => None,
        }
    }

    #[test]
    fn the_selected_element_moves_even_under_another_one() {
        let mut p = Project::new("t", Default::default());
        let text = p.add_text_clip(None, 0.0, 5.0, crate::clip::TextLayer::default()).unwrap();
        let card = p.add_shape_clip(None, 0.0, 5.0, crate::clip::ShapeLayer::new(crate::clip::ShapeKind::Rectangle, 1200.0, 400.0, crate::clip::Rgba::WHITE, None)).unwrap();
        // The card is drawn over the text's lower half (later layers are on top).
        let layers = || vec![layer(text, [960.0, 540.0], [800.0, 200.0]), layer(card, [960.0, 700.0], [1200.0, 400.0])];
        assert_eq!(press(&p, layers(), &[], 960.0, 600.0), Some(card), "nothing selected: the top element");
        assert_eq!(press(&p, layers(), &[text], 960.0, 600.0), Some(text), "the selected text keeps the drag");
        assert_eq!(press(&p, layers(), &[text], 960.0, 850.0), Some(card), "outside the text: what's under the cursor");
        // A locked selected element doesn't take the drag.
        let track = p.clip(text).unwrap().track;
        p.track_mut(track).unwrap().locked = true;
        assert_eq!(press(&p, layers(), &[text], 960.0, 600.0), Some(card));
    }

    #[test]
    fn layer_fractions_round_trip_through_rotation() {
        let l = LayerSpec { rotation: 0.7, ..layer(1, [500.0, 300.0], [400.0, 200.0]) };
        let p = from_frac(&l, (0.25, 0.8));
        let (fx, fy) = to_frac(&l, p);
        assert!((fx - 0.25).abs() < 1e-4 && (fy - 0.8).abs() < 1e-4);
    }
}
