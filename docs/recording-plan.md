# Recording: progress and plan

Where recording stands in AI Video Studio, and the plan for the next phase (screen recording).
Last updated: 2026-11-12, branch `screen-camera`.

- **Phase 1, camera recording:** done (this document's first half).
- **Phase 2, screen recording:** planned, not started (second half).
- **Keyframes by hand:** tasks at the end; not recording work, but kept here.

---

## Phase 1: camera recording (done)

### What a user can do

- Open **Record camera** with `Ctrl+R`, the **Record** button in the title bar, or **Record camera**
  in the empty Media panel.
- Pick a camera and a microphone (or "No microphone"); see a live 30 fps preview and a level meter.
- Turn the picture a quarter at a time (an upright 9:16 video from a camera on its side) or mirror it.
- **Start recording**: a 3-second countdown, then a red border and a clock.
  **Stop and add to timeline** (red button) puts the clip at the playhead; **Discard** throws the take away.
- The last camera, microphone, turn and mirror are remembered.
- Recordings go to `Recordings/` next to the project (the projects folder for an unsaved project).

### How it works

| Part | File |
|---|---|
| One FFmpeg per session: camera + microphone to the file, a small RGBA preview on stdout, the mic level on stderr | `studio/src/recorder.rs` |
| The dialog's state machine and view | `studio/src/editor/record_dialog.rs` |
| Opening it, importing the take, putting it on the timeline (`Project::add_recording`) | `studio/src/editor/mod.rs`, `studio/src/edit.rs` |
| Quit / New / Open / Close refused while recording | `studio/src/app.rs` (`update_inner`) |

- **Devices.** Windows: DirectShow (`ffmpeg -list_devices`, `-list_options`). Linux: the V4L2 driver
  is asked directly with ioctls (only capture nodes; infrared face-login cameras hidden; names
  cleaned up), microphones from `ffmpeg -sources pulse`.
- **Camera mode.** `best_mode` picks the largest picture up to Full HD that reaches 30 fps. On Linux
  the format must be passed (`-input_format mjpeg`): left to itself FFmpeg took raw YUYV at 10 fps.
- **Crash safety.** The take is written as `*.recording.mkv` with 1-second clusters (a crash or an
  unplugged camera loses at most the last second or two), remuxed to MP4 when it ends. Unfinished
  takes are recovered into the library the next time the dialog opens.
- **One camera, one owner.** A camera opens in one program at a time and FFmpeg needs ~0.1 s to let
  go, so sessions take a `Hold` on it. On Linux FFmpeg gets `PR_SET_PDEATHSIG`, so the camera turns
  off with the app even if it crashes (Windows: the job object).
- **Encoder.** x264 `superfast`, CRF 23: same picture as `veryfast` CRF 20 (within 0.06 dB on a camera
  clip) for a third to a half less CPU.
- **Smooth preview.** 640 px at 30 fps; the dialog redraws when a picture arrives
  (`recorder::on_news`) instead of on a timer. On Linux the microphone is read in 20 ms pieces
  (`-fragment_size 3840`); larger pieces made pictures arrive in bursts.
- **Microphone failure.** If the microphone can't be opened, the camera carries on without it and
  the dialog says why.

### Measured on the development laptop (ThinkPad T14s, Ubuntu, built-in camera)

| | Result |
|---|---|
| Camera opens | 0.4–0.6 s |
| Real frame rate in the file | 29.8–30.0 distinct frames a second (was ~10 before the MJPEG fix) |
| FFmpeg CPU while recording | 0.65–0.8 of a core (was ~1.7) |
| Preview | 30 fps, longest gap 76–103 ms |
| 3-minute take | 5,404 frames all distinct; picture 180.13 s, sound 180.12 s; memory steady at 15 MB; file ready 0.3 s after Stop |
| Sound vs picture start | −34 to +26 ms (4 runs) |

### Tests

- In `cargo test -p studio` (fake camera = FFmpeg test pattern + tone):
  - **Recording:** turned/mirrored recording matches its preview; discard; camera that dies mid-take; broken microphone.
  - **Files and devices:** recovery of unfinished takes; the camera `Hold`; device/mode parsing for both systems; plain-words error messages.
  - **Dialog and editor:** dialog flow and countdown; the editor-level flow (recording lands at the playhead, Quit refused).
  - **Stress:** 300 sessions stopped/discarded/dropped at awkward moments; 2,700 random clicks on the dialog; the dialog opened and closed 30 times. `STUDIO_STRESS=10` makes the two storm tests ten times longer.
- Real camera (switch the camera on; run by hand):
  - `cargo test -p studio records_from_a_real_camera -- --ignored --nocapture`: frame rate, preview smoothness, CPU, memory, finishing time. `STUDIO_RECORD_SECONDS=180` for a long take.
  - `cargo test -p studio a_real_camera_survives_rough_handling -- --ignored --nocapture`: 12 back-to-back rounds, then another app holding the camera.

### Also done in this phase

- **Title bar:** never cuts off Minimize/Maximize/Close. The format and zoom chips sit next to the
  project name, and buttons compact as the window narrows.
- **Downloads:** one per system. `AI-Video-Studio-Windows-Setup.exe`, and `AI-Video-Studio-Linux.deb`
  (installs FFmpeg too). README links always fetch the newest one.
- **Resize pointers:** the preview's handles show the right pointer for each corner and side, on
  rotated clips too.
- **Text overflow:**
  - The library tabs turn into icons when the panel is narrow.
  - Settings and Keyboard shortcuts fit small windows.
  - Long names end in "…".

### Still open

- [ ] **Windows:** one real recording on Windows (its capture code is unchanged, but the shared code changed).
- [ ] **By ear:** a clap test for lip sync.
- [ ] **Countdown freeze:** the preview freezes for about half a second when the countdown starts, while the camera reopens to record.
- [ ] **Alt+F4:** closes the app at once, even mid-recording (the take is recovered next time).
- [ ] **Hardware encoding:** NVENC / Quick Sync / VAAPI with x264 fallback. Measured on the laptop: VAAPI uses about a third of a core. This matters much more for screen recording (below).
- [ ] **Unrelated failing test:** `assistant::tests::codex_replies_continue_the_thread_and_stop_cleanly` fails on Linux (its fake `codex` script has a shell syntax error).

---

## Phase 2: screen recording (planned)

### Goal

Record the screen for product demos and SaaS explainers, the app's main kind of footage:
- **Video and sound:** the whole screen, one window or an area, with microphone and system sound.
- **Camera bubble:** optionally the camera at the same time, as a separate clip.
- **Cursor:** where it went and what it clicked, so the editor can add zooms and click effects
  automatically. That turns a raw capture into a polished demo, and it is the reason to record in
  the app instead of with another tool.

### What the user does

1. **Open:** **Record ▸ Screen** (Record becomes a small menu: Camera / Screen), or `Ctrl+Shift+R`.
2. **Choose:**
   - what to record: a monitor, a window, or an area dragged on the screen;
   - the microphone, and system sound on/off;
   - camera bubble on/off.
3. **Record:**
   - the countdown runs, then the editor window gets out of the way (minimized);
   - a small always-on-top control shows the clock, Pause, Stop and Discard;
   - `Ctrl+Shift+R` also stops.
4. **Stop:**
   - the screen recording lands on the timeline at the playhead;
   - the camera clip (if any) lands on the track above it, as a rounded bubble in a corner (the
     editor's corners, border and shadow);
   - with a camera, both are grouped.
5. **Auto zoom.** A **Zoom to clicks** action (inspector and right-click menu) turns the recorded
   clicks into zooms that pan between them, plus press pulses on each click. Both already exist as
   editor features (`zooms`, `camera_at`, press pulses).

### How it works, per system

| | Windows | Linux, X11 session | Linux, Wayland session (this laptop) |
|---|---|---|---|
| Screen | FFmpeg `ddagrab` (Desktop Duplication, GPU, FFmpeg 6+; the bundled BtbN build has it), `gdigrab` as fallback | FFmpeg `x11grab` | xdg-desktop-portal **ScreenCast** → PipeWire stream (the portal is present here: monitors, windows, virtual; cursor hidden / embedded / metadata) |
| Choose a window/area | `ddagrab` monitor index + crop; `gdigrab title=` for a window | `x11grab` with offset and size | the portal's own picker (the system asks the user) |
| System sound | WASAPI **loopback** through `cpal` (already used for playback; 0.15.3: verify loopback on an output device) | the output's pulse **monitor** source (`-f pulse -i <sink>.monitor`), the ones the camera dialog now hides | same as X11 |
| Microphone | as for the camera (DirectShow) | as for the camera (pulse) | as for the camera |
| Cursor and clicks | `GetCursorPos` + button state, polled at 60 Hz | `XQueryPointer` (position + button mask), polled at 60 Hz | position from the portal's cursor **metadata**; clicks are not available on Wayland (see Risks) |
| Keep the control out of the recording | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` | none: place the control on another monitor, or accept it | the portal records the chosen source only |

**Wayland needs code FFmpeg can't provide.** This FFmpeg (6.1) has no PipeWire input. And `x11grab`
under XWayland only sees X11 windows: on a Wayland desktop most of the screen comes out black.
The app runs under XWayland on Linux (`main.rs`), but capture still has to go through the portal.
- **Option A (recommended):** the `ashpd` crate (portal) plus the `pipewire` crate. The app reads
  frames from PipeWire and writes them to FFmpeg's stdin as raw video. Cost: a build dependency on
  `libpipewire-0.3-dev`, which isn't installed here yet, and one more library for the `.deb`'s
  `Depends`.
- **Option B:** GStreamer's `pipewiresrc` as a separate process. Fewer Rust bindings, but a large
  runtime dependency.

**Encoding.** Screens are large (1440p / 4K, often 60 fps), so the encoder choice matters more
than for the camera:
- use a hardware encoder when present: NVENC, Quick Sync, AMF on Windows; VAAPI on Linux;
- check it at startup by encoding one frame, and fall back to x264 `superfast`;
- screen content with x264: CRF 20 and `-tune stillimage` is worth measuring (text must stay sharp);
- frame rate: 30 by default, 60 as an option; a static screen costs almost nothing.

**Files.**
- The video, as for the camera: `Screen recording N.recording.mkv` → `.mp4`.
- Each source written separately: screen video, microphone, system sound, camera.
  - They are combined into one file at the end, with their offsets measured.
  - Separate tracks also mean the editor can mute system sound without touching the voice.
- Cursor data: `Screen recording N.cursor.json` next to the video: `{t, x, y}` at 60 Hz, plus `{t, button, down}` events, in the recording's own pixels.

**Model changes** (through `edit.rs`/`Doc` like every edit):
- `Asset` gets an optional link to its cursor file.
- A new edit `zoom_to_clicks(clip, settings)` adds zooms + press pulses inside the clip's range.
- Add it to `random_edit` in the random-edit stress test.

### Steps

1. **Share the recorder.** Turn `recorder.rs` into a general capture session; the camera keeps
   working the same and its tests stay green:
   - more than one input;
   - more than one output file;
   - the same crash safety and `Hold` logic.
2. **Screen + microphone, Windows and Linux X11.** `ddagrab`/`gdigrab` and `x11grab`; monitor
   choice; the dialog; the timeline drop. Fake source for tests: `lavfi testsrc2` at 1920×1080.
3. **System sound.** Pulse monitor on Linux; WASAPI loopback via `cpal` on Windows, written to its
   own file; offset measured against the screen video.
4. **The recording control.**
   - the editor minimizes;
   - an always-on-top mini window shows the clock and Stop (iced multi-window or a shrunk main
     window, to decide by spike);
   - a global stop shortcut;
   - Windows: kept out of the capture.
5. **Camera bubble.** The camera session runs alongside the screen session; both clips land grouped.
6. **Cursor and clicks.** Pollers on Windows and X11 write the `.cursor.json`; **Zoom to clicks**
   builds zooms and press pulses from it.
7. **Wayland.** Portal + PipeWire (option A); cursor metadata for positions; auto zoom on cursor
   dwell instead of clicks.
8. **Hardware encoding**, shared by camera and screen.
9. **Polish:**
   - pause/resume;
   - area selection overlay;
   - a window list with thumbnails;
   - docs (README, CLAUDE.md), the `.deb` dependencies.

Each step ends with the same kind of checks as phase 1:
- unit tests with fake sources;
- a stress test;
- a real-hardware developer test (`--ignored`) that prints frame rate, CPU, memory and A/V offset;
- debug snapshots of new UI at 1100×680 and 1480×920.

### Risks and decisions to make

- **Wayland clicks.** No portal reports mouse clicks, and a global input hook isn't allowed on
  Wayland. So on Wayland, auto zoom can follow where the cursor *rests* (dwell), not clicks.
  Decide whether that's good enough, or whether Wayland users get manual zooms only.
- **Windows system sound.** `cpal` loopback needs a spike before committing; the fallback is
  a small WASAPI module.
- **CPU at 4K/60** without a hardware encoder may not keep up. Cap software encoding at 1440p/30
  and say so in the dialog.
- **The mini control window.** iced 0.14 can do several windows, but the app is built as one;
  a spike decides between a second window and shrinking the main one.
- **macOS** is out of scope, as for the rest of the app.

### Not in this phase

- Voice narration while the timeline plays (a later "voiceover" feature; it reuses the microphone
  part).
- Device frames (phone, laptop mockups) and green-screen removal: listed in the README's "Not built
  yet", separate work.

---

## Other tasks: keyframes by hand (not started)

Not recording work, kept here so it isn't lost.

**Today:**
- Keyframes exist in the project and render in preview and export. A clip's X, Y, Scale, Rotation
  and Opacity can be keyframed, each keyframe with its own easing (`clip::Keyframe`, `clip.keyframes`).
- Only the AI assistant can make them (`set_keyframes` / `clear_keyframes` in `editor/tools.rs`).
  Nothing in the interface shows, adds, edits or removes them.

**Bug this causes:** on a keyframed property the keyframes override the clip's own value
(`render/scene.rs`, the `animated` closure in `clip_layer`). So after the assistant animates a clip,
dragging it on the preview or changing that value in the inspector does nothing visible.

Tasks:

- [ ] **Fix the bug:** on a keyframed property, a change from the inspector or the preview sets a
  keyframe at the playhead instead of changing the value the keyframes hide.
- [ ] **Inspector:**
  - [ ] a keyframe button next to X, Y, Scale, Rotation and Opacity that adds a keyframe at the
    playhead;
  - [ ] once a property is animated, its field shows the value at the playhead, and changing it
    adds or updates the keyframe there;
  - [ ] previous / next keyframe arrows;
  - [ ] an easing choice and a delete for the keyframe at the playhead;
  - [ ] "Remove animation" for one property or all.
- [ ] **Timeline:**
  - [ ] diamonds on the selected clip at each keyframe;
  - [ ] click a diamond to jump the playhead there, drag it to change its time, right-click to
    delete;
  - [ ] a small marker on clips that have keyframes.
- [ ] **Edits:** add, move, delete and change keyframes through `edit.rs` / `Doc`, so undo/redo work
  and the AI and the interface share the same code; add them to `random_edit` in the random-edit
  stress test.
- [ ] **Checks:** tests for the edits; debug snapshots of the inspector and the timeline diamonds at
  1100×680 and 1480×920; README (feature list and any new shortcuts).
