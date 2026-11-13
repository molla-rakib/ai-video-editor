//! Which video encoder a recording uses, and how it is fed.
//!
//! A screen is large (a laptop panel is often 2.8K or more, at 60 pictures a second): x264 on the
//! processor cannot keep up with it, but the graphics chip's encoder (Intel Quick Sync, NVIDIA NVENC,
//! AMD AMF, VAAPI on Linux) takes the same picture for a tenth of the processor time. Whether one
//! works depends on the computer and its drivers, and FFmpeg listing an encoder proves nothing, so
//! each candidate is tried for real on a few pictures (`pick`), and the first that works wins. x264 is
//! always the last resort.
//!
//! On Windows the desktop capture (`ddagrab`) produces pictures that live in graphics memory. The
//! fastest path keeps them there all the way into the encoder (`Path::Gpu`); copying them to the
//! processor's memory first (`Path::Download`) costs about 1.5 GB a second at 3200×2000 and is what
//! makes x264 and downloading encoders fall behind.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ffmpeg;

/// How long one trial may take before the candidate counts as not working (a driver that hangs).
const TRIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// Pictures a trial encodes.
const TRIAL_FRAMES: &str = "6";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoder {
    Nvenc,
    Qsv,
    Amf,
    Vaapi,
    X264,
}

impl Encoder {
    /// FFmpeg's name for it.
    pub fn codec(self) -> &'static str {
        match self {
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Qsv => "h264_qsv",
            Encoder::Amf => "h264_amf",
            Encoder::Vaapi => "h264_vaapi",
            Encoder::X264 => "libx264",
        }
    }

    /// What the recording dialog calls it.
    pub fn label(self) -> &'static str {
        match self {
            Encoder::Nvenc => "NVIDIA NVENC",
            Encoder::Qsv => "Intel Quick Sync",
            Encoder::Amf => "AMD AMF",
            Encoder::Vaapi => "VAAPI",
            Encoder::X264 => "x264 (processor)",
        }
    }

    pub fn is_hardware(self) -> bool {
        self != Encoder::X264
    }

    /// The name `AIVE_RECORD_ENCODER` uses.
    fn word(self) -> &'static str {
        match self {
            Encoder::Nvenc => "nvenc",
            Encoder::Qsv => "qsv",
            Encoder::Amf => "amf",
            Encoder::Vaapi => "vaapi",
            Encoder::X264 => "x264",
        }
    }
}

/// What the pictures are, which decides how they reach the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Frames {
    /// The Windows desktop capture: pictures in graphics memory.
    Desktop,
    /// Ordinary pictures in memory (a camera, X11 screen capture).
    Memory,
}

/// How the pictures get from the capture to the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Path {
    /// Never leave graphics memory.
    Gpu,
    /// Graphics memory, copied to the processor's memory first.
    Download,
    /// Already in memory.
    Memory,
}

/// What kind of picture is recorded: the quality is tuned for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    Camera,
    /// Text and interface: edges must stay sharp.
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Choice {
    pub encoder: Encoder,
    pub path: Path,
}

impl Choice {
    fn new(encoder: Encoder, path: Path) -> Choice {
        Choice { encoder, path }
    }

