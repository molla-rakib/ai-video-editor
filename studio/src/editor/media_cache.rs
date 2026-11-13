//! Thumbnails, filmstrips and waveforms for library assets, generated in the background.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iced::Task;
use iced::widget::image::Handle;

use crate::media;
use crate::model::{Asset, Id, MediaKind};

pub const COVER_W: u32 = 320;
pub const COVER_H: u32 = 180;
pub const STRIP_H: u32 = 72;

#[derive(Debug, Clone, Default)]
pub struct AssetMedia {
    pub cover: Option<Handle>,
    /// Evenly spaced frames across the whole file.
    pub strip: Vec<Handle>,
    pub strip_w: u32,
    /// Peak levels, `PEAKS_PER_SECOND` per second.
    pub peaks: Option<Arc<Vec<f32>>>,
}

pub const PEAKS_PER_SECOND: u32 = 50;

#[derive(Default)]
pub struct MediaCache {
    entries: HashMap<Id, AssetMedia>,
    loading: Vec<Id>,
}

impl MediaCache {
    pub fn get(&self, id: Id) -> Option<&AssetMedia> {
        self.entries.get(&id)
    }

    /// Starts generating media for assets that don't have it yet.
    pub fn ensure<M: Send + 'static>(&mut self, assets: &[Asset], done: impl Fn(Id, AssetMedia) -> M + Send + Sync + Clone + 'static) -> Task<M> {
        let mut tasks = Vec::new();
        for a in assets {
            if self.entries.contains_key(&a.id) || self.loading.contains(&a.id) {
                continue;
            }
            self.loading.push(a.id);
            let (id, path, kind, duration, width, height, has_audio) = (a.id, a.path.clone(), a.kind, a.duration, a.width, a.height, a.has_audio);
            let done = done.clone();
            tasks.push(Task::perform(
                async move { tokio::task::spawn_blocking(move || generate(path, kind, duration, width, height, has_audio)).await.unwrap_or_default() },
                move |m| done(id, m),
            ));
        }
        Task::batch(tasks)
    }

    /// Drops cached thumbnails so they're made again (after relinking).
    pub fn forget(&mut self, id: Id) {
        self.entries.remove(&id);
        self.loading.retain(|&l| l != id);
    }

    pub fn insert(&mut self, id: Id, media: AssetMedia) {
        self.loading.retain(|&l| l != id);
        self.entries.insert(id, media);
    }
}

fn generate(path: PathBuf, kind: MediaKind, duration: f64, width: u32, height: u32, has_audio: bool) -> AssetMedia {
    let mut out = AssetMedia::default();
    if kind != MediaKind::Audio {
        let t = if kind == MediaKind::Video { (duration * 0.1).min(3.0) } else { 0.0 };
        if let Ok(rgba) = media::thumbnail(&path, kind, t, COVER_W, COVER_H) {
            out.cover = Some(Handle::from_rgba(COVER_W, COVER_H, rgba));
        }
        let aspect = if height > 0 { width as f64 / height as f64 } else { 16.0 / 9.0 };
        let strip_w = ((STRIP_H as f64 * aspect).round() as u32).clamp(32, 256) & !1;
        out.strip_w = strip_w;
        if kind == MediaKind::Video {
            // About one frame per 2 seconds, between 6 and 60.
            let count = ((duration / 2.0).ceil() as usize).clamp(6, 60);
            if let Ok(frames) = media::thumbnail_strip(&path, duration, count, strip_w, STRIP_H) {
                out.strip = frames.into_iter().map(|f| Handle::from_rgba(strip_w, STRIP_H, f)).collect();
            }
        } else if let Ok(rgba) = media::thumbnail(&path, kind, 0.0, strip_w, STRIP_H) {
            out.strip = vec![Handle::from_rgba(strip_w, STRIP_H, rgba)];
        }
    }
    if has_audio {
        if let Ok(peaks) = media::waveform(&path, PEAKS_PER_SECOND) {
            out.peaks = Some(Arc::new(peaks));
        }
    }
    out
}
