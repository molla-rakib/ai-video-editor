# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A motion graphics editor for product showcases (Apple-style launches, SaaS explainers), fully
usable by hand, with an optional AI assistant that edits through MCP. English and Bangla.
The Cargo workspace has one app:

- **`studio/`** (binary `ai-video-studio`, "AI Video Studio", Iced 0.14 + wgpu). The original egui
  app that used to live at the repo root, and the throwaway `spikes/iced-ui/` prototype that picked
  Iced, were both removed (they're in git history if ever needed).

README.md is the user-facing setup guide (Windows + Linux), feature list and shortcuts — keep it
in sync when adding features, shortcuts, env vars or file locations. `docs/recording-plan.md` holds
the recording work: what camera recording does and how it was measured, what's still open, and the
plan for screen recording (the next phase) — read it before working on recording, update it after.

## Working rules

- **Never `git commit`** (or push) — the user makes all commits. Read-only git is fine.
- The project is built on **both Windows (GNU toolchain) and Linux** — changes must compile on
  both, and Windows behavior must not change when fixing Linux. OS-specific code goes in
  `studio/src/platform.rs` or behind `#[cfg(windows)]` / `#[cfg(target_os = "linux")]`.
- The user's own running app owns MCP port 47822. Test instances must use another port:
  `AIVE_STUDIO_MCP_PORT=47950`.
- Don't drive the user's Claude account (live assistant chats) without asking.
- UI must look premium: every color/size/font comes from `ui/tokens.rs`; check new UI visually
  with the debug snapshot below (not desktop screenshots — they capture the user's other windows).

## Commands

```sh
cargo run -p studio --release                    # start screen
cargo run -p studio --release -- path/to/x.aivp  # open a project
cargo test -p studio                             # unit + FFmpeg/GPU integration tests
cargo test -p studio range_volume                # tests matching a name
cargo build -p studio --release                  # release build check
```

Tests generate fixtures with real FFmpeg, so it must be findable (lookup in `studio/src/ffmpeg.rs`:
`AIVE_FFMPEG_DIR`, `ffmpeg/` next to the exe, next to the exe, workspace `vendor/ffmpeg/`, `PATH`).
Linux needs `libasound2-dev`. Studio doesn't depend on `ring`, so Windows needs only the Rust
GNU toolchain + MinGW-w64 (WinLibs). Windows packaging:
`scripts\package-studio.ps1` (`-Installer` builds the single `AI-Video-Studio-Windows-Setup.exe` from
`scripts\installer.iss` with Inno Setup 6); Linux: `scripts/package-studio.sh` (needs `dpkg-dev`; builds
`AI-Video-Studio-Linux.deb`, whose `Depends` pulls in FFmpeg and the X11/EGL libraries the app loads, plus a
`.tar.gz` for other distros). Pushing a `v<version>` tag that matches `studio/Cargo.toml` runs
`.github/workflows/release.yml`, which builds both (downloading BtbN FFmpeg for Windows) and publishes a GitHub
Release with exactly those two files. Their names carry no version on purpose: the README links to
`releases/latest/download/<file>`, so renaming them breaks the download links.

### Debug snapshots (debug builds only)

`cargo run -p studio -- project.aivp` with these env vars renders the real window to a PNG and exits:
`STUDIO_SNAPSHOT=out.png`, `STUDIO_SNAPSHOT_DELAY` (seconds), `STUDIO_SNAPSHOT_TIME` (playhead),
`STUDIO_SNAPSHOT_SCROLL`, `STUDIO_SNAPSHOT_SIZE=1100x760` (resize the window first, to check narrow layouts),
`STUDIO_SNAPSHOT_SELECT=clip[,a,b]` (select a clip, optionally mark a range),
`STUDIO_SNAPSHOT_OPEN=ai|assistant|effects|settings|skills|shortcuts|confirm|export|record|recording|record-screen|recording-screen|recorded-screen|record-menu|menu` (open the Connect AI dialog, the
assistant panel, the Effects tab, Settings, the assistant's Skills manager, the Keyboard shortcuts dialog, the
unsaved-changes confirm dialog, the Export dialog, the Record camera dialog — with `AIVE_RECORD_FAKE=1` for a test
pattern instead of the real camera, `=broken-mic` for the microphone-failed note, `=no-screen` for a screen that can't be recorded (a Wayland session: the Record screen dialog records the camera and microphone); `recording` also presses Start, so with
`STUDIO_SNAPSHOT_DELAY=8` it shows the dialog mid-recording; `record-screen` / `recording-screen` are the same for the Record screen dialog (while it records the window is the recording bar, so the snapshot is the bar), `recorded-screen` records a few seconds and stops (delay 14: the window back from the bar with the take on the timeline), `record-menu` opens the title bar's Record menu — or the right-click menu for the selection),
`STUDIO_SNAPSHOT_FIND=words` (type into the inspector's search box, to bring a section into view), `STUDIO_SNAPSHOT_ZOOM=2` (preview zoom), `STUDIO_SNAPSHOT_CHAT=reply.md` (a sample
conversation with that Markdown reply, no AI needed), `STUDIO_SNAPSHOT_REOPEN=1` (switch to the
assistant tab last), `STUDIO_SNAPSHOT_SEEK=seconds` (move the playhead), `STUDIO_SNAPSHOT_TEXT=start,end` (select those characters in, or `STUDIO_SNAPSHOT_TYPE=text` to type at the end of,
the text box of a text clip selected with `STUDIO_SNAPSHOT_SELECT`). Set `APPDATA` (Windows) / `XDG_CONFIG_HOME` (Linux) to a scratch folder for test instances so they don't touch the
user's settings or recent projects.
Wired through `Message::Snapshot` / `DevSelect` in `editor/mod.rs`, all `cfg(debug_assertions)`.

## Studio architecture (`studio/src`)

**Document model.** `model.rs` (Project, Asset, Track with optional `height`, Canvas; saved as
`.aivp` JSON) and `clip.rs` (Clip with transform, look — corners/border/shadow, grade, audio with
fades and range `gains`, Camtasia-style `zooms` where `camera_at` pans between adjacent zooms,
keyframes, intro/outro motions, transitions, text (with `spans`: styled character ranges — change the text only
through `TextLayer::set_text` and styles through `style_range`, which keep spans on their words), shape and blur layers (`Content::Blur`: a box that blurs
the media below it while the clip is on), `crop`, and `part` — Whole, or Picture/Sound after "Separate audio"; use
`Project::clip_kind`, `clip_has_picture`, `clip_has_sound` instead of the asset kind). Timeline
`markers` live on the Project (colored points or spans; not rendered). Positions are canvas
fractions; lengths are "canvas units" (px at a 1080 short side). Types derive `PartialEq`.

**All edits are methods in `edit.rs`, run through `doc.rs`.** `Doc::apply` / `apply_merge` clone
the project, run the edit on the draft, and only commit if it succeeded *and* changed something
(no-op edits make no undo step and don't dirty the project). `apply_merge` with a key coalesces
slider drags/typing within 900 ms. `next_id` never goes backwards across undo/redo because caches
are keyed by ids. Edit invariants: clips never overlap on a track; new clips keep their time and
go to a free (or new) track; locked tracks reject edits (`ensure_unlocked`); a transition (stored on the incoming clip) sits on the cut between two touching clips and moves nothing, Camtasia-style: it is centred on the cut (`Project::transition_into` / `transition_out_of`), the outgoing clip plays on past its end and the incoming one starts early (footage beyond their edges, or the edge frame held; `scene::evaluate`, and an equal-power crossfade in `mix_graph`); `tidy_transitions` (run by `Doc` after every edit and at load) shortens or drops one whose clips no longer touch or have no room, each clip keeping a moment of its own; split / freeze frame refuse to cut inside one; projects saved when a transition was an overlap are converted at load to a cut in the middle of it (`migrate_transitions`), looking the same; trim drops zooms/gains
outside the clip; `set_speed` ripples following clips and rescales zooms/gains/keyframes.
UI, MCP tools and tests all go through `Doc` — never mutate the project directly.
`edit::tests::thousands_of_random_edits_never_break_the_timeline` runs thousands of random edits (with undo/redo and a
save/load round trip) and checks the timeline rules after each; when you add or change an edit, add it to
`random_edit` and raise the seed count once to hunt for rare cases. Projects save with serde_json's
`float_roundtrip`, so numbers load back exactly.

**One renderer for preview and export.** `render/scene.rs::evaluate(project, t, size, sources)`
→ `LayerSpec`s; `render/compositor.rs` + `layer.wgsl` (wgpu; SDF shapes, gradients, corners,
shadows) draws them inside iced's render pass for the preview (`editor/preview.rs`) and headless
for export/snapshots (`render/export.rs::Offscreen`). A new visual effect must go through scene +
compositor to appear anywhere. Crop shrinks the layer and its `uv`; zooms work inside the crop; a blur
clip adds no pixels: `scene::evaluate` maps its box onto each media layer below it (through rotation,
crop and zoom) as per-layer blur regions (up to 4; 9×9 Gaussian in `layer.wgsl`) and keeps an
invisible layer for picking. A mask clip (`Content::Mask`) works the same way but restricts
visibility instead of blurring: `scene::evaluate` maps its box onto each media layer below as
per-layer mask regions (up to 4, feathered, optionally inverted to hide the box instead of
showing only it) — keyframe the mask clip's X/Y (the same general per-clip keyframe system) to
have it follow a moving subject by hand; there's no automatic tracking. The
per-layer uniform is 496 bytes (stride 512) and carries a `view` transform: the preview can be
zoomed past its pane, so `preview.rs` draws only the visible part of the canvas rectangle. Color correction
(`clip::Grade`) is brightness/contrast/saturation/temperature, a lift/gamma/gain three-way (shown as Shadows/
Midtones/Highlights), tint, vibrance, fade, split tone and vignette, applied in `layer.wgsl::apply_grade` from the
per-layer `grade`..`grade4` uniforms; smoothness (an edge-keeping blur for skin) and sharpness sample the texture
in `fs_main` and apply to media only. `Grade::apply` in `clip.rs` is a CPU copy of the same math, used to draw the
Color preset tiles (`inspector::preset_tiles`, cached per selected clip in `Editor::preset_tiles`): change the
shader and it together (a test renders every preset on the GPU and compares). The looks are
`clip::COLOR_PRESETS` (nine, on purpose); the tool schema lists them by name in `tools.rs`. `render/frames.rs` (FrameServer) runs one FFmpeg decoder per
visible clip and caches images; `render/text.rs` rasterizes with cosmic-text (Bangla shaping);
`render/audio.rs::mix_graph` builds the FFmpeg audio mix used by export and playback, including each clip's
EQ (`clip::Eq`, three fixed-band `equalizer` filters), compressor (`clip::Compressor`, `acompressor`) and
denoise (`afftdn`), and — export only — a final `loudnorm` pass when `ExportSettings.loudness` is set.
`playback.rs` plays audio with cpal; the audio clock is the master clock. While playing, `Editor::warm_upcoming` starts the decoder of each video clip a second before it comes on (`frames::Bound::warm`, not counted as lag): a decoder starting at a cut decodes from the keyframe before it (0.15–0.4 s measured), which froze the picture at some cuts.

**Editor UI** (`editor/`): `mod.rs` holds `Editor`, its `Message`s, keyboard shortcuts, dialogs,
autosave (2 s) and the timeline layout; `timeline.rs` (ruler, lanes, zoom bands, volume envelope),
`inspector.rs` (`Edit` enum → edits; `HexDraft` color inputs), `canvas_overlay.rs`
(select/move/scale/rotate, zoom-focus editing), `library.rs` (media/text/shape/effect/sound
presets), `assistant_panel.rs`, `export_dialog.rs` (delivery presets — Web/YouTube/Social/Archive/Custom,
via `render::export::Preset` — cap resolution, audio bitrate and loudness target; "Also as …" queues
further presets to export as a batch after the current one), `media_cache.rs` (thumbnails, filmstrips,
waveforms). `ui/` is the design system: `tokens.rs`, `style.rs`, `widgets.rs`, Lucide `icons.rs`,
`chrome.rs` (frameless title bar — the right-hand group and the window controls get their width first and the
left side, name then the format/zoom chips, is clipped if it must; `App::editor_title` drops Keyboard/Settings and
labels as the window narrows, so Minimize/Maximize/Close are never cut off — and the motion logo), `motion.rs` (`Appear`: dialogs and toasts ease in; every dialog is
built with `widgets::modal(card, dismiss, appear)`). `icon.rs` rasterizes the same logo with plain std: the window icon, `build.rs` (embeds a
`.ico` in the Windows exe through `windres`/`rc.exe`) and `--write-icons DIR` (packaging) all use it.
Right-clicks on the lanes and the preview select what's under the cursor, then one `context_menu()`
serves both (cut/copy/paste/duplicate/delete are one icon row, `MenuItem::Icons`; keep the menu short); it also has **Copy color / Paste color** (`Editor::color_clipboard`, a single `Grade`)
to carry a clip's whole color grade over to others via `Doc::set_grade`. **Keyframes** (X, Y, Scale, Rotation, Opacity) are edited by hand in the inspector's Animation section (`inspector::keyframes`) and as diamonds on the timeline (`Lanes::key_hit`, `Drag::Key`; click jumps the playhead, drag moves, right-click offers Delete keyframe); the edits are `Project::toggle_key`/`key_all`/`remove_keys_at`/`move_keys`/`set_keys_ease`/`clear_animation`. Once a property has keyframes, `Edit::apply` changes it at the playhead (`Clip::set_value_at`: adds or updates a keyframe) and the preview drags from `Clip::value_at`, so a change is never hidden by the animation; keyframes closer than `clip::KEY_EPS` are one. The inspector takes an
`inspector::Ctx` (edit modes, range, collapsed sections, search); every section goes through
`ctx.section(...)` so search and collapse work. The inspector is tabbed (`inspector::Tab`: the clip's own content — Text/Shape/Callout/Counter — then Style, Color, Motion, Audio; `tabs_for` lists a clip's tabs with a dot where something is set): a section's tab comes from its title (`tab_of`), so a new section needs a line there; each kind of clip (`kind_of`) remembers its tab (`Editor::inspector_tabs`, saved as `Settings::inspector_tabs`); the clip's name and the tabs stay put above the scroll, and a search searches every tab. Color shows four sliders, the rest behind "More adjustments" (`COLOR_MORE` in the collapsed set). Every `slider_row` has a number box to type the value into (applied as typed, kept in range; the text being typed is `Editor::number_draft`, rows numbered per draw through the `NUMBERS` thread-local); values never read "-0" (`no_negative_zero`). Framing (Fit/Fill) also sets the clip to full size and centred. Library effects are `library::Effect`s: click
applies to the selection, drag drops on a clip (`Message::DropEffect`) — except `Effect::Transition`,
which targets the seam between two touching clips instead of a clip's body (`timeline::Lanes::seam_hit`
for the drop, `transition_hit` for the resize handle the applied transition shows at its leading edge).
Packaging: `scripts/package-studio.ps1` (Windows) and `scripts/package-studio.sh` (Linux). The window never draws its own frame; on Windows
it gets native rounded corners/shadow.

**AI.** `mcp.rs` serves MCP (JSON-RPC over `tiny_http`) on the first free port from 47822
(`AIVE_STUDIO_MCP_PORT`), local origins only; calls reach the UI thread through a subscription and
answer on a reply channel. Tools live in `editor/tools.rs` (schema + handler per tool) — adding a
tool means adding both there. `assistant.rs` runs the user's `claude` CLI headless against that
server (enabled only when connected). Each Claude Code process has its own inbox (a stopped one can't
leak events into the next reply), runs in a `platform::ProcessGroup` (Windows job object / Linux process
group, so Stop ends everything it started), and tags its MCP requests with `X-Aive-Run`; the editor
refuses tool calls whose run isn't the reply in progress (`Agent::accepts`). Every reply ends in
exactly one `TurnDone`, and the UI closes it through `end_chat_turn`; `ai_connect.rs` is the Connect AI dialog and title-bar chip;
the chosen provider persists in `settings.rs`. ChatGPT runs OpenAI's Codex CLI (`codex exec --json`, one
process per reply, `exec resume <thread>` to continue; settings as `-c key='value'` TOML literal strings so
nothing needs double quotes through `cmd /C`; the run tag goes in the MCP URL as `?run=`). `codex_status`
checks install + `codex login status`; `codex_sign_in` runs `codex login` (browser). Tests use fake
`claude`/`codex` scripts — never the user's accounts. The Codex flags come from the docs, not a live run:
verify them (`codex exec --help`) when Codex is available.

**Groups, ripple edits, clipboard.** `Clip::group` links clips (a video and its separated audio
get one automatically). The selection stays what the user clicked; `Editor::acting()` /
`Project::with_groups` expand it to whole groups at action time (move, trim of a shared edge,
split, copy, delete), and the timeline outlines group mates as linked. `edit.rs` has
`move_clips` (a set lands at the nearest time where all of it fits), `ripple_delete`,
`ripple_delete_range` (cuts a time range out of every track), `close_track_gaps` /
`close_gap_at` / `close_empty_time` (removes only time where nothing plays, so tracks stay in
sync), `paste_clips`, `paste_effects`, `freeze_frame` (`Clip::freeze`: holds the frame at
`src_in`, no sound, no footage limit), `replace_media`, `use_reversed` (`reverse.rs` writes a
backwards copy of the footage with FFmpeg, in pieces so memory stays bounded) and
`set_disabled` (`Clip::disabled`: skipped by `scene::evaluate` and `mix_graph`).

**Shortcuts.** `shortcuts.rs` lists every action with its default keys (`ACTIONS`) and holds the
user's own (`Keymap`, saved in `Prefs::keys`). Key presses arrive as `Message::Key` (editor and
app), are turned into a chord string ("Ctrl+Shift+Z") and looked up there: never match keys
directly. A new shortcut is one `ACTIONS` row plus an arm in `editor::action_message` (or
`App::key_pressed` for app-level ones). The shortcuts dialog rebinds by capturing the next key.
Tooltips and menus read keys with `Editor::key_of`.

**Timeline motion.** `Editor::edit` records where clips were drawn and `glide_from` animates the
ones an edit moved (`timeline::Anim`); `glide_view` eases zoom and scroll. Both advance in
`tick_motion` and clear the lanes cache each frame while running.

**Settings dialog.** `settings_dialog.rs` edits `settings::Prefs` (preview quality with Auto, UI
scale via `App::scale_factor`, snapping/mute defaults, default still length, autosave interval,
export quality, projects folder); `Editor::apply_prefs` applies them. Auto quality steps down while
`FrameServer::take_lag` reports late frames during playback and resets when playback stops.
The audio mix must keep `apad=whole_dur=…`: an unbounded `apad` makes long exports crawl.

**Camera recording.** `recorder/mod.rs` runs one FFmpeg per session (`Session::preview` / `record`): the camera
and microphone go to the file (written as `*.recording.mkv` with 1-second clusters, so a kill keeps all but the
last second, then remuxed to MP4) and to a small RGBA preview on stdout (640 px, 30 fps) plus a mic level on
stderr. The dialog isn't polled on a timer: `recorder::on_news` wakes it (`record_dialog::wakeups`) when a picture
arrives or the state changes, and 10 times a second otherwise. The encoder is x264 `superfast` CRF 23: measured on
a camera clip it looks the same as `veryfast` CRF 20 (within 0.1 dB) for a third to a half less CPU. Devices:
Windows DirectShow (`system` module, `-list_devices`/`-list_options`); Linux asks the V4L2 driver with ioctls
(`system::v4l2`, capture nodes only, infrared greyscale-only cameras hidden) and lists microphones with
`ffmpeg -sources pulse`. `best_mode` picks the size/format for both — on Linux the format must be passed
(`-input_format mjpeg`), or cameras fall back to raw YUYV at ~10 fps, and the pulse microphone needs
`-fragment_size 3840` (20 ms), or pictures reach the preview in bursts. A camera opens in one program at a time
and FFmpeg takes ~0.1 s to let go, so sessions take a `Hold` on it; on Linux FFmpeg gets `PR_SET_PDEATHSIG`
so it ends with the app. `friendly` turns FFmpeg's log into a message and flags microphone failures (input #1
on Linux), which `editor/record_dialog.rs` answers by carrying on without the microphone. Unfinished
recordings (`recorder::unfinished`) are recovered when the dialog opens; `App::update_inner` refuses
Quit/New/Open/Close while `Editor::recording()`. Stress tests (fake camera, in the normal suite; `STUDIO_STRESS=10`
makes them ten times longer): `a_storm_of_sessions_all_end_cleanly`, `a_storm_of_clicks_never_wedges_the_dialog`,
`opening_and_closing_the_record_dialog_over_and_over_leaves_nothing_behind`. Real-device checks (they switch the
camera on — ask before running them): `cargo test -p studio records_from_a_real_camera -- --ignored --nocapture`
prints frame rate, preview smoothness, FFmpeg CPU, memory and finishing time (`STUDIO_RECORD_CAMERA` /
`STUDIO_RECORD_MIC` pick devices by label, `STUDIO_RECORD_SECONDS=180` for a long take), and
`a_real_camera_survives_rough_handling` opens, records, stops and discards back to back, then checks the
"another app is using it" message.

