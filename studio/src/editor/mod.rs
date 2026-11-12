//! The editor screen: library, preview, inspector and timeline in resizable panes.

mod assistant_panel;
mod canvas_overlay;
pub mod export_dialog;
pub mod inspector;
pub mod library;
pub mod tools;
pub mod media_cache;
pub mod preview;
pub mod record_dialog;
pub mod skills_dialog;
pub mod timeline;

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, key};
use iced::widget::{button, column, container, mouse_area, pane_grid, responsive, row, space, stack, text, text_input};
use iced::{Alignment, Color, Element, Event, Fill, Length, Point, Size, Subscription, Task, event, mouse, window};
use iced_aw::ContextMenu;

use self::inspector::Edit;
use self::media_cache::{AssetMedia, MediaCache};
use self::timeline::{HEADER_W, Lanes, RULER_H, Ruler, View};
use crate::clip::{Rgba, TextLayer, Zoom};
use crate::doc::Doc;
use crate::edit::Edge;
use crate::media::{self, ProbeInfo};
use crate::model::{Id, MediaKind, Project, TrackKind};
use crate::render::frames::FrameServer;
use crate::render::scene;
use crate::settings::Layout;
use crate::ui::icons::{code, icon, tinted};
use crate::ui::tokens::*;
use crate::ui::widgets::{self, MenuIcon, MenuItem};
use crate::ui::style;

/// Gaps between the editor blocks, and around them.
const PANE_GAP: f32 = 6.0;
const GRID_PAD: iced::Padding = iced::Padding { top: PANE_GAP, right: PANE_GAP, bottom: PANE_GAP, left: PANE_GAP };
/// Smallest a pane can be dragged.
const PANE_MIN: f32 = 160.0;
/// Until the user sizes the timeline themselves it is as tall as its tracks need, between this
/// and `TIMELINE_AUTO_SHARE` of the window.
const TIMELINE_AUTO_MIN: f32 = 230.0;
const TIMELINE_AUTO_SHARE: f32 = 0.42;
/// The timeline's toolbar row and the padding around its lanes.
const TIMELINE_CHROME_H: f32 = 56.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Library,
    Preview,
    Inspector,
    Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryTab {
    Media,
    Text,
    Transitions,
    Animation,
    Sounds,
}

impl LibraryTab {
    const ALL: [LibraryTab; 5] = [LibraryTab::Media, LibraryTab::Text, LibraryTab::Transitions, LibraryTab::Animation, LibraryTab::Sounds];

