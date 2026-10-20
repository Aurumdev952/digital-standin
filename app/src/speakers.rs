//! Who spoke when: the diarizer's speaker probabilities on the stream clock, and the transcript
//! split into lines by speaker.

use std::collections::VecDeque;

pub const MAX_SPEAKERS: usize = 8;
/// How much history the timeline and the spans keep, in seconds.
const KEEP_SECS: f64 = 600.0;
/// A probability at or above this counts as speaking.
pub const ACTIVE: f32 = 0.5;
/// The shortest stretch of audio a span is attributed from. Words reach us a little after they
/// were spoken, and a short window would catch only their tail.
const MIN_WINDOW_SECS: f64 = 0.4;
/// The longest. Text that follows a long silence was spoken at the end of it.
const MAX_WINDOW_SECS: f64 = 2.0;

/// Speaker probabilities over `[start, end)`, in stream seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub start: f64,
    pub end: f64,
    pub p: [f32; MAX_SPEAKERS],
}

#[derive(Debug, Default)]
pub struct SpeakerTimeline {
    frames: VecDeque<Frame>,
    seen: [bool; MAX_SPEAKERS],
}

impl SpeakerTimeline {
    pub fn push(&mut self, f: Frame) {
        for (s, p) in f.p.iter().enumerate() {
            self.seen[s] |= *p >= ACTIVE;
        }
        self.frames.push_back(f);
        while self.frames.front().is_some_and(|x| f.end - x.end > KEEP_SECS) {
            self.frames.pop_front();
        }
    }

    pub fn frames(&self) -> impl DoubleEndedIterator<Item = &Frame> {
        self.frames.iter()
    }

    /// Stream time up to which speakers are known.
    pub fn covered_until(&self) -> f64 {
        self.frames.back().map_or(0.0, |f| f.end)
    }

    /// Every speaker that has spoken so far, in number order.
    pub fn seen(&self) -> impl Iterator<Item = u8> + '_ {
        (0..MAX_SPEAKERS as u8).filter(|s| self.seen[*s as usize])
    }

    /// Speakers in the latest frame at or above `threshold`, with their probability.
    pub fn active(&self, threshold: f32) -> impl Iterator<Item = (u8, f32)> + '_ {
        let p = self.frames.back().map(|f| f.p).unwrap_or_default();
        (0..MAX_SPEAKERS as u8).map(move |s| (s, p[s as usize])).filter(move |(_, p)| *p >= threshold)
    }

    /// The speaker who spoke most in `[a, b]`, judged over the frames where someone speaks.
    /// `None` until the timeline reaches `b`, or when that speaker's mean probability is below
    /// [`ACTIVE`].
    pub fn speaker_between(&self, a: f64, b: f64) -> Option<u8> {
        if self.covered_until() < b {
            return None;
        }
        let mut sum = [0f32; MAX_SPEAKERS];
        let mut voiced = 0;
        for f in self.frames.iter().filter(|f| f.end > a && f.start < b) {
            if f.p.iter().any(|p| *p >= ACTIVE) {
                voiced += 1;
                for (s, p) in sum.iter_mut().zip(f.p) {
                    *s += p;
                }
            }
        }
        let (best, total) = sum.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1))?;
        (voiced > 0 && total / voiced as f32 >= ACTIVE).then_some(best as u8)
    }

    /// The longest stretch in `[a, b]` where nobody spoke, in seconds. `None` until the timeline
    /// reaches `b`.
    pub fn longest_silence(&self, a: f64, b: f64) -> Option<f64> {
        if self.covered_until() < b {
            return None;
        }
        let (mut longest, mut run_start) = (0.0f64, None);
        for f in self.frames.iter().filter(|f| f.end > a && f.start < b) {
            if f.p.iter().any(|p| *p >= ACTIVE) {
                run_start = None;
            } else {
                let from = *run_start.get_or_insert(f.start.max(a));
                longest = longest.max(f.end.min(b) - from);
            }
        }
        Some(longest)
    }
}

/// Text from one speech-recognition result, and the audio it is attributed from, in stream seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Spans(VecDeque<Span>);

