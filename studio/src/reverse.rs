//! Reversed playback: FFmpeg writes a backwards copy of the part of a file a clip uses, and
//! the clip then plays that copy. (Decoding runs forward only, so a clip can't simply be
//! played from its end.)

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::ffmpeg;

/// What to reverse: the part of `path` from `from` lasting `length` seconds.
#[derive(Debug, Clone)]
pub struct Job {
    pub path: PathBuf,
    pub from: f64,
    pub length: f64,
    /// The file has a picture (video); otherwise only its sound is reversed.
    pub video: bool,
    pub audio: bool,
    pub fps: f64,
    pub size: (u32, u32),
    /// Folder the reversed copy is written to.
    pub out_dir: PathBuf,
}

/// Memory FFmpeg's `reverse` filter may hold at once: it keeps every frame of a piece in RAM,
/// so long clips are reversed piece by piece and the pieces joined back to front.
const PIECE_BYTES: f64 = 600.0 * 1024.0 * 1024.0;

fn run(mut cmd: std::process::Command, what: &str) -> anyhow::Result<()> {
    let out = cmd.output().with_context(|| format!("could not start FFmpeg to {what}"))?;
    if !out.status.success() {
        let log = String::from_utf8_lossy(&out.stderr);
        bail!("Couldn't {what}: {}", log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error").trim());
    }
    Ok(())
}

/// Where the reversed copy of this job goes: the same job always gives the same file,
/// so reversing the same part twice reuses it.
fn target(job: &Job) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (job.path.as_os_str(), (job.from * 1000.0).round() as i64, (job.length * 1000.0).round() as i64).hash(&mut h);
    let stem = job.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    job.out_dir.join(format!("{stem} (reversed {:08x}).{}", h.finish() as u32, if job.video { "mp4" } else { "wav" }))
}