    fn label(self) -> &'static str {
        match self {
            LibraryTab::Media => "Media",
            LibraryTab::Text => "Text",
            LibraryTab::Transitions => "Effects",
            LibraryTab::Animation => "Animation",
            LibraryTab::Sounds => "Sounds",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Inspector,
    Assistant,
}

#[derive(Debug, Clone, Copy)]
pub enum TrackAction {
    Rename,
    ToggleHidden,
    ToggleLocked,
    ToggleMuted,
    MoveUp,
    MoveDown,
    Delete,
    AddVideo,
    AddAudio,
}

/// Where a dragged effect was dropped.
#[derive(Debug, Clone, Copy)]
pub enum DropTarget {
    /// On the preview: the layer under the cursor, and the point (canvas fractions).
    Canvas { clip: Option<Id>, at: (f64, f64) },
    /// On the timeline: the clip under the cursor, the track and the time.
    Timeline { clip: Option<Id>, track: Option<Id>, t: f64 },
}

#[derive(Debug, Clone)]
pub enum Message {
    PaneResized(pane_grid::ResizeEvent),
    /// The window size changed (or is first known).
    WindowSize(Size),
    /// Preview zoom: a scale (1 = one canvas pixel per screen point), or None to fit.
    PreviewZoom(Option<f32>),
    /// Zoom the preview by a factor around a point of the stage.
    PreviewZoomAt(f32, Point),
    /// Move a zoomed preview by this many pixels.
    PreviewPan(iced::Vector),
    LibraryTab(LibraryTab),
    SideTab(SideTab),
    TogglePlay,
    ToggleMute,
    ToggleSafeZones,
    ToggleSnapping,
    Import,
    ImportPicked(Vec<PathBuf>),
    Probed(PathBuf, Result<ProbeInfo, String>),
    MediaLoaded(Id, AssetMedia),
    LibraryPress(Id),
    LibraryAdd(Id),
    LibraryRemove(Id),
    CursorMoved(Point),
    MouseReleased,
    /// Files are being dragged over the window (true) or left it (false).
    FileHover(bool),
    Seek(f64),
    SetView(View),
    ZoomBy(f64),
    ZoomFit,
    Select(Option<Id>, bool),
    MoveClip { clip: Id, track: Id, start: f64 },
    /// A clip dropped above the top track: onto a new track on top.
    MoveClipToNewTrack { clip: Id, start: f64 },
    Trim { clip: Id, edge: Edge, t: f64 },
    SplitAtPlayhead,
    /// The Range tool button (or R): turns range mode on, or finishes it when it's on.
    RangeTool,
    /// Range mode: a press on the timeline starts a new range (and selects the clip pressed on).
    RangePress { clip: Option<Id> },
    /// Done (✓): the range has served its purpose; it goes away and range mode ends.
    RangeDone,
    AddZoomAtPlayhead,
    /// A one-shot "button press" pulse on the selected clip, at the playhead.
    AddPressAtPlayhead,
    /// Staggers the selection's entrances a few frames apart, in selection order.
    StaggerSelection,
    ZoomTiming { clip: Id, zoom: Id, at: f64, duration: f64 },
    /// Scroll the track list by this many pixels (positive = down).
    ScrollTracks(f32),
    /// Volume for the selected range of a clip.
    RangeVolume(Id, f64),
    RemoveGain(Id, Id),
    /// Volume of one changed part of a clip: (clip, gain, volume).
    GainVolume(Id, Id, f64),
    /// Collapse or expand an inspector section.
    ToggleSection(&'static str),
    InspectorSearch(String),
    TrackResizeStart(Id),
    HexDraft(Id, &'static str, String),
    /// Point a library item at a new file (for moved or missing media).
    Relink(Id),
    Relinked(Id, Option<PathBuf>),
    /// Debug snapshots: select a clip and a range.
    DevSelect(Id, Option<(f64, f64)>),
    /// Debug snapshots: show a sample conversation (user text, reply Markdown).
    DevChat(String, String),
    /// Debug snapshots: select characters `start..end` in the inspector's text box.
    DevSelectText(usize, usize),
    /// Debug snapshots: type into the inspector's text box at the end of the text.
    DevType(String),
    /// Debug snapshots: show the right-click menu for the current selection in the middle of the window.
    DevMenu,
    /// Opens the Record camera dialog.
    OpenRecord,
    Record(record_dialog::Msg),
    DeleteSelection,
    DuplicateSelection,
    /// A key press no widget used: looked up in the keymap.
    Key(Key, keyboard::Modifiers),
    SelectAll,
    /// Select every clip that starts at or after the playhead.
    SelectForward,
    /// The clips inside a selection box dragged on the timeline.
    SelectBox(Vec<Id>),
    /// The timeline was right-clicked: the clip under the cursor (it gets selected), the track and the time.
    LaneContext { clip: Option<Id>, track: Option<Id>, t: f64 },
    Copy,
    Cut,
    Paste,
    /// Put the copied clip's effects (frame, color, animations, sound settings) on the selection.
    PasteEffects,
    Group,
    Ungroup,
    /// Delete and close the space: the marked range out of every track, or the selected clips.
    RippleDelete,
    /// Close gaps: on the selected clips' tracks, or (nothing selected) all time where nothing plays.
    CloseGaps,
    /// Close the one gap on this track at this time.
    CloseGapAt(Id, f64),
    /// The default transition at the cut before the selected clip (or the cut nearest the playhead).
    DefaultTransition,
    /// Hold the frame under the playhead (a still of it is inserted into the video).
    FreezeFrame,
    /// Turn the selected clips off (kept, but not shown or heard) or back on.
    ToggleDisabled,
    /// Swap a clip's media for another file, keeping its edits.
    ReplaceMedia(Id),
    ReplacePicked(Id, Option<PathBuf>),
    /// Play the selected clip backwards (a reversed copy of its footage is made in the background).
    ReverseClip,
    /// The reversed copy is ready (or failed): the clip, the footage it was made from (media, start, length) and the file.
    Reversed(Id, (Id, f64, f64), Result<(PathBuf, ProbeInfo), String>),
    /// Bring the selected pictures to the front (true) or send them to the back.
    Arrange(bool),
    /// Put the sound of the selected video clips on an audio track of its own.
    SeparateAudio,
    /// Mute or unmute the sound of the selected clips.
    ToggleClipMute,
    /// Copies the selected clip's color grade, to paste onto others later.
    CopyColor,
    /// Pastes the copied color grade onto every selected clip that has a picture.
    PasteColor,
    SetRange(Option<(f64, f64)>),
    DropAsset { asset: Id, track: Option<Id>, t: f64 },
    /// An effect card was pressed (a drag may follow), released without dragging, or hovered.
    EffectPress(library::Effect),
    EffectClick(library::Effect),
    EffectHover(Option<library::Effect>),
    /// An effect dropped on the preview or the timeline.
    DropEffect { effect: library::Effect, target: DropTarget },
    MarkerColor(Rgba),
    /// Star or unstar an effect in the Effects tab.
    ToggleFavoriteEffect(library::Effect),
    /// Open or close a group of the Effects tab.
    ToggleEffectGroup(&'static str),
    /// Add a timeline marker at the playhead (spanning the range, if one is marked).
    AddMarker,
    SelectMarker(Option<Id>),
    MoveMarker(Id, f64),
    MarkerName(Id, String),
    MarkerColorOf(Id, Rgba),
    DeleteMarker(Id),
    Edit(Id, Edit),
    Undo,
    Redo,
    Tick(Instant),
    AddText,
    AddShape(library::ShapePreset),
    AddCallout(library::CalloutPreset),
    AddCounter,
    ApplyTransition(crate::clip::TransitionKind),
    ApplyMotion { intro: bool, kind: crate::clip::MotionKind },
    AddSound(&'static str),
    SoundReady(&'static str, Result<(PathBuf, ProbeInfo), String>),
    PreviewSound(&'static str),
    SoundPreviewReady(&'static str, Result<(PathBuf, ProbeInfo), String>),
    SoundPreviewTick,
    ZoomEdit(Option<(Id, Id)>),
    /// Crop the clip on the canvas (None leaves crop mode).
    CropEdit(Option<Id>),
    SetCanvas(crate::model::Canvas),
    AiTool(crate::mcp::Call),
    ChatEdit(iced::widget::text_editor::Action),
    /// Typing or selecting in the inspector's text box of a text clip.
    TextDraft(Id, iced::widget::text_editor::Action),
    /// The transcript scrolled; true when it's at the bottom.
    ChatScrolled(bool),
    ChatSend,
    ChatSuggestion(String),
    ChatStop,
    ChatNew,
    /// Opens or closes the skills manager.
    OpenSkills,
    CloseSkills,
    /// Opens the add/edit form (an index to edit, or `None` to start a new skill).
    SkillEditStart(Option<usize>),
    SkillNameInput(String),
    SkillBodyEdit(iced::widget::text_editor::Action),
    SkillSave,
    SkillEditCancel,
    SkillDelete(usize),
    SkillToggle(usize, bool),
    /// A link in an assistant reply was clicked.
    OpenLink(String),
    /// Copy text (a code block) to the clipboard.
    CopyText(String),
    ChatPoll,
    /// Ask the app to open the Connect AI dialog.
    OpenAiConnect,
    ToolImported(crate::mcp::Call, Vec<(PathBuf, Result<ProbeInfo, String>)>, bool),
    ToolSound(crate::mcp::Call, &'static str, Result<(PathBuf, ProbeInfo), String>),
    /// `analyze_audio_beats` finished: timeline-second beat times, already mapped from the clip's source time.
    ToolBeats(crate::mcp::Call, Result<Vec<f64>, String>),
    Track(Id, TrackAction),
    RenameInput(String),
    RenameCommit,
    /// Start editing the project's name in the title bar.
    RenameProjectStart,
    RenameProjectInput(String),
    RenameProjectCommit,
    Step(i32),
    GoStart,
    GoEnd,
    Escape,
    DismissToast,
    OpenExport,
    CloseExport,
    ExportPath(String),
    ExportBrowse,
    ExportBrowsed(Option<PathBuf>),
    ExportQuality(crate::render::export::Quality),
    /// Export only the marked range (true) or the whole video.
    ExportOnlyRange(bool),
    ExportPreset(crate::render::export::Preset),
    ExportQueueAddPreset(crate::render::export::Preset),
    ExportQueueRemove(usize),
    StartExport,
    ExportProgress(crate::render::export::Progress),
    CancelExport,
    ExportRetry,
    RevealExport(usize),
    PlayExport(usize),
    Noop,
}

pub struct Editor {
    pub doc: Doc,
    pub path: Option<PathBuf>,
    panes: pane_grid::State<Pane>,
    library_tab: LibraryTab,
    pub side_tab: SideTab,
    /// The Range tool is on: dragging on the timeline selects a range (until Done).
    range_mode: bool,
    /// Track list scroll offset in pixels.
    vscroll: f32,
    playing: Option<(Instant, f64)>,
    snapping: bool,
    playhead: f64,
    view: View,
    selection: Vec<Id>,
    range: Option<(f64, f64)>,
    media: MediaCache,
    lanes_cache: timeline::Caches,
    importing: Vec<PathBuf>,
    library_selected: Option<Id>,
    /// Library item pressed (drag starts once the cursor moves).
    press: Option<(Id, Point)>,
    pub dnd: Option<Id>,
    /// Effect card pressed (drag starts once the cursor moves), and the effect being dragged.
    effect_press: Option<(library::Effect, Point)>,
    pub effect_dnd: Option<library::Effect>,
    effect_hover: Option<library::Effect>,
    /// Color for new markers.
    marker_color: Rgba,
    /// A clip's color grade, copied via its context menu, ready to paste onto another.
    color_clipboard: Option<crate::clip::Grade>,
    /// The Color preset previews for the selected clip: which asset they were made from (and
    /// whether its thumbnail had arrived), then one picture per look.
    preset_tiles: Option<((Option<Id>, bool), Vec<iced::widget::image::Handle>)>,
    /// Clips copied or cut, ready to paste.
    clipboard: Vec<crate::clip::Clip>,
    /// Where the timeline was last right-clicked (track, time), for "Close gap here".
    menu_at: Option<(Option<Id>, f64)>,
    /// Clips gliding to where an edit moved them, and the timeline view easing to a new zoom or scroll.
    clip_anim: Option<timeline::Anim>,
    view_anim: Option<(View, View, Instant)>,
    /// Clips whose reversed copy is being made.
    reversing: Vec<Id>,
    keymap: crate::shortcuts::Keymap,
    /// The timeline marker shown in the inspector.
    selected_marker: Option<Id>,
    /// Inspector sections the user collapsed (by title), and the inspector search text.
    pub collapsed: std::collections::BTreeSet<String>,
    /// Starred effects (by `Effect::key`) and the Effects-tab groups that are open.
    pub favorite_effects: Vec<String>,
    pub effects_open: std::collections::BTreeSet<String>,
    /// The user's AI assistant skills (see `settings::Skill`), persisted the same way as the
    /// fields above.
    pub skills: Vec<crate::settings::Skill>,
    skills_dialog: Option<skills_dialog::SkillsDialog>,
    inspector_query: String,
    pub cursor: Point,
    renaming: Option<(Id, String)>,
    /// Draft name while editing the project's title in the title bar.
    pub renaming_project: Option<String>,
    toast: Option<(String, bool, Instant)>,
    /// Debug snapshots: the right-click menu is drawn in the middle of the window.
    dev_menu: bool,
    /// Eases the Export, Skills and Record dialogs in.
    dialog: crate::ui::motion::Appear,
    /// The Record camera dialog, while it is open.
    record: Option<record_dialog::RecordDialog>,
    /// The camera and microphone last recorded with (kept in the settings file).
    pub record_choice: crate::settings::RecordChoice,
    /// Finished recordings being imported, each with the seconds to leave out at its start (see
    /// `record_dialog::Outcome::Recorded`): it goes on the timeline once it is in the library.
    recorded: Vec<(PathBuf, f64)>,
    timeline_size: Cell<Size>,
    frames: FrameServer,
    export: Option<export_dialog::ExportDialog>,
    audio: Option<crate::playback::AudioPlayer>,
    /// Mixing and spawning FFmpeg for a play/seek runs in the background so the picture starts
    /// at once; this is where its result lands, tagged with the `audio_request` it answers.
    audio_pending: Arc<Mutex<Option<(u64, Result<Option<crate::playback::Prepared>, String>)>>>,
    /// Bumped on every play/seek; a pending result tagged with an older id is stale and discarded.
    audio_request: u64,
    /// Fingerprint of the sound the running audio player was started with.
    audio_revision: u64,
    /// Why the last playback had no sound, already shown once (so seeking doesn't repeat it).
    audio_problem: Option<String>,
    muted: bool,
    /// The "preview is muted" reminder was shown, or the user toggled mute themselves.
    mute_hint_shown: bool,
    sound_preview: Option<(&'static str, crate::playback::AudioPlayer, Instant)>,
    zoom_edit: Option<(Id, Id)>,
    crop_edit: Option<Id>,
    file_hover: bool,
    /// Track being resized: (track, cursor y at start, height at start).
    resizing_track: Option<(Id, f32, f32)>,
    /// Library items whose file can't be found.
    missing: std::collections::HashSet<Id>,
    hex_draft: Option<inspector::HexDraft>,
    agent: crate::assistant::Agent,
    chat: Vec<assistant_panel::ChatItem>,
    chat_input: iced::widget::text_editor::Content,
    /// The selected text clip's text, as edited in the inspector (it keeps the cursor and selection).
    text_draft: Option<(Id, iced::widget::text_editor::Content)>,
    /// Keep the transcript pinned to the newest message (off once the user scrolls up).
    chat_follow: bool,
    /// Debug snapshots show a sample conversation without a connected AI.
    dev_chat: bool,
    /// Whether the timeline height follows the default (until the user drags it).
    timeline_auto: bool,
    /// The automatic timeline height last applied, and the window height it was for.
    timeline_fitted: Option<(f32, f32)>,
    window_size: Size,
    /// Preview zoom (None = fit) and how far the zoomed canvas is moved from the center.
    preview_zoom: Option<f32>,
    preview_pan: iced::Vector,
    /// Live 9:16/1:1 crop guides over a 16:9 canvas, for one-shot multi-aspect re-exports.
    show_safe_zones: bool,
    stage_size: Cell<Size>,
    /// Settings that shape the editor (preview quality, default lengths, …).
    prefs: crate::settings::Prefs,
    /// With Auto preview quality: the level in use (lowered while playback can't keep up).
    auto_level: crate::settings::PreviewQuality,
    lag_check: Instant,
}

impl Editor {
    pub fn new(project: Project, path: Option<PathBuf>, layout: Layout) -> Editor {
        use pane_grid::{Axis, Configuration as C};
        let config = C::Split {
            axis: Axis::Horizontal,
            ratio: layout.top_bottom,
            a: Box::new(C::Split {
                axis: Axis::Vertical,
                ratio: layout.library,
                a: Box::new(C::Pane(Pane::Library)),
                b: Box::new(C::Split {
                    axis: Axis::Vertical,
                    ratio: layout.preview_inspector,
                    a: Box::new(C::Pane(Pane::Preview)),
                    b: Box::new(C::Pane(Pane::Inspector)),
                }),
            }),
            b: Box::new(C::Pane(Pane::Timeline)),
        };
        Editor {
            doc: Doc::new(project),
            path,
            panes: pane_grid::State::with_configuration(config),
            library_tab: LibraryTab::Media,
            side_tab: SideTab::Inspector,
            range_mode: false,
            vscroll: 0.0,
            playing: None,
            snapping: true,
            playhead: 0.0,
            view: View::default(),
            selection: Vec::new(),
            range: None,
            media: MediaCache::default(),
            lanes_cache: timeline::Caches::default(),
            importing: Vec::new(),
            library_selected: None,
            press: None,
            dnd: None,
            effect_press: None,
            effect_dnd: None,
            effect_hover: None,
            marker_color: Rgba::hex(library::MARKER_COLORS[0]),
            color_clipboard: None,
            preset_tiles: None,
            clipboard: Vec::new(),
            menu_at: None,
            clip_anim: None,
            view_anim: None,
            reversing: Vec::new(),
            keymap: Default::default(),
            selected_marker: None,
            collapsed: Default::default(),
            favorite_effects: Vec::new(),
            effects_open: Default::default(),
            skills: Vec::new(),
            skills_dialog: None,
            inspector_query: String::new(),
            cursor: Point::ORIGIN,
            renaming: None,
            renaming_project: None,
            toast: None,
            dev_menu: false,
            dialog: Default::default(),
            record: None,
            record_choice: Default::default(),
            recorded: Vec::new(),
            timeline_size: Cell::new(Size::new(1000.0, 300.0)),
            frames: FrameServer::default(),
            export: None,
            audio: None,
            audio_pending: Arc::new(Mutex::new(None)),
            audio_request: 0,
            audio_revision: 0,
            audio_problem: None,
            muted: false,
            mute_hint_shown: false,
            sound_preview: None,
            zoom_edit: None,
            crop_edit: None,
            file_hover: false,
            resizing_track: None,
            missing: Default::default(),
            hex_draft: None,
            agent: crate::assistant::Agent::default(),
            chat: Vec::new(),
            chat_input: iced::widget::text_editor::Content::new(),
            text_draft: None,
            chat_follow: true,
            dev_chat: false,
            timeline_auto: layout.timeline_auto,
            timeline_fitted: None,
            window_size: Size::ZERO,
            preview_zoom: None,
            preview_pan: iced::Vector::ZERO,
            show_safe_zones: false,
            stage_size: Cell::new(Size::new(960.0, 540.0)),
            prefs: Default::default(),
            auto_level: crate::settings::PreviewQuality::Auto,
            lag_check: Instant::now(),
        }
    }

    /// Uses new settings. `opening`: the project was just opened, so session toggles
    /// (snapping, mute) start from the settings too.
    pub fn apply_prefs(&mut self, prefs: &crate::settings::Prefs, opening: bool) {
        let p = prefs.sane();
        if opening || p.snapping != self.prefs.snapping {
            self.snapping = p.snapping;
        }
        if opening || p.start_muted != self.prefs.start_muted {
            self.muted = p.start_muted;
            if let Some(a) = &self.audio {
                a.set_muted(self.muted);
            }
        }
        if p.preview_quality != self.prefs.preview_quality {
            self.auto_level = crate::settings::PreviewQuality::Auto;
        }
        self.keymap = p.keys.clone();
        self.prefs = p;
    }

    /// The key of an action as shown in tooltips and menus ("" when it has none).
    fn key_of(&self, action: crate::shortcuts::Action) -> String {
        self.keymap.key(action).unwrap_or_default()
    }

    /// The selected clips plus everything grouped with them: what moves, copies and deletes together.
    fn acting(&self) -> Vec<Id> {
        self.doc.project.with_groups(&self.selection)
    }

    /// The preview quality in use right now.
    fn preview_quality(&self) -> crate::settings::PreviewQuality {
        match self.prefs.preview_quality {
            crate::settings::PreviewQuality::Auto => self.auto_level,
            q => q,
        }
    }

    /// The split between the top panes and the timeline.
    fn top_split(&self) -> Option<pane_grid::Split> {
        match self.panes.layout() {
            pane_grid::Node::Split { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// The height the timeline takes by itself: enough for every track, within limits.
    fn timeline_auto_height(&self, grid_h: f32) -> f32 {
        let wanted = TIMELINE_CHROME_H + RULER_H + timeline::content_height(&self.doc.project);
        wanted.clamp(TIMELINE_AUTO_MIN, (grid_h * TIMELINE_AUTO_SHARE).max(TIMELINE_AUTO_MIN))
    }

    /// Sizes the timeline to its tracks, until the user sizes it themselves.
    fn fit_timeline(&mut self) {
        let grid_h = self.window_size.height - TITLE_BAR_H - GRID_PAD.top - GRID_PAD.bottom;
        if !self.timeline_auto || grid_h < TIMELINE_AUTO_MIN * 2.0 {
            return;
        }
        let height = self.timeline_auto_height(grid_h);
        if self.timeline_fitted == Some((height, grid_h)) {
            return;
        }
        if let Some(split) = self.top_split() {
            // The pane grid gives the bottom pane `height * (1 - ratio) - spacing / 2`.
            self.panes.resize(split, 1.0 - (height + PANE_GAP / 2.0) / grid_h);
            self.timeline_fitted = Some((height, grid_h));
        }
    }

    fn refresh_missing(&mut self) {
        self.missing = self.doc.project.assets.iter().filter(|a| !a.path.is_file()).map(|a| a.id).collect();
        self.lanes_cache.clear();
    }

    /// Stops playback, previews and any running export (before the editor is closed or replaced).
    pub fn shutdown(&mut self) {
        self.stop_playback();
        self.sound_preview = None;
        if let Some(export_dialog::ExportDialog { phase: export_dialog::Phase::Running { cancel, .. }, .. }) = &self.export {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.agent.stop();
    }

    /// Turns the assistant on or off when the user connects or disconnects an AI.
    pub fn set_ai(&mut self, provider: Option<crate::settings::AiProvider>) {
        self.agent.set_provider(provider);
    }

    pub fn project(&self) -> &Project {
        &self.doc.project
    }

    /// Starts loading thumbnails and waveforms for the project's media.
    pub fn boot(&mut self) -> Task<Message> {
        self.refresh_missing();
        if !self.missing.is_empty() {
            let n = self.missing.len();
            self.notify(if n == 1 { "1 media file can't be found — right-click it in the library to locate it".to_string() } else { format!("{n} media files can't be found — right-click them in the library to locate them") }, true);
        }
        let assets = self.doc.project.assets.clone();
        self.media.ensure(&assets, Message::MediaLoaded)
    }

    /// Current split ratios, for saving the layout.
    pub fn layout(&self) -> Layout {
        use pane_grid::Node;
        let mut l = Layout { timeline_auto: self.timeline_auto, ..Layout::default() };
        if let Node::Split { ratio, a, .. } = self.panes.layout() {
            l.top_bottom = *ratio;
            if let Node::Split { ratio, b, .. } = a.as_ref() {
                l.library = *ratio;
                if let Node::Split { ratio, .. } = b.as_ref() {
                    l.preview_inspector = *ratio;
                }
            }
        }
        l
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![event::listen_with(editor_event)];
        // Redraw every frame while playing, or while decoders are still catching up after a seek.
        let easing_in = self.dialog.running() || self.toast.as_ref().is_some_and(|(_, _, at)| at.elapsed() < crate::ui::motion::APPEAR);
        if self.playing.is_some() || self.frames.busy() || self.clip_anim.is_some() || self.view_anim.is_some() || easing_in {
            subs.push(window::frames().map(Message::Tick));
        }
        if self.toast.is_some() {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Message::DismissToast));
        }
        if self.agent.busy {
            subs.push(iced::time::every(Duration::from_millis(60)).map(|_| Message::ChatPoll));
        }
        if self.sound_preview.is_some() {
            subs.push(iced::time::every(Duration::from_millis(250)).map(|_| Message::SoundPreviewTick));
        }
        // The Record dialog's live picture, level meter and countdown.
        if self.record.is_some() {
            subs.push(iced::time::every(Duration::from_millis(33)).map(|_| Message::Record(record_dialog::Msg::Tick)));
        }
        Subscription::batch(subs)
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), error, Instant::now()));
    }

    /// Runs an edit; on failure shows the reason instead. Clips the edit moved (a ripple, a
    /// closed gap) glide to their new place.
    fn edit<T>(&mut self, desc: &str, f: impl FnOnce(&mut Project) -> anyhow::Result<T>) -> Option<T> {
        let before = self.clip_places();
        let result = self.doc.apply(desc, f);
        let out = self.after_edit(result);
        if out.is_some() {
            self.glide_from(before);
        }
        out
    }

    /// Where every clip is drawn right now: its start and the top of its row (mid-glide included).
    fn clip_places(&self) -> Vec<(Id, f64, f32)> {
        let p = &self.doc.project;
        let rows = timeline::rows(p, 0.0);
        let now = Instant::now();
        let gliding = self.clip_anim.as_ref().map(|a| (a, a.progress(now)));
        p.clips
            .iter()
            .filter_map(|c| {
                let top = rows.iter().find(|r| r.0 == c.track)?.2;
                let (start, y) = match gliding {
                    Some((anim, e)) => anim.shown(c.id, c.start, top, e),
                    None => (c.start, top),
                };
                Some((c.id, start, y))
            })
            .collect()
    }

    /// Starts gliding the clips that are no longer where `before` says they were drawn.
    fn glide_from(&mut self, before: Vec<(Id, f64, f32)>) {
        let p = &self.doc.project;
        let rows = timeline::rows(p, 0.0);
        let from: std::collections::HashMap<Id, (f64, f32)> = before
            .into_iter()
            .filter(|(id, start, y)| {
                p.clip(*id).is_some_and(|c| (c.start - start).abs() > 1e-6 || rows.iter().find(|r| r.0 == c.track).is_some_and(|r| (r.2 - y).abs() > 0.5))
            })
            .map(|(id, start, y)| (id, (start, y)))
            .collect();
        self.clip_anim = (!from.is_empty()).then(|| timeline::Anim { from, started: Instant::now() });
    }

    /// Clips the user put somewhere by hand (dragged, trimmed) are already where they belong: no glide.
    fn settle(&mut self, ids: &[Id]) {
        if let Some(anim) = &mut self.clip_anim {
            anim.from.retain(|id, _| !ids.contains(id));
            if anim.from.is_empty() {
                self.clip_anim = None;
            }
        }
    }

    /// Eases the timeline view to `to` (zoom buttons, fit, following the playhead).
    fn glide_view(&mut self, to: View) {
        self.view_anim = Some((self.view, to, Instant::now()));
    }

    /// The view the timeline is heading for (the current one when it is not moving).
    fn view_target(&self) -> View {
        self.view_anim.map(|(_, to, _)| to).unwrap_or(self.view)
    }

    /// Advances the animations of the timeline by one frame.
    fn tick_motion(&mut self, now: Instant) {
        const VIEW_SECONDS: f32 = 0.18;
        if let Some((from, to, started)) = self.view_anim {
            let k = (now.saturating_duration_since(started).as_secs_f32() / VIEW_SECONDS).clamp(0.0, 1.0);
            if k >= 1.0 {
                (self.view, self.view_anim) = (to, None);
            } else {
                // Interpolating the visible window (its left edge and its length) keeps whatever
                // the zoom is anchored on (the playhead, the cursor) still on screen.
                let e = (1.0 - (1.0 - k).powi(3)) as f64;
                let span = 1.0 / from.zoom + (1.0 / to.zoom - 1.0 / from.zoom) * e;
                self.view = View { scroll: from.scroll + (to.scroll - from.scroll) * e, zoom: 1.0 / span };
            }
            self.lanes_cache.clear();
        }
        if let Some(anim) = &self.clip_anim {
            if anim.done(now) {
                self.clip_anim = None;
            }
            self.lanes_cache.clear();
        }
    }

    fn edit_merge<T>(&mut self, desc: &str, key: Option<&str>, f: impl FnOnce(&mut Project) -> anyhow::Result<T>) -> Option<T> {
        let result = self.doc.apply_merge(desc, key, f);
        self.after_edit(result)
    }

    fn after_edit<T>(&mut self, result: anyhow::Result<T>) -> Option<T> {
        match result {
            Ok(v) => {
                self.lanes_cache.clear();
                Some(v)
            }
            Err(e) => {
                self.notify(e.to_string(), true);
                None
            }
        }
    }

    fn seek(&mut self, t: f64) {
        self.playhead = t.max(0.0);
        if self.playing.is_some() {
            self.start_playback();
        }
        self.follow_playhead();
    }

    /// (Re)starts playback from the playhead. The picture starts at once, on the wall clock;
    /// mixing and spawning FFmpeg for the sound happens in the background (it can take a
    /// noticeable moment) and plugs in via `audio_pending` once ready, same as sound that's
    /// simply slow to start (`AudioPlayer::stalled`/`join_at`).
    fn start_playback(&mut self) {
        self.sound_preview = None;
        self.playing = Some((Instant::now(), self.playhead));
        self.audio = None;
        self.audio_revision = audio_fingerprint(&self.doc.project);
        self.audio_request += 1;
        let id = self.audio_request;
        let project = self.doc.project.clone();
        let (from, muted) = (self.playhead, self.muted);
        let slot = self.audio_pending.clone();
        std::thread::spawn(move || {
            let result = crate::playback::AudioPlayer::prepare(&project, from, muted).map_err(|e| e.to_string());
            let mut g = slot.lock().unwrap();
            // A newer request may have already landed (finished out of order): never clobber it.
            if g.as_ref().is_none_or(|(existing, _)| id >= *existing) {
                *g = Some((id, result));
            }
        });
    }

    /// Applies a background `start_playback` result once ready, ignoring one a later
    /// play/pause/seek has already superseded (its `Prepared` cleans itself up when dropped).
    fn poll_audio(&mut self) {
        let Some((id, result)) = self.audio_pending.lock().unwrap().take() else { return };
        if id != self.audio_request {
            return;
        }
        match result {
            Ok(Some(prepared)) => match crate::playback::AudioPlayer::finish(prepared) {
                Ok(audio) => self.audio = Some(audio),
                Err(e) => self.report_audio_problem(e.to_string()),
            },
            Ok(None) => {}
            Err(e) => self.report_audio_problem(e),
        }
    }

    /// Says why playback has no sound, once while the same problem lasts.
    fn report_audio_problem(&mut self, why: String) {
        if self.audio_problem.as_deref() != Some(why.as_str()) {
            self.notify(format!("No sound: {why}"), true);
            self.audio_problem = Some(why);
        }
    }

    fn stop_playback(&mut self) {
        self.playing = None;
        self.audio = None;
        // Auto goes back to sharp once the picture stands still.
        self.auto_level = crate::settings::PreviewQuality::Auto;
        self.frames.take_lag();
    }

    /// Scrolls the timeline so the playhead stays visible.
    fn follow_playhead(&mut self) {
        let width = (self.timeline_size.get().width - HEADER_W).max(100.0) as f64;
        // Judged against where the view is heading, so a scroll already under way is not restarted.
        let view = self.view_target();
        let x = (self.playhead - view.scroll) * view.zoom;
        if x < 0.0 || x > width - 40.0 {
            self.glide_view(View { scroll: (self.playhead - width * 0.2 / view.zoom).max(0.0), ..view });
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_inner(message);
        self.sync_text_draft();
        self.sync_preset_tiles();
        self.fit_timeline();
        self.dialog.track(if self.export.is_some() { Some(1) } else if self.skills_dialog.is_some() { Some(2) } else if self.record.is_some() { Some(3) } else { None });
        task
    }

    /// Makes the Color preset previews for the selected clip when they are missing or were made
    /// from another picture. They are small, so this is quick; it runs only when the source changes.
    fn sync_preset_tiles(&mut self) {
        let [id] = self.selection.as_slice() else { return };
        let Some(clip) = self.doc.project.clip(*id) else { return };
        if !self.doc.project.clip_has_picture(clip) {
            return;
        }
        let asset = clip.asset();
        let cover = asset.and_then(|a| self.media.get(a)).and_then(|m| m.cover.as_ref());
        let key = (asset, cover.is_some());
        if self.preset_tiles.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.preset_tiles = Some((key, inspector::preset_tiles(cover)));
    }

    /// Keeps the inspector's text box in step with the selected text clip (undo, the AI and
    /// other edits change its text too), without disturbing the cursor while they match.
    fn sync_text_draft(&mut self) {
        let text = match self.selection.as_slice() {
            [id] => self.doc.project.clip(*id).and_then(|c| match &c.content {
                crate::clip::Content::Text(t) => Some((*id, t.text.as_str())),
                _ => None,
            }),
            _ => None,
        };
        match text {
            None => self.text_draft = None,
            Some((id, text)) => {
                if !matches!(&self.text_draft, Some((d, c)) if *d == id && c.text() == text) {
                    self.text_draft = Some((id, iced::widget::text_editor::Content::with_text(text)));
                }
            }
        }
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        if self.crop_edit.is_some_and(|c| self.selection != [c]) {
            self.crop_edit = None;
        }
        if (self.export.is_some() || self.record.is_some()) && blocked_by_dialog(&message) {
            return Task::none();
        }
        match message {
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                if self.top_split() == Some(split) {
                    self.timeline_auto = false;
                }
                self.panes.resize(split, ratio);
            }
            Message::PreviewZoom(z) => {
                self.preview_zoom = z.map(|z| z.clamp(PREVIEW_ZOOM_MIN, PREVIEW_ZOOM_MAX));
                self.preview_pan = iced::Vector::ZERO;
            }
            Message::PreviewZoomAt(factor, at) => self.zoom_preview_at(factor, at),
            Message::PreviewPan(d) => {
                if self.preview_zoom.is_some() {
                    let size = self.stage_size.get();
                    let (w, h, _) = self.canvas_placement(size);
                    self.preview_pan = clamp_pan(self.preview_pan + d, size, w, h);
                }
            }
            Message::WindowSize(size) => {
                self.window_size = size;
                self.fit_timeline();
            }
            Message::LibraryTab(t) => self.library_tab = t,
            Message::SideTab(t) => {
                self.side_tab = t;
                // The transcript is rebuilt when its tab opens: show where the conversation ended.
                if t == SideTab::Assistant {
                    self.chat_follow = true;
                    return iced::widget::operation::snap_to_end(assistant_panel::SCROLL_ID);
                }
            }
            Message::ToggleSnapping => self.snapping = !self.snapping,
            Message::TogglePlay => {
                if self.playing.is_some() {
                    self.stop_playback();
                } else {
                    if self.playhead >= self.doc.project.duration() - 0.01 {
                        self.playhead = 0.0;
                    }
                    self.start_playback();
                    // Muted with M or by "Start muted" in Settings: say so once, not on every play.
                    if self.muted && !self.mute_hint_shown {
                        self.mute_hint_shown = true;
                        self.notify("The preview is muted. Press M to hear it", false);
                    }
                }
            }
            Message::ToggleSafeZones => self.show_safe_zones = !self.show_safe_zones,
            Message::ToggleMute => {
                self.muted = !self.muted;
                self.mute_hint_shown = true;
                if let Some(a) = &self.audio {
                    a.set_muted(self.muted);
                }
                self.notify(if self.muted { "Sound off" } else { "Sound on" }, false);
            }
            Message::Tick(now) => {
                self.tick_motion(now);
                self.frames.settle();
                self.frames.collect_idle();
                if let Some((started, from)) = self.playing {
                    self.poll_audio();
                    // Auto quality: step down when more than a fifth of the frames came late.
                    if self.prefs.preview_quality == crate::settings::PreviewQuality::Auto && self.lag_check.elapsed() > Duration::from_secs(1) {
                        let (asked, late) = self.frames.take_lag();
                        if asked >= 10 && late * 5 > asked && self.auto_level != crate::settings::PreviewQuality::Draft {
                            self.auto_level = self.auto_level.lower();
                        }
                        self.lag_check = now;
                    }
                    // Edits during playback change the sound: restart the mix where we are.
                    // Only edits that change the sound restart the mix (moving a layer doesn't).
                    // Also when nothing was audible before: a sound added during playback starts playing.
                    if audio_fingerprint(&self.doc.project) != self.audio_revision {
                        self.start_playback();
                        return Task::none();
                    }
                    // A new default device takes over from one that went away.
                    if self.audio.as_ref().is_some_and(|a| a.device_lost()) {
                        self.report_audio_problem("the audio device was disconnected".into());
                        self.start_playback();
                        return Task::none();
                    }
                    // FFmpeg couldn't mix the sound: say why and keep playing the picture.
                    if let Some(why) = self.audio.as_ref().and_then(|a| a.failure()) {
                        let at = self.playhead;
                        self.audio = None;
                        self.playing = Some((now, at));
                        self.report_audio_problem(why);
                        return Task::none();
                    }
                    let t = match &self.audio {
                        Some(a) => match a.position() {
                            Some(p) => {
                                self.audio_problem = None;
                                p
                            }
                            // The sound is slow to start: the picture goes ahead on the wall
                            // clock and the sound joins in sync when it arrives.
                            None if a.stalled() => {
                                a.join_at(started);
                                from + now.duration_since(started).as_secs_f64()
                            }
                            // Wait for the first samples so picture and sound start together.
                            None => {
                                self.playing = Some((now, from));
                                from
                            }
                        },
                        None => from + now.duration_since(started).as_secs_f64(),
                    };
                    let end = self.doc.project.duration();
                    if t >= end {
                        self.playhead = end;
                        self.stop_playback();
                    } else {
                        self.playhead = t;
                    }
                    self.follow_playhead();
                }
            }
            Message::Import => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Import media")
                            .add_filter("Video, audio and images", media::MEDIA_EXTENSIONS)
                            .pick_files()
                            .await
                            .map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect())
                            .unwrap_or_default()
                    },
                    Message::ImportPicked,
                );
            }
            Message::FileHover(on) => self.file_hover = on,
            Message::ImportPicked(paths) => {
                self.file_hover = false;
                let skipped = paths.iter().filter(|p| !media::is_media_path(p)).count();
                if skipped > 0 {
                    self.notify(if skipped == 1 { "That file type can't be imported".to_string() } else { format!("{skipped} files couldn't be imported (unsupported type)") }, true);
                }
                let mut tasks = Vec::new();
                for path in paths.into_iter().filter(|p| media::is_media_path(p)) {
                    if self.importing.contains(&path) || self.doc.project.assets.iter().any(|a| a.path == path) {
                        continue;
                    }
                    self.importing.push(path.clone());
                    tasks.push(Task::perform(
                        async move {
                            let p = path.clone();
                            let r = tokio::task::spawn_blocking(move || media::probe(&p).map_err(|e| e.to_string())).await.unwrap_or_else(|e| Err(e.to_string()));
                            (path, r)
                        },
                        |(path, r)| Message::Probed(path, r),
                    ));
                }
                self.library_tab = LibraryTab::Media;
                return Task::batch(tasks);
            }
            Message::Probed(path, result) => {
                self.importing.retain(|p| p != &path);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match result {
                    Ok(info) => {
                        let recorded = self.recorded.iter().position(|(p, _)| *p == path).map(|i| self.recorded.remove(i).1);
                        if let Some(id) = self.edit(&format!("Import {name}"), |p| Ok(p.add_asset(&path, &info))) {
                            self.library_selected = Some(id);
                            self.refresh_missing();
                            let assets = self.doc.project.assets.clone();
                            let thumbnails = self.media.ensure(&assets, Message::MediaLoaded);
                            // A recording goes straight onto the timeline at the playhead.
                            if let Some(lead_in) = recorded {
                                let at = self.playhead;
                                if let Some(clip) = self.edit("Add recording", |p| p.add_recording(id, at, lead_in)) {
                                    self.selection = vec![clip];
                                    self.notify("Recording added to the timeline", false);
                                }
                            }
                            return thumbnails;
                        }
                    }
                    Err(e) => self.notify(format!("Couldn't import {name}: {e}"), true),
                }
            }
            Message::MediaLoaded(id, m) => {
                self.media.insert(id, m);
                self.lanes_cache.clear();
            }
            Message::LibraryPress(id) => {
                self.library_selected = Some(id);
                self.press = Some((id, self.cursor));
            }
            Message::LibraryAdd(id) => {
                self.press = None;
                let at = self.playhead;
                let still = self.prefs.still_seconds;
                if let Some(clip) = self.edit("Add to timeline", |p| p.add_media_clip_still(id, None, at, still)) {
                    self.selection = vec![clip];
                }
            }
            Message::LibraryRemove(id) => {
                if self.edit("Remove media", |p| p.remove_asset(id)).is_some() && self.library_selected == Some(id) {
                    self.library_selected = None;
                }
            }
            Message::CursorMoved(p) => {
                self.cursor = p;
                if let Some((id, y0, h0)) = self.resizing_track {
                    let h = (h0 + p.y - y0).clamp(timeline::MIN_TRACK_H, timeline::MAX_TRACK_H);
                    self.edit_merge("Resize track", Some(&format!("track-height:{id}")), |pr| {
                        pr.track_mut(id)?.height = Some(h.round());
                        Ok(())
                    });
                }
                if let Some((id, origin)) = self.press {
                    if self.dnd.is_none() && origin.distance(p) > 6.0 {
                        self.dnd = Some(id);
                    }
                }
                if let Some((effect, origin)) = self.effect_press {
                    if self.effect_dnd.is_none() && origin.distance(p) > 6.0 {
                        self.effect_dnd = Some(effect);
                    }
                }
            }
            Message::MouseReleased => {
                self.press = None;
                self.dnd = None;
                self.effect_press = None;
                self.effect_dnd = None;
                self.resizing_track = None;
            }
            Message::DropAsset { asset, track, t } => {
                self.press = None;
                self.dnd = None;
                // Fall back to the default track if the hovered one doesn't fit this media.
                let fits = |p: &Project| {
                    track.is_some_and(|tr| {
                        let (Some(tk), Some(a)) = (p.track(tr), p.asset(asset)) else { return false };
                        (tk.kind == TrackKind::Audio) == (a.kind == MediaKind::Audio) && !tk.locked
                    })
                };
                let still = self.prefs.still_seconds;
                if let Some(clip) = self.edit("Add to timeline", |p| {
                    let track = if fits(p) { track } else { None };
                    p.add_media_clip_still(asset, track, t, still)
                }) {
                    self.selection = vec![clip];
                }
            }
            Message::Seek(t) => {
                self.seek(t);
            }
            Message::SetView(v) => {
                self.view_anim = None;
                self.view = v;
                self.lanes_cache.clear();
            }
            Message::ZoomBy(f) => {
                // From where the view is heading, so quick presses add up; around the playhead.
                let target = self.view_target();
                let anchor = target.x(self.playhead).clamp(0.0, self.timeline_size.get().width - HEADER_W);
                self.glide_view(target.zoomed(f, anchor));
            }
            Message::ZoomFit => {
                let width = (self.timeline_size.get().width - HEADER_W - 40.0).max(200.0) as f64;
                let dur = self.doc.project.duration().max(10.0);
                self.glide_view(View { scroll: 0.0, zoom: (width / dur).clamp(2.0, 2400.0) });
            }
            Message::Select(id, additive) => {
                if id.is_some() {
                    self.selected_marker = None;
                }
                match id {
                    Some(id) if additive => {
                        if let Some(i) = self.selection.iter().position(|&s| s == id) {
                            self.selection.remove(i);
                        } else {
                            self.selection.push(id);
                        }
                    }
                    Some(id) => {
                        if !self.selection.contains(&id) {
                            self.selection = vec![id];
                        }
                    }
                    None => self.selection.clear(),
                }
                if !self.selection.is_empty() {
                    self.side_tab = SideTab::Inspector;
                }
                if self.zoom_edit.is_some_and(|(c, _)| !self.selection.contains(&c)) {
                    self.zoom_edit = None;
                }
                if self.crop_edit.is_some_and(|c| !self.selection.contains(&c)) {
                    self.crop_edit = None;
                }
                self.lanes_cache.clear();
            }
            Message::MoveClip { clip, track, start } => {
                // The other selected clips, and whatever is grouped with them, come along.
                let ids = self.moving_with(clip);
                self.edit(if ids.len() > 1 { "Move clips" } else { "Move clip" }, |p| p.move_clips(&ids, clip, track, start));
                self.settle(&ids);
            }
            Message::MoveClipToNewTrack { clip, start } => {
                let ids = self.moving_with(clip);
                self.edit("Move clip to a new track", |p| {
                    // Grouped clips follow in time; only the dragged one goes to the new track.
                    if ids.len() > 1 {
                        let own = p.clip(clip).map(|c| c.track).unwrap_or(0);
                        p.move_clips(&ids, clip, own, start)?;
                    }
                    let start = if ids.len() > 1 { p.clip(clip).map(|c| c.start).unwrap_or(start) } else { start };
                    p.move_to_new_top_track(clip, start)
                });
            }
            Message::Trim { clip, edge, t } => {
                // Linked clips whose edge sits at the same time are trimmed with it.
                let p = &self.doc.project;
                let at = |c: &crate::clip::Clip| if edge == Edge::Start { c.start } else { c.end() };
                let own = p.clip(clip).map(at);
                let ids: Vec<Id> = p.group_members(clip).into_iter().filter(|id| *id == clip || p.clip(*id).map(at).zip(own).is_some_and(|(a, b)| (a - b).abs() < 1e-6)).collect();
                self.edit("Trim clip", |p| ids.iter().try_for_each(|id| p.trim(*id, edge, t)));
                self.settle(&ids);
            }
            Message::SplitAtPlayhead => {
                let t = self.playhead;
                let p = &self.doc.project;
                let under = |id: &Id| p.clip(*id).is_some_and(|c| t > c.start + 0.1 && t < c.end() - 0.1);
                let targets: Vec<Id> = if self.selection.is_empty() {
                    p.clips.iter().filter(|c| !p.track(c.track).is_some_and(|t| t.locked)).map(|c| c.id).filter(under).collect()
                } else {
                    self.acting().into_iter().filter(under).collect()
                };
                if targets.is_empty() {
                    self.notify(if self.selection.is_empty() { "Move the playhead over a clip to split it" } else { "Move the playhead inside the clip to split it" }, false);
                } else {
                    self.edit("Split", |p| p.split_many(&targets, t));
                }
            }
            Message::Key(key, modifiers) => {
                if let Some(m) = crate::shortcuts::chord(&key, modifiers).and_then(|c| self.keymap.lookup(&c)).and_then(action_message) {
                    return self.update(m);
                }
            }
            Message::SelectAll => {
                let p = &self.doc.project;
                self.selection = p.clips.iter().filter(|c| !p.track(c.track).is_some_and(|t| t.locked)).map(|c| c.id).collect();
                self.after_select();
            }
            Message::SelectForward => {
                let (p, t) = (&self.doc.project, self.playhead);
                self.selection = p.clips.iter().filter(|c| c.start >= t - 1e-6 && !p.track(c.track).is_some_and(|t| t.locked)).map(|c| c.id).collect();
                if self.selection.is_empty() {
                    self.notify("Nothing starts after the playhead", false);
                }
                self.after_select();
            }
            Message::SelectBox(ids) => {
                self.selection = ids;
                self.after_select();
            }
            Message::LaneContext { clip, track, t } => {
                self.menu_at = Some((track, t));
                return self.update(Message::Select(clip, false));
            }
            Message::Copy | Message::Cut => {
                let ids = self.acting();
                if ids.is_empty() {
                    self.notify("Select a clip to copy", false);
                    return Task::none();
                }
                let cut = matches!(message, Message::Cut);
                self.clipboard = ids.iter().filter_map(|id| self.doc.project.clip(*id)).cloned().collect();
                let n = self.clipboard.len();
                if !cut {
                    self.notify(if n == 1 { "Copied — Ctrl+V pastes at the playhead".to_string() } else { format!("{n} clips copied — Ctrl+V pastes at the playhead") }, false);
                } else if self.edit(if n == 1 { "Cut clip" } else { "Cut clips" }, |p| p.delete_clips(&ids)).is_some() {
                    self.selection.clear();
                    self.after_history();
                }
            }
            Message::Paste => {
                if self.clipboard.is_empty() {
                    self.notify("Nothing to paste yet: copy a clip first (Ctrl+C)", false);
                    return Task::none();
                }
                let (clips, at) = (self.clipboard.clone(), self.playhead);
                if let Some(new) = self.edit(if clips.len() == 1 { "Paste clip" } else { "Paste clips" }, |p| p.paste_clips(&clips, at)) {
                    self.selection = new;
                    self.after_select();
                }
            }
            Message::PasteEffects => {
                let [from] = self.clipboard.as_slice() else {
                    self.notify("Copy one clip first; its effects can then be pasted onto others", false);
                    return Task::none();
                };
                let (from, ids) = (from.clone(), self.selection.clone());
                if ids.is_empty() {
                    self.notify("Select the clips to paste the effects onto", false);
                } else if self.edit("Paste effects", |p| p.paste_effects(&from, &ids)).is_some() {
                    self.notify("Effects pasted", false);
                }
            }
            Message::Group => {
                let ids = self.selection.clone();
                if self.edit("Group", |p| p.group_clips(&ids)).is_some() {
                    self.notify("Grouped — the clips now move, trim and delete together", false);
                }
            }
            Message::Ungroup => {
                let ids = self.selection.clone();
                if self.edit("Ungroup", |p| p.ungroup_clips(&ids)).is_some() {
                    self.notify("Ungrouped", false);
                }
            }
            Message::RippleDelete => {
                // A marked range is cut out of every track; otherwise the selected clips go.
                if let Some((a, b)) = self.range.filter(|_| self.range_mode) {
                    if self.edit("Cut out the range", |p| p.ripple_delete_range(a, b)).is_some() {
                        self.range = None;
                        self.range_mode = false;
                        self.after_history();
                        self.seek(a);
                        self.notify("Range removed — Ctrl+Z to undo", false);
                    }
                    return Task::none();
                }
                let ids = self.acting();
                if ids.is_empty() {
                    self.notify("Select a clip (or mark a range) to delete and close the space", false);
                } else if self.edit("Ripple delete", |p| p.ripple_delete(&ids)).is_some() {
                    self.selection.clear();
                    self.after_history();
                    self.notify("Deleted and closed the space — Ctrl+Z to undo", false);
                }
            }
            Message::CloseGaps => {
                let p = &self.doc.project;
                let mut tracks: Vec<Id> = self.acting().iter().filter_map(|id| p.clip(*id)).map(|c| c.track).collect();
                tracks.sort_unstable();
                tracks.dedup();
                let removed = if tracks.is_empty() {
                    self.edit("Close gaps", |p| p.close_empty_time())
                } else {
                    self.edit("Close gaps", |p| tracks.iter().map(|t| p.close_track_gaps(*t)).sum::<anyhow::Result<f64>>())
                };
                match removed {
                    Some(s) if s > 1e-6 => self.notify(format!("Closed {} of empty space — Ctrl+Z to undo", fmt_seconds(s)), false),
                    Some(_) => self.notify(if tracks.is_empty() { "There are no gaps to close" } else { "There are no gaps between the clips on this track" }, false),
                    None => {}
                }
            }
            Message::CloseGapAt(track, t) => {
                self.edit("Close gap", |p| p.close_gap_at(track, t));
            }
            Message::DefaultTransition => self.add_default_transition(),
            Message::FreezeFrame => self.freeze_frame_at_playhead(),
            Message::ToggleDisabled => {
                let ids = self.acting();
                let p = &self.doc.project;
                let disable = !ids.iter().all(|id| p.clip(*id).is_some_and(|c| c.disabled));
                if ids.is_empty() {
                    self.notify("Select a clip to disable it", false);
                } else if self.edit(if disable { "Disable clip" } else { "Enable clip" }, |p| p.set_disabled(&ids, disable)).is_some() {
                    self.notify(if disable { "Disabled — the clip stays here but isn't shown or heard" } else { "Enabled" }, false);
                }
            }
            Message::ReplaceMedia(clip) => {
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new().set_title("Replace media").add_filter("Video, audio and images", media::MEDIA_EXTENSIONS).pick_file().await.map(|f| f.path().to_path_buf())
                    },
                    move |p| Message::ReplacePicked(clip, p),
                );
            }
            Message::ReverseClip => {
                use crate::clip::{Content, Part};
                let p = &self.doc.project;
                let clip = match self.selection.as_slice() {
                    [id] => p.clip(*id).filter(|c| !c.freeze),
                    _ => None,
                };
                let job = clip.and_then(|c| {
                    let Content::Media { asset, src_in, speed } = c.content else { return None };
                    let a = p.asset(asset).filter(|a| a.kind != MediaKind::Image)?;
                    // Next to the project when it has a file, so the copy travels with it.
                    let out_dir = match self.path.as_ref().and_then(|f| Some(f.parent()?.join(format!("{} files", f.file_stem()?.to_string_lossy())))) {
                        Some(dir) => dir,
                        None => crate::settings::config_dir().join("reversed"),
                    };
                    let job = crate::reverse::Job {
                        path: a.path.clone(),
                        from: src_in,
                        length: c.duration * speed,
                        video: a.kind == MediaKind::Video && c.part != Part::Sound,
                        audio: a.has_audio && c.part != Part::Picture,
                        fps: a.fps,
                        size: (a.width, a.height),
                        out_dir,
                    };
                    Some((c.id, (asset, src_in, c.duration), job))
                });
                let Some((id, footage, job)) = job else {
                    self.notify("Select one video or audio clip to reverse", false);
                    return Task::none();
                };
                if self.reversing.contains(&id) {
                    return Task::none();
                }
                self.reversing.push(id);
                self.notify("Reversing the clip… long clips take a moment", false);
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let path = crate::reverse::reversed_file(&job).map_err(|e| e.to_string())?;
                            let info = media::probe(&path).map_err(|e| e.to_string())?;
                            Ok((path, info))
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    move |r| Message::Reversed(id, footage, r),
                );
            }
            Message::Reversed(id, footage, result) => {
                self.reversing.retain(|c| *c != id);
                match result {
                    Ok((path, info)) => {
                        // Trimmed or changed in the meantime: the copy no longer matches what the clip shows.
                        let same = self.doc.project.clip(id).is_some_and(|c| matches!(c.content, crate::clip::Content::Media { asset, src_in, .. } if (asset, src_in, c.duration) == footage));
                        if !same {
                            self.notify("The clip changed while it was being reversed. Reverse it again", true);
                            return Task::none();
                        }
                        let done = self.edit("Reverse clip", |p| {
                            let original = p.asset(footage.0).map(|a| a.name.clone()).unwrap_or_default();
                            let asset = p.add_asset(&path, &info);
                            if let Some(a) = p.assets.iter_mut().find(|a| a.id == asset) {
                                a.name = format!("{original} (reversed)");
                            }
                            p.use_reversed(id, asset)
                        });
                        if done.is_some() {
                            self.refresh_missing();
                            self.notify("Clip reversed — Ctrl+Z to undo", false);
                            let assets = self.doc.project.assets.clone();
                            return self.media.ensure(&assets, Message::MediaLoaded);
                        }
                    }
                    Err(e) => self.notify(e, true),
                }
            }
            Message::ReplacePicked(_, None) => {}
            Message::ReplacePicked(clip, Some(path)) => match media::probe(&path) {
                Ok(info) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let replaced = self.edit("Replace media", |p| {
                        let asset = p.add_asset(&path, &info);
                        p.replace_media(clip, asset)
                    });
                    if replaced.is_some() {
                        self.frames.forget_clip(clip);
                        self.refresh_missing();
                        self.notify(format!("Now showing {name}"), false);
                        let assets = self.doc.project.assets.clone();
                        return self.media.ensure(&assets, Message::MediaLoaded);
                    }
                }
                Err(e) => self.notify(format!("Couldn't use that file: {e}"), true),
            },
            // The Range tool: turn it on, drag on the timeline to select, adjust the edges, Done.
            Message::RangeTool => {
                if self.range_mode {
                    return self.update(Message::RangeDone);
                }
                self.range_mode = true;
                self.notify("Drag on the timeline to select a range", false);
                self.lanes_cache.clear();
            }
            Message::RangePress { clip } => {
                // A new range starts; it appears once the mouse moves.
                self.range_mode = true;
                self.range = None;
                if let Some(clip) = clip {
                    self.selection = vec![clip];
                    self.selected_marker = None;
                    self.zoom_edit = None;
                }
                self.lanes_cache.clear();
            }
            Message::RangeDone => {
                if self.range_mode || self.range.is_some() {
                    self.range_mode = false;
                    self.range = None;
                    self.lanes_cache.clear();
                }
            }
            Message::AddZoomAtPlayhead => self.add_zoom_at_playhead(),
            Message::AddPressAtPlayhead => self.add_press_at_playhead(),
            Message::StaggerSelection => self.stagger_selection(),
            Message::RangeVolume(clip, v) => {
                if let Some((a, b)) = self.range {
                    self.edit_merge("Range volume", Some(&format!("range-volume:{clip}")), |p| p.set_range_volume(clip, a, b, v));
                }
            }
            Message::DevChat(user, reply) => {
                self.chat.push(assistant_panel::ChatItem::User { text: user, context: None });
                self.chat.push(assistant_panel::ChatItem::Reply(assistant_panel::Reply::new(&reply)));
                self.dev_chat = true;
                self.side_tab = SideTab::Assistant;
                return iced::widget::operation::snap_to_end(assistant_panel::SCROLL_ID);
            }
            Message::DevSelect(clip, range) => {
                self.selection = vec![clip];
                match range {
                    Some((a, b)) => self.set_range(a, b),
                    None => self.range = None,
                }
                self.side_tab = SideTab::Inspector;
                self.lanes_cache.clear();
            }
            Message::DevType(typed) => {
                use iced::widget::text_editor::{Action, Edit as Typed, Motion};
                let Some(id) = self.text_draft.as_ref().map(|(id, _)| *id) else { return Task::none() };
                let _ = self.update(Message::TextDraft(id, Action::Move(Motion::DocumentEnd)));
                for c in typed.chars() {
                    let _ = self.update(Message::TextDraft(id, Action::Edit(Typed::Insert(c))));
                }
            }
            Message::DevMenu => self.dev_menu = true,
            Message::OpenRecord => {
                if self.record.is_none() && self.export.is_none() && self.skills_dialog.is_none() {
                    if self.playing.is_some() {
                        self.stop_playback();
                    }
                    let mut dialog = record_dialog::RecordDialog::new(crate::recorder::recordings_dir(self.path.as_deref()), self.record_choice.clone());
                    let ask = dialog.take_device_request();
                    self.record = Some(dialog);
                    if ask {
                        return record_devices();
                    }
                }
            }
            Message::Record(msg) => {
                let Some(dialog) = &mut self.record else { return Task::none() };
                let outcome = dialog.update(msg);
                self.record_choice = dialog.choice();
                match outcome {
                    Some(record_dialog::Outcome::Close) => self.record = None,
                    Some(record_dialog::Outcome::Recorded(file, problem, lead_in)) => {
                        self.record = None;
                        if let Some(problem) = problem {
                            self.notify(format!("{problem} What was recorded is kept."), true);
                        }
                        // Into the library like any import, then onto the timeline (see `Probed`).
                        self.recorded.push((file.clone(), lead_in));
                        self.importing.push(file.clone());
                        self.library_tab = LibraryTab::Media;
                        return Task::perform(
                            async move {
                                let p = file.clone();
                                let r = tokio::task::spawn_blocking(move || media::probe(&p).map_err(|e| e.to_string())).await.unwrap_or_else(|e| Err(e.to_string()));
                                (file, r)
                            },
                            |(path, r)| Message::Probed(path, r),
                        );
                    }
                    None => {
                        if dialog.take_device_request() {
                            return record_devices();
                        }
                    }
                }
            }
            Message::DevSelectText(a, b) => {
                use iced::widget::text_editor::{Action, Motion};
                if let Some((_, content)) = &mut self.text_draft {
                    content.perform(Action::Move(Motion::DocumentStart));
                    (0..a).for_each(|_| content.perform(Action::Move(Motion::Right)));
                    (a..b).for_each(|_| content.perform(Action::Select(Motion::Right)));
                }
            }
            Message::Relink(id) => {
                let name = self.doc.project.asset(id).map(|a| a.name.clone()).unwrap_or_default();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(format!("Locate {name}"))
                            .add_filter("Video, audio and images", media::MEDIA_EXTENSIONS)
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    move |p| Message::Relinked(id, p),
                );
            }
            Message::Relinked(_, None) => {}
            Message::Relinked(id, Some(path)) => {
                let probe = media::probe(&path);
                match probe {
                    Ok(info) => {
                        let kind = self.doc.project.asset(id).map(|a| a.kind);
                        if kind.is_some_and(|k| k != info.kind) {
                            self.notify("That file is a different kind of media (video, audio or image)", true);
                        } else {
                            self.edit("Relink media", |p| {
                                let a = p.assets.iter_mut().find(|a| a.id == id).ok_or_else(|| anyhow::anyhow!("That media is no longer in the library"))?;
                                a.path = path.clone();
                                (a.duration, a.width, a.height, a.fps, a.has_audio) = (info.duration, info.width, info.height, info.fps, info.has_audio);
                                Ok(())
                            });
                            self.media.forget(id);
                            self.frames.forget_asset(id);
                            self.refresh_missing();
                            let assets = self.doc.project.assets.clone();
                            return self.media.ensure(&assets, Message::MediaLoaded);
                        }
                    }
                    Err(e) => self.notify(format!("Couldn't use that file: {e}"), true),
                }
            }
            Message::HexDraft(clip, field, text) => {
                let parsed = Rgba::parse(&text);
                self.hex_draft = Some((clip, field, text));
                if let (Some(c), Some(current)) = (parsed, self.doc.project.clip(clip).and_then(|c| inspector::current_color(c, field))) {
                    if let Some(edit) = inspector::hex_edit(field, Rgba(c.0, c.1, c.2, current.3)) {
                        return self.update(Message::Edit(clip, edit));
                    }
                }
            }
            Message::TrackResizeStart(id) => {
                if let Some(t) = self.doc.project.track(id) {
                    self.resizing_track = Some((id, self.cursor.y, timeline::track_height(t)));
                }
            }
            Message::GainVolume(clip, gain, v) => {
                self.edit_merge("Range volume", Some(&format!("gain:{gain}")), |p| {
                    p.ensure_unlocked(clip)?;
                    let g = p.clip_mut(clip)?.audio.gains.iter_mut().find(|g| g.id == gain).ok_or_else(|| anyhow::anyhow!("That volume change no longer exists"))?;
                    g.volume = v.clamp(0.0, 4.0);
                    Ok(())
                });
            }
            Message::ToggleSection(title) => {
                if !self.collapsed.remove(title) {
                    self.collapsed.insert(title.to_string());
                }
            }
            Message::InspectorSearch(q) => self.inspector_query = q,
            Message::RemoveGain(clip, gain) => {
                self.edit("Remove range volume", |p| {
                    p.clip_mut(clip)?.audio.gains.retain(|g| g.id != gain);
                    Ok(())
                });
            }
            Message::ScrollTracks(dy) => {
                let visible = (self.timeline_size.get().height - RULER_H).max(1.0);
                let max = (timeline::content_height(&self.doc.project) - visible).max(0.0);
                self.vscroll = (self.vscroll + dy).clamp(0.0, max);
                self.lanes_cache.clear();
            }
            Message::ZoomTiming { clip, zoom, at, duration } => {
                self.edit("Move zoom", |p| p.set_zoom_timing(clip, zoom, at, duration));
            }
            Message::DeleteSelection => {
                if let Some((clip, zoom)) = self.zoom_edit.take() {
                    self.edit("Remove zoom", |p| {
                        p.clip_mut(clip)?.zooms.retain(|z| z.id != zoom);
                        Ok(())
                    });
                    return Task::none();
                }
                if self.selection.is_empty() {
                    if let Some(m) = self.selected_marker {
                        return self.update(Message::DeleteMarker(m));
                    }
                }
                let ids = self.acting();
                self.selection.clear();
                if !ids.is_empty() {
                    let count = ids.len();
                    if self.edit(if count == 1 { "Delete clip" } else { "Delete clips" }, |p| p.delete_clips(&ids)).is_some() {
                        self.notify(if count == 1 { "Clip deleted — Ctrl+Z to undo" } else { "Clips deleted — Ctrl+Z to undo" }, false);
                    }
                }
            }
            Message::DuplicateSelection => {
                let ids = self.acting();
                if ids.is_empty() {
                    self.notify("Select a clip to duplicate", false);
                } else if let Some(new) = self.edit("Duplicate", |p| p.duplicate_clips(&ids)) {
                    self.selection = new;
                    self.after_select();
                }
            }
            Message::Arrange(front) => {
                // Only pictures have a stacking order; sound in the selection is left alone.
                let p = &self.doc.project;
                let ids: Vec<Id> = self.selection.iter().copied().filter(|&id| p.clip(id).is_some_and(|c| p.clip_has_picture(c))).collect();
                if !ids.is_empty() {
                    self.edit(if front { "Bring to front" } else { "Send to back" }, |p| ids.iter().try_for_each(|&id| p.arrange(id, front)));
                }
            }
            Message::EffectPress(effect) => self.effect_press = Some((effect, self.cursor)),
            Message::EffectHover(h) => self.effect_hover = h,
            Message::EffectClick(effect) => {
                // A press and release on the same card, without a drag in between, is a click.
                if self.effect_dnd.is_none() && self.effect_press.is_some_and(|(e, _)| e == effect) {
                    self.effect_press = None;
                    return self.apply_effect(effect, None);
                }
            }
            Message::DropEffect { effect, target } => {
                (self.effect_press, self.effect_dnd) = (None, None);
                return self.apply_effect(effect, Some(target));
            }
            Message::ToggleFavoriteEffect(effect) => {
                let key = effect.key();
                match self.favorite_effects.iter().position(|k| *k == key) {
                    Some(i) => {
                        self.favorite_effects.remove(i);
                    }
                    None => self.favorite_effects.push(key),
                }
            }
            Message::ToggleEffectGroup(title) => {
                if !self.effects_open.remove(title) {
                    self.effects_open.insert(title.to_string());
                }
            }
            Message::MarkerColor(c) => {
                self.marker_color = c;
                // Recolor a selected marker, too.
                if let [id] = self.selection.as_slice() {
                    if self.doc.project.clip(*id).is_some_and(|c| matches!(c.content, crate::clip::Content::Shape(_))) {
                        return self.update(Message::Edit(*id, Edit::ShapeFill(Rgba(c.0, c.1, c.2, self.doc.project.clip(*id).and_then(|c| inspector::current_color(c, "shape1")).map(|f| f.3).unwrap_or(1.0)))));
                    }
                }
            }
            Message::AddMarker => {
                // Over a marked range the marker spans it; otherwise it marks the playhead.
                let (at, duration) = match self.range {
                    Some((a, b)) => (a, b - a),
                    None => (self.playhead, 0.0),
                };
                let color = self.marker_color;
                if let Some(id) = self.edit("Add marker", |p| Ok(p.add_marker(at, duration, color))) {
                    self.selected_marker = Some(id);
                    self.selection.clear();
                    self.side_tab = SideTab::Inspector;
                }
            }
            Message::SelectMarker(id) => {
                self.selected_marker = id;
                if id.is_some() {
                    self.selection.clear();
                    self.side_tab = SideTab::Inspector;
                    self.lanes_cache.clear();
                }
            }
            Message::MoveMarker(id, at) => {
                self.edit_merge("Move marker", Some(&format!("marker-move:{id}")), |p| p.move_marker(id, at));
            }
            Message::MarkerName(id, name) => {
                self.edit_merge("Rename marker", Some(&format!("marker-name:{id}")), |p| {
                    p.marker_mut(id)?.name = name;
                    Ok(())
                });
            }
            Message::MarkerColorOf(id, color) => {
                self.marker_color = color;
                self.edit("Marker color", |p| {
                    p.marker_mut(id)?.color = color;
                    Ok(())
                });
            }
            Message::DeleteMarker(id) => {
                if self.edit("Delete marker", |p| p.delete_marker(id)).is_some() {
                    self.selected_marker = None;
                }
            }
            Message::SeparateAudio => {
                let p = &self.doc.project;
                let ids: Vec<Id> = self.selection.iter().copied().filter(|&id| p.clip(id).is_some_and(|c| c.part == crate::clip::Part::Whole && p.clip_has_sound(c) && p.clip_has_picture(c))).collect();
                if ids.is_empty() {
                    self.notify("Select a video clip with sound to separate its audio", false);
                } else if let Some(sounds) = self.edit("Separate audio", |p| ids.iter().map(|&id| p.separate_audio(id)).collect::<anyhow::Result<Vec<_>>>()) {
                    self.notify(if sounds.len() == 1 { "Audio separated — it's on its own track now" } else { "Audio separated" }, false);
                    self.selection = sounds;
                }
            }
            Message::ToggleClipMute => {
                let p = &self.doc.project;
                let ids: Vec<Id> = self.selection.iter().copied().filter(|&id| p.clip(id).is_some_and(|c| p.clip_has_sound(c))).collect();
                let mute = !ids.iter().all(|&id| p.clip(id).is_some_and(|c| c.audio.muted));
                if !ids.is_empty() {
                    self.edit(if mute { "Mute clip" } else { "Unmute clip" }, |p| {
                        for id in &ids {
                            p.ensure_unlocked(*id)?;
                            p.clip_mut(*id)?.audio.muted = mute;
                        }
                        Ok(())
                    });
                }
            }
            Message::CopyColor => {
                let p = &self.doc.project;
                if let [id] = self.selection[..] {
                    if let Some(c) = p.clip(id).filter(|c| p.clip_has_picture(c)) {
                        self.color_clipboard = Some(c.grade);
                        self.notify("Color copied", false);
                    }
                }
            }
            Message::PasteColor => {
                let Some(grade) = self.color_clipboard else { return Task::none() };
                let p = &self.doc.project;
                let ids: Vec<Id> = self.selection.iter().copied().filter(|&id| p.clip(id).is_some_and(|c| p.clip_has_picture(c))).collect();
                if ids.is_empty() {
                    self.notify("Select a clip to paste the color onto", false);
                } else {
                    self.edit("Paste color", |p| ids.iter().try_for_each(|&id| p.set_grade(id, grade)));
                }
            }
            Message::SetRange(r) => {
                self.range = r;
                // A range drawn with Shift on the ruler turns the Range tool on too, so Done shows.
                if r.is_some() {
                    self.range_mode = true;
                }
                self.lanes_cache.clear();
            }
            Message::Edit(id, edit) => {
                let (key, desc) = edit.describe();
                let key = format!("{key}:{id}");
                self.edit_merge(desc, Some(&key), |p| {
                    p.ensure_unlocked(id)?;
                    if let Edit::Speed(s) = edit {
                        return p.set_speed(id, s);
                    }
                    if let Edit::Transition(k) = edit {
                        let duration = p.clip(id).and_then(|c| c.transition).map(|t| t.duration).unwrap_or(0.6);
                        return p.set_transition(id, k, duration);
                    }
                    if let Edit::TransitionDuration(d) = edit {
                        let kind = p.clip(id).and_then(|c| c.transition).map(|t| t.kind);
                        return p.set_transition(id, kind, d);
                    }
                    edit.apply(p.clip_mut(id)?);
                    Ok(())
                });
            }
            Message::Undo | Message::Redo => {
                // Clips that go back to where they were glide there.
                let before = self.clip_places();
                let undo = matches!(message, Message::Undo);
                match if undo { self.doc.undo() } else { self.doc.redo() } {
                    Some(d) => {
                        self.notify(format!("{}: {d}", if undo { "Undid" } else { "Redid" }), false);
                        self.after_history();
                        self.glide_from(before);
                    }
                    None => self.notify(if undo { "Nothing to undo" } else { "Nothing to redo" }, false),
                }
            }
            Message::AddText => {
                let at = self.playhead;
                let still = self.prefs.still_seconds;
                if let Some(id) = self.edit("Add text", |p| p.add_text_clip(None, at, still, TextLayer::default())) {
                    self.selection = vec![id];
                    self.side_tab = SideTab::Inspector;
                }
            }
            Message::AddShape(preset) => {
                let at = self.playhead;
                let still = self.prefs.still_seconds;
                let (shape, radius) = preset.build();
                if let Some(id) = self.edit("Add shape", |p| {
                    let id = p.add_shape_clip(None, at, still, shape)?;
                    p.clip_mut(id)?.look.corner_radius = radius;
                    Ok(id)
                }) {
                    self.selection = vec![id];
                    self.side_tab = SideTab::Inspector;
                }
            }
            Message::AddCallout(preset) => {
                let at = self.playhead;
                if let Some(id) = self.edit("Add callout", |p| p.add_callout_clip(None, at, 1.5, preset.build())) {
                    self.selection = vec![id];
                    self.side_tab = SideTab::Inspector;
                }
            }
            Message::AddCounter => {
                let at = self.playhead;
                let still = self.prefs.still_seconds;
                if let Some(id) = self.edit("Add counter", |p| p.add_counter_clip(None, at, still, crate::clip::CounterLayer::new(0.0, 100.0))) {
                    self.selection = vec![id];
                    self.side_tab = SideTab::Inspector;
                }
            }
            Message::ApplyTransition(kind) => {
                let ids = self.selection.clone();
                self.edit("Add transition", |p| {
                    for id in &ids {
                        let duration = p.clip(*id).and_then(|c| c.transition).map(|t| t.duration).unwrap_or(0.6);
                        p.set_transition(*id, Some(kind), duration)?;
                    }
                    Ok(())
                });
            }
            Message::ApplyMotion { intro, kind } => {
                let ids = self.selection.clone();
                self.edit(if intro { "Animate in" } else { "Animate out" }, |p| {
                    for id in &ids {
                        let c = p.clip_mut(*id)?;
                        let default_duration = if intro { 0.6 } else { c.intro.map(|m| m.duration * 0.6).unwrap_or(0.6) };
                        let slot = if intro { &mut c.intro } else { &mut c.outro };
                        let (duration, ease, delay) = (slot.map(|m| m.duration).unwrap_or(default_duration).min(c.duration / 2.0), slot.and_then(|m| m.ease), slot.map(|m| m.delay).unwrap_or(0.0));
                        *slot = Some(crate::clip::Motion { kind, duration, ease, delay });
                    }
                    Ok(())
                });
            }
            Message::AddSound(id) => return sound_task(id, Message::SoundReady),
            Message::SoundReady(_, Err(e)) | Message::SoundPreviewReady(_, Err(e)) => self.notify(e, true),
            Message::SoundReady(id, Ok((path, info))) => {
                let at = self.playhead;
                let name = crate::sfx::SOUNDS.iter().find(|s| s.id == id).map(|s| s.name).unwrap_or("Sound");
                if let Some(clip) = self.edit(&format!("Add {name}"), |p| {
                    let asset = p.add_asset(&path, &info);
                    if let Some(a) = p.assets.iter_mut().find(|a| a.id == asset) {
                        a.name = name.to_string();
                    }
                    p.add_media_clip(asset, None, at)
                }) {
                    self.selection = vec![clip];
                    let assets = self.doc.project.assets.clone();
                    return self.media.ensure(&assets, Message::MediaLoaded);
                }
            }
            Message::PreviewSound(id) => {
                if self.sound_preview.as_ref().is_some_and(|(s, _, _)| *s == id) {
                    self.sound_preview = None;
                } else {
                    return sound_task(id, Message::SoundPreviewReady);
                }
            }
            Message::SoundPreviewReady(id, Ok((path, info))) => {
                let mut p = Project::new("preview", self.doc.project.canvas);
                let asset = p.add_asset(&path, &info);
                if p.add_media_clip(asset, None, 0.0).is_ok() {
                    self.sound_preview = crate::playback::AudioPlayer::start(&p, 0.0, false).ok().flatten().map(|a| (id, a, Instant::now() + Duration::from_secs_f64(info.duration + 0.3)));
                }
            }
            Message::SoundPreviewTick => {
                if self.sound_preview.as_ref().is_some_and(|(_, _, until)| Instant::now() > *until) {
                    self.sound_preview = None;
                }
            }
            Message::AiTool(call) => {
                // A reply the user stopped (or one from an earlier process) may still have calls on the way.
                if call.run.as_deref().is_some_and(|run| !self.agent.accepts(run)) {
                    call.answer(Err("The user stopped this request. Don't make any more changes.".into()));
                    return Task::none();
                }
                return self.handle_tool(call);
            }
            Message::SetCanvas(c) => {
                if c != self.doc.project.canvas {
                    self.edit("Change format", |p| {
                        p.canvas = c;
                        Ok(())
                    });
                }
            }
            Message::ChatEdit(action) => self.chat_input.perform(action),
            Message::TextDraft(id, action) => {
                let Some((draft_id, content)) = &mut self.text_draft else { return Task::none() };
                if *draft_id != id {
                    return Task::none();
                }
                // Selecting only moves the cursor; typing changes the clip (one undo step per burst).
                let typed = action.is_edit();
                content.perform(action);
                if typed {
                    let text = content.text();
                    return self.update(Message::Edit(id, inspector::Edit::Text(text)));
                }
            }
            Message::ChatScrolled(at_bottom) => self.chat_follow = at_bottom,
            Message::ChatSuggestion(s) => {
                if self.agent.busy {
                    return Task::none();
                }
                self.chat_input = iced::widget::text_editor::Content::with_text(&s);
                return self.update(Message::ChatSend);
            }
            Message::ChatSend => {
                let text = self.chat_input.text().trim().to_string();
                if text.is_empty() || self.agent.busy {
                    return Task::none();
                }
                let context = self.context_line();
                let label = self.range.map(|(a, b)| format!("Range {} – {}", fmt_ruler(a), fmt_ruler(b)));
                let cwd = self.path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_else(crate::settings::home_dir);
                match self.agent.send(&text, &context, &cwd, &self.skills_text()) {
                    Ok(()) => {
                        self.chat.push(assistant_panel::ChatItem::User { text, context: label });
                        self.chat_input = iced::widget::text_editor::Content::new();
                        self.chat_follow = true;
                    }
                    Err(e) => self.chat.push(assistant_panel::ChatItem::Notice(e.to_string())),
                }
                return iced::widget::operation::snap_to_end(assistant_panel::SCROLL_ID);
            }
            Message::ChatStop => {
                if self.agent.stop() {
                    self.end_chat_turn(assistant_panel::TurnEnd::Stopped);
                    if self.chat_follow {
                        return iced::widget::operation::snap_to_end(assistant_panel::SCROLL_ID);
                    }
                }
            }
            Message::OpenLink(url) => crate::platform::open_url(&url),
            Message::CopyText(text) => {
                self.notify("Copied", false);
                return iced::clipboard::write(text);
            }
            Message::ChatNew => {
                self.agent.reset();
                self.chat.clear();
            }
            Message::OpenSkills => self.skills_dialog = Some(skills_dialog::SkillsDialog { editing: None }),
            Message::CloseSkills => self.skills_dialog = None,
            Message::SkillEditStart(index) => {
                let (name, body) = match index.and_then(|i| self.skills.get(i)) {
                    Some(s) => (s.name.clone(), s.body.clone()),
                    None => (String::new(), String::new()),
                };
                if let Some(d) = &mut self.skills_dialog {
                    d.editing = Some(skills_dialog::Editing { index, name, body: iced::widget::text_editor::Content::with_text(&body) });
                }
                return iced::widget::operation::focus(skills_dialog::NAME_ID);
            }
            Message::SkillNameInput(s) => {
                if let Some(d) = self.skills_dialog.as_mut().and_then(|d| d.editing.as_mut()) {
                    d.name = s;
                }
            }
            Message::SkillBodyEdit(action) => {
                if let Some(d) = self.skills_dialog.as_mut().and_then(|d| d.editing.as_mut()) {
                    d.body.perform(action);
                }
            }
            Message::SkillSave => {
                if let Some(editing) = self.skills_dialog.as_mut().and_then(|d| d.editing.take()) {
                    let name = editing.name.trim().to_string();
                    let body = editing.body.text().trim().to_string();
                    if !name.is_empty() && !body.is_empty() {
                        let skill = crate::settings::Skill { name, body, enabled: true };
                        match editing.index {
                            Some(i) if i < self.skills.len() => self.skills[i] = crate::settings::Skill { enabled: self.skills[i].enabled, ..skill },
                            _ => self.skills.push(skill),
                        }
                    }
                }
            }
            Message::SkillEditCancel => {
                if let Some(d) = &mut self.skills_dialog {
                    d.editing = None;
                }
            }
            Message::SkillDelete(i) => {
                if i < self.skills.len() {
                    self.skills.remove(i);
                }
            }
            Message::SkillToggle(i, on) => {
                if let Some(s) = self.skills.get_mut(i) {
                    s.enabled = on;
                }
            }
            Message::OpenAiConnect => {}
            Message::ChatPoll => {
                use assistant_panel::{ChatItem, ToolState};
                let events = self.agent.poll();
                if events.is_empty() {
                    return Task::none();
                }
                for ev in events {
                    match ev {
                        crate::assistant::Event::TextDelta(t) => match self.chat.last_mut() {
                            Some(ChatItem::Reply(r)) => r.push_str(&t),
                            _ => self.chat.push(ChatItem::Reply(assistant_panel::Reply::new(&t))),
                        },
                        crate::assistant::Event::Text(t) => self.chat.push(ChatItem::Reply(assistant_panel::Reply::new(&t))),
                        crate::assistant::Event::ToolStart { id, name, input } => {
                            self.chat.push(ChatItem::Tool { id, label: crate::assistant::describe_tool(&name, &input), state: ToolState::Running, detail: String::new(), image: None })
                        }
                        crate::assistant::Event::ToolDone { id, ok, summary, image } => {
                            if let Some(ChatItem::Tool { state, detail, image: img, .. }) = self.chat.iter_mut().rev().find(|i| matches!(i, ChatItem::Tool { id: tid, .. } if *tid == id)) {
                                *state = if ok { ToolState::Done } else { ToolState::Failed };
                                *detail = summary;
                                *img = image.map(iced::widget::image::Handle::from_bytes);
                            }
                        }
                        crate::assistant::Event::TurnDone { error } => self.end_chat_turn(match error {
                            None => assistant_panel::TurnEnd::Finished,
                            Some(e) => assistant_panel::TurnEnd::Failed(e),
                        }),
                    }
                }
                if self.chat_follow {
                    return iced::widget::operation::snap_to_end(assistant_panel::SCROLL_ID);
                }
            }
            Message::ToolImported(call, results, add) => return self.finish_tool_import(call, results, add),
            Message::ToolSound(call, sound, r) => return self.finish_tool_sound(call, sound, r),
            Message::ToolBeats(call, r) => {
                call.answer(r.map(|beats| {
                    let times = serde_json::to_string(&beats).unwrap_or_default();
                    crate::mcp::text(if beats.is_empty() {
                        "No clear beats found (silence, or music/narration without sharp hits). Pace the edit by eye instead.".to_string()
                    } else {
                        format!("Beats, timeline seconds: {times}")
                    })
                }));
            }
            Message::CropEdit(target) => {
                self.crop_edit = target;
                if let Some(clip) = target {
                    self.zoom_edit = None;
                    self.focus_clip(clip);
                }
                self.lanes_cache.clear();
            }
            Message::ZoomEdit(target) => {
                self.zoom_edit = target;
                if target.is_some() {
                    self.crop_edit = None;
                }
                if let Some((clip, zoom)) = target {
                    self.stop_playback();
                    self.selection = vec![clip];
                    self.side_tab = SideTab::Inspector;
                    let c = self.doc.project.clip(clip).cloned();
                    if let Some((c, z)) = c.as_ref().and_then(|c| c.zooms.iter().find(|z| z.id == zoom).map(|z| (c, *z))) {
                        // Show a moment inside the zoom so its clip is on screen.
                        let (a, b) = (c.start + z.at, c.start + z.at + z.duration);
                        if self.playhead < a || self.playhead > b {
                            self.seek(a + z.duration / 2.0);
                        }
                    }
                    self.lanes_cache.clear();
                }
            }
            Message::Track(id, action) => return self.track_action(id, action),
            Message::RenameInput(s) => {
                if let Some((_, name)) = &mut self.renaming {
                    *name = s;
                }
            }
            Message::RenameCommit => {
                if let Some((id, name)) = self.renaming.take() {
                    self.edit("Rename track", |p| p.rename_track(id, &name));
                }
            }
            Message::RenameProjectStart => {
                self.renaming_project = Some(self.doc.project.name.clone());
                return iced::widget::operation::focus("project-rename");
            }
            Message::RenameProjectInput(s) => self.renaming_project = Some(s),
            Message::RenameProjectCommit => {
                if let Some(name) = self.renaming_project.take() {
                    self.edit("Rename project", |p| p.rename(&name));
                }
            }
            Message::Step(frames) => {
                let fps = self.doc.project.canvas.fps as f64;
                let t = ((self.playhead * fps).round() + frames as f64) / fps;
                self.seek(t);
            }
            Message::GoStart => self.seek(0.0),
            Message::GoEnd => {
                let end = self.doc.project.duration();
                self.seek(end);
            }
            Message::Escape => {
                if let Some(d) = &self.record {
                    // Not while something is being recorded: that takes Stop or Discard.
                    if !d.busy() {
                        self.record = None;
                    }
                    return Task::none();
                }
                if self.export.as_ref().is_some_and(|e| !e.is_running()) {
                    self.export = None;
                    return Task::none();
                }
                if self.renaming.take().is_some() {
                    return Task::none();
                }
                if self.renaming_project.take().is_some() {
                    return Task::none();
                }
                if self.zoom_edit.take().is_some() {
                    self.lanes_cache.clear();
                    return Task::none();
                }
                if self.crop_edit.take().is_some() {
                    self.lanes_cache.clear();
                    return Task::none();
                }
                if self.range_mode {
                    return self.update(Message::RangeDone);
                }
                if self.renaming.take().is_none() {
                    self.range = None;
                    self.selected_marker = None;
                    self.selection.clear();
                    self.lanes_cache.clear();
                }
            }
            Message::DismissToast => {
                if self.toast.as_ref().is_some_and(|(_, err, at)| at.elapsed() > Duration::from_secs(if *err { 5 } else { 2 })) {
                    self.toast = None;
                }
            }
            Message::OpenExport => {
                if self.doc.project.duration() <= 0.0 {
                    self.notify("Add something to the timeline before exporting", false);
                } else if self.export.as_ref().is_none_or(|e| !e.is_running()) {
                    self.stop_playback();
                    self.export = Some(export_dialog::ExportDialog::new(&self.doc.project, self.path.as_deref(), self.range));
                    if let Some(d) = &mut self.export {
                        d.quality = self.prefs.export_quality;
                    }
                }
            }
            Message::CloseExport => {
                if self.export.as_ref().is_some_and(|e| !e.is_running()) {
                    self.export = None;
                }
            }
            Message::ExportPath(p) => {
                if let Some(e) = &mut self.export {
                    e.path = p;
                }
            }
            Message::ExportBrowse => {
                let Some(e) = &self.export else { return Task::none() };
                let current = PathBuf::from(&e.path);
                let dir = current.parent().map(|d| d.to_path_buf()).filter(|d| d.is_dir()).unwrap_or_else(crate::settings::home_dir);
                let name = current.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "video.mp4".into());
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new().set_title("Export video").set_directory(dir).set_file_name(name).add_filter("MP4 video", &["mp4"]).save_file().await.map(|h| h.path().to_path_buf())
                    },
                    Message::ExportBrowsed,
                );
            }
            Message::ExportBrowsed(Some(p)) => {
                if let Some(e) = &mut self.export {
                    e.path = p.with_extension("mp4").display().to_string();
                }
            }
            Message::ExportBrowsed(None) => {}
            Message::ExportQuality(q) => {
                if let Some(e) = &mut self.export {
                    e.quality = q;
                }
            }
            Message::ExportOnlyRange(only) => {
                if let Some(e) = &mut self.export {
                    e.only_range = only;
                }
            }
            Message::ExportPreset(p) => {
                if let Some(e) = &mut self.export {
                    e.preset = p;
                }
            }
            Message::ExportQueueAddPreset(p) => {
                if let Some(e) = &mut self.export {
                    let path = e.suggest_path(p);
                    e.queue.push(export_dialog::QueuedJob { preset: p, path });
                }
            }
            Message::ExportQueueRemove(i) => {
                if let Some(e) = &mut self.export {
                    if i < e.queue.len() {
                        e.queue.remove(i);
                    }
                }
            }
            Message::StartExport => {
                if let Some(e) = &mut self.export {
                    e.completed.clear();
                    e.total_seconds = 0.0;
                }
                return self.start_export();
            }
            Message::ExportProgress(p) => {
                use crate::render::export::Progress;
                let mut continue_batch = false;
                if let Some(e) = &mut self.export {
                    match p {
                        Progress::Rendering { done, total, eta } => {
                            if let export_dialog::Phase::Running { done: d, total: t, eta: e2, .. } = &mut e.phase {
                                (*d, *t, *e2) = (done, total, eta);
                            }
                        }
                        Progress::Finished { path, seconds } => {
                            e.completed.push((path, e.preset));
                            e.total_seconds += seconds;
                            if e.queue.is_empty() {
                                let items = std::mem::take(&mut e.completed);
                                e.phase = export_dialog::Phase::Done { items, seconds: e.total_seconds };
                            } else {
                                let next = e.queue.remove(0);
                                e.path = next.path.display().to_string();
                                e.preset = next.preset;
                                continue_batch = true;
                            }
                        }
                        Progress::Failed(err) => e.phase = export_dialog::Phase::Failed(err),
                        Progress::Cancelled => e.phase = export_dialog::Phase::Setup,
                    }
                }
                if continue_batch {
                    return self.start_export();
                }
            }
            Message::CancelExport => {
                if let Some(export_dialog::ExportDialog { phase: export_dialog::Phase::Running { cancel, .. }, .. }) = &self.export {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Message::ExportRetry => {
                if let Some(e) = &mut self.export {
                    e.phase = export_dialog::Phase::Setup;
                }
            }
            Message::RevealExport(i) | Message::PlayExport(i) => {
                if let Some(export_dialog::ExportDialog { phase: export_dialog::Phase::Done { items, .. }, .. }) = &self.export {
                    if let Some((path, _)) = items.get(i) {
                        if matches!(message, Message::RevealExport(_)) {
                            crate::platform::reveal(path);
                        } else {
                            crate::platform::open(path);
                        }
                    }
                }
            }
            Message::Noop => {}
        }
        Task::none()
    }

    fn start_export(&mut self) -> Task<Message> {
        use crate::render::export::{self, Progress};
        use iced::futures::SinkExt;
        let Some(dialog) = &mut self.export else { return Task::none() };
        let mut path = PathBuf::from(dialog.path.trim());
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("mp4")) {
            path.set_extension("mp4");
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job = dialog.completed.len() + 1;
        let job_total = dialog.completed.len() + dialog.queue.len() + 1;
        dialog.phase = export_dialog::Phase::Running { done: 0, total: 1, eta: None, job, job_total, cancel: cancel.clone() };
        let project = self.doc.project.clone();
        let settings = dialog.settings();
        let stream = iced::stream::channel(64, async move |mut tx: iced::futures::channel::mpsc::Sender<Progress>| {
            let (stx, srx) = std::sync::mpsc::channel::<Progress>();
            std::thread::Builder::new()
                .name("export".into())
                .spawn(move || {
                    let started = std::time::Instant::now();
                    let progress = |p: Progress| {
                        let _ = stx.send(p);
                    };
                    let result = export::export(&project, &path, settings, &cancel, &progress);
                    let last = match result {
                        Ok(path) => Progress::Finished { path, seconds: started.elapsed().as_secs_f64() },
                        Err(e) if e.to_string() == "cancelled" => Progress::Cancelled,
                        Err(e) => Progress::Failed(e.to_string()),
                    };
                    let _ = stx.send(last);
                })
                .ok();
            loop {
                match srx.try_recv() {
                    Ok(p) => {
                        let end = !matches!(p, Progress::Rendering { .. });
                        let _ = tx.send(p).await;
                        if end {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => tokio::time::sleep(Duration::from_millis(40)).await,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // The export thread ended without a result (it crashed): say so instead of hanging.
                        let _ = tx.send(Progress::Failed("The export stopped unexpectedly. Please try again.".into())).await;
                        break;
                    }
                }
            }
        });
        Task::run(stream, Message::ExportProgress)
    }

    /// Preview scale that fits the whole canvas in a stage of `size`.
    fn fit_scale(&self, size: Size) -> f32 {
        let (cw, ch) = self.doc.project.canvas.size();
        ((size.width - PREVIEW_ROOM * 2.0) / cw as f32).min((size.height - PREVIEW_ROOM * 2.0) / ch as f32).max(0.01)
    }

    /// The canvas size on screen and its top-left corner inside a stage of `size`.
    fn canvas_placement(&self, size: Size) -> (f32, f32, iced::Vector) {
        let (cw, ch) = self.doc.project.canvas.size();
        let k = self.preview_zoom.unwrap_or_else(|| self.fit_scale(size));
        let (w, h) = ((cw as f32 * k).floor(), (ch as f32 * k).floor());
        let pan = clamp_pan(self.preview_pan, size, w, h);
        (w, h, iced::Vector::new(((size.width - w) / 2.0 + pan.x).floor(), ((size.height - h) / 2.0 + pan.y).floor()))
    }

    /// Current preview zoom as a percentage (the fit scale when fitting).
    pub fn preview_percent(&self) -> f32 {
        self.preview_zoom.unwrap_or_else(|| self.fit_scale(self.stage_size.get())) * 100.0
    }

    pub fn preview_fit(&self) -> bool {
        self.preview_zoom.is_none()
    }

    /// Zooms the preview by `factor`, keeping the canvas point under `at` (stage coordinates) in place.
    fn zoom_preview_at(&mut self, factor: f32, at: Point) {
        let size = self.stage_size.get();
        let (w, h, origin) = self.canvas_placement(size);
        let old = w / self.doc.project.canvas.size().0 as f32;
        let new = (old * factor).clamp(PREVIEW_ZOOM_MIN, PREVIEW_ZOOM_MAX);
        let (cw, ch) = self.doc.project.canvas.size();
        let (nw, nh) = (cw as f32 * new, ch as f32 * new);
        // Canvas point under the cursor stays under the cursor.
        let (px, py) = ((at.x - origin.x) / w.max(1.0), (at.y - origin.y) / h.max(1.0));
        let (ox, oy) = (at.x - px * nw, at.y - py * nh);
        self.preview_zoom = Some(new);
        self.preview_pan = clamp_pan(iced::Vector::new(ox - (size.width - nw) / 2.0, oy - (size.height - nh) / 2.0), size, nw, nh);
    }

    /// Right-click menu for the timeline and the preview: actions for the selected clips,
    /// or general ones when nothing is selected.
    fn context_menu(&self) -> Element<'_, Message> {
        use crate::clip::{Content, Part};
        use crate::shortcuts::Action as A;
        let p = &self.doc.project;
        let clips: Vec<&crate::clip::Clip> = self.selection.iter().filter_map(|id| p.clip(*id)).collect();
        let t = self.playhead;
        let key = |a: A| self.key_of(a);
        let mut items = Vec::new();
        let can_paste = !self.clipboard.is_empty();
        if clips.is_empty() {
            if can_paste {
                items.push(MenuItem::new(code::CLIPBOARD_PASTE, "Paste at playhead", Message::Paste).shortcut(key(A::Paste)));
            }
            if !p.clips.is_empty() {
                items.push(MenuItem::new(code::SQUARE_DASHED, "Select all clips", Message::SelectAll).shortcut(key(A::SelectAll)));
                items.push(MenuItem::new(code::ARROW_RIGHT, "Select everything after the playhead", Message::SelectForward).shortcut(key(A::SelectForward)));
                // Right-clicked in the empty space between (or before) clips: that one gap can go.
                if let Some((Some(track), at)) = self.menu_at {
                    let gap = !p.clips_on(track).any(|c| at > c.start && at < c.end()) && p.clips_on(track).any(|c| c.start >= at) && !p.track(track).is_some_and(|t| t.locked);
                    if gap {
                        items.push(MenuItem::new(code::FOLD_HORIZONTAL, "Close this gap", Message::CloseGapAt(track, at)));
                    }
                }
                items.push(MenuItem::new(code::CHEVRONS_RIGHT_LEFT, "Close all gaps", Message::CloseGaps).shortcut(key(A::CloseGaps)));
                items.push(MenuItem::Separator);
            }
            items.push(MenuItem::new(code::TYPE, "Add text at playhead", Message::AddText).shortcut(key(A::AddText)));
            items.push(MenuItem::new(code::FLAG, if self.range.is_some() { "Add marker over range" } else { "Add marker at playhead" }, Message::AddMarker).shortcut(key(A::AddMarker)));
            if self.range_mode {
                if self.range.is_some() {
                    items.push(MenuItem::new(code::SCISSORS, "Cut out the marked range", Message::RippleDelete).shortcut(key(A::RippleDelete)));
                }
                items.push(MenuItem::new(code::CHECK, "Done with the range", Message::RangeDone).shortcut(key(A::RangeDone)));
            } else {
                items.push(MenuItem::new(code::SQUARE_DASHED, "Select a range", Message::RangeTool).shortcut(key(A::RangeTool)));
            }
            return widgets::menu(items, 290.0);
        }
        let acting = self.acting();
        let single = match clips.as_slice() {
            [c] => Some(*c),
            _ => None,
        };
        // The everyday five, as one compact row.
        let paste = MenuIcon::new(code::CLIPBOARD_PASTE, "Paste", Message::Paste);
        items.push(MenuItem::Icons(vec![
            MenuIcon::new(code::SCISSORS, "Cut", Message::Cut),
            MenuIcon::new(code::CLIPBOARD_COPY, "Copy", Message::Copy),
            if can_paste { paste } else { paste.disabled() },
            MenuIcon::new(code::COPY, "Duplicate", Message::DuplicateSelection),
            MenuIcon::new(code::TRASH_2, "Delete", Message::DeleteSelection).danger(),
        ]));
        items.push(MenuItem::Separator);

        // At the playhead.
        let under_playhead = acting.iter().filter_map(|id| p.clip(*id)).any(|c| t > c.start + 0.1 && t < c.end() - 0.1);
        let split = MenuItem::new(code::SCISSORS, "Split at playhead", Message::SplitAtPlayhead).shortcut(key(A::Split));
        items.push(if under_playhead { split } else { split.disabled() });
        if let Some(c) = single {
            let media_picture = matches!(c.content, Content::Media { .. }) && p.clip_has_picture(c);
            let is_video = media_picture && c.asset().and_then(|a| p.asset(a)).is_some_and(|a| a.kind == MediaKind::Video);
            let inside = t >= c.start && t < c.end();
            if media_picture {
                let zoom = MenuItem::new(code::ZOOM_IN, "Add zoom at playhead", Message::AddZoomAtPlayhead).shortcut(key(A::AddZoom));
                items.push(if inside { zoom } else { zoom.disabled() });
                if is_video && !c.freeze {
                    let freeze = MenuItem::new(code::SNOWFLAKE, "Freeze frame at playhead", Message::FreezeFrame).shortcut(key(A::FreezeFrame));
                    items.push(if t >= c.start && t <= c.end() { freeze } else { freeze.disabled() });
                }
            }
            if p.clip_has_picture(c) {
                let press = MenuItem::new(code::POINTER, "Add press pulse at playhead", Message::AddPressAtPlayhead).shortcut(key(A::AddPress));
                items.push(if inside { press } else { press.disabled() });
            }
            let cut_before = c.transition.is_none() && p.prev_on_track(c).is_some_and(|pv| (pv.end() - c.start).abs() < 1e-6);
            let cut_after = p.clips_on(c.track).any(|n| n.transition.is_none() && (n.start - c.end()).abs() < 1e-6);
            if cut_before || cut_after {
                items.push(MenuItem::new(code::BLEND, if cut_before { "Add transition from the previous clip" } else { "Add transition to the next clip" }, Message::DefaultTransition).shortcut(key(A::DefaultTransition)));
            }

            // The clip's own media.
            if matches!(c.content, Content::Media { .. }) && (media_picture || !c.freeze) {
                items.push(MenuItem::Separator);
            }
            if media_picture {
                items.push(MenuItem::new(code::CROP, "Crop…", Message::CropEdit(Some(c.id))));
            }
            if matches!(c.content, Content::Media { .. }) && !c.freeze {
                items.push(MenuItem::new(code::REPLACE, "Replace media…", Message::ReplaceMedia(c.id)));
                if c.asset().and_then(|a| p.asset(a)).is_some_and(|a| a.kind != MediaKind::Image) {
                    let reverse = MenuItem::new(code::REWIND, if self.reversing.contains(&c.id) { "Reversing…" } else { "Reverse" }, Message::ReverseClip);
                    items.push(if self.reversing.contains(&c.id) { reverse.disabled() } else { reverse });
                }
            }
        }

        // Sound.
        let separable = clips.iter().any(|c| c.part == Part::Whole && p.clip_has_sound(c) && p.clip_has_picture(c));
        let sounding: Vec<_> = clips.iter().filter(|c| p.clip_has_sound(c)).collect();
        if separable || !sounding.is_empty() {
            items.push(MenuItem::Separator);
        }
        if separable {
            items.push(MenuItem::new(code::UNLINK, "Separate audio", Message::SeparateAudio));
        }
        if !sounding.is_empty() {
            let all_muted = sounding.iter().all(|c| c.audio.muted);
            items.push(MenuItem::new(if all_muted { code::VOLUME_2 } else { code::VOLUME_X }, if all_muted { "Unmute audio" } else { "Mute audio" }, Message::ToggleClipMute));
        }
        // Volume of just the marked range (the rest of the clip stays as it is).
        if let ([c], Some((a, b))) = (sounding.as_slice(), self.range) {
            if a.max(c.start) + 0.05 < b.min(c.end()) {
                items.push(MenuItem::new(code::VOLUME_X, "Mute the marked range", Message::RangeVolume(c.id, 0.0)));
                items.push(MenuItem::new(code::VOLUME_1, "Lower the marked range to 30%", Message::RangeVolume(c.id, 0.3)));
            }
        }

        // Several clips together. Grouping is offered when the selection isn't one group
        // already; ungrouping when any of it is grouped.
        let one_group = clips[0].group.is_some() && clips.iter().all(|c| c.group == clips[0].group);
        let grouped = clips.iter().any(|c| c.group.is_some());
        if clips.len() > 1 || grouped {
            items.push(MenuItem::Separator);
        }
        if clips.len() > 1 && !one_group {
            items.push(MenuItem::new(code::GROUP, "Group", Message::Group).shortcut(key(A::Group)));
        }
        if grouped {
            items.push(MenuItem::new(code::UNGROUP, "Ungroup", Message::Ungroup).shortcut(key(A::Ungroup)));
        }
        if clips.len() > 1 {
            items.push(MenuItem::new(code::SPARKLES, "Stagger entrances", Message::StaggerSelection));
        }

        // Order and look.
        let pictures = clips.iter().any(|c| p.clip_has_picture(c));
        let paste_effects = can_paste && self.clipboard.len() == 1;
        if single.is_some_and(|c| p.clip_has_picture(c)) || (pictures && self.color_clipboard.is_some()) || paste_effects {
            items.push(MenuItem::Separator);
        }
        if single.is_some_and(|c| p.clip_has_picture(c)) {
            items.push(MenuItem::new(code::CHEVRON_UP, "Bring to front", Message::Arrange(true)).shortcut(key(A::BringFront)));
            items.push(MenuItem::new(code::CHEVRON_DOWN, "Send to back", Message::Arrange(false)).shortcut(key(A::SendBack)));
            items.push(MenuItem::new(code::DROPLET, "Copy color", Message::CopyColor));
        }
        if pictures && self.color_clipboard.is_some() {
            items.push(MenuItem::new(code::PALETTE, "Paste color", Message::PasteColor));
        }
        if paste_effects {
            items.push(MenuItem::new(code::WAND_SPARKLES, "Paste effects", Message::PasteEffects).shortcut(key(A::PasteEffects)));
        }

        items.push(MenuItem::Separator);
        let all_off = clips.iter().all(|c| c.disabled);
        items.push(MenuItem::new(if all_off { code::POWER } else { code::BAN }, if all_off { "Enable" } else { "Disable" }, Message::ToggleDisabled).shortcut(key(A::ToggleDisabled)));
        items.push(MenuItem::new(code::DELETE, "Ripple delete", Message::RippleDelete).shortcut(key(A::RippleDelete)));
        widgets::menu(items, 290.0)
    }

    /// Applies a library effect: dropped on a target, or (clicked) to the selection.
    fn apply_effect(&mut self, effect: library::Effect, target: Option<DropTarget>) -> Task<Message> {
        use library::Effect;
        let dropped_on = match target {
            Some(DropTarget::Canvas { clip, .. } | DropTarget::Timeline { clip, .. }) => clip,
            None => None,
        };
        // Markers and blurs are new clips, placed in time and on the picture: dropped on the
        // timeline they start there; otherwise they cover the marked range (or 3 s from the
        // playhead), centered where they were dropped on the preview.
        if matches!(effect, Effect::Highlight(_) | Effect::Blur | Effect::Mask) {
            let (start, duration) = match (target, self.range) {
                (Some(DropTarget::Timeline { t, .. }), _) => (t, 3.0),
                (_, Some((a, b))) if b - a >= 0.1 => (a, b - a),
                _ => (self.playhead, 3.0),
            };
            let (track, at) = match target {
                Some(DropTarget::Timeline { track, .. }) => (track, None),
                Some(DropTarget::Canvas { at, .. }) => (None, Some(at)),
                None => (None, None),
            };
            // Only use the hovered track if it's a free video track there; otherwise pick one.
            let p = &self.doc.project;
            let track = track.filter(|tr| p.track(*tr).is_some_and(|t| t.kind == TrackKind::Visual && !t.locked) && (p.nearest_free(*tr, start, duration, &[]) - start).abs() < 1e-6);
            let marker = self.marker_color;
            let created = self.edit(&format!("Add {}", effect.label().to_lowercase()), |p| {
                let id = match effect {
                    Effect::Highlight(h) => {
                        let (shape, radius, name) = h.build(marker);
                        let id = p.add_shape_clip(track, start, duration, shape)?;
                        let c = p.clip_mut(id)?;
                        c.look.corner_radius = radius;
                        c.name = Some(name.to_string());
                        id
                    }
                    Effect::Mask => p.add_mask_clip(track, start, duration, crate::clip::MaskLayer::default())?,
                    _ => p.add_blur_clip(track, start, duration, crate::clip::BlurLayer::default())?,
                };
                if let Some((x, y)) = at {
                    let c = p.clip_mut(id)?;
                    (c.transform.x, c.transform.y) = (x, y);
                }
                Ok(id)
            });
            if created.is_some() && effect == Effect::Blur {
                self.notify("Blur added — drag its box over what to hide, and trim it on the timeline", false);
            }
            if created.is_some() && effect == Effect::Mask {
                self.notify("Mask added — drag its box over what to keep, and trim it on the timeline", false);
            }
            if let Some(id) = created {
                self.selection = vec![id];
                self.side_tab = SideTab::Inspector;
            }
            return Task::none();
        }
        let targets: Vec<Id> = match (target, dropped_on) {
            (Some(_), Some(clip)) => vec![clip],
            (Some(_), None) => {
                let msg = if matches!(effect, Effect::Transition(_)) { "Drop it where two clips meet" } else { "Drop it on a clip" };
                self.notify(msg, false);
                return Task::none();
            }
            (None, _) => self.selection.clone(),
        };
        if targets.is_empty() {
            let msg = if matches!(effect, Effect::Transition(_)) { "Select the second of two touching clips first, or drag the effect onto where they meet" } else { "Select a clip first, or drag the effect onto one" };
            self.notify(msg, false);
            return Task::none();
        }
        if target.is_some() {
            self.selection = targets.clone();
            self.lanes_cache.clear();
        }
        match effect {
            Effect::Transition(kind) => {
                let task = self.update(Message::ApplyTransition(kind));
                if target.is_some() && self.doc.project.clip(targets[0]).is_some_and(|c| c.transition.is_some()) {
                    self.notify("Transition added — drag its handle (at the cut) to resize it", false);
                }
                task
            }
            Effect::Intro(kind) => self.update(Message::ApplyMotion { intro: true, kind }),
            Effect::Outro(kind) => self.update(Message::ApplyMotion { intro: false, kind }),
            Effect::Frame(style) => {
                self.apply_frame_style(&targets, style);
                Task::none()
            }
            Effect::Zoom => {
                // Dropped on the timeline: the zoom starts there. On the preview: at the playhead,
                // aimed at the drop point.
                let [clip] = targets[..] else {
                    self.notify("Zooms go on one video or image at a time", false);
                    return Task::none();
                };
                self.selection = vec![clip];
                if let Some(DropTarget::Timeline { t, .. }) = target {
                    self.playhead = t;
                }
                let before = self.doc.project.clip(clip).map(|c| c.zooms.len()).unwrap_or(0);
                self.add_zoom_at_playhead();
                if let (Some(DropTarget::Canvas { at: (x, y), .. }), Some(c)) = (target, self.doc.project.clip(clip)) {
                    if c.zooms.len() > before {
                        if let Some(z) = c.zooms.iter().max_by_key(|z| z.id) {
                            let zid = z.id;
                            return self.update(Message::Edit(clip, Edit::ZoomFocus(zid, x, y)));
                        }
                    }
                }
                Task::none()
            }
            Effect::Camera(preset) => {
                let [clip] = targets[..] else {
                    self.notify("Camera moves go on one video or image at a time", false);
                    return Task::none();
                };
                self.selection = vec![clip];
                if let Some(DropTarget::Timeline { t, .. }) = target {
                    self.playhead = t;
                }
                self.apply_camera_preset(clip, preset);
                Task::none()
            }
            Effect::Blur | Effect::Mask | Effect::Highlight(_) => Task::none(),
        }
    }

    /// Inserts a named camera move (push in / pull back) from the playhead to the end of the clip.
    fn apply_camera_preset(&mut self, clip_id: Id, preset: library::CameraPreset) {
        let at = self.playhead - self.doc.project.clip(clip_id).map(|c| c.start).unwrap_or(0.0);
        let applied = match preset {
            library::CameraPreset::PushIn => self.edit(preset.label(), |p| p.push_in(clip_id, at)),
            library::CameraPreset::PullBack => self.edit(preset.label(), |p| p.pull_back(clip_id, at)),
        };
        if applied.is_some() {
            self.notify(format!("{} added at the playhead", preset.label()), false);
        }
    }

    /// Rounded corners, a border or a shadow on the selected pictures, as one undo step.
    /// Text has none of these; it's skipped (with a word if that's all there was).
    fn apply_frame_style(&mut self, targets: &[Id], style: library::FrameStyle) {
        use library::FrameStyle;
        let p = &self.doc.project;
        let ids: Vec<Id> = targets.iter().copied().filter(|&id| p.clip(id).is_some_and(|c| p.clip_has_picture(c) && !matches!(c.content, crate::clip::Content::Text(_) | crate::clip::Content::Blur(_) | crate::clip::Content::Mask(_)))).collect();
        if ids.is_empty() {
            self.notify("Frame styles go on videos, images and shapes", false);
            return;
        }
        let name = match style {
            FrameStyle::Corners => "Rounded corners",
            FrameStyle::Border => "Border",
            FrameStyle::Shadow => "Shadow",
        };
        self.edit(name, |p| {
            for &id in &ids {
                p.ensure_unlocked(id)?;
                let l = &mut p.clip_mut(id)?.look;
                match style {
                    FrameStyle::Corners => l.corner_radius = if l.corner_radius > 0.0 { l.corner_radius } else { 28.0 },
                    FrameStyle::Border => {
                        if l.border_width <= 0.0 {
                            l.border_width = 4.0;
                            l.border_color = crate::clip::Rgba::WHITE;
                        }
                    }
                    FrameStyle::Shadow => l.shadow.enabled = true,
                }
            }
            Ok(())
        });
    }

    /// The clips that move when `clip` is dragged: the whole selection when it is part of it,
    /// and always whatever is grouped with them.
    fn moving_with(&self, clip: Id) -> Vec<Id> {
        let p = &self.doc.project;
        if self.selection.contains(&clip) { p.with_groups(&self.selection) } else { p.with_groups(&[clip]) }
    }

    /// After the selection changed as a whole: close editors that belonged to another clip.
    fn after_select(&mut self) {
        if !self.selection.is_empty() {
            self.selected_marker = None;
        }
        if self.zoom_edit.is_some_and(|(c, _)| !self.selection.contains(&c)) {
            self.zoom_edit = None;
        }
        if self.crop_edit.is_some_and(|c| self.selection != [c]) {
            self.crop_edit = None;
        }
        self.lanes_cache.clear();
    }

    /// Crossfades into the selected clips from the clips they touch. With one clip selected that
    /// has nothing before it, the cut after it is used; with nothing selected, the cut nearest
    /// the playhead.
    fn add_default_transition(&mut self) {
        let p = &self.doc.project;
        let open_cut = |c: &crate::clip::Clip| c.transition.is_none() && p.prev_on_track(c).is_some_and(|pv| (pv.end() - c.start).abs() < 1e-6) && !p.track(c.track).is_some_and(|t| t.locked);
        let mut targets: Vec<Id> = self.acting().into_iter().filter(|id| p.clip(*id).is_some_and(open_cut)).collect();
        if targets.is_empty() {
            targets = match self.selection.as_slice() {
                // The clip right after the selected one.
                [id] => p.clip(*id).and_then(|c| p.clips_on(c.track).find(|n| (n.start - c.end()).abs() < 1e-6 && open_cut(n))).map(|n| n.id).into_iter().collect(),
                [] => p.clips.iter().filter(|c| open_cut(c) && (c.start - self.playhead).abs() <= 2.0).min_by(|a, b| (a.start - self.playhead).abs().total_cmp(&(b.start - self.playhead).abs())).map(|c| c.id).into_iter().collect(),
                _ => Vec::new(),
            };
        }
        if targets.is_empty() {
            self.notify("Transitions go where two clips touch: select the second clip, or put the playhead on the cut", false);
            return;
        }
        let n = targets.len();
        if self.edit("Add transition", |p| targets.iter().try_for_each(|id| p.set_transition(*id, Some(crate::clip::TransitionKind::Crossfade), 0.6))).is_some() {
            self.selection = targets;
            self.side_tab = SideTab::Inspector;
            self.after_select();
            self.notify(if n == 1 { "Crossfade added — change it in the inspector, or drag its handle to resize" } else { "Crossfades added" }, false);
        }
    }

    /// Holds the frame under the playhead of the selected video (or the topmost one there).
    fn freeze_frame_at_playhead(&mut self) {
        let t = self.playhead;
        let p = &self.doc.project;
        let video = |c: &crate::clip::Clip| !c.freeze && t >= c.start && t <= c.end() && p.clip_has_picture(c) && c.asset().and_then(|a| p.asset(a)).is_some_and(|a| a.kind == MediaKind::Video);
        let selected = self.selection.iter().filter_map(|id| p.clip(*id)).find(|c| video(c)).map(|c| c.id);
        let topmost = p.tracks.iter().filter(|tr| !tr.locked).find_map(|tr| p.clips_on(tr.id).find(|c| video(c)).map(|c| c.id));
        let Some(clip) = selected.or(topmost) else {
            self.notify("Move the playhead over a video clip to freeze its frame", false);
            return;
        };
        if let Some(still) = self.edit("Freeze frame", |p| p.freeze_frame(clip, t, 2.0)) {
            self.selection = vec![still];
            self.after_select();
            self.notify("Frame held for 2 seconds — drag its edge to make it longer or shorter", false);
        }
    }

    /// Selects a clip for on-canvas editing and moves the playhead onto it if needed.
    fn focus_clip(&mut self, clip: Id) {
        self.stop_playback();
        self.selection = vec![clip];
        self.side_tab = SideTab::Inspector;
        if let Some((a, b)) = self.doc.project.clip(clip).map(|c| (c.start, c.end())) {
            if self.playhead < a || self.playhead >= b {
                self.seek(a + (b - a).min(1.0) / 2.0);
            }
        }
        self.lanes_cache.clear();
    }

    fn set_range(&mut self, a: f64, b: f64) {
        self.range = Some((a, b));
        self.range_mode = true;
    }

    /// Camtasia-style: a zoom starting at the playhead on the clip under it. Right after
    /// another zoom it continues at the same magnification, which makes a pan.
    fn add_zoom_at_playhead(&mut self) {
        let t = self.playhead;
        let p = &self.doc.project;
        let visual = |c: &crate::clip::Clip| c.asset().is_some() && p.clip_has_picture(c) && t >= c.start && t < c.end();
        let selected = self.selection.iter().filter_map(|id| p.clip(*id)).find(|c| visual(c)).map(|c| c.id);
        let topmost = p.tracks.iter().find_map(|tr| p.clips_on(tr.id).find(|c| visual(c)).map(|c| c.id));
        let Some(clip_id) = selected.or(topmost) else {
            self.notify("Move the playhead over a video or image clip to add a zoom", false);
            return;
        };
        let c = p.clip(clip_id).unwrap();
        let local = t - c.start;
        if let Some(z) = c.zooms.iter().find(|z| local >= z.at && local < z.at + z.duration) {
            let target = Some((clip_id, z.id));
            let _ = self.update(Message::ZoomEdit(target));
            return;
        }
        let prev = c.zooms.iter().filter(|z| z.at + z.duration <= local + 1e-6).max_by(|a, b| (a.at + a.duration).total_cmp(&(b.at + b.duration))).copied();
        let next_start = c.zooms.iter().filter(|z| z.at > local).map(|z| z.at).fold(c.duration, f64::min);
        let (at, scale, pan) = match prev {
            Some(z) if local - (z.at + z.duration) < 0.4 => (z.at + z.duration, z.scale, true),
            _ => (local, 1.8, false),
        };
        let duration = 3.0f64.min(next_start - at);
        if duration < 0.3 {
            self.notify("There's no room for another zoom here", false);
            return;
        }
        let (fx, fy) = match prev {
            Some(z) if pan => ((z.focus_x + 0.25).min(0.85), z.focus_y),
            _ => (0.5, 0.5),
        };
        let created = self.edit(if pan { "Add pan" } else { "Add zoom" }, |p| {
            let zid = p.alloc_id();
            p.clip_mut(clip_id)?.zooms.push(Zoom { id: zid, at, duration, scale, focus_x: fx, focus_y: fy, ramp: 0.6 });
            Ok(zid)
        });
        if let Some(zid) = created {
            let _ = self.update(Message::ZoomEdit(Some((clip_id, zid))));
            self.notify(if pan { "Pan added — drag the frame on the canvas to aim it" } else { "Zoom added — drag the frame on the canvas to aim it" }, false);
        }
    }

    /// A one-shot "button press" pulse on the clip under the playhead (selected clip preferred).
    fn add_press_at_playhead(&mut self) {
        let t = self.playhead;
        let p = &self.doc.project;
        let visual = |c: &crate::clip::Clip| p.clip_has_picture(c) && t >= c.start && t < c.end();
        let selected = self.selection.iter().filter_map(|id| p.clip(*id)).find(|c| visual(c)).map(|c| c.id);
        let topmost = p.tracks.iter().find_map(|tr| p.clips_on(tr.id).find(|c| visual(c)).map(|c| c.id));
        let Some(clip_id) = selected.or(topmost) else {
            self.notify("Move the playhead over a clip to add a press pulse", false);
            return;
        };
        let local = t - p.clip(clip_id).unwrap().start;
        self.edit("Add press pulse", |p| {
            let id = p.alloc_id();
            p.clip_mut(clip_id)?.presses.push(crate::clip::PressPulse { id, at: local, duration: crate::clip::PressPulse::DEFAULT_DURATION });
            Ok(())
        });
        self.notify("Press pulse added at the playhead", false);
    }

    /// Staggers the current selection's entrances a few frames apart, in selection order.
    fn stagger_selection(&mut self) {
        let ids = self.selection.clone();
        if ids.len() < 2 {
            self.notify("Select more than one clip to stagger their entrances", false);
            return;
        }
        const GAP_FRAMES: u32 = 3;
        let fps = self.doc.project.canvas.fps;
        self.edit("Stagger entrances", |p| p.stagger_group(&ids, GAP_FRAMES, fps));
        self.notify("Entrances staggered", false);
    }

    /// Drops UI state that points at things an undo, redo or delete removed.
    fn after_history(&mut self) {
        let p = &self.doc.project;
        self.selection.retain(|id| p.clip(*id).is_some());
        if self.zoom_edit.is_some_and(|(c, z)| !p.clip(c).is_some_and(|c| c.zooms.iter().any(|zz| zz.id == z))) {
            self.zoom_edit = None;
        }
        if self.selected_marker.is_some_and(|m| !p.markers.iter().any(|x| x.id == m)) {
            self.selected_marker = None;
        }
        if self.crop_edit.is_some_and(|c| p.clip(c).is_none()) {
            self.crop_edit = None;
        }
        if self.renaming.as_ref().is_some_and(|(t, _)| p.track(*t).is_none()) {
            self.renaming = None;
        }
        if self.library_selected.is_some_and(|a| p.asset(a).is_none()) {
            self.library_selected = None;
        }
        if self.hex_draft.as_ref().is_some_and(|(c, _, _)| !self.selection.contains(c)) {
            self.hex_draft = None;
        }
        self.lanes_cache.clear();
    }

    fn track_action(&mut self, id: Id, action: TrackAction) -> Task<Message> {
        match action {
            TrackAction::Rename => {
                if let Some(t) = self.doc.project.track(id) {
                    self.renaming = Some((id, t.name.clone()));
                    return iced::widget::operation::focus("track-rename");
                }
            }
            TrackAction::ToggleHidden => {
                self.edit("Toggle track visibility", |p| {
                    let t = p.track_mut(id)?;
                    t.hidden = !t.hidden;
                    Ok(())
                });
            }
            TrackAction::ToggleLocked => {
                self.edit("Toggle track lock", |p| {
                    let t = p.track_mut(id)?;
                    t.locked = !t.locked;
                    Ok(())
                });
            }
            TrackAction::ToggleMuted => {
                self.edit("Toggle track mute", |p| {
                    let t = p.track_mut(id)?;
                    t.muted = !t.muted;
                    Ok(())
                });
            }
            TrackAction::MoveUp => {
                self.edit("Move track", |p| p.move_track(id, -1));
            }
            TrackAction::MoveDown => {
                self.edit("Move track", |p| p.move_track(id, 1));
            }
            TrackAction::Delete => {
                self.edit("Delete track", |p| p.delete_track(id));
                self.after_history();
            }
            TrackAction::AddVideo => {
                let index = self.doc.project.track_index(id);
                let n = self.doc.project.tracks.iter().filter(|t| t.kind == TrackKind::Visual).count() + 1;
                self.edit("Add track", |p| Ok(p.add_track(TrackKind::Visual, &format!("Video {n}"), index)));
            }
            TrackAction::AddAudio => {
                let index = self.doc.project.track_index(id).map(|i| i + 1);
                let n = self.doc.project.tracks.iter().filter(|t| t.kind == TrackKind::Audio).count() + 1;
                self.edit("Add track", |p| Ok(p.add_track(TrackKind::Audio, &format!("Audio {n}"), index)));
            }
        }
        Task::none()
    }

    // ------------------------------------------------------------ view

    pub fn view(&self) -> Element<'_, Message> {
        let grid = pane_grid(&self.panes, |_, pane, _| {
            // The preview has no header, so the canvas gets the space; it sits on the workspace color.
            if *pane == Pane::Preview {
                return pane_grid::Content::new(container(self.preview()).width(Fill).height(Fill)).style(style::workspace);
            }
            let (header, body): (Element<'_, Message>, Element<'_, Message>) = match pane {
                Pane::Library => (self.library_header(), self.library()),
                Pane::Inspector => (self.side_header(), self.inspector()),
                _ => (self.timeline_header(), self.timeline()),
            };
            let header = column![container(header).padding([0, 6]), widgets::divider()];
            pane_grid::Content::new(container(body).width(Fill).height(Fill)).title_bar(pane_grid::TitleBar::new(header).style(style::panel_header)).style(style::panel)
        })
        // Small gaps of the black background between the rounded blocks.
        .spacing(PANE_GAP)
        .min_size(PANE_MIN)
        .on_resize(10, Message::PaneResized)
        .style(|_| pane_grid::Style {
            hovered_region: pane_grid::Highlight { background: ACCENT_SOFT.into(), border: iced::Border { color: ACCENT, width: 1.0, radius: R_LG.into() } },
            picked_split: pane_grid::Line { color: SPLIT_DRAG, width: PANE_GAP },
            hovered_split: pane_grid::Line { color: SPLIT_HOVER, width: PANE_GAP },
        });
        // A small grip in the middle of every gap that can be dragged.
        let grips = responsive(|size| {
            let splits = self.panes.layout().split_regions(PANE_GAP, PANE_MIN, size);
            iced::widget::canvas(SplitGrips(splits.into_values().collect())).width(Fill).height(Fill).into()
        });
        let body = container(iced::widget::stack![grid, grips]).padding(GRID_PAD).width(Fill).height(Fill).style(style::backdrop);
        let mut layers: Vec<Element<'_, Message>> = vec![body.into()];
        if let Some((msg, error, since)) = &self.toast {
            let toast = container(
                row![icon(if *error { code::TRIANGLE_ALERT } else { code::CIRCLE_CHECK }, 14).color(if *error { DANGER } else { SUCCESS }), text(msg).size(T_SMALL)]
                    .spacing(S2)
                    .align_y(Alignment::Center),
            )
            .padding([9, 14])
            .style(style::popover);
            // It rises from the bottom edge into place.
            let rise = 22.0 * crate::ui::motion::appear_progress(*since);
            layers.push(container(toast).center_x(Fill).align_bottom(Fill).padding(iced::Padding { bottom: rise, ..Default::default() }).into());
        }
        if self.dev_menu {
            layers.push(container(self.context_menu()).center(Fill).into());
        }
        if self.file_hover {
            let card = column![
                container(icon(code::UPLOAD, 22).color(ON_ACCENT)).center(48).style(style::logo),
                text("Drop to import").font(INTER_SEMIBOLD).size(T_TITLE),
                text("Video, audio and images go into your media library").size(T_SMALL).color(TEXT_2),
            ]
            .spacing(S2)
            .align_x(Alignment::Center);
            layers.push(
                container(container(card).center(Fill).style(|_| container::Style {
                    background: Some(Color { a: 0.82, ..WINDOW }.into()),
                    border: iced::Border { color: ACCENT, width: 2.0, radius: R_XL.into() },
                    ..Default::default()
                }))
                .padding(10)
                .into(),
            );
        }
        if let Some(dialog) = &self.export {
            layers.push(dialog.view(&self.doc.project, self.dialog.progress()));
        }
        if let Some(dialog) = &self.skills_dialog {
            layers.push(dialog.view(&self.skills, self.dialog.progress()));
        }
        if let Some(dialog) = &self.record {
            layers.push(dialog.view(self.dialog.progress()));
        }
        stack(layers).into()
    }

    fn library_header(&self) -> Element<'_, Message> {
        // The five tabs share the width left of the import button (each in proportion to its
        // name), so none is pushed out of a narrow panel.
        let tabs = row(LibraryTab::ALL.iter().map(|&t| {
            let name = text(t.label()).font(INTER_MEDIUM).size(T_SMALL).wrapping(iced::widget::text::Wrapping::None);
            button(container(name).center_x(Fill))
                .width(Length::FillPortion(t.label().len() as u16 + 3))
                .padding([5, 2])
                .style(style::segment(t == self.library_tab))
                .on_press(Message::LibraryTab(t))
                .into()
        }))
        .spacing(2);
        let import = widgets::tip_keys(widgets::icon_button(code::PLUS, Some(Message::Import)), "Import media", self.key_of(crate::shortcuts::Action::Import), tip_pos());
        // Same frame as `header_row`, but the tabs take the free width instead of a spacer.
        container(row![container(tabs).width(Fill).max_width(380), import].spacing(S1).align_y(Alignment::Center)).height(38).padding([0, 2]).center_y(38).into()
    }

    fn side_header(&self) -> Element<'_, Message> {
        let tabs = row![
            tab("Inspector", self.side_tab == SideTab::Inspector, Message::SideTab(SideTab::Inspector)),
            tab_icon(code::SPARKLES, "Assistant", self.side_tab == SideTab::Assistant, Message::SideTab(SideTab::Assistant)),
        ]
        .spacing(2);
        let right: Element<'_, Message> = if self.side_tab == SideTab::Assistant {
            let mut row = row![widgets::tip(widgets::icon_button(code::WAND_SPARKLES, Some(Message::OpenSkills)), "Skills", None, tip_pos())].spacing(2);
            if !self.chat.is_empty() {
                row = row.push(widgets::tip(widgets::icon_button(code::FILE_PLUS, Some(Message::ChatNew)), "New conversation", None, tip_pos()));
            }
            row.into()
        } else {
            space().into()
        };
        header_row(tabs.into(), right)
    }

    fn preview(&self) -> Element<'_, Message> {
        let (cw, ch) = self.doc.project.canvas.size();
        let stage = responsive(move |size: Size| {
            self.stage_size.set(size);
            let (w, h, origin) = self.canvas_placement(size);
            // Decode a little above the on-screen size so high-DPI screens stay sharp (never above the canvas).
            self.frames.set_preview_width(self.preview_quality().decode_width(w, cw as f32));
            self.frames.collect_idle();
            let project = &self.doc.project;
            // While editing a zoom, show that clip unzoomed so the whole frame is visible.
            let editing = self.zoom_edit.and_then(|(clip, zoom)| project.clip(clip).and_then(|c| c.zooms.iter().find(|z| z.id == zoom).map(|z| (clip, *z))));
            // While cropping, show the whole frame too, so the crop can be widened again.
            let cropping = self.crop_edit.filter(|clip| self.selection == [*clip]).and_then(|clip| project.clip(clip).map(|c| (clip, c.crop.clamped())));
            let unzoomed;
            let shown = match editing.map(|(clip, _)| clip).or(cropping.map(|(clip, _)| clip)) {
                Some(clip) => {
                    let mut p = project.clone();
                    if let Ok(c) = p.clip_mut(clip) {
                        c.zooms.clear();
                        if cropping.is_some() {
                            c.crop = Default::default();
                        }
                    }
                    unzoomed = p;
                    &unzoomed
                }
                None => project,
            };
            let scene = std::sync::Arc::new(scene::evaluate(shown, self.playhead, (cw, ch), &self.frames.bind(project), self.playing.is_some()));
            // The overlay spans the whole stage, so handles keep working past the canvas edge.
            let overlay = canvas_overlay::Overlay { project, scene: scene.clone(), selection: &self.selection, zoom_edit: editing, crop_edit: cropping, dragging: self.effect_dnd, origin, display: Size::new(w, h), zoomed: self.preview_zoom.is_some(), show_safe_zones: self.show_safe_zones };
            let canvas = iced::widget::shader(preview::Preview { scene, canvas: iced::Rectangle::new(Point::new(origin.x, origin.y), Size::new(w, h)) }).width(Fill).height(Fill);
            stack![canvas, iced::widget::canvas(overlay).width(Fill).height(Fill)].into()
        });
        let duration = self.doc.project.duration();
        let fps = self.doc.project.canvas.fps;
        // Three fixed zones: the controls in the middle never move while the time ticks.
        let time = row![tabular(&fmt_timecode_frames(self.playhead, fps), TEXT_1), text("  /  ").size(T_SMALL).color(TEXT_4), tabular(&fmt_timecode(duration), TEXT_3)].align_y(Alignment::Center);
        let controls = row![
            widgets::tip_keys(widgets::icon_button(code::SKIP_BACK, Some(Message::GoStart)), "Go to start", self.key_of(crate::shortcuts::Action::GoStart), tip_pos()),
            widgets::tip_keys(play_button(self.playing.is_some()), if self.playing.is_some() { "Pause" } else { "Play" }, self.key_of(crate::shortcuts::Action::PlayPause), tip_pos()),
            widgets::tip_keys(widgets::icon_button(code::SKIP_FORWARD, Some(Message::GoEnd)), "Go to end", self.key_of(crate::shortcuts::Action::GoEnd), tip_pos()),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let tools = row![
            widgets::tip(widgets::icon_toggle(code::SMARTPHONE, self.show_safe_zones, Message::ToggleSafeZones), if self.show_safe_zones { "Hide safe zones" } else { "Show 9:16 / 1:1 safe zones" }, None, tip_pos()),
            widgets::tip_keys(widgets::icon_toggle(if self.muted { code::VOLUME_X } else { code::VOLUME_2 }, self.muted, Message::ToggleMute), if self.muted { "Unmute preview" } else { "Mute preview" }, self.key_of(crate::shortcuts::Action::MutePreview), tip_pos()),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let transport = row![container(time).width(Fill), controls, container(tools).width(Fill).align_right(Fill)].align_y(Alignment::Center);
        let bar = column![widgets::divider(), container(transport).padding([7, 12])];
        let stage = ContextMenu::new(container(stage).height(Fill), move || self.context_menu()).style(|_, _| iced_aw::style::context_menu::Style { background: Color::TRANSPARENT.into() });
        column![stage, bar].into()
    }

    fn inspector(&self) -> Element<'_, Message> {
        match self.side_tab {
            SideTab::Inspector => match self.selection.as_slice() {
                [] => match self.selected_marker.and_then(|id| self.doc.project.markers.iter().find(|m| m.id == id)) {
                    Some(m) => inspector::marker(m),
                    None => inspector::empty(),
                },
                [id] => match self.doc.project.clip(*id) {
                    Some(c) => {
                        let ctx = inspector::Ctx {
                            text_draft: self.text_draft.as_ref().filter(|(d, _)| *d == c.id).map(|(_, content)| content),
                            zoom_edit: self.zoom_edit.map(|(_, z)| z),
                            crop_edit: self.crop_edit == Some(c.id),
                            range: self.range,
                            hex: self.hex_draft.as_ref(),
                            collapsed: &self.collapsed,
                            query: &self.inspector_query,
                            preset_tiles: self.preset_tiles.as_ref().map(|(_, tiles)| tiles.as_slice()).unwrap_or(&[]),
                        };
                        column![container(inspector::search_bar(&self.inspector_query)).padding(iced::Padding { top: 8.0, right: 10.0, bottom: 4.0, left: 4.0 }), inspector::view(&self.doc.project, c, &ctx)].into()
                    }
                    None => inspector::empty(),
                },
                many => inspector::multiple(many.len()),
            },
            SideTab::Assistant => self.assistant_view(),
        }
    }

    /// One line describing the editor state, sent with each assistant message.
    fn context_line(&self) -> String {
        let p = &self.doc.project;
        let mut parts = vec![format!("playhead {:.2}s", self.playhead), format!("canvas {}", p.canvas.summary())];
        match self.selection.as_slice() {
            [] => parts.push("no clip selected".into()),
            [id] => {
                if let Some(c) = p.clip(*id) {
                    parts.push(format!("selected clip #{} \"{}\" ({:.2}–{:.2}s)", c.id, timeline::clip_label(p, c), c.start, c.end()));
                }
            }
            many => parts.push(format!("selected clips {many:?}")),
        }
        if let Some((a, b)) = self.range {
            parts.push(format!("selected range {a:.2}–{b:.2}s"));
        }
        parts.join("; ")
    }

    /// The user's enabled skills, joined into one block for the assistant's system prompt
    /// (empty if none are on).
    fn skills_text(&self) -> String {
        self.skills.iter().filter(|s| s.enabled).map(|s| format!("## {}\n{}", s.name, s.body)).collect::<Vec<_>>().join("\n\n")
    }

    fn timeline_header(&self) -> Element<'_, Message> {
        // Range mode: Done replaces the tool; it clears the range when its job is done.
        let range_tool: Element<'_, Message> = if self.range_mode {
            let done = button(row![icon(code::CHECK, 13), text("Done").font(INTER_SEMIBOLD).size(T_SMALL)].spacing(6).align_y(Alignment::Center)).padding([5, 10]).style(style::primary).on_press(Message::RangeDone);
            let tip = if self.range.is_some() { "Drag the edges to adjust. Done clears the range" } else { "Drag on the timeline to select a range" };
            widgets::tip_keys(done, tip, self.key_of(crate::shortcuts::Action::RangeDone), tip_pos())
        } else {
            widgets::tip_keys(labeled_tool(code::SQUARE_DASHED, "Range", false, Message::RangeTool), "Select a part of the timeline: drag across it", self.key_of(crate::shortcuts::Action::RangeTool), tip_pos())
        };
        // Group / Ungroup share one button: it shows what would happen to the selection.
        let p = &self.doc.project;
        let chosen: Vec<&crate::clip::Clip> = self.selection.iter().filter_map(|id| p.clip(*id)).collect();
        let one_group = chosen.first().is_some_and(|f| f.group.is_some() && chosen.iter().all(|c| c.group == f.group));
        let group_tool: Element<'_, Message> = if one_group {
            widgets::tip_keys(widgets::icon_button(code::UNGROUP, Some(Message::Ungroup)), "Ungroup", self.key_of(crate::shortcuts::Action::Ungroup), tip_pos())
        } else {
            widgets::tip_keys(widgets::icon_button(code::GROUP, (chosen.len() > 1).then_some(Message::Group)), "Group the selected clips, so they move and trim together", self.key_of(crate::shortcuts::Action::Group), tip_pos())
        };
        let marked = self.range_mode && self.range.is_some();
        let can_ripple = marked || !self.selection.is_empty();
        let tools = row![
            widgets::tip_keys(labeled_tool(code::SCISSORS, "Split", false, Message::SplitAtPlayhead), "Split at the playhead", self.key_of(crate::shortcuts::Action::Split), tip_pos()),
            range_tool,
            widgets::tip_keys(labeled_tool(code::ZOOM_IN, "Zoom", self.zoom_edit.is_some(), Message::AddZoomAtPlayhead), "Add a zoom or pan at the playhead", self.key_of(crate::shortcuts::Action::AddZoom), tip_pos()),
            widgets::tip_keys(labeled_tool(code::FLAG, "Marker", self.selected_marker.is_some(), Message::AddMarker), "Add a marker at the playhead (or over the marked range)", self.key_of(crate::shortcuts::Action::AddMarker), tip_pos()),
            container(widgets::vdivider(16.0)).padding([0, 4]),
            group_tool,
            widgets::tip_keys(widgets::icon_button(code::CHEVRONS_RIGHT_LEFT, (!self.doc.project.clips.is_empty()).then_some(Message::CloseGaps)), if self.selection.is_empty() { "Close gaps: remove all time where nothing plays" } else { "Close the gaps between clips on this track" }, self.key_of(crate::shortcuts::Action::CloseGaps), tip_pos()),
            widgets::tip_keys(widgets::icon_button(code::DELETE, can_ripple.then_some(Message::RippleDelete)), if marked { "Cut the marked range out of every track" } else { "Ripple delete: delete and close the space" }, self.key_of(crate::shortcuts::Action::RippleDelete), tip_pos()),
            widgets::tip_keys(widgets::icon_button(code::TRASH_2, (!self.selection.is_empty() || self.zoom_edit.is_some()).then_some(Message::DeleteSelection)), "Delete", self.key_of(crate::shortcuts::Action::Delete), tip_pos()),
            widgets::tip_keys(widgets::icon_toggle(code::MAGNET, self.snapping, Message::ToggleSnapping), "Snapping", self.key_of(crate::shortcuts::Action::ToggleSnapping), tip_pos()),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let zoom = row![
            widgets::tip_keys(widgets::icon_button(code::ZOOM_OUT, Some(Message::ZoomBy(1.0 / 1.4))), "Zoom out", self.key_of(crate::shortcuts::Action::ZoomOut), tip_pos()),
            widgets::tip_keys(widgets::icon_button(code::SCAN, Some(Message::ZoomFit)), "Fit timeline", self.key_of(crate::shortcuts::Action::ZoomFit), tip_pos()),
            widgets::tip_keys(widgets::icon_button(code::ZOOM_IN, Some(Message::ZoomBy(1.4))), "Zoom in", self.key_of(crate::shortcuts::Action::ZoomIn), tip_pos()),
        ]
        .spacing(2);
        // Lines up with where the ruler/lanes start past the track-header column (`HEADER_W`),
        // minus the padding this header already has from the pane edge (6 outer + 12 here).
        container(
            row![text("Timeline").font(INTER_SEMIBOLD).size(T_SMALL).color(TEXT_2), space().width(timeline::HEADER_W - 18.0), tools, space().width(Fill), zoom]
                .align_y(Alignment::Center)
                .spacing(12),
        )
        .height(38)
        .padding([0, 12])
        .center_y(38)
        .into()
    }

    fn timeline(&self) -> Element<'_, Message> {
        responsive(move |size| {
            self.timeline_size.set(size);
            let project = &self.doc.project;
            let content_h = timeline::content_height(project);
            let overflow = content_h > size.height - RULER_H;
            let add = DropDownless::add_track_button(project);
            let ruler = row![
                container(add).width(HEADER_W).height(RULER_H).padding([0, 8]).center_y(RULER_H),
                iced::widget::canvas(Ruler { markers: &project.markers, selected_marker: self.selected_marker, view: self.view, playhead: self.playhead, range: self.range, range_mode: self.range_mode }).width(Fill).height(RULER_H),
            ];
            let now = Instant::now();
            let lanes = Lanes {
                project,
                anim: self.clip_anim.as_ref().map(|a| (a, a.progress(now))),
                media: &self.media,
                selection: &self.selection,
                playhead: self.playhead,
                view: self.view,
                snapping: self.snapping,
                zoom_edit: self.zoom_edit,
                range: self.range,
                range_mode: self.range_mode,
                dnd: self.dnd,
                dnd_effect: self.effect_dnd,
                missing: &self.missing,
                cache: &self.lanes_cache,
                overflow,
                vscroll: self.vscroll.min((content_h - (size.height - RULER_H)).max(0.0)),
            };
            // No scroll area: the lanes scroll themselves (always clipped to their bounds)
            // and the headers follow the same offset inside a clipped column.
            // Leave out tracks scrolled completely out of view; offset only the first visible one.
            let rows = timeline::rows(project, lanes.vscroll);
            let first = rows.iter().position(|r| r.2 + r.3 + timeline::GAP > 0.0).unwrap_or(0);
            let offset = rows.get(first).map(|r| r.2 - timeline::TOP_PAD).unwrap_or(0.0);
            let headers = mouse_area(
                widgets::clipped(container(iced::widget::pin(self.track_headers(first)).y(offset)).width(HEADER_W).height(Fill)),
            )
            .on_scroll(|d| Message::ScrollTracks(-match d {
                mouse::ScrollDelta::Lines { y, .. } => y * 48.0,
                mouse::ScrollDelta::Pixels { y, .. } => y,
            }));
            let body = row![
                headers,
                ContextMenu::new(stack![iced::widget::canvas(timeline::LanesBase(lanes)).width(Fill).height(Fill), iced::widget::canvas(lanes).width(Fill).height(Fill)], move || self.context_menu())
                    .style(|_, _| iced_aw::style::context_menu::Style { background: Color::TRANSPARENT.into() }),
            ];
            column![ruler, container(widgets::divider()), container(body).height(Fill).clip(true)].into()
        })
        .into()
    }

    fn track_headers(&self, skip: usize) -> Element<'_, Message> {
        let project = &self.doc.project;
        let mut col = column![space().height(timeline::TOP_PAD)].spacing(0);
        for t in project.tracks.iter().skip(skip) {
            let h = timeline::track_height(t);
            let id = t.id;
            let (glyph, color) = match t.kind {
                TrackKind::Visual => (code::LAYERS, clip::VIDEO),
                TrackKind::Audio => (code::AUDIO_LINES, clip::AUDIO),
            };
            let name: Element<'_, Message> = match &self.renaming {
                Some((rid, value)) if *rid == id => text_input("Track name", value)
                    .id("track-rename")
                    .on_input(Message::RenameInput)
                    .on_submit(Message::RenameCommit)
                    .padding([3, 6])
                    .size(T_SMALL)
                    .style(style::input)
                    .into(),
                _ => {
                    let font = if t.name.is_ascii() { INTER_MEDIUM } else { HIND_SEMIBOLD };
                    mouse_area(text(&t.name).font(font).size(T_SMALL).wrapping(text::Wrapping::None)).on_double_click(Message::Track(id, TrackAction::Rename)).into()
                }
            };
            let small = |glyph: char, active: bool, action: TrackAction, tip: &'static str| {
                widgets::tip(
                    button(container(icon(glyph, 12).color(if active { WARNING } else { TEXT_3 })).center(22)).width(22).height(22).padding(0).style(style::ghost).on_press(Message::Track(id, action)),
                    tip,
                    None,
                    tip_pos(),
                )
            };
            let mut toggles = row![].spacing(0);
            if t.kind == TrackKind::Audio {
                toggles = toggles.push(small(if t.muted { code::VOLUME_X } else { code::VOLUME_2 }, t.muted, TrackAction::ToggleMuted, "Mute track"));
            } else {
                toggles = toggles.push(small(if t.hidden { code::EYE_OFF } else { code::EYE }, t.hidden, TrackAction::ToggleHidden, "Hide track"));
            }
            toggles = toggles.push(small(if t.locked { code::LOCK } else { code::LOCK_OPEN }, t.locked, TrackAction::ToggleLocked, "Lock track"));
            let header = container(row![tinted(glyph, 13, color), container(name).width(Fill).clip(true), toggles].spacing(8).align_y(Alignment::Center))
                .padding(iced::Padding { left: 10.0, right: 4.0, ..Default::default() })
                .height(h)
                .center_y(h)
                .style(style::card);
            let menu = ContextMenu::new(header, move || {
                widgets::menu(
                    vec![
                        MenuItem::new(code::PENCIL, "Rename track", Message::Track(id, TrackAction::Rename)),
                        MenuItem::Separator,
                        MenuItem::new(code::LAYERS, "Add video track", Message::Track(id, TrackAction::AddVideo)),
                        MenuItem::new(code::AUDIO_LINES, "Add audio track", Message::Track(id, TrackAction::AddAudio)),
                        MenuItem::Separator,
                        MenuItem::new(code::CHEVRON_UP, "Move up", Message::Track(id, TrackAction::MoveUp)),
                        MenuItem::new(code::CHEVRON_DOWN, "Move down", Message::Track(id, TrackAction::MoveDown)),
                        MenuItem::Separator,
                        MenuItem::new(code::TRASH_2, "Delete track", Message::Track(id, TrackAction::Delete)).danger(),
                    ],
                    210.0,
                )
            })
            .style(|_, _| iced_aw::style::context_menu::Style { background: Color::TRANSPARENT.into() });
            // The gap under each header is a handle for resizing the track's height.
            let grip = mouse_area(space().width(Fill).height(timeline::GAP)).interaction(mouse::Interaction::ResizingVertically).on_press(Message::TrackResizeStart(id));
            col = col.push(column![container(menu).padding(iced::Padding { left: 8.0, right: 6.0, ..Default::default() }), grip]);
        }
        col.into()
    }
}

