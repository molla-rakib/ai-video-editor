//! Renders a project to an MP4 file: frames come from the same compositor as
//! the preview (on a headless GPU device), audio from an FFmpeg mix.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, bail};
use iced::wgpu;

use super::compositor::Compositor;
use super::frames::FrameServer;
use super::{audio, scene};
use crate::ffmpeg;
use crate::model::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    High,
    Standard,
    Small,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::High, Quality::Standard, Quality::Small];

    fn crf(self) -> u32 {
        match self {
            Quality::High => 17,
            Quality::Standard => 21,
            Quality::Small => 26,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::High => "High",
            Quality::Standard => "Standard",
            Quality::Small => "Small file",
        }
    }

    /// Rough video bitrate in Mbit/s at 1080p30, for size estimates.
    fn mbps_1080p30(self) -> f64 {
        match self {
            Quality::High => 14.0,
            Quality::Standard => 8.0,
            Quality::Small => 4.0,
        }
    }
}

/// A named delivery target for the export dialog: besides a quality, it can cap the
/// output resolution and pick an audio bitrate to match what a platform actually uses.
/// `Custom` defers entirely to the dialog's own quality picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Preset {
    Custom,
    Web,
    YouTube,
    Social,
    Archive,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Custom, Preset::Web, Preset::YouTube, Preset::Social, Preset::Archive];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Custom => "Custom",
            Preset::Web => "Web",
            Preset::YouTube => "YouTube",
            Preset::Social => "Social",
            Preset::Archive => "Archive",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Preset::Custom => "Pick your own quality below",
            Preset::Web => "H.264, capped at 1080p — small enough to embed or email",
            Preset::YouTube => "High bitrate H.264 at full resolution",
            Preset::Social => "For Instagram, TikTok and Reels: capped at 1080 px wide",
            Preset::Archive => "A master copy: highest quality at full resolution, for safekeeping or re-editing",
        }
    }

    fn quality(self, custom: Quality) -> Quality {
        match self {
            Preset::Custom => custom,
            Preset::Web | Preset::Social => Quality::Standard,
            Preset::YouTube | Preset::Archive => Quality::High,
        }
    }

    /// Caps the output width (height scales to match); `None` keeps the project's own size.
    fn max_width(self) -> Option<u32> {
        match self {
            Preset::Web | Preset::Social => Some(1080),
            Preset::Custom | Preset::YouTube | Preset::Archive => None,
        }
    }

    fn audio_kbps(self) -> u32 {
        match self {
            Preset::Web | Preset::Social => 128,
            Preset::Custom | Preset::YouTube => 192,
            Preset::Archive => 320,
        }
    }

    /// Integrated loudness target in LUFS for the final mix; `None` leaves it unprocessed
    /// (Custom defers to the user, Archive keeps the original dynamics for re-editing).
    fn loudness(self) -> Option<f64> {
        match self {
            Preset::Web | Preset::YouTube | Preset::Social => Some(-14.0),
            Preset::Custom | Preset::Archive => None,
        }
    }
}

/// Resolved settings for one export: a preset folds down to this, and a bare `Quality`
/// (existing callers, tests, MCP) converts into the full-resolution, 192 kbps default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportSettings {
    pub quality: Quality,
    pub max_width: Option<u32>,
    pub audio_kbps: u32,
    pub loudness: Option<f64>,
    /// Export only this part of the timeline (seconds), instead of all of it.
    pub range: Option<(f64, f64)>,
}

impl From<Quality> for ExportSettings {
    fn from(quality: Quality) -> Self {
        ExportSettings { quality, max_width: None, audio_kbps: 192, loudness: None, range: None }
    }
}

impl ExportSettings {
    pub fn from_preset(preset: Preset, custom_quality: Quality) -> Self {
        ExportSettings { quality: preset.quality(custom_quality), max_width: preset.max_width(), audio_kbps: preset.audio_kbps(), loudness: preset.loudness(), range: None }
    }
}