    /// Words for the dialog: "Intel Quick Sync".
    pub fn label(&self) -> &'static str {
        self.encoder.label()
    }

    /// Options that go before the inputs.
    pub fn global_args(&self) -> Vec<String> {
        if self.encoder == Encoder::Vaapi {
            return vec!["-vaapi_device".into(), vaapi_device()];
        }
        Vec::new()
    }

    /// The filters that end a video chain: they turn the capture's pictures into what the encoder
    /// takes. Empty when it takes them as they are.
    ///
    /// Screen pictures are RGB and H.264 is not, so somewhere they are converted, and the file must
    /// say how. Left implicit, Quick Sync converted them one way and labelled the file another (its
    /// colours came out visibly wrong: 26 dB against the original instead of 47), so every path here
    /// converts explicitly, to BT.709 in limited range, the convention for HD video.
    pub fn filters(&self) -> String {
        const TO_709: &str = "scale=out_color_matrix=bt709:out_range=tv";
        let in_memory = match self.encoder {
            Encoder::X264 => format!("{TO_709},format=yuv420p"),
            Encoder::Vaapi => format!("{TO_709},format=nv12,hwupload"),
            Encoder::Nvenc | Encoder::Qsv | Encoder::Amf => format!("{TO_709},format=nv12"),
        };
        match self.path {
            Path::Memory => in_memory,
            // The desktop capture's pictures are BGRA, whatever the encoder.
            Path::Download => format!("hwdownload,format=bgra,{in_memory}"),
            Path::Gpu => match self.encoder {
                // Quick Sync has its own pictures, mapped from the capture's without a copy, and
                // converts them on the graphics chip.
                Encoder::Qsv => "hwmap=derive_device=qsv,format=qsv,vpp_qsv=format=nv12:out_color_matrix=bt709:out_range=tv".into(),
                // NVENC and AMF take the capture's graphics-memory pictures directly and convert
                // them themselves (`keeps_colours` checks that they do it right before one is picked).
                _ => String::new(),
            },
        }
    }

    /// The encoder's options: constant quality and a keyframe every two seconds. The steady frame
    /// rate is not asked of FFmpeg's output (`-fps_mode cfr` re-bases the start time to 0, which
    /// loses the real gap between picture and sound): put an `fps=N` filter in the graph instead.
    pub fn args(&self, content: Content, fps: u32) -> Vec<String> {
        // Constant quality, like x264's CRF: lower is better. Text needs a little more than a face.
        let quality = match content {
            Content::Camera => 23,
            Content::Screen => 21,
        };
        let q = quality.to_string();
        let mut a: Vec<String> = vec!["-c:v".into(), self.encoder.codec().into()];
        let mut add = |list: &[&str]| a.extend(list.iter().map(|s| s.to_string()));
        match self.encoder {
            Encoder::X264 => add(&["-preset", "superfast", "-crf", &(quality - 1).to_string(), "-pix_fmt", "yuv420p"]),
            Encoder::Qsv => add(&["-global_quality", &q]),
            Encoder::Nvenc => add(&["-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", &q, "-b:v", "0"]),
            Encoder::Amf => add(&["-quality", "balanced", "-rc", "cqp", "-qp_i", &q, "-qp_p", &(quality + 2).to_string()]),
            Encoder::Vaapi => add(&["-qp", &q]),
        }
        // Where the graph converts the picture to BT.709 (`filters`), the file says so: FFmpeg 6 (Linux)
        // doesn't carry the conversion's colour space to the encoder, the file went out unlabelled and
        // was read back as BT.601, a colour cast ([187, 45, 32] for [200, 60, 30]). NVENC and AMF fed
        // from graphics memory convert and label it themselves, and are left alone.
        if self.converts_to_709() {
            add(&["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv"]);
        }
        add(&["-fps_mode", "passthrough", "-g", &(fps * 2).to_string()]);
        a
    }

    /// Whether `filters` converts the picture to BT.709 itself (rather than the encoder).
    fn converts_to_709(&self) -> bool {
        self.path != Path::Gpu || self.encoder == Encoder::Qsv
    }
}

/// The render node VAAPI opens (the first one there is).
fn vaapi_device() -> String {
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("renderD")).collect())
        .unwrap_or_default();
    nodes.sort();
    format!("/dev/dri/{}", nodes.first().map(String::as_str).unwrap_or("renderD128"))
}

/// Everything worth trying for these pictures, best first. x264 is last.
pub fn candidates(frames: Frames) -> Vec<Choice> {
    use Encoder::*;
    use Path::*;
    if cfg!(windows) && frames == Frames::Desktop {
        return vec![
            Choice::new(Nvenc, Gpu),
            Choice::new(Qsv, Gpu),
            Choice::new(Amf, Gpu),
            Choice::new(Nvenc, Download),
            Choice::new(Qsv, Download),
            Choice::new(Amf, Download),
            Choice::new(X264, Download),
        ];
    }
    if cfg!(windows) {
        vec![Choice::new(Nvenc, Memory), Choice::new(Qsv, Memory), Choice::new(Amf, Memory), Choice::new(X264, Memory)]
    } else {
        vec![Choice::new(Vaapi, Memory), Choice::new(Nvenc, Memory), Choice::new(X264, Memory)]
    }
}

/// `AIVE_RECORD_ENCODER=x264|qsv|nvenc|amf|vaapi`: use that one, whatever the trial would say
/// (to compare encoders, or to get out of a driver that misbehaves).
fn parse_override(text: &str) -> Option<Encoder> {
    [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Vaapi, Encoder::X264].into_iter().find(|e| e.word() == text.trim().to_lowercase())
}

