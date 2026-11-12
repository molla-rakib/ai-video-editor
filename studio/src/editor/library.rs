//! The library panel: media, text, effects and sounds.

use iced::widget::{button, column, container, image, mouse_area, row, space, stack, text};
use iced::{Alignment, Color, Element, Fill, Length, mouse};
use iced_aw::ContextMenu;

use super::{Editor, LibraryTab, Message, fmt_duration};
use crate::clip::{CalloutIcon, MotionKind, Rgba, TransitionKind};
use crate::model::MediaKind;
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::widgets::{self, MenuItem};
use crate::ui::style;

impl Editor {
    pub(super) fn library(&self) -> Element<'_, Message> {
        match self.library_tab {
            LibraryTab::Media => self.media_library(),
            LibraryTab::Text => {
                let card = |title: &'static str, sub: &'static str, font: iced::Font, size: f32| {
                    button(column![text(title).font(font).size(size), text(sub).size(T_CAPTION).color(TEXT_3)].spacing(4))
                        .width(Fill)
                        .padding([12, 12])
                        .style(style::choice_card(false))
                        .on_press(Message::AddText)
                };
                widgets::scroll(
                    column![
                        widgets::section("Add text"),
                        card("Headline", "Big bold title", INTER_BOLD, 20.0),
                        card("Subtitle", "Supporting line", INTER_MEDIUM, 15.0),
                        card("শিরোনাম", "Bangla headline · Hind Siliguri", HIND_SEMIBOLD, 19.0),
                        text("Click to add at the playhead. Press T anytime.").size(T_CAPTION).color(TEXT_4),
                        space().height(6),
                        widgets::section("Shapes"),
                        shape_grid(),
                        space().height(6),
                        widgets::section("Callouts"),
                        text("A glowing icon accent that pops in — use sparingly, at a genuinely important moment.").size(T_CAPTION).color(TEXT_4),
                        callout_grid(),
                        space().height(6),
                        widgets::section("Counter"),
                        text("A number that counts up or down — great for before/after metrics.").size(T_CAPTION).color(TEXT_4),
                        button(column![text("0 → 100").font(INTER_BOLD).size(20.0), text("Counts up over 1s").size(T_CAPTION).color(TEXT_3)].spacing(4))
                            .width(Fill)
                            .padding([12, 12])
                            .style(style::choice_card(false))
                            .on_press(Message::AddCounter),
                    ]
                    .spacing(S2)
                    .padding([8, 10]),
                )
                .height(Fill)
                .into()
            }
            LibraryTab::Transitions => self.effects_tab(),
            LibraryTab::Animation => self.animation_tab(),
            LibraryTab::Sounds => self.sounds_tab(),
        }
    }

    fn media_library(&self) -> Element<'_, Message> {
        let project = &self.doc.project;
        if project.assets.is_empty() && self.importing.is_empty() {
            return widgets::empty_state(
                code::UPLOAD,
                "Bring in your media",
                "Drop screen recordings, face-cam videos, voiceovers, images or logos here.",
                Some(
                    column![
                        widgets::primary_button(Some(code::PLUS), "Import media", Some(Message::Import)),
                        widgets::secondary_button(Some(code::VIDEO), "Record camera", Some(Message::OpenRecord)),
                        widgets::secondary_button(Some(code::MONITOR), "Record screen", Some(Message::OpenScreenRecord)),
                    ]
                    .spacing(S2)
                    .align_x(Alignment::Center)
                    .into(),
                ),
            );
        }
        fn grid(cards: Vec<Element<'_, Message>>) -> Element<'_, Message> {
            iced::widget::grid(cards).fluid(190).spacing(6).height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink)).into()
        }
        // Grouped by kind, each group in import order; files still being read come first.
        let mut groups = column![].spacing(S3);
        if !self.importing.is_empty() {
            let cards = self
                .importing
                .iter()
                .map(|path| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    container(
                        column![
                            container(iced_aw::Spinner::new().width(Length::Fixed(20.0)).height(Length::Fixed(20.0)).circle_radius(2.0)).center_x(Fill).height(76).align_y(Alignment::Center).style(style::inset),
                            text(name).size(T_CAPTION).color(TEXT_3).wrapping(text::Wrapping::None),
                        ]
                        .spacing(6),
                    )
                    .padding(6)
                    .into()
                })
                .collect();
            groups = groups.push(column![media_group_header("Importing", self.importing.len()), grid(cards)].spacing(4));
        }
        for (kind, title) in [(MediaKind::Video, "Videos"), (MediaKind::Image, "Images"), (MediaKind::Audio, "Audio")] {
            let cards: Vec<Element<'_, Message>> = project.assets.iter().filter(|a| a.kind == kind).map(|a| self.media_card(a)).collect();
            if !cards.is_empty() {
                groups = groups.push(column![media_group_header(title, cards.len()), grid(cards)].spacing(4));
            }
        }
        widgets::scroll(container(groups).padding([8, 8])).height(Fill).into()
    }

    fn media_card<'a>(&'a self, a: &'a crate::model::Asset) -> Element<'a, Message> {
        let selected = self.library_selected == Some(a.id);
        let thumb: Element<'a, Message> = match self.media.get(a.id).and_then(|m| m.cover.clone()) {
            Some(h) => image(h).width(Fill).height(76).content_fit(iced::ContentFit::Cover).border_radius(6.0).into(),
            None => {
                let glyph = match a.kind {
                    MediaKind::Audio => code::AUDIO_WAVEFORM,
                    MediaKind::Image => code::IMAGE,
                    MediaKind::Video => code::FILM,
                };
                let color = if a.kind == MediaKind::Audio { clip::AUDIO } else { TEXT_3 };
                container(tinted(glyph, 22, color)).center_x(Fill).height(76).align_y(Alignment::Center).style(style::inset).into()
            }
        };
        let missing = self.missing.contains(&a.id);
        let badge = if missing { "Missing".to_string() } else if a.kind == MediaKind::Image { "IMG".to_string() } else { fmt_duration(a.duration) };
        let thumb = stack![
            thumb,
            container(container(text(badge).font(INTER_SEMIBOLD).size(10)).padding([1, 5]).style(move |_| container::Style {
                background: Some(if missing { DANGER } else { Color { a: 0.7, ..Color::BLACK } }.into()),
                text_color: Some(if missing { ON_ACCENT } else { Color::WHITE }),
                border: iced::Border { radius: 4.0.into(), ..Default::default() },
                ..Default::default()
            }))
            .align_right(Fill)
            .align_bottom(76)
            .padding(5),
        ];
        let name_font = if a.name.is_ascii() { INTER_MEDIUM } else { HIND };
        // Long file names are shortened in the middle to fit the card (the start and the
        // extension stay readable); the full name shows on hover.
        let name: &'a str = &a.name;
        let fitted = iced::widget::responsive(move |size| {
            let fits = ((size.width / NAME_CHAR_W).floor() as usize).max(6);
            container(text(fit_name(name, fits)).font(name_font).size(T_CAPTION).wrapping(text::Wrapping::None)).width(Fill).clip(true).into()
        });
        let name_row = widgets::tip(container(fitted).width(Fill).height(16), name, None, iced::widget::tooltip::Position::Bottom);
        let card = container(column![thumb, name_row].spacing(6))
            .padding(5)
            .width(Fill)
            .style(move |t| {
                let mut s = style::card(t);
                if selected {
                    s.border = iced::Border { color: ACCENT, width: 1.5, radius: R_LG.into() };
                    s.background = Some(Color { a: 0.08, ..ACCENT }.into());
                } else {
                    s.background = None;
                    s.border.color = Color::TRANSPARENT;
                }
                s
            });
        let id = a.id;
        let area = mouse_area(card).on_press(Message::LibraryPress(id)).on_double_click(Message::LibraryAdd(id)).interaction(mouse::Interaction::Grab);
        ContextMenu::new(area, move || {
            widgets::menu(
                vec![
                    MenuItem::new(code::PLUS, "Add to timeline", Message::LibraryAdd(id)).shortcut("Double-click"),
                    MenuItem::new(code::FOLDER_OPEN, if missing { "Locate file…" } else { "Replace file…" }, Message::Relink(id)),
                    MenuItem::Separator,
                    MenuItem::new(code::TRASH_2, "Remove from project", Message::LibraryRemove(id)).danger(),
                ],
                220.0,
            )
        })
        .style(|_, _| iced_aw::style::context_menu::Style { background: Color::TRANSPARENT.into() })
        .into()
    }


    fn effects_tab(&self) -> Element<'_, Message> {
        let groups: Vec<EffectGroup> = catalog().into_iter().filter(|g| !is_animation_group(g.title)).collect();
        self.effect_groups_view(groups)
    }

    fn animation_tab(&self) -> Element<'_, Message> {
        let groups: Vec<EffectGroup> = catalog().into_iter().filter(|g| is_animation_group(g.title)).collect();
        self.effect_groups_view(groups)
    }

    /// Shared by the Effects and Animation tabs: a Favorites shelf on top, then every group
    /// (from the given subset of `catalog()`) as a collapsible card grid.
    fn effect_groups_view(&self, groups: Vec<EffectGroup>) -> Element<'_, Message> {
        let has_selection = !self.selection.is_empty();
        // Click applies to the selection; drag drops onto any clip (timeline or preview).
        // The star in the corner keeps an effect in Favorites.
        let card = |glyph: char, label: &'static str, caption: Option<&'static str>, effect: Effect| -> Element<'_, Message> {
            let hovered = self.effect_hover == Some(effect);
            let favorite = self.favorite_effects.iter().any(|k| *k == effect.key());
            let glyph_row = row![space().width(5), icon(glyph, 18).color(ACCENT_HOVER)].align_y(Alignment::Center);
            let mut info = column![container(glyph_row).center_x(Fill).height(30).align_y(Alignment::Center), text(label).font(INTER_MEDIUM).size(T_SMALL)].spacing(4).align_x(Alignment::Center);
            if let Some(c) = caption {
                info = info.push(text(c).size(T_CAPTION).color(TEXT_3));
            }
            let body = container(info).width(Fill).padding([10, 6]).style(move |t| effect_card(t, hovered));
            let star = button(icon(code::STAR, 11).color(if favorite { ACCENT } else if hovered { TEXT_3 } else { TEXT_4 }))
                .padding(4)
                .style(style::ghost)
                .on_press(Message::ToggleFavoriteEffect(effect));
            let star = widgets::tip(star, if favorite { "Remove from favorites" } else { "Add to favorites" }, None, iced::widget::tooltip::Position::Top);
            let card = stack![body, container(star).align_right(Fill).padding(6)];
            mouse_area(card)
                .on_press(Message::EffectPress(effect))
                .on_release(Message::EffectClick(effect))
                .on_enter(Message::EffectHover(Some(effect)))
                .on_exit(Message::EffectHover(None))
                .interaction(mouse::Interaction::Grab)
                .into()
        };
        fn grid(items: Vec<Element<'_, Message>>) -> iced::widget::Grid<'_, Message> {
            iced::widget::grid(items).fluid(110).spacing(6).height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink))
        }
        let hint = if has_selection { "Click to apply to the selected clips, or drag onto any clip." } else { "Drag onto a clip on the timeline or the preview." };
        let mut col = column![text(hint).size(T_CAPTION).color(TEXT_3)].spacing(S3);

        // Favorites: always open, on top.
        let favorites: Vec<Element<'_, Message>> = self
            .favorite_effects
            .iter()
            .filter_map(|key| groups.iter().find_map(|g| g.items.iter().find(|i| i.2.key() == *key).map(|i| card(i.0, i.1, Some(g.title), i.2))))
            .collect();
        let fav_head = row![tinted(code::STAR, 12, ACCENT), widgets::section("Favorites")].spacing(6).align_y(Alignment::Center);
        let fav_body: Element<'_, Message> = if favorites.is_empty() {
            container(text("Star an effect to keep it here.").size(T_CAPTION).color(TEXT_4)).padding([10, 12]).width(Fill).style(style::inset).into()
        } else {
            grid(favorites).into()
        };
        col = col.push(column![fav_head, fav_body].spacing(S2));

        // Every other group starts collapsed; its title opens it.
        for g in &groups {
            let open = self.effects_open.contains(g.title);
            let head = button(
                row![
                    icon(if open { code::CHEVRON_DOWN } else { code::CHEVRON_RIGHT }, 12).color(TEXT_3),
                    text(g.title).font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_2),
                    space().width(Fill),
                    text(g.items.len()).size(T_CAPTION).color(TEXT_4),
                ]
                .spacing(7)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding([8, 10])
            .style(style::ghost)
            .on_press(Message::ToggleEffectGroup(g.title));
            let mut group = column![head].spacing(S2);
            if open {
                let mut body = column![].spacing(S2);
                if g.title == MARKERS {
                    body = body.push(self.marker_swatches());
                }
                body = body.push(grid(g.items.iter().map(|&(glyph, label, effect)| card(glyph, label, None, effect)).collect()));
                if g.title == MARKERS {
                    let timeline_marker = widgets::secondary_button(Some(code::FLAG), "Add timeline marker", Some(Message::AddMarker)).width(Fill);
                    body = body.push(widgets::tip(timeline_marker, "A colored flag on the ruler (or a span over the marked range)", Some("Shift+M"), iced::widget::tooltip::Position::Top));
                }
                group = group.push(container(body).padding(iced::Padding { top: 0.0, right: 10.0, bottom: 10.0, left: 10.0 }));
            }
            col = col.push(container(group).width(Fill).style(style::inset));
        }
        widgets::scroll(col.padding([8, 10])).height(Fill).into()
    }

    /// The color new markers get (each one can be recolored in the inspector later).
    fn marker_swatches(&self) -> Element<'_, Message> {
        row(MARKER_COLORS.iter().map(|&hex| {
            let c = Rgba::hex(hex);
            let selected = c.to_hex() == self.marker_color.to_hex();
            button(space().width(18).height(18))
                .padding(0)
                .style(move |_, status| button::Style {
                    background: Some(Color::from_rgb(c.0, c.1, c.2).into()),
                    border: iced::Border {
                        color: if selected { Color::WHITE } else if matches!(status, button::Status::Hovered) { LINE_STRONG } else { LINE },
                        width: if selected { 2.0 } else { 1.0 },
                        radius: 10.0.into(),
                    },
                    ..Default::default()
                })
                .on_press(Message::MarkerColor(c))
                .into()
        }))
        .spacing(6)
        .padding([0, 10])
        .into()
    }

    fn sounds_tab(&self) -> Element<'_, Message> {
        let rows = crate::sfx::SOUNDS.iter().map(|s| {
            let playing = self.sound_preview.as_ref().is_some_and(|(id, _, _)| *id == s.id);
            row![
                widgets::tip(
                    button(container(icon(if playing { code::AUDIO_LINES } else { code::PLAY }, 13)).center(30)).width(30).height(30).padding(0).style(style::toggle(playing)).on_press(Message::PreviewSound(s.id)),
                    "Listen",
                    None,
                    iced::widget::tooltip::Position::Top,
                ),
                column![text(s.name).font(INTER_MEDIUM).size(T_BODY), text(s.hint).size(T_CAPTION).color(TEXT_3).wrapping(text::Wrapping::None)].spacing(1).width(Fill),
                widgets::tip(widgets::icon_button(code::PLUS, Some(Message::AddSound(s.id))), "Add at playhead", None, iced::widget::tooltip::Position::Left),
            ]
            .spacing(S3)
            .align_y(Alignment::Center)
            .padding([6, 8])
            .into()
        });
        widgets::scroll(
            column![
                text("Original sounds, free to use anywhere. They go on an audio track at the playhead.").size(T_CAPTION).color(TEXT_3),
                column(rows).spacing(2),
            ]
            .spacing(S3)
            .padding([8, 10]),
        )
        .height(Fill)
        .into()
    }
}

