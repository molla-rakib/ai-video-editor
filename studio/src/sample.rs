//! A small starter project offered on the start screen, so a first-time user has
//! something finished to explore, play with and pull apart — instead of a blank
//! canvas and an empty library. Built entirely from text, shapes and a callout
//! (no bundled media needed), so it works for any aspect ratio.

use crate::clip::{Align, CalloutIcon, CalloutLayer, Content, Motion, MotionKind, Rgba, SpanChange, TextLayer, TextReveal};
use crate::model::{Canvas, Project};

const ACCENT: Rgba = Rgba(0.839, 1.0, 0.0, 1.0);
const MUTED: Rgba = Rgba(0.651, 0.651, 0.651, 1.0);

fn motion(kind: MotionKind, duration: f64) -> Motion {
    Motion { kind, duration, ease: None, delay: 0.0 }
}

/// A short "product launch" scene: a headline that reveals word by word, a
/// tagline, a glowing accent, and a marker — a few seconds of something that
/// already looks intentional, ready to be renamed, retimed or replaced.
pub fn demo_project(canvas: Canvas) -> Project {
    let mut p = Project::new("Sample project", canvas);
    let track = |p: &Project, name: &str| p.tracks.iter().find(|t| t.name == name).map(|t| t.id);
    let text_track = track(&p, "Text");
    let overlay = track(&p, "Overlay");

    let headline = "Your Product, Beautifully Shown.";
    let layer = TextLayer { text: headline.into(), size: 84.0, align: Align::Center, reveal: Some(TextReveal::default()), ..TextLayer::default() };
    if let Ok(id) = p.add_text_clip(text_track, 0.6, 4.0, layer) {
        if let Ok(c) = p.clip_mut(id) {
            c.transform.y = 0.42;
            c.intro = Some(motion(MotionKind::SlideUp, 0.6));
            c.outro = Some(motion(MotionKind::Fade, 0.36));
            if let Content::Text(t) = &mut c.content {
                if let Some(start) = headline.find("Beautifully") {
                    t.style_range(start, start + "Beautifully".chars().count(), &SpanChange::Color(Some(ACCENT)));
                }
            }
        }
    }

    let tagline = TextLayer { text: "Edit by hand, or ask the assistant.".into(), size: 32.0, color: MUTED, align: Align::Center, ..TextLayer::default() };
    if let Ok(id) = p.add_text_clip(overlay, 1.8, 4.2, tagline) {
        if let Ok(c) = p.clip_mut(id) {
            c.transform.y = 0.58;
            c.intro = Some(motion(MotionKind::Fade, 0.6));
            c.outro = Some(motion(MotionKind::Fade, 0.36));
        }
    }

    let sparkle = CalloutLayer { pulse: true, ..CalloutLayer::new(CalloutIcon::Sparkle, ACCENT) };
    if let Ok(id) = p.add_callout_clip(overlay, 6.2, 1.6, sparkle) {
        if let Ok(c) = p.clip_mut(id) {
            c.transform.y = 0.8;
        }
    }

    let marker = p.add_marker(0.05, 0.0, ACCENT);
    if let Ok(m) = p.marker_mut(marker) {
        m.name = "Start here — try editing this text".into();
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Developer tool: `STUDIO_DUMP_OUT=out.aivp cargo test -p studio dump_demo_project_for_snapshot -- --ignored`
    #[test]
    #[ignore]
    fn dump_demo_project_for_snapshot() {
        let out = std::env::var("STUDIO_DUMP_OUT").expect("STUDIO_DUMP_OUT");
        demo_project(Canvas::default()).save(std::path::Path::new(&out)).unwrap();
    }

    #[test]
    fn demo_project_is_well_formed() {
        let p = demo_project(Canvas::default());
        assert!(p.duration() > 0.0);
        assert!(!p.clips.is_empty());
        for a in &p.clips {
            for b in &p.clips {
                assert!(a.id == b.id || a.track != b.track || a.end() <= b.start || b.end() <= a.start, "clips {} and {} overlap", a.id, b.id);
            }
        }
    }
}
