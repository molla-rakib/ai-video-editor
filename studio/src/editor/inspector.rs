//! The inspector: properties of the selected clip.

use std::ops::RangeInclusive;

use iced::widget::{button, checkbox, column, container, image, pick_list, row, slider, space, text, text_input, toggler};
use iced::{Alignment, Color, Element, Fill, Length};

use super::Message;
use super::timeline::clip_label;
use crate::clip::{Align, Clip, ColorPreset, Content, Fit, Grade, MotionKind, Rgba, TransitionKind, COLOR_PRESETS};
use crate::model::{Id, MediaKind, Project};
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

#[derive(Debug, Clone)]
pub enum Edit {
    Name(String),
    Position(f64, f64),
    Scale(f64),
    Rotation(f64),
    Opacity(f64),
    Fit(Fit),
    ResetTransform,
    Radius(f64),
    BorderWidth(f64),
    BorderColor(Rgba),
    Shadow(bool),
    ShadowBlur(f64),
    ShadowOffset(f64),
    ShadowOpacity(f64),
    Brightness(f64),
    Contrast(f64),
    Saturation(f64),
    Temperature(f64),
    Lift(f64),
    Gamma(f64),
    Gain(f64),
    Tint(f64),
    Vibrance(f64),
    Fade(f64),
    SplitTone(f64),
    Smoothness(f64),
    Sharpness(f64),
    Vignette(f64),
    ResetGrade,
    /// Replaces the whole color correction with a look from `clip::COLOR_PRESETS`.
    GradePreset(crate::clip::Grade),
    Volume(f64),
    Mute(bool),
    FadeIn(f64),
    FadeOut(f64),
    EqLow(f64),
    EqMid(f64),
    EqHigh(f64),
    ResetEq,
    Compressor(bool),
    CompressorAmount(f64),
    Denoise(f64),
    Speed(f64),
    Text(String),
    Font(String),
    FontSize(f64),
    Weight(u16),
    TextColor(Rgba),
    Align(Align),
    TextBox(bool),
    /// Words fade in one after another instead of all at once.
    TextReveal(bool),
    TextRevealDuration(f64),
    TextRevealStagger(f64),
    ShapeKind(crate::clip::ShapeKind),
    ShapeFill(Rgba),
    ShapeGradient(bool),
    ShapeFillTo(Rgba),
    ShapeRadial(bool),
    ShapeAngle(f64),
    ShapeThickness(f64),
    Intro(Option<MotionKind>),
    Outro(Option<MotionKind>),
    IntroDuration(f64),
    OutroDuration(f64),
    /// Snaps the intro to an overshooting "bouncy" entrance (`Ease::Back`, 0.5s), or back off it.
    IntroBouncy(bool),
    Transition(Option<TransitionKind>),
    TransitionDuration(f64),
    RemoveZoom(Id),
    RemovePress(Id),
    ZoomScale(Id, f64),
    ZoomFocusX(Id, f64),
    ZoomFocusY(Id, f64),
    ZoomFocus(Id, f64, f64),
    Crop(crate::clip::Crop),
    CropLeft(f64),
    CropTop(f64),
    CropRight(f64),
    CropBottom(f64),
    /// Blur clips: strength (0..100), oval or box (the box is moved and sized on the preview).
    BlurStrength(f64),
    BlurEllipse(bool),
    /// Mask clips: edge softness (0..100), oval or box, and invert (hide the box instead of
    /// showing only it) — the box itself is moved and sized on the preview.
    MaskFeather(f64),
    MaskEllipse(bool),
    MaskInvert(bool),
    /// A shape, blur or mask resized on the canvas: width and height (canvas units) and its new center.
    ShapeBox(f64, f64, f64, f64),
    /// Restyles characters `start..end` of a text clip.
    SpanStyle(usize, usize, crate::clip::SpanChange),
    CalloutIcon(crate::clip::CalloutIcon),
    CalloutColor(Rgba),
    CalloutPulse(bool),
    /// A slow, continuous breathing so a still moment never looks frozen.
    IdleDrift(bool),
    CounterFrom(f64),
    CounterTo(f64),
    CounterDuration(f64),
    CounterDecimals(u32),
    CounterPrefix(String),
    CounterSuffix(String),
}

impl Edit {
    /// Undo-merge key and description: repeated edits of the same field merge into one step.
    pub fn describe(&self) -> (&'static str, &'static str) {
        match self {
            Edit::Name(_) => ("name", "Rename clip"),
            Edit::Position(..) => ("position", "Move"),
            Edit::Scale(_) => ("scale", "Scale"),
            Edit::Rotation(_) => ("rotation", "Rotate"),
            Edit::Opacity(_) => ("opacity", "Opacity"),
            Edit::Fit(_) => ("fit", "Framing"),
            Edit::ResetTransform => ("reset-transform", "Reset transform"),
            Edit::Radius(_) => ("radius", "Corner radius"),
            Edit::BorderWidth(_) | Edit::BorderColor(_) => ("border", "Border"),
            Edit::Shadow(_) | Edit::ShadowBlur(_) | Edit::ShadowOffset(_) | Edit::ShadowOpacity(_) => ("shadow", "Shadow"),
            Edit::Brightness(_) | Edit::Contrast(_) | Edit::Saturation(_) | Edit::Temperature(_) | Edit::Lift(_) | Edit::Gamma(_) | Edit::Gain(_) => ("grade", "Color"),
            Edit::Tint(_) | Edit::Vibrance(_) | Edit::Fade(_) | Edit::SplitTone(_) | Edit::Vignette(_) | Edit::Smoothness(_) | Edit::Sharpness(_) => ("grade", "Color"),
            Edit::ResetGrade => ("reset-grade", "Reset color"),
            Edit::GradePreset(_) => ("grade-preset", "Color preset"),
            Edit::Volume(_) | Edit::Mute(_) => ("volume", "Volume"),
            Edit::FadeIn(_) | Edit::FadeOut(_) => ("fade", "Audio fade"),
            Edit::EqLow(_) | Edit::EqMid(_) | Edit::EqHigh(_) => ("eq", "Equalizer"),
            Edit::ResetEq => ("reset-eq", "Reset equalizer"),
            Edit::Compressor(_) | Edit::CompressorAmount(_) => ("compressor", "Compressor"),
            Edit::Denoise(_) => ("denoise", "Noise reduction"),
            Edit::Speed(_) => ("speed", "Speed"),
            Edit::Text(_) => ("text", "Edit text"),
            Edit::SpanStyle(..) => ("text-span", "Style words"),
            Edit::Font(_) | Edit::FontSize(_) | Edit::Weight(_) | Edit::TextColor(_) | Edit::Align(_) | Edit::TextBox(_) => ("text-style", "Text style"),
            Edit::TextReveal(_) | Edit::TextRevealDuration(_) | Edit::TextRevealStagger(_) => ("text-reveal", "Kinetic reveal"),
            Edit::Intro(_) | Edit::Outro(_) | Edit::IntroDuration(_) | Edit::OutroDuration(_) | Edit::IntroBouncy(_) => ("motion", "Animation"),
            Edit::ShapeKind(_) | Edit::ShapeThickness(_) | Edit::ShapeBox(..) => ("shape", "Shape"),
            Edit::ShapeFill(_) | Edit::ShapeGradient(_) | Edit::ShapeFillTo(_) | Edit::ShapeRadial(_) | Edit::ShapeAngle(_) => ("shape-fill", "Shape fill"),
            Edit::Transition(_) | Edit::TransitionDuration(_) => ("transition", "Transition"),
            Edit::RemoveZoom(_) => ("remove-zoom", "Remove zoom"),
            Edit::RemovePress(_) => ("remove-press", "Remove press pulse"),
            Edit::ZoomScale(..) | Edit::ZoomFocusX(..) | Edit::ZoomFocusY(..) | Edit::ZoomFocus(..) => ("zoom", "Zoom"),
            Edit::Crop(_) | Edit::CropLeft(_) | Edit::CropTop(_) | Edit::CropRight(_) | Edit::CropBottom(_) => ("crop", "Crop"),
            Edit::BlurStrength(_) | Edit::BlurEllipse(_) => ("blur", "Blur"),
            Edit::MaskFeather(_) | Edit::MaskEllipse(_) | Edit::MaskInvert(_) => ("mask", "Mask"),
            Edit::CalloutIcon(_) => ("callout-icon", "Callout icon"),
            Edit::CalloutColor(_) => ("callout-color", "Callout color"),
            Edit::CalloutPulse(_) => ("callout-pulse", "Callout pulse"),
            Edit::IdleDrift(_) => ("idle-drift", "Idle drift"),
            Edit::CounterFrom(_) | Edit::CounterTo(_) | Edit::CounterDuration(_) | Edit::CounterDecimals(_) | Edit::CounterPrefix(_) | Edit::CounterSuffix(_) => ("counter", "Counter"),
        }
    }

