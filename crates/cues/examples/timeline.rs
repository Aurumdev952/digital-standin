//! cargo run --release -p cues --example timeline -- <audio file>
//! Prints one line per second: speech flag and the peak laughter and applause scores.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("audio file");
    let env = ort::init().build()?;
    let mut det = cues::CueDetector::load(&env, "models".as_ref())?;
    let samples = audio::decode_file(path.as_ref())?;
    let t = std::time::Instant::now();
    let mut all = Vec::new();
    for b in samples.chunks(320) {
        all.extend(det.feed(b)?);
    }
    eprintln!("{} windows in {:.2?} ({:.2} ms each)", all.len(), t.elapsed(), t.elapsed().as_secs_f64() * 1000.0 / all.len() as f64);
    for sec in all.chunks(8) {
        let laugh = sec.iter().map(|c| c.laughter).fold(0.0, f32::max);
        let clap = sec.iter().map(|c| c.applause).fold(0.0, f32::max);
        let speech = sec.iter().filter(|c| c.speech).count();
        println!("{:6.1}s speech {speech}/8 laugh {laugh:.2} {:<20} applause {clap:.2}", sec[0].at, "#".repeat((laugh * 20.0) as usize));
    }
    Ok(())
}
