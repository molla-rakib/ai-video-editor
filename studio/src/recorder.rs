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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::ffmpeg;

/// Live pictures a second sent back for the preview (the recording keeps the camera's own rate).
const PREVIEW_FPS: u32 = 15;
/// Longest side of the live preview picture, in pixels.
const PREVIEW_SIDE: u32 = 480;
/// How long FFmpeg gets to finish the file after being asked to stop, before it is ended.
const STOP_GRACE: Duration = Duration::from_secs(8);

/// Stand-ins used by tests and debug snapshots: a moving test pattern and a steady tone, so
/// nothing ever opens a real camera or microphone there.
pub const FAKE_CAMERA: &str = "test:camera";
pub const FAKE_MIC: &str = "test:microphone";

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
    pub rgba: Vec<u8>,
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
    if std::env::var_os("AIVE_RECORD_FAKE").is_some() {
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
    (1..).map(|n| dir.join(format!("Camera recording {n}.mp4"))).find(|p| !p.exists() && !temp_file(p).exists() && !p.with_extension("mkv").exists()).expect("an endless range")
}

/// Where a recording is written while it runs.
fn temp_file(target: &Path) -> PathBuf {
    target.with_extension("recording.mkv")
}

/// The preview picture as it will be recorded: mirrored first, then turned clockwise, the
/// same order FFmpeg applies to the file. Returns width, height and RGBA pixels.
pub fn orient(frame: &Frame, turns: u8, mirror: bool) -> (u32, u32, Vec<u8>) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let turns = turns % 4;
    if (turns == 0 && !mirror) || frame.rgba.len() < w * h * 4 {
        return (frame.width, frame.height, frame.rgba.clone());
    }
    let (dw, dh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        for dx in 0..dw {
            // Where this pixel comes from in the (already mirrored) picture.
            let (sx, sy) = match turns {
                1 => (dy, h - 1 - dx),
                2 => (w - 1 - dx, h - 1 - dy),
                3 => (w - 1 - dy, dx),
                _ => (dx, dy),
            };
            let sx = if mirror { w - 1 - sx } else { sx };
            let (s, d) = ((sy * w + sx) * 4, (dy * dw + dx) * 4);
            out[d..d + 4].copy_from_slice(&frame.rgba[s..s + 4]);
        }
    }
    (dw as u32, dh as u32, out)
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
    /// The last things FFmpeg complained about.
    log: Mutex<Vec<String>>,
}

/// A running camera: a live preview, or a recording with its preview.
pub struct Session {
    shared: Arc<Shared>,
    recording: bool,
}

impl Session {
    /// Opens the camera for a live preview only.
    pub fn preview(setup: &Setup) -> Session {
        Session::start(setup.clone(), None)
    }

    /// Opens the camera and records to `file` (an `.mp4` path whose folder is created if needed).
    pub fn record(setup: &Setup, file: PathBuf) -> Session {
        Session::start(setup.clone(), Some(file))
    }