/// Room around the fitted canvas so selection handles stay reachable at its edges.
const PREVIEW_ROOM: f32 = 14.0;
pub const PREVIEW_ZOOM_MIN: f32 = 0.05;
pub const PREVIEW_ZOOM_MAX: f32 = 8.0;
/// Zoom levels offered in the toolbar (besides Fit).
pub const PREVIEW_ZOOMS: [f32; 8] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];

/// Keeps a zoomed canvas from being panned out of view (no panning when it fits).
fn clamp_pan(pan: iced::Vector, stage: Size, w: f32, h: f32) -> iced::Vector {
    let mx = ((w - stage.width) / 2.0 + PREVIEW_ROOM).max(0.0);
    let my = ((h - stage.height) / 2.0 + PREVIEW_ROOM).max(0.0);
    iced::Vector::new(pan.x.clamp(-mx, mx), pan.y.clamp(-my, my))
}

/// Editing actions (mostly from shortcuts) that a modal dialog keeps from reaching the project.
/// Lists the cameras and microphones in the background, for the Record dialog.
fn record_devices() -> Task<Message> {
    Task::perform(
        async { tokio::task::spawn_blocking(|| crate::recorder::devices().map_err(|e| format!("{e:#}"))).await.unwrap_or_else(|e| Err(e.to_string())) },
        |found| Message::Record(record_dialog::Msg::Found(found)),
    )
}

