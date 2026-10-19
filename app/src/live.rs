use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use audio::Tap;
use brain::decider::{self, Client};
use brain::{Brain, BrainConfig, Decision, Output, Settings, StateInfo};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use video::{Command, Engine, Library, Manifest, Player, Request, Urgency};

use crate::alerts::Alerter;
use crate::sidecar::Sidecar;
use crate::telemetry::{CuePoint, EventKind, Telemetry};

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
    pub sink: Option<Box<dyn video::FrameSink>>,
}

/// The running stand-in: every thread, and the handles the GUI needs.
pub struct Live {
    pub telemetry: Arc<Mutex<Telemetry>>,
    pub engine: Arc<Engine>,
    pub lib: Arc<Library>,
    pub tx: Sender<BrainIn>,
    sidecar: Option<Sidecar>,
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

        let sidecar = if opts.decider { Some(Sidecar::ensure(&opts.root, decider::default_socket())?) } else { None };

        let models = opts.root.join("models");
        let env = asr::environment()?;
        let rec = asr::Recognizer::load(&env, &s.asr.resolve(&opts.root), asr::Device::Cuda)?;
        tracing::info!("speech model chunk {:.0} ms", rec.chunk_secs() * 1000.0);
        let det = cues::CueDetector::load(&env, &models)?;
        tracing::info!("models loaded in {:.2?}", t0.elapsed());

        // Audio goes to the cue detector, whose voice detector gates what reaches speech recognition.
        let (asr_tx, asr_rx) = crossbeam_channel::bounded(512);
        let (cue_tx, cue_rx) = crossbeam_channel::bounded(256);
        let (words_tx, words_rx) = crossbeam_channel::unbounded();
        let (cues_tx, cues_rx) = crossbeam_channel::unbounded();
        let gate = cues::Gate::default();
        let gate_stats = gate.stats.clone();
        asr::spawn(rec, asr_rx, words_tx);
        cues::spawn(det, gate, cue_rx, cues_tx, asr_tx);

        let tap = Arc::new(Tap::default());
        let fanout = audio::Fanout::new(tap.clone(), vec![cue_tx]);
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
        let socket = sidecar.as_ref().map(|s| s.socket.clone());
        spawn_decider_client(socket, ask_rx, tx.clone());

        let telemetry = Arc::new(Mutex::new(Telemetry { gate: Some(gate_stats), ..Default::default() }));
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

        spawn_resource_monitor(telemetry.clone());
        Ok(Self { telemetry, engine, lib, tx, sidecar, _ort: env })
    }

    /// Stops what must not outlive the app. Call before `process::exit`, which skips destructors.
    pub fn shutdown(&mut self) {
        if let Some(s) = self.sidecar.as_mut() {
            s.stop();
        }
    }

    /// Reports, once, that the decider sidecar process died.
    pub fn sidecar_exited(&mut self) -> Option<String> {
        let msg = self.sidecar.as_mut()?.exited()?;
        self.sidecar = None;
        Some(msg)
    }
}

/// Samples this process's CPU and memory and the GPU's memory and load every 2 s.
fn spawn_resource_monitor(telemetry: Arc<Mutex<Telemetry>>) {
    std::thread::Builder::new()
        .name("resources".into())
        .spawn(move || {
            let cpu_secs = || {
                let usage = nix::sys::resource::getrusage(nix::sys::resource::UsageWho::RUSAGE_SELF).ok()?;
                let tv = |t: nix::sys::time::TimeVal| t.tv_sec() as f64 + t.tv_usec() as f64 / 1e6;
                Some(tv(usage.user_time()) + tv(usage.system_time()))
            };
            let rss_mb = || {
                let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
                Some(statm.split_whitespace().nth(1)?.parse::<f64>().ok()? * 4096.0 / 1e6)
            };
            let gpu = || {
                let out = std::process::Command::new("nvidia-smi")
                    .args(["--query-gpu=memory.used,utilization.gpu", "--format=csv,noheader,nounits"])
                    .output()
                    .ok()?;
                let line = String::from_utf8_lossy(&out.stdout);
                let mut it = line.lines().next()?.split(',').map(|v| v.trim().parse::<f32>().ok());
                Some((it.next()??, it.next()??))
            };
            let mut last = (Instant::now(), cpu_secs().unwrap_or(0.0));
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let (now_i, cpu) = (Instant::now(), cpu_secs().unwrap_or(0.0));
                let pct = (cpu - last.1) / now_i.duration_since(last.0).as_secs_f64() * 100.0;
                last = (now_i, cpu);
                let rss = rss_mb();
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
        t.transcript = window.lines().map(|l| (now, l.to_string(), true)).collect();
        if let Some(open) = transcript.open()
            && let Some(last) = t.transcript.last_mut()
        {
            last.2 = false;
            last.1 = format!("{} …", open.text.trim());
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