    /// Applies edits that only touch the clip itself.
    pub fn apply(&self, c: &mut Clip) {
        let t = &mut c.transform;
        let l = &mut c.look;
        let g = &mut c.grade;
        match self.clone() {
            Edit::Name(s) => c.name = (!s.trim().is_empty()).then(|| s.trim().to_string()),
            Edit::Position(x, y) => {
                t.x = x;
                t.y = y;
            }
            Edit::Scale(v) => t.scale = v,
            Edit::Rotation(v) => t.rotation = v,
            Edit::Opacity(v) => t.opacity = v,
            Edit::Fit(f) => t.fit = f,
            Edit::ResetTransform => c.transform = crate::clip::Transform { fit: t.fit, ..Default::default() },
            Edit::Radius(v) => l.corner_radius = v,
            Edit::BorderWidth(v) => l.border_width = v,
            Edit::BorderColor(col) => l.border_color = col,
            Edit::Shadow(on) => l.shadow.enabled = on,
            Edit::ShadowBlur(v) => l.shadow.blur = v,
            Edit::ShadowOffset(v) => l.shadow.offset_y = v,
            Edit::ShadowOpacity(v) => l.shadow.opacity = v,
            Edit::Brightness(v) => g.brightness = v,
            Edit::Contrast(v) => g.contrast = v,
            Edit::Saturation(v) => g.saturation = v,
            Edit::Temperature(v) => g.temperature = v,
            Edit::Lift(v) => g.lift = v,
            Edit::Gamma(v) => g.gamma = v,
            Edit::Gain(v) => g.gain = v,
            Edit::Tint(v) => g.tint = v,
            Edit::Vibrance(v) => g.vibrance = v,
            Edit::Fade(v) => g.fade = v,
            Edit::SplitTone(v) => g.split = v,
            Edit::Smoothness(v) => g.smooth = v,
            Edit::Sharpness(v) => g.sharpen = v,
            Edit::Vignette(v) => g.vignette = v,
            Edit::ResetGrade => c.grade = Default::default(),
            Edit::GradePreset(grade) => c.grade = grade,
            Edit::Volume(v) => c.audio.volume = v,
            Edit::Mute(m) => c.audio.muted = m,
            Edit::FadeIn(v) => c.audio.fade_in = v,
            Edit::FadeOut(v) => c.audio.fade_out = v,
            Edit::EqLow(v) => c.audio.eq.low = v,
            Edit::EqMid(v) => c.audio.eq.mid = v,
            Edit::EqHigh(v) => c.audio.eq.high = v,
            Edit::ResetEq => c.audio.eq = Default::default(),
            Edit::Compressor(on) => c.audio.compressor.enabled = on,
            Edit::CompressorAmount(v) => c.audio.compressor.amount = v,
            Edit::Denoise(v) => c.audio.denoise = v,
            Edit::Intro(k) => c.intro = k.map(|kind| crate::clip::Motion { kind, duration: c.intro.map(|m| m.duration).unwrap_or(0.6), ease: c.intro.and_then(|m| m.ease), delay: c.intro.map(|m| m.delay).unwrap_or(0.0) }),
            Edit::Outro(k) => c.outro = k.map(|kind| crate::clip::Motion { kind, duration: c.outro.map(|m| m.duration).unwrap_or_else(|| c.intro.map(|m| m.duration * 0.6).unwrap_or(0.6)), ease: c.outro.and_then(|m| m.ease), delay: 0.0 }),
            Edit::IntroDuration(d) => {
                if let Some(m) = &mut c.intro {
                    m.duration = d;
                }
            }
            Edit::OutroDuration(d) => {
                if let Some(m) = &mut c.outro {
                    m.duration = d;
                }
            }
            Edit::IntroBouncy(on) => {
                // Same cap as every other intro/outro duration path: never longer than half the
                // clip, so the entrance actually finishes instead of animating for its whole life.
                let bouncy_duration = 0.5f64.min((c.duration / 2.0).max(0.1));
                if let Some(m) = &mut c.intro {
                    if on {
                        m.ease = Some(crate::clip::Ease::Back);
                        m.duration = bouncy_duration;
                    } else {
                        m.ease = None;
                    }
                }
            }
            // Transition and TransitionDuration are intercepted in `Message::Edit` and routed to
            // `Project::set_transition`, since they need project-level access (the previous clip,
            // and rippling later clips) that a single-clip `Edit::apply` can't provide.
            Edit::Transition(_) | Edit::TransitionDuration(_) => {}
            Edit::RemoveZoom(id) => c.zooms.retain(|z| z.id != id),
            Edit::RemovePress(id) => c.presses.retain(|p| p.id != id),
            Edit::ZoomScale(id, v) => c.zooms.iter_mut().filter(|z| z.id == id).for_each(|z| z.scale = v),
            Edit::ZoomFocusX(id, v) => c.zooms.iter_mut().filter(|z| z.id == id).for_each(|z| z.focus_x = v),
            Edit::ZoomFocusY(id, v) => c.zooms.iter_mut().filter(|z| z.id == id).for_each(|z| z.focus_y = v),
            Edit::ZoomFocus(id, x, y) => c.zooms.iter_mut().filter(|z| z.id == id).for_each(|z| {
                z.focus_x = x;
                z.focus_y = y;
            }),
            Edit::Crop(k) => c.crop = k.clamped(),
            Edit::CropLeft(v) => c.crop.left = v.clamp(0.0, (crate::clip::Crop::MAX - c.crop.right).max(0.0)),
            Edit::CropTop(v) => c.crop.top = v.clamp(0.0, (crate::clip::Crop::MAX - c.crop.bottom).max(0.0)),
            Edit::CropRight(v) => c.crop.right = v.clamp(0.0, (crate::clip::Crop::MAX - c.crop.left).max(0.0)),
            Edit::CropBottom(v) => c.crop.bottom = v.clamp(0.0, (crate::clip::Crop::MAX - c.crop.top).max(0.0)),
            Edit::IdleDrift(on) => c.idle_drift = on.then_some(crate::clip::DriftKind::ScaleBreath),
            edit if matches!(c.content, Content::Blur(_)) => {
                if let Content::Blur(b) = &mut c.content {
                    match edit {
                        Edit::BlurStrength(v) => b.strength = v.clamp(0.0, 100.0),
                        Edit::BlurEllipse(on) => b.ellipse = on,
                        Edit::ShapeBox(w, h, x, y) => {
                            b.width = w.clamp(4.0, 4000.0);
                            b.height = h.clamp(4.0, 4000.0);
                            (c.transform.x, c.transform.y) = (x, y);
                        }
                        _ => {}
                    }
                }
            }
            edit if matches!(c.content, Content::Mask(_)) => {
                if let Content::Mask(m) = &mut c.content {
                    match edit {
                        Edit::MaskFeather(v) => m.feather = v.clamp(0.0, 100.0),
                        Edit::MaskEllipse(on) => m.ellipse = on,
                        Edit::MaskInvert(on) => m.invert = on,
                        Edit::ShapeBox(w, h, x, y) => {
                            m.width = w.clamp(4.0, 4000.0);
                            m.height = h.clamp(4.0, 4000.0);
                            (c.transform.x, c.transform.y) = (x, y);
                        }
                        _ => {}
                    }
                }
            }
            edit if matches!(c.content, Content::Shape(_)) => {
                if let Content::Shape(s) = &mut c.content {
                    match edit {
                        Edit::ShapeKind(k) => s.kind = k,
                        Edit::ShapeFill(col) => s.fill = col,
                        Edit::ShapeGradient(on) => s.fill_to = on.then(|| s.fill_to.unwrap_or(Rgba::hex(0x2EF2A2))),
                        Edit::ShapeFillTo(col) => s.fill_to = Some(col),
                        Edit::ShapeRadial(r) => s.radial = r,
                        Edit::ShapeAngle(a) => s.gradient_angle = a,
                        Edit::ShapeThickness(v) => s.thickness = v,
                        Edit::ShapeBox(w, h, x, y) => {
                            s.width = w.clamp(4.0, 4000.0);
                            s.height = h.clamp(4.0, 4000.0);
                            (c.transform.x, c.transform.y) = (x, y);
                        }
                        _ => {}
                    }
                }
            }
            edit if matches!(c.content, Content::Callout(_)) => {
                if let Content::Callout(cal) = &mut c.content {
                    match edit {
                        Edit::CalloutIcon(i) => cal.icon = i,
                        Edit::CalloutColor(col) => {
                            cal.color = col;
                            l.shadow.color = col;
                        }
                        Edit::CalloutPulse(on) => cal.pulse = on,
                        _ => {}
                    }
                }
            }
            edit if matches!(c.content, Content::Counter(_)) => {
                if let Content::Counter(counter) = &mut c.content {
                    match edit {
                        Edit::CounterFrom(v) => counter.from = v,
                        Edit::CounterTo(v) => counter.to = v,
                        Edit::CounterDuration(v) => counter.duration = v.max(0.05),
                        Edit::CounterDecimals(n) => counter.decimals = n,
                        Edit::CounterPrefix(s) => counter.prefix = s,
                        Edit::CounterSuffix(s) => counter.suffix = s,
                        // The displayed number's styling reuses the text edits.
                        Edit::Font(f) => counter.format.font = f,
                        Edit::FontSize(v) => counter.format.size = v,
                        Edit::Weight(w) => counter.format.weight = w,
                        Edit::TextColor(col) => counter.format.color = col,
                        Edit::Align(a) => counter.format.align = a,
                        _ => {}
                    }
                }
            }
            edit => {
                if let Content::Text(tl) = &mut c.content {
                    match edit {
                        Edit::Text(s) => tl.set_text(s),
                        Edit::SpanStyle(a, b, change) => tl.style_range(a, b, &change),
                        Edit::Font(f) => tl.font = f,
                        Edit::FontSize(v) => tl.size = v,
                        Edit::Weight(w) => tl.weight = w,
                        Edit::TextColor(col) => tl.color = col,
                        Edit::Align(a) => tl.align = a,
                        Edit::TextBox(on) => tl.background = on.then_some(Rgba(0.0, 0.0, 0.0, 0.55)),
                        Edit::TextReveal(on) => tl.reveal = on.then(crate::clip::TextReveal::default),
                        Edit::TextRevealDuration(v) => {
                            if let Some(r) = &mut tl.reveal {
                                r.duration = v.max(0.05);
                            }
                        }
                        Edit::TextRevealStagger(v) => {
                            if let Some(r) = &mut tl.reveal {
                                r.stagger = v.max(0.0);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

pub const FONTS: [&str; 3] = ["Inter", "Poppins", "Hind Siliguri"];

const PALETTE: [u32; 8] = [0xFFFFFF, 0x050605, 0xD6FF00, 0x2EF2A2, 0x4C8DFF, 0x8B7CF6, 0xFFB547, 0xFF6363];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MotionChoice(Option<MotionKind>);

impl std::fmt::Display for MotionChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            None => "None",
            Some(MotionKind::Fade) => "Fade",
            Some(MotionKind::SlideUp) => "Slide up",
            Some(MotionKind::SlideDown) => "Slide down",
            Some(MotionKind::SlideLeft) => "Slide left",
            Some(MotionKind::SlideRight) => "Slide right",
            Some(MotionKind::Pop) => "Pop",
            Some(MotionKind::ZoomIn) => "Zoom in",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TransitionChoice(Option<TransitionKind>);

impl std::fmt::Display for TransitionChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            None => "None",
            Some(TransitionKind::Crossfade) => "Crossfade",
            Some(TransitionKind::SlideLeft) => "Slide",
            Some(TransitionKind::SlideUp) => "Slide up",
            Some(TransitionKind::Push) => "Push",
            Some(TransitionKind::ZoomThrough) => "Zoom through",
            Some(TransitionKind::Wipe) => "Wipe",
            Some(TransitionKind::SlideRight) => "Slide right",
            Some(TransitionKind::SlideDown) => "Slide down",
            Some(TransitionKind::DipToBlack) => "Dip to black",
        })
    }
}

pub fn view<'a>(project: &'a Project, clip: &'a Clip, ctx: &Ctx<'a>) -> Element<'a, Message> {
    let (zoom_edit, range, hex) = (ctx.zoom_edit, ctx.range, ctx.hex);
    let id = clip.id;
    let e = move |edit: Edit| Message::Edit(id, edit);
    let kind = clip.asset().and_then(|a| project.asset(a)).map(|a| a.kind);
    let is_visual = project.clip_has_picture(clip);
    let has_audio = project.clip_has_sound(clip);
    let sound_only = clip.part == crate::clip::Part::Sound;
    let is_video = kind == Some(MediaKind::Video) && !sound_only;

    let (glyph, color, kind_label) = match (&clip.content, kind) {
        (Content::Text(_), _) => (code::TYPE, clip::TEXT, "Text"),
        (Content::Shape(_), _) => (code::SQUARE, clip::EFFECT, "Shape"),
        (Content::Blur(_), _) => (code::DROPLETS, clip::BLUR, "Blur"),
        (Content::Mask(_), _) => (code::SCAN_EYE, clip::MASK, "Mask"),
        (Content::Callout(_), _) => (code::SPARKLE, clip::EFFECT, "Callout"),
        (Content::Counter(_), _) => (code::GAUGE, clip::TEXT, "Counter"),
        (_, Some(MediaKind::Audio)) => (code::AUDIO_LINES, clip::AUDIO, "Audio"),
        _ if sound_only => (code::AUDIO_LINES, clip::AUDIO, "Audio"),
        (_, Some(MediaKind::Image)) => (code::IMAGE, clip::IMAGE, "Image"),
        _ => (code::FILM, clip::VIDEO, "Video"),
    };
    let header = container(
        row![
            container(tinted(glyph, 15, color)).center(32).style(style::badge(color)),
            column![
                text_input("Clip name", &clip.name.clone().unwrap_or_else(|| clip_label(project, clip)))
                    .on_input(move |s| e(Edit::Name(s)))
                    .font(INTER_SEMIBOLD)
                    .size(T_BODY)
                    .padding([3, 6])
                    .style(|t, s| {
                        let mut st = style::input(t, s);
                        if matches!(s, text_input::Status::Active) {
                            st.border.color = Color::TRANSPARENT;
                            st.background = Color::TRANSPARENT.into();
                        }
                        st
                    }),
                text(format!("{kind_label} · {:.2}s at {}", clip.duration, super::fmt_timecode(clip.start))).size(T_CAPTION).color(TEXT_3),
            ]
            .spacing(2)
            .width(Fill),
        ]
        .spacing(S3)
        .align_y(Alignment::Center),
    )
    .padding([S3, S3]);

    let mut col = column![header].spacing(S2);
    let mut shown = 0;

    // A blur clip only has its blur and its place.
    if let Content::Blur(b) = &clip.content {
        let col = blur_clip(clip, b, ctx, col, &mut shown);
        return widgets::scroll(col.padding(iced::Padding { top: 0.0, right: 10.0, bottom: 12.0, left: 4.0 })).height(Fill).into();
    }

    // A mask clip only has its mask and its place.
    if let Content::Mask(m) = &clip.content {
        let col = mask_clip(clip, m, ctx, col, &mut shown);
        return widgets::scroll(col.padding(iced::Padding { top: 0.0, right: 10.0, bottom: 12.0, left: 4.0 })).height(Fill).into();
    }

    if let Content::Text(t) = &clip.content {
        let font_pick = pick_list(&FONTS[..], FONTS.iter().find(|f| **f == t.font).copied(), move |f: &str| e(Edit::Font(f.to_string())))
            .padding([6, 9])
            .text_size(T_BODY)
            .width(Fill)
            .style(style::select)
            .menu_style(style::select_menu);
        let weights = [(400u16, "Regular"), (500, "Medium"), (600, "Semibold"), (700, "Bold")];
        let aligns = [(Align::Left, code::ALIGN_LEFT, "Align left"), (Align::Center, code::ALIGN_CENTER, "Align center"), (Align::Right, code::ALIGN_RIGHT, "Align right")];
        let font = if t.text.is_ascii() { INTER } else { HIND };
        // A multi-line box that keeps its selection: select words to style just them.
        let (text_box, selected): (Element<'a, Message>, Option<(usize, usize)>) = match ctx.text_draft {
            Some(content) => (
                iced::widget::text_editor(content)
                    .placeholder("Type your text…")
                    .on_action(move |a| Message::TextDraft(id, a))
                    .key_binding(widgets::editor_keys)
                    .highlight_with::<SpanHighlighter>(span_marks(t), span_format)
                    .padding([8, 10])
                    .size(T_BODY)
                    .font(font)
                    .min_height(40)
                    .max_height(160)
                    .style(style::text_area)
                    .into(),
                selected_chars(content),
            ),
            None => (text_input("Type your text…", &t.text).on_input(move |s| e(Edit::Text(s))).padding([8, 10]).size(T_BODY).font(font).style(style::input).into(), None),
        };
        let mut text_col = column![text_box].spacing(S3);
        match selected {
            Some((a, b)) => text_col = text_col.push(selection_style(id, t, a, b, hex)),
            None => text_col = text_col.push(text("Select words to give them their own color, weight, size or highlight.").size(T_CAPTION).color(TEXT_4)),
        }
        col = add(col, &mut shown, ctx.section(
            code::TYPE,
            "Text",
            column![
                text_col,
                row![container(font_pick).width(Length::FillPortion(3)), container(widgets::segmented_icons(&aligns, t.align, move |a| e(Edit::Align(a)))).width(Length::FillPortion(2))]
                .spacing(S2)
                .align_y(Alignment::Center),
                widgets::segmented(&weights.map(|w| w.0), t.weight, move |w| weights.iter().find(|x| x.0 == *w).map(|x| x.1).unwrap_or("").to_string(), move |w| e(Edit::Weight(w))),
                slider_row("Size", t.size, 12.0..=400.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::FontSize(v))),
                color_row("Color", t.color, (id, "text", hex), move |c| e(Edit::TextColor(c))),
                toggle_row("Background box", t.background.is_some(), move |on| e(Edit::TextBox(on))),
            ]
            .spacing(S3)
            .into(),
        ));
        let mut reveal_col = column![toggle_row("Word by word", t.reveal.is_some(), move |on| e(Edit::TextReveal(on)))].spacing(S3);
        if let Some(r) = t.reveal {
            reveal_col = reveal_col
                .push(slider_row("Word duration", r.duration, 0.05..=1.5, 0.05, |v| format!("{v:.2}s"), move |v| e(Edit::TextRevealDuration(v))))
                .push(slider_row("Stagger", r.stagger, 0.0..=0.3, 0.01, |v| format!("{v:.2}s"), move |v| e(Edit::TextRevealStagger(v))));
        }
        col = add(col, &mut shown, ctx.section(code::SPARKLES, "Kinetic reveal", reveal_col.into()));
    }

    if let Content::Counter(counter) = &clip.content {
        let t = &counter.format;
        let font_pick = pick_list(&FONTS[..], FONTS.iter().find(|f| **f == t.font).copied(), move |f: &str| e(Edit::Font(f.to_string())))
            .padding([6, 9])
            .text_size(T_BODY)
            .width(Fill)
            .style(style::select)
            .menu_style(style::select_menu);
        let weights = [(400u16, "Regular"), (500, "Medium"), (600, "Semibold"), (700, "Bold")];
        let aligns = [(Align::Left, code::ALIGN_LEFT, "Align left"), (Align::Center, code::ALIGN_CENTER, "Align center"), (Align::Right, code::ALIGN_RIGHT, "Align right")];
        col = add(col, &mut shown, ctx.section(
            code::GAUGE,
            "Counter",
            column![
                slider_row("From", counter.from, -9999.0..=9999.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::CounterFrom(v))),
                slider_row("To", counter.to, -9999.0..=9999.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::CounterTo(v))),
                slider_row("Duration", counter.duration, 0.2..=5.0, 0.1, |v| format!("{v:.1}s"), move |v| e(Edit::CounterDuration(v))),
                widgets::segmented(&[0u32, 1, 2], counter.decimals, |d| format!("{d} decimals"), move |d| e(Edit::CounterDecimals(d))),
                row![
                    text_input("Prefix", &counter.prefix).on_input(move |s| e(Edit::CounterPrefix(s))).padding([8, 10]).size(T_BODY).style(style::input).width(Length::FillPortion(1)),
                    text_input("Suffix", &counter.suffix).on_input(move |s| e(Edit::CounterSuffix(s))).padding([8, 10]).size(T_BODY).style(style::input).width(Length::FillPortion(1)),
                ]
                .spacing(S2),
            ]
            .spacing(S3)
            .into(),
        ));
        col = add(col, &mut shown, ctx.section(
            code::TYPE,
            "Number style",
            column![
                row![container(font_pick).width(Length::FillPortion(3)), container(widgets::segmented_icons(&aligns, t.align, move |a| e(Edit::Align(a)))).width(Length::FillPortion(2))]
                .spacing(S2)
                .align_y(Alignment::Center),
                widgets::segmented(&weights.map(|w| w.0), t.weight, move |w| weights.iter().find(|x| x.0 == *w).map(|x| x.1).unwrap_or("").to_string(), move |w| e(Edit::Weight(w))),
                slider_row("Size", t.size, 12.0..=400.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::FontSize(v))),
                color_row("Color", t.color, (id, "text", hex), move |c| e(Edit::TextColor(c))),
            ]
            .spacing(S3)
            .into(),
        ));
    }

    if let Content::Shape(s) = &clip.content {
        let kind_pick = pick_list(&crate::clip::ShapeKind::ALL[..], Some(s.kind), move |k| e(Edit::ShapeKind(k)))
            .padding([6, 9])
            .text_size(T_BODY)
            .width(Fill)
            .style(style::select)
            .menu_style(style::select_menu);
        let mut body = column![
            row![text("Shape").size(T_SMALL).color(TEXT_2).width(84), kind_pick].align_y(Alignment::Center),
        ]
        .spacing(S3);
        if matches!(s.kind, crate::clip::ShapeKind::Ring | crate::clip::ShapeKind::Frame) {
            body = body.push(slider_row("Thickness", s.thickness, 1.0..=200.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::ShapeThickness(v))));
        }
        body = body.push(color_row(if s.fill_to.is_some() { "Color 1" } else { "Fill" }, s.fill, (id, "shape1", hex), move |c| e(Edit::ShapeFill(c))));
        body = body.push(toggle_row("Gradient", s.fill_to.is_some(), move |on| e(Edit::ShapeGradient(on))));
        if let Some(to) = s.fill_to {
            body = body.push(color_row("Color 2", to, (id, "shape2", hex), move |c| e(Edit::ShapeFillTo(c))));
            body = body.push(row![
                text("Style").size(T_SMALL).color(TEXT_2).width(84),
                widgets::segmented(&[false, true], s.radial, |r| if *r { "Radial".into() } else { "Linear".into() }, move |r| e(Edit::ShapeRadial(r))),
            ].align_y(Alignment::Center));
            if !s.radial {
                body = body.push(slider_row("Direction", s.gradient_angle, 0.0..=360.0, 1.0, |v| format!("{v:.0}°"), move |v| e(Edit::ShapeAngle(v))));
            }
        }
        col = add(col, &mut shown, ctx.section(code::SHAPES, "Shape", body.into()));
    }

    if let Content::Callout(cal) = &clip.content {
        let icon_pick = pick_list(&crate::clip::CalloutIcon::ALL[..], Some(cal.icon), move |k| e(Edit::CalloutIcon(k)))
            .padding([6, 9])
            .text_size(T_BODY)
            .width(Fill)
            .style(style::select)
            .menu_style(style::select_menu);
        let body = column![
            row![text("Icon").size(T_SMALL).color(TEXT_2).width(84), icon_pick].align_y(Alignment::Center),
            color_row("Glow color", cal.color, (id, "callout", hex), move |c| e(Edit::CalloutColor(c))),
            toggle_row("Pulse", cal.pulse, move |on| e(Edit::CalloutPulse(on))),
        ]
        .spacing(S3);
        col = add(col, &mut shown, ctx.section(code::SPARKLE, "Callout", body.into()));
    }

    // Everything below adjusts what the clip already has. Effects are added from the Effects
    // tab; position and size are set on the preview.
    if is_visual {
        let tr = &clip.transform;
        let mut layer = column![
            slider_row("Rotation", tr.rotation, -180.0..=180.0, 0.5, |v| format!("{v:.0}°"), move |v| e(Edit::Rotation(v))),
            slider_row("Opacity", tr.opacity * 100.0, 0.0..=100.0, 1.0, |v| format!("{v:.0}%"), move |v| e(Edit::Opacity(v / 100.0))),
            toggle_row("Idle drift", clip.idle_drift.is_some(), move |on| e(Edit::IdleDrift(on))),
        ]
        .spacing(S3);
        if matches!(clip.content, Content::Media { .. }) {
            layer = layer.push(
                row![
                    text("Framing").size(T_SMALL).color(TEXT_2).width(84),
                    widgets::segmented(&[Fit::Contain, Fit::Cover], tr.fit, |f| if *f == Fit::Contain { "Fit".into() } else { "Fill".into() }, move |f| e(Edit::Fit(f))),
                ]
                .align_y(Alignment::Center),
            );
        }
        col = add(col, &mut shown, ctx.section_reset(code::MOVE, "Layer", e(Edit::ResetTransform), layer.into()));
        if matches!(clip.content, Content::Media { .. }) {
            col = add(col, &mut shown, crop_section(clip, ctx));
        }

        // Frame style: only the parts added from Effects → Frame, each with its remove button.
        if !matches!(clip.content, Content::Text(_)) {
            let l = &clip.look;
            let mut frame = column![].spacing(S3);
            if l.corner_radius > 0.0 {
                frame = frame
                    .push(applied("Rounded corners", e(Edit::Radius(0.0))))
                    .push(slider_row("Radius", l.corner_radius, 1.0..=200.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::Radius(v))));
            }
            if l.border_width > 0.0 {
                frame = frame
                    .push(applied("Border", e(Edit::BorderWidth(0.0))))
                    .push(slider_row("Width", l.border_width, 0.5..=40.0, 0.5, |v| format!("{v:.1}"), move |v| e(Edit::BorderWidth(v))))
                    .push(color_row("Color", l.border_color, (id, "border", hex), move |c| e(Edit::BorderColor(c))));
            }
            if l.shadow.enabled {
                frame = frame
                    .push(applied("Shadow", e(Edit::Shadow(false))))
                    .push(slider_row("Blur", l.shadow.blur, 0.0..=200.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::ShadowBlur(v))))
                    .push(slider_row("Distance", l.shadow.offset_y, -100.0..=100.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::ShadowOffset(v))))
                    .push(slider_row("Strength", l.shadow.opacity * 100.0, 0.0..=100.0, 1.0, |v| format!("{v:.0}%"), move |v| e(Edit::ShadowOpacity(v / 100.0))));
            }
            if l.corner_radius > 0.0 || l.border_width > 0.0 || l.shadow.enabled {
                col = add(col, &mut shown, ctx.section(code::FRAME, "Frame style", frame.into()));
            }
        }

        let g = &clip.grade;
        col = add(col, &mut shown, ctx.section(code::PALETTE, "Color preset", color_presets(*g, ctx.preset_tiles, move |g| e(Edit::GradePreset(g)))));

        let signed = |v: f64| if v.round() == 0.0 { "0".to_string() } else { format!("{v:+.0}") };
        let amount = |v: f64| format!("{v:.0}");
        let group = |name: &'static str| text(name).font(INTER_SEMIBOLD).size(T_CAPTION).color(TEXT_3);
        let mut color = column![
            group("Light"),
            slider_row("Brightness", g.brightness * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Brightness(v / 100.0))),
            slider_row("Contrast", g.contrast * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Contrast(v / 100.0))),
            slider_row("Shadows", g.lift * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Lift(v / 100.0))),
            slider_row("Midtones", g.gamma * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Gamma(v / 100.0))),
            slider_row("Highlights", g.gain * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Gain(v / 100.0))),
            slider_row("Fade", g.fade * 100.0, 0.0..=100.0, 1.0, amount, move |v| e(Edit::Fade(v / 100.0))),
            widgets::divider(),
            group("Color"),
            slider_row("Saturation", g.saturation * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Saturation(v / 100.0))),
            slider_row("Vibrance", g.vibrance * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Vibrance(v / 100.0))),
            slider_row("Warmth", g.temperature * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Temperature(v / 100.0))),
            slider_row("Tint", g.tint * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::Tint(v / 100.0))),
            slider_row("Split tone", g.split * 100.0, -100.0..=100.0, 1.0, signed, move |v| e(Edit::SplitTone(v / 100.0))),
            widgets::divider(),
            group("Finish"),
        ]
        .spacing(S3);
        // Smoothing and sharpening work on pictures only: text and shapes stay crisp.
        if kind.is_some() {
            color = color
                .push(slider_row("Smoothness", g.smooth * 100.0, 0.0..=100.0, 1.0, amount, move |v| e(Edit::Smoothness(v / 100.0))))
                .push(slider_row("Sharpness", g.sharpen * 100.0, 0.0..=100.0, 1.0, amount, move |v| e(Edit::Sharpness(v / 100.0))));
        }
        color = color.push(slider_row("Vignette", g.vignette * 100.0, 0.0..=100.0, 1.0, amount, move |v| e(Edit::Vignette(v / 100.0))));
        col = add(col, &mut shown, ctx.section_reset(code::CONTRAST, "Color", e(Edit::ResetGrade), color.into()));

        // Animation and transition: only the ones on the clip (pick another in Effects to change it).
        let max_motion = (clip.duration / 2.0).max(0.1);
        let mut anim = column![].spacing(S3);
        if let Some(m) = clip.intro {
            anim = anim
                .push(applied(format!("In · {}", MotionChoice(Some(m.kind))), e(Edit::Intro(None))))
                .push(slider_row("Duration", m.duration, 0.1..=max_motion.max(0.11), 0.05, |v| format!("{v:.2}s"), move |v| e(Edit::IntroDuration(v))))
                .push(toggle_row("Bouncy", m.ease == Some(crate::clip::Ease::Back), move |on| e(Edit::IntroBouncy(on))));
        }
        if let Some(m) = clip.outro {
            anim = anim
                .push(applied(format!("Out · {}", MotionChoice(Some(m.kind))), e(Edit::Outro(None))))
                .push(slider_row("Duration", m.duration, 0.1..=max_motion.max(0.11), 0.05, |v| format!("{v:.2}s"), move |v| e(Edit::OutroDuration(v))));
        }
        if clip.intro.is_some() || clip.outro.is_some() {
            col = add(col, &mut shown, ctx.section(code::SPARKLES, "Animation", anim.into()));
        }
        if let Some(t) = clip.transition {
            // Capped by the real overlap, not half this clip's length: it can't eat more of
            // either clip than that clip actually has.
            let cap = project.prev_on_track(clip).map_or(clip.duration, |p| clip.duration.min(p.duration));
            let trans = column![
                applied(TransitionChoice(Some(t.kind)).to_string(), e(Edit::Transition(None))),
                slider_row("Duration", t.duration, 0.1..=cap.max(0.11), 0.05, |v| format!("{v:.2}s"), move |v| e(Edit::TransitionDuration(v))),
            ]
            .spacing(S3);
            col = add(col, &mut shown, ctx.section(code::BLEND, "Transition", trans.into()));
        }
    }

    if !clip.zooms.is_empty() && (is_video || (kind == Some(MediaKind::Image) && is_visual)) {
        let mut zooms = column![].spacing(S3);
        for (i, z) in clip.zooms.iter().enumerate() {
            let zid = z.id;
            let editing = zoom_edit == Some(zid);
            zooms = zooms.push(
                container(
                    column![
                        row![
                            tinted(code::ZOOM_IN, 13, BRAND_2),
                            text(format!("Zoom {}", i + 1)).font(INTER_SEMIBOLD).size(T_SMALL),
                            text(format!("{:.1}s – {:.1}s", z.at, z.at + z.duration)).size(T_CAPTION).color(TEXT_3),
                            space().width(Fill),
                            widgets::tip(widgets::icon_button(code::TRASH_2, Some(e(Edit::RemoveZoom(zid)))).width(24).height(24), "Remove", None, iced::widget::tooltip::Position::Left),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                        checkbox(editing)
                            .label("Edit on canvas")
                            .on_toggle(move |on| Message::ZoomEdit(on.then_some((id, zid))))
                            .size(15)
                            .spacing(8)
                            .text_size(T_SMALL)
                            .style(style::checkbox),
                        slider_row("Zoom", z.scale * 100.0, 100.0..=500.0, 1.0, |v| format!("{v:.0}%"), move |v| e(Edit::ZoomScale(zid, v / 100.0))),
                        slider_row("Focus X", z.focus_x * 100.0, 0.0..=100.0, 0.5, |v| format!("{v:.0}%"), move |v| e(Edit::ZoomFocusX(zid, v / 100.0))),
                        slider_row("Focus Y", z.focus_y * 100.0, 0.0..=100.0, 0.5, |v| format!("{v:.0}%"), move |v| e(Edit::ZoomFocusY(zid, v / 100.0))),
                    ]
                    .spacing(S2),
                )
                .padding(S3)
                .style(move |t| {
                    let mut s = style::inset(t);
                    if editing {
                        s.border = iced::Border { color: BRAND_2, width: 1.5, radius: R_MD.into() };
                    }
                    s
                }),
            );
        }
        col = add(col, &mut shown, ctx.section(code::ZOOM_IN, "Zoom & pan", zooms.into()));
    }

    if !clip.presses.is_empty() {
        let mut presses = column![].spacing(S3);
        for (i, press) in clip.presses.iter().enumerate() {
            let pid = press.id;
            presses = presses.push(
                row![
                    tinted(code::POINTER, 13, BRAND_2),
                    text(format!("Press {}", i + 1)).font(INTER_SEMIBOLD).size(T_SMALL),
                    text(format!("at {:.2}s", press.at)).size(T_CAPTION).color(TEXT_3),
                    space().width(Fill),
                    widgets::tip(widgets::icon_button(code::TRASH_2, Some(e(Edit::RemovePress(pid)))).width(24).height(24), "Remove", None, iced::widget::tooltip::Position::Left),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
        }
        col = add(col, &mut shown, ctx.section(code::POINTER, "Press pulses", presses.into()));
    }

    if let Content::Media { speed, .. } = clip.content {
        if kind != Some(MediaKind::Image) {
            col = add(col, &mut shown, ctx.section(
                code::GAUGE,
                "Speed",
                slider_row("Speed", speed * 100.0, 25.0..=400.0, 5.0, |v| format!("{:.2}×", v / 100.0), move |v| e(Edit::Speed(v / 100.0))),
            ));
        }
    }

    if has_audio {
        let a = &clip.audio;
        let mut audio = column![
            slider_row("Volume", a.volume * 100.0, 0.0..=200.0, 1.0, |v| format!("{v:.0}%"), move |v| e(Edit::Volume(v / 100.0))),
            toggle_row("Mute", a.muted, move |m| e(Edit::Mute(m))),
            slider_row("Fade in", a.fade_in, 0.0..=5.0, 0.05, |v| format!("{v:.1}s"), move |v| e(Edit::FadeIn(v))),
            slider_row("Fade out", a.fade_out, 0.0..=5.0, 0.05, |v| format!("{v:.1}s"), move |v| e(Edit::FadeOut(v))),
        ]
        .spacing(S3);

        // Volume for just the selected range of this clip.
        let overlap = range.and_then(|(ra, rb)| {
            let (s, t) = (ra.max(clip.start), rb.min(clip.end()));
            (t - s >= 0.05).then_some((s, t))
        });
        let mut part = column![].spacing(S2);
        match overlap {
            Some((s, t)) => {
                let (ls, lt) = (s - clip.start, t - clip.start);
                let current = a.gains.iter().find(|g| (g.at - ls).abs() < 1e-3 && (g.at + g.duration - lt).abs() < 1e-3).map(|g| g.volume).unwrap_or(1.0);
                part = part
                    .push(row![tinted(code::SQUARE_DASHED, 12, ACCENT), text(format!("Selected range {} – {}", super::fmt_timecode_short(s), super::fmt_timecode_short(t))).size(T_SMALL).color(TEXT_2)].spacing(6).align_y(Alignment::Center))
                    .push(slider_row("Range volume", current * 100.0, 0.0..=200.0, 1.0, |v| format!("{v:.0}%"), move |v| Message::RangeVolume(id, v / 100.0)))
                    .push(row![
                        widgets::secondary_button(Some(code::VOLUME_X), "Mute range", Some(Message::RangeVolume(id, 0.0))),
                        widgets::ghost_button(None, "Reset", Some(Message::RangeVolume(id, 1.0))),
                    ].spacing(S2));
            }
            None => {
                part = part.push(text("Mark a range (R) to change the volume of just one part of this clip.").size(T_CAPTION).color(TEXT_3));
            }
        }
        if !a.gains.is_empty() {
            part = part.push(text("Changed parts").font(INTER_SEMIBOLD).size(T_CAPTION).color(TEXT_3));
        }
        for g in &a.gains {
            let gid = g.id;
            part = part.push(
                column![
                    row![
                        text(format!("{} – {}", super::fmt_timecode_short(clip.start + g.at.max(0.0)), super::fmt_timecode_short(clip.start + (g.at + g.duration).min(clip.duration)))).size(T_CAPTION).color(TEXT_2).width(Fill),
                        widgets::tip(widgets::icon_button(code::X, Some(Message::RemoveGain(id, gid))).width(22).height(22), "Back to the clip volume", None, iced::widget::tooltip::Position::Left),
                    ]
                    .spacing(S2)
                    .align_y(Alignment::Center),
                    slider_row("Volume", g.volume * 100.0, 0.0..=200.0, 1.0, |v| if v == 0.0 { "Muted".into() } else { format!("{v:.0}%") }, move |v| Message::GainVolume(id, gid, v / 100.0)),
                ]
                .spacing(4),
            );
        }
        audio = audio.push(container(part).padding(S3).width(Fill).style(style::inset));
        col = add(col, &mut shown, ctx.section(code::VOLUME_2, "Audio", audio.into()));

        let polish = column![
            text("Equalizer").font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_3),
            slider_row("Low", a.eq.low, -15.0..=15.0, 0.5, |v| format!("{v:+.1} dB"), move |v| e(Edit::EqLow(v))),
            slider_row("Mid", a.eq.mid, -15.0..=15.0, 0.5, |v| format!("{v:+.1} dB"), move |v| e(Edit::EqMid(v))),
            slider_row("High", a.eq.high, -15.0..=15.0, 0.5, |v| format!("{v:+.1} dB"), move |v| e(Edit::EqHigh(v))),
            widgets::ghost_button(None, "Reset equalizer", (!a.eq.is_flat()).then_some(e(Edit::ResetEq))).padding([5, 9]),
            widgets::divider(),
            toggle_row("Compressor", a.compressor.enabled, move |on| e(Edit::Compressor(on))),
            slider_row("Amount", a.compressor.amount, 0.0..=100.0, 1.0, |v| format!("{v:.0}%"), move |v| e(Edit::CompressorAmount(v))),
            widgets::divider(),
            slider_row("Noise reduction", a.denoise, 0.0..=97.0, 1.0, |v| if v == 0.0 { "Off".into() } else { format!("{v:.0}%") }, move |v| e(Edit::Denoise(v))),
        ]
        .spacing(S3);
        col = add(col, &mut shown, ctx.section(code::SLIDERS_HORIZONTAL, "Audio polish", polish.into()));
    }

    // Header plus at least one section: otherwise the search found nothing here.
    if shown == 0 {
        col = col.push(container(text(format!("No settings match \u{201c}{}\u{201d}", ctx.query.trim())).size(T_SMALL).color(TEXT_3)).padding([S2, S3]));
    }
    widgets::scroll(col.padding(iced::Padding { top: 0.0, right: 10.0, bottom: 12.0, left: 4.0 })).height(Fill).into()
}

/// Adds a section if the search shows it, counting what is shown.
fn add<'a>(col: iced::widget::Column<'a, Message>, shown: &mut usize, section: Option<Element<'a, Message>>) -> iced::widget::Column<'a, Message> {
    match section {
        Some(s) => {
            *shown += 1;
            col.push(s)
        }
        None => col,
    }
}

/// Crop sliders, with on-canvas cropping.
fn crop_section<'a>(clip: &'a Clip, ctx: &Ctx<'a>) -> Option<Element<'a, Message>> {
    let id = clip.id;
    let e = move |edit: Edit| Message::Edit(id, edit);
    let k = clip.crop;
    let pct = |v: f64| format!("{:.0}%", v);
    let max = crate::clip::Crop::MAX * 100.0;
    let body = column![
        checkbox(ctx.crop_edit).label("Crop on canvas").on_toggle(move |on| Message::CropEdit(on.then_some(id))).size(15).spacing(8).text_size(T_SMALL).style(style::checkbox),
        slider_row("Left", k.left * 100.0, 0.0..=max, 0.5, pct, move |v| e(Edit::CropLeft(v / 100.0))),
        slider_row("Right", k.right * 100.0, 0.0..=max, 0.5, pct, move |v| e(Edit::CropRight(v / 100.0))),
        slider_row("Top", k.top * 100.0, 0.0..=max, 0.5, pct, move |v| e(Edit::CropTop(v / 100.0))),
        slider_row("Bottom", k.bottom * 100.0, 0.0..=max, 0.5, pct, move |v| e(Edit::CropBottom(v / 100.0))),
    ]
    .spacing(S3);
    ctx.section_reset(code::CROP, "Crop", e(Edit::Crop(Default::default())), body.into())
}

/// A blur clip: how strong, what shape, how big, and where.
fn blur_clip<'a>(clip: &'a Clip, b: &crate::clip::BlurLayer, ctx: &Ctx<'a>, col: iced::widget::Column<'a, Message>, shown: &mut usize) -> iced::widget::Column<'a, Message> {
    let id = clip.id;
    let e = move |edit: Edit| Message::Edit(id, edit);
    let body = column![
        slider_row("Strength", b.strength, 0.0..=100.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::BlurStrength(v))),
        row![
            text("Shape").size(T_SMALL).color(TEXT_2).width(84),
            widgets::segmented(&[false, true], b.ellipse, |el| if *el { "Oval".into() } else { "Box".into() }, move |el| e(Edit::BlurEllipse(el))),
        ]
        .align_y(Alignment::Center),
        text("Blurs the video and images under it, only inside the box and only while it's on the timeline. Move and size it on the preview; trim it on the timeline.").size(T_CAPTION).color(TEXT_3),
    ]
    .spacing(S3);
    add(col, shown, ctx.section(code::DROPLETS, "Blur", body.into()))
}

