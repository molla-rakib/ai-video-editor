//! Per-user settings: recent projects and the editor layout.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    pub summary: String,
    pub opened_at: u64,
}

/// Split ratios of the editor panes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub top_bottom: f32,
    pub library: f32,
    pub preview_inspector: f32,
    /// The timeline keeps its compact default height until the user drags its edge.
    /// (Saved under a new key when the compact height got smaller, so every layout starts compact once.)
    #[serde(default = "yes", rename = "timeline_compact")]
    pub timeline_auto: bool,
}

fn yes() -> bool {
    true
}

impl Default for Layout {
    fn default() -> Self {
        Layout { top_bottom: 0.6, library: 0.2, preview_inspector: 0.74, timeline_auto: true }
    }
}

/// The AI service the assistant uses. Nothing is used until the user connects one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiProvider {
    ClaudeCode,
    /// ChatGPT, through OpenAI's Codex CLI signed in with the user's ChatGPT account.
    Codex,
}

impl AiProvider {
    pub fn name(self) -> &'static str {
        match self {
            AiProvider::ClaudeCode => "Claude Code",
            AiProvider::Codex => "ChatGPT",
        }
    }
}

/// What the Record camera dialog was last set to, by device name (devices come and go).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordChoice {
    pub camera: Option<String>,
    pub mic: Option<String>,
    /// "No microphone" was chosen.
    pub no_mic: bool,
    /// Quarter turns clockwise.
    pub turns: u8,
    pub mirror: bool,
    /// Flipped top to bottom.
    pub flip: bool,
}

/// What the Record screen dialog was last set to, by name (screens and microphones come and go).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenChoice {
    pub screen: Option<String>,
    pub mic: Option<String>,
    /// "No microphone" was chosen.
    pub no_mic: bool,
    /// Pictures a second (0 = never chosen: 30).
    pub fps: u32,
    /// The camera recorded with the screen, by name.
    pub camera: Option<String>,
    /// "No camera" was chosen. Otherwise a camera is recorded with the screen whenever one is
    /// connected (the one above, or the first), as screen recorders do.
    pub no_camera: bool,
    /// The camera's picture is mirrored.
    pub mirror: bool,
    /// The camera's picture is flipped top to bottom.
    pub flip: bool,
    /// The camera's picture, quarter turns clockwise.
    pub turns: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub recent: Vec<RecentProject>,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub ai: Option<AiProvider>,
    /// Inspector sections the user collapsed.
    #[serde(default)]
    pub inspector_collapsed: std::collections::BTreeSet<String>,
    /// The inspector tab each kind of clip was last left on ("Video" → Color).
    #[serde(default)]
    pub inspector_tabs: std::collections::BTreeMap<String, crate::editor::inspector::Tab>,
    /// Effects the user starred (by `Effect::key`), in the order they were starred.
    #[serde(default)]
    pub favorite_effects: Vec<String>,
    /// Effects-tab groups the user opened (they all start collapsed).
    #[serde(default)]
    pub effects_open: std::collections::BTreeSet<String>,
    /// Choices from the Settings dialog.
    #[serde(default)]
    pub prefs: Prefs,
    /// The camera and microphone used for the last recording.
    #[serde(default)]
    pub record: RecordChoice,
    /// The screen, microphone and frame rate used for the last screen recording.
    #[serde(default)]
    pub screen_record: ScreenChoice,
    /// Reusable instruction blocks for the AI assistant (see `Skill`).
    #[serde(default)]
    pub skills: Vec<Skill>,
    /// Whether `default_skill()` has already been seeded into `skills` once. Runs only on the
    /// first load ever (fresh installs and any settings file saved before skills existed), so
    /// deleting the starter skill doesn't bring it back on the next launch.
    #[serde(default)]
    pub skills_seeded: bool,
}

/// A reusable block of instructions for the AI assistant: written once from the assistant
/// panel, then switched on or off per conversation instead of retyping it every time. Enabled
/// skills are appended to the system prompt sent to whichever provider is connected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub body: String,
    pub enabled: bool,
}

/// Seeded once into every fresh install, so the Skills manager isn't empty the first time it's
/// opened and new users have a real example to edit or delete.
fn default_skill() -> Skill {
    Skill {
        name: "Motion polish".into(),
        body: "Extra motion-design taste on top of your base instructions:\n\n\
- Staggered groups: when several related elements enter together (a checklist, feature cards, bullet points), use \
stagger_entrances (or set intro_delay per clip) instead of letting every intro fire on the same instant.\n\
- Multi-aspect safety: if a project might get re-cropped later for 9:16 or 1:1 social cutdowns, keep key subjects and all text \
within the center square of the frame (roughly x 0.2-0.8 on a 16:9 canvas), not just the usual edge margins.\n\
- Reach for the right accent, sparingly: add_press_pulse when something is clicked or tapped in a screen recording, add_counter \
for a before/after number, add_camera_move for a slow deliberate push in or pull back (add_zoom is for calling attention to \
part of the frame instead), idle_drift on a held shot that would otherwise look frozen. Use them like glow icons — only where \
they genuinely earn their place, not on every clip."
            .into(),
        enabled: true,
    }
}

