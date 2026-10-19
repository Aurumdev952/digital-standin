//! The hybrid policy. Reflexes react to laughter and keep up back-channel nods within a few hundred
//! milliseconds; the decider is consulted at the end of each utterance (and periodically during long
//! ones) for the subtler calls. Everything is driven by an explicit clock so replays are exact.

use std::collections::HashMap;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::Value;

use crate::config::BrainConfig;
use crate::decider::{self, Decision, Moment, StateInfo};
use crate::names::mentions;
use crate::transcript::Transcript;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Nod,
    Decider,
    /// Someone asked the listener something directly.
    Addressed,
    Laughter,
    Manual,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    pub state: String,
    /// Crossfade now instead of letting the current one-shot finish.
    pub urgent: bool,
    pub intensity: Option<f32>,
    pub source: Source,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub reason: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    React(Reaction),
    /// Send this request to the decider and report back with [`Brain::on_decision`].
    Ask(Value),
    Alert(Alert),
}

#[derive(Debug, Clone)]
struct Scheduled {
    at: f64,
    reaction: Reaction,
}

/// What the room sounds like right now.
#[derive(Debug, Default, Clone, Copy)]
pub struct Room {
    pub speaking: bool,
    pub last_speech_at: f64,
    /// When the current run of speech began; short gaps do not end a run.
    pub run_start: Option<f64>,
    pub laughter: f32,
    pub applause: f32,
}

pub struct Brain {
    cfg: BrainConfig,
    states: Vec<StateInfo>,
    base: String,
    transcript: Transcript,
    room: Room,
    showing: String,
    last_reaction_at: f64,
    last_by_state: HashMap<String, f64>,
    scheduled: Option<Scheduled>,
    /// When the outstanding decider request was sent, and whether it covered a finished utterance.
    in_flight: Option<(f64, bool)>,
    ask_after_flight: bool,
    last_ask_at: f64,
    words_since_ask: bool,
    engagement: f32,
    next_nod_at: Option<f64>,
    last_alert_at: f64,
    last_decision: Option<Decision>,
    rng: SmallRng,
}

const DECIDER_TIMEOUT_SECS: f64 = 3.0;
const RUN_GAP_SECS: f64 = 1.5;
const MAX_DEFER_SECS: f64 = 2.0;

