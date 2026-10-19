//! Offline run of the whole pipeline on an audio file, on the audio's own clock. Produces a video
//! of the stand-in with the original audio, an SRT of transcript and reactions, and a JSONL log.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use brain::decider::{Client, default_socket};
use brain::{Brain, Output, Settings, StateInfo};
use serde_json::json;
use video::{FrameSink, Library, Manifest, Player, Request, Urgency, sink::FfmpegSink};

pub struct ReplayOptions {
    pub root: PathBuf,
    pub audio: PathBuf,
    pub out: PathBuf,
    pub settings: Settings,
    pub decider: bool,
    /// Pass only speech to recognition, as the live pipeline does.
    pub gate: bool,
    pub seed: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoggedReaction {
    pub t: f64,
    pub state: String,
    pub source: String,
    pub reason: String,
}

pub struct ReplayResult {
    pub reactions: Vec<LoggedReaction>,
    pub alerts: Vec<(f64, String)>,
    pub decider_ms: Vec<f32>,
    pub duration: f64,
    /// Seconds of audio that reached speech recognition.
    pub asr_secs: f64,
    /// Tagger windows analysed and skipped as too quiet.
    pub tagger_runs: (u64, u64),
}

pub fn run(o: ReplayOptions) -> Result<ReplayResult> {
    let lib = Arc::new(Library::load(&Manifest::load(&o.root.join(&o.settings.video.manifest))?)?);
    let fps = lib.output.fps as f64;
    let models = o.root.join("models");
    let env = asr::environment()?;
    let mut rec = asr::Recognizer::load(&env, &o.settings.asr.resolve(&o.root), asr::Device::Cuda)?;
    let mut det = cues::CueDetector::load(&env, &models)?;
    let mut client = if o.decider { Some(Client::connect(&default_socket(), Duration::from_secs(10))?) } else { None };
    let samples = audio::decode_file(&o.audio)?;
    let duration = samples.len() as f64 / audio::SAMPLE_RATE as f64;

    let states: Vec<StateInfo> =
        lib.states.iter().map(|s| StateInfo { name: s.name.clone(), description: s.description.clone() }).collect();
    let mut brain = Brain::new(o.settings.brain.clone(), states, lib.states[lib.base].name.clone(), o.seed);
    let mut player = Player::new(lib.clone(), o.settings.video.blend_frames, o.seed);
    let mut sink = FfmpegSink::create(&o.out.with_extension("mp4"), lib.output, Some(&o.audio))?;
    let mut log = std::io::BufWriter::new(std::fs::File::create(o.out.with_extension("jsonl"))?);
    let mut srt = Srt::default();

    let mut pending: VecDeque<(f64, brain::policy::Output)> = VecDeque::new();
    let mut decisions: VecDeque<(f64, anyhow::Result<brain::Decision>)> = VecDeque::new();
    let mut result = ReplayResult {
        reactions: Vec::new(),
        alerts: Vec::new(),
        decider_ms: Vec::new(),
        duration,
        asr_secs: 0.0,
        tagger_runs: (0, 0),
    };
    let mut gate = cues::Gate::default();
    let per_frame = audio::SAMPLE_RATE as f64 / fps;
    let frames = (duration * fps).ceil() as usize;
    let mut fed = 0usize;

    for k in 0..frames {
        let now = (k + 1) as f64 / fps;
        let upto = (((k + 1) as f64) * per_frame).round().min(samples.len() as f64) as usize;
        let block = audio::Block { start: fed as u64, samples: samples[fed..upto].into() };
        fed = upto;
        let mut outs = Vec::new();
        for c in det.feed(&block.samples)? {
            writeln!(log, "{}", json!({"t": now, "type": "cues", "speech": c.speech, "laughter": c.laughter, "applause": c.applause}))?;
            outs.extend(brain.on_cues(now, c.speech, c.laughter, c.applause));
        }
        let feeds = if o.gate { gate.push(block, det.speaking()) } else { vec![cues::Feed::Audio(block)] };
        for f in feeds {
            if let cues::Feed::Audio(b) = &f {
                result.asr_secs += b.samples.len() as f64 / audio::SAMPLE_RATE as f64;
            }
            if let Some(w) = rec.apply(f)? {
                writeln!(log, "{}", json!({"t": now, "type": "words", "text": w.text, "asr_ms": w.compute_ms}))?;
                srt.words(now, &w.text);
                outs.extend(brain.on_words(now, &w.text));
            }
        }
        while decisions.front().is_some_and(|(t, _)| *t <= now) {
            let (_, d) = decisions.pop_front().expect("checked");
            if let Ok(d) = &d {
                let (best, p) = d.best();
                writeln!(log, "{}", json!({"t": now, "type": "decision", "best": best, "p": p, "probs": d.reaction,
                    "addressed": d.addressed, "question": d.question, "engagement": d.engagement, "ms": d.ms}))?;
            }
            outs.extend(brain.on_decision(now, d));
        }
        let status = player.status();
        brain.set_showing(&lib.states[status.state].name);
        outs.extend(brain.tick(now));
        pending.extend(outs.into_iter().map(|o| (now, o)));

        while let Some((t, o)) = pending.pop_front() {
            match o {
                Output::React(r) => {
                    if let Some(state) = lib.state_index(&r.state) {
                        let urgency = if r.urgent { Urgency::Now } else { Urgency::AtBoundary };
                        player.request(Request { state, urgency, intensity: r.intensity });
                    }
                    let source = format!("{:?}", r.source);
                    writeln!(log, "{}", json!({"t": t, "type": "react", "state": r.state, "source": source, "reason": r.reason}))?;
                    srt.event(t, format!("[{}] {}", r.state, r.reason));
                    result.reactions.push(LoggedReaction { t, state: r.state, source, reason: r.reason });
                }
                Output::Alert(a) => {
                    writeln!(log, "{}", json!({"t": t, "type": "alert", "reason": a.reason, "text": a.text}))?;
                    srt.event(t, format!("ALERT {}", a.reason));
                    result.alerts.push((t, a.reason));
                }
                Output::Ask(req) => {
                    let d = match client.as_mut() {
                        Some(c) => c.decide(&req),
                        None => Err(anyhow::anyhow!("decider disabled")),
                    };
                    // The answer arrives as late as it took the model, on the audio clock.
                    let ms = d.as_ref().map(|d| d.ms).unwrap_or(0.0);
                    if d.is_ok() {
                        result.decider_ms.push(ms);
                    }
                    decisions.push_back((t + ms as f64 / 1000.0 + 0.002, d));
                }
            }
        }
        sink.write_frame(player.next_frame())?;
    }
    for w in rec.flush()? {
        srt.words(duration, &w.text);
    }
    result.tagger_runs = det.tagger_runs();
    sink.finish()?;
    log.flush()?;
    std::fs::write(o.out.with_extension("srt"), srt.render(duration))?;
    Ok(result)
}

/// Captions: the running transcript, with reactions shown as they happen.
#[derive(Default)]
struct Srt {
    line: String,
    line_start: f64,
    cues: Vec<(f64, f64, String)>,
}

impl Srt {
    fn words(&mut self, t: f64, text: &str) {
        if self.line.is_empty() {
            self.line_start = t;
        }
        self.line.push_str(text);
        if self.line.len() > 70 || text.ends_with(['.', '?', '!']) {
            self.cues.push((self.line_start, t + 1.5, self.line.trim().to_string()));
            self.line.clear();
        }
    }

    fn event(&mut self, t: f64, text: String) {
        self.cues.push((t, t + 2.0, text));
    }

    fn render(mut self, end: f64) -> String {
        if !self.line.is_empty() {
            self.cues.push((self.line_start, end, self.line.trim().to_string()));
        }
        self.cues.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ts = |t: f64| {
            let ms = (t.max(0.0) * 1000.0) as u64;
            format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
        };
        self.cues
            .iter()
            .enumerate()
            .map(|(i, (a, b, text))| format!("{}\n{} --> {}\n{}\n", i + 1, ts(*a), ts(*b), text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn default_out(audio: &Path) -> PathBuf {
    let stem = audio.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "replay".into());
    PathBuf::from("out").join(format!("{stem}-replay"))
}
