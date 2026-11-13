//! Small interface animations: things ease into place when they appear instead of popping.

use std::time::{Duration, Instant};

/// How long something takes to settle after it appears.
pub const APPEAR: Duration = Duration::from_millis(180);

/// 0 right when something appeared, 1 once it has settled; fast at first, gentle at the end.
pub fn appear_progress(since: Instant) -> f32 {
    let t = (since.elapsed().as_secs_f32() / APPEAR.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Tracks which dialog (if any) is showing, and since when, so it can ease in.
#[derive(Debug, Default)]
pub struct Appear {
    shown: Option<(u32, Instant)>,
}

impl Appear {
    /// Call after every update with what is showing now (`None` for nothing). The animation
    /// starts over whenever that changes.
    pub fn track(&mut self, what: Option<u32>) {
        if self.shown.map(|(w, _)| w) != what {
            self.shown = what.map(|w| (w, Instant::now()));
        }
    }

    /// Whether the window needs redrawing every frame for it.
    pub fn running(&self) -> bool {
        self.shown.is_some_and(|(_, at)| at.elapsed() < APPEAR)
    }

    /// 0 when it just appeared, 1 once settled (or when nothing is tracked).
    pub fn progress(&self) -> f32 {
        self.shown.map_or(1.0, |(_, at)| appear_progress(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_eases_in_once_and_again_when_another_replaces_it() {
        let mut a = Appear::default();
        assert!(!a.running());
        assert_eq!(a.progress(), 1.0);
        a.track(Some(1));
        assert!(a.running());
        assert!(a.progress() < 0.5);
        // The same dialog on later updates: the animation carries on, it doesn't restart.
        let started = a.shown.unwrap().1;
        a.track(Some(1));
        assert_eq!(a.shown.unwrap().1, started);
        // Once settled it stops asking for frames.
        a.shown = Some((1, Instant::now() - APPEAR * 2));
        assert!(!a.running());
        assert_eq!(a.progress(), 1.0);
        // Another dialog, or none, starts over.
        a.track(Some(2));
        assert!(a.running());
        a.track(None);
        assert!(!a.running());
        assert_eq!(a.progress(), 1.0);
    }
}
