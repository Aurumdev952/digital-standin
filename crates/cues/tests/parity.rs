//! Our CED frontend against sherpa-onnx's, on the talk-show excerpt. The reference was produced by
//! sherpa-onnx 1.13.8's AudioTagging before it was replaced (one line per second: speech windows out
//! of 4 from its Silero VAD, which we no longer use, then peak laughter and peak applause).
//! Needs `models/` and `test-media/laughs.wav`.

use std::path::Path;

fn parse(line: &str) -> Option<(usize, f32, f32)> {
    let speech = line.split("speech ").nth(1)?.split('/').next()?.parse().ok()?;
    let laugh = line.split("laugh ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let clap = line.split("applause ").nth(1)?.trim().parse().ok()?;
    Some((speech, laugh, clap))
}

#[test]
fn matches_sherpa_onnx_reference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let audio = root.join("test-media/laughs.wav");
    if !audio.exists() || !root.join("models/ced-mini").exists() {
        eprintln!("skipping: run scripts/fetch-models.sh and scripts/fetch-test-media.sh");
        return;
    }
    let reference: Vec<_> = include_str!("data/sherpa-laughs-timeline.txt").lines().filter_map(parse).collect();
    let env = ort::init().build().unwrap();
    let models = root.join("models");
    let mut det = cues::CueDetector::new(
        cues::Vad::default(),
        cues::Tagger::load(&env, &models.join("ced-mini/model.int8.onnx")).unwrap(),
        0.25,
    );
    let samples = audio::decode_file(&audio).unwrap();
    let mut all = Vec::new();
    for b in samples.chunks(320) {
        all.extend(det.feed(b).unwrap());
    }
    let ours: Vec<_> = all
        .chunks(4)
        .map(|s| {
            let speech = s.iter().filter(|c| c.speech).count();
            let laugh = s.iter().map(|c| c.laughter).fold(0.0, f32::max);
            let clap = s.iter().map(|c| c.applause).fold(0.0, f32::max);
            (speech, laugh, clap)
        })
        .collect();
    assert_eq!(ours.len(), reference.len());
    let n = ours.len() as f32;
    let mean = |f: &dyn Fn(usize) -> f32| (0..ours.len()).map(f).sum::<f32>() / n;
    let laugh = mean(&|i| (ours[i].1 - reference[i].1).abs());
    let clap = mean(&|i| (ours[i].2 - reference[i].2).abs());
    let worst = (0..ours.len()).map(|i| (ours[i].1 - reference[i].1).abs()).fold(0.0, f32::max);
    assert!(laugh < 0.01 && clap < 0.01 && worst < 0.1, "laugh {laugh} (worst {worst}), applause {clap}");
}
