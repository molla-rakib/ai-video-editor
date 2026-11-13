//! Evaluates a project at one moment into a flat list of layers to draw.

use std::sync::Arc;

use super::Frame;
use crate::clip::{self, Clip, Content, Ease, Fit, MotionKind, Prop, TextLayer, TransitionKind};
use crate::model::{Id, MediaKind, Project, TrackKind};
use crate::ui::icons::code;

/// The Lucide glyph for a callout icon — the single source of this mapping; reused by the
/// library grid (`editor::library::CalloutPreset::glyph`) so the two never drift apart.
pub fn callout_glyph(icon: clip::CalloutIcon) -> char {
    match icon {
        clip::CalloutIcon::Check => code::CHECK,
        clip::CalloutIcon::Arrow => code::ARROW_UP_RIGHT,
        clip::CalloutIcon::Star => code::STAR,
        clip::CalloutIcon::Sparkle => code::SPARKLE,
        clip::CalloutIcon::Zap => code::ZAP,
    }
}

/// Where a layer's pixels come from. The scene builder asks for these.
pub trait Sources {
    /// The decoded video frame closest to `src_time` for this clip, if one is ready.
    fn video(&self, clip: Id, asset: Id, src_time: f64, playing: bool) -> Option<Arc<Frame>>;
    fn image(&self, asset: Id) -> Option<Arc<Frame>>;
    /// Rendered text at `px_per_unit` canvas pixels per canvas unit. `local` is the clip-relative
    /// time, used only when the layer has a kinetic reveal.
    fn text(&self, layer: &clip::TextLayer, px_per_unit: f32, local: f64) -> Option<Arc<Frame>>;
}

#[derive(Debug, Clone)]
pub struct LayerSpec {
    /// The clip this layer shows (0 for the background).
    pub clip: Id,
    /// Texture slot: the same slot is reused for a clip's video frames.
    pub slot: u64,
    pub frame: Option<Arc<Frame>>,
    /// Center and size in canvas pixels, before rotation.
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub rotation: f32,
    pub opacity: f32,
    pub radius: f32,
    pub border: f32,
    pub border_color: [f32; 4],
    pub shadow_blur: f32,
    pub shadow_y: f32,
    pub shadow_color: [f32; 4],
    /// Part of the source texture to show (u0, v0, u1, v1), for camera zooms and wipes.
    pub uv: [f32; 4],
    /// Multiplies the texture; for shapes this is the fill (top and bottom of a gradient).
    pub tint_top: [f32; 4],
    pub tint_bottom: [f32; 4],
    /// Brightness, contrast, saturation, temperature (0 = unchanged).
    pub grade: [f32; 4],
    /// Lift, gamma, gain (0 = unchanged), fourth component unused.
    pub grade2: [f32; 4],
    /// Tint, vibrance, fade, split tone (0 = unchanged).
    pub grade3: [f32; 4],
    /// Smoothness, sharpness, vignette (0 = off), fourth component unused.
    pub grade4: [f32; 4],
    /// Outline: kind (0 rectangle, 1 ellipse, 2 triangle, 3 star, 4 hexagon, 5 ring),
    /// ring thickness in px, gradient kind (0 linear, 1 radial), gradient angle in radians.
    pub shape: [f32; 4],
    /// Blurred areas: (u0, v0, u1, v1, radius u, radius v, feather, ellipse), radius 0 = unused.
    pub blurs: [[f32; 8]; MAX_BLURS],
    /// Mask areas: (u0, v0, u1, v1, feather, ellipse, invert, edge fade), feather 0 = unused.
    pub masks: [[f32; 8]; MAX_MASKS],
    /// Video and images: blur and mask clips above can affect this layer.
    pub blurrable: bool,
    /// A blur clip (drawn invisibly; it blurs the layers below): strength in canvas px, ellipse (1/0).
    pub blur_box: Option<[f32; 2]>,
    /// A mask clip (drawn invisibly; it masks the layers below): feather in canvas px, ellipse (1/0),
    /// invert (1/0), edge fade (0 at the clip's own fade-in/out edges, else 1).
    pub mask_box: Option<[f32; 4]>,
}

pub use crate::clip::{MAX_BLURS, MAX_MASKS};

#[derive(Debug, Clone)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub background: [f32; 4],
    pub layers: Vec<LayerSpec>,
}

