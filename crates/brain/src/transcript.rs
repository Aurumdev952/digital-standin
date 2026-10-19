use std::collections::VecDeque;

/// One stretch of speech between pauses. Speakers are not separated.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub closed: bool,
}

/// Rolling transcript assembled from streaming ASR text, closed into utterances by pauses.
#[derive(Debug, Default)]
pub struct Transcript {
    items: VecDeque<Utterance>,
}

impl Transcript {
    /// Appends ASR text heard at time `at`. Text continues the open utterance or starts one.
    pub fn push(&mut self, at: f64, text: &str) {
        match self.items.back_mut() {
            Some(u) if !u.closed => {
                u.text.push_str(text);
                u.end = at;
            }
            _ => self.items.push_back(Utterance { text: text.trim_start().to_string(), start: at, end: at, closed: false }),
        }
    }

    /// Closes the open utterance once no words arrived for `gap` seconds and nobody is speaking.
    /// Returns the utterance it closed.
    pub fn close_if_paused(&mut self, now: f64, speaking: bool, gap: f64) -> Option<&Utterance> {
        let u = self.items.back_mut()?;
        if u.closed || speaking || now - u.end < gap || u.text.trim().is_empty() {
            return None;
        }
        u.closed = true;
        Some(u)
    }

    pub fn forget_before(&mut self, t: f64) {
        while self.items.front().is_some_and(|u| u.closed && u.end < t) {
            self.items.pop_front();
        }
    }

    pub fn last(&self) -> Option<&Utterance> {
        self.items.back()
    }

    pub fn open(&self) -> Option<&Utterance> {
        self.items.back().filter(|u| !u.closed)
    }

    /// Utterances that ended within `secs` of `now`, oldest first, each prefixed with its age.
    pub fn window(&self, now: f64, secs: f64) -> String {
        self.items
            .iter()
            .filter(|u| now - u.end <= secs)
            .map(|u| format!("[{:>3.0}s ago] {}", (now - u.end).max(0.0), u.text.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_join_until_a_pause_closes_the_utterance() {
        let mut t = Transcript::default();
        t.push(1.0, " So what do");
        t.push(1.5, " you think?");
        assert!(t.close_if_paused(1.9, false, 0.7).is_none(), "pause too short");
        assert!(t.close_if_paused(3.0, true, 0.7).is_none(), "still speaking");
        let u = t.close_if_paused(3.0, false, 0.7).unwrap();
        assert_eq!((u.text.as_str(), u.start, u.end), ("So what do you think?", 1.0, 1.5));
        t.push(4.0, " Next");
        assert_eq!(t.open().unwrap().text, "Next");
        assert_eq!(t.window(4.0, 60.0), "[  2s ago] So what do you think?\n[  0s ago] Next");
    }

    #[test]
    fn forgets_only_closed_old_utterances() {
        let mut t = Transcript::default();
        t.push(1.0, "a");
        t.close_if_paused(5.0, false, 0.5);
        t.push(6.0, "b");
        t.forget_before(10.0);
        assert_eq!(t.window(10.0, 60.0), "[  4s ago] b");
    }
}
