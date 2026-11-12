# AI Video Studio

A motion graphics editor for product showcases — Apple-style launch videos and SaaS explainers
made from screen recordings. Everything works by hand, and an optional AI assistant (your own
Claude Code) can edit alongside you: every change appears live and can be undone.

The app lives in `studio/` (binary `ai-video-studio`, built with Iced + wgpu).

## Download

One file for your computer, nothing else to install. Both links always give the newest version.

**Windows 10 or 11**: [Download for Windows](https://github.com/molla-rakib/ai-video-editor/releases/latest/download/AI-Video-Studio-Windows-Setup.exe)
(`AI-Video-Studio-Windows-Setup.exe`)

Open it and follow the installer. It installs the app with everything it needs (FFmpeg included), a
Start menu shortcut and an uninstaller, and makes `.aivp` projects open with a double-click. No
administrator rights needed. The installer isn't code-signed yet, so Windows may say "Windows
protected your PC": click **More info**, then **Run anyway** (see [Code signing](#code-signing)).

**Linux: Ubuntu, Debian, Linux Mint, Pop!_OS** (64-bit PC): [Download for Linux](https://github.com/molla-rakib/ai-video-editor/releases/latest/download/AI-Video-Studio-Linux.deb)
(`AI-Video-Studio-Linux.deb`)

Double-click it and click **Install**: that also installs FFmpeg, adds the app to your app menu
and makes `.aivp` projects open with a double-click. If no installer opens, run this in a
terminal in your Downloads folder:
```sh
sudo apt install ./AI-Video-Studio-Linux.deb
```

To update, install the new version the same way; projects and settings stay. Every version is on
the [Releases page](https://github.com/molla-rakib/ai-video-editor/releases); you don't need its
**Source code** files, GitHub adds those to every release for developers.

To build it yourself instead, follow Setup below.

---

## Setup

You need **Rust**, **FFmpeg** (ffmpeg + ffprobe) and, for the assistant, **Claude Code**.
The same code builds on Windows and Linux.

### Windows

1. **Rust** (GNU toolchain — works without Visual Studio):
   ```powershell
   rustup-init.exe --default-host x86_64-pc-windows-gnu
   ```
2. **MinGW-w64** (the linker the GNU toolchain uses):
   ```powershell
   winget install BrechtSanders.WinLibs.POSIX.UCRT
   ```
   Open a new terminal afterwards so `gcc` is on `PATH`.
3. **FFmpeg**: download a `win64-gpl` build from https://github.com/BtbN/FFmpeg-Builds/releases
   and copy `ffmpeg.exe` and `ffprobe.exe` into `vendor\ffmpeg\` in this repo
   (`vendor\` is git-ignored). Putting them on `PATH` works too.
4. **Run**:
   ```powershell
   cargo run -p studio --release
   ```
   The first build takes several minutes; later builds are quick.

On Windows the window gets native rounded corners and shadow, and drag and drop from Explorer
works out of the box.

### Linux (tested on Ubuntu 24.04)

1. **Rust**:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source ~/.cargo/env        # or open a new terminal
   ```
2. **System packages** (ALSA headers for audio, FFmpeg 6.x or 7.x):
   ```sh
   sudo apt install build-essential pkg-config libasound2-dev ffmpeg
   ```
3. **Run**:
   ```sh
   cargo run -p studio --release
   ```

Notes for Linux:
- **Drag and drop**: the window library (winit) can't receive dropped files on Wayland, so on a
  Wayland desktop the app runs through XWayland automatically, where drag and drop works. If text
  looks slightly soft on a scaled display, `STUDIO_WAYLAND=1 cargo run -p studio --release` runs
  natively on Wayland (without drag and drop; use the **+** button to import instead).
- If you see `cargo: command not found`, run `source ~/.cargo/env` or open a new terminal.

### The AI assistant (optional, both platforms)

1. Install **Claude Code** from https://claude.com/claude-code and run `claude` once in a
   terminal to sign in. No API key is needed — it uses your Claude subscription.
2. In the app, click **Connect AI** in the title bar → **Connect** next to Claude Code.
   The chip then shows **Claude Code** with a lime dot.

**Or use ChatGPT** (your ChatGPT plan, through OpenAI's Codex app):

1. Install Node.js, then Codex: `npm install -g @openai/codex`.
2. Click **Connect AI** → **Connect** next to ChatGPT, then **Sign in with ChatGPT**: the sign-in page
   opens in your browser, and the dialog updates by itself when you're done.

The app finds Claude Code and Codex in `~/.local/bin`, `~/.codex/bin`, on `PATH`, or (Windows) in
`%APPDATA%\npm`. Nothing is sent to any AI until you connect one. Other MCP apps on the same computer
(Claude Desktop, Cursor, VS Code…) can edit the open project too: the dialog shows the server address,
and **Copy setup** copies the JSON they accept.

### Run options

```sh
cargo run -p studio --release -- path/to/project.aivp   # open a project directly
cargo test -p studio                                    # tests (need FFmpeg; some use the GPU)
```

| Variable | Effect |
|---|---|
| `AIVE_FFMPEG_DIR` | Folder containing ffmpeg/ffprobe (overrides the search) |
| `AIVE_STUDIO_MCP_PORT` | Port for the assistant/MCP server (default 47822; the next free one is used if busy) |
| `STUDIO_WAYLAND=1` | Linux: stay on native Wayland instead of XWayland |
| `AIVE_RECORD_FAKE=1` | Record camera uses a test pattern and tone instead of real devices (for testing) |

FFmpeg is searched in this order: `AIVE_FFMPEG_DIR`, `ffmpeg\` next to the app, next to the
app, `vendor/ffmpeg/` in the repo, then `PATH`.

### Where things are stored

| What | Windows | Linux |
|---|---|---|
| Settings (recent projects, layout, connected AI, preferences) | `%APPDATA%\AIVideoEditor\studio.json` | `~/.config/ai-video-editor/studio.json` |
| Built-in sound effects (made on first use) | `%APPDATA%\AIVideoEditor\sounds\` | `~/.config/ai-video-editor/sounds/` |
| New projects (`.aivp`) | `Videos\AI Video Editor\` | `~/Videos/AI Video Editor/` |
| Camera recordings | `Recordings\` next to the project file (in the projects folder for a project never saved) | same |

Projects save automatically a couple of seconds after each change.

---

## What you can do

- **Start screen**: pick a format (16:9, 9:16, 1:1, 4:5, 21:9 · 720p–4K · 24–60 fps) or reopen a
  recent project. Drop media on it to start a project from that file. **Try an example project**
  opens a short pre-built scene (text, a kinetic reveal, a glowing accent and a marker) to explore
  and pull apart instead of starting from a blank timeline.
- **Import** with the **+** button, `Ctrl+I`, or by dragging files into the window. Thumbnails,
  filmstrips and waveforms load in the background. Missing files show a red badge — right-click →
  **Locate file…** to relink.
- **Record camera** (`Ctrl+R`, or **Record** in the title bar): pick a camera and microphone, see
  yourself live with a microphone level meter, turn the picture (an upright 9:16 video from a camera
  on its side) or mirror it, then a 3-second countdown and record. **Stop** puts the clip on the
  timeline at the playhead; **Discard** throws the take away. The camera's best mode is chosen for
  you (on Linux, e.g. compressed 720p at 30 fps rather than raw at 10); infrared face-login cameras
  aren't offered. Recordings are written to disk every second, so a camera that is unplugged or a
  crash keeps all but the last moment; anything left unfinished is recovered into the library the
  next time Record opens. Quitting or switching projects waits until you stop.
- **Timeline**: as many video and audio tracks as you like (right-click a track header to rename,
  add, move, lock, hide/mute or delete; drag a header's bottom edge to resize the track; drag the
  gap above the timeline to resize the whole area; it starts compact). Move, trim and snap clips;
  drag a clip above the top track to put it on a new track over everything; drag media in from the
  library. **Right-click a clip** (on the timeline or the preview) for split, duplicate, **separate
  audio** (the sound moves to its own audio track), mute, zoom, crop, **bring to front / send to back**,
  **copy color** (its color grade) and **paste color** (onto the selected clip(s), as one undo step),
  range volume and delete. On the preview, a selected element keeps the drag even where another
  element lies over it.
- **Select many clips**: `Ctrl+A` selects all, dragging over empty timeline space draws a selection
  box (Shift or Ctrl adds to the selection), `Shift+A` selects everything after the playhead.
  Dragging one selected clip moves all of them, keeping their layout.
- **Cut, copy, paste** (`Ctrl+X` / `Ctrl+C` / `Ctrl+V`, pasted at the playhead on the same tracks
  when they are free) and **Paste effects** (`Ctrl+Shift+V`: the copied clip's frame, color,
  animations and sound settings onto the selection).
- **Group** (`Ctrl+G`) clips so they move, trim, copy and delete together; **Ungroup** with
  `Ctrl+Shift+G`. A video and its separated audio are grouped automatically. Grouped clips show a
  small link badge, and selecting one outlines the rest of its group.
- **Remove space in one click**: **Close gaps** (`G`, or the toolbar button) removes all time where
  nothing plays, keeping every track in sync; with clips selected it closes the gaps on their
  tracks. Right-click an empty spot for **Close this gap**. **Ripple delete** (`Shift+Del`) deletes
  the selection and pulls what follows left; with a range marked it cuts that time out of every track.
- **Transitions in one step**: `Ctrl+T` (or right-click a clip → **Add transition**) crossfades at
  the cut before the selected clip, or at the cut nearest the playhead. Nine kinds, including
  slide right/down and dip to black, in the Effects tab and the inspector.
- **Freeze frame** (`Shift+F`): holds the frame under the playhead for 2 seconds (drag its edge
  for longer). **Reverse** a video or audio clip, **Replace media** keeping the clip's edits, and
  **Disable** a clip (`Shift+E`) to keep it on the timeline without showing or playing it; all
  from the right-click menu.
- Clips glide to their new place after ripple edits, closing gaps and undo, and timeline zoom eases.
- **Split at the playhead** (`S`). **Select a range** for one job (range volume, a marker over it, or
  telling the assistant where): click **Range** (`R`), then press on the timeline and drag across the part
  you want; the clip you press on gets selected. Drag the range's edges to adjust it, or drag again for
  a new one. Click **✓ Done** (`Enter`, `R` or `Esc`) when the job is done: the range goes away.
  Shift+drag on the ruler selects a range without the button.
- **Markers**: colored flags on the ruler (`Shift+M`, or the **Marker** button) — a span when a
  range is marked. Click one to jump to it and rename or recolor it in the inspector; drag to move.
- **Zoom & pan, Camtasia-style**: put the playhead where the zoom should start and press `Z`.
  Zooms appear as bands on the clip — drag to move, drag the ends to resize, click to aim the
  focus rectangle on the canvas. Pressing `Z` again right after a zoom adds a smooth **pan**. The
  Animation tab also has one-click **Push in / Pull back** camera presets that zoom from the
  playhead to the end of the clip.
- **Motion polish**: entrances can be **Bouncy** (a slight overshoot) in the inspector; exits
  default to 60% of the entrance's duration. A **press pulse** (`P`) gives a clip a quick, one-shot
  scale dip, like a button being clicked. **Idle drift** (in Layer) keeps a still moment subtly
  alive with a slow breathing scale. Select several clips and use **Stagger entrances** (right-click
  menu) to offset their intros a few frames apart instead of starting all at once.
- **Safe zones**: toggle 9:16 / 1:1 crop guides over a 16:9 preview (toolbar button) to check a
  shot will still read once it's re-cropped for social cutdowns.
- **Preview**: GPU-rendered, with audio. Click layers to select them; drag to move (snaps to the
  center), corners to scale, the knob to rotate, the inner dot to round the corners; shapes and
  markers resize from every side (Shift on a corner keeps the proportions). Handles work past the
  canvas edge. **Zoom the preview** from the toolbar (Fit, 25–400%), `Ctrl+0` / `Ctrl+1`, or
  Ctrl + wheel; the wheel or the middle button pans a zoomed preview. This never changes the video size.
- **Effects add, the Inspector adjusts.** The **Effects** tab is where things are added: markers (box,
  circle, highlight, underline in a color you pick), **blur** and **mask** areas, frame styles (corners,
  border, shadow) and transitions; a separate **Animation** tab (same layout) holds zoom & pan and the
  animate-in/out motion presets, so entrances/exits/camera moves don't crowd the Effects list. Click an effect to apply it to the
  selected clips, or drag it onto any clip on the timeline or the preview. Groups start collapsed; the ☆ on a
  card keeps it in **Favorites**, always open at the top (each tab favorites its own effects). Markers, blurs and masks go on the timeline at the
  playhead for 3 s (or over the selected range); a blur only blurs the video under its box, a **mask**
  shows only the video under its box (or, inverted, hides it) — drag its edges on the preview to resize it,
  and keyframe its position (same as any clip) to have it follow a moving subject by hand.
  A **transition** goes between two touching clips, not onto a clip's own body: drag it onto the
  seam where they meet (a highlight previews the real crossfade region before you drop), or apply
  it to a selected clip that already touches a previous one. It genuinely overlaps the two clips
  for its duration, Camtasia-style — drag its handle (right where the clips meet) to resize it, or
  use the Inspector's Duration slider.
- **Inspector** (search box on top; click a section title to collapse it) shows only what the
  selected clip has: rotation, opacity and framing (position and size are set on the preview; the
  section's reset button puts both back);
  **crop** (sliders, or drag the edges on the canvas); the frame styles, animations, transition and
  zooms it got from Effects, each with its settings and a ✕ to remove it; **color presets** — nine one-click
  looks (Original, Natural, Cinematic, Beautify, Vivid, Warm, Cool, Vintage, Mono), each shown on the
  clip's own picture exactly as it will render; **color correction** — light (brightness, contrast,
  shadows, midtones, highlights, fade), color (saturation, vibrance, warmth, tint, split tone) and finish
  (**smoothness** for skin, sharpness, vignette), so any preset can be fine-tuned afterwards; speed; volume and fades; **audio polish** — a 3-band EQ, a compressor and noise reduction, per
  clip; **range volume** (change or mute just the selected range
  of a clip — each changed part keeps its own slider); text (Inter, Poppins, Hind Siliguri for Bangla) —
  **select words in the text box** to give just them their own color, weight, font, size, marker
  highlight or underline (styles stay on their words while you keep typing), and a **kinetic
  reveal** toggle that fades words in one after another instead of all at once; shapes (rectangle,
  ellipse, triangle, star, hexagon, ring, frame with linear/radial gradients at any angle); a
  **counter** (from/to/duration/decimals, prefix/suffix) that counts up or down, eased, for
  before/after metrics.
- **Library tabs**: Media (grouped into videos, images and audio), Text (headline, subtitle, Bangla
  headline, shape presets, callouts, counter), Effects (above), Sounds (9 original effects — whoosh,
  pop, click, riser…).
- **Assistant**: describe what you want; it sees the playhead, the selected clip and the marked
  range. Write on several lines freely: Enter starts a new line and only the Send button sends. Replies show
  formatted (headings, lists, code with a copy button, links) with a Copy button for the whole reply. Tool calls show
  as cards, and it can look at rendered frames to check its work. **Stop** ends the reply at once, including
  anything Claude Code started, and refuses edits still on the way; your next message continues the conversation.
  The wand icon opens the **Skills manager**: reusable instruction blocks (brand colors, a house style,
  a standing request) written once and switched on or off per conversation instead of retyping them.
- **Export** to MP4 (H.264 + AAC) with progress, time left and cancel. Pick a **delivery preset** — Web,
  YouTube, Social (capped at 1080px wide for Instagram/TikTok/Reels), Archive (a master copy), or Custom for your
  own quality — each sets the right bitrate, resolution cap and loudness target; **"Also as …"** queues
  further presets to export as a batch right after the current one. With a range marked on the
  timeline, **What to export** offers just that range. The export uses the same
  renderer as the preview, and a cancelled or failed export leaves no broken file.
- **Settings** (gear in the title bar, the app menu, or `Ctrl+,`): preview quality — **Auto**
  (sharp while paused, lighter during playback when the computer can't keep up), Full, High,
  Balanced or Draft; it never affects exports — interface size (90–125%), snapping on by default,
  how long new text/shapes/images last, how soon projects autosave, starting with the preview
  muted, the default export quality, and the folder for new projects. Changes apply right away.
- **Other AI apps** (Claude Desktop, Cursor, …) can connect to the MCP server shown in the
  Connect AI dialog (`http://127.0.0.1:47822/mcp`) while the app is open.
- **Keyboard shortcuts** (keyboard icon in the title bar, the app menu, or `Ctrl+/`): every
  shortcut, grouped by file, edit, playback, timeline and view. **Click a shortcut and press new
  keys to change it**; the key is taken from whatever used it before. Reset one, or all, to the
  defaults. Tooltips and menus show your keys.
- **Unsaved changes are never discarded silently**: New, Close, Open and Quit ask first — Save,
  Don't save or Cancel — whenever the open project has edits that haven't been saved yet.

### Keyboard

| Key | Action |
|---|---|
| Space | Play / pause |
| S or B | Split at the playhead |
| R | Range tool on · Done (clears the range) |
| Z | Add a zoom (or pan) at the playhead |
| P | Add a press pulse at the playhead |
| T | Add text at the playhead |
| Delete / Backspace | Delete the selected clips (or the zoom being edited, or the selected marker) |
| Ctrl+D | Duplicate |
| Ctrl+X · Ctrl+C · Ctrl+V | Cut · copy · paste at the playhead |
| Ctrl+Shift+V | Paste effects |
| Ctrl+A · Shift+A | Select all clips · everything after the playhead |
| Ctrl+G · Ctrl+Shift+G | Group · ungroup |
| Shift+Delete or Ctrl+Backspace | Ripple delete (or cut out the marked range) |
| G | Close gaps |
| Ctrl+T | Add a transition at the cut |
| Shift+F · Shift+E | Freeze frame · disable / enable clip |
| Ctrl+] · Ctrl+[ | Bring to front · send to back |
| ← / → (Shift: 10 frames) | Step one frame |
| Home / End | Start / end |
| + / − · Shift+Z | Zoom the timeline in / out · fit |
| N · M | Snapping · mute preview |
| Shift+M | Add a marker (over the marked range, if any) |
| Ctrl+0 · Ctrl+1 · Ctrl+wheel | Fit the preview · 100% · zoom the preview |
| Ctrl+Z · Ctrl+Shift+Z or Ctrl+Y | Undo · redo |
| Ctrl+Backspace · Ctrl+Delete | In text boxes: delete the word before · after the cursor |
| Ctrl+N · Ctrl+O · Ctrl+S · Ctrl+Shift+S | New · open · save · save as |
| Ctrl+I · Ctrl+E · Ctrl+K · Ctrl+, | Import · export · assistant · settings |
| Ctrl+R | Record camera |
| Ctrl+/ | Keyboard shortcuts |
| Esc | Close a dialog, cancel a rename, leave zoom/crop editing, clear the selection |

### Building

- **Quick local test build** — just the exe, nothing else: `cargo build -p studio --release` builds
  it into `target\release\ai-video-studio.exe` (Cargo's own build cache — don't hand-edit anything
  else under `target\`). To get one clean, versioned file instead of digging through `target\`:
  - **Windows**: `powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1`
  - **Linux**: `scripts/build-release.sh`

  Either one builds release and copies just the binary to `build\ai-video-studio-v<version>.exe`
  (or `build/ai-video-studio-v<version>` on Linux) — a separate folder from Cargo's own
  `target\release\`, cleared and rewritten each time so it never accumulates old builds. This exe
  still finds FFmpeg the normal way (see `AIVE_FFMPEG_DIR` above), so it works for local testing but
  isn't meant to be copied to another machine on its own — use Packaging below for that.

### Packaging

For a standalone package to hand to someone else (bundles FFmpeg so they don't need to install
anything):

- **Windows**: `powershell -ExecutionPolicy Bypass -File scripts\package-studio.ps1 -Installer`
  builds `dist\AI-Video-Studio-Windows-Setup.exe`: one file that installs the app, FFmpeg and the
  licenses, adds a Start menu shortcut and an uninstaller, and registers `.aivp` files
  (`scripts\installer.iss`). It needs Inno Setup 6 (`winget install JRSoftware.InnoSetup`).
  Without `-Installer` it only builds the portable folder `dist\AI Video Studio\` (add `-Zip` for
  a .zip of it). Close the app running from that folder first.
- **Linux**: `scripts/package-studio.sh` builds `dist/AI-Video-Studio-Linux.deb`: one file that
  installs the app with FFmpeg and the libraries it needs (they come from the distro, through
  `Depends`), adds it to the app menu and registers `.aivp` files. It needs `dpkg-dev`
  (`sudo apt install dpkg-dev`). It also builds `dist/ai-video-studio-linux-<arch>/` and a
  `.tar.gz` with an `install.sh` (installs to `~/.local`, or system-wide with
  `PREFIX=/usr/local`) for distros that don't use `.deb`; there FFmpeg comes from the system.

### Releasing a new version

`.github/workflows/release.yml` builds both packages on GitHub and publishes them as a Release:

1. Set `version` in `studio/Cargo.toml` (e.g. `0.2.0`), commit, and push to `main`.
2. Tag that commit and push the tag:
   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

The tag has to match the Cargo.toml version, or the workflow stops before building anything.
A tag with a suffix (`v0.3.0-beta.1`) is published as a pre-release.

The icon is drawn from code (`studio/src/icon.rs`); `ai-video-studio --write-icons DIR` writes it
as PNGs and an `.ico`. The build embeds it in the exe together with the product name and version
(`studio/build.rs`; it finds `rc.exe` in the Windows SDK or MinGW's `windres`).

### Code signing

The "Windows protected your PC" warning appears because the installer has no digital signature.
No code change removes it: it takes a code-signing certificate. The options:

- **Microsoft Store**: Microsoft signs the app, so there is no warning.
- **Azure Trusted Signing**: a paid cloud certificate. The release workflow is ready for it: set
  the repository variable `WINDOWS_SIGNING=azure` and the secrets `AZURE_TENANT_ID`,
  `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT` and
  `AZURE_SIGNING_PROFILE`. The app is then signed before it goes into the installer, and the
  installer after. These steps have not run with a real certificate yet: check the first signed
  release.
- **SignPath Foundation**: free signing for open-source projects.

### Not built yet

Planned, in this order, and not in the app today:

1. **Sound and captions**: live audio level meters, lowering music automatically under voice,
   captions (typed or imported from SRT, styled, burned in or exported as SRT).
2. **Recording and showcase tools** (camera recording is done): screen recording with microphone and system sound, voice
   narration recorded while the timeline plays, cursor highlight and click effects, device frames
   (phone, laptop), removing a color (green screen).
3. Tools for the AI assistant (MCP) for the newer edits: group, ripple delete, close gaps, freeze
   frame, reverse, replace media and disable.