/// How sharp the preview is decoded. Only the preview: exports are always full quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PreviewQuality {
    /// High while paused; steps down during playback when frames arrive late.
    #[default]
    Auto,
    /// The video's own resolution.
    Full,
    /// A little above the on-screen size, so high-DPI screens stay sharp.
    High,
    /// The on-screen size.
    Balanced,
    /// Half the on-screen size: smoothest on slower computers.
    Draft,
}

impl PreviewQuality {
    pub const ALL: [PreviewQuality; 5] = [PreviewQuality::Auto, PreviewQuality::Full, PreviewQuality::High, PreviewQuality::Balanced, PreviewQuality::Draft];

    pub fn label(self) -> &'static str {
        match self {
            PreviewQuality::Auto => "Auto",
            PreviewQuality::Full => "Full",
            PreviewQuality::High => "High",
            PreviewQuality::Balanced => "Balanced",
            PreviewQuality::Draft => "Draft",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PreviewQuality::Auto => "Sharp while paused; lowers itself during playback if your computer can't keep up. Recommended.",
            PreviewQuality::Full => "Every pixel of the video. Uses the most memory and power.",
            PreviewQuality::High => "Sharp on high-DPI screens, always.",
            PreviewQuality::Balanced => "Matches the preview's size on screen.",
            PreviewQuality::Draft => "Half resolution, for smooth playback on slower computers.",
        }
    }

    /// The next lower level, for Auto when playback can't keep up.
    pub fn lower(self) -> PreviewQuality {
        match self {
            PreviewQuality::Full | PreviewQuality::Auto | PreviewQuality::High => PreviewQuality::Balanced,
            _ => PreviewQuality::Draft,
        }
    }

    /// Decode width for a preview shown `display` points wide, for a canvas `canvas` pixels wide.
    pub fn decode_width(self, display: f32, canvas: f32) -> f32 {
        match self {
            PreviewQuality::Full => canvas,
            PreviewQuality::High | PreviewQuality::Auto => (display * 1.5).min(canvas),
            PreviewQuality::Balanced => display.min(canvas),
            PreviewQuality::Draft => (display * 0.5).min(canvas),
        }
    }
}

/// User preferences (the Settings dialog). Every field has a default, so older settings files load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub preview_quality: PreviewQuality,
    /// Size of the whole interface (1 = 100%).
    pub ui_scale: f32,
    /// Snapping is on when a project opens.
    pub snapping: bool,
    /// Length of new text, shapes and images, in seconds.
    pub still_seconds: f64,
    /// Seconds after a change before the project saves itself.
    pub autosave_seconds: u64,
    /// The preview starts muted (off by default: the preview plays its sound). Saved under a new
    /// key when sound-on became the rule, so earlier "muted" choices reset once.
    #[serde(rename = "preview_starts_muted")]
    pub start_muted: bool,
    pub export_quality: crate::render::export::Quality,
    /// Where new projects go (None = Videos/AI Video Editor).
    pub projects_dir: Option<PathBuf>,
    /// Keyboard shortcuts the user changed.
    pub keys: crate::shortcuts::Keymap,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            preview_quality: PreviewQuality::Auto,
            ui_scale: 1.0,
            snapping: true,
            still_seconds: 5.0,
            autosave_seconds: 2,
            start_muted: false,
            export_quality: crate::render::export::Quality::High,
            projects_dir: None,
            keys: Default::default(),
        }
    }
}

/// Choices offered in the Settings dialog.
pub const UI_SCALES: [f32; 4] = [0.9, 1.0, 1.1, 1.25];
pub const STILL_SECONDS: [f64; 4] = [3.0, 5.0, 8.0, 10.0];
pub const AUTOSAVE_SECONDS: [u64; 4] = [2, 5, 15, 30];

impl Prefs {
    /// The values clamped to what the app supports (settings files can be edited by hand).
    pub fn sane(&self) -> Prefs {
        Prefs {
            ui_scale: if self.ui_scale.is_finite() { self.ui_scale.clamp(0.75, 1.5) } else { 1.0 },
            still_seconds: if self.still_seconds.is_finite() { self.still_seconds.clamp(0.5, 60.0) } else { 5.0 },
            autosave_seconds: self.autosave_seconds.clamp(1, 300),
            ..self.clone()
        }
    }
}

const MAX_RECENT: usize = 12;

impl Settings {
    pub fn load() -> Settings {
        let mut s: Settings = std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if !s.skills_seeded {
            s.skills.insert(0, default_skill());
            s.skills_seeded = true;
            s.save();
        }
        s
    }

    pub fn save(&self) {
        let path = file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn touch_recent(&mut self, name: &str, path: &Path, summary: String) {
        self.recent.retain(|r| r.path != path);
        let opened_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.recent.insert(0, RecentProject { name: name.to_string(), path: path.to_path_buf(), summary, opened_at });
        self.recent.truncate(MAX_RECENT);
    }
}

pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("AIVideoEditor")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_default()
            .join("ai-video-editor")
    }
}