/// Estimated file size in megabytes.
pub fn estimate_mb(size: (u32, u32), fps: u32, quality: Quality, seconds: f64) -> f64 {
    let pixels = (size.0 * size.1) as f64 / (1920.0 * 1080.0);
    let video = quality.mbps_1080p30() * pixels.powf(0.75) * (fps as f64 / 30.0).powf(0.6);
    (video + 0.19) * seconds / 8.0
}

/// `size` scaled down to `max_width` (height keeps the aspect ratio, rounded to an even
/// number as `-2` would in the FFmpeg scale filter); unchanged if it's already narrower.
pub fn scaled_size(size: (u32, u32), max_width: Option<u32>) -> (u32, u32) {
    match max_width {
        Some(mw) if mw < size.0 => (mw, ((size.1 as f64 * mw as f64 / size.0 as f64) / 2.0).round() as u32 * 2),
        _ => size,
    }
}

#[derive(Debug, Clone)]
pub enum Progress {
    Rendering { done: u64, total: u64, eta: Option<f64> },
    Finished { path: PathBuf, seconds: f64 },
    Failed(String),
    Cancelled,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn headless_gpu() -> anyhow::Result<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }))
        .context("No graphics adapter available for rendering")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("export"), ..Default::default() })).context("Could not open the graphics device")?;
    Ok(Gpu { device, queue })
}

/// Renders the whole project to `path`. Writes to a temporary file first, so a
/// failed or cancelled export never leaves a broken video behind.
pub fn export(project: &Project, path: &Path, settings: impl Into<ExportSettings>, cancel: &AtomicBool, progress: &dyn Fn(Progress)) -> anyhow::Result<PathBuf> {
    let settings = settings.into();
    if project.duration() <= 0.0 {
        bail!("The timeline is empty — add a clip before exporting");
    }
    // The part to export: all of it, or the marked range.
    let (from, total) = match settings.range {
        Some((a, b)) => {
            let a = a.min(b).clamp(0.0, project.duration());
            (a, a.max(b).min(project.duration()) - a)
        }
        None => (0.0, project.duration()),
    };
    if total < 0.05 {
        bail!("The marked range is empty — mark a longer part of the timeline");
    }
    let (w, h) = project.canvas.size();
    let fps = project.canvas.fps;
    let frames_total = (total * fps as f64).ceil() as u64;
    let started = Instant::now();
    crate::fonts::ensure_loaded();

    let mut renderer = Offscreen::new(w, h)?;

    // FFmpeg: raw frames on stdin, audio mixed from the source files.
    // Sound for a range: the same mix, from a copy of the project that starts at the range.
    let from_range;
    let heard = if from > 0.0 {
        from_range = crate::playback::shifted(project, from);
        &from_range
    } else {
        project
    };
    let (inputs, graph) = audio::mix_graph(heard, 1, total, settings.loudness);
    // A name of its own per export: two at once (say, a batch and a test) must not share one,
    // or the first to finish deletes the file the other's FFmpeg is still reading.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let graph_file = ffmpeg::work_dir().join(format!("export-{}-{}.txt", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&graph_file, &graph).context("could not write the audio graph")?;
    let part = path.with_file_name(format!("{}.part", path.file_name().and_then(|n| n.to_str()).unwrap_or("export.mp4")));
    let mut cmd = ffmpeg::ffmpeg()?;
    cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "pipe:0"]);
    for input in &inputs {
        cmd.args(["-ss", &ffmpeg::secs(input.seek), "-t", &ffmpeg::secs(input.length), "-i"]).arg(&input.path);
    }
    cmd.arg(ffmpeg::filter_script_option()).arg(&graph_file);
    cmd.args(["-map", "0:v", "-map", "[aout]"]);
    if let Some(mw) = settings.max_width.filter(|&mw| mw < w) {
        cmd.args(["-vf", &format!("scale={mw}:-2")]);
    }
    cmd.args(["-c:v", "libx264", "-preset", "medium", "-crf", &settings.quality.crf().to_string(), "-pix_fmt", "yuv420p", "-r", &fps.to_string()]);
    cmd.args(["-c:a", "aac", "-b:a", &format!("{}k", settings.audio_kbps), "-movflags", "+faststart", "-f", "mp4", "-y"]).arg(&part);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("could not start FFmpeg")?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    // Whatever goes wrong below, the encoder is stopped and the partial file removed.
    struct Cleanup<'a> {
        child: &'a mut std::process::Child,
        files: [&'a Path; 2],
        done: bool,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            if !self.done {
                let _ = self.child.kill();
                let _ = self.child.wait();
                for f in self.files {
                    let _ = std::fs::remove_file(f);
                }
            }
        }
    }
    let mut guard = Cleanup { child: &mut child, files: [&part, &graph_file], done: false };

    let server = FrameServer::default();
    server.set_preview_width(w as f32);
    let sources = ExactSources { inner: server.bind(project), server: &server };
    let mut row = Vec::new();
    for n in 0..frames_total {
        if cancel.load(Ordering::Relaxed) {
            drop(stdin);
            bail!("cancelled");
        }
        let t = from + n as f64 / fps as f64;
        let scene = scene::evaluate(project, t, (w, h), &sources, true);
        renderer.render(&scene, &mut row)?;
        if stdin.write_all(&row).is_err() {
            break;
        }
        server.collect_idle();
        if n % 3 == 0 || n + 1 == frames_total {
            let elapsed = started.elapsed().as_secs_f64();
            let eta = (n > 10).then(|| elapsed / (n + 1) as f64 * (frames_total - n - 1) as f64);
            progress(Progress::Rendering { done: n + 1, total: frames_total, eta });
        }
    }
    drop(stdin);
    let status = guard.child.wait().context("FFmpeg stopped unexpectedly")?;
    let log = err_thread.join().unwrap_or_default();
    if !status.success() {
        bail!("Encoding failed: {}", log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error").trim());
    }
    std::fs::rename(&part, path).with_context(|| format!("could not move the video to {}", path.display()))?;
    guard.done = true;
    drop(guard);
    let _ = std::fs::remove_file(&graph_file);
    Ok(path.to_path_buf())
}

