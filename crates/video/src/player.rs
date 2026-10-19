use std::sync::Arc;

use rand::Rng;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use rayon::prelude::*;

use crate::library::Library;
use crate::manifest::ClipKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// Let a playing one-shot finish its motion first. Loops are left immediately.
    AtBoundary,
    /// Crossfade into the new clip on the next frame.
    Now,
}

#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub state: usize,
    pub urgency: Urgency,
    pub intensity: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    clip: usize,
    frame: usize,
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Steady(Cursor),
    Blend { from: Cursor, to: Cursor, step: usize, len: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: usize,
    pub clip: usize,
    pub blending: bool,
    pub pending: Option<usize>,
}

/// Frame-accurate playback over a [`Library`]. Each call to [`Player::next_frame`] advances one
/// output frame. Every clip change, including loop wrap-around, crossfades for `blend_frames`.
pub struct Player {
    lib: Arc<Library>,
    mode: Mode,
    pending: Option<Request>,
    blend_frames: usize,
    last_clip_of_state: Vec<Option<usize>>,
    rng: SmallRng,
    out: Vec<u8>,
}

impl Player {
    pub fn new(lib: Arc<Library>, blend_frames: usize, seed: u64) -> Self {
        let mut p = Self {
            mode: Mode::Steady(Cursor { clip: 0, frame: 0 }),
            pending: None,
            blend_frames,
            last_clip_of_state: vec![None; lib.states.len()],
            rng: SmallRng::seed_from_u64(seed),
            out: vec![0; lib.frame_bytes()],
            lib,
        };
        let first = p.pick_clip(p.lib.base, None);
        p.mode = Mode::Steady(Cursor { clip: first, frame: 0 });
        p
    }

    pub fn library(&self) -> &Arc<Library> {
        &self.lib
    }

    pub fn set_blend_frames(&mut self, n: usize) {
        self.blend_frames = n;
    }

    /// A newer request replaces an older one that has not started yet.
    pub fn request(&mut self, req: Request) {
        self.pending = Some(req);
    }

    pub fn status(&self) -> Status {
        let (clip, blending) = match self.mode {
            Mode::Steady(c) => (c.clip, false),
            Mode::Blend { to, .. } => (to.clip, true),
        };
        Status { state: self.lib.clips[clip].state, clip, blending, pending: self.pending.map(|r| r.state) }
    }

    pub fn next_frame(&mut self) -> &[u8] {
        if let Mode::Steady(cur) = self.mode {
            self.maybe_start_transition(cur);
        }
        match self.mode {
            Mode::Steady(cur) => {
                self.mode = Mode::Steady(Cursor { clip: cur.clip, frame: cur.frame + 1 });
                &self.lib.clips[cur.clip].frames[cur.frame]
            }
            Mode::Blend { from, to, step, len } => {
                let a = &self.lib.clips[from.clip].frames[from.frame];
                let b = &self.lib.clips[to.clip].frames[to.frame];
                let w = (((step + 1) * 256) / (len + 1)) as u16;
                blend_into(&mut self.out, a, b, w);
                let from_len = self.lib.clips[from.clip].frames.len();
                let from = Cursor { clip: from.clip, frame: (from.frame + 1).min(from_len - 1) };
                let to = self.advance(to);
                self.mode = if step + 1 >= len { Mode::Steady(to) } else { Mode::Blend { from, to, step: step + 1, len } };
                &self.out
            }
        }
    }

    fn advance(&self, c: Cursor) -> Cursor {
        let n = self.lib.clips[c.clip].frames.len();
        Cursor { clip: c.clip, frame: (c.frame + 1).min(n - 1) }
    }

    fn maybe_start_transition(&mut self, cur: Cursor) {
        let clip_len = self.lib.clips[cur.clip].frames.len();
        let len = self.blend_frames.min(clip_len / 2);
        // A loop has no motion to finish, so any request leaves it right away.
        let in_loop = self.lib.states[self.lib.clips[cur.clip].state].kind == ClipKind::Loop;
        let urgent = matches!(self.pending, Some(r) if r.urgency == Urgency::Now || in_loop);
        // Without blending the cursor runs one past the last frame before cutting.
        let at_tail = cur.frame + len >= clip_len;
        if !urgent && !at_tail {
            return;
        }
        let (state, intensity) = match self.pending.take() {
            Some(r) => (r.state, r.intensity),
            None => {
                let s = self.lib.clips[cur.clip].state;
                (if self.lib.states[s].kind == ClipKind::Loop { s } else { self.lib.base }, None)
            }
        };
        let next = self.pick_clip(state, intensity);
        let next_len = self.lib.clips[next].frames.len();
        let len = len.min(next_len / 2).max(1);
        let to = Cursor { clip: next, frame: 0 };
        let cur = Cursor { clip: cur.clip, frame: cur.frame.min(clip_len - 1) };
        self.mode = if self.blend_frames == 0 { Mode::Steady(to) } else { Mode::Blend { from: cur, to, step: 0, len } };
    }