fn file() -> PathBuf {
    config_dir().join("studio.json")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

/// Folder where new projects are saved by default.
pub fn projects_dir() -> PathBuf {
    let home = home_dir();
    let videos = ["Videos", "Movies"].iter().map(|d| home.join(d)).find(|d| d.is_dir()).unwrap_or(home);
    videos.join("AI Video Editor")
}

impl Settings {
    /// Where new projects go: the folder chosen in Settings, if it still exists, or the default.
    pub fn projects_dir(&self) -> PathBuf {
        self.prefs.projects_dir.clone().filter(|d| d.is_dir()).unwrap_or_else(projects_dir)
    }
}

/// "just now", "5 min ago", "3 days ago".
pub fn relative_time(unix: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(unix);
    let s = now.saturating_sub(unix);
    match s {
        0..60 => "Just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        86400..172800 => "Yesterday".into(),
        _ => format!("{} days ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_files_load_with_default_prefs() {
        let s: Settings = serde_json::from_str(r#"{ "recent": [], "layout": { "top_bottom": 0.6, "library": 0.2, "preview_inspector": 0.74 } }"#).unwrap();
        assert_eq!(s.prefs, Prefs::default());
        assert!(s.layout.timeline_auto, "old layouts get the compact timeline");
        let resized: Settings = serde_json::from_str(r#"{ "recent": [], "layout": { "top_bottom": 0.6, "library": 0.2, "preview_inspector": 0.74, "timeline_auto": false } }"#).unwrap();
        assert!(resized.layout.timeline_auto, "a timeline sized before the smaller default starts compact again");
        assert_eq!(s.prefs.preview_quality, PreviewQuality::Auto);
        // The preview plays its sound by default; an old "start muted" choice resets once.
        assert!(!Prefs::default().start_muted);
        let muted: Settings = serde_json::from_str(r#"{ "recent": [], "prefs": { "start_muted": true } }"#).unwrap();
        assert!(!muted.prefs.start_muted);
        let chosen = Settings { prefs: Prefs { start_muted: true, ..Prefs::default() }, ..s };
        let again: Settings = serde_json::from_str(&serde_json::to_string(&chosen).unwrap()).unwrap();
        assert!(again.prefs.start_muted, "choosing it again in Settings sticks");
    }

    #[test]
    fn hand_edited_prefs_are_kept_in_range() {
        let p = Prefs { ui_scale: f32::NAN, still_seconds: -3.0, autosave_seconds: 0, ..Prefs::default() }.sane();
        assert_eq!((p.ui_scale, p.still_seconds, p.autosave_seconds), (1.0, 0.5, 1));
    }

    #[test]
    fn preview_quality_sets_the_decode_width() {
        assert_eq!(PreviewQuality::Full.decode_width(800.0, 1920.0), 1920.0);
        assert_eq!(PreviewQuality::High.decode_width(800.0, 1920.0), 1200.0);
        assert_eq!(PreviewQuality::Balanced.decode_width(800.0, 1920.0), 800.0);
        assert_eq!(PreviewQuality::Draft.decode_width(800.0, 1920.0), 400.0);
        assert_eq!(PreviewQuality::High.decode_width(2000.0, 1920.0), 1920.0, "never above the video");
        // Auto steps down while playback can't keep up, and stops at Draft.
        assert_eq!(PreviewQuality::Auto.lower(), PreviewQuality::Balanced);
        assert_eq!(PreviewQuality::Balanced.lower().lower(), PreviewQuality::Draft);
    }

    #[test]
    fn a_fresh_or_pre_skills_settings_file_gets_seeded_once() {
        // A brand-new install (no file at all) decodes to `Settings::default()`: no skills, not
        // yet seeded — the same shape `load()` falls back to.
        let mut fresh = Settings::default();
        assert!(fresh.skills.is_empty() && !fresh.skills_seeded);
        fresh.skills.insert(0, default_skill());
        fresh.skills_seeded = true;
        assert_eq!(fresh.skills.len(), 1);
        assert!(fresh.skills[0].enabled, "the starter skill is on by default");

        // A settings file saved before skills existed also decodes with `skills_seeded: false`
        // (the field didn't exist yet), so it gets the same one-time seed.
        let old: Settings = serde_json::from_str(r#"{ "recent": [] }"#).unwrap();
        assert!(!old.skills_seeded);

        // But once seeded (and persisted with the flag set), it never comes back after the user
        // deletes it — simulated here by round-tripping a settings value that already has the
        // flag set and an empty skills list.
        let deleted_it = Settings { skills: Vec::new(), skills_seeded: true, ..Settings::default() };
        let round: Settings = serde_json::from_str(&serde_json::to_string(&deleted_it).unwrap()).unwrap();
        assert!(round.skills.is_empty() && round.skills_seeded, "seeding is one-time, not re-applied on every load");
    }
}