/// Renders scenes into a CPU buffer on a headless GPU device.
pub struct Offscreen {
    gpu: Gpu,
    compositor: Compositor,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    size: (u32, u32),
    padded_row: u32,
}

impl Offscreen {
    pub fn new(w: u32, h: u32) -> anyhow::Result<Offscreen> {
        let gpu = headless_gpu()?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let compositor = Compositor::new(&gpu.device, &gpu.queue, format);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let padded_row = (w * 4).div_ceil(256) * 256;
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback"),
            size: (padded_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Offscreen { gpu, compositor, target, view, readback, size: (w, h), padded_row })
    }

    /// Draws `scene` and copies the pixels (tightly packed RGBA) into `out`.
    pub fn render(&mut self, scene: &scene::Scene, out: &mut Vec<u8>) -> anyhow::Result<()> {
        let (w, h) = self.size;
        let gpu = &self.gpu;
        self.compositor.prepare(&gpu.device, &gpu.queue, scene, 1.0, Compositor::full_view(w, h));
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("offscreen") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.compositor.draw(&mut pass);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &self.readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.padded_row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("GPU error: {e}"))?;
        out.resize((w * h * 4) as usize, 0);
        {
            let data = slice.get_mapped_range();
            let row = (w * 4) as usize;
            for y in 0..h as usize {
                let src = y * self.padded_row as usize;
                out[y * row..(y + 1) * row].copy_from_slice(&data[src..src + row]);
            }
        }
        self.readback.unmap();
        Ok(())
    }
}

