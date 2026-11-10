//! Audio during preview playback. FFmpeg mixes the project from the playhead
//! (the same graph as export) into a buffer that the audio device drains; the
//! number of samples played is the master clock the video follows.

use std::collections::VecDeque;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::clip::Content;
use crate::ffmpeg;
use crate::model::Project;
use crate::render::audio;

/// Seconds of decoded audio kept ahead of the device.
const BUFFER_SECONDS: f64 = 1.5;
/// How long the picture waits for the first sound before it starts on its own.
const START_WAIT: Duration = Duration::from_millis(600);

struct Shared {
    samples: Mutex<VecDeque<f32>>,
    /// Frames (samples per channel) played or skipped: the audio clock.
    played: AtomicU64,
    muted: AtomicBool,
    finished: AtomicBool,
    stop: AtomicBool,
    /// Set when the picture started without the sound: from then on the sound skips
    /// whatever the picture already passed, so it joins in sync instead of late.
    wall_start: Mutex<Option<Instant>>,
    /// The sound is still skipping ahead to the picture.
    catching_up: AtomicBool,
    /// The audio device went away (unplugged or disabled).
    device_lost: AtomicBool,
    /// What FFmpeg reported on stderr, once it has ended.
    ffmpeg_error: Mutex<String>,
}

pub struct AudioPlayer {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
    child: Option<Child>,
    rate: u32,
    from: f64,
    opened: Instant,
    graph_file: std::path::PathBuf,
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&self.graph_file);
    }
}

/// Everything `AudioPlayer::start` builds before it opens the audio device: mixing the project,
/// spawning FFmpeg and querying the device. `cpal::Stream` can't cross threads, so this is the
/// part that's safe to prepare in the background; `finish` turns it into an `AudioPlayer` with
/// just the (fast) device open left, on whichever thread will own the player.
pub struct Prepared {
    device: cpal::Device,
    stream_config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    shared: Arc<Shared>,
    /// Taken by `finish`; left `None` there so `Drop` knows not to clean up a player that's now
    /// tracking the same process itself.
    child: Option<Child>,
    rate: u32,
    channels: usize,
    from: f64,
    graph_file: std::path::PathBuf,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        // Dropped without being finished (superseded by a newer play/pause/seek): clean up the
        // FFmpeg process and reader thread the same way a started AudioPlayer would. A finished
        // `Prepared` has already taken `child`, so this is a no-op for it.
        if let Some(mut c) = self.child.take() {
            self.shared.stop.store(true, Ordering::Relaxed);
            let _ = c.kill();
            let _ = c.wait();
            let _ = std::fs::remove_file(&self.graph_file);
        }
    }
}

/// The project with everything before `t` cut away and the rest moved to start at 0.
pub(crate) fn shifted(project: &Project, t: f64) -> Project {
    let mut p = project.clone();
    p.clips.retain(|c| c.end() > t + 1e-6);
    for c in &mut p.clips {
        if c.start < t {
            let cut = t - c.start;
            if let Content::Media { src_in, speed, .. } = &mut c.content {
                *src_in += cut * *speed;
            }
            c.duration -= cut;
            c.audio.fade_in = (c.audio.fade_in - cut).max(0.0);
            for g in &mut c.audio.gains {
                g.at -= cut;
            }
            c.start = 0.0;
        } else {
            c.start -= t;
        }
    }
    // A missing file would make FFmpeg fail the whole mix; the rest still plays without it.
    let missing: Vec<_> = p.assets.iter().filter(|a| !a.path.is_file()).map(|a| a.id).collect();
    p.clips.retain(|c| !matches!(c.content, Content::Media { asset, .. } if missing.contains(&asset)));
    p
}

/// Opens an output stream of sample type `T`, converting the f32 mix.
fn open_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut fill: impl FnMut(&mut [f32]) + Send + 'static,
    shared: Arc<Shared>,
) -> anyhow::Result<cpal::Stream> {
    let mut mix = Vec::new();
    let stream = device.build_output_stream(
        config,
        move |out: &mut [T], _| {
            mix.resize(out.len(), 0.0);
            fill(&mut mix);
            for (o, v) in out.iter_mut().zip(&mix) {
                *o = T::from_sample(v.clamp(-1.0, 1.0));
            }
        },
        move |e| {
            eprintln!("audio stream error: {e}");
            if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                shared.device_lost.store(true, Ordering::Relaxed);
            }
        },
        None,
    )?;
    Ok(stream)
}

