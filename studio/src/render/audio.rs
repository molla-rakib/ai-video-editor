//! Builds the FFmpeg audio mix for a project: every audible clip trimmed,
//! sped up, faded, delayed to its timeline position and mixed.

use std::path::PathBuf;

use crate::clip::Content;
use crate::model::{Project, TrackKind};

/// One input file for the mix, with the seek and length to read.
pub struct AudioInput {
    pub path: PathBuf,
    pub seek: f64,
    pub length: f64,
}

/// FFmpeg inputs (starting at index `first_input`) and a filter graph ending in `[aout]`.
/// `loudness`, when set, is a target integrated loudness in LUFS (e.g. -14 for streaming
/// platforms) applied to the final mix — used at export time, not for live playback.
pub fn mix_graph(project: &Project, first_input: usize, total: f64, loudness: Option<f64>) -> (Vec<AudioInput>, String) {
    let mut inputs = Vec::new();
    let mut chains = Vec::new();
    for clip in &project.clips {
        let Content::Media { asset, src_in, speed } = clip.content else { continue };
        let Some(a) = project.asset(asset) else { continue };
        let Some(track) = project.track(clip.track) else { continue };
        // Muted audio tracks and hidden video tracks are silent.
        let audible = project.clip_has_sound(clip) && !clip.disabled && !clip.audio.muted && clip.audio.volume > 0.0 && !track.muted && !(track.kind == TrackKind::Visual && track.hidden);
        if !audible || clip.start >= total {
            continue;
        }
        // Across a transition the sound crossfades over the cut: it starts early (`lead`) and plays
        // on past its end (`tail`), half the transition each way, as far as the file has sound.
        let half_in = project.transition_into(clip).filter(|(p, _)| !p.disabled).map_or(0.0, |(_, tr)| tr.duration / 2.0);
        let half_out = project.transition_out_of(clip).filter(|(n, _)| !n.disabled).map_or(0.0, |(_, tr)| tr.duration / 2.0);
        let speed = speed.max(1e-3);
        let lead = half_in.min(src_in / speed).max(0.0);
        let tail = half_out.min((a.duration - (src_in + clip.duration * speed)) / speed).max(0.0);
        let length = lead + clip.duration + tail;
        let k = first_input + inputs.len();
        inputs.push(AudioInput { path: a.path.clone(), seek: src_in - lead * speed, length: length * speed + 0.05 });
        let mut f = format!("[{k}:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo");
        f.push_str(&atempo(speed));
        f.push_str(&format!(",atrim=0:{length:.6},asetpts=PTS-STARTPTS"));
        if clip.audio.denoise > 0.0 {
            f.push_str(&format!(",afftdn=nr={:.1}", clip.audio.denoise.clamp(0.01, 97.0)));
        }
        f.push_str(&eq_expression(&clip.audio.eq));
        if clip.audio.compressor.enabled {
            f.push_str(&compressor_expression(clip.audio.compressor.amount));
        }
        f.push_str(&format!(",volume={:.4}", clip.audio.volume));
        if !clip.audio.gains.is_empty() {
            // (The clip's own times start `lead` into this sound.)
            let shifted: Vec<crate::clip::Gain> = clip.audio.gains.iter().map(|g| crate::clip::Gain { at: g.at + lead, ..*g }).collect();
            f.push_str(&format!(",volume='{}':eval=frame", gain_expression(&shifted)));
        }
        if clip.audio.fade_in > 0.0 {
            f.push_str(&format!(",afade=t=in:st={lead:.4}:d={:.4}", clip.audio.fade_in.min(clip.duration)));
        }
        if clip.audio.fade_out > 0.0 {
            let d = clip.audio.fade_out.min(clip.duration);
            f.push_str(&format!(",afade=t=out:st={:.4}:d={:.4}", lead + clip.duration - d, d));
        }
        // The crossfade, equal-power so the cut doesn't dip: in from its first sound to half a
        // transition past the cut, out from half a transition before the cut to its last sound.
        if half_in > 0.0 {
            f.push_str(&format!(",afade=t=in:st=0:d={:.4}:curve=qsin", (lead + half_in).min(length)));
        }
        if half_out > 0.0 {
            let d = (half_out + tail).min(length);
            f.push_str(&format!(",afade=t=out:st={:.4}:d={d:.4}:curve=qsin", length - d));
        }
        let ms = ((clip.start - lead).max(0.0) * 1000.0).round() as u64;
        f.push_str(&format!(",adelay=delays={ms}:all=1[a{}]", chains.len()));
        chains.push(f);
    }
    let n = chains.len();
    let mut graph = chains.join(";\n");
    if n == 0 {
        graph = format!("anullsrc=r=48000:cl=stereo,atrim=duration={total:.6}[aout]");
        if let Some(target) = loudness {
            graph = format!("anullsrc=r=48000:cl=stereo,atrim=duration={total:.6},loudnorm=I={target:.1}:TP=-1.5:LRA=11[aout]");
        }
    } else {
        let labels: String = (0..n).map(|i| format!("[a{i}]")).collect();
        // Pad up to the total only: an endless apad keeps FFmpeg generating silence while the
        // video is still encoding, which slows long exports to a crawl.
        graph.push_str(&format!(";\n{labels}amix=inputs={n}:normalize=0:dropout_transition=0,apad=whole_dur={total:.6},atrim=duration={total:.6}"));
        match loudness {
            Some(target) => graph.push_str(&format!(",loudnorm=I={target:.1}:TP=-1.5:LRA=11[aout]")),
            None => graph.push_str("[aout]"),
        }
    }
    (inputs, graph)
}

