//! Story timers (`TimerLaunch`, `ObjectTimerLaunch`, `RealtimeObjectTimerLaunch`), driven by
//! the sim clock.
//!
//! All of them go to `esv::GameTimerSystem::LEGACY_StartTimer(name, entity, seconds, repeat,
//! flags, …)`, keyed by `(name, entity)`; the global timers use the null entity. The story
//! gives milliseconds, the engine stores `ms / 1000.0` as an `f32`. Flags: `TimerLaunch` 1;
//! `ObjectTimerLaunch` 8, plus 1 if its fourth argument is non-zero; `RealtimeObjectTimerLaunch`
//! 8 | 16. `ObjectTimerCancel` and `RealtimeObjectTimerCancel` both call `LEGACY_StopTimer`,
//! so either cancels either kind (the trace cancels a realtime timer with `ObjectTimerCancel`).
//! A finished timer raises `TimerFinished(name)` or `ObjectTimerFinished(entity, name)`
//! (`esv::OsirisGameFunctions::OnGameTimerFinished`).
//!
//! Launching a timer that is already running restarts it: the trace launches the same object
//! timer twice in one event and sees one `ObjectTimerFinished`.

use crate::guid::Guid;

/// `esv::EGameTimerFlags` bits as the Osiris functions pass them. Only their values are known;
/// the sim keeps them for inspection but does not change behaviour on them (no pausing yet).
pub mod flags {
    /// Set by `TimerLaunch` and by `ObjectTimerLaunch(…, 1)`.
    pub const STORY: u8 = 1;
    /// Object timers.
    pub const OBJECT: u8 = 8;
    /// `RealtimeObjectTimerLaunch`.
    pub const REALTIME: u8 = 16;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timer {
    pub name: String,
    /// Null for global timers.
    pub object: Guid,
    /// Sim time (seconds) at which it fires.
    pub due: f64,
    pub flags: u8,
    /// Launch sequence number; breaks ties between timers due at the same time.
    pub seq: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Timers {
    active: Vec<Timer>,
    seq: u64,
}

impl Timers {
    /// Start (or restart) a timer.
    // bg3: esv::GameTimerSystem::LEGACY_StartTimer (4.1.1.7631656)
    pub fn start(&mut self, now: f64, name: &str, object: Guid, ms: i32, flags: u8) {
        self.stop(name, object);
        let secs = (ms as u32 as f32) / 1000.0;
        self.seq += 1;
        self.active.push(Timer {
            name: name.to_owned(),
            object,
            due: now + secs as f64,
            flags,
            seq: self.seq,
        });
    }

    /// Stop a timer; false if none was running.
    // bg3: esv::GameTimerSystem::LEGACY_StopTimer (4.1.1.7631656)
    pub fn stop(&mut self, name: &str, object: Guid) -> bool {
        let before = self.active.len();
        self.active
            .retain(|t| !(t.object == object && t.name == name));
        self.active.len() != before
    }

    pub fn exists(&self, name: &str, object: Guid) -> bool {
        self.active
            .iter()
            .any(|t| t.object == object && t.name == name)
    }

    /// Remove and return every timer due at or before `now`, in firing order.
    pub fn take_due(&mut self, now: f64) -> Vec<Timer> {
        let mut due: Vec<Timer> = Vec::new();
        self.active.retain(|t| {
            if t.due <= now {
                due.push(t.clone());
                false
            } else {
                true
            }
        });
        due.sort_by(|a, b| a.due.total_cmp(&b.due).then(a.seq.cmp(&b.seq)));
        due
    }

    /// The earliest due time, if any timer runs.
    pub fn next_due(&self) -> Option<f64> {
        self.active.iter().map(|t| t.due).min_by(f64::total_cmp)
    }

    pub fn active(&self) -> &[Timer] {
        &self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_cancel_and_order() {
        let mut t = Timers::default();
        t.start(0.0, "a", Guid(1), 1000, flags::OBJECT);
        t.start(0.0, "b", Guid::NULL, 500, flags::STORY);
        t.start(0.2, "a", Guid(1), 1000, flags::OBJECT);
        assert_eq!(t.active().len(), 2);
        t.start(0.0, "c", Guid(2), 0, flags::OBJECT);
        assert!(t.stop("c", Guid(2)));
        assert!(!t.stop("c", Guid(2)));
        let due: Vec<String> = t.take_due(1.0).into_iter().map(|t| t.name).collect();
        assert_eq!(due, ["b"]);
        let due: Vec<String> = t.take_due(1.2).into_iter().map(|t| t.name).collect();
        assert_eq!(due, ["a"]);
        assert_eq!(t.next_due(), None);
    }
}
