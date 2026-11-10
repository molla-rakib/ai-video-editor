//! The assistant's tools: their schemas, and how the editor carries them out.
//! Every change is one undo step, so the user can take back anything.

use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use iced::Task;
use serde_json::{Value, json};

use super::{Editor, Message, SideTab};
use crate::clip::{Align, Clip, Content, Ease, Keyframe, Motion, MotionKind, Prop, Rgba, ShapeKind, SpanChange, ShapeLayer, TextLayer, Transition, TransitionKind, Zoom};
use crate::mcp::{self, Call};
use crate::model::{Aspect, Id, MediaKind, Project, Resolution, TrackKind};

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": properties, "required": required } })
}

fn num(d: &str) -> Value {
    json!({ "type": "number", "description": d })
}
fn int(d: &str) -> Value {
    json!({ "type": "integer", "description": d })
}
fn string(d: &str) -> Value {
    json!({ "type": "string", "description": d })
}
fn boolean(d: &str) -> Value {
    json!({ "type": "boolean", "description": d })
}
fn choice(values: &[&str], d: &str) -> Value {
    json!({ "type": "string", "enum": values, "description": d })
}

const MOTIONS: &[&str] = &["none", "fade", "slide_up", "slide_down", "slide_left", "slide_right", "pop", "zoom_in"];
const TRANSITIONS: &[&str] = &["none", "crossfade", "slide", "slide_up", "push", "zoom_through", "wipe", "slide_right", "slide_down", "dip_to_black"];
const EASES: &[&str] = &["linear", "smooth", "ease_in", "ease_out", "back"];
const PROPS: &[&str] = &["x", "y", "scale", "rotation", "opacity"];

fn style_props() -> serde_json::Map<String, Value> {
    let v = json!({
        "x": num("Center x, fraction of canvas width (0 left, 1 right)"),
        "y": num("Center y, fraction of canvas height (0 top, 1 bottom)"),
        "scale": num("1 = media fits the canvas; text/shapes at their own size"),
        "rotation": num("Degrees, clockwise"),
        "opacity": num("0–1"),
        "fit": choice(&["fit", "fill"], "Media framing: whole frame visible, or fill the canvas"),
        "corner_radius": num("Rounded corners, canvas units"),
        "border_width": num("Border, canvas units (0 = none)"),
        "border_color": string("#RRGGBB"),
        "shadow": boolean("Soft drop shadow"),
        "shadow_blur": num("Shadow softness, canvas units"),
        "brightness": num("-1…1"),
        "contrast": num("-1…1"),
        "saturation": num("-1…1"),
        "warmth": num("-1…1"),
        "volume": num("0–2 (1 = original)"),
        "muted": boolean("Silence this clip"),
        "fade_in": num("Audio fade in, seconds"),
        "fade_out": num("Audio fade out, seconds"),
        "intro": choice(MOTIONS, "Entrance animation"),
        "intro_duration": num("Seconds"),
        "outro": choice(MOTIONS, "Exit animation"),
        "outro_duration": num("Seconds"),
        "transition": choice(TRANSITIONS, "Transition from the previous clip on the same track into this one"),
        "transition_duration": num("Seconds"),
        "intro_ease": choice(EASES, "Entrance easing (default depends on the motion). \"back\" gives a slight springy overshoot — a bouncy entrance."),
        "intro_delay": num("Seconds after the clip starts before the intro begins. Give a group of clips slightly larger delays (e.g. 0.05-0.1s apart) to stagger their entrances instead of landing them all at once."),
        "outro_ease": choice(EASES, "Exit easing (default depends on the motion)."),
        "idle_drift": boolean("A slow, continuous breathing scale (1.0-1.02) so a still moment never looks frozen. Good on a held shot with no other motion."),
    });
    v.as_object().unwrap().clone()
}

/// Styling for a counter's displayed number: the subset of `text_props()` that applies (no
/// `text` — it's computed every frame — or `background`, which counters don't support).
fn counter_style_props() -> serde_json::Map<String, Value> {
    let v = json!({
        "font": choice(&["Inter", "Poppins", "Hind Siliguri"], "Inter or Poppins for English, Hind Siliguri for Bangla"),
        "weight": int("400 regular, 500 medium, 600 semibold, 700 bold"),
        "size": num("Font size, canvas units"),
        "color": string("#RRGGBB"),
        "align": choice(&["left", "center", "right"], "Alignment"),
    });
    v.as_object().unwrap().clone()
}

fn text_props() -> serde_json::Map<String, Value> {
    let v = json!({
        "text": string("The text; use \\n for line breaks. Bangla is fully supported."),
        "font": choice(&["Inter", "Poppins", "Hind Siliguri"], "Inter or Poppins for English, Hind Siliguri for Bangla"),
        "weight": int("400 regular, 500 medium, 600 semibold, 700 bold"),
        "size": num("Font size, canvas units (96 is a big headline at 1080p)"),
        "color": string("#RRGGBB"),
        "align": choice(&["left", "center", "right"], "Alignment of multi-line text"),
        "background": string("#RRGGBB box behind the text, or \"none\""),
    });
    v.as_object().unwrap().clone()
}

fn merged(parts: &[serde_json::Map<String, Value>], extra: Value) -> Value {
    let mut m = serde_json::Map::new();
    for p in parts {
        m.extend(p.clone());
    }
    m.extend(extra.as_object().unwrap().clone());
    Value::Object(m)
}