/// Something from the Effects tab that can be clicked onto the selection or dragged onto a clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    Transition(TransitionKind),
    Intro(MotionKind),
    Outro(MotionKind),
    Blur,
    /// Isolates (or, inverted, hides) part of the picture inside a box.
    Mask,
    Highlight(Highlight),
    /// Rounded corners, a border or a drop shadow on a picture.
    Frame(FrameStyle),
    /// A zoom (or a pan, right after another zoom) on a video or image.
    Zoom,
    /// A named, one-click camera move (push in / pull back), tied to the playhead.
    Camera(CameraPreset),
}

/// A ready-made camera move, built from the playhead to the end of the clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraPreset {
    /// Slow, continuous zoom in from normal to a closer framing, held to the end.
    PushIn,
    /// Starts punched in and pulls back to reveal the whole frame, held to the end.
    PullBack,
}

impl CameraPreset {
    pub fn label(self) -> &'static str {
        match self {
            CameraPreset::PushIn => "Push in",
            CameraPreset::PullBack => "Pull back",
        }
    }
}

/// The frame styles a picture can get from the Effects tab (adjusted in the inspector).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStyle {
    Corners,
    Border,
    Shadow,
}

/// The marker group of the Effects tab (it also has the color swatches).
pub const MARKERS: &str = "Markers";

