//! Top-level state: which screen is showing, window state, settings and project files.

use std::path::{Path, PathBuf};

use iced::keyboard::{self, Key, key};
use iced::widget::{button, column, container, mouse_area, row, space, text, text_input};
use iced::window;
use iced::{Alignment, Element, Event, Fill, Subscription, Task, event};
use iced_aw::DropDown;

use crate::doc::Doc;
use crate::editor::{self, Editor, SideTab};
use crate::model::{FILE_EXTENSION, Project};
use crate::settings::Settings;
use crate::start::{self, Start};
use crate::ui::chrome::{self, WindowAction};
use crate::ui::icons::{code, icon};
use crate::ui::tokens::*;
use crate::ui::widgets::{self, MenuItem};
use crate::ui::style;

enum Screen {
    Start(Start),
    Editor(Box<Editor>),
}

/// An action that was about to discard unsaved changes; held while the user
/// is asked whether to save, discard or cancel (see `App::confirm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    Quit,
    NewProject,
    CloseProject,
    Open,
}

#[derive(Debug, Clone)]
pub enum Message {
    Window(WindowAction),
    WindowResized,
    WindowSize(iced::Size),
    Maximized(bool),
    Start(start::Message),
    Editor(editor::Message),
    AppMenu(bool),
    FormatMenu(bool),
    ZoomMenu(bool),
    RecordMenu(bool),
    /// Record from the title bar menu: closes the menu, then opens that recorder.
    PickRecord(RecordKind),
    Autosave,
    NewProject,
    Open,
    OpenPicked(Option<PathBuf>),
    Save,
    SaveAs,
    SavePicked(Option<PathBuf>),
    CloseProject,
    ToggleAssistant,
    Quit,
    Mcp(crate::mcp::Call),
    /// Debug builds: `STUDIO_SNAPSHOT=out.png` saves a picture of the window (only this app's pixels).
    Snapshot(window::Screenshot),
    Ai(crate::ai_connect::Message),
    OpenAi,
    AiChecked(crate::settings::AiProvider, Result<String, String>),
    /// The ChatGPT sign-in in the browser finished (or failed).
    AiSignedIn(Result<(), String>),
    /// Media dropped on the start screen: make a project from it.
    StartDrop(PathBuf),
    OpenSettings,
    Settings(crate::settings_dialog::Message),
    OpenShortcuts,
    Shortcuts(crate::shortcuts_dialog::Message),
    /// A key press no widget used: app-level shortcuts, or the new keys of a shortcut being changed.
    Key(Key, keyboard::Modifiers),
    /// Preview zoom from the toolbar menu (None = fit).
    PreviewZoom(Option<f32>),
    /// Save the current project, then continue the action that was waiting on it.
    ConfirmSave,
    /// Discard unsaved changes, then continue the action that was waiting on it.
    ConfirmDiscard,
    /// Keep working; abandon whatever action was waiting on unsaved changes.
    ConfirmCancel,
    DismissError,
    /// Debug builds: `STUDIO_SNAPSHOT_OPEN=confirm` opens the unsaved-changes dialog.
    DevConfirm,
    /// A new frame while a dialog eases in: nothing to do but draw again.
    Frame,
    /// Where the window was before it became the recording bar, and whether it was maximized.
    BarSaved(Option<iced::Point>, bool),
}

/// The smallest the window can be made (the recording bar aside).
pub const MIN_WINDOW_SIZE: iced::Size = iced::Size::new(1100.0, 680.0);
/// The window's size when it was maximized before the bar and has to be given one first.
const DEFAULT_WINDOW_SIZE: iced::Size = iced::Size::new(1480.0, 920.0);
/// How far the bar sits above the bottom of where the window was (clear of the taskbar), in screen points.
const BAR_LIFT: f32 = 72.0;

/// The window while a screen recording runs: shrunk into a small always-on-top bar so it is out of
/// the way of what is being recorded, then put back exactly where and how it was.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bar {
    Off,
    /// Asking where the window is before shrinking it.
    Entering,
    On { size: iced::Size, position: Option<iced::Point>, maximized: bool },
}

/// What the title bar's Record menu can record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordKind {
    Camera,
    Screen,
}

pub struct App {
    settings: Settings,
    screen: Screen,
    maximized: bool,
    app_menu: bool,
    format_menu: bool,
    zoom_menu: bool,
    record_menu: bool,
    ai_dialog: bool,
    settings_dialog: bool,
    shortcuts_dialog: bool,
    /// The action whose shortcut is being changed: the next key press becomes its keys.
    shortcuts_capture: Option<crate::shortcuts::Action>,
    /// The AI being checked in the Connect AI dialog.
    ai_checking: Option<crate::settings::AiProvider>,
    /// Waiting for the ChatGPT sign-in in the browser.
    ai_signing_in: bool,
    ai_detail: Option<(crate::settings::AiProvider, Result<String, String>)>,
    error: Option<String>,
    window_size: iced::Size,
    /// Set when New/Close/Open/Quit would discard unsaved changes; shows the confirm dialog.
    confirm: Option<PendingAction>,
    /// Eases the open dialog in.
    modal: crate::ui::motion::Appear,
    bar: Bar,
}

impl App {
    pub fn new() -> (App, Task<Message>) {
        if let Err(e) = crate::mcp::start() {
            eprintln!("assistant server: {e}");
        }
        let error = (!crate::ffmpeg::is_available()).then(|| format!("FFmpeg was not found, so media can't be imported or previewed. {}", crate::ffmpeg::missing_hint()));
        let app = App { settings: Settings::load(), screen: Screen::Start(Start::default()), maximized: false, app_menu: false, format_menu: false, zoom_menu: false, record_menu: false, ai_dialog: false, settings_dialog: false, shortcuts_dialog: false, shortcuts_capture: None, ai_checking: None, ai_signing_in: false, ai_detail: None, error, window_size: iced::Size::ZERO, confirm: None, modal: Default::default(), bar: Bar::Off };
        let path = std::env::args().nth(1).map(PathBuf::from).filter(|p| p.is_file());
        let task = match path {
            Some(p) => Task::done(Message::OpenPicked(Some(p))),
            None => Task::none(),
        };
        (app, task.chain(snapshot_task()))
    }

    /// The interface size from Settings.
    pub fn scale_factor(&self) -> f32 {
        self.settings.prefs.sane().ui_scale
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::Start(_) => "AI Video Studio".into(),
            Screen::Editor(e) => format!("{}{} — AI Video Studio", e.project().name, if e.doc.is_dirty() { " •" } else { "" }),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let editor = match &self.screen {
            Screen::Editor(e) => {
                // Save automatically a moment after each change.
                let autosave = if e.doc.is_dirty() && e.path.is_some() { iced::time::every(std::time::Duration::from_secs(self.settings.prefs.sane().autosave_seconds)).map(|_| Message::Autosave) } else { Subscription::none() };
                Subscription::batch([e.subscription().map(Message::Editor), autosave])
            }
            Screen::Start(_) => event::listen_with(|event, _, _| match event {
                Event::Window(window::Event::FileDropped(p)) => Some(Message::StartDrop(p)),
                _ => None,
            }),
        };
        let frames = if self.modal.running() { window::frames().map(|_| Message::Frame) } else { Subscription::none() };
        Subscription::batch([window::resize_events().map(|(_, size)| Message::WindowSize(size)), event::listen_with(shortcut), editor, crate::mcp::subscription().map(Message::Mcp), frames])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_inner(message);
        // Whichever dialog is open now eases in from the moment it opened.
        let open = if self.confirm.is_some() {
            Some(4)
        } else if self.shortcuts_dialog {
            Some(3)
        } else if self.ai_dialog {
            Some(2)
        } else if self.settings_dialog {
            Some(1)
        } else {
            None
        };
        self.modal.track(open);
        Task::batch([task, self.sync_bar()])
    }