**Screen recording.** `recorder/` is a directory: `mod.rs` (sessions: `run` handles FFmpeg, stop, recovery for both kinds from a `Plan` — what to open, the filter graph, the encoder options; camera: `camera_plan`, screen: `screen::plan`), `camera.rs` (devices and modes), `screen.rs` (monitors and windows as `sources`, `plan`, `finish`), `encoder.rs` (which encoder to use), `desktop.rs` (Windows: monitors from DXGI in `ddagrab`'s numbering, the window list, `bring_to_front`, `hide_from_capture`; each asks for real pixels with `SetThreadDpiAwarenessContext`, or a scaled display mixes sizes). A window is recorded as the area of its monitor it covers (`Screen::area`, `ddagrab` `offset_x/offset_y/video_size`): FFmpeg's `gfxcapture` only delivers frames when the window changes, so a still window never starts. A camera recorded with the screen is a second video stream of the same take (`Plan::extra`, options numbered `-c:v:1`; `screen::for_stream`); it keeps its own smooth timestamps, moved onto the take's clock by the smallest wall-clock gap seen (`screen::anchored_stamp`): stamping it with the wall clock as it arrives threw away more than half its pictures. The Windows desktop capture is anchored the same way (`ddagrab` stamps each picture when it took it): stamped with `time(0)` as it passed through the graph, which is busy with the camera, a loaded machine repeated up to 40% of the pictures (`a_capture_inside_the_graph_keeps_every_picture_once` reproduces it with a paced source in the graph). `finish` splits picture, camera and microphone in one pass; `recorder::recover` does the same for a crashed take. The editor hides itself from captures while the Record screen dialog is open (`Editor::update`, `with_own_window`). One FFmpeg writes one crash-safe `*.recording.mkv`; `screen::finish` then splits it without re-encoding into the picture (`Screen recording N.mp4`) and the microphone (`Screen recording N microphone.m4a`) and returns each file's `start` on the take's clock (`Session::outputs`, `Session::clock`). **Never use `-fps_mode cfr` or `-r` on a screen take**: FFmpeg re-bases the start to 0 and the real gap between picture and sound (measured 0.3–0.4 s) is lost. Instead every input is put on the wall clock (`-use_wallclock_as_timestamps 1` and `-copyts` for the inputs, `asetpts`/`setpts` minus the take's T0; the camera and `ddagrab` by `anchored_stamp`) and the steady rate comes from an `fps=N` filter in the graph (see `docs/recording-plan.md`). `encoder::pick` finds the encoder by *trying* each candidate on a few frames (NVENC, Quick Sync, AMF; VAAPI on Linux; x264 last), once per run; on Windows `ddagrab` frames stay in graphics memory into the encoder (`Path::Gpu`; about 0.1 core at 3200x2000/60 against 3.9 for x264), and x264 is held to 1440p/30. `AIVE_RECORD_ENCODER=x264|qsv|nvenc|amf|vaapi` forces one. `editor/screen_dialog.rs` is its own dialog (not a mode of the camera one); when no screen can be listed (a Wayland session) it still lists the cameras and microphones and records them like the camera dialog does (`camera_only`, `Outcome::Camera`), without the recording bar; a finished take is imported as its assets and `Project::add_screen_recording` lands them grouped, each trimmed or delayed by its `start` against when the countdown ended (`go`); the camera lands as a plain picture in the canvas's bottom-right corner (`place_in_corner`: only size and place, no crop, corners or border: the user styles it). From Start until Stop the window is the **recording bar** (`ScreenDialog::bar`, `bar_view`; `App::sync_bar` saves the window's place, shrinks it to `BAR_SIZE` above everything and puts it back after; the editor isn't sent the bar's size): countdown, clock, mic level, Discard, Stop. The Record screen shortcut stops a take (and cancels a countdown); on Windows it is also held system-wide while the bar is up (`desktop::global_shortcut`, `RegisterHotKey` on a thread of its own), so it works with another app in front. Files are labelled BT.709 wherever the graph converts to it (`Choice::converts_to_709`): FFmpeg 6 doesn't pass the conversion's colour space to the encoder, and unlabelled files were read back as BT.601 (a colour cast). **Never use `-re` on a wall-clock-stamped input** (the fake screen and microphone pace themselves with `realtime`/`arealtime`): FFmpeg 6 then sometimes waits for ever before the first picture (7 takes in 10 with a camera beside it), and a stuck FFmpeg ignores SIGTERM, which is why Linux gives FFmpeg `PR_SET_PDEATHSIG` SIGKILL. Linux (`x11grab`, `xrandr`) builds and its tests pass with fake sources, but no real `x11grab` take has been made (the laptop runs Wayland; it needs an Xorg session); Wayland is not supported yet (the dialog says so). Real-device check (records your screen and microphone, deletes the files, never looks at the pictures): `cargo test -p studio records_the_real_screen -- --ignored --nocapture` (`STUDIO_RECORD_SCREEN`, `STUDIO_RECORD_FPS`, `STUDIO_RECORD_SECONDS`, `STUDIO_RECORD_MIC`); `the_desktop_encoder_for_this_computer` lists which encoders work.

