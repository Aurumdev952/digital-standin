mod alerts;
mod doctor;
mod eval;
mod gui;
mod live;
mod replay;
mod sidecar;
mod speakers;
mod telemetry;
mod theme;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use brain::Settings;
use clap::{Parser, Subcommand};
use video::{Command, Engine, Library, Manifest, Player, Request, Urgency, V4l2Sink};

use crate::live::{AudioSource, Live, Options, project_root, seed};

#[derive(Parser)]
#[command(name = "standin", about = "Digital Standin: a reactive virtual camera for meetings")]
struct Cli {
    /// Settings file (created with defaults when saved from the GUI).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the control panel and start the stand-in (the default).
    Run {
        /// Feed this file instead of the meeting audio, in real time.
        #[arg(long)]
        audio_file: Option<PathBuf>,
        /// Do not start or use the decision model.
        #[arg(long)]
        no_decider: bool,
        /// Do not start or use the diarizer, which labels who speaks.
        #[arg(long)]
        no_diarizer: bool,
        /// No window; print events to the terminal.
        #[arg(long)]
        headless: bool,
        /// Stop after this many seconds (headless only).
        #[arg(long)]
        seconds: Option<f64>,
        /// Render frames but send them nowhere, for tests that run several instances.
        #[arg(long)]
        no_camera: bool,
    },
    /// Run the pipeline offline on an audio file and render the stand-in's reactions to a video.
    Replay {
        audio: PathBuf,
        /// Output path without extension; writes .mp4, .srt and .jsonl.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Labels to score against (defaults to <audio>.labels.toml when present).
        #[arg(long)]
        labels: Option<PathBuf>,
        #[arg(long)]
        no_decider: bool,
        /// Run speech recognition on all audio, not only on speech (for comparison).
        #[arg(long)]
        no_gate: bool,
        #[arg(long, default_value_t = 7)]
        seed: u64,
    },
    /// Play clips to the virtual camera; type a state name (prefix `!` to crossfade immediately).
    Play,
    /// Capture the meeting audio for a while and save it, to check capture works.
    Listen {
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
        #[arg(long, default_value = "listen.wav")]
        out: PathBuf,
    },
    /// Check the camera device, audio capture, models, decider and GPU.
    Doctor,
    /// Internal: the speech-recognition worker process.
    #[command(hide = true)]
    AsrWorker {
        #[arg(long)]
        model: PathBuf,
    },
}

fn main() -> Result<()> {
    // ONNX Runtime warns about every CUDA graph it partitions; only its errors matter here.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,ort=error".into()))
        .init();
    let cli = Cli::parse();
    let root = project_root();
    let config_path = cli.config.clone().unwrap_or_else(Settings::default_path);
    let settings = Settings::load(&config_path)?;
    match cli.cmd.unwrap_or(Cmd::Run { audio_file: None, no_decider: false, no_diarizer: false, headless: false, seconds: None, no_camera: false }) {
        Cmd::Run { audio_file, no_decider, no_diarizer, headless, seconds, no_camera } => {
            let audio = audio_file.map(AudioSource::File).unwrap_or(AudioSource::Monitor);
            let sink: Option<Box<dyn video::FrameSink>> = if no_camera { Some(Box::new(video::sink::NullSink)) } else { None };
            let opts = Options { root, settings: settings.clone(), audio, decider: !no_decider, diarizer: !no_diarizer, sink };
            if headless {
                headless_run(Live::start(opts)?, seconds)
            } else {
                gui::run(opts, settings, config_path)
            }
        }
        Cmd::Replay { audio, out, labels, no_decider, no_gate, seed } => {
            let out = out.unwrap_or_else(|| replay::default_out(&audio));
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let t = std::time::Instant::now();
            let opts = replay::ReplayOptions { root, audio: audio.clone(), out: out.clone(), settings, decider: !no_decider, gate: !no_gate, seed };
            let result = replay::run(opts)?;
            eprintln!("replayed {:.1} s of audio in {:.1?} -> {}.mp4", result.duration, t.elapsed(), out.display());
            let (tagged, skipped) = result.tagger_runs;
            println!(
                "speech recognition ran on {:.1} of {:.1} s ({:.0}%); laughter tagger ran on {tagged} of {} windows",
                result.asr_secs,
                result.duration,
                100.0 * result.asr_secs / result.duration,
                tagged + skipped
            );
            let labels = labels.or_else(|| Some(audio.with_extension("labels.toml")).filter(|p| p.exists()));
            if let Some(l) = labels {
                let report = eval::score(&eval::Labels::load(&l)?, &result);
                std::fs::write(out.with_extension("report.txt"), &report)?;
                print!("{report}");
            }
            Ok(())
        }
        Cmd::Play => play(&root.join(&settings.video.manifest), &settings.video.device, settings.video.blend_frames),
        Cmd::Listen { seconds, out } => listen(seconds, &out),
        Cmd::Doctor => doctor::run(&root, &settings),
        Cmd::AsrWorker { model } => asr::worker::serve(&model, asr::Device::Cuda),
    }
}

