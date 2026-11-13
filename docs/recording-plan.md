# Recording: progress and plan

Where recording stands in AI Video Studio, and the plan for the next phase (screen recording).
Last updated: 2026-11-13.

- **Phase 1, camera recording:** done and checked on Windows and Linux (this document's first half).
- **Phase 2, screen recording:** first version built and working on Windows (screen + microphone → grouped clips on the timeline, hardware encoding); see "Progress" in the second half for what is left.
- **Keyframes by hand:** tasks at the end; not recording work, but kept here.

---

## Phase 1: camera recording (done)

### What a user can do

- Open **Record camera** with `Ctrl+R`, the **Record** button in the title bar, or **Record camera**
  in the empty Media panel.
- Pick a camera and a microphone (or "No microphone"); see a live 30 fps preview and a level meter.
- Turn the picture a quarter at a time (an upright 9:16 video from a camera on its side), mirror it (left to right) or flip it upside down. The Record screen dialog has the same four buttons for its camera.
- **Start recording**: a 3-second countdown, then a red border and a clock.
  **Stop and add to timeline** (red button) puts the clip at the playhead; **Discard** throws the take away.
- The last camera, microphone, turn, mirror and flip are remembered.
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

### Measured on Windows (Windows 11, Core Ultra 5 125H, USB2.0 FHD UVC WebCam, Intel mic array; 2026-11-12)

| | Result |
|---|---|
| Camera opens | 1.8 s first picture (Linux laptop: 0.4–0.6 s); worst case 4.5 s when reopened right after a stop |
| Real frame rate in the file | 28.9 distinct frames a second in a 1920×1080, 30 fps file |
| FFmpeg CPU while recording | 0.65 of a core |
| Preview | 28–30 fps, longest gap 65–74 ms |
| 5-second take | picture 5.37 s, sound 5.31 s; memory flat at 12 MB; file ready 0.73 s after Stop |
| Rough handling | 12 back-to-back rounds, 7 kept, none failed; another program holding the camera is reported in plain words and the camera opens again afterwards |

`cpu_seconds` and `memory_mb` in the real-camera tests now work on Windows too (they only read
`/proc` before, so Windows printed 0%).

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

- [x] **Windows:** real recording and rough-handling tests pass (numbers above). Still to do by hand: one take through the dialog.
- [ ] **Windows camera start-up:** 1.8 s to the first picture, up to 4.5 s right after a stop (DirectShow is slow to let go). The countdown hides most of it; look at it together with the countdown freeze.
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

**Measured on Windows (2026-11-12): keep the frames on the GPU.** A 3200×2000 screen asked for at
60 fps, 10 seconds, FFmpeg 8.1, encoded to nothing (`-f null`):

| Pipeline | Frames out | CPU used |
|---|---|---|
| `ddagrab` → download → x264 `superfast` | 520 of 600 | 3.9 cores |
| **`ddagrab` → `hwmap=derive_device=qsv` → `h264_qsv`** | **573** | **0.10 core** |
| `ddagrab` → download → `h264_qsv` | 460 | 1.5 cores |
| `ddagrab` → download → `h264_mf` | 451 | 2.8 cores |

Only the all-GPU path keeps 60 fps at native size. Downloading frames to system memory is what
costs: at this size it is about 1.5 GB a second. NVENC and AMF can't be tried on this machine
(Intel only), so their paths stay unverified and fall back to download-then-encode.

**Measured on Windows (2026-11-12): sync needs wall-clock stamps.** Screen (`ddagrab`, a filter) and
microphone (`dshow`, an input) are on different clocks. Left to itself FFmpeg puts each one's *first
sample* at 0, so the gap between them is thrown away: 6-second recordings of screen + microphone
showed the screen starting 318–355 ms after the microphone (it varies per run), which would have
played as sound 0.3 s late. What works:
- video `setpts='(time(0)-T0)/TB'` (T0 = wall-clock seconds taken at spawn), then `fps=N` in the same
  graph for steady spacing (0.033–0.034 s between frames at 30 fps);
- audio `-use_wallclock_as_timestamps 1` on the input, then `asetpts='PTS-T0/TB'`;
- `-copyts` globally, and **`-fps_mode passthrough`**: with `-fps_mode cfr` FFmpeg re-based the video
  to 0 again and the gap was lost.

Each stream's real start then survives in the file (`ffprobe` `start_time`), and Stop reads the
offsets from it instead of trusting FFmpeg. Not yet measured: the microphone's own capture delay
(about one 50 ms buffer: the first chunk is stamped when it arrives) and the real speaker-to-picture
latency; a flash-and-beep take on real hardware and the clap test settle those.

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

### Progress (2026-11-12)

Done, on Windows with real hardware (3200x2000 screen, Quick Sync, Intel mic array) and with fake sources in the normal test suite:

- [x] **Recorder split** into `recorder/{mod,camera,screen,encoder}.rs`; `run` is driven by a `Plan`, so the camera and the screen share process handling, stop, recovery and tests. The 12 camera tests stayed green untouched.
- [x] **Encoder choice by trying** (`encoder.rs`): the first of NVENC, Quick Sync, AMF that really encodes, x264 last; `AIVE_RECORD_ENCODER` forces one. Windows desktop frames stay in graphics memory.
- [x] **Sync by wall-clock stamps** (measured above) and `screen::finish` splitting the take into picture and microphone files with their starts; the picture is cut where the sound ends (the capture filter keeps going about 0.65 s after Stop).
- [x] **Screen + microphone, to the timeline:** `editor/screen_dialog.rs`, title bar Record menu (Camera / Screen), `Ctrl+Shift+R`, `Project::add_screen_recording` (in `random_edit`; 400 seeds passed), remembered choices (`ScreenChoice`), screen thumbnail, crash recovery of unfinished takes.
- [x] Debug snapshots checked at 1480x920 and 1100x680 (`record-screen`, `recording-screen`, `record-menu`).

Measured, 3200x2000, Quick Sync on the graphics chip, 30 pictures a second, 6-second take: FFmpeg 14-15% of a core (x264 would be about 390%), this app's memory flat at 11 MB, Stop to both files ready 1.06 s, 219 pictures with every gap 33-34 ms, the picture started 427-429 ms after the microphone (kept, not lost). First picture after 2.4 s the first time (about 1 s of that is the one-time encoder trial: the dialog already runs it when it opens, so later starts are quicker).

Test-and-tighten pass (2026-11-12, same machine):
- **Colours fixed.** Quick Sync was converting the screen one way and labelling the file another (26 dB
  against the original instead of 47: a visible colour cast). Every path now converts explicitly to
  BT.709 limited range, and `encoder::works` also checks a known colour survives encode and decode, so a
  driver that gets it wrong is passed over. Here: [199, 58, 29] back for [200, 60, 30].
- **Voice no longer clipped at Stop.** FFmpeg dropped the microphone's last ~0.32 s on `q`. A screen
  take now records 600 ms past Stop and both files are cut at the moment Stop was pressed: the voice
  ends within +4 to +20 ms of it (3 takes).
- **Cheaper:** the thumbnail drops pictures before copying them out of graphics memory (halves
  FFmpeg's CPU: 1.36 s → 0.47 s of CPU per 10 s); finishing is one pass without rewriting the files
  (10-minute take: 2.7 s → 0.8 s); encoder candidates are tried in parallel (pick: 1.5 s → 1.0 s).
- Real takes: 30 fps 13% of a core, 60 fps 15%, gaps 16–17 ms at 60 fps; memory flat.

Screen + camera + one window (2026-11-13, Windows, real hardware):
- One take records screen, webcam and microphone into three files, cut where Stop was pressed (voice within
  +1 to +16 ms of it). Camera 1920x1080 at 29.5-29.9 real pictures a second, after fixing its stamping
  (13 a second when stamped with the wall clock as frames arrived). FFmpeg 0.8-0.9 of a core with the
  camera (0.15 without): mostly decoding the webcam's 1080p MJPEG on the processor.
- The camera lands as a round bubble (centred square crop, ~28% of the canvas's short side, bottom right,
  border and shadow) on a track above the screen, grouped; one undo.
- Windows are listed in 8 ms (DXGI + EnumWindows) and recorded as their screen area; the app is hidden
  from the capture while the dialog is open (`SetWindowDisplayAffinity`).
- Still to check by you: lip sync of the camera with a clap; a window take; two monitors.
- Follow-up: record the camera at 720p for the bubble (about half the processor time), or decode its
  MJPEG on the graphics chip.

Smooth and professional pass (2026-11-13, on the Linux laptop with fake sources; Windows type-checked from Linux):
- **Recording bar (step 4, mostly).** From Start until Stop the editor window becomes a small always-on-top
  bar (468x56) at the bottom centre of where it was, draggable: the countdown, then a pulsing red dot, the
  clock, the microphone's level, Discard and Stop. The window is put back exactly where and how it was
  (maximized included) when the take ends, fails or is cancelled. On Windows it was already hidden from
  the capture, so the bar isn't recorded; on Linux X11 it is (nothing hides a window from `x11grab`).
- **Stop from anywhere.** The Record screen shortcut (`Ctrl+Shift+R`) stops a take and cancels a countdown;
  on Windows it is held system-wide while the bar is up (`RegisterHotKey`), so it works with the recorded
  app in front. Not on Linux (X11 would need a key grab; Wayland allows none).
- **Camera lands plain.** No more round bubble: the camera clip lands as its whole picture, square-cornered,
  no border or shadow, shrunk into the bottom-right corner (28% of the canvas's short side tall) on a
  track above the screen, grouped. Styling is left to the user. The dialog shows it the same way.
- **Smoother screen picture (Windows).** `ddagrab`'s pictures were stamped with `time(0)` when they passed
  through the graph; with the camera in the same graph, a busy machine stamped some late and the `fps`
  filter doubled one and dropped the next. They now keep `ddagrab`'s own capture times, anchored onto the
  take's clock like the camera's. Reproduced with a paced test pattern inside the graph, a camera and a
  microphone, every processor core busy: old stamping repeated 3 and 52 of 127 pictures (two runs), the new
  one 0 of 105 (two runs), and 0 without load either way (`a_capture_inside_the_graph_keeps_every_picture_once`).
- **Colours on Linux.** FFmpeg 6.1 doesn't pass the BT.709 conversion's colour space to the encoder, so
  files went out unlabelled and were read back as BT.601: [187, 45, 32] for [200, 60, 30]. Files are now
  labelled BT.709 wherever the graph converts to it (`converts_to_709`; NVENC and AMF from graphics memory,
  which convert themselves, are left as they were).
- **A take that never started.** The fake screen and microphone (tests and snapshots) used `-re` on
  wall-clock-stamped inputs: FFmpeg 6.1 then sometimes waited for ever before the first picture (7 replays
  in 10; 3 of 12 test runs). They pace themselves with `realtime`/`arealtime` now: 0 in 20 replays, 0 of 15
  test runs. Real devices never used `-re`.
- **No FFmpeg left behind on Linux.** A stuck FFmpeg ignores SIGTERM, and one was found still recording
  minutes after its app had gone. `PR_SET_PDEATHSIG` is now SIGKILL (Windows' job object already does the
  same); the take keeps all but its last second and is recovered as before.
- Also: `app_name` reads Windows paths on any system (its test failed on Linux).

Still open from this pass:
- [ ] **Check on Windows by hand:** the bar (shrinks, stays on top, isn't in the take, comes back, also
  from maximized and on a second monitor), `Ctrl+Shift+R` with another app in front, and
  `records_the_real_screen` with `STUDIO_RECORD_CAMERA=first` for the picture gaps.
- [ ] **Pause / resume** (Camtasia's F9): not built.
- [ ] Under the full test suite on an 8-core laptop, takes made with fake sources sometimes come out shorter
  than the tests expect (the machine falls behind real time and what is still queued at Stop is cut): see
  the end of this list for whether HEAD did the same.

Not done yet, in this order (from the steps above):

- [ ] **Linux X11 check.** `x11grab`, `xrandr` parsing (unit-tested) and the pulse microphone are written but never built or run: build, run `cargo test -p studio`, run `records_the_real_screen` on the laptop under an Xorg session, and check VAAPI with `the_desktop_encoder_for_this_computer`.
- [ ] **Step 4, the rest of the recording control:** the bar and the Windows global stop shortcut are done (above); still to do: stop and save when the window is closed mid-take, and the 60 Hz input thread that also feeds the cursor file.
- [ ] **Step 3, system sound** (loopback on Windows, pulse monitor on Linux) as a third stream in the take.
- [ ] **Step 5, camera bubble**; **step 6, cursor and Zoom to clicks**; **step 7, window and area**; **step 8 (Wayland)**; **step 9, pause and polish**.
- [ ] **Microphone delay.** The microphone is stamped when a 50 ms chunk arrives, so it may lead the picture by up to about 50 ms; a flash-and-beep take and the clap test decide whether to correct it. The 3-second countdown also hides the camera and encoder start-up, but a very slow start (over 3 s) now makes the clips start a little after the end of the countdown (they are placed in step, never out of it).
- [ ] Camera recording does not use the encoder picker yet (x264 `superfast`, as measured in phase 1): adopt it with a quality comparison.

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

## Other tasks: keyframes by hand (done 2026-11-13)

Not recording work, kept here so it isn't lost.

- **The bug is fixed.** A change to an animated property (X, Y, Scale, Rotation, Opacity) from the
  inspector or the preview adds or updates a keyframe at the playhead (`Clip::set_value_at`, used by
  `Edit::apply`); the preview drags an animated clip from where it is at the playhead. Without keyframes
  a change is still just a change. **Reset** in the Layer section also removes the animation.
- **Inspector, Animation section** (shown for every clip with a picture): previous / next keyframe,
  **Add** (keys every property at the playhead) or **Remove** (the keyframe there), one row per
  property with its value at the playhead and a diamond to add or remove that property's key, the
  **Easing** of the keyframe at the playhead (Smooth, Linear, Ease in, Ease out, Bounce) and **Remove
  animation** (holds what the playhead shows). Rotation and Opacity sliders show the value at the playhead.
- **Timeline:** a diamond at each keyframe of every clip (white on the selected one), joined by a line.
  Click: the clip is selected and the playhead jumps there. Drag: the keyframe moves (snapping, kept
  inside the clip; a keyframe it lands on gives way), the playhead follows. Right-click: **Delete
  keyframe** in the menu.
- **Edits** in `edit.rs` (`toggle_key`, `key_all`, `remove_keys_at`, `move_keys`, `set_keys_ease`,
  `clear_animation`), all through `Doc` so undo/redo work; in `random_edit`, 400 seeds passed; keyframes
  in order and finite is a timeline rule. Tests: `keyframes_by_hand_animate_and_never_hide_a_change`,
  `keyframes_are_added_seen_and_changed_by_hand`, `a_keyframe_s_diamond_is_clicked_dragged_and_right_clicked`.
- Not done: keyframes for other properties (color, crop, volume); a curve editor.
