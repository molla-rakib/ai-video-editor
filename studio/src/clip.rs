//! Clips and everything that can be set on them.
//!
//! Positions are fractions of the canvas (0..1, so they survive resolution
//! changes). Lengths such as corner radius, border width and font size are in
//! "canvas units": pixels at a 1080-pixel short side, scaled for other sizes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::Id;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

impl Rgba {
    pub const WHITE: Rgba = Rgba(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Rgba = Rgba(0.0, 0.0, 0.0, 1.0);

    pub fn hex(rgb: u32) -> Rgba {
        Rgba(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, 1.0)
    }

    pub fn to_hex(self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.0), c(self.1), c(self.2))
    }

    pub fn parse(s: &str) -> Option<Rgba> {
        let s = s.trim().trim_start_matches('#');
        (s.len() == 6).then(|| u32::from_str_radix(s, 16).ok().map(Rgba::hex)).flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Fit {
    /// Whole frame visible (letterboxed if needed).
    #[default]
    Contain,
    /// Frame fills the canvas (edges cropped).
    Cover,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub rotation: f64,
    pub opacity: f64,
    pub fit: Fit,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { x: 0.5, y: 0.5, scale: 1.0, rotation: 0.0, opacity: 1.0, fit: Fit::Contain }
    }
}

/// A slow, continuous idle motion so a still moment never looks frozen ("nothing ever fully
/// stops"). Content-agnostic, independent of `CalloutLayer::pulse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriftKind {
    /// Scale breathes gently between 1.0 and 1.02.
    ScaleBreath,
    /// Position drifts within about 4 canvas-unit pixels.
    PositionDrift,
}

/// Cropping, as fractions of the source frame cut from each edge.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Crop {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Crop {
    /// Largest total crop on one axis, so something always stays visible.
    pub const MAX: f64 = 0.95;

    pub fn is_none(&self) -> bool {
        *self == Crop::default()
    }

    /// The same crop with every edge in range and opposite edges leaving at least 5% visible.
    pub fn clamped(self) -> Crop {
        let c = |v: f64| v.clamp(0.0, Crop::MAX);
        let (mut l, mut t, mut r, mut b) = (c(self.left), c(self.top), c(self.right), c(self.bottom));
        if l + r > Crop::MAX {
            let k = Crop::MAX / (l + r);
            (l, r) = (l * k, r * k);
        }
        if t + b > Crop::MAX {
            let k = Crop::MAX / (t + b);
            (t, b) = (t * k, b * k);
        }
        Crop { left: l, top: t, right: r, bottom: b }
    }
}

/// A blur clip: blurs the video and images under it, inside its box, while it's on
/// the timeline. It sits on a video track like a highlight; position comes from the
/// clip's transform, the box size is in canvas units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BlurLayer {
    pub width: f64,
    pub height: f64,
    /// 0..100.
    pub strength: f64,
    #[serde(default)]
    pub ellipse: bool,
}

impl Default for BlurLayer {
    fn default() -> Self {
        BlurLayer { width: 560.0, height: 200.0, strength: 60.0, ellipse: false }
    }
}

/// Seconds a blur takes to fade in and out at the ends of its clip.
pub const BLUR_FADE: f64 = 0.15;

/// Most blur boxes that can cover one layer at the same moment.
pub const MAX_BLURS: usize = 4;

/// A mask clip: restricts the video and images under it to only its box, while it's on
/// the timeline — the opposite of a blur's "hide this", it's "show only this". `invert`
/// flips it to hide the box instead and show everywhere else (a cutout/vignette). Position
/// comes from the clip's transform, the box size is in canvas units; keyframe the clip's X/Y
/// (the same general keyframe system every clip has) to have the mask follow a moving subject.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MaskLayer {
    pub width: f64,
    pub height: f64,
    /// Edge softness, 0 (hard edge) to 100.
    pub feather: f64,
    #[serde(default)]
    pub ellipse: bool,
    #[serde(default)]
    pub invert: bool,
}

impl Default for MaskLayer {
    fn default() -> Self {
        MaskLayer { width: 480.0, height: 480.0, feather: 15.0, ellipse: true, invert: false }
    }
}

/// Most mask boxes that can cover one layer at the same moment.
pub const MAX_MASKS: usize = 4;

/// Which part of a video clip plays: after "Separate audio" the picture and the
/// sound are two clips of the same media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Part {
    #[default]
    Whole,
    /// Picture only (its sound was separated).
    Picture,
    /// Sound only, on an audio track.
    Sound,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub blur: f64,
    pub offset_y: f64,
    pub opacity: f64,
    pub color: Rgba,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { enabled: false, blur: 40.0, offset_y: 16.0, opacity: 0.45, color: Rgba::BLACK }
    }
}

/// Frame styling for media and shapes: rounded corners, border, drop shadow.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub corner_radius: f64,
    pub border_width: f64,
    pub border_color: Rgba,
    pub shadow: Shadow,
}

impl Default for Look {
    fn default() -> Self {
        Look { corner_radius: 0.0, border_width: 0.0, border_color: Rgba(1.0, 1.0, 1.0, 0.9), shadow: Shadow::default() }
    }
}

/// Color correction. 0 means unchanged for every field. Lift/gamma/gain target the shadows,
/// midtones and highlights separately; the rest are the usual light, color and detail controls.
/// `Grade::apply` is the CPU copy of `layer.wgsl::apply_grade` (used for the preset previews):
/// change both together.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Grade {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub lift: f64,
    pub gamma: f64,
    pub gain: f64,
    /// Green (negative) to magenta (positive).
    pub tint: f64,
    /// Saturation that works mostly on the muted colors, so skin and already-strong colors stay natural.
    pub vibrance: f64,
    /// 0..1: a soft, matte film look (raised blacks, slightly lowered whites).
    pub fade: f64,
    /// Split toning: positive cools the shadows and warms the highlights (the cinema
    /// "teal and orange"), negative does the reverse.
    pub split: f64,
    /// 0..1: smooths skin and fine noise while keeping edges (media only).
    pub smooth: f64,
    /// 0..1: sharpens fine detail (media only).
    pub sharpen: f64,
    /// 0..1: darkens the corners.
    pub vignette: f64,
}

