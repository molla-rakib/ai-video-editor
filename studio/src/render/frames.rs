//! Supplies decoded pixels to the scene: one background FFmpeg decoder per
//! visible video clip, plus cached images and text bitmaps.
//!
//! A decoder reads forward from where it is; it restarts with a fresh seek only
//! when asked for a time behind it or far ahead, so playback is sequential decoding.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::scene::Sources;
use super::{Frame, text};
use crate::clip::TextLayer;
use crate::ffmpeg;
use crate::model::{Id, MediaKind, Project};

/// How far ahead a seek target can be before the decoder restarts instead of reading forward.
const MAX_READ_AHEAD: f64 = 2.5;
/// Frames decoded ahead of the playhead while playing.
const PLAY_BUFFER: usize = 5;
const IDLE_DROP: Duration = Duration::from_secs(4);

struct Timed {
    pts: f64,
    frame: Arc<Frame>,
}

#[derive(Default)]
struct DecState {
    wanted: f64,
    playing: bool,
    quit: bool,
    eof: bool,
    frames: VecDeque<Timed>,
    /// Last frame handed out, shown until a better one is decoded.
    last: Option<Arc<Frame>>,
}

struct DecShared {
    state: Mutex<DecState>,
    wake: Condvar,
    frame_dur: f64,
}

impl DecShared {
    fn satisfied(&self, s: &DecState) -> bool {
        if s.eof {
            return true;
        }
        let has_current = s.frames.iter().any(|f| (f.pts - s.wanted).abs() <= self.frame_dur * 0.6 || (f.pts <= s.wanted && f.pts + self.frame_dur > s.wanted));
        if !has_current {
            return false;
        }
        !s.playing || s.frames.iter().filter(|f| f.pts > s.wanted).count() >= PLAY_BUFFER
    }
}

struct Decoder {
    shared: Arc<DecShared>,
    /// The media it reads: a clip pointed at other media (replace, undo) gets a fresh decoder.
    asset: Id,
    width: u32,
    last_used: Cell<Instant>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().quit = true;
        self.shared.wake.notify_all();
    }
}

struct Process {
    child: Child,
    next_pts: f64,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_process(path: &PathBuf, at: f64, w: u32, h: u32) -> Option<Process> {
    let mut cmd = ffmpeg::ffmpeg().ok()?;
    cmd.args(["-ss", &ffmpeg::secs(at)]).arg("-i").arg(path);
    cmd.args(["-an", "-sn", "-vf", &format!("scale={w}:{h}:flags=fast_bilinear,format=rgba"), "-f", "rawvideo", "pipe:1"]);
    let child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    Some(Process { child, next_pts: at })
}

fn run_decoder(shared: Arc<DecShared>, path: PathBuf, w: u32, h: u32, signal: Arc<AtomicBool>) {
    let frame_len = (w * h * 4) as usize;
    let mut proc: Option<Process> = None;
    loop {
        let (wanted, oldest) = {
            let mut s = shared.state.lock().unwrap();
            while !s.quit && shared.satisfied(&s) {
                s = shared.wake.wait(s).unwrap();
            }
            if s.quit {
                return;
            }
            (s.wanted, s.frames.front().map(|f| f.pts))
        };
        // Reading forward only helps if the target is just ahead. A target before
        // everything decoded so far (buffer included) needs a seek.
        let restart = match proc.as_ref().map(|p| p.next_pts) {
            None => true,
            Some(next) => {
                let earliest = oldest.unwrap_or(next);
                wanted < earliest - shared.frame_dur * 0.5 || wanted > next + MAX_READ_AHEAD
            }
        };
        if restart {
            proc = spawn_process(&path, wanted, w, h);
            let mut s = shared.state.lock().unwrap();
            s.frames.clear();
            s.eof = false;
            if proc.is_none() {
                s.eof = true;
                continue;
            }
        }
        let p = proc.as_mut().unwrap();
        let mut buf = vec![0u8; frame_len];
        let ok = p.child.stdout.as_mut().is_some_and(|out| out.read_exact(&mut buf).is_ok());
        let mut s = shared.state.lock().unwrap();
        if !ok {
            // End of the file: keep showing the last frame.
            s.eof = true;
            proc = None;
            continue;
        }
        let pts = p.next_pts;
        p.next_pts += shared.frame_dur;
        // Skip frames that are already in the past while catching up.
        if pts + shared.frame_dur < s.wanted && !(s.frames.is_empty() && s.last.is_none()) {
            continue;
        }
        s.frames.push_back(Timed { pts, frame: Arc::new(Frame::new(w, h, buf)) });
        let keep_from = s.wanted - shared.frame_dur * 2.0;
        while s.frames.len() > 1 && s.frames.front().is_some_and(|f| f.pts < keep_from) {
            s.frames.pop_front();
        }
        while s.frames.len() > PLAY_BUFFER + 4 {
            s.frames.pop_front();
        }
        signal.store(true, Ordering::Relaxed);
    }
}

pub struct FrameServer {
    decoders: RefCell<HashMap<Id, Decoder>>,
    images: Arc<Mutex<HashMap<Id, Arc<Frame>>>>,
    images_loading: RefCell<HashSet<Id>>,
    /// Images that couldn't be decoded; not retried until the file changes.
    images_failed: Arc<Mutex<HashSet<Id>>>,
    texts: RefCell<HashMap<u64, Arc<Frame>>>,
    /// Widest frame worth decoding (the preview's size in physical pixels).
    max_width: Cell<u32>,
    /// Set by background threads when new pixels arrive.
    signal: Arc<AtomicBool>,
    /// During playback: video frames asked for, and how many were shown late (Auto quality).
    frames_asked: Cell<u32>,
    frames_late: Cell<u32>,
}

impl Default for FrameServer {
    fn default() -> Self {
        FrameServer {
            decoders: RefCell::default(),
            images: Arc::default(),
            images_loading: RefCell::default(),
            images_failed: Arc::default(),
            texts: RefCell::default(),
            max_width: Cell::new(1280),
            signal: Arc::default(),
            frames_asked: Cell::new(0),
            frames_late: Cell::new(0),
        }
    }
}

impl FrameServer {
    /// Decode size limit; snapped to a few sizes so small resizes don't restart decoders.
    pub fn set_preview_width(&self, physical: f32) {
        let w = [640, 960, 1280, 1920, 2560, 3840].into_iter().find(|&b| b as f32 >= physical * 0.95).unwrap_or(3840);
        self.max_width.set(w);
    }

