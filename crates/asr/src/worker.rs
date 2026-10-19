//! Speech recognition in a child process that is replaced from time to time.
//!
//! The CUDA driver keeps a little memory for every kernel launch the speech model makes, about
//! 2 MB a minute, and gives it back only when the process exits. Running the recognizer in a worker
//! and swapping in a fresh one every `recycle_after_secs` of audio keeps the app's memory flat. The
//! swap happens where speech resumes after the gate was closed (a [`Feed::Reset`] or a gap in the
//! audio), so the new worker starts on a fresh utterance with its pre-roll. It is loaded before the
//! old one is let go, so no audio is lost.
//!
//! Wire format on the worker's stdin, little-endian: `0, start: u64, n: u32, n × f32` for audio and
//! `1` for a reset. The worker answers with JSON lines on stdout: `{"ready":true}` once the model is
//! loaded, then one [`Words`] per line. It exits when its stdin closes, so it cannot outlive the app.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use audio::{Block, SAMPLE_RATE};
use crossbeam_channel::{Receiver, Sender};
use cues::Feed;

use crate::{Device, Recognizer, Words};

const AUDIO: u8 = 0;
const RESET: u8 = 1;

pub fn write_feed(w: &mut impl Write, feed: &Feed) -> std::io::Result<()> {
    match feed {
        Feed::Reset => w.write_all(&[RESET]),
        Feed::Audio(b) => {
            w.write_all(&[AUDIO])?;
            w.write_all(&b.start.to_le_bytes())?;
            w.write_all(&(b.samples.len() as u32).to_le_bytes())?;
            for s in b.samples.iter() {
                w.write_all(&s.to_le_bytes())?;
            }
            Ok(())
        }
    }
}

/// Reads one feed; `None` at a clean end of stream.
pub fn read_feed(r: &mut impl Read) -> Result<Option<Feed>> {
    let mut tag = [0u8; 1];
    match r.read_exact(&mut tag) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    match tag[0] {
        RESET => Ok(Some(Feed::Reset)),
        AUDIO => {
            let mut head = [0u8; 12];
            r.read_exact(&mut head)?;
            let start = u64::from_le_bytes(head[..8].try_into().expect("8 bytes"));
            let n = u32::from_le_bytes(head[8..].try_into().expect("4 bytes")) as usize;
            let mut bytes = vec![0u8; n * 4];
            r.read_exact(&mut bytes)?;
            let samples = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes"))).collect();
            Ok(Some(Feed::Audio(Block { start, samples })))
        }
        t => bail!("unknown feed tag {t}"),
    }
}