/// The `catalog()` groups shown on the Animation tab instead of the Effects tab.
fn is_animation_group(title: &str) -> bool {
    title == "Animate in" || title == "Animate out" || title == "Zoom & pan"
}

/// One group of the Effects tab: its title and its effects (icon, name, effect).
pub struct EffectGroup {
    pub title: &'static str,
    pub items: Vec<(char, &'static str, Effect)>,
}

/// Everything the Effects tab offers, in order.
pub fn catalog() -> Vec<EffectGroup> {
    let motions = [
        (code::SUN, "Fade", MotionKind::Fade),
        (code::CHEVRON_UP, "Slide up", MotionKind::SlideUp),
        (code::CHEVRON_DOWN, "Slide down", MotionKind::SlideDown),
        (code::CHEVRON_LEFT, "Slide left", MotionKind::SlideLeft),
        (code::CHEVRON_RIGHT, "Slide right", MotionKind::SlideRight),
        (code::SPARKLE, "Pop", MotionKind::Pop),
        (code::FOCUS, "Zoom in", MotionKind::ZoomIn),
    ];
    vec![
        EffectGroup {
            title: MARKERS,
            items: vec![
                (code::RECTANGLE_HORIZONTAL, "Box", Effect::Highlight(Highlight::Box)),
                (code::CIRCLE, "Circle", Effect::Highlight(Highlight::Circle)),
                (code::HIGHLIGHTER, "Highlight", Effect::Highlight(Highlight::Marker)),
                (code::MINUS, "Underline", Effect::Highlight(Highlight::Underline)),
            ],
        },
        EffectGroup { title: "Hide", items: vec![(code::DROPLETS, "Blur area", Effect::Blur), (code::SCAN_EYE, "Mask area", Effect::Mask)] },
        EffectGroup {
            title: "Frame",
            items: vec![
                (code::SQUARE_DASHED, "Corners", Effect::Frame(FrameStyle::Corners)),
                (code::FRAME, "Border", Effect::Frame(FrameStyle::Border)),
                (code::LAYERS, "Shadow", Effect::Frame(FrameStyle::Shadow)),
            ],
        },
        EffectGroup {
            title: "Zoom & pan",
            items: vec![
                (code::ZOOM_IN, "Zoom", Effect::Zoom),
                (code::MAXIMIZE_2, CameraPreset::PushIn.label(), Effect::Camera(CameraPreset::PushIn)),
                (code::MINIMIZE_2, CameraPreset::PullBack.label(), Effect::Camera(CameraPreset::PullBack)),
            ],
        },
        EffectGroup {
            title: "Transition",
            items: vec![
                (code::BLEND, "Crossfade", Effect::Transition(TransitionKind::Crossfade)),
                (code::ARROW_LEFT_RIGHT, "Slide", Effect::Transition(TransitionKind::SlideLeft)),
                (code::CHEVRON_UP, "Slide up", Effect::Transition(TransitionKind::SlideUp)),
                (code::ZOOM_IN, "Zoom through", Effect::Transition(TransitionKind::ZoomThrough)),
                (code::COLUMNS_3, "Wipe", Effect::Transition(TransitionKind::Wipe)),
                (code::MOVE, "Push", Effect::Transition(TransitionKind::Push)),
                (code::ARROW_RIGHT, "Slide right", Effect::Transition(TransitionKind::SlideRight)),
                (code::ARROW_DOWN, "Slide down", Effect::Transition(TransitionKind::SlideDown)),
                (code::MOON, "Dip to black", Effect::Transition(TransitionKind::DipToBlack)),
            ],
        },
        EffectGroup { title: "Animate in", items: motions.iter().map(|&(g, l, k)| (g, l, Effect::Intro(k))).collect() },
        EffectGroup { title: "Animate out", items: motions.iter().map(|&(g, l, k)| (g, l, Effect::Outro(k))).collect() },
    ]
}

impl Effect {
    /// "Animate out · Pop": the group and the effect, for the drag label and messages.
    pub fn label(self) -> String {
        catalog()
            .iter()
            .find_map(|g| g.items.iter().find(|i| i.2 == self).map(|i| if g.items.len() == 1 || g.title == MARKERS { i.1.to_string() } else { format!("{} · {}", g.title, i.1) }))
            .unwrap_or_default()
    }

