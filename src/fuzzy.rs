//! A small fuzzy matcher for picker lists of a few hundred short labels.
//!
//! Ranking, best first: the label starts with the query, a word in it starts with the query,
//! the query appears anywhere, then the query's characters appear in order (`tcno` → `techno`).
//! Each tier's scores are strictly above the next one's. Case-insensitive. Multi-word queries
//! match each word separately, in any order (`metal doom` finds `doom metal`).

/// Scores at or above this mean the query appears as a substring (not just scattered letters).
pub const SUBSTRING_SCORE: u32 = 500;

/// Scores how well `candidate` matches `query`; `None` if it doesn't match. Higher is better.
#[must_use]
pub fn score(query: &str, candidate: &str) -> Option<u32> {
    let candidate = candidate.to_lowercase();
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Some(0);
    }
    // Whole query first, so "drum bass" still ranks an exact "drum bass" label highest.
    if words.len() > 1
        && let Some(s) = score_word(&words.join(" "), &candidate)
    {
        return Some(s);
    }
    let mut total = 0;
    for word in &words {
        total += score_word(word, &candidate)?;
    }
    let count = u32::try_from(words.len()).unwrap_or(u32::MAX);
    Some(total / count)
}

fn score_word(query: &str, candidate: &str) -> Option<u32> {
    if let Some(pos) = candidate.find(query) {
        let at_word_start = candidate[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let base = match (pos, at_word_start) {
            (0, _) => 3000,
            (_, true) => 2000,
            _ => 1000,
        };
        // Prefer shorter candidates: "techno" over "techno-pop" for "techno". Capped so a
        // substring always outranks a subsequence (< 500).
        let length_penalty = u32::try_from(candidate.len()).unwrap_or(u32::MAX).min(400);
        return Some(base - length_penalty);
    }

    // Subsequence: every query character in order. Fewer gaps is better.
    let mut chars = candidate.chars();
    let mut gaps = 0u32;
    for q in query.chars() {
        let mut skipped = 0;
        loop {
            let c = chars.next()?;
            if c == q {
                break;
            }
            skipped += 1;
        }
        gaps += skipped;
    }
    Some(499u32.saturating_sub(gaps.min(499)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[test]
    fn ranks_prefix_then_word_then_substring_then_subsequence() {
        let prefix = score("tech", "techno").unwrap();
        let word = score("tech", "detroit techno").unwrap();
        let inside = score("tech", "biotechnology").unwrap();
        let subsequence = score("tcno", "techno").unwrap();
        assert!(prefix > word, "{prefix} > {word}");
        assert!(word > inside, "{word} > {inside}");
        assert!(inside > subsequence, "{inside} > {subsequence}");
    }

    #[test]
    fn is_case_insensitive_and_rejects_non_matches() {
        assert!(score("BERLIN", "berlin").is_some());
        assert!(score("berlin", "Berlin").is_some());
        assert_eq!(score("xyz", "techno"), None);
        assert_eq!(score("onhcet", "techno"), None, "order matters");
    }

    #[test]
    fn words_match_in_any_order() {
        assert!(score("metal doom", "doom metal").is_some());
        assert!(
            score("doom metal", "doom metal").unwrap() > score("metal doom", "doom metal").unwrap()
        );
        assert_eq!(
            score("metal jazz", "doom metal"),
            None,
            "every word must match"
        );
    }

    #[test]
    fn substring_in_a_long_label_beats_a_subsequence() {
        let long = "a".repeat(600) + "tech";
        assert!(score("tech", &long).unwrap() > score("tcno", "techno").unwrap());
    }

    #[test]
    fn shorter_exact_matches_win() {
        assert!(score("techno", "techno").unwrap() > score("techno", "techno-pop").unwrap());
    }

    #[test]
    fn handles_non_ascii() {
        assert!(score("zür", "Zürich").is_some());
        assert!(score("東京", "東京 tokyo").is_some());
    }
}