/// Three `equalizer` stages (low/mid/high), skipping any band left flat.
fn eq_expression(eq: &crate::clip::Eq) -> String {
    if eq.is_flat() {
        return String::new();
    }
    let mut s = String::new();
    if eq.low != 0.0 {
        s.push_str(&format!(",equalizer=f=100:width_type=o:width=1.5:g={:.2}", eq.low));
    }
    if eq.mid != 0.0 {
        s.push_str(&format!(",equalizer=f=1000:width_type=o:width=1.5:g={:.2}", eq.mid));
    }
    if eq.high != 0.0 {
        s.push_str(&format!(",equalizer=f=8000:width_type=o:width=1.5:g={:.2}", eq.high));
    }
    s
}

/// Maps a 0–100% amount to `acompressor`'s threshold/ratio: gentle at low amounts,
/// noticeably tighter (lower threshold, higher ratio) as it approaches 100.
fn compressor_expression(amount: f64) -> String {
    let t = amount.clamp(0.0, 100.0) / 100.0;
    let threshold = 1.0 - t * 0.85; // 1.0 (off) down to 0.15 (linear, matches acompressor's 0..1 scale)
    let ratio = 1.5 + t * 6.5; // 1.5:1 up to 8:1
    format!(",acompressor=threshold={threshold:.3}:ratio={ratio:.2}:attack=20:release=250:makeup=1")
}

/// FFmpeg expression for the range gains, matching `Audio::gain_at` (t is clip-relative).
fn gain_expression(gains: &[crate::clip::Gain]) -> String {
    gains
        .iter()
        .map(|g| {
            let ramp = crate::clip::GAIN_RAMP.min(g.duration / 2.0).max(1e-4);
            format!("(1+({:.4}-1)*clip(min((t-{:.4})/{ramp:.4},({:.4}-t)/{ramp:.4}),0,1))", g.volume, g.at, g.at + g.duration)
        })
        .collect::<Vec<_>>()
        .join("*")
}

