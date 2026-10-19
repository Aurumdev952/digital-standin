use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Every tunable of the decision layer. Serialized as the `[brain]` table of the config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BrainConfig {
    /// Names the listener answers to, matched in the transcript for alerts.
    pub aliases: Vec<String>,
    /// States the stand-in never picks on its own. Manual triggers still work.
    pub disabled: Vec<String>,

    /// Laughter score at which the stand-in smiles; at `big_laugh` it laughs.
    pub laugh_threshold: f32,
    pub big_laugh: f32,
    pub applause_threshold: f32,

    /// Silence after speech that closes an utterance and triggers a decision.
    pub end_of_utterance_secs: f64,
    /// While someone keeps talking, ask the decider at most this often.
    pub decide_every_secs: f64,
    /// Seconds of transcript the decider sees.
    pub context_secs: f64,

    /// Back-channel nods during continuous speech, at a random interval in this range.
    /// High engagement shortens it, low engagement stretches it.
    pub nod_min_secs: f64,
    pub nod_max_secs: f64,

    /// Minimum probability the decider must give a state before it is shown.
    pub reaction_threshold: f32,
    /// Per-state overrides of `reaction_threshold`.
    pub state_thresholds: BTreeMap<String, f32>,
    /// P(addressed to me) that raises an alert.
    pub alert_threshold: f32,

    /// Human-like delay between a cue and the visible reaction.
    pub delay_min_secs: f64,
    pub delay_max_secs: f64,
    /// Minimum gap between any two reactions, and between two of the same state.
    pub min_gap_secs: f64,
    pub same_state_gap_secs: f64,
    /// Minimum gap between two alerts.
    pub alert_gap_secs: f64,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            aliases: Vec::new(),
            disabled: Vec::new(),
            laugh_threshold: 0.15,
            big_laugh: 0.45,
            applause_threshold: 0.5,
            end_of_utterance_secs: 0.7,
            decide_every_secs: 2.0,
            context_secs: 60.0,
            nod_min_secs: 7.0,
            nod_max_secs: 16.0,
            reaction_threshold: 0.45,
            state_thresholds: BTreeMap::new(),
            alert_threshold: 0.8,
            delay_min_secs: 0.25,
            delay_max_secs: 0.7,
            min_gap_secs: 2.0,
            same_state_gap_secs: 6.0,
            alert_gap_secs: 15.0,
        }
    }
}

impl BrainConfig {
    pub fn threshold_for(&self, state: &str) -> f32 {
        self.state_thresholds.get(state).copied().unwrap_or(self.reaction_threshold)
    }
}

/// The whole settings file, `~/.config/digital-standin/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub brain: BrainConfig,
    pub video: VideoSettings,
    pub asr: AsrSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AsrSettings {
    /// Speech model directory. Unset means the 160 ms export when present, else the published
    /// 560 ms one.
    pub model: Option<PathBuf>,
}

const ASR_160: &str = "models/nemotron-160ms";
const ASR_560: &str = "models/parakeet/nemotron-speech-streaming-en-0.6b";

impl AsrSettings {
    pub fn resolve(&self, root: &Path) -> PathBuf {
        match &self.model {
            Some(p) => root.join(p),
            None if root.join(ASR_160).join("encoder.onnx").exists() => root.join(ASR_160),
            None => root.join(ASR_560),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VideoSettings {
    pub blend_frames: usize,
    pub device: PathBuf,
    pub manifest: PathBuf,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { blend_frames: 8, device: "/dev/video10".into(), manifest: "clips/manifest.toml".into() }
    }
}

impl Settings {
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("digital-standin").join("config.toml")
    }

    /// Missing file means defaults; a malformed file is an error rather than a silent reset.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_fills_defaults_and_roundtrips() {
        let s: Settings = toml::from_str("[brain]\naliases = [\"Sam\"]\nlaugh_threshold = 0.4\n").unwrap();
        assert_eq!(s.brain.aliases, vec!["Sam"]);
        assert_eq!(s.brain.laugh_threshold, 0.4);
        assert_eq!(s.brain.nod_min_secs, BrainConfig::default().nod_min_secs);
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn state_threshold_overrides_default() {
        let mut c = BrainConfig::default();
        c.state_thresholds.insert("confused".into(), 0.8);
        assert_eq!(c.threshold_for("confused"), 0.8);
        assert_eq!(c.threshold_for("nod"), c.reaction_threshold);
    }
}
