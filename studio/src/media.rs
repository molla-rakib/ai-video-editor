//! Probing media files and extracting thumbnails and waveforms.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};

use crate::ffmpeg;
use crate::model::MediaKind;

#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub kind: MediaKind,
    /// Seconds; 0 for still images.
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
}

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff"];
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "wmv", "flv", "mts", "m2ts", "3gp", "mpg", "mpeg", "mp3", "wav", "aac", "m4a", "flac", "ogg", "opus", "wma",
    "png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff",
];

pub fn is_media_path(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn parse_rate(s: &str) -> f64 {
    let mut parts = s.split('/');
    let num: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let den: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    if den > 0.0 { num / den } else { 0.0 }
}

fn json_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn stream_rotation(stream: &serde_json::Value) -> i64 {
    if let Some(list) = stream["side_data_list"].as_array() {
        for sd in list {
            if let Some(r) = sd["rotation"].as_f64() {
                return r as i64;
            }
        }
    }
    stream["tags"]["rotate"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}

pub fn probe(path: &Path) -> anyhow::Result<ProbeInfo> {
    if !path.is_file() {
        bail!("File not found: {}", path.display());
    }
    let output = ffmpeg::ffprobe()?.args(["-print_format", "json", "-show_format", "-show_streams"]).arg(path).output().context("failed to run ffprobe")?;
    if !output.status.success() {
        bail!("Unsupported or damaged file: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).context("invalid ffprobe output")?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let format_name = json["format"]["format_name"].as_str().unwrap_or_default();
    // Cover art inside audio files shows up as a video stream; ignore it.
    let video = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0);
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let is_image_format = format_name.contains("image2") || format_name.ends_with("_pipe") || IMAGE_EXTENSIONS.contains(&ext.as_str());

    let mut duration = json_f64(&json["format"]["duration"]).unwrap_or(0.0);
    if let Some(v) = video {
        if duration <= 0.0 {
            duration = json_f64(&v["duration"]).unwrap_or(0.0);
        }
        let (mut width, mut height) = (v["width"].as_u64().unwrap_or(0) as u32, v["height"].as_u64().unwrap_or(0) as u32);
        if width == 0 || height == 0 {
            bail!("The video has no picture size");
        }
        if stream_rotation(v).rem_euclid(180) == 90 {
            std::mem::swap(&mut width, &mut height);
        }
        let mut fps = parse_rate(v["avg_frame_rate"].as_str().unwrap_or("0/1"));
        if !(1.0..=240.0).contains(&fps) {
            fps = parse_rate(v["r_frame_rate"].as_str().unwrap_or("0/1"));
        }
        if !(1.0..=240.0).contains(&fps) {
            fps = 30.0;
        }
        if is_image_format && !has_audio && duration < 0.5 {
            return Ok(ProbeInfo { kind: MediaKind::Image, duration: 0.0, width, height, fps: 0.0, has_audio: false });
        }
        if duration <= 0.0 {
            bail!("Could not read the duration");
        }
        return Ok(ProbeInfo { kind: MediaKind::Video, duration, width, height, fps, has_audio });
    }
    if has_audio {
        if duration <= 0.0 {
            bail!("Could not read the duration");
        }
        return Ok(ProbeInfo { kind: MediaKind::Audio, duration, width: 0, height: 0, fps: 0.0, has_audio: true });
    }
    bail!("No video or audio found")
}

/// Reads exactly `len` bytes of raw output from an ffmpeg command.
pub fn read_raw(mut cmd: Command, len: usize) -> anyhow::Result<Vec<u8>> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("failed to start ffmpeg")?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        match stdout.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) => {
                let _ = child.kill();
                return Err(e.into());
            }
        }
    }
    drop(stdout);
    let _ = child.kill();
    let _ = child.wait();
    let err = err_thread.join().unwrap_or_default();
    if filled < len {
        bail!("ffmpeg produced no frame: {}", err.trim());
    }
    Ok(buf)
}

/// One RGBA frame of exactly `w`×`h`, cropped to fill.
pub fn thumbnail(path: &Path, kind: MediaKind, t: f64, w: u32, h: u32) -> anyhow::Result<Vec<u8>> {
    let mut cmd = ffmpeg::ffmpeg()?;
    if kind == MediaKind::Video {
        cmd.args(["-ss", &ffmpeg::secs(t)]);
    }
    cmd.arg("-i").arg(path);
    let vf = format!("scale={w}:{h}:force_original_aspect_ratio=increase:flags=bilinear,crop={w}:{h},format=rgba");
    cmd.args(["-frames:v", "1", "-an", "-vf", &vf, "-f", "rawvideo", "pipe:1"]);
    match read_raw(cmd, (w * h * 4) as usize) {
        Ok(frame) => Ok(frame),
        // Seeking right at the end of a file can come back empty; retry from the start.
        Err(_) if kind == MediaKind::Video && t > 0.0 => thumbnail(path, kind, 0.0, w, h),
        Err(e) => Err(e),
    }
}