    fn start(setup: Setup, file: Option<PathBuf>) -> Session {
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
            log: Mutex::new(Vec::new()),
        });
        let recording = file.is_some();
        let worker = shared.clone();
        std::thread::spawn(move || {
            let end = run(&worker, &setup, file.as_deref());
            *worker.state.lock().unwrap() = end;
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

    /// Whether pictures stopped arriving for a while although the camera is meant to be running.
    pub fn stalled(&self) -> bool {
        self.state() == State::Running && self.shared.last_frame.lock().unwrap().elapsed() > Duration::from_secs(3)
    }

    /// Ends the session; a recording is finished and kept. Returns at once: watch `state`.
    pub fn stop(&self) {
        if self.shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        // FFmpeg finishes its files when it reads a `q`.
        if let Some(stdin) = self.shared.stdin.lock().unwrap().as_mut() {
            let _ = stdin.write_all(b"q\n").and_then(|_| stdin.flush());
        }
        // If it doesn't end by itself in time (a camera driver that hangs), end it.
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + STOP_GRACE;
            while Instant::now() < deadline {
                if shared.state.lock().unwrap().ended() || shared.child.lock().unwrap().is_none() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
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
        // The peak level 20 times a second, printed on stderr (the colon of `pipe:2` escaped twice:
        // once for the filter's option list, once for the graph). `direct` writes each line at
        // once: buffered, they would arrive in a burst every several seconds.
        let meter = "astats=metadata=1:reset=1:length=0.05,ametadata=mode=print:key=lavfi.astats.Overall.Peak_level:direct=1:file=pipe\\\\:2[alvo]";
        if record {
            g.push_str(&format!(";[{a}]asplit=2[mic][alv];[mic]aresample=async=1[arec];[alv]{meter}"));
        } else {
            g.push_str(&format!(";[{a}]{meter}"));
        }
    }
    g
}

/// Runs one session to its end and says how it ended.
fn run(shared: &Arc<Shared>, setup: &Setup, target: Option<&Path>) -> State {
    let fail = |e: anyhow::Error| State::Failed(format!("{e:#}"));
    let input = match input_for(setup) {
        Ok(i) => i,
        Err(e) => return fail(e),
    };
    let (pw, ph) = preview_size(input.size);
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
    cmd.args(&input.args).arg("-filter_complex").arg(graph(&input, setup, (pw, ph), temp.is_some()));
    if let Some(temp) = &temp {
        cmd.args(["-map", "[rec]"]);
        if input.audio.is_some() {
            cmd.args(["-map", "[arec]", "-c:a", "aac", "-b:a", "192k", "-ar", "48000"]);
        }
        // A steady frame rate and a keyframe every two seconds keep the file easy to edit.
        let fps = input.fps.to_string();
        let gop = (input.fps * 2).to_string();
        cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-fps_mode", "cfr", "-r", &fps, "-g", &gop, "-y"]).arg(temp);
    }
    cmd.args(["-map", "[pvo]", "-f", "rawvideo", "pipe:1"]);
    if input.audio.is_some() {
        cmd.args(["-map", "[alvo]", "-f", "null", "-"]);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::platform::own_process_group(&mut cmd);
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
            *shared.frame.lock().unwrap() = Some(Arc::new(Frame { width: pw, height: ph, rgba, seq }));
        }
    }
    *shared.stdin.lock().unwrap() = None;
    let child = shared.child.lock().unwrap().take();
    if let Some(mut child) = child {
        let _ = child.wait();
        group.kill(&mut child);
    }
    let _ = errors.join();
    shared.level.store(0, Ordering::Relaxed);

    let asked = shared.stopping.load(Ordering::SeqCst);
    let got_pictures = shared.started.lock().unwrap().is_some();
    let complaint = friendly(&shared.log.lock().unwrap(), got_pictures);
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
    let problem = (!asked).then_some(complaint);
    match repackage(&temp, target) {
        Ok(()) => {
            let _ = std::fs::remove_file(&temp);
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

/// FFmpeg's complaints, in words a person can act on.
fn friendly(log: &[String], got_pictures: bool) -> String {
    let all = log.join("\n").to_lowercase();
    if all.contains("could not find video device") || all.contains("no such device") {
        "That camera isn't connected any more. Plug it in, or choose another camera.".into()
    } else if all.contains("could not find audio") {
        "That microphone isn't connected any more. Choose another microphone, or record without one.".into()
    } else if !got_pictures && (all.contains("could not run graph") || all.contains("i/o error") || all.contains("device or resource busy") || all.contains("in use")) {
        "The camera couldn't be started. Another app may be using it: close that app and try again.".into()
    } else if got_pictures {
        "The camera stopped by itself. It may have been unplugged or taken over by another app.".into()
    } else {
        match log.last() {
            Some(line) => format!("The camera couldn't be started ({}).", line.split("] ").last().unwrap_or(line).trim()),
            None => "The camera couldn't be started.".into(),
        }
    }
}

/// One way a camera can deliver pictures, as DirectShow lists them.
#[derive(Debug, Clone, PartialEq)]
struct Mode {
    /// `("pixel_format", "yuyv422")` or `("vcodec", "mjpeg")`.
    format: (String, String),
    width: u32,
    height: u32,
    min_fps: f64,
    max_fps: f64,
}

/// Reads `ffmpeg -list_devices true -f dshow -i dummy`: names in quotes followed by their kind.
/// Devices that don't say what they are (virtual cameras) are offered as cameras.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn parse_dshow_devices(text: &str) -> Devices {
    let mut found = Devices::default();
    for line in text.lines().filter(|l| !l.contains("Alternative name")) {
        let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) else { continue };
        if b <= a + 1 {
            continue;
        }
        let device = Device::named(&line[a + 1..b]);
        match line[b + 1..].trim() {
            "(audio)" => found.mics.push(device),
            "(video)" | "(none)" => found.cameras.push(device),
            _ => {}
        }
    }
    found
}

/// Reads `ffmpeg -list_options true -f dshow -i video=…`.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn parse_dshow_modes(text: &str) -> Vec<Mode> {
    // "s=1920x1080 fps=30" -> (1920, 1080, 30.0)
    fn size_fps(part: &str) -> Option<(u32, u32, f64)> {
        let mut words = part.split_whitespace();
        let (w, h) = words.find_map(|x| x.strip_prefix("s="))?.split_once('x')?;
        let fps = words.find_map(|x| x.strip_prefix("fps="))?;
        Some((w.parse().ok()?, h.parse().ok()?, fps.parse().ok()?))
    }
    text.lines()
        .filter_map(|line| {
            let (head, rest) = line.split_once(" min ")?;
            let (min, max) = rest.split_once(" max ")?;
            let (key, value) = head.split_whitespace().last()?.split_once('=')?;
            if key != "pixel_format" && key != "vcodec" {
                return None;
            }
            let ((_, _, min_fps), (width, height, max_fps)) = (size_fps(min)?, size_fps(max)?);
            Some(Mode { format: (key.to_string(), value.to_string()), width, height, min_fps, max_fps })
        })
        .collect()
}

/// The mode to record with: the largest picture up to Full HD that reaches 30 pictures a
/// second (or the fastest there is), uncompressed rather than compressed when both will do.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn best_mode(modes: &[Mode]) -> Option<&Mode> {
    let fits = |m: &&Mode| m.width <= 1920 && m.height <= 1920;
    let pool: Vec<&Mode> = if modes.iter().any(|m| fits(&m)) { modes.iter().filter(fits).collect() } else { modes.iter().collect() };
    let score = |m: &Mode| {
        let fps = m.max_fps.min(30.0);
        // Smooth motion first (at least 24 pictures a second), then size, then rate, then uncompressed.
        (fps >= 24.0, if pool.iter().any(fits) { m.width * m.height } else { u32::MAX - m.width * m.height }, (fps * 100.0) as u32, m.format.0 == "pixel_format")
    };
    pool.iter().copied().max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
}

