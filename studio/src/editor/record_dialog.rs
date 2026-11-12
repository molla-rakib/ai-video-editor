//! The Record camera dialog: pick a camera and microphone, see yourself, count down, record.
//! The camera itself is run by `crate::recorder`; this is its state machine and its view.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::{column, container, image, pick_list, row, space, stack, text};
use iced::{Alignment, Color, Element, Fill};

use super::Message;
use crate::recorder::{self, Device, Devices, Session, Setup, State};
use crate::settings::RecordChoice;
use crate::ui::icons::{code, icon};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

/// Seconds of countdown before a recording starts.
const COUNTDOWN: f64 = 3.0;
const STAGE: (f32, f32) = (592.0, 333.0);

#[derive(Debug, Clone)]
pub enum Msg {
    /// The cameras and microphones that are connected (or why they couldn't be listed).
    Found(Result<Devices, String>),
    Camera(Device),
    Mic(MicChoice),
    /// A quarter turn: clockwise or counter-clockwise.
    Turn(bool),
    Mirror,
    Start,
    Stop,
    Discard,
    /// Look for devices again and reopen the camera.
    Refresh,
    Close,
    Tick,
}

/// A row of the microphone list.
#[derive(Debug, Clone, PartialEq)]
pub enum MicChoice {
    None,
    Device(Device),
}

impl std::fmt::Display for MicChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MicChoice::None => f.write_str("No microphone"),
            MicChoice::Device(d) => d.fmt(f),
        }
    }
}

/// Wakes the dialog (a `Msg::Tick`) the moment the camera has a new picture or changes state,
/// so the live picture is shown as it arrives; and ten times a second while nothing arrives,
/// for the countdown, the clock and the level meter.
pub fn wakeups() -> iced::Subscription<Msg> {
    use iced::futures::channel::mpsc;
    use iced::futures::{SinkExt, StreamExt};
    iced::Subscription::run(|| {
        iced::stream::channel(1, async |mut out: mpsc::Sender<Msg>| {
            // Room for one: news that arrives while the last is still being shown is dropped.
            let (tx, mut rx) = mpsc::channel::<()>(1);
            let tx = std::sync::Mutex::new(tx);
            recorder::on_news(Some(Box::new(move || {
                let _ = tx.lock().unwrap().try_send(());
            })));
            while !matches!(tokio::time::timeout(Duration::from_millis(100), rx.next()).await, Ok(None)) {
                let _ = out.send(Msg::Tick).await;
            }
        })
    })
}

/// What the editor does after an update.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Close,
    /// A finished recording to import and put on the timeline: the file, a note if it ended by
    /// itself, and how many seconds at its start were recorded before the countdown ended.
    Recorded(PathBuf, Option<String>, f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Listing the devices.
    Looking,
    /// Showing the live preview (or an error), ready to record.
    Idle,
    /// Counting down since then. The camera is already being reopened to record (it takes a
    /// moment to come back on), so the countdown hides that wait; what it records before the
    /// countdown ends is left out of the clip.
    Countdown(Instant),
    Recording,
    /// Stopped: the file is being finished.
    Saving,
}

/// What to open once the camera is free.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Next {
    Preview,
    Record,
}

pub struct RecordDialog {
    devices: Option<Devices>,
    /// The rows of the microphone list: the microphones found, then "No microphone".
    mic_rows: Vec<MicChoice>,
    camera: Option<Device>,
    mic: Option<Device>,
    turns: u8,
    mirror: bool,
    /// What was chosen last time, to choose again when it is still connected.
    remembered: RecordChoice,
    session: Option<Session>,
    next: Option<Next>,
    phase: Phase,
    /// The preview picture on screen: its sequence number, turn and mirror, then the picture.
    picture: Option<((u64, u8, bool), image::Handle)>,
    level: f32,
    /// When the level meter was last moved (it falls back at a steady pace).
    metered: Instant,
    dir: PathBuf,
    error: Option<String>,
    /// Why the microphone was let go of: it couldn't be opened, and the camera carries on without it.
    mic_note: Option<String>,
    /// The device list was asked for and hasn't arrived yet.
    asking: bool,
    /// When the countdown ended and the recording proper began.
    go: Option<Instant>,
    /// Seconds the file holds from before that moment.
    lead_in: f64,
}

