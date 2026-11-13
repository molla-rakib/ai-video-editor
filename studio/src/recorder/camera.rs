//! Finding cameras and microphones and choosing how to open a camera: DirectShow on Windows,
//! the V4L2 driver and PulseAudio on Linux. Nothing here records; `Session` in the parent module does.

use super::*;

/// One way a camera can deliver pictures, as DirectShow lists them.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mode {
    /// `("pixel_format", "yuyv422")` or `("vcodec", "mjpeg")`.
    pub(super) format: (String, String),
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) min_fps: f64,
    pub(super) max_fps: f64,
}

/// Reads `ffmpeg -list_devices true -f dshow -i dummy`: names in quotes followed by their kind.
/// Devices that don't say what they are (virtual cameras) are offered as cameras.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) fn parse_dshow_devices(text: &str) -> Devices {
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
pub(super) fn parse_dshow_modes(text: &str) -> Vec<Mode> {
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

/// The FFmpeg name of a V4L2 picture format: `("vcodec", "mjpeg")` for a compressed one,
/// `("pixel_format", …)` for a raw one. Formats FFmpeg's camera input can't take are `None`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(super) fn v4l2_format(fourcc: &[u8; 4]) -> Option<(&'static str, &'static str)> {
    Some(match fourcc {
        b"MJPG" | b"JPEG" => ("vcodec", "mjpeg"),
        b"YUYV" => ("pixel_format", "yuyv422"),
        b"UYVY" => ("pixel_format", "uyvy422"),
        b"NV12" => ("pixel_format", "nv12"),
        b"YU12" => ("pixel_format", "yuv420p"),
        b"GREY" => ("pixel_format", "gray"),
        _ => return None,
    })
}

/// One picture size of a V4L2 format, with the frame rates the driver offers for it.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(super) fn v4l2_mode(fourcc: &[u8; 4], width: u32, height: u32, rates: &[f64]) -> Option<Mode> {
    let (key, name) = v4l2_format(fourcc)?;
    let rates: Vec<f64> = rates.iter().copied().filter(|r| r.is_finite() && *r > 0.0).collect();
    // A driver that doesn't list its rates usually runs at the common 30.
    let (min_fps, max_fps) = if rates.is_empty() { (30.0, 30.0) } else { (rates.iter().copied().fold(f64::INFINITY, f64::min), rates.iter().copied().fold(0.0, f64::max)) };
    Some(Mode { format: (key.to_string(), name.to_string()), width, height, min_fps, max_fps })
}

/// The kernel cuts a camera's name to 31 characters, and USB cameras often repeat it after a
/// colon ("Integrated Camera: Integrated C"): the name once.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(super) fn camera_label(card: &str) -> String {
    let card = card.trim();
    if let Some((name, rest)) = card.split_once(": ") {
        let (name, rest) = (name.trim(), rest.trim());
        if !name.is_empty() && name.starts_with(rest) {
            return name.to_string();
        }
    }
    card.to_string()
}

/// Two cameras of the same name are told apart: "Webcam", "Webcam (2)".
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(super) fn number_duplicates(devices: &mut [Device]) {
    let labels: Vec<String> = devices.iter().map(|d| d.label.clone()).collect();
    for (i, device) in devices.iter_mut().enumerate() {
        let before = labels[..i].iter().filter(|l| **l == labels[i]).count();
        if before > 0 {
            device.label = format!("{} ({})", labels[i], before + 1);
        }
    }
}

/// Leaves out infrared cameras (greyscale only; laptops use them for face sign-in) when there
/// is a colour camera to record with.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(super) fn colour_cameras(found: Vec<(Device, Vec<Mode>)>) -> Vec<(Device, Vec<Mode>)> {
    let grey = |modes: &[Mode]| !modes.is_empty() && modes.iter().all(|m| m.format.1 == "gray");
    if found.iter().any(|(_, modes)| !grey(modes)) {
        found.into_iter().filter(|(_, modes)| !grey(modes)).collect()
    } else {
        found
    }
}