/// A mask clip: how soft its edge is, what shape, invert, how big, and where.
fn mask_clip<'a>(clip: &'a Clip, m: &crate::clip::MaskLayer, ctx: &Ctx<'a>, col: iced::widget::Column<'a, Message>, shown: &mut usize) -> iced::widget::Column<'a, Message> {
    let id = clip.id;
    let e = move |edit: Edit| Message::Edit(id, edit);
    let body = column![
        slider_row("Feather", m.feather, 0.0..=100.0, 1.0, |v| format!("{v:.0}"), move |v| e(Edit::MaskFeather(v))),
        row![
            text("Shape").size(T_SMALL).color(TEXT_2).width(84),
            widgets::segmented(&[false, true], m.ellipse, |el| if *el { "Oval".into() } else { "Box".into() }, move |el| e(Edit::MaskEllipse(el))),
        ]
        .align_y(Alignment::Center),
        toggle_row("Invert", m.invert, move |on| e(Edit::MaskInvert(on))),
        text(if m.invert {
            "Hides the video and images under its box; everywhere else stays visible. Move and size it on the preview; trim it on the timeline, or keyframe its position to follow a moving subject."
        } else {
            "Shows only the video and images under its box; everywhere else is hidden. Move and size it on the preview; trim it on the timeline, or keyframe its position to follow a moving subject."
        })
        .size(T_CAPTION)
        .color(TEXT_3),
    ]
    .spacing(S3);
    add(col, shown, ctx.section(code::SCAN_EYE, "Mask", body.into()))
}

