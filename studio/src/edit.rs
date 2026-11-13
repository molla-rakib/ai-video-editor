//! Every change to a project. Each operation either succeeds or leaves the
//! project untouched, and clips on one track never overlap.

use std::path::Path;

use anyhow::{Context, bail, ensure};

use crate::clip::{CalloutLayer, Clip, Content, Fit, Prop, KEY_EPS, Grade, Motion, MotionKind, Part, Shadow, ShapeLayer, TextLayer, Transition, TransitionKind, Zoom};
use crate::media::ProbeInfo;
use crate::model::{Asset, Id, MediaKind, Project, Track, TrackKind};

/// Shortest clip allowed, in seconds.
pub const MIN_CLIP: f64 = 0.1;

/// A recorded camera's shorter side, and its margin from the canvas edges, as fractions of the canvas's short side.
const CAMERA_SIZE: f64 = 0.28;
const CAMERA_MARGIN: f64 = 0.045;

/// The files of a finished screen recording, as library assets with the moment each starts on the
/// take's clock (`recorder::Output::start`), and when the countdown ended on that clock (`go`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenTake {
    pub screen: (Id, f64),
    pub camera: Option<(Id, f64)>,
    pub mic: Option<(Id, f64)>,
    pub go: f64,
}
/// Default length for images, text and shapes.
pub const DEFAULT_STILL: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