**Other modules.** `settings.rs` (recent projects, layout, AI provider; `%APPDATA%\AIVideoEditor\studio.json`
/ `~/.config/ai-video-editor/studio.json`; projects default to `~/Videos/AI Video Editor`),
`sfx.rs` (9 synthesized sounds written to the config dir), `fonts.rs` (bundled Inter, Poppins,
Hind Siliguri), `ffmpeg.rs`, `platform.rs` (reveal/open in file manager), `main.rs` (on Linux,
removes `WAYLAND_DISPLAY` to run under XWayland so drag and drop works — winit has no Wayland DnD;
`STUDIO_WAYLAND=1` opts out), `shortcuts_dialog.rs` (read-only keyboard shortcuts reference, a
modal layer in `App` like the Settings/Connect AI dialogs), `sample.rs` (`demo_project`: the
"Try an example project" starter scene offered on the start screen, built from text/shape/callout
clips so it needs no bundled media).

**Unsaved changes.** `App::confirm` (a `PendingAction`) gates New/Close/Open/Quit on
`doc.is_dirty()`: a dialog offers Save, Don't save (reverts to the last saved file via
`Project::load`, or is a no-op if never saved) or Cancel before any of them proceeds. `persist()`
surfaces a save failure as the error banner instead of discarding the change silently.

