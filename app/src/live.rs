use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use audio::Tap;
use brain::decider::Client;
use brain::{Brain, BrainConfig, Decision, Output, Settings, StateInfo};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use video::{Command, Engine, Library, Manifest, Player, Request, Urgency};

use crate::alerts::Alerter;
use crate::sidecar::{Kind, Sidecar};
use crate::speakers::{self, Frame, Line, MAX_SPEAKERS};
use crate::telemetry::{CuePoint, Diarizer, EventKind, Telemetry};

pub enum BrainIn {
    Words(asr::Words),
    Cues(cues::Cues),
    Decision(Result<Decision>),
    Manual(String),
    Config(BrainConfig),
    BlendFrames(usize),
    /// While paused only manual reactions reach the camera.
    Pause(bool),
}

pub enum AudioSource {
    Monitor,
    /// Play a file through the pipeline in real time, for testing without a meeting.
    File(PathBuf),
}

pub struct Options {
    pub root: PathBuf,
    pub settings: Settings,
    pub audio: AudioSource,
    pub decider: bool,
    pub diarizer: bool,
    pub sink: Option<Box<dyn video::FrameSink>>,
}

/// The running stand-in: every thread, and the handles the GUI needs.
pub struct Live {
    pub telemetry: Arc<Mutex<Telemetry>>,
    pub engine: Arc<Engine>,
    pub lib: Arc<Library>,
    pub tx: Sender<BrainIn>,
    sidecars: Vec<Sidecar>,
    _ort: asr::Environment,
}

impl Live {
    pub fn start(opts: Options) -> Result<Self> {
        let t0 = Instant::now();
        let s = &opts.settings;
        let manifest = Manifest::load(&opts.root.join(&s.video.manifest))?;
        let lib = Arc::new(Library::load(&manifest)?);
        tracing::info!("clips decoded in {:.2?}", t0.elapsed());
        let sink = match opts.sink {
            Some(sink) => sink,
            None => Box::new(video::V4l2Sink::open(&s.video.device, lib.output)?),
        };
        let engine = Arc::new(Engine::spawn(Player::new(lib.clone(), s.video.blend_frames, seed()), sink));

        let mut sidecars = Vec::new();
        for (on, kind) in [(opts.decider, Kind::Decider), (opts.diarizer, Kind::Diarizer)] {
            if on {
                sidecars.push(Sidecar::ensure(&opts.root, kind)?);
            }
        }
        let socket = |kind| sidecars.iter().find(|s: &&Sidecar| s.kind == kind).map(|s| s.socket.clone());
        let telemetry = Arc::new(Mutex::new(Telemetry {
            diarizer: if opts.diarizer { Diarizer::Starting } else { Diarizer::Off },
            ..Default::default()
        }));

        let models = opts.root.join("models");
        let env = asr::environment()?;
        let det = cues::CueDetector::load(&env, &models)?;
        tracing::info!("models loaded in {:.2?}", t0.elapsed());

        // Audio goes to the cue detector, whose voice detector gates what reaches speech recognition.
        let (asr_tx, asr_rx) = crossbeam_channel::bounded(512);
        let (cue_tx, cue_rx) = crossbeam_channel::bounded(256);
        let (words_tx, words_rx) = crossbeam_channel::unbounded();
        let (cues_tx, cues_rx) = crossbeam_channel::unbounded();
        let gate = cues::Gate::default();
        let gate_stats = gate.stats.clone();
        let worker = asr::worker::WorkerConfig {
            program: std::env::current_exe()?,
            args: vec!["asr-worker".into(), "--model".into(), s.asr.resolve(&opts.root).into_os_string()],
            recycle_after_secs: s.asr.recycle_minutes * 60.0,
        };
        let (_, worker_pids) = asr::worker::spawn(worker, asr_rx, words_tx)?;
        tracing::info!("speech worker ready after {:.2?}", t0.elapsed());
        cues::spawn(det, gate, cue_rx, cues_tx, asr_tx);

        let tap = Arc::new(Tap::default());
        let mut consumers = vec![cue_tx];
        if let Some(path) = socket(Kind::Diarizer) {
            let (tx, rx) = crossbeam_channel::bounded(256);
            consumers.push(tx);
            spawn_diarizer_client(path, rx, telemetry.clone());
        }
        let fanout = audio::Fanout::new(tap.clone(), consumers);
        match opts.audio {
            AudioSource::Monitor => audio::capture_default_sink(fanout)?,
            AudioSource::File(path) => {
                let samples = audio::decode_file(&path)?;
                std::thread::spawn(move || audio::play_samples(&samples, fanout, true));
            }
        }

        let (tx, rx) = crossbeam_channel::unbounded();
        forward(words_rx, tx.clone(), BrainIn::Words);
        forward(cues_rx, tx.clone(), BrainIn::Cues);
        let (ask_tx, ask_rx) = crossbeam_channel::bounded::<serde_json::Value>(1);
        spawn_decider_client(socket(Kind::Decider), ask_rx, tx.clone());

        telemetry.lock().gate = Some(gate_stats);
        let states: Vec<StateInfo> =
            lib.states.iter().map(|s| StateInfo { name: s.name.clone(), description: s.description.clone() }).collect();
        let brain = Brain::new(s.brain.clone(), states, lib.states[lib.base].name.clone(), seed());
        let alerter = Alerter::new(tap.clone());
        let ctx = BrainLoop {
            brain,
            rx,
            ask_tx,
            engine: engine.clone(),
            lib: lib.clone(),
            tap: tap.clone(),
            telemetry: telemetry.clone(),
            alerter,
            paused: false,
        };
        std::thread::Builder::new().name("brain".into()).spawn(move || ctx.run())?;

        spawn_resource_monitor(telemetry.clone(), worker_pids);
        Ok(Self { telemetry, engine, lib, tx, sidecars, _ort: env })
    }

