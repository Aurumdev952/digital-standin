//! Streaming speech-to-text over the meeting audio with NVIDIA Nemotron (cache-aware FastConformer).

use std::path::Path;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context, Result};
use audio::SAMPLE_RATE;
use cues::Feed;
use crossbeam_channel::{Receiver, Sender};
pub use parakeet_rs::ort::environment::Environment;
use parakeet_rs::{ExecutionConfig, ExecutionProvider, Nemotron};

/// Newly recognised text. `text` continues the previous `Words` and carries its own leading
/// space when it starts a new word.
#[derive(Debug, Clone)]
pub struct Words {
    pub text: String,
    /// Stream time, in seconds, of the end of the audio that produced this text.
    pub audio_end: f64,
    /// GPU time spent on the chunk that produced this text.
    pub compute_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Cuda,
    Cpu,
}

pub struct Recognizer {
    model: Nemotron,
    fed: u64,
}

impl Recognizer {
    /// `env` must be the process's single ONNX Runtime environment; build it once with
    /// [`environment`] and share it with every other model.
    pub fn load(env: &Environment, model_dir: &Path, device: Device) -> Result<Self> {
        let provider = match device {
            Device::Cuda => ExecutionProvider::Cuda,
            Device::Cpu => ExecutionProvider::Cpu,
        };
        let cfg = ExecutionConfig::new().with_environment(env.clone()).with_execution_provider(provider);
        let model = Nemotron::from_pretrained(model_dir, Some(cfg))
            .with_context(|| format!("loading Nemotron from {}", model_dir.display()))?;
        Ok(Self { model, fed: 0 })
    }

    pub fn chunk_secs(&self) -> f64 {
        self.model.chunk_samples() as f64 / SAMPLE_RATE as f64
    }

    /// Feeds samples ending at stream sample `end` and returns any text the model committed. The
    /// model buffers internally and runs once per full chunk.
    pub fn feed(&mut self, samples: &[f32], end: u64) -> Result<Option<Words>> {
        let t = Instant::now();
        let text = self.model.transcribe_chunk(samples)?;
        self.fed = end;
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(Words {
            text,
            audio_end: end as f64 / SAMPLE_RATE as f64,
            compute_ms: t.elapsed().as_secs_f32() * 1000.0,
        }))
    }

    /// Starts over, for audio that does not continue what was fed before.
    pub fn reset(&mut self) {
        self.model.reset();
    }

    pub fn apply(&mut self, feed: Feed) -> Result<Option<Words>> {
        match feed {
            Feed::Audio(b) => self.feed(&b.samples, b.end()),
            Feed::Reset => {
                self.reset();
                Ok(None)
            }
        }
    }

    /// Pushes silence through so the last words of the stream are emitted.
    pub fn flush(&mut self) -> Result<Vec<Words>> {
        let silence = vec![0.0; self.model.chunk_samples()];
        let mut out = Vec::new();
        for _ in 0..3 {
            let end = self.fed + silence.len() as u64;
            out.extend(self.feed(&silence, end)?);
        }
        Ok(out)
    }
}

/// The process's ONNX Runtime environment. Keep it alive for the life of the process: the runtime
/// refuses to create a second one even after the first is dropped.
pub fn environment() -> Result<Environment> {
    Ok(parakeet_rs::ort::init().with_name("digital-standin").build()?)
}

/// Runs the recognizer on its own thread until the audio channel closes.
pub fn spawn(mut rec: Recognizer, feeds: Receiver<Feed>, out: Sender<Words>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("asr".into())
        .spawn(move || {
            for feed in feeds {
                match rec.apply(feed) {
                    Ok(Some(w)) => {
                        if out.send(w).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::error!("asr: {e:#}"),
                }
            }
            if let Ok(rest) = rec.flush() {
                rest.into_iter().for_each(|w| drop(out.send(w)));
            }
        })
        .expect("spawn asr thread")
}
