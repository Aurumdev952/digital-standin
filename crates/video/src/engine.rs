use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use parking_lot::Mutex;

use crate::player::{Player, Request, Status};
use crate::sink::FrameSink;

pub enum Command {
    Request(Request),
    SetBlendFrames(usize),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EngineStats {
    pub frames: u64,
    /// Frames whose deadline had already passed by more than a full period.
    pub late: u64,
    pub write_errors: u64,
    pub jitter_us_max: u32,
    pub jitter_us_mean: f32,
    pub jitter_us_last: u32,
    pub status: Option<Status>,
}

/// Half-resolution RGBA copy of the output for the GUI.
#[derive(Default, Clone)]
pub struct Preview {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub seq: u64,
}

pub struct Engine {
    tx: Sender<Command>,
    pub stats: Arc<Mutex<EngineStats>>,
    pub preview: Arc<Mutex<Preview>>,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    /// Paces `player` into `sink` at the library's frame rate on a dedicated high-priority thread.
    pub fn spawn(player: Player, sink: Box<dyn FrameSink>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let stats = Arc::new(Mutex::new(EngineStats::default()));
        let preview = Arc::new(Mutex::new(Preview::default()));
        let (s, p) = (stats.clone(), preview.clone());
        let handle = std::thread::Builder::new()
            .name("video".into())
            .spawn(move || run(player, sink, rx, s, p))
            .expect("spawn video thread");
        Self { tx, stats, preview, handle: Some(handle) }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let (tx, _) = crossbeam_channel::bounded(0);
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(
    mut player: Player,
    mut sink: Box<dyn FrameSink>,
    rx: Receiver<Command>,
    stats: Arc<Mutex<EngineStats>>,
    preview: Arc<Mutex<Preview>>,
) {
    if let Err(e) = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max) {
        tracing::debug!("video thread keeps default priority: {e:?}");
    }
    let o = player.library().output;
    let period = Duration::from_secs_f64(1.0 / o.fps as f64);
    let preview_every = (o.fps / 15).max(1) as u64;
    let sleeper = spin_sleep::SpinSleeper::default();
    let mut deadline = Instant::now() + period;
    let mut n: u64 = 0;
    let mut jitter_sum = 0f64;
    loop {
        loop {
            match rx.try_recv() {
                Ok(Command::Request(r)) => player.request(r),
                Ok(Command::SetBlendFrames(b)) => player.set_blend_frames(b),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let frame = player.next_frame();
        let now = Instant::now();
        if deadline > now {
            sleeper.sleep(deadline - now);
        }
        let wrote_at = Instant::now();
        let failed = sink.write_frame(frame).is_err();
        if n.is_multiple_of(preview_every) {
            let mut p = preview.lock();
            yuyv_to_rgba_half(frame, o.width as usize, o.height as usize, &mut p);
        }
        n += 1;
        let jitter = wrote_at.saturating_duration_since(deadline);
        jitter_sum += jitter.as_micros() as f64;
        let late = jitter > period;
        {
            let mut s = stats.lock();
            s.frames = n;
            s.late += late as u64;
            s.write_errors += failed as u64;
            s.jitter_us_max = s.jitter_us_max.max(jitter.as_micros() as u32);
            s.jitter_us_mean = (jitter_sum / n as f64) as f32;
            s.jitter_us_last = jitter.as_micros() as u32;
            s.status = Some(player.status());
        }
        deadline = if late { wrote_at + period } else { deadline + period };
        if failed {
            // The loopback rejects writes while the device is reconfigured; back off briefly.
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn yuyv_to_rgba_half(src: &[u8], w: usize, h: usize, out: &mut Preview) {
    let (pw, ph) = (w / 2, h / 2);
    out.width = pw;
    out.height = ph;
    out.rgba.resize(pw * ph * 4, 0);
    for y in 0..ph {
        let row = &src[(y * 2) * w * 2..];
        for x in 0..pw {
            let px = &row[x * 4..x * 4 + 4];
            let (yy, u, v) = (px[0] as f32 - 16.0, px[1] as f32 - 128.0, px[3] as f32 - 128.0);
            let c = 1.164 * yy;
            let o = &mut out.rgba[(y * pw + x) * 4..][..4];
            o[0] = (c + 1.596 * v).clamp(0.0, 255.0) as u8;
            o[1] = (c - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8;
            o[2] = (c + 2.017 * u).clamp(0.0, 255.0) as u8;
            o[3] = 255;
        }
    }
    out.seq += 1;
}