impl RecordDialog {
    pub fn new(dir: PathBuf, remembered: RecordChoice) -> RecordDialog {
        RecordDialog {
            devices: None,
            mic_rows: vec![MicChoice::None],
            camera: None,
            mic: None,
            turns: remembered.turns % 4,
            mirror: remembered.mirror,
            remembered,
            session: None,
            next: None,
            phase: Phase::Looking,
            picture: None,
            level: 0.0,
            metered: Instant::now(),
            dir,
            error: None,
            mic_note: None,
            asking: false,
            go: None,
            lead_in: 0.0,
        }
    }

    /// Whether a recording is running or being saved (the dialog can't be closed then).
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Countdown(_) | Phase::Recording | Phase::Saving)
    }

    /// The choices to remember for next time.
    pub fn choice(&self) -> RecordChoice {
        RecordChoice {
            camera: self.camera.as_ref().map(|d| d.label.clone()).or_else(|| self.remembered.camera.clone()),
            mic: self.mic.as_ref().map(|d| d.label.clone()).or_else(|| self.remembered.mic.clone()),
            // A microphone that failed wasn't the user's choice: it's tried again next time.
            no_mic: if self.devices.is_some() && self.mic_note.is_none() { self.mic.is_none() } else { self.remembered.no_mic },
            turns: self.turns,
            mirror: self.mirror,
        }
    }

    fn setup(&self) -> Option<Setup> {
        Some(Setup { camera: self.camera.clone()?, mic: self.mic.clone(), turns: self.turns, mirror: self.mirror })
    }

    /// Opens the camera for `next`, once whatever holds it now has let go.
    fn open(&mut self, next: Next) {
        if let Some(s) = &self.session {
            s.stop();
        }
        self.next = Some(next);
        self.start_pending();
    }

    fn start_pending(&mut self) {
        if self.session.as_ref().is_some_and(|s| !s.state().ended()) {
            return;
        }
        let (Some(next), Some(setup)) = (self.next.take(), self.setup()) else { return };
        self.session = Some(match next {
            Next::Preview => Session::preview(&setup),
            Next::Record => Session::record(&setup, recorder::next_file(&self.dir)),
        });
    }

    fn fail(&mut self, why: String) {
        self.error = Some(why);
        self.session = None;
        self.next = None;
        self.picture = None;
        self.level = 0.0;
        self.phase = Phase::Idle;
    }

    pub fn update(&mut self, msg: Msg) -> Option<Outcome> {
        match msg {
            Msg::Found(Ok(devices)) => {
                self.asking = false;
                let keep = |list: &[Device], label: &Option<String>| label.as_ref().and_then(|l| list.iter().find(|d| d.label == *l)).cloned();
                // The camera and microphone used last time if they are still there, else the first.
                self.camera = self.camera.take().filter(|c| devices.cameras.contains(c)).or_else(|| keep(&devices.cameras, &self.remembered.camera)).or_else(|| devices.cameras.first().cloned());
                self.mic = if self.remembered.no_mic { None } else { keep(&devices.mics, &self.remembered.mic).or_else(|| devices.mics.first().cloned()) };
                self.error = devices.cameras.is_empty().then(|| "No camera was found. Plug one in, then try again.".to_string());
                self.mic_rows = devices.mics.iter().cloned().map(MicChoice::Device).chain([MicChoice::None]).collect();
                self.devices = Some(devices);
                self.phase = Phase::Idle;
                if self.camera.is_some() {
                    self.open(Next::Preview);
                }
            }
            Msg::Found(Err(e)) => {
                self.asking = false;
                self.devices = Some(Devices::default());
                self.fail(e);
            }
            Msg::Camera(camera) if self.phase == Phase::Idle => {
                self.camera = Some(camera);
                self.error = None;
                self.picture = None;
                self.open(Next::Preview);
            }
            Msg::Mic(choice) if self.phase == Phase::Idle => {
                self.mic = match choice {
                    MicChoice::None => None,
                    MicChoice::Device(d) => Some(d),
                };
                self.remembered.no_mic = self.mic.is_none();
                self.error = None;
                self.mic_note = None;
                self.open(Next::Preview);
            }
            // The preview is turned in the app, so this shows at once; the file is turned when recording starts.
            Msg::Turn(clockwise) if self.phase == Phase::Idle => self.turns = (self.turns + if clockwise { 1 } else { 3 }) % 4,
            Msg::Mirror if self.phase == Phase::Idle => self.mirror = !self.mirror,
            Msg::Start if self.phase == Phase::Idle && self.session.as_ref().is_some_and(|s| s.state() == State::Running) => {
                self.phase = Phase::Countdown(Instant::now());
                self.go = None;
                self.open(Next::Record);
            }
            Msg::Stop if self.phase == Phase::Recording => {
                if let Some(s) = &self.session {
                    s.stop();
                }
                self.phase = Phase::Saving;
            }
            // Throw the take away (also cancels a countdown) and go back to the live preview.
            Msg::Discard if matches!(self.phase, Phase::Recording | Phase::Countdown(..)) => {
                if let Some(s) = self.session.as_ref().filter(|s| s.is_recording()) {
                    s.discard();
                }
                self.next = Some(Next::Preview);
                self.phase = Phase::Idle;
            }
            Msg::Refresh if self.phase == Phase::Idle => {
                self.error = None;
                self.mic_note = None;
                self.session = None;
                self.picture = None;
                self.devices = None;
                self.phase = Phase::Looking;
            }
            Msg::Close if !self.busy() => return Some(Outcome::Close),
            Msg::Tick => return self.tick(),
            _ => {}
        }
        None
    }

    /// Whether the editor should list the devices now (true once per search).
    pub fn take_device_request(&mut self) -> bool {
        let wanted = self.phase == Phase::Looking && self.devices.is_none() && !self.asking;
        self.asking |= wanted;
        wanted
    }

    fn tick(&mut self) -> Option<Outcome> {
        self.start_pending();
        // The newest picture, turned and mirrored the way the file will be.
        if let Some(frame) = self.session.as_ref().and_then(|s| s.frame()) {
            let key = (frame.seq, self.turns, self.mirror);
            if self.picture.as_ref().is_none_or(|(k, _)| *k != key) {
                let (w, h, rgba) = recorder::orient(&frame, self.turns, self.mirror);
                self.picture = Some((key, image::Handle::from_rgba(w, h, rgba)));
            }
        }
        // The meter jumps up at once and falls back gently (to a hundredth in a second), so
        // short sounds are still seen.
        let heard = self.session.as_ref().map_or(0.0, |s| s.level());
        let fall = 0.01f32.powf(self.metered.elapsed().as_secs_f32());
        self.metered = Instant::now();
        self.level = heard.max(self.level * fall);

        let state = self.session.as_ref().map(|s| s.state());
        let recording = self.session.as_ref().is_some_and(|s| s.is_recording());
        match (self.phase, state) {
            // A session that was asked to make room for the next one ends without a file: fine.
            (_, Some(State::Done { file: None, .. })) | (_, None) if self.next.is_some() => {}
            (Phase::Countdown(since), state) if recording => match state {
                // "Go": the countdown is over and the camera is on (whichever comes last).
                Some(State::Running) if since.elapsed().as_secs_f64() >= COUNTDOWN => {
                    let started = self.session.as_ref().and_then(|s| s.started()).unwrap_or_else(Instant::now);
                    self.go = Some((since + std::time::Duration::from_secs_f64(COUNTDOWN)).max(started));
                    self.lead_in = self.go.map_or(0.0, |go| go.saturating_duration_since(started).as_secs_f64());
                    self.phase = Phase::Recording;
                }
                Some(State::Failed(why)) => self.fail(why),
                Some(State::Done { problem, .. }) => self.fail(problem.unwrap_or_else(|| "The camera stopped before the recording began.".into())),
                _ => {}
            },
            // The microphone couldn't be opened: keep the camera going without it, and say so.
            (Phase::Idle, Some(State::Failed(why))) if self.mic.is_some() && self.session.as_ref().is_some_and(|s| s.mic_failed()) => {
                self.mic = None;
                self.mic_note = Some(why);
                self.open(Next::Preview);
            }
            (Phase::Idle, Some(State::Failed(why))) => self.fail(why),
            // Ended while recording or saving: keep whatever there is.
            (Phase::Recording | Phase::Saving, Some(State::Done { file: Some(file), problem })) => return Some(Outcome::Recorded(file, problem, self.lead_in)),
            (Phase::Recording | Phase::Saving, Some(State::Done { file: None, problem })) => self.fail(problem.unwrap_or_else(|| "Nothing was recorded.".into())),
            (Phase::Recording | Phase::Saving, Some(State::Failed(why))) => self.fail(why),
            _ => {}
        }
        None
    }

    pub fn view<'a>(&'a self, appear: f32) -> Element<'a, Message> {
        let m = |msg: Msg| Message::Record(msg);
        let idle = self.phase == Phase::Idle;
        let live = self.session.as_ref().is_some_and(|s| s.state() == State::Running);

        let header = row![
            container(icon(code::VIDEO, 16).color(ON_ACCENT)).center(34).style(style::logo),
            column![
                text("Record camera").font(INTER_SEMIBOLD).size(T_TITLE),
                // The end of the folder's path is the part that tells it apart.
                text(format!("Saved to {}", widgets::fit_end(&self.dir.display().to_string(), 420.0, T_SMALL))).size(T_SMALL).color(TEXT_3).wrapping(text::Wrapping::None),
            ]
            .spacing(2)
            .width(Fill)
            .clip(true),
            widgets::icon_button(code::X, (!self.busy()).then(|| m(Msg::Close))),
        ]
        .spacing(S3)
        .align_y(Alignment::Center);

        // The stage: the live picture, with whatever needs saying on top of it.
        let picture: Element<'a, Message> = match &self.picture {
            Some((_, handle)) => image(handle.clone()).width(Fill).height(Fill).content_fit(iced::ContentFit::Contain).into(),
            None => space().into(),
        };
        let note = |glyph: char, color: Color, words: String| -> Element<'a, Message> {
            container(column![icon(glyph, 22).color(color), text(words).size(T_SMALL).color(TEXT_2).align_x(Alignment::Center)].spacing(S2).align_x(Alignment::Center).max_width(380)).center(Fill).into()
        };
        let over: Element<'a, Message> = match (&self.error, self.phase) {
            (Some(why), _) => note(code::TRIANGLE_ALERT, DANGER, why.clone()),
            (None, Phase::Looking) => note(code::LOADER_CIRCLE, TEXT_3, "Looking for cameras…".into()),
            (None, Phase::Countdown(since)) => {
                let left = (COUNTDOWN - since.elapsed().as_secs_f64()).ceil().clamp(1.0, COUNTDOWN) as u32;
                container(container(text(left.to_string()).font(INTER_BOLD).size(64.0).color(Color::WHITE)).center(112).style(countdown_disc)).center(Fill).into()
            }
            (None, Phase::Recording) => {
                let seconds = self.go.map_or(0.0, |t| t.elapsed().as_secs_f64());
                let stalled = self.session.as_ref().is_some_and(|s| s.stalled());
                let badge = container(row![icon(code::CIRCLE, 9).color(DANGER), super::tabular(&super::fmt_timecode_short(seconds), Color::WHITE)].spacing(6).align_y(Alignment::Center)).padding([5, 10]).style(badge_dark);
                let mut col = column![badge].spacing(S2);
                if stalled {
                    col = col.push(container(text("No picture is arriving from the camera").size(T_SMALL).color(Color::WHITE)).padding([5, 10]).style(badge_dark));
                }
                container(col).padding(S3).into()
            }
            (None, Phase::Saving) => note(code::LOADER_CIRCLE, TEXT_3, "Finishing the recording…".into()),
            (None, Phase::Idle) if self.picture.is_none() => note(code::LOADER_CIRCLE, TEXT_3, "Starting the camera…".into()),
            (None, Phase::Idle) if self.session.as_ref().is_some_and(|s| s.stalled()) => {
                container(container(text("No picture is arriving from the camera").size(T_SMALL).color(Color::WHITE)).padding([5, 10]).style(badge_dark)).padding(S3).into()
            }
            _ => space().into(),
        };
        // (Inset by the border's width: a picture that fills the stage would cover the red recording border.)
        let stage = container(stack![container(picture).center(Fill), over]).width(STAGE.0).height(STAGE.1).padding(2).clip(true).style(stage_style(self.phase == Phase::Recording));

        // Camera: which one, and how its picture is turned.
        let cameras: &[Device] = self.devices.as_ref().map_or(&[], |d| &d.cameras);
        let camera_pick = pick_list(cameras, self.camera.clone(), move |d| m(Msg::Camera(d)))
            .placeholder(if self.devices.is_some() { "No camera found" } else { "Looking…" })
            .padding([7, 10])
            .text_size(T_BODY)
            .width(Fill)
            .style(style::select)
            .menu_style(style::select_menu);
        // Only while the live preview shows: a recording keeps the turn it started with.
        let tool = |glyph: char, label: &'static str, on: bool, msg: Msg| widgets::tip(widgets::icon_toggle(glyph, on, m(msg.clone())).on_press_maybe(idle.then(|| m(msg))), label, None, iced::widget::tooltip::Position::Top);
        let turn_label = match self.turns {
            1 => "Turned right",
            2 => "Upside down",
            3 => "Turned left",
            _ => "Not turned",
        };
        let camera_row = row![
            text("Camera").size(T_SMALL).color(TEXT_2).width(92),
            camera_pick,
            tool(code::ROTATE_CCW, "Turn the picture left", false, Msg::Turn(false)),
            tool(code::ROTATE_CW, "Turn the picture right", false, Msg::Turn(true)),
            tool(code::FLIP_HORIZONTAL, "Mirror the picture", self.mirror, Msg::Mirror),
        ]
        .spacing(S2)
        .align_y(Alignment::Center);

        let mics: &[MicChoice] = &self.mic_rows;
        let mic_now = if self.devices.is_some() { Some(self.mic.clone().map_or(MicChoice::None, MicChoice::Device)) } else { None };
        let mic_pick = pick_list(mics, mic_now, move |c| m(Msg::Mic(c))).placeholder("Looking…").padding([7, 10]).text_size(T_BODY).width(Fill).style(style::select).menu_style(style::select_menu);
        let mic_row = row![text("Microphone").size(T_SMALL).color(TEXT_2).width(92), mic_pick, meter(if self.mic.is_some() { self.level } else { 0.0 })].spacing(S2).align_y(Alignment::Center);

        let hint = match self.phase {
            Phase::Recording => "Recording. The clip lands on the timeline at the playhead when you stop.".to_string(),
            Phase::Countdown(..) => "Get ready…".to_string(),
            Phase::Saving => String::new(),
            _ if self.mic_note.is_some() && self.mic.is_none() => self.mic_note.clone().unwrap_or_default(),
            _ if self.turns % 2 == 1 => format!("{turn_label}: records an upright (portrait) video."),
            _ if self.turns == 2 => format!("{turn_label}."),
            _ => "Turn or mirror the picture before you start.".to_string(),
        };
        let hint_color = if self.phase == Phase::Idle && self.mic_note.is_some() && self.mic.is_none() { WARNING } else { TEXT_3 };
        let buttons: Element<'a, Message> = match self.phase {
            Phase::Recording => row![
                widgets::ghost_button(Some(code::TRASH_2), "Discard", Some(m(Msg::Discard))),
                widgets::stop_button(Some(code::SQUARE), "Stop and add to timeline", Some(m(Msg::Stop))),
            ]
            .spacing(S2)
            .into(),
            Phase::Countdown(..) => widgets::secondary_button(None, "Cancel", Some(m(Msg::Discard))).into(),
            Phase::Saving => space().into(),
            Phase::Looking | Phase::Idle => {
                let mut r = row![widgets::ghost_button(None, "Close", Some(m(Msg::Close)))].spacing(S2);
                if self.error.is_some() {
                    r = r.push(widgets::secondary_button(Some(code::REFRESH_CW), "Try again", Some(m(Msg::Refresh))));
                } else {
                    r = r.push(widgets::primary_button(Some(code::CIRCLE), "Start recording", (idle && live).then(|| m(Msg::Start))));
                }
                r.into()
            }
        };
        let footer = row![text(hint).size(T_CAPTION).color(hint_color).width(Fill), buttons].spacing(S3).align_y(Alignment::Center);

        let card = container(column![header, stage, camera_row, mic_row, widgets::divider(), footer].spacing(S4)).width(STAGE.0 + 2.0 * S6).padding(S6).style(style::popover);
        // A click outside closes it, except while something is being recorded.
        widgets::modal(card, if self.busy() { Message::Noop } else { m(Msg::Close) }, appear)
    }
}