    /// Turns the window into the recording bar when a screen take starts, and back when it ends.
    fn sync_bar(&mut self) -> Task<Message> {
        let want = matches!(&self.screen, Screen::Editor(e) if e.recording_bar());
        match (self.bar, want) {
            (Bar::Off, true) => {
                self.bar = Bar::Entering;
                window::latest().and_then(|id| window::is_maximized(id).then(move |maximized| window::position(id).map(move |position| Message::BarSaved(position, maximized))))
            }
            (Bar::On { size, position, maximized }, false) => {
                self.bar = Bar::Off;
                window::latest().and_then(move |id| {
                    let mut steps = vec![window::set_level(id, window::Level::Normal), window::set_min_size(id, Some(MIN_WINDOW_SIZE)), window::resize(id, if maximized { DEFAULT_WINDOW_SIZE } else { size })];
                    steps.extend(position.map(|p| window::move_to(id, p)));
                    if maximized {
                        steps.push(window::maximize(id, true));
                    }
                    steps.push(window::gain_focus(id));
                    Task::batch(steps).chain(Task::done(Message::WindowResized))
                })
            }
            _ => Task::none(),
        }
    }

    /// The window shrunk into the bar: bottom centre of where it was, above everything else.
    fn shrink_to_bar(&self, position: Option<iced::Point>) -> Task<Message> {
        let (w, h) = editor::screen_dialog::BAR_SIZE;
        // Sizes are in the app's points, positions in the screen's: the interface scale is between them.
        let u = self.scale_factor();
        let at = position.map(|p| iced::Point::new(p.x + (self.window_size.width - w) * u / 2.0, p.y + (self.window_size.height - h) * u - BAR_LIFT));
        window::latest().and_then(move |id| {
            let mut steps = vec![window::maximize(id, false), window::set_min_size(id, None), window::resize(id, iced::Size::new(w, h))];
            steps.extend(at.map(|p| window::move_to(id, p)));
            steps.push(window::set_level(id, window::Level::AlwaysOnTop));
            Task::batch(steps)
        })
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        // Closing or replacing the editor would cut a camera recording off: it takes Stop first.
        if matches!(message, Message::Quit | Message::NewProject | Message::Open | Message::OpenPicked(Some(_)) | Message::CloseProject) {
            if let Screen::Editor(e) = &mut self.screen {
                if e.refuse_while_recording() {
                    self.app_menu = false;
                    return Task::none();
                }
            }
        }
        match message {
            Message::Window(action) => return window_task(action).chain(Task::done(Message::WindowResized)),
            Message::WindowResized => return window::latest().and_then(window::is_maximized).map(Message::Maximized),
            Message::Maximized(m) => self.maximized = m,
            Message::BarSaved(position, maximized) => {
                let want = matches!(&self.screen, Screen::Editor(e) if e.recording_bar());
                if self.bar == Bar::Entering {
                    if !want {
                        // The take ended before the window was touched: nothing to put back.
                        self.bar = Bar::Off;
                        return Task::none();
                    }
                    self.bar = Bar::On { size: self.window_size, position, maximized };
                    return self.shrink_to_bar(position);
                }
            }
            Message::WindowSize(size) => {
                // (The editor keeps its layout while the window is the recording bar.)
                if self.bar != Bar::Off {
                    return Task::none();
                }
                self.window_size = size;
                if let Screen::Editor(e) = &mut self.screen {
                    let _ = e.update(editor::Message::WindowSize(size));
                }
                return window::latest().and_then(window::is_maximized).map(Message::Maximized);
            }
            Message::Start(msg) => {
                if let Screen::Start(start) = &mut self.screen {
                    start.update(&msg);
                }
                match msg {
                    start::Message::Create => return self.create_project(),
                    start::Message::TryExample => return self.create_example_project(),
                    start::Message::Open => return self.update(Message::Open),
                    start::Message::OpenRecent(p) => return self.open_project(&p),
                    start::Message::ForgetRecent(p) => {
                        self.settings.recent.retain(|r| r.path != p);
                        self.settings.save();
                    }
                    _ => {}
                }
            }
            Message::Editor(editor::Message::OpenAiConnect) | Message::OpenAi => self.ai_dialog = true,
            // The Connect AI dialog (and friends) are modal: shortcuts don't edit the project behind them.
            Message::Editor(ref m) if (self.ai_dialog || self.settings_dialog || self.shortcuts_dialog || self.confirm.is_some()) && editor::blocked_by_dialog(m) => {}
            Message::Editor(editor::Message::Escape) if self.ai_dialog => self.ai_dialog = false,
            Message::Editor(editor::Message::Escape) if self.settings_dialog => self.settings_dialog = false,
            Message::Editor(editor::Message::Escape) if self.shortcuts_capture.is_some() => self.shortcuts_capture = None,
            Message::Editor(editor::Message::Escape) if self.shortcuts_dialog => self.shortcuts_dialog = false,
            Message::Editor(editor::Message::Escape) if self.confirm.is_some() => self.confirm = None,
            Message::OpenShortcuts => {
                self.app_menu = false;
                self.shortcuts_dialog = true;
            }
            Message::Shortcuts(m) => {
                use crate::shortcuts_dialog::Message as M;
                let keys = &mut self.settings.prefs.keys;
                match m {
                    M::Close => (self.shortcuts_dialog, self.shortcuts_capture) = (false, None),
                    M::Rebind(action) => self.shortcuts_capture = if self.shortcuts_capture == Some(action) { None } else { Some(action) },
                    M::Reset(action) => keys.reset(action),
                    M::ResetAll => keys.reset_all(),
                }
                if matches!(m, M::Reset(_) | M::ResetAll) {
                    self.shortcuts_capture = None;
                    self.keys_changed();
                }
            }
            Message::Key(key, modifiers) => return self.key_pressed(key, modifiers),
            Message::OpenSettings => {
                self.app_menu = false;
                self.settings_dialog = true;
            }
            Message::Settings(crate::settings_dialog::Message::Close) => self.settings_dialog = false,
            Message::Settings(crate::settings_dialog::Message::BrowseFolder) => {
                let dir = self.settings.projects_dir();
                return Task::perform(
                    async move { rfd::AsyncFileDialog::new().set_title("Folder for new projects").set_directory(dir).pick_folder().await.map(|h| h.path().to_path_buf()) },
                    |p| Message::Settings(crate::settings_dialog::Message::FolderPicked(p)),
                );
            }
            Message::Settings(m) => {
                if crate::settings_dialog::apply(&mut self.settings.prefs, &m) {
                    self.settings.prefs = self.settings.prefs.sane();
                    self.settings.save();
                    if let Screen::Editor(e) = &mut self.screen {
                        e.apply_prefs(&self.settings.prefs, false);
                    }
                }
            }
            Message::Ai(msg) => match msg {
                crate::ai_connect::Message::Close => self.ai_dialog = false,
                crate::ai_connect::Message::Connect(provider) => {
                    self.ai_checking = Some(provider);
                    self.ai_detail = None;
                    let check = match provider {
                        crate::settings::AiProvider::ClaudeCode => crate::assistant::claude_version,
                        crate::settings::AiProvider::Codex => crate::assistant::codex_status,
                    };
                    return Task::perform(async move { tokio::task::spawn_blocking(check).await.unwrap_or_else(|e| Err(e.to_string())) }, move |r| Message::AiChecked(provider, r));
                }
                crate::ai_connect::Message::SignIn => {
                    self.ai_signing_in = true;
                    return Task::perform(async { tokio::task::spawn_blocking(crate::assistant::codex_sign_in).await.unwrap_or_else(|e| Err(e.to_string())) }, Message::AiSignedIn);
                }
                crate::ai_connect::Message::Disconnect => {
                    self.settings.ai = None;
                    self.settings.save();
                    self.ai_detail = None;
                    if let Screen::Editor(e) = &mut self.screen {
                        e.set_ai(None);
                    }
                }
                crate::ai_connect::Message::CopyUrl => return iced::clipboard::write(crate::mcp::url()),
                crate::ai_connect::Message::CopySetup => return iced::clipboard::write(crate::ai_connect::setup_snippet(&crate::mcp::url())),
                crate::ai_connect::Message::CopyInstall => return iced::clipboard::write(crate::assistant::CODEX_INSTALL.to_string()),
            },
            Message::StartDrop(path) => {
                if !crate::media::is_media_path(&path) {
                    self.error = Some("That file type can't be imported".into());
                    return Task::none();
                }
                if let Screen::Start(start) = &mut self.screen {
                    if let Some(stem) = path.file_stem() {
                        start.name = stem.to_string_lossy().into_owned();
                    }
                }
                let open = self.create_project();
                return open.chain(Task::done(Message::Editor(editor::Message::ImportPicked(vec![path]))));
            }
            // Signed in: check again, which connects it.
            Message::AiSignedIn(Ok(())) => {
                self.ai_signing_in = false;
                return self.update(Message::Ai(crate::ai_connect::Message::Connect(crate::settings::AiProvider::Codex)));
            }
            Message::AiSignedIn(Err(e)) => {
                self.ai_signing_in = false;
                self.ai_detail = Some((crate::settings::AiProvider::Codex, Err(e)));
            }
            Message::AiChecked(provider, result) => {
                if self.ai_checking == Some(provider) {
                    self.ai_checking = None;
                }
                if result.is_ok() {
                    self.settings.ai = Some(provider);
                    self.settings.save();
                    if let Screen::Editor(e) = &mut self.screen {
                        e.set_ai(Some(provider));
                    }
                }
                self.ai_detail = Some((provider, result));
            }
            Message::Editor(msg) => {
                if let Screen::Editor(e) = &mut self.screen {
                    let resized = matches!(msg, editor::Message::PaneResized(_));
                    let task = e.update(msg).map(Message::Editor);
                    if resized {
                        self.settings.layout = e.layout();
                    }
                    return task;
                }
            }
            Message::AppMenu(open) => self.app_menu = open,
            Message::FormatMenu(open) => self.format_menu = open,
            Message::ZoomMenu(open) => self.zoom_menu = open,
            Message::RecordMenu(open) => self.record_menu = open,
            Message::PickRecord(kind) => {
                self.record_menu = false;
                if let Screen::Editor(e) = &mut self.screen {
                    // The task must be run: it lists the cameras / screens and microphones. (Dropped,
                    // the dialog opened and then waited at "Looking for…" for ever.)
                    return e
                        .update(match kind {
                            RecordKind::Camera => editor::Message::OpenRecord,
                            RecordKind::Screen => editor::Message::OpenScreenRecord,
                        })
                        .map(Message::Editor);
                }
            }
            Message::PreviewZoom(z) => {
                self.zoom_menu = false;
                if let Screen::Editor(e) = &mut self.screen {
                    return e.update(editor::Message::PreviewZoom(z)).map(Message::Editor);
                }
            }
            Message::Autosave => {
                if let Screen::Editor(e) = &mut self.screen {
                    if let (true, Some(path)) = (e.doc.is_dirty(), e.path.clone()) {
                        match e.doc.project.save(&path) {
                            Ok(()) => e.doc.mark_saved(),
                            Err(err) => self.error = Some(format!("Autosave failed: {err}")),
                        }
                    }
                }
            }
            Message::NewProject => {
                self.app_menu = false;
                if self.is_dirty() {
                    self.confirm = Some(PendingAction::NewProject);
                } else {
                    self.close_project();
                }
            }
            Message::Open => {
                self.app_menu = false;
                if self.is_dirty() {
                    self.confirm = Some(PendingAction::Open);
                    return Task::none();
                }
                return pick_project_file();
            }
            Message::OpenPicked(Some(p)) => return self.open_project(&p),
            Message::OpenPicked(None) | Message::SavePicked(None) => {}
            Message::Save => {
                self.app_menu = false;
                let has_path = matches!(&self.screen, Screen::Editor(e) if e.path.is_some());
                if has_path {
                    self.save_editor(None);
                } else if matches!(self.screen, Screen::Editor(_)) {
                    return self.update(Message::SaveAs);
                }
            }
            Message::SaveAs => {
                self.app_menu = false;
                let Screen::Editor(e) = &self.screen else { return Task::none() };
                let name = format!("{}.{FILE_EXTENSION}", e.project().name);
                let dir = self.settings.projects_dir();
                return Task::perform(
                    async move {
                        let _ = std::fs::create_dir_all(&dir);
                        rfd::AsyncFileDialog::new()
                            .set_title("Save project")
                            .set_directory(&dir)
                            .set_file_name(name)
                            .add_filter("AI Video Editor project", &[FILE_EXTENSION])
                            .save_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::SavePicked,
                );
            }
            Message::SavePicked(Some(p)) => self.save_editor(Some(p.with_extension(FILE_EXTENSION))),
            Message::CloseProject => {
                self.app_menu = false;
                if self.is_dirty() {
                    self.confirm = Some(PendingAction::CloseProject);
                } else {
                    self.close_project();
                }
            }
            Message::ToggleAssistant => {
                if let Screen::Editor(e) = &mut self.screen {
                    let tab = if e.side_tab == SideTab::Assistant { SideTab::Inspector } else { SideTab::Assistant };
                    return e.update(editor::Message::SideTab(tab)).map(Message::Editor);
                }
            }
            Message::Mcp(call) => match &mut self.screen {
                Screen::Editor(e) => return e.update(editor::Message::AiTool(call)).map(Message::Editor),
                Screen::Start(_) => call.answer(Err("No project is open. Ask the user to create or open a project first.".into())),
            },
            Message::Snapshot(shot) => {
                if let Ok(path) = std::env::var("STUDIO_SNAPSHOT") {
                    let _ = image::save_buffer(&path, &shot.rgba, shot.size.width, shot.size.height, image::ExtendedColorType::Rgba8);
                    return iced::exit();
                }
            }
            Message::Quit => {
                self.app_menu = false;
                if self.is_dirty() {
                    self.confirm = Some(PendingAction::Quit);
                    return Task::none();
                }
                self.persist();
                if let Screen::Editor(e) = &mut self.screen {
                    e.shutdown();
                }
                return window::latest().and_then(window::close);
            }
            Message::ConfirmCancel => self.confirm = None,
            Message::ConfirmDiscard => {
                if let Some(action) = self.confirm.take() {
                    self.discard_changes();
                    return self.run_pending(action);
                }
            }
            Message::ConfirmSave => {
                if let Some(action) = self.confirm.take() {
                    let Screen::Editor(e) = &mut self.screen else { return Task::none() };
                    match e.path.clone() {
                        Some(path) => match e.doc.project.save(&path) {
                            Ok(()) => {
                                e.doc.mark_saved();
                                self.settings.touch_recent(&e.doc.project.name, &path, e.doc.project.canvas.summary());
                                self.settings.save();
                                return self.run_pending(action);
                            }
                            Err(err) => {
                                self.error = Some(format!("Could not save: {err}"));
                                // Keep the dialog open so the user can retry or discard instead.
                                self.confirm = Some(action);
                            }
                        },
                        // Never saved before: let them pick where, then they can try again.
                        None => return self.update(Message::SaveAs),
                    }
                }
            }
            Message::DismissError => self.error = None,
            Message::DevConfirm => self.confirm = Some(PendingAction::Quit),
            Message::Frame => {}
        }
        Task::none()
    }

    /// Whether the open project (if any) has edits that aren't saved to disk yet.
    fn is_dirty(&self) -> bool {
        matches!(&self.screen, Screen::Editor(e) if e.doc.is_dirty())
    }

    /// Reverts the open project to its last saved file, dropping in-memory edits
    /// ("Don't save" in the unsaved-changes dialog). A project that was never saved
    /// has nothing on disk to protect, so it's left as-is — it's about to be replaced anyway.
    fn discard_changes(&mut self) {
        if let Screen::Editor(e) = &mut self.screen {
            if let Some(path) = e.path.clone() {
                if let Ok(fresh) = Project::load(&path) {
                    e.doc = Doc::new(fresh);
                }
            }
        }
    }

    /// Continues whatever New/Close/Open/Quit was waiting on the unsaved-changes dialog.
    fn run_pending(&mut self, action: PendingAction) -> Task<Message> {
        match action {
            PendingAction::Quit => {
                self.persist();
                if let Screen::Editor(e) = &mut self.screen {
                    e.shutdown();
                }
                window::latest().and_then(window::close)
            }
            PendingAction::NewProject | PendingAction::CloseProject => {
                self.close_project();
                Task::none()
            }
            PendingAction::Open => pick_project_file(),
        }
    }

    /// Saves the project (if it has a file) and the layout.
    fn persist(&mut self) {
        if let Screen::Editor(e) = &mut self.screen {
            self.settings.inspector_collapsed = e.collapsed.clone();
            self.settings.inspector_tabs = e.inspector_tabs.clone();
            self.settings.favorite_effects = e.favorite_effects.clone();
            self.settings.record = e.record_choice.clone();
            self.settings.screen_record = e.screen_choice.clone();
            self.settings.effects_open = e.effects_open.clone();
            self.settings.skills = e.skills.clone();
            self.settings.layout = e.layout();
            if let (true, Some(path)) = (e.doc.is_dirty(), e.path.clone()) {
                match e.doc.project.save(&path) {
                    Ok(()) => e.doc.mark_saved(),
                    Err(err) => self.error = Some(format!("Could not save \"{}\": {err}", e.doc.project.name)),
                }
            }
        }
        self.settings.save();
    }

    /// Saves changed shortcuts and hands them to the open editor.
    fn keys_changed(&mut self) {
        self.settings.save();
        if let Screen::Editor(e) = &mut self.screen {
            e.apply_prefs(&self.settings.prefs, false);
        }
    }

    /// A key press no widget used. While a shortcut is being changed it becomes that shortcut's
    /// keys; otherwise the app's own actions (files, settings, view) run here. Editing actions
    /// reach the editor through its own `Message::Key`.
    fn key_pressed(&mut self, key: Key, modifiers: keyboard::Modifiers) -> Task<Message> {
        use crate::shortcuts::Action as A;
        if key == Key::Named(key::Named::Escape) {
            // In the editor, Esc arrives as the editor's own message (handled above).
            if matches!(self.screen, Screen::Start(_)) {
                if self.shortcuts_capture.take().is_none() {
                    (self.shortcuts_dialog, self.settings_dialog, self.ai_dialog, self.app_menu) = (false, false, false, false);
                }
            }
            return Task::none();
        }
        // The recording bar has only its own key (handled by the editor): nothing opens behind it.
        if self.bar != Bar::Off {
            return Task::none();
        }
        let Some(chord) = crate::shortcuts::chord(&key, modifiers) else { return Task::none() };
        if let Some(action) = self.shortcuts_capture.take() {
            self.settings.prefs.keys.assign(action, chord);
            self.keys_changed();
            return Task::none();
        }
        let message = match self.settings.prefs.keys.lookup(&chord) {
            Some(A::NewProject) => Message::NewProject,
            Some(A::Open) => Message::Open,
            Some(A::Save) => Message::Save,
            Some(A::SaveAs) => Message::SaveAs,
            Some(A::Quit) => Message::Quit,
            Some(A::Settings) => Message::OpenSettings,
            Some(A::Shortcuts) => Message::OpenShortcuts,
            Some(A::ToggleAssistant) => Message::ToggleAssistant,
            Some(A::PreviewFit) => Message::PreviewZoom(None),
            Some(A::Preview100) => Message::PreviewZoom(Some(1.0)),
            _ => return Task::none(),
        };
        self.update(message)
    }

    fn create_project(&mut self) -> Task<Message> {
        let Screen::Start(start) = &self.screen else { return Task::none() };
        let name = if start.name.trim().is_empty() { "Untitled project".to_string() } else { start.name.trim().to_string() };
        let project = Project::new(&name, start.canvas);
        self.save_new_project(project)
    }

    /// A short pre-built scene, so a first-time user has something finished to
    /// explore and pull apart instead of a blank canvas.
    fn create_example_project(&mut self) -> Task<Message> {
        let Screen::Start(start) = &self.screen else { return Task::none() };
        let project = crate::sample::demo_project(start.canvas);
        self.save_new_project(project)
    }

    fn save_new_project(&mut self, project: Project) -> Task<Message> {
        let dir = self.settings.projects_dir();
        let path = unique_path(&dir, &project.name);
        let saved = std::fs::create_dir_all(&dir).map_err(anyhow::Error::from).and_then(|_| project.save(&path));
        let path = match saved {
            Ok(()) => {
                self.settings.touch_recent(&project.name, &path, project.canvas.summary());
                self.settings.save();
                Some(path)
            }
            Err(e) => {
                self.error = Some(format!("Could not save the new project: {e}"));
                None
            }
        };
        self.show_editor(project, path)
    }

    fn show_editor(&mut self, project: Project, path: Option<PathBuf>) -> Task<Message> {
        // Save and stop the current project first.
        self.persist();
        if let Screen::Editor(old) = &mut self.screen {
            old.shutdown();
        }
        let mut editor = Editor::new(project, path, self.settings.layout);
        editor.set_ai(self.settings.ai);
        editor.apply_prefs(&self.settings.prefs, true);
        editor.collapsed = self.settings.inspector_collapsed.clone();
        editor.inspector_tabs = self.settings.inspector_tabs.clone();
        editor.favorite_effects = self.settings.favorite_effects.clone();
        editor.record_choice = self.settings.record.clone();
        editor.screen_choice = self.settings.screen_record.clone();
        editor.effects_open = self.settings.effects_open.clone();
        editor.skills = self.settings.skills.clone();
        let _ = editor.update(editor::Message::WindowSize(self.window_size));
        let task = editor.boot().map(Message::Editor);
        let task = task.chain(window::latest().and_then(window::size).map(Message::WindowSize));
        self.screen = Screen::Editor(Box::new(editor));
        task
    }

    fn open_project(&mut self, path: &Path) -> Task<Message> {
        match Project::load(path) {
            Ok(project) => {
                self.settings.touch_recent(&project.name, path, project.canvas.summary());
                self.settings.save();
                self.error = None;
                self.show_editor(project, Some(path.to_path_buf()))
            }
            Err(e) => {
                self.error = Some(format!("Could not open {}: {e}", path.display()));
                Task::none()
            }
        }
    }

    fn save_editor(&mut self, new_path: Option<PathBuf>) {
        let Screen::Editor(e) = &mut self.screen else { return };
        if let Some(p) = new_path {
            e.path = Some(p);
        }
        let Some(path) = e.path.clone() else { return };
        match e.doc.project.save(&path) {
            Ok(()) => {
                e.doc.mark_saved();
                self.settings.touch_recent(&e.doc.project.name, &path, e.doc.project.canvas.summary());
                self.settings.layout = e.layout();
                self.settings.save();
            }
            Err(err) => self.error = Some(format!("Could not save: {err}")),
        }
    }

    fn close_project(&mut self) {
        self.persist();
        if let Screen::Editor(e) = &mut self.screen {
            e.shutdown();
        }
        self.screen = Screen::Start(Start::default());
    }

    pub fn view(&self) -> Element<'_, Message> {
        // While a screen take runs the whole window is its recording bar.
        if let Screen::Editor(e) = &self.screen {
            if let Some(bar) = e.bar_view() {
                return bar.map(Message::Editor);
            }
        }
        let (left, center, right): (Element<'_, Message>, Element<'_, Message>, Element<'_, Message>) = match &self.screen {
            Screen::Start(_) => (
                row![chrome::logo(), text("AI Video Studio").font(INTER_SEMIBOLD).size(T_BODY)].spacing(S2 + 2.0).align_y(Alignment::Center).into(),
                space().into(),
                row![
                    crate::ai_connect::chip(self.settings.ai, Message::OpenAi),
                    widgets::tip(widgets::icon_button(code::KEYBOARD, Some(Message::OpenShortcuts)), "Keyboard shortcuts", Some("Ctrl+/"), iced::widget::tooltip::Position::Bottom),
                    widgets::tip(widgets::icon_button(code::SETTINGS, Some(Message::OpenSettings)), "Settings", Some("Ctrl+,"), iced::widget::tooltip::Position::Bottom),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
            ),
            Screen::Editor(e) => self.editor_title(e),
        };
        let title_bar = chrome::title_bar(left, center, right, self.maximized, Message::Window);
        let body: Element<'_, Message> = match &self.screen {
            Screen::Start(s) => s.view(&self.settings.recent).map(Message::Start),
            Screen::Editor(e) => e.view().map(Message::Editor),
        };
        let mut content = column![title_bar, body];
        if let Some(err) = &self.error {
            content = content.push(
                container(
                    row![
                        icon(code::TRIANGLE_ALERT, 14).color(DANGER),
                        text(err).size(T_SMALL).color(TEXT_1),
                        space().width(Fill),
                        widgets::icon_button(code::X, Some(Message::DismissError)).width(22).height(22),
                    ]
                    .spacing(S2)
                    .align_y(Alignment::Center),
                )
                .padding([8, 14])
                .width(Fill)
                .style(style::badge(DANGER)),
            );
        }
        // The window is always the same stack of layers, empty when unused: changing the shape
        // of the widget tree (say, wrapping it in a stack only while something is dragged) makes
        // iced rebuild every panel, which loses their scroll positions and text cursors.
        let mut dragged_layer: Element<'_, Message> = space().into();
        if let Screen::Editor(e) = &self.screen {
            // What's being dragged: media from the library, or an effect.
            let dragged = match (e.dnd.and_then(|id| e.project().asset(id)), e.effect_dnd) {
                (Some(asset), _) => Some((code::FILM, asset.name.clone())),
                (None, Some(effect)) => Some((code::SPARKLES, effect.label())),
                _ => None,
            };
            if let Some((glyph, label)) = dragged {
                let ghost = container(row![icon(glyph, 13), text(label).size(T_SMALL)].spacing(6).align_y(Alignment::Center)).padding([6, 10]).style(style::popover);
                let inset = if chrome::draws_frame(self.maximized) { chrome::SHADOW_INSET + 1.0 } else { 0.0 };
                dragged_layer = iced::widget::pin(iced::widget::opaque(ghost)).x(e.cursor.x - inset + 12.0).y(e.cursor.y - inset + 10.0).into();
            }
        }
        let appear = self.modal.progress();
        let settings_layer: Element<'_, Message> = if self.settings_dialog {
            crate::settings_dialog::view(&self.settings.prefs, crate::settings::projects_dir(), appear).map(Message::Settings)
        } else {
            space().into()
        };
        let ai_layer: Element<'_, Message> = if self.ai_dialog {
            crate::ai_connect::view(crate::ai_connect::State {
                connected: self.settings.ai,
                checking: self.ai_checking,
                signing_in: self.ai_signing_in,
                detail: self.ai_detail.as_ref().map(|(p, r)| (*p, r)),
                mcp_url: crate::mcp::url(),
            }, appear)
            .map(Message::Ai)
        } else {
            space().into()
        };
        let confirm_layer: Element<'_, Message> = match self.confirm {
            Some(action) => confirm_dialog(action, appear),
            None => space().into(),
        };
        let shortcuts_layer: Element<'_, Message> = if self.shortcuts_dialog { crate::shortcuts_dialog::view(&self.settings.prefs.keys, self.shortcuts_capture, appear).map(Message::Shortcuts) } else { space().into() };
        let window_content: Element<'_, Message> = iced::widget::stack![content, dragged_layer, settings_layer, ai_layer, shortcuts_layer, confirm_layer].into();
        chrome::frame(window_content, self.maximized, Message::Window)
    }

    fn editor_title<'a>(&'a self, e: &'a Editor) -> (Element<'a, Message>, Element<'a, Message>, Element<'a, Message>) {
        let menu_button = button(row![chrome::logo(), icon(code::CHEVRON_DOWN, 13).color(TEXT_3)].spacing(5).align_y(Alignment::Center))
            .padding([4, 6])
            .style(style::ghost)
            .on_press(Message::AppMenu(!self.app_menu));
        let menu = widgets::menu(
            vec![
                MenuItem::new(code::FILE_PLUS, "New project", Message::NewProject).shortcut("Ctrl+N"),
                MenuItem::new(code::FOLDER_OPEN, "Open…", Message::Open).shortcut("Ctrl+O"),
                MenuItem::Separator,
                MenuItem::new(code::SAVE, "Save", Message::Save).shortcut("Ctrl+S"),
                MenuItem::new(code::COPY, "Save as…", Message::SaveAs).shortcut("Ctrl+Shift+S"),
                MenuItem::new(code::PENCIL, "Rename project…", Message::Editor(editor::Message::RenameProjectStart)),
                MenuItem::Separator,
                MenuItem::new(code::SLIDERS_HORIZONTAL, "Settings…", Message::OpenSettings).shortcut("Ctrl+,"),
                MenuItem::new(code::KEYBOARD, "Keyboard shortcuts…", Message::OpenShortcuts).shortcut("Ctrl+/"),
                MenuItem::Separator,
                MenuItem::new(code::X, "Close project", Message::CloseProject),
                MenuItem::new(code::ARROW_UP_RIGHT, "Quit", Message::Quit).shortcut("Ctrl+Q"),
            ],
            230.0,
        );
        let dropdown = DropDown::new(menu_button, menu, self.app_menu)
            .on_dismiss(Message::AppMenu(false))
            .width(iced::Length::Fixed(230.0))
            .alignment(iced_aw::drop_down::Alignment::BottomEnd)
            .offset(6.0);
        let status = match (e.path.is_some(), e.doc.is_dirty()) {
            (true, true) => ("Saving…", ACCENT_HOVER),
            (true, false) => ("Saved", TEXT_4),
            (false, _) => ("Not saved", WARNING),
        };
        // How much room the title bar has (in the UI's own units, so a larger UI scale counts as a
        // narrower window). Narrower than everything needs, it drops what has another way in.
        let width = if self.window_size.width > 0.0 { self.window_size.width / self.scale_factor() } else { f32::MAX };
        // Everything fits in about 1150 with a short project name (measured), a long name needs
        // more. Below that, Keyboard shortcuts and Settings (both also in the logo menu) hide and
        // Record shows its icon only; below about 1020, Export too, and the save status its dot.
        // (About 7 px a character for the name's font; a guess, so the name is also clipped.)
        let name_width = (e.doc.project.name.chars().count() as f32 * 7.0).min(260.0);
        let longer = (name_width - 60.0).max(0.0);
        let roomy = width >= 1150.0 + longer;
        let tight = width < 1020.0 + longer;
        let name: Element<'_, Message> = match &e.renaming_project {
            Some(draft) => text_input("Project name", draft)
                .id("project-rename")
                .on_input(|s| Message::Editor(editor::Message::RenameProjectInput(s)))
                .on_submit(Message::Editor(editor::Message::RenameProjectCommit))
                .padding([3, 6])
                .size(T_BODY)
                .style(style::input)
                .into(),
            // A long name is cut short ("Product launch video for…") rather than pushing everything else aside.
            None => mouse_area(container(text(widgets::fit(&e.doc.project.name, if tight { 140.0 } else { 260.0 }, T_BODY)).font(INTER_SEMIBOLD).size(T_BODY).wrapping(iced::widget::text::Wrapping::None)).max_width(if tight { 140.0 } else { 260.0 }).clip(true))
                .on_double_click(Message::Editor(editor::Message::RenameProjectStart))
                .into(),
        };
        let status_label: Element<'_, Message> = if tight { space().into() } else { text(status.0).size(T_CAPTION).color(TEXT_3).into() };
        let left = row![
            dropdown,
            name,
            widgets::tip(
                container(row![container(space().width(6).height(6)).style(style::swatch(status.1, 3.0)), status_label].spacing(5).align_y(Alignment::Center)).padding([0, 4]),
                status.0,
                None,
                iced::widget::tooltip::Position::Bottom,
            ),
        ]
        .spacing(S2)
        .align_y(Alignment::Center);
        let chip = button(
            row![icon(code::MONITOR, 12).color(TEXT_3), text(e.doc.project.canvas.summary()).font(INTER_MEDIUM).size(T_SMALL).color(TEXT_2), icon(code::CHEVRON_DOWN, 12).color(TEXT_3)].spacing(6).align_y(Alignment::Center),
        )
        .padding([5, 10])
        .style(|t, s| {
            let mut st = style::secondary(t, s);
            if matches!(s, iced::widget::button::Status::Active) {
                st.background = Some(SURFACE_0.into());
                st.shadow = Default::default();
            }
            st
        })
        .on_press(Message::FormatMenu(!self.format_menu));
        let canvas = e.doc.project.canvas;
        let set = move |c: crate::model::Canvas| Message::Editor(editor::Message::SetCanvas(c));
        let (w, h) = canvas.size();
        let panel = container(
            column![
                row![text("Video format").font(INTER_SEMIBOLD).size(T_BODY), space().width(Fill), text(format!("{w} × {h}")).size(T_CAPTION).color(TEXT_3)].align_y(Alignment::Center),
                widgets::field("Aspect ratio", widgets::segmented(&crate::model::Aspect::ALL, canvas.aspect, |a| a.label(), move |a| set(crate::model::Canvas { aspect: a, ..canvas }))),
                widgets::field("Resolution", widgets::segmented(&crate::model::Resolution::ALL, canvas.resolution, |r| r.label().to_string(), move |r| set(crate::model::Canvas { resolution: r, ..canvas }))),
                widgets::field("Frame rate", widgets::segmented(&crate::model::FRAME_RATES, canvas.fps, |f| format!("{f} fps"), move |f| set(crate::model::Canvas { fps: f, ..canvas }))),
                text("Positions and sizes adapt automatically when you change the format.").size(T_CAPTION).color(TEXT_3),
            ]
            .spacing(S3),
        )
        .width(380)
        .padding(S4)
        .style(style::popover);
        let format = DropDown::new(chip, iced::widget::opaque(panel), self.format_menu).on_dismiss(Message::FormatMenu(false)).width(iced::Length::Fixed(380.0)).alignment(iced_aw::drop_down::Alignment::Bottom).offset(8.0);

        // Preview zoom: only changes how the canvas is shown, never the video's size.
        let percent = e.preview_percent();
        let zoom_label = if e.preview_fit() { format!("Fit · {percent:.0}%") } else { format!("{percent:.0}%") };
        let zoom_chip = button(row![icon(code::SEARCH, 12).color(TEXT_3), text(zoom_label).font(INTER_MEDIUM).size(T_SMALL).color(TEXT_2), icon(code::CHEVRON_DOWN, 12).color(TEXT_3)].spacing(6).align_y(Alignment::Center))
            .padding([5, 10])
            .style(|t, s| {
                let mut st = style::secondary(t, s);
                if matches!(s, iced::widget::button::Status::Active) {
                    st.background = Some(SURFACE_0.into());
                    st.shadow = Default::default();
                }
                st
            })
            .on_press(Message::ZoomMenu(!self.zoom_menu));
        const LABELS: [&str; 8] = ["25%", "50%", "75%", "100%", "150%", "200%", "300%", "400%"];
        let level = |label: &'static str, z: Option<f32>, shortcut: Option<&'static str>| {
            let current = match z {
                None => e.preview_fit(),
                Some(z) => !e.preview_fit() && (percent - z * 100.0).abs() < 0.5,
            };
            MenuItem::Action { icon: current.then_some(code::CHECK), label, shortcut: shortcut.map(Into::into), on_press: Some(Message::PreviewZoom(z)), danger: false }
        };
        let mut items = vec![level("Fit to window", None, Some("Ctrl+0")), MenuItem::Separator];
        items.extend(editor::PREVIEW_ZOOMS.iter().zip(LABELS).map(|(&z, l)| level(l, Some(z), (z == 1.0).then_some("Ctrl+1"))));
        let zoom_menu = DropDown::new(zoom_chip, widgets::menu(items, 190.0), self.zoom_menu)
            .on_dismiss(Message::ZoomMenu(false))
            .width(iced::Length::Fixed(190.0))
            .alignment(iced_aw::drop_down::Alignment::Bottom)
            .offset(8.0);
        let center = row![format, zoom_menu].spacing(6).align_y(Alignment::Center);
        let pos = iced::widget::tooltip::Position::Bottom;
        let right = row![
            widgets::tip(widgets::icon_button(code::UNDO_2, e.doc.can_undo().then_some(Message::Editor(editor::Message::Undo))), e.doc.undo_label().unwrap_or("Undo"), Some("Ctrl+Z"), pos),
            widgets::tip(widgets::icon_button(code::REDO_2, e.doc.can_redo().then_some(Message::Editor(editor::Message::Redo))), e.doc.redo_label().unwrap_or("Redo"), Some("Ctrl+Shift+Z"), pos),
            container(widgets::vdivider(18.0)).padding([0, 6]),
            widgets::tip(crate::ai_connect::chip(self.settings.ai, Message::OpenAi), "AI connection", None, pos),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        let right = if roomy {
            right.push(space().width(4)).push(widgets::tip(widgets::icon_button(code::KEYBOARD, Some(Message::OpenShortcuts)), "Keyboard shortcuts", Some("Ctrl+/"), pos)).push(widgets::tip(widgets::icon_button(code::SETTINGS, Some(Message::OpenSettings)), "Settings", Some("Ctrl+,"), pos))
        } else {
            right
        };
        // Record opens the recorder straight away, screen and camera together, as screen recorders do.
        // (Camera only: Ctrl+R, or Record camera in the Media panel.)
        let record = Message::PickRecord(RecordKind::Screen);
        let record_menu = widgets::tip(
            if roomy { widgets::secondary_button(Some(code::CIRCLE_DOT), "Record", Some(record)) } else { widgets::icon_button(code::CIRCLE_DOT, Some(record)).style(style::secondary) },
            "Record screen and camera",
            Some("Ctrl+Shift+R"),
            pos,
        );
        let export = Message::Editor(editor::Message::OpenExport);
        let right = right
            .push(space().width(4))
            .push(widgets::tip(widgets::icon_toggle(code::SPARKLES, e.side_tab == SideTab::Assistant, Message::ToggleAssistant), "Assistant", Some("Ctrl+K"), pos))
            .push(space().width(6))
            // (No tooltip, like the other dropdowns: it was drawn over the open menu.)
            .push(record_menu)
            .push(space().width(4))
            .push(widgets::tip(
                if tight { widgets::icon_button(code::DOWNLOAD, Some(export)).style(style::primary) } else { widgets::primary_button(Some(code::DOWNLOAD), "Export", Some(export)) },
                "Export video",
                Some("Ctrl+E"),
                pos,
            ));
        (left.into(), center.into(), right.into())
    }
}

