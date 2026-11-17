//! A small fuzzy matcher: every query character must appear in order;
//! matches at word starts and in unbroken runs score higher.

/// A query prepared once — lowercased, spaces dropped — and then scored
/// against every candidate of a search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pattern {
    chars: Vec<char>,
}

impl Pattern {
    pub fn new(query: &str) -> Self {
        Self {
            chars: query
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect(),
        }
    }

    /// Score `candidate` (higher is better), or `None` if it does not
    /// match. Matching ignores case; an empty pattern matches everything.
    pub fn score(&self, candidate: &str) -> Option<i32> {
        let query = &self.chars;
        if query.is_empty() {
            return Some(0);
        }

        let mut score = 0;
        let mut next = 0;
        let mut previous_matched = false;
        let mut previous: Option<char> = None;
        let mut first_match = None;
        let mut length = 0;

        for (index, ch) in candidate.chars().enumerate() {
            length = index + 1;
            let at_word_start = previous.is_none_or(|p| !p.is_alphanumeric())
                || previous.is_some_and(|p| p.is_lowercase() && ch.is_uppercase());
            let matches = next < query.len() && ch.to_lowercase().eq(std::iter::once(query[next]));
            if matches {
                score += 10;
                if at_word_start {
                    score += 30;
                }
                if previous_matched {
                    score += 20;
                }
                first_match.get_or_insert(index);
                next += 1;
            }
            previous_matched = matches;
            previous = Some(ch);
        }
        if next < query.len() {
            return None;
        }
        // Prefer matches that start early and candidates that are not much
        // longer than the query.
        let start_penalty = first_match.unwrap_or(0).min(20) as i32 * 2;
        let length_penalty = length.min(60) as i32 / 4;
        Some(score - start_penalty - length_penalty)
    }
}

/// Score one candidate against a query not prepared beforehand.
#[cfg(test)]
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    Pattern::new(query).score(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_all_characters_in_order() {
        assert!(score("ffx", "Firefox").is_some());
        assert!(score("xff", "Firefox").is_none());
        assert!(score("firefoxes", "Firefox").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[test]
    fn ignores_case_and_spaces() {
        assert_eq!(score("FIRE", "firefox"), score("fire", "firefox"));
        assert!(score("vs code", "Visual Studio Code").is_some());
    }

    #[test]
    fn ranks_prefixes_word_starts_and_runs_higher() {
        let rank = |query: &str, better: &str, worse: &str| {
            let (b, w) = (score(query, better).unwrap(), score(query, worse).unwrap());
            assert!(
                b > w,
                "{query:?}: {better:?} ({b}) should beat {worse:?} ({w})"
            );
        };
        rank("fire", "Firefox", "Campfire Notes");
        rank("vsc", "Visual Studio Code", "Device Scanner");
        rank("term", "Terminal", "Xterm");
        rank("al", "Alacritty", "Calculator");
        rank("files", "Files", "Files (Legacy Compatibility Edition)");
    }
}