    fn pick_clip(&mut self, state: usize, intensity: Option<f32>) -> usize {
        let ids = &self.lib.states[state].clips;
        let last = self.last_clip_of_state[state];
        let candidates: Vec<usize> = match ids.len() {
            1 => ids.clone(),
            _ => ids.iter().copied().filter(|&c| Some(c) != last).collect(),
        };
        let chosen = match intensity {
            Some(want) => *candidates
                .iter()
                .min_by(|&&a, &&b| {
                    let d = |c: usize| (self.lib.clips[c].intensity - want).abs();
                    d(a).total_cmp(&d(b))
                })
                .expect("state has clips"),
            None => {
                let total: f32 = candidates.iter().map(|&c| self.lib.clips[c].weight).sum();
                let mut r = self.rng.random::<f32>() * total;
                *candidates
                    .iter()
                    .find(|&&c| {
                        r -= self.lib.clips[c].weight;
                        r <= 0.0
                    })
                    .unwrap_or(candidates.last().expect("state has clips"))
            }
        };
        self.last_clip_of_state[state] = Some(chosen);
        chosen
    }
}

/// `out = a + (b - a) * w / 256`, per byte. Valid for packed YUYV since every byte is a
/// linear sample.
pub fn blend_into(out: &mut [u8], a: &[u8], b: &[u8], w: u16) {
    const CHUNK: usize = 64 * 1024;
    let wa = 256 - w;
    out.par_chunks_mut(CHUNK).zip(a.par_chunks(CHUNK)).zip(b.par_chunks(CHUNK)).for_each(|((o, a), b)| {
        for ((o, &a), &b) in o.iter_mut().zip(a).zip(b) {
            *o = ((a as u16 * wa + b as u16 * w) >> 8) as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Frame;
    use crate::manifest::Output;

    const OUT: Output = Output { width: 2, height: 1, fps: 30 };

    /// Each frame is 4 bytes filled with `tag + index`, so the byte value identifies the frame.
    fn clip(tag: u8, n: usize) -> Vec<Frame> {
        (0..n).map(|i| Frame::from(vec![tag + i as u8; 4])).collect()
    }

    fn lib() -> Arc<Library> {
        Arc::new(Library::from_parts(
            OUT,
            vec![
                ("idle".into(), ClipKind::Loop, vec![clip(0, 10)]),
                ("nod".into(), ClipKind::Oneshot, vec![clip(100, 6)]),
            ],
            0,
        ))
    }

    fn run(p: &mut Player, n: usize) -> Vec<u8> {
        (0..n).map(|_| p.next_frame()[0]).collect()
    }

    #[test]
    fn loops_base_without_requests_and_cuts_cleanly_without_blend() {
        let mut p = Player::new(lib(), 0, 1);
        assert_eq!(run(&mut p, 12), vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1]);
    }

    #[test]
    fn request_leaves_loop_immediately() {
        let mut p = Player::new(lib(), 0, 1);
        run(&mut p, 3);
        p.request(Request { state: 1, urgency: Urgency::AtBoundary, intensity: None });
        assert_eq!(run(&mut p, 2), vec![100, 101]);
    }

    #[test]
    fn oneshot_finishes_before_next_request_then_returns_to_base() {
        let mut p = Player::new(lib(), 0, 1);
        p.request(Request { state: 1, urgency: Urgency::AtBoundary, intensity: None });
        run(&mut p, 2);
        p.request(Request { state: 1, urgency: Urgency::AtBoundary, intensity: None });
        let frames = run(&mut p, 4 + 6 + 2);
        assert_eq!(frames, vec![102, 103, 104, 105, 100, 101, 102, 103, 104, 105, 0, 1]);
    }

    #[test]
    fn urgent_request_starts_crossfade_immediately() {
        let mut p = Player::new(lib(), 2, 1);
        run(&mut p, 3);
        p.request(Request { state: 1, urgency: Urgency::Now, intensity: None });
        let f = run(&mut p, 3);
        // Blend weights are 1/3 then 2/3 of the way from idle frames 3,4 to nod frames 100,101.
        assert_eq!(f[0], ((3u16 * 171 + 100 * 85) >> 8) as u8);
        assert_eq!(f[1], ((4u16 * 86 + 101 * 170) >> 8) as u8);
        assert_eq!(f[2], 102);
        assert!(p.status().state == 1 && !p.status().blending);
    }

    #[test]
    fn loop_wraparound_crossfades_tail_into_head() {
        let mut p = Player::new(lib(), 2, 1);
        let f = run(&mut p, 11);
        assert_eq!(&f[..8], &[0, 1, 2, 3, 4, 5, 6, 7]);
        assert!(f[8] > 0 && f[8] < 8, "frame 8 blends tail 8 with head 0, got {}", f[8]);
        assert_eq!(f[10], 2);
    }

    #[test]
    fn blend_endpoints_are_exact() {
        let a = [10u8, 200, 0, 255];
        let b = [250u8, 0, 255, 0];
        let mut o = [0u8; 4];
        blend_into(&mut o, &a, &b, 0);
        assert_eq!(o, a);
        blend_into(&mut o, &a, &b, 128);
        assert_eq!(o, [130, 100, 127, 127]);
    }
}