impl Grade {
    pub const NONE: Grade = Grade {
        brightness: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        temperature: 0.0,
        lift: 0.0,
        gamma: 0.0,
        gain: 0.0,
        tint: 0.0,
        vibrance: 0.0,
        fade: 0.0,
        split: 0.0,
        smooth: 0.0,
        sharpen: 0.0,
        vignette: 0.0,
    };

    /// Keeps every field inside the range the renderer expects (old or hand-edited files).
    pub fn clamped(self) -> Grade {
        let s = |v: f64| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        let u = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        Grade {
            brightness: s(self.brightness),
            contrast: s(self.contrast),
            saturation: s(self.saturation),
            temperature: s(self.temperature),
            lift: s(self.lift),
            gamma: s(self.gamma),
            gain: s(self.gain),
            tint: s(self.tint),
            vibrance: s(self.vibrance),
            fade: u(self.fade),
            split: s(self.split),
            smooth: u(self.smooth),
            sharpen: u(self.sharpen),
            vignette: u(self.vignette),
        }
    }

    /// Grades one display-space (sRGB, 0..1) color the way the renderer does, without the
    /// detail controls (smoothness, sharpness). `edge` is the distance from the picture's
    /// center, 0 at the center and about 1.41 in a corner, for the vignette.
    pub fn apply(&self, rgb: [f32; 3], edge: f32) -> [f32; 3] {
        const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
        let g = self.clamped();
        let f = |v: f64| v as f32;
        let luma = |c: [f32; 3]| c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
        let mut c = rgb.map(|v| v.clamp(0.0, 1.0));
        c = c.map(|v| v + f(g.brightness) * 0.5);
        c = c.map(|v| (v - 0.5) * (1.0 + f(g.contrast)) + 0.5);
        let l = luma(c);
        c = c.map(|v| l + (v - l) * (1.0 + f(g.saturation)));
        let spread = (c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])).clamp(0.0, 1.0);
        let l = luma(c);
        c = c.map(|v| l + (v - l) * (1.0 + f(g.vibrance) * (1.0 - spread)));
        let (t, m) = (f(g.temperature), f(g.tint));
        c = [c[0] * (1.0 + t * 0.12) * (1.0 + m * 0.05), c[1] * (1.0 + t * 0.02) * (1.0 - m * 0.09), c[2] * (1.0 - t * 0.12) * (1.0 + m * 0.05)];
        c = c.map(|v| v.clamp(0.0, 1.0));
        c = c.map(|v| v + f(g.lift) * (1.0 - v) * (1.0 - v));
        c = c.map(|v| v.clamp(0.0, 1.0).powf(1.0 / f(g.gamma).exp()));
        c = c.map(|v| (v * (1.0 + f(g.gain) * v)).clamp(0.0, 1.0));
        let l = luma(c);
        let (sh, hi) = ((1.0 - l) * (1.0 - l), l * l);
        let s = f(g.split);
        c = [c[0] + s * (-0.09 * sh + 0.09 * hi), c[1] + s * (0.02 * sh + 0.01 * hi), c[2] + s * (0.08 * sh - 0.07 * hi)];
        let fade = f(g.fade);
        c = c.map(|v| v + fade * (0.11 - 0.17 * v));
        let v = f(g.vignette) * smoothstep(0.6, 1.6, edge) * 0.8;
        c.map(|x| (x * (1.0 - v)).clamp(0.0, 1.0))
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One named look in the Color preset section of the inspector.
pub struct ColorPreset {
    pub name: &'static str,
    pub grade: Grade,
}

/// A short list of looks that each do one clear job, plus "Original" to clear the color back
/// to the untouched source. Every preset is built from the same controls as the Color
/// section in the inspector, so any of them can be fine-tuned there afterwards.
pub const COLOR_PRESETS: &[ColorPreset] = &[
    ColorPreset { name: "Original", grade: Grade::NONE },
    // True to life, only a little clearer: the safe choice for screen recordings and talking heads.
    ColorPreset { name: "Natural", grade: Grade { contrast: 0.08, vibrance: 0.18, gamma: 0.03, sharpen: 0.2, ..Grade::NONE } },
    // Cinema look: deeper contrast, cool shadows and warm highlights, soft blacks, dark corners.
    ColorPreset { name: "Cinematic", grade: Grade { contrast: 0.16, saturation: -0.06, gamma: -0.04, split: 0.5, fade: 0.15, vignette: 0.3, ..Grade::NONE } },
    // Flattering for faces: smooth skin, a little brighter and warmer, gentle contrast.
    ColorPreset { name: "Beautify", grade: Grade { brightness: 0.03, contrast: -0.04, gamma: 0.08, temperature: 0.08, tint: 0.05, vibrance: 0.12, smooth: 0.6, fade: 0.05, ..Grade::NONE } },
    // Punchy and colorful without burning skin tones.
    ColorPreset { name: "Vivid", grade: Grade { contrast: 0.12, saturation: 0.05, vibrance: 0.4, sharpen: 0.25, ..Grade::NONE } },
    // Golden, sunny light.
    ColorPreset { name: "Warm", grade: Grade { contrast: 0.06, temperature: 0.5, gamma: 0.03, vibrance: 0.12, ..Grade::NONE } },
    // Clean, modern and slightly blue: suits tech and product shots.
    ColorPreset { name: "Cool", grade: Grade { contrast: 0.1, saturation: -0.05, temperature: -0.42, tint: -0.03, ..Grade::NONE } },
    // Faded, nostalgic film stock.
    ColorPreset { name: "Vintage", grade: Grade { contrast: -0.04, saturation: -0.16, temperature: 0.24, split: 0.25, fade: 0.36, vignette: 0.3, ..Grade::NONE } },
    // Black and white with real contrast.
    ColorPreset { name: "Mono", grade: Grade { contrast: 0.22, saturation: -1.0, gamma: -0.03, fade: 0.06, vignette: 0.2, ..Grade::NONE } },
];

/// A volume change on part of a clip (from a range selection). Times are clip-relative.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gain {
    pub id: Id,
    pub at: f64,
    pub duration: f64,
    /// Multiplies the clip volume inside the span (0 = silent).
    pub volume: f64,
}

/// Length of the smooth ramp at each edge of a `Gain`, so changes don't click.
pub const GAIN_RAMP: f64 = 0.06;