impl Project {
    pub fn asset(&self, id: Id) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    pub fn clip(&self, id: Id) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    pub fn clip_mut(&mut self, id: Id) -> anyhow::Result<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id).context("That clip no longer exists")
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: Id) -> anyhow::Result<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id).context("That track no longer exists")
    }

    pub fn track_index(&self, id: Id) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// End of the last clip.
    pub fn duration(&self) -> f64 {
        self.clips.iter().map(Clip::end).fold(0.0, f64::max)
    }

    pub fn clips_on(&self, track: Id) -> impl Iterator<Item = &Clip> {
        self.clips.iter().filter(move |c| c.track == track)
    }

    /// Track kind a clip with this content needs.
    pub fn kind_for(&self, content: &Content) -> TrackKind {
        match content {
            Content::Media { asset, .. } => match self.asset(*asset).map(|a| a.kind) {
                Some(MediaKind::Audio) => TrackKind::Audio,
                _ => TrackKind::Visual,
            },
            _ => TrackKind::Visual,
        }
    }

    /// Track kind an existing clip needs (separated sound goes on audio tracks).
    pub fn clip_kind(&self, clip: &Clip) -> TrackKind {
        if clip.part == Part::Sound { TrackKind::Audio } else { self.kind_for(&clip.content) }
    }

    /// Whether the clip shows a picture (text, shapes, images, and video that isn't sound-only).
    pub fn clip_has_picture(&self, clip: &Clip) -> bool {
        match clip.content {
            Content::Media { asset, .. } => clip.part != Part::Sound && self.asset(asset).is_some_and(|a| a.is_visual()),
            _ => true,
        }
    }

    /// Whether the clip plays sound (audio, and video with sound that hasn't been separated).
    pub fn clip_has_sound(&self, clip: &Clip) -> bool {
        match clip.content {
            Content::Media { asset, .. } => !clip.freeze && clip.part != Part::Picture && self.asset(asset).is_some_and(|a| a.has_audio && a.kind != MediaKind::Image),
            _ => false,
        }
    }

    /// Longest the clip can be, given its source media.
    pub fn max_duration(&self, clip: &Clip) -> f64 {
        match clip.content {
            Content::Media { asset, src_in, speed } => match self.asset(asset) {
                Some(a) if a.kind != MediaKind::Image && !clip.freeze => ((a.duration - src_in) / speed).max(MIN_CLIP),
                _ => f64::INFINITY,
            },
            _ => f64::INFINITY,
        }
    }

    /// Fails if the clip sits on a locked track.
    pub fn ensure_unlocked(&self, clip: Id) -> anyhow::Result<()> {
        let c = self.clip(clip).context("That clip no longer exists")?;
        ensure!(!self.track(c.track).is_some_and(|t| t.locked), "That track is locked");
        Ok(())
    }

    fn overlaps(&self, track: Id, start: f64, duration: f64, ignore: &[Id]) -> bool {
        const EPS: f64 = 1e-6;
        self.clips_on(track).any(|c| !ignore.contains(&c.id) && start < c.end() - EPS && start + duration > c.start + EPS)
    }

    /// The clip immediately before `clip` on its own track, by start time.
    pub fn prev_on_track(&self, clip: &Clip) -> Option<&Clip> {
        self.clips_on(clip.track).filter(|c| c.id != clip.id && c.start < clip.start + 1e-6).max_by(|a, b| a.start.total_cmp(&b.start))
    }

    // Transitions sit on the cut between two touching clips, as in Camtasia: nothing moves. Over the
    // transition (centred on the cut, half its length on each side) the outgoing clip plays on past
    // its end and the incoming one starts early, from footage beyond their edges where the file has
    // it, or holding the edge frame where it doesn't. Clips never overlap on a track.

    /// The transition into `c` from the clip right before it: that clip and the transition.
    pub fn transition_into(&self, c: &Clip) -> Option<(&Clip, Transition)> {
        let tr = c.transition?;
        let prev = self.prev_on_track(c).filter(|p| (p.end() - c.start).abs() < 1e-4)?;
        Some((prev, tr))
    }

    /// The transition out of `c` into the clip right after it: that clip and the transition.
    pub fn transition_out_of(&self, c: &Clip) -> Option<(&Clip, Transition)> {
        self.clips_on(c.track).filter(|n| n.id != c.id && n.start > c.start && (n.start - c.end()).abs() < 1e-4).find_map(|n| n.transition.map(|t| (n, t)))
    }

    /// The longest transition that fits between `prev` and `clip`: each keeps a moment of its own
    /// between its transitions.
    fn transition_room(&self, prev: &Clip, clip: &Clip) -> f64 {
        let prev_in = self.transition_into(prev).map_or(0.0, |(_, t)| t.duration / 2.0);
        let clip_out = self.transition_out_of(clip).map_or(0.0, |(_, t)| t.duration / 2.0);
        2.0 * (prev.duration - prev_in - MIN_CLIP).min(clip.duration - clip_out - MIN_CLIP)
    }

    /// Removes clip `id`'s own transition (nothing moves).
    fn unapply_transition(&mut self, id: Id) {
        if let Ok(c) = self.clip_mut(id) {
            c.transition = None;
        }
    }

    /// Removes clip `id`'s own transition and the one out of it: called by edits that move or
    /// resize `id`, which would leave either without its touching partner.
    fn detach_transition(&mut self, id: Id) {
        let dependent = self.clips.iter().find(|c| c.id != id && c.transition.is_some() && self.prev_on_track(c).is_some_and(|p| p.id == id)).map(|c| c.id);
        if let Some(dep) = dependent {
            self.unapply_transition(dep);
        }
        self.unapply_transition(id);
    }

    /// Keeps every transition true after an edit: between two touching clips, short enough that
    /// each keeps a moment of its own. One that no longer fits is shortened, or removed when nothing
    /// fits. (Run by `Doc` after every edit and when a project is opened.)
    pub(crate) fn tidy_transitions(&mut self) {
        let mut ids: Vec<(Id, f64)> = self.clips.iter().filter(|c| c.transition.is_some()).map(|c| (c.id, c.start)).collect();
        ids.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (id, _) in ids {
            let Some(clip) = self.clip(id).cloned() else { continue };
            let room = self.prev_on_track(&clip).filter(|p| (p.end() - clip.start).abs() < 1e-4).map(|p| self.transition_room(p, &clip));
            let c = self.clip_mut(id).unwrap();
            match (room, &mut c.transition) {
                (Some(room), Some(tr)) if room >= MIN_CLIP - 1e-9 && tr.duration.is_finite() => tr.duration = tr.duration.min(room).max(MIN_CLIP),
                _ => c.transition = None,
            }
        }
    }

    /// Replaces clip `id`'s color grade wholesale (used by grade presets, and to paste a grade
    /// copied from another clip).
    pub fn set_grade(&mut self, id: Id, grade: Grade) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        self.clip_mut(id)?.grade = grade;
        Ok(())
    }

    /// Sets, resizes, or (with `kind: None`) removes the transition from the clip touching clip
    /// `id`'s start into it. Nothing moves: the transition is centred on the cut (see `transition_into`).
    pub fn set_transition(&mut self, id: Id, kind: Option<TransitionKind>, duration: f64) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        let Some(kind) = kind else {
            self.unapply_transition(id);
            return Ok(());
        };
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let prev = self.prev_on_track(&clip).filter(|p| (p.end() - clip.start).abs() < 1e-4).context("Nothing to transition from")?.clone();
        let longest = self.transition_room(&prev, &clip);
        ensure!(longest >= MIN_CLIP - 1e-9, "These clips are too short for a transition");
        // (min/max, not clamp: rounding can leave `longest` a hair under MIN_CLIP.)
        let wanted = if duration.is_finite() { duration } else { 0.6 };
        let duration = wanted.min(longest).max(MIN_CLIP.min(longest));
        self.clip_mut(id)?.transition = Some(Transition { kind, duration });
        Ok(())
    }

    /// The free start time closest to `wanted` where a clip of `duration` fits on `track`.
    pub fn nearest_free(&self, track: Id, wanted: f64, duration: f64, ignore: &[Id]) -> f64 {
        let wanted = wanted.max(0.0);
        if !self.overlaps(track, wanted, duration, ignore) {
            return wanted;
        }
        let mut candidates = vec![0.0];
        for c in self.clips_on(track).filter(|c| !ignore.contains(&c.id)) {
            candidates.push(c.end());
            candidates.push(c.start - duration);
        }
        candidates
            .into_iter()
            .filter(|&s| s >= 0.0 && !self.overlaps(track, s, duration, ignore))
            .min_by(|a, b| (a - wanted).abs().total_cmp(&(b - wanted).abs()))
            .unwrap_or_else(|| self.clips_on(track).map(Clip::end).fold(0.0, f64::max))
    }

    pub fn add_track(&mut self, kind: TrackKind, name: &str, index: Option<usize>) -> Id {
        let id = self.alloc_id();
        let track = Track { id, name: name.to_string(), kind, muted: false, locked: false, hidden: false, height: None };
        // Visual tracks stay above audio tracks.
        let first_audio = self.tracks.iter().position(|t| t.kind == TrackKind::Audio).unwrap_or(self.tracks.len());
        let at = match (kind, index) {
            (TrackKind::Visual, Some(i)) => i.min(first_audio),
            (TrackKind::Visual, None) => first_audio,
            (TrackKind::Audio, Some(i)) => i.clamp(first_audio, self.tracks.len()),
            (TrackKind::Audio, None) => self.tracks.len(),
        };
        self.tracks.insert(at, track);
        id
    }

    pub fn rename_track(&mut self, id: Id, name: &str) -> anyhow::Result<()> {
        let name = name.trim();
        ensure!(!name.is_empty(), "Track names can't be empty");
        self.track_mut(id)?.name = name.to_string();
        Ok(())
    }

    pub fn rename(&mut self, name: &str) -> anyhow::Result<()> {
        let name = name.trim();
        ensure!(!name.is_empty(), "Project names can't be empty");
        self.name = name.to_string();
        Ok(())
    }

    pub fn delete_track(&mut self, id: Id) -> anyhow::Result<()> {
        let i = self.track_index(id).context("That track no longer exists")?;
        let kind = self.tracks[i].kind;
        ensure!(self.tracks.iter().filter(|t| t.kind == kind).count() > 1, "Keep at least one {} track", if kind == TrackKind::Audio { "audio" } else { "video" });
        self.tracks.remove(i);
        self.clips.retain(|c| c.track != id);
        // A video whose separated sound was on that track is no longer part of a group.
        self.tidy_groups();
        Ok(())
    }

    /// Moves a track up (`delta` < 0) or down, staying within its kind's group.
    pub fn move_track(&mut self, id: Id, delta: i32) -> anyhow::Result<()> {
        let i = self.track_index(id).context("That track no longer exists")?;
        let j = i as i32 + delta;
        ensure!(j >= 0 && (j as usize) < self.tracks.len(), "Can't move further");
        ensure!(self.tracks[j as usize].kind == self.tracks[i].kind, "Video and audio tracks stay in their own groups");
        self.tracks.swap(i, j as usize);
        Ok(())
    }

    pub fn add_asset(&mut self, path: &Path, info: &ProbeInfo) -> Id {
        if let Some(existing) = self.assets.iter().find(|a| a.path == path) {
            return existing.id;
        }
        let id = self.alloc_id();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Media".into());
        self.assets.push(Asset {
            id,
            path: path.to_path_buf(),
            name,
            kind: info.kind,
            duration: info.duration,
            width: info.width,
            height: info.height,
            fps: info.fps,
            has_audio: info.has_audio,
        });
        id
    }

    pub fn remove_asset(&mut self, id: Id) -> anyhow::Result<()> {
        ensure!(!self.clips.iter().any(|c| c.asset() == Some(id)), "This media is used on the timeline. Delete those clips first.");
        self.assets.retain(|a| a.id != id);
        Ok(())
    }

    /// A track of `kind` that is free for the whole span, keeping the requested time.
    /// Text and shapes prefer the top; media prefers the bottom ("Main"). If every
    /// track is busy there, a new one is added (overlays above, audio below).
    fn free_track(&mut self, kind: TrackKind, prefer_top: bool, start: f64, duration: f64) -> Id {
        let mut tracks: Vec<Id> = self.tracks.iter().filter(|t| t.kind == kind && !t.locked).map(|t| t.id).collect();
        if !prefer_top && kind == TrackKind::Visual {
            tracks.reverse();
        }
        if let Some(t) = tracks.into_iter().find(|&t| !self.overlaps(t, start, duration, &[])) {
            return t;
        }
        let n = self.tracks.iter().filter(|t| t.kind == kind).count() + 1;
        match kind {
            TrackKind::Visual => self.add_track(kind, &format!("Video {n}"), Some(0)),
            TrackKind::Audio => self.add_track(kind, &format!("Audio {n}"), None),
        }
    }

    /// Places a clip at `start` on `track`, or on a free track of the right kind.
    /// With an explicit track that is busy, the clip moves to the nearest free time.
    pub fn add_clip(&mut self, content: Content, track: Option<Id>, start: f64, duration: f64) -> anyhow::Result<Id> {
        let kind = self.kind_for(&content);
        let prefer_top = !matches!(content, Content::Media { .. });
        let duration = duration.max(MIN_CLIP);
        let start = start.max(0.0);
        let track = match track {
            Some(t) => {
                let tr = self.track(t).context("That track no longer exists")?;
                ensure!(tr.kind == kind, "{} clips go on {} tracks", if kind == TrackKind::Audio { "Audio" } else { "Video" }, if kind == TrackKind::Audio { "audio" } else { "video" });
                ensure!(!tr.locked, "That track is locked");
                t
            }
            None => self.free_track(kind, prefer_top, start, duration),
        };
        let start = self.nearest_free(track, start, duration, &[]);
        let id = self.alloc_id();
        self.clips.push(Clip::new(id, track, start, duration, content));
        Ok(id)
    }

    pub fn add_media_clip(&mut self, asset: Id, track: Option<Id>, start: f64) -> anyhow::Result<Id> {
        self.add_media_clip_still(asset, track, start, DEFAULT_STILL)
    }

    /// Like `add_media_clip`, with images lasting `still` seconds.
    pub fn add_media_clip_still(&mut self, asset: Id, track: Option<Id>, start: f64, still: f64) -> anyhow::Result<Id> {
        let a = self.asset(asset).context("That media is no longer in the library")?;
        let duration = if a.kind == MediaKind::Image { still.max(MIN_CLIP) } else { a.duration };
        let id = self.add_clip(Content::Media { asset, src_in: 0.0, speed: 1.0 }, track, start, duration)?;
        if self.asset(asset).is_some_and(|a| a.kind == MediaKind::Image) {
            // Logos and screenshots usually shouldn't be blown up to the full frame.
            self.clip_mut(id)?.transform.scale = 0.6;
        }
        Ok(id)
    }

    /// Puts a new recording on the timeline at `at`, leaving out its first `lead_in` seconds:
    /// what the camera recorded while the countdown was still running. (The footage is still
    /// there: dragging the clip's left edge brings it back.)
    pub fn add_recording(&mut self, asset: Id, at: f64, lead_in: f64) -> anyhow::Result<Id> {
        let id = self.add_media_clip(asset, None, at)?;
        self.drop_lead_in(id, lead_in)?;
        Ok(id)
    }

    /// Leaves out the first `lead_in` seconds of a media clip (a recording's countdown). Nonsense
    /// values change nothing, and a lead-in longer than the clip keeps a sliver of it.
    fn drop_lead_in(&mut self, id: Id, lead_in: f64) -> anyhow::Result<()> {
        let c = self.clip_mut(id)?;
        let lead = if lead_in.is_finite() { lead_in.clamp(0.0, (c.duration - MIN_CLIP).max(0.0)) } else { 0.0 };
        if let (Content::Media { src_in, .. }, true) = (&mut c.content, lead > 1e-3) {
            *src_in += lead;
            c.duration -= lead;
        }
        Ok(())
    }

    /// Puts a screen recording on the timeline: the picture, the camera (a plain picture in the
    /// bottom-right corner, on a track above the picture) and the microphone, each a clip of its
    /// own, grouped so they move together. `at` on the timeline is where `take.go` lands, so
    /// whatever was recorded before it is left out, and a file that started after it comes in that
    /// much later: picture, face and voice stay exactly where they were, though FFmpeg started them
    /// a fraction of a second apart. Returns the clips: the picture, the camera, the voice.
    pub fn add_screen_recording(&mut self, take: &ScreenTake, at: f64) -> anyhow::Result<Vec<Id>> {
        let finite = |s: f64| if s.is_finite() { s } else { 0.0 };
        let parts: Vec<(Id, f64, bool)> = [Some((take.screen, false)), take.camera.map(|c| (c, true)), take.mic.map(|m| (m, false))].into_iter().flatten().map(|((asset, start), camera)| (asset, finite(start), camera)).collect();
        // The moment that lands on `at`: the end of the countdown, but not before anything exists.
        let earliest = parts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let from = if take.go.is_finite() { take.go.max(earliest) } else { earliest };
        let mut clips: Vec<Id> = Vec::new();
        for (asset, start, camera) in parts {
            let delta = start - from;
            let when = at + delta.max(0.0);
            let id = match clips.first() {
                // The camera goes above the picture so it is drawn on top of it.
                Some(&picture) if camera => {
                    let duration = self.asset(asset).map_or(MIN_CLIP, |a| a.duration);
                    let track = self.track_above(self.clip(picture).context("The recording's picture is gone")?.track, when, duration);
                    self.add_media_clip(asset, Some(track), when)?
                }
                // (With no explicit track a clip keeps its time: a track is added if the free ones are busy.)
                _ => self.add_media_clip(asset, None, when)?,
            };
            self.drop_lead_in(id, (-delta).max(0.0))?;
            if camera {
                self.place_in_corner(id)?;
            }
            clips.push(id);
        }
        if clips.len() > 1 {
            self.group_clips(&clips)?;
        }
        Ok(clips)
    }

    /// The nearest visual track above `below` that is free from `start` for `duration`, or a new one
    /// added right above it.
    fn track_above(&mut self, below: Id, start: f64, duration: f64) -> Id {
        let index = self.track_index(below).unwrap_or(0);
        let free = (0..index).rev().map(|i| &self.tracks[i]).find(|t| t.kind == TrackKind::Visual && !t.locked && !self.overlaps(t.id, start, duration, &[])).map(|t| t.id);
        free.unwrap_or_else(|| self.add_track(TrackKind::Visual, "Camera", Some(index)))
    }

    /// Shrinks a camera clip into the canvas's bottom-right corner, as screen recorders do, so the
    /// screen stays in view. Only its size and place change: the whole picture, square corners, no
    /// border or shadow (styling it is left to the user).
    fn place_in_corner(&mut self, id: Id) -> anyhow::Result<()> {
        let asset = self.clip(id).and_then(|c| c.asset()).and_then(|a| self.asset(a)).context("The camera recording is gone")?;
        let (aw, ah) = (asset.width as f64, asset.height as f64);
        if aw <= 0.0 || ah <= 0.0 {
            return Ok(());
        }
        let (cw, ch) = self.canvas.size();
        let (cw, ch) = (cw as f64, ch as f64);
        let short = cw.min(ch);
        // The picture is first fitted inside the canvas (contain), then scaled so its shorter side is
        // `CAMERA_SIZE` of the canvas's.
        let fit = (cw / aw).min(ch / ah);
        let scale = CAMERA_SIZE * short / (aw.min(ah) * fit);
        let (w, h) = (aw * fit * scale, ah * fit * scale);
        let margin = CAMERA_MARGIN * short;
        let c = self.clip_mut(id)?;
        c.transform.fit = Fit::Contain;
        c.transform.scale = scale;
        c.transform.x = 1.0 - (margin + w / 2.0) / cw;
        c.transform.y = 1.0 - (margin + h / 2.0) / ch;
        Ok(())
    }

    // Keyframes by hand: what the inspector's Animation section and the timeline's diamonds do.
    // Times are clip-relative (`local`), as the keyframes store them.

    /// The clip, if keyframes can be set on it at `local`: unlocked, with a picture, `local` inside it.
    fn keyable(&mut self, id: Id, local: f64) -> anyhow::Result<&mut Clip> {
        self.ensure_unlocked(id)?;
        let c = self.clip(id).context("That clip no longer exists")?;
        ensure!(self.clip_has_picture(c), "Only clips with a picture can be animated");
        ensure!(local.is_finite() && local >= -KEY_EPS && local <= c.duration + KEY_EPS, "Move the playhead onto the clip first");
        self.clip_mut(id)
    }

    /// Adds a keyframe of `prop` at `local`, holding the value the clip has there, or removes the one there.
    pub fn toggle_key(&mut self, id: Id, prop: Prop, local: f64) -> anyhow::Result<()> {
        let c = self.keyable(id, local)?;
        if !c.remove_key(prop, local) {
            let v = c.value_at(prop, local);
            c.put_key(prop, local.clamp(0.0, c.duration), v, None);
        }
        Ok(())
    }

    /// Keyframes every property at `local` as it is there (a pose to animate from or to).
    pub fn key_all(&mut self, id: Id, local: f64) -> anyhow::Result<()> {
        let c = self.keyable(id, local)?;
        let at = local.clamp(0.0, c.duration);
        for prop in Prop::ALL {
            let v = c.value_at(prop, at);
            c.put_key(prop, at, v, None);
        }
        Ok(())
    }

    /// Removes every property's keyframe at `local`.
    pub fn remove_keys_at(&mut self, id: Id, local: f64) -> anyhow::Result<()> {
        let c = self.keyable(id, local)?;
        let mut any = false;
        for prop in Prop::ALL {
            any |= c.remove_key(prop, local);
        }
        ensure!(any, "There is no keyframe there");
        Ok(())
    }

    /// Moves every property's keyframe at `from` to `to` (kept inside the clip). A keyframe already
    /// at `to` gives way to the one moved there.
    pub fn move_keys(&mut self, id: Id, from: f64, to: f64) -> anyhow::Result<()> {
        let c = self.keyable(id, from)?;
        ensure!(to.is_finite(), "That isn't a time");
        let to = to.clamp(0.0, c.duration);
        let mut any = false;
        for keys in c.keyframes.values_mut() {
            let Some(i) = keys.iter().position(|k| (k.t - from).abs() < KEY_EPS) else { continue };
            let mut moved = keys.remove(i);
            moved.t = to;
            keys.retain(|k| (k.t - to).abs() >= KEY_EPS);
            keys.push(moved);
            keys.sort_by(|a, b| a.t.total_cmp(&b.t));
            any = true;
        }
        ensure!(any, "There is no keyframe there");
        Ok(())
    }

    /// How the motion eases from the keyframes at `local` to the next ones.
    pub fn set_keys_ease(&mut self, id: Id, local: f64, ease: crate::clip::Ease) -> anyhow::Result<()> {
        let c = self.keyable(id, local)?;
        let mut any = false;
        for k in c.keyframes.values_mut().flatten().filter(|k| (k.t - local).abs() < KEY_EPS) {
            k.ease = ease;
            any = true;
        }
        ensure!(any, "There is no keyframe there");
        Ok(())
    }

    /// Ends the animation of `prop` (every property for `None`), holding what it shows at `local`.
    pub fn clear_animation(&mut self, id: Id, prop: Option<Prop>, local: f64) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        let c = self.clip_mut(id)?;
        let at = if local.is_finite() { local.clamp(0.0, c.duration) } else { 0.0 };
        c.clear_animation(prop, at);
        Ok(())
    }

    pub fn add_text_clip(&mut self, track: Option<Id>, start: f64, duration: f64, text: TextLayer) -> anyhow::Result<Id> {
        self.add_clip(Content::Text(text), track, start, duration)
    }

    pub fn add_shape_clip(&mut self, track: Option<Id>, start: f64, duration: f64, shape: ShapeLayer) -> anyhow::Result<Id> {
        self.add_clip(Content::Shape(shape), track, start, duration)
    }

    /// A number that counts up or down over its first second, then holds at `to`.
    pub fn add_counter_clip(&mut self, track: Option<Id>, start: f64, duration: f64, counter: crate::clip::CounterLayer) -> anyhow::Result<Id> {
        self.add_clip(Content::Counter(counter), track, start, duration)
    }

    /// A blur box over whatever is below it, from `start` for `duration` seconds.
    pub fn add_blur_clip(&mut self, track: Option<Id>, start: f64, duration: f64, blur: crate::clip::BlurLayer) -> anyhow::Result<Id> {
        self.add_clip(Content::Blur(blur), track, start, duration)
    }

    /// A mask box restricting whatever is below it to only its shape (or, inverted, hiding
    /// its shape), from `start` for `duration` seconds.
    pub fn add_mask_clip(&mut self, track: Option<Id>, start: f64, duration: f64, mask: crate::clip::MaskLayer) -> anyhow::Result<Id> {
        self.add_clip(Content::Mask(mask), track, start, duration)
    }

    /// A glowing icon accent: pops in by default and glows via the clip's own shadow.
    pub fn add_callout_clip(&mut self, track: Option<Id>, start: f64, duration: f64, callout: CalloutLayer) -> anyhow::Result<Id> {
        let color = callout.color;
        let id = self.add_clip(Content::Callout(callout), track, start, duration)?;
        let c = self.clip_mut(id)?;
        c.look.shadow = Shadow { enabled: true, blur: 36.0, offset_y: 0.0, opacity: 0.85, color };
        c.intro = Some(Motion { kind: MotionKind::Pop, duration: 0.35, ease: None, delay: 0.0 });
        Ok(id)
    }

    /// Puts a picture in front of everything else (the top video track) or behind it (the
    /// bottom one), at the same time. A new track is added when that one is busy then.
    pub fn arrange(&mut self, id: Id, front: bool) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?;
        ensure!(self.clip_kind(clip) == TrackKind::Visual, "Only pictures, text and shapes are layered");
        let (start, duration, current) = (clip.start, clip.duration, clip.track);
        let visual: Vec<Id> = self.tracks.iter().filter(|t| t.kind == TrackKind::Visual).map(|t| t.id).collect();
        let edge = if front { visual.first() } else { visual.last() }.copied();
        // Already the edge track: nothing else on it overlaps, so nothing can be in front (or behind).
        if edge == Some(current) {
            return Ok(());
        }
        let target = match edge.filter(|&t| !self.track(t).is_some_and(|t| t.locked) && !self.overlaps(t, start, duration, &[id])) {
            Some(t) => t,
            None => self.new_visual_track(front),
        };
        self.clip_mut(id)?.track = target;
        Ok(())
    }

    /// Moves a picture to a new track on top of all others, at `start` (dropped above the top track).
    pub fn move_to_new_top_track(&mut self, id: Id, start: f64) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?;
        ensure!(self.clip_kind(clip) == TrackKind::Visual, "Only pictures, text and shapes go on video tracks");
        self.detach_transition(id);
        let track = self.new_visual_track(true);
        let clip = self.clip_mut(id)?;
        clip.track = track;
        clip.start = start.max(0.0);
        Ok(())
    }

    /// A new video track above all others (`top`) or below them.
    pub fn new_visual_track(&mut self, top: bool) -> Id {
        let n = self.tracks.iter().filter(|t| t.kind == TrackKind::Visual).count() + 1;
        self.add_track(TrackKind::Visual, &format!("Video {n}"), Some(if top { 0 } else { usize::MAX }))
    }

    /// Moves a clip to `track` at `start`, nudging it to the nearest free spot.
    pub fn move_clip(&mut self, id: Id, track: Id, start: f64) -> anyhow::Result<()> {
        let clip = self.clip(id).context("That clip no longer exists")?;
        let (kind, duration) = (self.clip_kind(clip), clip.duration);
        let tr = self.track(track).context("That track no longer exists")?;
        ensure!(tr.kind == kind, "This clip can't go on that kind of track");
        ensure!(!tr.locked, "That track is locked");
        self.detach_transition(id);
        let start = self.nearest_free(track, start, duration, &[id]);
        let clip = self.clip_mut(id)?;
        clip.track = track;
        clip.start = start;
        Ok(())
    }

    /// Drags one edge of a clip to timeline time `t`, clamped to the media and the neighbours.
    pub fn trim(&mut self, id: Id, edge: Edge, t: f64) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        // Either edge can change the boundary a transition owns; simplest and safest is to
        // always detach rather than track exactly which edge invalidates what.
        self.detach_transition(id);
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let neighbours: Vec<&Clip> = self.clips_on(clip.track).filter(|c| c.id != id).collect();
        let prev_end = neighbours.iter().filter(|c| c.end() <= clip.start + 1e-6).map(|c| c.end()).fold(0.0, f64::max);
        let next_start = neighbours.iter().filter(|c| c.start >= clip.end() - 1e-6).map(|c| c.start).fold(f64::INFINITY, f64::min);
        let speed = clip.speed();
        // Stills (images and freeze frames) have no footage to run out of.
        let still = clip.freeze || clip.asset().and_then(|a| self.asset(a)).is_some_and(|a| a.kind == MediaKind::Image);
        let c = self.clip_mut(id)?;
        match edge {
            Edge::Start => {
                // max/min rather than clamp: neighbours can overlap by a rounding error.
                let mut new_start = t.max(prev_end).min(clip.end() - MIN_CLIP);
                if let (Content::Media { src_in, .. }, false) = (&mut c.content, still) {
                    // Can't reveal footage before the start of the file (unless it's a still image).
                    let earliest = clip.start - *src_in / speed;
                    new_start = new_start.max(earliest);
                    *src_in = (*src_in + (new_start - clip.start) * speed).max(0.0);
                }
                c.duration = clip.end() - new_start;
                c.start = new_start;
                // Keep relative times of zooms and keyframes anchored to the footage.
                let shift = new_start - clip.start;
                for z in &mut c.zooms {
                    z.at -= shift;
                }
                for k in c.keyframes.values_mut().flatten() {
                    k.t -= shift;
                }
                for g in &mut c.audio.gains {
                    g.at -= shift;
                }
                c.audio.gains.retain(|g| g.at + g.duration > 0.0);
                for p in &mut c.presses {
                    p.at -= shift;
                }
            }
            Edge::End => {
                c.duration = (t.min(next_start) - clip.start).max(MIN_CLIP);
            }
        }
        let max = self.max_duration(&self.clip(id).unwrap().clone());
        let c = self.clip_mut(id)?;
        c.duration = c.duration.min(max);
        // Drop or shorten zooms and range volumes that now fall outside the clip.
        let len = c.duration;
        c.zooms.retain(|z| z.at + z.duration > 0.05 && z.at < len - 0.05);
        for z in &mut c.zooms {
            let end = (z.at + z.duration).min(len);
            z.at = z.at.max(0.0);
            z.duration = end - z.at;
        }
        c.audio.gains.retain(|g| g.at + g.duration > 0.01 && g.at < len - 0.01);
        for g in &mut c.audio.gains {
            let end = (g.at + g.duration).min(len);
            g.at = g.at.max(0.0);
            g.duration = end - g.at;
        }
        // Drop presses trimmed off the front entirely; clamp ones only partly cut into.
        c.presses.retain(|p| p.at + p.duration > 0.0 && p.at < len);
        for p in &mut c.presses {
            p.at = p.at.max(0.0);
        }
        // A freeze frame holds its framing for however long it is made.
        if c.freeze {
            for z in &mut c.zooms {
                (z.at, z.duration) = (0.0, len);
            }
        }
        Ok(())
    }

    /// Splits a clip at timeline time `t`. Returns the id of the new right-hand clip.
    pub fn split(&mut self, id: Id, t: f64) -> anyhow::Result<Id> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        ensure!(t > clip.start + MIN_CLIP && t < clip.end() - MIN_CLIP, "Move the playhead inside the clip to split it");
        // Not inside the transition into it or out of it: each piece keeps a moment of its own.
        let half_in = self.transition_into(&clip).map_or(0.0, |(_, tr)| tr.duration / 2.0);
        let half_out = self.transition_out_of(&clip).map_or(0.0, |(_, tr)| tr.duration / 2.0);
        ensure!(t >= clip.start + half_in + MIN_CLIP - 1e-6 && t <= clip.end() - half_out - MIN_CLIP + 1e-6, "Can't split inside a transition");
        let new_id = self.alloc_id();
        let mut right = clip.clone();
        right.id = new_id;
        right.start = t;
        right.duration = clip.end() - t;
        if let (Content::Media { src_in, speed, .. }, false) = (&mut right.content, clip.freeze) {
            *src_in += (t - clip.start) * *speed;
        }
        right.intro = None;
        right.transition = None;
        let cut = t - clip.start;
        right.zooms = clip.zooms.iter().filter(|z| z.at + z.duration > cut + 0.01).map(|z| crate::clip::Zoom { at: z.at - cut, ..*z }).collect();
        for k in right.keyframes.values_mut().flatten() {
            k.t -= cut;
        }
        right.audio.gains = clip.audio.gains.iter().filter(|g| g.at + g.duration > cut + 0.01).map(|g| crate::clip::Gain { at: g.at - cut, ..*g }).collect();
        right.presses = clip.presses.iter().filter(|p| p.at >= cut).map(|p| crate::clip::PressPulse { at: p.at - cut, ..*p }).collect();
        let left = self.clip_mut(id)?;
        left.duration = cut;
        left.outro = None;
        // Each piece keeps only what really reaches into it (not a rounding error's worth).
        left.zooms.retain(|z| z.at < cut - 0.01);
        left.audio.gains.retain(|g| g.at < cut - 0.01);
        left.presses.retain(|p| p.at < cut);
        self.clips.push(right);
        Ok(new_id)
    }

    pub fn delete_clips(&mut self, ids: &[Id]) -> anyhow::Result<()> {
        ensure!(!ids.is_empty(), "Nothing selected");
        for &id in ids {
            self.ensure_unlocked(id)?;
        }
        // Undo any transition touching a deleted clip first, so a surviving partner's position
        // doesn't keep an orphaned overlap pull.
        for &id in ids {
            self.detach_transition(id);
        }
        self.clips.retain(|c| !ids.contains(&c.id));
        self.tidy_groups();
        Ok(())
    }

    /// Changes playback speed, keeping the same footage: the clip gets shorter or longer,
    /// later clips on its track move along so none of it is lost, and zooms and range
    /// volumes stay on the same footage.
    pub fn set_speed(&mut self, id: Id, new_speed: f64) -> anyhow::Result<()> {
        ensure!((0.1..=8.0).contains(&new_speed), "Speed must be between 0.1× and 8×");
        self.ensure_unlocked(id)?;
        // Changing this clip's own duration invalidates any transition touching it; clips merely
        // rippled below keep theirs, since a uniform shift preserves their relative overlap.
        self.detach_transition(id);
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let Content::Media { speed, .. } = clip.content else { bail!("Only video and audio clips have a speed") };
        ensure!(!clip.freeze, "A freeze frame has no speed: drag its edge to make it longer or shorter");
        let ratio = speed / new_speed;
        // Never longer than the footage left at the new speed (a clip at the minimum length can
        // hold less footage than that, and slowing it down must not stretch past the file's end).
        let mut sped = clip.clone();
        if let Content::Media { speed, .. } = &mut sped.content {
            *speed = new_speed;
        }
        let new_duration = (clip.duration * ratio).min(self.max_duration(&sped)).max(MIN_CLIP);
        let delta = new_duration - clip.duration;
        let old_end = clip.end();
        for c in self.clips.iter_mut().filter(|c| c.track == clip.track && c.id != id && c.start >= old_end - 1e-6) {
            c.start = (c.start + delta).max(0.0);
        }
        let c = self.clip_mut(id)?;
        if let Content::Media { speed, .. } = &mut c.content {
            *speed = new_speed;
        }
        c.duration = new_duration;
        for z in &mut c.zooms {
            z.at *= ratio;
            z.duration *= ratio;
        }
        for g in &mut c.audio.gains {
            g.at *= ratio;
            g.duration *= ratio;
        }
        for k in c.keyframes.values_mut().flatten() {
            k.t *= ratio;
        }
        for p in &mut c.presses {
            p.at *= ratio;
            p.duration *= ratio;
        }
        if let Some(m) = &mut c.intro {
            m.duration *= ratio;
            m.delay *= ratio;
        }
        if let Some(m) = &mut c.outro {
            m.duration *= ratio;
        }
        Ok(())
    }

    /// Staggers the intro of each of `ids` (in the order given) so it starts `gap_frames` after
    /// the previous one — clips with no intro motion are left alone.
    pub fn stagger_group(&mut self, ids: &[Id], gap_frames: u32, fps: u32) -> anyhow::Result<()> {
        ensure!(ids.len() > 1, "Select more than one clip to stagger their entrances");
        let gap = gap_frames as f64 / (fps.max(1) as f64);
        for (i, &id) in ids.iter().enumerate() {
            self.ensure_unlocked(id)?;
            if let Some(m) = self.clip_mut(id)?.intro.as_mut() {
                m.delay = i as f64 * gap;
            }
        }
        Ok(())
    }

    /// Shared math behind `push_in`/`pull_back`: a camera move from `at` (clip-relative seconds,
    /// clamped to the clip) to the end of the clip, built from the same zoom mechanism as
    /// `add_zoom` rather than a new primitive.
    fn camera_move(&mut self, clip_id: Id, at: f64, push_in: bool) -> anyhow::Result<()> {
        self.ensure_unlocked(clip_id)?;
        let c = self.clip(clip_id).context("That clip no longer exists")?.clone();
        ensure!(c.asset().is_some() && self.clip_has_picture(&c), "Camera moves go on a video or image clip");
        let local = at.clamp(0.0, c.duration);
        let remaining = c.duration - local;
        ensure!(remaining >= 0.3, "There's no room for a camera move here");
        let ramp = 1.5f64.min(remaining / 2.0).max(0.1);
        if push_in {
            // Ramps in, then holds: the duration runs past the clip's own end so the symmetric
            // ramp-out (built into `camera_at`) never becomes visible.
            let id = self.alloc_id();
            self.clip_mut(clip_id)?.zooms.push(Zoom { id, at: local, duration: remaining + ramp, scale: 1.15, focus_x: 0.5, focus_y: 0.5, ramp });
        } else {
            // A near-instant zoom-in, then an adjacent zoom back to normal: back-to-back zooms
            // pan straight from one to the next, with no return-to-identity dip between them.
            let a_duration = 0.05f64.min(remaining * 0.1).max(0.01);
            let b_remaining = (remaining - a_duration).max(0.1);
            let b_ramp = ramp.min(b_remaining / 2.0).max(0.05);
            let aid = self.alloc_id();
            let bid = self.alloc_id();
            let c = self.clip_mut(clip_id)?;
            c.zooms.push(Zoom { id: aid, at: local, duration: a_duration, scale: 1.3, focus_x: 0.5, focus_y: 0.5, ramp: a_duration / 2.0 });
            c.zooms.push(Zoom { id: bid, at: local + a_duration, duration: b_remaining + b_ramp, scale: 1.0, focus_x: 0.5, focus_y: 0.5, ramp: b_ramp });
        }
        Ok(())
    }

    /// A slow, continuous zoom in from normal to a closer framing, held to the end of the clip.
    pub fn push_in(&mut self, clip_id: Id, at: f64) -> anyhow::Result<()> {
        self.camera_move(clip_id, at, true)
    }

    /// Starts punched in and pulls back to reveal the whole frame, held to the end of the clip.
    pub fn pull_back(&mut self, clip_id: Id, at: f64) -> anyhow::Result<()> {
        self.camera_move(clip_id, at, false)
    }

    /// Moves or resizes a zoom along its clip without overlapping the clip's other zooms.
    pub fn set_zoom_timing(&mut self, clip: Id, zoom: Id, at: f64, duration: f64) -> anyhow::Result<()> {
        let c = self.clip_mut(clip)?;
        let len = c.duration;
        let others: Vec<(f64, f64)> = c.zooms.iter().filter(|z| z.id != zoom).map(|z| (z.at, z.at + z.duration)).collect();
        let z = c.zooms.iter_mut().find(|z| z.id == zoom).context("That zoom no longer exists")?;
        let mut at = at.clamp(0.0, (len - 0.3).max(0.0));
        let mut duration = duration.max(0.3).min(len - at);
        // Stay between the neighbours on either side of where it was dropped.
        let mid = at + duration / 2.0;
        let lo = others.iter().filter(|(_, e)| *e <= mid).map(|(_, e)| *e).fold(0.0, f64::max);
        let hi = others.iter().filter(|(s, _)| *s >= mid).map(|(s, _)| *s).fold(len, f64::min);
        ensure!(hi - lo >= 0.3, "There's no room for the zoom there");
        duration = duration.min(hi - lo);
        at = at.max(lo).min(hi - duration);
        z.at = at;
        z.duration = duration;
        Ok(())
    }

    /// Sets the volume of the part of `clip` between timeline times `a` and `b` (replacing
    /// earlier range changes that overlap it). `volume` 1 removes the change.
    pub fn set_range_volume(&mut self, clip: Id, a: f64, b: f64, volume: f64) -> anyhow::Result<Option<Id>> {
        self.ensure_unlocked(clip)?;
        let id = self.alloc_id();
        let c = self.clip_mut(clip)?;
        let (s, e) = ((a.min(b) - c.start).max(0.0), (a.max(b) - c.start).min(c.duration));
        ensure!(e - s >= 0.05, "The selected range doesn't cover this clip");
        c.audio.gains.retain(|g| g.at + g.duration <= s + 1e-6 || g.at >= e - 1e-6);
        if (volume - 1.0).abs() < 1e-3 {
            return Ok(None);
        }
        c.audio.gains.push(crate::clip::Gain { id, at: s, duration: e - s, volume: volume.clamp(0.0, 4.0) });
        c.audio.gains.sort_by(|x, y| x.at.total_cmp(&y.at));
        Ok(Some(id))
    }

    /// Separates a video clip's sound into its own clip on an audio track, at the same
    /// time and with the same volume settings. The video keeps playing, silently.
    pub fn separate_audio(&mut self, id: Id) -> anyhow::Result<Id> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let Content::Media { asset, .. } = clip.content else { bail!("Only video clips have sound to separate") };
        let a = self.asset(asset).context("That media is no longer in the library")?;
        ensure!(a.kind == MediaKind::Video, "Only video clips have sound to separate");
        ensure!(a.has_audio, "This video has no sound");
        ensure!(clip.part == Part::Whole, "The sound of this clip is already separate");
        let track = self.free_track(TrackKind::Audio, false, clip.start, clip.duration);
        let new_id = self.alloc_id();
        let mut sound = Clip::new(new_id, track, clip.start, clip.duration, clip.content.clone());
        sound.part = Part::Sound;
        sound.audio = clip.audio.clone();
        sound.name = clip.name.as_ref().map(|n| format!("{n} (audio)"));
        if self.track(clip.track).is_some_and(|t| t.hidden) {
            sound.audio.muted = true;
        }
        // The two stay linked: they move, trim and delete together until ungrouped.
        let group = match clip.group {
            Some(g) => g,
            None => self.alloc_id(),
        };
        sound.group = Some(group);
        self.clips.push(sound);
        let video = self.clip_mut(id)?;
        video.part = Part::Picture;
        video.audio = Default::default();
        video.group = Some(group);
        Ok(new_id)
    }

    /// Adds a timeline marker at `at` (a span when `duration` > 0), named "Marker N".
    pub fn add_marker(&mut self, at: f64, duration: f64, color: crate::clip::Rgba) -> Id {
        let id = self.alloc_id();
        let n = self.markers.len() + 1;
        self.markers.push(crate::model::Marker { id, at: at.max(0.0), duration: duration.max(0.0), name: format!("Marker {n}"), color });
        self.markers.sort_by(|a, b| a.at.total_cmp(&b.at));
        id
    }

    pub fn marker_mut(&mut self, id: Id) -> anyhow::Result<&mut crate::model::Marker> {
        self.markers.iter_mut().find(|m| m.id == id).context("That marker no longer exists")
    }

    /// Moves a marker so it starts at `at`, keeping the list in time order.
    pub fn move_marker(&mut self, id: Id, at: f64) -> anyhow::Result<()> {
        self.marker_mut(id)?.at = at.max(0.0);
        self.markers.sort_by(|a, b| a.at.total_cmp(&b.at));
        Ok(())
    }

    pub fn delete_marker(&mut self, id: Id) -> anyhow::Result<()> {
        let before = self.markers.len();
        self.markers.retain(|m| m.id != id);
        ensure!(self.markers.len() < before, "That marker no longer exists");
        Ok(())
    }

    /// Brings transitions saved by earlier versions to how they are now (centred on a cut between
    /// two touching clips). Projects saved while a transition was an overlap had the incoming clip
    /// start `d` seconds before the previous one ended: each becomes a cut in the middle of that
    /// overlap, both clips trimmed back to it, so the transition shows exactly what the overlap did
    /// and nothing else on the timeline moves. (The audio fades the overlap set go: the transition
    /// crossfades the sound itself now.) Touching transitions from before that are already right; a
    /// transition with no clip before it was only ever a fake "animate in" and goes.
    pub(crate) fn migrate_transitions(&mut self) {
        let overlapping: Vec<(Id, Id, f64, f64)> = self
            .clips
            .iter()
            .filter_map(|c| {
                let tr = c.transition?;
                let prev = self.prev_on_track(c)?;
                let overlap = prev.end() - c.start;
                (overlap > 1e-4).then_some((prev.id, c.id, overlap, tr.duration))
            })
            .collect();
        for (prev, id, overlap, d) in overlapping {
            let half = overlap / 2.0;
            if let Ok(p) = self.clip_mut(prev) {
                p.duration = (p.duration - half).max(MIN_CLIP);
                if (p.audio.fade_out - d).abs() < 1e-6 {
                    p.audio.fade_out = 0.0;
                }
            }
            if let Ok(c) = self.clip_mut(id) {
                trim_front(c, half);
                if (c.audio.fade_in - d).abs() < 1e-6 {
                    c.audio.fade_in = 0.0;
                }
            }
        }
        self.tidy_transitions();
    }

    /// Duplicates a clip right after itself (or the nearest free spot).
    pub fn duplicate_clip(&mut self, id: Id) -> anyhow::Result<Id> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let start = self.nearest_free(clip.track, clip.end(), clip.duration, &[]);
        let new_id = self.alloc_id();
        self.clips.push(Clip { id: new_id, start, transition: None, group: None, ..clip });
        Ok(new_id)
    }

    // ------------------------------------------------------------ groups

    /// The clips grouped with `id` (itself included; just itself when it isn't grouped).
    pub fn group_members(&self, id: Id) -> Vec<Id> {
        match self.clip(id).and_then(|c| c.group) {
            Some(g) => self.clips.iter().filter(|c| c.group == Some(g)).map(|c| c.id).collect(),
            None => vec![id],
        }
    }

    /// `ids` plus every clip grouped with one of them, in order and without repeats.
    pub fn with_groups(&self, ids: &[Id]) -> Vec<Id> {
        let mut out: Vec<Id> = Vec::new();
        for &id in ids {
            for m in self.group_members(id) {
                if !out.contains(&m) && self.clip(m).is_some() {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Groups the clips (and whatever they were already grouped with) so they act as one.
    pub fn group_clips(&mut self, ids: &[Id]) -> anyhow::Result<Id> {
        let ids = self.with_groups(ids);
        ensure!(ids.len() > 1, "Select two or more clips to group them");
        let first = self.clip(ids[0]).and_then(|c| c.group);
        if let Some(g) = first.filter(|g| ids.iter().all(|id| self.clip(*id).is_some_and(|c| c.group == Some(*g)))) {
            return Ok(g);
        }
        let g = self.alloc_id();
        for c in self.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
            c.group = Some(g);
        }
        Ok(g)
    }

    /// Takes the clips' groups apart.
    pub fn ungroup_clips(&mut self, ids: &[Id]) -> anyhow::Result<()> {
        let ids = self.with_groups(ids);
        ensure!(ids.iter().any(|id| self.clip(*id).is_some_and(|c| c.group.is_some())), "These clips aren't grouped");
        for c in self.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
            c.group = None;
        }
        Ok(())
    }

    /// A group with one clip left is no group.
    fn tidy_groups(&mut self) {
        let groups: Vec<Id> = self.clips.iter().filter_map(|c| c.group).collect();
        for c in &mut self.clips {
            if c.group.is_some_and(|g| groups.iter().filter(|x| **x == g).count() < 2) {
                c.group = None;
            }
        }
    }

    /// The track each of `ids` lands on when `anchor` is dragged to `track`: every clip moves the
    /// same number of tracks, or (when one of them can't: wrong kind, locked, no such track) none does.
    pub fn move_targets(&self, ids: &[Id], anchor: Id, track: Id) -> Option<Vec<(Id, Id)>> {
        let from = self.track_index(self.clip(anchor)?.track)? as i64;
        let to = self.track_index(track)? as i64;
        let targets_for = |shift: i64| -> Option<Vec<(Id, Id)>> {
            ids.iter()
                .map(|&id| {
                    let c = self.clip(id)?;
                    let t = self.tracks.get(usize::try_from(self.track_index(c.track)? as i64 + shift).ok()?)?;
                    (t.kind == self.clip_kind(c) && !t.locked).then_some((id, t.id))
                })
                .collect()
        };
        targets_for(to - from).or_else(|| targets_for(0))
    }

    /// Moves several clips together: `anchor` (the one being dragged) goes to `track` at `start`,
    /// the others keep their distance from it in time and in tracks. Blocked by other clips,
    /// the whole set lands at the nearest time where all of it fits.
    pub fn move_clips(&mut self, ids: &[Id], anchor: Id, track: Id, start: f64) -> anyhow::Result<()> {
        let ids = self.with_groups(ids);
        if ids.len() <= 1 {
            return self.move_clip(anchor, track, start);
        }
        ensure!(ids.contains(&anchor), "That clip no longer exists");
        for &id in &ids {
            self.ensure_unlocked(id)?;
        }
        // A transition with a clip that stays behind can't survive; pairs moving together keep theirs.
        let broken: Vec<Id> = self
            .clips
            .iter()
            .filter(|c| c.transition.is_some() && ids.contains(&c.id) != self.prev_on_track(c).is_some_and(|p| ids.contains(&p.id)))
            .map(|c| c.id)
            .collect();
        for id in broken {
            self.unapply_transition(id);
        }
        let a = self.clip(anchor).context("That clip no longer exists")?.clone();
        let targets = self.move_targets(&ids, anchor, track).context("These clips can't move there")?;
        let min_start = ids.iter().filter_map(|&id| self.clip(id)).map(|c| c.start).fold(f64::INFINITY, f64::min);
        let wanted = (start - a.start).max(-min_start);
        let fits = |p: &Project, dt: f64| {
            dt >= -min_start - 1e-9 && targets.iter().all(|&(id, tr)| p.clip(id).is_some_and(|c| !p.overlaps(tr, c.start + dt, c.duration, &ids)))
        };
        let dt = if fits(self, wanted) {
            wanted
        } else {
            let mut candidates = Vec::new();
            for &(id, tr) in &targets {
                let Some(c) = self.clip(id) else { continue };
                for o in self.clips_on(tr).filter(|o| !ids.contains(&o.id)) {
                    candidates.push(o.end() - c.start);
                    candidates.push(o.start - c.end());
                }
            }
            candidates.into_iter().filter(|&d| fits(self, d)).min_by(|x, y| (x - wanted).abs().total_cmp(&(y - wanted).abs())).context("There's no room for these clips there")?
        };
        for (id, tr) in targets {
            let c = self.clip_mut(id)?;
            c.track = tr;
            c.start = (c.start + dt).max(0.0);
        }
        Ok(())
    }

    /// Splits several clips at `t`. Right-hand halves that came from one group form a group of
    /// their own, so a linked video and its sound stay linked on both sides of the cut.
    pub fn split_many(&mut self, ids: &[Id], t: f64) -> anyhow::Result<Vec<Id>> {
        let mut rights: Vec<(Id, Option<Id>)> = Vec::new();
        for &id in ids {
            let group = self.clip(id).and_then(|c| c.group);
            rights.push((self.split(id, t)?, group));
        }
        let mut groups: Vec<Id> = rights.iter().filter_map(|r| r.1).collect();
        groups.sort_unstable();
        groups.dedup();
        for g in groups {
            let members: Vec<Id> = rights.iter().filter(|r| r.1 == Some(g)).map(|r| r.0).collect();
            if members.len() > 1 {
                let new_group = self.alloc_id();
                for m in members {
                    self.clip_mut(m)?.group = Some(new_group);
                }
            }
        }
        Ok(rights.into_iter().map(|r| r.0).collect())
    }

    // ------------------------------------------------------------ ripple edits and gaps

    /// Deletes clips and closes the space each one leaves: later clips on its track move left.
    pub fn ripple_delete(&mut self, ids: &[Id]) -> anyhow::Result<()> {
        ensure!(!ids.is_empty(), "Nothing selected");
        for &id in ids {
            self.ensure_unlocked(id)?;
        }
        for &id in ids {
            self.detach_transition(id);
        }
        let removed: Vec<(Id, f64, f64)> = ids.iter().filter_map(|&id| self.clip(id)).map(|c| (c.track, c.start, c.duration)).collect();
        self.clips.retain(|c| !ids.contains(&c.id));
        for c in &mut self.clips {
            let shift: f64 = removed.iter().filter(|(track, start, _)| *track == c.track && *start < c.start + 1e-6).map(|r| r.2).sum();
            c.start = (c.start - shift).max(0.0);
        }
        self.tidy_groups();
        Ok(())
    }

    /// Closes the gaps between the clips on `track` (its first clip stays where it is).
    /// Returns how many seconds of empty space went away.
    pub fn close_track_gaps(&mut self, track: Id) -> anyhow::Result<f64> {
        ensure!(!self.track(track).context("That track no longer exists")?.locked, "That track is locked");
        let mut order: Vec<(Id, f64, f64)> = self.clips_on(track).map(|c| (c.id, c.start, c.duration)).collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (mut shift, mut prev_end) = (0.0, None::<f64>);
        for (id, start, duration) in order {
            let mut s = start - shift;
            if let Some(gap) = prev_end.map(|e| s - e).filter(|g| *g > 1e-6) {
                shift += gap;
                s -= gap;
            }
            prev_end = Some(prev_end.map_or(s + duration, |e| e.max(s + duration)));
            self.clip_mut(id)?.start = s;
        }
        Ok(shift)
    }

    /// Closes the gap on `track` around time `t` (before the first clip too): everything after
    /// it on that track moves left. Returns the seconds removed.
    pub fn close_gap_at(&mut self, track: Id, t: f64) -> anyhow::Result<f64> {
        ensure!(!self.track(track).context("That track no longer exists")?.locked, "That track is locked");
        ensure!(!self.clips_on(track).any(|c| t > c.start + 1e-6 && t < c.end() - 1e-6), "There's no gap here");
        let before = self.clips_on(track).filter(|c| c.end() <= t + 1e-6).map(Clip::end).fold(0.0, f64::max);
        let after = self.clips_on(track).filter(|c| c.start >= t - 1e-6).map(|c| c.start).fold(f64::INFINITY, f64::min);
        ensure!(after.is_finite() && after - before > 1e-6, "There's no gap here");
        let gap = after - before;
        for c in self.clips.iter_mut().filter(|c| c.track == track && c.start >= after - 1e-6) {
            c.start -= gap;
        }
        Ok(gap)
    }

    /// Removes every stretch of time where no track has anything (the start of the timeline
    /// included). All later clips and markers move left together, so everything stays in sync.
    /// Returns the seconds removed.
    pub fn close_empty_time(&mut self) -> anyhow::Result<f64> {
        let mut spans: Vec<(f64, f64)> = self.clips.iter().map(|c| (c.start, c.end())).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut gaps, mut cursor) = (Vec::new(), 0.0f64);
        for (s, e) in spans {
            if s - cursor > 1e-6 {
                gaps.push((cursor, s - cursor));
            }
            cursor = cursor.max(e);
        }
        if gaps.is_empty() {
            return Ok(0.0);
        }
        ensure!(!self.clips.iter().any(|c| self.track(c.track).is_some_and(|t| t.locked)), "Unlock every track first: closing these gaps moves all of them together");
        let removed_before = |t: f64| gaps.iter().map(|&(start, len)| (t - start).clamp(0.0, len)).sum::<f64>();
        for c in &mut self.clips {
            c.start = (c.start - removed_before(c.start + 1e-6)).max(0.0);
        }
        for m in &mut self.markers {
            m.at = (m.at - removed_before(m.at)).max(0.0);
        }
        Ok(gaps.iter().map(|g| g.1).sum())
    }

    /// Cuts the time between `a` and `b` out of every track: clips are split at both ends, what
    /// lies between is deleted, and everything after moves left to close the hole.
    pub fn ripple_delete_range(&mut self, a: f64, b: f64) -> anyhow::Result<()> {
        let (a, b) = (a.min(b).max(0.0), a.max(b));
        ensure!(b - a >= 0.01, "Mark a range on the timeline first");
        ensure!(self.clips.iter().any(|c| c.end() > a + 1e-6), "There's nothing in that range");
        ensure!(
            !self.clips.iter().any(|c| c.end() > a + 1e-6 && self.track(c.track).is_some_and(|t| t.locked)),
            "Unlock every track first: cutting a range moves all of them together"
        );
        for edge in [a, b] {
            let ids: Vec<Id> = self.clips.iter().filter(|c| edge > c.start + MIN_CLIP && edge < c.end() - MIN_CLIP).map(|c| c.id).collect();
            self.split_many(&ids, edge)?;
        }
        // An edge too close to a clip's end to split there takes that sliver along.
        let inside = |c: &Clip| {
            let mid = c.start + c.duration / 2.0;
            mid > a && mid < b && c.start >= a - MIN_CLIP - 1e-6 && c.end() <= b + MIN_CLIP + 1e-6
        };
        let doomed: Vec<Id> = self.clips.iter().filter(|c| inside(c)).map(|c| c.id).collect();
        // The first clip after the range on each track tells how far that track has to move
        // (deleting a clip can undo a transition, which shifts things by itself).
        let anchors: Vec<(Id, Id, f64)> = self
            .tracks
            .iter()
            .filter_map(|t| {
                self.clips_on(t.id).filter(|c| !doomed.contains(&c.id) && c.start >= b - MIN_CLIP - 1e-6).min_by(|x, y| x.start.total_cmp(&y.start)).map(|c| (t.id, c.id, c.start))
            })
            .collect();
        if !doomed.is_empty() {
            self.delete_clips(&doomed)?;
        }
        for (track, anchor, old_start) in anchors {
            let now = self.clip(anchor).map(|c| c.start).unwrap_or(old_start);
            let prev_end = self.clips_on(track).filter(|c| c.start < now - 1e-6).map(Clip::end).fold(0.0, f64::max);
            let target = (old_start - (b - a)).max(prev_end).max(0.0);
            let shift = now - target;
            for c in self.clips.iter_mut().filter(|c| c.track == track && c.start >= now - 1e-6) {
                c.start -= shift;
            }
        }
        self.markers.retain(|m| m.at < a || m.at >= b);
        for m in self.markers.iter_mut().filter(|m| m.at >= b) {
            m.at -= b - a;
        }
        self.tidy_groups();
        Ok(())
    }

    // ------------------------------------------------------------ copy, paste, duplicate

    /// Pastes copies of `clips` so the earliest starts at `at`, the others keeping their distance
    /// from it. Each goes back on its own track when that is free there, otherwise on a free one.
    pub fn paste_clips(&mut self, clips: &[Clip], at: f64) -> anyhow::Result<Vec<Id>> {
        ensure!(!clips.is_empty(), "Nothing to paste");
        let base = clips.iter().map(|c| c.start).fold(f64::INFINITY, f64::min);
        let mut order: Vec<&Clip> = clips.iter().collect();
        order.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut pasted: Vec<(Id, Id)> = Vec::new();
        let mut groups: Vec<(Id, Id)> = Vec::new();
        for src in order {
            if let Some(asset) = src.asset() {
                ensure!(self.asset(asset).is_some(), "The media of a copied clip is no longer in the library");
            }
            let kind = self.clip_kind(src);
            let start = (at + (src.start - base)).max(0.0);
            let mut c = src.clone();
            c.id = self.alloc_id();
            c.start = start;
            // A transition survives when the clip it blends from was pasted right before it.
            let partner = src.transition.and_then(|tr| {
                let prev = clips.iter().filter(|p| p.track == src.track && p.id != src.id && p.start < src.start + 1e-6).max_by(|x, y| x.start.total_cmp(&y.start))?;
                let new_prev = self.clip(pasted.iter().find(|(old, _)| *old == prev.id)?.1)?;
                let _ = tr;
                let fits = (new_prev.end() - start).abs() < 1e-3 && !self.overlaps(new_prev.track, start, src.duration, &[]);
                fits.then_some(new_prev.track)
            });
            c.track = match partner {
                Some(track) => track,
                None => {
                    c.transition = None;
                    let own = self.track(src.track).filter(|t| t.kind == kind && !t.locked && !self.overlaps(t.id, start, src.duration, &[])).map(|t| t.id);
                    match own {
                        Some(t) => t,
                        None => self.free_track(kind, !matches!(src.content, Content::Media { .. }), start, src.duration),
                    }
                }
            };
            c.group = match src.group.filter(|g| clips.iter().filter(|x| x.group == Some(*g)).count() > 1) {
                Some(g) => Some(match groups.iter().find(|(old, _)| *old == g) {
                    Some((_, new)) => *new,
                    None => {
                        let new = self.alloc_id();
                        groups.push((g, new));
                        new
                    }
                }),
                None => None,
            };
            pasted.push((src.id, c.id));
            self.clips.push(c);
        }
        Ok(pasted.into_iter().map(|(_, new)| new).collect())
    }

    /// Duplicates clips: one clip goes right after itself; several are copied as a set (keeping
    /// their layout and groups) right after the last of them.
    pub fn duplicate_clips(&mut self, ids: &[Id]) -> anyhow::Result<Vec<Id>> {
        ensure!(!ids.is_empty(), "Nothing selected");
        if let [id] = ids {
            return Ok(vec![self.duplicate_clip(*id)?]);
        }
        for &id in ids {
            self.ensure_unlocked(id)?;
        }
        let clones: Vec<Clip> = ids.iter().filter_map(|&id| self.clip(id)).cloned().collect();
        let end = clones.iter().map(Clip::end).fold(0.0, f64::max);
        self.paste_clips(&clones, end)
    }

    /// Copies one clip's effects onto others: frame, color and animations for pictures;
    /// volume, EQ, compressor and noise removal for sound.
    pub fn paste_effects(&mut self, from: &Clip, ids: &[Id]) -> anyhow::Result<()> {
        ensure!(!ids.is_empty(), "Select a clip to paste the effects onto");
        let styled = !matches!(from.content, Content::Text(_) | Content::Counter(_) | Content::Blur(_) | Content::Mask(_));
        for &id in ids {
            self.ensure_unlocked(id)?;
            let (picture, sound) = {
                let c = self.clip(id).context("That clip no longer exists")?;
                (self.clip_has_picture(c), self.clip_has_sound(c))
            };
            let c = self.clip_mut(id)?;
            if picture && !matches!(c.content, Content::Blur(_) | Content::Mask(_)) {
                if styled {
                    c.look = from.look.clone();
                }
                c.grade = from.grade;
                let half = c.duration / 2.0;
                c.intro = from.intro.map(|m| Motion { duration: m.duration.min(half), ..m });
                c.outro = from.outro.map(|m| Motion { duration: m.duration.min(half), ..m });
                c.idle_drift = from.idle_drift;
            }
            if sound {
                c.audio.volume = from.audio.volume;
                c.audio.eq = from.audio.eq;
                c.audio.compressor = from.audio.compressor;
                c.audio.denoise = from.audio.denoise;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ clip states

    /// Turns clips off (kept on the timeline, but neither shown nor heard) or back on.
    pub fn set_disabled(&mut self, ids: &[Id], disabled: bool) -> anyhow::Result<()> {
        ensure!(!ids.is_empty(), "Nothing selected");
        for &id in ids {
            self.ensure_unlocked(id)?;
            self.clip_mut(id)?.disabled = disabled;
        }
        Ok(())
    }

    /// Holds the frame of video clip `id` at timeline time `t` for `duration` seconds: the clip
    /// is split there, a still of that frame goes in between, and everything after it on the
    /// track (and on the tracks of clips grouped with it) moves right to make room.
    pub fn freeze_frame(&mut self, id: Id, t: f64, duration: f64) -> anyhow::Result<Id> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let Content::Media { asset, .. } = clip.content else { bail!("Freeze frames go on video clips") };
        ensure!(self.asset(asset).is_some_and(|a| a.kind == MediaKind::Video) && self.clip_has_picture(&clip) && !clip.freeze, "Freeze frames go on video clips");
        ensure!(t >= clip.start - 1e-6 && t <= clip.end() + 1e-6, "Move the playhead over the clip to freeze a frame");
        let duration = duration.max(MIN_CLIP);
        // Too close to an end to split there: the still goes before or after the whole clip.
        let at = if t <= clip.start + MIN_CLIP {
            clip.start
        } else if t >= clip.end() - MIN_CLIP {
            clip.end()
        } else {
            t
        };
        let mates = self.group_members(id);
        let mut tracks: Vec<Id> = mates.iter().filter_map(|m| self.clip(*m)).map(|c| c.track).collect();
        tracks.sort_unstable();
        tracks.dedup();
        ensure!(!tracks.iter().any(|t| self.track(*t).is_some_and(|t| t.locked)), "A track of this group is locked");
        // Making room inside a transition (it spans the cut, half on each side) would tear the pair apart.
        ensure!(
            !self.clips.iter().any(|c| tracks.contains(&c.track) && self.transition_into(c).is_some_and(|(_, tr)| at >= c.start - tr.duration / 2.0 - 1e-6 && at <= c.start + tr.duration / 2.0 + 1e-6)),
            "Can't freeze a frame inside a transition"
        );
        let to_split: Vec<Id> = mates.into_iter().filter(|m| self.clip(*m).is_some_and(|c| at > c.start + MIN_CLIP && at < c.end() - MIN_CLIP)).collect();
        self.split_many(&to_split, at)?;
        for c in self.clips.iter_mut().filter(|c| tracks.contains(&c.track) && c.start >= at - 1e-6) {
            c.start += duration;
        }
        // The still looks exactly like the video did at that moment.
        let local = at - clip.start;
        let new_id = self.alloc_id();
        let mut still = Clip::new(new_id, clip.track, at, duration, Content::Media { asset, src_in: clip.source_time(at), speed: 1.0 });
        still.freeze = true;
        still.part = Part::Picture;
        still.name = Some("Freeze frame".into());
        still.transform = clip.transform;
        for (prop, keys) in &clip.keyframes {
            let Some(v) = crate::clip::sample(keys, local) else { continue };
            match prop {
                crate::clip::Prop::X => still.transform.x = v,
                crate::clip::Prop::Y => still.transform.y = v,
                crate::clip::Prop::Scale => still.transform.scale = v,
                crate::clip::Prop::Rotation => still.transform.rotation = v,
                crate::clip::Prop::Opacity => still.transform.opacity = v,
            }
        }
        (still.look, still.grade, still.crop) = (clip.look.clone(), clip.grade, clip.crop);
        let camera = crate::clip::camera_at(&clip.zooms, local);
        if camera != crate::clip::Camera::IDENTITY {
            let zoom = self.alloc_id();
            still.zooms.push(Zoom { id: zoom, at: 0.0, duration, scale: camera.scale, focus_x: camera.focus_x, focus_y: camera.focus_y, ramp: 0.0 });
        }
        self.clips.push(still);
        Ok(new_id)
    }

    /// Points a clip at other media, keeping its edits. A shorter file shortens the clip.
    pub fn replace_media(&mut self, id: Id, asset: Id) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        let clip = self.clip(id).context("That clip no longer exists")?.clone();
        let Content::Media { src_in, speed, .. } = clip.content else { bail!("Only video, image and audio clips can have their media replaced") };
        let a = self.asset(asset).context("That media is no longer in the library")?.clone();
        let on_audio_track = self.clip_kind(&clip) == TrackKind::Audio;
        if on_audio_track {
            ensure!(a.has_audio && a.kind != MediaKind::Image, "Pick a file with sound for this clip");
        } else {
            ensure!(a.is_visual(), "Pick a video or image for this clip");
        }
        let still = a.kind == MediaKind::Image;
        let src_in = if still || src_in >= a.duration - MIN_CLIP { 0.0 } else { src_in };
        let c = self.clip_mut(id)?;
        c.content = Content::Media { asset, src_in, speed: if still { 1.0 } else { speed } };
        if still {
            c.freeze = false;
        }
        // Sound from a video file stays on its audio track as sound only.
        if on_audio_track && a.kind == MediaKind::Video {
            c.part = Part::Sound;
        }
        let max = self.max_duration(&self.clip(id).unwrap().clone());
        if clip.duration > max + 1e-6 {
            self.trim(id, Edge::End, clip.start + max)?;
        }
        Ok(())
    }

    /// Plays clip `id` from `asset`: a backwards copy of the footage it showed. Zooms, range
    /// volumes, press pulses and fades flip with it, so they stay on the same footage.
    pub fn use_reversed(&mut self, id: Id, asset: Id) -> anyhow::Result<()> {
        self.ensure_unlocked(id)?;
        let length = self.asset(asset).context("That media is no longer in the library")?.duration;
        let c = self.clip_mut(id)?;
        let Content::Media { speed, .. } = c.content else { bail!("Only video and audio clips can be reversed") };
        c.content = Content::Media { asset, src_in: 0.0, speed };
        let len = c.duration;
        for z in &mut c.zooms {
            z.at = len - z.at - z.duration;
        }
        for g in &mut c.audio.gains {
            g.at = len - g.at - g.duration;
        }
        c.audio.gains.sort_by(|x, y| x.at.total_cmp(&y.at));
        for p in &mut c.presses {
            p.at = len - p.at - p.duration;
        }
        std::mem::swap(&mut c.audio.fade_in, &mut c.audio.fade_out);
        // The copy can come out a frame shorter than the part it was made from.
        c.duration = len.min((length / speed).max(MIN_CLIP));
        Ok(())
    }
}

/// Moves a clip's start `shift` seconds later, keeping the rest of it where it was: its footage,
/// zooms, keyframes, volume ranges and press pulses stay on the same moments.
fn trim_front(c: &mut Clip, shift: f64) {
    let shift = shift.clamp(0.0, (c.duration - MIN_CLIP).max(0.0));
    if let (Content::Media { src_in, speed, .. }, false) = (&mut c.content, c.freeze) {
        *src_in += shift * *speed;
    }
    c.start += shift;
    c.duration -= shift;
    for z in &mut c.zooms {
        z.at -= shift;
    }
    c.zooms.retain(|z| z.at + z.duration > 0.05);
    for z in &mut c.zooms {
        let end = z.at + z.duration;
        z.at = z.at.max(0.0);
        z.duration = end - z.at;
    }
    for k in c.keyframes.values_mut().flatten() {
        k.t -= shift;
    }
    for g in &mut c.audio.gains {
        g.at -= shift;
    }
    c.audio.gains.retain(|g| g.at + g.duration > 0.01);
    for g in &mut c.audio.gains {
        let end = g.at + g.duration;
        g.at = g.at.max(0.0);
        g.duration = end - g.at;
    }
    for p in &mut c.presses {
        p.at -= shift;
    }
    c.presses.retain(|p| p.at >= 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Canvas;
    use std::path::PathBuf;

    fn project_with_video(len: f64) -> (Project, Id) {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Video, duration: len, width: 1920, height: 1080, fps: 30.0, has_audio: true };
        let a = p.add_asset(&PathBuf::from("/v.mp4"), &info);
        (p, a)
    }

    #[test]
    fn new_clips_keep_their_time_on_a_free_track() {
        let (mut p, a) = project_with_video(10.0);
        let c1 = p.add_media_clip(a, None, 0.0).unwrap();
        let c2 = p.add_media_clip(a, None, 3.0).unwrap();
        let (k1, k2) = (p.clip(c1).unwrap().clone(), p.clip(c2).unwrap().clone());
        assert_eq!(p.track(k1.track).unwrap().name, "Main");
        assert_eq!(k2.start, 3.0, "time is kept");
        assert_ne!(k1.track, k2.track, "the overlapping clip goes on the next free video track");
        // With every video track busy, a new one appears on top.
        let tracks = p.tracks.len();
        for _ in 0..3 {
            p.add_media_clip(a, None, 3.0).unwrap();
        }
        assert!(p.tracks.len() > tracks);
        assert_eq!(p.tracks[0].kind, TrackKind::Visual);
        // An explicit busy track still nudges the time instead.
        let c = p.add_media_clip(a, Some(k1.track), 1.0).unwrap();
        assert_eq!(p.clip(c).unwrap().start, 10.0);
    }

    #[test]
    fn a_recording_lands_at_the_playhead_without_its_countdown() {
        let (mut p, a) = project_with_video(10.0);
        let id = p.add_recording(a, 4.0, 1.5).unwrap();
        let c = p.clip(id).unwrap();
        assert_eq!((c.start, c.duration), (4.0, 8.5));
        assert!(matches!(c.content, Content::Media { src_in, .. } if src_in == 1.5));
        // The left-out part is still there to drag back.
        p.trim(id, Edge::Start, 0.0).unwrap();
        assert_eq!(p.clip(id).unwrap().start, 2.5);
        // Nonsense lead-ins change nothing, and one longer than the recording keeps a sliver.
        let plain = p.add_recording(a, 20.0, f64::NAN).unwrap();
        assert_eq!(p.clip(plain).unwrap().duration, 10.0);
        let sliver = p.add_recording(a, 40.0, 99.0).unwrap();
        assert!((p.clip(sliver).unwrap().duration - MIN_CLIP).abs() < 1e-9);
        assert!(broken_rule(&p).is_none());
    }

    #[test]
    fn a_screen_recording_and_its_voice_land_in_step_as_one_group() {
        let mut p = Project::new("t", Canvas::default());
        let video = ProbeInfo { kind: MediaKind::Video, duration: 10.0, width: 3200, height: 2000, fps: 30.0, has_audio: false };
        let voice = ProbeInfo { kind: MediaKind::Audio, duration: 10.4, width: 0, height: 0, fps: 0.0, has_audio: true };
        let screen = p.add_asset(&PathBuf::from("/Screen recording 1.mp4"), &video);
        let mic = p.add_asset(&PathBuf::from("/Screen recording 1 microphone.m4a"), &voice);
        // On the take's clock the voice began at 1.0 s and the picture at 1.43 s; the countdown ended at 3.0 s.
        let ids = p.add_screen_recording(&ScreenTake { screen: (screen, 1.43), camera: None, mic: Some((mic, 1.0)), go: 3.0 }, 5.0).unwrap();
        let (s, m) = (p.clip(ids[0]).unwrap(), p.clip(ids[1]).unwrap());
        // Both leave out what came before the end of the countdown, so they start together at the playhead...
        assert_eq!((s.start, m.start), (5.0, 5.0));
        assert!(matches!(s.content, Content::Media { src_in, .. } if (src_in - 1.57).abs() < 1e-9), "{:?}", s.content);
        assert!(matches!(m.content, Content::Media { src_in, .. } if (src_in - 2.0).abs() < 1e-9), "{:?}", m.content);
        // ...and the same moment of the take plays at the same moment: 3.0 s on the take is 1.57 s into the picture and 2.0 s into the voice.
        assert!((s.duration - 8.43).abs() < 1e-9 && (m.duration - 8.4).abs() < 1e-9);
        assert_eq!((p.track(s.track).unwrap().kind, p.track(m.track).unwrap().kind), (TrackKind::Visual, TrackKind::Audio));
        assert!(s.group.is_some() && s.group == m.group);
        assert!(broken_rule(&p).is_none());
        // A take whose files started after the countdown ended: they come in later, still in step.
        let late = p.add_screen_recording(&ScreenTake { screen: (screen, 4.5), camera: None, mic: Some((mic, 4.0)), go: 3.0 }, 30.0).unwrap();
        let (s, m) = (p.clip(late[0]).unwrap(), p.clip(late[1]).unwrap());
        // The picture starts 0.5 s after the voice, and neither is trimmed: the voice is the earliest part, so it starts at `at`.
        assert_eq!((s.start, m.start), (30.5, 30.0));
        assert!(matches!(s.content, Content::Media { src_in, .. } if src_in == 0.0) && matches!(m.content, Content::Media { src_in, .. } if src_in == 0.0));
        // Without a microphone it is just the picture, and a busy spot gets another track, at the same time.
        let alone = p.add_screen_recording(&ScreenTake { screen: (screen, 1.0), camera: None, mic: None, go: 1.0 }, 6.0).unwrap();
        assert_eq!(alone.len(), 1);
        assert_eq!(p.clip(alone[0]).unwrap().start, 6.0);
        assert!(p.clip(alone[0]).unwrap().group.is_none());
        // Nonsense clock values change nothing but the lead-in.
        let odd = p.add_screen_recording(&ScreenTake { screen: (screen, f64::NAN), camera: None, mic: Some((mic, f64::NAN)), go: f64::NAN }, 60.0).unwrap();
        assert_eq!((p.clip(odd[0]).unwrap().start, p.clip(odd[1]).unwrap().start), (60.0, 60.0));
        assert!(broken_rule(&p).is_none());
    }

    #[test]
    fn a_camera_lands_as_a_plain_picture_in_the_corner_above_the_screen_in_the_same_group() {
        let mut p = Project::new("t", Canvas::default());
        let (cw, ch) = p.canvas.size();
        let video = ProbeInfo { kind: MediaKind::Video, duration: 10.0, width: 3200, height: 2000, fps: 30.0, has_audio: false };
        let face = ProbeInfo { kind: MediaKind::Video, duration: 10.2, width: 1920, height: 1080, fps: 30.0, has_audio: false };
        let voice = ProbeInfo { kind: MediaKind::Audio, duration: 10.4, width: 0, height: 0, fps: 0.0, has_audio: true };
        let screen = p.add_asset(&PathBuf::from("/Screen recording 1.mp4"), &video);
        let camera = p.add_asset(&PathBuf::from("/Screen recording 1 camera.mp4"), &face);
        let mic = p.add_asset(&PathBuf::from("/Screen recording 1 microphone.m4a"), &voice);
        let ids = p.add_screen_recording(&ScreenTake { screen: (screen, 1.4), camera: Some((camera, 1.2)), mic: Some((mic, 1.0)), go: 3.0 }, 2.0).unwrap();
        assert_eq!(ids.len(), 3);
        let (s, c, m) = (p.clip(ids[0]).unwrap().clone(), p.clip(ids[1]).unwrap().clone(), p.clip(ids[2]).unwrap().clone());
        // In step: all three leave out the countdown and start at the playhead.
        assert_eq!((s.start, c.start, m.start), (2.0, 2.0, 2.0));
        assert!(matches!(c.content, Content::Media { src_in, .. } if (src_in - 1.8).abs() < 1e-9), "{:?}", c.content);
        // One group, the camera drawn on top (tracks[0] is drawn last, on top).
        assert!(s.group.is_some() && s.group == c.group && c.group == m.group);
        assert!(p.track_index(c.track).unwrap() < p.track_index(s.track).unwrap());
        // The whole picture, untouched: no crop, square corners, no border or shadow (the user styles it).
        assert_eq!((c.crop, c.look), (crate::clip::Crop::default(), crate::clip::Look::default()));
        // 28% of the canvas's short side tall, in the bottom-right corner, inside the canvas.
        let fit = (cw as f64 / 1920.0).min(ch as f64 / 1080.0);
        let (w, h) = (1920.0 * fit * c.transform.scale, 1080.0 * fit * c.transform.scale);
        assert!((h / (cw.min(ch) as f64) - 0.28).abs() < 1e-6, "{h}");
        let (right, bottom) = (c.transform.x * cw as f64 + w / 2.0, c.transform.y * ch as f64 + h / 2.0);
        let margin = 0.045 * cw.min(ch) as f64;
        assert!((cw as f64 - right - margin).abs() < 1e-6 && (ch as f64 - bottom - margin).abs() < 1e-6, "{right} {bottom}");
        // The screen picture itself is untouched.
        assert_eq!((s.transform.scale, s.look.corner_radius, s.crop), (1.0, 0.0, crate::clip::Crop::default()));
        // Another take where the track above is busy gets a new track, still above its picture.
        let again = p.add_screen_recording(&ScreenTake { screen: (screen, 1.0), camera: Some((camera, 1.0)), mic: None, go: 1.0 }, 4.0).unwrap();
        let (s2, c2) = (p.clip(again[0]).unwrap(), p.clip(again[1]).unwrap());
        assert!(p.track_index(c2.track).unwrap() < p.track_index(s2.track).unwrap());
        assert!(broken_rule(&p).is_none());
        // One undo step in the editor: it's one edit here.
    }

    #[test]
    fn keyframes_by_hand_animate_and_never_hide_a_change() {
        use crate::clip::Ease;
        let mut p = Project::new("t", Canvas::default());
        let id = p.add_text_clip(None, 0.0, 4.0, TextLayer::default()).unwrap();
        let keys = |p: &Project, prop: Prop| p.clip(id).unwrap().keyframes.get(&prop).map(|k| k.iter().map(|k| (k.t, k.value)).collect::<Vec<_>>()).unwrap_or_default();
        // Before any keyframe, a change is the clip's own value.
        p.clip_mut(id).unwrap().set_value_at(Prop::X, 1.0, 0.3);
        assert_eq!((p.clip(id).unwrap().transform.x, keys(&p, Prop::X)), (0.3, vec![]));
        // A keyframe at 1 s holds where it is; moving the clip at 3 s adds the second one.
        p.toggle_key(id, Prop::X, 1.0).unwrap();
        p.clip_mut(id).unwrap().set_value_at(Prop::X, 3.0, 0.7);
        assert_eq!(keys(&p, Prop::X), [(1.0, 0.3), (3.0, 0.7)]);
        let c = p.clip(id).unwrap();
        assert_eq!((c.value_at(Prop::X, 0.0), c.value_at(Prop::X, 2.0), c.value_at(Prop::X, 3.0), c.value_at(Prop::X, 4.0)), (0.3, 0.5, 0.7, 0.7));
        // Changing it again on (nearly) the same frame updates that keyframe, it doesn't add one.
        p.clip_mut(id).unwrap().set_value_at(Prop::X, 3.005, 0.8);
        assert_eq!(keys(&p, Prop::X), [(1.0, 0.3), (3.0, 0.8)]);
        // A pose at 2 s: every property keyed there, each as it is there.
        p.key_all(id, 2.0).unwrap();
        assert_eq!(p.clip(id).unwrap().key_times(), [1.0, 2.0, 3.0]);
        assert!(Prop::ALL.iter().all(|&prop| p.clip(id).unwrap().animated(prop)));
        assert_eq!(keys(&p, Prop::Scale), [(2.0, 1.0)]);
        // The moment's keyframes move together, ease together and, moved onto another, replace it.
        p.move_keys(id, 2.0, 2.5).unwrap();
        assert_eq!(p.clip(id).unwrap().key_times(), [1.0, 2.5, 3.0]);
        p.set_keys_ease(id, 2.5, Ease::Linear).unwrap();
        assert!(p.clip(id).unwrap().keyframes.values().flatten().filter(|k| k.t == 2.5).all(|k| k.ease == Ease::Linear));
        p.move_keys(id, 2.5, 3.0).unwrap();
        assert_eq!(p.clip(id).unwrap().key_times(), [1.0, 3.0]);
        assert_eq!(keys(&p, Prop::X).len(), 2);
        // Toggling off a property's last keyframe leaves it where that keyframe had it.
        p.toggle_key(id, Prop::Scale, 3.0).unwrap();
        assert!(!p.clip(id).unwrap().animated(Prop::Scale));
        // Keyframes stay inside the clip: one dragged past its end lands on it.
        p.move_keys(id, 3.0, 9.0).unwrap();
        assert_eq!(p.clip(id).unwrap().key_times(), [1.0, 4.0]);
        // "Remove animation" holds what was shown at the playhead.
        let shown = p.clip(id).unwrap().value_at(Prop::X, 2.0);
        p.clear_animation(id, Some(Prop::X), 2.0).unwrap();
        assert_eq!((p.clip(id).unwrap().animated(Prop::X), p.clip(id).unwrap().transform.x), (false, shown));
        p.clear_animation(id, None, 0.0).unwrap();
        assert!(p.clip(id).unwrap().keyframes.is_empty());
        // Refused: nothing at that time, the playhead off the clip, a sound-only clip, a locked track.
        assert!(p.remove_keys_at(id, 1.5).is_err() && p.move_keys(id, 1.5, 2.0).is_err() && p.set_keys_ease(id, 1.5, Ease::Back).is_err());
        assert!(p.toggle_key(id, Prop::X, 5.0).is_err() && p.key_all(id, -1.0).is_err());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 5.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let a = p.add_asset(&PathBuf::from("/a.wav"), &info);
        let sound = p.add_media_clip(a, None, 0.0).unwrap();
        assert!(p.key_all(sound, 1.0).is_err());
        let track = p.clip(id).unwrap().track;
        p.track_mut(track).unwrap().locked = true;
        assert!(p.key_all(id, 1.0).is_err());
        assert!(broken_rule(&p).is_none());
    }

    #[test]
    fn audio_cannot_go_on_video_tracks() {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 5.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let a = p.add_asset(&PathBuf::from("/a.wav"), &info);
        let visual = p.tracks[0].id;
        assert!(p.add_media_clip(a, Some(visual), 0.0).is_err());
        let id = p.add_media_clip(a, None, 0.0).unwrap();
        assert_eq!(p.track(p.clip(id).unwrap().track).unwrap().kind, TrackKind::Audio);
    }

    #[test]
    fn trim_respects_media_length_and_neighbours() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 2.0).unwrap();
        p.trim(c, Edge::End, 30.0).unwrap();
        assert_eq!(p.clip(c).unwrap().duration, 10.0, "can't extend past the media");
        p.trim(c, Edge::Start, 4.0).unwrap();
        let clip = p.clip(c).unwrap();
        assert_eq!((clip.start, clip.duration), (4.0, 8.0));
        assert!(matches!(clip.content, Content::Media { src_in, .. } if (src_in - 2.0).abs() < 1e-9));
        p.trim(c, Edge::Start, 0.0).unwrap();
        assert_eq!(p.clip(c).unwrap().start, 2.0, "can't reveal footage before the file starts");
    }

    #[test]
    fn trimming_the_start_past_a_press_pulse_drops_or_clamps_it() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        let clip = p.clip_mut(c).unwrap();
        clip.presses.push(crate::clip::PressPulse { id: 1, at: 1.0, duration: 0.2 });
        clip.presses.push(crate::clip::PressPulse { id: 2, at: 3.0, duration: 0.2 });
        // Trimming the start to 2.0 moves everything 2.0s earlier in clip-relative time: the
        // first press (now fully before the new start) is dropped, the second is kept and
        // re-anchored to 1.0, never left with a negative `at`.
        p.trim(c, Edge::Start, 2.0).unwrap();
        let presses = &p.clip(c).unwrap().presses;
        assert_eq!(presses.len(), 1, "the press entirely trimmed off the front is dropped");
        assert!((presses[0].at - 1.0).abs() < 1e-9, "the surviving press is re-anchored, not left negative");
    }

    #[test]
    fn split_keeps_source_continuous() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        let r = p.split(c, 4.0).unwrap();
        assert_eq!(p.clip(c).unwrap().duration, 4.0);
        let right = p.clip(r).unwrap();
        assert_eq!((right.start, right.duration), (4.0, 6.0));
        assert_eq!(right.source_time(5.0), 5.0);
        assert!(p.split(c, 0.05).is_err());
    }

    #[test]
    fn stagger_group_delays_entrances_in_order() {
        let mut p = Project::new("t", Canvas { fps: 60, ..Default::default() });
        let ids: Vec<Id> = (0..3)
            .map(|i| {
                let id = p.add_text_clip(None, i as f64, DEFAULT_STILL, TextLayer::default()).unwrap();
                p.clip_mut(id).unwrap().intro = Some(crate::clip::Motion { kind: crate::clip::MotionKind::Fade, duration: 0.5, ease: None, delay: 0.0 });
                id
            })
            .collect();
        p.stagger_group(&ids, 3, 60).unwrap();
        for (i, &id) in ids.iter().enumerate() {
            let delay = p.clip(id).unwrap().intro.unwrap().delay;
            assert!((delay - i as f64 * 3.0 / 60.0).abs() < 1e-9, "clip {i} delay {delay}");
        }
        // A clip with no intro is left alone rather than erroring.
        let no_intro = p.add_text_clip(None, 10.0, DEFAULT_STILL, TextLayer::default()).unwrap();
        p.stagger_group(&[ids[0], no_intro], 2, 60).unwrap();
        assert!(p.clip(no_intro).unwrap().intro.is_none());
        assert!(p.stagger_group(&[ids[0]], 2, 60).is_err(), "needs more than one clip");
    }

    #[test]
    fn speed_change_keeps_footage() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        p.set_speed(c, 2.0).unwrap();
        assert_eq!(p.clip(c).unwrap().duration, 5.0);
        assert_eq!(p.clip(c).unwrap().source_time(5.0), 10.0);
    }

    #[test]
    fn slowing_down_next_to_a_neighbour_loses_nothing() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        let track = p.clip(c).unwrap().track;
        let next = p.add_media_clip(a, Some(track), 10.0).unwrap();
        p.clip_mut(c).unwrap().zooms.push(crate::clip::Zoom { id: 99, at: 4.0, duration: 2.0, scale: 2.0, focus_x: 0.5, focus_y: 0.5, ramp: 0.5 });
        p.set_speed(c, 0.5).unwrap();
        assert_eq!(p.clip(c).unwrap().duration, 20.0);
        assert_eq!(p.clip(next).unwrap().start, 20.0, "the neighbour moves along");
        assert_eq!(p.clip(c).unwrap().zooms[0].at, 8.0, "the zoom stays on the same footage");
        p.set_speed(c, 1.0).unwrap();
        assert_eq!(p.clip(c).unwrap().duration, 10.0, "back to the full original length");
        assert_eq!(p.clip(next).unwrap().start, 10.0);
    }

    #[test]
    fn locked_tracks_refuse_edits() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        let track = p.clip(c).unwrap().track;
        p.track_mut(track).unwrap().locked = true;
        assert!(p.trim(c, Edge::End, 5.0).is_err());
        assert!(p.split(c, 5.0).is_err());
        assert!(p.delete_clips(&[c]).is_err());
        assert!(p.set_speed(c, 2.0).is_err());
    }

    #[test]
    fn moving_onto_another_clip_finds_the_nearest_gap() {
        let (mut p, a) = project_with_video(4.0);
        let c1 = p.add_media_clip(a, None, 0.0).unwrap();
        let c2 = p.add_media_clip(a, None, 10.0).unwrap();
        let track = p.clip(c1).unwrap().track;
        p.move_clip(c2, track, 1.0).unwrap();
        assert_eq!(p.clip(c2).unwrap().start, 4.0);
    }

    #[test]
    fn zoom_timing_stays_between_neighbours() {
        let (mut p, a) = project_with_video(20.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        let mk = |id, at, duration| crate::clip::Zoom { id, at, duration, scale: 2.0, focus_x: 0.5, focus_y: 0.5, ramp: 0.5 };
        p.clip_mut(c).unwrap().zooms = vec![mk(1, 2.0, 3.0), mk(2, 10.0, 3.0)];
        // Dragged onto its neighbour: pushed back next to it.
        p.set_zoom_timing(c, 2, 4.0, 3.0).unwrap();
        let z = p.clip(c).unwrap().zooms[1];
        assert_eq!((z.at, z.duration), (5.0, 3.0));
        // Stretched past the clip end: clamped.
        p.set_zoom_timing(c, 2, 18.0, 9.0).unwrap();
        let z = p.clip(c).unwrap().zooms[1];
        assert!(z.at + z.duration <= 20.0 + 1e-9);
    }

    #[test]
    fn range_volume_only_changes_the_range() {
        let mut p = Project::new("t", Canvas::default());
        let info = ProbeInfo { kind: MediaKind::Audio, duration: 10.0, width: 0, height: 0, fps: 0.0, has_audio: true };
        let a = p.add_asset(&PathBuf::from("/a.wav"), &info);
        let c = p.add_media_clip(a, None, 2.0).unwrap();
        p.set_range_volume(c, 4.0, 6.0, 0.25).unwrap();
        let audio = &p.clip(c).unwrap().audio;
        assert_eq!(audio.gain_at(1.0), 1.0, "before the range");
        assert!((audio.gain_at(3.0) - 0.25).abs() < 1e-9, "inside the range");
        assert_eq!(audio.gain_at(5.0), 1.0, "after the range");
        // Splitting keeps the range on the right half, at the right place.
        let right = p.split(c, 4.5).unwrap();
        let r = &p.clip(right).unwrap().audio;
        assert!((r.gain_at(0.5) - 0.25).abs() < 1e-9 && r.gain_at(2.0) == 1.0);
        // Setting 100% again removes the change.
        p.set_range_volume(right, 4.5, 6.0, 1.0).unwrap();
        assert!(p.clip(right).unwrap().audio.gains.is_empty());
    }

    #[test]
    fn separating_audio_makes_a_sound_clip_and_silences_the_video() {
        let (mut p, a) = project_with_video(10.0);
        let v = p.add_media_clip(a, None, 2.0).unwrap();
        p.clip_mut(v).unwrap().audio.volume = 0.5;
        let s = p.separate_audio(v).unwrap();
        let (video, sound) = (p.clip(v).unwrap().clone(), p.clip(s).unwrap().clone());
        assert_eq!(p.track(sound.track).unwrap().kind, TrackKind::Audio, "the sound goes on an audio track");
        assert_eq!((sound.start, sound.duration), (video.start, video.duration), "at the same time");
        assert_eq!(sound.audio.volume, 0.5, "with the same volume");
        assert!(p.clip_has_sound(&sound) && !p.clip_has_picture(&sound));
        assert!(p.clip_has_picture(&video) && !p.clip_has_sound(&video));
        assert!(p.separate_audio(v).is_err(), "only once");
        // The sound clip stays on audio tracks when moved.
        let video_track = video.track;
        assert!(p.move_clip(s, video_track, 0.0).is_err());
    }

    #[test]
    fn markers_stay_in_time_order() {
        let mut p = Project::new("t", Canvas::default());
        let red = crate::clip::Rgba::hex(0xFF0000);
        let late = p.add_marker(5.0, 0.0, red);
        let early = p.add_marker(1.0, 2.0, red);
        assert_eq!(p.markers.iter().map(|m| m.id).collect::<Vec<_>>(), vec![early, late]);
        p.move_marker(early, 9.0).unwrap();
        assert_eq!(p.markers[1].id, early);
        p.delete_marker(late).unwrap();
        assert!(p.delete_marker(late).is_err());
    }

    #[test]
    fn tracks_keep_video_above_audio() {
        let mut p = Project::new("t", Canvas::default());
        let v = p.add_track(TrackKind::Visual, "B-roll", None);
        let i = p.track_index(v).unwrap();
        assert_eq!(p.tracks[i + 1].kind, TrackKind::Audio);
        assert!(p.move_track(v, 1).is_err(), "can't move a video track into the audio group");
        p.rename_track(v, "  Screen  ").unwrap();
        assert_eq!(p.track(v).unwrap().name, "Screen");
        assert!(p.rename_track(v, "  ").is_err());
    }

    #[test]
    fn bring_to_front_and_send_to_back() {
        let mut p = Project::new("t", Canvas::default());
        let visual = |p: &Project| p.tracks.iter().filter(|t| t.kind == TrackKind::Visual).map(|t| t.id).collect::<Vec<_>>();
        let (top, bottom) = (visual(&p)[0], *visual(&p).last().unwrap());
        // A title already on the top track, and a text on the bottom one under it.
        let title = p.add_text_clip(Some(top), 0.0, 5.0, TextLayer::default()).unwrap();
        let text = p.add_text_clip(Some(bottom), 1.0, 2.0, TextLayer::default()).unwrap();
        // The top track is busy then: a new track on top.
        p.arrange(text, true).unwrap();
        let new_top = visual(&p)[0];
        assert!(new_top != top && p.clip(text).unwrap().track == new_top);
        assert_eq!(p.clip(text).unwrap().start, 1.0, "it keeps its time");
        // Already in front: nothing changes.
        let before = p.clone();
        p.arrange(text, true).unwrap();
        assert_eq!(p, before);
        // Behind everything: the bottom track is free again.
        p.arrange(text, false).unwrap();
        assert_eq!(p.clip(text).unwrap().track, bottom);
        // The title goes to the back too, but the bottom track is busy now: a new bottom track.
        p.arrange(title, false).unwrap();
        assert_eq!(p.clip(title).unwrap().track, *visual(&p).last().unwrap());
        assert_ne!(p.clip(title).unwrap().track, bottom);
        assert!(p.tracks.iter().position(|t| t.id == p.clip(title).unwrap().track) < p.tracks.iter().position(|t| t.kind == TrackKind::Audio), "video tracks stay above audio");
        // Sound can't be layered; locked clips don't move.
        let a = p.add_asset(&PathBuf::from("/a.wav"), &crate::media::ProbeInfo { kind: MediaKind::Audio, duration: 3.0, width: 0, height: 0, fps: 0.0, has_audio: true });
        let sound = p.add_media_clip(a, None, 0.0).unwrap();
        assert!(p.arrange(sound, true).is_err());
        p.track_mut(bottom).unwrap().locked = true;
        assert!(p.arrange(text, true).is_err());
        assert!(p.move_to_new_top_track(text, 0.0).is_err(), "locked clips stay put");
        // Dropped above the top track: a new track on top, at the drop time.
        p.track_mut(bottom).unwrap().locked = false;
        let tracks = p.tracks.len();
        p.move_to_new_top_track(text, 2.5).unwrap();
        assert_eq!(p.tracks.len(), tracks + 1);
        assert_eq!((p.clip(text).unwrap().track, p.clip(text).unwrap().start), (p.tracks[0].id, 2.5));
        assert!(p.move_to_new_top_track(sound, 0.0).is_err());
    }

    #[test]
    fn push_in_ramps_a_zoom_held_past_the_clips_end() {
        let (mut p, a) = project_with_video(10.0);
        let clip = p.add_media_clip(a, None, 0.0).unwrap();
        p.push_in(clip, 2.0).unwrap();
        let c = p.clip(clip).unwrap();
        assert_eq!(c.zooms.len(), 1);
        let z = c.zooms[0];
        assert_eq!(z.at, 2.0);
        assert!(z.duration > c.duration - z.at, "runs past the clip's own end so the ramp-out never shows");
        assert_eq!((z.focus_x, z.focus_y), (0.5, 0.5));
        assert!(z.scale > 1.0, "a closer framing");
    }

    #[test]
    fn pull_back_adds_two_back_to_back_zooms_with_no_gap() {
        let (mut p, a) = project_with_video(10.0);
        let clip = p.add_media_clip(a, None, 0.0).unwrap();
        p.pull_back(clip, 1.0).unwrap();
        let c = p.clip(clip).unwrap();
        assert_eq!(c.zooms.len(), 2);
        let (first, second) = (c.zooms[0], c.zooms[1]);
        assert_eq!(first.at, 1.0);
        assert!(first.scale > second.scale, "starts punched in...");
        assert_eq!(second.scale, 1.0, "...and settles back to normal");
        assert!((second.at - (first.at + first.duration)).abs() < 1e-9, "back-to-back: no return-to-identity dip between them");
    }

    #[test]
    fn camera_moves_refuse_non_picture_clips_and_clips_with_no_room_left() {
        let mut p = Project::new("t", Canvas::default());
        let text = p.add_text_clip(None, 0.0, 5.0, TextLayer::default()).unwrap();
        assert!(p.push_in(text, 0.0).is_err(), "text has no camera to move");
        let (mut p, a) = project_with_video(10.0);
        let clip = p.add_media_clip(a, None, 0.0).unwrap();
        assert!(p.push_in(clip, 9.9).is_err(), "not enough room left for a camera move");
    }

    #[test]
    fn callouts_pop_in_and_glow_by_default() {
        let mut p = Project::new("t", Canvas::default());
        let layer = crate::clip::CalloutLayer::new(crate::clip::CalloutIcon::Check, crate::clip::Rgba::hex(0x2EF2A2));
        let id = p.add_callout_clip(None, 2.0, 1.3, layer).unwrap();
        let c = p.clip(id).unwrap();
        assert!(matches!(c.content, Content::Callout(cal) if cal.icon == crate::clip::CalloutIcon::Check));
        assert!(c.look.shadow.enabled, "the glow is on by default");
        assert_eq!(c.look.shadow.color, crate::clip::Rgba::hex(0x2EF2A2), "glow matches the icon color");
        assert_eq!(c.intro.map(|m| m.kind), Some(crate::clip::MotionKind::Pop), "pops in by default");
    }

    fn video_asset(p: &mut Project, len: f64) -> Id {
        let info = ProbeInfo { kind: MediaKind::Video, duration: len, width: 1920, height: 1080, fps: 30.0, has_audio: true };
        p.add_asset(&PathBuf::from(format!("/v{len}.mp4")), &info)
    }

    /// `c1` (4s, at 0) touching `c2` (10s, at 4) touching `c3` (10s, at 14), all on one track.
    fn three_clips_in_a_row(p: &mut Project) -> (Id, Id, Id) {
        let a4 = video_asset(p, 4.0);
        let a10 = video_asset(p, 10.0);
        let c1 = p.add_media_clip(a4, None, 0.0).unwrap();
        let track = p.clip(c1).unwrap().track;
        let c2 = p.add_media_clip(a10, Some(track), 4.0).unwrap();
        let c3 = p.add_media_clip(a10, Some(track), 14.0).unwrap();
        (c1, c2, c3)
    }

    #[test]
    fn a_transition_sits_on_the_cut_and_moves_nothing() {
        let mut p = Project::new("t", Canvas::default());
        let (c1, c2, c3) = three_clips_in_a_row(&mut p);
        let before: Vec<(f64, f64)> = p.clips.iter().map(|c| (c.start, c.duration)).collect();
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        // Music and sound on other tracks stay in step: every clip keeps its place and length.
        assert_eq!(p.clips.iter().map(|c| (c.start, c.duration)).collect::<Vec<_>>(), before);
        assert_eq!(p.clip(c2).unwrap().transition.map(|t| (t.kind, t.duration)), Some((TransitionKind::Crossfade, 1.0)));
        assert_eq!(p.transition_into(p.clip(c2).unwrap()).map(|(prev, _)| prev.id), Some(c1));
        assert_eq!(p.transition_out_of(p.clip(c1).unwrap()).map(|(next, _)| next.id), Some(c2));
        assert!(p.transition_into(p.clip(c3).unwrap()).is_none());
        // Resizing it moves nothing either; it is held to what both clips have room for.
        p.set_transition(c2, Some(TransitionKind::Crossfade), 2.0).unwrap();
        p.set_transition(c2, Some(TransitionKind::Wipe), 60.0).unwrap();
        assert_eq!(p.clips.iter().map(|c| (c.start, c.duration)).collect::<Vec<_>>(), before);
        let d = p.clip(c2).unwrap().transition.unwrap().duration;
        assert!((d - 2.0 * (4.0 - MIN_CLIP)).abs() < 1e-9, "{d}");
        assert!(broken_rule(&p).is_none());
    }

    #[test]
    fn removing_a_transition_restores_the_gapless_layout() {
        let mut p = Project::new("t", Canvas::default());
        let (_, c2, c3) = three_clips_in_a_row(&mut p);
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        p.set_transition(c2, None, 0.0).unwrap();
        assert_eq!(p.clip(c2).unwrap().start, 4.0);
        assert!(p.clip(c2).unwrap().transition.is_none());
        assert_eq!(p.clip(c3).unwrap().start, 14.0);
    }

    #[test]
    fn transition_requires_a_touching_previous_clip() {
        let mut p = Project::new("t", Canvas::default());
        let a = video_asset(&mut p, 4.0);
        let first = p.add_media_clip(a, None, 0.0).unwrap();
        assert!(p.set_transition(first, Some(TransitionKind::Crossfade), 1.0).is_err(), "nothing before the first clip");
        let track = p.clip(first).unwrap().track;
        let after_gap = p.add_media_clip(a, Some(track), 10.0).unwrap();
        assert!(p.set_transition(after_gap, Some(TransitionKind::Crossfade), 1.0).is_err(), "a gap isn't touching");
    }

    #[test]
    fn transitions_saved_as_an_overlap_become_a_cut_in_its_middle_and_nothing_else_moves() {
        let mut p = Project::new("t", Canvas::default());
        let (c1, c2, c3) = three_clips_in_a_row(&mut p);
        // Saved by the previous version: the incoming clip pulled 1 s into the outgoing one (from
        // 4 s to 3 s, its footage from the same moment), with the audio fades the overlap set.
        {
            let c = p.clip_mut(c2).unwrap();
            c.start = 3.0;
            c.transition = Some(Transition { kind: TransitionKind::Crossfade, duration: 1.0 });
            c.audio.fade_in = 1.0;
        }
        p.clip_mut(c1).unwrap().audio.fade_out = 1.0;
        p.clip_mut(c3).unwrap().start = 13.0;
        let src = |p: &Project, id| match p.clip(id).unwrap().content { Content::Media { src_in, .. } => src_in, _ => unreachable!() };
        p.migrate_transitions();
        // A cut at 3.5 s, the middle of the old overlap: the transition still covers 3..4 s.
        let (a, b) = (p.clip(c1).unwrap().clone(), p.clip(c2).unwrap().clone());
        assert_eq!((a.start, a.end(), b.start, b.end()), (0.0, 3.5, 3.5, 13.0));
        assert_eq!((src(&p, c2), b.transition.map(|t| t.duration)), (0.5, Some(1.0)));
        assert_eq!((a.audio.fade_out, b.audio.fade_in), (0.0, 0.0), "the transition crossfades the sound itself");
        assert_eq!(p.clip(c3).unwrap().start, 13.0, "everything else stays where it was");
        assert!(broken_rule(&p).is_none());
        // One saved before that (touching already) is right as it is.
        let mut q = Project::new("t", Canvas::default());
        let (_, d2, _) = three_clips_in_a_row(&mut q);
        q.clip_mut(d2).unwrap().transition = Some(Transition { kind: TransitionKind::Wipe, duration: 1.0 });
        let before = q.clone();
        q.migrate_transitions();
        assert_eq!(q, before);
    }

    #[test]
    fn migrating_an_orphaned_transition_just_drops_it() {
        let mut p = Project::new("t", Canvas::default());
        let a = video_asset(&mut p, 4.0);
        let first = p.add_media_clip(a, None, 0.0).unwrap();
        p.clip_mut(first).unwrap().transition = Some(Transition { kind: TransitionKind::Crossfade, duration: 1.0 });
        p.migrate_transitions();
        assert_eq!(p.clip(first).unwrap().start, 0.0);
        assert!(p.clip(first).unwrap().transition.is_none(), "nothing before the first clip to blend from");
    }

    #[test]
    fn moving_a_transitioned_clip_detaches_it() {
        let mut p = Project::new("t", Canvas::default());
        let (_, c2, c3) = three_clips_in_a_row(&mut p);
        let track = p.clip(c2).unwrap().track;
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        p.move_clip(c2, track, 30.0).unwrap();
        assert!(p.clip(c2).unwrap().transition.is_none());
        assert_eq!(p.clip(c3).unwrap().start, 14.0, "the rest of the track is un-rippled");
    }

    #[test]
    fn trimming_the_outgoing_clip_detaches_its_dependents_transition() {
        let mut p = Project::new("t", Canvas::default());
        let (c1, c2, _) = three_clips_in_a_row(&mut p);
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        p.trim(c1, Edge::End, 2.0).unwrap();
        assert!(p.clip(c2).unwrap().transition.is_none());
        assert_eq!(p.clip(c2).unwrap().start, 4.0);
    }

    #[test]
    fn splitting_inside_a_transition_window_is_rejected() {
        let mut p = Project::new("t", Canvas::default());
        let (_, c2, _) = three_clips_in_a_row(&mut p);
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        // c2 now runs 3.0..13.0, with the shared overlap over 3.0..4.0.
        assert!(p.split(c2, 3.5).is_err(), "would split inside the blend");
        assert!(p.split(c2, 5.0).is_ok());
    }

    #[test]
    fn deleting_a_transition_partner_detaches_the_survivor() {
        let mut p = Project::new("t", Canvas::default());
        let (c1, c2, c3) = three_clips_in_a_row(&mut p);
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        p.delete_clips(&[c1]).unwrap();
        assert!(p.clip(c2).unwrap().transition.is_none());
        assert_eq!(p.clip(c2).unwrap().start, 4.0);
        assert_eq!(p.clip(c3).unwrap().start, 14.0);
    }

    #[test]
    fn duplicating_a_transitioned_clip_does_not_copy_it() {
        let mut p = Project::new("t", Canvas::default());
        let (_, c2, _) = three_clips_in_a_row(&mut p);
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        let dup = p.duplicate_clip(c2).unwrap();
        assert!(p.clip(dup).unwrap().transition.is_none());
    }

    #[test]
    fn set_speed_clears_its_own_transition_but_preserves_a_rippled_one() {
        let mut p = Project::new("t", Canvas::default());
        // c0, then an untouched-by-any-transition cMid, then a c1-into-c2 transition further
        // along — resizing c0 should just uniformly ripple all three without touching c1/c2's pair.
        let a4 = video_asset(&mut p, 4.0);
        let a10 = video_asset(&mut p, 10.0);
        let c0 = p.add_media_clip(a4, None, 0.0).unwrap();
        let track = p.clip(c0).unwrap().track;
        let mid = p.add_media_clip(a10, Some(track), 4.0).unwrap();
        let c1 = p.add_media_clip(a10, Some(track), 14.0).unwrap();
        let c2 = p.add_media_clip(a10, Some(track), 24.0).unwrap();
        p.set_transition(c2, Some(TransitionKind::Crossfade), 1.0).unwrap();
        assert_eq!((p.clip(c1).unwrap().start, p.clip(c2).unwrap().start), (14.0, 24.0));
        p.set_speed(c0, 2.0).unwrap();
        assert_eq!(p.clip(c0).unwrap().duration, 2.0, "half the duration at double speed");
        // Everything after ripples by the same delta (-2.0), c1 and c2 still touching.
        assert_eq!(p.clip(mid).unwrap().start, 2.0);
        assert_eq!(p.clip(c1).unwrap().start, 12.0);
        assert_eq!(p.clip(c2).unwrap().start, 22.0);
        assert_eq!(p.clip(c2).unwrap().transition.map(|t| t.duration), Some(1.0), "a uniform ripple preserves it");
        // But resizing the outgoing clip of a pair directly must detach that pair's transition.
        p.set_speed(c1, 2.0).unwrap();
        assert!(p.clip(c2).unwrap().transition.is_none(), "resizing the outgoing clip itself detaches it");
    }

    // ---- groups, ripple edits, gaps, paste, freeze frames

    /// A video on Main at 0–10 with its sound separated (so the two are linked).
    fn linked_pair() -> (Project, Id, Id) {
        let (mut p, a) = project_with_video(10.0);
        let v = p.add_media_clip(a, None, 0.0).unwrap();
        let s = p.separate_audio(v).unwrap();
        (p, v, s)
    }

    #[test]
    fn separated_audio_stays_linked_and_moves_with_its_video() {
        let (mut p, v, s) = linked_pair();
        assert_eq!(p.group_members(v), vec![v, s]);
        assert_eq!(p.with_groups(&[s]), vec![v, s]);
        let track = p.clip(v).unwrap().track;
        p.move_clips(&[v], v, track, 3.0).unwrap();
        assert_eq!((p.clip(v).unwrap().start, p.clip(s).unwrap().start), (3.0, 3.0), "the sound follows");
        // Ungrouped, each moves alone again.
        p.ungroup_clips(&[v]).unwrap();
        assert!(p.clip(s).unwrap().group.is_none());
        p.move_clips(&[v], v, track, 0.0).unwrap();
        assert_eq!((p.clip(v).unwrap().start, p.clip(s).unwrap().start), (0.0, 3.0));
        assert!(p.ungroup_clips(&[v]).is_err(), "nothing left to ungroup");
        // Grouping again is one group; grouping an existing group changes nothing.
        let g = p.group_clips(&[v, s]).unwrap();
        let before = p.clone();
        assert_eq!(p.group_clips(&[s, v]).unwrap(), g);
        assert_eq!(p, before);
        assert!(p.group_clips(&[v]).is_ok(), "one clip of a group stands for the whole group");
        p.ungroup_clips(&[v]).unwrap();
        assert!(p.group_clips(&[v]).is_err(), "a single clip can't be a group");
    }

    #[test]
    fn moving_a_set_keeps_its_layout_and_finds_room() {
        let (mut p, a) = project_with_video(4.0);
        let c1 = p.add_media_clip(a, None, 0.0).unwrap();
        let track = p.clip(c1).unwrap().track;
        let c2 = p.add_media_clip(a, Some(track), 6.0).unwrap();
        let wall = p.add_media_clip(a, Some(track), 20.0).unwrap();
        // Both move by the same amount.
        p.move_clips(&[c1, c2], c1, track, 2.0).unwrap();
        assert_eq!((p.clip(c1).unwrap().start, p.clip(c2).unwrap().start), (2.0, 8.0));
        // Dropped where the second would land on another clip: the set stops against it.
        p.move_clips(&[c1, c2], c1, track, 11.0).unwrap();
        assert_eq!((p.clip(c1).unwrap().start, p.clip(c2).unwrap().start), (10.0, 16.0), "c2 ends where the wall starts");
        // Never before the start of the timeline.
        p.move_clips(&[c1, c2], c2, track, 0.0).unwrap();
        assert_eq!((p.clip(c1).unwrap().start, p.clip(c2).unwrap().start), (0.0, 6.0));
        // Moving both to the track above keeps them side by side there.
        let above = p.tracks[p.track_index(track).unwrap() - 1].id;
        p.move_clips(&[c1, c2], c1, above, 0.0).unwrap();
        assert!(p.clip(c1).unwrap().track == above && p.clip(c2).unwrap().track == above);
        assert_eq!(p.clip(wall).unwrap().start, 20.0);
    }

    #[test]
    fn ripple_delete_closes_the_hole_on_each_track() {
        let (mut p, a) = project_with_video(4.0);
        let c1 = p.add_media_clip(a, None, 0.0).unwrap();
        let track = p.clip(c1).unwrap().track;
        let c2 = p.add_media_clip(a, Some(track), 4.0).unwrap();
        let c3 = p.add_media_clip(a, Some(track), 10.0).unwrap();
        let title = p.add_text_clip(None, 9.0, 2.0, TextLayer::default()).unwrap();
        p.ripple_delete(&[c2]).unwrap();
        assert!(p.clip(c2).is_none());
        assert_eq!(p.clip(c3).unwrap().start, 6.0, "moved left by the deleted clip's length, keeping its own gap");
        assert_eq!(p.clip(title).unwrap().start, 9.0, "other tracks are left alone");
        // A linked pair goes together and both tracks close up.
        let (mut p, v, s) = linked_pair();
        let (vt, st) = (p.clip(v).unwrap().track, p.clip(s).unwrap().track);
        let a2 = p.assets[0].id;
        let after_v = p.add_media_clip(a2, Some(vt), 10.0).unwrap();
        let music = p.add_asset(&PathBuf::from("/m.wav"), &ProbeInfo { kind: MediaKind::Audio, duration: 5.0, width: 0, height: 0, fps: 0.0, has_audio: true });
        let after_s = p.add_media_clip(music, Some(st), 12.0).unwrap();
        let both = p.with_groups(&[v]);
        p.ripple_delete(&both).unwrap();
        assert_eq!((p.clip(after_v).unwrap().start, p.clip(after_s).unwrap().start), (0.0, 2.0));
    }

    #[test]
    fn closing_gaps_on_a_track_and_across_the_timeline() {
        let (mut p, a) = project_with_video(4.0);
        let c1 = p.add_media_clip(a, None, 1.0).unwrap();
        let track = p.clip(c1).unwrap().track;
        let c2 = p.add_media_clip(a, Some(track), 7.0).unwrap();
        let c3 = p.add_media_clip(a, Some(track), 11.0).unwrap();
        let c4 = p.add_media_clip(a, Some(track), 20.0).unwrap();
        p.set_transition(c3, Some(TransitionKind::Crossfade), 1.0).unwrap();
        // Now: 1–5, 7–11, 11–15 (a transition on the cut at 11: nothing moved), 20–24.
        let starts = |p: &Project| [c1, c2, c3, c4].map(|c| p.clip(c).unwrap().start);
        assert_eq!(starts(&p), [1.0, 7.0, 11.0, 20.0]);
        let mut q = p.clone();
        // On the track: clips close up behind the first one, a transition stays on its cut.
        let removed = q.close_track_gaps(track).unwrap();
        assert_eq!(starts(&q), [1.0, 5.0, 9.0, 13.0]);
        assert!(q.clip(c3).unwrap().transition.is_some());
        assert!((removed - 7.0).abs() < 1e-9, "{removed}");
        assert_eq!(q.close_track_gaps(track).unwrap(), 0.0, "nothing left to close");
        // One gap only, by pointing at it — the one before the first clip too.
        let mut q = p.clone();
        assert!((q.close_gap_at(track, 6.0).unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(starts(&q), [1.0, 5.0, 9.0, 18.0]);
        assert!((q.close_gap_at(track, 0.5).unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(starts(&q), [0.0, 4.0, 8.0, 17.0]);
        assert!(q.close_gap_at(track, 2.0).is_err(), "a clip is there");
        // Across the timeline: only time where nothing plays goes away, so tracks stay in sync.
        let title = p.add_text_clip(None, 5.5, 1.0, TextLayer::default()).unwrap();
        let marker = p.add_marker(18.0, 0.0, crate::clip::Rgba::hex(0xFF0000));
        let removed = p.close_empty_time().unwrap();
        // Empty before: 0–1, 5–5.5, 6.5–7 and 15–20.
        assert!((removed - 7.0).abs() < 1e-9, "{removed}");
        assert_eq!(starts(&p), [0.0, 5.0, 9.0, 13.0]);
        assert_eq!(p.clip(title).unwrap().start, 4.0);
        assert_eq!(p.markers.iter().find(|m| m.id == marker).unwrap().at, 13.0, "a marker inside a gap lands where the gap was");
        assert_eq!(p.close_empty_time().unwrap(), 0.0);
    }

    #[test]
    fn cutting_a_range_out_of_every_track() {
        let (mut p, a) = project_with_video(10.0);
        let c1 = p.add_media_clip(a, None, 0.0).unwrap();
        let track = p.clip(c1).unwrap().track;
        let c2 = p.add_media_clip(a, Some(track), 10.0).unwrap();
        let title = p.add_text_clip(None, 4.0, 3.0, TextLayer::default()).unwrap();
        let title_track = p.clip(title).unwrap().track;
        let late = p.add_text_clip(Some(title_track), 12.0, 2.0, TextLayer::default()).unwrap();
        p.add_marker(5.0, 0.0, crate::clip::Rgba::hex(0xFF0000));
        let m_late = p.add_marker(15.0, 0.0, crate::clip::Rgba::hex(0xFF0000));
        p.ripple_delete_range(3.0, 8.0).unwrap();
        // The first video keeps 0–3 and 8–10 of itself, now back to back.
        let mut on_track: Vec<(f64, f64)> = p.clips_on(track).map(|c| (c.start, c.end())).collect();
        on_track.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(on_track, vec![(0.0, 3.0), (3.0, 5.0), (5.0, 15.0)]);
        assert_eq!(p.clip(c2).unwrap().start, 5.0);
        assert!(p.clip(title).is_none(), "what lay inside the range is gone");
        assert_eq!(p.clip(late).unwrap().start, 7.0);
        assert_eq!(p.markers.len(), 1);
        assert_eq!(p.markers.iter().find(|m| m.id == m_late).unwrap().at, 10.0);
        // The right-hand piece continues the footage where the range ended.
        let piece = p.clips_on(track).find(|c| (c.start - 3.0).abs() < 1e-9).unwrap();
        assert!((piece.source_time(3.0) - 8.0).abs() < 1e-9);
        assert!(p.ripple_delete_range(100.0, 110.0).is_err(), "nothing there");
    }

    #[test]
    fn pasting_keeps_layout_groups_and_transitions() {
        let (mut p, v, s) = linked_pair();
        let vt = p.clip(v).unwrap().track;
        let a = p.assets[0].id;
        let next = p.add_media_clip(a, Some(vt), 10.0).unwrap();
        p.set_transition(next, Some(TransitionKind::Crossfade), 1.0).unwrap();
        let copied: Vec<Clip> = [v, s, next].iter().map(|id| p.clip(*id).unwrap().clone()).collect();
        let new = p.paste_clips(&copied, 30.0).unwrap();
        assert_eq!(new.len(), 3);
        // `paste_clips` answers in time order: the video and its sound (both at 0), then the next clip.
        let by_old = |old: Id| {
            let src = copied.iter().find(|c| c.id == old).unwrap();
            new.iter().map(|id| p.clip(*id).unwrap()).find(|c| c.content == src.content && c.part == src.part && (c.start - 30.0 - src.start).abs() < 1e-9).unwrap()
        };
        // Same tracks (they were free there), same distances, a group of their own, the transition kept.
        assert_eq!((by_old(v).start, by_old(s).start, by_old(next).start), (30.0, 30.0, 40.0));
        assert_eq!((by_old(v).track, by_old(s).track, by_old(next).track), (vt, p.clip(s).unwrap().track, vt));
        assert!(by_old(v).group.is_some() && by_old(v).group == by_old(s).group && by_old(v).group != p.clip(v).unwrap().group);
        assert!(by_old(next).group.is_none());
        assert_eq!(by_old(next).transition.map(|t| t.duration), Some(1.0));
        // Pasted where the track is busy: onto another track, at the same time.
        let again = p.paste_clips(&copied[..1], 2.0).unwrap();
        let c = p.clip(again[0]).unwrap();
        assert!(c.start == 2.0 && c.track != vt && c.group.is_none());
        // Several clips duplicate as a set, right after the last of them.
        let dup = p.duplicate_clips(&[v, s]).unwrap();
        assert_eq!(dup.len(), 2);
        assert!(dup.iter().all(|id| p.clip(*id).unwrap().start == 10.0), "{:?}", dup.iter().map(|id| p.clip(*id).unwrap().start).collect::<Vec<_>>());
    }

    #[test]
    fn freeze_frame_holds_a_frame_and_makes_room() {
        let (mut p, v, s) = linked_pair();
        let (vt, st) = (p.clip(v).unwrap().track, p.clip(s).unwrap().track);
        let a = p.assets[0].id;
        let later = p.add_media_clip(a, Some(vt), 10.0).unwrap();
        p.clip_mut(v).unwrap().zooms.push(Zoom { id: 99, at: 2.0, duration: 6.0, scale: 2.0, focus_x: 0.3, focus_y: 0.6, ramp: 0.5 });
        let still = p.freeze_frame(v, 4.0, 2.0).unwrap();
        let f = p.clip(still).unwrap().clone();
        assert!(f.freeze && (f.start, f.duration, f.track) == (4.0, 2.0, vt));
        assert_eq!((f.source_time(4.0), f.source_time(5.9)), (4.0, 4.0), "one frame for its whole length");
        assert!(!p.clip_has_sound(&f) && p.clip_has_picture(&f));
        assert_eq!(crate::clip::camera_at(&f.zooms, 0.0).scale, 2.0, "it keeps the zoom the video had there");
        assert_eq!(crate::clip::camera_at(&f.zooms, 2.0).scale, 2.0);
        // The video continues after it, and so does its linked sound (silent during the hold).
        let pieces = |p: &Project, track: Id| {
            let mut v: Vec<_> = p.clips_on(track).map(|c| (c.start, c.end())).collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            v
        };
        assert_eq!(pieces(&p, vt), vec![(0.0, 4.0), (4.0, 6.0), (6.0, 12.0), (12.0, 22.0)]);
        assert_eq!(pieces(&p, st), vec![(0.0, 4.0), (6.0, 12.0)]);
        assert_eq!(p.clip(later).unwrap().start, 12.0);
        // Longer by its edge with no footage limit (once there is room); it has no speed.
        p.ripple_delete(&[later]).unwrap();
        let right = p.clips_on(vt).find(|c| c.start == 6.0).unwrap().id;
        let right_group = p.with_groups(&[right]);
        p.move_clips(&right_group, right, vt, 20.0).unwrap();
        p.trim(still, Edge::End, 15.0).unwrap();
        let f = p.clip(still).unwrap();
        assert_eq!(f.duration, 11.0);
        assert_eq!(crate::clip::camera_at(&f.zooms, 10.5).scale, 2.0, "the framing holds for the new length");
        assert!(p.set_speed(still, 2.0).is_err());
        assert!(p.freeze_frame(still, 4.5, 1.0).is_err(), "a still can't be frozen again");
        let text = p.add_text_clip(None, 0.0, 2.0, TextLayer::default()).unwrap();
        assert!(p.freeze_frame(text, 1.0, 1.0).is_err());
    }

    #[test]
    fn disabling_and_replacing_media() {
        let (mut p, a) = project_with_video(10.0);
        let c = p.add_media_clip(a, None, 0.0).unwrap();
        p.set_disabled(&[c], true).unwrap();
        assert!(p.clip(c).unwrap().disabled);
        p.trim(c, Edge::Start, 6.0).unwrap();
        // A shorter video: the clip keeps its edits and shrinks to what the file has.
        let short = p.add_asset(&PathBuf::from("/short.mp4"), &ProbeInfo { kind: MediaKind::Video, duration: 3.0, width: 1280, height: 720, fps: 30.0, has_audio: true });
        p.replace_media(c, short).unwrap();
        let clip = p.clip(c).unwrap();
        assert!(matches!(clip.content, Content::Media { asset, src_in, .. } if asset == short && src_in == 0.0));
        assert_eq!((clip.start, clip.duration), (6.0, 3.0));
        // Sound can't stand in for a picture.
        let sound = p.add_asset(&PathBuf::from("/a.wav"), &ProbeInfo { kind: MediaKind::Audio, duration: 5.0, width: 0, height: 0, fps: 0.0, has_audio: true });
        assert!(p.replace_media(c, sound).is_err());
        // A video dropped onto an audio clip gives its sound only.
        let s = p.add_media_clip(sound, None, 0.0).unwrap();
        p.replace_media(s, a).unwrap();
        let clip = p.clip(s).unwrap();
        assert!(clip.part == Part::Sound && p.clip_kind(clip) == TrackKind::Audio && p.clip_has_sound(clip));
    }

    #[test]
    fn splitting_a_linked_pair_links_both_halves() {
        let (mut p, v, s) = linked_pair();
        let rights = p.split_many(&[v, s], 4.0).unwrap();
        let (vr, sr) = (p.clip(rights[0]).unwrap(), p.clip(rights[1]).unwrap());
        assert!(vr.group.is_some() && vr.group == sr.group);
        assert_ne!(vr.group, p.clip(v).unwrap().group);
        assert_eq!(p.clip(v).unwrap().group, p.clip(s).unwrap().group);
        // Deleting one of a pair leaves the other ungrouped.
        p.delete_clips(&[rights[0]]).unwrap();
        assert!(p.clip(rights[1]).unwrap().group.is_none());
    }

    /// A small repeatable random number generator (xorshift), so a failing run can be replayed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
        /// A time between `a` and `b`, on a 0.05 s grid so edges often touch exactly.
        fn time(&mut self, a: f64, b: f64) -> f64 {
            let steps = (((b - a) / 0.05).round() as usize).max(1);
            a + self.below(steps + 1) as f64 * 0.05
        }
        fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
            (!items.is_empty()).then(|| items[self.below(items.len())])
        }
    }

    /// What must hold after any edit, whatever it was. Returns the first broken rule.
    fn broken_rule(p: &Project) -> Option<String> {
        const EPS: f64 = 1e-6;
        let mut ids: Vec<Id> = p.clips.iter().map(|c| c.id).chain(p.tracks.iter().map(|t| t.id)).chain(p.assets.iter().map(|a| a.id)).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Some("two things share an id".into());
        }
        if ids.last().is_some_and(|&last| last >= p.next_id) {
            return Some("next_id is not past every id in use".into());
        }
        for c in &p.clips {
            let name = format!("clip {} ({:.3}..{:.3} on track {})", c.id, c.start, c.end(), c.track);
            let Some(track) = p.track(c.track) else { return Some(format!("{name} is on a track that doesn't exist")) };
            if track.kind != p.clip_kind(c) {
                return Some(format!("{name} is on the wrong kind of track"));
            }
            if !c.start.is_finite() || !c.duration.is_finite() || c.start < -EPS || c.duration < MIN_CLIP - EPS {
                return Some(format!("{name} has a bad position or length"));
            }
            if c.duration > p.max_duration(c) + 1e-4 {
                return Some(format!("{name} is longer ({:.4}) than its footage allows ({:.4})", c.duration, p.max_duration(c)));
            }
            if let Content::Media { asset, src_in, speed } = c.content {
                if p.asset(asset).is_none() || src_in < -EPS || !(speed > 0.0) {
                    return Some(format!("{name} has bad media settings"));
                }
            }
            // A split keeps a zoom or volume change running across the cut (so the picture and
            // sound stay continuous), so these may reach past the clip, but must overlap it.
            if c.zooms.iter().any(|z| z.at >= c.duration - EPS || z.at + z.duration <= EPS || z.duration <= 0.0) {
                return Some(format!("{name} has a zoom outside itself"));
            }
            if c.audio.gains.iter().any(|g| g.at >= c.duration - EPS || g.at + g.duration <= EPS || g.duration <= 0.0) {
                return Some(format!("{name} has a volume range outside itself: {:?}", c.audio.gains));
            }
            if c.presses.iter().any(|x| x.at < -EPS || x.at > c.duration + EPS) {
                return Some(format!("{name} has a press pulse outside itself"));
            }
            // Keyframes may reach past the clip (a split or trim keeps the motion going across the
            // cut), but each property's are in time order, with real numbers.
            for (prop, keys) in &c.keyframes {
                if keys.iter().any(|k| !k.t.is_finite() || !k.value.is_finite()) || keys.windows(2).any(|w| w[1].t < w[0].t) {
                    return Some(format!("{name} has {prop:?} keyframes out of order or not numbers: {keys:?}"));
                }
            }
            if let Some(g) = c.group {
                if p.clips.iter().filter(|o| o.group == Some(g)).count() < 2 {
                    return Some(format!("{name} is alone in group {g}"));
                }
            }
        }
        for t in &p.tracks {
            let mut on: Vec<&Clip> = p.clips_on(t.id).collect();
            on.sort_by(|a, b| a.start.total_cmp(&b.start));
            for (i, c) in on.iter().enumerate() {
                let prev = i.checked_sub(1).map(|j| on[j]);
                // Clips never overlap, transitions included (they sit on the cut).
                if let Some(prev) = prev.filter(|p| c.start < p.end() - EPS) {
                    return Some(format!("clips {} and {} overlap on track {} ({:.3}..{:.3} and {:.3}..{:.3})", prev.id, c.id, t.id, prev.start, prev.end(), c.start, c.end()));
                }
                if let Some(tr) = c.transition {
                    if !prev.is_some_and(|p| (p.end() - c.start).abs() < 1e-4) {
                        return Some(format!("clip {} has a transition but no clip right before it", c.id));
                    }
                    if !(tr.duration >= MIN_CLIP - EPS) {
                        return Some(format!("clip {} has a transition of {:.3}s", c.id, tr.duration));
                    }
                }
                // Each clip keeps a moment of its own between the transitions into and out of it.
                let half_in = p.transition_into(c).map_or(0.0, |(_, tr)| tr.duration / 2.0);
                let half_out = p.transition_out_of(c).map_or(0.0, |(_, tr)| tr.duration / 2.0);
                if half_in + half_out > c.duration - MIN_CLIP + 1e-6 {
                    return Some(format!("clip {} ({:.3}s) is too short for its transitions ({:.3}s in, {:.3}s out)", c.id, c.duration, half_in, half_out));
                }
            }
        }
        // Video tracks stay above audio tracks.
        let first_audio = p.tracks.iter().position(|t| t.kind == TrackKind::Audio).unwrap_or(p.tracks.len());
        if p.tracks[first_audio..].iter().any(|t| t.kind != TrackKind::Audio) {
            return Some("a video track is below an audio track".into());
        }
        None
    }

    /// One random edit. Failing is fine (many are refused on purpose); breaking a rule is not.
    fn random_edit(rng: &mut Rng, p: &mut Project, assets: &[Id], clipboard: &mut Vec<Clip>) -> (String, anyhow::Result<()>) {
        let clips: Vec<Id> = p.clips.iter().map(|c| c.id).collect();
        let tracks: Vec<Id> = p.tracks.iter().map(|t| t.id).collect();
        let end = p.duration().max(4.0);
        let clip = rng.pick(&clips);
        let some = |rng: &mut Rng, clips: &[Id]| -> Vec<Id> {
            let n = 1 + rng.below(3);
            let mut out: Vec<Id> = (0..n).filter_map(|_| rng.pick(clips)).collect();
            out.sort_unstable();
            out.dedup();
            out
        };
        let span = |p: &Project, id: Id| p.clip(id).map(|c| (c.start, c.end())).unwrap_or((0.0, 1.0));
        match rng.below(34) {
            0 | 1 => {
                let (asset, track, at) = (rng.pick(assets).unwrap(), if rng.below(2) == 0 { rng.pick(&tracks) } else { None }, rng.time(0.0, end + 2.0));
                (format!("add media {asset} on {track:?} at {at:.2}"), p.add_media_clip(asset, track, at).map(|_| ()))
            }
            2 => {
                let at = rng.time(0.0, end + 2.0);
                (format!("add text at {at:.2}"), p.add_text_clip(None, at, 1.0 + rng.below(4) as f64, TextLayer::default()).map(|_| ()))
            }
            3 | 4 => {
                let Some(id) = clip else { return ("move (nothing)".into(), Ok(())) };
                let (track, at) = (rng.pick(&tracks).unwrap(), rng.time(0.0, end + 2.0));
                (format!("move {id} to track {track} at {at:.2}"), p.move_clip(id, track, at))
            }
            5 => {
                let ids = some(rng, &clips);
                let Some(&anchor) = ids.first() else { return ("move set (nothing)".into(), Ok(())) };
                let (track, at) = (rng.pick(&tracks).unwrap(), rng.time(0.0, end + 2.0));
                let ids = p.with_groups(&ids);
                (format!("move set {ids:?} anchor {anchor} to track {track} at {at:.2}"), p.move_clips(&ids, anchor, track, at))
            }
            6 | 7 => {
                let Some(id) = clip else { return ("trim (nothing)".into(), Ok(())) };
                let (a, b) = span(p, id);
                let (edge, t) = if rng.below(2) == 0 { (Edge::Start, rng.time(a - 1.0, b)) } else { (Edge::End, rng.time(a, b + 2.0)) };
                (format!("trim {id} {edge:?} to {t:.2}"), p.trim(id, edge, t))
            }
            8 | 9 => {
                let Some(id) = clip else { return ("split (nothing)".into(), Ok(())) };
                let (a, b) = span(p, id);
                let t = rng.time(a, b);
                (format!("split {id} at {t:.2}"), p.split(id, t).map(|_| ()))
            }
            10 => {
                let (ids, t) = (p.with_groups(&some(rng, &clips)), rng.time(0.0, end));
                (format!("split {ids:?} at {t:.2}"), p.split_many(&ids, t).map(|_| ()))
            }
            11 => {
                let ids = p.with_groups(&some(rng, &clips));
                (format!("delete {ids:?}"), p.delete_clips(&ids))
            }
            12 => {
                let ids = p.with_groups(&some(rng, &clips));
                (format!("ripple delete {ids:?}"), p.ripple_delete(&ids))
            }
            13 => {
                let a = rng.time(0.0, end);
                let b = a + rng.time(0.05, 3.0);
                (format!("cut range {a:.2}..{b:.2}"), p.ripple_delete_range(a, b))
            }
            14 => {
                let track = rng.pick(&tracks).unwrap();
                (format!("close gaps on track {track}"), p.close_track_gaps(track).map(|_| ()))
            }
            15 => ("close empty time".into(), p.close_empty_time().map(|_| ())),
            16 => {
                let (track, t) = (rng.pick(&tracks).unwrap(), rng.time(0.0, end));
                (format!("close gap on track {track} at {t:.2}"), p.close_gap_at(track, t).map(|_| ()))
            }
            17 => {
                let Some(id) = clip else { return ("speed (nothing)".into(), Ok(())) };
                let speed = [0.25, 0.5, 1.0, 1.5, 2.0, 4.0][rng.below(6)];
                (format!("speed {id} to {speed}"), p.set_speed(id, speed))
            }
            18 | 19 => {
                let Some(id) = clip else { return ("transition (nothing)".into(), Ok(())) };
                let kind = if rng.below(4) == 0 { None } else { Some(TransitionKind::Crossfade) };
                let d = rng.time(0.1, 1.5);
                (format!("transition {kind:?} {d:.2} on {id}"), p.set_transition(id, kind, d))
            }
            20 => {
                let ids = p.with_groups(&some(rng, &clips));
                (format!("duplicate {ids:?}"), p.duplicate_clips(&ids).map(|_| ()))
            }
            21 => {
                let ids = p.with_groups(&some(rng, &clips));
                *clipboard = ids.iter().filter_map(|id| p.clip(*id).cloned()).collect();
                let at = rng.time(0.0, end + 2.0);
                (format!("copy {ids:?} and paste at {at:.2}"), p.paste_clips(clipboard, at).map(|_| ()))
            }
            22 => {
                let ids = some(rng, &clips);
                if rng.below(2) == 0 { (format!("group {ids:?}"), p.group_clips(&ids).map(|_| ())) } else { (format!("ungroup {ids:?}"), p.ungroup_clips(&ids)) }
            }
            23 => {
                let Some(id) = clip else { return ("separate (nothing)".into(), Ok(())) };
                (format!("separate audio of {id}"), p.separate_audio(id).map(|_| ()))
            }
            24 => {
                let Some(id) = clip else { return ("freeze (nothing)".into(), Ok(())) };
                let (a, b) = span(p, id);
                let t = rng.time(a, b);
                (format!("freeze frame of {id} at {t:.2}"), p.freeze_frame(id, t, 2.0).map(|_| ()))
            }
            25 => {
                let Some(id) = clip else { return ("range volume (nothing)".into(), Ok(())) };
                let (a, b) = span(p, id);
                let from = rng.time(a - 0.5, b);
                let to = from + rng.time(0.05, 2.0);
                (format!("volume of {id} over {from:.2}..{to:.2}"), p.set_range_volume(id, from, to, 0.3).map(|_| ()))
            }
            26 => {
                let Some(id) = clip else { return ("zoom (nothing)".into(), Ok(())) };
                let (a, b) = span(p, id);
                let t = rng.time(a, b);
                if rng.below(2) == 0 { (format!("push in on {id} at {t:.2}"), p.push_in(id, t)) } else { (format!("pull back on {id} at {t:.2}"), p.pull_back(id, t)) }
            }
            27 => {
                let Some(id) = clip else { return ("replace (nothing)".into(), Ok(())) };
                let asset = rng.pick(assets).unwrap();
                (format!("replace media of {id} with {asset}"), p.replace_media(id, asset))
            }
            28 => match rng.below(4) {
                0 => {
                    let kind = if rng.below(2) == 0 { TrackKind::Visual } else { TrackKind::Audio };
                    p.add_track(kind, "New", None);
                    (format!("add {kind:?} track"), Ok(()))
                }
                1 if tracks.len() > 2 => {
                    let track = rng.pick(&tracks).unwrap();
                    (format!("delete track {track}"), p.delete_track(track))
                }
                2 => {
                    let (track, delta) = (rng.pick(&tracks).unwrap(), if rng.below(2) == 0 { -1 } else { 1 });
                    (format!("move track {track} by {delta}"), p.move_track(track, delta))
                }
                _ => {
                    let track = rng.pick(&tracks).unwrap();
                    let locked = rng.below(4) == 0;
                    (format!("lock track {track}: {locked}"), p.track_mut(track).map(|t| t.locked = locked))
                }
            },
            29 => {
                // A screen recording: a picture and, half the time each, a camera and a voice, started a little apart.
                let (picture, face, voice) = (rng.pick(assets).unwrap(), rng.pick(assets).unwrap(), rng.pick(assets).unwrap());
                let camera = (rng.below(2) == 0).then(|| (face, rng.time(0.0, 3.0)));
                let mic = (rng.below(2) == 0).then(|| (voice, rng.time(0.0, 3.0)));
                let (at, picture_start, go) = (rng.time(0.0, end + 2.0), rng.time(0.0, 3.0), rng.time(0.0, 5.0));
                let take = ScreenTake { screen: (picture, picture_start), camera, mic, go };
                (format!("add screen recording {take:?} at {at:.2}"), p.add_screen_recording(&take, at).map(|_| ()))
            }
            30..=32 => {
                // Keyframes by hand: what the inspector and the timeline's diamonds do.
                let Some(id) = clip else { return ("keyframe (nothing)".into(), Ok(())) };
                let duration = p.clip(id).map_or(1.0, |c| c.duration);
                let local = rng.time(-0.2, duration + 0.2);
                let prop = Prop::ALL[rng.below(5)];
                let keyed = p.clip(id).map(|c| c.key_times()).unwrap_or_default();
                let on_key = rng.pick(&keyed).unwrap_or(local);
                match rng.below(7) {
                    0 => (format!("toggle {prop:?} key of {id} at {local:.2}"), p.toggle_key(id, prop, local)),
                    1 => (format!("key everything of {id} at {local:.2}"), p.key_all(id, local)),
                    2 => (format!("move keys of {id} from {on_key:.2} to {local:.2}"), p.move_keys(id, on_key, local)),
                    3 => (format!("remove keys of {id} at {on_key:.2}"), p.remove_keys_at(id, on_key)),
                    4 => {
                        let ease = crate::clip::Ease::ALL[rng.below(5)];
                        (format!("ease keys of {id} at {on_key:.2}: {ease:?}"), p.set_keys_ease(id, on_key, ease))
                    }
                    5 => {
                        let value = rng.time(-1.0, 2.0);
                        (format!("set {prop:?} of {id} at {local:.2} to {value:.2}"), p.ensure_unlocked(id).and_then(|()| p.clip_mut(id)).map(|c| c.set_value_at(prop, local, value)))
                    }
                    _ => {
                        let all = rng.below(2) == 0;
                        (format!("clear {} animation of {id}", if all { "all".to_string() } else { format!("{prop:?}") }), p.clear_animation(id, (!all).then_some(prop), local))
                    }
                }
            }
            _ => {
                let ids = some(rng, &clips);
                let off = rng.below(2) == 0;
                (format!("disable {ids:?}: {off}"), p.set_disabled(&ids, off))
            }
        }
    }

    #[test]
    fn thousands_of_random_edits_never_break_the_timeline() {
        // 60 runs keep the suite quick; raise this to hunt for rare cases (400 passed on 2026-11-13, keyframe edits included).
        for seed in 1..=60u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut p = Project::new("stress", Canvas::default());
            let assets = [video_asset(&mut p, 4.0), video_asset(&mut p, 10.0), video_asset(&mut p, 30.0), {
                let info = ProbeInfo { kind: MediaKind::Audio, duration: 20.0, width: 0, height: 0, fps: 0.0, has_audio: true };
                p.add_asset(&PathBuf::from("/music.mp3"), &info)
            }, {
                let info = ProbeInfo { kind: MediaKind::Image, duration: 0.0, width: 800, height: 600, fps: 0.0, has_audio: false };
                p.add_asset(&PathBuf::from("/picture.png"), &info)
            }];
            let start = p.clone();
            let mut doc = crate::doc::Doc::new(p);
            let mut clipboard = Vec::new();
            let mut log: Vec<String> = Vec::new();
            for step in 0..400 {
                let mut what = String::new();
                let result = doc.apply("edit", |p| {
                    let (name, result) = random_edit(&mut rng, p, &assets, &mut clipboard);
                    what = name;
                    result
                });
                log.push(format!("{what}{}", if result.is_err() { "  (refused)" } else { "" }));
                if let Some(rule) = broken_rule(&doc.project) {
                    let recent = log[log.len().saturating_sub(12)..].join("\n  ");
                    panic!("seed {seed}, step {step}: {rule}\nlast edits:\n  {recent}");
                }
                // Now and then: undo a few steps and redo them; the result must be the same project.
                if step % 37 == 36 {
                    let before = doc.project.clone();
                    let undone = (0..3).filter(|_| doc.undo().is_some()).count();
                    assert!(broken_rule(&doc.project).is_none(), "seed {seed}, step {step}: undo broke a rule");
                    (0..undone).for_each(|_| {
                        doc.redo();
                    });
                    assert!(doc.project == before, "seed {seed}, step {step}: undo then redo changed the project");
                }
            }
            // The file format keeps everything.
            let json = serde_json::to_string(&doc.project).unwrap();
            let back: Project = serde_json::from_str(&json).unwrap();
            if back != doc.project {
                let diff = back.clips.iter().zip(&doc.project.clips).find(|(a, b)| a != b).map(|(a, b)| format!("{a:?}
---
{b:?}"));
                panic!("seed {seed}: saving and loading changed the project: {diff:?}");
            }
            // Undoing everything (the history keeps 200 steps) never breaks a rule on the way back.
            let mut steps = 0;
            while doc.undo().is_some() {
                steps += 1;
                assert!(broken_rule(&doc.project).is_none(), "seed {seed}: undo step {steps} broke a rule");
            }
            if steps < 200 {
                let mut first = doc.project.clone();
                first.next_id = start.next_id;
                assert!(first == start, "seed {seed}: undoing everything didn't return to the start");
            }
        }
    }
}