    pub fn bind<'a>(&'a self, project: &'a Project) -> Bound<'a> {
        Bound { server: self, project }
    }

    /// Frames asked for during playback and how many were late, since the last call.
    pub fn take_lag(&self) -> (u32, u32) {
        (self.frames_asked.replace(0), self.frames_late.replace(0))
    }

    /// Whether frames are still being decoded for the last request.
    pub fn busy(&self) -> bool {
        let decoding = self.decoders.borrow().values().any(|d| {
            let s = d.shared.state.lock().unwrap();
            !d.shared.satisfied(&s) && !s.playing
        });
        decoding || !self.images_loading.borrow().is_empty()
    }

    /// Whether the decoder for `clip` holds the frame for `src_time` (or reached the end of the file).
    pub fn has_frame(&self, clip: Id, src_time: f64) -> bool {
        let decoders = self.decoders.borrow();
        let Some(d) = decoders.get(&clip) else { return false };
        let s = d.shared.state.lock().unwrap();
        let dur = d.shared.frame_dur;
        s.eof || s.frames.iter().any(|f| f.pts <= src_time + dur * 0.5 && f.pts + dur > src_time - dur * 0.5)
    }

    /// Forgets decoded pixels of an asset whose file changed.
    pub fn forget_asset(&self, asset: Id) {
        self.images.lock().unwrap().remove(&asset);
        self.images_loading.borrow_mut().remove(&asset);
        self.images_failed.lock().unwrap().remove(&asset);
        // Decoders are per clip; drop them all so none keeps reading the old file.
        self.decoders.borrow_mut().clear();
    }

    /// Stops the decoder of a clip whose media was swapped for another file.
    pub fn forget_clip(&self, clip: Id) {
        self.decoders.borrow_mut().remove(&clip);
    }

    /// Stops decoders for clips that haven't been shown for a while.
    pub fn collect_idle(&self) {
        self.decoders.borrow_mut().retain(|_, d| d.last_used.get().elapsed() < IDLE_DROP);
        let mut texts = self.texts.borrow_mut();
        if texts.len() > 128 {
            texts.clear();
        }
    }
}

pub struct Bound<'a> {
    server: &'a FrameServer,
    project: &'a Project,
}