/// Reads `ffmpeg -sources pulse`: `name [description]` per line, `*` before the default one.
/// Outputs' "monitor" sources (what the speakers play) aren't microphones and are left out;
/// the default microphone comes first.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(super) fn parse_pulse_sources(text: &str) -> Vec<Device> {
    let mut found = Vec::new();
    for line in text.lines() {
        let (default, rest) = match line.strip_prefix('*') {
            Some(rest) => (true, rest),
            // The heading doesn't start with a space; listed sources do.
            None if line.starts_with(' ') => (false, line),
            None => continue,
        };
        let Some((name, after)) = rest.trim().split_once(" [") else { continue };
        let Some(end) = after.rfind(']') else { continue };
        let (name, label) = (name.trim(), after[..end].trim());
        if name.is_empty() || name.ends_with(".monitor") {
            continue;
        }
        let device = Device { id: name.to_string(), label: if label.is_empty() { name.to_string() } else { label.to_string() } };
        if default {
            found.insert(0, device);
        } else {
            found.push(device);
        }
    }
    found
}

/// The mode to record with: the largest picture up to Full HD that reaches 30 pictures a
/// second (or the fastest there is), uncompressed rather than compressed when both will do.
#[cfg_attr(not(any(windows, target_os = "linux", test)), allow(dead_code))]
pub(super) fn best_mode(modes: &[Mode]) -> Option<&Mode> {
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
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(super) static MODES: Mutex<Option<HashMap<String, Option<Mode>>>> = Mutex::new(None);

#[cfg(windows)]
pub(super) mod system {
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
pub(super) mod system {
    use super::*;

    /// The cameras in `/sys/class/video4linux`, as their drivers describe them, and the
    /// microphones the sound server knows.
    pub fn devices() -> anyhow::Result<Devices> {
        let mut found: Vec<(Device, Vec<Mode>)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") {
            for entry in entries.flatten() {
                let id = format!("/dev/{}", entry.file_name().to_string_lossy());
                let name = std::fs::read_to_string(entry.path().join("name")).map(|n| n.trim().to_string()).unwrap_or_else(|_| id.clone());
                match v4l2::query(&id) {
                    // Nodes that don't capture pictures carry a camera's metadata.
                    Some(info) if info.captures => found.push((Device { id, label: camera_label(if info.card.is_empty() { &name } else { &info.card }) }, info.modes)),
                    Some(_) => {}
                    // The driver couldn't be asked (no permission yet): each camera's first node.
                    None if std::fs::read_to_string(entry.path().join("index")).is_ok_and(|i| i.trim() == "0") => found.push((Device { id, label: camera_label(&name) }, Vec::new())),
                    None => {}
                }
            }
        }
        found.sort_by_key(|(d, _)| (d.id.trim_start_matches("/dev/video").parse::<u32>().unwrap_or(u32::MAX), d.id.clone()));
        let found = colour_cameras(found);
        {
            let mut modes = MODES.lock().unwrap();
            let modes = modes.get_or_insert_with(HashMap::new);
            for (device, list) in found.iter().filter(|(_, list)| !list.is_empty()) {
                modes.insert(device.id.clone(), best_mode(list).cloned());
            }
        }
        let mut cameras: Vec<Device> = found.into_iter().map(|(d, _)| d).collect();
        number_duplicates(&mut cameras);
        Ok(Devices { cameras, mics: microphones() })
    }

    /// The sound server's microphones, or its default one when they can't be listed (ALSA's
    /// when this FFmpeg can't talk to a sound server).
    fn microphones() -> Vec<Device> {
        let listed = ffmpeg::ffmpeg().ok().and_then(|mut c| c.args(["-sources", "pulse"]).output().ok()).map(|o| parse_pulse_sources(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default();
        if !listed.is_empty() {
            return listed;
        }
        let has_pulse = ffmpeg::ffmpeg().ok().and_then(|mut c| c.arg("-devices").output().ok()).is_none_or(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim_start().starts_with('D') && l.contains(" pulse ")));
        vec![Device { id: if has_pulse { "default" } else { "alsa:default" }.into(), label: "Default microphone".into() }]
    }

    fn mode_of(camera: &str) -> Option<Mode> {
        if let Some(known) = MODES.lock().unwrap().get_or_insert_with(HashMap::new).get(camera) {
            return known.clone();
        }
        let mode = v4l2::query(camera).and_then(|info| best_mode(&info.modes).cloned());
        MODES.lock().unwrap().get_or_insert_with(HashMap::new).insert(camera.to_string(), mode.clone());
        mode
    }

    pub fn input(setup: &Setup) -> anyhow::Result<Input> {
        let mode = mode_of(&setup.camera.id);
        let mut args: Vec<String> = vec!["-f".into(), "v4l2".into()];
        let mut fps = 30;
        match &mode {
            // The format matters: many cameras send Full HD or 720p smoothly only compressed
            // (MJPEG), and fall back to a few pictures a second when the format is left to them.
            Some(m) => {
                let rate = 30f64.clamp(m.min_fps.min(m.max_fps), m.max_fps);
                fps = rate.round().max(1.0) as u32;
                args.extend(["-input_format".to_string(), m.format.1.clone(), "-video_size".to_string(), format!("{}x{}", m.width, m.height), "-framerate".to_string(), format!("{rate}")]);
            }
            None => args.extend(["-framerate", "30", "-video_size", "1280x720"].map(String::from)),
        }
        args.extend(["-i".to_string(), setup.camera.id.clone()]);
        if let Some(mic) = &setup.mic {
            let (format, source) = match mic.id.strip_prefix("alsa:") {
                Some(source) => ("alsa", source),
                None => ("pulse", mic.id.as_str()),
            };
            args.extend(["-f".to_string(), format.to_string()]);
            if format == "pulse" {
                // Sound in 20 ms pieces (48 kHz stereo, 16 bit). Left to the sound server they
                // are several times longer, and FFmpeg then hands pictures over in bursts: the
                // live picture stutters and the level meter lags.
                args.extend(["-fragment_size".to_string(), "3840".to_string()]);
            }
            args.extend(["-i".to_string(), source.to_string()]);
        }
        // Without a known mode the driver may pick another size than asked for, so the preview doesn't assume one.
        Ok(Input { args, audio: setup.mic.as_ref().map(|_| "1:a"), size: mode.map(|m| (m.width, m.height)), fps })
    }

    /// Asks a camera's driver what it is and which pictures it can send (V4L2 ioctls; nothing
    /// is switched on).
    mod v4l2 {
        use super::super::{v4l2_format, v4l2_mode, Mode};
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        #[repr(C)]
        #[derive(Default)]
        struct Capability {
            driver: [u8; 16],
            card: [u8; 32],
            bus_info: [u8; 32],
            version: u32,
            capabilities: u32,
            device_caps: u32,
            reserved: [u32; 3],
        }

        #[repr(C)]
        #[derive(Default)]
        struct FmtDesc {
            index: u32,
            kind: u32,
            flags: u32,
            description: [u8; 32],
            pixelformat: u32,
            mbus_code: u32,
            reserved: [u32; 3],
        }

        /// `data`: width, height (a single size), or min/max/step width then height (a range).
        #[repr(C)]
        #[derive(Default)]
        struct FrmSize {
            index: u32,
            pixel_format: u32,
            kind: u32,
            data: [u32; 6],
            reserved: [u32; 2],
        }

        /// `data`: one interval as numerator, denominator; or min, max, step intervals (a range).
        #[repr(C)]
        #[derive(Default)]
        struct FrmIval {
            index: u32,
            pixel_format: u32,
            width: u32,
            height: u32,
            kind: u32,
            data: [u32; 6],
            reserved: [u32; 2],
        }

        const _: () = assert!(size_of::<Capability>() == 104 && size_of::<FmtDesc>() == 64 && size_of::<FrmSize>() == 44 && size_of::<FrmIval>() == 52);

        const fn request(dir: u64, nr: u64, size: usize) -> u64 {
            (dir << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | nr
        }
        const QUERYCAP: u64 = request(2, 0, size_of::<Capability>());
        const ENUM_FMT: u64 = request(3, 2, size_of::<FmtDesc>());
        const ENUM_FRAMESIZES: u64 = request(3, 74, size_of::<FrmSize>());
        const ENUM_FRAMEINTERVALS: u64 = request(3, 75, size_of::<FrmIval>());
        const CAP_VIDEO_CAPTURE: u32 = 0x1;
        const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
        const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
        const DISCRETE: u32 = 1;

        pub struct Info {
            pub card: String,
            pub captures: bool,
            pub modes: Vec<Mode>,
        }

        fn ask<T>(fd: i32, request: u64, value: &mut T) -> bool {
            loop {
                // SAFETY: `value` is the #[repr(C)] struct this request reads and fills, of the size the request names.
                if unsafe { libc::ioctl(fd, request as _, value as *mut T) } == 0 {
                    return true;
                }
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    return false;
                }
            }
        }

        pub fn query(node: &str) -> Option<Info> {
            let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(node).ok()?;
            let fd = file.as_raw_fd();
            let mut cap = Capability::default();
            if !ask(fd, QUERYCAP, &mut cap) {
                return None;
            }
            let card = String::from_utf8_lossy(cap.card.split(|b| *b == 0).next().unwrap_or_default()).trim().to_string();
            // Newer drivers describe each node; older ones only the whole device.
            let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 { cap.device_caps } else { cap.capabilities };
            let captures = caps & CAP_VIDEO_CAPTURE != 0;
            Some(Info { card, captures, modes: if captures { modes(fd) } else { Vec::new() } })
        }

        fn modes(fd: i32) -> Vec<Mode> {
            let mut out = Vec::new();
            for index in 0..64 {
                let mut format = FmtDesc { index, kind: BUF_TYPE_VIDEO_CAPTURE, ..Default::default() };
                if !ask(fd, ENUM_FMT, &mut format) {
                    break;
                }
                let fourcc = format.pixelformat.to_le_bytes();
                if v4l2_format(&fourcc).is_some() {
                    for (w, h) in sizes(fd, format.pixelformat) {
                        out.extend(v4l2_mode(&fourcc, w, h, &rates(fd, format.pixelformat, w, h)));
                    }
                }
            }
            out
        }

        fn sizes(fd: i32, format: u32) -> Vec<(u32, u32)> {
            let mut out = Vec::new();
            for index in 0..64 {
                let mut size = FrmSize { index, pixel_format: format, ..Default::default() };
                if !ask(fd, ENUM_FRAMESIZES, &mut size) {
                    break;
                }
                if size.kind == DISCRETE {
                    out.push((size.data[0], size.data[1]));
                    continue;
                }
                // A range of sizes: the usual ones that fit, and the largest.
                let [min_w, max_w, _, min_h, max_h, _] = size.data;
                out.extend([(1920, 1080), (1280, 720), (640, 480), (max_w, max_h)].into_iter().filter(|(w, h)| (min_w..=max_w).contains(w) && (min_h..=max_h).contains(h)));
                break;
            }
            out
        }

        fn rates(fd: i32, format: u32, width: u32, height: u32) -> Vec<f64> {
            let fps = |num: u32, den: u32| (num > 0).then(|| den as f64 / num as f64);
            let mut out = Vec::new();
            for index in 0..64 {
                let mut ival = FrmIval { index, pixel_format: format, width, height, ..Default::default() };
                if !ask(fd, ENUM_FRAMEINTERVALS, &mut ival) {
                    break;
                }
                out.extend(fps(ival.data[0], ival.data[1]));
                if ival.kind != DISCRETE {
                    // A range: the shortest interval above, the longest here.
                    out.extend(fps(ival.data[2], ival.data[3]));
                    break;
                }
            }
            out
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(super) mod system {
    use super::*;

    pub fn devices() -> anyhow::Result<Devices> {
        anyhow::bail!("Recording isn't available on this system yet")
    }

    pub fn input(_: &Setup) -> anyhow::Result<Input> {
        anyhow::bail!("Recording isn't available on this system yet")
    }
}
