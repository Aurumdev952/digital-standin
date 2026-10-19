/// Returns the first alias mentioned in `text`. Matching ignores case and punctuation, and allows
/// one ASR spelling slip in single-word aliases of five letters or more.
pub fn mentions<'a>(text: &str, aliases: &'a [String]) -> Option<&'a str> {
    let words = tokens(text);
    aliases.iter().map(String::as_str).find(|alias| {
        let want = tokens(alias);
        match want.as_slice() {
            [] => false,
            [one] if one.chars().count() >= 5 => words.iter().any(|w| edit_distance(w, one) <= 1),
            _ => words.windows(want.len()).any(|w| w == want.as_slice()),
        }
    })
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + (ca != *cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn matches_whole_words_case_insensitively() {
        assert_eq!(mentions("Thanks, SAM.", &a(&["Sam"])), Some("Sam"));
        assert_eq!(mentions("the same thing", &a(&["Sam"])), None);
    }

    #[test]
    fn tolerates_one_slip_only_in_long_names() {
        assert_eq!(mentions("over to jonathon now", &a(&["Jonathan"])), Some("Jonathan"));
        assert_eq!(mentions("ben said", &a(&["Bea"])), None);
        assert_eq!(mentions("jonny boy", &a(&["Jonathan"])), None);
    }

    #[test]
    fn multiword_alias_needs_the_full_sequence() {
        let al = a(&["Mary Ann"]);
        assert_eq!(mentions("ask mary ann about it", &al), Some("Mary Ann"));
        assert_eq!(mentions("ask mary about it", &al), None);
    }
}