pub fn schemas() -> Vec<Value> {
    vec![
        tool("get_project", "The whole project: canvas, tracks (top to bottom; higher video tracks draw on top), media library, every clip with its timing and properties, the playhead, the user's selected clips and selected time range. Call this first.", json!({}), &[]),
        tool(
            "import_media",
            "Import video, audio or image files (absolute paths) into the media library.",
            json!({ "paths": { "type": "array", "items": { "type": "string" } }, "add_to_timeline": boolean("Also place them on the timeline at the playhead") }),
            &["paths"],
        ),
        tool("add_media_clip", "Put a library item on the timeline. Videos/images go on a video track, audio on an audio track. Returns the clip id.", json!({ "media_id": int("Id from get_project media"), "start": num("Timeline seconds"), "track_id": int("Optional track") }), &["media_id", "start"]),
        tool(
            "analyze_audio_beats",
            "Find where the beats/hits are in an audio or video clip's sound, as timeline seconds. Use this before animating anything \
over music or narration: sync zooms, cuts and text entrances to the returned times instead of guessing — that's what makes an edit \
feel professional instead of random. Finds clear percussive hits, not a tempo/BPM analysis; silence or music without clear hits \
returns few or no beats.",
            json!({ "clip_id": int("A clip on the timeline (from get_project) that has sound") }),
            &["clip_id"],
        ),
        tool(
            "add_text",
            "Add a text layer (titles, captions, lower-thirds). Returns the clip id.",
            merged(&[text_props(), style_props()], json!({ "start": num("Timeline seconds"), "duration": num("Seconds (default 4)"), "track_id": int("Optional video track") })),
            &["text", "start"],
        ),
        tool(
            "add_shape",
            "Add a rectangle or ellipse (cards, highlights, backgrounds). Returns the clip id.",
            merged(&[style_props()], json!({
                "start": num("Timeline seconds"), "duration": num("Seconds (default 4)"),
                "shape": choice(&["rectangle", "ellipse", "triangle", "star", "hexagon", "ring", "frame"], "Default rectangle (use corner_radius for rounded cards and pills). ring and frame are outlines: use them, in a bright color, to circle or box part of the picture"),
                "width": num("Canvas units"), "height": num("Canvas units"),
                "color": string("#RRGGBB fill"), "color_to": string("#RRGGBB second color for a gradient"),
                "gradient": choice(&["linear", "radial"], "Gradient style when color_to is set"),
                "gradient_angle": num("Linear gradient direction in degrees: 180 top→bottom, 90 left→right"),
                "thickness": num("Outline thickness for ring and frame, canvas units"),
                "track_id": int("Optional video track"),
            })),
            &["start", "width", "height", "color"],
        ),
        tool(
            "add_glow_icon",
            "Add a glowing icon accent (check, arrow, star, sparkle, zap) that pops in with a spring and can pulse — a tasteful \
highlight for one genuinely important moment (a result landing, a step completing, a point being emphasized). Use sparingly: one or \
two per video, short (around 1-1.5s), never as decoration on every cut. Returns the clip id.",
            merged(&[style_props()], json!({
                "start": num("Timeline seconds"), "duration": num("Seconds (default 1.3)"),
                "icon": choice(&["check", "arrow", "star", "sparkle", "zap"], "Which glyph"),
                "color": string("#RRGGBB — also tints the glow"),
                "pulse": boolean("Keep gently breathing/pulsing while on screen (default off: just pops in and holds)"),
                "track_id": int("Optional video track"),
            })),
            &["start", "icon", "color"],
        ),
        tool(
            "add_counter",
            "Add an animated number that counts up or down, eased, over `count_duration`, then holds at the final value — for \
before/after metrics (revenue, users, a percentage). Never jumps to the final value in one frame. Returns the clip id.",
            merged(&[style_props(), counter_style_props()], json!({
                "start": num("Timeline seconds"), "duration": num("Seconds the clip is on screen (default 2)"),
                "from": num("Starting number"), "to": num("Ending number"),
                "count_duration": num("Seconds the count animation takes, eased (default 1); it holds at `to` for the rest of the clip"),
                "decimals": int("Decimal places (default 0)"),
                "prefix": string("e.g. \"$\""), "suffix": string("e.g. \"%\" or \"+ users\""),
                "track_id": int("Optional video track"),
            })),
            &["start", "from", "to"],
        ),
        tool(
            "add_camera_move",
            "A one-click, slow continuous camera move on a video or image clip, from a time to the end of the clip — reads as a \
deliberate camera move, not a highlight (use add_zoom to call attention to part of the frame instead). push_in: ramps in to a closer \
framing and holds. pull_back: starts punched in and pulls back to reveal the whole frame.",
            json!({ "clip_id": int(""), "move": choice(&["push_in", "pull_back"], ""), "start": num("Timeline seconds the move starts (default: clip start)") }),
            &["clip_id", "move"],
        ),
        tool(
            "add_press_pulse",
            "A one-shot \"button press\" pulse on a clip at a moment: a quick scale dip and recovery, like a real click. Use right \
when the user taps or clicks something in a screen recording. Returns the pulse id.",
            json!({ "clip_id": int(""), "at": num("Timeline seconds"), "duration": num("Seconds, default ~0.13 (8 frames at 60fps)") }),
            &["clip_id", "at"],
        ),
        tool("remove_press_pulse", "Remove a press pulse.", json!({ "clip_id": int(""), "pulse_id": int("") }), &["clip_id", "pulse_id"]),
        tool(
            "stagger_entrances",
            "Offsets a group of clips' intro animations a few frames apart (in the order given) instead of landing them all on the \
same instant — for a checklist, feature cards or bullet points entering together. Clips with no intro motion are left alone.",
            json!({ "clip_ids": { "type": "array", "items": { "type": "integer" } }, "gap_frames": int("Frames between each clip's entrance (default 3)") }),
            &["clip_ids"],
        ),
        tool("update_clip", "Change any properties of a clip. Only the fields you pass change.", merged(&[style_props(), text_props()], json!({ "clip_id": int(""), "name": string("Label on the timeline"), "speed": num("Playback speed, 0.25–4 (video/audio)"), "crop_left": num("Video/image crop: fraction of the source frame cut from the left (0–0.95); the rest stays in place"), "crop_right": num("Fraction cut from the right"), "crop_top": num("Fraction cut from the top"), "crop_bottom": num("Fraction cut from the bottom"), "icon": choice(&["check", "arrow", "star", "sparkle", "zap"], "Glow icon clips only: change the glyph"), "pulse": boolean("Glow icon clips only: breathe/pulse while on screen"), "from": num("Counter clips only: starting number"), "to": num("Counter clips only: ending number"), "count_duration": num("Counter clips only: seconds the count animation takes"), "decimals": int("Counter clips only: decimal places"), "prefix": string("Counter clips only"), "suffix": string("Counter clips only") })), &["clip_id"]),
        tool(
            "style_text",
            "Style some words of a text clip differently from the rest (an accent-colored keyword, a bolder or bigger word, a marker highlight, an underline). Pick the words with `words` (exact text, case-sensitive) or with start/end character positions. Only the fields you pass change; `clear` removes their styling.",
            json!({
                "clip_id": int("A text clip"),
                "words": string("The exact text to style, e.g. \"AI\" or \"in seconds\""),
                "occurrence": choice(&["first", "all"], "Which matches of `words` to style (default first)"),
                "start": int("Instead of `words`: first character (0-based, in Unicode characters)"),
                "end": int("Instead of `words`: character after the last one"),
                "color": string("#RRGGBB text color"),
                "weight": int("400 regular – 700 bold"),
                "font": choice(&["Inter", "Poppins", "Hind Siliguri"], "Font for these words (Hind Siliguri for Bangla)"),
                "size_scale": num("Size relative to the rest of the text, 0.5–3 (1 = same)"),
                "highlight": string("#RRGGBB marker box behind the words, or \"none\" to remove it"),
                "underline": boolean("Underline the words"),
                "clear": boolean("Remove all styling from these words first"),
            }),
            &["clip_id"],
        ),
        tool(
            "arrange_clip",
            "Put a picture (video, image, text or shape) in front of everything else or behind it, at the same time. It moves to the top or bottom video track (a new one when that is busy).",
            json!({ "clip_id": int(""), "to": choice(&["front", "back"], "front: drawn over all other layers; back: under them") }),
            &["clip_id", "to"],
        ),
        tool("move_clip", "Move a clip to a new start time (and optionally another track of the same kind).", json!({ "clip_id": int(""), "start": num("Timeline seconds"), "track_id": int("Optional") }), &["clip_id", "start"]),
        tool("trim_clip", "Change where a clip starts and/or ends on the timeline (media can't extend past its source).", json!({ "clip_id": int(""), "start": num("New start, timeline seconds"), "end": num("New end, timeline seconds") }), &["clip_id"]),
        tool("split_clip", "Split clips at a time. Without clip_id, splits every clip under that time.", json!({ "time": num("Timeline seconds"), "clip_id": int("Optional") }), &["time"]),
        tool("delete_clips", "Remove clips from the timeline.", json!({ "clip_ids": { "type": "array", "items": { "type": "integer" } } }), &["clip_ids"]),
        tool("duplicate_clip", "Copy a clip right after itself. Returns the new id.", json!({ "clip_id": int("") }), &["clip_id"]),
        tool(
            "add_zoom",
            "Camtasia-style zoom on a video or image clip: magnify part of the frame, easing in and out. Back-to-back zooms pan smoothly from one spot to the next. Use for screen recordings to highlight UI.",
            json!({
                "clip_id": int(""), "start": num("Timeline seconds"), "duration": num("Seconds"),
                "scale": num("Magnification, e.g. 1.6–2.5"), "focus_x": num("Point to zoom into, fraction of the frame width"), "focus_y": num("Fraction of the frame height"),
                "ease": num("Seconds to ease in/out (default 0.6)"),
            }),
            &["clip_id", "start", "duration", "scale", "focus_x", "focus_y"],
        ),
        tool("update_zoom", "Change a zoom region.", json!({ "clip_id": int(""), "zoom_id": int(""), "start": num(""), "duration": num(""), "scale": num(""), "focus_x": num(""), "focus_y": num(""), "ease": num("") }), &["clip_id", "zoom_id"]),
        tool("remove_zoom", "Remove a zoom region.", json!({ "clip_id": int(""), "zoom_id": int("") }), &["clip_id", "zoom_id"]),
        tool(
            "set_keyframes",
            "Animate a property with keyframes (replaces existing keys for that property). Use for custom motion: e.g. drift a product shot, scale a logo up, slide a card along a path.",
            json!({
                "clip_id": int(""), "property": choice(PROPS, "x/y are canvas fractions, rotation in degrees, scale and opacity as numbers"),
                "keys": { "type": "array", "items": { "type": "object", "properties": { "time": num("Timeline seconds"), "value": num(""), "ease": choice(EASES, "Easing to the next key") }, "required": ["time", "value"] } },
            }),
            &["clip_id", "property", "keys"],
        ),
        tool("clear_keyframes", "Remove keyframes from a clip (one property, or all).", json!({ "clip_id": int(""), "property": choice(PROPS, "Optional") }), &["clip_id"]),
        tool(
            "set_range_volume",
            "Change the volume of only part of a clip (e.g. duck music under speech, or mute a cough). Other parts are unchanged. Volume 1 removes the change.",
            json!({ "clip_id": int(""), "start": num("Timeline seconds"), "end": num("Timeline seconds"), "volume": num("0 = silent, 0.3 = ducked, 1 = original, up to 2") }),
            &["clip_id", "start", "end", "volume"],
        ),
        tool(
            "add_blur",
            "Blur part of the picture for a time range (faces, emails, passwords): a blur clip on a video track that blurs the video and images under it, only inside its box and only between start and end. Move or trim it like any clip (move_clip, trim_clip); remove it with delete_clips. Returns the clip id.",
            json!({ "start": num("Timeline seconds"), "end": num("Timeline seconds (default start + 3)"), "x": num("Box center, fraction of the canvas width"), "y": num("Box center, fraction of the canvas height"), "width": num("Fraction of the canvas width"), "height": num("Fraction of the canvas height"), "strength": num("0–100, default 60"), "ellipse": boolean("Oval instead of a box"), "track_id": int("Optional video track; it must be above the footage to blur (default: a free top track)") }),
            &["start", "x", "y", "width", "height"],
        ),
        tool("update_blur", "Change a blur clip's box or strength. Only the fields you pass change. Change its timing with move_clip / trim_clip.", json!({ "clip_id": int(""), "x": num("Canvas fraction"), "y": num("Canvas fraction"), "width": num("Canvas fraction"), "height": num("Canvas fraction"), "strength": num("0–100"), "ellipse": boolean("") }), &["clip_id"]),
        tool(
            "add_mask",
            "Isolate part of the picture for a time range (a face, a product, a speaker): a mask clip on a video track that shows only the video and images under it, only inside its box and only between start and end (everywhere else is hidden). Set invert to flip it into a cutout that hides the box instead. Move or trim it like any clip (move_clip, trim_clip); keyframe its X/Y (set_keyframes) to follow a moving subject; remove it with delete_clips. Returns the clip id.",
            json!({ "start": num("Timeline seconds"), "end": num("Timeline seconds (default start + 3)"), "x": num("Box center, fraction of the canvas width"), "y": num("Box center, fraction of the canvas height"), "width": num("Fraction of the canvas width"), "height": num("Fraction of the canvas height"), "feather": num("Edge softness, 0–100, default 15"), "ellipse": boolean("Oval instead of a box, default true"), "invert": boolean("Hide the box instead of isolating it"), "track_id": int("Optional video track; it must be above the footage to mask (default: a free top track)") }),
            &["start", "x", "y", "width", "height"],
        ),
        tool("update_mask", "Change a mask clip's box, feather or invert. Only the fields you pass change. Change its timing with move_clip / trim_clip.", json!({ "clip_id": int(""), "x": num("Canvas fraction"), "y": num("Canvas fraction"), "width": num("Canvas fraction"), "height": num("Canvas fraction"), "feather": num("0–100"), "ellipse": boolean(""), "invert": boolean("") }), &["clip_id"]),
        tool("separate_audio", "Put the sound of a video clip on an audio track of its own (same timing and volume); the video keeps playing silently. Returns the new audio clip id.", json!({ "clip_id": int("") }), &["clip_id"]),
        tool("add_marker", "Add a timeline marker: a colored flag at a time, or a span when duration > 0. Markers organize the edit (sections, notes); they don't show in the video. Returns the marker id.", json!({ "time": num("Timeline seconds"), "duration": num("Seconds, 0 for a point (default)"), "name": string("Label"), "color": string("#RRGGBB") }), &["time"]),
        tool("update_marker", "Change a timeline marker.", json!({ "marker_id": int(""), "time": num(""), "duration": num(""), "name": string(""), "color": string("#RRGGBB") }), &["marker_id"]),
        tool("delete_marker", "Remove a timeline marker.", json!({ "marker_id": int("") }), &["marker_id"]),
        tool("add_sound", "Add a built-in sound effect on an audio track. Sounds: whoosh, swoosh, pop, click, ding, riser, impact, typing, chime.", json!({ "sound": choice(&["whoosh", "swoosh", "pop", "click", "ding", "riser", "impact", "typing", "chime"], ""), "start": num("Timeline seconds"), "volume": num("0–2, default 1") }), &["sound", "start"]),
        tool("add_track", "Add a track.", json!({ "kind": choice(&["video", "audio"], ""), "name": string("") }), &["kind"]),
        tool("rename_track", "Rename a track.", json!({ "track_id": int(""), "name": string("") }), &["track_id", "name"]),
        tool("set_canvas", "Change the video format.", json!({ "aspect": choice(&["16:9", "9:16", "1:1", "4:5", "21:9"], ""), "resolution": choice(&["720p", "1080p", "1440p", "4K"], ""), "fps": choice(&["24", "25", "30", "60"], "") }), &[]),
        tool("get_frame", "See the rendered video at a time (as an image), exactly as it will export. Use it to check your work.", json!({ "time": num("Timeline seconds (default: playhead)") }), &[]),
        tool("seek", "Move the playhead so the user sees a moment.", json!({ "time": num("Timeline seconds") }), &["time"]),
        tool("play", "Start playback in the preview.", json!({}), &[]),
        tool("pause", "Pause playback.", json!({}), &[]),
        tool("undo", "Undo the last change.", json!({}), &[]),
        tool("redo", "Redo.", json!({}), &[]),
        tool("export_video", "Render the project to an MP4 file in the background.", json!({ "path": string("Absolute .mp4 path"), "quality": choice(&["high", "standard", "small"], "Default high") }), &["path"]),
        tool("get_export_status", "Progress of the current export.", json!({}), &[]),
    ]
}

