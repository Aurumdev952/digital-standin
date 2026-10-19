//! Sweeps tagger settings against labelled laughter and a laughter-free recording.
//! cargo run --release -p cues --example tagger_eval -- <labelled.wav> <labels.toml> <plain.wav> <model.onnx> [window_s] [hop_s]
use std::collections::BTreeMap;

const LAUGH_ADULT: &[usize] = &[16, 18, 19, 20, 21];
const LAUGH_ALL: &[usize] = &[16, 17, 18, 19, 20, 21];

fn series(tagger: &mut cues::Tagger, x: &[f32], win: usize, hop: usize) -> Vec<(f64, Vec<f32>)> {
    (0..x.len().saturating_sub(win) / hop + 1)
        .map(|k| {
            let end = k * hop + win;
            ((end as f64) / 16_000.0, tagger.tag(&x[k * hop..end]).unwrap())
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let win = (a.get(5).map_or(1.0, |s| s.parse().unwrap()) * 16_000.0) as usize;
    let hop = (a.get(6).map_or(0.25, |s| s.parse().unwrap()) * 16_000.0) as usize;
    let labels: Vec<(f64, f64)> = std::fs::read_to_string(&a[2])?
        .split("[[event]]")
        .skip(1)
        .map(|e| {
            let get = |k: &str| e.lines().find(|l| l.starts_with(k)).unwrap().split('=').nth(1).unwrap().trim().parse::<f64>().unwrap();
            (get("start"), get("end"))
        })
        .collect();
    let env = ort::init().build()?;
    let mut tagger = cues::Tagger::load(&env, a[4].as_ref())?;
    let t = std::time::Instant::now();
    let mixed = series(&mut tagger, &audio::decode_file(a[1].as_ref())?, win, hop);
    let per_window = t.elapsed().as_secs_f64() * 1000.0 / mixed.len() as f64;
    let plain = series(&mut tagger, &audio::decode_file(a[3].as_ref())?, win, hop);
    let minutes = plain.last().map_or(1.0, |p| p.0 / 60.0);
    println!("model {} window {:.2}s hop {:.3}s: {:.1} ms/window", a[4], win as f64 / 16e3, hop as f64 / 16e3, per_window);
    for (name, classes) in [("adult", LAUGH_ADULT), ("incl-baby", LAUGH_ALL)] {
        let score = |p: &Vec<f32>| classes.iter().map(|&i| p[i]).fold(0.0f32, f32::max);
        let mut rows = BTreeMap::new();
        for th in [0.06f32, 0.08, 0.1, 0.12, 0.15, 0.2] {
            let mut lat = Vec::new();
            for &(s, e) in &labels {
                if let Some(x) = mixed.iter().find(|(t, p)| *t >= s && *t <= e + 1.5 && score(p) >= th) {
                    lat.push(x.0 - s);
                }
            }
            lat.sort_by(f64::total_cmp);
            let mut episodes = 0;
            let mut above = false;
            for (_, p) in &plain {
                let now = score(p) >= th;
                episodes += (now && !above) as usize;
                above = now;
            }
            let med = lat.get(lat.len() / 2).copied().unwrap_or(f64::NAN);
            rows.insert((th * 100.0) as u32, format!("recall {:>2}/{} median {:.2}s  false episodes {:.2}/min", lat.len(), labels.len(), med, episodes as f64 / minutes));
        }
        for (th, r) in rows {
            println!("  {name:9} th {:.2}: {r}", th as f32 / 100.0);
        }
    }
    Ok(())
}
