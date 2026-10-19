//! cargo run --release -p asr --example transcribe -- <audio file> [model dir] [cpu]
//! Prints the transcript; with STANDIN_EMIT_LOG=path also writes "<audio_end>\t<text>" per emission.
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let device = if args.get(3).map(String::as_str) == Some("cpu") { asr::Device::Cpu } else { asr::Device::Cuda };
    let model = args.get(2).cloned().unwrap_or_else(|| "models/parakeet/nemotron-speech-streaming-en-0.6b".into());
    let mut emit_log = std::env::var("STANDIN_EMIT_LOG").ok().map(|p| std::fs::File::create(p).expect("emit log"));
    let t = Instant::now();
    let env = asr::environment()?;
    let mut rec = asr::Recognizer::load(&env, model.as_ref(), device)?;
    eprintln!("loaded in {:.2?}, chunk {:.3}s", t.elapsed(), rec.chunk_secs());
    let samples = audio::decode_file(args[1].as_ref())?;
    let t = Instant::now();
    let mut text = String::new();
    let mut compute = Vec::new();
    for (i, block) in samples.chunks(320).enumerate() {
        if let Some(w) = rec.feed(block, (i * 320 + block.len()) as u64)? {
            compute.push(w.compute_ms);
            text.push_str(&w.text);
            if let Some(f) = emit_log.as_mut() {
                use std::io::Write;
                writeln!(f, "{:.3}\t{}", w.audio_end, w.text)?;
            }
        }
    }
    for w in rec.flush()? {
        text.push_str(&w.text);
    }
    let secs = samples.len() as f64 / audio::SAMPLE_RATE as f64;
    compute.sort_by(f32::total_cmp);
    let pct = |p: f64| compute.get(((compute.len() as f64 - 1.0) * p) as usize).copied().unwrap_or(0.0);
    eprintln!(
        "audio {secs:.1}s in {:.2?} (RTFx {:.1}); chunk compute p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        t.elapsed(),
        secs / t.elapsed().as_secs_f64(),
        pct(0.5),
        pct(0.95),
        pct(1.0)
    );
    println!("{}", text.trim());
    Ok(())
}