    /// Stops what must not outlive the app. Call before `exit_now`, which skips destructors.
    pub fn shutdown(&mut self) {
        for s in &mut self.sidecars {
            s.stop();
        }
    }

    /// Reports in the telemetry, once, each sidecar process that died.
    pub fn poll_sidecars(&mut self) {
        let mut t = self.telemetry.lock();
        let now = t.now;
        self.sidecars.retain_mut(|s| {
            let Some(msg) = s.exited() else { return true };
            match s.kind {
                Kind::Decider => t.decider = msg.clone(),
                Kind::Diarizer => t.diarizer = Diarizer::Failed(msg.clone()),
            }
            t.event(now, EventKind::Info, msg);
            false
        });
    }
}

/// CPU seconds used so far by a process (`/proc/<pid>/stat` utime + stime, in 100 Hz ticks).
fn cpu_secs(pid: &str) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?) / 100.0)
}

fn rss_mb(pid: &str) -> Option<f64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    Some(statm.split_whitespace().nth(1)?.parse::<f64>().ok()? * 4096.0 / 1e6)
}

/// Samples the app's CPU and memory, its speech workers' included, and the GPU every 2 s.
fn spawn_resource_monitor(telemetry: Arc<Mutex<Telemetry>>, workers: asr::worker::WorkerPids) {
    std::thread::Builder::new()
        .name("resources".into())
        .spawn(move || {
            let pids = || {
                let mut p = vec!["self".to_string()];
                p.extend(workers.lock().map(|w| w.iter().map(u32::to_string).collect::<Vec<_>>()).unwrap_or_default());
                p
            };
            let total = |f: fn(&str) -> Option<f64>| -> f64 { pids().iter().filter_map(|p| f(p)).sum() };
            let gpu = || {
                let out = std::process::Command::new("nvidia-smi")
                    .args(["--query-gpu=memory.used,utilization.gpu", "--format=csv,noheader,nounits"])
                    .output()
                    .ok()?;
                let line = String::from_utf8_lossy(&out.stdout);
                let mut it = line.lines().next()?.split(',').map(|v| v.trim().parse::<f32>().ok());
                Some((it.next()??, it.next()??))
            };
            let mut last = (Instant::now(), total(cpu_secs));
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let (now_i, cpu) = (Instant::now(), total(cpu_secs));
                // A recycled worker takes its CPU time with it, so the difference can dip below zero.
                let pct = ((cpu - last.1) / now_i.duration_since(last.0).as_secs_f64() * 100.0).max(0.0);
                last = (now_i, cpu);
                let rss = Some(total(rss_mb));
                let g = gpu();
                let mut t = telemetry.lock();
                let now = t.now;
                t.history.cpu_pct.push(now, pct as f32);
                if let Some(r) = rss {
                    t.history.rss_mb.push(now, r as f32);
                }
                if let Some((mem, util)) = g {
                    t.history.gpu_mb.push(now, mem);
                    t.history.gpu_util.push(now, util);
                }
            }
        })
        .expect("spawn resource monitor");
}

fn forward<T: Send + 'static>(rx: Receiver<T>, tx: Sender<BrainIn>, wrap: fn(T) -> BrainIn) {
    std::thread::spawn(move || {
        for v in rx {
            if tx.send(wrap(v)).is_err() {
                return;
            }
        }
    });
}

