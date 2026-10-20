// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fixed, process-local startup timings. No paths, requests, or DNS data are saved.
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
pub enum Phase {
    MainEntered,
    BuilderReady,
    SetupEntered,
    TrayReady,
    MonitorReady,
    SetupFinished,
    RunReady,
    NavigationStarted,
    NavigationFinished,
    InterfaceContext,
    StatusStarted,
    StatusFinished,
    AdaptersStarted,
    AdaptersFinished,
}
const NAMES: [&str; 14] = [
    "main_entered",
    "builder_ready",
    "setup_entered",
    "tray_ready",
    "monitor_ready",
    "setup_finished",
    "run_ready",
    "navigation_started",
    "navigation_finished",
    "interface_context",
    "status_started",
    "status_finished",
    "adapters_started",
    "adapters_finished",
];

struct Inner {
    start: Instant,
    started_unix_ms: f64,
    phases: Mutex<[Option<f64>; NAMES.len()]>,
}
#[derive(Clone)]
pub struct StartupTimeline(Arc<Inner>);

#[derive(serde::Serialize)]
pub struct Snapshot {
    pub schema: u32,
    pub main_entered_unix_ms: f64,
    pub phases_ms: std::collections::BTreeMap<&'static str, Option<f64>>,
}

impl StartupTimeline {
    pub fn new() -> Self {
        let timeline = Self(Arc::new(Inner {
            start: Instant::now(),
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                * 1000.0,
            phases: Mutex::new([None; NAMES.len()]),
        }));
        timeline.mark(Phase::MainEntered);
        timeline
    }

    // Return whether this is the first occurrence, so a repeated request cannot
    // finish the span belonging to an earlier, still-running request.
    pub fn mark(&self, phase: Phase) -> bool {
        let mut phases = self.0.phases.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &mut phases[phase as usize];
        if slot.is_some() {
            return false;
        }
        *slot = Some(self.0.start.elapsed().as_secs_f64() * 1000.0);
        true
    }

    pub fn snapshot(&self) -> Snapshot {
        let phases = self.0.phases.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            schema: 1,
            main_entered_unix_ms: self.0.started_unix_ms,
            phases_ms: NAMES.into_iter().zip(*phases).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_read_claims_preserve_the_first_span_and_fixed_storage() {
        let timeline = StartupTimeline::new();
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let timeline = timeline.clone();
                std::thread::spawn(move || timeline.mark(Phase::StatusStarted))
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .filter_map(|t| t.join().ok())
                .filter(|b| *b)
                .count(),
            1
        );
        assert!(timeline.mark(Phase::StatusFinished));
        let snapshot = timeline.snapshot();
        assert!(!timeline.mark(Phase::StatusStarted));
        assert!(!timeline.mark(Phase::StatusFinished));
        assert_eq!(snapshot.phases_ms, timeline.snapshot().phases_ms);
        assert_eq!(snapshot.phases_ms.len(), NAMES.len());
        assert!(snapshot.phases_ms["status_finished"] >= snapshot.phases_ms["status_started"]);
        assert_eq!(snapshot.phases_ms["adapters_finished"], None);
    }
}