/// What the inspector shows besides the clip: edit modes, the range, typed colors,
/// collapsed sections and the search text.
pub struct Ctx<'a> {
    /// The text box of the selected text clip (see `Editor::sync_text_draft`).
    pub text_draft: Option<&'a iced::widget::text_editor::Content>,
    pub zoom_edit: Option<Id>,
    pub crop_edit: bool,
    pub range: Option<(f64, f64)>,
    pub hex: Option<&'a HexDraft>,
    pub collapsed: &'a std::collections::BTreeSet<String>,
    pub query: &'a str,
    /// One preview picture per look in `COLOR_PRESETS`, made from the selected clip's thumbnail
    /// (see `preset_tiles`).
    pub preset_tiles: &'a [iced::widget::image::Handle],
}

/// Words that find a section in the search box, besides its title.
fn keywords(title: &str) -> &'static str {
    match title {
        "Text" => "font size weight bold color align background box type words",
        "Shape" => "fill gradient color radial linear direction thickness ring kind",
        "Layer" => "rotation rotate angle opacity transparency framing fit fill transform",
        "Frame style" => "corners rounded radius border outline stroke drop shadow blur distance strength",
        "Color" => "brightness contrast saturation vibrance warmth temperature tint shadows midtones highlights fade split tone smoothness smooth skin sharpness sharpen vignette grade correction light finish",
        "Color preset" => "look grade style filter natural cinematic beautify vivid warm cool vintage mono black white original",
        "Crop" => "cut trim edges left right top bottom",
        "Blur" => "hide privacy pixelate blur area censor obscure strength oval box",
        "Animation" => "intro outro in out motion fade slide pop zoom duration",
        "Transition" => "crossfade slide push wipe zoom through dip to black duration",
        "Zoom & pan" => "camera focus magnify pan",
        "Speed" => "fast slow time tempo",
        "Audio" => "volume mute fade sound range level loud quiet",
        _ => "",
    }
}

