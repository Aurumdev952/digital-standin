use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Result;
use brain::Settings;
use brain::decider::{Client, default_socket};

use crate::sidecar::Kind;

/// Prints one line per check and exits non-zero when any fails.
pub fn run(root: &Path, s: &Settings) -> Result<()> {
    let mut failed = 0;
    let mut check = |name: &str, r: Result<String>| match r {
        Ok(msg) => println!("  ok    {name}: {msg}"),
        Err(e) => {
            failed += 1;
            println!("  FAIL  {name}: {e:#}");
        }
    };

    check("virtual camera", camera(s));
    check("clips", clips(root, s));
    check("audio capture", capture());
    for (name, path) in [
        ("laughter tagger", "models/ced-small/model.int8.onnx"),
        ("decision model", "models/pplx-decider-flash-v1.2/readout.safetensors"),
    ] {
        let p = root.join(path);
        check(name, if p.exists() { Ok(p.display().to_string()) } else { Err(anyhow::anyhow!("missing {}; run scripts/fetch-models.sh", p.display())) });
    }
    let asr = s.asr.resolve(root);
    check("speech model", if asr.join("encoder.onnx").exists() {
        let fast = asr.ends_with("nemotron-160ms");
        Ok(format!("{} ({})", asr.display(), if fast { "160 ms chunks" } else { "560 ms chunks; `make asr-160` builds the faster export" }))
    } else {
        Err(anyhow::anyhow!("missing {}; run scripts/fetch-models.sh", asr.display()))
    });
    check("GPU", gpu());
    check("decider sidecar", decider());
    check("diarizer sidecar", diarizer());
    if failed > 0 {
        anyhow::bail!("{failed} check(s) failed");
    }
    Ok(())
}

fn camera(s: &Settings) -> Result<String> {
    let out = Command::new("v4l2-ctl").arg("-d").arg(&s.video.device).arg("--info").output()?;
    let info = String::from_utf8_lossy(&out.stdout);
    let card = info.lines().find(|l| l.contains("Card type")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string());
    match card {
        Some(c) if info.contains("v4l2 loopback") => Ok(format!("{} is \"{c}\"", s.video.device.display())),
        _ => anyhow::bail!("{} is not a v4l2loopback device; run scripts/setup-vcam.sh", s.video.device.display()),
    }
}

fn clips(root: &Path, s: &Settings) -> Result<String> {
    let m = video::Manifest::load(&root.join(&s.video.manifest))?;
    let missing: Vec<_> = m.clips.iter().filter(|c| !c.file.exists()).map(|c| c.file.display().to_string()).collect();
    if !missing.is_empty() {
        anyhow::bail!("missing clip files: {missing:?}");
    }
    Ok(format!("{} states, {} clips, base \"{}\"", m.states.len(), m.clips.len(), m.base))
}

fn capture() -> Result<String> {
    let (tx, rx) = crossbeam_channel::unbounded();
    audio::capture_default_sink(audio::Fanout::new(Default::default(), vec![tx]))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut n = 0u64;
    while Instant::now() < deadline {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) {
            n = b.end();
        }
    }
    let sink = Command::new("pactl").arg("get-default-sink").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if n == 0 {
        anyhow::bail!("no audio blocks from the monitor of {sink} in 2 s");
    }
    Ok(format!("{:.1} s received in 2 s from the monitor of {sink}", n as f64 / audio::SAMPLE_RATE as f64))
}

fn gpu() -> Result<String> {
    let out = Command::new("nvidia-smi").args(["--query-gpu=name,memory.used,memory.free", "--format=csv,noheader"]).output()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let free_mb: f64 = line.rsplit(',').next().and_then(|f| f.trim().trim_end_matches(" MiB").parse().ok()).unwrap_or(0.0);
    if free_mb < 12_000.0 {
        anyhow::bail!("{line}: about 12 GB free is needed (decider ~10 GB, speech ~1 GB, diarizer ~0.6 GB). Is vLLM still running?");
    }
    Ok(line)
}

fn decider() -> Result<String> {
    let sock = default_socket();
    match Client::connect(&sock, Duration::from_secs(2)).and_then(|mut c| c.ping()) {
        Ok(p) => Ok(format!("answering on {} (fast path {})", sock.display(), p["fast"])),
        Err(_) => Ok(format!("not running; `standin run` starts it (socket {})", sock.display())),
    }
}

fn diarizer() -> Result<String> {
    let sock = Kind::Diarizer.default_socket();
    match std::os::unix::net::UnixStream::connect(&sock) {
        Ok(_) => Ok(format!("answering on {}", sock.display())),
        Err(_) => Ok(format!("not running; `standin run` starts it (socket {})", sock.display())),
    }
}
