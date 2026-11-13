# Contributing to AI Video Studio

Thanks for your interest in contributing! This is a motion graphics editor for product
showcases (Apple-style launches, SaaS explainers), built with Rust + [Iced](https://iced.rs/)
+ wgpu, with an optional AI assistant that edits through MCP.

## Before you start

- Open an issue (or comment on an existing one) before starting large changes, so we can agree
  on the approach first. Small fixes and clear bugs can go straight to a PR.
- Read `CLAUDE.md` at the repo root — it documents the architecture, invariants and gotchas in
  detail and is the best single reference for how the codebase fits together, whether you're a
  human or an AI contributor.

## Setup

The workspace has one app, `studio/` (binary `ai-video-studio`).

```sh
cargo run -p studio --release                    # start screen
cargo run -p studio --release -- path/to/x.aivp  # open a project
cargo test -p studio                             # unit + FFmpeg/GPU integration tests
cargo build -p studio --release                  # release build check
```

**Requirements:**
- Rust (stable), via [rustup](https://rustup.rs/).
- **Windows:** the GNU toolchain (`rustup default stable-x86_64-pc-windows-gnu`) and
  MinGW-w64 (e.g. [WinLibs](https://winlibs.com/)) on `PATH`. No `ring` dependency, so no MSVC
  build tools needed.
- **Linux:** `libasound2-dev` (ALSA headers, for audio playback).
- **FFmpeg**, for both running the app and the test suite (tests generate fixtures with real
  FFmpeg). See `studio/src/ffmpeg.rs` for how it's located (`AIVE_FFMPEG_DIR`, next to the exe,
  workspace `vendor/ffmpeg/`, or `PATH`).

## Project rules

- The app must compile and run correctly on **both Windows (GNU toolchain) and Linux** — a fix
  for one platform must not change behavior on the other. OS-specific code goes in
  `studio/src/platform.rs` or behind `#[cfg(windows)]` / `#[cfg(target_os = "linux")]`.
- UI must look premium: every color/size/font comes from `studio/src/ui/tokens.rs`, not a
  hardcoded literal. Check new UI visually — see the debug snapshot mechanism in `CLAUDE.md`
  (`STUDIO_SNAPSHOT=out.png` and friends) for a way to render a real frame to a PNG without a
  desktop screenshot.
- All document edits (anything that changes a `Project`) go through `edit.rs` methods run via
  `Doc::apply`/`apply_merge` — never mutate the project directly from UI code, tools, or tests.
- A new visual effect must go through the shared render path (`render/scene.rs` → `render/
  compositor.rs` + `layer.wgsl`) so it renders identically in the live preview and in exports.
- If you touch the MCP tool surface (`editor/tools.rs`), update both the tool's JSON schema and
  its handler — they're defined together.
- Keep `README.md` in sync when you add a feature, keyboard shortcut, environment variable, or
  file location that a user would need to know about.

## Testing

```sh
cargo test -p studio                 # full suite
cargo test -p studio range_volume    # tests matching a name
```

Tests that exercise FFmpeg or the renderer need a real FFmpeg on the path described above. Test
instances of the app must use a separate MCP port (`AIVE_STUDIO_MCP_PORT=47950`) and a scratch
`APPDATA`/config dir, so they never touch a real user's settings, recent-projects list, or the
default MCP port.

## Submitting changes

- Keep PRs focused — one feature or fix per PR is easier to review.
- Describe *why* a change is needed, not just what it does; the code and tests should make the
  "what" self-evident.
- Make sure `cargo test -p studio` passes and `cargo build -p studio --release` succeeds before
  opening a PR.

## License

By contributing, you agree that your contributions will be licensed under the same dual
MIT / Apache-2.0 license as the rest of the project (see `LICENSE-MIT` and `LICENSE-APACHE`).