impl Brain {
    pub fn new(cfg: BrainConfig, states: Vec<StateInfo>, base: String, seed: u64) -> Self {
        Self {
            cfg,
            showing: base.clone(),
            states,
            base,
            transcript: Transcript::default(),
            room: Room::default(),
            last_reaction_at: f64::NEG_INFINITY,
            last_by_state: HashMap::new(),
            scheduled: None,
            in_flight: None,
            ask_after_flight: false,
            last_ask_at: f64::NEG_INFINITY,
            words_since_ask: false,
            engagement: 0.5,
            next_nod_at: None,
            last_alert_at: f64::NEG_INFINITY,
            last_decision: None,
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    pub fn set_config(&mut self, cfg: BrainConfig) {
        self.cfg = cfg;
    }

    pub fn config(&self) -> &BrainConfig {
        &self.cfg
    }

    pub fn room(&self) -> Room {
        self.room
    }

    pub fn engagement(&self) -> f32 {
        self.engagement
    }

    pub fn last_decision(&self) -> Option<&Decision> {
        self.last_decision.as_ref()
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The state the video is actually showing, reported back by the player.
    pub fn set_showing(&mut self, state: &str) {
        if self.showing != state {
            self.showing = state.to_string();
        }
    }

    pub fn manual(&mut self, now: f64, state: &str) -> Vec<Output> {
        let r = Reaction { state: state.to_string(), urgent: true, intensity: None, source: Source::Manual, reason: "manual".into() };
        self.schedule(now, 0.0, r);
        self.fire_due(now)
    }

    pub fn on_words(&mut self, now: f64, text: &str) -> Vec<Output> {
        let before = self.transcript.open().map(|u| u.text.len()).unwrap_or(0);
        self.transcript.push(now, text);
        self.words_since_ask = true;
        self.room.last_speech_at = now;
        self.room.run_start.get_or_insert(now);
        let utter = self.transcript.open().map(|u| u.text.clone()).unwrap_or_default();
        // The last word may still grow ("Sam" then "ia"), so judge only words that have ended. Look
        // back far enough to catch a name that was the unfinished word last time.
        let ended = utter.rfind(|c: char| !c.is_alphanumeric() && c != '\'').unwrap_or(0);
        let from = utter.floor_char_boundary(before.saturating_sub(24)).min(ended);
        self.name_alert(now, &utter[from..ended], &utter).into_iter().collect()
    }

    fn name_alert(&mut self, now: f64, window: &str, utterance: &str) -> Option<Output> {
        if now - self.last_alert_at < self.cfg.alert_gap_secs {
            return None;
        }
        let alias = mentions(window, &self.cfg.aliases)?;
        self.last_alert_at = now;
        Some(Output::Alert(Alert { reason: format!("someone said \"{alias}\""), text: utterance.to_string() }))
    }

    pub fn on_cues(&mut self, now: f64, speaking: bool, laughter: f32, applause: f32) -> Vec<Output> {
        self.room.speaking = speaking;
        self.room.laughter = laughter;
        self.room.applause = applause;
        if speaking {
            self.room.last_speech_at = now;
            self.room.run_start.get_or_insert(now);
        } else if now - self.room.last_speech_at > RUN_GAP_SECS {
            self.room.run_start = None;
            self.next_nod_at = None;
        }
        let laugh_state = if laughter >= self.cfg.big_laugh {
            self.first_available(&["laugh", "smile"])
        } else if laughter >= self.cfg.laugh_threshold || applause >= self.cfg.applause_threshold {
            self.first_available(&["smile", "laugh"])
        } else {
            None
        };
        if let Some(state) = laugh_state {
            let reason = format!("laughter {laughter:.2}, applause {applause:.2}");
            // Detection already lags laughter onset by about half a second, a human-like delay.
            // Lower thresholds did not detect sooner through the pipeline, only added stray smiles.
            self.schedule(now, 0.0, Reaction { state, urgent: true, intensity: None, source: Source::Laughter, reason });
        }
        self.fire_due(now)
    }

    pub fn on_decision(&mut self, now: f64, result: anyhow::Result<Decision>) -> Vec<Output> {
        let finished = self.in_flight.take().is_some_and(|(_, f)| f);
        let mut out = Vec::new();
        let d = match result {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("decider: {e:#}");
                return out;
            }
        };
        self.engagement = 0.7 * self.engagement + 0.3 * d.engagement;
        let latest = self.transcript.last().map(|u| u.text.clone()).unwrap_or_default();
        let addressed = finished && d.addressed >= self.cfg.alert_threshold;
        if addressed && now - self.last_alert_at >= self.cfg.alert_gap_secs {
            self.last_alert_at = now;
            out.push(Output::Alert(Alert { reason: format!("you may have been asked (p={:.2})", d.addressed), text: latest }));
        }
        let (best, p) = d
            .reaction
            .iter()
            .filter(|(s, _)| !self.cfg.disabled.contains(s))
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(s, p)| (s.as_str(), *p))
            .unwrap_or(("", 0.0));
        let pick = if addressed {
            self.first_available(&["thinking"]).map(|s| (s, d.addressed, Source::Addressed))
        } else if best != self.base && p >= self.cfg.threshold_for(best) {
            Some((best.to_string(), p, Source::Decider))
        } else {
            None
        };
        if let Some((state, p, source)) = pick {
            let delay = self.rng.random_range(self.cfg.delay_min_secs..=self.cfg.delay_max_secs.max(self.cfg.delay_min_secs));
            let reason = format!("decider {state} p={p:.2} (q={:.2}, addressed={:.2})", d.question, d.addressed);
            let urgent = source == Source::Addressed;
            self.schedule(now, delay, Reaction { state, urgent, intensity: None, source, reason });
        }
        self.last_decision = Some(d);
        if self.ask_after_flight {
            self.ask_after_flight = false;
            out.extend(self.ask(now, true));
        }
        out.extend(self.fire_due(now));
        out
    }

    pub fn tick(&mut self, now: f64) -> Vec<Output> {
        let mut out = Vec::new();
        if self.in_flight.is_some_and(|(t, _)| now - t > DECIDER_TIMEOUT_SECS) {
            tracing::warn!("decider timed out");
            self.in_flight = None;
        }
        let gap = self.cfg.end_of_utterance_secs;
        if let Some(closed) = self.transcript.close_if_paused(now, self.room.speaking, gap) {
            // The utterance's last word has ended now, so it can be a name too.
            let text = closed.text.clone();
            let tail = &text[text.floor_char_boundary(text.len().saturating_sub(24))..];
            out.extend(self.name_alert(now, tail, &text));
            out.extend(self.ask(now, true));
        } else if let Some(open) = self.transcript.open()
            && self.words_since_ask
            && now - self.last_ask_at.max(open.start) >= self.cfg.decide_every_secs
            && self.in_flight.is_none()
        {
            out.extend(self.ask(now, false));
        }
        self.transcript.forget_before(now - self.cfg.context_secs);
        self.maybe_nod(now);
        out.extend(self.fire_due(now));
        out
    }

    fn ask(&mut self, now: f64, finished: bool) -> Option<Output> {
        if self.in_flight.is_some() {
            self.ask_after_flight |= finished;
            return None;
        }
        let latest = self.transcript.last()?.text.clone();
        let m = Moment {
            listener: self.cfg.aliases.first().map(String::as_str).unwrap_or(""),
            transcript: self.transcript.window_words(now, self.cfg.context_secs, self.cfg.context_words),
            latest: &latest,
            latest_finished: finished,
            secs_since_speech: now - self.room.last_speech_at,
            room_laughter: self.room.laughter,
            current: &self.showing,
            secs_since_reaction: (now - self.last_reaction_at).min(600.0),
        };
        let req = decider::request(&m, &self.states);
        self.in_flight = Some((now, finished));
        self.last_ask_at = now;
        self.words_since_ask = false;
        Some(Output::Ask(req))
    }

    fn maybe_nod(&mut self, now: f64) {
        let Some(run_start) = self.room.run_start else { return };
        let recently_spoke = now - self.room.last_speech_at < 1.0;
        if !recently_spoke || self.showing != self.base {
            return;
        }
        // Stretch the interval when the talk is dull, shorten it when lively.
        let scale = 1.3 - 0.6 * self.engagement as f64;
        let (lo, hi) = (self.cfg.nod_min_secs * scale, self.cfg.nod_max_secs.max(self.cfg.nod_min_secs) * scale);
        let since = run_start.max(self.last_reaction_at);
        let due = *self.next_nod_at.get_or_insert_with(|| since + self.rng.random_range(lo..=hi));
        if now >= due {
            self.next_nod_at = None;
            if let Some(nod) = self.first_available(&["nod"]) {
                let intensity = Some(0.3 + 0.5 * self.engagement);
                let r = Reaction { state: nod, urgent: false, intensity, source: Source::Nod, reason: "back-channel nod".into() };
                self.schedule(now, 0.0, r);
            }
        }
    }

    fn first_available(&self, names: &[&str]) -> Option<String> {
        names
            .iter()
            .find(|n| self.states.iter().any(|s| s.name == **n) && !self.cfg.disabled.iter().any(|d| d == *n))
            .map(|n| n.to_string())
    }

    fn schedule(&mut self, now: f64, delay: f64, reaction: Reaction) {
        // Keep a pending reaction that outranks this one, or is the same state already on its way;
        // re-scheduling it would keep pushing it into the future while a cue persists.
        if self.scheduled.as_ref().is_some_and(|s| s.reaction.source > reaction.source || s.reaction.state == reaction.state) {
            return;
        }
        self.scheduled = Some(Scheduled { at: now + delay, reaction });
    }

    fn fire_due(&mut self, now: f64) -> Vec<Output> {
        let Some(s) = self.scheduled.take_if(|s| s.at <= now) else { return Vec::new() };
        let r = s.reaction;
        let manual = r.source == Source::Manual;
        let since_any = now - self.last_reaction_at;
        let since_same = now - self.last_by_state.get(&r.state).copied().unwrap_or(f64::NEG_INFINITY);
        // Being addressed and laughter may cut into another reaction; nothing repeats within the
        // same-state gap.
        let gap_ok = since_any >= self.cfg.min_gap_secs || r.source >= Source::Addressed;
        if !manual && since_same < self.cfg.same_state_gap_secs {
            return Vec::new();
        }
        if !manual && !gap_ok {
            // Wait out the gap rather than lose the reaction, unless it would be stale by then.
            let when = self.last_reaction_at + self.cfg.min_gap_secs;
            if r.source > Source::Nod && when - s.at <= MAX_DEFER_SECS {
                self.scheduled = Some(Scheduled { at: when, reaction: r });
            }
            return Vec::new();
        }
        self.last_reaction_at = now;
        self.last_by_state.insert(r.state.clone(), now);
        self.next_nod_at = None;
        self.showing = r.state.clone();
        vec![Output::React(r)]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn states() -> Vec<StateInfo> {
        ["idle", "nod", "thinking", "smile", "laugh"]
            .iter()
            .map(|n| StateInfo { name: n.to_string(), description: n.to_string() })
            .collect()
    }

    fn brain() -> Brain {
        let cfg = BrainConfig { aliases: vec!["Sam".into()], ..BrainConfig::default() };
        Brain::new(cfg, states(), "idle".into(), 7)
    }

    fn reactions(out: &[Output]) -> Vec<&str> {
        out.iter()
            .filter_map(|o| match o {
                Output::React(r) => Some(r.state.as_str()),
                _ => None,
            })
            .collect()
    }

    fn decision(state: &str, p: f32, addressed: f32) -> Decision {
        let mut reaction = BTreeMap::from([("idle".to_string(), 1.0 - p)]);
        reaction.insert(state.to_string(), p);
        Decision { reaction, addressed, question: addressed, engagement: 0.5, ms: 30.0 }
    }

    /// Runs the clock from `from` to `to` in 50 ms steps, returning every output.
    fn run(b: &mut Brain, from: f64, to: f64) -> Vec<Output> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to {
            out.extend(b.tick(t));
            t += 0.05;
        }
        out
    }

    #[test]
    fn big_laughter_laughs_as_soon_as_it_is_detected() {
        let mut b = brain();
        let out = b.on_cues(10.0, false, 0.7, 0.0);
        assert_eq!(reactions(&out), vec!["laugh"]);
        assert!(matches!(&out[0], Output::React(r) if r.urgent && r.source == Source::Laughter));
    }

    #[test]
    fn mild_laughter_smiles_and_does_not_repeat_within_the_gap() {
        let mut b = brain();
        assert_eq!(reactions(&b.on_cues(10.0, false, 0.35, 0.0)), vec!["smile"]);
        assert!(reactions(&b.on_cues(12.0, false, 0.35, 0.0)).is_empty(), "same-state gap holds");
        assert!(reactions(&run(&mut b, 12.05, 13.0)).is_empty());
    }

    #[test]
    fn end_of_utterance_asks_once_and_a_confident_answer_reacts() {
        let mut b = brain();
        b.on_cues(1.0, true, 0.0, 0.0);
        b.on_words(1.0, " We shipped the release today.");
        b.on_cues(1.5, false, 0.0, 0.0);
        let asks = run(&mut b, 1.5, 3.0).into_iter().filter(|o| matches!(o, Output::Ask(_))).count();
        assert_eq!(asks, 1);
        assert!(b.on_decision(3.0, Ok(decision("nod", 0.4, 0.0))).is_empty(), "below threshold");
        b.on_words(4.0, " Isn't that great?");
        let asked = run(&mut b, 4.0, 5.0);
        assert!(asked.iter().any(|o| matches!(o, Output::Ask(r) if r["state"]["latest_utterance_finished"] == true)));
        b.on_decision(5.0, Ok(decision("nod", 0.8, 0.0)));
        assert_eq!(reactions(&run(&mut b, 5.0, 6.0)), vec!["nod"]);
    }

    #[test]
    fn being_addressed_alerts_and_thinks() {
        let mut b = brain();
        b.on_words(1.0, " What do you think about it?");
        let ask = run(&mut b, 1.0, 2.0);
        assert!(ask.iter().any(|o| matches!(o, Output::Ask(_))));
        let out = b.on_decision(2.0, Ok(decision("idle", 0.0, 0.9)));
        assert!(out.iter().any(|o| matches!(o, Output::Alert(a) if a.reason.contains("asked"))));
        assert_eq!(reactions(&run(&mut b, 2.0, 3.0)), vec!["thinking"]);
    }

    #[test]
    fn a_name_split_across_chunks_is_judged_as_the_whole_word() {
        let mut b = brain();
        assert!(b.on_words(1.0, " thanks for the link to Sam").is_empty(), "the word may continue");
        assert!(b.on_words(1.2, "ia").is_empty());
        assert!(b.on_words(1.4, " on the roles").is_empty(), "Samia is not Sam");
    }

    #[test]
    fn a_name_ending_an_utterance_alerts_when_it_closes() {
        let mut b = brain();
        b.on_words(1.0, " what do you think Sam");
        let out = run(&mut b, 1.0, 2.5);
        assert!(out.iter().any(|o| matches!(o, Output::Alert(a) if a.reason.contains("Sam"))), "{out:?}");
    }

    #[test]
    fn name_mention_alerts_once_per_gap() {
        let mut b = brain();
        let out = b.on_words(1.0, " thanks, sam, for the update");
        assert!(matches!(&out[..], [Output::Alert(a)] if a.reason.contains("Sam")));
        assert!(b.on_words(2.0, " and Sam again").is_empty());
        assert!(b.on_words(1.0 + b.config().alert_gap_secs + 1.0, " Sam?").len() == 1);
    }

    #[test]
    fn continuous_speech_gets_back_channel_nods_but_silence_does_not() {
        let mut b = brain();
        let mut nods = 0;
        let mut t = 0.0;
        while t < 60.0 {
            b.on_cues(t, true, 0.0, 0.0);
            nods += reactions(&b.tick(t)).iter().filter(|s| **s == "nod").count();
            b.set_showing("idle");
            t += 0.25;
        }
        let cfg = BrainConfig::default();
        let scale_max = 1.3 - 0.6 * 0.5;
        assert!(nods >= (60.0 / (cfg.nod_max_secs * scale_max)) as usize && nods <= (60.0 / cfg.nod_min_secs) as usize + 1, "{nods} nods");
        let mut quiet = 0;
        while t < 120.0 {
            b.on_cues(t, false, 0.0, 0.0);
            quiet += reactions(&b.tick(t)).len();
            t += 0.25;
        }
        assert_eq!(quiet, 0);
    }

    /// Answers outstanding asks with errors until one covers a finished utterance, and returns it.
    fn answer_finished(b: &mut Brain, from: f64, to: f64) -> f64 {
        let mut t = from;
        while t <= to {
            for o in b.tick(t) {
                if let Output::Ask(r) = o {
                    if r["state"]["latest_utterance_finished"] == true {
                        return t;
                    }
                    b.on_decision(t, Err(anyhow::anyhow!("not this one")));
                }
            }
            t += 0.05;
        }
        panic!("no finished ask between {from} and {to}");
    }

    #[test]
    fn a_direct_question_right_after_a_nod_still_shows_thinking() {
        let mut b = brain();
        b.on_cues(9.0, true, 0.0, 0.0);
        b.on_words(9.5, " what do you think Sam?");
        b.on_cues(10.0, false, 0.0, 0.0);
        assert_eq!(reactions(&b.manual(10.0, "nod")), vec!["nod"]);
        let t = answer_finished(&mut b, 10.0, 12.0);
        b.on_decision(t + 0.1, Ok(decision("nod", 0.4, 0.97)));
        let out = run(&mut b, t + 0.1, t + 1.0);
        assert!(matches!(&out[..], [Output::React(r)] if r.state == "thinking" && r.source == Source::Addressed), "{out:?}");
        assert!(t + 1.0 - 10.0 < b.config().min_gap_secs + 1.0, "shown inside what would have been the nod's gap");
    }

    #[test]
    fn a_decider_reaction_blocked_by_the_gap_is_deferred_not_lost() {
        let mut b = brain();
        b.manual(5.0, "smile");
        b.on_words(5.5, " good point");
        let t = answer_finished(&mut b, 5.5, 7.0);
        b.on_decision(t, Ok(decision("nod", 0.9, 0.0)));
        let mut fired = Vec::new();
        let mut now = t;
        while now <= 9.0 {
            if !reactions(&b.tick(now)).is_empty() {
                fired.push(now);
            }
            now += 0.05;
        }
        assert_eq!(fired.len(), 1, "exactly one nod");
        assert!(fired[0] >= 5.0 + b.config().min_gap_secs - 1e-9, "not before the gap ends: {fired:?}");
    }

    #[test]
    fn disabled_states_are_never_chosen_automatically() {
        let mut b = brain();
        let mut cfg = b.config().clone();
        cfg.disabled = vec!["laugh".into(), "nod".into()];
        b.set_config(cfg);
        assert_eq!(reactions(&b.on_cues(10.0, false, 0.9, 0.0)), vec!["smile"], "big laughter falls back to smile");
        b.on_words(20.0, " right");
        let t = answer_finished(&mut b, 20.0, 22.0);
        b.on_decision(t, Ok(decision("nod", 0.9, 0.0)));
        assert!(reactions(&run(&mut b, t, t + 2.0)).is_empty(), "nod is off");
        assert_eq!(reactions(&b.manual(30.0, "nod")), vec!["nod"], "manual still works");
    }

    #[test]
    fn laughter_outranks_a_pending_decider_reaction() {
        let mut b = brain();
        b.on_words(1.0, " fine");
        run(&mut b, 1.0, 2.0);
        b.on_decision(2.0, Ok(decision("nod", 0.9, 0.0)));
        assert_eq!(reactions(&b.on_cues(2.05, false, 0.8, 0.0)), vec!["laugh"]);
        assert!(reactions(&run(&mut b, 2.1, 3.5)).is_empty(), "the pending nod gave way to the laugh");
    }

    #[test]
    fn failed_decision_clears_in_flight_so_the_next_utterance_asks() {
        let mut b = brain();
        b.on_words(1.0, " one");
        run(&mut b, 1.0, 2.0);
        b.on_decision(2.0, Err(anyhow::anyhow!("socket closed")));
        b.on_words(3.0, " two");
        assert!(run(&mut b, 3.0, 4.5).iter().any(|o| matches!(o, Output::Ask(_))));
    }
}