/// Canvas pixels per canvas unit (1 unit = 1 px at a 1080-pixel short side).
pub fn px_per_unit(width: u32, height: u32) -> f32 {
    width.min(height) as f32 / 1080.0
}

pub fn evaluate(project: &Project, t: f64, size: (u32, u32), sources: &dyn Sources, playing: bool) -> Scene {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let unit = px_per_unit(size.0, size.1);
    let mut layers = Vec::new();
    // Bottom track first; tracks[0] is drawn on top.
    for track in project.tracks.iter().rev().filter(|tr| tr.kind == TrackKind::Visual && !tr.hidden) {
        // Normally exactly one clip is active on a track at once; two only inside a transition's
        // shared overlap window — the one case clips on a track are allowed to overlap.
        let mut actives: Vec<&Clip> = project.clips_on(track.id).filter(|c| !c.disabled && t >= c.start && t < c.end()).collect();
        actives.sort_by(|a, b| a.start.total_cmp(&b.start));
        if let [outgoing, incoming] = actives[..] {
            if let Some(tr) = incoming.transition {
                let p = Ease::Smooth.apply((t - incoming.start) / tr.duration.max(0.01)) as f32;
                if let Some(layer) = clip_layer(project, outgoing, t, w, h, unit, sources, playing, Some((tr.kind, p))) {
                    push_layer(&mut layers, layer);
                }
            }
        }
        if let Some(&clip) = actives.last() {
            if let Some(layer) = clip_layer(project, clip, t, w, h, unit, sources, playing, None) {
                push_layer(&mut layers, layer);
            }
        }
    }
    Scene { width: size.0, height: size.1, background: [0.0, 0.0, 0.0, 1.0], layers }
}

/// Pushes a layer, letting a blur or mask clip affect what's already below it on this track.
fn push_layer(layers: &mut Vec<LayerSpec>, layer: LayerSpec) {
    if let Some([radius, ellipse]) = layer.blur_box {
        if radius > 0.0 {
            for below in layers.iter_mut().filter(|l: &&mut LayerSpec| l.blurrable) {
                blur_under(below, layer.center, layer.size, radius, ellipse);
            }
        }
        layers.push(LayerSpec { opacity: 0.0, ..layer });
    } else if let Some([feather, ellipse, invert, edge]) = layer.mask_box {
        for below in layers.iter_mut().filter(|l: &&mut LayerSpec| l.blurrable) {
            mask_under(below, layer.center, layer.size, feather, ellipse, invert, edge);
        }
        layers.push(LayerSpec { opacity: 0.0, ..layer });
    } else {
        layers.push(layer);
    }
}

