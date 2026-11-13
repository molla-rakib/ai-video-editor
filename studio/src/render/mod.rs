//! Rendering: what is visible at a moment (`scene`), where pixels come from
//! (`frames`, `text`) and how they are drawn on the GPU (`compositor`).
//! Preview and export share all of it, so the preview matches the file.

pub mod audio;
pub mod compositor;
pub mod export;
pub mod frames;
pub mod scene;
pub mod text;

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// An RGBA image with straight alpha, sRGB encoded.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Unique per frame, so the GPU re-uploads only when it changes.
    pub version: u64,
}

impl Frame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Frame {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Frame { width, height, rgba, version: NEXT.fetch_add(1, Ordering::Relaxed) }
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frame {}x{} v{}", self.width, self.height, self.version)
    }
}
