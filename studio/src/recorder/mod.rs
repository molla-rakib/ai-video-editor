//! Camera recording. One FFmpeg process opens the camera and the microphone, writes the
//! recording, and sends a small live picture (on its stdout) and the microphone level (on its
//! stderr) back to the app. Nothing here touches the project: the editor imports the finished
//! file like any other media.
//!
//! The recording is written as Matroska, which stays readable if the app or the computer dies
//! mid-recording, and is repackaged as MP4 (no re-encoding) once it ends cleanly.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use bytes::Bytes;

use crate::ffmpeg;

mod camera;
use camera::system;
/// Screen recording; the dialog that uses `screens` comes next.
#[allow(dead_code)]
pub mod screen;
/// Used by screen recording (the camera still has its own x264 settings).
#[allow(dead_code)]
pub mod encoder;

/// Live pictures a second sent back for the preview, at most (the recording keeps the camera's own rate).
const PREVIEW_FPS: u32 = 30;
/// Longest side of the live preview picture, in pixels: a little more than the dialog shows it at.
const PREVIEW_SIDE: u32 = 640;
/// How long FFmpeg gets to finish the file after being asked to stop, before it is ended.
const STOP_GRACE: Duration = Duration::from_secs(8);

/// Stand-ins used by tests and debug snapshots: a moving test pattern and a steady tone, so
/// nothing ever opens a real camera or microphone there.
pub const FAKE_CAMERA: &str = "test:camera";
pub const FAKE_MIC: &str = "test:microphone";
/// A stand-in microphone that fails to open, the way an unplugged one does
/// (`AIVE_RECORD_FAKE=broken-mic` in debug builds, to see the dialog's note).
#[cfg(any(test, debug_assertions))]
pub const FAKE_BROKEN_MIC: &str = "test:broken-microphone";

/// A camera or microphone: `id` is what FFmpeg opens, `label` what the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub label: String,
}

impl Device {
    fn named(name: &str) -> Device {
        Device { id: name.to_string(), label: name.to_string() }
    }
}

impl std::fmt::Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Devices {
    pub cameras: Vec<Device>,
    pub mics: Vec<Device>,
}

/// What to record and how the picture is turned.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub camera: Device,
    /// `None` records the picture only.
    pub mic: Option<Device>,
    /// Quarter turns clockwise (0–3): 1 or 3 makes an upright (portrait) video from a camera on its side.
    pub turns: u8,
    /// Flips the picture left to right, like a mirror.
    pub mirror: bool,
}

/// One picture of the live preview, as the camera sees it (not turned or mirrored: see `orient`).
#[derive(Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Shared, not copied, when it is handed on (to the dialog's picture).
    pub rgba: Bytes,
    /// Counts up with every new picture.
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// The camera is being opened.
    Starting,
    /// Pictures are arriving (and, when recording, being saved).
    Running,
    /// The recording ended and its file is being finished.
    Saving,
    /// Ended. `file` is the finished recording (none for a preview or a discarded recording);
    /// `problem` says what went wrong if it ended by itself but the file could be kept.
    Done { file: Option<PathBuf>, problem: Option<String> },
    Failed(String),
}

impl State {
    pub fn ended(&self) -> bool {
        matches!(self, State::Done { .. } | State::Failed(_))
    }
}

/// The cameras and microphones connected right now.
pub fn devices() -> anyhow::Result<Devices> {
    if let Some(fake) = std::env::var_os("AIVE_RECORD_FAKE") {
        #[cfg(debug_assertions)]
        if fake == "broken-mic" {
            return Ok(Devices { cameras: vec![Device { id: FAKE_CAMERA.into(), label: "Test camera".into() }], mics: vec![Device { id: FAKE_BROKEN_MIC.into(), label: "Unplugged headset".into() }] });
        }
        let _ = fake;
        return Ok(Devices {
            cameras: vec![Device { id: FAKE_CAMERA.into(), label: "Test camera".into() }, Device { id: FAKE_CAMERA.into(), label: "External test camera".into() }],
            mics: vec![Device { id: FAKE_MIC.into(), label: "Test microphone".into() }],
        });
    }
    system::devices()
}

/// The folder recordings go to: next to the project file, or in the projects folder for a
/// project that was never saved.
pub fn recordings_dir(project_path: Option<&Path>) -> PathBuf {
    project_path.and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(crate::settings::projects_dir).join("Recordings")
}

/// A file name in `dir` that isn't taken yet: "Camera recording 1.mp4", "Camera recording 2.mp4", …
pub fn next_file(dir: &Path) -> PathBuf {
    next_named(dir, "Camera recording")
}

/// The same for a screen recording: "Screen recording 1.mp4", …
pub fn next_screen_file(dir: &Path) -> PathBuf {
    next_named(dir, "Screen recording")
}

fn next_named(dir: &Path, name: &str) -> PathBuf {
    let writing = WRITING.lock().unwrap().clone();
    // (A screen recording also writes a microphone file beside its picture: that name must be free too.)
    let beside = |p: &Path| dir.join(format!("{} microphone.m4a", p.file_stem().unwrap_or_default().to_string_lossy()));
    (1..).map(|n| dir.join(format!("{name} {n}.mp4"))).find(|p| !p.exists() && !beside(p).exists() &&!temp_file(p).exists() && !p.with_extension("mkv").exists() && !writing.contains(&temp_file(p))).expect("an endless range")
}

/// Where a recording is written while it runs.
fn temp_file(target: &Path) -> PathBuf {
    target.with_extension("recording.mkv")
}

const TEMP_SUFFIX: &str = ".recording.mkv";

/// Recordings being written by this app right now (see `unfinished`).
static WRITING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Marks a recording's working file as being written until dropped.
struct Writing(PathBuf);

impl Writing {
    fn new(temp: &Path) -> Writing {
        WRITING.lock().unwrap().push(temp.to_path_buf());
        Writing(temp.to_path_buf())
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        let mut writing = WRITING.lock().unwrap();
        if let Some(i) = writing.iter().position(|p| *p == self.0) {
            writing.remove(i);
        }
    }
}

/// Recordings in `dir` that were never finished (the app was closed or crashed while
/// recording): working files nothing is writing to any more.
pub fn unfinished(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let writing = WRITING.lock().unwrap().clone();
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(TEMP_SUFFIX)))
        .filter(|p| !writing.contains(p))
        // Another copy of the app may still be writing it.
        .filter(|p| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > Duration::from_secs(10))))
        .collect();
    found.sort();
    found
}

/// Turns an unfinished recording into a normal file, like the end of a recording would have:
/// returns the finished file, or `None` when nothing usable was in it (it is deleted then).
pub fn finish_unfinished(temp: &Path) -> anyhow::Result<Option<PathBuf>> {
    if !std::fs::metadata(temp).is_ok_and(|m| m.len() > 1024) {
        let _ = std::fs::remove_file(temp);
        return Ok(None);
    }
    let name = temp.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = name.strip_suffix(TEMP_SUFFIX).unwrap_or(&name);
    let dir = temp.parent().unwrap_or(Path::new("."));
    let target = std::iter::once(dir.join(format!("{stem}.mp4")))
        .chain((1..).map(|n| dir.join(format!("{stem} (recovered {n}).mp4"))))
        .find(|p| !p.exists() && !p.with_extension("mkv").exists())
        .expect("an endless range");
    if repackage(temp, &target).is_ok() {
        let _ = std::fs::remove_file(temp);
        return Ok(Some(target));
    }
    // The Matroska file plays and imports just as well: keep it under a plain name.
    let kept = target.with_extension("mkv");
    std::fs::rename(temp, &kept).context("The unfinished recording couldn't be renamed")?;
    Ok(Some(kept))
}

/// Cameras held by a session of this app. A camera can only be opened by one program at a
/// time, and a stopped FFmpeg takes a moment to let go of it: a new session waits for that
/// instead of failing with "device busy" (closing and quickly reopening the Record dialog).
static HELD: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RELEASED: Condvar = Condvar::new();

struct Hold(String);