/// A simple 3-band parametric EQ (low/mid/high shelf-ish bands), in dB. 0 on every
/// band means flat — no filter is added to the mix for it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Eq {
    pub low: f64,
    pub mid: f64,
    pub high: f64,
}

impl Eq {
    pub fn is_flat(&self) -> bool {
        self.low == 0.0 && self.mid == 0.0 && self.high == 0.0
    }
}

/// A light compressor to even out a clip's level. `amount` (0–100%) scales a fixed
/// threshold/ratio/attack/release curve; 0 or disabled adds no filter to the mix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compressor {
    pub enabled: bool,
    pub amount: f64,
}

impl Default for Compressor {
    fn default() -> Self {
        Compressor { enabled: false, amount: 50.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub volume: f64,
    pub muted: bool,
    pub fade_in: f64,
    pub fade_out: f64,
    pub gains: Vec<Gain>,
    pub eq: Eq,
    pub compressor: Compressor,
    /// Noise reduction strength, 0 (off) to 97.
    pub denoise: f64,
}

impl Default for Audio {
    fn default() -> Self {
        Audio { volume: 1.0, muted: false, fade_in: 0.0, fade_out: 0.0, gains: Vec::new(), eq: Eq::default(), compressor: Compressor::default(), denoise: 0.0 }
    }
}

impl Audio {
    /// The range-gain factor at clip-relative time `t` (1 outside every range).
    pub fn gain_at(&self, t: f64) -> f64 {
        self.gains
            .iter()
            .map(|g| {
                let ramp = GAIN_RAMP.min(g.duration / 2.0).max(1e-4);
                let k = ((t - g.at) / ramp).min((g.at + g.duration - t) / ramp).clamp(0.0, 1.0);
                1.0 + (g.volume - 1.0) * k
            })
            .product()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Ease {
    Linear,
    #[default]
    Smooth,
    EaseIn,
    EaseOut,
    /// Overshoots slightly, for pops.
    Back,
}

impl Ease {
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            // Cubic in-out.
            Ease::Smooth => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Ease::EaseIn => t * t * t,
            Ease::EaseOut => 1.0 - (1.0 - t).powi(3),
            Ease::Back => {
                let (c1, c3) = (1.70158, 2.70158);
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
        }
    }
}

/// A Camtasia-style zoom on part of the frame. Times are relative to the clip start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Zoom {
    pub id: Id,
    pub at: f64,
    pub duration: f64,
    pub scale: f64,
    /// Point of the frame to zoom into, as fractions of the frame.
    pub focus_x: f64,
    pub focus_y: f64,
    /// Seconds to ease in and out.
    pub ramp: f64,
}

/// The camera at one moment: how much the frame is magnified and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub scale: f64,
    pub focus_x: f64,
    pub focus_y: f64,
}

impl Camera {
    pub const IDENTITY: Camera = Camera { scale: 1.0, focus_x: 0.5, focus_y: 0.5 };

    fn lerp(a: Camera, b: Camera, t: f64) -> Camera {
        Camera { scale: a.scale + (b.scale - a.scale) * t, focus_x: a.focus_x + (b.focus_x - a.focus_x) * t, focus_y: a.focus_y + (b.focus_y - a.focus_y) * t }
    }
}

/// Camera at clip-relative time `t`. Back-to-back zooms pan directly from one to the next.
pub fn camera_at(zooms: &[Zoom], t: f64) -> Camera {
    let mut sorted: Vec<&Zoom> = zooms.iter().collect();
    sorted.sort_by(|a, b| a.at.total_cmp(&b.at));
    for (i, z) in sorted.iter().enumerate() {
        let target = Camera { scale: z.scale, focus_x: z.focus_x, focus_y: z.focus_y };
        let (start, end) = (z.at, z.at + z.duration);
        if t < start || t > end {
            continue;
        }
        // A zoom without a ramp holds its framing from its first moment to its last (freeze frames).
        let ramp = z.ramp.clamp(0.0, z.duration / 2.0);
        // Coming from the previous zoom if it ends exactly where this one starts.
        let from = match i.checked_sub(1).map(|j| sorted[j]) {
            Some(p) if (p.at + p.duration - start).abs() < 1e-3 => Camera { scale: p.scale, focus_x: p.focus_x, focus_y: p.focus_y },
            _ => Camera::IDENTITY,
        };
        let next_adjacent = sorted.get(i + 1).is_some_and(|n| (n.at - end).abs() < 1e-3);
        if ramp > 1e-9 && t < start + ramp {
            return Camera::lerp(from, target, Ease::Smooth.apply((t - start) / ramp));
        }
        if ramp > 1e-9 && t > end - ramp && !next_adjacent {
            return Camera::lerp(target, Camera::IDENTITY, Ease::Smooth.apply((t - (end - ramp)) / ramp));
        }
        return target;
    }
    Camera::IDENTITY
}

/// A one-shot "button press" pulse at one moment: scale dips and recovers, like a real click.
/// Times are relative to the clip start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PressPulse {
    pub id: Id,
    pub at: f64,
    pub duration: f64,
}

impl PressPulse {
    /// 8 frames at 60fps.
    pub const DEFAULT_DURATION: f64 = 8.0 / 60.0;
    /// How far the scale dips at the midpoint of the press.
    const DIP: f64 = 0.04;
}

/// Idle-drift offset at clip-relative time `t`: `(dx, dy)` in canvas units and a scale
/// multiplier. Slower than `CalloutLayer::pulse`'s breathing (0.4 Hz vs. ~1.1 Hz) so it reads as
/// restlessness, not pulsing. Bounded and non-cumulative: safe to evaluate at any `t`, including
/// on a loop, since it never integrates away from its starting point.
pub fn idle_drift(kind: DriftKind, t: f64) -> (f64, f64, f64) {
    const HZ: f64 = 0.4;
    let osc = |phase: f64| (t * std::f64::consts::TAU * HZ + phase).sin();
    match kind {
        DriftKind::ScaleBreath => (0.0, 0.0, 1.0 + 0.01 * (osc(0.0) + 1.0)),
        DriftKind::PositionDrift => (4.0 * osc(0.0), 4.0 * osc(std::f64::consts::FRAC_PI_2), 1.0),
    }
}

