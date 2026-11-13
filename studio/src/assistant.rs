//! The built-in assistant: runs the user's own Claude Code or Codex (ChatGPT) headless, connected
//! to this editor's MCP server, and streams its replies and tool calls back.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow};
use serde_json::{Value, json};

use crate::mcp::{self, SERVER_KEY};
use crate::settings::AiProvider;

#[derive(Debug, Clone)]
pub enum Event {
    TextDelta(String),
    Text(String),
    ToolStart { id: String, name: String, input: Value },
    ToolDone { id: String, ok: bool, summary: String, image: Option<Vec<u8>> },
    TurnDone { error: Option<String> },
}

const SYSTEM_PROMPT: &str = "You are the motion designer built into AI Video Editor, a motion graphics editor for product \
showcases: Apple-style product launches and SaaS explainers built from screen recordings. The user watches every change \
live in the preview and can undo anything, so act directly with the ai-video-editor tools instead of describing what you \
would do. Call get_project first when you need ids, tracks or times.

Each user message starts with an <editor> line: the playhead, the selected clip and, if set, the selected time range. When a \
range is selected, the user means that part of the timeline: put new elements and changes inside it.

Design like a premium product film:
- Typography: few words, big and bold. Inter or Poppins (600–700) for English, Hind Siliguri (600) for Bangla. Headlines \
around 90–140 units, subtitles 40–60. Keep text off the edges (x 0.1–0.9, y 0.1–0.9) and centered unless asked. \
To make one keyword pop (the product name, a number), use style_text on just that word: the accent color, a marker \
highlight or a bolder weight.
- Motion: give titles an entrance (slide_up or fade, 0.4–0.7 s) and an exit; pop for callouts and badges; crossfade or \
zoom_through between scenes. Use set_keyframes for slow drifts and scale-ups on hero shots.
- Screen recordings: use add_zoom to guide the eye to the UI being discussed (scale 1.6–2.5, focus on that element), with \
back-to-back zooms to pan between parts. Leave breathing room between zooms.
- Frames: rounded corners (24–48), a soft shadow and a thin border make face-cam and screenshot overlays look polished.
- Sound: a whoosh on transitions, a pop or click on callouts, a riser before a big reveal; keep effects subtle (volume 0.5–0.8).
- Colors: match the brand if the user gives one; otherwise white text on dark or the accent #7C6DFF.
- Timing: whenever there's music or a narration track, call analyze_audio_beats on it before animating anything over it, and land \
zooms, cuts and text entrances on the returned times. Pacing that matches the audio is what reads as professional instead of \
arbitrary; don't guess timing against audio you haven't analyzed.
- Glow icons (add_glow_icon): a sparse accent, not decoration. Use at most one or two per video, only at a genuinely important \
moment (a result landing, a step completing, a point being emphasized) — never on every cut or every clip. Keep them short \
(around 1-1.5 s); let the pop-in read as the moment, don't leave pulse on for long stretches. Overusing these reads as cheap, \
not premium.
- Other polish tools, used the same sparingly: add_press_pulse for a quick click-scale dip right when something is tapped or \
clicked in a screen recording; add_counter for a before/after number that counts up or down, eased, never a one-frame jump; \
add_camera_move for a slow, deliberate push in or pull back held to the end of a clip (different from add_zoom, which calls \
attention to part of the frame); idle_drift (on update_clip) to keep a long held shot from looking frozen; intro_ease: \"back\" \
for a bouncy, springy entrance; stagger_entrances to offset a group's intros a few frames apart instead of all firing at once.
After a few changes, use get_frame to check your work visually and fix anything that looks off (overlaps, text too small or \
cut off). Keep replies short and friendly: one or two sentences about what you changed.";