impl<'a> Ctx<'a> {
    /// Whether the search text (if any) finds this section.
    fn matches(&self, title: &str) -> bool {
        let q = self.query.trim().to_lowercase();
        q.is_empty() || q.split_whitespace().all(|w| title.to_lowercase().contains(w) || keywords(title).contains(w))
    }

    fn section(&self, glyph: char, title: &'static str, body: Element<'a, Message>) -> Option<Element<'a, Message>> {
        self.section_with(glyph, title, None, body)
    }

    fn section_reset(&self, glyph: char, title: &'static str, reset: Message, body: Element<'a, Message>) -> Option<Element<'a, Message>> {
        self.section_with(glyph, title, Some(reset), body)
    }

    /// A card with a clickable header that collapses it; hidden when the search doesn't find it.
    fn section_with(&self, glyph: char, title: &'static str, reset: Option<Message>, body: Element<'a, Message>) -> Option<Element<'a, Message>> {
        if !self.matches(title) {
            return None;
        }
        // While searching, every found section is open.
        let open = !self.query.trim().is_empty() || !self.collapsed.contains(title);
        let toggle = button(
            row![
                icon(if open { code::CHEVRON_DOWN } else { code::CHEVRON_RIGHT }, 12).color(TEXT_3),
                tinted(glyph, 13, TEXT_3),
                text(title).font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_2),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding([2, 0])
        .style(|_, _| button::Style::default())
        .on_press(Message::ToggleSection(title));
        let mut head = row![toggle].align_y(Alignment::Center);
        if let (Some(reset), true) = (reset, open) {
            head = head.push(widgets::tip(widgets::icon_button(code::REFRESH_CW, Some(reset)).width(24).height(24), "Reset", None, iced::widget::tooltip::Position::Left));
        }
        let content = if open { column![head, body].spacing(S3) } else { column![head] };
        Some(container(content).padding(S3).width(Fill).style(style::card).into())
    }
}