/// Scale multiplier from every press pulse active at clip-relative time `t` (1.0 outside any
/// press window; dips to `1.0 - DIP` at the midpoint of a press and recovers by its end).
pub fn press_scale(presses: &[PressPulse], t: f64) -> f32 {
    presses
        .iter()
        .map(|p| {
            let d = p.duration.max(1e-6);
            let k = ((t - p.at) / d).clamp(0.0, 1.0);
            1.0 - PressPulse::DIP * (k * std::f64::consts::PI).sin()
        })
        .product::<f64>() as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Prop {
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Clip-relative time.
    pub t: f64,
    pub value: f64,
    /// Easing from this keyframe to the next.
    pub ease: Ease,
}

/// Value of an animated property at `t`, or `None` if it has no keyframes.
pub fn sample(keys: &[Keyframe], t: f64) -> Option<f64> {
    let first = keys.first()?;
    if t <= first.t {
        return Some(first.value);
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.t {
            let span = (b.t - a.t).max(1e-9);
            return Some(a.value + (b.value - a.value) * a.ease.apply((t - a.t) / span));
        }
    }
    Some(keys.last().unwrap().value)
}

/// Ready-made entrance/exit animations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MotionKind {
    Fade,
    SlideUp,
    SlideDown,
    SlideLeft,
    SlideRight,
    Pop,
    ZoomIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Motion {
    pub kind: MotionKind,
    pub duration: f64,
    /// Easing of the entrance/exit progress. `None` (the default, for projects saved before
    /// this field existed) falls back to the engine's old hard-coded behavior: `EaseOut` for
    /// intro, `EaseIn` for outro — see `Motion::ease_or_default`.
    #[serde(default)]
    pub ease: Option<Ease>,
    /// Seconds after the clip starts before this motion begins (for staggering a group's
    /// entrances). Only meaningful on `intro`.
    #[serde(default)]
    pub delay: f64,
}

impl Motion {
    /// `ease` if set, else the engine's old hard-coded default for an intro (`exit: false`) or
    /// outro (`exit: true`) motion.
    pub fn ease_or_default(self, exit: bool) -> Ease {
        self.ease.unwrap_or(if exit { Ease::EaseIn } else { Ease::EaseOut })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionKind {
    Crossfade,
    SlideLeft,
    SlideUp,
    Push,
    ZoomThrough,
    Wipe,
    SlideRight,
    SlideDown,
    /// Fades out to black, then in from black.
    DipToBlack,
}

/// Transition from the previous clip on the same track into this one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextLayer {
    pub text: String,
    pub font: String,
    pub weight: u16,
    pub size: f64,
    pub color: Rgba,
    pub align: Align,
    pub line_height: f64,
    pub letter_spacing: f64,
    /// Optional box behind the text.
    pub background: Option<Rgba>,
    pub padding: f64,
    /// Words styled differently from the rest: sorted, non-overlapping character ranges.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<TextSpan>,
    /// Kinetic reveal: words fade/ease in one after another instead of all at once.
    pub reveal: Option<TextReveal>,
}

/// Word-by-word kinetic text reveal: each word starts `stagger` seconds after the previous one
/// and eases in over `duration` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextReveal {
    pub duration: f64,
    pub stagger: f64,
}

impl Default for TextReveal {
    fn default() -> Self {
        TextReveal { duration: 0.35, stagger: 0.05 }
    }
}

/// Part of a text layer (characters `start..end`, counted in Unicode chars) with its own style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    #[serde(flatten)]
    pub style: SpanStyle,
}

/// What a span changes; unset fields follow the layer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpanStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgba>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Size relative to the layer's (1.0 = the same).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// A marker-pen box behind the characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<Rgba>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
}

impl SpanStyle {
    pub fn is_plain(&self) -> bool {
        *self == SpanStyle::default()
    }
}

/// One change to the style of part of a text layer.
#[derive(Debug, Clone, PartialEq)]
pub enum SpanChange {
    Color(Option<Rgba>),
    Weight(Option<u16>),
    Font(Option<String>),
    Scale(Option<f64>),
    Highlight(Option<Rgba>),
    Underline(bool),
    /// Back to the layer's style.
    Clear,
}

impl SpanChange {
    fn apply(&self, s: &mut SpanStyle) {
        match self.clone() {
            SpanChange::Color(c) => s.color = c,
            SpanChange::Weight(w) => s.weight = w,
            SpanChange::Font(f) => s.font = f,
            SpanChange::Scale(v) => s.scale = v.map(|v| v.clamp(0.25, 4.0)).filter(|v| (v - 1.0).abs() > 1e-6),
            SpanChange::Highlight(c) => s.highlight = c,
            SpanChange::Underline(on) => s.underline = on,
            SpanChange::Clear => *s = SpanStyle::default(),
        }
    }
}

impl TextLayer {
    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// The style of the character at `i` (plain if no span covers it).
    pub fn style_at(&self, i: usize) -> SpanStyle {
        self.spans.iter().find(|s| s.start <= i && i < s.end).map(|s| s.style.clone()).unwrap_or_default()
    }

    /// Applies `change` to characters `start..end`, keeping whatever else those characters had.
    pub fn style_range(&mut self, start: usize, end: usize, change: &SpanChange) {
        let end = end.min(self.char_len());
        if start >= end {
            return;
        }
        // Cut the spans at the range's edges, so every span is fully inside or outside it.
        let mut runs: Vec<TextSpan> = Vec::new();
        for s in self.spans.drain(..) {
            for (a, b) in [(s.start, s.end.min(start)), (s.start.max(start), s.end.min(end)), (s.start.max(end), s.end)] {
                if a < b {
                    runs.push(TextSpan { start: a, end: b, style: s.style.clone() });
                }
            }
        }
        // Plain gaps inside the range become spans too, so they take the change.
        let mut at = start;
        let inside: Vec<(usize, usize)> = runs.iter().filter(|r| r.start >= start && r.end <= end).map(|r| (r.start, r.end)).collect();
        for (a, b) in inside {
            if at < a {
                runs.push(TextSpan { start: at, end: a, style: SpanStyle::default() });
            }
            at = b;
        }
        if at < end {
            runs.push(TextSpan { start: at, end, style: SpanStyle::default() });
        }
        for r in runs.iter_mut().filter(|r| r.start >= start && r.end <= end) {
            change.apply(&mut r.style);
        }
        self.spans = runs;
        self.normalize_spans();
    }

