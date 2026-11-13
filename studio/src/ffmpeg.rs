//! Locating and launching FFmpeg.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

struct Binaries {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

static BINARIES: OnceLock<Option<Binaries>> = OnceLock::new();

fn exe(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_string() }
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("AIVE_FFMPEG_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("ffmpeg"));
        dirs.push(exe_dir);
    }
    // Development: the vendored copy next to the workspace root.
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("vendor").join("ffmpeg"));
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

fn binaries() -> anyhow::Result<&'static Binaries> {
    BINARIES
        .get_or_init(|| {
            candidate_dirs().into_iter().find_map(|dir| {
                let (ffmpeg, ffprobe) = (dir.join(exe("ffmpeg")), dir.join(exe("ffprobe")));
                (ffmpeg.is_file() && ffprobe.is_file()).then_some(Binaries { ffmpeg, ffprobe })
            })
        })
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("FFmpeg was not found. {}", missing_hint()))
}

pub fn missing_hint() -> &'static str {
    if cfg!(windows) {
        "Put ffmpeg.exe and ffprobe.exe in an 'ffmpeg' folder next to the app."
    } else {
        "Install FFmpeg (for example `sudo apt install ffmpeg`) so ffmpeg and ffprobe are on PATH."
    }
}

pub fn is_available() -> bool {
    binaries().is_ok()
}

#[cfg_attr(not(windows), allow(unused_mut))]
fn quiet(mut cmd: Command) -> Command {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

pub fn ffmpeg() -> anyhow::Result<Command> {
    let mut cmd = quiet(Command::new(&binaries()?.ffmpeg));
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
    Ok(cmd)
}

/// FFmpeg that still listens on its stdin, so a long-running capture can be told to finish
/// its file (`q`) instead of being killed.
pub fn ffmpeg_live() -> anyhow::Result<Command> {
    let mut cmd = quiet(Command::new(&binaries()?.ffmpeg));
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostats"]);
    Ok(cmd)
}

pub fn ffprobe() -> anyhow::Result<Command> {
    let mut cmd = quiet(Command::new(&binaries()?.ffprobe));
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    Ok(cmd)
}

/// Seconds with microsecond precision, for ffmpeg arguments.
pub fn secs(t: f64) -> String {
    format!("{:.6}", t.max(0.0))
}

/// The option that reads a filter graph from a file: `-/filter_complex` since
/// FFmpeg 7, `-filter_complex_script` before (e.g. the 6.1 in Ubuntu 24.04).
pub fn filter_script_option() -> &'static str {
    static OPTION: OnceLock<&'static str> = OnceLock::new();
    OPTION.get_or_init(|| {
        let major = binaries()
            .ok()
            .and_then(|b| quiet(Command::new(&b.ffmpeg)).arg("-version").output().ok())
            .and_then(|out| parse_major_version(&String::from_utf8_lossy(&out.stdout)));
        match major {
            Some(v) if v < 7 => "-filter_complex_script",
            _ => "-/filter_complex",
        }
    })
}

/// `ffmpeg version 6.1.1-3ubuntu5 ...` -> 6, `n7.1-...` -> 7. Git builds (`N-118000-...`) give None.
fn parse_major_version(version_output: &str) -> Option<u32> {
    let v = version_output.split_whitespace().nth(2)?;
    v.strip_prefix('n').unwrap_or(v).split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Per-user scratch folder for filter graphs and other temporary files.
pub fn work_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("ai-video-editor");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::parse_major_version;

    #[test]
    fn reads_ffmpeg_major_version() {
        assert_eq!(parse_major_version("ffmpeg version 6.1.1-3ubuntu5 Copyright"), Some(6));
        assert_eq!(parse_major_version("ffmpeg version n7.1-12-g1234 Copyright"), Some(7));
        assert_eq!(parse_major_version("ffmpeg version 7.1-full_build-www.gyan.dev Copyright"), Some(7));
        assert_eq!(parse_major_version("ffmpeg version N-118000-gabc Copyright"), None);
    }
}
