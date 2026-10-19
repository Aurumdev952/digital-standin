//! The request we send to pplx-decider, the answer we read back, and the Unix-socket client.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

/// A state the stand-in can show, as the decider sees it.
#[derive(Debug, Clone)]
pub struct StateInfo {
    pub name: String,
    pub description: String,
}

/// Everything the decider is told about the moment.
#[derive(Debug, Clone)]
pub struct Moment<'a> {
    pub listener: &'a str,
    pub transcript: String,
    pub latest: &'a str,
    pub latest_finished: bool,
    pub secs_since_speech: f64,
    pub room_laughter: f32,
    pub current: &'a str,
    pub secs_since_reaction: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub reaction: BTreeMap<String, f32>,
    pub addressed: f32,
    pub question: f32,
    /// Expected engagement level scaled to 0..1.
    pub engagement: f32,
    pub ms: f32,
}

impl Decision {
    pub fn best(&self) -> (&str, f32) {
        self.reaction
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, v)| (k.as_str(), *v))
            .unwrap_or(("", 0.0))
    }
}

const ENGAGEMENT: [&str; 3] = [
    "Dull, routine or off-topic chatter.",
    "Ordinary discussion.",
    "Engaging, important, emotional or lively discussion.",
];

pub fn request(m: &Moment, states: &[StateInfo]) -> Value {
    let listener = if m.listener.is_empty() { "the listener" } else { m.listener };
    let criteria: serde_json::Map<String, Value> =
        states.iter().map(|s| (s.name.clone(), Value::String(s.description.clone()))).collect();
    json!({
        "state": {
            "situation": format!(
                "{listener} is in a video meeting with the camera on and the microphone muted, listening without \
                 speaking. Pick the nonverbal reaction a natural, attentive listener would show right now."),
            "transcript_of_meeting_audio": m.transcript,
            "latest_utterance": m.latest,
            "latest_utterance_finished": m.latest_finished,
            "seconds_since_anyone_spoke": round1(m.secs_since_speech),
            "room_laughter_level_0_to_1": round1(m.room_laughter as f64),
            "listener_current_expression": m.current,
            "seconds_since_listener_last_reacted": round1(m.secs_since_reaction),
        },
        "questions": {
            "reaction": {
                "type": "choice",
                "instructions": "Which visible reaction should the listener show now? Keep the neutral listening \
                                 state unless a reaction is clearly natural at this moment.",
                "criteria": criteria,
            },
            "addressed": {
                "type": "noul",
                "instructions": format!(
                    "Is the latest utterance a question or request directed at {listener} personally, so they \
                     would be expected to answer or act?"),
            },
            "question": {
                "type": "noul",
                "instructions": "Does the latest utterance ask a question?",
            },
            "engagement": {
                "type": "score",
                "instructions": "How engaging is the conversation right now?",
                "criteria": ENGAGEMENT,
            },
        }
    })
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn parse(response: &Value, ms: f32) -> Result<Decision> {
    let a = &response["answers"];
    let probs = a["reaction"]["probabilities"].as_object().ok_or_else(|| anyhow!("no reaction probabilities"))?;
    let reaction = probs.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0) as f32)).collect();
    let f = |q: &str, field: &str| a[q][field].as_f64().map(|x| x as f32).ok_or_else(|| anyhow!("missing {q}.{field}"));
    let levels = (ENGAGEMENT.len() - 1) as f32;
    Ok(Decision {
        reaction,
        addressed: f("addressed", "noul")?,
        question: f("question", "noul")?,
        engagement: f("engagement", "score")? / levels,
        ms,
    })
}

pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/tmp".into())
        .join("digital-standin-decider.sock")
}

/// Blocking client. The brain keeps at most one request in flight, so no multiplexing is needed.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    pub fn connect(path: &Path, timeout: Duration) -> Result<Self> {
        let s = UnixStream::connect(path).with_context(|| format!("connecting to {}", path.display()))?;
        s.set_read_timeout(Some(timeout))?;
        Ok(Self { reader: BufReader::new(s.try_clone()?), writer: s, next_id: 1 })
    }

    fn call(&mut self, msg: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = msg;
        msg["id"] = id.into();
        self.writer.write_all(format!("{msg}\n").as_bytes())?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            bail!("decider closed the connection");
        }
        let reply: Value = serde_json::from_str(&line)?;
        if reply["id"] != id {
            bail!("reply id {} does not match request {id}", reply["id"]);
        }
        if let Some(e) = reply["error"].as_str() {
            bail!("decider: {e}");
        }
        Ok(reply)
    }

    pub fn ping(&mut self) -> Result<Value> {
        self.call(json!({"ping": true}))
    }

    pub fn decide(&mut self, request: &Value) -> Result<Decision> {
        let reply = self.call(json!({ "request": request }))?;
        parse(&reply["response"], reply["ms"].as_f64().unwrap_or(0.0) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lists_every_state_and_parse_reads_the_answers() {
        let states = vec![
            StateInfo { name: "idle".into(), description: "neutral".into() },
            StateInfo { name: "nod".into(), description: "agree".into() },
        ];
        let m = Moment {
            listener: "Sam",
            transcript: "[ 1s ago] hi".into(),
            latest: "hi",
            latest_finished: true,
            secs_since_speech: 1.04,
            room_laughter: 0.0,
            current: "idle",
            secs_since_reaction: 3.0,
        };
        let r = request(&m, &states);
        assert_eq!(r["questions"]["reaction"]["criteria"]["nod"], "agree");
        assert_eq!(r["state"]["seconds_since_anyone_spoke"], 1.0);
        assert!(r["questions"]["addressed"]["instructions"].as_str().unwrap().contains("Sam"));

        let resp = json!({"answers": {
            "reaction": {"type": "choice", "choice": "nod", "probabilities": {"idle": 0.3, "nod": 0.7}, "confidence": 0.7},
            "addressed": {"type": "noul", "noul": 0.1},
            "question": {"type": "noul", "noul": 0.2},
            "engagement": {"type": "score", "score": 1.5, "probabilities": {}, "legend": {}, "confidence": 0.5},
        }});
        let d = parse(&resp, 40.0).unwrap();
        assert_eq!(d.best(), ("nod", 0.7));
        assert_eq!((d.addressed, d.question, d.engagement, d.ms), (0.1, 0.2, 0.75, 40.0));
    }
}