pub fn blocked_by_dialog(m: &Message) -> bool {
    matches!(
        m,
        Message::TogglePlay
            | Message::Key(..)
            | Message::SelectAll
            | Message::SelectForward
            | Message::Copy
            | Message::Cut
            | Message::Paste
            | Message::PasteEffects
            | Message::Group
            | Message::Ungroup
            | Message::RippleDelete
            | Message::CloseGaps
            | Message::DefaultTransition
            | Message::FreezeFrame
            | Message::ToggleDisabled
            | Message::DeleteSelection
            | Message::DuplicateSelection
            | Message::Arrange(_)
            | Message::SplitAtPlayhead
            | Message::RangeTool
            | Message::RangePress { .. }
            | Message::RangeDone
            | Message::AddZoomAtPlayhead
            | Message::AddPressAtPlayhead
            | Message::StaggerSelection
            | Message::AddText
            | Message::Undo
            | Message::Redo
            | Message::Step(_)
            | Message::GoStart
            | Message::GoEnd
            | Message::ToggleSnapping
            | Message::ImportPicked(_)
            | Message::AddMarker
            | Message::ToggleMute
            | Message::ToggleSafeZones
            | Message::SeparateAudio
            | Message::ToggleClipMute
    )
}

/// Grips centered in the gaps between blocks: (axis, region the split divides, ratio) per split.
/// Draw-only, so dragging still goes to the pane grid underneath.
struct SplitGrips(Vec<(pane_grid::Axis, iced::Rectangle, f32)>);

