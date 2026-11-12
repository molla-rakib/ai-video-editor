//! The Record camera dialog: pick a camera and microphone, see yourself, count down, record.
//! The camera itself is run by `crate::recorder`; this is its state machine and its view.

use std::path::PathBuf;
use std::time::Instant;

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
    dir: PathBuf,
    error: Option<String>,
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
            dir,
            error: None,
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
            no_mic: if self.devices.is_some() { self.mic.is_none() } else { self.remembered.no_mic },
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
        // The meter jumps up at once and falls back gently, so short sounds are still seen.
        let heard = self.session.as_ref().map_or(0.0, |s| s.level());
        self.level = heard.max(self.level * 0.86);

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
                text(format!("Saved to {}", self.dir.display())).size(T_SMALL).color(TEXT_3).wrapping(text::Wrapping::None),
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
            _ => space().into(),
        };
        let stage = container(stack![container(picture).center(Fill), over]).width(STAGE.0).height(STAGE.1).clip(true).style(stage_style(self.phase == Phase::Recording));

        // Camera: which one, and how its picture is turned.
        let cameras = self.devices.as_ref().map(|d| d.cameras.clone()).unwrap_or_default();
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

        let mut mics: Vec<MicChoice> = self.devices.as_ref().map(|d| d.mics.iter().cloned().map(MicChoice::Device).collect()).unwrap_or_default();
        mics.push(MicChoice::None);
        let mic_now = if self.devices.is_some() { Some(self.mic.clone().map_or(MicChoice::None, MicChoice::Device)) } else { None };
        let mic_pick = pick_list(mics, mic_now, move |c| m(Msg::Mic(c))).placeholder("Looking…").padding([7, 10]).text_size(T_BODY).width(Fill).style(style::select).menu_style(style::select_menu);
        let mic_row = row![text("Microphone").size(T_SMALL).color(TEXT_2).width(92), mic_pick, meter(if self.mic.is_some() { self.level } else { 0.0 })].spacing(S2).align_y(Alignment::Center);

        let hint = match self.phase {
            Phase::Recording => "Recording. The clip lands on the timeline at the playhead when you stop.".to_string(),
            Phase::Countdown(..) => "Get ready…".to_string(),
            Phase::Saving => String::new(),
            _ if self.turns % 2 == 1 => format!("{turn_label}: records an upright (portrait) video."),
            _ if self.turns == 2 => format!("{turn_label}."),
            _ => "Turn or mirror the picture before you start.".to_string(),
        };
        let buttons: Element<'a, Message> = match self.phase {
            Phase::Recording => row![
                widgets::ghost_button(Some(code::TRASH_2), "Discard", Some(m(Msg::Discard))),
                widgets::primary_button(Some(code::SQUARE), "Stop and add to timeline", Some(m(Msg::Stop))),
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
        let footer = row![text(hint).size(T_CAPTION).color(TEXT_3).width(Fill), buttons].spacing(S3).align_y(Alignment::Center);

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

        // No camera at all: said plainly, and "Try again" looks once more.
        let mut none = RecordDialog::new(dir.clone(), RecordChoice::default());
        let _ = none.update(Msg::Found(Ok(Devices::default())));
        assert!(none.error.as_deref().is_some_and(|e| e.contains("No camera")));
        let _ = none.update(Msg::Refresh);
        assert!(none.take_device_request());
        std::fs::remove_dir_all(dir).ok();
    }
}