impl Hold {
    /// Waits until no other session holds `camera` (for as long as a stop can take at most),
    /// then holds it. `None` if this session was asked to stop while waiting.
    fn take(camera: &str, stopping: &AtomicBool) -> Option<Hold> {
        let deadline = Instant::now() + STOP_GRACE + Duration::from_secs(2);
        let mut held = HELD.lock().unwrap();
        while held.iter().any(|c| c == camera) {
            if stopping.load(Ordering::SeqCst) {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            held = RELEASED.wait_timeout(held, left.min(Duration::from_millis(100))).unwrap().0;
        }
        held.push(camera.to_string());
        Some(Hold(camera.to_string()))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap();
        if let Some(i) = held.iter().position(|c| *c == self.0) {
            held.remove(i);
        }
        RELEASED.notify_all();
    }
}

/// The preview picture as it will be recorded: mirrored first, then turned clockwise, the
/// same order FFmpeg applies to the file. Returns width, height and RGBA pixels.
pub fn orient(frame: &Frame, turns: u8, mirror: bool) -> (u32, u32, Bytes) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let turns = turns % 4;
    if (turns == 0 && !mirror) || w == 0 || h == 0 || frame.rgba.len() < w * h * 4 {
        return (frame.width, frame.height, frame.rgba.clone());
    }
    let (dw, dh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    // Where a pixel comes from in the camera's picture, as an index into its pixels.
    let source = |dx: usize, dy: usize| {
        let (sx, sy) = match turns {
            1 => (dy, h - 1 - dx),
            2 => (w - 1 - dx, h - 1 - dy),
            3 => (w - 1 - dy, dx),
            _ => (dx, dy),
        };
        (sy * w + if mirror { w - 1 - sx } else { sx }) as isize
    };
    let mut out = vec![0u8; dw * dh * 4];
    for (dy, row) in out.chunks_exact_mut(dw * 4).enumerate() {
        // Along a row the source moves in even steps (one pixel, or one line, forward or back).
        let first = source(0, dy);
        let step = if dw > 1 { source(1, dy) - first } else { 0 };
        for (dx, pixel) in row.chunks_exact_mut(4).enumerate() {
            let s = (first + step * dx as isize) as usize * 4;
            pixel.copy_from_slice(&frame.rgba[s..s + 4]);
        }
    }
    (dw as u32, dh as u32, out.into())
}

/// Told whenever a session has something new to show: a preview picture, a change of state.
static NEWS: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// Sets what is called (from the camera's threads) whenever a session has something new to
/// show, so the dialog redraws exactly then instead of asking over and over.
pub fn on_news(hook: Option<Box<dyn Fn() + Send>>) {
    *NEWS.lock().unwrap() = hook;
}

fn news() {
    if let Some(hook) = NEWS.lock().unwrap().as_ref() {
        hook();
    }
}

struct Shared {
    state: Mutex<State>,
    frame: Mutex<Option<Arc<Frame>>>,
    /// Microphone level 0..1 (the bits of an `f32`).
    level: AtomicU32,
    /// When the first picture arrived: the recording's time zero.
    started: Mutex<Option<Instant>>,
    last_frame: Mutex<Instant>,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    stopping: AtomicBool,
    discard: AtomicBool,
    /// It failed because of the microphone, not the camera.
    mic_failed: AtomicBool,
    /// The last things FFmpeg complained about.
    log: Mutex<Vec<String>>,
    /// The files a finished screen recording was split into (a camera recording has just its one).
    outputs: Mutex<Vec<Output>>,
    /// Where the take's clock counts from (see `Plan::t0`).
    t0: Mutex<Option<f64>>,
    /// The wall-clock second Stop was pressed: nothing recorded after it is kept.
    stopped: Mutex<Option<f64>>,
    /// How long a stopped take keeps recording before FFmpeg is told to finish. FFmpeg throws away
    /// the microphone's last buffered sound when it is told to stop (measured on Windows: the voice
    /// ended 310-320 ms before Stop was pressed, so a last word could be cut off). A screen take
    /// runs on past Stop and is cut back to it (`screen::finish`); the camera's countdown lead-in
    /// works differently and it stops at once.
    overrun: Duration,
}

/// See `Shared::overrun`: twice the sound that was measured to go missing.
const SCREEN_OVERRUN: Duration = Duration::from_millis(600);

/// What is recorded: they share the process handling in `run` and differ in what FFmpeg opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Camera,
    Screen,
}

/// What a file of a finished screen recording holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The picture (and, later, the computer's sound).
    Screen,
    /// The microphone, alone, so it can be treated apart from the rest.
    Microphone,
}

/// One file of a finished screen recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub file: PathBuf,
    pub role: Role,
    /// Where this file's first moment lies on the take's clock: seconds after the recording was
    /// prepared (`Session::clock` reads the same clock). The files of one take start at slightly
    /// different moments; the editor trims or delays each clip by the difference, so the picture and
    /// the voice stay where they really were.
    pub start: f64,
}

/// Everything that differs between recording a camera and recording a screen: what FFmpeg opens,
/// the filter graph, how the file is encoded. `run` handles the rest the same way for both.
struct Plan {
    /// Options and inputs, as FFmpeg is given them.
    args: Vec<String>,
    /// Ends in `[pvo]` (the live picture: RGBA, `preview` pixels), in `[alvo]` when there is `audio`
    /// (the level meter, printed on stderr) and, when recording, in `[rec]` and, with `audio`, `[arec]`.
    graph: String,
    /// The file's video options: encoder and quality.
    video: Vec<String>,
    audio: bool,
    preview: (u32, u32),
    /// The wall-clock second (since 1970) the take's own clock counts from, when it has one.
    t0: Option<f64>,
    /// A device only one program can open at a time, by name (the camera).
    hold: Option<String>,
}

/// A running camera or screen: a live picture, or a recording with its live picture.
pub struct Session {
    shared: Arc<Shared>,
    recording: bool,
}

impl Session {
    /// Opens the camera for a live preview only.
    pub fn preview(setup: &Setup) -> Session {
        let setup = setup.clone();
        Session::start(Kind::Camera, move |record| camera_plan(&setup, record), None)
    }

    /// Opens the camera and records to `file` (an `.mp4` path whose folder is created if needed).
    pub fn record(setup: &Setup, file: PathBuf) -> Session {
        let setup = setup.clone();
        Session::start(Kind::Camera, move |record| camera_plan(&setup, record), Some(file))
    }

    /// Records a screen (and a microphone) to `file`: the picture lands there, the microphone in a
    /// file beside it (see `outputs`). The live picture is a small, slow thumbnail.
    pub fn record_screen(setup: &screen::ScreenSetup, file: PathBuf) -> Session {
        let setup = setup.clone();
        Session::start(Kind::Screen, move |record| screen::plan(&setup, record), Some(file))
    }

    /// The files a finished screen recording was split into, with where each one starts.
    pub fn outputs(&self) -> Vec<Output> {
        self.shared.outputs.lock().unwrap().clone()
    }

    /// Seconds on the take's clock right now (what `Output::start` is measured on), once the
    /// recording has been prepared: the dialog notes it when its countdown ends.
    pub fn clock(&self) -> Option<f64> {
        let t0 = (*self.shared.t0.lock().unwrap())?;
        Some(screen::wall_clock() - t0)
    }

    fn start(kind: Kind, plan: impl FnOnce(bool) -> anyhow::Result<Plan> + Send + 'static, file: Option<PathBuf>) -> Session {
        let writing = file.as_deref().map(|f| Writing::new(&temp_file(f)));
        let shared = Arc::new(Shared {
            state: Mutex::new(State::Starting),
            frame: Mutex::new(None),
            level: AtomicU32::new(0),
            started: Mutex::new(None),
            last_frame: Mutex::new(Instant::now()),
            stdin: Mutex::new(None),
            child: Mutex::new(None),
            stopping: AtomicBool::new(false),
            discard: AtomicBool::new(false),
            mic_failed: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            outputs: Mutex::new(Vec::new()),
            t0: Mutex::new(None),
            stopped: Mutex::new(None),
            overrun: if kind == Kind::Screen && file.is_some() { SCREEN_OVERRUN } else { Duration::ZERO },
        });
        let recording = file.is_some();
        let worker = shared.clone();
        std::thread::spawn(move || {
            let end = run(&worker, kind, plan, file.as_deref());
            drop(writing);
            *worker.state.lock().unwrap() = end;
            news();
        });
        Session { shared, recording }
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn state(&self) -> State {
        self.shared.state.lock().unwrap().clone()
    }

    /// The newest preview picture, if any arrived yet.
    pub fn frame(&self) -> Option<Arc<Frame>> {
        self.shared.frame.lock().unwrap().clone()
    }

    /// Microphone level, 0 (silence) to 1 (as loud as it gets).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// When the first picture arrived (a recording's time zero).
    pub fn started(&self) -> Option<Instant> {
        *self.shared.started.lock().unwrap()
    }

    /// Whether it failed because the microphone couldn't be opened (the camera may be fine).
    pub fn mic_failed(&self) -> bool {
        self.shared.mic_failed.load(Ordering::SeqCst)
    }

    /// Whether pictures stopped arriving for a while although the camera is meant to be running.
    pub fn stalled(&self) -> bool {
        self.state() == State::Running && self.shared.last_frame.lock().unwrap().elapsed() > Duration::from_secs(3)
    }

    /// Ends the session; a recording is finished and kept. Returns at once: watch `state`.
    pub fn stop(&self) {
        if self.shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.shared.stopped.lock().unwrap() = Some(screen::wall_clock());
        // FFmpeg finishes its files when it reads a `q`. A screen take keeps recording a moment
        // longer first (`Shared::overrun`) and is cut back to this moment when it is finished;
        // a take being thrown away ends at once.
        let overrun = if self.shared.discard.load(Ordering::SeqCst) { Duration::ZERO } else { self.shared.overrun };
        let quit = |shared: &Shared| {
            if let Some(stdin) = shared.stdin.lock().unwrap().as_mut() {
                let _ = stdin.write_all(b"q\n").and_then(|_| stdin.flush());
            }
        };
        if overrun.is_zero() {
            quit(&self.shared);
        }
        // If it doesn't end by itself in time (a camera driver that hangs), end it.
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            if !overrun.is_zero() {
                // (A discard during the wait ends it at once.)
                let until = Instant::now() + overrun;
                while Instant::now() < until && !shared.discard.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                quit(&shared);
            }
            let deadline = Instant::now() + STOP_GRACE;
            while Instant::now() < deadline {
                // (Not yet started counts as running: FFmpeg may be starting this moment.)
                if shared.state.lock().unwrap().ended() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if let Some(child) = shared.child.lock().unwrap().as_mut() {
                let _ = child.kill();
            }
        });
    }