#[allow(clippy::too_many_arguments)]
/// `outgoing_fade`, when set, means `clip` is the *outgoing* half of a transition pair (the
/// incoming clip's own `(kind, progress)`): it gets the mirror of the incoming clip's motion
/// instead of applying its own `clip.transition` (which, if any, belongs to a different pair).
fn clip_layer(project: &Project, clip: &Clip, t: f64, w: f32, h: f32, unit: f32, sources: &dyn Sources, playing: bool, outgoing_fade: Option<(TransitionKind, f32)>) -> Option<LayerSpec> {
    if clip.part == clip::Part::Sound {
        return None;
    }
    let local = t - clip.start;
    let animated = |prop: Prop, base: f64| clip.keyframes.get(&prop).and_then(|k| clip::sample(k, local)).unwrap_or(base);
    let tr = &clip.transform;
    let mut x = animated(Prop::X, tr.x) as f32 * w;
    let mut y = animated(Prop::Y, tr.y) as f32 * h;
    let mut scale = animated(Prop::Scale, tr.scale) as f32;
    let rotation = if matches!(clip.content, Content::Blur(_) | Content::Mask(_)) { 0.0 } else { (animated(Prop::Rotation, tr.rotation) as f32).to_radians() };
    let mut opacity = animated(Prop::Opacity, tr.opacity) as f32;
    let mut uv = [0.0f32, 0.0, 1.0, 1.0];

    // Entrance and exit animations.
    let mut apply_motion = |kind: MotionKind, p: f32, exit: bool| {
        let dir = if exit { -1.0 } else { 1.0 };
        let shift = 1.0 - p;
        match kind {
            MotionKind::Fade => opacity *= p,
            MotionKind::SlideUp => {
                y += dir * shift * 0.08 * h;
                opacity *= p;
            }
            MotionKind::SlideDown => {
                y -= dir * shift * 0.08 * h;
                opacity *= p;
            }
            MotionKind::SlideLeft => {
                x += dir * shift * 0.08 * w;
                opacity *= p;
            }
            MotionKind::SlideRight => {
                x -= dir * shift * 0.08 * w;
                opacity *= p;
            }
            MotionKind::Pop => {
                scale *= 0.55 + 0.45 * Ease::Back.apply(p as f64) as f32;
                opacity *= (p * 3.0).min(1.0);
            }
            MotionKind::ZoomIn => {
                scale *= 1.0 + 0.22 * (1.0 - p);
                opacity *= p;
            }
        }
    };
    if let Some(m) = clip.intro {
        let p = m.ease_or_default(false).apply((local - m.delay) / m.duration.max(0.01)) as f32;
        if p < 1.0 {
            apply_motion(m.kind, p, false);
        }
    }
    if let Some(m) = clip.outro {
        let p = m.ease_or_default(true).apply((clip.duration - local) / m.duration.max(0.01)) as f32;
        if p < 1.0 {
            apply_motion(m.kind, p.min(1.0), true);
        }
    }

    if !clip.presses.is_empty() {
        scale *= clip::press_scale(&clip.presses, local);
    }

    // "Nothing ever fully stops": a slow idle breathing/drift so a still moment doesn't freeze.
    if let Some(kind) = clip.idle_drift {
        let (dx, dy, scale_mul) = clip::idle_drift(kind, local);
        x += dx as f32 * unit;
        y += dy as f32 * unit;
        scale *= scale_mul as f32;
    }

    // A pulsing callout breathes gently: a touch bigger and a touch brighter in sync.
    let mut glow_mul = 1.0f32;
    if let Content::Callout(c) = &clip.content {
        if c.pulse {
            let osc = (local * std::f64::consts::TAU * 1.1).sin() as f32;
            scale *= 1.0 + 0.06 * osc;
            glow_mul = 1.0 + 0.25 * osc.max(0.0);
        }
    }

    let mut shape = [0.0, 0.0, 0.0, std::f32::consts::PI];
    let (frame, natural, slot, tint_top, tint_bottom, is_media) = match &clip.content {
        Content::Media { asset, .. } => {
            let a = project.asset(*asset)?;
            if a.kind == MediaKind::Audio {
                return None;
            }
            let frame = if a.kind == MediaKind::Image { sources.image(a.id) } else { sources.video(clip.id, a.id, clip.source_time(t), playing) };
            let natural = [a.width.max(1) as f32, a.height.max(1) as f32];
            (frame, natural, clip.id, [1.0; 4], [1.0; 4], true)
        }
        Content::Text(layer) => {
            let frame = sources.text(layer, unit, local)?;
            let natural = [frame.width as f32, frame.height as f32];
            (Some(frame), natural, text_slot(clip.id), [1.0; 4], [1.0; 4], false)
        }
        Content::Shape(s) => {
            let natural = [(s.width as f32 * unit).max(1.0), (s.height as f32 * unit).max(1.0)];
            let kind = match s.kind {
                crate::clip::ShapeKind::Rectangle => 0.0,
                crate::clip::ShapeKind::Ellipse => 1.0,
                crate::clip::ShapeKind::Triangle => 2.0,
                crate::clip::ShapeKind::Star => 3.0,
                crate::clip::ShapeKind::Hexagon => 4.0,
                crate::clip::ShapeKind::Ring => 5.0,
                crate::clip::ShapeKind::Frame => 6.0,
            };
            shape = [kind, s.thickness as f32 * unit * scale, if s.radial { 1.0 } else { 0.0 }, (s.gradient_angle as f32).to_radians()];
            let top = [s.fill.0, s.fill.1, s.fill.2, s.fill.3];
            let bottom = s.fill_to.map(|c| [c.0, c.1, c.2, c.3]).unwrap_or(top);
            (None, natural, 0, top, bottom, false)
        }
        Content::Blur(b) => (None, [(b.width as f32 * unit).max(1.0), (b.height as f32 * unit).max(1.0)], 0, [0.0; 4], [0.0; 4], false),
        Content::Mask(m) => (None, [(m.width as f32 * unit).max(1.0), (m.height as f32 * unit).max(1.0)], 0, [0.0; 4], [0.0; 4], false),
        Content::Callout(c) => {
            let icon_layer = TextLayer { text: callout_glyph(c.icon).to_string(), font: "lucide".into(), size: c.size, color: c.color, padding: 0.0, ..Default::default() };
            let frame = sources.text(&icon_layer, unit, local)?;
            let natural = [frame.width as f32, frame.height as f32];
            (Some(frame), natural, text_slot(clip.id), [1.0; 4], [1.0; 4], false)
        }
        Content::Counter(counter) => {
            let layer = TextLayer { text: counter.text_at(local), ..counter.format.clone() };
            let frame = sources.text(&layer, unit, local)?;
            let natural = [frame.width as f32, frame.height as f32];
            (Some(frame), natural, text_slot(clip.id), [1.0; 4], [1.0; 4], false)
        }
    };

    // Media fits the canvas; text and shapes keep their own size.
    let base = if is_media {
        let fit = match tr.fit {
            Fit::Contain => (w / natural[0]).min(h / natural[1]),
            Fit::Cover => (w / natural[0]).max(h / natural[1]),
        };
        [natural[0] * fit, natural[1] * fit]
    } else {
        natural
    };
    let mut size = [base[0] * scale, base[1] * scale];

    // Crop: show part of the frame and leave that part where it was.
    let crop = if is_media { clip.crop.clamped() } else { clip::Crop::default() };
    if !crop.is_none() {
        let (cl, ct, cr, cb) = (crop.left as f32, crop.top as f32, crop.right as f32, crop.bottom as f32);
        let full = size;
        size = [full[0] * (1.0 - cl - cr), full[1] * (1.0 - ct - cb)];
        let (s, c) = rotation.sin_cos();
        let (dx, dy) = ((cl - cr) / 2.0 * full[0], (ct - cb) / 2.0 * full[1]);
        x += dx * c - dy * s;
        y += dx * s + dy * c;
        uv = [cl, ct, 1.0 - cr, 1.0 - cb];
    }

    // Camtasia-style zoom: magnify part of the (cropped) frame inside the clip's own bounds.
    if !clip.zooms.is_empty() {
        let cam = clip::camera_at(&clip.zooms, local);
        let span = 1.0 / cam.scale.max(1.0) as f32;
        let half = span / 2.0;
        let cx = (cam.focus_x as f32).clamp(half, 1.0 - half);
        let cy = (cam.focus_y as f32).clamp(half, 1.0 - half);
        let (u0, v0, du, dv) = (uv[0], uv[1], uv[2] - uv[0], uv[3] - uv[1]);
        uv = [u0 + (cx - half) * du, v0 + (cy - half) * dv, u0 + (cx + half) * du, v0 + (cy + half) * dv];
    }

    if let Some((kind, p)) = outgoing_fade {
        // Mirror of the incoming clip's blend below: this clip fades/slides/zooms OUT as the
        // next one fades in, so the pair reads as one continuous transition, not two effects.
        if p < 1.0 {
            match kind {
                TransitionKind::Crossfade => opacity *= 1.0 - p,
                TransitionKind::SlideLeft | TransitionKind::Push => x -= p * w,
                TransitionKind::SlideUp => y -= p * h,
                TransitionKind::SlideRight => x += p * w,
                TransitionKind::SlideDown => y += p * h,
                TransitionKind::DipToBlack => opacity *= (1.0 - 2.0 * p).max(0.0),
                TransitionKind::ZoomThrough => {
                    let s = 1.0 + 0.35 * p;
                    size = [size[0] * s, size[1] * s];
                    opacity *= 1.0 - p;
                }
                TransitionKind::Wipe => {
                    // Recedes from the right edge as the incoming clip reveals from the left.
                    let right = x + size[0] / 2.0;
                    let u_span = uv[2] - uv[0];
                    size[0] *= 1.0 - p;
                    x = right - size[0] / 2.0;
                    uv[0] += u_span * p;
                }
            }
        }
    } else if let Some(tr) = clip.transition {
        // Transition into this clip.
        let p = Ease::Smooth.apply(local / tr.duration.max(0.01)) as f32;
        if p < 1.0 {
            match tr.kind {
                TransitionKind::Crossfade => opacity *= p,
                TransitionKind::SlideLeft | TransitionKind::Push => x += (1.0 - p) * w,
                TransitionKind::SlideUp => y += (1.0 - p) * h,
                TransitionKind::SlideRight => x -= (1.0 - p) * w,
                TransitionKind::SlideDown => y -= (1.0 - p) * h,
                TransitionKind::DipToBlack => opacity *= (2.0 * p - 1.0).max(0.0),
                TransitionKind::ZoomThrough => {
                    let s = 1.0 + 0.35 * (1.0 - p);
                    size = [size[0] * s, size[1] * s];
                    opacity *= p;
                }
                TransitionKind::Wipe => {
                    // Reveal from the left edge.
                    let left = x - size[0] / 2.0;
                    size[0] *= p;
                    x = left + size[0] / 2.0;
                    uv[2] = uv[0] + (uv[2] - uv[0]) * p;
                }
            }
        }
    }

    let look = &clip.look;
    let (radius, border, shadow) = if is_media || matches!(clip.content, Content::Shape(_) | Content::Callout(_)) {
        (look.corner_radius as f32 * unit * scale, look.border_width as f32 * unit * scale, look.shadow.enabled)
    } else {
        (0.0, 0.0, false)
    };
    let sc = look.shadow.color;
    let g = clip.grade.clamped();
    // Smoothing and sharpening work on pictures only: text and shapes stay crisp.
    let detail = if is_media { 1.0 } else { 0.0 };
    Some(LayerSpec {
        clip: clip.id,
        slot,
        frame,
        center: [x, y],
        size,
        rotation,
        opacity: opacity.clamp(0.0, 1.0),
        radius: radius.min(size[0].min(size[1]) / 2.0),
        border,
        border_color: [look.border_color.0, look.border_color.1, look.border_color.2, look.border_color.3],
        shadow_blur: if shadow { look.shadow.blur as f32 * unit * glow_mul } else { 0.0 },
        shadow_y: if shadow { look.shadow.offset_y as f32 * unit } else { 0.0 },
        shadow_color: if shadow { [sc.0, sc.1, sc.2, (look.shadow.opacity as f32 * glow_mul).min(1.0)] } else { [0.0; 4] },
        uv,
        tint_top,
        tint_bottom,
        grade: [g.brightness as f32, g.contrast as f32, g.saturation as f32, g.temperature as f32],
        grade2: [g.lift as f32, g.gamma as f32, g.gain as f32, 0.0],
        grade3: [g.tint as f32, g.vibrance as f32, g.fade as f32, g.split as f32],
        grade4: [g.smooth as f32 * detail, g.sharpen as f32 * detail, g.vignette as f32, 0.0],
        shape,
        blurs: [[0.0; 8]; MAX_BLURS],
        masks: [[0.0; 8]; MAX_MASKS],
        blurrable: is_media,
        blur_box: match clip.content {
            // Strength 100 is a 60 px radius at 1080p; it fades in and out with the clip.
            Content::Blur(b) => {
                let fade = clip::BLUR_FADE.min(clip.duration / 2.0).max(1e-4);
                let edge = (local / fade).min((clip.duration - local) / fade).clamp(0.0, 1.0) as f32;
                Some([b.strength.clamp(0.0, 100.0) as f32 * 0.6 * unit * edge * opacity.clamp(0.0, 1.0), if b.ellipse { 1.0 } else { 0.0 }])
            }
            _ => None,
        },
        mask_box: match clip.content {
            // Fades in and out with the clip, same as a blur box.
            Content::Mask(m) => {
                let fade = clip::BLUR_FADE.min(clip.duration / 2.0).max(1e-4);
                let edge = (local / fade).min((clip.duration - local) / fade).clamp(0.0, 1.0) as f32;
                let feather = (m.feather.clamp(0.0, 100.0) as f32 / 100.0 * 80.0 * unit).max(1.0);
                Some([feather, if m.ellipse { 1.0 } else { 0.0 }, if m.invert { 1.0 } else { 0.0 }, edge * opacity.clamp(0.0, 1.0)])
            }
            _ => None,
        },
    })
}