/// The first candidate for `frames` that really encodes on this computer (found by trying it, once
/// per run). Takes a second or two the first time, so call it away from the interface.
pub fn pick(frames: Frames) -> Choice {
    static PICKED: Mutex<Option<HashMap<Frames, Choice>>> = Mutex::new(None);
    // One trial run at a time, and a second caller gets the first one's answer.
    static TRYING: Mutex<()> = Mutex::new(());
    let known = |frames: Frames| PICKED.lock().unwrap().as_ref().and_then(|m| m.get(&frames).copied());
    if let Some(choice) = known(frames) {
        return choice;
    }
    let _one_at_a_time = TRYING.lock().unwrap();
    if let Some(choice) = known(frames) {
        return choice;
    }
    let list = candidates(frames);
    let forced = std::env::var("AIVE_RECORD_ENCODER").ok().and_then(|v| parse_override(&v));
    let choice = match forced {
        Some(e) => list.iter().find(|c| c.encoder == e).copied().unwrap_or_else(|| Choice::new(e, if frames == Frames::Desktop && cfg!(windows) { Path::Download } else { Path::Memory })),
        None => first_working(&list, frames).unwrap_or_else(|| *list.last().expect("a last resort")),
    };
    PICKED.lock().unwrap().get_or_insert_with(HashMap::new).insert(frames, choice);
    choice
}

/// The first of `list` that works. Candidates that take the pictures the same way are tried at
/// the same time (each trial is a short FFmpeg run), the cheapest way first.
fn first_working(list: &[Choice], frames: Frames) -> Option<Choice> {
    let mut at = 0;
    while at < list.len() {
        let end = at + list[at..].iter().take_while(|c| c.path == list[at].path).count();
        let results: Vec<bool> = std::thread::scope(|scope| {
            let trials: Vec<_> = list[at..end].iter().map(|c| scope.spawn(move || works(*c, frames))).collect();
            trials.into_iter().map(|t| t.join().unwrap_or(false)).collect()
        });
        if let Some(i) = results.iter().position(|ok| *ok) {
            return Some(list[at + i]);
        }
        at = end;
    }
    None
}

/// Whether `choice` can be used on this computer: it encodes, and the colours come out right.
pub fn works(choice: Choice, frames: Frames) -> bool {
    // Both checks at once: each is a short FFmpeg run.
    std::thread::scope(|scope| {
        let colours = scope.spawn(move || keeps_colours(choice, frames));
        encodes(choice, frames) && colours.join().unwrap_or(false)
    })
}

