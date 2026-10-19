//! Writes "<word>\t<start>\t<end>" per word from Parakeet TDT, whose duration prediction gives
//! acoustically grounded word timings. The reference for measuring streaming word latency.
//! cargo run --release -p asr --example word_times -- <audio file> <tdt model dir> > words.tsv
use parakeet_rs::{ExecutionConfig, ExecutionProvider, ParakeetTDT, TimestampMode, Transcriber};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let env = asr::environment()?;
    let cfg = ExecutionConfig::new().with_environment(env).with_execution_provider(ExecutionProvider::Cuda);
    let mut tdt = ParakeetTDT::from_pretrained(&args[2], Some(cfg))?;
    let audio = audio::decode_file(args[1].as_ref())?;
    let r = tdt.transcribe_samples(audio, audio::SAMPLE_RATE, 1, Some(TimestampMode::Words))?;
    for t in r.tokens {
        println!("{}\t{:.3}\t{:.3}", t.text.trim(), t.start, t.end);
    }
    Ok(())
}