impl iced::widget::canvas::Program<Message> for SplitGrips {
    type State = ();

    fn draw(&self, _: &(), renderer: &iced::Renderer, _: &iced::Theme, bounds: iced::Rectangle, _: iced::mouse::Cursor) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path};
        const LONG: f32 = 22.0;
        const THICK: f32 = 3.0;
        let mut frame = Frame::new(renderer, bounds.size());
        for (axis, r, ratio) in &self.0 {
            // The gap's center line sits at `ratio` across the region (see iced's `Axis::split`).
            let (at, size) = match axis {
                pane_grid::Axis::Horizontal => (iced::Point::new(r.center_x() - LONG / 2.0, r.y + r.height * ratio - THICK / 2.0), iced::Size::new(LONG, THICK)),
                pane_grid::Axis::Vertical => (iced::Point::new(r.x + r.width * ratio - THICK / 2.0, r.center_y() - LONG / 2.0), iced::Size::new(THICK, LONG)),
            };
            frame.fill(&Path::rounded_rectangle(at, size, (THICK / 2.0).into()), SPLIT_GRIP);
        }
        vec![frame.into_geometry()]
    }
}

/// A hash of everything that affects the audio mix, so playback restarts only when the sound changes.
fn audio_fingerprint(p: &Project) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in &p.clips {
        if let crate::clip::Content::Media { asset, src_in, speed } = c.content {
            (c.id, asset, c.track).hash(&mut h);
            for v in [c.start, c.duration, src_in, speed, c.audio.volume, c.audio.fade_in, c.audio.fade_out] {
                v.to_bits().hash(&mut h);
            }
            (c.audio.muted, c.part, c.disabled, c.freeze).hash(&mut h);
            for g in &c.audio.gains {
                (g.at.to_bits(), g.duration.to_bits(), g.volume.to_bits()).hash(&mut h);
            }
        }
    }
    for t in &p.tracks {
        (t.id, t.muted, t.hidden).hash(&mut h);
    }
    for a in &p.assets {
        a.path.hash(&mut h);
    }
    h.finish()
}