impl Bound<'_> {
    /// Starts a clip's decoder ahead of time at `src_time` (the clip comes on in a moment), so its
    /// first pictures are ready when the playhead gets there: starting at a cut means decoding from
    /// the keyframe before it, measured at 0.15–0.4 s on a screen recording, a visible freeze. Not
    /// counted as a late frame (Auto quality).
    pub fn warm(&self, clip: Id, asset: Id, src_time: f64) {
        let _ = self.fetch(clip, asset, src_time, true, false);
    }

    fn fetch(&self, clip: Id, asset: Id, src_time: f64, playing: bool, counted: bool) -> Option<Arc<Frame>> {
        let a = self.project.asset(asset)?;
        let server = self.server;
        let max_w = server.max_width.get();
        let w = (a.width.min(max_w).max(2)) & !1;
        let h = (((w as f64 * a.height as f64 / a.width.max(1) as f64).round() as u32).max(2)) & !1;
        let mut decoders = server.decoders.borrow_mut();
        if decoders.get(&clip).is_some_and(|d| d.width != w || d.asset != asset) {
            decoders.remove(&clip);
        }
        let dec = decoders.entry(clip).or_insert_with(|| {
            let shared = Arc::new(DecShared { state: Mutex::new(DecState::default()), wake: Condvar::new(), frame_dur: 1.0 / a.fps.clamp(1.0, 240.0) });
            {
                let mut s = shared.state.lock().unwrap();
                s.wanted = src_time;
                s.playing = playing;
            }
            let (sh, path, sig) = (shared.clone(), a.path.clone(), server.signal.clone());
            let _ = std::thread::Builder::new().name("decoder".into()).spawn(move || run_decoder(sh, path, w, h, sig));
            Decoder { shared, asset, width: w, last_used: Cell::new(Instant::now()) }
        });
        dec.last_used.set(Instant::now());
        let src_time = src_time.clamp(0.0, (a.duration - dec.shared.frame_dur).max(0.0));
        let mut s = dec.shared.state.lock().unwrap();
        if (s.wanted - src_time).abs() > 1e-6 || s.playing != playing {
            if (s.wanted - src_time).abs() > 1e-6 {
                s.eof = false;
            }
            s.wanted = src_time;
            s.playing = playing;
            dec.shared.wake.notify_all();
        }
        let half = dec.shared.frame_dur * 0.5;
        let timed = s.frames.iter().rev().find(|f| f.pts <= src_time + half).or_else(|| s.frames.front());
        if playing && counted {
            // Late: nothing decoded yet, or the newest frame is a few frames behind.
            let late = timed.is_none_or(|f| f.pts < src_time - dec.shared.frame_dur * 2.5);
            server.frames_asked.set(server.frames_asked.get() + 1);
            server.frames_late.set(server.frames_late.get() + late as u32);
        }
        let pick = timed.map(|f| f.frame.clone());
        if let Some(f) = &pick {
            s.last = Some(f.clone());
        }
        pick.or_else(|| s.last.clone())
    }
}

impl Sources for Bound<'_> {
    fn video(&self, clip: Id, asset: Id, src_time: f64, playing: bool) -> Option<Arc<Frame>> {
        self.fetch(clip, asset, src_time, playing, true)
    }

    fn image(&self, asset: Id) -> Option<Arc<Frame>> {
        let server = self.server;
        if let Some(f) = server.images.lock().unwrap().get(&asset) {
            return Some(f.clone());
        }
        let a = self.project.asset(asset)?;
        if a.kind != MediaKind::Image || server.images_failed.lock().unwrap().contains(&asset) || !server.images_loading.borrow_mut().insert(asset) {
            return None;
        }
        let (path, images, sig, failed) = (a.path.clone(), server.images.clone(), server.signal.clone(), server.images_failed.clone());
        let scale = (2048.0 / a.width.max(a.height).max(1) as f64).min(1.0);
        let (w, h) = (((a.width as f64 * scale) as u32).max(1), ((a.height as f64 * scale) as u32).max(1));
        std::thread::spawn(move || {
            let decoded = ffmpeg::ffmpeg().and_then(|mut cmd| {
                cmd.arg("-i").arg(&path).args(["-frames:v", "1", "-vf", &format!("scale={w}:{h},format=rgba"), "-f", "rawvideo", "pipe:1"]);
                crate::media::read_raw(cmd, (w * h * 4) as usize)
            });
            match decoded {
                Ok(rgba) => {
                    images.lock().unwrap().insert(asset, Arc::new(Frame::new(w, h, rgba)));
                }
                Err(_) => {
                    failed.lock().unwrap().insert(asset);
                }
            }
            sig.store(true, Ordering::Relaxed);
        });
        None
    }

    fn text(&self, layer: &TextLayer, px_per_unit: f32, local: f64) -> Option<Arc<Frame>> {
        let key = text::key(layer, px_per_unit, local);
        let mut texts = self.server.texts.borrow_mut();
        Some(texts.entry(key).or_insert_with(|| text::rasterize(layer, px_per_unit, local)).clone())
    }
}

impl FrameServer {
    /// Marks image loads that finished, so `busy` turns false.
    pub fn settle(&self) {
        let loaded: Vec<Id> = self.images.lock().unwrap().keys().copied().collect();
        let failed = self.images_failed.lock().unwrap();
        self.images_loading.borrow_mut().retain(|id| !loaded.contains(id) && !failed.contains(id));
    }

