//! The Record screen dialog: pick a screen and a microphone, count down, record.
//! The recording itself is run by `crate::recorder::screen`; this is its state machine and its view.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bytes::Bytes;
use iced::widget::{column, container, image, pick_list, row, space, stack, text};
use iced::{Alignment, Color, Element, Fill};

use super::record_dialog::{badge_dark, countdown_disc, meter, stage_style, MicChoice, COUNTDOWN, STAGE};
use super::Message;
use crate::recorder::encoder::{Choice, Encoder};
use crate::recorder::screen::Screen;
use crate::recorder::{self, Device, Role, Session, State};
use crate::settings::ScreenChoice;
use crate::ui::icons::{code, icon};
use crate::ui::tokens::*;
use crate::ui::{style, widgets};

/// Pictures a second the dialog offers.
static RATES: [Rate; 2] = [Rate(30), Rate(60)];

/// What the editor lists in the background when the dialog opens.
#[derive(Debug, Clone)]
pub struct Found {
    pub screens: Vec<Screen>,
    pub mics: Vec<Device>,
    /// How the screen will be encoded on this computer (found by trying: see `recorder::encoder`).
    pub encoder: Choice,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Found(Result<Found, String>),
    /// A picture of a screen (its id, the size and the RGBA pixels), or why none could be made.
    Thumbnail(String, Result<(u32, u32, Bytes), String>),
    Screen(Screen),
    Mic(MicChoice),
    Fps(Rate),
    Start,
    Stop,
    Discard,
    /// Look for screens and microphones again.
    Refresh,
    Close,
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate(pub u32);

impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} fps", self.0)
    }
}

/// Wakes the dialog when the recording has news, and ten times a second otherwise (the countdown,
/// the clock, the level meter). The same wake-up the camera dialog uses: only one is open at a time.
pub fn wakeups() -> iced::Subscription<Msg> {
    super::record_dialog::wakeups().map(|_| Msg::Tick)
}

/// A finished recording, ready for the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// The picture's file and where it starts on the take's clock.
    pub picture: (PathBuf, f64),
    /// The microphone's file and where it starts, if one was recorded.
    pub mic: Option<(PathBuf, f64)>,
    /// When the countdown ended on the same clock: the clips come in from there.
    pub go: f64,
    /// A note if the recording ended by itself.
    pub problem: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Close,
    Recorded(Take),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Listing the screens and microphones.
    Looking,
    /// Ready: the chosen screen is shown.
    Idle,
    /// Counting down since then. The recording is already starting (it takes a moment to come
    /// on), so the countdown hides that wait; what it records before the countdown ends is left out.
    Countdown(Instant),
    Recording,
    /// Stopped: the files are being finished.
    Saving,
}

pub struct ScreenDialog {
    found: Option<Found>,
    mic_rows: Vec<MicChoice>,
    screen: Option<Screen>,
    mic: Option<Device>,
    fps: u32,
    /// What was chosen last time, to choose again when it is still there.
    remembered: ScreenChoice,
    session: Option<Session>,
    phase: Phase,
    /// The screen's picture before recording: the screen it shows, and the picture.
    thumbnail: Option<(String, image::Handle)>,
    /// A picture of this screen is wanted (taken by the editor, which makes it in the background).
    thumbnail_wanted: Option<Screen>,
    /// The live picture while recording: its sequence number, then the picture.
    live: Option<(u64, image::Handle)>,
    level: f32,
    metered: Instant,
    dir: PathBuf,
    error: Option<String>,
    /// Why the microphone was let go of: it couldn't be opened, and the recording goes without it.
    mic_note: Option<String>,
    asking: bool,
    /// When the countdown ended and the recording proper began, and the same moment on the take's clock.
    go: Option<Instant>,
    go_clock: f64,
}