/// Sends each ask to the sidecar, reconnecting as needed. A failure is reported back so the brain
/// stops waiting.
fn spawn_decider_client(socket: Option<PathBuf>, asks: Receiver<serde_json::Value>, tx: Sender<BrainIn>) {
    std::thread::Builder::new()
        .name("decider-client".into())
        .spawn(move || {
            let mut client: Option<Client> = None;
            for req in asks {
                let result = match &socket {
                    None => Err(anyhow::anyhow!("decider disabled")),
                    Some(path) => {
                        if client.is_none() {
                            client = Client::connect(path, Duration::from_secs(3)).ok();
                        }
                        match client.as_mut() {
                            None => Err(anyhow::anyhow!("decider not ready yet")),
                            Some(c) => c.decide(&req).inspect_err(|_| client = None),
                        }
                    }
                };
                if tx.send(BrainIn::Decision(result)).is_err() {
                    return;
                }
            }
        })
        .expect("spawn decider client");
}

/// Streams the audio to the diarizer sidecar and records the speakers it reports. It never blocks
/// the audio: while the sidecar is unreachable, blocks are dropped and it reconnects every 2 s.
fn spawn_diarizer_client(socket: PathBuf, blocks: Receiver<audio::Block>, telemetry: Arc<Mutex<Telemetry>>) {
    std::thread::Builder::new()
        .name("diarizer-client".into())
        .spawn(move || {
            let mut conn: Option<UnixStream> = None;
            let mut last_try: Option<Instant> = None;
            let mut bytes = Vec::new();
            for (i, block) in blocks.iter().enumerate() {
                if i % 10 == 0 {
                    let mut t = telemetry.lock();
                    t.stream_to_now = t.now - block.end() as f64 / audio::SAMPLE_RATE as f64;
                }
                if conn.is_none() && last_try.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
                    last_try = Some(Instant::now());
                    conn = connect_diarizer(&socket, block.start_secs(), &telemetry);
                }
                let Some(c) = conn.as_mut() else { continue };
                bytes.clear();
                bytes.extend(block.samples.iter().flat_map(|s| s.to_le_bytes()));
                if let Err(e) = c.write_all(&bytes) {
                    telemetry.lock().diarizer = Diarizer::Failed(format!("diarizer: {e}"));
                    let _ = c.shutdown(std::net::Shutdown::Both);
                    conn = None;
                }
            }
        })
        .expect("spawn diarizer client");
}

/// Opens a session whose first sample is at stream time `offset`, and starts reading its frames.
fn connect_diarizer(socket: &Path, offset: f64, telemetry: &Arc<Mutex<Telemetry>>) -> Option<UnixStream> {
    let conn = UnixStream::connect(socket).ok()?;
    conn.set_write_timeout(Some(Duration::from_secs(1))).ok()?;
    let reader = conn.try_clone().ok()?;
    let telemetry = telemetry.clone();
    std::thread::Builder::new()
        .name("diarizer-reader".into())
        .spawn(move || {
            for line in BufReader::new(&reader).lines() {
                let Ok(line) = line else { break };
                let Ok(msg) = serde_json::from_str::<DiarizerFrames>(&line) else { continue };
                let mut t = telemetry.lock();
                let now = t.now;
                t.diarizer = Diarizer::Ready { ms: msg.ms };
                t.history.diarizer_ms.push(now, msg.ms);
                for (i, p) in msg.probs.iter().enumerate() {
                    let start = offset + msg.start + i as f64 * msg.frame;
                    t.speakers.push(Frame { start, end: start + msg.frame, p: *p });
                }
            }
            // Stops the writer too, so it reconnects.
            let _ = reader.shutdown(std::net::Shutdown::Both);
        })
        .ok()?;
    Some(conn)
}

/// One line from the diarizer sidecar.
#[derive(serde::Deserialize)]
struct DiarizerFrames {
    start: f64,
    frame: f64,
    probs: Vec<[f32; MAX_SPEAKERS]>,
    ms: f32,
}

struct BrainLoop {
    brain: Brain,
    rx: Receiver<BrainIn>,
    ask_tx: Sender<serde_json::Value>,
    engine: Arc<Engine>,
    lib: Arc<Library>,
    tap: Arc<Tap>,
    telemetry: Arc<Mutex<Telemetry>>,
    alerter: Alerter,
    paused: bool,
}

impl BrainLoop {
    fn run(mut self) {
        let start = Instant::now();
        let mut last_snapshot = 0.0;
        loop {
            let msg = match self.rx.recv_timeout(Duration::from_millis(40)) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let now = start.elapsed().as_secs_f64();
            if let Some(status) = self.engine.stats.lock().status {
                self.brain.set_showing(&self.lib.states[status.state].name);
            }
            let mut out = Vec::new();
            if let Some(m) = msg {
                out.extend(self.handle(now, m));
            }
            out.extend(self.brain.tick(now));
            for o in out {
                self.apply(now, o);
            }
            if now - last_snapshot > 0.2 {
                last_snapshot = now;
                self.snapshot(now);
            }
        }
    }

