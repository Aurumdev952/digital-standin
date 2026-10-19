//! Shows the process growing with each small ONNX Runtime call, which is why voice activity no
//! longer goes through ONNX Runtime. Needs the official Silero model:
//! curl -L -o models/silero_vad_v5.onnx https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx
//! cargo run --release -p cues --example ortgrowth -- [base|nostate|recreate]
use ort::session::Session;
use ort::value::Tensor;

fn rss_mb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap();
    s.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() * 4096 / 1_000_000
}

fn main() -> anyhow::Result<()> {
    let env = ort::init().build()?;
    let variant = std::env::args().nth(1).unwrap_or_default();
    let mut b = Session::builder(&env)?.with_intra_threads(1).map_err(|e| anyhow::anyhow!("{e}"))?;
    if variant == "noarena" {
        b = b.with_config_entry("session.use_env_allocators", "0").map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let mut s = b.commit_from_file("models/silero_vad_v5.onnx")?;
    let recreate = variant == "recreate";
    let x: Vec<f32> = (0..576).map(|i| ((i as f32) * 0.05).sin() * 0.2).collect();
    let mut state = vec![0f32; 256];
    let before = rss_mb();
    for i in 0..=100_000 {
        let out = s.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, 576], x.clone()))?,
            "state" => Tensor::from_array(([2usize, 1, 128], state.clone()))?,
            "sr" => Tensor::from_array(([0usize; 0], vec![16_000i64]))?,
        ])?;
        if variant != "nostate" {
            state = out["stateN"].try_extract_tensor::<f32>()?.1.to_vec();
        }
        std::hint::black_box(out["output"].try_extract_tensor::<f32>()?.1[0]);
        drop(out);
        if recreate && i % 10_000 == 0 && i > 0 {
            s = Session::builder(&env)?.with_intra_threads(1).map_err(|e| anyhow::anyhow!("{e}"))?.commit_from_file("models/silero_vad_v5.onnx")?;
        }
        if i % 10_000 == 0 {
            println!("{variant:8} {i:6} runs rss {} MB", rss_mb());
        }
    }
    println!("{variant:8} grew {} MB", rss_mb() - before);
    Ok(())
}