/// Maps a box (canvas px, centered at `center`) into `target`'s own UV space, through its
/// rotation, crop and zoom. `None` if the box misses the visible part of the layer entirely.
fn uv_box(target: &LayerSpec, center: [f32; 2], size: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let [u0, v0, u1, v1] = target.uv;
    let (s, c) = (-target.rotation).sin_cos();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (px, py) = (center[0] + dx * size[0] / 2.0 - target.center[0], center[1] + dy * size[1] / 2.0 - target.center[1]);
        // Into the layer's own (unrotated) box, then into its source texture.
        let (lx, ly) = (px * c - py * s, px * s + py * c);
        let (fx, fy) = (lx / target.size[0].max(1e-3) + 0.5, ly / target.size[1].max(1e-3) + 0.5);
        let (sx, sy) = (u0 + fx * (u1 - u0), v0 + fy * (v1 - v0));
        (lo, hi) = ([lo[0].min(sx), lo[1].min(sy)], [hi[0].max(sx), hi[1].max(sy)]);
    }
    // Nothing to do if the box misses the visible part of the layer.
    if hi[0] <= u0.min(u1) || lo[0] >= u0.max(u1) || hi[1] <= v0.min(v1) || lo[1] >= v0.max(v1) {
        return None;
    }
    Some((lo, hi))
}