/// A JPEG of the project at time `t`, at most `max_width` wide (for the assistant to look at).
pub fn snapshot_jpeg(project: &Project, t: f64, max_width: u32) -> anyhow::Result<Vec<u8>> {
    crate::fonts::ensure_loaded();
    let (cw, ch) = project.canvas.size();
    let scale = (max_width as f64 / cw as f64).min(1.0);
    let (w, h) = (((cw as f64 * scale) as u32).max(2) & !1, ((ch as f64 * scale) as u32).max(2) & !1);
    let server = FrameServer::default();
    server.set_preview_width(w as f32);
    let sources = ExactSources { inner: server.bind(project), server: &server };
    // Render at the reduced size directly: the scene scales everything with the canvas.
    let scene = scene::evaluate(project, t, (w, h), &sources, false);
    let mut renderer = Offscreen::new(w, h)?;
    let mut rgba = Vec::new();
    renderer.render(&scene, &mut rgba)?;
    let img = image::RgbaImage::from_raw(w, h, rgba).context("bad frame")?;
    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 82).encode_image(&rgb)?;
    Ok(jpeg)
}

/// Like the preview's sources, but waits for the exact frame instead of showing the latest one.
struct ExactSources<'a> {
    inner: super::frames::Bound<'a>,
    server: &'a FrameServer,
}

