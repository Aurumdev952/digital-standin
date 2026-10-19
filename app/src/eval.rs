//! Scores a replay against hand-made labels of what a good listener would have reacted to.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::replay::ReplayResult;

#[derive(Debug, Deserialize)]
pub struct Labels {
    #[serde(rename = "event", default)]
    pub events: Vec<Label>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// The room laughs or applauds: expect a smile or laugh.
    Laughter,
    /// Someone asks the listener directly: expect an alert and a thinking look.
    QuestionToMe,
    /// The listener's name is said: expect an alert.
    Name,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct Label {
    pub kind: LabelKind,
    pub start: f64,
    #[serde(default)]
    pub end: Option<f64>,
}

impl Labels {
    pub fn load(path: &Path) -> Result<Self> {
        toml::from_str(&std::fs::read_to_string(path)?).with_context(|| format!("parsing {}", path.display()))
    }
}

const LATE: f64 = 2.5;

pub fn score(labels: &Labels, r: &ReplayResult) -> String {
    let mut out = String::new();
    let minutes = r.duration / 60.0;
    let mut by_state: BTreeMap<&str, usize> = BTreeMap::new();
    for x in &r.reactions {
        *by_state.entry(x.state.as_str()).or_default() += 1;
    }
    out += &format!("duration {:.1} min, {} reactions ({:.1}/min): {by_state:?}\n", minutes, r.reactions.len(), r.reactions.len() as f64 / minutes);
    out += &format!("alerts: {}\n", r.alerts.len());
    if !r.decider_ms.is_empty() {
        let mut ms = r.decider_ms.clone();
        ms.sort_by(f32::total_cmp);
        out += &format!(
            "decider: {} calls, p50 {:.0} ms, p95 {:.0} ms\n",
            ms.len(),
            ms[ms.len() / 2],
            ms[((ms.len() - 1) as f64 * 0.95) as usize]
        );
    }

    let mirth = |s: &str| s == "smile" || s == "laugh";
    let laughs: Vec<&Label> = labels.events.iter().filter(|l| l.kind == LabelKind::Laughter).collect();
    if !laughs.is_empty() {
        let mut hits = 0;
        let mut latency = Vec::new();
        for l in &laughs {
            let end = l.end.unwrap_or(l.start) + LATE;
            if let Some(x) = r.reactions.iter().find(|x| mirth(&x.state) && x.t >= l.start && x.t <= end) {
                hits += 1;
                latency.push(x.t - l.start);
            }
        }
        let spurious = r
            .reactions
            .iter()
            .filter(|x| mirth(&x.state))
            .filter(|x| !laughs.iter().any(|l| x.t >= l.start - 1.0 && x.t <= l.end.unwrap_or(l.start) + LATE))
            .count();
        latency.sort_by(f64::total_cmp);
        let median = latency.get(latency.len() / 2).copied().unwrap_or(f64::NAN);
        out += &format!(
            "laughter: recall {hits}/{} ({:.0}%), median reaction {median:.2} s after onset, {spurious} smiles/laughs outside labelled laughter\n",
            laughs.len(),
            100.0 * hits as f64 / laughs.len() as f64
        );
    }

    for (kind, name) in [(LabelKind::QuestionToMe, "question to me"), (LabelKind::Name, "name mention")] {
        let qs: Vec<&Label> = labels.events.iter().filter(|l| l.kind == kind).collect();
        if qs.is_empty() {
            continue;
        }
        let alerted = qs
            .iter()
            .filter(|l| r.alerts.iter().any(|(t, _)| *t >= l.start - 0.5 && *t <= l.end.unwrap_or(l.start) + 4.0))
            .count();
        let thought = qs
            .iter()
            .filter(|l| r.reactions.iter().any(|x| x.state == "thinking" && x.t >= l.start && x.t <= l.end.unwrap_or(l.start) + 4.0))
            .count();
        out += &format!("{name}: alerted {alerted}/{}, thinking shown {thought}/{}\n", qs.len(), qs.len());
    }
    let labelled_alert = |t: f64| {
        labels
            .events
            .iter()
            .any(|l| l.kind != LabelKind::Laughter && t >= l.start - 0.5 && t <= l.end.unwrap_or(l.start) + 4.0)
    };
    let false_alerts = r.alerts.iter().filter(|(t, _)| !labelled_alert(*t)).count();
    out += &format!("alerts outside labelled questions or mentions: {false_alerts}\n");
    out
}