/// Camera modes already looked up (asking takes a moment and the answer doesn't change).
#[cfg_attr(not(windows), allow(dead_code))]
static MODES: Mutex<Option<HashMap<String, Option<Mode>>>> = Mutex::new(None);

#[cfg(windows)]
mod system {
    use super::*;

    pub fn devices() -> anyhow::Result<Devices> {
        let out = ffmpeg::ffmpeg()?.args(["-loglevel", "info", "-list_devices", "true", "-f", "dshow", "-i", "dummy"]).output().context("FFmpeg couldn't be started")?;
        Ok(parse_dshow_devices(&String::from_utf8_lossy(&out.stderr)))
    }

    fn mode_of(camera: &str) -> Option<Mode> {
        if let Some(known) = MODES.lock().unwrap().get_or_insert_with(HashMap::new).get(camera) {
            return known.clone();
        }
        let out = ffmpeg::ffmpeg().ok()?.args(["-loglevel", "info", "-list_options", "true", "-f", "dshow", "-i"]).arg(format!("video={camera}")).output().ok()?;
        let mode = best_mode(&parse_dshow_modes(&String::from_utf8_lossy(&out.stderr))).cloned();
        MODES.lock().unwrap().get_or_insert_with(HashMap::new).insert(camera.to_string(), mode.clone());
        mode
    }

    pub fn input(setup: &Setup) -> anyhow::Result<Input> {
        let mode = mode_of(&setup.camera.id);
        // A generous buffer and small audio chunks: no dropped pictures, sound in step with them.
        let mut args: Vec<String> = ["-f", "dshow", "-rtbufsize", "256M", "-audio_buffer_size", "50"].map(String::from).to_vec();
        let mut fps = 30;
        if let Some(m) = &mode {
            let rate = 30f64.clamp(m.min_fps.min(m.max_fps), m.max_fps);
            fps = rate.round().max(1.0) as u32;
            args.extend(["-video_size".to_string(), format!("{}x{}", m.width, m.height), "-framerate".to_string(), format!("{rate}"), format!("-{}", m.format.0), m.format.1.clone()]);
        }
        let mut source = format!("video={}", setup.camera.id);
        if let Some(mic) = &setup.mic {
            source.push_str(&format!(":audio={}", mic.id));
        }
        args.extend(["-i".to_string(), source]);
        Ok(Input { args, audio: setup.mic.as_ref().map(|_| "0:a"), size: mode.map(|m| (m.width, m.height)), fps })
    }
}

#[cfg(target_os = "linux")]
mod system {
    use super::*;