impl AudioPlayer {
    /// Starts playing the project's sound from `from`. `Ok(None)` when nothing there makes a sound.
    /// Runs `prepare` and `finish` back to back; prefer calling them separately (`prepare` in the
    /// background, `finish` on the thread that will own the player) when startup latency matters.
    pub fn start(project: &Project, from: f64, muted: bool) -> anyhow::Result<Option<AudioPlayer>> {
        match Self::prepare(project, from, muted)? {
            Some(p) => Ok(Some(Self::finish(p)?)),
            None => Ok(None),
        }
    }

    /// Mixes the project, spawns FFmpeg and queries the audio device — safe to run off the UI
    /// thread. `Ok(None)` when nothing there makes a sound.
    pub fn prepare(project: &Project, from: f64, muted: bool) -> anyhow::Result<Option<Prepared>> {
        let total = project.duration() - from;
        if total <= 0.0 {
            return Ok(None);
        }
        let part = shifted(project, from);
        let (inputs, graph) = audio::mix_graph(&part, 0, total, None);
        if inputs.is_empty() {
            return Ok(None);
        }

        let host = cpal::default_host();
        let device = host.default_output_device().context("No audio output device")?;
        let config = device.default_output_config().context("The audio device has no usable format")?;
        let (rate, channels) = (config.sample_rate().0, config.channels() as usize);

        // FFmpeg reads this file after it starts, so it stays until playback stops.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let graph_file = ffmpeg::work_dir().join(format!("play-{}-{}.txt", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&graph_file, &graph)?;
        let mut cmd = ffmpeg::ffmpeg()?;
        for input in &inputs {
            cmd.args(["-ss", &ffmpeg::secs(input.seek), "-t", &ffmpeg::secs(input.length), "-i"]).arg(&input.path);
        }
        cmd.arg(ffmpeg::filter_script_option()).arg(&graph_file);
        cmd.args(["-map", "[aout]", "-f", "f32le", "-ac", &channels.to_string(), "-ar", &rate.to_string(), "pipe:1"]);
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("could not start FFmpeg")?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        let shared = Arc::new(Shared {
            samples: Mutex::new(VecDeque::new()),
            played: AtomicU64::new(0),
            muted: AtomicBool::new(muted),
            finished: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            wall_start: Mutex::new(None),
            catching_up: AtomicBool::new(false),
            device_lost: AtomicBool::new(false),
            ffmpeg_error: Mutex::new(String::new()),
        });
        let errors = shared.clone();
        let stderr_thread = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            *errors.ffmpeg_error.lock().unwrap() = s;
        });
        let reader = shared.clone();
        let cap = (rate as f64 * BUFFER_SECONDS) as usize * channels;
        std::thread::Builder::new().name("audio-reader".into()).spawn(move || {
            let mut buf = vec![0u8; 4096 * 4 * channels];
            // Bytes of a sample split across two reads.
            let mut carry: Vec<u8> = Vec::with_capacity(4);
            loop {
                if reader.stop.load(Ordering::Relaxed) {
                    break;
                }
                if reader.samples.lock().unwrap().len() > cap {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut bytes = std::mem::take(&mut carry);
                        bytes.extend_from_slice(&buf[..n]);
                        let whole = bytes.len() - bytes.len() % 4;
                        carry = bytes[whole..].to_vec();
                        let mut q = reader.samples.lock().unwrap();
                        q.extend(bytes[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
                    }
                }
            }
            // FFmpeg's messages are complete once its output has closed.
            drop(stdout);
            let _ = stderr_thread.join();
            reader.finished.store(true, Ordering::Relaxed);
        })?;

        let stream_config: cpal::StreamConfig = config.clone().into();
        Ok(Some(Prepared { device, stream_config, sample_format: config.sample_format(), shared, child: Some(child), rate, channels, from, graph_file }))
    }

    /// Opens the audio device and starts the stream from a `Prepared`. Fast (no FFmpeg or device
    /// enumeration left to do), but must run on the thread that will own the resulting player,
    /// since `cpal::Stream` can't move between threads.
    pub fn finish(mut p: Prepared) -> anyhow::Result<AudioPlayer> {
        let child = p.child.take().expect("a Prepared is only finished once");
        let shared = p.shared.clone();
        let graph_file = p.graph_file.clone();
        let (rate, channels, from) = (p.rate, p.channels, p.from);
        let out = shared.clone();
        let fill = move |data: &mut [f32]| {
            let mut q = out.samples.lock().unwrap();
            // Sound that arrived after the picture started: skip what the picture already showed.
            if out.catching_up.load(Ordering::Relaxed) {
                let behind = out.wall_start.lock().unwrap().map_or(0.0, |s| s.elapsed().as_secs_f64());
                let target = (behind * rate as f64) as u64;
                let played = out.played.load(Ordering::Relaxed);
                let skip = (target.saturating_sub(played) as usize).min(q.len() / channels);
                q.drain(..skip * channels);
                out.played.fetch_add(skip as u64, Ordering::Relaxed);
                if played + skip as u64 >= target {
                    out.catching_up.store(false, Ordering::Relaxed);
                } else {
                    data.fill(0.0);
                    return;
                }
            }
            let muted = out.muted.load(Ordering::Relaxed);
            let available = q.len().min(data.len()) / channels * channels;
            for (i, s) in data.iter_mut().enumerate() {
                *s = if i < available { q.pop_front().map(|v| if muted { 0.0 } else { v }).unwrap_or(0.0) } else { 0.0 };
            }
            out.played.fetch_add((available / channels) as u64, Ordering::Relaxed);
        };
        let lost = shared.clone();
        let stream = match p.sample_format {
            cpal::SampleFormat::F32 => open_stream::<f32>(&p.device, &p.stream_config, fill, lost)?,
            cpal::SampleFormat::F64 => open_stream::<f64>(&p.device, &p.stream_config, fill, lost)?,
            cpal::SampleFormat::I16 => open_stream::<i16>(&p.device, &p.stream_config, fill, lost)?,
            cpal::SampleFormat::I32 => open_stream::<i32>(&p.device, &p.stream_config, fill, lost)?,
            cpal::SampleFormat::U16 => open_stream::<u16>(&p.device, &p.stream_config, fill, lost)?,
            cpal::SampleFormat::U8 => open_stream::<u8>(&p.device, &p.stream_config, fill, lost)?,
            cpal::SampleFormat::I8 => open_stream::<i8>(&p.device, &p.stream_config, fill, lost)?,
            other => bail!("Unsupported audio sample format {other:?}"),
        };
        stream.play().context("could not start audio playback")?;
        Ok(AudioPlayer { _stream: stream, shared, child: Some(child), rate, from, opened: Instant::now(), graph_file })
    }

    /// Timeline position by the audio clock, once sound has started in step with the picture.
    pub fn position(&self) -> Option<f64> {
        let played = self.shared.played.load(Ordering::Relaxed);
        (played > 0 && !self.shared.catching_up.load(Ordering::Relaxed)).then(|| self.from + played as f64 / self.rate as f64)
    }

    /// True if the picture should run on a wall clock: the sound hasn't started in time,
    /// or is still catching up after `join_at`.
    pub fn stalled(&self) -> bool {
        let s = &self.shared;
        s.catching_up.load(Ordering::Relaxed) || (s.played.load(Ordering::Relaxed) == 0 && (self.opened.elapsed() > START_WAIT || s.finished.load(Ordering::Relaxed)))
    }

    /// The picture started without the sound at `wall_start` (at the player's `from`).
    /// The sound joins in sync when it arrives, after which `position` takes over again.
    pub fn join_at(&self, wall_start: Instant) {
        let mut w = self.shared.wall_start.lock().unwrap();
        if w.is_none() && self.shared.played.load(Ordering::Relaxed) == 0 {
            *w = Some(wall_start);
            self.shared.catching_up.store(true, Ordering::Relaxed);
        }
    }

    /// Why no sound will come: FFmpeg ended without producing any, or the device went away.
    pub fn failure(&self) -> Option<String> {
        let s = &self.shared;
        if s.device_lost.load(Ordering::Relaxed) {
            return Some("The audio device was disconnected".into());
        }
        if !s.finished.load(Ordering::Relaxed) || s.played.load(Ordering::Relaxed) > 0 || !s.samples.lock().unwrap().is_empty() {
            return None;
        }
        let err = s.ffmpeg_error.lock().unwrap();
        let line = err.lines().map(str::trim).rfind(|l| !l.is_empty())?;
        Some(line.chars().take(160).collect())
    }

    /// The audio device went away; playback should reopen the current default device.
    pub fn device_lost(&self) -> bool {
        self.shared.device_lost.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ProbeInfo;
    use crate::model::{Canvas, MediaKind};
    use std::path::PathBuf;

    #[test]
    fn shifting_trims_clips_that_started_earlier() {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 10.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let a = p.add_asset(&std::env::current_exe().unwrap(), &info);
        let c1 = p.add_media_clip(a, None, 0.0).unwrap();
        let c2 = p.add_media_clip(a, None, 12.0).unwrap();
        let s = shifted(&p, 4.0);
        let (s1, s2) = (s.clip(c1).unwrap(), s.clip(c2).unwrap());
        assert_eq!((s1.start, s1.duration), (0.0, 6.0));
        assert!(matches!(s1.content, Content::Media { src_in, .. } if (src_in - 4.0).abs() < 1e-9));
        assert_eq!(s2.start, 8.0);
        assert!(shifted(&p, 11.0).clip(c1).is_none());
    }

    #[test]
    fn missing_files_are_left_out_of_the_mix() {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 10.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let here = p.add_asset(&std::env::current_exe().unwrap(), &info);
        let gone = p.add_asset(&PathBuf::from("/no/such/file.wav"), &info);
        let kept = p.add_media_clip(here, None, 0.0).unwrap();
        let dropped = p.add_media_clip(gone, None, 0.0).unwrap();
        let s = shifted(&p, 0.0);
        assert!(s.clip(kept).is_some() && s.clip(dropped).is_none());
    }

    fn tone(dir: &std::path::Path, seconds: u32) -> Project {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("tone.wav");
        assert!(ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", &format!("sine=frequency=440:duration={seconds}"), "-y"]).arg(&path).status().unwrap().success());
        let mut p = Project::new("t", Canvas::default());
        let a = p.add_asset(&path, &crate::media::probe(&path).unwrap());
        p.add_media_clip(a, None, 0.0).unwrap();
        p
    }

    #[test]
    fn plays_muted_and_the_clock_advances() {
        let dir = std::env::temp_dir().join(format!("studio-play-{}", std::process::id()));
        let p = tone(&dir, 3);
        let Ok(Some(player)) = AudioPlayer::start(&p, 1.0, true) else {
            eprintln!("no audio device; skipping");
            return;
        };
        std::thread::sleep(Duration::from_millis(900));
        let pos = player.position().expect("audio started");
        assert!(pos > 1.2 && pos < 2.0, "position {pos}");
        assert!(player.failure().is_none());
        drop(player);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn late_sound_joins_in_sync_with_the_picture() {
        let dir = std::env::temp_dir().join(format!("studio-late-{}", std::process::id()));
        let p = tone(&dir, 6);
        let Ok(Some(player)) = AudioPlayer::start(&p, 0.0, true) else {
            eprintln!("no audio device; skipping");
            return;
        };
        // The picture has been running for a second already when the sound comes.
        let wall = Instant::now() - Duration::from_secs(1);
        player.join_at(wall);
        assert!(player.stalled() && player.position().is_none(), "the picture keeps the clock until the sound catches up");
        std::thread::sleep(Duration::from_millis(800));
        let pos = player.position().expect("sound caught up");
        let picture = wall.elapsed().as_secs_f64();
        assert!((pos - picture).abs() < 0.25, "sound at {pos}, picture at {picture}");
        assert!(!player.stalled());
        drop(player);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_broken_mix_reports_why() {
        let dir = std::env::temp_dir().join(format!("studio-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("broken.wav");
        std::fs::write(&bad, b"not audio at all").unwrap();
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 3.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let a = p.add_asset(&bad, &info);
        p.add_media_clip(a, None, 0.0).unwrap();
        let Ok(Some(player)) = AudioPlayer::start(&p, 0.0, true) else {
            eprintln!("no audio device; skipping");
            return;
        };
        let t = Instant::now();
        while player.failure().is_none() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(player.failure().is_some(), "FFmpeg's error is reported");
        assert!(player.stalled() && player.position().is_none());
        drop(player);
        std::fs::remove_dir_all(dir).ok();
    }
}
