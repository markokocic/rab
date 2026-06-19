/// Result of fuzzy matching.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

/// Fuzzy match a query against text.
/// Characters must appear in order (not necessarily consecutive).
/// Lower score = better match.
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();

    let match_query = |normalized_query: &str| -> FuzzyMatch {
        if normalized_query.is_empty() {
            return FuzzyMatch {
                matches: true,
                score: 0.0,
            };
        }

        let query_chars: Vec<char> = normalized_query.chars().collect();
        let text_chars: Vec<char> = text_lower.chars().collect();

        if query_chars.len() > text_chars.len() {
            return FuzzyMatch {
                matches: false,
                score: 0.0,
            };
        }

        let mut query_idx = 0;
        let mut score: f64 = 0.0;
        let mut last_match_idx: i32 = -1;
        let mut consecutive = 0;

        for (i, ch) in text_chars.iter().enumerate() {
            if query_idx >= query_chars.len() {
                break;
            }

            if *ch == query_chars[query_idx] {
                let is_word_boundary = i == 0
                    || matches!(
                        text_lower.as_bytes().get(i - 1),
                        Some(b' ') | Some(b'-') | Some(b'_') | Some(b'.') | Some(b'/') | Some(b':')
                    );

                if last_match_idx == (i as i32) - 1 {
                    consecutive += 1;
                    score -= (consecutive as f64) * 5.0;
                } else {
                    consecutive = 0;
                    if last_match_idx >= 0 {
                        score += ((i as i32) - last_match_idx - 1) as f64 * 2.0;
                    }
                }

                if is_word_boundary {
                    score -= 10.0;
                }

                score += (i as f64) * 0.1;
                last_match_idx = i as i32;
                query_idx += 1;
            }
        }

        if query_idx < query_chars.len() {
            return FuzzyMatch {
                matches: false,
                score: 0.0,
            };
        }

        if normalized_query == text_lower.as_str() {
            score -= 100.0;
        }

        FuzzyMatch {
            matches: true,
            score,
        }
    };

    let primary = match_query(&query_lower);
    if primary.matches {
        return primary;
    }

    // Try swapped alphanumeric (e.g., "gpt5.2" for "5.2-gpt")
    let swapped = swap_alphanumeric(&query_lower);
    if swapped.is_empty() {
        return primary;
    }

    let swapped_match = match_query(&swapped);
    if swapped_match.matches {
        FuzzyMatch {
            matches: true,
            score: swapped_match.score + 5.0,
        }
    } else {
        primary
    }
}

/// Swap letter/digit groups in a query string.
/// "codex52" -> "52codex", "gpt5.2" -> "5.2gpt"
fn swap_alphanumeric(query: &str) -> String {
    // Check for alpha then digits pattern
    if let Some(pos) = query.find(|c: char| c.is_ascii_digit())
        && pos > 0
        && query[..pos].chars().all(|c| c.is_ascii_alphabetic())
    {
        let alpha = &query[..pos];
        let rest = &query[pos..];
        return format!("{}{}", rest, alpha);
    }
    // Check for digits then alpha pattern
    if let Some(pos) = query.find(|c: char| c.is_ascii_alphabetic())
        && pos > 0
        && query[..pos].chars().all(|c| c.is_ascii_digit())
    {
        let digits = &query[..pos];
        let rest = &query[pos..];
        return format!("{}{}", rest, digits);
    }
    String::new()
}

/// Filter and sort items by fuzzy match quality (best matches first).
/// Supports whitespace- and slash-separated tokens: all tokens must match.
pub fn fuzzy_filter<T>(items: &[T], query: &str, get_text: impl Fn(&T) -> &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..items.len()).collect();
    }

    let tokens: Vec<&str> = query
        .trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() {
        return (0..items.len()).collect();
    }

    let mut results: Vec<(usize, f64)> = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        let text = get_text(item);
        let mut total_score = 0.0;
        let mut all_match = true;

        for token in &tokens {
            let m = fuzzy_match(token, text);
            if m.matches {
                total_score += m.score;
            } else {
                all_match = false;
                break;
            }
        }

        if all_match {
            results.push((idx, total_score));
        }
    }

    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    results.into_iter().map(|(idx, _)| idx).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_matches_all() {
        let result = fuzzy_match("", "anything");
        assert!(result.matches);
        assert_eq!(result.score, 0.0);
    }

    #[test]
    fn test_query_longer_than_text() {
        let result = fuzzy_match("longquery", "short");
        assert!(!result.matches);
    }

    #[test]
    fn test_characters_must_appear_in_order() {
        let in_order = fuzzy_match("abc", "aXbXc");
        assert!(in_order.matches);

        let out_of_order = fuzzy_match("abc", "cba");
        assert!(!out_of_order.matches);
    }

    #[test]
    fn test_case_insensitive() {
        assert!(fuzzy_match("ABC", "abc").matches);
        assert!(fuzzy_match("abc", "ABC").matches);
    }

    #[test]
    fn test_consecutive_better_than_scattered() {
        let consecutive = fuzzy_match("foo", "foobar");
        let scattered = fuzzy_match("foo", "f_o_o_bar");
        assert!(consecutive.matches);
        assert!(scattered.matches);
        assert!(consecutive.score < scattered.score);
    }

    #[test]
    fn test_word_boundary_better() {
        let at_boundary = fuzzy_match("fb", "foo-bar");
        let not_boundary = fuzzy_match("fb", "afbx");
        assert!(at_boundary.matches);
        assert!(not_boundary.matches);
        assert!(at_boundary.score < not_boundary.score);
    }

    #[test]
    fn test_swapped_alphanumeric() {
        let result = fuzzy_match("codex52", "gpt-5.2-codex");
        assert!(result.matches);
    }

    #[test]
    fn test_fuzzy_filter_empty_query_returns_all() {
        let items = vec!["apple", "banana", "cherry"];
        let result = fuzzy_filter(&items, "", |s| s);
        assert_eq!(result, vec![0, 1, 2]);
    }

    #[test]
    fn test_fuzzy_filter_filters_non_matching() {
        let items = vec!["apple", "banana", "cherry"];
        let result = fuzzy_filter(&items, "an", |s| s);
        assert!(result.contains(&1)); // banana
        assert!(!result.contains(&0)); // apple has "a" but not after "n"
        // Actually "apple" does contain "a" and "p" and "p" and "l" and "e"
        // "an": 'a' at 0, then 'n' - apple doesn't have 'n'
        assert!(!result.contains(&2)); // cherry doesn't have 'a' then 'n'
    }

    #[test]
    fn test_fuzzy_filter_sorts_by_quality() {
        let items = vec!["a_p_p", "app", "application"];
        let result = fuzzy_filter(&items, "app", |s| s);
        assert_eq!(items[result[0]], "app");
    }

    #[test]
    fn test_fuzzy_filter_slash_separated() {
        let items = vec!["gpt-5.5 openai-codex"];
        let result = fuzzy_filter(&items, "openai-codex/gpt-5.5", |s| s);
        assert_eq!(result.len(), 1);
    }
}