    /// Replaces the text, keeping each style on the characters it was on: text typed inside
    /// a styled word takes its style, text typed at its edges doesn't, and typing over a
    /// selection keeps the style it started with.
    pub fn set_text(&mut self, text: String) {
        let old: Vec<char> = self.text.chars().collect();
        let new: Vec<char> = text.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
        let (old_tail, new_tail) = (old.len() - suffix, new.len() - suffix);
        // Characters before the change stay; those after it move with the length change;
        // a span reaching into the replaced part covers the new text there.
        let start = |p: usize| if p < prefix { p } else if p >= old_tail { p - old_tail + new_tail } else { prefix };
        let end = |p: usize| if p <= prefix { p } else if p >= old_tail { p - old_tail + new_tail } else { new_tail };
        for s in &mut self.spans {
            (s.start, s.end) = (start(s.start), end(s.end));
        }
        self.text = text;
        self.normalize_spans();
    }

    /// Sorted, inside the text, no empty or plain spans, neighbors with the same style merged.
    fn normalize_spans(&mut self) {
        let len = self.char_len();
        let mut spans: Vec<TextSpan> = std::mem::take(&mut self.spans)
            .into_iter()
            .map(|s| TextSpan { start: s.start.min(len), end: s.end.min(len), style: s.style })
            .filter(|s| s.start < s.end && !s.style.is_plain())
            .collect();
        spans.sort_by_key(|s| s.start);
        let mut out: Vec<TextSpan> = Vec::new();
        for mut s in spans {
            if let Some(last) = out.last_mut() {
                // Overlaps (from hand-edited files or the AI) keep the earlier span.
                s.start = s.start.max(last.end);
                if s.start >= s.end {
                    continue;
                }
                if last.end == s.start && last.style == s.style {
                    last.end = s.end;
                    continue;
                }
            }
            out.push(s);
        }
        self.spans = out;
    }

    /// Byte ranges of the text with the style of each (plain parts included), for rendering.
    pub fn styled_runs(&self) -> Vec<(std::ops::Range<usize>, Option<&SpanStyle>)> {
        let bytes: Vec<usize> = self.text.char_indices().map(|(b, _)| b).chain(std::iter::once(self.text.len())).collect();
        let byte = |c: usize| bytes[c.min(bytes.len() - 1)];
        let mut out = Vec::new();
        let mut at = 0;
        for s in &self.spans {
            if s.start > at {
                out.push((byte(at)..byte(s.start), None));
            }
            out.push((byte(s.start)..byte(s.end), Some(&s.style)));
            at = s.end;
        }
        if byte(at) < self.text.len() {
            out.push((byte(at)..self.text.len(), None));
        }
        out
    }
}

impl Default for TextLayer {
    fn default() -> Self {
        TextLayer {
            text: "Your headline".into(),
            font: "Inter".into(),
            weight: 700,
            size: 96.0,
            color: Rgba::WHITE,
            align: Align::Center,
            line_height: 1.15,
            letter_spacing: -1.0,
            background: None,
            padding: 24.0,
            spans: Vec::new(),
            reveal: None,
        }
    }
}

/// A number that counts up or down from `from` to `to` over `duration` seconds. `format`
/// supplies styling only (font, size, color, align, …); its `text` is overwritten every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CounterLayer {
    pub from: f64,
    pub to: f64,
    pub duration: f64,
    #[serde(default)]
    pub decimals: u32,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    pub format: TextLayer,
}

impl CounterLayer {
    pub fn new(from: f64, to: f64) -> CounterLayer {
        CounterLayer { from, to, duration: 1.0, decimals: 0, prefix: String::new(), suffix: String::new(), format: TextLayer::default() }
    }

    /// The displayed value at clip-relative time `t`: eased, continuous, never a one-frame jump.
    pub fn value_at(&self, t: f64) -> f64 {
        let p = Ease::EaseOut.apply((t / self.duration.max(1e-6)).clamp(0.0, 1.0));
        self.from + (self.to - self.from) * p
    }

    /// The text to display at clip-relative time `t`.
    pub fn text_at(&self, t: f64) -> String {
        format!("{}{:.*}{}", self.prefix, self.decimals as usize, self.value_at(t), self.suffix)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Triangle,
    Star,
    Hexagon,
    /// An outlined ellipse.
    Ring,
    /// An outlined (rounded) rectangle.
    Frame,
}

impl std::fmt::Display for ShapeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 7] = [ShapeKind::Rectangle, ShapeKind::Ellipse, ShapeKind::Triangle, ShapeKind::Star, ShapeKind::Hexagon, ShapeKind::Ring, ShapeKind::Frame];

    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Triangle => "Triangle",
            ShapeKind::Star => "Star",
            ShapeKind::Hexagon => "Hexagon",
            ShapeKind::Ring => "Ring",
            ShapeKind::Frame => "Frame",
        }
    }
}

fn default_angle() -> f64 {
    180.0
}

fn default_thickness() -> f64 {
    24.0
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShapeLayer {
    pub kind: ShapeKind,
    /// Size in canvas units.
    pub width: f64,
    pub height: f64,
    pub fill: Rgba,
    /// Second color, for a gradient.
    pub fill_to: Option<Rgba>,
    /// Gradient direction in degrees, CSS-style: 180 runs top to bottom, 90 left to right.
    #[serde(default = "default_angle")]
    pub gradient_angle: f64,
    /// A radial gradient from the center instead of a linear one.
    #[serde(default)]
    pub radial: bool,
    /// Outline thickness for rings and frames, canvas units.
    #[serde(default = "default_thickness")]
    pub thickness: f64,
}

impl ShapeLayer {
    pub fn new(kind: ShapeKind, width: f64, height: f64, fill: Rgba, fill_to: Option<Rgba>) -> ShapeLayer {
        ShapeLayer { kind, width, height, fill, fill_to, gradient_angle: 180.0, radial: false, thickness: 24.0 }
    }
}

/// A small set of bundled glyphs for callouts; `ui::icon_codes` maps these to the Lucide font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalloutIcon {
    Check,
    Arrow,
    Star,
    Sparkle,
    Zap,
}

impl std::fmt::Display for CalloutIcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl CalloutIcon {
    pub const ALL: [CalloutIcon; 5] = [CalloutIcon::Check, CalloutIcon::Arrow, CalloutIcon::Star, CalloutIcon::Sparkle, CalloutIcon::Zap];