    /// A stable name for saving favorites.
    pub fn key(self) -> String {
        match self {
            Effect::Transition(k) => format!("transition:{k:?}"),
            Effect::Intro(k) => format!("in:{k:?}"),
            Effect::Outro(k) => format!("out:{k:?}"),
            Effect::Blur => "blur".into(),
            Effect::Mask => "mask".into(),
            Effect::Highlight(h) => format!("marker:{h:?}"),
            Effect::Frame(f) => format!("frame:{f:?}"),
            Effect::Zoom => "zoom".into(),
            Effect::Camera(p) => format!("camera:{p:?}"),
        }
    }
}

/// On-video markers that point at part of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    Box,
    Circle,
    /// A translucent highlighter stroke.
    Marker,
    Underline,
}

impl Highlight {
    /// The shape, its corner radius, and its clip name.
    pub fn build(self, color: Rgba) -> (crate::clip::ShapeLayer, f64, &'static str) {
        use crate::clip::{ShapeKind, ShapeLayer};
        let mut s = match self {
            Highlight::Box => ShapeLayer::new(ShapeKind::Frame, 520.0, 300.0, color, None),
            Highlight::Circle => ShapeLayer::new(ShapeKind::Ring, 360.0, 360.0, color, None),
            Highlight::Marker => ShapeLayer::new(ShapeKind::Rectangle, 520.0, 90.0, Rgba(color.0, color.1, color.2, 0.4), None),
            Highlight::Underline => ShapeLayer::new(ShapeKind::Rectangle, 520.0, 14.0, color, None),
        };
        s.thickness = 10.0;
        let radius = match self {
            Highlight::Box => 18.0,
            Highlight::Circle => 0.0,
            Highlight::Marker => 10.0,
            Highlight::Underline => 7.0,
        };
        (s, radius, match self {
            Highlight::Box => "Box marker",
            Highlight::Circle => "Circle marker",
            Highlight::Marker => "Highlight",
            Highlight::Underline => "Underline",
        })
    }
}