// ---------------------------------------------------------------- arguments

fn f(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64)
}
fn req_f(a: &Value, k: &str) -> anyhow::Result<f64> {
    f(a, k).ok_or_else(|| anyhow!("`{k}` must be a number"))
}
fn id(a: &Value, k: &str) -> Option<Id> {
    a.get(k).and_then(Value::as_u64)
}
fn req_id(a: &Value, k: &str) -> anyhow::Result<Id> {
    id(a, k).ok_or_else(|| anyhow!("`{k}` must be an id from get_project"))
}
fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}
/// The character ranges `style_text` works on: matches of `words`, or `start`..`end`.
fn text_ranges(tl: &crate::clip::TextLayer, a: &Value) -> anyhow::Result<Vec<(usize, usize)>> {
    let len = tl.text.chars().count();
    if let Some(words) = s(a, "words").filter(|w| !w.is_empty()) {
        let all = s(a, "occurrence") == Some("all");
        let found: Vec<(usize, usize)> = tl
            .text
            .match_indices(words)
            .map(|(b, m)| {
                let start = tl.text[..b].chars().count();
                (start, start + m.chars().count())
            })
            .take(if all { usize::MAX } else { 1 })
            .collect();
        anyhow::ensure!(!found.is_empty(), "“{words}” isn't in the text “{}”", tl.text);
        return Ok(found);
    }
    let start = a.get("start").and_then(Value::as_u64).map(|v| v as usize);
    let end = a.get("end").and_then(Value::as_u64).map(|v| v as usize);
    match (start, end) {
        (Some(s), Some(e)) if s < e && e <= len => Ok(vec![(s, e)]),
        (Some(_), Some(_)) => Err(anyhow!("start/end must be 0 ≤ start < end ≤ {len} (the text has {len} characters)")),
        _ => Err(anyhow!("Pass `words`, or `start` and `end`")),
    }
}

fn color(a: &Value, k: &str) -> anyhow::Result<Option<Rgba>> {
    match s(a, k) {
        None => Ok(None),
        Some(v) => Rgba::parse(v).map(Some).ok_or_else(|| anyhow!("`{k}` must be a color like #7C6DFF")),
    }
}

fn motion(v: &str) -> anyhow::Result<Option<MotionKind>> {
    Ok(Some(match v {
        "none" => return Ok(None),
        "fade" => MotionKind::Fade,
        "slide_up" => MotionKind::SlideUp,
        "slide_down" => MotionKind::SlideDown,
        "slide_left" => MotionKind::SlideLeft,
        "slide_right" => MotionKind::SlideRight,
        "pop" => MotionKind::Pop,
        "zoom_in" => MotionKind::ZoomIn,
        other => bail!("Unknown animation `{other}`"),
    }))
}

fn transition(v: &str) -> anyhow::Result<Option<TransitionKind>> {
    Ok(Some(match v {
        "none" => return Ok(None),
        "crossfade" => TransitionKind::Crossfade,
        "slide" => TransitionKind::SlideLeft,
        "slide_up" => TransitionKind::SlideUp,
        "push" => TransitionKind::Push,
        "zoom_through" => TransitionKind::ZoomThrough,
        "wipe" => TransitionKind::Wipe,
        "slide_right" => TransitionKind::SlideRight,
        "slide_down" => TransitionKind::SlideDown,
        "dip_to_black" => TransitionKind::DipToBlack,
        other => bail!("Unknown transition `{other}`"),
    }))
}

fn prop(v: &str) -> anyhow::Result<Prop> {
    Ok(match v {
        "x" => Prop::X,
        "y" => Prop::Y,
        "scale" => Prop::Scale,
        "rotation" => Prop::Rotation,
        "opacity" => Prop::Opacity,
        other => bail!("Unknown property `{other}`"),
    })
}

fn ease(v: Option<&str>) -> Ease {
    match v {
        Some("linear") => Ease::Linear,
        Some("ease_in") => Ease::EaseIn,
        Some("ease_out") => Ease::EaseOut,
        Some("back") => Ease::Back,
        _ => Ease::Smooth,
    }
}

/// Like `ease`, but for an explicit `intro_ease`/`outro_ease` field: an unrecognized value is a
/// mistake worth reporting, not something to silently fall back from.
fn ease_choice(v: &str) -> anyhow::Result<Ease> {
    Ok(match v {
        "linear" => Ease::Linear,
        "smooth" => Ease::Smooth,
        "ease_in" => Ease::EaseIn,
        "ease_out" => Ease::EaseOut,
        "back" => Ease::Back,
        other => bail!("Unknown ease `{other}`"),
    })
}

/// Applies the shared style/text fields from `a` to a clip.
fn apply_props(c: &mut Clip, a: &Value) -> anyhow::Result<()> {
    let t = &mut c.transform;
    if let Some(v) = f(a, "x") {
        t.x = v;
    }
    if let Some(v) = f(a, "y") {
        t.y = v;
    }
    if let Some(v) = f(a, "scale") {
        t.scale = v.clamp(0.01, 20.0);
    }
    if let Some(v) = f(a, "rotation") {
        t.rotation = v;
    }
    if let Some(v) = f(a, "opacity") {
        t.opacity = v.clamp(0.0, 1.0);
    }
    match s(a, "fit") {
        Some("fit") => t.fit = crate::clip::Fit::Contain,
        Some("fill") => t.fit = crate::clip::Fit::Cover,
        _ => {}
    }
    let l = &mut c.look;
    if let Some(v) = f(a, "corner_radius") {
        l.corner_radius = v.max(0.0);
    }
    if let Some(v) = f(a, "border_width") {
        l.border_width = v.max(0.0);
    }
    if let Some(col) = color(a, "border_color")? {
        l.border_color = col;
    }
    if let Some(v) = a.get("shadow").and_then(Value::as_bool) {
        l.shadow.enabled = v;
    }
    if let Some(v) = f(a, "shadow_blur") {
        l.shadow.blur = v.max(0.0);
        l.shadow.enabled = true;
    }
    let g = &mut c.grade;
    for (k, field) in [("brightness", &mut g.brightness), ("contrast", &mut g.contrast), ("saturation", &mut g.saturation), ("warmth", &mut g.temperature)] {
        if let Some(v) = f(a, k) {
            *field = v.clamp(-1.0, 1.0);
        }
    }
    if let Some(v) = f(a, "volume") {
        c.audio.volume = v.clamp(0.0, 2.0);
    }
    if let Some(v) = a.get("muted").and_then(Value::as_bool) {
        c.audio.muted = v;
    }
    if let Some(v) = f(a, "fade_in") {
        c.audio.fade_in = v.max(0.0);
    }
    if let Some(v) = f(a, "fade_out") {
        c.audio.fade_out = v.max(0.0);
    }
    let half = (c.duration / 2.0).max(0.05);
    if let Some(v) = s(a, "intro") {
        c.intro = motion(v)?.map(|kind| Motion { kind, duration: c.intro.map(|m| m.duration).unwrap_or(0.6).min(half), ease: c.intro.and_then(|m| m.ease), delay: c.intro.map(|m| m.delay).unwrap_or(0.0) });
    }
    if let (Some(d), Some(m)) = (f(a, "intro_duration"), c.intro.as_mut()) {
        m.duration = d.clamp(0.05, half);
    }
    if let Some(v) = s(a, "intro_ease") {
        let e = ease_choice(v)?;
        if let Some(m) = c.intro.as_mut() {
            m.ease = Some(e);
        }
    }
    if let (Some(v), Some(m)) = (f(a, "intro_delay"), c.intro.as_mut()) {
        m.delay = v.max(0.0);
    }
    if let Some(v) = s(a, "outro") {
        c.outro = motion(v)?.map(|kind| Motion { kind, duration: c.outro.map(|m| m.duration).unwrap_or(0.6).min(half), ease: c.outro.and_then(|m| m.ease), delay: 0.0 });
    }
    if let (Some(d), Some(m)) = (f(a, "outro_duration"), c.outro.as_mut()) {
        m.duration = d.clamp(0.05, half);
    }
    if let Some(v) = s(a, "outro_ease") {
        let e = ease_choice(v)?;
        if let Some(m) = c.outro.as_mut() {
            m.ease = Some(e);
        }
    }
    if let Some(v) = a.get("idle_drift").and_then(Value::as_bool) {
        c.idle_drift = v.then_some(crate::clip::DriftKind::ScaleBreath);
    }
    if let Some(v) = s(a, "transition") {
        c.transition = transition(v)?.map(|kind| Transition { kind, duration: c.transition.map(|t| t.duration).unwrap_or(0.6).min(half) });
    }
    if let (Some(d), Some(t)) = (f(a, "transition_duration"), c.transition.as_mut()) {
        t.duration = d.clamp(0.05, half);
    }
    if ["crop_left", "crop_right", "crop_top", "crop_bottom"].iter().any(|k| a.get(*k).is_some()) {
        anyhow::ensure!(matches!(c.content, Content::Media { .. }), "Only video and image clips can be cropped");
        let k = c.crop;
        c.crop = crate::clip::Crop {
            left: f(a, "crop_left").unwrap_or(k.left),
            right: f(a, "crop_right").unwrap_or(k.right),
            top: f(a, "crop_top").unwrap_or(k.top),
            bottom: f(a, "crop_bottom").unwrap_or(k.bottom),
        }
        .clamped();
    }
    if let Content::Text(tl) = &mut c.content {
        if let Some(v) = s(a, "text") {
            tl.set_text(v.replace("\\n", "\n"));
        }
        if let Some(v) = s(a, "font") {
            tl.font = v.to_string();
        }
        if let Some(v) = a.get("weight").and_then(Value::as_u64) {
            tl.weight = (v as u16).clamp(100, 900);
        }
        if let Some(v) = f(a, "size") {
            tl.size = v.clamp(4.0, 1000.0);
        }
        if let Some(col) = color(a, "color")? {
            tl.color = col;
        }
        match s(a, "align") {
            Some("left") => tl.align = Align::Left,
            Some("center") => tl.align = Align::Center,
            Some("right") => tl.align = Align::Right,
            _ => {}
        }
        match s(a, "background") {
            Some("none") => tl.background = None,
            Some(_) => tl.background = color(a, "background")?.map(|c| Rgba(c.0, c.1, c.2, 0.6)),
            None => {}
        }
    }
    if let Some(col) = color(a, "color")? {
        if let Content::Callout(cal) = &mut c.content {
            cal.color = col;
            c.look.shadow.color = col;
        }
        if let Content::Counter(counter) = &mut c.content {
            counter.format.color = col;
        }
    }
    if let Some(v) = a.get("pulse").and_then(Value::as_bool) {
        if let Content::Callout(cal) = &mut c.content {
            cal.pulse = v;
        }
    }
    if let Some(v) = s(a, "icon") {
        if let Content::Callout(cal) = &mut c.content {
            cal.icon = callout_icon(v)?;
        }
    }
    if let Content::Counter(counter) = &mut c.content {
        if let Some(v) = f(a, "from") {
            counter.from = v;
        }
        if let Some(v) = f(a, "to") {
            counter.to = v;
        }
        if let Some(v) = f(a, "count_duration") {
            counter.duration = v.max(0.05);
        }
        if let Some(v) = a.get("decimals").and_then(Value::as_u64) {
            counter.decimals = v as u32;
        }
        if let Some(v) = s(a, "prefix") {
            counter.prefix = v.to_string();
        }
        if let Some(v) = s(a, "suffix") {
            counter.suffix = v.to_string();
        }
        if let Some(v) = s(a, "font") {
            counter.format.font = v.to_string();
        }
        if let Some(v) = a.get("weight").and_then(Value::as_u64) {
            counter.format.weight = (v as u16).clamp(100, 900);
        }
        if let Some(v) = f(a, "size") {
            counter.format.size = v.clamp(4.0, 1000.0);
        }
        match s(a, "align") {
            Some("left") => counter.format.align = Align::Left,
            Some("center") => counter.format.align = Align::Center,
            Some("right") => counter.format.align = Align::Right,
            _ => {}
        }
    }
    Ok(())
}