/// `atempo` only accepts 0.5–2.0 per stage, so larger changes are chained.
fn atempo(speed: f64) -> String {
    if (speed - 1.0).abs() < 1e-6 {
        return String::new();
    }
    let mut s = speed;
    let mut out = String::new();
    while s > 2.0 {
        out.push_str(",atempo=2.0");
        s /= 2.0;
    }
    while s < 0.5 {
        out.push_str(",atempo=0.5");
        s /= 0.5;
    }
    out.push_str(&format!(",atempo={s:.6}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempo_chains_stay_in_range() {
        assert_eq!(atempo(1.0), "");
        assert_eq!(atempo(4.0), ",atempo=2.0,atempo=2.000000");
        assert_eq!(atempo(0.25), ",atempo=0.5,atempo=0.500000");
    }

    #[test]
    fn range_volume_reaches_the_mix() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("studio-gain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        assert!(crate::ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-y"]).arg(&tone).status().unwrap().success());
        let mut p = Project::new("t", Default::default());
        let a = p.add_asset(&tone, &crate::media::probe(&tone).unwrap());
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        p.set_range_volume(c, 1.0, 2.0, 0.0).unwrap();
        let (inputs, graph) = mix_graph(&p, 0, 3.0, None);
        let script = dir.join("g.txt");
        std::fs::write(&script, graph).unwrap();
        let mut cmd = crate::ffmpeg::ffmpeg().unwrap();
        for i in &inputs {
            cmd.args(["-ss", &crate::ffmpeg::secs(i.seek), "-t", &crate::ffmpeg::secs(i.length), "-i"]).arg(&i.path);
        }
        cmd.arg(crate::ffmpeg::filter_script_option()).arg(&script).args(["-map", "[aout]", "-f", "f32le", "-ac", "1", "-ar", "8000", "pipe:1"]);
        let mut child = cmd.stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut bytes = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut bytes).unwrap();
        child.wait().unwrap();
        let samples: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let rms = |a: f64, b: f64| {
            let s = &samples[(a * 8000.0) as usize..(b * 8000.0) as usize];
            (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
        };
        assert!(rms(0.2, 0.8) > 0.05, "before the range is audible");
        assert!(rms(1.2, 1.8) < 0.001, "the range is muted");
        assert!(rms(2.2, 2.8) > 0.05, "after the range is audible again");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn silent_projects_still_produce_audio() {
        let p = Project::new("t", Default::default());
        let (inputs, graph) = mix_graph(&p, 1, 5.0, None);
        assert!(inputs.is_empty());
        assert!(graph.contains("anullsrc") && graph.ends_with("[aout]"));
    }

    /// EQ, compressor and denoise are hand-written FFmpeg filter expressions — this runs them
    /// through real FFmpeg (alongside loudnorm) to catch a syntax mistake, not just build a string.
    #[test]
    fn audio_polish_filters_are_valid_ffmpeg_syntax() {
        let dir = std::env::temp_dir().join(format!("studio-polish-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        assert!(crate::ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-y"]).arg(&tone).status().unwrap().success());
        let mut p = Project::new("t", Default::default());
        let a = p.add_asset(&tone, &crate::media::probe(&tone).unwrap());
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        {
            let clip = p.clip_mut(c).unwrap();
            clip.audio.eq = crate::clip::Eq { low: 4.0, mid: -3.0, high: 6.0 };
            clip.audio.compressor = crate::clip::Compressor { enabled: true, amount: 70.0 };
            clip.audio.denoise = 12.0;
        }
        let (inputs, graph) = mix_graph(&p, 0, 2.0, Some(-14.0));
        let script = dir.join("g.txt");
        std::fs::write(&script, &graph).unwrap();
        let mut cmd = crate::ffmpeg::ffmpeg().unwrap();
        for i in &inputs {
            cmd.args(["-ss", &crate::ffmpeg::secs(i.seek), "-t", &crate::ffmpeg::secs(i.length), "-i"]).arg(&i.path);
        }
        let out = cmd.arg(crate::ffmpeg::filter_script_option()).arg(&script).args(["-map", "[aout]", "-f", "null", "-"]).output().unwrap();
        assert!(out.status.success(), "ffmpeg rejected the filter graph:\n{graph}\n{}", String::from_utf8_lossy(&out.stderr));
        std::fs::remove_dir_all(dir).ok();
    }
}