fn section<'a>(glyph: char, title: &'a str, body: Element<'a, Message>) -> Element<'a, Message> {
    let head = row![tinted(glyph, 13, TEXT_3), text(title).font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_2)].spacing(7).align_y(Alignment::Center);
    container(column![head, body].spacing(S3)).padding(S3).width(Fill).style(style::card).into()
}

/// The search box above the inspector.
pub fn search_bar<'a>(query: &str) -> Element<'a, Message> {
    let mut r = row![
        icon(code::SEARCH, 13).color(TEXT_3),
        text_input("Search settings and effects…", query).id(SEARCH_ID).on_input(Message::InspectorSearch).size(T_SMALL).padding([5, 0]).style(|t, s| {
            let mut st = style::input(t, s);
            st.border = iced::Border { width: 0.0, ..st.border };
            st.background = Color::TRANSPARENT.into();
            st
        }),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if !query.is_empty() {
        r = r.push(button(icon(code::X, 12).color(TEXT_3)).padding(4).style(style::ghost).on_press(Message::InspectorSearch(String::new())));
    }
    container(r).padding([2, 10]).style(style::inset).into()
}

pub const SEARCH_ID: &str = "inspector-search";

fn slider_row<'a>(name: &'a str, value: f64, range: RangeInclusive<f64>, step: f64, fmt: impl Fn(f64) -> String, on_change: impl Fn(f64) -> Message + 'a) -> Element<'a, Message> {
    let (lo, hi) = (*range.start(), *range.end());
    let v = value.clamp(lo, hi);
    // A range around zero (−100…+100) fills from its middle; the others from the left.
    let centered = lo < 0.0 && (lo + hi).abs() < 1e-9;
    let at = ((v - lo) / (hi - lo).max(1e-9)) as f32;
    let control = slider(range, v, on_change).step(step).width(Fill);
    row![
        text(name).size(T_SMALL).color(TEXT_2).width(84),
        if centered { control.style(style::slider_centered(at)) } else { control.style(style::slider) },
        container(text(fmt(value)).font(INTER_MEDIUM).size(T_CAPTION).color(TEXT_1)).width(52).align_right(52),
    ]
    .spacing(S2)
    .align_y(Alignment::Center)
    .into()
}

/// Size in pixels of a Color preset tile's picture (3:2, drawn at about half that, so it stays
/// sharp on high-density screens).
const PRESET_TILE: (u32, u32) = (192, 128);

/// One small picture per look in `COLOR_PRESETS`: the clip's own thumbnail (or, for clips
/// without one, a neutral color chart) run through the same color math as the renderer, so a
/// tile shows what the look really does to this clip.
pub fn preset_tiles(cover: Option<&image::Handle>) -> Vec<image::Handle> {
    let (tw, th) = PRESET_TILE;
    let picture = match cover {
        Some(image::Handle::Rgba { width, height, pixels, .. }) if *width > 0 && *height > 0 && pixels.len() >= (*width * *height * 4) as usize => Some((*width, *height, pixels)),
        _ => None,
    };
    // Where each tile pixel sits in the tile, 0..1 across and down.
    let place = |i: usize| (((i as u32 % tw) as f32 + 0.5) / tw as f32, ((i as u32 / tw) as f32 + 0.5) / th as f32);
    // The source, in display-space floats: the middle of the thumbnail, cut to the tile's shape.
    let source: Vec<[f32; 3]> = (0..(tw * th) as usize)
        .map(|i| {
            let (fx, fy) = place(i);
            match picture {
                Some((w, h, pixels)) => {
                    let (w_f, h_f) = (w as f32, h as f32);
                    let (cut_w, cut_h) = if w_f / h_f > tw as f32 / th as f32 { (h_f * tw as f32 / th as f32, h_f) } else { (w_f, w_f * th as f32 / tw as f32) };
                    let sx = (((w_f - cut_w) / 2.0 + fx * cut_w) as u32).min(w - 1);
                    let sy = (((h_f - cut_h) / 2.0 + fy * cut_h) as u32).min(h - 1);
                    let at = ((sy * w + sx) * 4) as usize;
                    [pixels[at] as f32 / 255.0, pixels[at + 1] as f32 / 255.0, pixels[at + 2] as f32 / 255.0]
                }
                None => chart_color(fx, fy),
            }
        })
        .collect();
    COLOR_PRESETS
        .iter()
        .map(|preset| {
            let mut rgba = Vec::with_capacity(source.len() * 4);
            for (i, &rgb) in source.iter().enumerate() {
                let (fx, fy) = place(i);
                let edge = ((fx * 2.0 - 1.0).powi(2) + (fy * 2.0 - 1.0).powi(2)).sqrt();
                let c = preset.grade.apply(rgb, edge);
                rgba.extend_from_slice(&[(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8, 255]);
            }
            image::Handle::from_rgba(tw, th, rgba)
        })
        .collect()
}

/// A stand-in picture for clips without a thumbnail: a sweep of hues, light at the top and
/// dark at the bottom, so every look has something to show.
fn chart_color(x: f32, y: f32) -> [f32; 3] {
    let hue = (0.02 + x * 0.62) * 6.0;
    let value = 0.92 - 0.62 * y;
    let saturation = 0.55;
    let channel = |offset: f32| {
        let k = (offset + hue) % 6.0;
        value - value * saturation * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [channel(5.0), channel(3.0), channel(1.0)]
}

/// The Color preset grid: three preview tiles a row, one per look in `COLOR_PRESETS`. Clicking
/// a tile applies that look to the clip immediately.
fn color_presets<'a>(current: Grade, tiles: &[image::Handle], on_pick: impl Fn(Grade) -> Message + Clone + 'a) -> Element<'a, Message> {
    let items: Vec<Element<'a, Message>> = COLOR_PRESETS
        .iter()
        .enumerate()
        .map(|(i, preset)| {
            let on_pick = on_pick.clone();
            preset_tile(preset, tiles.get(i).cloned(), current == preset.grade, move || on_pick(preset.grade))
        })
        .collect();
    // Each cell is a 3:2 picture with its name below; the cells grow with the panel.
    iced::widget::grid(items).columns(3).spacing(S1).height(iced::widget::grid::aspect_ratio(100.0, 88.0)).into()
}

/// One preview tile in the Color preset grid: the picture fills the cell above the name.
fn preset_tile<'a>(preset: &'static ColorPreset, tile: Option<image::Handle>, active: bool, on_press: impl Fn() -> Message + 'a) -> Element<'a, Message> {
    let picture: Element<'a, Message> = match tile {
        Some(h) => image(h).width(Fill).height(Fill).content_fit(iced::ContentFit::Cover).border_radius(R_MD - 1.0).into(),
        None => container(space()).width(Fill).height(Fill).style(style::inset).into(),
    };
    // The ring sits around the picture with a small gap, so it never hides the look itself.
    let preview = container(picture).padding(3).width(Fill).height(Fill).style(move |_| container::Style {
        border: iced::Border { color: if active { ACCENT } else { Color::TRANSPARENT }, width: 2.0, radius: (R_MD + 2.0).into() },
        ..Default::default()
    });
    let name = text(preset.name).font(if active { INTER_SEMIBOLD } else { INTER_MEDIUM }).size(T_CAPTION).color(if active { TEXT_1 } else { TEXT_2 }).wrapping(text::Wrapping::None);
    let tile = column![preview, name].spacing(3).width(Fill).height(Fill).align_x(Alignment::Center);
    button(tile).padding(3).width(Fill).height(Fill).style(style::ghost).on_press(on_press()).into()
}