/// The worker's main loop: load the model, then recognise until stdin closes.
pub fn serve(model_dir: &Path, device: Device) -> Result<()> {
    let env = crate::environment()?;
    let mut rec = Recognizer::load(&env, model_dir, device)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{{\"ready\":true}}")?;
    out.flush()?;
    let mut input = BufReader::with_capacity(1 << 16, std::io::stdin().lock());
    let emit = |w: Words, out: &mut std::io::StdoutLock| -> Result<()> {
        serde_json::to_writer(&mut *out, &w)?;
        out.write_all(b"\n")?;
        out.flush()?;
        Ok(())
    };
    while let Some(feed) = read_feed(&mut input)? {
        if let Some(w) = rec.apply(feed)? {
            emit(w, &mut out)?;
        }
    }
    for w in rec.flush()? {
        emit(w, &mut out)?;
    }
    Ok(())
}

pub struct WorkerConfig {
    /// The program and arguments that run [`serve`].
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Audio after which a fresh worker takes over at the next reset.
    pub recycle_after_secs: f64,
}

struct Worker {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    ready: Arc<AtomicBool>,
}

impl Worker {
    fn start(cfg: &WorkerConfig, out: Sender<Words>) -> Result<Self> {
        let mut child = Command::new(&cfg.program)
            .args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("starting {}", cfg.program.display()))?;
        let stdin = BufWriter::with_capacity(1 << 16, child.stdin.take().expect("piped"));
        let stdout = child.stdout.take().expect("piped");
        let ready = Arc::new(AtomicBool::new(false));
        let flag = ready.clone();
        std::thread::Builder::new().name("asr-worker-out".into()).spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.contains("\"ready\"") {
                    flag.store(true, Ordering::Release);
                } else if let Ok(w) = serde_json::from_str::<Words>(&line)
                    && out.send(w).is_err()
                {
                    return;
                }
            }
        })?;
        Ok(Self { child, stdin, ready })
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !self.is_ready() {
            if let Some(status) = self.child.try_wait()? {
                bail!("speech worker exited with {status} while loading");
            }
            if Instant::now() > deadline {
                bail!("speech worker did not load within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn send(&mut self, feed: &Feed) -> std::io::Result<()> {
        write_feed(&mut self.stdin, feed)?;
        self.stdin.flush()
    }

    /// Closes stdin: the worker flushes its last words, which still reach the app, then exits.
    fn retire(self) {
        let Worker { mut child, stdin, .. } = self;
        drop(stdin);
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Process ids of the current speech workers, for the resource graphs.
pub type WorkerPids = Arc<Mutex<Vec<u32>>>;

/// Runs recognition in worker processes, replacing the worker after `recycle_after_secs` of audio.
/// Returns once the first worker has loaded.
pub fn spawn(cfg: WorkerConfig, feeds: Receiver<Feed>, out: Sender<Words>) -> Result<(JoinHandle<()>, WorkerPids)> {
    let mut current = Worker::start(&cfg, out.clone())?;
    current.wait_ready(Duration::from_secs(120))?;
    let pids: WorkerPids = Arc::default();
    *pids.lock().expect("not poisoned") = vec![current.child.id()];
    let shared = pids.clone();
    let handle = std::thread::Builder::new().name("asr-client".into()).spawn(move || {
        let mut fed_secs = 0.0;
        let mut next_start = None;
        let mut standby: Option<Worker> = None;
        for feed in feeds {
            let resumes = match &feed {
                Feed::Reset => true,
                Feed::Audio(b) => next_start.is_some_and(|n| b.start > n),
            };
            if fed_secs >= cfg.recycle_after_secs && standby.is_none() {
                match Worker::start(&cfg, out.clone()) {
                    Ok(w) => {
                        *shared.lock().expect("not poisoned") = vec![current.child.id(), w.child.id()];
                        standby = Some(w);
                    }
                    Err(e) => tracing::error!("speech worker: {e:#}"),
                }
            }
            if resumes && standby.as_ref().is_some_and(Worker::is_ready) {
                let fresh = standby.take().expect("checked");
                *shared.lock().expect("not poisoned") = vec![fresh.child.id()];
                std::mem::replace(&mut current, fresh).retire();
                fed_secs = 0.0;
                tracing::info!("speech worker recycled");
            }
            if let Feed::Audio(b) = &feed {
                fed_secs += b.samples.len() as f64 / SAMPLE_RATE as f64;
                next_start = Some(b.end());
            }
            if let Err(e) = current.send(&feed) {
                tracing::error!("speech worker stopped ({e}); starting a new one");
                match Worker::start(&cfg, out.clone()).and_then(|mut w| w.wait_ready(Duration::from_secs(120)).map(|_| w)) {
                    Ok(w) => {
                        *shared.lock().expect("not poisoned") = vec![w.child.id()];
                        std::mem::replace(&mut current, w).retire();
                        fed_secs = 0.0;
                    }
                    Err(e) => {
                        tracing::error!("speech worker: {e:#}");
                        return;
                    }
                }
            }
        }
        current.retire();
        if let Some(w) = standby {
            w.retire();
        }
    })?;
    Ok((handle, pids))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_roundtrip_through_the_wire_format() {
        let feeds = vec![
            Feed::Reset,
            Feed::Audio(Block { start: 1234, samples: vec![0.25, -1.0, 0.5].into() }),
            Feed::Audio(Block { start: 1237, samples: Vec::new().into() }),
        ];
        let mut bytes = Vec::new();
        for f in &feeds {
            write_feed(&mut bytes, f).unwrap();
        }
        let mut r = bytes.as_slice();
        let mut back = Vec::new();
        while let Some(f) = read_feed(&mut r).unwrap() {
            back.push(f);
        }
        let show = |f: &Feed| match f {
            Feed::Reset => "reset".to_string(),
            Feed::Audio(b) => format!("{}:{:?}", b.start, b.samples),
        };
        assert_eq!(back.iter().map(show).collect::<Vec<_>>(), feeds.iter().map(show).collect::<Vec<_>>());
    }

    #[test]
    fn words_survive_the_json_line() {
        let w = Words { text: " hello".into(), audio_end: 1.5, compute_ms: 20.0 };
        let line = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<Words>(&line).unwrap(), w);
    }
}
