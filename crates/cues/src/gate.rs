//! Lets audio through to speech recognition only while someone speaks.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use audio::{Block, SAMPLE_RATE};

/// Audio is kept from this long before the voice detector fires, so the first word is whole.
const PREROLL_SECS: f64 = 0.6;
/// Audio keeps flowing this long after speech stops: two 560 ms recognizer chunks, enough for the
/// model to commit the last words.
const HANGOVER_SECS: f64 = 1.2;
/// A silence this long makes the next utterance start the recognizer from a clean state.
const RESET_AFTER_SECS: f64 = 3.0;

#[derive(Debug, Clone)]
pub enum Feed {
    Audio(Block),
    /// Clear recognizer state: the audio that follows is not contiguous with what came before.
    Reset,
}

/// Seconds of audio passed to and held back from recognition.
#[derive(Debug, Default)]
pub struct GateStats {
    passed: AtomicU64,
    held: AtomicU64,
}

impl GateStats {
    pub fn passed_secs(&self) -> f64 {
        self.passed.load(Ordering::Relaxed) as f64 / SAMPLE_RATE as f64
    }

    pub fn held_secs(&self) -> f64 {
        self.held.load(Ordering::Relaxed) as f64 / SAMPLE_RATE as f64
    }
}

#[derive(Default)]
pub struct Gate {
    preroll: VecDeque<Block>,
    preroll_len: usize,
    /// Sample index until which audio keeps flowing after speech stopped.
    open_until: Option<u64>,
    closed_at: u64,
    pub stats: Arc<GateStats>,
}

impl Gate {
    /// `speaking` is the voice detector's state after `block`.
    pub fn push(&mut self, block: Block, speaking: bool) -> Vec<Feed> {
        let hang = (HANGOVER_SECS * SAMPLE_RATE as f64) as u64;
        let n = block.samples.len() as u64;
        if speaking {
            let mut out = Vec::new();
            if self.open_until.is_none() {
                let silent = block.start.saturating_sub(self.closed_at) as f64 / SAMPLE_RATE as f64;
                if self.closed_at == 0 || silent >= RESET_AFTER_SECS {
                    out.push(Feed::Reset);
                }
                self.preroll_len = 0;
                for b in self.preroll.drain(..) {
                    self.stats.held.fetch_sub(b.samples.len() as u64, Ordering::Relaxed);
                    self.stats.passed.fetch_add(b.samples.len() as u64, Ordering::Relaxed);
                    out.push(Feed::Audio(b));
                }
            }
            self.open_until = Some(block.end() + hang);
            self.stats.passed.fetch_add(n, Ordering::Relaxed);
            out.push(Feed::Audio(block));
            return out;
        }
        match self.open_until {
            Some(until) if block.start < until => {
                self.stats.passed.fetch_add(n, Ordering::Relaxed);
                vec![Feed::Audio(block)]
            }
            _ => {
                if self.open_until.take().is_some() {
                    self.closed_at = block.start;
                }
                self.stats.held.fetch_add(n, Ordering::Relaxed);
                self.preroll_len += block.samples.len();
                self.preroll.push_back(block);
                let keep = (PREROLL_SECS * SAMPLE_RATE as f64) as usize;
                while self.preroll_len - self.preroll.front().map_or(0, |b| b.samples.len()) >= keep {
                    let b = self.preroll.pop_front().expect("non-empty");
                    self.preroll_len -= b.samples.len();
                }
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 ms blocks numbered from `start` (in blocks).
    fn block(i: u64) -> Block {
        Block { start: i * 1600, samples: vec![i as f32; 1600].into() }
    }

    fn starts(out: &[Feed]) -> Vec<String> {
        out.iter()
            .map(|f| match f {
                Feed::Audio(b) => (b.start / 1600).to_string(),
                Feed::Reset => "reset".into(),
            })
            .collect()
    }

    #[test]
    fn silence_is_held_back_until_speech_then_released_with_preroll() {
        let mut g = Gate::default();
        for i in 0..20 {
            assert!(g.push(block(i), false).is_empty(), "block {i} is silence");
        }
        let out = g.push(block(20), true);
        assert_eq!(starts(&out), ["reset", "14", "15", "16", "17", "18", "19", "20"], "0.6 s of pre-roll");
        assert!((g.stats.passed_secs() - 0.7).abs() < 1e-9);
        assert!((g.stats.held_secs() - 1.4).abs() < 1e-9);
    }

    #[test]
    fn hangover_keeps_audio_flowing_then_closes() {
        let mut g = Gate::default();
        g.push(block(0), true);
        let tail: Vec<usize> = (1..20).map(|i| g.push(block(i), false).len()).collect();
        assert_eq!(tail.iter().filter(|&&n| n == 1).count(), 12, "1.2 s of hangover after the last speech block");
        assert!(tail[12..].iter().all(|&n| n == 0));
    }

    #[test]
    fn short_pause_continues_without_reset_but_long_silence_resets() {
        let mut g = Gate::default();
        g.push(block(0), true);
        for i in 1..20 {
            g.push(block(i), false);
        }
        let quick = g.push(block(20), true);
        assert!(!starts(&quick).contains(&"reset".to_string()), "0.7 s since the gate closed");
        for i in 21..70 {
            g.push(block(i), false);
        }
        let later = g.push(block(70), true);
        assert_eq!(starts(&later)[0], "reset", "3.7 s of silence");
    }
}