/// The selected characters of a text box, as char indices `start..end` into its text.
pub fn selected_chars(content: &iced::widget::text_editor::Content) -> Option<(usize, usize)> {
    let selected = content.selection().filter(|s| !s.is_empty())?;
    let text = content.text();
    let cursor = content.cursor();
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let offset = |p: iced::widget::text_editor::Position| {
        let mut b = (line_starts.get(p.line).copied().unwrap_or(text.len()) + p.column).min(text.len());
        while !text.is_char_boundary(b) {
            b -= 1;
        }
        b
    };
    let (p, q) = (offset(cursor.position), cursor.selection.map(offset).unwrap_or(text.len()));
    let (mut a, mut b) = (p.min(q), p.max(q));
    // Double-click selects a whole word but reports where the click was: find the selected
    // text nearest to the cursor instead.
    if text.get(a..b) != Some(selected.as_str()) {
        a = text.match_indices(selected.as_str()).map(|(i, _)| i).min_by_key(|i| i.abs_diff(a))?;
        b = a + selected.len();
    }
    let chars = |i: usize| text[..i].chars().count();
    Some((chars(a), chars(b)))
}

/// The style controls for the selected words of a text clip.
fn selection_style<'a>(id: Id, t: &'a crate::clip::TextLayer, a: usize, b: usize, hex: Option<&'a HexDraft>) -> Element<'a, Message> {
    use crate::clip::SpanChange as C;
    let e = move |change: C| Message::Edit(id, Edit::SpanStyle(a, b, change));
    let s = t.style_at(a);
    let words: String = t.text.chars().skip(a).take(b - a).collect::<String>().replace('\n', " ");
    let shown: String = if words.chars().count() > 24 { format!("{}…", words.chars().take(23).collect::<String>()) } else { words };
    let font = if shown.is_ascii() { INTER_MEDIUM } else { HIND };
    let head = row![
        tinted(code::TYPE, 12, ACCENT_HOVER),
        text("Selection").size(T_CAPTION).color(TEXT_3),
        text(format!("“{shown}”")).font(font).size(T_SMALL).color(TEXT_1).wrapping(text::Wrapping::None).width(Fill),
        button(text("Reset").size(T_CAPTION)).padding([2, 8]).style(style::ghost).on_press_maybe((!s.is_plain() || t.spans.iter().any(|sp| sp.start < b && sp.end > a)).then(|| e(C::Clear))),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let weights = [(400u16, "Regular"), (500, "Medium"), (600, "Semibold"), (700, "Bold")];
    let base_weight = t.weight;
    let font_pick = pick_list(&FONTS[..], Some(s.font.as_deref().unwrap_or(t.font.as_str())).and_then(|f| FONTS.iter().find(|x| **x == f).copied()), {
        let base = t.font.clone();
        move |f: &str| e(C::Font((f != base).then(|| f.to_string())))
    })
    .padding([6, 9])
    .text_size(T_BODY)
    .width(Fill)
    .style(style::select)
    .menu_style(style::select_menu);
    let mut body = column![
        head,
        color_row("Color", s.color.unwrap_or(t.color), (id, "span", hex), move |c| e(C::Color(Some(c)))),
        widgets::segmented(&weights.map(|w| w.0), s.weight.unwrap_or(base_weight), move |w| weights.iter().find(|x| x.0 == *w).map(|x| x.1).unwrap_or("").to_string(), move |w| e(C::Weight((w != base_weight).then_some(w)))),
        row![text("Font").size(T_SMALL).color(TEXT_2).width(84), font_pick].align_y(Alignment::Center),
        slider_row("Size", s.scale.unwrap_or(1.0) * 100.0, 50.0..=300.0, 5.0, |v| format!("{v:.0}%"), move |v| e(C::Scale(Some(v / 100.0)))),
        toggle_row("Highlight", s.highlight.is_some(), move |on| e(C::Highlight(on.then(|| Rgba::hex(0xD6FF00))))),
    ]
    .spacing(S3);
    if let Some(hl) = s.highlight {
        body = body.push(color_row("Marker", hl, (id, "span-marker", hex), move |c| e(C::Highlight(Some(c)))));
    }
    body = body.push(toggle_row("Underline", s.underline, move |on| e(C::Underline(on))));
    container(body).padding([10, 10]).width(Fill).style(style::inset).into()
}

/// How a styled word looks in the inspector's text box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanMark {
    color: Option<Color>,
    bold: bool,
}