/// Synthesizes (if needed) and probes a built-in sound in the background.
fn sound_task(id: &'static str, done: fn(&'static str, Result<(PathBuf, ProbeInfo), String>) -> Message) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let path = crate::sfx::path(id).map_err(|e| e.to_string())?;
                let info = media::probe(&path).map_err(|e| e.to_string())?;
                Ok((path, info))
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        },
        move |r| done(id, r),
    )
}

/// Track creation button above the headers.
struct DropDownless;

impl DropDownless {
    fn add_track_button<'a>(project: &Project) -> Element<'a, Message> {
        let first_visual = project.tracks.iter().find(|t| t.kind == TrackKind::Visual).map(|t| t.id).unwrap_or(0);
        let last = project.tracks.last().map(|t| t.id).unwrap_or(0);
        row![
            widgets::tip(
                button(row![icon(code::PLUS, 12), text("Video").size(T_CAPTION)].spacing(4).align_y(Alignment::Center)).padding([3, 7]).style(style::ghost).on_press(Message::Track(first_visual, TrackAction::AddVideo)),
                "Add video track",
                None,
                tip_pos(),
            ),
            widgets::tip(
                button(row![icon(code::PLUS, 12), text("Audio").size(T_CAPTION)].spacing(4).align_y(Alignment::Center)).padding([3, 7]).style(style::ghost).on_press(Message::Track(last, TrackAction::AddAudio)),
                "Add audio track",
                None,
                tip_pos(),
            ),
        ]
        .spacing(2)
        .into()
    }
}

