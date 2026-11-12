//! Screen recording: the monitor list, what FFmpeg opens to record one, and splitting the finished
//! take into a picture file and a microphone file.
//!
//! One FFmpeg writes everything into one crash-safe Matroska file (like the camera does), stamped with
//! the wall clock so the real gap between picture and sound survives (see `docs/recording-plan.md`:
//! left alone FFmpeg puts each stream's first sample at 0). When the take ends, `finish` reads each
//! stream's start from that file and remuxes the picture and the microphone into their own files
//! without re-encoding, telling the editor how far apart they started.

use std::time::{SystemTime, UNIX_EPOCH};

use super::encoder::{self, Choice, Content, Encoder, Frames, Path as EncoderPath};
use super::*;

/// The stand-in monitor tests and snapshots use (`AIVE_RECORD_FAKE`): a moving test pattern.
pub const FAKE_SCREEN: &str = "test:screen";
/// Pictures a second of the small live picture: it only shows the take is alive.
const THUMBNAIL_FPS: u32 = 2;
/// Software (x264) encoding is limited to this long side and frame rate: more would fall behind.
const SOFTWARE_LONG_SIDE: u32 = 2560;
const SOFTWARE_FPS: u32 = 30;

/// One monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// What FFmpeg opens: the output number on Windows, the display position on X11.
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    /// Where it sits on the desktop (X11 needs it; Windows addresses a monitor by number).
    pub x: i32,
    pub y: i32,
}

impl std::fmt::Display for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// What to record.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenSetup {
    pub screen: Screen,
    /// `None` records the picture only.
    pub mic: Option<Device>,
    /// Pictures a second asked for (30 or 60); software encoding is held to 30.
    pub fps: u32,
}

/// The monitors connected right now.
pub fn screens() -> anyhow::Result<Vec<Screen>> {
    if std::env::var_os("AIVE_RECORD_FAKE").is_some() {
        return Ok(vec![Screen { id: FAKE_SCREEN.into(), label: "Test screen (1920×1080)".into(), width: 1920, height: 1080, x: 0, y: 0 }]);
    }
    system::screens()
}

fn label(n: usize, count: usize, width: u32, height: u32) -> String {
    if count > 1 { format!("Screen {} ({width}×{height})", n + 1) } else { format!("Screen ({width}×{height})") }
}

/// The picture size in FFmpeg's stream line: `Stream #0:0: Video: wrapped_avframe, bgra(pc, ...), 3200x2000 [SAR 1:1 DAR 8:5], ...`.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn parse_video_size(text: &str) -> Option<(u32, u32)> {
    let line = text.lines().find(|l| l.contains("Video:"))?;
    // The size is the first `WxH` after the codec, followed by a space, a comma or a bracket.
    line.split(|c: char| c == ' ' || c == ',').filter_map(|word| word.split_once('x')).find_map(|(w, h)| Some((w.parse::<u32>().ok().filter(|w| *w > 15)?, h.parse::<u32>().ok().filter(|h| *h > 15)?)))
}

/// Reads `xrandr --listactivemonitors`: ` 0: +*eDP-1 1920/344x1080/194+0+0  eDP-1`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_xrandr_monitors(text: &str) -> Vec<Screen> {
    let mut found: Vec<(String, u32, u32, i32, i32)> = Vec::new();
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(geometry) = words.iter().find(|w| w.contains('/') && w.contains('x') && w.contains('+')) else { continue };
        let Some((w, rest)) = geometry.split_once('x') else { continue };
        // `1080/194+0+0`: the height with its millimetres, then the position.
        let Some((h, at)) = rest.split_once('+') else { continue };
        let number = |s: &str| s.split('/').next().and_then(|n| n.parse::<u32>().ok());
        let mut position = at.split('+').map(|p| p.parse::<i32>().unwrap_or(0));
        let (x, y) = (position.next().unwrap_or(0), position.next().unwrap_or(0));
        if let (Some(w), Some(h), Some(name)) = (number(w), number(h), words.last()) {
            found.push((name.to_string(), w, h, x, y));
        }
    }
    let count = found.len();
    found.into_iter().enumerate().map(|(n, (name, width, height, x, y))| Screen { id: name.clone(), label: if count > 1 { format!("{name} ({width}×{height})") } else { label(n, 1, width, height) }, width, height, x, y }).collect()
}

/// Seconds since 1970, to the microsecond: the same clock the picture and the sound are stamped with.
pub(super) fn wall_clock() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// How a screen's pictures reach FFmpeg: a source inside the filter graph (Windows `ddagrab`, the
/// test pattern) or an input of its own (X11).
struct Video {
    /// Options and the input, when the capture is an input.
    args: Vec<String>,
    /// A graph start: a source filter, or the input's label.
    source: String,
    /// Whether the pictures are stamped by the graph (a source filter) or by the input.
    in_graph: bool,
    /// Pictures that live in graphics memory.
    frames: Frames,
}

/// What opens the screen's pictures, and which input numbers that takes.
fn video_for(setup: &ScreenSetup) -> anyhow::Result<Video> {
    let fps = setup.fps;
    if setup.screen.id == FAKE_SCREEN {
        // An input, so `-re` can pace it like a real capture (a source inside the graph runs flat out).
        let args = ["-use_wallclock_as_timestamps", "1", "-re", "-f", "lavfi", "-i", &format!("testsrc2=size=1920x1080:rate={fps}")].map(String::from).to_vec();
        return Ok(Video { args, source: "[0:v]".into(), in_graph: false, frames: Frames::Memory });
    }
    system::video(setup)
}