/// Debug builds only: after `STUDIO_SNAPSHOT_DELAY` seconds (default 5), capture the window to `STUDIO_SNAPSHOT`.
fn snapshot_task() -> Task<Message> {
    if !cfg!(debug_assertions) || std::env::var("STUDIO_SNAPSHOT").is_err() {
        return Task::none();
    }
    // At least 2 s, so every step below runs after the window is up (and no wait goes negative).
    let delay: f64 = std::env::var("STUDIO_SNAPSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(5.0_f64).max(2.0);
    // Optionally scroll the track list first (`STUDIO_SNAPSHOT_SCROLL=pixels`).
    let scroll: f32 = std::env::var("STUDIO_SNAPSHOT_SCROLL").ok().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let scroll_first = Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay)), move |_| Message::Editor(editor::Message::ScrollTracks(scroll)));
    // `STUDIO_SNAPSHOT_TIME=seconds` scrolls the timeline so that time is at the left edge.
    let hscroll = match std::env::var("STUDIO_SNAPSHOT_TIME").ok().and_then(|d| d.parse::<f64>().ok()) {
        Some(t) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.2)), move |_| Message::Editor(editor::Message::SetView(editor::timeline::View { scroll: t, zoom: 48.0 }))),
        None => Task::none(),
    };
    let shot = Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay + 0.5)), |_| ()).then(|_| window::latest()).and_then(window::screenshot).map(Message::Snapshot);
    // `STUDIO_SNAPSHOT_SIZE=1100x700` sets the window's size first (to check narrow layouts).
    let resize = match std::env::var("STUDIO_SNAPSHOT_SIZE").ok().and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)))) {
        Some((w, h)) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(0.5)), |_| ()).then(|_| window::latest()).and_then(move |id| window::resize::<Message>(id, iced::Size::new(w, h))),
        None => Task::none(),
    };
    // `STUDIO_SNAPSHOT_OPEN=ai` opens the Connect AI dialog first.
    let open = match std::env::var("STUDIO_SNAPSHOT_OPEN").as_deref() {
        Ok("ai") => Task::done(Message::OpenAi),
        Ok("assistant") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 1.0)), |_| Message::ToggleAssistant),
        Ok("settings") => Task::done(Message::OpenSettings),
        Ok("effects") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 1.0)), |_| Message::Editor(editor::Message::LibraryTab(editor::LibraryTab::Transitions))),
        Ok("skills") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 1.0)), |_| Message::Editor(editor::Message::OpenSkills)),
        Ok("shortcuts") => Task::done(Message::OpenShortcuts),
        Ok("confirm") => Task::done(Message::DevConfirm),
        // Early enough for the camera to be showing, and (with a long delay) to watch the dialog run.
        Ok("record") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64((delay - 1.8).min(3.0))), |_| Message::PickRecord(RecordKind::Camera)),
        // The Record dialog while it records: opened, then Start pressed once the camera is on
        // (the countdown takes 3 s, so use a delay of 8 or more; with `AIVE_RECORD_FAKE=1`).
        Ok("recording") => Task::batch([
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(1.0)), |_| Message::PickRecord(RecordKind::Camera)),
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64((delay - 6.0).max(2.5))), |_| Message::Editor(editor::Message::Record(editor::record_dialog::Msg::Start))),
        ]),
        // The Record screen dialog (with `AIVE_RECORD_FAKE=1`), idle, or while it records (delay 8 or more).
        Ok("record-screen") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64((delay - 1.8).min(3.0))), |_| Message::PickRecord(RecordKind::Screen)),
        Ok("recording-screen") => Task::batch([
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(1.0)), |_| Message::PickRecord(RecordKind::Screen)),
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64((delay - 6.0).max(3.0))), |_| Message::Editor(editor::Message::ScreenRecord(editor::screen_dialog::Msg::Start))),
        ]),
        // A finished screen take (delay 12 or more): the window back from the recording bar and the clips on the timeline.
        Ok("recorded-screen") => Task::batch([
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(1.0)), |_| Message::PickRecord(RecordKind::Screen)),
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(3.0)), |_| Message::Editor(editor::Message::ScreenRecord(editor::screen_dialog::Msg::Start))),
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64((delay - 3.0).max(9.0))), |_| Message::Editor(editor::Message::ScreenRecord(editor::screen_dialog::Msg::Stop))),
        ]),
        // The title bar's Record menu, open.
        Ok("record-menu") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.6)), |_| Message::RecordMenu(true)),
        Ok("menu") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.1)), |_| Message::Editor(editor::Message::DevMenu)),
        Ok("export") => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 1.0)), |_| Message::Editor(editor::Message::OpenExport)),
        _ => Task::none(),
    };
    // `STUDIO_SNAPSHOT_SELECT=clip[,range_start,range_end]` selects a clip (and a range).
    let select = match std::env::var("STUDIO_SNAPSHOT_SELECT").ok().map(|v| v.split(',').filter_map(|s| s.parse::<f64>().ok()).collect::<Vec<_>>()) {
        Some(v) if !v.is_empty() => {
            let range = (v.len() >= 3).then(|| (v[1], v[2]));
            Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.3)), move |_| Message::Editor(editor::Message::DevSelect(v[0] as u64, range)))
        }
        _ => Task::none(),
    };
    // `STUDIO_SNAPSHOT_ZOOM=2` zooms the preview to 200% (`fit` fits it).
    let zoom = match std::env::var("STUDIO_SNAPSHOT_ZOOM").ok().and_then(|z| z.parse::<f32>().ok()) {
        Some(z) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.4)), move |_| Message::PreviewZoom(Some(z))),
        None => Task::none(),
    };
    // `STUDIO_SNAPSHOT_CHAT=reply.md` shows a sample conversation with that reply.
    let chat = match std::env::var("STUDIO_SNAPSHOT_CHAT").ok().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(md) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 1.0)), move |_| Message::Editor(editor::Message::DevChat("How should I structure the launch video?".into(), md.clone()))),
        None => Task::none(),
    };
    // `STUDIO_SNAPSHOT_REOPEN=1` switches to the assistant tab last (after a selection opened the inspector).
    // `STUDIO_SNAPSHOT_TEXT=start,end` selects those characters in the text box of the selected text clip.
    // `STUDIO_SNAPSHOT_TYPE=text` types that at the end of the selected text clip's text box.
    let typing = match std::env::var("STUDIO_SNAPSHOT_TYPE") {
        Ok(t) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.25)), move |_| Message::Editor(editor::Message::DevType(t.clone()))),
        Err(_) => Task::none(),
    };
    let text_select = match std::env::var("STUDIO_SNAPSHOT_TEXT").ok().map(|v| v.split(',').filter_map(|s| s.parse::<usize>().ok()).collect::<Vec<_>>()) {
        Some(v) if v.len() == 2 => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.2)), move |_| Message::Editor(editor::Message::DevSelectText(v[0], v[1]))),
        _ => Task::none(),
    };
    let reopen = match std::env::var("STUDIO_SNAPSHOT_REOPEN") {
        Ok(_) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.1)), |_| Message::ToggleAssistant),
        Err(_) => Task::none(),
    };
    // `STUDIO_SNAPSHOT_SEEK=seconds` moves the playhead.
    let seek = match std::env::var("STUDIO_SNAPSHOT_SEEK").ok().and_then(|d| d.parse::<f64>().ok()) {
        Some(t) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 1.5)), move |_| Message::Editor(editor::Message::Seek(t))),
        None => Task::none(),
    };
    // `STUDIO_SNAPSHOT_FIND=words` types into the inspector's search box (to bring a section into view).
    let find = match std::env::var("STUDIO_SNAPSHOT_FIND") {
        Ok(q) => Task::perform(tokio::time::sleep(std::time::Duration::from_secs_f64(delay - 0.15)), move |_| Message::Editor(editor::Message::InspectorSearch(q.clone()))),
        Err(_) => Task::none(),
    };
    Task::batch([resize, open, scroll_first, hscroll, select, typing, text_select, zoom, chat, reopen, seek, find, shot])
}

