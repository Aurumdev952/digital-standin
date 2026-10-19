//! Meeting audio as one 16 kHz mono stream, fanned out to every consumer as [`Block`]s.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;

mod capture;

pub use capture::capture_default_sink;

pub const SAMPLE_RATE: u32 = 16_000;

/// Contiguous samples starting at `start`, counted in samples since the stream began.
#[derive(Clone, Debug)]
pub struct Block {
    pub start: u64,
    pub samples: Arc<[f32]>,
}

impl Block {
    pub fn end(&self) -> u64 {
        self.start + self.samples.len() as u64
    }

    pub fn start_secs(&self) -> f64 {
        self.start as f64 / SAMPLE_RATE as f64
    }
}

/// State shared between the audio source and the rest of the app.
#[derive(Default)]
pub struct Tap {
    /// While set, blocks carry silence. Used so our own alert sound is not transcribed.
    pub gate: AtomicBool,
    /// Peak level of the most recent block, as `f32` bits.
    peak: AtomicU32,
}

impl Tap {
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }
}

/// Sends each block to every consumer, applying the gate and recording the level.
pub struct Fanout {
    pub tap: Arc<Tap>,
    consumers: Vec<Sender<Block>>,
    next: u64,
}

impl Fanout {
    pub fn new(tap: Arc<Tap>, consumers: Vec<Sender<Block>>) -> Self {
        Self { tap, consumers, next: 0 }
    }

    pub fn push(&mut self, samples: &[f32]) {
        let gated = self.tap.gate.load(Ordering::Relaxed);
        let peak = if gated { 0.0 } else { samples.iter().fold(0f32, |m, s| m.max(s.abs())) };
        self.tap.peak.store(peak.to_bits(), Ordering::Relaxed);
        let samples: Arc<[f32]> = if gated { vec![0.0; samples.len()].into() } else { samples.into() };
        let block = Block { start: self.next, samples };
        self.next = block.end();
        self.consumers.retain(|c| c.send(block.clone()).is_ok());
    }
}

/// Decodes any audio or video file to 16 kHz mono f32 through ffmpeg.
pub fn decode_file(path: &Path) -> Result<Vec<f32>> {
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning ffmpeg")?;
    let mut bytes = Vec::new();
    child.stdout.take().expect("piped").read_to_end(&mut bytes)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("ffmpeg could not decode {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    }
    Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes"))).collect())
}

/// Feeds decoded samples in 20 ms blocks, in real time when `realtime` is set.
pub fn play_samples(samples: &[f32], mut fanout: Fanout, realtime: bool) {
    const BLOCK: usize = SAMPLE_RATE as usize / 50;
    let t0 = Instant::now();
    for (i, chunk) in samples.chunks(BLOCK).enumerate() {
        if realtime {
            let due = t0 + Duration::from_secs_f64((i * BLOCK) as f64 / SAMPLE_RATE as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        fanout.push(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fanout_numbers_blocks_contiguously_and_gates_to_silence() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let tap = Arc::new(Tap::default());
        let mut f = Fanout::new(tap.clone(), vec![tx]);
        f.push(&[0.5, -0.75]);
        tap.gate.store(true, Ordering::Relaxed);
        f.push(&[0.9; 3]);
        let a = rx.recv().unwrap();
        let b = rx.recv().unwrap();
        assert_eq!((a.start, b.start, b.end()), (0, 2, 5));
        assert_eq!(&*b.samples, &[0.0; 3]);
        assert_eq!(tap.peak(), 0.0);
    }

    #[test]
    fn fanout_drops_disconnected_consumers() {
        let (tx1, rx1) = crossbeam_channel::unbounded();
        let (tx2, rx2) = crossbeam_channel::unbounded::<Block>();
        let mut f = Fanout::new(Arc::default(), vec![tx1, tx2]);
        drop(rx2);
        f.push(&[0.1]);
        f.push(&[0.2]);
        assert_eq!(rx1.len(), 2);
        assert_eq!(f.consumers.len(), 1);
        assert!((f.tap.peak() - 0.2).abs() < 1e-6);
    }
}