impl Spans {
    /// Adds text whose audio ended at `end`. It is attributed from the audio since the previous
    /// text, widened or cut to between [`MIN_WINDOW_SECS`] and [`MAX_WINDOW_SECS`].
    pub fn push(&mut self, end: f64, text: &str) {
        let prev = self.0.back().map_or(f64::NEG_INFINITY, |s| s.end);
        let start = prev.clamp(end - MAX_WINDOW_SECS, end - MIN_WINDOW_SECS).max(0.0);
        self.0.push_back(Span { start, end, text: text.to_string() });
        while self.0.front().is_some_and(|s| end - s.end > KEEP_SECS) {
            self.0.pop_front();
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Span> {
        self.0.iter()
    }
}

/// A transcript line spoken by one speaker; `None` when it is not known yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub speaker: Option<u8>,
    pub text: String,
}

/// Groups spans into lines. A line ends where the speaker changes or where nobody spoke for
/// `pause_secs` before the text. Text whose speaker is not known yet stays with the line before
/// it, so the newest words do not flicker between lines.
pub fn lines<'a>(spans: impl IntoIterator<Item = &'a Span>, timeline: &SpeakerTimeline, pause_secs: f64) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut prev_end = f64::NEG_INFINITY;
    for s in spans {
        let current = out.last().and_then(|l| l.speaker);
        let speaker = timeline.speaker_between(s.start, s.end).or(current);
        // Words arrive a little after they are spoken, so the pause can begin before `prev_end`.
        let paused = timeline.longest_silence(prev_end - MIN_WINDOW_SECS, s.end).is_some_and(|q| q >= pause_secs);
        prev_end = s.end;
        match out.last_mut() {
            Some(l) if !paused && speaker == l.speaker => l.text.push_str(&s.text),
            _ => out.push(Line { speaker, text: s.text.trim_start().to_string() }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 0.08;

    /// A timeline from `(seconds, speaker)` turns; `None` is silence.
    fn timeline(turns: &[(f64, Option<u8>)]) -> SpeakerTimeline {
        let mut t = SpeakerTimeline::default();
        let mut at = 0.0;
        for (secs, who) in turns {
            let end = at + secs;
            while at < end - 1e-9 {
                let mut p = [0.02; MAX_SPEAKERS];
                if let Some(s) = who {
                    p[*s as usize] = 0.95;
                }
                t.push(Frame { start: at, end: at + FRAME, p });
                at += FRAME;
            }
        }
        t
    }

    fn spans(words: &[(f64, &str)]) -> Spans {
        let mut s = Spans::default();
        for (end, text) in words {
            s.push(*end, text);
        }
        s
    }

    fn render(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| format!("{}: {}", l.speaker.map_or("?".into(), |s| format!("S{}", s + 1)), l.text)).collect()
    }

    #[test]
    fn alternating_speakers_split_lines_at_the_word_where_the_turn_changes() {
        let t = timeline(&[(4.0, Some(0)), (4.0, Some(1)), (4.0, Some(0))]);
        let s = spans(&[(1.0, " hello"), (1.5, " there"), (3.5, " friend"), (4.6, " hi"), (5.2, " back"), (7.6, " so"), (8.9, " again")]);
        assert_eq!(render(&lines(s.iter(), &t, 5.0)), ["S1: hello there friend", "S2: hi back so", "S1: again"]);
    }

    #[test]
    fn words_past_the_diarizer_stay_on_the_current_line() {
        let t = timeline(&[(4.0, Some(1))]);
        let s = spans(&[(2.0, " still"), (3.0, " talking"), (4.5, " and"), (5.0, " more")]);
        assert_eq!(render(&lines(s.iter(), &t, 5.0)), ["S2: still talking and more"]);
    }

    #[test]
    fn a_pause_starts_a_new_line_for_the_same_speaker() {
        let t = timeline(&[(2.0, Some(0)), (2.0, None), (2.0, Some(0))]);
        let s = spans(&[(1.0, " one"), (1.8, " two"), (5.0, " three")]);
        assert_eq!(render(&lines(s.iter(), &t, 0.7)), ["S1: one two", "S1: three"]);
    }

    #[test]
    fn text_before_any_speaker_is_known_is_unattributed() {
        let t = SpeakerTimeline::default();
        let s = spans(&[(0.5, " early"), (0.9, " words")]);
        assert_eq!(render(&lines(s.iter(), &t, 5.0)), ["?: early words"]);
    }

    #[test]
    fn silence_before_words_does_not_hide_their_speaker() {
        let t = timeline(&[(1.0, Some(0)), (6.0, None), (1.0, Some(1))]);
        let s = spans(&[(0.9, " before"), (7.9, " after")]);
        assert_eq!(render(&lines(s.iter(), &t, 0.7)), ["S1: before", "S2: after"]);
    }

    #[test]
    fn active_and_seen_report_speakers_at_or_above_the_threshold() {
        let mut t = timeline(&[(1.0, Some(2)), (1.0, Some(0))]);
        let mut p = [0.0; MAX_SPEAKERS];
        (p[0], p[3]) = (0.7, 0.6);
        t.push(Frame { start: 2.0, end: 2.08, p });
        assert_eq!(t.active(ACTIVE).map(|a| a.0).collect::<Vec<_>>(), [0, 3]);
        assert_eq!(t.seen().collect::<Vec<_>>(), [0, 2, 3]);
    }
}