fn callout_icon(v: &str) -> anyhow::Result<crate::clip::CalloutIcon> {
    use crate::clip::CalloutIcon;
    Ok(match v {
        "check" => CalloutIcon::Check,
        "arrow" => CalloutIcon::Arrow,
        "star" => CalloutIcon::Star,
        "sparkle" => CalloutIcon::Sparkle,
        "zap" => CalloutIcon::Zap,
        other => bail!("Unknown callout icon `{other}`"),
    })
}

fn kind_name(p: &Project, c: &Clip) -> &'static str {
    match &c.content {
        Content::Text(_) => "text",
        Content::Shape(_) => "shape",
        Content::Blur(_) => "blur",
        Content::Mask(_) => "mask",
        Content::Callout(_) => "callout",
        Content::Counter(_) => "counter",
        Content::Media { .. } if c.part == crate::clip::Part::Sound => "audio",
        Content::Media { asset, .. } => match p.asset(*asset).map(|a| a.kind) {
            Some(MediaKind::Audio) => "audio",
            Some(MediaKind::Image) => "image",
            _ => "video",
        },
    }
}

impl Editor {
    fn project_json(&self) -> Value {
        let p = &self.doc.project;
        let (w, h) = p.canvas.size();
        let clips: Vec<Value> = p
            .clips
            .iter()
            .map(|c| {
                let mut o = json!({
                    "id": c.id, "track_id": c.track, "start": r3(c.start), "end": r3(c.end()), "kind": kind_name(p, c),
                    "name": super::timeline::clip_label(p, c),
                    "x": r3(c.transform.x), "y": r3(c.transform.y), "scale": r3(c.transform.scale), "rotation": r3(c.transform.rotation), "opacity": r3(c.transform.opacity),
                });
                let m = o.as_object_mut().unwrap();
                if let Content::Text(t) = &c.content {
                    m.insert("text".into(), json!(t.text));
                    m.insert("font".into(), json!(t.font));
                    m.insert("size".into(), json!(t.size));
                    m.insert("weight".into(), json!(t.weight));
                    m.insert("color".into(), json!(t.color.to_hex()));
                    if !t.spans.is_empty() {
                        let styled: Vec<Value> = t
                            .spans
                            .iter()
                            .map(|sp| {
                                let st = &sp.style;
                                let mut o = json!({ "start": sp.start, "end": sp.end, "words": t.text.chars().skip(sp.start).take(sp.end - sp.start).collect::<String>() });
                                let m = o.as_object_mut().unwrap();
                                if let Some(c) = st.color {
                                    m.insert("color".into(), json!(c.to_hex()));
                                }
                                if let Some(w) = st.weight {
                                    m.insert("weight".into(), json!(w));
                                }
                                if let Some(f) = &st.font {
                                    m.insert("font".into(), json!(f));
                                }
                                if let Some(k) = st.scale {
                                    m.insert("size_scale".into(), json!(k));
                                }
                                if let Some(c) = st.highlight {
                                    m.insert("highlight".into(), json!(c.to_hex()));
                                }
                                if st.underline {
                                    m.insert("underline".into(), json!(true));
                                }
                                o
                            })
                            .collect();
                        m.insert("styled_words".into(), json!(styled));
                    }
                }
                if let Content::Media { asset, speed, src_in } = c.content {
                    m.insert("media_id".into(), json!(asset));
                    m.insert("source_start".into(), json!(r3(src_in)));
                    if speed != 1.0 {
                        m.insert("speed".into(), json!(speed));
                    }
                    m.insert("volume".into(), json!(c.audio.volume));
                }
                if c.look.corner_radius > 0.0 {
                    m.insert("corner_radius".into(), json!(c.look.corner_radius));
                }
                if c.look.shadow.enabled {
                    m.insert("shadow".into(), json!(true));
                }
                if let Some(i) = c.intro {
                    m.insert("intro".into(), json!({ "kind": format!("{:?}", i.kind), "duration": r3(i.duration), "ease": i.ease.map(|e| format!("{e:?}").to_lowercase()), "delay": r3(i.delay) }));
                }
                if let Some(o2) = c.outro {
                    m.insert("outro".into(), json!({ "kind": format!("{:?}", o2.kind), "duration": r3(o2.duration), "ease": o2.ease.map(|e| format!("{e:?}").to_lowercase()) }));
                }
                if let Some(t) = c.transition {
                    m.insert("transition".into(), json!(format!("{:?}", t.kind)));
                }
                if !c.zooms.is_empty() {
                    let zooms: Vec<Value> = c.zooms.iter().map(|z| json!({ "id": z.id, "start": r3(c.start + z.at), "end": r3(c.start + z.at + z.duration), "scale": z.scale, "focus_x": r3(z.focus_x), "focus_y": r3(z.focus_y) })).collect();
                    m.insert("zooms".into(), json!(zooms));
                }
                if !c.presses.is_empty() {
                    let presses: Vec<Value> = c.presses.iter().map(|pp| json!({ "id": pp.id, "at": r3(c.start + pp.at), "duration": r3(pp.duration) })).collect();
                    m.insert("press_pulses".into(), json!(presses));
                }
                if c.idle_drift.is_some() {
                    m.insert("idle_drift".into(), json!(true));
                }
                if !c.crop.is_none() {
                    m.insert("crop".into(), json!({ "left": r3(c.crop.left), "right": r3(c.crop.right), "top": r3(c.crop.top), "bottom": r3(c.crop.bottom) }));
                }
                if let Content::Blur(b) = &c.content {
                    let (cw, ch) = p.canvas.size();
                    let per = cw.min(ch) as f64 / 1080.0;
                    m.insert("blur".into(), json!({ "width": r3(b.width * per / cw as f64), "height": r3(b.height * per / ch as f64), "strength": b.strength, "ellipse": b.ellipse }));
                }
                if let Content::Mask(mask) = &c.content {
                    let (cw, ch) = p.canvas.size();
                    let per = cw.min(ch) as f64 / 1080.0;
                    m.insert("mask".into(), json!({ "width": r3(mask.width * per / cw as f64), "height": r3(mask.height * per / ch as f64), "feather": mask.feather, "ellipse": mask.ellipse, "invert": mask.invert }));
                }
                if let Content::Callout(cal) = &c.content {
                    m.insert("icon".into(), json!(format!("{:?}", cal.icon).to_lowercase()));
                    m.insert("color".into(), json!(cal.color.to_hex()));
                    m.insert("pulse".into(), json!(cal.pulse));
                }
                if let Content::Counter(counter) = &c.content {
                    m.insert("from".into(), json!(counter.from));
                    m.insert("to".into(), json!(counter.to));
                    m.insert("count_duration".into(), json!(r3(counter.duration)));
                    m.insert("decimals".into(), json!(counter.decimals));
                    m.insert("prefix".into(), json!(counter.prefix));
                    m.insert("suffix".into(), json!(counter.suffix));
                    m.insert("color".into(), json!(counter.format.color.to_hex()));
                }
                match c.part {
                    crate::clip::Part::Picture => {
                        m.insert("audio_separated".into(), json!(true));
                    }
                    crate::clip::Part::Sound => {
                        m.insert("separated_from_video".into(), json!(true));
                    }
                    crate::clip::Part::Whole => {}
                }
                if !c.keyframes.is_empty() {
                    let keys: serde_json::Map<String, Value> =
                        c.keyframes.iter().map(|(k, v)| (format!("{k:?}").to_lowercase(), json!(v.iter().map(|kf| json!({ "time": r3(c.start + kf.t), "value": kf.value })).collect::<Vec<_>>()))).collect();
                    m.insert("keyframes".into(), Value::Object(keys));
                }
                o
            })
            .collect();
        json!({
            "canvas": { "aspect": p.canvas.aspect.label(), "resolution": p.canvas.resolution.label(), "fps": p.canvas.fps, "width": w, "height": h },
            "duration": r3(p.duration()),
            "playhead": r3(self.playhead),
            "selected_clip_ids": self.selection,
            "selected_range": self.range.map(|(a, b)| json!({ "start": r3(a), "end": r3(b) })),
            "tracks": p.tracks.iter().map(|t| json!({ "id": t.id, "name": t.name, "kind": if t.kind == TrackKind::Audio { "audio" } else { "video" }, "locked": t.locked, "hidden": t.hidden, "muted": t.muted })).collect::<Vec<_>>(),
            "media": p.assets.iter().map(|a| json!({ "id": a.id, "name": a.name, "kind": format!("{:?}", a.kind).to_lowercase(), "duration": r3(a.duration), "width": a.width, "height": a.height, "has_audio": a.has_audio, "path": a.path })).collect::<Vec<_>>(),
            "clips": clips,
            "markers": p.markers.iter().map(|m| json!({ "id": m.id, "time": r3(m.at), "duration": r3(m.duration), "name": m.name, "color": m.color.to_hex() })).collect::<Vec<_>>(),
        })
    }

