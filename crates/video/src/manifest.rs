use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Loop,
    Oneshot,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Output {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StateSpec {
    pub kind: ClipKind,
    #[serde(default)]
    pub base: bool,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClipSpec {
    pub state: String,
    pub file: PathBuf,
    #[serde(default = "one")]
    pub weight: f32,
    #[serde(default = "one")]
    pub intensity: f32,
}

fn one() -> f32 {
    1.0
}

/// A validated manifest: every clip names a known state, every state has a clip,
/// and exactly one loop state is the base.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub output: Output,
    pub states: BTreeMap<String, StateSpec>,
    pub clips: Vec<ClipSpec>,
    pub base: String,
}

#[derive(Deserialize)]
struct Raw {
    output: Output,
    states: BTreeMap<String, StateSpec>,
    clips: Vec<ClipSpec>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut m = Self::parse(&text)?;
        for c in &mut m.clips {
            c.file = dir.join(&c.file);
        }
        Ok(m)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let raw: Raw = toml::from_str(text).context("parsing manifest")?;
        let bases: Vec<_> = raw.states.iter().filter(|(_, s)| s.base).map(|(k, _)| k.clone()).collect();
        let [base] = bases.as_slice() else {
            bail!("exactly one state must have base = true, found {bases:?}");
        };
        if raw.states[base].kind != ClipKind::Loop {
            bail!("base state {base:?} must be kind = \"loop\"");
        }
        for c in &raw.clips {
            if !raw.states.contains_key(&c.state) {
                bail!("clip {} names unknown state {:?}", c.file.display(), c.state);
            }
        }
        for name in raw.states.keys() {
            if !raw.clips.iter().any(|c| &c.state == name) {
                bail!("state {name:?} has no clips");
            }
        }
        let o = raw.output;
        if !o.width.is_multiple_of(2) || o.width == 0 || o.height == 0 || o.fps == 0 {
            bail!("output must have a non-zero even width, height and fps");
        }
        Ok(Self { output: raw.output, base: base.clone(), states: raw.states, clips: raw.clips })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
        [output]
        width = 4
        height = 2
        fps = 30
        [states.idle]
        kind = "loop"
        base = true
        description = "x"
        [states.nod]
        kind = "oneshot"
        description = "y"
        [[clips]]
        state = "idle"
        file = "a.mp4"
        [[clips]]
        state = "nod"
        file = "b.mp4"
        intensity = 0.5
    "#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!(m.base, "idle");
        assert_eq!(m.clips[1].intensity, 0.5);
        assert_eq!(m.clips[0].weight, 1.0);
    }

    #[test]
    fn rejects_state_without_clips() {
        let bad = OK.replace("state = \"nod\"", "state = \"idle\"");
        let err = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(err.contains("\"nod\" has no clips"), "{err}");
    }

    #[test]
    fn rejects_unknown_state() {
        let bad = OK.replace("state = \"nod\"", "state = \"wave\"");
        assert!(Manifest::parse(&bad).unwrap_err().to_string().contains("unknown state \"wave\""));
    }

    #[test]
    fn requires_single_loop_base() {
        let two = OK.replace("kind = \"oneshot\"", "kind = \"loop\"\nbase = true");
        assert!(Manifest::parse(&two).unwrap_err().to_string().contains("exactly one"));
        let oneshot_base = OK.replace("kind = \"loop\"", "kind = \"oneshot\"");
        assert!(Manifest::parse(&oneshot_base).unwrap_err().to_string().contains("must be kind"));
    }
}