## Gotchas

- Iced draws images above meshes within one layer — stack separate canvases to layer them
  (the timeline is `LanesBase` + `Lanes`). Stacked layers escape a scrollable's clipping, and
  images/text aren't clipped to canvas bounds, so clip manually (`on_canvas`).
- Canvas `with_clip` keeps absolute coordinates.
- In `App::update`, an `editor.update(...)` call's `Task` must be returned (`.map(Message::Editor)`), never
  dropped with `let _ =`: the editor's work in the background (listing cameras and screens, imports,
  probes) is in that task. Dropping it made the title bar's Record menu open dialogs that waited at
  "Looking for…" for ever, while `Ctrl+R` worked. Debug snapshots open the recorders through that menu
  (`Message::PickRecord`) so they exercise the path a user clicks.
- `container(..).clip(true)` only narrows the viewport its children get; their backgrounds and borders still
  draw outside it. Use `widgets::clipped` (a real clip layer that also hides the mouse outside) when content can
  move past its bounds (the track headers scroll that way).
- Keep the widget tree's shape stable: iced matches widget state (scroll offsets, text cursors) by
  position, so wrapping the window in a `stack` only while something is shown rebuilds every panel and
  resets their scrolling. `App::view` always stacks the same layers (`space()` when empty); do the same
  for new overlays.
- FFmpeg filter-script files must outlive the FFmpeg process that reads them. Use
  `ffmpeg::filter_script_option()` (FFmpeg 7 `-/filter_complex` vs 6.x `-filter_complex_script`).
- Export writes through a cleanup guard so a cancelled/failed export leaves no partial file.
- cosmic-text's `SwashCache::with_pixels` discards a glyph color's alpha channel, replacing it with
  the glyph's own coverage/anti-aliasing mask (`Color((coverage << 24) | base.0 & 0xFF_FFFF)` in
  their source) — setting alpha via `Attrs::color()` has no visible effect. Per-word fade (the
  kinetic text reveal, `render/text.rs`) instead packs a word index alongside the existing
  style-span index into one `Attrs::metadata()` value (`pack_word_meta`/`unpack_style`/
  `unpack_word`), builds word bounding boxes from the laid-out glyphs, and multiplies alpha by
  position inside the `buffer.draw` callback, after coverage is known.