/// The "You have unsaved changes" dialog shown before New/Close/Open/Quit would discard them.
fn confirm_dialog(action: PendingAction, appear: f32) -> Element<'static, Message> {
    let verb = match action {
        PendingAction::Quit => "quitting",
        PendingAction::NewProject => "starting a new project",
        PendingAction::CloseProject => "closing this project",
        PendingAction::Open => "opening another project",
    };
    let card = container(
        column![
            row![
                container(icon(code::TRIANGLE_ALERT, 16).color(ON_ACCENT)).center(34).style(style::logo),
                column![text("Unsaved changes").font(INTER_SEMIBOLD).size(T_TITLE), text(format!("Save your changes before {verb}?")).size(T_SMALL).color(TEXT_3)].spacing(2).width(Fill),
            ]
            .spacing(S3)
            .align_y(Alignment::Center),
            row![
                widgets::ghost_button(None, "Cancel", Some(Message::ConfirmCancel)),
                space().width(Fill),
                widgets::secondary_button(None, "Don't save", Some(Message::ConfirmDiscard)),
                widgets::primary_button(Some(code::SAVE), "Save", Some(Message::ConfirmSave)),
            ]
            .spacing(S2)
            .align_y(Alignment::Center),
        ]
        .spacing(S4),
    )
    .width(420)
    .padding(S6)
    .style(style::popover);
    widgets::modal(card, Message::ConfirmCancel, appear)
}

/// Opens the "Open project" file picker.
fn pick_project_file() -> Task<Message> {
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .set_title("Open project")
                .add_filter("AI Video Editor project", &[FILE_EXTENSION])
                .pick_file()
                .await
                .map(|h| h.path().to_path_buf())
        },
        Message::OpenPicked,
    )
}

fn window_task(action: WindowAction) -> Task<Message> {
    match action {
        WindowAction::Drag => window::latest().and_then(window::drag),
        WindowAction::ToggleMaximize => window::latest().and_then(window::toggle_maximize),
        WindowAction::Minimize => window::latest().and_then(|id| window::minimize(id, true)),
        WindowAction::Close => Task::done(Message::Quit),
        WindowAction::Resize(dir) => window::latest().and_then(move |id| window::drag_resize(id, dir)),
    }
}

fn shortcut(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else { return None };
    (status != event::Status::Captured).then_some(Message::Key(key, modifiers))
}

/// `dir/name.aivp`, or `dir/name (2).aivp` if taken.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let mut path = dir.join(format!("{safe}.{FILE_EXTENSION}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{safe} ({n}).{FILE_EXTENSION}"));
        n += 1;
    }
    path
}