/// Global events the editor cares about: cursor, mouse release, file drops and shortcuts.
fn editor_event(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::CursorMoved(position)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::MouseReleased),
        Event::Window(window::Event::FileDropped(path)) => Some(Message::ImportPicked(vec![path])),
        Event::Window(window::Event::FileHovered(_)) => Some(Message::FileHover(true)),
        Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FileHover(false)),
        Event::Keyboard(keyboard::Event::KeyPressed { key: Key::Named(key::Named::Escape), .. }) if status == event::Status::Ignored => Some(Message::Escape),
        // Every other key goes through the keymap (`Message::Key`), so shortcuts can be changed.
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) if status == event::Status::Ignored => Some(Message::Key(key, modifiers)),
        _ => None,
    }
}

/// What an action does in the editor (`None` for the ones the app itself handles: files, settings).
fn action_message(action: crate::shortcuts::Action) -> Option<Message> {
    use crate::shortcuts::Action as A;
    Some(match action {
        A::NewProject | A::Open | A::Save | A::SaveAs | A::Settings | A::Shortcuts | A::Quit | A::PreviewFit | A::Preview100 | A::ToggleAssistant => return None,
        A::Import => Message::Import,
        A::Export => Message::OpenExport,
        A::RecordCamera => Message::OpenRecord,
        A::Undo => Message::Undo,
        A::Redo => Message::Redo,
        A::Cut => Message::Cut,
        A::Copy => Message::Copy,
        A::Paste => Message::Paste,
        A::PasteEffects => Message::PasteEffects,
        A::Duplicate => Message::DuplicateSelection,
        A::Delete => Message::DeleteSelection,
        A::RippleDelete => Message::RippleDelete,
        A::SelectAll => Message::SelectAll,
        A::SelectForward => Message::SelectForward,
        A::Group => Message::Group,
        A::Ungroup => Message::Ungroup,
        A::BringFront => Message::Arrange(true),
        A::SendBack => Message::Arrange(false),
        A::PlayPause => Message::TogglePlay,
        A::GoStart => Message::GoStart,
        A::GoEnd => Message::GoEnd,
        A::StepBack => Message::Step(-1),
        A::StepForward => Message::Step(1),
        A::StepBack10 => Message::Step(-10),
        A::StepForward10 => Message::Step(10),
        A::MutePreview => Message::ToggleMute,
        A::AddText => Message::AddText,
        A::Split => Message::SplitAtPlayhead,
        A::RangeTool => Message::RangeTool,
        A::RangeDone => Message::RangeDone,
        A::AddZoom => Message::AddZoomAtPlayhead,
        A::AddPress => Message::AddPressAtPlayhead,
        A::AddMarker => Message::AddMarker,
        A::DefaultTransition => Message::DefaultTransition,
        A::FreezeFrame => Message::FreezeFrame,
        A::ToggleDisabled => Message::ToggleDisabled,
        A::CloseGaps => Message::CloseGaps,
        A::ToggleSnapping => Message::ToggleSnapping,
        A::ZoomIn => Message::ZoomBy(1.4),
        A::ZoomOut => Message::ZoomBy(1.0 / 1.4),
        A::ZoomFit => Message::ZoomFit,
    })
}

/// "4.5 s" or "1:05", for messages about lengths of time.
fn fmt_seconds(s: f64) -> String {
    if s < 60.0 { format!("{:.1} s", s) } else { fmt_duration(s) }
}

fn tip_pos() -> iced::widget::tooltip::Position {
    iced::widget::tooltip::Position::Top
}

fn header_row<'a>(left: Element<'a, Message>, right: Element<'a, Message>) -> Element<'a, Message> {
    container(row![left, space().width(Fill), right].spacing(S1).align_y(Alignment::Center)).height(38).padding([0, 2]).center_y(38).into()
}

/// A toolbar button with an icon and a short label.
fn labeled_tool<'a>(glyph: char, label: &'a str, active: bool, on_press: Message) -> Element<'a, Message> {
    button(row![icon(glyph, 13), text(label).font(INTER_MEDIUM).size(T_SMALL)].spacing(6).align_y(Alignment::Center)).padding([5, 9]).style(style::toggle(active)).on_press(on_press).into()
}

fn tab<'a>(label: &'a str, active: bool, on_press: Message) -> Element<'a, Message> {
    button(text(label).font(INTER_MEDIUM).size(T_SMALL)).padding([5, 10]).style(style::segment(active)).on_press(on_press).into()
}

fn tab_icon<'a>(glyph: char, label: &'a str, active: bool, on_press: Message) -> Element<'a, Message> {
    let content = row![icon(glyph, 12), text(label).font(INTER_MEDIUM).size(T_SMALL)].spacing(5).align_y(Alignment::Center);
    button(content).padding([5, 10]).style(style::segment(active)).on_press(on_press).into()
}

fn play_button<'a>(playing: bool) -> Element<'a, Message> {
    button(container(icon(if playing { code::PAUSE } else { code::PLAY }, 15)).center(Length::Fill)).width(38).height(38).padding(0).style(style::primary).on_press(Message::TogglePlay).into()
}

/// Text with every digit in a fixed-width cell, so changing numbers don't jiggle.
fn tabular<'a>(s: &str, color: Color) -> Element<'a, Message> {
    row(s.chars().map(|c| {
        let w = if c.is_ascii_digit() { 8.2 } else { 5.0 };
        container(text(c.to_string()).font(INTER_MEDIUM).size(T_BODY).color(color)).width(w).center_x(w).into()
    }))
    .into()
}

pub fn fmt_timecode(t: f64) -> String {
    let total = t.max(0.0).floor() as u64;
    format!("{:02}:{:02}:{:02}", total / 3600, (total / 60) % 60, total % 60)
}

/// "0:03.2" style, for ranges.
pub fn fmt_timecode_short(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    format!("{m}:{:04.1}", t - m as f64 * 60.0)
}

