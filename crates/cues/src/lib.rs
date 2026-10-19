//! Non-verbal cues in the meeting audio: is someone speaking, is the room laughing or applauding.
//! Speech-to-text never transcribes laughter, so these come from a separate AudioSet tagger (CED)
//! running on the CPU through the same ONNX Runtime as the speech model, and a pure-Rust voice
//! activity detector (Earshot).

mod fbank;
mod gate;

use std::collections::VecDeque;
use std::path::Path;
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use audio::{Block, SAMPLE_RATE};
use crossbeam_channel::{Receiver, Sender};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

pub use fbank::Fbank;
pub use gate::{Feed, Gate, GateStats};

/// RMS below which a window cannot hold speech or laughter worth reacting to (-50 dBFS).
const QUIET_RMS: f32 = 0.003_16;
/// RMS of digital silence, as when nothing is playing (-80 dBFS).
const SILENT_RMS: f32 = 1e-4;

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// AudioSet class indices (see `class_labels_indices.csv`).
const LAUGHTER: &[usize] = &[16, 18, 19, 20, 21];
const APPLAUSE: &[usize] = &[66, 67];

#[derive(Debug, Clone, Copy, Default)]
pub struct Cues {
    /// Stream time, in seconds, at the end of the analysed window.
    pub at: f64,
    pub speech: bool,
    pub laughter: f32,
    pub applause: f32,
}

fn session(env: &Environment, path: &Path) -> Result<Session> {
    let mut builder = Session::builder(env)?.with_intra_threads(1).map_err(|e| anyhow::anyhow!("{e}"))?;
    builder.commit_from_file(path).with_context(|| format!("loading {}", path.display()))
}

/// Voice activity from Earshot (pure Rust, 16 ms frames) with hysteresis: speech starts after
/// 160 ms above 0.5 and ends after 320 ms below 0.35.
///
/// Silero VAD ran through ONNX Runtime 31 times a second, and every run grew the process by about
/// 0.55 KB that no session drop released (`examples/ortgrowth.rs`): 150 MB over a 43-minute
/// meeting. Earshot keeps its whole state in a few kilobytes.
#[derive(Default)]
pub struct Vad {
    detector: earshot::Detector,
    pending: Vec<f32>,
    speaking: bool,
    run: usize,
}

const VAD_FRAME: usize = 256;
const VAD_ON_FRAMES: usize = 10;
const VAD_OFF_FRAMES: usize = 20;

impl Vad {
    pub fn speaking(&self) -> bool {
        self.speaking
    }

    pub fn feed(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        let full = self.pending.len() / VAD_FRAME * VAD_FRAME;
        let frames: Vec<f32> = self.pending.drain(..full).collect();
        for frame in frames.chunks_exact(VAD_FRAME) {
            // Digital silence needs no model to know nobody is speaking.
            let p = if rms(frame) < SILENT_RMS { 0.0 } else { self.detector.predict_f32(frame) };
            let flips = if self.speaking { p < 0.35 } else { p >= 0.5 };
            self.run = if flips { self.run + 1 } else { 0 };
            if self.run >= if self.speaking { VAD_OFF_FRAMES } else { VAD_ON_FRAMES } {
                self.speaking = !self.speaking;
                self.run = 0;
            }
        }
    }
}

/// CED-mini AudioSet tagger.
pub struct Tagger {
    session: Session,
    fbank: Fbank,
}

impl Tagger {
    pub fn load(env: &Environment, path: &Path) -> Result<Self> {
        Ok(Self { session: session(env, path)?, fbank: Fbank::new() })
    }

    /// Probabilities for all 527 AudioSet classes.
    pub fn tag(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let (frames, feats) = self.fbank.compute(samples);
        let out = self.session.run(ort::inputs!["feats" => Tensor::from_array(([1usize, 64, frames], feats))?])?;
        Ok(out["prob"].try_extract_tensor::<f32>()?.1.to_vec())
    }
}

pub struct CueDetector {
    vad: Vad,
    tagger: Tagger,
    window: VecDeque<f32>,
    window_len: usize,
    hop: usize,
    since_hop: usize,
    fed: u64,
    tagged: u64,
    skipped: u64,
}

impl CueDetector {
    /// The tuned configuration: CED-small over a 1 s window every 125 ms. On 12 laughs mixed into a
    /// meeting (`scripts/make-laugh-mix.py`) it caught all 12 a median 0.92 s after onset, with no
    /// false episode in 5 minutes of plain meeting audio, where CED-mini caught 11 with 0.6/min.
    ///
    /// `env` must be the process's single ONNX Runtime environment, shared with the speech model.
    pub fn load(env: &Environment, models: &Path) -> Result<Self> {
        Ok(Self::new(Vad::default(), Tagger::load(env, &models.join("ced-small/model.int8.onnx"))?, 0.125))
    }

    pub fn new(vad: Vad, tagger: Tagger, hop_secs: f64) -> Self {
        Self {
            vad,
            tagger,
            window: VecDeque::new(),
            window_len: SAMPLE_RATE as usize,
            hop: (hop_secs * SAMPLE_RATE as f64) as usize,
            since_hop: 0,
            fed: 0,
            tagged: 0,
            skipped: 0,
        }
    }

    pub fn speaking(&self) -> bool {
        self.vad.speaking()
    }

    /// Tagger windows analysed and skipped as too quiet.
    pub fn tagger_runs(&self) -> (u64, u64) {
        (self.tagged, self.skipped)
    }

    /// Returns one [`Cues`] per completed hop over a 1 s window.
    pub fn feed(&mut self, samples: &[f32]) -> Result<Vec<Cues>> {
        let mut out = Vec::new();
        for chunk in samples.chunks(self.hop) {
            self.vad.feed(chunk);
            self.window.extend(chunk);
            let excess = self.window.len().saturating_sub(self.window_len);
            self.window.drain(..excess);
            self.fed += chunk.len() as u64;
            self.since_hop += chunk.len();
            if self.since_hop >= self.hop && self.window.len() == self.window_len {
                self.since_hop = 0;
                let window: Vec<f32> = self.window.iter().copied().collect();
                let (laughter, applause) = if rms(&window) < QUIET_RMS {
                    self.skipped += 1;
                    (0.0, 0.0)
                } else {
                    self.tagged += 1;
                    let probs = self.tagger.tag(&window)?;
                    let max_of = |ids: &[usize]| ids.iter().map(|&i| probs[i]).fold(0.0, f32::max);
                    (max_of(LAUGHTER), max_of(APPLAUSE))
                };
                out.push(Cues { at: self.fed as f64 / SAMPLE_RATE as f64, speech: self.vad.speaking(), laughter, applause });
            }
        }
        Ok(out)
    }
}

/// Analyses every block for cues and passes only speech on to recognition, through `gate`.
pub fn spawn(mut det: CueDetector, mut gate: Gate, blocks: Receiver<Block>, out: Sender<Cues>, speech: Sender<Feed>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("cues".into())
        .spawn(move || {
            for block in blocks {
                match det.feed(&block.samples) {
                    Ok(cs) => {
                        for c in cs {
                            if out.send(c).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => tracing::error!("cues: {e:#}"),
                }
                for f in gate.push(block, det.speaking()) {
                    if speech.send(f).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn cues thread")
}
