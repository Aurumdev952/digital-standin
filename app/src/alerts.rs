use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use audio::Tap;

/// Desktop notification plus a short chime. The listener tap is gated while the chime plays so our
/// own sound is not transcribed or mistaken for the room.
pub struct Alerter {
    tap: Arc<Tap>,
    chime: Option<PathBuf>,
}

impl Alerter {
    pub fn new(tap: Arc<Tap>) -> Self {
        Self { tap, chime: write_chime().ok() }
    }

    pub fn alert(&self, reason: &str, text: &str) {
        let body = text.chars().rev().take(160).collect::<Vec<_>>().into_iter().rev().collect::<String>();
        let shown = notify_rust::Notification::new()
            .summary(&format!("Digital Standin: {reason}"))
            .body(&body)
            .urgency(notify_rust::Urgency::Critical)
            .timeout(notify_rust::Timeout::Milliseconds(8000))
            .show();
        if let Err(e) = shown {
            tracing::warn!("desktop notification failed: {e}");
        }
        let Some(chime) = self.chime.clone() else { return };
        let tap = self.tap.clone();
        std::thread::spawn(move || {
            tap.gate.store(true, Ordering::Relaxed);
            let _ = Command::new("pw-play").arg(&chime).status();
            std::thread::sleep(Duration::from_millis(250));
            tap.gate.store(false, Ordering::Relaxed);
        });
    }
}

/// Two soft sine notes, written once to the runtime dir.
fn write_chime() -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir().join("digital-standin-chime.wav");
    if path.exists() {
        return Ok(path);
    }
    let rate = 44_100u32;
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec)?;
    for (freq, secs) in [(880.0f32, 0.18f32), (1318.5, 0.32)] {
        let n = (rate as f32 * secs) as usize;
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let env = (1.0 - i as f32 / n as f32).powi(2) * (t * 200.0).min(1.0);
            let s = (2.0 * std::f32::consts::PI * freq * t).sin() * env * 0.35;
            w.write_sample((s * i16::MAX as f32) as i16)?;
        }
    }
    w.finalize()?;
    Ok(path)
}