/// Colors offered for markers (on the video and on the timeline).
pub const MARKER_COLORS: [u32; 8] = [0xFFD43B, 0xD6FF00, 0x2EF2A2, 0x4C8DFF, 0x8B7CF6, 0xFF6363, 0xFFA94D, 0xFFFFFF];

/// Widest average character of a caption-size name (digits and capitals included), in pixels.
const NAME_CHAR_W: f32 = 6.7;

/// `name` shortened to `max` characters with "…" in the middle, keeping its extension.
fn fit_name(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_string();
    }
    let ext = name.rfind('.').map(|i| name[i..].chars().count()).filter(|&n| n <= 6).unwrap_or(0);
    let tail = (ext + 4).min(max / 2);
    let head = max.saturating_sub(tail + 1).max(1);
    format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}

/// "VIDEOS  3": a media group's title and how many files it holds.
fn media_group_header<'a>(title: &str, count: usize) -> Element<'a, Message> {
    row![widgets::section(title), text(count).font(INTER_MEDIUM).size(10.5).color(TEXT_4)].spacing(6).align_y(Alignment::Center).padding([0, 6]).into()
}

fn effect_card(_: &iced::Theme, hovered: bool) -> container::Style {
    container::Style {
        background: Some(if hovered { SURFACE_3 } else { SURFACE_2 }.into()),
        border: iced::Border { color: if hovered { LINE_STRONG } else { LINE }, width: 1.0, radius: R_LG.into() },
        text_color: Some(TEXT_1),
        ..Default::default()
    }
}