    /// Ends a recording and deletes what was recorded.
    pub fn discard(&self) {
        self.shared.discard.store(true, Ordering::SeqCst);
        self.stop();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A recording still running when its owner goes away is finished and kept, not lost.
        self.stop();
    }
}

/// What FFmpeg is told to open, and what is known about it.
struct Input {
    args: Vec<String>,
    /// The filter label of the microphone's sound ("0:a"), if a microphone is recorded.
    audio: Option<&'static str>,
    /// The camera picture's size, when known in advance.
    size: Option<(u32, u32)>,
    fps: u32,
}

fn input_for(setup: &Setup) -> anyhow::Result<Input> {
    if setup.camera.id == FAKE_CAMERA {
        let mut args: Vec<String> = ["-re", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30"].map(String::from).to_vec();
        #[cfg(any(test, debug_assertions))]
        if setup.mic.as_ref().is_some_and(|m| m.id == FAKE_BROKEN_MIC) {
            // Fails to open as input #1, like a microphone that went away.
            args.extend(["-f", "lavfi", "-i", "no_such_microphone"].map(String::from));
            return Ok(Input { args, audio: Some("1:a"), size: Some((1280, 720)), fps: 30 });
        }
        if setup.mic.is_some() {
            args.extend(["-re", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"].map(String::from));
        }
        return Ok(Input { args, audio: setup.mic.as_ref().map(|_| "1:a"), size: Some((1280, 720)), fps: 30 });
    }
    system::input(setup)
}

/// The size of the preview picture for a camera picture of `size` (or the usual 16:9 when unknown).
fn preview_size(size: Option<(u32, u32)>) -> (u32, u32) {
    let (w, h) = size.filter(|(w, h)| *w > 0 && *h > 0).unwrap_or((16, 9));
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if w >= h { (PREVIEW_SIDE, even(PREVIEW_SIDE as f64 * h as f64 / w as f64)) } else { (even(PREVIEW_SIDE as f64 * w as f64 / h as f64), PREVIEW_SIDE) }
}

/// The filter graph: the camera picture goes to the preview (small, as RGBA) and, when
/// recording, turned and mirrored to the file; the microphone goes to the file and a level meter.
fn graph(input: &Input, setup: &Setup, preview: (u32, u32), record: bool) -> String {
    let (pw, ph) = preview;
    let scale = if input.size.is_some() {
        format!("scale={pw}:{ph}:flags=fast_bilinear")
    } else {
        // Size unknown: fit inside the preview box and fill the rest with black.
        format!("scale={pw}:{ph}:force_original_aspect_ratio=decrease:flags=fast_bilinear,pad={pw}:{ph}:(ow-iw)/2:(oh-ih)/2")
    };
    let preview = format!("fps={PREVIEW_FPS},{scale},format=rgba[pvo]");
    let mut g = if record {
        let mut turn = String::new();
        if setup.mirror {
            turn.push_str("hflip,");
        }
        turn.push_str(match setup.turns % 4 {
            1 => "transpose=1,",
            2 => "transpose=1,transpose=1,",
            3 => "transpose=2,",
            _ => "",
        });
        format!("[0:v]split=2[cam][pv];[cam]{turn}scale=trunc(iw/2)*2:trunc(ih/2)*2[rec];[pv]{preview}")
    } else {
        format!("[0:v]{preview}")
    };
    if let Some(a) = input.audio {
        if record {
            g.push_str(&format!(";[{a}]asplit=2[mic][alv];[mic]aresample=async=1[arec];[alv]{LEVEL_METER}"));
        } else {
            g.push_str(&format!(";[{a}]{LEVEL_METER}"));
        }
    }
    g
}

/// The peak level 20 times a second, printed on stderr (the colon of `pipe:2` escaped twice: once for
/// the filter's option list, once for the graph), ending in the label `[alvo]`. `direct` writes each
/// line at once: buffered, they would arrive in a burst every several seconds.
const LEVEL_METER: &str = "astats=metadata=1:reset=1:length=0.05,ametadata=mode=print:key=lavfi.astats.Overall.Peak_level:direct=1:file=pipe\\\\:2[alvo]";

fn camera_plan(setup: &Setup, record: bool) -> anyhow::Result<Plan> {
    let input = input_for(setup)?;
    let preview = preview_size(input.size);
    // A steady frame rate and a keyframe every two seconds keep the file easy to edit.
    let fps = input.fps.to_string();
    let gop = (input.fps * 2).to_string();
    // "superfast" at quality 23 looks the same as "veryfast" at 20 on camera pictures (within
    // 0.1 dB) for a third to a half less processor time and a slightly larger file: a recording
    // must never fall behind the camera, and a busy processor's fan ends up in the microphone.
    let video = ["-c:v", "libx264", "-preset", "superfast", "-crf", "23", "-pix_fmt", "yuv420p", "-fps_mode", "cfr", "-r", &fps, "-g", &gop].map(String::from).to_vec();
    Ok(Plan { graph: graph(&input, setup, preview, record), args: input.args, video, audio: input.audio.is_some(), preview, t0: None, hold: (setup.camera.id != FAKE_CAMERA).then(|| setup.camera.id.clone()) })
}

/// Runs one session to its end and says how it ended.
fn run(shared: &Arc<Shared>, kind: Kind, plan: impl FnOnce(bool) -> anyhow::Result<Plan>, target: Option<&Path>) -> State {
    let fail = |e: anyhow::Error| State::Failed(format!("{e:#}"));
    let plan = match plan(target.is_some()) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let (pw, ph) = plan.preview;
    *shared.t0.lock().unwrap() = plan.t0;
    let temp = target.map(temp_file);
    if let Some(dir) = target.and_then(Path::parent) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return fail(anyhow::Error::new(e).context("The recordings folder couldn't be created"));
        }
    }
    let mut cmd = match ffmpeg::ffmpeg_live() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    cmd.args(&plan.args).arg("-filter_complex").arg(&plan.graph);
    if let Some(temp) = &temp {
        cmd.args(["-map", "[rec]"]);
        if plan.audio {
            cmd.args(["-map", "[arec]", "-c:a", "aac", "-b:a", "192k", "-ar", "48000"]);
        }
        cmd.args(&plan.video);
        // Written to disk every second: by default Matroska holds several seconds in memory,
        // so a camera that is unplugged (or an FFmpeg that is killed) early lost everything.
        cmd.args(["-cluster_time_limit", "1000", "-flush_packets", "1", "-y"]).arg(temp);
    }
    cmd.args(["-map", "[pvo]", "-f", "rawvideo", "pipe:1"]);
    if plan.audio {
        cmd.args(["-map", "[alvo]", "-f", "null", "-"]);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::platform::own_process_group(&mut cmd);
    // Linux: FFmpeg is told to finish (SIGTERM, like `q`) the moment this thread goes away,
    // the app quitting or crashing included, so the camera goes off with the app. (Windows:
    // the job object below.) This thread waits for FFmpeg, so it never ends first otherwise.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: only async-signal-safe calls between fork and exec.
        let parent = unsafe { libc::getpid() };
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 || libc::getppid() != parent {
                    return Err(std::io::Error::other("the app is gone"));
                }
                Ok(())
            });
        }
    }
    // Wait for an earlier session to let go of the camera (the test camera can be shared).
    let hold = match &plan.hold {
        None => None,
        Some(device) => match Hold::take(device, &shared.stopping) {
            Some(h) => Some(h),
            None => return State::Done { file: None, problem: None },
        },
    };
    // Asked to stop while the camera's details were being read: nothing to start.
    if shared.stopping.load(Ordering::SeqCst) {
        return State::Done { file: None, problem: None };
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(anyhow::Error::new(e).context("FFmpeg couldn't be started")),
    };
    // Ends with the app (a crash included), so a camera is never left switched on.
    let group = crate::platform::ProcessGroup::of(&child);
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    *shared.stdin.lock().unwrap() = child.stdin.take();
    *shared.child.lock().unwrap() = Some(child);
    // A stop that arrived between the check above and here found no stdin to write to.
    if shared.stopping.load(Ordering::SeqCst) {
        if let Some(stdin) = shared.stdin.lock().unwrap().as_mut() {
            let _ = stdin.write_all(b"q\n").and_then(|_| stdin.flush());
        }
    }

    let errors = {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let Some(stderr) = stderr else { return };
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(db) = line.strip_prefix("lavfi.astats.Overall.Peak_level=") {
                    let db: f32 = db.trim().parse().unwrap_or(f32::NEG_INFINITY);
                    let level = if db.is_nan() { 0.0 } else { ((db + 60.0) / 60.0).clamp(0.0, 1.0) };
                    shared.level.store(level.to_bits(), Ordering::Relaxed);
                } else if !line.starts_with("frame:") && !line.trim().is_empty() {
                    let mut log = shared.log.lock().unwrap();
                    log.push(line);
                    let extra = log.len().saturating_sub(20);
                    log.drain(..extra);
                }
            }
        })
    };

    // Preview pictures, one after another, until FFmpeg ends.
    if let Some(mut stdout) = stdout {
        // Room for a whole picture in the pipe, so FFmpeg hands each one over in one go.
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: a plain fcntl on a pipe we own; failing leaves the pipe as it was.
            unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETPIPE_SZ, 1 << 20) };
        }
        let mut seq = 0u64;
        loop {
            let mut rgba = vec![0u8; (pw * ph * 4) as usize];
            if stdout.read_exact(&mut rgba).is_err() {
                break;
            }
            seq += 1;
            if seq == 1 {
                *shared.started.lock().unwrap() = Some(Instant::now());
                let mut state = shared.state.lock().unwrap();
                if *state == State::Starting {
                    *state = State::Running;
                }
            }
            *shared.last_frame.lock().unwrap() = Instant::now();
            *shared.frame.lock().unwrap() = Some(Arc::new(Frame { width: pw, height: ph, rgba: rgba.into(), seq }));
            news();
        }
    }
    *shared.stdin.lock().unwrap() = None;
    let child = shared.child.lock().unwrap().take();
    if let Some(mut child) = child {
        let _ = child.wait();
        group.kill(&mut child);
    }
    drop(hold);
    let _ = errors.join();
    shared.level.store(0, Ordering::Relaxed);

    let asked = shared.stopping.load(Ordering::SeqCst);
    let got_pictures = shared.started.lock().unwrap().is_some();
    let (complaint, mic) = match kind {
        Kind::Camera => friendly(&shared.log.lock().unwrap(), got_pictures),
        Kind::Screen => screen::friendly(&shared.log.lock().unwrap(), got_pictures),
    };
    shared.mic_failed.store(mic && !asked, Ordering::SeqCst);
    let (Some(temp), Some(target)) = (temp, target) else {
        return if asked { State::Done { file: None, problem: None } } else { State::Failed(complaint) };
    };
    let recorded = got_pictures && std::fs::metadata(&temp).is_ok_and(|m| m.len() > 1024);
    if shared.discard.load(Ordering::SeqCst) || !recorded {
        let _ = std::fs::remove_file(&temp);
        return if asked { State::Done { file: None, problem: None } } else { State::Failed(complaint) };
    }
    // Whatever was recorded is kept, even when the camera went away by itself.
    *shared.state.lock().unwrap() = State::Saving;
    news();
    let problem = (!asked).then_some(complaint);
    let finished = match kind {
        Kind::Camera => repackage(&temp, target).map(|()| Vec::new()),
        // Cut where Stop was pressed (on the take's own clock); a take that ended by itself is kept whole.
        Kind::Screen => screen::finish(&temp, target, if asked { (*shared.stopped.lock().unwrap()).zip(plan.t0).map(|(stopped, t0)| stopped - t0) } else { None }),
    };
    match finished {
        Ok(outputs) => {
            let _ = std::fs::remove_file(&temp);
            *shared.outputs.lock().unwrap() = outputs;
            State::Done { file: Some(target.to_path_buf()), problem }
        }
        Err(_) => {
            // The Matroska file plays and imports just as well: keep it under a plain name.
            let kept = target.with_extension("mkv");
            match std::fs::rename(&temp, &kept) {
                Ok(()) => State::Done { file: Some(kept), problem },
                Err(_) => State::Done { file: Some(temp), problem },
            }
        }
    }
}