/// Locates the command-line app a provider runs: Claude Code (`claude`) or Codex (`codex`).
pub fn find_cli(provider: AiProvider) -> Option<PathBuf> {
    let base = match provider {
        AiProvider::ClaudeCode => "claude",
        AiProvider::Codex => "codex",
    };
    let names: Vec<String> = if cfg!(windows) { vec![format!("{base}.exe"), format!("{base}.cmd")] } else { vec![base.to_string()] };
    let home = crate::settings::home_dir();
    // Native installers put it in ~/.local/bin (Claude Code) or ~/.codex/bin; npm in its global folder.
    let mut candidates = vec![home.join(".local").join("bin").join(&names[0]), home.join(".codex").join("bin").join(&names[0])];
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.extend(names.iter().map(|n| dir.join(n)));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("npm").join(format!("{base}.cmd")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// A command that runs `exe` without a console window (npm's `.cmd` shims through `cmd /C`),
/// in its own process group so everything it starts can be ended together.
fn cli_command(exe: &Path) -> std::process::Command {
    let mut cmd = if exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd")) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(exe);
        c
    } else {
        std::process::Command::new(exe)
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    crate::platform::own_process_group(&mut cmd);
    cmd
}

/// A friendly line for a tool call, shown in the chat.
pub fn describe_tool(name: &str, input: &Value) -> String {
    let short = name.strip_prefix(&format!("mcp__{SERVER_KEY}__")).unwrap_or(name);
    let t = |k: &str| input.get(k).and_then(Value::as_f64).map(|v| format!("{v:.1}s")).unwrap_or_default();
    let q = |k: &str| input.get(k).and_then(Value::as_str).map(|s| s.chars().take(36).collect::<String>().replace('\n', " ")).unwrap_or_default();
    match short {
        "get_project" => "Looking at the project".into(),
        "import_media" => "Importing media".into(),
        "add_media_clip" => format!("Placing media at {}", t("start")),
        "add_text" => format!("Adding “{}”", q("text")),
        "add_shape" => "Adding a shape".into(),
        "add_glow_icon" => format!("Adding a glow icon ({})", q("icon")),
        "update_clip" => "Adjusting a clip".into(),
        "move_clip" => format!("Moving a clip to {}", t("start")),
        "trim_clip" => "Trimming a clip".into(),
        "split_clip" => format!("Splitting at {}", t("time")),
        "delete_clips" => "Removing clips".into(),
        "duplicate_clip" => "Duplicating a clip".into(),
        "analyze_audio_beats" => "Listening for the beat".into(),
        "add_zoom" => format!("Zooming in at {}", t("start")),
        "update_zoom" => "Adjusting a zoom".into(),
        "remove_zoom" => "Removing a zoom".into(),
        "set_keyframes" => format!("Animating {}", q("property")),
        "style_text" => format!("Styling “{}”", q("words")),
        "arrange_clip" => if q("to") == "back" { "Sending a layer to the back".into() } else { "Bringing a layer to the front".into() },
        "clear_keyframes" => "Clearing animation".into(),
        "add_sound" => format!("Adding a {} sound", q("sound")),
        "add_track" => "Adding a track".into(),
        "rename_track" => "Renaming a track".into(),
        "set_canvas" => "Changing the format".into(),
        "get_frame" => format!("Checking the frame at {}", t("time")),
        "seek" => format!("Moving the playhead to {}", t("time")),
        "play" => "Playing the preview".into(),
        "pause" => "Pausing".into(),
        "undo" => "Undoing".into(),
        "redo" => "Redoing".into(),
        "export_video" => "Exporting the video".into(),
        "get_export_status" => "Checking the export".into(),
        "Glob" => format!("Looking for files {}", q("pattern")),
        "Read" => "Reading a file".into(),
        "ToolSearch" => "Getting ready".into(),
        "Shell" => "Running a command".into(),
        "WebSearch" => format!("Searching the web for “{}”", q("query")),
        other => format!("Using {other}"),
    }
}

/// Checks that Claude Code runs, without using any tokens. Returns its version line.
pub fn claude_version() -> Result<String, String> {
    let claude = find_cli(AiProvider::ClaudeCode).ok_or("Claude Code isn't installed. Install it from claude.com/claude-code and sign in once.")?;
    let out = cli_command(&claude).arg("--version").stdin(Stdio::null()).output().map_err(|e| format!("Couldn't run Claude Code: {e}"))?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || version.is_empty() {
        return Err("Claude Code didn't respond. Open a terminal, run `claude` once to sign in, then try again.".into());
    }
    Ok(version.split_whitespace().next().unwrap_or(&version).to_string())
}

/// How to install Codex (it needs Node.js).
pub const CODEX_INSTALL: &str = "npm install -g @openai/codex";
/// `codex_status`'s answer when Codex is installed but not signed in.
pub const CODEX_SIGNED_OUT: &str = "Sign in with your ChatGPT account to use it here.";

/// Checks that Codex is installed and signed in, without using any tokens. Returns its version.
pub fn codex_status() -> Result<String, String> {
    let codex = find_cli(AiProvider::Codex).ok_or_else(|| format!("Codex isn't installed. Install it (it needs Node.js) with  {CODEX_INSTALL}  then click Connect again."))?;
    let out = cli_command(&codex).arg("--version").stdin(Stdio::null()).output().map_err(|e| format!("Couldn't run Codex: {e}"))?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || line.is_empty() {
        return Err("Codex didn't respond. Open a terminal and run `codex --version` to see why.".into());
    }
    let signed_in = cli_command(&codex).args(["login", "status"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if !signed_in {
        return Err(CODEX_SIGNED_OUT.into());
    }
    // "codex-cli 0.150.0" → "0.150.0"
    Ok(line.split_whitespace().last().unwrap_or(&line).to_string())
}

/// Signs Codex in with the user's ChatGPT account: `codex login` opens the sign-in page in
/// the browser. Waits up to five minutes for the user to finish there.
pub fn codex_sign_in() -> Result<(), String> {
    let codex = find_cli(AiProvider::Codex).ok_or("Codex isn't installed")?;
    let mut child = cli_command(&codex).arg("login").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Couldn't start Codex: {e}"))?;
    let group = crate::platform::ProcessGroup::of(&child);
    // Keep its output (for the error message) without letting a full pipe block it.
    let said: Arc<Mutex<String>> = Arc::default();
    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)].into_iter().flatten() {
        let said = said.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                said.lock().unwrap().push_str(&format!("{line}\n"));
            }
        });
    }
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let said = said.lock().unwrap();
                let last = said.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("Signing in didn't finish. Try again.");
                return Err(last.to_string());
            }
            Ok(None) if started.elapsed() > std::time::Duration::from_secs(300) => {
                group.kill(&mut child);
                return Err("Signing in took too long. Click Sign in to try again.".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(300)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The system prompt, with the platform the editor runs on and the user's own enabled skills
/// (already joined by the caller, empty if none).
fn instructions(skills: &str) -> String {
    let os = if cfg!(windows) { "Windows" } else { "Linux" };
    let mut s = format!("{SYSTEM_PROMPT}\nThe editor runs on {os}; file paths are {os} paths.");
    if !skills.is_empty() {
        s.push_str("\n\nThe user has also written these additional skills for you:\n\n");
        s.push_str(skills);
    }
    s
}

/// What one assistant process has reported, collected by its reader thread.
#[derive(Default)]
struct Inbox {
    events: Vec<Event>,
    /// The conversation's id (Claude Code session, Codex thread), to continue it later.
    session: Option<String>,
    /// The process got as far as starting the conversation.
    started: bool,
    /// The process closed its output: it has ended.
    exited: bool,
    /// What it wrote to stderr, once it has ended.
    stderr: String,
}

/// One assistant process. Each has its own inbox, so nothing a stopped process still
/// says can reach a later reply. Dropping it ends the process and everything it started.
struct Running {
    child: Child,
    group: crate::platform::ProcessGroup,
    /// Claude Code takes every message on stdin; Codex reads one prompt and gets EOF.
    stdin: Option<ChildStdin>,
    inbox: Arc<Mutex<Inbox>>,
    /// Names this process in its MCP requests, so the editor can refuse calls from a stopped one.
    tag: String,
    /// Continues an earlier conversation.
    resumed: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.group.kill(&mut self.child);
    }
}

/// Reads a provider's output format.
enum Parser {
    Claude(StreamParser),
    Codex(CodexParser),
}

impl Parser {
    fn handle(&mut self, v: &Value) -> Vec<Event> {
        match self {
            Parser::Claude(p) => p.handle(v),
            Parser::Codex(p) => p.handle(v),
        }
    }

    /// The conversation id in a line, if it carries one.
    fn session(&self, v: &Value) -> Option<String> {
        match self {
            Parser::Claude(_) => v["session_id"].as_str().map(str::to_string),
            Parser::Codex(_) => (v["type"] == "thread.started").then(|| v["thread_id"].as_str().map(str::to_string)).flatten(),
        }
    }

    /// The line that shows the conversation has started.
    fn starts(&self, v: &Value) -> bool {
        match self {
            Parser::Claude(_) => v["type"] == "system" && v["subtype"] == "init",
            Parser::Codex(_) => v["type"] == "thread.started",
        }
    }
}

/// The built-in assistant. Owns at most one process of the connected provider's app; the
/// conversation carries over between processes through its id (Claude Code `--resume`,
/// Codex `exec resume`). Claude Code keeps one process for many replies, Codex runs one per reply.
#[derive(Default)]
pub struct Agent {
    provider: Option<AiProvider>,
    exe: Option<PathBuf>,
    running: Option<Running>,
    session: Option<String>,
    /// A message was sent and its reply hasn't finished, failed or been stopped.
    pub busy: bool,
}

impl Agent {
    /// Connected by the user and installed.
    pub fn available(&self) -> bool {
        self.provider.is_some() && self.exe.is_some()
    }

    /// Switches to the provider the user connected (or none). A different provider starts
    /// a new conversation: the old one lives in the other app.
    pub fn set_provider(&mut self, provider: Option<AiProvider>) {
        if provider != self.provider {
            self.reset();
        }
        self.provider = provider;
        self.recheck();
    }

    pub fn recheck(&mut self) {
        self.exe = self.provider.and_then(find_cli);
    }

    /// The app doing the work, for messages.
    fn app_name(&self) -> &'static str {
        match self.provider {
            Some(AiProvider::Codex) => "Codex",
            _ => "Claude Code",
        }
    }

    /// True if an MCP call tagged `run` comes from the reply in progress. Calls from a
    /// stopped or replaced process are refused, so nothing edits after the user pressed Stop.
    pub fn accepts(&self, run: &str) -> bool {
        self.busy && self.running.as_ref().is_some_and(|r| r.tag == run)
    }

    fn new_tag() -> String {
        static RUNS: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("{}-{}-{nanos}", std::process::id(), RUNS.fetch_add(1, Ordering::Relaxed))
    }

    /// Claude Code: one long-lived process reading stream-json messages on stdin.
    fn spawn_claude(&mut self, exe: &Path, cwd: &Path, skills: &str) -> anyhow::Result<()> {
        let tag = Self::new_tag();
        let dir = crate::ffmpeg::work_dir();
        let cfg = dir.join(format!("assistant-mcp-{}.json", std::process::id()));
        let server = json!({ "type": "http", "url": mcp::url(), "headers": { mcp::RUN_HEADER: tag } });
        std::fs::write(&cfg, serde_json::to_string_pretty(&json!({ "mcpServers": { SERVER_KEY: server } }))?)?;
        let prompt = dir.join(format!("assistant-prompt-{}.txt", std::process::id()));
        std::fs::write(&prompt, instructions(skills))?;
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--strict-mcp-config",
            // Only file search and reading; all editing goes through the editor's tools.
            "--tools",
            "Glob,Read",
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        args.extend(["--mcp-config".into(), cfg.display().to_string()]);
        args.extend(["--allowedTools".into(), format!("mcp__{SERVER_KEY},Glob,Read")]);
        args.extend(["--append-system-prompt-file".into(), prompt.display().to_string()]);
        let resumed = self.session.is_some();
        if let Some(id) = &self.session {
            args.extend(["--resume".into(), id.clone()]);
        }
        let mut cmd = cli_command(exe);
        cmd.args(&args).current_dir(cwd);
        self.launch(cmd, Parser::Claude(StreamParser::default()), tag, resumed)
    }

    /// Codex: `codex exec --json` for one reply, reading the prompt from stdin. Settings go
    /// in as `-c key='value'` (TOML literal strings: no double quotes to mangle through `cmd /C`);
    /// the run tag rides in the MCP URL.
    fn spawn_codex(&mut self, exe: &Path, cwd: &Path, prompt: &str) -> anyhow::Result<()> {
        let tag = Self::new_tag();
        let server = format!("mcp_servers.{SERVER_KEY}");
        let mut args: Vec<String> = vec![
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            // It may look at files (to import media) but never change anything outside the editor.
            "--sandbox".into(),
            "read-only".into(),
            "-c".into(),
            "approval_policy='never'".into(),
            "-c".into(),
            format!("{server}.url='{}?run={tag}'", mcp::url()),
            // The editor's tools run without asking: every change shows live and can be undone.
            "-c".into(),
            format!("{server}.default_tools_approval_mode='approve'"),
            "-c".into(),
            format!("{server}.tool_timeout_sec=180"),
        ];
        let resumed = self.session.is_some();
        if let Some(id) = &self.session {
            args.extend(["resume".into(), id.clone()]);
        }
        args.push("-".into());
        let mut cmd = cli_command(exe);
        cmd.args(&args).current_dir(cwd);
        self.launch(cmd, Parser::Codex(CodexParser::default()), tag, resumed)?;
        // The prompt, then EOF: Codex starts once stdin closes.
        let r = self.running.as_mut().unwrap();
        let mut stdin = r.stdin.take().unwrap();
        if let Err(e) = stdin.write_all(prompt.as_bytes()).and_then(|_| stdin.flush()) {
            self.end_process();
            return Err(anyhow!("could not talk to Codex: {e}"));
        }
        Ok(())
    }

    /// Starts `cmd` with piped stdio and a reader thread feeding a fresh inbox.
    fn launch(&mut self, mut cmd: std::process::Command, mut parser: Parser, tag: String, resumed: bool) -> anyhow::Result<()> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| format!("could not start {}", self.app_name()))?;
        let group = crate::platform::ProcessGroup::of(&child);
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let err_text: Arc<Mutex<Option<String>>> = Arc::default();
        let err_out = err_text.clone();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            *err_out.lock().unwrap() = Some(s);
        });
        let inbox: Arc<Mutex<Inbox>> = Arc::default();
        let reader_inbox = inbox.clone();
        let reader = std::thread::Builder::new().name("assistant-reader".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let events = parser.handle(&v);
                let mut inbox = reader_inbox.lock().unwrap();
                if let Some(id) = parser.session(&v) {
                    inbox.session = Some(id);
                }
                inbox.started |= parser.starts(&v);
                inbox.events.extend(events);
            }
            // stderr closes along with stdout; give its reader a moment to finish.
            let mut stderr = None;
            for _ in 0..25 {
                stderr = err_text.lock().unwrap().take();
                if stderr.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let mut inbox = reader_inbox.lock().unwrap();
            inbox.stderr = stderr.unwrap_or_default();
            inbox.exited = true;
        });
        if let Err(e) = reader {
            group.kill(&mut child);
            return Err(e.into());
        }
        self.running = Some(Running { child, group, stdin: Some(stdin), inbox, tag, resumed });
        Ok(())
    }

    /// Sends a message. `context` describes the editor state in one line; `skills` is the user's
    /// enabled skills, already joined into one block (empty if none).
    pub fn send(&mut self, text: &str, context: &str, cwd: &Path, skills: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!self.busy, "The assistant is still working on the last message");
        let (Some(provider), Some(exe)) = (self.provider, self.exe.clone()) else {
            anyhow::bail!("Connect an AI first");
        };
        let content = format!("<editor>{context}</editor>\n{text}");
        match provider {
            AiProvider::ClaudeCode => {
                // A process that ended between replies is replaced; the conversation resumes.
                if self.running.as_mut().is_some_and(|r| !matches!(r.child.try_wait(), Ok(None))) {
                    self.end_process();
                }
                if self.running.is_none() {
                    self.spawn_claude(&exe, cwd, skills)?;
                }
                let msg = json!({ "type": "user", "message": { "role": "user", "content": content } });
                let r = self.running.as_mut().unwrap();
                let stdin = r.stdin.as_mut().expect("Claude Code keeps its stdin");
                if let Err(e) = writeln!(stdin, "{msg}").and_then(|_| stdin.flush()) {
                    self.end_process();
                    return Err(anyhow!("could not talk to Claude Code: {e}"));
                }
            }
            AiProvider::Codex => {
                // One process per reply. A new conversation carries the instructions in its first message.
                self.end_process();
                let prompt = if self.session.is_none() { format!("<instructions>\n{}\n</instructions>\n\n{content}", instructions(skills)) } else { content };
                self.spawn_codex(&exe, cwd, &prompt)?;
            }
        }
        self.busy = true;
        Ok(())
    }

    /// Ends the process now, keeping its conversation id so the conversation can resume.
    fn end_process(&mut self) {
        if let Some(r) = self.running.take() {
            if let Some(id) = r.inbox.lock().unwrap().session.take() {
                self.session = Some(id);
            }
        }
    }

    /// Stops the current reply at once: the process and everything it started end, and
    /// whatever it was still saying is dropped. The conversation continues with the next
    /// message. Returns true if a reply was in progress.
    pub fn stop(&mut self) -> bool {
        self.end_process();
        std::mem::take(&mut self.busy)
    }

    /// Starts a fresh conversation.
    pub fn reset(&mut self) {
        self.stop();
        self.session = None;
    }

    /// Takes what the assistant said since the last poll. A reply always ends with exactly
    /// one `TurnDone`, also when the app quits in the middle of it.
    pub fn poll(&mut self) -> Vec<Event> {
        let Some(r) = &self.running else { return Vec::new() };
        let resumed = r.resumed;
        let (mut events, exited, started, stderr) = {
            let mut inbox = r.inbox.lock().unwrap();
            if let Some(id) = inbox.session.take() {
                self.session = Some(id);
            }
            (std::mem::take(&mut inbox.events), inbox.exited, inbox.started, std::mem::take(&mut inbox.stderr))
        };
        if events.iter().any(|e| matches!(e, Event::TurnDone { .. })) {
            self.busy = false;
        }
        if exited {
            let name = self.app_name();
            self.end_process();
            if std::mem::take(&mut self.busy) {
                let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
                let mut msg = if lines.is_empty() { format!("{name} stopped unexpectedly.") } else { lines[lines.len().saturating_sub(6)..].join("\n") };
                // It couldn't even open the saved conversation: start a new one next time
                // instead of failing the same way on every message.
                if resumed && !started {
                    self.session = None;
                    msg.push_str("\nYour next message starts a new conversation.");
                }
                events.push(Event::TurnDone { error: Some(msg) });
            }
        }
        events
    }
}