/// Writes the reversed copy and returns its path.
pub fn reversed_file(job: &Job) -> anyhow::Result<PathBuf> {
    if job.length < 0.05 {
        bail!("This clip is too short to reverse");
    }
    std::fs::create_dir_all(&job.out_dir).with_context(|| format!("could not create {}", job.out_dir.display()))?;
    let out = target(job);
    if out.is_file() {
        return Ok(out);
    }
    let work = ffmpeg::work_dir().join(format!("reverse-{}-{:x}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    std::fs::create_dir_all(&work).context("could not create a working folder")?;
    let result = build(job, &work, &out);
    let _ = std::fs::remove_dir_all(&work);
    if result.is_err() {
        let _ = std::fs::remove_file(&out);
    }
    result.map(|_| out)
}

fn build(job: &Job, work: &Path, out: &Path) -> anyhow::Result<()> {
    // Written under another name first, so a failed run never leaves a broken file behind.
    let part = work.join(format!("out.{}", if job.video { "mp4" } else { "wav" }));
    let sound = work.join("sound.wav");
    if job.audio {
        let mut cmd = ffmpeg::ffmpeg()?;
        cmd.args(["-ss", &ffmpeg::secs(job.from), "-t", &ffmpeg::secs(job.length), "-i"]).arg(&job.path);
        cmd.args(["-vn", "-sn", "-af", "areverse", "-c:a", "pcm_s16le", "-y"]).arg(if job.video { &sound } else { &part });
        run(cmd, "reverse the sound")?;
    }
    if job.video {
        let fps = job.fps.clamp(1.0, 240.0);
        let frames = ((job.length * fps).round() as u64).max(1);
        // Frames are kept as 4:2:0 pictures: one and a half bytes per pixel.
        let per_piece = ((PIECE_BYTES / (job.size.0.max(16) as f64 * job.size.1.max(16) as f64 * 1.5)) as u64).clamp(8, 900);
        let pieces = frames.div_ceil(per_piece);
        let mut list = String::new();
        for i in 0..pieces {
            let first = i * per_piece;
            let count = per_piece.min(frames - first);
            // Half a frame early, so the seek lands on the piece's first frame and not the one after.
            let seek = (job.from + (first as f64 - 0.5) / fps).max(0.0);
            let name = format!("piece{i:05}.mkv");
            let mut cmd = ffmpeg::ffmpeg()?;
            cmd.args(["-ss", &ffmpeg::secs(seek), "-i"]).arg(&job.path);
            cmd.args(["-an", "-sn", "-vf", &format!("trim=end_frame={count},reverse,setpts=PTS-STARTPTS"), "-r", &format!("{fps:.4}")]);
            cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "15", "-pix_fmt", "yuv420p", "-y"]).arg(work.join(&name));
            run(cmd, "reverse the picture")?;
        }
        // Joined back to front. Names are relative to the working folder, so no path needs quoting.
        for i in (0..pieces).rev() {
            list.push_str(&format!("file 'piece{i:05}.mkv'\n"));
        }
        std::fs::write(work.join("list.txt"), list).context("could not write the list of pieces")?;
        let mut cmd = ffmpeg::ffmpeg()?;
        cmd.current_dir(work).args(["-f", "concat", "-safe", "0", "-i", "list.txt"]);
        if job.audio {
            cmd.arg("-i").arg(&sound).args(["-map", "0:v", "-map", "1:a", "-c:a", "aac", "-b:a", "256k"]);
        }
        cmd.args(["-c:v", "copy", "-movflags", "+faststart", "-y"]).arg(&part);
        run(cmd, "join the reversed pieces")?;
    } else if !job.audio {
        bail!("This clip has neither picture nor sound to reverse");
    }
    // A rename fails across drives (the working folder is in the temp folder): copy then.
    if std::fs::rename(&part, out).is_err() {
        std::fs::copy(&part, out).with_context(|| format!("could not write {}", out.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The average brightness of the frame at `t` seconds (0–255).
    fn brightness(path: &Path, t: f64) -> f64 {
        let out = ffmpeg::ffmpeg().unwrap().args(["-ss", &ffmpeg::secs(t), "-i"]).arg(path).args(["-frames:v", "1", "-vf", "scale=8:8,format=gray", "-f", "rawvideo", "pipe:1"]).output().unwrap();
        assert_eq!(out.stdout.len(), 64, "one 8×8 frame");
        out.stdout.iter().map(|b| *b as f64).sum::<f64>() / 64.0
    }

    #[test]
    fn a_reversed_copy_plays_the_part_backwards() {
        let dir = std::env::temp_dir().join(format!("studio-reverse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 4 s that fade from black to white, with a tone that starts halfway.
        let src = dir.join("in.mp4");
        let ok = ffmpeg::ffmpeg()
            .unwrap()
            .args(["-f", "lavfi", "-i", "color=c=black:size=320x180:rate=30:duration=4,format=gray,geq=lum='255*T/4'"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4,volume='if(gte(t,2),1,0)':eval=frame"])
            .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", "-y"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert!(brightness(&src, 0.2) < brightness(&src, 3.5), "the source gets brighter");
        // The part from 1 s to 3 s, reversed: it starts at the 3 s frame and ends at the 1 s frame.
        let job = Job { path: src.clone(), from: 1.0, length: 2.0, video: true, audio: true, fps: 30.0, size: (320, 180), out_dir: dir.join("out") };
        let out = reversed_file(&job).unwrap();
        let info = crate::media::probe(&out).unwrap();
        assert!((info.duration - 2.0).abs() < 0.12, "duration {}", info.duration);
        assert!(info.has_audio);
        let (start, end) = (brightness(&out, 0.1), brightness(&out, 1.8));
        assert!(start > end + 40.0, "it gets darker now: {start} then {end}");
        assert!((start - brightness(&src, 2.9)).abs() < 12.0, "its first frame is the part's last one");
        // The same job again reuses the file.
        let modified = std::fs::metadata(&out).unwrap().modified().unwrap();
        assert_eq!(reversed_file(&job).unwrap(), out);
        assert_eq!(std::fs::metadata(&out).unwrap().modified().unwrap(), modified);
        // Sound only.
        let sound = reversed_file(&Job { video: false, ..job.clone() }).unwrap();
        assert_eq!(sound.extension().and_then(|e| e.to_str()), Some("wav"));
        assert!((crate::media::probe(&sound).unwrap().duration - 2.0).abs() < 0.1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_clips_are_reversed_in_pieces_that_join_seamlessly() {
        let dir = std::env::temp_dir().join(format!("studio-reverse-pieces-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        let ok = ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "color=c=black:size=320x180:rate=30:duration=3,format=gray,geq=lum='255*T/3'", "-pix_fmt", "yuv420p", "-y"]).arg(&src).status().unwrap().success();
        assert!(ok);
        // A pretend frame size so big that only 8 frames fit in one piece: 90 frames make 12 pieces.
        let job = Job { path: src.clone(), from: 0.0, length: 3.0, video: true, audio: false, fps: 30.0, size: (8192, 6000), out_dir: dir.join("out") };
        let out = reversed_file(&job).unwrap();
        let info = crate::media::probe(&out).unwrap();
        assert!((info.duration - 3.0).abs() < 0.12, "duration {}", info.duration);
        // Brightness falls steadily from start to end: no piece is out of order.
        let levels: Vec<f64> = (0..10).map(|i| brightness(&out, 0.1 + i as f64 * 0.29)).collect();
        assert!(levels.windows(2).all(|w| w[0] > w[1] - 2.0), "{levels:?}");
        assert!(levels[0] > levels[9] + 150.0, "{levels:?}");
        std::fs::remove_dir_all(dir).ok();
    }
}