/// Ready-made shapes in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapePreset {
    Card,
    Pill,
    Circle,
    Triangle,
    Star,
    Hexagon,
    Ring,
    Spotlight,
}

impl ShapePreset {
    const ALL: [ShapePreset; 8] = [ShapePreset::Card, ShapePreset::Pill, ShapePreset::Circle, ShapePreset::Triangle, ShapePreset::Star, ShapePreset::Hexagon, ShapePreset::Ring, ShapePreset::Spotlight];

    fn label(self) -> &'static str {
        match self {
            ShapePreset::Card => "Card",
            ShapePreset::Pill => "Pill",
            ShapePreset::Circle => "Circle",
            ShapePreset::Triangle => "Triangle",
            ShapePreset::Star => "Star",
            ShapePreset::Hexagon => "Hexagon",
            ShapePreset::Ring => "Ring",
            ShapePreset::Spotlight => "Spotlight",
        }
    }

    /// The shape, and its corner radius.
    pub fn build(self) -> (crate::clip::ShapeLayer, f64) {
        use crate::clip::{Rgba, ShapeKind, ShapeLayer};
        let (lime, mint) = (Rgba::hex(0xD6FF00), Rgba::hex(0x2EF2A2));
        match self {
            ShapePreset::Card => (ShapeLayer::new(ShapeKind::Rectangle, 560.0, 300.0, Rgba::hex(0x232720), Some(Rgba::hex(0x14160F))), 40.0),
            ShapePreset::Pill => (ShapeLayer::new(ShapeKind::Rectangle, 420.0, 110.0, lime, Some(mint)), 55.0),
            ShapePreset::Circle => (ShapeLayer::new(ShapeKind::Ellipse, 320.0, 320.0, lime, Some(mint)), 0.0),
            ShapePreset::Triangle => (ShapeLayer::new(ShapeKind::Triangle, 320.0, 280.0, lime, Some(mint)), 16.0),
            ShapePreset::Star => (ShapeLayer::new(ShapeKind::Star, 320.0, 320.0, lime, Some(mint)), 8.0),
            ShapePreset::Hexagon => (ShapeLayer::new(ShapeKind::Hexagon, 320.0, 320.0, lime, Some(mint)), 12.0),
            ShapePreset::Ring => (ShapeLayer::new(ShapeKind::Ring, 320.0, 320.0, lime, Some(mint)), 0.0),
            ShapePreset::Spotlight => {
                let mut s = ShapeLayer::new(ShapeKind::Ellipse, 900.0, 900.0, Rgba(0.84, 1.0, 0.0, 0.55), Some(Rgba(0.84, 1.0, 0.0, 0.0)));
                s.radial = true;
                (s, 0.0)
            }
        }
    }
}