    pub fn label(self) -> &'static str {
        match self {
            CalloutIcon::Check => "Check",
            CalloutIcon::Arrow => "Arrow",
            CalloutIcon::Star => "Star",
            CalloutIcon::Sparkle => "Sparkle",
            CalloutIcon::Zap => "Zap",
        }
    }
}

fn default_callout_size() -> f64 {
    96.0
}

/// A bundled icon glyph with a glow (drawn from the clip's own `look.shadow`), that pops in and
/// can pulse — a tasteful accent at an important moment, not a drawing tool like `ShapeLayer`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalloutLayer {
    pub icon: CalloutIcon,
    /// Glyph size, canvas units.
    #[serde(default = "default_callout_size")]
    pub size: f64,
    pub color: Rgba,
    /// A slow breathing scale/glow while the clip is on screen.
    #[serde(default)]
    pub pulse: bool,
}

impl CalloutLayer {
    pub fn new(icon: CalloutIcon, color: Rgba) -> CalloutLayer {
        CalloutLayer { icon, size: default_callout_size(), color, pulse: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Content {
    /// Video, image or audio from the media library. `src_in` is where playback starts in the file.
    Media { asset: Id, src_in: f64, speed: f64 },
    Text(TextLayer),
    Shape(ShapeLayer),
    Blur(BlurLayer),
    Mask(MaskLayer),
    Callout(CalloutLayer),
    Counter(CounterLayer),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Id,
    pub track: Id,
    /// Timeline position and length, in seconds.
    pub start: f64,
    pub duration: f64,
    pub content: Content,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default)]
    pub look: Look,
    #[serde(default)]
    pub grade: Grade,
    #[serde(default)]
    pub audio: Audio,
    #[serde(default)]
    pub zooms: Vec<Zoom>,
    #[serde(default)]
    pub keyframes: BTreeMap<Prop, Vec<Keyframe>>,
    #[serde(default)]
    pub intro: Option<Motion>,
    #[serde(default)]
    pub outro: Option<Motion>,
    #[serde(default)]
    pub transition: Option<Transition>,
    #[serde(default)]
    pub crop: Crop,
    #[serde(default)]
    pub part: Part,
    #[serde(default)]
    pub presses: Vec<PressPulse>,
    #[serde(default)]
    pub idle_drift: Option<DriftKind>,
    /// Clips sharing a group id move, trim, copy and delete together (a video and its
    /// separated sound, or anything the user grouped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Id>,
    /// A disabled clip stays on the timeline but is neither shown nor heard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// A freeze frame: a video clip that holds the frame at `src_in` for its whole length.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub freeze: bool,
}

impl Clip {
    pub fn new(id: Id, track: Id, start: f64, duration: f64, content: Content) -> Clip {
        Clip {
            id,
            track,
            start,
            duration,
            content,
            name: None,
            transform: Transform::default(),
            look: Look::default(),
            grade: Grade::default(),
            audio: Audio::default(),
            zooms: Vec::new(),
            keyframes: BTreeMap::new(),
            intro: None,
            outro: None,
            transition: None,
            crop: Crop::default(),
            part: Part::Whole,
            presses: Vec::new(),
            idle_drift: None,
            group: None,
            disabled: false,
            freeze: false,
        }
    }

    pub fn end(&self) -> f64 {
        self.start + self.duration
    }

    pub fn asset(&self) -> Option<Id> {
        match self.content {
            Content::Media { asset, .. } => Some(asset),
            _ => None,
        }
    }

    pub fn speed(&self) -> f64 {
        match self.content {
            Content::Media { speed, .. } => speed,
            _ => 1.0,
        }
    }

    /// Source-file time shown at timeline time `t`.
    pub fn source_time(&self, t: f64) -> f64 {
        match self.content {
            Content::Media { src_in, .. } if self.freeze => src_in,
            Content::Media { src_in, speed, .. } => src_in + (t - self.start) * speed,
            _ => 0.0,
        }
    }