/// Blurs the part of `target` under a blur box (canvas pixels), through the layer's
/// rotation, crop and zoom, in the first free blur slot.
fn blur_under(target: &mut LayerSpec, center: [f32; 2], size: [f32; 2], radius: f32, ellipse: f32) {
    let [u0, v0, u1, v1] = target.uv;
    let Some((lo, hi)) = uv_box(target, center, size) else { return };
    let Some(slot) = target.blurs.iter_mut().find(|b| b[4] == 0.0) else { return };
    let (ru, rv) = (radius / target.size[0].max(1e-3) * (u1 - u0).abs(), radius / target.size[1].max(1e-3) * (v1 - v0).abs());
    *slot = [lo[0], lo[1], hi[0], hi[1], ru.max(1e-5), rv.max(1e-5), 0.003, ellipse];
}

/// Restricts (or, inverted, excludes) the part of `target` under a mask box (canvas pixels),
/// through the layer's rotation, crop and zoom, in the first free mask slot. Note: if the box
/// misses the layer entirely, the layer is left fully visible rather than fully hidden — same
/// edge case as a blur box dragged off a layer, and just as rare in practice.
fn mask_under(target: &mut LayerSpec, center: [f32; 2], size: [f32; 2], feather: f32, ellipse: f32, invert: f32, edge: f32) {
    let [u0, v0, u1, v1] = target.uv;
    let Some((lo, hi)) = uv_box(target, center, size) else { return };
    let Some(slot) = target.masks.iter_mut().find(|m| m[4] == 0.0) else { return };
    let (fu, fv) = (feather / target.size[0].max(1e-3) * (u1 - u0).abs(), feather / target.size[1].max(1e-3) * (v1 - v0).abs());
    *slot = [lo[0], lo[1], hi[0], hi[1], fu.max(fv).max(1e-5), ellipse, invert, edge];
}