/// `count` evenly spaced thumbnails in a single ffmpeg run.
pub fn thumbnail_strip(path: &Path, duration: f64, count: usize, w: u32, h: u32) -> anyhow::Result<Vec<Vec<u8>>> {
    let count = count.max(1);
    let rate = count as f64 / duration.max(0.1);
    let vf = format!("fps={rate:.6},scale={w}:{h}:force_original_aspect_ratio=increase:flags=bilinear,crop={w}:{h},format=rgba");
    let mut cmd = ffmpeg::ffmpeg()?;
    cmd.arg("-i").arg(path).args(["-an", "-vf", &vf, "-frames:v", &count.to_string(), "-f", "rawvideo", "pipe:1"]);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("failed to start ffmpeg")?;
    let mut stdout = child.stdout.take().unwrap();
    let frame_len = (w * h * 4) as usize;
    let mut frames = Vec::with_capacity(count);
    loop {
        let mut frame = vec![0u8; frame_len];
        if stdout.read_exact(&mut frame).is_err() {
            break;
        }
        frames.push(frame);
        if frames.len() == count {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    if frames.is_empty() {
        bail!("no frames decoded");
    }
    Ok(frames)
}

/// Peak levels (0..1) at `per_second` buckets per second of audio.
pub fn waveform(path: &Path, per_second: u32) -> anyhow::Result<Vec<f32>> {
    const RATE: u32 = 8000;
    let mut child = ffmpeg::ffmpeg()?
        .arg("-i")
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", &RATE.to_string(), "-f", "s16le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start ffmpeg")?;
    let mut stdout = child.stdout.take().unwrap();
    let bucket = (RATE / per_second).max(1) as usize;
    let mut bytes = Vec::new();
    stdout.read_to_end(&mut bytes)?;
    let _ = child.wait();
    let peaks = bytes
        .chunks_exact(2)
        .map(|c| (i16::from_le_bytes([c[0], c[1]]) as i32).unsigned_abs())
        .collect::<Vec<_>>()
        .chunks(bucket)
        .map(|b| *b.iter().max().unwrap_or(&0) as f32 / 32768.0)
        .collect();
    Ok(peaks)
}

/// Times (seconds after `seek`) of percussive hits and beats in `seek..seek+length` of the audio
/// at `path`: for syncing motion (zooms, cuts, text entrances) to music instead of guessing.
/// Energy-based onset detection (no FFT, no extra dependency) — finds clear hits in typical
/// music and sound effects; not a tempo/BPM analysis.
pub fn detect_beats(path: &Path, seek: f64, length: f64) -> anyhow::Result<Vec<f64>> {
    const RATE: u32 = 8000;
    // 10 ms frames: fine enough to place a hit, coarse enough that one hit isn't several.
    const FRAME: usize = 80;
    // Two hits closer than this read as one.
    const MIN_GAP_SECS: f64 = 0.15;
    // Decode a bit before `seek` too: a beat right at the start of the requested range still
    // needs a quieter frame before it to stand out against, or it's indistinguishable from the
    // decode just starting loud.
    const LEAD_SECS: f64 = 0.5;
    let lead = LEAD_SECS.min(seek);

    let mut child = ffmpeg::ffmpeg()?
        .args(["-ss", &ffmpeg::secs(seek - lead), "-t", &ffmpeg::secs(length + lead), "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", &RATE.to_string(), "-f", "s16le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start ffmpeg")?;
    let mut stdout = child.stdout.take().unwrap();
    let mut bytes = Vec::new();
    stdout.read_to_end(&mut bytes)?;
    let _ = child.wait();

    let energy: Vec<f32> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect::<Vec<_>>()
        .chunks(FRAME)
        .map(|f| ((f.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / f.len() as f64).sqrt() / 32768.0) as f32)
        .collect();
    if energy.len() < 3 {
        return Ok(Vec::new());
    }
    // Onset strength: how much louder a frame is than the one before it.
    let flux: Vec<f32> = std::iter::once(0.0).chain(energy.windows(2).map(|w| (w[1] - w[0]).max(0.0))).collect();
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let variance = flux.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / flux.len() as f32;
    let threshold = mean + 1.5 * variance.sqrt();

    let min_gap = ((MIN_GAP_SECS * RATE as f64 / FRAME as f64) as usize).max(1);
    let mut beats = Vec::new();
    let mut last = None::<usize>;
    for i in 1..flux.len() - 1 {
        let is_peak = flux[i] > threshold && flux[i] >= flux[i - 1] && flux[i] >= flux[i + 1];
        if is_peak && last.is_none_or(|l| i - l >= min_gap) {
            last = Some(i);
            // Spacing (and the threshold/mean) still accounts for a peak found in the lead-in,
            // but only hits inside the actually requested range are reported.
            let t = i as f64 * FRAME as f64 / RATE as f64 - lead;
            if t >= 0.0 {
                beats.push(t);
            }
        }
    }
    Ok(beats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_beats_finds_periodic_clicks() {
        let dir = std::env::temp_dir().join(format!("studio-beats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clicks.wav");
        let status = ffmpeg::ffmpeg()
            .unwrap()
            .args(["-f", "lavfi", "-i", "aevalsrc=exprs=lt(mod(t\\,0.5)\\,0.01)*sin(2*PI*1000*t):s=8000:d=4", "-y"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let beats = detect_beats(&path, 0.0, 4.0).unwrap();
        // A click every 0.5s over 4s: about 8 onsets.
        assert!(beats.len() >= 6 && beats.len() <= 9, "expected ~8 beats, got {beats:?}");
        for pair in beats.windows(2) {
            let gap = pair[1] - pair[0];
            assert!((gap - 0.5).abs() < 0.08, "gap {gap} should be close to 0.5s");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn detect_beats_is_quiet_on_silence() {
        let dir = std::env::temp_dir().join(format!("studio-silence-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("silence.wav");
        let status = ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "anullsrc=r=8000:cl=mono:d=2", "-y"]).arg(&path).status().unwrap();
        assert!(status.success());
        let beats = detect_beats(&path, 0.0, 2.0).unwrap();
        assert!(beats.is_empty(), "silence shouldn't register any beats, got {beats:?}");
    }
}