impl ScreenDialog {
    pub fn new(dir: PathBuf, remembered: ScreenChoice) -> ScreenDialog {
        ScreenDialog {
            found: None,
            mic_rows: vec![MicChoice::None],
            screen: None,
            mic: None,
            fps: if remembered.fps == 60 { 60 } else { 30 },
            remembered,
            session: None,
            phase: Phase::Looking,
            thumbnail: None,
            thumbnail_wanted: None,
            live: None,
            level: 0.0,
            metered: Instant::now(),
            dir,
            error: None,
            mic_note: None,
            asking: false,
            go: None,
            go_clock: 0.0,
        }
    }

    /// Whether a recording is running or being saved (the dialog can't be closed then).
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Countdown(_) | Phase::Recording | Phase::Saving)
    }

    /// The choices to remember for next time.
    pub fn choice(&self) -> ScreenChoice {
        ScreenChoice {
            screen: self.screen.as_ref().map(|s| s.label.clone()).or_else(|| self.remembered.screen.clone()),
            mic: self.mic.as_ref().map(|d| d.label.clone()).or_else(|| self.remembered.mic.clone()),
            // A microphone that failed wasn't the user's choice: it's tried again next time.
            no_mic: if self.found.is_some() && self.mic_note.is_none() { self.mic.is_none() } else { self.remembered.no_mic },
            fps: self.fps,
        }
    }

    /// Whether the editor should list the screens and microphones now (true once per search).
    pub fn take_device_request(&mut self) -> bool {
        let wanted = self.phase == Phase::Looking && self.found.is_none() && !self.asking;
        self.asking |= wanted;
        wanted
    }

    /// The screen whose picture the editor should make now, if one is wanted (once per request).
    pub fn take_thumbnail_request(&mut self) -> Option<Screen> {
        self.thumbnail_wanted.take()
    }

    /// Whether the chosen screen will be encoded by the processor (and so held to 30 a second).
    fn software(&self) -> bool {
        self.found.as_ref().is_some_and(|f| f.encoder.encoder == Encoder::X264)
    }

    fn setup(&self) -> Option<recorder::screen::ScreenSetup> {
        Some(recorder::screen::ScreenSetup { screen: self.screen.clone()?, mic: self.mic.clone(), fps: self.fps })
    }

    fn fail(&mut self, why: String) {
        self.error = Some(why);
        self.session = None;
        self.live = None;
        self.level = 0.0;
        self.phase = Phase::Idle;
    }

    pub fn update(&mut self, msg: Msg) -> Option<Outcome> {
        match msg {
            Msg::Found(Ok(found)) => {
                self.asking = false;
                let keep = |list: &[Device], label: &Option<String>| label.as_ref().and_then(|l| list.iter().find(|d| d.label == *l)).cloned();
                // The screen and microphone used last time if they are still there, else the first.
                self.screen = self.screen.take().filter(|s| found.screens.contains(s)).or_else(|| self.remembered.screen.as_ref().and_then(|l| found.screens.iter().find(|s| s.label == *l)).cloned()).or_else(|| found.screens.first().cloned());
                self.mic = if self.remembered.no_mic { None } else { keep(&found.mics, &self.remembered.mic).or_else(|| found.mics.first().cloned()) };
                self.error = found.screens.is_empty().then(|| "No screen was found to record.".to_string());
                self.mic_rows = found.mics.iter().cloned().map(MicChoice::Device).chain([MicChoice::None]).collect();
                self.thumbnail_wanted = self.screen.clone();
                self.found = Some(found);
                self.phase = Phase::Idle;
            }
            Msg::Found(Err(e)) => {
                self.asking = false;
                self.found = Some(Found { screens: Vec::new(), mics: Vec::new(), encoder: Choice { encoder: Encoder::X264, path: recorder::encoder::Path::Memory } });
                self.fail(e);
            }
            Msg::Thumbnail(id, result) if self.screen.as_ref().is_some_and(|s| s.id == id) => match result {
                Ok((w, h, rgba)) => self.thumbnail = Some((id, image::Handle::from_rgba(w, h, rgba))),
                Err(e) => {
                    if self.phase == Phase::Idle {
                        self.error = Some(e);
                    }
                }
            },
            Msg::Screen(screen) if self.phase == Phase::Idle => {
                self.error = None;
                self.thumbnail = None;
                self.thumbnail_wanted = Some(screen.clone());
                self.screen = Some(screen);
            }
            Msg::Mic(choice) if self.phase == Phase::Idle => {
                self.mic = match choice {
                    MicChoice::None => None,
                    MicChoice::Device(d) => Some(d),
                };
                self.remembered.no_mic = self.mic.is_none();
                self.mic_note = None;
            }
            Msg::Fps(Rate(n)) if self.phase == Phase::Idle => self.fps = if n == 60 { 60 } else { 30 },
            Msg::Start if self.phase == Phase::Idle && self.error.is_none() && self.screen.is_some() => {
                if let Some(setup) = self.setup() {
                    self.session = Some(Session::record_screen(&setup, recorder::next_screen_file(&self.dir)));
                    self.phase = Phase::Countdown(Instant::now());
                    self.go = None;
                    self.live = None;
                }
            }
            Msg::Stop if self.phase == Phase::Recording => {
                if let Some(s) = &self.session {
                    s.stop();
                }
                self.phase = Phase::Saving;
            }
            // Throw the take away (also cancels a countdown).
            Msg::Discard if matches!(self.phase, Phase::Recording | Phase::Countdown(..)) => {
                if let Some(s) = &self.session {
                    s.discard();
                }
                self.live = None;
                self.level = 0.0;
                self.phase = Phase::Idle;
            }
            Msg::Refresh if self.phase == Phase::Idle => {
                self.error = None;
                self.mic_note = None;
                self.thumbnail = None;
                self.found = None;
                self.phase = Phase::Looking;
            }
            Msg::Close if !self.busy() => return Some(Outcome::Close),
            Msg::Tick => return self.tick(),
            _ => {}
        }
        None
    }

    fn tick(&mut self) -> Option<Outcome> {
        // The newest live picture (a small one a couple of times a second: it shows the take is alive).
        if let Some(frame) = self.session.as_ref().filter(|_| self.busy()).and_then(|s| s.frame()) {
            if self.live.as_ref().is_none_or(|(seq, _)| *seq != frame.seq) {
                self.live = Some((frame.seq, image::Handle::from_rgba(frame.width, frame.height, frame.rgba.clone())));
            }
        }
        // The meter jumps up at once and falls back gently, so short sounds are still seen.
        let heard = self.session.as_ref().map_or(0.0, |s| s.level());
        let fall = 0.01f32.powf(self.metered.elapsed().as_secs_f32());
        self.metered = Instant::now();
        self.level = heard.max(self.level * fall);

        let state = self.session.as_ref().map(|s| s.state());
        match (self.phase, state) {
            // "Go": the countdown is over and the recording is on (whichever comes last).
            (Phase::Countdown(since), Some(State::Running)) if since.elapsed().as_secs_f64() >= COUNTDOWN => {
                let session = self.session.as_ref()?;
                let started = session.started().unwrap_or_else(Instant::now);
                let go = (since + Duration::from_secs_f64(COUNTDOWN)).max(started);
                // The same moment on the take's own clock (which the files' starts are measured on).
                self.go_clock = session.clock().map_or(0.0, |now| now - go.elapsed().as_secs_f64());
                self.go = Some(go);
                self.phase = Phase::Recording;
            }
            // The microphone couldn't be opened: the recording goes without it, and says so.
            (Phase::Countdown(_), Some(State::Failed(why))) if self.mic.is_some() && self.session.as_ref().is_some_and(|s| s.mic_failed()) => {
                self.mic = None;
                self.mic_note = Some(why);
                self.session = None;
                self.phase = Phase::Idle;
            }
            (Phase::Countdown(_), Some(State::Failed(why))) => self.fail(why),
            (Phase::Countdown(_), Some(State::Done { problem, .. })) => self.fail(problem.unwrap_or_else(|| "The screen recording stopped before it began.".into())),
            // Ended while recording or saving: keep whatever there is.
            (Phase::Recording | Phase::Saving, Some(State::Done { file: Some(file), problem })) => return Some(Outcome::Recorded(self.take(file, problem))),
            (Phase::Recording | Phase::Saving, Some(State::Done { file: None, problem })) => self.fail(problem.unwrap_or_else(|| "Nothing was recorded.".into())),
            (Phase::Recording | Phase::Saving, Some(State::Failed(why))) => self.fail(why),
            _ => {}
        }
        None
    }

    /// The finished recording: its files with where each starts on the take's clock.
    fn take(&self, file: PathBuf, problem: Option<String>) -> Take {
        let outputs = self.session.as_ref().map(|s| s.outputs()).unwrap_or_default();
        let find = |role: Role| outputs.iter().find(|o| o.role == role).map(|o| (o.file.clone(), o.start));
        // (If the take could only be kept as it was written, its start is unknown: it comes in from the countdown's end.)
        Take { picture: find(Role::Screen).unwrap_or((file, self.go_clock)), mic: find(Role::Microphone), go: self.go_clock, problem }
    }

    pub fn view<'a>(&'a self, appear: f32) -> Element<'a, Message> {
        let m = |msg: Msg| Message::ScreenRecord(msg);
        let idle = self.phase == Phase::Idle;
        let ready = idle && self.error.is_none() && self.screen.is_some() && self.found.is_some();

        let header = row![
            container(icon(code::MONITOR, 16).color(ON_ACCENT)).center(34).style(style::logo),
            column![
                text("Record screen").font(INTER_SEMIBOLD).size(T_TITLE),
                text(format!("Saved to {}", widgets::fit_end(&self.dir.display().to_string(), 420.0, T_SMALL))).size(T_SMALL).color(TEXT_3).wrapping(text::Wrapping::None),
            ]
            .spacing(2)
            .width(Fill)
            .clip(true),
            widgets::icon_button(code::X, (!self.busy()).then(|| m(Msg::Close))),
        ]
        .spacing(S3)
        .align_y(Alignment::Center);

        // The stage: the screen (a still before recording, the small live picture during it).
        let shown = if self.busy() { self.live.as_ref().map(|(_, h)| h) } else { self.thumbnail.as_ref().map(|(_, h)| h) };
        let picture: Element<'a, Message> = match shown {
            Some(handle) => image(handle.clone()).width(Fill).height(Fill).content_fit(iced::ContentFit::Contain).into(),
            None => space().into(),
        };
        let note = |glyph: char, color: Color, words: String| -> Element<'a, Message> {
            container(column![icon(glyph, 22).color(color), text(words).size(T_SMALL).color(TEXT_2).align_x(Alignment::Center)].spacing(S2).align_x(Alignment::Center).max_width(380)).center(Fill).into()
        };
        let over: Element<'a, Message> = match (&self.error, self.phase) {
            (Some(why), _) => note(code::TRIANGLE_ALERT, DANGER, why.clone()),
            (None, Phase::Looking) => note(code::LOADER_CIRCLE, TEXT_3, "Looking for screens…".into()),
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
                    col = col.push(container(text("No picture is arriving from the screen").size(T_SMALL).color(Color::WHITE)).padding([5, 10]).style(badge_dark));
                }
                container(col).padding(S3).into()
            }
            (None, Phase::Saving) => note(code::LOADER_CIRCLE, TEXT_3, "Finishing the recording…".into()),
            (None, Phase::Idle) if self.thumbnail.is_none() && self.screen.is_some() => note(code::LOADER_CIRCLE, TEXT_3, "Looking at the screen…".into()),
            _ => space().into(),
        };
        let stage = container(stack![container(picture).center(Fill), over]).width(STAGE.0).height(STAGE.1).padding(2).clip(true).style(stage_style(self.phase == Phase::Recording));

        let screens: &[Screen] = self.found.as_ref().map_or(&[], |f| &f.screens);
        let screen_pick = pick_list(screens, self.screen.clone(), move |s| m(Msg::Screen(s)))
            .placeholder(if self.found.is_some() { "No screen found" } else { "Looking…" })
            .padding([7, 10])
            .text_size(T_BODY)
            .width(Fill)
            .style(style::select)
            .menu_style(style::select_menu);
        // The frame rate sits beside the screen, so the card is no taller than the camera dialog's.
        let rate_pick = pick_list(&RATES[..], Some(Rate(self.fps)), move |r| m(Msg::Fps(r))).padding([7, 10]).text_size(T_BODY).width(112).style(style::select).menu_style(style::select_menu);
        let screen_row = row![text("Screen").size(T_SMALL).color(TEXT_2).width(92), screen_pick, rate_pick].spacing(S2).align_y(Alignment::Center);

        let mics: &[MicChoice] = &self.mic_rows;
        let mic_now = if self.found.is_some() { Some(self.mic.clone().map_or(MicChoice::None, MicChoice::Device)) } else { None };
        let mic_pick = pick_list(mics, mic_now, move |c| m(Msg::Mic(c))).placeholder("Looking…").padding([7, 10]).text_size(T_BODY).width(Fill).style(style::select).menu_style(style::select_menu);
        let mic_row = row![text("Microphone").size(T_SMALL).color(TEXT_2).width(92), mic_pick, meter(if self.mic.is_some() { self.level } else { 0.0 })].spacing(S2).align_y(Alignment::Center);


        let (hint, hint_color) = match self.phase {
            Phase::Recording => ("Recording. The clips land on the timeline at the playhead when you stop.".to_string(), TEXT_3),
            Phase::Countdown(..) => ("Get ready…".to_string(), TEXT_3),
            Phase::Saving => (String::new(), TEXT_3),
            _ if self.mic_note.is_some() && self.mic.is_none() => (self.mic_note.clone().unwrap_or_default(), WARNING),
            _ => match &self.found {
                Some(_) if self.fps > 30 && self.software() => ("No graphics encoder was found, so this computer records 30 pictures a second.".to_string(), WARNING),
                Some(f) if f.screens.is_empty() => (String::new(), TEXT_3),
                Some(f) => (format!("Encoded by {}.", f.encoder.label()), TEXT_3),
                None => (String::new(), TEXT_3),
            },
        };
        let buttons: Element<'a, Message> = match self.phase {
            Phase::Recording => row![widgets::ghost_button(Some(code::TRASH_2), "Discard", Some(m(Msg::Discard))), widgets::stop_button(Some(code::SQUARE), "Stop and add to timeline", Some(m(Msg::Stop)))].spacing(S2).into(),
            Phase::Countdown(..) => widgets::secondary_button(None, "Cancel", Some(m(Msg::Discard))).into(),
            Phase::Saving => space().into(),
            Phase::Looking | Phase::Idle => {
                let mut r = row![widgets::ghost_button(None, "Close", Some(m(Msg::Close)))].spacing(S2);
                if self.error.is_some() {
                    r = r.push(widgets::secondary_button(Some(code::REFRESH_CW), "Try again", Some(m(Msg::Refresh))));
                } else {
                    r = r.push(widgets::primary_button(Some(code::CIRCLE), "Start recording", ready.then(|| m(Msg::Start))));
                }
                r.into()
            }
        };
        let footer = row![text(hint).size(T_CAPTION).color(hint_color).width(Fill), buttons].spacing(S3).align_y(Alignment::Center);

        let card = container(column![header, stage, screen_row, mic_row, widgets::divider(), footer].spacing(S4)).width(STAGE.0 + 2.0 * S6).padding(S6).style(style::popover);
        // A click outside closes it, except while something is being recorded.
        widgets::modal(card, if self.busy() { Message::Noop } else { m(Msg::Close) }, appear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::encoder::Path as EncoderPath;
    use crate::recorder::screen::FAKE_SCREEN;

    fn fake_found() -> Found {
        Found {
            screens: vec![
                Screen { id: FAKE_SCREEN.into(), label: "Test screen".into(), width: 1920, height: 1080, x: 0, y: 0 },
                Screen { id: FAKE_SCREEN.into(), label: "Second test screen".into(), width: 1920, height: 1080, x: 1920, y: 0 },
            ],
            mics: vec![Device { id: recorder::FAKE_MIC.into(), label: "Test microphone".into() }],
            encoder: Choice { encoder: Encoder::X264, path: EncoderPath::Memory },
        }
    }

    /// What the editor does with a request for a picture of a screen.
    fn serve_thumbnail(d: &mut ScreenDialog) {
        if let Some(screen) = d.take_thumbnail_request() {
            let result = recorder::screen::thumbnail(&screen).map(|(w, h, rgba)| (w, h, Bytes::from(rgba))).map_err(|e| e.to_string());
            let _ = d.update(Msg::Thumbnail(screen.id, result));
        }
    }

    /// Ticks the dialog like the editor does until `done`, or fails after `seconds`.
    fn tick_until(d: &mut ScreenDialog, what: &str, seconds: u64, done: impl Fn(&ScreenDialog) -> bool) -> Option<Outcome> {
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

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-screen-dialog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn records_the_remembered_screen_after_a_countdown() {
        let dir = scratch("flow");
        let remembered = ScreenChoice { screen: Some("Second test screen".into()), mic: None, no_mic: false, fps: 60 };
        let mut d = ScreenDialog::new(dir.clone(), remembered);
        assert!(d.take_device_request());
        assert!(!d.take_device_request(), "asked once");
        // Nothing can start before the screens are known.
        assert_eq!(d.update(Msg::Start), None);
        assert_eq!(d.phase, Phase::Looking);

        assert_eq!(d.update(Msg::Found(Ok(fake_found()))), None);
        // The screen used last time is chosen again; the microphone is the first there is.
        assert_eq!(d.screen.as_ref().map(|s| s.label.as_str()), Some("Second test screen"));
        assert_eq!(d.mic.as_ref().map(|c| c.label.as_str()), Some("Test microphone"));
        assert_eq!(d.fps, 60);
        // A picture of it is asked for, once, and shown when it arrives.
        assert!(d.thumbnail.is_none());
        serve_thumbnail(&mut d);
        assert!(d.thumbnail.is_some() && d.take_thumbnail_request().is_none());
        // Choosing another screen asks for its picture.
        let _ = d.update(Msg::Screen(fake_found().screens[0].clone()));
        assert!(d.thumbnail.is_none() && d.thumbnail_wanted.is_some());
        serve_thumbnail(&mut d);
        assert!(d.thumbnail.is_some());
        let _ = d.update(Msg::Fps(Rate(30)));

        // Start: a countdown while the recording comes on.
        assert_eq!(d.update(Msg::Start), None);
        assert!(matches!(d.phase, Phase::Countdown(_)) && d.busy());
        assert_eq!(d.update(Msg::Close), None, "already recording: Cancel, not Close");
        let pressed = Instant::now();
        tick_until(&mut d, "the recording to start", 60, |d| d.phase == Phase::Recording);
        assert!(pressed.elapsed().as_secs_f64() >= COUNTDOWN);
        // "Go" is a moment on the take's clock, after the countdown and before now.
        assert!(d.go_clock >= COUNTDOWN - 0.3 && d.go_clock < COUNTDOWN + 6.0, "go at {}", d.go_clock);
        assert_eq!(d.update(Msg::Close), None, "can't close while recording");
        // The choices stay put while recording.
        let _ = d.update(Msg::Screen(fake_found().screens[1].clone()));
        let _ = d.update(Msg::Fps(Rate(60)));
        assert_eq!((d.screen.as_ref().map(|s| s.label.as_str()), d.fps), (Some("Test screen"), 30));
        tick_until(&mut d, "a live picture and some sound", 20, |d| d.live.is_some() && d.level > 0.2);
        std::thread::sleep(Duration::from_millis(700));

        assert_eq!(d.update(Msg::Stop), None);
        assert_eq!(d.phase, Phase::Saving);
        let outcome = tick_until(&mut d, "the finished files", 60, |_| false).unwrap();
        let Outcome::Recorded(take) = outcome else { panic!("{outcome:?}") };
        assert_eq!(take.problem, None);
        assert_eq!(take.go, d.go_clock);
        assert_eq!(take.picture.0, dir.join("Screen recording 1.mp4"));
        let mic = take.mic.as_ref().expect("a microphone file");
        assert_eq!(mic.0, dir.join("Screen recording 1 microphone.m4a"));
        // Both started a moment after the take was prepared, and before its countdown ended.
        assert!(take.picture.1 > 0.0 && take.picture.1 < take.go + 1.0 && mic.1 > 0.0 && mic.1 < take.go + 1.0, "{take:?}");
        let info = crate::media::probe(&take.picture.0).unwrap();
        assert_eq!((info.width, info.height, info.has_audio), (1920, 1080, false));
        assert!(crate::media::probe(&mic.0).unwrap().has_audio);
        assert_eq!(d.choice(), ScreenChoice { screen: Some("Test screen".into()), mic: Some("Test microphone".into()), no_mic: false, fps: 30 });
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_cancelled_countdown_and_a_discarded_take_leave_no_file() {
        let dir = scratch("cancel");
        let mut d = ScreenDialog::new(dir.clone(), ScreenChoice { no_mic: true, ..Default::default() });
        let _ = d.update(Msg::Found(Ok(fake_found())));
        assert_eq!(d.screen.as_ref().map(|s| s.label.as_str()), Some("Test screen"));
        assert_eq!(d.mic, None, "\"No microphone\" is remembered too");
        assert_eq!(d.fps, 30);

        // Cancel during the countdown: the take is thrown away.
        let _ = d.update(Msg::Start);
        let _ = d.update(Msg::Discard);
        assert_eq!(d.phase, Phase::Idle);
        // Record, then discard the take.
        let _ = d.update(Msg::Start);
        tick_until(&mut d, "the recording to start", 60, |d| d.phase == Phase::Recording);
        let _ = d.update(Msg::Discard);
        assert_eq!(d.phase, Phase::Idle);
        assert_eq!(d.update(Msg::Close), Some(Outcome::Close));
        drop(d);
        let deadline = Instant::now() + Duration::from_secs(30);
        let empty = || std::fs::read_dir(&dir).map(|mut files| files.next().is_none()).unwrap_or(true);
        while !empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(empty(), "nothing was kept");

        // A microphone that can't be opened: the recording goes without it, and it's tried again next time.
        let mut broken = fake_found();
        broken.mics = vec![Device { id: recorder::FAKE_BROKEN_MIC.into(), label: "Unplugged headset".into() }];
        let mut d = ScreenDialog::new(dir.clone(), ScreenChoice::default());
        let _ = d.update(Msg::Found(Ok(broken)));
        assert!(d.mic.is_some());
        let _ = d.update(Msg::Start);
        tick_until(&mut d, "the microphone to be let go of", 60, |d| d.phase == Phase::Idle);
        assert_eq!((d.mic.as_ref(), d.error.as_ref()), (None, None), "no error over the picture");
        assert!(d.mic_note.as_deref().is_some_and(|n| n.contains("microphone")), "{:?}", d.mic_note);
        assert!(!d.choice().no_mic, "\"No microphone\" wasn't the user's choice");
        // Choosing a microphone setting again clears the note.
        let _ = d.update(Msg::Mic(MicChoice::None));
        assert_eq!(d.mic_note, None);
        assert!(d.choice().no_mic);

        // No screen at all: said plainly, and "Try again" looks once more.
        let mut none = ScreenDialog::new(dir.clone(), ScreenChoice::default());
        let _ = none.update(Msg::Found(Ok(Found { screens: Vec::new(), ..fake_found() })));
        assert!(none.error.as_deref().is_some_and(|e| e.contains("No screen")));
        assert_eq!(none.update(Msg::Start), None, "nothing to record");
        let _ = none.update(Msg::Refresh);
        assert!(none.take_device_request());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Stress: random clicks on everything, faster than anyone could, with stretches where the
    /// "undo" buttons are left alone so recordings do get made. The dialog must never get stuck, every
    /// recording it reports must be whole, and nothing else may be left on disk. `STUDIO_STRESS=5`
    /// runs five times as long.
    #[test]
    fn a_storm_of_clicks_never_wedges_the_screen_dialog() {
        let dir = scratch("storm");
        let steps = 260 * std::env::var("STUDIO_STRESS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
        let mut x = 0x5C4E_E4u64;
        let mut roll = move |sides: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % sides
        };
        let found = fake_found();
        let mut d = ScreenDialog::new(dir.clone(), ScreenChoice::default());
        let mut kept: Vec<PathBuf> = Vec::new();
        let mut closed = 0;
        let after = |d: &mut ScreenDialog, outcome: Option<Outcome>, kept: &mut Vec<PathBuf>, closed: &mut u32| {
            match outcome {
                Some(Outcome::Recorded(take)) => {
                    assert_eq!(take.problem, None);
                    assert!(take.go >= 0.0 && take.go < 30.0, "go {}", take.go);
                    kept.push(take.picture.0);
                    kept.extend(take.mic.map(|m| m.0));
                    *d = ScreenDialog::new(dir.clone(), d.choice());
                }
                Some(Outcome::Close) => {
                    *closed += 1;
                    *d = ScreenDialog::new(dir.clone(), d.choice());
                }
                None => {}
            }
            if d.take_device_request() {
                let _ = d.update(Msg::Found(Ok(fake_found())));
            }
            serve_thumbnail(d);
        };
        after(&mut d, None, &mut kept, &mut closed);
        let mut since = (d.phase, Instant::now());
        for step in 0..steps {
            let patient = step % 200 < 150;
            let msg = match roll(14) {
                0 => Msg::Screen(found.screens[roll(2) as usize].clone()),
                1 => Msg::Mic(MicChoice::Device(found.mics[0].clone())),
                2 => Msg::Mic(MicChoice::None),
                3 => Msg::Fps(Rate(if roll(2) == 0 { 30 } else { 60 })),
                4..=6 => Msg::Start,
                7 if d.phase == Phase::Recording && d.go.is_some_and(|go| go.elapsed() > Duration::from_millis(400)) => Msg::Stop,
                8 if !patient => Msg::Discard,
                9 if !patient => Msg::Refresh,
                10 if !patient => Msg::Close,
                _ => Msg::Tick,
            };
            let outcome = d.update(msg);
            after(&mut d, outcome, &mut kept, &mut closed);
            for _ in 0..roll(4) {
                std::thread::sleep(Duration::from_millis(roll(30)));
                let outcome = d.update(Msg::Tick);
                after(&mut d, outcome, &mut kept, &mut closed);
            }
            if since.0 != d.phase {
                since = (d.phase, Instant::now());
            }
            let limit = if d.phase == Phase::Idle || d.phase == Phase::Recording { 600 } else { 40 };
            assert!(since.1.elapsed() < Duration::from_secs(limit), "stuck in {:?} (error {:?})", d.phase, d.error);
            assert!(d.error.is_none(), "step {step}: {:?}", d.error);
            assert_eq!(d.busy(), matches!(d.phase, Phase::Countdown(_) | Phase::Recording | Phase::Saving));
        }
        // Wind down: let a countdown finish and stop a recording.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let msg = if d.phase == Phase::Recording { Msg::Stop } else { Msg::Tick };
            let outcome = d.update(msg);
            after(&mut d, outcome, &mut kept, &mut closed);
            if d.phase == Phase::Idle {
                break;
            }
            assert!(Instant::now() < deadline, "never settled: {:?}, error {:?}", d.phase, d.error);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(d.update(Msg::Close), Some(Outcome::Close));
        drop(d);
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
            assert!(info.duration > 0.2, "{file:?}: {info:?}");
        }
        assert!(!kept.is_empty(), "some recordings were made");
        println!("{steps} clicks: {} files kept, closed {closed} times", kept.len());
        std::fs::remove_dir_all(dir).ok();
    }
}