/// Ready-made glowing icon callouts in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalloutPreset {
    Check,
    Arrow,
    Star,
    Sparkle,
    Zap,
}

impl CalloutPreset {
    const ALL: [CalloutPreset; 5] = [CalloutPreset::Check, CalloutPreset::Arrow, CalloutPreset::Star, CalloutPreset::Sparkle, CalloutPreset::Zap];

    /// Delegates to `CalloutIcon`'s own label/glyph so this preset list and the clip's actual
    /// icon enum never drift apart.
    fn label(self) -> &'static str {
        self.icon().label()
    }

    fn glyph(self) -> char {
        crate::render::scene::callout_glyph(self.icon())
    }

    fn color(self) -> Rgba {
        match self {
            CalloutPreset::Check => Rgba::hex(0x2EF2A2),
            CalloutPreset::Arrow => Rgba::hex(0x4C8DFF),
            CalloutPreset::Star => Rgba::hex(0xFFD166),
            CalloutPreset::Sparkle => Rgba::hex(0xD6FF00),
            CalloutPreset::Zap => Rgba::hex(0xFF8A4C),
        }
    }

    /// The callout layer for this preset.
    pub fn build(self) -> crate::clip::CalloutLayer {
        crate::clip::CalloutLayer::new(self.icon(), self.color())
    }

    fn icon(self) -> CalloutIcon {
        match self {
            CalloutPreset::Check => CalloutIcon::Check,
            CalloutPreset::Arrow => CalloutIcon::Arrow,
            CalloutPreset::Star => CalloutIcon::Star,
            CalloutPreset::Sparkle => CalloutIcon::Sparkle,
            CalloutPreset::Zap => CalloutIcon::Zap,
        }
    }
}

fn callout_grid<'a>() -> Element<'a, Message> {
    let items = CalloutPreset::ALL.iter().map(|&p| -> Element<'a, Message> {
        let Rgba(r, g, b, a) = p.color();
        let col = Color::from_rgba(r, g, b, a);
        button(
            column![container(tinted(p.glyph(), 18, col)).center(34).style(style::badge(col)), text(p.label()).font(INTER_MEDIUM).size(T_CAPTION)]
                .spacing(6)
                .align_x(Alignment::Center),
        )
        .width(Fill)
        .padding([10, 4])
        .style(style::choice_card(false))
        .on_press(Message::AddCallout(p))
        .into()
    });
    iced::widget::grid(items.collect::<Vec<_>>()).fluid(84).spacing(6).height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink)).into()
}