/// Repackages the finished recording as MP4 without re-encoding it.
fn repackage(from: &Path, to: &Path) -> anyhow::Result<()> {
    let out = ffmpeg::ffmpeg()?.arg("-i").arg(from).args(["-c", "copy", "-movflags", "+faststart", "-y"]).arg(to).output().context("FFmpeg couldn't be started")?;
    if !out.status.success() || !std::fs::metadata(to).is_ok_and(|m| m.len() > 1024) {
        let _ = std::fs::remove_file(to);
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// FFmpeg's complaints, in words a person can act on, and whether the microphone (rather than
/// the camera) was the problem.
fn friendly(log: &[String], got_pictures: bool) -> (String, bool) {
    let all = log.join("\n").to_lowercase();
    // DirectShow names the missing microphone; elsewhere the microphone is input #1 and the
    // camera input #0, so a failure to open input #1 is the microphone's.
    let mic = all.contains("could not find audio") || (all.contains("[in#1 ") && all.contains("error opening input")) || all.contains("pa_simple_new") || all.contains("cannot open audio device");
    let camera_said = |what: &str| all.lines().any(|l| l.contains(what) && !l.contains("[in#1 "));
    let words = if all.contains("no space left on device") {
        "The disk is full, so the recording couldn't be saved. Free some space, then record again."
    } else if mic {
        "That microphone couldn't be opened. It may have been unplugged: choose another microphone, or record without one."
    } else if all.contains("error opening output") {
        "The recording couldn't be saved: the Recordings folder can't be written to."
    } else if camera_said("permission denied") {
        if cfg!(windows) {
            "The app isn't allowed to use the camera. Allow camera access for desktop apps in Windows Settings > Privacy > Camera."
        } else {
            "The app isn't allowed to use the camera. Make sure your user can open the camera (on most systems, the \"video\" group)."
        }
    } else if all.contains("could not find video device") || all.contains("no such device") || camera_said("no such file or directory") {
        "That camera isn't connected any more. Plug it in, or choose another camera."
    } else if !got_pictures && (all.contains("could not run graph") || all.contains("i/o error") || all.contains("device or resource busy") || all.contains("in use")) {
        "The camera couldn't be started. Another app may be using it: close that app and try again."
    } else if got_pictures {
        "The camera stopped by itself. It may have been unplugged or taken over by another app."
    } else {
        return match log.last() {
            Some(line) => (format!("The camera couldn't be started ({}).", line.split("] ").last().unwrap_or(line).trim()), false),
            None => ("The camera couldn't be started.".into(), false),
        };
    };
    (words.to_string(), mic)
}

#[cfg(test)]
mod tests {
    use super::camera::*;
    use super::*;

    fn test_setup(turns: u8, mirror: bool, mic: bool) -> Setup {
        Setup { camera: Device { id: FAKE_CAMERA.into(), label: "Test camera".into() }, mic: mic.then(|| Device { id: FAKE_MIC.into(), label: "Test microphone".into() }), turns, mirror }
    }

    fn wait_for(what: &str, seconds: u64, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    #[test]
    fn reads_the_directshow_device_list() {
        let text = r#"[in#0 @ 000001e985282c00] "USB2.0 FHD UVC WebCam" (video)
[in#0 @ 000001e985282c00]   Alternative name "@device_pnp_\\?\usb#vid_3277&pid_0060&mi_00#6&174babf4&0&0000#{65e8773d-8f56-11d0-a3b9-00a0c9223196}\global"
[in#0 @ 000001e985282c00] "OBS Virtual Camera" (none)
[in#0 @ 000001e985282c00]   Alternative name "@device_sw_{860BB310-5D01-11D0-BD3B-00A0C911CE86}\{A3FCE0F5-3493-419F-958A-ABA1250EC20B}"
[in#0 @ 000001e985282c00] "Microphone Array (Intel® Smart Sound Technology for Digital Microphones)" (audio)
[in#0 @ 000001e985282c00] "Headset (QCY "MeloBuds" N20)" (audio)
Error opening input file dummy."#;
        let found = parse_dshow_devices(text);
        assert_eq!(found.cameras.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["USB2.0 FHD UVC WebCam", "OBS Virtual Camera"]);
        assert_eq!(found.mics.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["Microphone Array (Intel® Smart Sound Technology for Digital Microphones)", r#"Headset (QCY "MeloBuds" N20)"#]);
        assert_eq!(parse_dshow_devices("Error opening input file dummy."), Devices::default());
    }

    #[test]
    fn picks_a_smooth_full_hd_camera_mode() {
        let text = r#"[in#0 @ 0] DirectShow video device options (from video devices)
[in#0 @ 0]  Pin "Capture" (alternative pin name "Capture")
[in#0 @ 0]   pixel_format=yuyv422  min s=640x480 fps=15 max s=640x480 fps=30
[in#0 @ 0]   pixel_format=yuyv422  min s=1920x1080 fps=5 max s=1920x1080 fps=5
[in#0 @ 0]   vcodec=mjpeg  min s=1920x1080 fps=15 max s=1920x1080 fps=60
[in#0 @ 0]   vcodec=mjpeg  min s=3840x2160 fps=15 max s=3840x2160 fps=30
[in#0 @ 0]   pixel_format=nv12  min s=1280x720 fps=15 max s=1280x720 fps=30"#;
        let modes = parse_dshow_modes(text);
        assert_eq!(modes.len(), 5);
        // Full HD at 5 pictures a second is no use; the compressed Full HD mode is smooth.
        let best = best_mode(&modes).unwrap();
        assert_eq!((best.format.1.as_str(), best.width, best.height), ("mjpeg", 1920, 1080));
        // The same size and rate uncompressed wins over compressed.
        let both = parse_dshow_modes("x pixel_format=yuyv422  min s=1280x720 fps=30 max s=1280x720 fps=30\nx vcodec=mjpeg  min s=1280x720 fps=30 max s=1280x720 fps=30");
        assert_eq!(best_mode(&both).unwrap().format.0, "pixel_format");
        // A camera that only does 4K still records.
        let big = parse_dshow_modes("x vcodec=mjpeg  min s=3840x2160 fps=30 max s=3840x2160 fps=30");
        assert_eq!(best_mode(&big).map(|m| m.width), Some(3840));
        assert!(best_mode(&[]).is_none());
    }

    #[test]
    fn the_preview_is_mirrored_then_turned() {
        // Two pixels side by side: red on the left, blue on the right.
        let frame = Frame { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255].into(), seq: 1 };
        let red = [255, 0, 0, 255];
        assert_eq!(orient(&frame, 0, false), (2, 1, frame.rgba.clone()));
        // Mirrored: red moves to the right.
        assert_eq!(orient(&frame, 0, true).2[4..], red);
        // A quarter turn clockwise: the left edge becomes the top.
        let (w, h, turned) = orient(&frame, 1, false);
        assert_eq!((w, h), (1, 2));
        assert_eq!(turned[..4], red);
        // Counter-clockwise: the left edge becomes the bottom. Half a turn: left becomes right.
        assert_eq!(orient(&frame, 3, false).2[4..], red);
        assert_eq!(orient(&frame, 2, false).2[4..], red);
        assert_eq!(preview_size(Some((1920, 1080))), (640, 360));
        assert_eq!(preview_size(Some((1080, 1920))), (360, 640));
        assert_eq!(preview_size(None), (640, 360));
    }

    /// The average color of each quarter of a picture (top left, top right, bottom left, bottom right).
    fn quarters(w: u32, h: u32, rgba: &[u8]) -> [[f32; 3]; 4] {
        let mut sums = [[0f32; 3]; 4];
        let mut counts = [0f32; 4];
        for y in 0..h {
            for x in 0..w {
                let q = (if y < h / 2 { 0 } else { 2 }) + (if x < w / 2 { 0 } else { 1 });
                let i = ((y * w + x) * 4) as usize;
                for c in 0..3 {
                    sums[q][c] += rgba[i + c] as f32;
                }
                counts[q] += 1.0;
            }
        }
        std::array::from_fn(|q| sums[q].map(|s| s / counts[q].max(1.0)))
    }

    #[test]
    fn records_a_turned_mirrored_video_with_sound_that_matches_its_preview() {
        let dir = std::env::temp_dir().join(format!("studio-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = next_file(&dir);
        assert_eq!(file.file_name().unwrap(), "Camera recording 1.mp4");
        let setup = test_setup(1, true, true);
        let session = Session::record(&setup, file.clone());
        assert!(session.is_recording());
        wait_for("the first picture", 20, || session.frame().is_some());
        assert_eq!(session.state(), State::Running);
        assert!(session.started().is_some());
        // While it records the name is taken, so a second recording gets the next one.
        assert_eq!(next_file(&dir).file_name().unwrap(), "Camera recording 2.mp4");
        // The meter follows the sound closely: a level within a second and a half of the first picture.
        wait_for("the microphone level", 20, || session.level() > 0.3 || session.frame().is_some_and(|f| f.seq > PREVIEW_FPS as u64 * 3 / 2));
        assert!(session.level() > 0.3, "the level arrived late");
        wait_for("a second of video", 20, || session.frame().is_some_and(|f| f.seq >= PREVIEW_FPS as u64 * 3 / 2));
        assert!(!session.stalled());
        let preview = session.frame().unwrap();
        session.stop();
        wait_for("the file to be finished", 30, || session.state().ended());
        assert_eq!(session.state(), State::Done { file: Some(file.clone()), problem: None });
        assert!(!temp_file(&file).exists(), "the working file is gone");

        // Upright (the camera's 1280x720 turned a quarter), with picture and sound.
        let info = crate::media::probe(&file).unwrap();
        assert_eq!((info.width, info.height, info.has_audio), (720, 1280, true));
        assert!(info.duration > 1.0 && info.duration < 15.0, "duration {}", info.duration);
        // What the preview showed is what was recorded: same turn, same mirroring.
        let (w, h, shown) = orient(&preview, setup.turns, setup.mirror);
        assert_eq!((w, h), (360, 640));
        let saved = crate::media::thumbnail(&file, crate::model::MediaKind::Video, 0.5, w, h).unwrap();
        for (a, b) in quarters(w, h, &shown).iter().zip(quarters(w, h, &saved)) {
            assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 45.0), "preview {a:?} and recording {b:?} differ");
        }
        // And a quarter that would differ if the turn went the other way really does differ.
        let (_, _, wrong) = orient(&preview, 3, setup.mirror);
        let differs = quarters(w, h, &wrong).iter().zip(quarters(w, h, &saved)).any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 60.0));
        assert!(differs, "the test picture can tell the two turns apart");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_preview_saves_nothing_and_a_discarded_recording_is_deleted() {
        let dir = std::env::temp_dir().join(format!("studio-record-discard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Preview, without a microphone.
        let preview = Session::preview(&test_setup(0, false, false));
        wait_for("a preview picture", 20, || preview.frame().is_some());
        assert_eq!(preview.frame().map(|f| (f.width, f.height)), Some((640, 360)));
        assert_eq!(preview.level(), 0.0);
        preview.stop();
        wait_for("the preview to end", 20, || preview.state().ended());
        assert_eq!(preview.state(), State::Done { file: None, problem: None });
        assert!(!dir.exists(), "a preview writes nothing");

        // A recording thrown away.
        let file = next_file(&dir);
        let session = Session::record(&test_setup(0, false, true), file.clone());
        wait_for("the first picture", 20, || session.frame().is_some());
        session.discard();
        wait_for("the recording to end", 20, || session.state().ended());
        assert_eq!(session.state(), State::Done { file: None, problem: None });
        assert!(!file.exists() && !temp_file(&file).exists());

        // A camera that isn't there fails with a reason instead of hanging.
        let missing = Setup { camera: Device::named("No such camera 12345"), mic: None, turns: 0, mirror: false };
        let session = Session::preview(&missing);
        wait_for("the failure", 30, || session.state().ended());
        assert!(matches!(session.state(), State::Failed(_)), "{:?}", session.state());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_recording_whose_camera_dies_is_kept() {
        let dir = std::env::temp_dir().join(format!("studio-record-dies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = next_file(&dir);
        let session = Session::record(&test_setup(0, false, true), file.clone());
        wait_for("three seconds of video", 20, || session.frame().is_some_and(|f| f.seq >= PREVIEW_FPS as u64 * 3));
        // The camera goes away without FFmpeg finishing its file (as if it were unplugged, or
        // FFmpeg crashed): all but the last second or two is on disk by then.
        session.shared.child.lock().unwrap().as_mut().unwrap().kill().unwrap();
        wait_for("the recording to end", 30, || session.state().ended());
        let State::Done { file: Some(kept), problem: Some(problem) } = session.state() else { panic!("{:?}", session.state()) };
        assert!(problem.contains("stopped by itself"), "{problem}");
        assert!(!session.mic_failed());
        let info = crate::media::probe(&kept).unwrap();
        assert!(info.duration > 1.0 && info.has_audio, "{info:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_microphone_that_fails_is_told_apart_from_the_camera() {
        let mut setup = test_setup(0, false, true);
        setup.mic = Some(Device { id: FAKE_BROKEN_MIC.into(), label: "Gone".into() });
        let session = Session::preview(&setup);
        wait_for("the failure", 30, || session.state().ended());
        let State::Failed(why) = session.state() else { panic!("{:?}", session.state()) };
        assert!(why.contains("microphone"), "{why}");
        assert!(session.mic_failed());
    }

    #[test]
    fn ffmpeg_complaints_in_plain_words() {
        let say = |lines: &[&str], pictures: bool| friendly(&lines.iter().map(|l| l.to_string()).collect::<Vec<_>>(), pictures);
        // Linux: the camera held by another app, gone, or not allowed.
        let (busy, mic) = say(&["[in#0 @ 0x5cb0] Error opening input: Device or resource busy", "Error opening input file /dev/video0."], false);
        assert!(busy.contains("Another app") && !mic, "{busy}");
        let (gone, _) = say(&["[video4linux2,v4l2 @ 0x61] Cannot open video device /dev/video9: No such file or directory", "[in#0 @ 0x61] Error opening input: No such file or directory"], false);
        assert!(gone.contains("isn't connected"), "{gone}");
        let (denied, _) = say(&["[video4linux2,v4l2 @ 0x61] Cannot open video device /dev/video0: Permission denied"], false);
        assert!(denied.contains("isn't allowed"), "{denied}");
        // The microphone, on Linux (input #1) and Windows (DirectShow names it).
        let (linux_mic, mic) = say(&["[in#1 @ 0x56e9] Error opening input: Input/output error"], false);
        assert!(mic && linux_mic.contains("microphone"), "{linux_mic}");
        assert!(say(&["[dshow @ 0x1] Could not find audio only device with name [Headset] among source devices of type audio."], false).1);
        // Disk full, and a camera that stopped after pictures arrived.
        assert!(say(&["[matroska @ 0x1] Error writing packet: No space left on device"], true).0.contains("disk is full"));
        assert!(say(&["[in#0 @ 0x1] some read error"], true).0.contains("stopped by itself"));
    }

    #[test]
    fn a_camera_is_opened_by_one_session_at_a_time() {
        let stopping = AtomicBool::new(false);
        let first = Hold::take("/dev/video-test-hold", &stopping).unwrap();
        let waited = std::thread::spawn(|| {
            let since = Instant::now();
            let second = Hold::take("/dev/video-test-hold", &AtomicBool::new(false));
            (second.is_some(), since.elapsed())
        });
        std::thread::sleep(Duration::from_millis(300));
        drop(first);
        let (got, after) = waited.join().unwrap();
        assert!(got && after >= Duration::from_millis(250), "the second waited {after:?}");
        // A session stopped while it waits gives up instead.
        let _held = Hold::take("/dev/video-test-hold-2", &stopping).unwrap();
        assert!(Hold::take("/dev/video-test-hold-2", &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn an_unfinished_recording_is_recovered() {
        let dir = std::env::temp_dir().join(format!("studio-record-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // What a crash leaves behind: a working file, old enough that nothing is writing it.
        let temp = temp_file(&dir.join("Camera recording 1.mp4"));
        let out = ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=1", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "matroska", "-y"]).arg(&temp).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let empty = temp_file(&dir.join("Camera recording 2.mp4"));
        std::fs::write(&empty, b"").unwrap();
        assert!(unfinished(&dir).is_empty(), "too fresh: another copy of the app may be writing them");
        let old = std::time::SystemTime::now() - Duration::from_secs(60);
        for f in [&temp, &empty] {
            std::fs::File::options().write(true).open(f).unwrap().set_modified(old).unwrap();
        }
        assert_eq!(unfinished(&dir), vec![temp.clone(), empty.clone()]);
        // One being written by this app right now is left alone.
        {
            let _writing = Writing::new(&temp);
            assert_eq!(unfinished(&dir), vec![empty.clone()]);
        }
        assert_eq!(finish_unfinished(&empty).unwrap(), None, "nothing in it");
        assert!(!empty.exists());
        let kept = finish_unfinished(&temp).unwrap().unwrap();
        assert_eq!(kept, dir.join("Camera recording 1.mp4"));
        assert!(!temp.exists() && crate::media::probe(&kept).unwrap().duration > 0.5);
        assert!(unfinished(&dir).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reads_linux_cameras_and_microphones() {
        // This ThinkPad's colour camera, as its driver describes it: 720p only reaches 30
        // pictures a second compressed (raw manages 10), so compressed 720p is chosen.
        let raw = |w, h, fps: &[f64]| v4l2_mode(b"YUYV", w, h, fps).unwrap();
        let mjpg = |w, h| v4l2_mode(b"MJPG", w, h, &[30.0]).unwrap();
        let colour = vec![mjpg(1280, 720), mjpg(640, 480), raw(640, 480, &[30.0]), raw(960, 540, &[15.0]), raw(1280, 720, &[10.0])];
        let best = best_mode(&colour).unwrap();
        assert_eq!((best.format.1.as_str(), best.width, best.height, best.max_fps), ("mjpeg", 1280, 720, 30.0));
        assert_eq!(v4l2_mode(b"H264", 1920, 1080, &[30.0]), None, "FFmpeg can't take that format from the camera");
        assert_eq!(v4l2_mode(b"GREY", 640, 360, &[15.0, 30.0]).map(|m| (m.min_fps, m.max_fps)), Some((15.0, 30.0)));
        assert_eq!(v4l2_mode(b"NV12", 640, 480, &[]).map(|m| m.max_fps), Some(30.0));

        // Names: the kernel's cut-off repeat is dropped, other names are kept whole.
        assert_eq!(camera_label("Integrated Camera: Integrated C"), "Integrated Camera");
        assert_eq!(camera_label("HD Pro Webcam C920"), "HD Pro Webcam C920");
        assert_eq!(camera_label("Studio: Left"), "Studio: Left");

        // The infrared camera is left out, unless it's the only one.
        let device = |id: &str, label: &str| Device { id: id.into(), label: label.into() };
        let ir = (device("/dev/video2", "Integrated Camera"), vec![v4l2_mode(b"GREY", 640, 360, &[30.0]).unwrap()]);
        let both = colour_cameras(vec![(device("/dev/video0", "Integrated Camera"), colour.clone()), ir.clone()]);
        assert_eq!(both.iter().map(|(d, _)| d.id.as_str()).collect::<Vec<_>>(), ["/dev/video0"]);
        assert_eq!(colour_cameras(vec![ir.clone()]).len(), 1);
        // A camera whose driver couldn't be asked is kept.
        assert_eq!(colour_cameras(vec![(device("/dev/video4", "USB"), Vec::new()), ir]).len(), 1);
        let mut twins = vec![device("/dev/video0", "Webcam"), device("/dev/video2", "Webcam"), device("/dev/video4", "Other")];
        number_duplicates(&mut twins);
        assert_eq!(twins.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["Webcam", "Webcam (2)", "Other"]);

        // `ffmpeg -sources pulse` on the same machine: outputs' monitors left out, the default first.
        let text = "Auto-detected sources for pulse:
  alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__hw_sofhdadsp__sink.monitor [Monitor of Comet Lake PCH-LP cAVS Speaker + Headphones] (none)
  alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__hw_sofhdadsp__source [Comet Lake PCH-LP cAVS Headphones Stereo Microphone] (none)
* alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__hw_sofhdadsp_6__source [Comet Lake PCH-LP cAVS Digital Microphone] (none)
  alsa_output.usb-Logitech_Inc._Logi_USB_Headset_H340-00.analog-stereo.monitor [Monitor of Headset H340 Analog Stereo] (none)
  alsa_input.usb-Logitech_Inc._Logi_USB_Headset_H340-00.analog-stereo [Headset H340 Analog Stereo] (none)
";
        let mics = parse_pulse_sources(text);
        assert_eq!(mics.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["Comet Lake PCH-LP cAVS Digital Microphone", "Comet Lake PCH-LP cAVS Headphones Stereo Microphone", "Headset H340 Analog Stereo"]);
        assert_eq!(mics[2].id, "alsa_input.usb-Logitech_Inc._Logi_USB_Headset_H340-00.analog-stereo");
        assert!(parse_pulse_sources("Auto-detected sources for pulse:\n").is_empty());
    }

    /// Numbers that look random but are the same every run.
    fn dice(seed: u64) -> impl FnMut(u64) -> u64 {
        let mut x = seed.max(1);
        move |sides| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % sides.max(1)
        }
    }

    /// The files in `dir`, sorted.
    fn files_in(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.sort();
        files
    }

    /// Stress: sessions started, stopped, discarded and dropped at every awkward moment (before
    /// FFmpeg exists, while it starts, mid-picture, twice over), many at once. Every one must end,
    /// by being asked; kept recordings are whole; nothing else is left on disk.
    /// `STUDIO_STRESS=5` runs five times as many.
    #[test]
    fn a_storm_of_sessions_all_end_cleanly() {
        let dir = std::env::temp_dir().join(format!("studio-record-storm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rounds = 30 * std::env::var("STUDIO_STRESS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
        let mut roll = dice(0x5EED_CA3E);
        let mut sessions: Vec<Session> = Vec::new();
        // The file each session records to (none for a preview), in step with `sessions`.
        let mut targets: Vec<Option<PathBuf>> = Vec::new();
        for _ in 0..rounds {
            let setup = test_setup(roll(4) as u8, roll(2) == 0, roll(2) == 0);
            let session = if roll(3) == 0 {
                // A name is free again once its recording was thrown away, never while it runs.
                let file = next_file(&dir);
                assert!(!sessions.iter().zip(&targets).any(|(s, t)| t.as_ref() == Some(&file) && !s.state().ended()), "{file:?} is being recorded to already");
                targets.push(Some(file.clone()));
                Session::record(&setup, file)
            } else {
                targets.push(None);
                Session::preview(&setup)
            };
            std::thread::sleep(Duration::from_millis(roll(4) * roll(200)));
            match roll(5) {
                0 => session.stop(),
                1 => session.discard(),
                2 => {
                    session.stop();
                    session.discard();
                    session.stop();
                }
                // Dropped while it runs (the dialog closed): finished and kept, like a stop.
                3 => {
                    let shared = session.shared.clone();
                    let recording = session.recording;
                    drop(session);
                    sessions.push(Session { shared, recording });
                    continue;
                }
                // Left running until the end, alongside the next ones.
                _ => {}
            }
            sessions.push(session);
            // At most a handful of cameras running at once.
            while sessions.iter().filter(|s| !s.state().ended()).count() > 6 {
                sessions.iter().filter(|s| !s.state().ended()).for_each(|s| s.stop());
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        sessions.iter().for_each(|s| s.stop());
        wait_for("every session to end", 90, || sessions.iter().all(|s| s.state().ended()));
        let mut kept = Vec::new();
        for session in &sessions {
            assert!(session.shared.child.lock().unwrap().is_none() && session.shared.stdin.lock().unwrap().is_none(), "FFmpeg is gone");
            match session.state() {
                State::Done { file, problem: None } => kept.extend(file),
                other => panic!("a session that was asked to stop ended as {other:?} (log: {:?})", session.shared.log.lock().unwrap()),
            }
        }
        // What is on disk is exactly what the sessions say they kept, and each is a whole video.
        kept.sort();
        assert_eq!(files_in(&dir), kept, "no working files or strays are left");
        for file in &kept {
            let info = crate::media::probe(file).unwrap_or_else(|e| panic!("{file:?}: {e}"));
            assert!(info.duration > 0.0 && info.width > 0, "{file:?}: {info:?}");
        }
        assert!(!WRITING.lock().unwrap().iter().any(|p| p.starts_with(&dir)), "no name is still reserved");
        println!("{} sessions, {} recordings kept", sessions.len(), kept.len());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Developer tool, like `records_from_a_real_camera`: rough handling of a real camera. The
    /// preview and recordings are opened back to back, stopped early, discarded, with and
    /// without a microphone; then another program holds the camera.
    /// `cargo test -p studio a_real_camera_survives_rough_handling -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn a_real_camera_survives_rough_handling() {
        let found = devices().unwrap();
        let camera = found.cameras.first().cloned().expect("no camera found");
        let dir = std::env::temp_dir().join(format!("studio-record-rough-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut roll = dice(0xCA3E_2A);
        let began = Instant::now();
        let (mut kept, mut slowest) = (Vec::new(), 0f64);
        for round in 0..12 {
            // Any microphone, or none.
            let mic = found.mics.get(roll(found.mics.len() as u64 + 1) as usize).cloned();
            let setup = Setup { camera: camera.clone(), mic, turns: roll(4) as u8, mirror: roll(2) == 0 };
            let preview = Session::preview(&setup);
            if roll(3) > 0 {
                wait_for("the preview", 30, || preview.frame().is_some() || preview.state().ended());
                assert_eq!(preview.state(), State::Running, "round {round}: {:?}", preview.shared.log.lock().unwrap());
            }
            std::thread::sleep(Duration::from_millis(roll(400)));
            // Straight on to a recording: it waits for the preview to let go of the camera.
            preview.stop();
            let file = next_file(&dir);
            let opened = Instant::now();
            let session = Session::record(&setup, file.clone());
            wait_for("the first picture", 30, || session.frame().is_some() || session.state().ended());
            assert_eq!(session.state(), State::Running, "round {round}: {:?}", session.shared.log.lock().unwrap());
            slowest = slowest.max(opened.elapsed().as_secs_f64());
            std::thread::sleep(Duration::from_millis(300 + roll(1200)));
            let discard = roll(3) == 0;
            if discard { session.discard() } else { session.stop() }
            wait_for("the recording to end", 60, || session.state().ended());
            match session.state() {
                State::Done { file: None, problem: None } if discard => assert!(!file.exists()),
                State::Done { file: Some(f), problem: None } if !discard => kept.push(f),
                other => panic!("round {round} ended as {other:?}"),
            }
        }
        assert_eq!(files_in(&dir), kept, "no working files or strays are left");
        for file in &kept {
            let info = crate::media::probe(file).unwrap();
            assert!(info.duration > 0.2 && info.width > 0, "{file:?}: {info:?}");
        }
        println!("12 rounds in {:.1}s: {} recordings kept, none failed; slowest start {slowest:.2}s", began.elapsed().as_secs_f64(), kept.len());

        // Another program holds the camera: said in plain words, and it works again afterwards.
        let setup = Setup { camera: camera.clone(), mic: None, turns: 0, mirror: false };
        let input = input_for(&setup).unwrap();
        let mut other = ffmpeg::ffmpeg().unwrap().args(&input.args).args(["-t", "6", "-f", "null", "-"]).spawn().unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let blocked = Session::preview(&setup);
        wait_for("the refusal", 30, || blocked.state().ended());
        println!("while another program has the camera: {:?}", blocked.state());
        assert!(matches!(blocked.state(), State::Failed(why) if why.contains("Another app")), "{:?}", blocked.state());
        other.wait().unwrap();
        let again = Session::preview(&setup);
        wait_for("the preview", 30, || again.frame().is_some() || again.state().ended());
        assert_eq!(again.state(), State::Running);
        println!("after it let go: the camera opens again");
        std::fs::remove_dir_all(dir).ok();
    }

    /// CPU seconds a process has used so far (Linux and Windows; 0 elsewhere).
    #[cfg(not(windows))]
    pub(super) fn cpu_seconds(pid: u32) -> f64 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let fields: Vec<&str> = stat.rsplit(')').next().unwrap_or_default().split_whitespace().collect();
        let ticks = |i: usize| fields.get(i).and_then(|f| f.parse::<f64>().ok()).unwrap_or(0.0);
        (ticks(11) + ticks(12)) / 100.0
    }

    #[cfg(windows)]
    pub(super) fn cpu_seconds(pid: u32) -> f64 {
        #[repr(C)]
        #[derive(Default)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn GetProcessTimes(process: *mut c_void, created: *mut FileTime, exited: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        // SAFETY: plain Win32 calls on a handle opened and closed here, with structs of the size they expect.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return 0.0;
            }
            let (mut created, mut exited, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
            let ok = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
            CloseHandle(process);
            // 100-nanosecond ticks.
            let seconds = |t: &FileTime| (((t.high as u64) << 32) | t.low as u64) as f64 / 1e7;
            if ok == 0 { 0.0 } else { seconds(&kernel) + seconds(&user) }
        }
    }

    /// This process's memory in use, in MB (Linux and Windows; 0 elsewhere).
    #[cfg(not(windows))]
    pub(super) fn memory_mb() -> f64 {
        std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmRSS:")?.trim().trim_end_matches("kB").trim().parse::<f64>().ok())).unwrap_or(0.0) / 1024.0
    }

    #[cfg(windows)]
    pub(super) fn memory_mb() -> f64 {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            size: u32,
            page_faults: u32,
            peak_working_set: usize,
            working_set: usize,
            peak_paged_pool: usize,
            paged_pool: usize,
            peak_non_paged_pool: usize,
            non_paged_pool: usize,
            pagefile: usize,
            peak_pagefile: usize,
        }
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn K32GetProcessMemoryInfo(process: *mut c_void, counters: *mut Counters, size: u32) -> i32;
        }
        let mut counters = Counters { size: size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: the current-process pseudo handle and a #[repr(C)] struct of the size the call is told.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.size) };
        if ok == 0 { 0.0 } else { counters.working_set as f64 / (1024.0 * 1024.0) }
    }

    /// What a session did over some seconds: how evenly preview pictures arrived, what FFmpeg cost.
    pub(super) struct Watched {
        pictures: usize,
        seconds: f64,
        /// Milliseconds between one preview picture and the next: average and longest.
        gap: (f64, f64),
        /// FFmpeg's share of one processor core, in percent.
        cpu: f64,
        loudest: f32,
    }

    impl std::fmt::Display for Watched {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:.1} preview pictures a second (gaps: average {:.0} ms, longest {:.0} ms), FFmpeg {:.0}% of a core, loudest level {:.2}", self.pictures as f64 / self.seconds, self.gap.0, self.gap.1, self.cpu, self.loudest)
        }
    }

    pub(super) fn watch(session: &Session, seconds: f64) -> Watched {
        let pid = session.shared.child.lock().unwrap().as_ref().map(|c| c.id()).unwrap_or(0);
        let (since, cpu0) = (Instant::now(), cpu_seconds(pid));
        let (mut seq, mut last, mut gaps, mut loudest) = (session.frame().map_or(0, |f| f.seq), Instant::now(), Vec::new(), 0f32);
        while since.elapsed().as_secs_f64() < seconds && !session.state().ended() {
            let now = session.frame().map_or(0, |f| f.seq);
            if now != seq {
                gaps.push(last.elapsed().as_secs_f64() * 1000.0);
                (seq, last) = (now, Instant::now());
            }
            loudest = loudest.max(session.level());
            std::thread::sleep(Duration::from_millis(1));
        }
        let seconds = since.elapsed().as_secs_f64();
        Watched { pictures: gaps.len(), seconds, gap: (gaps.iter().sum::<f64>() / gaps.len().max(1) as f64, gaps.iter().copied().fold(0.0, f64::max)), cpu: (cpu_seconds(pid) - cpu0) / seconds * 100.0, loudest }
    }

    /// Each stream's length in seconds ("v" or "a"), to see that sound and picture end together.
    pub(super) fn stream_seconds(file: &Path, kind: &str) -> f64 {
        let out = ffmpeg::ffprobe().unwrap().args(["-select_streams", kind, "-show_entries", "stream=duration", "-of", "csv=p=0"]).arg(file).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0.0)
    }

    /// Developer tool: a real camera, to check that it opens with the settings chosen for it,
    /// how smooth the live picture is, what it costs and how long the file takes to finish.
    /// Prints what it measured and deletes the file. Without `STUDIO_RECORD_CAMERA` /
    /// `STUDIO_RECORD_MIC` it uses the first camera and microphone; `STUDIO_RECORD_SECONDS`
    /// sets the recording's length (default 3; use 180 for a long take).
    /// `STUDIO_RECORD_CAMERA="USB2.0 FHD UVC WebCam" STUDIO_RECORD_MIC="…" cargo test -p studio records_from_a_real_camera -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn records_from_a_real_camera() {
        let listing = Instant::now();
        let found = devices().unwrap();
        println!("devices listed in {:.2}s", listing.elapsed().as_secs_f64());
        println!("cameras: {:?}", found.cameras.iter().map(|d| (&d.label, &d.id)).collect::<Vec<_>>());
        println!("microphones: {:?}", found.mics.iter().map(|d| &d.label).collect::<Vec<_>>());
        let pick = |list: &[Device], name: Option<String>| match name {
            Some(name) => list.iter().find(|d| d.label == name).cloned().unwrap_or_else(|| panic!("no device named {name}")),
            None => list.first().cloned().expect("no device found"),
        };
        let setup = Setup { camera: pick(&found.cameras, std::env::var("STUDIO_RECORD_CAMERA").ok()), mic: Some(pick(&found.mics, std::env::var("STUDIO_RECORD_MIC").ok())), turns: 0, mirror: false };
        let seconds: f64 = std::env::var("STUDIO_RECORD_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
        println!("recording {seconds}s from {} with {}", setup.camera, setup.mic.as_ref().unwrap());

        // The live preview, as the dialog shows it before recording.
        let opened = Instant::now();
        let preview = Session::preview(&setup);
        wait_for("the preview", 30, || preview.frame().is_some() || preview.state().ended());
        assert_eq!(preview.state(), State::Running);
        println!("preview: first picture after {:.2}s", opened.elapsed().as_secs_f64());
        println!("preview: {}", watch(&preview, 3.0));
        preview.stop();

        // Opening the camera again right after stopping it waits for it to be free.
        let dir = std::env::temp_dir().join(format!("studio-record-real-{}", std::process::id()));
        let file = next_file(&dir);
        let opened = Instant::now();
        let session = Session::record(&setup, file.clone());
        wait_for("the first picture", 30, || session.frame().is_some() || session.state().ended());
        println!("recording: first picture {:.2}s after the preview was stopped, state {:?}", opened.elapsed().as_secs_f64(), session.state());
        assert_eq!(session.state(), State::Running);
        // (This process's memory, in MB: the live picture must not pile up over a long take.)
        let before = memory_mb();
        let watched = watch(&session, seconds);
        println!("recording: {watched}");
        println!("memory: {before:.0} MB at the start, {:.0} MB at the end", memory_mb());
        let stopped = Instant::now();
        session.stop();
        wait_for("the camera to finish", 60, || matches!(session.state(), State::Saving) || session.state().ended());
        let encoded = stopped.elapsed().as_secs_f64();
        wait_for("the file to be finished", 120, || session.state().ended());
        println!("stop: camera off after {encoded:.2}s, file ready after {:.2}s; ended as {:?}", stopped.elapsed().as_secs_f64(), session.state());
        let info = crate::media::probe(&file).unwrap();
        println!("recorded {}x{} at {:.2} fps, {:.2}s, sound: {}, {} KB", info.width, info.height, info.fps, info.duration, info.has_audio, std::fs::metadata(&file).map(|m| m.len() / 1024).unwrap_or(0));
        println!("picture {:.2}s, sound {:.2}s", stream_seconds(&file, "v"), stream_seconds(&file, "a"));
        // Pictures that differ from the one before: the camera's real rate (the file repeats
        // pictures to keep a steady rate when the camera is slower).
        let frames = |filter: &str| {
            let out = ffmpeg::ffmpeg().unwrap().arg("-i").arg(&file).args(["-an", "-vf", filter, "-f", "framemd5", "-"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with('#')).count()
        };
        let (all, distinct) = (frames("null"), frames("mpdecimate=hi=64:lo=32:frac=0.1"));
        println!("{all} pictures in the file, {distinct} distinct: about {:.1} real pictures a second", distinct as f64 / info.duration);
        assert!(info.has_audio && info.width > 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
