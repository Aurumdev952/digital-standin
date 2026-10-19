use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use brain::Decision;
use cues::GateStats;

/// How much history the graphs keep.
pub const HISTORY_SECS: f64 = 120.0;

#[derive(Debug, Clone, Copy, Default)]
pub struct CuePoint {
    pub speaking: bool,
    pub laughter: f32,
    pub applause: f32,
}

/// Timestamped samples, trimmed to [`HISTORY_SECS`].
#[derive(Debug, Clone, Default)]
pub struct Series(pub VecDeque<(f64, f32)>);

impl Series {
    pub fn push(&mut self, t: f64, v: f32) {
        self.0.push_back((t, v));
        while self.0.front().is_some_and(|(s, _)| t - s > HISTORY_SECS) {
            self.0.pop_front();
        }
    }

    pub fn last(&self) -> Option<f32> {
        self.0.back().map(|p| p.1)
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub t: f64,
    pub kind: EventKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Reaction,
    Alert,
    Decision,
    Info,
}

/// Every time series the GUI plots.
#[derive(Default)]
pub struct History {
    pub laughter: Series,
    pub applause: Series,
    pub speech: Series,
    pub level: Series,
    pub asr_ms: Series,
    pub decider_ms: Series,
    pub jitter_ms: Series,
    pub cpu_pct: Series,
    pub rss_mb: Series,
    pub gpu_mb: Series,
    pub gpu_util: Series,
    /// Decision-model probability per state.
    pub probs: BTreeMap<String, Series>,
}

/// What the GUI shows, written by the pipeline threads.
#[derive(Default)]
pub struct Telemetry {
    pub now: f64,
    pub transcript: Vec<(f64, String, bool)>,
    pub last_cue: CuePoint,
    pub peak: f32,
    pub decision: Option<Decision>,
    pub decider: String,
    pub asr_ms: f32,
    pub engagement: f32,
    /// The most recent events; `events_total` counts every event ever added.
    pub events: VecDeque<Event>,
    pub events_total: u64,
    pub gate: Option<Arc<GateStats>>,
    pub history: History,
}

impl Telemetry {
    /// Share of the audio so far that reached speech recognition.
    pub fn asr_share(&self) -> Option<f64> {
        let g = self.gate.as_ref()?;
        let total = g.passed_secs() + g.held_secs();
        (total > 0.0).then(|| g.passed_secs() / total)
    }

    pub fn event(&mut self, t: f64, kind: EventKind, text: impl Into<String>) {
        self.events.push_back(Event { t, kind, text: text.into() });
        self.events_total += 1;
        while self.events.len() > 300 {
            self.events.pop_front();
        }
    }

    pub fn cue(&mut self, t: f64, c: CuePoint) {
        self.last_cue = c;
        self.history.laughter.push(t, c.laughter);
        self.history.applause.push(t, c.applause);
        self.history.speech.push(t, c.speaking as u8 as f32);
    }

    pub fn decision(&mut self, t: f64, d: &Decision) {
        self.history.decider_ms.push(t, d.ms);
        for (state, p) in &d.reaction {
            self.history.probs.entry(state.clone()).or_default().push(t, *p);
        }
    }
}