/// Text layers get their own texture slots, separate from media clip ids.
fn text_slot(clip: Id) -> u64 {
    clip | (1 << 62)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Motion, Zoom};
    use crate::media::ProbeInfo;
    use crate::model::Canvas;
    use std::path::PathBuf;

    struct NoSources;
    impl Sources for NoSources {
        fn video(&self, _: Id, _: Id, _: f64, _: bool) -> Option<Arc<Frame>> {
            None
        }
        fn image(&self, _: Id) -> Option<Arc<Frame>> {
            None
        }
        fn text(&self, _: &clip::TextLayer, _: f32, _: f64) -> Option<Arc<Frame>> {
            Some(Arc::new(Frame::new(400, 100, vec![0; 400 * 100 * 4])))
        }
    }

    fn project() -> (Project, Id) {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Video, duration: 10.0, width: 1280, height: 720, fps: 30.0, has_audio: false };
        let a = p.add_asset(&PathBuf::from("/v.mp4"), &info);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        (p, c)
    }

    #[test]
    fn media_fits_the_canvas_and_stacks_tracks() {
        let (mut p, _) = project();
        p.add_text_clip(None, 0.0, crate::edit::DEFAULT_STILL, Default::default()).unwrap();
        let s = evaluate(&p, 1.0, (1920, 1080), &NoSources, false);
        assert_eq!(s.layers.len(), 2);
        assert_eq!(s.layers[0].size, [1920.0, 1080.0], "720p video fills a 1080p canvas");
        assert_eq!(s.layers[1].size, [400.0, 100.0], "text is on top at its own size");
    }

    #[test]
    fn zoom_crops_the_source_and_intro_fades() {
        let (mut p, c) = project();
        let clip = p.clip_mut(c).unwrap();
        clip.zooms.push(Zoom { id: 9, at: 1.0, duration: 4.0, scale: 2.0, focus_x: 0.0, focus_y: 1.0, ramp: 0.5 });
        clip.intro = Some(Motion { kind: MotionKind::Fade, duration: 1.0, ease: None, delay: 0.0 });
        let s = evaluate(&p, 3.0, (1920, 1080), &NoSources, false);
        assert_eq!(s.layers[0].uv, [0.0, 0.5, 0.5, 1.0], "focus is clamped so the crop stays inside the frame");
        let early = evaluate(&p, 0.2, (1920, 1080), &NoSources, false);
        assert!(early.layers[0].opacity < 0.9);
    }

    #[test]
    fn delayed_intro_stays_hidden_until_the_delay_passes() {
        let (mut p, c) = project();
        let clip = p.clip_mut(c).unwrap();
        clip.intro = Some(Motion { kind: MotionKind::Fade, duration: 0.5, ease: None, delay: 1.0 });
        // Before the delay: fully pre-entrance (opacity 0, as if the intro hadn't started).
        let before = evaluate(&p, 0.5, (1920, 1080), &NoSources, false);
        assert_eq!(before.layers[0].opacity, 0.0, "a clip with a stagger delay stays hidden until the delay elapses");
        // After the delay elapses, the fade proceeds normally.
        let mid = evaluate(&p, 1.25, (1920, 1080), &NoSources, false);
        assert!(mid.layers[0].opacity > 0.0 && mid.layers[0].opacity < 1.0);
        let done = evaluate(&p, 2.0, (1920, 1080), &NoSources, false);
        assert_eq!(done.layers[0].opacity, 1.0);
    }

    #[test]
    fn crop_keeps_the_rest_of_the_frame_in_place() {
        let (mut p, c) = project();
        p.clip_mut(c).unwrap().crop = clip::Crop { left: 0.25, top: 0.0, right: 0.0, bottom: 0.5 };
        let l = &evaluate(&p, 1.0, (1920, 1080), &NoSources, false).layers[0];
        assert_eq!(l.size, [1440.0, 540.0], "a quarter cut from the left, half from the bottom");
        assert_eq!(l.center, [1920.0 * 0.625, 270.0], "what's left stays where it was");
        assert_eq!(l.uv, [0.25, 0.0, 1.0, 0.5]);
    }

    #[test]
    fn zoom_happens_inside_the_crop() {
        let (mut p, c) = project();
        let clip = p.clip_mut(c).unwrap();
        clip.crop = clip::Crop { left: 0.5, top: 0.0, right: 0.0, bottom: 0.0 };
        clip.zooms.push(Zoom { id: 9, at: 0.0, duration: 4.0, scale: 2.0, focus_x: 0.5, focus_y: 0.5, ramp: 0.1 });
        let l = &evaluate(&p, 2.0, (1920, 1080), &NoSources, false).layers[0];
        assert_eq!(l.uv, [0.625, 0.25, 0.875, 0.75], "the zoom magnifies the middle of the cropped part");
    }

    #[test]
    fn a_blur_clip_blurs_only_its_box_and_only_while_it_is_on() {
        let (mut p, _) = project();
        let b = p.add_blur_clip(None, 2.0, 3.0, clip::BlurLayer { width: 480.0, height: 270.0, strength: 50.0, ellipse: true }).unwrap();
        (p.clip_mut(b).unwrap().transform.x, p.clip_mut(b).unwrap().transform.y) = (0.25, 0.5);
        let before = evaluate(&p, 1.0, (1920, 1080), &NoSources, false);
        assert_eq!(before.layers.len(), 1);
        assert_eq!(before.layers[0].blurs[0][4], 0.0, "no blur before the blur clip starts");
        let during = evaluate(&p, 3.0, (1920, 1080), &NoSources, false);
        let (video, blur) = (&during.layers[0], &during.layers[1]);
        let r = video.blurs[0];
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(near(r[0], 0.125) && near(r[1], 0.375) && near(r[2], 0.375) && near(r[3], 0.625), "the box, in the footage's coordinates: {r:?}");
        assert!(r[4] > 0.0 && r[7] == 1.0, "has a radius and is an ellipse");
        assert_eq!(video.blurs[1][4], 0.0, "unused slots have no radius");
        assert_eq!(blur.opacity, 0.0, "the blur clip itself draws nothing");
        assert_eq!(evaluate(&p, 6.0, (1920, 1080), &NoSources, false).layers[0].blurs[0][4], 0.0, "and none after it ends");
    }

    #[test]
    fn a_mask_clip_isolates_only_its_box_and_only_while_it_is_on() {
        let (mut p, _) = project();
        let m = p.add_mask_clip(None, 2.0, 3.0, clip::MaskLayer { width: 480.0, height: 270.0, feather: 20.0, ellipse: true, invert: false }).unwrap();
        (p.clip_mut(m).unwrap().transform.x, p.clip_mut(m).unwrap().transform.y) = (0.25, 0.5);
        let before = evaluate(&p, 1.0, (1920, 1080), &NoSources, false);
        assert_eq!(before.layers.len(), 1);
        assert_eq!(before.layers[0].masks[0][4], 0.0, "no mask before the mask clip starts");
        let during = evaluate(&p, 3.0, (1920, 1080), &NoSources, false);
        let (video, mask) = (&during.layers[0], &during.layers[1]);
        let r = video.masks[0];
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(near(r[0], 0.125) && near(r[1], 0.375) && near(r[2], 0.375) && near(r[3], 0.625), "the box, in the footage's coordinates: {r:?}");
        assert!(r[4] > 0.0 && r[5] == 1.0 && r[6] == 0.0, "has a feather, is an ellipse, not inverted");
        assert_eq!(video.masks[1][4], 0.0, "unused slots have no feather");
        assert_eq!(mask.opacity, 0.0, "the mask clip itself draws nothing");
        assert_eq!(evaluate(&p, 6.0, (1920, 1080), &NoSources, false).layers[0].masks[0][4], 0.0, "and none after it ends");
    }

    #[test]
    fn separated_sound_is_not_drawn() {
        let (mut p, c) = project();
        p.clip_mut(c).unwrap().part = clip::Part::Sound;
        assert!(evaluate(&p, 1.0, (1920, 1080), &NoSources, false).layers.is_empty());
    }

    #[test]
    fn hidden_tracks_and_gaps_draw_nothing() {
        let (mut p, c) = project();
        assert!(evaluate(&p, 11.0, (1920, 1080), &NoSources, false).layers.is_empty());
        let track = p.clip(c).unwrap().track;
        p.track_mut(track).unwrap().hidden = true;
        assert!(evaluate(&p, 1.0, (1920, 1080), &NoSources, false).layers.is_empty());
    }

    #[test]
    fn a_callout_glows_and_a_pulsing_one_breathes_over_time() {
        let mut p = Project::new("t", Canvas::default());
        let layer = clip::CalloutLayer::new(clip::CalloutIcon::Check, clip::Rgba::hex(0x2EF2A2));
        let id = p.add_callout_clip(None, 0.0, 4.0, layer).unwrap();
        let still = evaluate(&p, 2.0, (1920, 1080), &NoSources, false).layers[0].clone();
        assert!(still.shadow_blur > 0.0 && still.shadow_color[3] > 0.0, "the glow renders via the clip's shadow");

        p.clip_mut(id).unwrap().content = match p.clip(id).unwrap().content.clone() {
            Content::Callout(mut c) => {
                c.pulse = true;
                Content::Callout(c)
            }
            other => other,
        };
        let a = evaluate(&p, 2.0, (1920, 1080), &NoSources, false).layers[0].clone();
        let b = evaluate(&p, 2.5, (1920, 1080), &NoSources, false).layers[0].clone();
        assert_ne!(a.shadow_blur, b.shadow_blur, "a pulsing callout's glow changes over time");
        assert_ne!(a.size, b.size, "and so does its scale");
    }
}