/// Turns `codex exec --json` lines into chat events.
#[derive(Default)]
pub struct CodexParser {
    /// Tool items already announced (some arrive only as completed).
    started: std::collections::HashSet<String>,
    /// The reply has ended (only the first ending counts).
    done: bool,
}

impl CodexParser {
    pub fn handle(&mut self, v: &Value) -> Vec<Event> {
        let mut out = Vec::new();
        let kind = v["type"].as_str().unwrap_or("");
        match kind {
            "item.started" | "item.completed" => {
                let item = &v["item"];
                let id = item["id"].as_str().unwrap_or("").to_string();
                let completed = kind == "item.completed";
                let (name, input) = match item["type"].as_str().unwrap_or("") {
                    "agent_message" => {
                        if let Some(t) = item["text"].as_str().filter(|t| completed && !t.trim().is_empty()) {
                            out.push(Event::Text(t.to_string()));
                        }
                        return out;
                    }
                    "mcp_tool_call" => {
                        // Arguments come as an object, or as JSON in a string.
                        let args = match &item["arguments"] {
                            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                            a => a.clone(),
                        };
                        (format!("mcp__{}__{}", item["server"].as_str().unwrap_or(""), item["tool"].as_str().unwrap_or("")), args)
                    }
                    "command_execution" => ("Shell".to_string(), json!({ "command": item["command"] })),
                    "web_search" => ("WebSearch".to_string(), json!({ "query": item["query"] })),
                    _ => return out,
                };
                if self.started.insert(id.clone()) {
                    out.push(Event::ToolStart { id: id.clone(), name, input });
                }
                if completed {
                    out.push(codex_tool_done(id, item));
                }
            }
            "turn.completed" if !self.done => {
                self.done = true;
                out.push(Event::TurnDone { error: None });
            }
            "turn.failed" if !self.done => {
                self.done = true;
                let msg = v["error"]["message"].as_str().unwrap_or("Codex couldn't finish the reply.");
                out.push(Event::TurnDone { error: Some(msg.to_string()) });
            }
            // Reconnect notices are only progress; anything else ends the reply.
            "error" if !self.done => {
                let msg = v["message"].as_str().unwrap_or("Codex reported an error.");
                if !msg.starts_with("Reconnecting") {
                    self.done = true;
                    out.push(Event::TurnDone { error: Some(msg.to_string()) });
                }
            }
            _ => {}
        }
        out
    }
}

