use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::manifest::{ClipKind, Manifest, Output};

/// One packed YUYV 4:2:2 frame.
pub type Frame = Arc<[u8]>;

pub struct Clip {
    pub state: usize,
    pub frames: Vec<Frame>,
    pub weight: f32,
    pub intensity: f32,
}

pub struct State {
    pub name: String,
    pub kind: ClipKind,
    pub description: String,
    pub clips: Vec<usize>,
}

/// Every clip decoded into RAM at the output resolution, so playback never decodes.
pub struct Library {
    pub output: Output,
    pub states: Vec<State>,
    pub clips: Vec<Clip>,
    pub base: usize,
}

impl Library {
    pub fn frame_bytes(&self) -> usize {
        (self.output.width * self.output.height * 2) as usize
    }

    pub fn state_index(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|s| s.name == name)
    }

    pub fn load(manifest: &Manifest) -> Result<Self> {
        let names: Vec<&String> = manifest.states.keys().collect();
        let index = |n: &str| names.iter().position(|s| *s == n).expect("validated");
        let clips = manifest
            .clips
            .par_iter()
            .map(|c| {
                let frames = decode_yuyv(&c.file, manifest.output)?;
                if frames.is_empty() {
                    bail!("{} decoded to zero frames", c.file.display());
                }
                Ok(Clip { state: index(&c.state), frames, weight: c.weight, intensity: c.intensity })
            })
            .collect::<Result<Vec<_>>>()?;
        let states = manifest
            .states
            .iter()
            .enumerate()
            .map(|(i, (name, spec))| State {
                name: name.clone(),
                kind: spec.kind,
                description: spec.description.clone(),
                clips: clips.iter().enumerate().filter(|(_, c)| c.state == i).map(|(ci, _)| ci).collect(),
            })
            .collect();
        Ok(Self { output: manifest.output, states, clips, base: index(&manifest.base) })
    }

    /// Builds a library directly from frames; used by tests and synthetic sources.
    pub fn from_parts(output: Output, states: Vec<(String, ClipKind, Vec<Vec<Frame>>)>, base: usize) -> Self {
        let mut clips = Vec::new();
        let states = states
            .into_iter()
            .enumerate()
            .map(|(si, (name, kind, cs))| {
                let ids = cs
                    .into_iter()
                    .map(|frames| {
                        clips.push(Clip { state: si, frames, weight: 1.0, intensity: 1.0 });
                        clips.len() - 1
                    })
                    .collect();
                State { name, kind, description: String::new(), clips: ids }
            })
            .collect();
        Self { output, states, clips, base }
    }
}

fn decode_yuyv(path: &std::path::Path, o: Output) -> Result<Vec<Frame>> {
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-vf",
            &format!(
                "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},fps={f}",
                w = o.width,
                h = o.height,
                f = o.fps
            ),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuyv422",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning ffmpeg (is it installed?)")?;
    let size = (o.width * o.height * 2) as usize;
    let mut out = child.stdout.take().expect("piped");
    let mut frames = Vec::new();
    loop {
        let mut buf = vec![0u8; size];
        match out.read_exact(&mut buf) {
            Ok(()) => frames.push(Frame::from(buf)),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
    }
    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!("ffmpeg failed on {}: {}", path.display(), String::from_utf8_lossy(&result.stderr));
    }
    Ok(frames)
}
