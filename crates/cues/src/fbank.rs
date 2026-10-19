//! Kaldi-compatible power mel filterbank as CED expects it: 32 ms Hann frames every 10 ms,
//! frames centred on their shift (`snip_edges = false`), 64 bins from 0 to 8 kHz, no log,
//! followed by `AmplitudeToDB` (10·log10, floor 1e-10, clamped to 120 dB below the maximum).

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

const FRAME: usize = 512;
const SHIFT: usize = 160;
const BINS: usize = 64;
const RATE: f32 = 16_000.0;

pub struct Fbank {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// Per mel bin: first FFT bin and its weights.
    banks: Vec<(usize, Vec<f32>)>,
}

fn mel(f: f32) -> f32 {
    1127.0 * (1.0 + f / 700.0).ln()
}

impl Default for Fbank {
    fn default() -> Self {
        Self::new()
    }
}

impl Fbank {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
        let window = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME - 1) as f32).cos())
            .collect();
        let (lo, hi) = (mel(0.0), mel(8000.0));
        let delta = (hi - lo) / (BINS + 1) as f32;
        let fft_bins = FRAME / 2;
        let width = RATE / FRAME as f32;
        let banks = (0..BINS)
            .map(|b| {
                let (left, center, right) = (lo + b as f32 * delta, lo + (b + 1) as f32 * delta, lo + (b + 2) as f32 * delta);
                let w: Vec<(usize, f32)> = (0..fft_bins)
                    .filter_map(|k| {
                        let m = mel(width * k as f32);
                        let v = if m > left && m <= center {
                            (m - left) / (center - left)
                        } else if m > center && m < right {
                            (right - m) / (right - center)
                        } else {
                            0.0
                        };
                        (v > 0.0).then_some((k, v))
                    })
                    .collect();
                let first = w.first().map(|x| x.0).unwrap_or(0);
                let mut dense = vec![0.0; w.last().map(|x| x.0 + 1 - first).unwrap_or(0)];
                for (k, v) in w {
                    dense[k - first] = v;
                }
                (first, dense)
            })
            .collect();
        Self { fft, window, banks }
    }

    /// Features for `samples`, laid out bin-major: `out[bin * frames + frame]`.
    pub fn compute(&self, samples: &[f32]) -> (usize, Vec<f32>) {
        let n = samples.len();
        let frames = (n + SHIFT / 2) / SHIFT;
        let mut out = vec![0.0f32; BINS * frames];
        let mut buf = self.fft.make_input_vec();
        let mut spec = self.fft.make_output_vec();
        let mut power = vec![0.0f32; FRAME / 2 + 1];
        for f in 0..frames {
            let start = (f * SHIFT + SHIFT / 2) as isize - (FRAME / 2) as isize;
            for (i, x) in buf.iter_mut().enumerate() {
                let mut s = start + i as isize;
                // Kaldi reflects out-of-range samples back into the signal.
                if s < 0 {
                    s = -s - 1;
                } else if s >= n as isize {
                    s = 2 * n as isize - 1 - s;
                }
                *x = samples.get(s.clamp(0, n as isize - 1) as usize).copied().unwrap_or(0.0) * self.window[i];
            }
            self.fft.process(&mut buf, &mut spec).expect("fixed-size fft");
            for (p, c) in power.iter_mut().zip(&spec) {
                *p = c.norm_sqr();
            }
            for (b, (first, w)) in self.banks.iter().enumerate() {
                out[b * frames + f] = w.iter().zip(&power[*first..]).map(|(w, p)| w * p).sum();
            }
        }
        let mut max = f32::MIN;
        for x in &mut out {
            *x = 10.0 * x.max(1e-10).log10();
            max = max.max(*x);
        }
        for x in &mut out {
            *x = x.max(max - 120.0);
        }
        (frames, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_gives_one_hundred_frames() {
        assert_eq!(Fbank::new().compute(&vec![0.1; 16_000]).0, 100);
    }

    #[test]
    fn a_tone_lands_in_the_mel_bin_covering_its_frequency() {
        let fb = Fbank::new();
        let tone: Vec<f32> = (0..16_000).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16_000.0).sin()).collect();
        let (frames, feats) = fb.compute(&tone);
        let mid = frames / 2;
        let loudest = (0..BINS).max_by(|&a, &b| feats[a * frames + mid].total_cmp(&feats[b * frames + mid])).unwrap();
        let (lo, hi) = (mel(0.0), mel(8000.0));
        let delta = (hi - lo) / (BINS + 1) as f32;
        let center = lo + (loudest + 1) as f32 * delta;
        assert!((center - mel(1000.0)).abs() <= delta, "bin {loudest} is centred at mel {center}, 1 kHz is mel {}", mel(1000.0));
    }
}