    /// Timeline time at which source-file time `s` plays: the inverse of `source_time`.
    pub fn timeline_time(&self, s: f64) -> f64 {
        match self.content {
            Content::Media { src_in, speed, .. } => self.start + (s - src_in) / speed,
            _ => self.start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom(at: f64, duration: f64, scale: f64, fx: f64) -> Zoom {
        Zoom { id: 1, at, duration, scale, focus_x: fx, focus_y: 0.5, ramp: 0.5 }
    }

    #[test]
    fn camera_ramps_in_holds_and_ramps_out() {
        let z = [zoom(1.0, 3.0, 2.0, 0.25)];
        assert_eq!(camera_at(&z, 0.5), Camera::IDENTITY);
        assert_eq!(camera_at(&z, 2.0).scale, 2.0);
        let mid_in = camera_at(&z, 1.25);
        assert!(mid_in.scale > 1.0 && mid_in.scale < 2.0);
        let mid_out = camera_at(&z, 3.75);
        assert!(mid_out.scale > 1.0 && mid_out.scale < 2.0);
        assert_eq!(camera_at(&z, 5.0), Camera::IDENTITY);
    }

    #[test]
    fn back_ease_overshoots() {
        // The whole point of the "bouncy" entrance: progress exceeds 1.0 partway through.
        let over = (1..100).map(|i| Ease::Back.apply(i as f64 / 100.0)).fold(0.0f64, f64::max);
        assert!(over > 1.0, "Ease::Back should overshoot past 1.0, got max {over}");
        assert!(Ease::Back.apply(0.0).abs() < 1e-9);
        assert!((Ease::Back.apply(1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn motion_ease_or_default_matches_old_hardcoded_behavior() {
        let m = Motion { kind: MotionKind::Fade, duration: 1.0, ease: None, delay: 0.0 };
        assert_eq!(m.ease_or_default(false), Ease::EaseOut, "intro used to always ease out");
        assert_eq!(m.ease_or_default(true), Ease::EaseIn, "outro used to always ease in");
        let m = Motion { kind: MotionKind::Fade, duration: 1.0, ease: Some(Ease::Back), delay: 0.0 };
        assert_eq!(m.ease_or_default(false), Ease::Back, "an explicit ease overrides the default");
    }

    #[test]
    fn press_pulse_dips_and_recovers() {
        let presses = [PressPulse { id: 1, at: 1.0, duration: 0.2 }];
        assert_eq!(press_scale(&presses, 0.5), 1.0, "unaffected before the press starts");
        assert!((press_scale(&presses, 1.0) - 1.0).abs() < 1e-6, "no dip right at the start");
        assert!(press_scale(&presses, 1.1) < 0.97, "dips at the midpoint");
        assert!((press_scale(&presses, 1.2) - 1.0).abs() < 1e-6, "recovers by the end");
        assert_eq!(press_scale(&presses, 2.0), 1.0, "unaffected long after the press");
    }

    #[test]
    fn idle_drift_stays_bounded_and_oscillates() {
        let samples: Vec<f64> = (0..50).map(|i| idle_drift(DriftKind::ScaleBreath, i as f64 * 0.05).2).collect();
        assert!(samples.iter().all(|&s| (1.0..=1.02 + 1e-9).contains(&s)), "scale breath should stay within [1.0, 1.02]");
        assert!(samples.iter().any(|&s| s > 1.015) && samples.iter().any(|&s| s < 1.005), "it should actually oscillate, not sit still");
        let (dx, dy, mul) = idle_drift(DriftKind::PositionDrift, 1.0);
        assert!(dx.abs() <= 4.0 + 1e-9 && dy.abs() <= 4.0 + 1e-9, "position drift should stay within about 4 units");
        assert_eq!(mul, 1.0, "position drift doesn't also change scale");
    }

    #[test]
    fn counter_counts_continuously_and_never_jumps() {
        let mut c = CounterLayer::new(0.0, 100.0);
        c.duration = 1.0;
        c.decimals = 1;
        c.prefix = "$".into();
        assert_eq!(c.value_at(0.0), 0.0);
        assert_eq!(c.value_at(1.0), 100.0);
        assert_eq!(c.value_at(5.0), 100.0, "holds at the target after it finishes");
        let mid = c.value_at(0.5);
        assert!(mid > 0.0 && mid < 100.0, "in between, it's in between");
        // Ease-out: the first half covers more ground than the second half.
        assert!(c.value_at(0.5) > 50.0, "ease-out front-loads the motion");
        assert_eq!(c.text_at(1.0), "$100.0");
    }

    #[test]
    fn adjacent_zooms_pan_without_zooming_out() {
        let z = [zoom(1.0, 2.0, 2.0, 0.25), zoom(3.0, 2.0, 2.0, 0.75)];
        // End of the first zoom holds instead of ramping out.
        assert_eq!(camera_at(&z, 2.9).scale, 2.0);
        // Start of the second pans across at full zoom.
        let pan = camera_at(&z, 3.25);
        assert!((pan.scale - 2.0).abs() < 1e-9);
        assert!(pan.focus_x > 0.25 && pan.focus_x < 0.75);
    }

    #[test]
    fn keyframes_interpolate_with_easing() {
        let keys = [Keyframe { t: 0.0, value: 0.0, ease: Ease::Linear }, Keyframe { t: 2.0, value: 10.0, ease: Ease::Linear }];
        assert_eq!(sample(&keys, -1.0), Some(0.0));
        assert_eq!(sample(&keys, 1.0), Some(5.0));
        assert_eq!(sample(&keys, 3.0), Some(10.0));
        assert_eq!(sample(&[], 1.0), None);
    }

    #[test]
    fn timeline_time_is_the_inverse_of_source_time() {
        let c = Clip::new(1, 1, 10.0, 5.0, Content::Media { asset: 1, src_in: 2.0, speed: 2.0 });
        // At timeline 10s (the clip's start) the source is at src_in; 1s of timeline is 2s of
        // source at 2x speed.
        assert_eq!(c.source_time(10.0), 2.0);
        assert_eq!(c.source_time(11.0), 4.0);
        assert_eq!(c.timeline_time(2.0), 10.0);
        assert_eq!(c.timeline_time(4.0), 11.0);
        for t in [10.0, 10.5, 12.3] {
            assert!((c.timeline_time(c.source_time(t)) - t).abs() < 1e-9, "round trip at t={t}");
        }
    }

    #[test]
    fn crop_always_leaves_part_of_the_frame() {
        let c = Crop { left: 0.8, top: -0.2, right: 0.6, bottom: 0.3 }.clamped();
        assert!((c.left + c.right - Crop::MAX).abs() < 1e-9, "opposite edges are scaled back");
        assert!(c.left > c.right, "keeping their proportions");
        assert_eq!((c.top, c.bottom), (0.0, 0.3));
        assert!(Crop::default().is_none());
    }

    #[test]
    fn colors_round_trip_through_hex() {
        assert_eq!(Rgba::parse("#7C6DFF").unwrap().to_hex(), "#7C6DFF");
        assert!(Rgba::parse("nope").is_none());
    }

    fn styled(text: &str) -> TextLayer {
        TextLayer { text: text.into(), ..Default::default() }
    }

    /// The styled words, as text.
    fn words(t: &TextLayer) -> Vec<String> {
        t.spans.iter().map(|s| t.text.chars().skip(s.start).take(s.end - s.start).collect()).collect()
    }

    #[test]
    fn styling_part_of_a_text_keeps_the_rest() {
        let mut t = styled("Make it fast today");
        t.style_range(8, 12, &SpanChange::Color(Some(Rgba::hex(0xD6FF00))));
        assert_eq!(words(&t), ["fast"]);
        // Bold over a range that half overlaps: the overlap keeps its color too.
        t.style_range(5, 10, &SpanChange::Weight(Some(800)));
        assert_eq!(words(&t), ["it ", "fa", "st"]);
        assert_eq!(t.style_at(9), SpanStyle { color: Some(Rgba::hex(0xD6FF00)), weight: Some(800), ..Default::default() });
        assert_eq!(t.style_at(6).color, None);
        assert_eq!(t.style_at(11).weight, None);
        // Clearing everything leaves no spans; neighbors with the same style merge.
        t.style_range(0, 18, &SpanChange::Clear);
        assert!(t.spans.is_empty());
        t.style_range(0, 4, &SpanChange::Underline(true));
        t.style_range(4, 7, &SpanChange::Underline(true));
        assert_eq!(words(&t), ["Make it"]);
        // A size back at 100% is no change.
        t.style_range(0, 7, &SpanChange::Underline(false));
        t.style_range(0, 4, &SpanChange::Scale(Some(1.0)));
        assert!(t.spans.is_empty());
    }

    #[test]
    fn styles_follow_their_words_while_typing() {
        let mut t = styled("Make it fast");
        t.style_range(8, 12, &SpanChange::Highlight(Some(Rgba::hex(0xFFB547))));
        t.set_text("Make it very fast".into());
        assert_eq!(words(&t), ["fast"], "text typed before a styled word pushes it along");
        t.set_text("Make it very faxst".into());
        assert_eq!(words(&t), ["faxst"], "typing inside a styled word extends it");
        t.set_text("Make it very faxst!".into());
        assert_eq!(words(&t), ["faxst"], "typing right after it doesn't");
        t.set_text("Make it very slow!".into());
        assert_eq!(words(&t), ["slow"], "typing over the whole word keeps its style");
        t.set_text("Make it very!".into());
        assert!(t.spans.is_empty(), "deleting the word removes its style");
        // Bangla: positions are characters, not bytes.
        let mut b = styled("নতুন অফার আজই");
        b.style_range(5, 9, &SpanChange::Color(Some(Rgba::hex(0xFF6363))));
        assert_eq!(words(&b), ["অফার"]);
        b.set_text("নতুন বড় অফার আজই".into());
        assert_eq!(words(&b), ["অফার"]);
        let runs = b.styled_runs();
        assert_eq!(runs.iter().filter(|r| r.1.is_some()).map(|r| &b.text[r.0.clone()]).collect::<Vec<_>>(), ["অফার"]);
        assert_eq!(runs.iter().map(|r| &b.text[r.0.clone()]).collect::<String>(), b.text, "the runs cover the whole text");
    }

    #[test]
    fn spans_load_from_files_and_bad_ones_are_cleaned() {
        let t: TextLayer = serde_json::from_str(r##"{ "text": "Hello", "spans": [ { "start": 0, "end": 99, "color": [1,0,0,1] }, { "start": 1, "end": 3, "underline": true } ] }"##).unwrap();
        let mut t = t;
        t.set_text(t.text.clone());
        assert_eq!(t.spans.len(), 1);
        assert_eq!((t.spans[0].start, t.spans[0].end), (0, 5));
        let plain: TextLayer = serde_json::from_str(r#"{ "text": "Old file" }"#).unwrap();
        assert!(plain.spans.is_empty());
        assert!(!serde_json::to_string(&plain).unwrap().contains("spans"), "no styles, nothing saved");
    }

    #[test]
    fn color_correction_leaves_an_untouched_clip_alone_and_reads_old_files() {
        for rgb in [[0.0, 0.0, 0.0], [0.2, 0.5, 0.9], [1.0, 1.0, 1.0]] {
            let out = Grade::NONE.apply(rgb, 1.41);
            assert!(out.iter().zip(rgb).all(|(a, b)| (a - b).abs() < 1e-6), "{rgb:?} became {out:?}");
        }
        assert_eq!(Grade::default(), Grade::NONE);
        // A file saved before the newer controls existed.
        let old: Grade = serde_json::from_str(r#"{ "brightness": 0.1, "contrast": 0.2, "saturation": -0.3, "temperature": 0.4, "lift": 0.0, "gamma": 0.0, "gain": 0.0 }"#).unwrap();
        assert_eq!(old, Grade { brightness: 0.1, contrast: 0.2, saturation: -0.3, temperature: 0.4, ..Grade::NONE });
        // Values out of range (a hand-edited file) are brought back inside it.
        let wild = Grade { smooth: 7.0, vignette: -2.0, tint: f64::NAN, ..Grade::NONE }.clamped();
        assert_eq!((wild.smooth, wild.vignette, wild.tint), (1.0, 0.0, 0.0));
    }

    #[test]
    fn each_color_preset_does_its_job() {
        let look = |name: &str| COLOR_PRESETS.iter().find(|p| p.name == name).unwrap_or_else(|| panic!("no {name} preset")).grade;
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let skin = [0.80, 0.60, 0.50];
        let (dark, bright) = ([0.12, 0.12, 0.12], [0.85, 0.85, 0.85]);

        assert_eq!(COLOR_PRESETS[0].name, "Original");
        assert_eq!(COLOR_PRESETS[0].grade, Grade::NONE);
        let mut names: Vec<_> = COLOR_PRESETS.iter().map(|p| p.name).collect();
        names.dedup();
        assert_eq!(names.len(), COLOR_PRESETS.len());

        // Mono has no color left at all.
        let mono = look("Mono").apply(skin, 0.0);
        assert!((mono[0] - mono[1]).abs() < 1e-4 && (mono[1] - mono[2]).abs() < 1e-4, "{mono:?}");
        // Cinematic: shadows lean blue, highlights lean orange, corners are darker.
        let cinematic = look("Cinematic");
        let (sh, hi) = (cinematic.apply(dark, 0.0), cinematic.apply(bright, 0.0));
        assert!(sh[2] > sh[0] + 0.02, "cool shadows {sh:?}");
        assert!(hi[0] > hi[2] + 0.02, "warm highlights {hi:?}");
        assert!(luma(cinematic.apply(bright, 1.41)) < luma(hi) - 0.1, "dark corners");
        // Warm and Cool pull a gray in opposite directions.
        let (warm, cool) = (look("Warm").apply(bright, 0.0), look("Cool").apply(bright, 0.0));
        assert!(warm[0] > warm[2] + 0.05 && cool[2] > cool[0] + 0.05, "{warm:?} {cool:?}");
        // Vivid makes a muted color stronger; Vintage raises the blacks.
        let muted = [0.55, 0.45, 0.42];
        let spread = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        assert!(spread(look("Vivid").apply(muted, 0.0)) > spread(muted) * 1.3);
        assert!(luma(look("Vintage").apply([0.0; 3], 0.0)) > 0.03);
        // Beautify smooths skin and keeps it bright; Natural stays close to the source.
        assert!(look("Beautify").smooth >= 0.5);
        assert!(luma(look("Beautify").apply(skin, 0.0)) >= luma(skin));
        let natural = look("Natural").apply(skin, 0.0);
        assert!(natural.iter().zip(skin).all(|(a, b)| (a - b).abs() < 0.06), "{natural:?}");
    }
}