impl scene::Sources for ExactSources<'_> {
    fn video(&self, clip: crate::model::Id, asset: crate::model::Id, src_time: f64, _playing: bool) -> Option<Arc<super::Frame>> {
        let deadline = Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let frame = self.inner.video(clip, asset, src_time, true);
            if self.server.has_frame(clip, src_time) || Instant::now() > deadline {
                return self.inner.video(clip, asset, src_time, true).or(frame);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn image(&self, asset: crate::model::Id) -> Option<Arc<super::Frame>> {
        let deadline = Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(f) = self.inner.image(asset) {
                return Some(f);
            }
            self.server.settle();
            if self.server.image_failed(asset) || Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn text(&self, layer: &crate::clip::TextLayer, px_per_unit: f32, local: f64) -> Option<Arc<super::Frame>> {
        self.inner.text(layer, px_per_unit, local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Motion, MotionKind, Rgba, ShapeKind, ShapeLayer, TextLayer};
    use crate::model::{Aspect, Canvas, Resolution};

    fn probe_json(path: &Path) -> serde_json::Value {
        let out = ffmpeg::ffprobe().unwrap().args(["-print_format", "json", "-show_format", "-show_streams"]).arg(path).output().unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }

    #[test]
    fn exports_video_text_shapes_and_mixed_audio() {
        let dir = std::env::temp_dir().join(format!("studio-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        let ok = ffmpeg::ffmpeg()
            .unwrap()
            .args(["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=3", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
            .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", "-y"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut p = Project::new("t", Canvas { aspect: Aspect::Portrait, resolution: Resolution::Hd, fps: 30 });
        let a = p.add_asset(&src, &crate::media::probe(&src).unwrap());
        let v = p.add_media_clip(a, None, 0.0).unwrap();
        p.clip_mut(v).unwrap().look.corner_radius = 30.0;
        p.set_speed(v, 2.0).unwrap();
        let t = p.add_text_clip(None, 0.2, crate::edit::DEFAULT_STILL, TextLayer { text: "নতুন Studio".into(), ..Default::default() }).unwrap();
        p.clip_mut(t).unwrap().intro = Some(Motion { kind: MotionKind::Pop, duration: 0.4, ease: None, delay: 0.0 });
        p.add_shape_clip(Some(p.tracks[1].id), 0.0, crate::edit::DEFAULT_STILL, ShapeLayer::new(ShapeKind::Rectangle, 200.0, 80.0, Rgba::hex(0x7C6DFF), None)).unwrap();

        let out = dir.join("out.mp4");
        let cancel = AtomicBool::new(false);
        let seen = std::sync::Mutex::new(0u64);
        export(&p, &out, Quality::Small, &cancel, &|pr| {
            if let Progress::Rendering { done, .. } = pr {
                *seen.lock().unwrap() = done;
            }
        })
        .unwrap();
        let info = probe_json(&out);
        let streams = info["streams"].as_array().unwrap();
        let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
        assert_eq!((video["width"].as_u64(), video["height"].as_u64()), (Some(720), Some(1280)));
        assert!(streams.iter().any(|s| s["codec_type"] == "audio"));
        let duration: f64 = info["format"]["duration"].as_str().unwrap().parse().unwrap();
        // The 3 s source at 2× lasts 1.5 s; the text clip runs to 5.2 s.
        assert!((duration - 5.2).abs() < 0.15, "duration {duration}");
        assert_eq!(*seen.lock().unwrap(), (5.2f64 * 30.0).ceil() as u64);
        assert!(!dir.join("out.mp4.part").exists());

        // Only a marked range: 1.0–2.5 s is 1.5 s long, with the video's sound (which ends at
        // 1.5 s on the timeline) in its first half second only.
        let part = dir.join("part.mp4");
        export(&p, &part, ExportSettings { range: Some((1.0, 2.5)), ..Quality::Small.into() }, &cancel, &|_| {}).unwrap();
        let duration: f64 = probe_json(&part)["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!((duration - 1.5).abs() < 0.15, "range duration {duration}");
        assert!(rms_db(&part, 0.0, 0.4) > -40.0, "the range starts inside the sound");
        assert!(rms_db(&part, 0.8, 1.4) < -60.0, "and is silent after it ends");

        // Cancelling removes the partial file.
        cancel.store(true, Ordering::Relaxed);
        let out2 = dir.join("cancelled.mp4");
        assert!(export(&p, &out2, Quality::Small, &cancel, &|_| {}).is_err());
        assert!(!out2.exists() && !dir.join("cancelled.mp4.part").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    /// RMS level (dB) of a file's audio between `from` and `to` seconds.
    fn rms_db(path: &Path, from: f64, to: f64) -> f64 {
        let out = ffmpeg::ffmpeg().unwrap().args(["-ss", &ffmpeg::secs(from), "-t", &ffmpeg::secs(to - from), "-i"]).arg(path).args(["-vn", "-af", "astats", "-f", "null", "-"]).args(["-loglevel", "info"]).output().unwrap();
        let log = String::from_utf8_lossy(&out.stderr);
        log.lines().filter_map(|l| l.split("RMS level dB:").nth(1)).last().and_then(|v| v.trim().parse().ok()).expect("astats output")
    }

    #[test]
    fn long_1080p_export_with_short_separated_audio_finishes_and_plays_it_once() {
        let dir = std::env::temp_dir().join(format!("studio-export-long-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        let ok = ffmpeg::ffmpeg()
            .unwrap()
            .args(["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=2", "-f", "lavfi", "-i", "sine=frequency=330:duration=2"])
            .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-shortest", "-y"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut p = Project::new("t", Canvas::default());
        let a = p.add_asset(&src, &crate::media::probe(&src).unwrap());
        let v = p.add_media_clip(a, None, 0.0).unwrap();
        p.separate_audio(v).unwrap();
        // Text runs on to 10 s, long after the sound ends at 2 s.
        p.add_text_clip(None, 0.0, 10.0, TextLayer::default()).unwrap();

        let out = dir.join("out.mp4");
        let started = Instant::now();
        export(&p, &out, Quality::Small, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(started.elapsed().as_secs() < 90, "export took {:?}", started.elapsed());
        let info = probe_json(&out);
        for s in info["streams"].as_array().unwrap() {
            let d: f64 = s["duration"].as_str().unwrap().parse().unwrap();
            assert!((d - 10.0).abs() < 0.1, "{} stream lasts {d}", s["codec_type"]);
        }
        let (exported, source) = (rms_db(&out, 0.3, 1.7), rms_db(&src, 0.3, 1.7));
        assert!((exported - source).abs() < 1.5, "the separated sound plays once: {exported:.1} dB vs {source:.1} dB");
        assert!(rms_db(&out, 3.0, 9.0) < -80.0, "silence after the sound ends");
        std::fs::remove_dir_all(dir).ok();
    }

    /// The center pixel of the canvas, as display-space floats.
    fn center_pixel(rgba: &[u8], w: u32, h: u32) -> [f32; 3] {
        let i = ((h / 2 * w + w / 2) * 4) as usize;
        [rgba[i] as f32 / 255.0, rgba[i + 1] as f32 / 255.0, rgba[i + 2] as f32 / 255.0]
    }

    /// The shader and `Grade::apply` (which draws the Color preset previews) must agree, or
    /// the preview tiles would promise a look the export doesn't deliver.
    #[test]
    fn every_color_preset_renders_like_its_preview() {
        use crate::clip::COLOR_PRESETS;
        let (w, h) = (320, 180);
        let mut p = Project::new("t", Canvas::default());
        let fill = Rgba::hex(0xC89474);
        let shape = p.add_shape_clip(None, 0.0, 2.0, ShapeLayer::new(ShapeKind::Rectangle, 600.0, 600.0, fill, None)).unwrap();
        let server = FrameServer::default();
        let mut renderer = Offscreen::new(w, h).unwrap();
        let mut rgba = Vec::new();
        for preset in COLOR_PRESETS {
            p.clip_mut(shape).unwrap().grade = preset.grade;
            renderer.render(&scene::evaluate(&p, 1.0, (w, h), &server.bind(&p), false), &mut rgba).unwrap();
            let (got, want) = (center_pixel(&rgba, w, h), preset.grade.apply([fill.0, fill.1, fill.2], 0.0));
            assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02), "{}: rendered {got:?}, preview {want:?}", preset.name);
        }
        // The vignette darkens the corners of the clip, not its middle.
        p.clip_mut(shape).unwrap().grade = crate::clip::Grade { vignette: 1.0, ..crate::clip::Grade::NONE };
        p.clip_mut(shape).unwrap().transform.scale = 4.0;
        renderer.render(&scene::evaluate(&p, 1.0, (w, h), &server.bind(&p), false), &mut rgba).unwrap();
        let middle = center_pixel(&rgba, w, h);
        assert!((middle[0] - fill.0).abs() < 0.02, "{middle:?}");
    }

    #[test]
    fn smoothness_removes_fine_noise_and_keeps_edges() {
        let dir = std::env::temp_dir().join(format!("studio-smooth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (320u32, 180u32);
        // Dark left half, light right half, with a fine checker (2 px squares) of noise over both.
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            let base: i32 = if x < w / 2 { 90 } else { 180 };
            let v = (base + if (x / 2 + y / 2) % 2 == 0 { 9 } else { -9 }) as u8;
            image::Rgba([v, v, v, 255])
        });
        let src = dir.join("noisy.png");
        img.save(&src).unwrap();
        let mut p = Project::new("t", Canvas::default());
        let a = p.add_asset(&src, &crate::media::probe(&src).unwrap());
        let clip = p.add_media_clip(a, None, 0.0).unwrap();

        let server = FrameServer::default();
        server.set_preview_width(w as f32);
        let mut renderer = Offscreen::new(w, h).unwrap();
        let mut frame = |p: &Project| {
            let sources = ExactSources { inner: server.bind(p), server: &server };
            let mut rgba = Vec::new();
            renderer.render(&scene::evaluate(p, 0.5, (w, h), &sources, false), &mut rgba).unwrap();
            rgba
        };
        // How much neighbouring pixels differ inside the left half, and the step across the edge.
        let roughness = |rgba: &[u8]| {
            let at = |x: u32, y: u32| rgba[((y * w + x) * 4) as usize] as f32;
            let mut sum = 0.0;
            for y in 60..120 {
                for x in 40..100 {
                    sum += (at(x, y) - at(x + 1, y)).abs();
                }
            }
            sum / 3600.0
        };
        let step = |rgba: &[u8]| {
            let row = |x: u32| (60..120).map(|y| rgba[((y * w + x) * 4) as usize] as f32).sum::<f32>() / 60.0;
            row(w / 2 + 6) - row(w / 2 - 7)
        };

        let plain = frame(&p);
        p.clip_mut(clip).unwrap().grade.smooth = 1.0;
        let smooth = frame(&p);
        assert!(roughness(&plain) > 2.0, "the test picture is noisy: {}", roughness(&plain));
        assert!(roughness(&smooth) < roughness(&plain) * 0.5, "noise {} -> {}", roughness(&plain), roughness(&smooth));
        assert!(step(&smooth) > step(&plain) * 0.9, "edge {} -> {}", step(&plain), step(&smooth));

        // Sharpening does the opposite: more local contrast.
        p.clip_mut(clip).unwrap().grade = crate::clip::Grade { sharpen: 1.0, ..crate::clip::Grade::NONE };
        let sharp = frame(&p);
        assert!(roughness(&sharp) > roughness(&plain) * 1.3, "detail {} -> {}", roughness(&plain), roughness(&sharp));
        std::fs::remove_dir_all(dir).ok();
    }
}
/// Developer tool: `STUDIO_EXPORT=project.aivp STUDIO_EXPORT_OUT=out.mp4 cargo test -p studio export_project_file -- --ignored`
#[cfg(test)]
#[test]
#[ignore]
fn export_project_file() {
    let project = Project::load(Path::new(&std::env::var("STUDIO_EXPORT").expect("STUDIO_EXPORT"))).unwrap();
    let out = PathBuf::from(std::env::var("STUDIO_EXPORT_OUT").expect("STUDIO_EXPORT_OUT"));
    let start = Instant::now();
    export(&project, &out, Quality::Standard, &AtomicBool::new(false), &|_| {}).unwrap();
    println!("exported {:.1}s of video in {:.1}s", project.duration(), start.elapsed().as_secs_f64());
}

/// Developer tool: renders a picture through every color preset into one contact sheet, to judge the looks by eye.
/// `STUDIO_SHEET=picture.jpg STUDIO_SHEET_OUT=sheet.png cargo test -p studio color_preset_sheet -- --ignored`
#[cfg(test)]
#[test]
#[ignore]
fn color_preset_sheet() {
    use crate::clip::COLOR_PRESETS;
    let src = PathBuf::from(std::env::var("STUDIO_SHEET").expect("STUDIO_SHEET"));
    let out = PathBuf::from(std::env::var("STUDIO_SHEET_OUT").expect("STUDIO_SHEET_OUT"));
    let mut p = Project::new("sheet", crate::model::Canvas::default());
    let a = p.add_asset(&src, &crate::media::probe(&src).unwrap());
    let clip = p.add_media_clip(a, None, 0.0).unwrap();
    p.clip_mut(clip).unwrap().transform.fit = crate::clip::Fit::Cover;
    let (w, h) = (640u32, 360u32);
    let server = FrameServer::default();
    server.set_preview_width(w as f32);
    let mut renderer = Offscreen::new(w, h).unwrap();
    let mut sheet = image::RgbaImage::new(w * 3, h * 3);
    for (i, preset) in COLOR_PRESETS.iter().enumerate() {
        p.clip_mut(clip).unwrap().grade = preset.grade;
        let sources = ExactSources { inner: server.bind(&p), server: &server };
        let mut rgba = Vec::new();
        renderer.render(&scene::evaluate(&p, 0.5, (w, h), &sources, false), &mut rgba).unwrap();
        let tile = image::RgbaImage::from_raw(w, h, rgba).unwrap();
        image::imageops::replace(&mut sheet, &tile, (i as u32 % 3 * w) as i64, (i as u32 / 3 * h) as i64);
    }
    sheet.save(&out).unwrap();
    println!("wrote {}", out.display());
}