/// The microphone level: a bar that fills from the left, turning red near the top.
fn meter<'a>(level: f32) -> Element<'a, Message> {
    const WIDTH: f32 = 112.0;
    let filled = (level.clamp(0.0, 1.0) * WIDTH).round();
    let color = if level > 0.94 { DANGER } else { SUCCESS };
    let bar = container(space().width(filled).height(6)).style(move |_| container::Style { background: Some(color.into()), border: iced::Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() });
    container(bar).width(WIDTH).height(6).style(|_| container::Style { background: Some(SURFACE_4.into()), border: iced::Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() }).into()
}

fn stage_style(recording: bool) -> impl Fn(&iced::Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Color::BLACK.into()),
        border: iced::Border { color: if recording { DANGER } else { LINE }, width: if recording { 2.0 } else { 1.0 }, radius: R_MD.into() },
        ..Default::default()
    }
}

fn countdown_disc(_: &iced::Theme) -> container::Style {
    container::Style { background: Some(Color { a: 0.55, ..Color::BLACK }.into()), border: iced::Border { radius: 56.0.into(), ..Default::default() }, ..Default::default() }
}

fn badge_dark(_: &iced::Theme) -> container::Style {
    container::Style { background: Some(Color { a: 0.6, ..Color::BLACK }.into()), border: iced::Border { radius: R_SM.into(), ..Default::default() }, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fake_devices() -> Devices {
        Devices {
            cameras: vec![Device { id: recorder::FAKE_CAMERA.into(), label: "Laptop camera".into() }, Device { id: recorder::FAKE_CAMERA.into(), label: "External camera".into() }],
            mics: vec![Device { id: recorder::FAKE_MIC.into(), label: "Test microphone".into() }],
        }
    }

    /// Ticks the dialog like the editor does until `done`, or fails after `seconds`.
    fn tick_until(d: &mut RecordDialog, what: &str, seconds: u64, done: impl Fn(&RecordDialog) -> bool) -> Option<Outcome> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if let Some(outcome) = d.update(Msg::Tick) {
                return Some(outcome);
            }
            if done(d) {
                return None;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what} (phase {:?}, error {:?})", d.phase, d.error);
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    #[test]
    fn records_from_the_remembered_camera_after_a_countdown() {
        let dir = std::env::temp_dir().join(format!("studio-record-dialog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let remembered = RecordChoice { camera: Some("External camera".into()), mic: None, no_mic: false, turns: 1, mirror: true };
        let mut d = RecordDialog::new(dir.clone(), remembered);
        assert!(d.take_device_request());
        assert!(!d.take_device_request(), "asked once");
        // Nothing can start before the devices are known.
        assert_eq!(d.update(Msg::Start), None);
        assert_eq!(d.phase, Phase::Looking);

        assert_eq!(d.update(Msg::Found(Ok(fake_devices()))), None);
        // The camera used last time is chosen again, with its turn and mirroring.
        assert_eq!(d.camera.as_ref().map(|c| c.label.as_str()), Some("External camera"));
        assert_eq!(d.mic.as_ref().map(|c| c.label.as_str()), Some("Test microphone"));
        assert_eq!((d.turns, d.mirror), (1, true));
        tick_until(&mut d, "the live preview", 20, |d| d.picture.is_some());
        // The microphone meter moves before anything is recorded.
        tick_until(&mut d, "the microphone level in the preview", 20, |d| d.level > 0.2);
        // Turning shows at once, without reopening the camera.
        let _ = d.update(Msg::Turn(true));
        let _ = d.update(Msg::Turn(false));
        assert_eq!(d.turns, 1);

        // Start: a countdown, then the camera reopens to record.
        assert_eq!(d.update(Msg::Start), None);
        assert!(matches!(d.phase, Phase::Countdown(_)));
        assert!(d.busy());
        assert_eq!(d.update(Msg::Close), None, "the camera is already recording: Cancel, not Close");
        let pressed = Instant::now();
        tick_until(&mut d, "the recording to start", 30, |d| d.phase == Phase::Recording);
        // The whole countdown ran, and what the camera recorded during it is known.
        assert!(pressed.elapsed().as_secs_f64() >= COUNTDOWN);
        assert!(d.lead_in > 0.2 && d.lead_in < COUNTDOWN, "lead-in {}", d.lead_in);
        assert!(d.busy());
        assert_eq!(d.update(Msg::Close), None, "can't close while recording");
        // The choices stay put while recording.
        let _ = d.update(Msg::Turn(true));
        let _ = d.update(Msg::Camera(fake_devices().cameras[0].clone()));
        assert_eq!((d.turns, d.camera.as_ref().map(|c| c.label.as_str())), (1, Some("External camera")));
        tick_until(&mut d, "some sound", 20, |d| d.level > 0.2);
        std::thread::sleep(Duration::from_millis(700));

        assert_eq!(d.update(Msg::Stop), None);
        assert_eq!(d.phase, Phase::Saving);
        let outcome = tick_until(&mut d, "the finished file", 30, |_| false).unwrap();
        let Outcome::Recorded(file, problem, lead_in) = outcome else { panic!("{outcome:?}") };
        assert_eq!(lead_in, d.lead_in);
        assert_eq!(problem, None);
        assert_eq!(file, dir.join("Camera recording 1.mp4"));
        let info = crate::media::probe(&file).unwrap();
        assert_eq!((info.width, info.height, info.has_audio), (720, 1280, true));
        assert_eq!(d.choice(), RecordChoice { camera: Some("External camera".into()), mic: Some("Test microphone".into()), no_mic: false, turns: 1, mirror: true });
        std::fs::remove_dir_all(dir).ok();
    }

    /// Stress: random clicks on everything, faster than anyone could, with stretches where the
    /// "undo" buttons are left alone so recordings do get made. The dialog must never get stuck
    /// in a state, every recording it reports must be a whole video, and nothing else may be
    /// left on disk. `STUDIO_STRESS=5` runs five times as long.
    #[test]
    fn a_storm_of_clicks_never_wedges_the_dialog() {
        let dir = std::env::temp_dir().join(format!("studio-record-dialog-storm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let steps = 450 * std::env::var("STUDIO_STRESS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
        let mut x = 0xD1A1_06u64;
        let mut roll = move |sides: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % sides
        };
        let devices = fake_devices();
        let mut d = RecordDialog::new(dir.clone(), RecordChoice::default());
        let mut kept: Vec<PathBuf> = Vec::new();
        let mut closed = 0;
        let mut patient;
        // How long the dialog has been in its present state.
        let mut since = (d.phase, Instant::now());
        // What the editor does with an outcome: a finished recording is taken, and the dialog is opened afresh.
        let after = |d: &mut RecordDialog, outcome: Option<Outcome>, kept: &mut Vec<PathBuf>, closed: &mut u32| {
            match outcome {
                Some(Outcome::Recorded(file, problem, lead_in)) => {
                    assert_eq!(problem, None);
                    assert!(lead_in >= 0.0 && lead_in <= COUNTDOWN + 0.5, "lead-in {lead_in}");
                    kept.push(file);
                    *d = RecordDialog::new(dir.clone(), d.choice());
                }
                Some(Outcome::Close) => {
                    *closed += 1;
                    *d = RecordDialog::new(dir.clone(), d.choice());
                }
                None => {}
            }
            if d.take_device_request() {
                let _ = d.update(Msg::Found(Ok(fake_devices())));
            }
        };
        after(&mut d, None, &mut kept, &mut closed);
        for step in 0..steps {
            // Seven seconds or so of leaving Discard, Refresh and Close alone, then two of everything.
            patient = step % 400 < 300;
            let msg = match roll(16) {
                0 => Msg::Camera(devices.cameras[roll(2) as usize].clone()),
                1 => Msg::Mic(MicChoice::Device(devices.mics[0].clone())),
                2 => Msg::Mic(MicChoice::None),
                3 => Msg::Turn(roll(2) == 0),
                4 => Msg::Mirror,
                5..=7 => Msg::Start,
                8 if d.phase == Phase::Recording && d.go.is_some_and(|go| go.elapsed() > Duration::from_millis(400)) => Msg::Stop,
                9 if !patient => Msg::Discard,
                10 if !patient => Msg::Refresh,
                11 if !patient => Msg::Close,
                _ => Msg::Tick,
            };
            let outcome = d.update(msg);
            after(&mut d, outcome, &mut kept, &mut closed);
            for _ in 0..roll(4) {
                std::thread::sleep(Duration::from_millis(roll(30)));
                let outcome = d.update(Msg::Tick);
                after(&mut d, outcome, &mut kept, &mut closed);
            }
            // Never stuck: a countdown ends, a save ends, a search ends, the camera comes on.
            if since.0 != d.phase {
                since = (d.phase, Instant::now());
            }
            let limit = if d.phase == Phase::Idle || d.phase == Phase::Recording { 600 } else { 40 };
            assert!(since.1.elapsed() < Duration::from_secs(limit), "stuck in {:?} (error {:?})", d.phase, d.error);
            assert!(d.error.is_none(), "step {step}: {:?}", d.error);
            assert_eq!(d.busy(), matches!(d.phase, Phase::Countdown(_) | Phase::Recording | Phase::Saving));
        }
        // Wind down: let a countdown finish, stop a recording, and wait for the live picture.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let msg = if d.phase == Phase::Recording { Msg::Stop } else { Msg::Tick };
            let outcome = d.update(msg);
            after(&mut d, outcome, &mut kept, &mut closed);
            if d.phase == Phase::Idle && d.session.as_ref().is_some_and(|s| !s.is_recording() && s.state() == State::Running) && d.picture.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "never settled: {:?}, error {:?}", d.phase, d.error);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(d.update(Msg::Close), Some(Outcome::Close));
        drop(d);
        // Sessions the dialog let go of finish on their own; then only the kept recordings remain.
        let on_disk = || {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            files.sort();
            files
        };
        kept.sort();
        let deadline = Instant::now() + Duration::from_secs(30);
        while on_disk() != kept && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(on_disk(), kept, "only the recordings that were reported are on disk");
        for file in &kept {
            let info = crate::media::probe(file).unwrap_or_else(|e| panic!("{file:?}: {e}"));
            assert!(info.duration > 0.3 && info.width > 0, "{file:?}: {info:?}");
        }
        assert!(!kept.is_empty(), "some recordings were made");
        println!("{steps} clicks: {} recordings kept, closed {closed} times", kept.len());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_cancelled_countdown_and_a_discarded_take_leave_no_file() {
        let dir = std::env::temp_dir().join(format!("studio-record-dialog-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut d = RecordDialog::new(dir.clone(), RecordChoice { no_mic: true, ..Default::default() });
        let _ = d.update(Msg::Found(Ok(fake_devices())));
        assert_eq!(d.camera.as_ref().map(|c| c.label.as_str()), Some("Laptop camera"));
        assert_eq!(d.mic, None, "\"No microphone\" is remembered too");
        tick_until(&mut d, "the live preview", 20, |d| d.picture.is_some());

        // Cancel during the countdown: the take is thrown away and the preview comes back.
        let _ = d.update(Msg::Start);
        let _ = d.update(Msg::Discard);
        assert_eq!(d.phase, Phase::Idle);
        tick_until(&mut d, "the preview to come back", 30, |d| d.session.as_ref().is_some_and(|s| !s.is_recording() && s.state() == State::Running));
        assert!(std::fs::read_dir(&dir).map(|mut files| files.next().is_none()).unwrap_or(true), "nothing was kept");

        // Record, then discard the take: back to a live preview, nothing kept.
        let _ = d.update(Msg::Start);
        tick_until(&mut d, "the recording to start", 30, |d| d.phase == Phase::Recording);
        let _ = d.update(Msg::Discard);
        assert_eq!(d.phase, Phase::Idle);
        tick_until(&mut d, "the preview to come back", 30, |d| d.session.as_ref().is_some_and(|s| !s.is_recording() && s.state() == State::Running));
        assert!(std::fs::read_dir(&dir).map(|mut files| files.next().is_none()).unwrap_or(true), "nothing was kept");
        assert_eq!(d.update(Msg::Close), Some(Outcome::Close));

        // A microphone that can't be opened: the camera carries on without it, and it's tried again next time.
        let mut broken = fake_devices();
        broken.mics = vec![Device { id: recorder::FAKE_BROKEN_MIC.into(), label: "Unplugged headset".into() }];
        let mut d = RecordDialog::new(dir.clone(), RecordChoice::default());
        let _ = d.update(Msg::Found(Ok(broken)));
        assert!(d.mic.is_some());
        tick_until(&mut d, "the live preview without the microphone", 30, |d| d.picture.is_some());
        assert_eq!((d.mic.as_ref(), d.error.as_ref()), (None, None), "the camera works; no error over the picture");
        assert!(d.mic_note.as_deref().is_some_and(|n| n.contains("microphone")), "{:?}", d.mic_note);
        assert!(!d.choice().no_mic, "\"No microphone\" wasn't the user's choice");
        // Choosing a microphone again clears the note.
        let _ = d.update(Msg::Mic(MicChoice::None));
        assert_eq!(d.mic_note, None);
        assert!(d.choice().no_mic);
        let _ = d.update(Msg::Close);

        // No camera at all: said plainly, and "Try again" looks once more.
        let mut none = RecordDialog::new(dir.clone(), RecordChoice::default());
        let _ = none.update(Msg::Found(Ok(Devices::default())));
        assert!(none.error.as_deref().is_some_and(|e| e.contains("No camera")));
        let _ = none.update(Msg::Refresh);
        assert!(none.take_device_request());
        std::fs::remove_dir_all(dir).ok();
    }
}