    /// Capture devices from `/sys/class/video4linux` (each camera's first node), by name.
    pub fn devices() -> anyhow::Result<Devices> {
        let mut cameras = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") {
            for entry in entries.flatten() {
                let node = entry.file_name().to_string_lossy().into_owned();
                // Nodes after the first of a camera carry metadata, not pictures.
                if std::fs::read_to_string(entry.path().join("index")).is_ok_and(|i| i.trim() != "0") {
                    continue;
                }
                let name = std::fs::read_to_string(entry.path().join("name")).map(|n| n.trim().to_string()).unwrap_or_else(|_| node.clone());
                cameras.push(Device { id: format!("/dev/{node}"), label: name });
            }
        }
        cameras.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Devices { cameras, mics: vec![Device { id: "default".into(), label: "Default microphone".into() }] })
    }

    pub fn input(setup: &Setup) -> anyhow::Result<Input> {
        let mut args: Vec<String> = ["-f", "v4l2", "-framerate", "30", "-video_size", "1280x720", "-i"].map(String::from).to_vec();
        args.push(setup.camera.id.clone());
        if let Some(mic) = &setup.mic {
            args.extend(["-f".to_string(), "pulse".to_string(), "-i".to_string(), mic.id.clone()]);
        }
        // The driver may pick another size than asked for, so the preview doesn't assume one.
        Ok(Input { args, audio: setup.mic.as_ref().map(|_| "1:a"), size: None, fps: 30 })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod system {
    use super::*;

    pub fn devices() -> anyhow::Result<Devices> {
        anyhow::bail!("Recording isn't available on this system yet")
    }

    pub fn input(_: &Setup) -> anyhow::Result<Input> {
        anyhow::bail!("Recording isn't available on this system yet")
    }
}

#[cfg(test)]
mod tests {
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
        let frame = Frame { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255], seq: 1 };
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
        assert_eq!(preview_size(Some((1920, 1080))), (480, 270));
        assert_eq!(preview_size(Some((1080, 1920))), (270, 480));
        assert_eq!(preview_size(None), (480, 270));
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
        assert_eq!((w, h), (270, 480));
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
        assert_eq!(preview.frame().map(|f| (f.width, f.height)), Some((480, 270)));
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

    /// Developer tool: a few seconds from a real camera, to check that it opens with the
    /// settings chosen for it. Prints what was recorded and deletes the file.
    /// `STUDIO_RECORD_CAMERA="USB2.0 FHD UVC WebCam" STUDIO_RECORD_MIC="…" cargo test -p studio records_from_a_real_camera -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn records_from_a_real_camera() {
        let camera = std::env::var("STUDIO_RECORD_CAMERA").expect("STUDIO_RECORD_CAMERA");
        let mic = std::env::var("STUDIO_RECORD_MIC").ok();
        let found = devices().unwrap();
        println!("cameras: {:?}", found.cameras.iter().map(|d| &d.label).collect::<Vec<_>>());
        println!("microphones: {:?}", found.mics.iter().map(|d| &d.label).collect::<Vec<_>>());
        let pick = |list: &[Device], name: &str| list.iter().find(|d| d.label == name).cloned().unwrap_or_else(|| panic!("no device named {name}"));
        let setup = Setup { camera: pick(&found.cameras, &camera), mic: mic.as_deref().map(|m| pick(&found.mics, m)), turns: 0, mirror: false };
        let dir = std::env::temp_dir().join(format!("studio-record-real-{}", std::process::id()));
        let file = next_file(&dir);
        let opened = Instant::now();
        let session = Session::record(&setup, file.clone());
        wait_for("the first picture", 30, || session.frame().is_some() || session.state().ended());
        println!("first picture after {:.1}s, state {:?}", opened.elapsed().as_secs_f64(), session.state());
        assert_eq!(session.state(), State::Running);
        let mut loudest = 0f32;
        while session.started().is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            loudest = loudest.max(session.level());
            std::thread::sleep(Duration::from_millis(30));
        }
        let pictures = session.frame().map_or(0, |f| f.seq);
        session.stop();
        wait_for("the file to be finished", 30, || session.state().ended());
        println!("ended as {:?}; {pictures} preview pictures; loudest microphone level {loudest:.2}", session.state());
        let info = crate::media::probe(&file).unwrap();
        println!("recorded {}x{} at {:.2} fps, {:.2}s, sound: {}, {} KB", info.width, info.height, info.fps, info.duration, info.has_audio, std::fs::metadata(&file).map(|m| m.len() / 1024).unwrap_or(0));
        std::fs::remove_dir_all(dir).ok();
    }
}
