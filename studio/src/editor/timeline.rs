//! The timeline: ruler and track lanes drawn on canvases, with all mouse interaction.
//!
//! Wheel scrolls tracks vertically (or time, when they all fit), Shift + wheel
//! scrolls time, Ctrl + wheel zooms around the cursor.

use iced::keyboard;
use iced::mouse::{self, ScrollDelta};
use iced::widget::canvas::{self, Action, Cache, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, alignment};

use super::media_cache::{MediaCache, PEAKS_PER_SECOND};
use super::Message;
use crate::clip::{Clip, Content};
use crate::edit::Edge;
use crate::model::{Id, MediaKind, Project, TrackKind};
use crate::ui::icons::code;
use crate::ui::tokens::*;

pub const RULER_H: f32 = 30.0;
pub const HEADER_W: f32 = 210.0;
pub const TOP_PAD: f32 = 6.0;
pub const GAP: f32 = 5.0;
const EDGE_GRAB: f32 = 7.0;
const SNAP_PX: f32 = 9.0;
const ZOOM_BAND_H: f32 = 15.0;

pub const MIN_TRACK_H: f32 = 30.0;
pub const MAX_TRACK_H: f32 = 220.0;

pub fn default_track_height(kind: TrackKind) -> f32 {
    match kind {
        TrackKind::Visual => 48.0,
        TrackKind::Audio => 38.0,
    }
}

pub fn track_height(track: &crate::model::Track) -> f32 {
    track.height.unwrap_or_else(|| default_track_height(track.kind)).clamp(MIN_TRACK_H, MAX_TRACK_H)
}

/// Horizontal view of the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Seconds at the left edge.
    pub scroll: f64,
    /// Pixels per second.
    pub zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        View { scroll: 0.0, zoom: 48.0 }
    }
}

impl View {
    pub fn x(&self, t: f64) -> f32 {
        ((t - self.scroll) * self.zoom) as f32
    }

    pub fn t(&self, x: f32) -> f64 {
        self.scroll + x as f64 / self.zoom
    }

    pub fn zoomed(self, factor: f64, anchor_x: f32) -> View {
        let anchor = self.t(anchor_x);
        let zoom = (self.zoom * factor).clamp(2.0, 2400.0);
        View { zoom, scroll: (anchor - anchor_x as f64 / zoom).max(0.0) }
    }
}

/// Track rows: (track id, kind, top y, height), with the list scrolled up by `vscroll`.
pub fn rows(project: &Project, vscroll: f32) -> Vec<(Id, TrackKind, f32, f32)> {
    let mut y = TOP_PAD - vscroll;
    project
        .tracks
        .iter()
        .map(|t| {
            let h = track_height(t);
            let row = (t.id, t.kind, y, h);
            y += h + GAP;
            row
        })
        .collect()
}

pub fn content_height(project: &Project) -> f32 {
    TOP_PAD + project.tracks.iter().map(|t| track_height(t) + GAP).sum::<f32>() + 24.0
}

fn clip_color(project: &Project, clip: &Clip) -> Color {
    match &clip.content {
        Content::Media { .. } if clip.part == crate::clip::Part::Sound => clip::AUDIO,
        Content::Media { asset, .. } => match project.asset(*asset).map(|a| a.kind) {
            Some(MediaKind::Audio) => clip::AUDIO,
            Some(MediaKind::Image) => clip::IMAGE,
            _ => clip::VIDEO,
        },
        Content::Text(_) => clip::TEXT,
        Content::Shape(_) => clip::EFFECT,
        Content::Blur(_) => clip::BLUR,
        Content::Mask(_) => clip::MASK,
        Content::Callout(_) => clip::EFFECT,
        Content::Counter(_) => clip::TEXT,
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }
}

pub fn clip_label(project: &Project, clip: &Clip) -> String {
    if let Some(n) = &clip.name {
        return n.clone();
    }
    match &clip.content {
        Content::Media { asset, .. } => project.asset(*asset).map(|a| a.name.clone()).unwrap_or_else(|| "Missing media".into()),
        Content::Text(t) => t.text.lines().next().unwrap_or("Text").to_string(),
        Content::Shape(_) => "Shape".into(),
        Content::Blur(_) => "Blur".into(),
        Content::Mask(_) => "Mask".into(),
        Content::Callout(c) => format!("Callout · {}", c.icon.label()),
        Content::Counter(c) => format!("Counter · {:.0}→{:.0}", c.from, c.to),
    }
}

pub fn fmt_ruler(t: f64, fine: bool) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    if fine { format!("{m}:{s:05.2}") } else { format!("{m}:{:02}", s.floor() as u64) }
}

// ---------------------------------------------------------------- ruler

pub struct Ruler<'a> {
    pub view: View,
    pub markers: &'a [crate::model::Marker],
    pub selected_marker: Option<Id>,
    pub playhead: f64,
    pub range: Option<(f64, f64)>,
    /// The Range tool is on: a press on the ruler starts a new range instead of scrubbing.
    pub range_mode: bool,
}