    fn handle(&mut self, now: f64, m: BrainIn) -> Vec<Output> {
        match m {
            BrainIn::Words(w) => {
                let mut t = self.telemetry.lock();
                t.asr_ms = w.compute_ms;
                t.history.asr_ms.push(now, w.compute_ms);
                t.spans.push(w.audio_end, &w.text);
                drop(t);
                self.brain.on_words(now, &w.text)
            }
            BrainIn::Cues(c) => {
                self.telemetry.lock().cue(now, CuePoint { speaking: c.speech, laughter: c.laughter, applause: c.applause });
                self.brain.on_cues(now, c.speech, c.laughter, c.applause)
            }
            BrainIn::Decision(d) => {
                {
                    let mut t = self.telemetry.lock();
                    match &d {
                        Ok(d) => {
                            let (best, p) = d.best();
                            t.decider = format!("ready, {:.0} ms", d.ms);
                            t.decision(now, d);
                            t.event(now, EventKind::Decision, format!("{best} {p:.2} · addressed {:.2} · question {:.2}", d.addressed, d.question));
                        }
                        Err(e) => t.decider = format!("{e:#}"),
                    }
                }
                self.brain.on_decision(now, d)
            }
            BrainIn::Manual(state) => self.brain.manual(now, &state),
            BrainIn::Config(cfg) => {
                self.brain.set_config(cfg);
                Vec::new()
            }
            BrainIn::BlendFrames(n) => {
                self.engine.send(Command::SetBlendFrames(n));
                Vec::new()
            }
            BrainIn::Pause(p) => {
                self.paused = p;
                if p {
                    let idle = self.lib.base;
                    self.engine.send(Command::Request(Request { state: idle, urgency: Urgency::Now, intensity: None }));
                }
                Vec::new()
            }
        }
    }

    fn apply(&mut self, now: f64, o: Output) {
        match o {
            Output::React(r) => {
                if self.paused && r.source != brain::Source::Manual {
                    return;
                }
                let Some(state) = self.lib.state_index(&r.state) else { return };
                let urgency = if r.urgent { Urgency::Now } else { Urgency::AtBoundary };
                self.engine.send(Command::Request(Request { state, urgency, intensity: r.intensity }));
                self.telemetry.lock().event(now, EventKind::Reaction, format!("{} ({})", r.state, r.reason));
            }
            Output::Ask(req) => {
                if self.ask_tx.try_send(req).is_err() {
                    let _ = self.brain.on_decision(now, Err(anyhow::anyhow!("decider busy")));
                }
            }
            Output::Alert(a) => {
                self.alerter.alert(&a.reason, &a.text);
                let tail: String = a.text.chars().rev().take(100).collect::<Vec<_>>().into_iter().rev().collect();
                self.telemetry.lock().event(now, EventKind::Alert, format!("{}: …{tail}", a.reason));
            }
        }
    }

    fn snapshot(&self, now: f64) {
        let transcript = self.brain.transcript();
        let window = transcript.window(now, 600.0);
        let jitter = self.engine.stats.lock().jitter_us_last;
        let mut t = self.telemetry.lock();
        t.now = now;
        t.peak = self.tap.peak();
        let peak = t.peak;
        t.history.level.push(now, peak);
        t.history.jitter_ms.push(now, jitter as f32 / 1000.0);
        t.engagement = self.brain.engagement();
        t.decision = self.brain.last_decision().cloned();
        t.transcript_open = transcript.open().is_some();
        if t.diarizer == Diarizer::Off {
            t.transcript = window.lines().map(|l| Line { speaker: None, text: l.to_string() }).collect();
            if let Some(open) = transcript.open()
                && let Some(last) = t.transcript.last_mut()
            {
                last.text = format!("{} …", open.text.trim());
            }
        } else {
            let lines = speakers::lines(t.spans.iter(), &t.speakers, self.brain.config().end_of_utterance_secs);
            t.transcript = lines;
            if t.transcript_open
                && let Some(last) = t.transcript.last_mut()
            {
                last.text.push_str(" …");
            }
        }
    }
}

pub fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7)
}

pub fn project_root() -> PathBuf {
    let here = std::env::current_dir().unwrap_or_else(|_| ".".into());
    here.ancestors().find(|p| p.join("clips/manifest.toml").exists()).map(Path::to_path_buf).unwrap_or(here)
}