/// One picture of a screen for the dialog to show, as RGBA: its size and pixels. Takes a moment
/// (FFmpeg starts and grabs one frame), so call it away from the interface.
pub fn thumbnail(screen: &Screen) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let setup = ScreenSetup { screen: screen.clone(), mic: None, fps: 1 };
    let video = video_for(&setup)?;
    let (w, h) = preview_size(Some((screen.width, screen.height)));
    // A capture that lives in graphics memory is copied out; the picture is only a thumbnail.
    let download = if video.frames == Frames::Desktop { "hwdownload,format=bgra," } else { "" };
    let start = if video.in_graph { format!("{},", video.source) } else { video.source.clone() };
    let mut cmd = ffmpeg::ffmpeg()?;
    cmd.args(&video.args).arg("-filter_complex").arg(format!("{start}{download}scale={w}:{h}:flags=fast_bilinear,format=rgba[o]")).args(["-map", "[o]", "-frames:v", "1", "-f", "rawvideo", "pipe:1"]);
    let out = cmd.output().context("FFmpeg couldn't be started")?;
    anyhow::ensure!(out.stdout.len() == (w * h * 4) as usize, "The screen couldn't be shown ({})", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("no picture arrived").trim());
    Ok((w, h, out.stdout))
}

/// The microphone as an input, stamped with the wall clock. Its sound is `[{input}:a]`.
fn mic_args(mic: &Device) -> Vec<String> {
    let mut args: Vec<String> = ["-use_wallclock_as_timestamps", "1"].map(String::from).to_vec();
    #[cfg(any(test, debug_assertions))]
    if mic.id == FAKE_BROKEN_MIC {
        args.extend(["-f", "lavfi", "-i", "no_such_microphone"].map(String::from));
        return args;
    }
    if mic.id == FAKE_MIC {
        args.extend(["-re", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"].map(String::from));
        return args;
    }
    system::mic_args(mic, &mut args);
    args
}

/// The encoder for a screen: the first that works here, found by trying it (see `encoder`).
fn choice_for(setup: &ScreenSetup, frames: Frames) -> Choice {
    if setup.screen.id == FAKE_SCREEN && std::env::var_os("AIVE_RECORD_ENCODER").is_none() {
        // Tests and snapshots don't depend on the graphics chip.
        return Choice { encoder: Encoder::X264, path: EncoderPath::Memory };
    }
    encoder::pick(frames)
}

/// The encoder a recording of `screen` will use (the first time takes a second or two: it tries them).
pub fn encoder(screen: &Screen) -> Choice {
    let setup = ScreenSetup { screen: screen.clone(), mic: None, fps: 30 };
    choice_for(&setup, video_for(&setup).map_or(Frames::Memory, |v| v.frames))
}

/// Builds what FFmpeg is run with. Slow (it may try encoders), so it runs on the session's thread.
pub(super) fn plan(setup: &ScreenSetup, record: bool) -> anyhow::Result<Plan> {
    let video = video_for(setup)?;
    let choice = choice_for(setup, video.frames);
    Ok(build(setup, video, choice, record))
}

/// The plan for a given capture and encoder (what `plan` settles by looking at this computer).
fn build(setup: &ScreenSetup, video: Video, choice: Choice, record: bool) -> Plan {
    let mic = setup.mic.as_ref();
    // Software encoding is held to 1440p and 30 pictures a second: it would fall behind otherwise.
    let software = choice.encoder == Encoder::X264;
    let fps = if software { setup.fps.min(SOFTWARE_FPS) } else { setup.fps }.clamp(1, 60);
    let preview = preview_size(Some((setup.screen.width, setup.screen.height)));
    let t0_secs = (wall_clock() * 1e6).round() / 1e6;
    let t0 = format!("{t0_secs:.6}");

    let mut args: Vec<String> = vec!["-copyts".into()];
    args.extend(choice.global_args());
    args.extend(video.args.iter().cloned());
    // The video input, if the capture is one, comes first: the microphone is the next input.
    let mic_input = if video.in_graph { 0 } else { 1 };
    if let Some(mic) = mic {
        args.extend(mic_args(mic));
    }

    // The picture: stamped with the wall clock (a source filter's own clock starts at its first
    // picture, an input's with the input), made a steady rate, then split into the recording and
    // the small live picture.
    let stamped = if video.in_graph { format!("{},setpts='(time(0)-{t0})/TB'", video.source) } else { format!("{}setpts='PTS-{t0}/TB'", video.source) };
    let mut g = format!("{stamped},fps={fps}");
    // The live picture is made from the same pictures, copied out of graphics memory if need be.
    let in_gpu_memory = video.frames == Frames::Desktop;
    let (pw, ph) = preview;
    // (Pictures are dropped to the thumbnail's rate *before* they are copied out of graphics memory:
    // copying every picture and then dropping most of them doubled FFmpeg's processor time.)
    let thumbnail = format!("fps={THUMBNAIL_FPS},{}scale={pw}:{ph}:flags=fast_bilinear,format=rgba[pvo]", if in_gpu_memory { "hwdownload,format=bgra," } else { "" });
    // What the encoder is fed. Software: copied out of graphics memory if need be, then scaled to at
    // most 1440p with even sides and converted to BT.709 (see `Choice::filters` on why that is said
    // out loud). A graphics encoder takes the picture as the encoder module says.
    let feed = if software {
        let download = if in_gpu_memory { "hwdownload,format=bgra," } else { "" };
        format!("{download}scale=w='min({SOFTWARE_LONG_SIDE},iw)':h=-2:force_divisible_by=2:flags=bicubic:out_color_matrix=bt709:out_range=tv,format=yuv420p")
    } else if choice.filters().is_empty() {
        "null".to_string()
    } else {
        choice.filters()
    };
    if record {
        g.push_str(&format!(",split=2[vrec][vpv];[vrec]{feed}[rec];[vpv]{thumbnail}"));
    } else {
        g.push_str(&format!(",{thumbnail}"));
    }
    if mic.is_some() {
        let stamp = format!("[{mic_input}:a]asetpts='PTS-{t0}/TB'");
        if record {
            g.push_str(&format!(";{stamp},asplit=2[mic][alv];[mic]aresample=async=1[arec];[alv]{LEVEL_METER}"));
        } else {
            g.push_str(&format!(";{stamp},{LEVEL_METER}"));
        }
    }
    Plan { args, graph: g, video: choice.args(Content::Screen, fps), audio: mic.is_some(), preview, t0: Some(t0_secs), hold: None }
}

// What the files of a finished take hold. The temp file is `Screen recording N.recording.mkv`.

/// Each stream's `(kind, start)` in seconds, from the take's Matroska file.
fn stream_starts(file: &Path) -> anyhow::Result<Vec<(String, f64)>> {
    let out = ffmpeg::ffprobe()?.args(["-show_entries", "stream=codec_type,start_time", "-of", "json"]).arg(file).output().context("FFprobe couldn't be started")?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).context("The recording couldn't be read")?;
    Ok(json["streams"]
        .as_array()
        .map(|streams| streams.iter().filter_map(|s| Some((s["codec_type"].as_str()?.to_string(), s["start_time"].as_str()?.parse::<f64>().ok()?))).collect())
        .unwrap_or_default())
}

/// The microphone's file beside the picture's: `Screen recording 3.mp4` → `Screen recording 3 microphone.m4a`.
fn microphone_file(target: &Path) -> PathBuf {
    let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!("{stem} microphone.m4a"))
}