/// Exits without running C++ static destructors. ONNX Runtime and CUDA tear down for seconds
/// under model threads that are still running, and log errors doing it; the OS frees all of it.
pub fn exit_now(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: _exit ends the process immediately; nothing runs after it.
    unsafe { nix::libc::_exit(code) }
}

fn headless_run(mut live: Live, seconds: Option<f64>) -> Result<()> {
    let start = std::time::Instant::now();
    let mut printed = 0u64;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let t = live.telemetry.lock();
        let new = (t.events_total - printed) as usize;
        for e in t.events.iter().skip(t.events.len().saturating_sub(new)) {
            println!("{:7.2}s {:?} {}", e.t, e.kind, e.text);
        }
        printed = t.events_total;
        drop(t);
        if seconds.is_some_and(|s| start.elapsed().as_secs_f64() >= s) {
            let s = *live.engine.stats.lock();
            println!("video: {} frames, {} late, jitter mean {:.0} us max {} us", s.frames, s.late, s.jitter_us_mean, s.jitter_us_max);
            if let Some(share) = live.telemetry.lock().asr_share() {
                println!("speech recognition ran on {:.0}% of the audio", share * 100.0);
            }
            let t = live.telemetry.lock();
            if t.diarizer == telemetry::Diarizer::Off {
                if let Some(last) = t.transcript.last() {
                    println!("last transcript line: {}", last.text);
                }
            } else {
                println!("transcript by speaker:");
                for l in &t.transcript {
                    println!("  {}  {}", l.speaker.map_or("??".into(), |s| format!("S{}", s + 1)), l.text);
                }
            }
            drop(t);
            live.shutdown();
            exit_now(0);
        }
    }
}

fn play(manifest: &Path, device: &Path, blend_frames: usize) -> Result<()> {
    let t = std::time::Instant::now();
    let lib = Arc::new(Library::load(&Manifest::load(manifest)?)?);
    let frames: usize = lib.clips.iter().map(|c| c.frames.len()).sum();
    eprintln!(
        "decoded {} clips, {frames} frames, {:.0} MB in {:.2?}",
        lib.clips.len(),
        (frames * lib.frame_bytes()) as f64 / 1e6,
        t.elapsed()
    );
    let names: Vec<String> = lib.states.iter().map(|s| s.name.clone()).collect();
    let engine = Engine::spawn(Player::new(lib.clone(), blend_frames, seed()), Box::new(V4l2Sink::open(device, lib.output)?));
    eprintln!("streaming to {}; states: {}", device.display(), names.join(", "));
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let (urgency, name) = match line.trim().strip_prefix('!') {
            Some(n) => (Urgency::Now, n),
            None => (Urgency::AtBoundary, line.trim()),
        };
        if name == "stats" {
            eprintln!("{:?}", *engine.stats.lock());
            continue;
        }
        match lib.state_index(name) {
            Some(state) => engine.send(Command::Request(Request { state, urgency, intensity: None })),
            None => eprintln!("unknown state {name:?}"),
        }
    }
    eprintln!("{:?}", *engine.stats.lock());
    Ok(())
}

fn listen(seconds: f64, out: &Path) -> Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    audio::capture_default_sink(audio::Fanout::new(Default::default(), vec![tx]))?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: audio::SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut wav = hound::WavWriter::create(out, spec)?;
    let want = (seconds * audio::SAMPLE_RATE as f64) as u64;
    let (mut peak, mut sum_sq, mut n) = (0f32, 0f64, 0u64);
    while n < want {
        let block = rx.recv()?;
        for &s in block.samples.iter() {
            wav.write_sample(s)?;
            peak = peak.max(s.abs());
            sum_sq += (s as f64).powi(2);
        }
        n = block.end();
    }
    wav.finalize()?;
    eprintln!(
        "captured {:.2} s, peak {peak:.3}, rms {:.4} -> {}",
        n as f64 / audio::SAMPLE_RATE as f64,
        (sum_sq / n as f64).sqrt(),
        out.display()
    );
    Ok(())
}