/// The end of a Codex tool item: whether it worked, its first text, and an image if it returned one.
fn codex_tool_done(id: String, item: &Value) -> Event {
    let failed = item["status"] == "failed" || !item["error"].is_null() || item["exit_code"].as_i64().is_some_and(|c| c != 0);
    let mut summary = item["error"]["message"].as_str().map(str::to_string).unwrap_or_default();
    let mut image = None;
    for block in item["result"]["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") if summary.is_empty() => summary = block["text"].as_str().unwrap_or("").to_string(),
            Some("image") => {
                use base64::Engine;
                image = block["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
            }
            _ => {}
        }
    }
    if summary.is_empty() {
        summary = item["aggregated_output"].as_str().unwrap_or("").chars().take(400).collect();
    }
    Event::ToolDone { id, ok: !failed, summary, image }
}

/// Turns Claude Code's stream-json lines into chat events.
#[derive(Default)]
pub struct StreamParser {
    streamed: std::collections::HashSet<String>,
    current_msg: Option<String>,
}

impl StreamParser {
    pub fn handle(&mut self, v: &Value) -> Vec<Event> {
        let mut out = Vec::new();
        match v["type"].as_str().unwrap_or("") {
            "stream_event" => {
                let ev = &v["event"];
                match ev["type"].as_str().unwrap_or("") {
                    "message_start" => self.current_msg = ev["message"]["id"].as_str().map(str::to_string),
                    "content_block_delta" if ev["delta"]["type"] == "text_delta" => {
                        if let Some(id) = &self.current_msg {
                            self.streamed.insert(id.clone());
                        }
                        if let Some(t) = ev["delta"]["text"].as_str() {
                            out.push(Event::TextDelta(t.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                let msg_id = v["message"]["id"].as_str().unwrap_or("").to_string();
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") if !self.streamed.contains(&msg_id) => {
                            if let Some(t) = block["text"].as_str() {
                                out.push(Event::Text(t.to_string()));
                            }
                        }
                        Some("tool_use") => out.push(Event::ToolStart {
                            id: block["id"].as_str().unwrap_or("").to_string(),
                            name: block["name"].as_str().unwrap_or("").to_string(),
                            input: block["input"].clone(),
                        }),
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let id = block["tool_use_id"].as_str().unwrap_or("").to_string();
                    let ok = !block["is_error"].as_bool().unwrap_or(false);
                    let (mut summary, mut image) = (String::new(), None);
                    match &block["content"] {
                        Value::String(s) => summary = s.clone(),
                        Value::Array(items) => {
                            for it in items {
                                match it["type"].as_str() {
                                    Some("text") if summary.is_empty() => summary = it["text"].as_str().unwrap_or("").to_string(),
                                    Some("image") => {
                                        use base64::Engine;
                                        image = it["source"]["data"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                    out.push(Event::ToolDone { id, ok, summary, image });
                }
            }
            "result" => {
                let failed = v["is_error"].as_bool().unwrap_or(false) || v["subtype"].as_str().is_some_and(|s| s != "success");
                let error = failed.then(|| v["result"].as_str().map(str::to_string).unwrap_or_else(|| format!("Claude Code reported an error ({})", v["subtype"].as_str().unwrap_or("unknown"))));
                out.push(Event::TurnDone { error });
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_recorded_turn() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"mcp__ai-video-editor__add_zoom","input":{"start":3}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"Added zoom 7"}]}]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Zoomed in"}}}"#,
            r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Zoomed in"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Zoomed in"}"#,
        ];
        let mut p = StreamParser::default();
        let events: Vec<Event> = lines.iter().flat_map(|l| p.handle(&serde_json::from_str(l).unwrap())).collect();
        assert!(!events.iter().any(|e| matches!(e, Event::Text(_))), "streamed text must not repeat");
        assert!(events.iter().any(|e| matches!(e, Event::ToolStart { name, .. } if name.ends_with("add_zoom"))));
        assert!(events.iter().any(|e| matches!(e, Event::ToolDone { ok: true, summary, .. } if summary == "Added zoom 7")));
        assert!(matches!(events.last(), Some(Event::TurnDone { error: None })));
        assert_eq!(describe_tool("mcp__ai-video-editor__add_zoom", &json!({ "start": 3.0 })), "Zooming in at 3.0s");
    }

    /// A stand-in for Claude Code that does what `mode.txt` (in its working folder) says for each
    /// message. "slow" hands the work to a child process, as `claude.cmd` does with node.
    fn fake_claude(dir: &Path) -> PathBuf {
        let init = r#"{"type":"system","subtype":"init","session_id":"s1"}"#;
        let tool = r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"mcp__ai-video-editor__add_zoom","input":{"start":3}}]}}"#;
        let result = r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#;
        #[cfg(windows)]
        let (main, slow, file) = (
            format!(
                "@echo off\necho %* > args.txt\n:loop\nset LINE=\nset /p LINE=\nif not defined LINE exit /b 0\nset /p MODE=<mode.txt\n\
                 if \"%MODE%\"==\"noresume\" (\n  echo No conversation found 1>&2\n  exit /b 1\n)\necho {init}\n\
                 if \"%MODE%\"==\"slow\" cmd /C \"%~dp0slow.cmd\"\nif \"%MODE%\"==\"crash\" (\n  echo boom 1>&2\n  exit /b 1\n)\necho {result}\ngoto loop\n"
            ),
            format!("@echo off\necho {tool}\nping -n 4 127.0.0.1 >nul\necho late> marker.txt\necho {result}\n"),
            "claude.cmd",
        );
        #[cfg(not(windows))]
        let (main, slow, file) = (
            format!(
                "#!/bin/sh\necho \"$@\" > args.txt\nwhile IFS= read -r line; do\n  mode=$(cat mode.txt)\n\
                 if [ \"$mode\" = noresume ]; then echo 'No conversation found' >&2; exit 1; fi\n  echo '{init}'\n\
                 if [ \"$mode\" = slow ]; then sh ./slow.sh; fi\n  if [ \"$mode\" = crash ]; then echo boom >&2; exit 1; fi\n  echo '{result}'\ndone\n"
            ),
            format!("#!/bin/sh\necho '{tool}'\nsleep 3\necho late > marker.txt\necho '{result}'\n"),
            "claude",
        );
        let crlf = |s: String| if cfg!(windows) { s.replace('\n', "\r\n") } else { s };
        std::fs::write(dir.join(if cfg!(windows) { "slow.cmd" } else { "slow.sh" }), crlf(slow)).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, crlf(main)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// Polls until `done` holds for everything received, or fails after 15 s.
    fn wait(agent: &mut Agent, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let (mut all, t) = (Vec::new(), std::time::Instant::now());
        while !done(&all) {
            assert!(t.elapsed().as_secs() < 15, "timed out; got {all:?}");
            all.extend(agent.poll());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        all
    }

    fn turn_done(events: &[Event]) -> Vec<&Option<String>> {
        events.iter().filter_map(|e| if let Event::TurnDone { error } = e { Some(error) } else { None }).collect()
    }

    #[test]
    fn stop_send_again_and_crashes() {
        let dir = std::env::temp_dir().join(format!("studio-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mode = |m: &str| std::fs::write(dir.join("mode.txt"), m).unwrap();
        let mut agent = Agent { provider: Some(AiProvider::ClaudeCode), exe: Some(fake_claude(&dir)), ..Agent::default() };

        // Stopped in the middle of a tool call: the whole process tree ends at once.
        mode("slow");
        agent.send("first", "", &dir, "").unwrap();
        assert!(agent.send("again", "", &dir, "").is_err(), "no second message while one is running");
        wait(&mut agent, |e| e.iter().any(|e| matches!(e, Event::ToolStart { .. })));
        let first_run = agent.running.as_ref().unwrap().tag.clone();
        assert!(agent.accepts(&first_run));
        assert!(agent.stop());
        assert!(!agent.busy && !agent.accepts(&first_run), "tool calls from the stopped reply are refused");
        assert!(agent.poll().is_empty(), "a stopped reply says nothing more");
        assert!(!agent.stop(), "stopping again is a no-op");

        // The next message gets a clean reply: no stale error, the conversation resumes.
        mode("fast");
        agent.send("second", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert_eq!(turn_done(&events), [&None], "exactly one clean end: {events:?}");
        assert!(!agent.busy);
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        assert!(args.contains("--resume") && args.contains("s1"), "resumed the conversation: {args}");
        let second_run = agent.running.as_ref().unwrap().tag.clone();
        assert_ne!(first_run, second_run);
        assert!(!agent.accepts(&second_run), "an idle assistant makes no calls");

        // Claude Code dies in the middle of a reply: one error with what it printed, then it recovers.
        mode("crash");
        agent.send("third", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert!(matches!(turn_done(&events)[..], [Some(e)] if e.contains("boom")), "{events:?}");
        assert!(!agent.busy && agent.running.is_none() && agent.session.is_some());

        // The saved conversation can't be opened: the next message starts a new one.
        mode("noresume");
        agent.send("fourth", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert!(matches!(turn_done(&events)[..], [Some(e)] if e.contains("new conversation")), "{events:?}");
        assert!(agent.session.is_none());
        mode("fast");
        agent.send("fifth", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert_eq!(turn_done(&events), [&None]);
        assert!(!std::fs::read_to_string(dir.join("args.txt")).unwrap().contains("--resume"));

        // The work the stopped reply handed to its child process never finished.
        std::thread::sleep(std::time::Duration::from_millis(4500));
        assert!(!dir.join("marker.txt").exists(), "the stopped reply kept running in the background");
        drop(agent);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn parses_a_recorded_codex_reply() {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        let lines = [
            r#"{"type":"thread.started","thread_id":"0199a213-81c0"}"#.to_string(),
            r#"{"type":"turn.started"}"#.to_string(),
            r#"{"type":"error","message":"Reconnecting... 1/5"}"#.to_string(),
            r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"ai-video-editor","tool":"add_zoom","arguments":{"start":3},"result":null,"error":null,"status":"in_progress"}}"#.to_string(),
            r#"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"ai-video-editor","tool":"add_zoom","arguments":{"start":3},"result":{"content":[{"type":"text","text":"Added zoom 7"}]},"error":null,"status":"completed"}}"#.to_string(),
            // Arguments as a JSON string, and only a completed line: still a card, with its picture.
            format!(r#"{{"type":"item.completed","item":{{"id":"item_2","type":"mcp_tool_call","server":"ai-video-editor","tool":"get_frame","arguments":"{{\"time\":2}}","result":{{"content":[{{"type":"image","data":"{png}","mimeType":"image/jpeg"}}]}},"error":null,"status":"completed"}}}}"#),
            r#"{"type":"item.completed","item":{"id":"item_3","type":"mcp_tool_call","server":"ai-video-editor","tool":"move_clip","arguments":{},"result":null,"error":{"message":"That clip no longer exists"},"status":"failed"}}"#.to_string(),
            r#"{"type":"item.completed","item":{"id":"item_4","type":"reasoning","text":"thinking"}}"#.to_string(),
            r#"{"type":"item.completed","item":{"id":"item_5","type":"agent_message","text":"Zoomed in on the dashboard."}}"#.to_string(),
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5}}"#.to_string(),
        ];
        let mut p = CodexParser::default();
        let events: Vec<Event> = lines.iter().flat_map(|l| p.handle(&serde_json::from_str(l).unwrap())).collect();
        assert!(matches!(&events[0], Event::ToolStart { id, name, input } if id == "item_1" && name == "mcp__ai-video-editor__add_zoom" && input["start"] == 3));
        assert!(matches!(&events[1], Event::ToolDone { ok: true, summary, .. } if summary == "Added zoom 7"));
        assert!(matches!(&events[2], Event::ToolStart { name, input, .. } if name.ends_with("get_frame") && input["time"] == 2));
        assert!(matches!(&events[3], Event::ToolDone { ok: true, image: Some(i), .. } if i == &[1u8, 2, 3]));
        assert!(matches!(&events[5], Event::ToolDone { ok: false, summary, .. } if summary == "That clip no longer exists"));
        assert!(matches!(&events[6], Event::Text(t) if t == "Zoomed in on the dashboard."));
        assert!(matches!(events.last(), Some(Event::TurnDone { error: None })));
        assert_eq!(events.iter().filter(|e| matches!(e, Event::TurnDone { .. })).count(), 1, "reconnect notices don't end the reply");
        assert_eq!(describe_tool("mcp__ai-video-editor__add_zoom", &json!({ "start": 3.0 })), "Zooming in at 3.0s");

        // A failed turn ends with its message, once.
        let mut p = CodexParser::default();
        let failed: Vec<Event> = [r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#, r#"{"type":"error","message":"stream closed"}"#]
            .iter()
            .flat_map(|l| p.handle(&serde_json::from_str(l).unwrap()))
            .collect();
        assert!(matches!(failed.as_slice(), [Event::TurnDone { error: Some(m) }] if m.contains("usage limit")));
    }

    /// A stand-in for `codex exec --json`: saves its arguments and the prompt from stdin,
    /// then answers as `mode.txt` says. "slow" hands the work to a child process, as npm's
    /// `codex.cmd` does with node.
    fn fake_codex(dir: &Path) -> PathBuf {
        let thread = r#"{"type":"thread.started","thread_id":"t-42"}"#;
        let tool_start = r#"{"type":"item.started","item":{"id":"i1","type":"mcp_tool_call","server":"ai-video-editor","tool":"add_text","arguments":{"text":"Hi"},"status":"in_progress"}}"#;
        let tool_done = r#"{"type":"item.completed","item":{"id":"i1","type":"mcp_tool_call","server":"ai-video-editor","tool":"add_text","arguments":{"text":"Hi"},"result":{"content":[{"type":"text","text":"Added text clip 9"}]},"error":null,"status":"completed"}}"#;
        let reply = r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"Added a title."}}"#;
        let done = r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let failed = r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#;
        #[cfg(windows)]
        let (main, slow, file) = (
            format!(
                "@echo off\necho %* > args.txt\nmore > prompt.txt\nset /p MODE=<mode.txt\necho {thread}\n\
                 if \"%MODE%\"==\"slow\" cmd /C \"%~dp0slow.cmd\"\nif \"%MODE%\"==\"fail\" (\n  echo {failed}\n  exit /b 1\n)\n\
                 echo {tool_start}\necho {tool_done}\necho {reply}\necho {done}\n"
            ),
            format!("@echo off\necho {tool_start}\nping -n 4 127.0.0.1 >nul\necho late> marker.txt\n"),
            "codex.cmd",
        );
        #[cfg(not(windows))]
        let (main, slow, file) = (
            format!(
                "#!/bin/sh\necho \"$@\" > args.txt\ncat > prompt.txt\nmode=$(cat mode.txt)\necho '{thread}'\n\
                 if [ \"$mode\" = slow ]; then sh ./slow.sh; fi\nif [ \"$mode\" = fail ]; then echo '{failed}'; exit 1; fi\n\
                 echo '{tool_start}'\necho '{tool_done}'\necho '{reply}'\necho '{done}'\n"
            ),
            format!("#!/bin/sh\necho '{tool_start}'\nsleep 3\necho late > marker.txt\n"),
            "codex",
        );
        let crlf = |s: String| if cfg!(windows) { s.replace('\n', "\r\n") } else { s };
        std::fs::write(dir.join(if cfg!(windows) { "slow.cmd" } else { "slow.sh" }), crlf(slow)).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, crlf(main)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn codex_replies_continue_the_thread_and_stop_cleanly() {
        let dir = std::env::temp_dir().join(format!("studio-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mode = |m: &str| std::fs::write(dir.join("mode.txt"), m).unwrap();
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        let mut agent = Agent { provider: Some(AiProvider::Codex), exe: Some(fake_codex(&dir)), ..Agent::default() };

        // First message: a new thread, with the instructions in the prompt read from stdin.
        mode("ok");
        agent.send("Add a title", "playhead 0.00s", &dir, "").unwrap();
        let tag = agent.running.as_ref().unwrap().tag.clone();
        assert!(agent.accepts(&tag));
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert_eq!(turn_done(&events), [&None], "{events:?}");
        assert!(events.iter().any(|e| matches!(e, Event::ToolStart { name, .. } if name == "mcp__ai-video-editor__add_text")));
        assert!(events.iter().any(|e| matches!(e, Event::ToolDone { ok: true, summary, .. } if summary == "Added text clip 9")));
        assert!(events.iter().any(|e| matches!(e, Event::Text(t) if t == "Added a title.")));
        let (args, prompt) = (read("args.txt"), read("prompt.txt"));
        assert!(args.contains("exec") && args.contains("--json") && args.contains("read-only") && !args.contains("resume"), "{args}");
        assert!(args.contains(&format!("?run={tag}")), "the run tag reaches the MCP url: {args}");
        assert!(prompt.contains("<instructions>") && prompt.contains("motion designer") && prompt.contains("<editor>playhead 0.00s</editor>") && prompt.contains("Add a title"), "{prompt}");
        assert!(!agent.busy);

        // The next message continues the same thread, without repeating the instructions.
        agent.send("Make it bigger", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert_eq!(turn_done(&events), [&None]);
        let (args, prompt) = (read("args.txt"), read("prompt.txt"));
        assert!(args.contains("resume t-42"), "{args}");
        assert!(!prompt.contains("<instructions>") && prompt.contains("Make it bigger"), "{prompt}");

        // A failed reply shows Codex's message; the next one still works.
        mode("fail");
        agent.send("Again", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert!(matches!(turn_done(&events)[..], [Some(m)] if m.contains("usage limit")), "{events:?}");

        // Stopped while a tool runs: the whole process tree ends, and late calls are refused.
        mode("slow");
        agent.send("Slow one", "", &dir, "").unwrap();
        wait(&mut agent, |e| e.iter().any(|e| matches!(e, Event::ToolStart { .. })));
        let tag = agent.running.as_ref().unwrap().tag.clone();
        assert!(agent.stop());
        assert!(!agent.accepts(&tag) && agent.poll().is_empty());
        mode("ok");
        agent.send("After stop", "", &dir, "").unwrap();
        let events = wait(&mut agent, |e| !turn_done(e).is_empty());
        assert_eq!(turn_done(&events), [&None], "no stale error after Stop: {events:?}");
        std::thread::sleep(std::time::Duration::from_millis(4000));
        assert!(!dir.join("marker.txt").exists(), "the stopped reply kept running in the background");

        // Switching providers starts a new conversation.
        agent.set_provider(Some(AiProvider::ClaudeCode));
        assert!(agent.session.is_none());
        drop(agent);
        std::fs::remove_dir_all(dir).ok();
    }
}