/// Runs FFmpeg to its end (or ends it after `TRIAL_TIMEOUT`) and says whether it succeeded.
fn finishes(mut cmd: std::process::Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else { return false };
    let deadline = Instant::now() + TRIAL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Encodes a few pictures to nowhere, the way a recording would, and says whether it worked.
pub fn encodes(choice: Choice, frames: Frames) -> bool {
    let source = match frames {
        Frames::Desktop if cfg!(windows) => "ddagrab=framerate=30".to_string(),
        _ => "testsrc2=size=1920x1080:rate=30".to_string(),
    };
    let filters = choice.filters();
    let graph = if filters.is_empty() { source } else { format!("{source},{filters}") };
    let Ok(mut cmd) = ffmpeg::ffmpeg() else { return false };
    cmd.args(choice.global_args()).arg("-filter_complex").arg(graph).args(["-frames:v", TRIAL_FRAMES]).args(choice.args(Content::Camera, 30)).args(["-f", "null", "-"]);
    finishes(cmd)
}

/// A flat, saturated colour. A wrong conversion (the wrong matrix, or full range taken for limited)
/// moves it by 10 to 40 levels; a right one by 1 or 2.
const CHECK_COLOUR: [u8; 3] = [200, 60, 30];
const CHECK_TOLERANCE: i32 = 8;

/// Whether a picture of one known colour still has that colour after `choice` encoded it and
/// FFmpeg decoded it again. Encoders given RGB pictures convert them themselves, and a driver that
/// does it one way and labels the file another gives every recording a colour cast: such an
/// encoder is passed over.
pub fn keeps_colours(choice: Choice, frames: Frames) -> bool {
    colour_after(&choice.global_args(), &choice.filters(), &choice.args(Content::Screen, 30), frames).is_some_and(close_to_check_colour)
}

fn close_to_check_colour(got: [u8; 3]) -> bool {
    got.iter().zip(CHECK_COLOUR).all(|(a, b)| (*a as i32 - b as i32).abs() <= CHECK_TOLERANCE)
}

/// Encodes a few pictures of `CHECK_COLOUR` through `filters` and the encoder, decodes the file
/// and returns the colour found (none if either step failed). On Windows, desktop pictures start
/// in graphics memory, as the capture's do.
fn colour_after(global: &[String], filters: &str, encode: &[String], frames: Frames) -> Option<[u8; 3]> {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = ffmpeg::work_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("encoder-check-{}-{}.mkv", std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let [r, g, b] = CHECK_COLOUR;
    let on_gpu = frames == Frames::Desktop && cfg!(windows);
    let mut graph = format!("color=c=0x{r:02X}{g:02X}{b:02X}:size=1280x720:rate=30,format=bgra");
    if on_gpu {
        graph.push_str(",hwupload");
    }
    if !filters.is_empty() {
        graph.push_str(&format!(",{filters}"));
    }
    let mut cmd = ffmpeg::ffmpeg().ok()?;
    if on_gpu {
        cmd.args(["-init_hw_device", "d3d11va=dx", "-filter_hw_device", "dx"]);
    }
    cmd.args(global).arg("-filter_complex").arg(graph).args(["-frames:v", "5"]).args(encode).arg("-y").arg(&file);
    let encoded = finishes(cmd);
    let decoded = encoded.then(|| ffmpeg::ffmpeg().ok()?.arg("-i").arg(&file).args(["-frames:v", "1", "-vf", "crop=2:2:640:360,format=rgb24", "-f", "rawvideo", "pipe:1"]).stdin(Stdio::null()).output().ok()).flatten();
    let _ = std::fs::remove_file(&file);
    let pixels = decoded?.stdout;
    (pixels.len() >= 3).then(|| [pixels[0], pixels[1], pixels[2]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x264_is_always_the_last_resort() {
        for frames in [Frames::Desktop, Frames::Memory] {
            let list = candidates(frames);
            assert_eq!(list.last().map(|c| c.encoder), Some(Encoder::X264), "{frames:?}");
            assert_eq!(list.iter().filter(|c| c.encoder == Encoder::X264).count(), 1, "{frames:?}");
        }
    }

    #[test]
    fn each_encoder_is_fed_what_it_takes() {
        let f = |e, p| Choice::new(e, p).filters();
        // Quick Sync converts on the graphics chip, saying how; left implicit its colours came out wrong.
        assert_eq!(f(Encoder::Qsv, Path::Gpu), "hwmap=derive_device=qsv,format=qsv,vpp_qsv=format=nv12:out_color_matrix=bt709:out_range=tv");
        assert_eq!(f(Encoder::Nvenc, Path::Gpu), "");
        assert_eq!(f(Encoder::Amf, Path::Gpu), "");
        assert_eq!(f(Encoder::X264, Path::Download), "hwdownload,format=bgra,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p");
        assert_eq!(f(Encoder::Qsv, Path::Download), "hwdownload,format=bgra,scale=out_color_matrix=bt709:out_range=tv,format=nv12");
        assert_eq!(f(Encoder::Nvenc, Path::Memory), "scale=out_color_matrix=bt709:out_range=tv,format=nv12");
        assert_eq!(f(Encoder::Vaapi, Path::Memory), "scale=out_color_matrix=bt709:out_range=tv,format=nv12,hwupload");
        // Every path that converts in the graph says which matrix and range it converts to.
        for c in candidates(Frames::Desktop).into_iter().chain(candidates(Frames::Memory)) {
            let filters = c.filters();
            assert!(filters.is_empty() || filters.contains("bt709") && filters.contains("=tv"), "{c:?}: {filters}");
        }
    }

    #[test]
    fn graphics_memory_paths_exist_only_for_the_desktop_capture() {
        assert!(candidates(Frames::Memory).iter().all(|c| c.path == Path::Memory));
        if cfg!(windows) {
            assert!(candidates(Frames::Desktop).iter().any(|c| c.path == Path::Gpu));
            // The all-graphics-memory paths come before the ones that copy.
            let first_copy = candidates(Frames::Desktop).iter().position(|c| c.path == Path::Download).unwrap();
            assert!(candidates(Frames::Desktop)[..first_copy].iter().all(|c| c.path == Path::Gpu));
        }
    }

    #[test]
    fn every_encoder_gets_constant_quality_untouched_timestamps_and_regular_keyframes() {
        for e in [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Vaapi, Encoder::X264] {
            let args = Choice::new(e, Path::Memory).args(Content::Screen, 60);
            let joined = args.join(" ");
            assert!(joined.starts_with(&format!("-c:v {}", e.codec())), "{joined}");
            // Never `cfr`: it would throw away the real start time of the picture.
            assert!(joined.contains("-fps_mode passthrough -g 120") && !joined.contains("cfr"), "{joined}");
            let quality = ["-crf", "-global_quality", "-cq", "-qp_i", "-qp"].iter().any(|k| args.iter().any(|a| a == k));
            assert!(quality, "{e:?} has no constant-quality option: {joined}");
        }
        // Text gets more quality than a face.
        let crf = |c| Choice::new(Encoder::X264, Path::Memory).args(c, 30).windows(2).find(|w| w[0] == "-crf").map(|w| w[1].parse::<u32>().unwrap()).unwrap();
        assert!(crf(Content::Screen) < crf(Content::Camera));
    }

    #[test]
    fn the_encoder_can_be_forced_by_name() {
        assert_eq!(parse_override("qsv"), Some(Encoder::Qsv));
        assert_eq!(parse_override(" NVENC "), Some(Encoder::Nvenc));
        assert_eq!(parse_override("x264"), Some(Encoder::X264));
        assert_eq!(parse_override("auto"), None);
        assert_eq!(parse_override(""), None);
    }

    #[test]
    fn the_processor_encoder_always_works_and_what_is_picked_works() {
        assert!(works(Choice::new(Encoder::X264, Path::Memory), Frames::Memory));
        let picked = pick(Frames::Memory);
        assert!(works(picked, Frames::Memory), "{picked:?} was picked but fails");
        // Asking again gives the same answer without another trial.
        assert_eq!(pick(Frames::Memory), picked);
        println!("encoder for pictures in memory: {} ({:?})", picked.label(), picked.path);
    }

    #[test]
    fn an_encoder_that_changes_the_colours_is_found_out() {
        let x264 = Choice::new(Encoder::X264, Path::Memory);
        // The real chain keeps the colour (to within a level or two).
        let kept = colour_after(&[], &x264.filters(), &x264.args(Content::Screen, 30), Frames::Memory).expect("x264 encodes");
        assert!(close_to_check_colour(kept), "{kept:?} instead of {CHECK_COLOUR:?}");
        assert!(keeps_colours(x264, Frames::Memory));
        // The fault Quick Sync had: converted one way (BT.601, full range), labelled another.
        let wrong = "scale=out_color_matrix=bt601:out_range=pc,format=yuv420p,setparams=colorspace=bt709:range=tv";
        let cast = colour_after(&[], wrong, &x264.args(Content::Screen, 30), Frames::Memory).expect("it still encodes");
        assert!(!close_to_check_colour(cast), "a mislabelled conversion went unnoticed: {cast:?}");
        // A chain that doesn't encode at all has no colour.
        assert_eq!(colour_after(&[], "no_such_filter", &x264.args(Content::Screen, 30), Frames::Memory), None);
        // No working files are left behind (other tests may be mid-check right now: theirs go too).
        let strays = || std::fs::read_dir(ffmpeg::work_dir()).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("encoder-check-{}-", std::process::id()))).count()).unwrap_or(0);
        let deadline = Instant::now() + Duration::from_secs(30);
        while strays() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(strays(), 0);
    }

    /// Developer tool: what the desktop capture would be encoded with on this computer.
    /// `cargo test -p studio the_desktop_encoder -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn the_desktop_encoder_for_this_computer() {
        for c in candidates(Frames::Desktop) {
            let began = Instant::now();
            let (encodes, colours) = (encodes(c, Frames::Desktop), colour_after(&c.global_args(), &c.filters(), &c.args(Content::Screen, 30), Frames::Desktop));
            let verdict = match (encodes, colours) {
                (false, _) => "does not encode".to_string(),
                (true, None) => "encodes the screen, but not the colour check".to_string(),
                (true, Some(got)) if close_to_check_colour(got) => format!("works, colours right ({got:?} for {CHECK_COLOUR:?})"),
                (true, Some(got)) => format!("encodes, but the colours are WRONG ({got:?} for {CHECK_COLOUR:?}): passed over"),
            };
            println!("{:<18} {:<9} {verdict} in {:.2}s", c.encoder.label(), format!("{:?}", c.path), began.elapsed().as_secs_f64());
        }
        let began = Instant::now();
        let picked = pick(Frames::Desktop);
        println!("picking took {:.2}s", began.elapsed().as_secs_f64());
        println!("picked: {} ({:?})", picked.label(), picked.path);
    }
}