fn fmt_timecode_frames(t: f64, fps: u32) -> String {
    let frames = (t.max(0.0) * fps as f64).round() as u64;
    let fps = fps.max(1) as u64;
    let secs = frames / fps;
    format!("{:02}:{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60, frames % fps)
}

fn fmt_ruler(t: f64) -> String {
    timeline::fmt_ruler(t, true)
}

fn fmt_duration(d: f64) -> String {
    let s = d.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Content, Rgba, SpanChange, TextLayer};
    use iced::widget::text_editor::{Action, Edit as Typed, Motion};

    fn editor_with_text(text: &str) -> (Editor, Id) {
        let mut p = Project::new("t", Default::default());
        let clip = p.add_text_clip(None, 0.0, 5.0, TextLayer { text: text.into(), ..Default::default() }).unwrap();
        let mut ed = Editor::new(p, None, Layout::default());
        ed.selection = vec![clip];
        let _ = ed.update(Message::SideTab(SideTab::Inspector));
        (ed, clip)
    }

    fn layer(ed: &Editor, clip: Id) -> TextLayer {
        match &ed.doc.project.clip(clip).unwrap().content {
            Content::Text(t) => t.clone(),
            _ => unreachable!(),
        }
    }

    fn draft(ed: &Editor) -> &iced::widget::text_editor::Content {
        &ed.text_draft.as_ref().expect("the text box follows the selected text clip").1
    }

    #[test]
    fn outro_defaults_to_60_percent_of_intro_duration() {
        let (mut ed, clip) = editor_with_text("x");
        let _ = ed.update(Message::ApplyMotion { intro: true, kind: crate::clip::MotionKind::Fade });
        ed.doc.project.clip_mut(clip).unwrap().intro.as_mut().unwrap().duration = 1.0;
        let _ = ed.update(Message::ApplyMotion { intro: false, kind: crate::clip::MotionKind::Fade });
        let outro = ed.doc.project.clip(clip).unwrap().outro.unwrap();
        assert!((outro.duration - 0.6).abs() < 1e-9, "outro should default to 60% of the intro duration, got {}", outro.duration);
    }

    #[test]
    fn the_range_tool_selects_by_dragging_and_done_clears_it() {
        let (mut ed, clip) = editor_with_text("x");
        ed.selection.clear();
        // Range on: the timeline header shows Done.
        let _ = ed.update(Message::RangeTool);
        assert!(ed.range_mode && ed.range.is_none());
        // Press on a clip and drag: the clip gets selected, the range follows the drag.
        let _ = ed.update(Message::RangePress { clip: Some(clip) });
        assert_eq!(ed.selection, [clip]);
        let _ = ed.update(Message::SetRange(Some((1.0, 3.0))));
        assert_eq!(ed.range, Some((1.0, 3.0)));
        // Adjusting an edge, or drawing again, replaces it.
        let _ = ed.update(Message::SetRange(Some((0.5, 3.0))));
        let _ = ed.update(Message::RangePress { clip: None });
        assert_eq!((ed.range, ed.selection.as_slice()), (None, [clip].as_slice()), "a press on empty space keeps the selection");
        let _ = ed.update(Message::SetRange(Some((4.0, 4.5))));
        // Done: the range is gone and the tool is off.
        let _ = ed.update(Message::RangeDone);
        assert!(!ed.range_mode && ed.range.is_none());
        // R toggles: on, then on again means Done. Esc also finishes.
        let _ = ed.update(Message::RangeTool);
        let _ = ed.update(Message::SetRange(Some((1.0, 2.0))));
        let _ = ed.update(Message::RangeTool);
        assert!(!ed.range_mode && ed.range.is_none());
        let _ = ed.update(Message::RangeTool);
        let _ = ed.update(Message::SetRange(Some((1.0, 2.0))));
        let _ = ed.update(Message::Escape);
        assert!(!ed.range_mode && ed.range.is_none());
        assert_eq!(ed.selection, [clip], "Esc ending the range doesn't also drop the selection");
        // Shift+drag on the ruler (a range without the tool) turns the tool on, so Done shows.
        let _ = ed.update(Message::SetRange(Some((2.0, 3.0))));
        assert!(ed.range_mode);
    }

    #[test]
    fn selected_words_get_their_own_style_and_keep_it_while_typing() {
        let (mut ed, clip) = editor_with_text("Make it fast");
        let _ = ed.update(Message::DevSelectText(8, 12));
        assert_eq!(inspector::selected_chars(draft(&ed)), Some((8, 12)));
        let _ = ed.update(Message::Edit(clip, inspector::Edit::SpanStyle(8, 12, SpanChange::Color(Some(Rgba::hex(0xD6FF00))))));
        let t = layer(&ed, clip);
        assert_eq!((t.spans.len(), t.spans[0].start, t.spans[0].end), (1, 8, 12));
        assert_eq!(inspector::selected_chars(draft(&ed)), Some((8, 12)), "styling keeps the selection");

        // Typing in the text box changes the clip, and the style moves with its word.
        let _ = ed.update(Message::TextDraft(clip, Action::Move(Motion::DocumentStart)));
        for c in "Now ".chars() {
            let _ = ed.update(Message::TextDraft(clip, Action::Edit(Typed::Insert(c))));
        }
        let t = layer(&ed, clip);
        assert_eq!(t.text, "Now Make it fast");
        assert_eq!((t.spans[0].start, t.spans[0].end), (12, 16));
        assert_eq!(draft(&ed).text(), t.text);

        // Undo puts the text back, and the box follows.
        let _ = ed.update(Message::Undo);
        let t = layer(&ed, clip);
        assert_eq!(t.text, "Make it fast");
        assert_eq!(draft(&ed).text(), "Make it fast");
        assert_eq!((t.spans[0].start, t.spans[0].end), (8, 12));

        // Another clip, or none: the box goes with the selection.
        ed.selection.clear();
        let _ = ed.update(Message::SideTab(SideTab::Inspector));
        assert!(ed.text_draft.is_none());
    }

    #[test]
    fn bring_to_front_from_the_menu_puts_the_text_over_everything() {
        let (mut ed, text) = editor_with_text("Hello");
        let top = ed.doc.project.tracks[0].id;
        let bottom = ed.doc.project.tracks.iter().filter(|t| t.kind == crate::model::TrackKind::Visual).last().unwrap().id;
        // The text on the bottom video track, and something else on the top one at the same time.
        ed.doc.project.move_clip(text, bottom, 0.0).unwrap();
        let card = ed.doc.project.add_text_clip(Some(top), 0.0, 5.0, TextLayer::default()).unwrap();
        assert_ne!(ed.doc.project.clip(text).unwrap().track, ed.doc.project.clip(card).unwrap().track);
        let _ = ed.update(Message::Arrange(true));
        assert_eq!(ed.doc.project.clip(text).unwrap().track, ed.doc.project.tracks[0].id, "on the new top track");
        let _ = ed.update(Message::Undo);
        assert_ne!(ed.doc.project.clip(text).unwrap().track, ed.doc.project.tracks[0].id, "one undo step");
    }

    #[test]
    fn double_click_word_selection_and_bangla_map_to_the_right_characters() {
        let (mut ed, _) = editor_with_text("নতুন অফার আজই");
        // Put the cursor inside the second word and select the word, as a double-click does.
        let _ = ed.update(Message::DevSelectText(0, 0));
        if let Some((_, c)) = &mut ed.text_draft {
            for _ in 0..6 {
                c.perform(Action::Move(Motion::Right));
            }
            c.perform(Action::SelectWord);
        }
        assert_eq!(draft(&ed).selection().as_deref(), Some("অফার"));
        assert_eq!(inspector::selected_chars(draft(&ed)), Some((5, 9)));
    }

    #[test]
    fn favorites_and_groups_of_the_effects_tab() {
        use library::{Effect, catalog};
        let (mut ed, _) = editor_with_text("x");
        // Every effect has its own key (favorites are saved by it).
        let keys: Vec<String> = catalog().iter().flat_map(|g| g.items.iter().map(|i| i.2.key())).collect();
        let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        // Starring adds in order, starring again removes.
        let (pop, fade) = (Effect::Outro(crate::clip::MotionKind::Pop), Effect::Intro(crate::clip::MotionKind::Fade));
        let _ = ed.update(Message::ToggleFavoriteEffect(pop));
        let _ = ed.update(Message::ToggleFavoriteEffect(fade));
        assert_eq!(ed.favorite_effects, [pop.key(), fade.key()]);
        let _ = ed.update(Message::ToggleFavoriteEffect(pop));
        assert_eq!(ed.favorite_effects, [fade.key()]);
        assert_eq!(pop.label(), "Animate out · Pop");
        // Groups start closed and open on click.
        assert!(ed.effects_open.is_empty());
        let _ = ed.update(Message::ToggleEffectGroup("Animate out"));
        assert!(ed.effects_open.contains("Animate out"));
        let _ = ed.update(Message::ToggleEffectGroup("Animate out"));
        assert!(ed.effects_open.is_empty());
    }

    #[test]
    fn frame_effects_and_zooms_from_the_effects_tab() {
        use library::{Effect, FrameStyle};
        let (mut ed, text) = editor_with_text("Title");
        let info = crate::media::ProbeInfo { kind: crate::model::MediaKind::Image, duration: 0.0, width: 800, height: 600, fps: 0.0, has_audio: false };
        let asset = ed.doc.project.add_asset(&std::env::current_exe().unwrap(), &info);
        let image = ed.doc.project.add_media_clip(asset, None, 0.0).unwrap();
        let look = |ed: &Editor| ed.doc.project.clip(image).unwrap().look.clone();

        // Click with the image and the text selected: the frame goes on the image only, as one step.
        ed.selection = vec![image, text];
        let _ = ed.apply_effect(Effect::Frame(FrameStyle::Corners), None);
        let _ = ed.apply_effect(Effect::Frame(FrameStyle::Border), None);
        let _ = ed.apply_effect(Effect::Frame(FrameStyle::Shadow), None);
        let l = look(&ed);
        assert!(l.corner_radius > 0.0 && l.border_width > 0.0 && l.shadow.enabled);
        assert_eq!(ed.doc.project.clip(text).unwrap().look, crate::clip::Look::default(), "text has no frame");
        // Applying corners again keeps the radius the user set.
        let _ = ed.update(Message::Edit(image, inspector::Edit::Radius(60.0)));
        let _ = ed.apply_effect(Effect::Frame(FrameStyle::Corners), None);
        assert_eq!(look(&ed).corner_radius, 60.0);
        // The inspector's remove buttons take each one off.
        let _ = ed.update(Message::Edit(image, inspector::Edit::Shadow(false)));
        assert!(!look(&ed).shadow.enabled);
        // Only text selected: nothing changes.
        ed.selection = vec![text];
        let before = ed.doc.project.clone();
        let _ = ed.apply_effect(Effect::Frame(FrameStyle::Shadow), None);
        assert_eq!(ed.doc.project, before);

        // A zoom dropped on the preview aims at the drop point.
        ed.playhead = 1.0;
        let _ = ed.apply_effect(Effect::Zoom, Some(DropTarget::Canvas { clip: Some(image), at: (0.25, 0.75) }));
        let z = ed.doc.project.clip(image).unwrap().zooms[0];
        assert!((z.focus_x - 0.25).abs() < 1e-6 && (z.focus_y - 0.75).abs() < 1e-6, "{z:?}");
        // Dropped on the timeline after it: a second zoom starts where it was dropped.
        let c = ed.doc.project.clip(image).unwrap().clone();
        let t = c.start + z.at + z.duration + 0.6;
        assert!(t < c.end());
        let _ = ed.apply_effect(Effect::Zoom, Some(DropTarget::Timeline { clip: Some(image), track: None, t }));
        let zooms = &ed.doc.project.clip(image).unwrap().zooms;
        assert!(zooms.len() == 2 && zooms.iter().any(|z| (z.at - (t - c.start)).abs() < 1e-6), "{zooms:?}");
        // Dropped on that zoom: it opens it instead of stacking another.
        let _ = ed.apply_effect(Effect::Zoom, Some(DropTarget::Timeline { clip: Some(image), track: None, t }));
        assert_eq!(ed.doc.project.clip(image).unwrap().zooms.len(), 2);
    }

    #[test]
    fn add_counter_inserts_a_default_clip_and_renders() {
        let (mut ed, _) = editor_with_text("x");
        ed.playhead = 0.0;
        let _ = ed.update(Message::AddCounter);
        let [id] = ed.selection[..] else { panic!("the new counter should be selected") };
        let clip = ed.doc.project.clip(id).unwrap();
        assert!(matches!(&clip.content, Content::Counter(c) if c.from == 0.0 && c.to == 100.0), "{:?}", clip.content);
        let project = ed.doc.project.clone();
        let scene = scene::evaluate(&project, 0.5, (1920, 1080), &ed.frames.bind(&project), false);
        assert!(scene.layers.iter().any(|l| l.clip == id), "the counter renders a layer");
    }

    #[test]
    fn camera_presets_insert_named_zooms_at_the_playhead() {
        use library::{CameraPreset, Effect};
        let (mut ed, _) = editor_with_text("x");
        let info = crate::media::ProbeInfo { kind: crate::model::MediaKind::Image, duration: 0.0, width: 800, height: 600, fps: 0.0, has_audio: false };
        let asset = ed.doc.project.add_asset(&std::env::current_exe().unwrap(), &info);
        let image = ed.doc.project.add_media_clip(asset, None, 0.0).unwrap();

        ed.selection = vec![image];
        ed.playhead = 0.0;
        let _ = ed.apply_effect(Effect::Camera(CameraPreset::PushIn), None);
        let zooms = ed.doc.project.clip(image).unwrap().zooms.clone();
        assert_eq!(zooms.len(), 1, "{zooms:?}");
        assert!((zooms[0].scale - 1.15).abs() < 1e-6);
        let clip_duration = ed.doc.project.clip(image).unwrap().duration;
        assert!(zooms[0].at + zooms[0].duration > clip_duration, "the hold should reach to the end of the clip");

        // A second clip for Pull back: two adjacent zooms, starting punched in and ending at 1.0×.
        let image2 = ed.doc.project.add_media_clip(asset, None, 6.0).unwrap();
        ed.selection = vec![image2];
        ed.playhead = 6.0;
        let _ = ed.apply_effect(Effect::Camera(CameraPreset::PullBack), None);
        let zooms = ed.doc.project.clip(image2).unwrap().zooms.clone();
        assert_eq!(zooms.len(), 2, "{zooms:?}");
        assert!((zooms[0].scale - 1.3).abs() < 1e-6, "starts punched in");
        assert!((zooms[1].scale - 1.0).abs() < 1e-6, "pulls back to normal");
        assert!((zooms[1].at - (zooms[0].at + zooms[0].duration)).abs() < 1e-6, "the two zooms are back-to-back");
    }

    #[test]
    fn color_presets_apply_from_the_inspector() {
        use crate::clip::COLOR_PRESETS;
        // A short list: every look has to earn its place.
        assert!(COLOR_PRESETS.len() <= 9);

        let (mut ed, text) = editor_with_text("Title");
        ed.selection = vec![text];
        let cinematic = COLOR_PRESETS.iter().find(|p| p.name == "Cinematic").unwrap();
        let _ = ed.update(Message::Edit(text, inspector::Edit::GradePreset(cinematic.grade)));
        assert_eq!(ed.doc.project.clip(text).unwrap().grade, cinematic.grade);
        // One preview picture per look, made as soon as a clip with a picture is selected.
        assert_eq!(ed.preset_tiles.as_ref().map(|(_, t)| t.len()), Some(COLOR_PRESETS.len()));

        // "Original" clears it back to the untouched default.
        let original = COLOR_PRESETS.iter().find(|p| p.name == "Original").unwrap();
        let _ = ed.update(Message::Edit(text, inspector::Edit::GradePreset(original.grade)));
        assert_eq!(ed.doc.project.clip(text).unwrap().grade, crate::clip::Grade::default());
    }

    fn press(c: &str, modifiers: keyboard::Modifiers) -> Message {
        Message::Key(Key::Character(c.into()), modifiers)
    }

    #[test]
    fn editing_shortcuts_go_through_the_keymap() {
        let (ctrl, none) = (keyboard::Modifiers::CTRL, keyboard::Modifiers::empty());
        let (mut ed, text) = editor_with_text("Title");
        ed.selection.clear();
        // Ctrl+A selects everything; Ctrl+C then Ctrl+V pastes a copy at the playhead and selects it.
        let _ = ed.update(press("a", ctrl));
        assert_eq!(ed.selection, [text]);
        let _ = ed.update(press("c", ctrl));
        ed.playhead = 10.0;
        let _ = ed.update(press("v", ctrl));
        assert_eq!(ed.doc.project.clips.len(), 2);
        let [copy] = ed.selection[..] else { panic!("the pasted clip is selected") };
        assert!(copy != text && ed.doc.project.clip(copy).unwrap().start == 10.0);
        // Ctrl+G groups them: selecting one and pressing Delete removes both; one undo brings both back.
        let _ = ed.update(Message::SelectAll);
        let _ = ed.update(press("g", ctrl));
        assert!(ed.doc.project.clip(text).unwrap().group.is_some());
        assert_eq!(ed.doc.project.clip(text).unwrap().group, ed.doc.project.clip(copy).unwrap().group);
        ed.selection = vec![text];
        let _ = ed.update(Message::Key(Key::Named(key::Named::Delete), none));
        assert!(ed.doc.project.clips.is_empty() && ed.selection.is_empty());
        let _ = ed.update(press("z", ctrl));
        assert_eq!(ed.doc.project.clips.len(), 2);
        // Ctrl+Shift+G ungroups; Ctrl+X cuts just the selected one, Ctrl+V brings it back.
        ed.selection = vec![copy];
        let _ = ed.update(press("G", ctrl | keyboard::Modifiers::SHIFT));
        assert!(ed.doc.project.clip(text).unwrap().group.is_none());
        let _ = ed.update(press("x", ctrl));
        assert!(ed.doc.project.clip(copy).is_none() && ed.doc.project.clip(text).is_some());
        let _ = ed.update(press("v", ctrl));
        assert_eq!(ed.doc.project.clips.len(), 2);
        // A shortcut the user changed: Q adds text now, T no longer does.
        let mut prefs = crate::settings::Prefs::default();
        prefs.keys.assign(crate::shortcuts::Action::AddText, "Q".into());
        ed.apply_prefs(&prefs, false);
        let _ = ed.update(press("t", none));
        assert_eq!(ed.doc.project.clips.len(), 2);
        let _ = ed.update(press("q", none));
        assert_eq!(ed.doc.project.clips.len(), 3);
        // While the export dialog is open, keys don't edit the project behind it.
        let _ = ed.update(Message::OpenExport);
        let _ = ed.update(press("q", none));
        assert_eq!(ed.doc.project.clips.len(), 3);
    }

    #[test]
    fn ripple_delete_close_gaps_and_cutting_a_range() {
        let (mut ed, a) = editor_with_text("a");
        let track = ed.doc.project.clip(a).unwrap().track;
        ed.doc.project.trim(a, Edge::End, 2.0).unwrap();
        let b = ed.doc.project.add_text_clip(Some(track), 2.0, 2.0, TextLayer::default()).unwrap();
        let c = ed.doc.project.add_text_clip(Some(track), 6.0, 2.0, TextLayer::default()).unwrap();
        let start = |ed: &Editor, id: Id| ed.doc.project.clip(id).unwrap().start;
        // Ripple delete: the clip goes and what follows on its track closes up.
        ed.selection = vec![b];
        let _ = ed.update(Message::RippleDelete);
        assert!(ed.doc.project.clip(b).is_none() && ed.selection.is_empty());
        assert_eq!(start(&ed, c), 4.0);
        // Close gaps with nothing selected: the empty time between them goes.
        let _ = ed.update(Message::CloseGaps);
        assert_eq!(start(&ed, c), 2.0);
        let _ = ed.update(Message::Undo);
        assert_eq!(start(&ed, c), 4.0);
        // With a clip selected: the gaps on its track.
        ed.selection = vec![a];
        let _ = ed.update(Message::CloseGaps);
        assert_eq!(start(&ed, c), 2.0);
        // A marked range is cut out of the timeline; the playhead lands where it was.
        let _ = ed.update(Message::SetRange(Some((0.5, 1.5))));
        let _ = ed.update(Message::RippleDelete);
        assert!(ed.range.is_none() && !ed.range_mode);
        assert_eq!(ed.playhead, 0.5);
        assert_eq!(start(&ed, c), 1.0);
        assert!((ed.doc.project.duration() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn transitions_freeze_frames_and_disabling_from_the_menu() {
        let (mut ed, _) = editor_with_text("x");
        let info = crate::media::ProbeInfo { kind: MediaKind::Video, duration: 10.0, width: 1920, height: 1080, fps: 30.0, has_audio: true };
        let asset = ed.doc.project.add_asset(&PathBuf::from("/clip.mp4"), &info);
        let first = ed.doc.project.add_media_clip(asset, None, 0.0).unwrap();
        let track = ed.doc.project.clip(first).unwrap().track;
        let second = ed.doc.project.add_media_clip(asset, Some(track), 10.0).unwrap();
        // With the first clip selected, the cut after it gets the transition.
        ed.selection = vec![first];
        let _ = ed.update(Message::DefaultTransition);
        assert!(ed.doc.project.clip(second).unwrap().transition.is_some());
        assert_eq!(ed.selection, [second]);
        let _ = ed.update(Message::Undo);
        // With nothing selected, the cut nearest the playhead.
        ed.selection.clear();
        ed.playhead = 9.5;
        let _ = ed.update(Message::DefaultTransition);
        assert!(ed.doc.project.clip(second).unwrap().transition.is_some());
        let _ = ed.update(Message::Undo);
        // Freeze frame: a 2 s still appears at the playhead and is selected.
        ed.selection = vec![first];
        ed.playhead = 4.0;
        let _ = ed.update(Message::FreezeFrame);
        let [still] = ed.selection[..] else { panic!("the still is selected") };
        let s = ed.doc.project.clip(still).unwrap();
        assert!(s.freeze && s.start == 4.0 && s.duration == 2.0);
        assert_eq!(ed.doc.project.clip(second).unwrap().start, 12.0);
        // Disabled clips aren't drawn (and enabling brings them back).
        let project = ed.doc.project.clone();
        let layers = |ed: &Editor, p: &Project| scene::evaluate(p, 5.0, (1920, 1080), &ed.frames.bind(p), false).layers.iter().filter(|l| l.clip == still).count();
        assert_eq!(layers(&ed, &project), 1);
        let _ = ed.update(Message::ToggleDisabled);
        let project = ed.doc.project.clone();
        assert!(project.clip(still).unwrap().disabled);
        assert_eq!(layers(&ed, &project), 0);
        let _ = ed.update(Message::ToggleDisabled);
        assert!(!ed.doc.project.clip(still).unwrap().disabled);
    }

    #[test]
    fn dragging_one_clip_of_a_selection_or_group_moves_all_of_it() {
        let (mut ed, a) = editor_with_text("a");
        let track = ed.doc.project.clip(a).unwrap().track;
        let b = ed.doc.project.add_text_clip(Some(track), 8.0, 2.0, TextLayer::default()).unwrap();
        let c = ed.doc.project.add_text_clip(Some(track), 20.0, 2.0, TextLayer::default()).unwrap();
        let start = |ed: &Editor, id: Id| ed.doc.project.clip(id).unwrap().start;
        // Two selected: dragging one moves both. The third stays.
        ed.selection = vec![a, b];
        let _ = ed.update(Message::MoveClip { clip: a, track, start: 1.0 });
        assert_eq!((start(&ed, a), start(&ed, b), start(&ed, c)), (1.0, 9.0, 20.0));
        // Grouped: dragging one moves the group even when only that one is selected,
        // and trimming an edge they share trims both.
        let _ = ed.update(Message::Group);
        ed.selection = vec![b];
        let _ = ed.update(Message::MoveClip { clip: b, track, start: 8.0 });
        assert_eq!((start(&ed, a), start(&ed, b)), (0.0, 8.0));
        // An unselected, ungrouped clip dragged while others are selected moves alone.
        let _ = ed.update(Message::MoveClip { clip: c, track, start: 30.0 });
        assert_eq!((start(&ed, a), start(&ed, b), start(&ed, c)), (0.0, 8.0, 30.0));
    }
}