impl Ruler<'_> {
    /// The marker under ruler point `p`: point markers by their pennant, spans by their band.
    fn marker_at(&self, p: Point) -> Option<&crate::model::Marker> {
        if p.y < MARKER_TOP - 2.0 {
            return None;
        }
        self.markers.iter().rev().find(|m| {
            let x = self.view.x(m.at);
            (p.x - x).abs() <= 6.0 || (m.duration > 0.0 && p.x >= x && p.x <= self.view.x(m.at + m.duration) && p.y >= RULER_H - 8.0)
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum RulerDrag {
    Scrub,
    /// Dragging one end of the range; `fixed` is the other end.
    RangeEdge { fixed: f64 },
    /// Drawing a new range from `from` (Range tool on, or Shift+drag).
    RangeNew { from: f64 },
    /// Moving a marker; `grab` is how far after its start it was picked up.
    Marker { id: Id, grab: f64, moved: bool },
}

#[derive(Default)]
pub struct RulerState {
    drag: Option<RulerDrag>,
    modifiers: keyboard::Modifiers,
}

/// Distance (px) at which a range flag can be grabbed.
const FLAG_GRAB: f32 = 7.0;

impl canvas::Program<Message> for Ruler<'_> {
    type State = RulerState;

    fn update(&self, state: &mut RulerState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        match event {
            canvas::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = *m;
                None
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                let t = self.view.t(p.x).max(0.0);
                if let Some(m) = self.marker_at(p) {
                    state.drag = Some(RulerDrag::Marker { id: m.id, grab: t - m.at, moved: false });
                    return Some(Action::publish(Message::SelectMarker(Some(m.id))).and_capture());
                }
                // The edges of the range can be dragged to adjust it.
                if let Some((a, b)) = self.range {
                    let (da, db) = ((self.view.x(a) - p.x).abs(), (self.view.x(b) - p.x).abs());
                    if da.min(db) <= FLAG_GRAB {
                        state.drag = Some(RulerDrag::RangeEdge { fixed: if da <= db { b } else { a } });
                        return Some(Action::capture());
                    }
                }
                if self.range_mode || state.modifiers.shift() {
                    state.drag = Some(RulerDrag::RangeNew { from: t });
                    return Some(Action::publish(Message::RangePress { clip: None }).and_capture());
                }
                state.drag = Some(RulerDrag::Scrub);
                Some(Action::publish(Message::Seek(t)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) if state.drag.is_some() => {
                let t = self.view.t(position.x - bounds.x).max(0.0);
                let msg = match state.drag? {
                    RulerDrag::Scrub => Message::Seek(t),
                    RulerDrag::Marker { id, grab, .. } => {
                        state.drag = Some(RulerDrag::Marker { id, grab, moved: true });
                        Message::MoveMarker(id, (t - grab).max(0.0))
                    }
                    RulerDrag::RangeEdge { fixed } | RulerDrag::RangeNew { from: fixed } => {
                        if (t - fixed).abs() < 0.05 {
                            return Some(Action::capture());
                        }
                        Message::SetRange(Some((t.min(fixed), t.max(fixed))))
                    }
                };
                Some(Action::publish(msg).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.drag.is_some() => {
                // A click on a marker (no drag) jumps to it.
                if let Some(RulerDrag::Marker { id, moved: false, .. }) = state.drag.take() {
                    if let Some(m) = self.markers.iter().find(|m| m.id == id) {
                        return Some(Action::publish(Message::Seek(m.at)).and_capture());
                    }
                }
                Some(Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                let (dx, dy) = wheel(*delta);
                let view = if state.modifiers.command() { self.view.zoomed(1.0025f64.powf(dy as f64), p.x) } else { scrolled(self.view, dx + dy) };
                Some(Action::publish(Message::SetView(view)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, _state: &RulerState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let mut f = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        f.fill_rectangle(Point::ORIGIN, size, SURFACE_1);
        f.fill_rectangle(Point::new(0.0, size.height - 1.0), Size::new(size.width, 1.0), LINE);
        if let Some((a, b)) = self.range {
            let (x0, x1) = (self.view.x(a), self.view.x(b));
            f.fill_rectangle(Point::new(x0, 0.0), Size::new(x1 - x0, size.height), Color { a: 0.18, ..ACCENT });
            for x in [x0, x1] {
                edge_handle(&mut f, x);
            }
        }
        let steps = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0];
        let step = steps.iter().copied().find(|s| s * self.view.zoom >= 84.0).unwrap_or(1800.0);
        let minor = step / 5.0;
        let first = (self.view.scroll / minor).floor() as i64;
        let last = ((self.view.scroll + size.width as f64 / self.view.zoom) / minor).ceil() as i64;
        for k in first.max(0)..=last {
            let t = k as f64 * minor;
            let x = self.view.x(t).round();
            let major = k % 5 == 0;
            let h = if major { 9.0 } else { 4.0 };
            f.fill_rectangle(Point::new(x, size.height - 1.0 - h), Size::new(1.0, h), if major { TEXT_3 } else { TEXT_4 });
            if major {
                f.fill_text(Text { content: fmt_ruler(t, step < 1.0), position: Point::new(x + 5.0, 7.0), color: TEXT_3, size: 10.5.into(), font: INTER_MEDIUM, ..Text::default() });
            }
        }
        for m in self.markers {
            marker_glyph(&mut f, self.view, m, self.selected_marker == Some(m.id), size);
        }
        // Playhead head.
        let x = self.view.x(self.playhead);
        if x > -8.0 && x < size.width + 8.0 {
            let head = Path::new(|b| {
                b.move_to(Point::new(x - 6.0, 3.0));
                b.line_to(Point::new(x + 6.0, 3.0));
                b.line_to(Point::new(x + 6.0, size.height - 10.0));
                b.line_to(Point::new(x, size.height - 3.0));
                b.line_to(Point::new(x - 6.0, size.height - 10.0));
                b.close();
            });
            f.fill(&head, ACCENT);
            f.fill_rectangle(Point::new(x - 0.75, size.height - 4.0), Size::new(1.5, 4.0), ACCENT);
        }
        vec![f.into_geometry()]
    }

    // The ruler keeps the normal pointer, even over the playhead and while scrubbing.
}

/// Top of the marker pennants on the ruler.
const MARKER_TOP: f32 = 14.0;

/// A marker on the ruler: a pennant at its start, and a band along the bottom for spans.
fn marker_glyph(f: &mut Frame, view: View, m: &crate::model::Marker, selected: bool, size: Size) {
    let color = Color::from_rgba(m.color.0, m.color.1, m.color.2, 1.0);
    let x = view.x(m.at).round();
    if m.duration > 0.0 {
        let x1 = view.x(m.at + m.duration);
        f.fill_rectangle(Point::new(x, size.height - 6.0), Size::new((x1 - x).max(2.0), 5.0), Color { a: 0.85, ..color });
    }
    if x < -10.0 || x > size.width + 10.0 {
        return;
    }
    f.fill_rectangle(Point::new(x - 0.5, MARKER_TOP + 6.0), Size::new(1.0, size.height - MARKER_TOP - 6.0), color);
    let pennant = Path::new(|b| {
        b.move_to(Point::new(x - 5.0, MARKER_TOP));
        b.line_to(Point::new(x + 5.0, MARKER_TOP));
        b.line_to(Point::new(x + 5.0, MARKER_TOP + 5.0));
        b.line_to(Point::new(x, MARKER_TOP + 10.0));
        b.line_to(Point::new(x - 5.0, MARKER_TOP + 5.0));
        b.close();
    });
    f.fill(&pennant, color);
    if selected {
        f.stroke(&pennant, Stroke::default().with_color(Color::WHITE).with_width(1.5));
        f.fill_text(Text { content: m.name.clone(), position: Point::new(x + 9.0, MARKER_TOP + 4.0), color: TEXT_1, size: 10.5.into(), font: INTER_SEMIBOLD, align_y: alignment::Vertical::Center, ..Text::default() });
    }
}

/// A small lettered flag hanging from the top of the ruler.
/// One edge of the range on the ruler: a line with a small grip to drag it by.
fn edge_handle(f: &mut Frame, x: f32) {
    f.fill_rectangle(Point::new(x - 0.5, 0.0), Size::new(1.0, RULER_H), ACCENT);
    let grip = Path::rounded_rectangle(Point::new(x - 3.0, 2.0), Size::new(6.0, 14.0), 3.0.into());
    f.fill(&grip, ACCENT);
}

fn wheel(delta: ScrollDelta) -> (f32, f32) {
    match delta {
        ScrollDelta::Lines { x, y } => (x * 48.0, y * 48.0),
        ScrollDelta::Pixels { x, y } => (x, y),
    }
}

fn scrolled(view: View, px: f32) -> View {
    View { scroll: (view.scroll - px as f64 / view.zoom).max(0.0), ..view }
}

// ---------------------------------------------------------------- lanes

/// Clips gliding to a new place after an edit moved them (a ripple, closing gaps, undo).
pub struct Anim {
    /// Where each moving clip was drawn before: its start time and the top of its row (unscrolled).
    pub from: std::collections::HashMap<Id, (f64, f32)>,
    pub started: std::time::Instant,
}

impl Anim {
    pub const SECONDS: f32 = 0.22;

    /// How far along the glide is, eased: 0 at the old place, 1 at the new.
    pub fn progress(&self, now: std::time::Instant) -> f32 {
        let k = (now.saturating_duration_since(self.started).as_secs_f32() / Self::SECONDS).clamp(0.0, 1.0);
        1.0 - (1.0 - k).powi(3)
    }

    pub fn done(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.started).as_secs_f32() >= Self::SECONDS
    }

    /// Where a clip that is really at (`start`, row top `top`) is drawn at progress `e`.
    pub fn shown(&self, id: Id, start: f64, top: f32, e: f32) -> (f64, f32) {
        match self.from.get(&id) {
            Some(&(s0, y0)) => (s0 + (start - s0) * e as f64, y0 + (top - y0) * e),
            None => (start, top),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Lanes<'a> {
    pub project: &'a Project,
    /// Clips on their way to a new place, and how far along they are (0–1).
    pub anim: Option<(&'a Anim, f32)>,
    pub media: &'a MediaCache,
    pub selection: &'a [Id],
    pub playhead: f64,
    pub view: View,
    pub snapping: bool,
    /// The zoom open in the canvas editor, highlighted on its clip.
    pub zoom_edit: Option<(Id, Id)>,
    pub range: Option<(f64, f64)>,
    /// Asset being dragged in from the library.
    pub dnd: Option<Id>,
    /// Effect being dragged in from the library.
    pub dnd_effect: Option<super::library::Effect>,
    /// Assets whose file can't be found.
    pub missing: &'a std::collections::HashSet<Id>,
    pub cache: &'a Caches,
    /// Whether the tracks overflow vertically (then the plain wheel scrolls them).
    pub overflow: bool,
    /// How far the track list is scrolled up, in pixels.
    pub vscroll: f32,
    /// The Range tool is on: pressing and dragging on the lanes draws the range.
    pub range_mode: bool,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Pan { x0: f32, scroll0: f64 },
    /// `new_top`: dropped above the top track, onto a new one (pictures only).
    Move { clip: Id, grab: f64, track: Id, start: f64, press: Point, moved: bool, new_top: bool },
    Trim { clip: Id, edge: Edge, t: f64 },
    /// Moving a zoom along its clip; `at` is clip-relative.
    ZoomMove { clip: Id, zoom: Id, grab: f64, at: f64, press: Point, moved: bool },
    ZoomTrim { clip: Id, zoom: Id, edge: Edge, at: f64, duration: f64 },
    /// Resizing a transition by its handle. `seam` is the cut it is centred on (it never moves).
    TransitionTrim { clip: Id, seam: f64, duration: f64 },
    /// Range tool: drawing a range, or dragging one of its edges; `fixed` is the other end.
    Range { fixed: f64 },
    /// A selection box dragged over empty space: every clip it touches gets selected.
    Box { from: Point, to: Point, moved: bool },
    /// A keyframe's diamond dragged along its clip: from `from` to `at` (both on the clip's clock).
    Key { clip: Id, from: f64, at: f64, press: Point, moved: bool },
}

#[derive(Default)]
pub struct LanesState {
    drag: Option<Drag>,
    modifiers: keyboard::Modifiers,
    hover: Option<Point>,
    guide: Option<f64>,
    /// Selection box: what was selected before it (kept with Shift/Ctrl) and what it last reported.
    box_base: Vec<Id>,
    box_last: Vec<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zone {
    Body,
    Edge(Edge),
}

impl Lanes<'_> {
    fn row_at(&self, y: f32) -> Option<(Id, TrackKind, f32, f32)> {
        rows(self.project, self.vscroll).into_iter().find(|&(_, _, top, h)| y >= top - GAP / 2.0 && y < top + h + GAP / 2.0)
    }

    fn row_of(&self, track: Id) -> Option<(Id, TrackKind, f32, f32)> {
        rows(self.project, self.vscroll).into_iter().find(|r| r.0 == track)
    }

    fn hit(&self, p: Point) -> Option<(&Clip, Zone)> {
        let (track, _, top, h) = self.row_at(p.y)?;
        if p.y < top || p.y > top + h {
            return None;
        }
        // Clips on locked tracks can't be picked up.
        if self.project.track(track).is_some_and(|t| t.locked) {
            return None;
        }
        let t = self.view.t(p.x);
        let clip = self.project.clips_on(track).find(|c| t >= c.start && t <= c.end())?;
        let (x0, x1) = (self.view.x(clip.start), self.view.x(clip.end()));
        let grab = EDGE_GRAB.min((x1 - x0) / 4.0);
        let zone = if p.x - x0 <= grab {
            Zone::Edge(Edge::Start)
        } else if x1 - p.x <= grab {
            Zone::Edge(Edge::End)
        } else {
            Zone::Body
        };
        Some((clip, zone))
    }

    /// The zoom band under `p`: (clip, zoom, which part).
    fn zoom_hit(&self, p: Point) -> Option<(&Clip, &crate::clip::Zoom, Zone)> {
        let (clip, _) = self.hit(p)?;
        let (_, _, top, h) = self.row_of(clip.track)?;
        let band_top = top + h - ZOOM_BAND_H - 3.0;
        if p.y < band_top || p.y > top + h {
            return None;
        }
        clip.zooms.iter().find_map(|z| {
            let (x0, x1) = (self.view.x(clip.start + z.at), self.view.x(clip.start + z.at + z.duration));
            if p.x < x0 - 3.0 || p.x > x1 + 3.0 {
                return None;
            }
            let grab = 6.0f32.min((x1 - x0) / 4.0);
            let zone = if p.x - x0 <= grab { Zone::Edge(Edge::Start) } else if x1 - p.x <= grab { Zone::Edge(Edge::End) } else { Zone::Body };
            Some((clip, z, zone))
        })
    }

    /// The keyframe diamond under `p`: its clip and its time on the clip's clock.
    fn key_hit(&self, p: Point) -> Option<(&Clip, f64)> {
        let (clip, _) = self.hit(p)?;
        let (_, _, top, h) = self.row_of(clip.track)?;
        if (p.y - key_row_y(top, h)).abs() > KEY_R + 4.0 {
            return None;
        }
        let times = clip.key_times();
        let nearest = times.into_iter().map(|t| (t, (self.view.x(clip.start + t) - p.x).abs())).filter(|(_, d)| *d <= KEY_R + 3.0).min_by(|a, b| a.1.total_cmp(&b.1))?;
        Some((clip, nearest.0))
    }

    /// The handle at the leading edge of an applied transition's bowtie, under `p`: drag it to
    /// resize the transition (it stays centred on the cut, which never moves). Checked before the
    /// clips themselves, since the handle sits inside the outgoing clip.
    fn transition_hit(&self, p: Point) -> Option<(&Clip, f64)> {
        let (track, _, top, h) = self.row_at(p.y)?;
        if p.y < top || p.y > top + h || self.project.track(track).is_some_and(|t| t.locked) {
            return None;
        }
        self.project.clips_on(track).find_map(|c| {
            let (_, tr) = self.project.transition_into(c)?;
            ((self.view.x(c.start - tr.duration / 2.0) - p.x).abs() <= EDGE_GRAB).then_some((c, tr.duration))
        })
    }

    /// The incoming clip of the nearest transition-eligible seam (a clip whose start touches a
    /// previous clip on the same track) within grab distance of `p`, for dropping a Transition
    /// effect precisely where two clips meet instead of generically "on a clip".
    fn seam_hit(&self, p: Point) -> Option<Id> {
        const SEAM_GRAB: f32 = 28.0;
        let (track, _, top, h) = self.row_at(p.y)?;
        if p.y < top || p.y > top + h || self.project.track(track).is_some_and(|t| t.locked) {
            return None;
        }
        self.project
            .clips_on(track)
            // A clip that touches the one before it.
            .filter(|c| self.project.prev_on_track(c).is_some_and(|pv| (pv.end() - c.start).abs() < 1e-4))
            .map(|c| (c.id, (self.view.x(c.start) - p.x).abs()))
            .filter(|&(_, d)| d <= SEAM_GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// The clips that move when `clip` is dragged: the whole selection when it is part of it,
    /// and always whatever is grouped with them.
    fn moving_with(&self, clip: Id) -> Vec<Id> {
        if self.selection.contains(&clip) { self.project.with_groups(self.selection) } else { self.project.with_groups(&[clip]) }
    }

    /// Clips touched by the selection box between `a` and `b` (not on locked tracks).
    fn box_hits(&self, a: Point, b: Point) -> Vec<Id> {
        let (x0, x1, y0, y1) = (a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y));
        let (t0, t1) = (self.view.t(x0), self.view.t(x1));
        rows(self.project, self.vscroll)
            .into_iter()
            .filter(|&(track, _, top, h)| top < y1 && top + h > y0 && !self.project.track(track).is_some_and(|t| t.locked))
            .flat_map(|(track, ..)| self.project.clips_on(track).filter(|c| c.start < t1 && c.end() > t0).map(|c| c.id))
            .collect()
    }

    fn snap_points(&self, exclude: &[Id]) -> Vec<f64> {
        let mut pts = vec![0.0, self.playhead];
        for c in self.project.clips.iter().filter(|c| !exclude.contains(&c.id)) {
            pts.push(c.start);
            pts.push(c.end());
        }
        if let Some((a, b)) = self.range {
            pts.extend([a, b]);
        }
        for m in &self.project.markers {
            pts.push(m.at);
            if m.duration > 0.0 {
                pts.push(m.at + m.duration);
            }
        }
        pts
    }

    /// Snaps `t` to the nearest point within a few pixels. Returns the time and the guide position.
    fn snap(&self, t: f64, exclude: &[Id]) -> (f64, Option<f64>) {
        if !self.snapping {
            return (t, None);
        }
        let limit = SNAP_PX as f64 / self.view.zoom;
        self.snap_points(exclude)
            .into_iter()
            .map(|p| (p, (p - t).abs()))
            .filter(|&(_, d)| d <= limit)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(p, _)| (p, Some(p)))
            .unwrap_or((t, None))
    }

    /// Snaps a range edge to clip edges, the playhead and markers (not to the range itself).
    fn snap_range(&self, t: f64) -> (f64, Option<f64>) {
        let lanes = Lanes { range: None, ..*self };
        lanes.snap(t, &[])
    }

    /// The other end of the range, if `x` is on one of its edges.
    fn range_edge_at(&self, x: f32) -> Option<f64> {
        let (a, b) = self.range?;
        let (da, db) = ((self.view.x(a) - x).abs(), (self.view.x(b) - x).abs());
        (da.min(db) <= FLAG_GRAB).then_some(if da <= db { b } else { a })
    }

    /// Snaps a moving clip by whichever of its edges is closer to a snap point.
    fn snap_span(&self, start: f64, duration: f64, exclude: &[Id]) -> (f64, Option<f64>) {
        let (s, gs) = self.snap(start, exclude);
        let (e, ge) = self.snap(start + duration, exclude);
        match (gs, ge) {
            (Some(_), Some(_)) if (e - (start + duration)).abs() < (s - start).abs() => (e - duration, ge),
            (Some(_), _) => (s, gs),
            (None, Some(_)) => (e - duration, ge),
            _ => (start, None),
        }
    }
}

impl canvas::Program<Message> for Lanes<'_> {
    type State = LanesState;

    fn update(&self, state: &mut LanesState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let local = cursor.position_in(bounds);
        match event {
            canvas::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = *m;
                None
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = local?;
                let (dx, dy) = wheel(*delta);
                if state.modifiers.command() {
                    return Some(Action::publish(Message::SetView(self.view.zoomed(1.0025f64.powf(dy as f64), p.x))).and_capture());
                }
                if state.modifiers.shift() || !self.overflow || dx.abs() > dy.abs() {
                    let px = if dx.abs() > dy.abs() { dx } else { dy };
                    return Some(Action::publish(Message::SetView(scrolled(self.view, px))).and_capture());
                }
                Some(Action::publish(Message::ScrollTracks(-dy)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let p = local?;
                if *button == mouse::Button::Middle {
                    state.drag = Some(Drag::Pan { x0: p.x, scroll0: self.view.scroll });
                    return Some(Action::capture());
                }
                // Right-click selects the clip under the cursor, so the context menu acts on it.
                if *button == mouse::Button::Right {
                    if let Some((clip, at)) = self.key_hit(p) {
                        return Some(Action::publish(Message::KeyMenu { clip: clip.id, at }));
                    }
                    let (track, t) = (self.row_at(p.y).filter(|&(_, _, top, h)| p.y >= top && p.y <= top + h).map(|r| r.0), self.view.t(p.x).max(0.0));
                    return Some(Action::publish(Message::LaneContext { clip: self.hit(p).map(|(c, _)| c.id), track, t }));
                }
                if *button != mouse::Button::Left {
                    return None;
                }
                let t = self.view.t(p.x).max(0.0);
                // Range tool: grab an edge of the range, or press and drag to draw a new one.
                // The clip pressed on gets selected, so range edits know which clip they mean.
                if self.range_mode {
                    if let Some(fixed) = self.range_edge_at(p.x) {
                        state.drag = Some(Drag::Range { fixed });
                        return Some(Action::capture());
                    }
                    let (from, _) = self.snap_range(t);
                    state.drag = Some(Drag::Range { fixed: from });
                    return Some(Action::publish(Message::RangePress { clip: self.hit(p).map(|(c, _)| c.id) }).and_capture());
                }
                // A keyframe's diamond: the clip is selected and the playhead goes there; a drag moves it.
                if let Some((clip, at)) = self.key_hit(p) {
                    state.drag = Some(Drag::Key { clip: clip.id, from: at, at, press: p, moved: false });
                    return Some(Action::publish(Message::KeyMenu { clip: clip.id, at }).and_capture());
                }
                // Zoom bands sit on top of their clip and take clicks first.
                if let Some((clip, z, zone)) = self.zoom_hit(p) {
                    state.drag = Some(match zone {
                        Zone::Edge(edge) => Drag::ZoomTrim { clip: clip.id, zoom: z.id, edge, at: z.at, duration: z.duration },
                        Zone::Body => Drag::ZoomMove { clip: clip.id, zoom: z.id, grab: t - clip.start - z.at, at: z.at, press: p, moved: false },
                    });
                    return Some(Action::publish(Message::ZoomEdit(Some((clip.id, z.id)))).and_capture());
                }
                // A transition's handle also takes clicks first, over the ordinary start-trim zone.
                if let Some((clip, duration)) = self.transition_hit(p) {
                    state.drag = Some(Drag::TransitionTrim { clip: clip.id, seam: clip.start, duration });
                    return Some(Action::publish(Message::Select(Some(clip.id), false)).and_capture());
                }
                {
                    match self.hit(p) {
                        Some((clip, Zone::Edge(edge))) => {
                            let t = if edge == Edge::Start { clip.start } else { clip.end() };
                            state.drag = Some(Drag::Trim { clip: clip.id, edge, t });
                            Some(Action::publish(Message::Select(Some(clip.id), false)).and_capture())
                        }
                        Some((clip, Zone::Body)) => {
                            state.drag = Some(Drag::Move { clip: clip.id, grab: t - clip.start, track: clip.track, start: clip.start, press: p, moved: false, new_top: false });
                            let additive = state.modifiers.shift() || state.modifiers.command();
                            Some(Action::publish(Message::Select(Some(clip.id), additive)).and_capture())
                        }
                        // Empty space: a click deselects, a drag draws a selection box
                        // (Shift or Ctrl keeps what was selected and adds to it).
                        None => {
                            let additive = state.modifiers.shift() || state.modifiers.command();
                            state.box_base = if additive { self.selection.to_vec() } else { Vec::new() };
                            state.box_last = state.box_base.clone();
                            state.drag = Some(Drag::Box { from: p, to: p, moved: false });
                            if additive { Some(Action::capture()) } else { Some(Action::publish(Message::Select(None, false)).and_capture()) }
                        }
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let p = Point::new(position.x - bounds.x, position.y - bounds.y);
                state.hover = local;
                let Some(drag) = state.drag.as_mut() else {
                    // Keep the drop preview following the cursor during a library drag.
                    return (self.dnd.is_some() || self.dnd_effect.is_some()).then(Action::request_redraw);
                };
                match drag {
                    Drag::Pan { x0, scroll0 } => {
                        let view = View { scroll: (*scroll0 - (p.x - *x0) as f64 / self.view.zoom).max(0.0), ..self.view };
                        return Some(Action::publish(Message::SetView(view)).and_capture());
                    }
                    Drag::Move { clip, grab, track, start, press, moved, new_top } => {
                        if !*moved && press.distance(p) < 4.0 {
                            return None;
                        }
                        *moved = true;
                        let c = self.project.clip(*clip)?;
                        let kind = self.project.clip_kind(c);
                        let first_top = rows(self.project, self.vscroll).first().map(|r| r.2).unwrap_or(TOP_PAD);
                        *new_top = kind == TrackKind::Visual && p.y < first_top - GAP / 2.0;
                        if let Some((row, row_kind, _, _)) = self.row_at(p.y) {
                            if row_kind == kind && !self.project.track(row).is_some_and(|t| t.locked) {
                                *track = row;
                            }
                        }
                        let (s, guide) = self.snap_span((self.view.t(p.x) - *grab).max(0.0), c.duration, &self.moving_with(c.id));
                        *start = s;
                        state.guide = guide;
                    }
                    Drag::Box { from, to, moved } => {
                        if !*moved && from.distance(p) < 4.0 {
                            return None;
                        }
                        (*moved, *to) = (true, p);
                        let mut ids = state.box_base.clone();
                        for id in self.box_hits(*from, p) {
                            if !ids.contains(&id) {
                                ids.push(id);
                            }
                        }
                        // Only tell the editor when the set of clips inside the box changes.
                        if ids == state.box_last {
                            return Some(Action::request_redraw().and_capture());
                        }
                        state.box_last = ids.clone();
                        return Some(Action::publish(Message::SelectBox(ids)).and_capture());
                    }
                    Drag::Range { fixed } => {
                        let (t, guide) = self.snap_range(self.view.t(p.x).max(0.0));
                        state.guide = guide;
                        if (t - *fixed).abs() < 0.05 {
                            return Some(Action::request_redraw().and_capture());
                        }
                        return Some(Action::publish(Message::SetRange(Some((t.min(*fixed), t.max(*fixed))))).and_capture());
                    }
                    Drag::Trim { clip, t, .. } => {
                        let (s, guide) = self.snap(self.view.t(p.x).max(0.0), &[*clip]);
                        *t = s;
                        state.guide = guide;
                    }
                    Drag::ZoomMove { clip, zoom, grab, at, press, moved } => {
                        if !*moved && press.distance(p) < 3.0 {
                            return None;
                        }
                        *moved = true;
                        let c = self.project.clip(*clip)?;
                        let z = c.zooms.iter().find(|z| z.id == *zoom)?;
                        let (s, guide) = self.snap_span((self.view.t(p.x) - *grab).max(0.0), z.duration, &[*clip]);
                        *at = (s - c.start).clamp(0.0, (c.duration - z.duration).max(0.0));
                        state.guide = guide;
                    }
                    Drag::Key { clip, at, press, moved, .. } => {
                        if !*moved && press.distance(p) < 3.0 {
                            return None;
                        }
                        *moved = true;
                        let c = self.project.clip(*clip)?;
                        let (t, guide) = self.snap(self.view.t(p.x), &[]);
                        *at = (t - c.start).clamp(0.0, c.duration);
                        state.guide = guide;
                    }
                    Drag::TransitionTrim { clip, seam, duration } => {
                        let c = self.project.clip(*clip)?;
                        let prev_dur = self.project.prev_on_track(c).map(|pv| pv.duration).unwrap_or(f64::MAX);
                        // (Roughly: the edit itself keeps each clip a moment of its own.)
                        let max_dur = (2.0 * c.duration.min(prev_dur)).max(0.1);
                        let t = self.view.t(p.x);
                        *duration = (2.0 * (*seam - t)).clamp(0.1, max_dur);
                    }
                    Drag::ZoomTrim { clip, zoom, edge, at, duration } => {
                        let c = self.project.clip(*clip)?;
                        let z = c.zooms.iter().find(|z| z.id == *zoom)?;
                        let (t, guide) = self.snap(self.view.t(p.x), &[*clip]);
                        let local = (t - c.start).clamp(0.0, c.duration);
                        let end = z.at + z.duration;
                        match edge {
                            Edge::Start => {
                                *at = local.min(end - 0.3);
                                *duration = end - *at;
                            }
                            Edge::End => {
                                *at = z.at;
                                *duration = (local - z.at).max(0.3);
                            }
                        }
                        state.guide = guide;
                    }
                }
                Some(Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                state.guide = None;
                if let (Some(effect), Some(p)) = (self.dnd_effect, local) {
                    // A transition targets the seam between two clips, not whichever clip's body
                    // happens to be under the cursor.
                    let clip = if matches!(effect, super::library::Effect::Transition(_)) { self.seam_hit(p) } else { self.hit(p).map(|(c, _)| c.id) };
                    let track = self.row_at(p.y).map(|r| r.0);
                    let (t, _) = self.snap(self.view.t(p.x).max(0.0), &[]);
                    return Some(Action::publish(Message::DropEffect { effect, target: super::DropTarget::Timeline { clip, track, t } }).and_capture());
                }
                if let (Some(asset), Some(p)) = (self.dnd, local) {
                    let track = self.row_at(p.y).map(|r| r.0);
                    let (t, _) = self.snap(self.view.t(p.x).max(0.0), &[]);
                    return Some(Action::publish(Message::DropAsset { asset, track, t }).and_capture());
                }
                let msg = match state.drag.take()? {
                    Drag::Move { clip, start, moved: true, new_top: true, .. } => Message::MoveClipToNewTrack { clip, start },
                    Drag::Move { clip, track, start, moved: true, .. } => Message::MoveClip { clip, track, start },
                    Drag::Move { .. } | Drag::Pan { .. } | Drag::Box { .. } => return Some(Action::request_redraw().and_capture()),
                    Drag::Trim { clip, edge, t } => Message::Trim { clip, edge, t },
                    Drag::Range { .. } => {
                        state.guide = None;
                        return Some(Action::request_redraw().and_capture());
                    }
                    Drag::ZoomMove { moved: false, .. } => return Some(Action::request_redraw().and_capture()),
                    Drag::ZoomMove { clip, zoom, at, .. } => {
                        let duration = self.project.clip(clip)?.zooms.iter().find(|z| z.id == zoom)?.duration;
                        Message::ZoomTiming { clip, zoom, at, duration }
                    }
                    Drag::ZoomTrim { clip, zoom, at, duration, .. } => Message::ZoomTiming { clip, zoom, at, duration },
                    Drag::TransitionTrim { clip, duration, .. } => Message::Edit(clip, super::inspector::Edit::TransitionDuration(duration)),
                    Drag::Key { moved: false, .. } => return Some(Action::request_redraw().and_capture()),
                    Drag::Key { clip, from, at, .. } => Message::MoveKeys { clip, from, to: at },
                };
                Some(Action::publish(msg).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                state.hover = None;
                Some(Action::request_redraw())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &LanesState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let deco = self.cache.deco.draw(renderer, bounds.size(), |f| self.draw_layer(f, Layer::Deco));
        let mut o = Frame::new(renderer, bounds.size());
        let h = bounds.height;

        // Range selection.
        let range = self.range;
        if let Some((a, b)) = range {
            let (x0, x1) = (self.view.x(a), self.view.x(b));
            o.fill_rectangle(Point::new(x0, 0.0), Size::new(x1 - x0, h), Color { a: 0.10, ..ACCENT });
            for x in [x0, x1] {
                o.fill_rectangle(Point::new(x - 0.5, 0.0), Size::new(1.0, h), Color { a: 0.8, ..ACCENT });
            }
        }

        // Move / trim previews.
        match state.drag {
            // Above the top track: a line where the new track will appear, and the clip on it.
            Some(Drag::Move { clip, start, moved: true, new_top: true, .. }) => {
                if let Some(c) = self.project.clip(clip) {
                    let first_top = rows(self.project, self.vscroll).first().map(|r| r.2).unwrap_or(TOP_PAD);
                    let y = (first_top - GAP / 2.0).max(1.0);
                    o.fill_rectangle(Point::new(0.0, y - 1.0), Size::new(bounds.width, 2.0), ACCENT);
                    let h = default_track_height(TrackKind::Visual);
                    let r = Rectangle::new(Point::new(self.view.x(start), y + 3.0), Size::new((c.duration * self.view.zoom) as f32, h * 0.5));
                    ghost(&mut o, r, clip_color(self.project, c));
                }
            }
            // Every clip that moves along (the rest of the selection, grouped clips) shows where it lands.
            Some(Drag::Move { clip, track, start, moved: true, .. }) => {
                if let Some(anchor) = self.project.clip(clip) {
                    let ids = self.moving_with(clip);
                    let dt = start - anchor.start;
                    for (id, target) in self.project.move_targets(&ids, clip, track).unwrap_or_else(|| vec![(clip, track)]) {
                        if let (Some(c), Some((_, _, top, rh))) = (self.project.clip(id), self.row_of(target)) {
                            let r = Rectangle::new(Point::new(self.view.x(c.start + dt), top), Size::new((c.duration * self.view.zoom) as f32, rh));
                            ghost(&mut o, r, clip_color(self.project, c));
                        }
                    }
                }
            }
            Some(Drag::Box { from, to, moved: true }) => {
                let r = Rectangle::new(Point::new(from.x.min(to.x), from.y.min(to.y)), Size::new((from.x - to.x).abs(), (from.y - to.y).abs()));
                o.fill_rectangle(r.position(), r.size(), Color { a: 0.10, ..ACCENT });
                o.stroke(&Path::rectangle(r.position(), r.size()), Stroke::default().with_color(Color { a: 0.85, ..ACCENT }).with_width(1.0));
            }
            Some(Drag::Trim { clip, edge, t }) => {
                if let (Some(c), Some((_, _, top, rh))) = (self.project.clip(clip), self.row_of(self.project.clip(clip).map(|c| c.track).unwrap_or(0))) {
                    let (s, e) = if edge == Edge::Start { (t.min(c.end() - 0.1), c.end()) } else { (c.start, t.max(c.start + 0.1)) };
                    let r = Rectangle::new(Point::new(self.view.x(s), top), Size::new(((e - s) * self.view.zoom) as f32, rh));
                    ghost(&mut o, r, clip_color(self.project, c));
                }
            }
            Some(Drag::TransitionTrim { clip, seam, duration }) => {
                if let Some((_, _, top, rh)) = self.project.clip(clip).and_then(|c| self.row_of(c.track)) {
                    let (x0, x1) = (self.view.x(seam - duration / 2.0), self.view.x(seam + duration / 2.0));
                    let r = Rectangle::new(Point::new(x0, top), Size::new(x1 - x0, rh));
                    ghost(&mut o, r, ACCENT);
                }
            }
            Some(Drag::ZoomMove { clip, zoom, at, moved: true, .. }) | Some(Drag::ZoomTrim { clip, zoom, at, .. }) => {
                if let (Some(c), Some((_, _, top, rh))) = (self.project.clip(clip), self.project.clip(clip).and_then(|c| self.row_of(c.track))) {
                    let duration = match state.drag {
                        Some(Drag::ZoomTrim { duration, .. }) => duration,
                        _ => c.zooms.iter().find(|z| z.id == zoom).map(|z| z.duration).unwrap_or(1.0),
                    };
                    let r = Rectangle::new(Point::new(self.view.x(c.start + at), top + rh - ZOOM_BAND_H - 3.0), Size::new((duration * self.view.zoom) as f32, ZOOM_BAND_H));
                    let path = Path::rounded_rectangle(r.position(), r.size(), 4.0.into());
                    o.fill(&path, Color { a: 0.35, ..ACCENT });
                    o.stroke(&path, Stroke::default().with_color(ACCENT).with_width(1.5));
                }
            }
            Some(Drag::Key { clip, at, moved: true, .. }) => {
                if let (Some(c), Some((_, _, top, rh))) = (self.project.clip(clip), self.project.clip(clip).and_then(|c| self.row_of(c.track))) {
                    diamond(&mut o, Point::new(self.view.x(c.start + at), key_row_y(top, rh)), KEY_R + 1.5, ACCENT);
                }
            }
            _ => {}
        }

        // Where a dragged effect would land. A transition doesn't cover the whole clip like
        // other effects - preview the actual overlap it would cut into the previous clip,
        // so dropping it reads as "a crossfade goes here", not "this effect fills the clip".
        if let (Some(effect), Some(p)) = (self.dnd_effect, state.hover) {
            if matches!(effect, super::library::Effect::Transition(_)) {
                if let Some(id) = self.seam_hit(p) {
                    if let (Some(c), Some(prev)) = (self.project.clip(id), self.project.clip(id).and_then(|c| self.project.prev_on_track(c))) {
                        if let Some((_, _, top, rh)) = self.row_of(c.track) {
                            let existing = c.transition.map(|t| t.duration).unwrap_or(0.0);
                            let seam = c.start;
                            let duration = (if existing > 0.0 { existing } else { 0.6 }).clamp(0.1, 2.0 * c.duration.min(prev.duration));
                            let (x0, x1) = (self.view.x(seam - duration / 2.0), self.view.x(seam + duration / 2.0));
                            let r = Rectangle::new(Point::new(x0, top), Size::new(x1 - x0, rh));
                            ghost(&mut o, r, ACCENT);
                        }
                    }
                }
            } else if let Some((c, _)) = self.hit(p) {
                if let Some((_, _, top, rh)) = self.row_of(c.track) {
                    let r = Rectangle::new(Point::new(self.view.x(c.start), top), Size::new((c.duration * self.view.zoom) as f32, rh));
                    ghost(&mut o, r, ACCENT);
                }
            }
        }

        // Timeline markers: a thin line in their color (spans are lightly shaded).
        for m in &self.project.markers {
            let color = Color::from_rgba(m.color.0, m.color.1, m.color.2, 1.0);
            let x = self.view.x(m.at);
            if m.duration > 0.0 {
                let x1 = self.view.x(m.at + m.duration);
                o.fill_rectangle(Point::new(x, 0.0), Size::new(x1 - x, h), Color { a: 0.05, ..color });
            }
            o.fill_rectangle(Point::new(x - 0.5, 0.0), Size::new(1.0, h), Color { a: 0.45, ..color });
        }

        // Drop preview for a library drag.
        if let (Some(asset), Some(p)) = (self.dnd, state.hover) {
            if let (Some(a), Some((_, kind, top, rh))) = (self.project.asset(asset), self.row_at(p.y)) {
                let fits = (kind == TrackKind::Audio) == (a.kind == MediaKind::Audio);
                let dur = if a.kind == MediaKind::Image { crate::edit::DEFAULT_STILL } else { a.duration };
                let r = Rectangle::new(Point::new(p.x, top), Size::new((dur * self.view.zoom) as f32, rh));
                ghost(&mut o, r, if fits { ACCENT } else { DANGER });
            }
        }

        if let Some(g) = state.guide {
            let x = self.view.x(g);
            o.fill_rectangle(Point::new(x - 0.5, 0.0), Size::new(1.0, h), WARNING);
        }

        // Playhead.
        let x = self.view.x(self.playhead);
        o.fill_rectangle(Point::new(x - 0.75, 0.0), Size::new(1.5, h), ACCENT);

        // Thin scrollbar when the tracks don't fit.
        let total = content_height(self.project);
        if total > h + 1.0 {
            let thumb_h = (h / total * h).max(24.0);
            let y = self.vscroll / (total - h) * (h - thumb_h);
            let thumb = Path::rounded_rectangle(Point::new(bounds.width - 6.0, y + 2.0), Size::new(4.0, thumb_h - 4.0), 2.0.into());
            o.fill(&thumb, Color { a: 0.22, ..Color::WHITE });
        }

        // Hovered clip edge.
        if state.drag.is_none() && self.zoom_hit(state.hover.unwrap_or(Point::new(-1.0, -1.0))).is_none() {
            if let Some((c, Zone::Edge(edge))) = state.hover.and_then(|p| self.hit(p)) {
                if let Some((_, _, top, rh)) = self.row_of(c.track) {
                    let x = self.view.x(if edge == Edge::Start { c.start } else { c.end() });
                    let bar = Path::rounded_rectangle(Point::new(if edge == Edge::Start { x } else { x - 4.0 }, top + 2.0), Size::new(4.0, rh - 4.0), 2.0.into());
                    o.fill(&bar, Color::WHITE);
                }
            }
        }
        vec![deco, o.into_geometry()]
    }

    fn mouse_interaction(&self, state: &LanesState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.drag {
            Some(Drag::Move { moved: true, .. }) | Some(Drag::Pan { .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Trim { .. } | Drag::ZoomTrim { .. } | Drag::TransitionTrim { .. }) => return mouse::Interaction::ResizingHorizontally,
            Some(Drag::ZoomMove { moved: true, .. }) | Some(Drag::Key { moved: true, .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Range { .. }) => return mouse::Interaction::ResizingHorizontally,
            _ => {}
        }
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        if self.range_mode {
            return if self.range_edge_at(p.x).is_some() { mouse::Interaction::ResizingHorizontally } else { mouse::Interaction::Crosshair };
        }
        if self.key_hit(p).is_some() {
            return mouse::Interaction::Grab;
        }
        if let Some((_, _, zone)) = self.zoom_hit(p) {
            return if zone == Zone::Body { mouse::Interaction::Grab } else { mouse::Interaction::ResizingHorizontally };
        }
        if self.transition_hit(p).is_some() {
            return mouse::Interaction::ResizingHorizontally;
        }
        match self.hit(p) {
            Some((_, Zone::Edge(_))) => mouse::Interaction::ResizingHorizontally,
            Some((_, Zone::Body)) => mouse::Interaction::Pointer,
            None => mouse::Interaction::default(),
        }
    }
}

fn ghost(f: &mut Frame, r: Rectangle, color: Color) {
    let path = Path::rounded_rectangle(r.position(), r.size(), 6.0.into());
    f.fill(&path, Color { a: 0.22, ..color });
    f.stroke(&path, Stroke::default().with_color(color).with_width(1.5));
}

/// The bottom canvas: lanes, clip bodies and filmstrips. It sits under `Lanes`
/// in a stack because images always draw above shapes within one layer.
pub struct LanesBase<'a>(pub Lanes<'a>);

impl canvas::Program<Message> for LanesBase<'_> {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        vec![self.0.cache.base.draw(renderer, bounds.size(), |f| self.0.draw_layer(f, Layer::Base))]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    /// Lane and clip backgrounds plus filmstrip images.
    Base,
    /// Everything drawn over the images: masks, waveforms, labels, outlines.
    Deco,
}

/// How a clip is outlined on the timeline.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outline {
    Plain,
    /// Grouped with a selected clip: it moves, trims and deletes with it.
    Linked,
    Selected,
}

/// Cached drawings of the two lane canvases.
pub struct Caches {
    base: Cache,
    deco: Cache,
}

impl Default for Caches {
    fn default() -> Self {
        Caches { base: Cache::new(), deco: Cache::new() }
    }
}

impl Caches {
    pub fn clear(&self) {
        self.base.clear();
        self.deco.clear();
    }
}

impl Lanes<'_> {
    fn draw_layer(&self, f: &mut Frame, layer: Layer) {
        let size = f.size();
        if layer == Layer::Base {
            f.fill_rectangle(Point::ORIGIN, size, SURFACE_0);
            if self.project.clips.is_empty() {
                self.draw_empty_hint(f, size);
            }
        }
        let (t0, t1) = (self.view.scroll, self.view.t(size.width));
        // Clips grouped with a selected one are outlined too: they act together.
        let acting = self.project.with_groups(self.selection);
        for (track, kind, top, h) in rows(self.project, self.vscroll).into_iter().filter(|r| r.2 + r.3 > 0.0 && r.2 < size.height) {
            if layer == Layer::Base {
                let lane = Path::rounded_rectangle(Point::new(0.0, top), Size::new(size.width, h), 6.0.into());
                f.fill(&lane, if kind == TrackKind::Audio { Color::from_rgb8(0x10, 0x10, 0x10) } else { SURFACE_1 });
            }
            let track_dim = self.project.track(track).is_some_and(|t| t.hidden || t.muted);
            for clip in self.project.clips_on(track).filter(|c| c.start < t1 && c.end() > t0) {
                // A clip an edit just moved glides from where it was.
                let (start, y) = match self.anim {
                    Some((anim, e)) => {
                        let (s, y) = anim.shown(clip.id, clip.start, top + self.vscroll, e);
                        (s, y - self.vscroll)
                    }
                    None => (clip.start, top),
                };
                // Drawn through a view moved by the same amount, so its filmstrip, waveform and
                // zoom bands travel with it.
                let lanes = Lanes { view: View { scroll: self.view.scroll - (start - clip.start), ..self.view }, ..*self };
                let x0 = lanes.view.x(clip.start);
                let w = (clip.duration * self.view.zoom) as f32;
                let r = Rectangle::new(Point::new(x0, y), Size::new(w.max(2.0), h));
                let outline = if self.selection.contains(&clip.id) {
                    Outline::Selected
                } else if acting.contains(&clip.id) {
                    Outline::Linked
                } else {
                    Outline::Plain
                };
                let dim = track_dim || clip.disabled;
                match layer {
                    Layer::Base => lanes.draw_clip_base(f, clip, r, dim),
                    Layer::Deco => lanes.draw_clip_deco(f, clip, r, outline, dim),
                }
            }
        }
    }

    /// Guidance shown in place of lane content before anything's been added to the project.
    fn draw_empty_hint(&self, f: &mut Frame, size: Size) {
        let cx = size.width / 2.0;
        let cy = (size.height / 2.0).max(40.0);
        f.fill_text(Text {
            content: code::FILM.to_string(),
            position: Point::new(cx, cy - 28.0),
            color: TEXT_4,
            size: 20.0.into(),
            font: ICONS,
            align_x: alignment::Horizontal::Center.into(),
            align_y: alignment::Vertical::Center,
            ..Text::default()
        });
        f.fill_text(Text {
            content: "Nothing here yet".to_string(),
            position: Point::new(cx, cy),
            color: TEXT_2,
            size: T_LARGE.into(),
            font: INTER_SEMIBOLD,
            align_x: alignment::Horizontal::Center.into(),
            align_y: alignment::Vertical::Center,
            ..Text::default()
        });
        f.fill_text(Text {
            content: "Drag media from the library, or press T to add text".to_string(),
            position: Point::new(cx, cy + 22.0),
            color: TEXT_3,
            size: T_SMALL.into(),
            font: INTER,
            align_x: alignment::Horizontal::Center.into(),
            align_y: alignment::Vertical::Center,
            ..Text::default()
        });
    }

    fn filmstrip(&self, clip: &Clip) -> Option<(&crate::model::Asset, &super::media_cache::AssetMedia)> {
        let a = clip.asset().and_then(|a| self.project.asset(a))?;
        let m = self.media.get(a.id)?;
        (self.project.clip_has_picture(clip) && !m.strip.is_empty()).then_some((a, m))
    }

    fn draw_clip_base(&self, f: &mut Frame, clip: &Clip, r: Rectangle, dim: bool) {
        let color = clip_color(self.project, clip);
        let radius = 6.0f32.min(r.width / 2.0);
        let body = Path::rounded_rectangle(r.position(), r.size(), radius.into());
        f.fill(&body, mix(SURFACE_1, color, if dim { 0.18 } else { 0.32 }));
        let inner = Rectangle::new(Point::new(r.x + 1.0, r.y + 1.0), Size::new((r.width - 2.0).max(0.0), r.height - 2.0));
        let Some((a, m)) = self.filmstrip(clip) else { return };
        if inner.width <= 4.0 {
            return;
        }
        let tile_w = (inner.height * m.strip_w as f32 / super::media_cache::STRIP_H as f32).max(8.0);
        let first = ((inner.x.max(-tile_w) - inner.x) / tile_w).floor().max(0.0) as i32;
        let canvas_w = f.size().width;
        // Images and text aren't clipped to the canvas by iced, so clip them to the visible lane area.
        let Some(visible) = on_canvas(inner, f.size()) else { return };
        f.with_clip(visible, |f| {
            let mut i = first;
            loop {
                let x = inner.x + i as f32 * tile_w;
                if x > inner.x + inner.width || x > canvas_w {
                    break;
                }
                let t = self.view.t(x + tile_w / 2.0).clamp(clip.start, clip.end());
                let src = clip.source_time(t);
                let idx = if a.kind == MediaKind::Image || a.duration <= 0.0 { 0 } else { ((src / a.duration) * m.strip.len() as f64).floor() as usize };
                let handle = &m.strip[idx.min(m.strip.len() - 1)];
                let img = canvas::Image::new(handle.clone()).opacity(if dim { 0.35_f32 } else { 0.92 });
                f.draw_image(Rectangle::new(Point::new(x, inner.y), Size::new(tile_w, inner.height)), img);
                i += 1;
            }
        });
    }

    fn draw_clip_deco(&self, f: &mut Frame, clip: &Clip, r: Rectangle, outline: Outline, dim: bool) {
        let selected = outline == Outline::Selected;
        let color = clip_color(self.project, clip);
        let radius = 6.0f32.min(r.width / 2.0);
        let body = Path::rounded_rectangle(r.position(), r.size(), radius.into());
        let inner = Rectangle::new(Point::new(r.x + 1.0, r.y + 1.0), Size::new((r.width - 2.0).max(0.0), r.height - 2.0));
        let has_strip = self.filmstrip(clip).is_some();
        if has_strip {
            corner_masks(f, r, radius, SURFACE_1);
            // Darken the top so the label stays readable over busy footage.
            let shade = Path::rounded_rectangle(r.position(), Size::new(r.width, 22.0f32.min(r.height)), radius.into());
            f.fill(&shade, Color { a: 0.28, ..Color::BLACK });
        }

        // Waveform for audio.
        let asset = clip.asset().and_then(|a| self.project.asset(a));
        let peaks = clip.asset().and_then(|a| self.media.get(a)).and_then(|m| m.peaks.clone());
        if let (Some(a), Some(peaks)) = (asset, peaks) {
            if (a.kind == MediaKind::Audio || clip.part == crate::clip::Part::Sound) && !peaks.is_empty() {
                let mid = r.y + r.height * 0.6;
                let amp = r.height * 0.34 * (clip.audio.volume.min(2.0) as f32);
                let wave_color = Color { a: if dim { 0.3 } else { 0.85 }, ..mix(color, Color::WHITE, 0.25) };
                let (x_start, x_end) = (inner.x.max(0.0), (inner.x + inner.width).min(f.size().width));
                let step = 2.0;
                let rate = PEAKS_PER_SECOND as f64;
                let path = Path::new(|b| {
                    let mut x = x_start;
                    while x < x_end {
                        let ia = (clip.source_time(self.view.t(x)) * rate).max(0.0) as usize;
                        let ib = ((clip.source_time(self.view.t(x + step)) * rate).ceil() as usize).max(ia + 1);
                        let peak = peaks.get(ia.min(peaks.len())..ib.min(peaks.len())).and_then(|s| s.iter().copied().reduce(f32::max)).unwrap_or(0.0);
                        let gain = clip.audio.gain_at(self.view.t(x) - clip.start) as f32;
                        let hh = (peak.sqrt() * amp * gain).max(0.5);
                        b.rectangle(Point::new(x, mid - hh), Size::new(step - 0.6, hh * 2.0));
                        x += step;
                    }
                });
                f.fill(&path, wave_color);
            }
        }

        // Volume line (like Camtasia's audio envelope), shown once part of the clip is changed.
        let audible = self.project.clip_has_sound(clip);
        if audible && !clip.audio.gains.is_empty() {
            let (x_start, x_end) = (inner.x.max(0.0), (inner.x + inner.width).min(f.size().width));
            let level = |v: f64| r.y + 5.0 + (1.0 - (v / 1.5).clamp(0.0, 1.0) as f32) * (r.height - 10.0);
            let line = Path::new(|b| {
                let mut x = x_start;
                b.move_to(Point::new(x, level(clip.audio.volume * clip.audio.gain_at(self.view.t(x) - clip.start))));
                while x < x_end {
                    x = (x + 2.0).min(x_end);
                    b.line_to(Point::new(x, level(clip.audio.volume * clip.audio.gain_at(self.view.t(x) - clip.start))));
                }
            });
            f.stroke(&line, Stroke::default().with_color(ACCENT).with_width(1.5));
        }

        // Text clips preview their text.
        if let Content::Text(t) = &clip.content {
            if r.width > 40.0 {
                let Some(visible) = on_canvas(inner, f.size()) else { return };
                f.with_clip(visible, |f| {
                    f.fill_text(Text {
                        content: t.text.replace('\n', " "),
                        // Sticks to the left edge while the clip start is scrolled out of view.
                        position: Point::new(r.x.max(0.0) + 10.0, r.y + r.height * 0.5),
                        color: Color { a: if dim { 0.4 } else { 0.92 }, ..Color::WHITE },
                        size: 15.0.into(),
                        font: if t.text.is_ascii() { INTER_BOLD } else { HIND_SEMIBOLD },
                        align_y: alignment::Vertical::Center,
                        ..Text::default()
                    });
                });
            }
        }

        // A transition pulls this clip's start to genuinely overlap the previous clip, so this
        // span (`r.x` to `r.x + tw`) is real shared screen time, not just this clip's own corner.
        // Draw it as a bowtie straddling the seam, like most editors' transition icon, instead of
        // a one-sided wedge that reads as this clip's own entrance effect.
        // Centred on the cut at this clip's start, half over the clip before it: nothing moved.
        if let Some((_, tr)) = self.project.transition_into(clip) {
            // min/max, not clamp: very short clips are narrower than the 6 px minimum.
            let tw = ((tr.duration * self.view.zoom) as f32).max(6.0).min(2.0 * r.width);
            let (x0, mid_y) = (r.x - tw / 2.0, r.y + r.height / 2.0);
            let bowtie = Path::new(|b| {
                b.move_to(Point::new(x0, r.y));
                b.line_to(Point::new(x0 + tw, mid_y));
                b.line_to(Point::new(x0, r.y + r.height));
                b.close();
                b.move_to(Point::new(x0 + tw, r.y));
                b.line_to(Point::new(x0, mid_y));
                b.line_to(Point::new(x0 + tw, r.y + r.height));
                b.close();
            });
            f.fill(&bowtie, Color { a: 0.35, ..Color::WHITE });
            f.stroke(&Path::line(Point::new(r.x, r.y), Point::new(r.x, r.y + r.height)), Stroke::default().with_color(Color { a: 0.8, ..Color::WHITE }).with_width(1.0));
        }

        // Zoom and pan animations as bands along the bottom, like Camtasia's animation arrows.
        let mut zooms: Vec<&crate::clip::Zoom> = clip.zooms.iter().collect();
        zooms.sort_by(|a, b| a.at.total_cmp(&b.at));
        for (i, z) in zooms.iter().enumerate() {
            let zx = self.view.x(clip.start + z.at);
            let zw = ((z.duration * self.view.zoom) as f32).max(6.0);
            let band = Rectangle::new(Point::new(zx + 1.0, r.y + r.height - ZOOM_BAND_H - 3.0), Size::new(zw - 2.0, ZOOM_BAND_H));
            let selected = self.zoom_edit == Some((clip.id, z.id));
            let pans = i > 0 && (zooms[i - 1].at + zooms[i - 1].duration - z.at).abs() < 1e-3;
            let fill = if selected { ACCENT } else { Color { a: 0.92, ..BRAND_2 } };
            let path = Path::rounded_rectangle(band.position(), band.size(), 4.0.into());
            f.fill(&path, fill);
            // Easing ramps: darker wedges where the camera moves in and out.
            let ramp = ((z.ramp.min(z.duration / 2.0) * self.view.zoom) as f32).min(band.width / 2.0);
            if ramp > 3.0 {
                let shade = Color { a: 0.28, ..ON_ACCENT };
                let (x0, x1, y0, y1) = (band.x, band.x + band.width, band.y, band.y + band.height);
                f.fill(&Path::new(|b| { b.move_to(Point::new(x0, y1)); b.line_to(Point::new(x0 + ramp, y0)); b.line_to(Point::new(x0, y0)); b.close(); }), shade);
                f.fill(&Path::new(|b| { b.move_to(Point::new(x1, y1)); b.line_to(Point::new(x1 - ramp, y0)); b.line_to(Point::new(x1, y0)); b.close(); }), shade);
            }
            if band.width > 54.0 {
                let Some(band_visible) = on_canvas(band, f.size()) else { continue };
                f.with_clip(band_visible, |f| {
                    f.fill_text(Text {
                        content: if pans { format!("Pan · {:.0}%", z.scale * 100.0) } else { format!("Zoom {:.0}%", z.scale * 100.0) },
                        position: Point::new(band.x + band.width / 2.0, band.y + band.height / 2.0),
                        color: ON_ACCENT,
                        size: 10.0.into(),
                        font: INTER_SEMIBOLD,
                        align_x: alignment::Horizontal::Center.into(),
                        align_y: alignment::Vertical::Center,
                        ..Text::default()
                    });
                });
            }
            if selected {
                f.stroke(&path, Stroke::default().with_color(Color::WHITE).with_width(1.5));
            }
        }

        // Keyframes: a diamond at each moment that has one, joined by a thin line while it animates.
        let times = clip.key_times();
        if !times.is_empty() && r.width > 12.0 {
            let y = key_row_y(r.y, r.height);
            let (first, last) = (self.view.x(clip.start + times[0]), self.view.x(clip.start + times[times.len() - 1]));
            let alpha = if selected { 1.0 } else { 0.7 };
            if last - first > 1.0 {
                f.fill_rectangle(Point::new(first, y - 0.75), Size::new(last - first, 1.5), Color { a: 0.55 * alpha, ..Color::WHITE });
            }
            for t in times {
                diamond(f, Point::new(self.view.x(clip.start + t), y), KEY_R, Color { a: alpha, ..if selected { Color::WHITE } else { TEXT_2 } });
            }
        }

        // Label chip (text clips show their content instead).
        if r.width > 34.0 && !matches!(clip.content, Content::Text(_)) {
            let missing = clip.asset().is_some_and(|a| self.missing.contains(&a));
            let label = if missing {
                format!("Missing · {}", clip_label(self.project, clip))
            } else if clip.disabled {
                format!("Off · {}", clip_label(self.project, clip))
            } else {
                clip_label(self.project, clip)
            };
            let font = if label.is_ascii() { INTER_SEMIBOLD } else { HIND_SEMIBOLD };
            let area = Rectangle::new(Point::new(r.x + 4.0, r.y + 3.0), Size::new((r.width - 8.0).max(0.0), 18.0));
            let Some(area) = on_canvas(area, f.size()) else { return };
            f.with_clip(area, |f| {
                f.fill_text(Text {
                    content: label,
                    position: Point::new(r.x.max(0.0) + 8.0, r.y + 12.0),
                    color: if missing { DANGER } else { Color::WHITE },
                    size: 11.0.into(),
                    font,
                    align_y: alignment::Vertical::Center,
                    ..Text::default()
                });
            });
        }

        // A small link in the corner of grouped clips.
        if clip.group.is_some() && r.width > 64.0 {
            let spot = Rectangle::new(Point::new(r.x + r.width - 20.0, r.y + 3.0), Size::new(16.0, 16.0));
            if let Some(visible) = on_canvas(spot, f.size()) {
                f.with_clip(visible, |f| {
                    f.fill_text(Text {
                        content: code::LINK.to_string(),
                        position: Point::new(spot.x + 8.0, spot.y + 8.0),
                        color: Color { a: if dim { 0.4 } else { 0.85 }, ..Color::WHITE },
                        size: 11.0.into(),
                        font: ICONS,
                        align_x: alignment::Horizontal::Center.into(),
                        align_y: alignment::Vertical::Center,
                        ..Text::default()
                    });
                });
            }
        }

        let (line, width) = match outline {
            Outline::Selected => (ACCENT, 2.0),
            Outline::Linked => (Color { a: 0.75, ..ACCENT }, 1.5),
            Outline::Plain => (Color { a: 0.55, ..color }, 1.0),
        };
        f.stroke(&body, Stroke::default().with_color(line).with_width(width));
        if selected {
            let glow = Path::rounded_rectangle(Point::new(r.x - 2.0, r.y - 2.0), Size::new(r.width + 4.0, r.height + 4.0), (radius + 2.0).into());
            f.stroke(&glow, Stroke::default().with_color(Color { a: 0.4, ..ACCENT }).with_width(2.0));
        }
    }
}

/// The part of `r` inside a canvas of `size`, if any.
fn on_canvas(r: Rectangle, size: Size) -> Option<Rectangle> {
    r.intersection(&Rectangle::new(Point::ORIGIN, size))
}

/// Paints the corners outside a rounded rectangle, hiding square image corners.
fn corner_masks(f: &mut Frame, r: Rectangle, radius: f32, bg: Color) {
    if radius < 1.0 {
        return;
    }
    let corners = [
        (Point::new(r.x, r.y), Point::new(r.x + radius, r.y + radius)),
        (Point::new(r.x + r.width, r.y), Point::new(r.x + r.width - radius, r.y + radius)),
        (Point::new(r.x + r.width, r.y + r.height), Point::new(r.x + r.width - radius, r.y + r.height - radius)),
        (Point::new(r.x, r.y + r.height), Point::new(r.x + radius, r.y + r.height - radius)),
    ];
    for (corner, center) in corners {
        let path = Path::new(|b| {
            let dx = (corner.x - center.x).signum();
            let dy = (corner.y - center.y).signum();
            b.move_to(corner);
            // Quarter arc from the horizontal edge to the vertical edge.
            for i in 0..=8 {
                let a = std::f32::consts::FRAC_PI_2 * (1.0 - i as f32 / 8.0);
                b.line_to(Point::new(center.x + dx * radius * a.cos(), center.y + dy * radius * a.sin()));
            }
            b.close();
        });
        f.fill(&path, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::TextLayer;

    /// Presses on a clip, drags it to (x, y) on the lanes and lets go: the message that produces.
    fn drag_clip(project: &Project, clip: &Clip, to: Point) -> Option<Message> {
        let (media, missing, cache) = (MediaCache::default(), std::collections::HashSet::new(), Caches::default());
        let lanes = Lanes {
            project, anim: None, media: &media, selection: &[], playhead: 0.0, view: View { scroll: 0.0, zoom: 100.0 }, snapping: false, zoom_edit: None, range: None, range_mode: false,
            dnd: None, dnd_effect: None, missing: &missing, cache: &cache, overflow: false, vscroll: 0.0,
        };
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(1200.0, 300.0));
        let (_, _, top, h) = lanes.row_of(clip.track)?;
        let grab = Point::new(lanes.view.x(clip.start) + 20.0, top + h / 2.0);
        let mut state = LanesState::default();
        let mut send = |event: mouse::Event, at: Point| {
            canvas::Program::update(&lanes, &mut state, &canvas::Event::Mouse(event), bounds, mouse::Cursor::Available(at)).and_then(|a| a.into_inner().0)
        };
        send(mouse::Event::ButtonPressed(mouse::Button::Left), grab);
        send(mouse::Event::CursorMoved { position: Point::new(to.x, to.y) }, to);
        send(mouse::Event::ButtonReleased(mouse::Button::Left), to)
    }

    #[test]
    fn dropping_a_clip_above_the_top_track_puts_it_on_a_new_one() {
        let mut p = Project::new("t", Default::default());
        let bottom = p.tracks.iter().filter(|t| t.kind == TrackKind::Visual).last().unwrap().id;
        let text = p.add_text_clip(Some(bottom), 1.0, 2.0, TextLayer::default()).unwrap();
        let clip = p.clip(text).unwrap().clone();
        // Above the first lane (or up into the ruler): a new top track, at the dragged time.
        match drag_clip(&p, &clip, Point::new(300.0, -15.0)) {
            Some(Message::MoveClipToNewTrack { clip, start }) => assert!(clip == text && (start - 2.8).abs() < 0.01, "start {start}"),
            other => panic!("expected a move to a new track, got {other:?}"),
        }
        // Onto the first lane: that track.
        let (first, _, top, h) = rows(&p, 0.0)[0];
        match drag_clip(&p, &clip, Point::new(300.0, top + h / 2.0)) {
            Some(Message::MoveClip { track, .. }) => assert_eq!(track, first),
            other => panic!("expected a move to the top track, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::clip::TextLayer;

    /// Runs mouse events through the lanes and collects the messages they publish.
    fn run(project: &Project, range: Option<(f64, f64)>, range_mode: bool, events: &[(mouse::Event, Point)]) -> Vec<Message> {
        let (media, missing, cache) = (MediaCache::default(), std::collections::HashSet::new(), Caches::default());
        let lanes = Lanes {
            project, anim: None, media: &media, selection: &[], playhead: 0.0, view: View { scroll: 0.0, zoom: 100.0 }, snapping: true, zoom_edit: None, range, range_mode,
            dnd: None, dnd_effect: None, missing: &missing, cache: &cache, overflow: false, vscroll: 0.0,
        };
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(1200.0, 300.0));
        let mut state = LanesState::default();
        events
            .iter()
            .filter_map(|(e, at)| canvas::Program::update(&lanes, &mut state, &canvas::Event::Mouse(*e), bounds, mouse::Cursor::Available(*at)).and_then(|a| a.into_inner().0))
            .collect()
    }

    fn drag(from: Point, to: Point) -> Vec<(mouse::Event, Point)> {
        vec![(mouse::Event::ButtonPressed(mouse::Button::Left), from), (mouse::Event::CursorMoved { position: to }, to), (mouse::Event::ButtonReleased(mouse::Button::Left), to)]
    }

    #[test]
    fn range_mode_draws_and_adjusts_the_range_by_dragging() {
        let mut p = Project::new("t", Default::default());
        let text = p.add_text_clip(None, 1.0, 4.0, TextLayer::default()).unwrap();
        let (_, _, top, h) = rows(&p, 0.0).into_iter().find(|r| r.0 == p.clip(text).unwrap().track).unwrap();
        let y = top + h / 2.0;
        // Press on the clip at 2 s, drag to 3.5 s: the clip is picked and the range follows.
        let msgs = run(&p, None, true, &drag(Point::new(200.0, y), Point::new(350.0, y)));
        assert!(matches!(msgs[0], Message::RangePress { clip: Some(c), .. } if c == text), "{msgs:?}");
        assert!(matches!(msgs[1], Message::SetRange(Some((a, b))) if (a - 2.0).abs() < 1e-6 && (b - 3.5).abs() < 1e-6), "{msgs:?}");
        // Dragging leftwards works too, and snaps to the clip's start.
        let msgs = run(&p, None, true, &drag(Point::new(300.0, y), Point::new(104.0, y)));
        assert!(matches!(msgs[1], Message::SetRange(Some((a, b))) if a == 1.0 && (b - 3.0).abs() < 1e-6), "{msgs:?}");
        // Grabbing an edge moves just that edge (no new range, the selection stays).
        let msgs = run(&p, Some((2.0, 3.5)), true, &drag(Point::new(351.0, y), Point::new(420.0, y)));
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(matches!(msgs[0], Message::SetRange(Some((a, b))) if a == 2.0 && (b - 4.2).abs() < 1e-6), "{msgs:?}");
        // Without the tool, the same drag moves the clip as before.
        let msgs = run(&p, None, false, &drag(Point::new(200.0, y), Point::new(350.0, y)));
        assert!(matches!(msgs.last(), Some(Message::MoveClip { .. })), "{msgs:?}");
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;
    use crate::clip::{TextLayer, TransitionKind};
    use crate::editor::library::Effect;

    fn run(project: &Project, dnd_effect: Option<Effect>, events: &[(mouse::Event, Point)]) -> Vec<Message> {
        let (media, missing, cache) = (MediaCache::default(), std::collections::HashSet::new(), Caches::default());
        let lanes = Lanes {
            project, anim: None, media: &media, selection: &[], playhead: 0.0, view: View { scroll: 0.0, zoom: 100.0 }, snapping: false, zoom_edit: None, range: None, range_mode: false,
            dnd: None, dnd_effect, missing: &missing, cache: &cache, overflow: false, vscroll: 0.0,
        };
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(1200.0, 300.0));
        let mut state = LanesState::default();
        events.iter().filter_map(|(e, at)| canvas::Program::update(&lanes, &mut state, &canvas::Event::Mouse(*e), bounds, mouse::Cursor::Available(*at)).and_then(|a| a.into_inner().0)).collect()
    }

    fn drag(from: Point, to: Point) -> Vec<(mouse::Event, Point)> {
        vec![(mouse::Event::ButtonPressed(mouse::Button::Left), from), (mouse::Event::CursorMoved { position: to }, to), (mouse::Event::ButtonReleased(mouse::Button::Left), to)]
    }

    /// Two clips touching at 4 s, with a 1 s crossfade on the cut between them (3.5 s to 4.5 s). Returns (project, second clip id, the row's mid-y).
    fn two_clips_with_a_transition() -> (Project, Id, f32) {
        let mut p = Project::new("t", Default::default());
        let track = p.tracks.iter().find(|t| t.kind == TrackKind::Visual).unwrap().id;
        p.add_text_clip(Some(track), 0.0, 4.0, TextLayer::default()).unwrap();
        let second = p.add_text_clip(Some(track), 4.0, 4.0, TextLayer::default()).unwrap();
        p.set_transition(second, Some(TransitionKind::Crossfade), 1.0).unwrap();
        let (_, _, top, h) = rows(&p, 0.0).into_iter().find(|r| r.0 == track).unwrap();
        (p, second, top + h / 2.0)
    }

    #[test]
    fn a_keyframe_s_diamond_is_clicked_dragged_and_right_clicked() {
        let mut p = Project::new("t", Default::default());
        let track = p.tracks.iter().find(|t| t.kind == TrackKind::Visual).unwrap().id;
        let clip = p.add_text_clip(Some(track), 1.0, 6.0, TextLayer::default()).unwrap();
        p.key_all(clip, 2.0).unwrap();
        let (_, _, top, h) = rows(&p, 0.0).into_iter().find(|r| r.0 == track).unwrap();
        let view = View { scroll: 0.0, zoom: 100.0 };
        let (x, y) = (view.x(3.0), key_row_y(top, h));
        // A click on it: the clip is selected and the playhead goes there; nothing moves.
        let msgs = run(&p, None, &[(mouse::Event::ButtonPressed(mouse::Button::Left), Point::new(x + 2.0, y)), (mouse::Event::ButtonReleased(mouse::Button::Left), Point::new(x + 2.0, y))]);
        assert!(matches!(msgs.as_slice(), [Message::KeyMenu { clip: c, at }] if *c == clip && *at == 2.0), "{msgs:?}");
        // A drag: the keyframe moves (1 s to the right is 3 s on the clip's clock), not the clip.
        let msgs = run(&p, None, &drag(Point::new(x, y), Point::new(x + 100.0, y)));
        assert!(matches!(msgs.last(), Some(Message::MoveKeys { clip: c, from, to }) if *c == clip && *from == 2.0 && (*to - 3.0).abs() < 0.02), "{msgs:?}");
        // Dragged past the clip's end, it stops there.
        let msgs = run(&p, None, &drag(Point::new(x, y), Point::new(x + 900.0, y)));
        assert!(matches!(msgs.last(), Some(Message::MoveKeys { to, .. }) if *to == 6.0), "{msgs:?}");
        // A right-click: the same, so the menu can delete it.
        let msgs = run(&p, None, &[(mouse::Event::ButtonPressed(mouse::Button::Right), Point::new(x, y))]);
        assert!(matches!(msgs.as_slice(), [Message::KeyMenu { at, .. }] if *at == 2.0), "{msgs:?}");
        // Away from the diamond the clip itself is picked up as before.
        let msgs = run(&p, None, &drag(Point::new(x + 60.0, y), Point::new(x + 160.0, y)));
        assert!(matches!(msgs.last(), Some(Message::MoveClip { clip: c, .. }) if *c == clip), "{msgs:?}");
    }

    #[test]
    fn dragging_a_transition_s_leading_edge_resizes_it() {
        let (p, second, y) = two_clips_with_a_transition();
        let view = View { scroll: 0.0, zoom: 100.0 };
        // The 1 s transition is centred on the cut at 4 s: its leading (left) edge is at 3.5 s.
        // Dragging that edge half a second further left makes it 2 s long; the cut never moves.
        let edge_x = view.x(p.clip(second).unwrap().start - 0.5);
        let msgs = run(&p, None, &drag(Point::new(edge_x, y), Point::new(edge_x - 50.0, y)));
        match msgs.last() {
            Some(Message::Edit(id, crate::editor::inspector::Edit::TransitionDuration(d))) => {
                assert_eq!(*id, second);
                assert!((*d - 2.0).abs() < 0.01, "{d}");
            }
            other => panic!("expected a transition duration edit, got {other:?}"),
        }
    }

    #[test]
    fn dropping_a_transition_near_the_seam_targets_the_incoming_clip_not_whatever_is_under_it() {
        let mut p = Project::new("t", Default::default());
        let track = p.tracks.iter().find(|t| t.kind == TrackKind::Visual).unwrap().id;
        p.add_text_clip(Some(track), 0.0, 4.0, TextLayer::default()).unwrap();
        let second = p.add_text_clip(Some(track), 4.0, 4.0, TextLayer::default()).unwrap();
        let (_, _, top, h) = rows(&p, 0.0).into_iter().find(|r| r.0 == track).unwrap();
        let y = top + h / 2.0;
        let view = View { scroll: 0.0, zoom: 100.0 };
        let seam_x = view.x(4.0);
        let effect = Effect::Transition(TransitionKind::Crossfade);
        // Dropped a little into the first clip's tail, not on the second clip's own body.
        let msgs = run(&p, Some(effect), &[(mouse::Event::ButtonReleased(mouse::Button::Left), Point::new(seam_x - 10.0, y))]);
        match msgs.as_slice() {
            [Message::DropEffect { effect: e, target: crate::editor::DropTarget::Timeline { clip, .. } }] => {
                assert_eq!(*e, effect);
                assert_eq!(*clip, Some(second));
            }
            other => panic!("expected a transition drop targeting the incoming clip, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod select_tests {
    use super::*;
    use crate::clip::TextLayer;

    fn run(project: &Project, selection: &[Id], modifiers: keyboard::Modifiers, events: &[(mouse::Event, Point)]) -> Vec<Message> {
        let (media, missing, cache) = (MediaCache::default(), std::collections::HashSet::new(), Caches::default());
        let lanes = Lanes {
            project, anim: None, media: &media, selection, playhead: 0.0, view: View { scroll: 0.0, zoom: 100.0 }, snapping: false, zoom_edit: None, range: None, range_mode: false,
            dnd: None, dnd_effect: None, missing: &missing, cache: &cache, overflow: false, vscroll: 0.0,
        };
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(1200.0, 300.0));
        let mut state = LanesState { modifiers, ..Default::default() };
        events.iter().filter_map(|(e, at)| canvas::Program::update(&lanes, &mut state, &canvas::Event::Mouse(*e), bounds, mouse::Cursor::Available(*at)).and_then(|a| a.into_inner().0)).collect()
    }

    /// Two clips on the top track (1–3 s and 5–7 s) and one on the track below (2–4 s).
    fn three_clips() -> (Project, [Id; 3], [f32; 2]) {
        let mut p = Project::new("t", Default::default());
        let (top, below) = (p.tracks[0].id, p.tracks[1].id);
        let a = p.add_text_clip(Some(top), 1.0, 2.0, TextLayer::default()).unwrap();
        let b = p.add_text_clip(Some(top), 5.0, 2.0, TextLayer::default()).unwrap();
        let c = p.add_text_clip(Some(below), 2.0, 2.0, TextLayer::default()).unwrap();
        let r = rows(&p, 0.0);
        (p, [a, b, c], [r[0].2 + r[0].3 / 2.0, r[1].2 + r[1].3 / 2.0])
    }

    #[test]
    fn dragging_over_empty_space_selects_the_clips_in_the_box() {
        let (p, [a, b, c], [y_top, y_below]) = three_clips();
        let press = mouse::Event::ButtonPressed(mouse::Button::Left);
        let release = mouse::Event::ButtonReleased(mouse::Button::Left);
        let moved = |x: f32, y: f32| (mouse::Event::CursorMoved { position: Point::new(x, y) }, Point::new(x, y));
        // From the gap between the two top clips (4 s) leftwards and down over both tracks.
        let msgs = run(&p, &[], keyboard::Modifiers::empty(), &[(press, Point::new(400.0, y_top)), moved(250.0, y_top), moved(250.0, y_below), (release, Point::new(250.0, y_below))]);
        assert!(matches!(msgs[0], Message::Select(None, false)), "a press on empty space deselects first: {msgs:?}");
        assert!(matches!(&msgs[1], Message::SelectBox(ids) if *ids == vec![a]), "{msgs:?}");
        assert!(matches!(&msgs[2], Message::SelectBox(ids) if *ids == vec![a, c]), "{msgs:?}");
        assert_eq!(msgs.len(), 3, "nothing more on release: {msgs:?}");
        // With Shift the earlier selection stays and the box adds to it.
        let msgs = run(&p, &[b], keyboard::Modifiers::SHIFT, &[(press, Point::new(400.0, y_top)), moved(250.0, y_top)]);
        assert!(matches!(msgs.as_slice(), [Message::SelectBox(ids)] if *ids == vec![b, a]), "{msgs:?}");
        // A plain click on empty space only deselects.
        let msgs = run(&p, &[b], keyboard::Modifiers::empty(), &[(press, Point::new(400.0, y_top)), (release, Point::new(400.0, y_top))]);
        assert!(matches!(msgs.as_slice(), [Message::Select(None, false)]), "{msgs:?}");
    }

    #[test]
    fn a_right_click_reports_the_clip_the_track_and_the_time() {
        let (p, [a, ..], [y_top, _]) = three_clips();
        let right = mouse::Event::ButtonPressed(mouse::Button::Right);
        let msgs = run(&p, &[], keyboard::Modifiers::empty(), &[(right, Point::new(150.0, y_top))]);
        assert!(matches!(msgs.as_slice(), [Message::LaneContext { clip: Some(c), track: Some(t), t: at }] if *c == a && *t == p.tracks[0].id && (*at - 1.5).abs() < 1e-6), "{msgs:?}");
        // In a gap: no clip, but the track and time are known (for "Close this gap").
        let msgs = run(&p, &[], keyboard::Modifiers::empty(), &[(right, Point::new(400.0, y_top))]);
        assert!(matches!(msgs.as_slice(), [Message::LaneContext { clip: None, track: Some(_), .. }]), "{msgs:?}");
    }
}

/// Half the width of a keyframe's diamond on the timeline, in pixels.
const KEY_R: f32 = 5.0;

/// Where a clip's keyframe diamonds sit: across its middle (zoom bands use the bottom, labels the top).
fn key_row_y(top: f32, height: f32) -> f32 {
    top + height / 2.0
}

/// A keyframe's diamond, outlined dark so it reads over any thumbnail.
fn diamond(f: &mut Frame, at: Point, r: f32, color: Color) {
    let path = Path::new(|b| {
        b.move_to(Point::new(at.x, at.y - r));
        b.line_to(Point::new(at.x + r, at.y));
        b.line_to(Point::new(at.x, at.y + r));
        b.line_to(Point::new(at.x - r, at.y));
        b.close();
    });
    f.fill(&path, color);
    f.stroke(&path, Stroke::default().with_color(Color { a: 0.7, ..Color::BLACK }).with_width(1.0));
}