    /// Whether an image couldn't be decoded (so waiting for it is pointless).
    pub fn image_failed(&self, asset: Id) -> bool {
        self.images_failed.lock().unwrap().contains(&asset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Canvas;

    fn wait_for(f: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn a_clip_warmed_before_its_cut_has_its_picture_ready_at_the_cut() {
        let dir = std::env::temp_dir().join(format!("studio-frames-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rec.mp4");
        // A keyframe every 2 s; the cut's footage starts just before the next one (the slowest case).
        let status = ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "testsrc2=size=640x400:rate=30:duration=4", "-c:v", "libx264", "-preset", "ultrafast", "-g", "60", "-pix_fmt", "yuv420p", "-y"]).arg(&path).status().unwrap();
        assert!(status.success());
        let info = crate::media::probe(&path).unwrap();
        let mut project = Project::new("t", Canvas::default());
        let asset = project.add_asset(&path, &info);
        let server = FrameServer::default();
        let sources = server.bind(&project);
        // A second before the cut: warmed, and not counted as a late picture.
        sources.warm(7, asset, 1.9);
        assert_eq!(server.take_lag(), (0, 0));
        std::thread::sleep(Duration::from_millis(800));
        // At the cut (and a few frames on) the picture is there at once.
        for step in 0..4 {
            let t = 1.9 + step as f64 / 30.0;
            assert!(sources.video(7, asset, t, true).is_some() && server.has_frame(7, t), "no picture at {t:.3}s");
        }
        let (asked, late) = server.take_lag();
        assert_eq!((asked, late), (4, 0));
        std::fs::remove_dir_all(dir).ok();
    }

    /// Developer measurement: how long a clip's decoder takes to show its first picture, depending on
    /// where its footage starts between keyframes. `cargo test -p studio first_picture_after_a_cut -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn first_picture_after_a_cut() {
        let dir = std::env::temp_dir().join(format!("studio-frames-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rec.mp4");
        // Like a screen recording: large, a keyframe every 2 s.
        let status = ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "testsrc2=size=2560x1600:rate=30:duration=8", "-c:v", "libx264", "-preset", "superfast", "-g", "60", "-pix_fmt", "yuv420p", "-y"]).arg(&path).status().unwrap();
        assert!(status.success());
        let info = crate::media::probe(&path).unwrap();
        let mut project = Project::new("t", Canvas::default());
        let asset = project.add_asset(&path, &info);
        for (n, src) in [0.05, 1.0, 1.95, 2.05, 3.9].into_iter().enumerate() {
            let server = FrameServer::default();
            server.set_preview_width(1280.0);
            let sources = server.bind(&project);
            let started = Instant::now();
            assert!(wait_for(|| sources.video(n as u64 + 1, asset, src, true).is_some() && server.has_frame(n as u64 + 1, src)));
            println!("footage from {src:.2}s: first picture after {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn decoder_seeks_and_plays_forward() {
        let dir = std::env::temp_dir().join(format!("studio-frames-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.mp4");
        let status = ffmpeg::ffmpeg()
            .unwrap()
            .args(["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=6", "-pix_fmt", "yuv420p", "-y"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let info = crate::media::probe(&path).unwrap();
        let mut project = Project::new("t", Canvas::default());
        let asset = project.add_asset(&path, &info);
        let server = FrameServer::default();
        let sources = server.bind(&project);

        // A still request (scrub) eventually returns a frame.
        assert!(wait_for(|| sources.video(1, asset, 3.0, false).is_some()), "no frame after seek");
        assert!(wait_for(|| !server.busy()), "decoder settles after a seek");

        // Playing forward keeps returning newer frames without restarting.
        let mut last = 0;
        let mut t = 3.0;
        let mut fresh = 0;
        for _ in 0..30 {
            t += 1.0 / 30.0;
            assert!(wait_for(|| sources.video(1, asset, t, true).is_some()));
            std::thread::sleep(Duration::from_millis(15));
            let v = sources.video(1, asset, t, true).unwrap().version;
            if v != last {
                fresh += 1;
                last = v;
            }
        }
        assert!(fresh >= 20, "only {fresh} new frames in 30 steps");

        // Seeking backwards restarts and lands near the target.
        assert!(wait_for(|| {
            sources.video(1, asset, 0.5, false);
            let d = server.decoders.borrow();
            let s = d.get(&1).unwrap().shared.state.lock().unwrap();
            s.frames.iter().any(|f| (f.pts - 0.5).abs() < 0.05)
        }));
        std::fs::remove_dir_all(dir).ok();
    }
}