/// Styled words of `t` per line of its text: (line, byte range in the line, look).
fn span_marks(t: &crate::clip::TextLayer) -> Vec<(usize, std::ops::Range<usize>, SpanMark)> {
    let mut out = Vec::new();
    let line_starts: Vec<usize> = std::iter::once(0).chain(t.text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    for (range, style) in t.styled_runs() {
        let Some(s) = style else { continue };
        // Its own color when it reads on the dark box, otherwise the accent, so it still stands out.
        let readable = |c: Rgba| 0.2126 * c.0 + 0.7152 * c.1 + 0.0722 * c.2 > 0.3;
        let color = s.color.filter(|c| readable(*c)).or(s.highlight.filter(|c| readable(*c))).map(|c| Color::from_rgb(c.0, c.1, c.2)).unwrap_or(ACCENT_HOVER);
        let mark = SpanMark { color: Some(color), bold: s.weight.unwrap_or(t.weight) >= 600 };
        for (line, &start) in line_starts.iter().enumerate() {
            let end = line_starts.get(line + 1).map(|n| n - 1).unwrap_or(t.text.len());
            let (a, b) = (range.start.max(start), range.end.min(end));
            if a < b {
                out.push((line, a - start..b - start, mark));
            }
        }
    }
    out
}

fn span_format(mark: &SpanMark, _: &iced::Theme) -> iced::advanced::text::highlighter::Format<iced::Font> {
    iced::advanced::text::highlighter::Format { color: mark.color, font: mark.bold.then_some(INTER_SEMIBOLD) }
}

/// Feeds `span_marks` to the text box line by line.
pub struct SpanHighlighter {
    marks: Vec<(usize, std::ops::Range<usize>, SpanMark)>,
    line: usize,
}

impl iced::advanced::text::Highlighter for SpanHighlighter {
    type Settings = Vec<(usize, std::ops::Range<usize>, SpanMark)>;
    type Highlight = SpanMark;
    type Iterator<'a> = std::vec::IntoIter<(std::ops::Range<usize>, SpanMark)>;

    fn new(marks: &Self::Settings) -> Self {
        SpanHighlighter { marks: marks.clone(), line: 0 }
    }

    fn update(&mut self, marks: &Self::Settings) {
        self.marks = marks.clone();
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = line;
    }

    fn highlight_line(&mut self, _: &str) -> Self::Iterator<'_> {
        let line = self.line;
        self.line += 1;
        self.marks.iter().filter(|m| m.0 == line).map(|m| (m.1.clone(), m.2)).collect::<Vec<_>>().into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

/// An effect on the clip: its name, and a button that takes it off.
fn applied<'a>(name: impl text::IntoFragment<'a>, remove: Message) -> Element<'a, Message> {
    row![
        container(space().width(6).height(6)).style(style::swatch(ACCENT, 3.0)),
        text(name).font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_1).width(Fill),
        widgets::tip(widgets::icon_button(code::X, Some(remove)).width(24).height(24), "Remove", None, iced::widget::tooltip::Position::Left),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn toggle_row<'a>(name: &'a str, on: bool, on_toggle: impl Fn(bool) -> Message + 'a) -> Element<'a, Message> {
    row![text(name).size(T_SMALL).color(TEXT_2).width(Fill), toggler(on).on_toggle(on_toggle).size(18).style(style::toggler)].align_y(Alignment::Center).into()
}

/// Hex color being typed: (clip, field, text). Kept until it parses, so typing works letter by letter.
pub type HexDraft = (Id, &'static str, String);

/// The color a hex field edits, so a typed value keeps the current transparency.
pub fn current_color(clip: &Clip, field: &str) -> Option<Rgba> {
    match (field, &clip.content) {
        ("text", Content::Text(t)) => Some(t.color),
        ("shape1", Content::Shape(s)) => Some(s.fill),
        ("shape2", Content::Shape(s)) => s.fill_to,
        ("border", _) => Some(clip.look.border_color),
        _ => None,
    }
}

/// The edit that sets a hex field's color.
pub fn hex_edit(field: &str, c: Rgba) -> Option<Edit> {
    Some(match field {
        "text" => Edit::TextColor(c),
        "shape1" => Edit::ShapeFill(c),
        "shape2" => Edit::ShapeFillTo(c),
        "border" => Edit::BorderColor(c),
        _ => return None,
    })
}

fn color_row<'a>(name: &'a str, value: Rgba, field: (Id, &'static str, Option<&'a HexDraft>), on_pick: impl Fn(Rgba) -> Message + Clone + 'a) -> Element<'a, Message> {
    let swatches = row(PALETTE.iter().map(|&hex| {
        let c = Rgba::hex(hex);
        let selected = c.to_hex() == value.to_hex();
        let on_pick = on_pick.clone();
        button(space().width(16).height(16))
            .padding(0)
            .style(move |_, status| button::Style {
                background: Some(Color::from_rgba(c.0, c.1, c.2, 1.0).into()),
                border: iced::Border {
                    color: if selected { Color::WHITE } else if matches!(status, button::Status::Hovered) { LINE_STRONG } else { LINE },
                    width: if selected { 2.0 } else { 1.0 },
                    radius: 9.0.into(),
                },
                ..Default::default()
            })
            .on_press(on_pick(Rgba(c.0, c.1, c.2, value.3)))
            .into()
    }))
    .spacing(5);
    let (clip, key, draft) = field;
    let shown = match draft {
        Some((c, k, text)) if *c == clip && *k == key => text.clone(),
        _ => value.to_hex(),
    };
    let hex = text_input("#FFFFFF", &shown)
        .on_input(move |s| Message::HexDraft(clip, key, s))
        .padding([4, 7])
        .size(T_CAPTION)
        .width(74)
        .style(style::input);
    column![row![text(name).size(T_SMALL).color(TEXT_2).width(Fill), hex].align_y(Alignment::Center), swatches].spacing(6).into()
}

/// A timeline marker: name, color, where it is.
pub fn marker(m: &crate::model::Marker) -> Element<'_, Message> {
    let id = m.id;
    let color = Color::from_rgb(m.color.0, m.color.1, m.color.2);
    let when = if m.duration > 0.0 {
        format!("{} – {}", super::fmt_timecode_short(m.at), super::fmt_timecode_short(m.at + m.duration))
    } else {
        super::fmt_timecode_short(m.at)
    };
    let header = row![
        container(tinted(code::FLAG, 15, color)).center(32).style(style::badge(color)),
        column![
            text_input("Marker name", &m.name).on_input(move |s| Message::MarkerName(id, s)).font(INTER_SEMIBOLD).size(T_BODY).padding([3, 6]).style(style::input),
            text(format!("Timeline marker · {when}")).size(T_CAPTION).color(TEXT_3),
        ]
        .spacing(2)
        .width(Fill),
    ]
    .spacing(S3)
    .align_y(Alignment::Center);
    let swatches = row(super::library::MARKER_COLORS.iter().map(|&hex| {
        let c = Rgba::hex(hex);
        let selected = c.to_hex() == m.color.to_hex();
        button(space().width(18).height(18))
            .padding(0)
            .style(move |_, status| button::Style {
                background: Some(Color::from_rgb(c.0, c.1, c.2).into()),
                border: iced::Border { color: if selected { Color::WHITE } else if matches!(status, button::Status::Hovered) { LINE_STRONG } else { LINE }, width: if selected { 2.0 } else { 1.0 }, radius: 10.0.into() },
                ..Default::default()
            })
            .on_press(Message::MarkerColorOf(id, c))
            .into()
    }))
    .spacing(6);
    let body = column![
        swatches,
        row![
            widgets::secondary_button(Some(code::SKIP_BACK), "Go to marker", Some(Message::Seek(m.at))),
            space().width(Fill),
            widgets::ghost_button(Some(code::TRASH_2), "Delete", Some(Message::DeleteMarker(id))),
        ]
        .align_y(Alignment::Center),
        text("Drag the flag on the ruler to move it. Markers help you find your way; they don't show in the video.").size(T_CAPTION).color(TEXT_3),
    ]
    .spacing(S3);
    widgets::scroll(column![container(header).padding([S3, S3]), section(code::PALETTE, "Color", body.into())].spacing(S2).padding(iced::Padding { top: 0.0, right: 10.0, bottom: 12.0, left: 4.0 })).height(Fill).into()
}

pub fn empty<'a>() -> Element<'a, Message> {
    widgets::empty_state(code::MOUSE_POINTER_2, "Nothing selected", "Select a clip on the timeline to edit its position, style, color and animation.", None)
}

pub fn multiple<'a>(count: usize) -> Element<'a, Message> {
    let _ = icon(code::LAYERS, 1);
    widgets::empty_state(code::LAYERS, "Multiple clips", if count == 2 { "2 clips selected. Select one clip to edit it." } else { "Several clips selected. Select one clip to edit it." }, None)
}