fn shape_grid<'a>() -> Element<'a, Message> {
    let items = ShapePreset::ALL.iter().map(|&p| -> Element<'a, Message> {
        button(column![iced::widget::canvas(ShapeIcon(p)).width(34).height(34), text(p.label()).font(INTER_MEDIUM).size(T_CAPTION)].spacing(6).align_x(Alignment::Center))
            .width(Fill)
            .padding([10, 4])
            .style(style::choice_card(false))
            .on_press(Message::AddShape(p))
            .into()
    });
    iced::widget::grid(items.collect::<Vec<_>>()).fluid(84).spacing(6).height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink)).into()
}

/// A small drawing of a shape preset, in the brand gradient.
struct ShapeIcon(ShapePreset);

impl iced::widget::canvas::Program<Message> for ShapeIcon {
    type State = ();

    fn draw(&self, _: &(), renderer: &iced::Renderer, _: &iced::Theme, bounds: iced::Rectangle, _: mouse::Cursor) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Gradient, Path, Stroke, gradient};
        use iced::{Point, Size};
        let mut f = Frame::new(renderer, bounds.size());
        let s = bounds.width.min(bounds.height);
        let c = Point::new(s / 2.0, s / 2.0);
        let fill = Gradient::Linear(gradient::Linear::new(Point::new(0.0, 0.0), Point::new(0.0, s)).add_stop(0.0, ACCENT).add_stop(1.0, BRAND_2));
        let poly = |pts: Vec<Point>| Path::new(|b| {
            b.move_to(pts[0]);
            for p in &pts[1..] {
                b.line_to(*p);
            }
            b.close();
        });
        match self.0 {
            ShapePreset::Card => f.fill(&Path::rounded_rectangle(Point::new(2.0, s * 0.2), Size::new(s - 4.0, s * 0.6), 6.0.into()), SURFACE_4),
            ShapePreset::Pill => f.fill(&Path::rounded_rectangle(Point::new(2.0, s * 0.32), Size::new(s - 4.0, s * 0.36), (s * 0.18).into()), fill),
            ShapePreset::Circle => f.fill(&Path::circle(c, s * 0.42), fill),
            ShapePreset::Triangle => f.fill(&poly(vec![Point::new(c.x, s * 0.1), Point::new(s * 0.92, s * 0.88), Point::new(s * 0.08, s * 0.88)]), fill),
            ShapePreset::Star => {
                let pts = (0..10)
                    .map(|i| {
                        let r = if i % 2 == 0 { s * 0.46 } else { s * 0.2 };
                        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                        Point::new(c.x + r * a.cos(), c.y + r * a.sin())
                    })
                    .collect();
                f.fill(&poly(pts), fill);
            }
            ShapePreset::Hexagon => {
                let pts = (0..6).map(|i| {
                    let a = std::f32::consts::FRAC_PI_6 + i as f32 * std::f32::consts::FRAC_PI_3;
                    Point::new(c.x + s * 0.44 * a.cos(), c.y + s * 0.44 * a.sin())
                });
                f.fill(&poly(pts.collect()), fill);
            }
            ShapePreset::Ring => f.stroke(&Path::circle(c, s * 0.36), Stroke::default().with_color(ACCENT).with_width(s * 0.13)),
            ShapePreset::Spotlight => {
                for (i, a) in [0.12, 0.2, 0.35].iter().enumerate() {
                    f.fill(&Path::circle(c, s * (0.46 - i as f32 * 0.12)), iced::Color { a: *a, ..ACCENT });
                }
            }
        }
        vec![f.into_geometry()]
    }
}


#[cfg(test)]
mod tests {
    use super::fit_name;

    #[test]
    fn long_names_are_shortened_in_the_middle() {
        assert_eq!(fit_name("logo.png", 20), "logo.png");
        let short = fit_name("8847400-hd_1920_1080_25fps.mp4", 20);
        assert_eq!(short.chars().count(), 20);
        assert!(short.starts_with("8847400-hd_") && short.ends_with("fps.mp4") && short.contains('…'), "{short}");
        let bangla = fit_name("পণ্যের ডেমো ভিডিও চূড়ান্ত সংস্করণ.mp4", 14);
        assert_eq!(bangla.chars().count(), 14);
        assert!(bangla.ends_with(".mp4"));
        assert_eq!(fit_name("abcdefghijklmnop", 6).chars().count(), 6);
    }
}