    /// Runs an edit for the assistant and reports the outcome.
    fn ai_edit<T>(&mut self, desc: &str, f: impl FnOnce(&mut Project) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let out = self.doc.apply(desc, f)?;
        self.lanes_cache.clear();
        self.after_history();
        self.notify(format!("Assistant: {desc}"), false);
        Ok(out)
    }

    /// Carries out one tool call. Slow tools answer later from a task.
    pub fn handle_tool(&mut self, call: Call) -> Task<Message> {
        let a = call.args.clone();
        let result: anyhow::Result<String> = match call.name.as_str() {
            "get_project" => Ok(serde_json::to_string_pretty(&self.project_json()).unwrap_or_default()),
            "import_media" => {
                let paths: Vec<PathBuf> = a["paths"].as_array().map(|v| v.iter().filter_map(|p| p.as_str().map(PathBuf::from)).collect()).unwrap_or_default();
                if paths.is_empty() {
                    Err(anyhow!("`paths` is empty"))
                } else {
                    let add = a["add_to_timeline"].as_bool().unwrap_or(false);
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || paths.into_iter().map(|p| (p.clone(), crate::media::probe(&p).map_err(|e| e.to_string()))).collect::<Vec<_>>()).await.unwrap_or_default()
                        },
                        move |results| Message::ToolImported(call.clone(), results, add),
                    );
                }
            }
            "add_media_clip" => (|| {
                let (asset, start) = (req_id(&a, "media_id")?, req_f(&a, "start")?);
                let track = id(&a, "track_id");
                let clip = self.ai_edit("Add clip", |p| p.add_media_clip(asset, track, start))?;
                self.selection = vec![clip];
                Ok(format!("Added clip {clip}"))
            })(),
            "analyze_audio_beats" => {
                let lookup = req_id(&a, "clip_id").and_then(|clip_id| beat_source(&self.doc.project, clip_id));
                match lookup {
                    Ok((path, seek, length, clip)) => {
                        return Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || crate::media::detect_beats(&path, seek, length).map_err(|e| e.to_string()))
                                    .await
                                    .unwrap_or_else(|e| Err(e.to_string()))
                            },
                            move |r| Message::ToolBeats(call.clone(), r.map(|beats| beats.into_iter().map(|b| clip.timeline_time(seek + b)).collect())),
                        );
                    }
                    Err(e) => Err(e),
                }
            }
            "add_text" => (|| {
                let start = req_f(&a, "start")?;
                let track = id(&a, "track_id");
                let duration = f(&a, "duration").unwrap_or(4.0);
                let mut layer = TextLayer { text: s(&a, "text").unwrap_or("Text").replace("\\n", "\n"), ..Default::default() };
                if !layer.text.is_ascii() && s(&a, "font").is_none() {
                    layer.font = "Hind Siliguri".into();
                    layer.weight = 600;
                    layer.letter_spacing = 0.0;
                }
                let clip = self.ai_edit("Add text", |p| {
                    let id = p.add_text_clip(track, start, duration.max(0.1), layer)?;
                    let c = p.clip_mut(id)?;
                    apply_props(c, &a)?;
                    Ok(id)
                })?;
                self.selection = vec![clip];
                Ok(format!("Added text clip {clip}"))
            })(),
            "add_shape" => (|| {
                let start = req_f(&a, "start")?;
                let fill = color(&a, "color")?.context("`color` is required")?;
                let kind = match s(&a, "shape").unwrap_or("rectangle") {
                    "ellipse" | "circle" => ShapeKind::Ellipse,
                    "triangle" => ShapeKind::Triangle,
                    "star" => ShapeKind::Star,
                    "hexagon" => ShapeKind::Hexagon,
                    "ring" => ShapeKind::Ring,
                    "frame" | "box" => ShapeKind::Frame,
                    _ => ShapeKind::Rectangle,
                };
                let mut shape = ShapeLayer::new(kind, req_f(&a, "width")?.max(1.0), req_f(&a, "height")?.max(1.0), fill, color(&a, "color_to")?);
                if let Some(v) = f(&a, "gradient_angle") {
                    shape.gradient_angle = v;
                }
                shape.radial = s(&a, "gradient") == Some("radial");
                if let Some(v) = f(&a, "thickness") {
                    shape.thickness = v.max(1.0);
                }
                let track = id(&a, "track_id");
                let duration = f(&a, "duration").unwrap_or(4.0);
                let clip = self.ai_edit("Add shape", |p| {
                    let id = p.add_shape_clip(track, start, duration.max(0.1), shape)?;
                    let c = p.clip_mut(id)?;
                    apply_props(c, &a)?;
                    Ok(id)
                })?;
                self.selection = vec![clip];
                Ok(format!("Added shape clip {clip}"))
            })(),
            "add_glow_icon" => (|| {
                let start = req_f(&a, "start")?;
                let icon = callout_icon(s(&a, "icon").context("`icon` is required")?)?;
                let fill = color(&a, "color")?.context("`color` is required")?;
                let mut callout = crate::clip::CalloutLayer::new(icon, fill);
                callout.pulse = a.get("pulse").and_then(Value::as_bool).unwrap_or(false);
                let track = id(&a, "track_id");
                let duration = f(&a, "duration").unwrap_or(1.3);
                let clip = self.ai_edit("Add glow icon", |p| {
                    let id = p.add_callout_clip(track, start, duration.max(0.1), callout)?;
                    let c = p.clip_mut(id)?;
                    apply_props(c, &a)?;
                    Ok(id)
                })?;
                self.selection = vec![clip];
                Ok(format!("Added glow icon clip {clip}"))
            })(),
            "add_counter" => (|| {
                let start = req_f(&a, "start")?;
                let (from, to) = (req_f(&a, "from")?, req_f(&a, "to")?);
                let track = id(&a, "track_id");
                let duration = f(&a, "duration").unwrap_or(2.0);
                let clip = self.ai_edit("Add counter", |p| {
                    let id = p.add_counter_clip(track, start, duration.max(0.1), crate::clip::CounterLayer::new(from, to))?;
                    let c = p.clip_mut(id)?;
                    apply_props(c, &a)?;
                    Ok(id)
                })?;
                self.selection = vec![clip];
                Ok(format!("Added counter clip {clip}"))
            })(),
            "add_camera_move" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let push_in = match s(&a, "move") {
                    Some("push_in") => true,
                    Some("pull_back") => false,
                    _ => anyhow::bail!("`move` must be \"push_in\" or \"pull_back\""),
                };
                self.ai_edit(if push_in { "Push in" } else { "Pull back" }, |p| {
                    let c0 = p.clip(clip).context("That clip no longer exists")?;
                    let local = f(&a, "start").map(|t| t - c0.start).unwrap_or(0.0);
                    if push_in { p.push_in(clip, local) } else { p.pull_back(clip, local) }
                })?;
                Ok(format!("Added a camera {} on clip {clip}", if push_in { "push in" } else { "pull back" }))
            })(),
            "add_press_pulse" => (|| {
                let (clip, at) = (req_id(&a, "clip_id")?, req_f(&a, "at")?);
                let duration = f(&a, "duration").unwrap_or(crate::clip::PressPulse::DEFAULT_DURATION).max(0.02);
                let pid = self.ai_edit("Add press pulse", |p| {
                    let pid = p.alloc_id();
                    let c = p.clip_mut(clip)?;
                    let local = (at - c.start).clamp(0.0, c.duration);
                    c.presses.push(crate::clip::PressPulse { id: pid, at: local, duration });
                    Ok(pid)
                })?;
                Ok(format!("Added press pulse {pid}"))
            })(),
            "remove_press_pulse" => (|| {
                let (clip, pid) = (req_id(&a, "clip_id")?, req_id(&a, "pulse_id")?);
                self.ai_edit("Remove press pulse", |p| {
                    p.clip_mut(clip)?.presses.retain(|x| x.id != pid);
                    Ok(())
                })?;
                Ok("Removed press pulse".into())
            })(),
            "stagger_entrances" => (|| {
                let ids: Vec<Id> = a["clip_ids"].as_array().map(|v| v.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                let gap = a.get("gap_frames").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(3);
                let fps = self.doc.project.canvas.fps;
                self.ai_edit("Stagger entrances", |p| p.stagger_group(&ids, gap, fps))?;
                Ok(format!("Staggered {} clips' entrances", ids.len()))
            })(),
            "update_clip" => (|| {
                let clip = req_id(&a, "clip_id")?;
                self.ai_edit("Update clip", |p| {
                    if let Some(speed) = f(&a, "speed") {
                        p.set_speed(clip, speed)?;
                    }
                    let c = p.clip_mut(clip)?;
                    if let Some(n) = s(&a, "name") {
                        c.name = Some(n.to_string());
                    }
                    apply_props(c, &a)
                })?;
                Ok(format!("Updated clip {clip}"))
            })(),
            "move_clip" => (|| {
                let (clip, start) = (req_id(&a, "clip_id")?, req_f(&a, "start")?);
                let track = match id(&a, "track_id") {
                    Some(t) => t,
                    None => self.doc.project.clip(clip).context("Unknown clip")?.track,
                };
                self.ai_edit("Move clip", |p| p.move_clip(clip, track, start))?;
                let c = self.doc.project.clip(clip).unwrap();
                Ok(format!("Clip {clip} now at {:.2}–{:.2}s", c.start, c.end()))
            })(),
            "trim_clip" => (|| {
                let clip = req_id(&a, "clip_id")?;
                self.ai_edit("Trim clip", |p| {
                    if let Some(t) = f(&a, "start") {
                        p.trim(clip, crate::edit::Edge::Start, t)?;
                    }
                    if let Some(t) = f(&a, "end") {
                        p.trim(clip, crate::edit::Edge::End, t)?;
                    }
                    Ok(())
                })?;
                let c = self.doc.project.clip(clip).unwrap();
                Ok(format!("Clip {clip} now at {:.2}–{:.2}s", c.start, c.end()))
            })(),
            "split_clip" => (|| {
                let t = req_f(&a, "time")?;
                let targets: Vec<Id> = match id(&a, "clip_id") {
                    Some(c) => vec![c],
                    None => self.doc.project.clips.iter().filter(|c| t > c.start && t < c.end()).map(|c| c.id).collect(),
                };
                anyhow::ensure!(!targets.is_empty(), "No clip under {t:.2}s");
                let new = self.ai_edit("Split", |p| targets.iter().map(|&c| p.split(c, t)).collect::<anyhow::Result<Vec<_>>>())?;
                Ok(format!("Split; new clips {new:?}"))
            })(),
            "delete_clips" => (|| {
                let ids: Vec<Id> = a["clip_ids"].as_array().map(|v| v.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                self.ai_edit("Delete clips", |p| p.delete_clips(&ids))?;
                Ok(format!("Deleted {} clip(s)", ids.len()))
            })(),
            "duplicate_clip" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let new = self.ai_edit("Duplicate clip", |p| p.duplicate_clip(clip))?;
                Ok(format!("New clip {new}"))
            })(),
            "add_zoom" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let (start, duration, scale) = (req_f(&a, "start")?, req_f(&a, "duration")?, req_f(&a, "scale")?);
                let (fx, fy) = (req_f(&a, "focus_x")?, req_f(&a, "focus_y")?);
                let ramp = f(&a, "ease").unwrap_or(0.6);
                let zid = self.ai_edit("Add zoom", |p| {
                    let zid = p.alloc_id();
                    let c = p.clip_mut(clip)?;
                    let at = (start - c.start).clamp(0.0, c.duration);
                    c.zooms.push(Zoom { id: zid, at, duration: duration.min(c.duration - at).max(0.2), scale: scale.clamp(1.0, 6.0), focus_x: fx.clamp(0.0, 1.0), focus_y: fy.clamp(0.0, 1.0), ramp });
                    Ok(zid)
                })?;
                Ok(format!("Added zoom {zid}"))
            })(),
            "update_zoom" => (|| {
                let (clip, zid) = (req_id(&a, "clip_id")?, req_id(&a, "zoom_id")?);
                self.ai_edit("Update zoom", |p| {
                    let c = p.clip_mut(clip)?;
                    let start = c.start;
                    let z = c.zooms.iter_mut().find(|z| z.id == zid).context("Unknown zoom")?;
                    if let Some(v) = f(&a, "start") {
                        z.at = (v - start).max(0.0);
                    }
                    if let Some(v) = f(&a, "duration") {
                        z.duration = v.max(0.2);
                    }
                    if let Some(v) = f(&a, "scale") {
                        z.scale = v.clamp(1.0, 6.0);
                    }
                    if let Some(v) = f(&a, "focus_x") {
                        z.focus_x = v.clamp(0.0, 1.0);
                    }
                    if let Some(v) = f(&a, "focus_y") {
                        z.focus_y = v.clamp(0.0, 1.0);
                    }
                    if let Some(v) = f(&a, "ease") {
                        z.ramp = v.max(0.0);
                    }
                    Ok(())
                })?;
                Ok("Updated zoom".into())
            })(),
            "remove_zoom" => (|| {
                let (clip, zid) = (req_id(&a, "clip_id")?, req_id(&a, "zoom_id")?);
                self.ai_edit("Remove zoom", |p| {
                    p.clip_mut(clip)?.zooms.retain(|z| z.id != zid);
                    Ok(())
                })?;
                Ok("Removed zoom".into())
            })(),
            "arrange_clip" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let front = match s(&a, "to") {
                    Some("front") => true,
                    Some("back") => false,
                    _ => anyhow::bail!("`to` must be \"front\" or \"back\""),
                };
                self.ai_edit(if front { "Bring to front" } else { "Send to back" }, |p| p.arrange(clip, front))?;
                Ok(format!("Clip {clip} is now {}", if front { "in front of everything" } else { "behind everything" }))
            })(),
            "style_text" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let shown = self.ai_edit("Style words", |p| {
                    let Content::Text(tl) = &mut p.clip_mut(clip)?.content else { anyhow::bail!("Clip {clip} isn't a text clip") };
                    let ranges = text_ranges(tl, &a)?;
                    let mut changes = Vec::new();
                    if a["clear"].as_bool() == Some(true) {
                        changes.push(SpanChange::Clear);
                    }
                    if let Some(c) = color(&a, "color")? {
                        changes.push(SpanChange::Color(Some(c)));
                    }
                    if let Some(w) = a.get("weight").and_then(Value::as_u64) {
                        changes.push(SpanChange::Weight(Some((w as u16).clamp(100, 900))));
                    }
                    if let Some(font) = s(&a, "font") {
                        changes.push(SpanChange::Font(Some(font.to_string())));
                    }
                    if let Some(k) = f(&a, "size_scale") {
                        changes.push(SpanChange::Scale(Some(k)));
                    }
                    match s(&a, "highlight") {
                        Some("none") => changes.push(SpanChange::Highlight(None)),
                        Some(_) => changes.push(SpanChange::Highlight(color(&a, "highlight")?)),
                        None => {}
                    }
                    if let Some(on) = a["underline"].as_bool() {
                        changes.push(SpanChange::Underline(on));
                    }
                    anyhow::ensure!(!changes.is_empty(), "Pass a style to apply (color, weight, font, size_scale, highlight, underline or clear)");
                    for &(start, end) in &ranges {
                        for change in &changes {
                            tl.style_range(start, end, change);
                        }
                    }
                    Ok(ranges.iter().map(|&(s, e)| tl.text.chars().skip(s).take(e - s).collect::<String>()).collect::<Vec<_>>())
                })?;
                Ok(format!("Styled {} in clip {clip}", shown.iter().map(|w| format!("“{w}”")).collect::<Vec<_>>().join(", ")))
            })(),
            "set_keyframes" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let pr = prop(s(&a, "property").unwrap_or(""))?;
                let keys = a["keys"].as_array().context("`keys` must be a list")?.clone();
                self.ai_edit("Animate", |p| {
                    let c = p.clip_mut(clip)?;
                    let mut ks: Vec<Keyframe> = keys
                        .iter()
                        .map(|k| Ok(Keyframe { t: req_f(k, "time")? - c.start, value: req_f(k, "value")?, ease: ease(s(k, "ease")) }))
                        .collect::<anyhow::Result<_>>()?;
                    ks.sort_by(|x, y| x.t.total_cmp(&y.t));
                    c.keyframes.insert(pr, ks);
                    Ok(())
                })?;
                Ok("Keyframes set".into())
            })(),
            "clear_keyframes" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let pr = s(&a, "property").map(prop).transpose()?;
                self.ai_edit("Clear animation", |p| {
                    let c = p.clip_mut(clip)?;
                    match pr {
                        Some(pr) => {
                            c.keyframes.remove(&pr);
                        }
                        None => c.keyframes.clear(),
                    }
                    Ok(())
                })?;
                Ok("Keyframes cleared".into())
            })(),
            "set_range_volume" => (|| {
                let (clip, a0, b0, v) = (req_id(&a, "clip_id")?, req_f(&a, "start")?, req_f(&a, "end")?, req_f(&a, "volume")?);
                self.ai_edit("Range volume", |p| p.set_range_volume(clip, a0, b0, v))?;
                Ok(format!("Volume {:.0}% between {a0:.2}s and {b0:.2}s", v * 100.0))
            })(),
            "add_blur" => (|| {
                let start = req_f(&a, "start")?;
                let end = f(&a, "end").unwrap_or(start + 3.0);
                anyhow::ensure!(end - start >= 0.1, "The blur must last at least 0.1 s");
                let (x, y, w, h) = (req_f(&a, "x")?, req_f(&a, "y")?, req_f(&a, "width")?, req_f(&a, "height")?);
                // Canvas fractions to canvas units (px at a 1080 short side).
                let (cw, ch) = self.doc.project.canvas.size();
                let per = 1080.0 / cw.min(ch) as f64;
                let layer = crate::clip::BlurLayer { width: (w * cw as f64 * per).clamp(4.0, 4000.0), height: (h * ch as f64 * per).clamp(4.0, 4000.0), strength: f(&a, "strength").unwrap_or(60.0).clamp(0.0, 100.0), ellipse: a.get("ellipse").and_then(Value::as_bool).unwrap_or(false) };
                let track = id(&a, "track_id");
                let clip = self.ai_edit("Add blur", |p| {
                    let id = p.add_blur_clip(track, start, end - start, layer)?;
                    let c = p.clip_mut(id)?;
                    (c.transform.x, c.transform.y) = (x, y);
                    Ok(id)
                })?;
                self.selection = vec![clip];
                Ok(format!("Added blur clip {clip}"))
            })(),
            "update_blur" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let (cw, ch) = self.doc.project.canvas.size();
                let per = 1080.0 / cw.min(ch) as f64;
                self.ai_edit("Update blur", |p| {
                    let c = p.clip_mut(clip)?;
                    let Content::Blur(b) = &mut c.content else { anyhow::bail!("Clip {clip} is not a blur") };
                    if let Some(v) = f(&a, "width") {
                        b.width = (v * cw as f64 * per).clamp(4.0, 4000.0);
                    }
                    if let Some(v) = f(&a, "height") {
                        b.height = (v * ch as f64 * per).clamp(4.0, 4000.0);
                    }
                    if let Some(v) = f(&a, "strength") {
                        b.strength = v.clamp(0.0, 100.0);
                    }
                    if let Some(v) = a.get("ellipse").and_then(Value::as_bool) {
                        b.ellipse = v;
                    }
                    if let Some(v) = f(&a, "x") {
                        c.transform.x = v;
                    }
                    if let Some(v) = f(&a, "y") {
                        c.transform.y = v;
                    }
                    Ok(())
                })?;
                Ok("Updated blur".into())
            })(),
            "add_mask" => (|| {
                let start = req_f(&a, "start")?;
                let end = f(&a, "end").unwrap_or(start + 3.0);
                anyhow::ensure!(end - start >= 0.1, "The mask must last at least 0.1 s");
                let (x, y, w, h) = (req_f(&a, "x")?, req_f(&a, "y")?, req_f(&a, "width")?, req_f(&a, "height")?);
                let (cw, ch) = self.doc.project.canvas.size();
                let per = 1080.0 / cw.min(ch) as f64;
                let layer = crate::clip::MaskLayer {
                    width: (w * cw as f64 * per).clamp(4.0, 4000.0),
                    height: (h * ch as f64 * per).clamp(4.0, 4000.0),
                    feather: f(&a, "feather").unwrap_or(15.0).clamp(0.0, 100.0),
                    ellipse: a.get("ellipse").and_then(Value::as_bool).unwrap_or(true),
                    invert: a.get("invert").and_then(Value::as_bool).unwrap_or(false),
                };
                let track = id(&a, "track_id");
                let clip = self.ai_edit("Add mask", |p| {
                    let id = p.add_mask_clip(track, start, end - start, layer)?;
                    let c = p.clip_mut(id)?;
                    (c.transform.x, c.transform.y) = (x, y);
                    Ok(id)
                })?;
                self.selection = vec![clip];
                Ok(format!("Added mask clip {clip}"))
            })(),
            "update_mask" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let (cw, ch) = self.doc.project.canvas.size();
                let per = 1080.0 / cw.min(ch) as f64;
                self.ai_edit("Update mask", |p| {
                    let c = p.clip_mut(clip)?;
                    let Content::Mask(mask) = &mut c.content else { anyhow::bail!("Clip {clip} is not a mask") };
                    if let Some(v) = f(&a, "width") {
                        mask.width = (v * cw as f64 * per).clamp(4.0, 4000.0);
                    }
                    if let Some(v) = f(&a, "height") {
                        mask.height = (v * ch as f64 * per).clamp(4.0, 4000.0);
                    }
                    if let Some(v) = f(&a, "feather") {
                        mask.feather = v.clamp(0.0, 100.0);
                    }
                    if let Some(v) = a.get("ellipse").and_then(Value::as_bool) {
                        mask.ellipse = v;
                    }
                    if let Some(v) = a.get("invert").and_then(Value::as_bool) {
                        mask.invert = v;
                    }
                    if let Some(v) = f(&a, "x") {
                        c.transform.x = v;
                    }
                    if let Some(v) = f(&a, "y") {
                        c.transform.y = v;
                    }
                    Ok(())
                })?;
                Ok("Updated mask".into())
            })(),
            "separate_audio" => (|| {
                let clip = req_id(&a, "clip_id")?;
                let sound = self.ai_edit("Separate audio", |p| p.separate_audio(clip))?;
                Ok(format!("The sound is now audio clip {sound}; video clip {clip} is silent"))
            })(),
            "add_marker" => (|| {
                let at = req_f(&a, "time")?;
                let color = color(&a, "color")?.unwrap_or(Rgba::hex(super::library::MARKER_COLORS[0]));
                let duration = f(&a, "duration").unwrap_or(0.0);
                let name = s(&a, "name").map(str::to_string);
                let mid = self.ai_edit("Add marker", |p| {
                    let id = p.add_marker(at, duration, color);
                    if let Some(n) = name {
                        p.marker_mut(id)?.name = n;
                    }
                    Ok(id)
                })?;
                Ok(format!("Added marker {mid}"))
            })(),
            "update_marker" => (|| {
                let mid = req_id(&a, "marker_id")?;
                let col = color(&a, "color")?;
                self.ai_edit("Update marker", |p| {
                    if let Some(t) = f(&a, "time") {
                        p.move_marker(mid, t)?;
                    }
                    let m = p.marker_mut(mid)?;
                    if let Some(d) = f(&a, "duration") {
                        m.duration = d.max(0.0);
                    }
                    if let Some(n) = s(&a, "name") {
                        m.name = n.to_string();
                    }
                    if let Some(c) = col {
                        m.color = c;
                    }
                    Ok(())
                })?;
                Ok("Updated marker".into())
            })(),
            "delete_marker" => (|| {
                let mid = req_id(&a, "marker_id")?;
                self.ai_edit("Delete marker", |p| p.delete_marker(mid))?;
                Ok("Deleted marker".into())
            })(),
            "add_sound" => {
                let sound = s(&a, "sound").unwrap_or("").to_string();
                match crate::sfx::SOUNDS.iter().find(|x| x.id == sound) {
                    None => Err(anyhow!("Unknown sound `{sound}`")),
                    Some(def) => {
                        let id: &'static str = def.id;
                        return Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    let path = crate::sfx::path(id).map_err(|e| e.to_string())?;
                                    let info = crate::media::probe(&path).map_err(|e| e.to_string())?;
                                    Ok((path, info))
                                })
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()))
                            },
                            move |r| Message::ToolSound(call.clone(), id, r),
                        );
                    }
                }
            }
            "add_track" => (|| {
                let kind = if s(&a, "kind") == Some("audio") { TrackKind::Audio } else { TrackKind::Visual };
                let name = s(&a, "name").unwrap_or(if kind == TrackKind::Audio { "Audio" } else { "Video" }).to_string();
                let t = self.ai_edit("Add track", |p| Ok(p.add_track(kind, &name, None)))?;
                Ok(format!("Added track {t}"))
            })(),
            "rename_track" => (|| {
                let (t, name) = (req_id(&a, "track_id")?, s(&a, "name").unwrap_or("").to_string());
                self.ai_edit("Rename track", |p| p.rename_track(t, &name))?;
                Ok("Renamed".into())
            })(),
            "set_canvas" => (|| {
                let mut canvas = self.doc.project.canvas;
                if let Some(v) = s(&a, "aspect") {
                    canvas.aspect = Aspect::ALL.into_iter().find(|x| x.label() == v).context("Unknown aspect")?;
                }
                if let Some(v) = s(&a, "resolution") {
                    canvas.resolution = Resolution::ALL.into_iter().find(|x| x.label().eq_ignore_ascii_case(v)).context("Unknown resolution")?;
                }
                if let Some(v) = s(&a, "fps").and_then(|v| v.parse().ok()).or_else(|| a.get("fps").and_then(Value::as_u64).map(|v| v as u32)) {
                    anyhow::ensure!(crate::model::FRAME_RATES.contains(&v), "Frame rate must be one of 24, 25, 30 or 60");
                    canvas.fps = v;
                }
                self.ai_edit("Change format", |p| {
                    p.canvas = canvas;
                    Ok(())
                })?;
                Ok(format!("Canvas is now {}", canvas.summary()))
            })(),
            "get_frame" => {
                let t = f(&a, "time").unwrap_or(self.playhead).max(0.0);
                self.seek(t);
                let project = self.doc.project.clone();
                std::thread::spawn(move || {
                    let r = crate::render::export::snapshot_jpeg(&project, t, 960).map(|jpeg| mcp::image_jpeg(&jpeg, &format!("Frame at {t:.2}s"))).map_err(|e| e.to_string());
                    call.answer(r);
                });
                return Task::none();
            }
            "seek" => req_f(&a, "time").map(|t| {
                self.seek(t);
                format!("Playhead at {t:.2}s")
            }),
            "play" => {
                if self.playing.is_none() {
                    self.start_playback();
                }
                Ok("Playing".into())
            }
            "pause" => {
                self.stop_playback();
                Ok("Paused".into())
            }
            "undo" => self.doc.undo().map(|d| {
                self.after_history();
                format!("Undid: {d}")
            }).context("Nothing to undo"),
            "redo" => self.doc.redo().map(|d| {
                self.after_history();
                format!("Redid: {d}")
            }).context("Nothing to redo"),
            "export_video" => (|| {
                let path = s(&a, "path").context("`path` is required")?;
                anyhow::ensure!(std::path::Path::new(path).is_absolute(), "Use an absolute path");
                let mut dialog = super::export_dialog::ExportDialog::new(&self.doc.project, self.path.as_deref(), None);
                dialog.path = path.to_string();
                dialog.quality = match s(&a, "quality") {
                    Some("standard") => crate::render::export::Quality::Standard,
                    Some("small") => crate::render::export::Quality::Small,
                    _ => crate::render::export::Quality::High,
                };
                anyhow::ensure!(self.export.as_ref().is_none_or(|e| !e.is_running()), "An export is already running");
                self.export = Some(dialog);
                Ok("Export started — check get_export_status".into())
            })()
            .map(|msg: String| {
                call.answer(Ok(mcp::text(&msg)));
                String::new()
            }),
            "get_export_status" => Ok(match self.export.as_ref().map(|e| &e.phase) {
                Some(super::export_dialog::Phase::Running { done, total, .. }) => format!("Rendering: {done}/{total} frames"),
                Some(super::export_dialog::Phase::Done { items, .. }) => match items.as_slice() {
                    [(path, _)] => format!("Done: {}", path.display()),
                    many => format!("Done: {} files ({})", many.len(), many.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>().join(", ")),
                },
                Some(super::export_dialog::Phase::Failed(e)) => format!("Failed: {e}"),
                _ => "No export running".into(),
            }),
            other => Err(anyhow!("Unknown tool `{other}`")),
        };
        if call.name == "export_video" {
            return match result {
                Ok(_) => self.start_export(),
                Err(e) => {
                    call.answer(Err(e.to_string()));
                    Task::none()
                }
            };
        }
        call.answer(result.map(mcp::text).map_err(|e| e.to_string()));
        if self.side_tab != SideTab::Assistant && call.name != "get_project" {
            self.lanes_cache.clear();
        }
        Task::none()
    }

    /// Finishes `import_media` once files are probed.
    pub fn finish_tool_import(&mut self, call: Call, results: Vec<(PathBuf, Result<crate::media::ProbeInfo, String>)>, add: bool) -> Task<Message> {
        let at = self.playhead;
        let mut report = Vec::new();
        let mut ids = Vec::new();
        for (path, r) in results {
            match r {
                Ok(info) => match self.ai_edit("Import media", |p| {
                    let id = p.add_asset(&path, &info);
                    let clip = if add { Some(p.add_media_clip(id, None, at)?) } else { None };
                    Ok((id, clip))
                }) {
                    Ok((id, clip)) => {
                        ids.push(id);
                        report.push(format!("{} → media {id}{}", path.display(), clip.map(|c| format!(", clip {c}")).unwrap_or_default()));
                    }
                    Err(e) => report.push(format!("{}: {e}", path.display())),
                },
                Err(e) => report.push(format!("{}: {e}", path.display())),
            }
        }
        call.answer(Ok(mcp::text(report.join("\n"))));
        let assets = self.doc.project.assets.clone();
        self.media.ensure(&assets, Message::MediaLoaded)
    }

    /// Finishes `add_sound` once the sound file exists.
    pub fn finish_tool_sound(&mut self, call: Call, sound: &'static str, r: Result<(PathBuf, crate::media::ProbeInfo), String>) -> Task<Message> {
        let (volume, start) = (f(&call.args, "volume"), f(&call.args, "start").unwrap_or(self.playhead));
        let name = crate::sfx::SOUNDS.iter().find(|s| s.id == sound).map(|s| s.name).unwrap_or("Sound");
        let result = r.map_err(|e| anyhow!(e)).and_then(|(path, info)| {
            self.ai_edit(&format!("Add {name}"), |p| {
                let asset = p.add_asset(&path, &info);
                if let Some(a) = p.assets.iter_mut().find(|a| a.id == asset) {
                    a.name = name.to_string();
                }
                let clip = p.add_media_clip(asset, None, start)?;
                if let Some(v) = volume {
                    p.clip_mut(clip)?.audio.volume = v.clamp(0.0, 2.0);
                }
                Ok(clip)
            })
        });
        call.answer(result.map(|c| mcp::text(format!("Added {name} as clip {c}"))).map_err(|e| e.to_string()));
        let assets = self.doc.project.assets.clone();
        self.media.ensure(&assets, Message::MediaLoaded)
    }
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// What `analyze_audio_beats` needs from a clip: the asset file, the portion of it the clip
/// actually uses (`seek..seek+length`, in source-file seconds), and the clip itself (to map
/// beat times back to the timeline afterwards).
fn beat_source(project: &Project, clip_id: Id) -> anyhow::Result<(PathBuf, f64, f64, Clip)> {
    let clip = project.clip(clip_id).context("That clip no longer exists")?;
    if !project.clip_has_sound(clip) {
        bail!("That clip has no sound to analyze");
    }
    let (asset, src_in, speed) = match &clip.content {
        Content::Media { asset, src_in, speed } => (*asset, *src_in, *speed),
        _ => bail!("That clip has no sound to analyze"),
    };
    let path = project.asset(asset).context("That clip's media is missing")?.path.clone();
    Ok((path, src_in, clip.duration * speed, clip.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ProbeInfo;
    use crate::model::{Canvas, MediaKind};

    fn text_clip() -> Clip {
        Clip::new(1, 1, 0.0, 4.0, Content::Text(TextLayer::default()))
    }

    #[test]
    fn apply_props_sets_idle_drift_intro_ease_and_delay() {
        let mut c = text_clip();
        c.intro = Some(Motion { kind: MotionKind::Fade, duration: 0.5, ease: None, delay: 0.0 });
        c.outro = Some(Motion { kind: MotionKind::Fade, duration: 0.3, ease: None, delay: 0.0 });
        apply_props(&mut c, &json!({ "idle_drift": true, "intro_ease": "back", "intro_delay": 0.1, "outro_ease": "ease_in" })).unwrap();
        assert_eq!(c.idle_drift, Some(crate::clip::DriftKind::ScaleBreath));
        assert_eq!(c.intro.unwrap().ease, Some(Ease::Back));
        assert_eq!(c.intro.unwrap().delay, 0.1);
        assert_eq!(c.outro.unwrap().ease, Some(Ease::EaseIn));

        apply_props(&mut c, &json!({ "idle_drift": false })).unwrap();
        assert_eq!(c.idle_drift, None);

        // An intro_ease with no intro motion set is simply a no-op, not an error.
        let mut no_intro = text_clip();
        apply_props(&mut no_intro, &json!({ "intro_ease": "back" })).unwrap();
        assert!(no_intro.intro.is_none());

        // An unrecognized ease is a reported error, unlike the lenient keyframe `ease()` helper.
        assert!(apply_props(&mut c, &json!({ "intro_ease": "bounce" })).is_err());
    }

    #[test]
    fn apply_props_updates_counter_fields_and_number_style() {
        let mut c = Clip::new(1, 1, 0.0, 4.0, Content::Counter(crate::clip::CounterLayer::new(0.0, 100.0)));
        apply_props(&mut c, &json!({ "from": 10.0, "to": 250.0, "count_duration": 1.5, "decimals": 1, "prefix": "$", "suffix": "k", "font": "Poppins", "weight": 700, "size": 120.0, "color": "#FF0000", "align": "center" })).unwrap();
        let Content::Counter(counter) = &c.content else { panic!("not a counter") };
        assert_eq!((counter.from, counter.to, counter.duration, counter.decimals), (10.0, 250.0, 1.5, 1));
        assert_eq!((counter.prefix.as_str(), counter.suffix.as_str()), ("$", "k"));
        assert_eq!((counter.format.font.as_str(), counter.format.weight, counter.format.size), ("Poppins", 700, 120.0));
        assert_eq!(counter.format.color, Rgba::hex(0xFF0000));
        assert_eq!(counter.format.align, Align::Center);
    }

    #[test]
    fn ease_choice_matches_every_schema_value_and_rejects_unknown() {
        for v in EASES {
            assert!(ease_choice(v).is_ok(), "{v} should be a recognized ease");
        }
        assert!(ease_choice("bounce").is_err());
    }

    fn project_with_clip() -> (Project, Id) {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 20.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let asset = p.add_asset(&PathBuf::from("music.wav"), &info);
        let clip = p.add_media_clip(asset, None, 5.0).unwrap();
        // A clip starting at 5s, trimmed 3s into the source, playing back at double speed.
        let c = p.clip_mut(clip).unwrap();
        c.content = Content::Media { asset, src_in: 3.0, speed: 2.0 };
        c.duration = 4.0;
        (p, clip)
    }

    #[test]
    fn beat_source_reports_the_used_portion_and_can_map_times_back() {
        let (p, clip) = project_with_clip();
        let (path, seek, length, c) = beat_source(&p, clip).unwrap();
        assert_eq!(path, PathBuf::from("music.wav"));
        assert_eq!((seek, length), (3.0, 8.0), "4s of timeline at 2x speed is 8s of source, starting at src_in");
        // A beat 1s into the used source (at source time seek+1=4.0) lands at timeline 5.5s:
        // the clip starts at 5.0, and 1s of source at 2x speed is 0.5s of timeline.
        assert_eq!(c.timeline_time(seek + 1.0), 5.5);
    }

    #[test]
    fn beat_source_refuses_a_clip_without_sound() {
        let mut p = Project::new("t", Canvas::default());
        let text = p.add_text_clip(None, 0.0, 5.0, crate::clip::TextLayer::default()).unwrap();
        assert!(beat_source(&p, text).is_err());
        assert!(beat_source(&p, 99999).is_err(), "a clip id that doesn't exist");
    }

    /// `analyze_audio_beats`'s two halves (`beat_source`'s mapping and `detect_beats`'s decoding)
    /// tested together, on real audio, the way the tool actually uses them — the async
    /// `Task::perform`/MCP plumbing around them isn't (nothing else in this file tests that layer
    /// either; see `handle_tool`'s `import_media` for the same shape).
    #[test]
    fn analyze_audio_beats_lines_up_with_real_clicks_through_trim_and_speed() {
        let dir = std::env::temp_dir().join(format!("studio-tool-beats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clicks.wav");
        let status = crate::ffmpeg::ffmpeg()
            .unwrap()
            .args(["-f", "lavfi", "-i", "aevalsrc=exprs=lt(mod(t\\,0.5)\\,0.01)*sin(2*PI*1000*t):s=8000:d=4", "-y"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());

        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 4.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let asset = p.add_asset(&path, &info);
        let clip = p.add_media_clip(asset, None, 10.0).unwrap();
        // Skips the clicks at 0.0 and 0.5s; plays the rest at half speed starting at timeline 10s.
        let c = p.clip_mut(clip).unwrap();
        c.content = Content::Media { asset, src_in: 1.0, speed: 2.0 };
        c.duration = 1.0; // source 1.0..3.0 → clicks at 1.0, 1.5, 2.0, 2.5

        let (resolved, seek, length, c) = beat_source(&p, clip).unwrap();
        let beats = crate::media::detect_beats(&resolved, seek, length).unwrap();
        let timeline: Vec<f64> = beats.into_iter().map(|b| c.timeline_time(seek + b)).collect();
        // Source clicks 1.0/1.5/2.0/2.5, half speed from timeline 10s: 10.0/10.25/10.5/10.75.
        let expected = [10.0, 10.25, 10.5, 10.75];
        assert_eq!(timeline.len(), expected.len(), "beats: {timeline:?}");
        for (t, e) in timeline.iter().zip(expected) {
            assert!((t - e).abs() < 0.05, "{t} should be near {e}");
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