/// Turns the finished take into its files: the picture in `target`, the microphone beside it, each
/// starting at 0, with where each really started on the take's clock.
///
/// `end` is when Stop was pressed on that clock: nothing after it is kept. (The screen capture
/// keeps delivering pictures for about half a second while FFmpeg shuts down: footage from after
/// the click.) A take that ended by itself has no `end` and is kept whole.
///
/// The take is read once and nothing is re-encoded: a 10-minute take is split in under a second.
/// (The files are not rearranged for streaming over the web, `+faststart`: that writes each a
/// second time, and they are only ever read from disk.)
pub(super) fn finish(take: &Path, target: &Path, end: Option<f64>) -> anyhow::Result<Vec<Output>> {
    let starts = stream_starts(take)?;
    let start = |kind: &str| starts.iter().find(|(k, _)| k == kind).map(|(_, s)| *s);
    let video = start("video").context("The recording has no picture")?;
    let audio = start("audio");
    let sound = microphone_file(target);
    // A stop from before anything was recorded cuts nothing.
    let end = end.filter(|e| e.is_finite() && *e > video.max(audio.unwrap_or(video)) + 0.1);
    let mut cmd = ffmpeg::ffmpeg()?;
    // (With `-copyts` an output's `-to` counts on the take's own clock, so it is an end time.)
    cmd.arg("-copyts").arg("-i").arg(take);
    let output = |cmd: &mut std::process::Command, map: &str, from: f64, to: &Path| {
        cmd.args(["-map", map, "-c", "copy", "-output_ts_offset", &format!("{:.6}", -from)]);
        if let Some(end) = end {
            cmd.args(["-to", &format!("{end:.6}")]);
        }
        cmd.arg("-y").arg(to);
    };
    output(&mut cmd, "0:v", video, target);
    if let Some(audio) = audio {
        output(&mut cmd, "0:a", audio, &sound);
    }
    let out = cmd.output().context("FFmpeg couldn't be started")?;
    let whole = |file: &Path| std::fs::metadata(file).is_ok_and(|m| m.len() > 1024);
    if !out.status.success() || !whole(target) || (audio.is_some() && !whole(&sound)) {
        let _ = std::fs::remove_file(target);
        let _ = std::fs::remove_file(&sound);
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut outputs = vec![Output { file: target.to_path_buf(), role: Role::Screen, start: video }];
    outputs.extend(audio.map(|start| Output { file: sound, role: Role::Microphone, start }));
    Ok(outputs)
}

/// FFmpeg's complaints about a screen recording, in words a person can act on, and whether the
/// microphone was the problem.
pub(super) fn friendly(log: &[String], got_pictures: bool) -> (String, bool) {
    let all = log.join("\n").to_lowercase();
    // The screen is a filter or the first input; a failure to open the other input is the microphone's.
    let mic = all.contains("could not find audio") || all.contains("pa_simple_new") || all.contains("cannot open audio device") || (all.contains("error opening input") && !all.contains("x11grab") && !all.contains("display"));
    let words = if all.contains("no space left on device") {
        "The disk is full, so the recording couldn't be saved. Free some space, then record again."
    } else if mic {
        "That microphone couldn't be opened. It may have been unplugged: choose another microphone, or record without one."
    } else if all.contains("error opening output") {
        "The recording couldn't be saved: the Recordings folder can't be written to."
    } else if got_pictures {
        "The screen recording stopped by itself. The screen may have been locked or the display changed."
    } else if all.contains("ddagrab") || all.contains("duplication") || all.contains("access is denied") || all.contains("e_accessdenied") {
        "Windows didn't let the app record the screen. A locked screen, a secure prompt or another recorder can block it: close it and try again."
    } else if all.contains("cannot open display") || all.contains("x11grab") {
        "The screen couldn't be recorded. This needs an X11 desktop session (on Linux, Wayland sessions aren't supported yet)."
    } else {
        return match log.last() {
            Some(line) => (format!("The screen couldn't be recorded ({}).", line.split("] ").last().unwrap_or(line).trim()), false),
            None => ("The screen couldn't be recorded.".into(), false),
        };
    };
    (words.to_string(), mic)
}

#[cfg(windows)]
mod system {
    use super::*;

    /// Monitors are numbered by Desktop Duplication: try each until one isn't there.
    pub fn screens() -> anyhow::Result<Vec<Screen>> {
        let mut found: Vec<(u32, u32)> = Vec::new();
        for n in 0..8 {
            let out = ffmpeg::ffmpeg()?.args(["-loglevel", "info", "-filter_complex"]).arg(format!("ddagrab=output_idx={n}:framerate=1")).args(["-frames:v", "1", "-f", "null", "-"]).output().context("FFmpeg couldn't be started")?;
            match out.status.success().then(|| parse_video_size(&String::from_utf8_lossy(&out.stderr))).flatten() {
                Some(size) => found.push(size),
                None => break,
            }
        }
        anyhow::ensure!(!found.is_empty(), "No screen could be opened for recording");
        let count = found.len();
        Ok(found.into_iter().enumerate().map(|(n, (width, height))| Screen { id: n.to_string(), label: label(n, count, width, height), width, height, x: 0, y: 0 }).collect())
    }

    pub fn video(setup: &ScreenSetup) -> anyhow::Result<Video> {
        let source = format!("ddagrab=output_idx={}:framerate={}", setup.screen.id, setup.fps.clamp(1, 60));
        Ok(Video { args: Vec::new(), source, in_graph: true, frames: Frames::Desktop })
    }

    pub fn mic_args(mic: &Device, args: &mut Vec<String>) {
        // A generous buffer and small chunks: no dropped sound, little delay.
        args.extend(["-f", "dshow", "-rtbufsize", "256M", "-audio_buffer_size", "50", "-i"].map(String::from));
        args.push(format!("audio={}", mic.id));
    }
}

#[cfg(target_os = "linux")]
mod system {
    use super::*;

    pub fn screens() -> anyhow::Result<Vec<Screen>> {
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland") {
            anyhow::bail!("Screen recording needs an X11 session: Wayland isn't supported yet. Choose \"Ubuntu on Xorg\" (or similar) when signing in.");
        }
        let out = std::process::Command::new("xrandr").arg("--listactivemonitors").output().context("xrandr couldn't be started (install x11-xserver-utils)")?;
        let found = parse_xrandr_monitors(&String::from_utf8_lossy(&out.stdout));
        anyhow::ensure!(!found.is_empty(), "No screen could be found");
        Ok(found)
    }

    pub fn video(setup: &ScreenSetup) -> anyhow::Result<Video> {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        let s = &setup.screen;
        let args = ["-use_wallclock_as_timestamps", "1", "-f", "x11grab", "-framerate", &setup.fps.clamp(1, 60).to_string(), "-video_size", &format!("{}x{}", s.width, s.height), "-i", &format!("{display}+{},{}", s.x, s.y)].map(String::from).to_vec();
        Ok(Video { args, source: "[0:v]".into(), in_graph: false, frames: Frames::Memory })
    }

    pub fn mic_args(mic: &Device, args: &mut Vec<String>) {
        let (format, source) = match mic.id.strip_prefix("alsa:") {
            Some(source) => ("alsa", source),
            None => ("pulse", mic.id.as_str()),
        };
        args.extend(["-f".to_string(), format.to_string()]);
        if format == "pulse" {
            // Sound in 20 ms pieces, as for the camera: longer ones make the level meter lag.
            args.extend(["-fragment_size".to_string(), "3840".to_string()]);
        }
        args.extend(["-i".to_string(), source.to_string()]);
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod system {
    use super::*;

    pub fn screens() -> anyhow::Result<Vec<Screen>> {
        anyhow::bail!("Recording isn't available on this system yet")
    }

    pub fn video(_: &ScreenSetup) -> anyhow::Result<Video> {
        anyhow::bail!("Recording isn't available on this system yet")
    }

    pub fn mic_args(_: &Device, _: &mut Vec<String>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_setup(mic: bool, fps: u32) -> ScreenSetup {
        ScreenSetup { screen: Screen { id: FAKE_SCREEN.into(), label: "Test screen".into(), width: 1920, height: 1080, x: 0, y: 0 }, mic: mic.then(|| Device { id: FAKE_MIC.into(), label: "Test microphone".into() }), fps }
    }

    fn wait_for(what: &str, seconds: u64, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-screen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn reads_the_size_of_a_captured_screen() {
        let text = "Output #0, null, to 'pipe:':\n  Stream #0:0: Video: wrapped_avframe, bgra(pc, gbr/bt709/iec61966-2-1, progressive), 3200x2000 [SAR 1:1 DAR 8:5], q=2-31, 200 kb/s, 30 fps, 30 tbn";
        assert_eq!(parse_video_size(text), Some((3200, 2000)));
        assert_eq!(parse_video_size("Error opening input file"), None);
        assert_eq!(parse_video_size("  Stream #0:0: Video: h264, yuv420p, 1920x1080, 30 fps"), Some((1920, 1080)));
    }

    #[test]
    fn a_screen_can_be_shown_as_a_thumbnail() {
        let screen = screens().ok().filter(|_| std::env::var_os("AIVE_RECORD_FAKE").is_some()).and_then(|s| s.into_iter().next()).unwrap_or(fake_setup(false, 30).screen);
        let (w, h, rgba) = thumbnail(&screen).unwrap();
        assert_eq!((w, h, rgba.len()), (640, 360, 640 * 360 * 4));
        // The test pattern isn't blank.
        assert!(rgba.iter().step_by(4).any(|v| *v > 200) && rgba.iter().step_by(4).any(|v| *v < 60));
    }

    #[test]
    fn reads_the_monitors_xrandr_lists() {
        let text = "Monitors: 2\n 0: +*eDP-1 1920/344x1080/194+0+0  eDP-1\n 1: +HDMI-1 2560/597x1440/336+1920+0  HDMI-1\n";
        let found = parse_xrandr_monitors(text);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].id.as_str(), found[0].width, found[0].height, found[0].x, found[0].y), ("eDP-1", 1920, 1080, 0, 0));
        assert_eq!((found[1].id.as_str(), found[1].width, found[1].height, found[1].x, found[1].y), ("HDMI-1", 2560, 1440, 1920, 0));
        assert_eq!(found[1].label, "HDMI-1 (2560×1440)");
        // One monitor needs no name.
        assert_eq!(parse_xrandr_monitors(" 0: +*eDP-1 1920/344x1080/194+0+0  eDP-1")[0].label, "Screen (1920×1080)");
        assert!(parse_xrandr_monitors("Monitors: 0").is_empty());
    }

    #[test]
    fn the_picture_and_the_sound_are_stamped_with_the_wall_clock_and_never_re_timed() {
        let p = plan(&fake_setup(true, 30), true).unwrap();
        let args = p.args.join(" ");
        // Without wall-clock stamps the real gap between picture and sound is thrown away.
        assert!(args.contains("-copyts") && args.contains("-use_wallclock_as_timestamps 1"), "{args}");
        // (An input that is stamped by FFmpeg itself has the epoch in its times: subtract the start.)
        assert!(p.graph.contains("[0:v]setpts='PTS-") && p.graph.contains("[1:a]asetpts='PTS-"), "{}", p.graph);
        // A steady rate comes from the graph; `cfr` on the output would re-base the start to 0.
        assert!(p.graph.contains(",fps=30,"), "{}", p.graph);
        assert!(!p.video.join(" ").contains("cfr"), "{:?}", p.video);
        // The take, a small live picture and a level meter.
        for label in ["[rec]", "[pvo]", "[arec]", "[alvo]"] {
            assert!(p.graph.contains(label), "{label} missing from {}", p.graph);
        }
        assert!(p.audio && p.hold.is_none());
    }

    /// The Windows desktop capture is a source inside the graph and its pictures live in graphics memory.
    fn desktop() -> Video {
        Video { args: Vec::new(), source: "ddagrab=output_idx=0:framerate=60".into(), in_graph: true, frames: Frames::Desktop }
    }

    #[test]
    fn the_desktop_capture_is_stamped_in_the_graph_and_the_microphone_is_the_only_input() {
        let setup = fake_setup(true, 60);
        let p = build(&setup, desktop(), Choice { encoder: Encoder::Qsv, path: EncoderPath::Gpu }, true);
        // The source is stamped by the graph's own wall-clock, the microphone is input 0.
        assert!(p.graph.starts_with("ddagrab=output_idx=0:framerate=60,setpts='(time(0)-"), "{}", p.graph);
        assert!(p.graph.contains("[0:a]asetpts='PTS-"), "{}", p.graph);
        // Quick Sync keeps the pictures in graphics memory all the way, and converts them there to BT.709.
        assert!(p.graph.contains(",fps=60,split=2[vrec][vpv];[vrec]hwmap=derive_device=qsv,format=qsv,vpp_qsv=format=nv12:out_color_matrix=bt709:out_range=tv[rec];"), "{}", p.graph);
        // Only the thumbnail is copied out, and only the two pictures a second it shows: the rate
        // is cut *before* the copy (copying all of them doubled FFmpeg's processor time).
        assert!(p.graph.contains("[vpv]fps=2,hwdownload,format=bgra,scale="), "{}", p.graph);
        assert!(!p.graph.contains("hwdownload,format=bgra,fps="), "{}", p.graph);
        assert_eq!(p.video[..2], ["-c:v", "h264_qsv"]);
        // No graphics encoder: copied out first, then held to 1440p and 30 pictures a second.
        let p = build(&setup, desktop(), Choice { encoder: Encoder::X264, path: EncoderPath::Download }, true);
        assert!(p.graph.contains(",fps=30,split=2") && p.graph.contains("[vrec]hwdownload,format=bgra,scale=w='min(2560,iw)':h=-2:force_divisible_by=2:flags=bicubic:out_color_matrix=bt709:out_range=tv,format=yuv420p[rec]"), "{}", p.graph);
        // NVENC takes the capture's pictures as they are.
        let p = build(&setup, desktop(), Choice { encoder: Encoder::Nvenc, path: EncoderPath::Gpu }, true);
        assert!(p.graph.contains("[vrec]null[rec]"), "{}", p.graph);
    }

    #[test]
    fn without_a_microphone_there_is_no_sound_in_the_graph() {
        let p = plan(&fake_setup(false, 30), true).unwrap();
        assert!(!p.audio && !p.graph.contains("[arec]") && !p.graph.contains("[alvo]") && !p.args.join(" ").contains("sine"), "{}", p.graph);
        // Only a live picture, no file outputs.
        let p = plan(&fake_setup(true, 30), false).unwrap();
        assert!(!p.graph.contains("[rec]") && !p.graph.contains("[arec]") && p.graph.contains("[pvo]") && p.graph.contains("[alvo]"), "{}", p.graph);
    }

    #[test]
    fn software_encoding_is_held_to_1440p_and_30_pictures_a_second() {
        let p = plan(&fake_setup(false, 60), true).unwrap();
        assert!(p.graph.contains("fps=30,") && !p.graph.contains("fps=60"), "{}", p.graph);
        assert!(p.graph.contains("min(2560,iw)"), "{}", p.graph);
    }

    /// A take as FFmpeg leaves it: six seconds of picture from 1.4 s on the take's clock, sound from 1.0 s.
    fn made_take(dir: &Path, sound: bool) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let take = dir.join("Screen recording 1.recording.mkv");
        let mut cmd = ffmpeg::ffmpeg().unwrap();
        cmd.args(["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"]);
        let picture = "[0:v]setpts=PTS+1.4/TB,format=yuv420p[v]";
        cmd.arg("-filter_complex").arg(if sound { format!("{picture};[1:a]asetpts=PTS+1.0/TB[a]") } else { picture.to_string() });
        cmd.args(["-map", "[v]", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30"]);
        if sound {
            cmd.args(["-map", "[a]", "-c:a", "aac"]);
        }
        let out = cmd.args(["-t", "7.4", "-copyts", "-y"]).arg(&take).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        take
    }

    fn seconds(file: &Path) -> f64 {
        crate::media::probe(file).unwrap().duration
    }

    #[test]
    fn a_take_is_split_in_one_pass_and_cut_where_stop_was_pressed() {
        let dir = scratch("finish");
        let take = made_take(&dir, true);
        let target = dir.join("Screen recording 1.mp4");
        // Stop pressed at 5.0 s on the take's clock: 3.6 s of picture and 4.0 s of sound are kept.
        let outputs = finish(&take, &target, Some(5.0)).unwrap();
        assert_eq!(outputs.iter().map(|o| (o.role, (o.start * 10.0).round() / 10.0)).collect::<Vec<_>>(), [(Role::Screen, 1.4), (Role::Microphone, 1.0)]);
        let (picture, sound) = (seconds(&target), seconds(&outputs[1].file));
        assert!((picture - 3.6).abs() < 0.15 && (sound - 4.0).abs() < 0.15, "picture {picture}, sound {sound}");
        // Each file starts at its own 0.
        for (file, kind) in [(&target, "video"), (&outputs[1].file, "audio")] {
            let start = stream_starts(file).unwrap().into_iter().find(|(k, _)| k == kind).map(|(_, s)| s).unwrap();
            assert!(start.abs() < 0.05, "{file:?} starts at {start}");
        }
        // A take that ended by itself is kept whole; so is one "stopped" before it began.
        for end in [None, Some(0.5), Some(f64::NAN)] {
            let outputs = finish(&take, &target, end).unwrap();
            let (picture, sound) = (seconds(&target), seconds(&outputs[1].file));
            assert!(picture > 5.8 && sound > 6.2, "{end:?}: picture {picture}, sound {sound}");
        }
        // Without a microphone there is one file, and no microphone file appears.
        let silent = made_take(&dir.join("silent"), false);
        let target = dir.join("silent").join("Screen recording 1.mp4");
        let outputs = finish(&silent, &target, Some(4.4)).unwrap();
        assert_eq!(outputs.len(), 1);
        assert!((seconds(&target) - 3.0).abs() < 0.15 && !microphone_file(&target).exists());
        // Something that isn't a recording fails cleanly and leaves nothing behind.
        let junk = dir.join("junk.recording.mkv");
        std::fs::write(&junk, vec![7u8; 4096]).unwrap();
        let target = dir.join("junk.mp4");
        assert!(finish(&junk, &target, None).is_err());
        assert!(!target.exists() && !microphone_file(&target).exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_microphone_file_sits_beside_the_picture() {
        assert_eq!(microphone_file(Path::new("/r/Screen recording 3.mp4")), Path::new("/r/Screen recording 3 microphone.m4a"));
    }

    #[test]
    fn a_screen_with_a_microphone_is_recorded_into_two_files_that_start_together() {
        let dir = scratch("two-files");
        let file = next_screen_file(&dir);
        assert_eq!(file.file_name().unwrap(), "Screen recording 1.mp4");
        let session = Session::record_screen(&fake_setup(true, 30), file.clone());
        wait_for("the first picture", 30, || session.frame().is_some() || session.state().ended());
        assert_eq!(session.state(), State::Running);
        std::thread::sleep(Duration::from_millis(3000));
        assert!(session.level() > 0.1, "the microphone level is shown while recording: {}", session.level());
        session.stop();
        wait_for("the files", 60, || session.state().ended());
        assert_eq!(session.state(), State::Done { file: Some(file.clone()), problem: None });

        let outputs = session.outputs();
        assert_eq!(outputs.iter().map(|o| o.role).collect::<Vec<_>>(), [Role::Screen, Role::Microphone]);
        assert_eq!(outputs[1].file, microphone_file(&file));
        // They start on the take's clock a moment after it was prepared, and no more than a moment
        // apart (the test microphone is a lavfi source that starts about when the picture does).
        assert!(outputs.iter().all(|o| (0.0..10.0).contains(&o.start)) && (outputs[0].start - outputs[1].start).abs() < 1.5, "{outputs:?}");

        let picture = crate::media::probe(&file).unwrap();
        assert!((picture.width, picture.height) == (1920, 1080) && !picture.has_audio, "{picture:?}");
        assert!(picture.duration > 2.0, "{picture:?}");
        let sound = crate::media::probe(&outputs[1].file).unwrap();
        assert!(sound.has_audio && sound.duration > 2.0, "{sound:?}");
        // Each file starts at its own 0 (within a frame or two), whatever the gap was in the take.
        for (file, kind) in [(&file, "video"), (&outputs[1].file, "audio")] {
            let start = stream_starts(file).unwrap().into_iter().find(|(k, _)| k == kind).map(|(_, s)| s).unwrap();
            assert!(start.abs() < 0.1, "{file:?} starts at {start}");
        }
        // Nothing but the two finished files is left behind.
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["Screen recording 1 microphone.m4a", "Screen recording 1.mp4"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_screen_without_a_microphone_is_one_file_and_a_discarded_take_leaves_nothing() {
        let dir = scratch("one-file");
        let file = next_screen_file(&dir);
        let session = Session::record_screen(&fake_setup(false, 30), file.clone());
        wait_for("the first picture", 30, || session.frame().is_some() || session.state().ended());
        std::thread::sleep(Duration::from_millis(1500));
        session.stop();
        wait_for("the file", 60, || session.state().ended());
        let outputs = session.outputs();
        assert!(matches!(outputs.as_slice(), [Output { file: f, role: Role::Screen, start }] if *f == file && (0.0..10.0).contains(start)), "{outputs:?}");
        assert!(crate::media::probe(&file).unwrap().duration > 1.0);

        let second = next_screen_file(&dir);
        assert_eq!(second.file_name().unwrap(), "Screen recording 2.mp4");
        let session = Session::record_screen(&fake_setup(true, 30), second.clone());
        wait_for("the first picture", 30, || session.frame().is_some() || session.state().ended());
        session.discard();
        wait_for("the end", 60, || session.state().ended());
        assert_eq!(session.state(), State::Done { file: None, problem: None });
        assert!(session.outputs().is_empty());
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Screen recording 1.mp4"], "the discarded take left files behind");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_microphone_that_fails_is_told_apart_from_the_screen() {
        let dir = scratch("broken-mic");
        let mut setup = fake_setup(true, 30);
        setup.mic = Some(Device { id: FAKE_BROKEN_MIC.into(), label: "Unplugged".into() });
        let session = Session::record_screen(&setup, next_screen_file(&dir));
        wait_for("the end", 30, || session.state().ended());
        assert!(session.mic_failed(), "{:?}", session.state());
        assert!(matches!(session.state(), State::Failed(why) if why.contains("microphone")), "{:?}", session.state());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Developer tool: the real screen and microphone, with the encoder this computer picks. Prints
    /// what it measured and deletes the files (the pictures are never looked at). Environment:
    /// `STUDIO_RECORD_SCREEN` (number, default the first), `STUDIO_RECORD_MIC` (name, default the first
    /// microphone, `none` for no microphone), `STUDIO_RECORD_FPS` (default 30), `STUDIO_RECORD_SECONDS`
    /// (default 5), `AIVE_RECORD_ENCODER` (to compare encoders).
    /// `cargo test -p studio records_the_real_screen -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn records_the_real_screen() {
        use super::super::tests::{cpu_seconds, memory_mb, watch};
        let listing = Instant::now();
        let found = screens().unwrap();
        println!("screens listed in {:.2}s: {:?}", listing.elapsed().as_secs_f64(), found.iter().map(|s| &s.label).collect::<Vec<_>>());
        let screen = found[std::env::var("STUDIO_RECORD_SCREEN").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0)].clone();
        let mics = devices().unwrap().mics;
        let mic = match std::env::var("STUDIO_RECORD_MIC").ok().as_deref() {
            Some("none") => None,
            Some(name) => Some(mics.iter().find(|d| d.label == name).cloned().unwrap_or_else(|| panic!("no microphone named {name}"))),
            None => mics.first().cloned(),
        };
        let fps: u32 = std::env::var("STUDIO_RECORD_FPS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
        let seconds: f64 = std::env::var("STUDIO_RECORD_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(5.0);
        let setup = ScreenSetup { screen: screen.clone(), mic: mic.clone(), fps };
        let choice = choice_for(&setup, video_for(&setup).unwrap().frames);
        println!("recording {seconds}s of {} at {fps} fps, microphone: {}, encoder: {} ({:?})", screen.label, mic.as_ref().map_or("none", |m| m.label.as_str()), choice.label(), choice.path);

        let dir = std::env::temp_dir().join(format!("studio-screen-real-{}", std::process::id()));
        let file = next_screen_file(&dir);
        let opened = Instant::now();
        let session = Session::record_screen(&setup, file.clone());
        wait_for("the first picture", 60, || session.frame().is_some() || session.state().ended());
        println!("first picture after {:.2}s, state {:?}", opened.elapsed().as_secs_f64(), session.state());
        assert_eq!(session.state(), State::Running, "{:?}", session.shared.log.lock().unwrap());
        let before = memory_mb();
        let pid = session.shared.child.lock().unwrap().as_ref().map(|c| c.id()).unwrap_or(0);
        let cpu0 = cpu_seconds(pid);
        let watched = watch(&session, seconds);
        let cores = (cpu_seconds(pid) - cpu0) / seconds;
        println!("recording: {watched}");
        println!("FFmpeg used {:.0}% of a core over the take; this app's memory {before:.0} MB at the start, {:.0} MB at the end", cores * 100.0, memory_mb());
        let stopped = Instant::now();
        session.stop();
        wait_for("the files", 120, || session.state().ended());
        println!("stop: files ready after {:.2}s, ended as {:?}", stopped.elapsed().as_secs_f64(), session.state());
        assert!(matches!(session.state(), State::Done { file: Some(_), problem: None }), "{:?}", session.state());

        let outputs = session.outputs();
        for o in &outputs {
            let info = crate::media::probe(&o.file).unwrap();
            println!("{:?}: {} — {}x{} {:.2}s, sound {}, {} KB, starts {:.0} ms into the take", o.role, o.file.file_name().unwrap().to_string_lossy(), info.width, info.height, info.duration, info.has_audio, std::fs::metadata(&o.file).map(|m| m.len() / 1024).unwrap_or(0), o.start * 1000.0);
        }
        if let [a, b] = outputs.as_slice() {
            println!("the picture started {:.0} ms after the microphone", (a.start - b.start) * 1000.0);
            // Where each ends on the take's clock, against the moment Stop was pressed.
            let stop = (*session.shared.stopped.lock().unwrap()).zip(*session.shared.t0.lock().unwrap()).map(|(s, t0)| s - t0).unwrap_or(f64::NAN);
            let (picture_end, sound_end) = (a.start + crate::media::probe(&a.file).unwrap().duration, b.start + crate::media::probe(&b.file).unwrap().duration);
            println!("Stop pressed at {stop:.3}s on the take's clock; the picture ends at {picture_end:.3}s, the voice at {sound_end:.3}s ({:+.0} ms)", (sound_end - stop) * 1000.0);
        }
        // Pictures: how many, and how even.
        let times = ffmpeg::ffprobe().unwrap().args(["-select_streams", "v", "-show_entries", "packet=pts_time", "-of", "csv=p=0"]).arg(&file).output().unwrap();
        let mut times: Vec<f64> = String::from_utf8_lossy(&times.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
        let (min, max) = gaps.iter().fold((f64::MAX, 0f64), |(lo, hi), g| (lo.min(*g), hi.max(*g)));
        println!("{} pictures in {:.2}s ({:.1} a second); gaps between them: shortest {:.1} ms, longest {:.1} ms", times.len(), times.last().unwrap_or(&0.0) - times.first().unwrap_or(&0.0), times.len() as f64 / seconds, min * 1000.0, max * 1000.0);
        println!("picture {:.2}s, sound {:.2}s", super::super::tests::stream_seconds(&file, "v"), outputs.get(1).map_or(0.0, |o| super::super::tests::stream_seconds(&o.file, "a")));
        assert!(times.len() as f64 > seconds * fps as f64 * 0.8, "dropped pictures: {} of about {}", times.len(), seconds * fps as f64);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn complaints_about_a_screen_are_told_in_plain_words() {
        let say = |lines: &[&str], pictures: bool| friendly(&lines.iter().map(|l| l.to_string()).collect::<Vec<_>>(), pictures);
        assert!(say(&["[out#0 @ 0] Error muxing: No space left on device"], true).0.contains("disk is full"));
        assert!(say(&["[dshow @ 0] Could not find audio only device with name [x] among source devices of type audio."], false).1);
        assert!(say(&["[ddagrab @ 0] Duplication failed: E_ACCESSDENIED"], false).0.contains("Windows didn't let the app"));
        assert!(say(&[], true).0.contains("stopped by itself"));
        assert!(say(&[], false).0.starts_with("The screen couldn't be recorded"));
    }
}
